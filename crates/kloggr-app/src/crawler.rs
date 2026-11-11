//! One open file: main view, search bar and filtered view (klogg:
//! `CrawlerWidget`).
//!
//! The search runs on its own thread (the matching itself on rayon) and
//! streams matching lines into a shared [`FilteredLines`], which both views
//! observe: the filtered view shows its rows, the main view its bullets.
//! Selecting a line in the filtered view shows it in the main view.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    Action, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, Task, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants, Toggle};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::resizable::{ResizableState, resizable_panel, v_resizable};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, IconName, Sizable};
use kloggr_core::search::{self, FilteredLines, Matcher, Pattern, SearchProgress, Visibility};
use kloggr_core::settings::{FileContext, FrozenView};
use kloggr_core::{Cancelled, Encoding, LogFile};

use crate::background;
use crate::follow::{self, Watch};
use crate::go_to_line::{self, GoToLine};
use crate::highlighters::Highlighters;
use crate::log_view::{self, LabelOp, LimitOp, LogView, LogViewEvent, SearchOp};
use crate::preferences::Preferences;
use crate::quick_find::{self, QuickFind, Target};
use crate::results::Results;
use crate::saved_searches::{self, SavedSearches};
use crate::shortcuts::Shortcut;
use kloggr_core::highlight::ColorLabels;
use kloggr_core::search::find::Direction;
use kloggr_core::searches::{self as compose, Mode};

actions!(
    crawler,
    [
        FocusSearch,
        VisibilityForward,
        VisibilityBackward,
        ShowMarksAndMatches,
        ShowMarks,
        ShowMatches,
        ToggleMatchCase,
        ToggleRegex,
        ToggleInverse,
        ToggleBoolean,
        ToggleAutoRefresh,
        ToggleKeepResults,
        IncreaseTopView,
        DecreaseTopView,
        ToggleFreezeMain,
        ToggleFreezeFiltered,
        ClearSearchHistory,
        EditSearchHistory,
        SaveSearchAsFilter,
        EditPredefinedFilters,
    ]
);

/// Runs a search from the history (the search box's dropdown).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = crawler, no_json)]
pub struct UseSearch {
    pub text: String,
}

/// Checks or unchecks a predefined filter: the search becomes the checked
/// filters combined (klogg: `PredefinedFiltersComboBox::filterChanged`).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = crawler, no_json)]
pub struct TogglePredefinedFilter {
    pub index: usize,
}

pub(crate) const CONTEXT: &str = "Crawler";

/// klogg's defaults (`settings/src/shortcuts.cpp`). klogg's bare `1`–`7`
/// are Alt+1…Alt+7 here, so that digits in a view always type a line
/// number (`docs/decisions.md`).
pub fn shortcuts() -> Vec<Shortcut> {
    let view = Some(log_view::CONTEXT);
    let crawler = Some(CONTEXT);
    vec![
        Shortcut::new(
            "qf.focus_search",
            "Set focus to search input",
            crawler,
            &["secondary-s", "secondary-shift-f"],
            FocusSearch,
        ),
        Shortcut::new(
            "crawler.change_visibility_type",
            "Next filtered view visibility",
            view,
            &["v"],
            VisibilityForward,
        ),
        Shortcut::new(
            "crawler.change_visibility_type_backward",
            "Previous filtered view visibility",
            view,
            &["shift-v"],
            VisibilityBackward,
        ),
        Shortcut::new(
            "crawler.change_visibility_to_marks_and_matches",
            "Show marks and matches",
            view,
            &["alt-1"],
            ShowMarksAndMatches,
        ),
        Shortcut::new(
            "crawler.change_visibility_to_marks",
            "Show marks",
            view,
            &["alt-2"],
            ShowMarks,
        ),
        Shortcut::new(
            "crawler.change_visibility_to_matches",
            "Show matches",
            view,
            &["alt-3"],
            ShowMatches,
        ),
        Shortcut::new(
            "crawler.enable_case_matching",
            "Toggle case matching",
            view,
            &["alt-4"],
            ToggleMatchCase,
        ),
        Shortcut::new(
            "crawler.enable_regex",
            "Toggle regular expressions",
            view,
            &["alt-5"],
            ToggleRegex,
        ),
        Shortcut::new(
            "crawler.enable_inverse_matching",
            "Toggle inverse matching",
            view,
            &["alt-6"],
            ToggleInverse,
        ),
        Shortcut::new(
            "crawler.enable_regex_combining",
            "Toggle boolean combining",
            view,
            &["alt-7"],
            ToggleBoolean,
        ),
        // klogg's bare 8 and 9, as Alt+8 and Alt+9 like 1–7.
        Shortcut::new(
            "crawler.enable_auto_refresh",
            "Toggle auto-refresh",
            view,
            &["alt-8"],
            ToggleAutoRefresh,
        ),
        Shortcut::new(
            "crawler.enable_keep_results",
            "Toggle keep search results",
            view,
            &["alt-9"],
            ToggleKeepResults,
        ),
        Shortcut::new(
            "crawler.increase_top_view_size",
            "Increase main view",
            view,
            &["+", "shift-="],
            IncreaseTopView,
        ),
        Shortcut::new(
            "crawler.decrease_top_view_size",
            "Decrease main view",
            view,
            &["-"],
            DecreaseTopView,
        ),
        Shortcut::new(
            "mainwindow.freeze_main",
            "Freeze columns in main view",
            crawler,
            &[],
            ToggleFreezeMain,
        ),
        Shortcut::new(
            "mainwindow.freeze_filtered",
            "Freeze columns in filtered view",
            crawler,
            &[],
            ToggleFreezeFiltered,
        ),
    ]
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

/// Search bar toggles; defaults as in klogg's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    pub match_case: bool,
    pub regex: bool,
    pub inverse: bool,
    pub boolean: bool,
    /// Search the lines appended to the file (klogg: "Auto-refresh", off by
    /// default).
    pub auto_refresh: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            match_case: true,
            regex: true,
            inverse: false,
            boolean: false,
            auto_refresh: false,
        }
    }
}

impl SearchOptions {
    fn pattern(&self, text: &str) -> Pattern {
        Pattern {
            text: text.to_owned(),
            case_sensitive: self.match_case,
            exclude: self.inverse,
            boolean: self.boolean,
            plain_text: !self.regex,
        }
    }
}

/// The line under the search bar (klogg: `searchInfoLine_`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchInfo {
    pub text: String,
    pub error: bool,
}

enum SearchEvent {
    Batch(SearchProgress, Vec<usize>),
    Done(Result<(), Cancelled>),
}

/// Emitted for the workspace (klogg: `dataStatusChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlerEvent {
    /// The file changed on disk and was reloaded: its tab shows it.
    NewData,
    /// The frozen columns of a view changed: remember them for the file.
    FrozenChanged,
    /// Indexing progress or follow mode changed: update the status line and
    /// the menus.
    Status,
}

impl gpui::EventEmitter<CrawlerEvent> for Crawler {}

pub struct Crawler {
    pub(crate) log: Option<Arc<LogFile>>,
    pub(crate) filter: Entity<FilteredLines>,
    pub(crate) main: Entity<LogView>,
    pub(crate) filtered: Entity<LogView>,
    pub(crate) qf: QuickFind,
    /// The view that last had focus: quick find works on it.
    pub(crate) target: Target,
    input: Entity<InputState>,
    pub(crate) options: SearchOptions,
    pub(crate) info: SearchInfo,
    /// The pattern of the last search started, re-run when a file is opened.
    pub(crate) last_pattern: Option<Pattern>,
    searching: bool,
    /// Line last selected in either view (klogg: `currentLineNumber_`), to
    /// keep the filtered view near it after a search or visibility change.
    pub(crate) current_line: usize,
    pub(crate) go_to: GoToLine,
    /// Follow mode, in both views (klogg: "Follow File").
    pub(crate) following: bool,
    pub(crate) watch: Watch,
    /// The file is being indexed again (View > Reload, or it was replaced).
    pub(crate) reindex: Option<follow::Reindex>,
    /// File lines searched (klogg: `searchStartLine_`, `searchEndLine_`);
    /// `usize::MAX`: to the end of the file, however long it grows.
    pub(crate) search_limits: std::ops::Range<usize>,
    /// Encoding chosen in the Encoding menu; `None`: the detected one.
    pub(crate) encoding: Option<Encoding>,
    /// Tabs of search results; `filter` and `filtered` are the current
    /// one's (klogg: Keep Results).
    pub(crate) results: Vec<Results>,
    pub(crate) current_results: usize,
    /// The next search opens a new results tab (klogg: Keep Results).
    pub(crate) keep_results: bool,
    /// Progress of a Save to file from either view.
    pub(crate) saving: Option<u8>,
    /// Sizes of the main and filtered views.
    split: Entity<ResizableState>,
    /// Height of the filtered view until the split is first laid out.
    filtered_height: f32,
    /// Lines and bytes covered by the last finished search (`None`: none
    /// finished); with auto-refresh, what comes after is searched when the
    /// file grows.
    pub(crate) searched: Option<(usize, usize)>,
    /// Lines and bytes of the file snapshot the running search covers.
    search_covers: (usize, usize),
    /// The running search only adds the lines appended to the file.
    updating: bool,
    /// Marks from the session, set once the file is loaded (klogg:
    /// `savedMarkedLines_`).
    pending_marks: Vec<u64>,
    /// Strings marked with a color label in this tab (klogg:
    /// `colorLabelsManager_`).
    pub(crate) color_labels: ColorLabels,
    cancel: Arc<AtomicBool>,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Crawler {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|_| FilteredLines::new());
        let main = cx.new(|cx| {
            let mut v = LogView::new(cx);
            v.set_filter(filter.clone(), cx);
            v
        });
        let filtered = cx.new(|cx| {
            let mut v = LogView::filtered(cx);
            v.set_filter(filter.clone(), cx);
            v
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let qf = QuickFind::new(window, cx);
        let go_to = GoToLine::new(window, cx);
        let main_focus = main.focus_handle(cx);
        let results = Results::new(filter.clone(), filtered.clone(), window, cx);
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.start_search(window, cx);
                }
            }),
            cx.subscribe_in(
                &main,
                window,
                |this, _, event: &LogViewEvent, window, cx| match event {
                    &LogViewEvent::LineSelected(line) => this.current_line = line,
                    &LogViewEvent::FollowChanged(on) => this.set_follow(on, cx),
                    LogViewEvent::ColorLabel(op, text) => this.color_label(*op, text, cx),
                    LogViewEvent::FreezeChanged(_) => cx.emit(CrawlerEvent::FrozenChanged),
                    LogViewEvent::Search(op, text) => this.search_from_view(*op, text, window, cx),
                    &LogViewEvent::SearchLimit(op) => this.change_search_limits(op, cx),
                    LogViewEvent::SaveSplitterPosition => this.save_split(cx),
                    LogViewEvent::ExitView => this.exit_view(window, cx),
                    &LogViewEvent::HoverLine(line) => {
                        this.main.update(cx, |v, cx| v.flash_overview(line, cx))
                    }
                    &LogViewEvent::Saving(p) => {
                        this.saving = p;
                        cx.emit(CrawlerEvent::Status);
                    }
                },
            ),
            // Color label styles may change in the highlighters dialog.
            cx.observe_global::<Highlighters>(|this, cx| this.show_color_labels(cx)),
            cx.observe_global::<Preferences>(|this, cx| {
                let hide = Preferences::get(cx).view.hide_ansi;
                this.set_hide_ansi(hide, cx)
            }),
            cx.on_focus_in(&main_focus, window, |this, _, _| this.target = Target::Main),
            cx.subscribe_in(
                &go_to.input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit_go_to_line(window, cx),
                    InputEvent::Change => this.go_to_line_edited(cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &qf.input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.quick_find_edited(cx),
                    InputEvent::PressEnter { shift, .. } => {
                        let direction = if *shift {
                            Direction::Backward
                        } else {
                            Direction::Forward
                        };
                        this.find_next(direction, window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        Self {
            log: None,
            filter,
            main,
            filtered,
            qf,
            target: Target::Main,
            input,
            options: SearchOptions::default(),
            info: SearchInfo::default(),
            last_pattern: None,
            searching: false,
            current_line: 0,
            go_to,
            following: false,
            watch: Watch::default(),
            reindex: None,
            encoding: None,
            results: vec![results],
            current_results: 0,
            keep_results: false,
            saving: None,
            split: cx.new(|_| ResizableState::default()),
            filtered_height: Preferences::get(cx).view.filtered_height,
            search_limits: 0..usize::MAX,
            searched: None,
            search_covers: (0, 0),
            updating: false,
            pending_marks: Vec::new(),
            color_labels: ColorLabels::new(),
            cancel: Arc::default(),
            _search: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows `log`, dropping the previous file's matches and marks. If a
    /// search was run before, it runs again on the new file.
    pub fn set_log(&mut self, log: Option<Arc<LogFile>>, cx: &mut Context<Self>) {
        self.stop_search();
        self.reindex = None;
        self.drop_kept_results();
        self.set_search_limits(0..usize::MAX, cx);
        self.filter.update(cx, |f, cx| {
            *f = FilteredLines::new();
            cx.notify();
        });
        self.log = log.clone();
        self.current_line = 0;
        self.main.update(cx, |v, cx| v.set_log(log.clone(), cx));
        self.filtered.update(cx, |v, cx| v.set_log(log, cx));
        self.info = SearchInfo::default();
        self.searched = None;
        if let Some(log) = &self.log
            && !self.pending_marks.is_empty()
        {
            let marks = std::mem::take(&mut self.pending_marks);
            let count = log.line_count();
            self.filter.update(cx, |f, cx| {
                f.toggle_marks(marks.into_iter().map(|m| m as usize), count);
                cx.notify();
            });
        }
        if let (Some(pattern), Some(_)) = (self.last_pattern.clone(), &self.log) {
            self.run(pattern, cx);
        }
        self.start_watching(cx);
        // The encoding was chosen while the file was first indexed.
        if let Some(log) = &self.log
            && log.options().encoding != self.encoding
        {
            self.set_encoding(self.encoding, cx);
        }
        cx.notify();
    }

    /// Set search start / end, Clear search limits (klogg:
    /// `setSearchLimits`). The next search covers only these lines. A start
    /// after the end (or an end before the start) drops the other limit.
    pub(crate) fn change_search_limits(&mut self, op: LimitOp, cx: &mut Context<Self>) {
        let mut limits = self.search_limits.clone();
        match op {
            LimitOp::Start(line) => {
                limits.start = line.unwrap_or(0);
                if limits.start >= limits.end {
                    limits.end = usize::MAX;
                }
            }
            LimitOp::End(line) => {
                limits.end = line.map_or(usize::MAX, |l| l + 1);
                if limits.start >= limits.end {
                    limits.start = 0;
                }
            }
            LimitOp::Clear => limits = 0..usize::MAX,
        }
        self.set_search_limits(limits, cx);
    }

    pub(crate) fn set_search_limits(
        &mut self,
        limits: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.search_limits = limits;
        for view in [&self.main, &self.filtered] {
            let limits = self.search_limits.clone();
            view.update(cx, |v, cx| v.set_search_limits(limits, cx));
        }
        cx.notify();
    }

    /// Options > Hide ANSI colour sequences (klogg: `setPrefilter`): the
    /// lines are shown and searched without them; the search runs again.
    pub(crate) fn set_hide_ansi(&mut self, hide: bool, cx: &mut Context<Self>) {
        let Some(log) = self.log.clone() else { return };
        if log.options().hide_ansi == hide {
            return;
        }
        let log = Arc::new(log.with_hide_ansi(hide));
        self.log = Some(log.clone());
        for view in [&self.main, &self.filtered] {
            let log = log.clone();
            view.update(cx, |v, cx| v.update_log(log, cx));
        }
        if let Some(pattern) = self.last_pattern.clone() {
            self.run(pattern, cx);
        }
        cx.notify();
    }

    /// Height of the filtered view, in pixels.
    pub(crate) fn filtered_height(&self, cx: &gpui::App) -> f32 {
        self.split
            .read(cx)
            .sizes()
            .get(1)
            .map_or(self.filtered_height, |h| f32::from(*h))
    }

    /// `+` and `-` (klogg: `changeTopViewSize`): the main view grows or
    /// shrinks by 10 pixels per step.
    fn change_top_view_size(&mut self, steps: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&top) = self.split.read(cx).sizes().first() else {
            return;
        };
        self.split.update(cx, |s, cx| {
            s.resize_panel(0, top + px(10. * steps), window, cx)
        });
    }

    /// Space in a view (klogg: `exitView`): the other view gets the focus.
    pub(crate) fn exit_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let other = if self.filtered.focus_handle(cx).contains_focused(window, cx) {
            self.main.focus_handle(cx)
        } else {
            self.filtered.focus_handle(cx)
        };
        window.focus(&other, cx);
    }

    /// Save splitter position (klogg: `saveSplitterSizes`): new tabs get
    /// this filtered view height.
    pub(crate) fn save_split(&mut self, cx: &mut Context<Self>) {
        let mut options = Preferences::get(cx).clone();
        options.view.filtered_height = self.filtered_height(cx);
        Preferences::set(options, None, cx);
    }

    /// Encoding > … (klogg: `CrawlerWidget::setEncoding`). The file is
    /// decoded again; indexed again only if its lines are laid out
    /// differently (UTF-16), which drops the marks. The search runs again.
    pub(crate) fn set_encoding(&mut self, encoding: Option<Encoding>, cx: &mut Context<Self>) {
        self.encoding = encoding;
        cx.notify();
        let Some(log) = self.log.clone() else {
            // Applied once the file is indexed (`set_log`).
            cx.emit(CrawlerEvent::Status);
            return;
        };
        if self.reindex.is_some() {
            self.reindex(cx);
            return;
        }
        let Some(log) = log.with_encoding(encoding) else {
            self.reindex(cx);
            return;
        };
        let log = Arc::new(log);
        self.log = Some(log.clone());
        for view in [&self.main, &self.filtered] {
            let log = log.clone();
            view.update(cx, |v, cx| v.update_log(log, cx));
        }
        if let Some(pattern) = self.last_pattern.clone() {
            self.run(pattern, cx);
        }
        cx.emit(CrawlerEvent::Status);
    }

    /// The per-file state saved in the session (klogg: `doGetViewContext`).
    pub fn context(&self, cx: &gpui::App) -> FileContext {
        let o = self.options;
        FileContext {
            match_case: o.match_case,
            regex: o.regex,
            inverse: o.inverse,
            boolean: o.boolean,
            auto_refresh: o.auto_refresh,
            follow: self.following,
            encoding: self.encoding.map(|e| e.name().to_owned()),
            filtered_height: Some(self.filtered_height(cx) as u32),
            marks: self
                .filter
                .read(cx)
                .marks()
                .iter()
                .map(|m| m as u64)
                .collect(),
        }
    }

    /// Restores the per-file state of a session, before the file is loaded
    /// (klogg: `doSetViewContext`).
    pub fn set_context(&mut self, context: FileContext, cx: &mut Context<Self>) {
        self.options = SearchOptions {
            match_case: context.match_case,
            regex: context.regex,
            inverse: context.inverse,
            boolean: context.boolean,
            auto_refresh: context.auto_refresh,
        };
        self.pending_marks = context.marks;
        self.encoding = context.encoding.as_deref().and_then(Encoding::for_label);
        if let Some(height) = context.filtered_height {
            self.filtered_height = height as f32;
        }
        self.set_follow(context.follow, cx);
    }

    #[cfg(test)]
    pub fn log(&self) -> Option<&LogFile> {
        self.log.as_deref()
    }

    #[cfg(test)]
    pub fn main_view(&self) -> &Entity<LogView> {
        &self.main
    }

    #[cfg(test)]
    pub fn filtered_view(&self) -> &Entity<LogView> {
        &self.filtered
    }

    #[cfg(test)]
    pub fn filter(&self) -> &Entity<FilteredLines> {
        &self.filter
    }

    #[cfg(test)]
    pub fn info(&self) -> &SearchInfo {
        &self.info
    }

    #[cfg(test)]
    pub fn options(&self) -> SearchOptions {
        self.options
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// Progress of a running reindex (View > Reload, or a replaced file).
    pub fn reindex_percent(&self) -> Option<u8> {
        self.reindex.as_ref().map(|r| r.percent)
    }

    pub fn is_following(&self) -> bool {
        self.following
    }

    #[cfg(test)]
    pub(crate) fn set_search_text_for_test(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
    }

    #[cfg(test)]
    pub fn toggle_marks_for_test(&mut self, line: usize, cx: &mut Context<Self>) {
        self.filter
            .update(cx, |f, _| f.toggle_marks([line], usize::MAX));
    }

    #[cfg(test)]
    pub fn set_search_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
    }

    /// Searches for the text in the search box (klogg: `startNewSearch`,
    /// `replaceCurrentSearch`).
    pub fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.is_empty() {
            self.clear_search(cx);
            return;
        }
        if std::mem::take(&mut self.keep_results) && self.log.is_some() {
            self.new_results(window, cx);
        }
        self.results[self.current_results].title = format!("Find \"{text}\"");
        // klogg: `savedSearches_->addRecent`.
        SavedSearches::update(cx, |s| s.add_recent(&text));
        self.run(self.options.pattern(&text), cx);
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.stop_search();
        self.last_pattern = None;
        self.filter.update(cx, |f, cx| {
            f.clear_matches();
            cx.notify();
        });
        self.filtered.update(cx, |v, cx| v.reset(cx));
        self.set_search_highlight(None, cx);
        self.info = SearchInfo::default();
        cx.notify();
    }

    /// Highlights the search pattern's matches in both views (klogg:
    /// `setSearchPattern`). Excluding and boolean patterns highlight nothing,
    /// as in klogg.
    /// Frozen columns of the main and filtered views (klogg:
    /// `FrozenColumns::FileState`).
    pub fn frozen_columns(&self, cx: &gpui::App) -> (FrozenView, FrozenView) {
        (self.main.read(cx).freeze(), self.filtered.read(cx).freeze())
    }

    pub fn set_frozen_columns(
        &mut self,
        main: FrozenView,
        filtered: FrozenView,
        cx: &mut Context<Self>,
    ) {
        self.main.update(cx, |v, cx| v.set_freeze(main, cx));
        self.filtered.update(cx, |v, cx| v.set_freeze(filtered, cx));
    }

    /// View > Freeze columns in main / filtered view (klogg:
    /// `setMainFrozenColumns`, `setFilteredFrozenColumns`).
    fn toggle_freeze(&mut self, filtered: bool, cx: &mut Context<Self>) {
        let view = if filtered { &self.filtered } else { &self.main };
        view.update(cx, |v, cx| {
            let freeze = v.freeze();
            v.set_freeze(
                FrozenView {
                    enabled: !freeze.enabled,
                    ..freeze
                },
                cx,
            )
        });
        cx.emit(CrawlerEvent::FrozenChanged);
    }

    /// Applies a color label change from either view (klogg:
    /// `addColorLabelToSelection`, `clearColorLabels`), then shows the
    /// labels in both views (klogg: `updateColorLabels`).
    pub(crate) fn color_label(&mut self, op: LabelOp, text: &str, cx: &mut Context<Self>) {
        let labels = &mut self.color_labels;
        match op {
            LabelOp::Set(label) => labels.set(label, text),
            LabelOp::Remove => labels.remove(text),
            LabelOp::Next => {
                let cycle: Vec<bool> = Highlighters::color_label_styles(cx)
                    .iter()
                    .map(|s| s.cycle)
                    .collect();
                labels.set_next(text, &cycle)
            }
            LabelOp::ClearAll => labels.clear(),
        }
        self.show_color_labels(cx);
    }

    pub(crate) fn show_color_labels(&mut self, cx: &mut Context<Self>) {
        let styles = Highlighters::color_label_styles(cx);
        let compiled = Arc::new(self.color_labels.compile(&styles));
        for view in [&self.main, &self.filtered] {
            let (labels, compiled) = (self.color_labels.clone(), compiled.clone());
            view.update(cx, |v, cx| v.set_color_labels(labels, compiled, cx));
        }
    }

    pub(crate) fn set_search_highlight(
        &mut self,
        matcher: Option<Arc<Matcher>>,
        cx: &mut Context<Self>,
    ) {
        for view in [&self.main, &self.filtered] {
            let matcher = matcher.clone();
            view.update(cx, |v, cx| v.set_search_highlight(matcher, cx));
        }
    }

    pub(crate) fn run(&mut self, pattern: Pattern, cx: &mut Context<Self>) {
        self.run_from(pattern, None, cx);
    }

    /// A new search that replaces the matches, or with `update`, a search of
    /// lines `update..` that adds to them: the lines appended to the file
    /// (klogg: `updateSearch`).
    pub(crate) fn run_from(
        &mut self,
        pattern: Pattern,
        update: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.stop_search();
        self.updating = update.is_some();
        let from = update.unwrap_or(0);
        if !self.updating {
            self.searched = None;
            self.filter.update(cx, |f, cx| {
                f.clear_matches();
                cx.notify();
            });
            self.filtered.update(cx, |v, cx| v.reset(cx));
        }
        let matcher = match Matcher::new(&pattern) {
            Ok(m) => m,
            Err(e) => {
                self.last_pattern = None;
                self.set_search_highlight(None, cx);
                self.info = SearchInfo {
                    text: format!("Error in expression: {e}"),
                    error: true,
                };
                cx.notify();
                return;
            }
        };
        let matcher = Arc::new(matcher);
        let highlight = (!pattern.exclude && !pattern.boolean).then(|| matcher.clone());
        self.set_search_highlight(highlight, cx);
        self.last_pattern = Some(pattern);
        let Some(log) = self.log.clone() else {
            return;
        };
        self.search_covers = (log.line_count(), log.bytes().len());
        self.searching = true;
        self.info = SearchInfo {
            text: "Search in progress…".into(),
            error: false,
        };

        let (tx, rx) = async_channel::unbounded();
        let cancel = self.cancel.clone();
        let range = from.max(self.search_limits.start)..self.search_limits.end;
        background::spawn_blocking(
            "kloggr-search",
            move || {
                let result = search::search(&log, &matcher, range, &cancel, |p, found| {
                    tx.send_blocking(SearchEvent::Batch(p, found.to_vec())).ok();
                });
                tx.send_blocking(SearchEvent::Done(result)).ok();
            },
            cx,
        );
        self._search = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.on_search_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_search_event(&mut self, event: SearchEvent, cx: &mut Context<Self>) {
        match event {
            SearchEvent::Batch(progress, mut found) => {
                // An update re-checks the last line, which may have grown.
                if let Some(last) = self.filter.read(cx).matches().max() {
                    found.retain(|&l| l > last);
                }
                if !found.is_empty() {
                    self.filter.update(cx, |f, cx| {
                        f.add_matches(&found);
                        cx.notify();
                    });
                }
                self.info.text = format!(
                    "Search in progress ({} %)… {} found so far.",
                    progress.percent(),
                    matches_text(progress.matches)
                );
            }
            SearchEvent::Done(Ok(())) => {
                self.searching = false;
                self.searched = Some(self.search_covers);
                let count = self.filter.read(cx).matches().len();
                self.info.text = format!("{} found", matches_text(count));
                if !self.updating {
                    // klogg: restore the filtered selection near the current
                    // line.
                    let line = self.current_line;
                    self.filtered.update(cx, |v, cx| v.show_line(line, cx));
                }
                // The file may have grown during the search.
                self.refresh_search(cx);
            }
            SearchEvent::Done(Err(Cancelled)) => self.searching = false,
        }
        cx.notify();
    }

    /// Stops the running search, keeping the matches found so far.
    pub fn stop_search(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self._search = None;
        if self.searching {
            self.searching = false;
            self.info.text = "Search stopped".into();
        }
    }

    fn set_visibility(&mut self, visibility: Visibility, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| {
            f.set_visibility(visibility);
            cx.notify();
        });
        // klogg: `changeFilteredViewVisibility`.
        let line = self.current_line;
        self.filtered.update(cx, |v, cx| v.show_line(line, cx));
    }

    fn visibility(&self, cx: &Context<Self>) -> Visibility {
        self.filter.read(cx).visibility()
    }

    fn search_mode(&self) -> Mode {
        Mode {
            regex: self.options.regex,
            boolean: self.options.boolean,
        }
    }

    fn search_text(&self, cx: &gpui::App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Puts `text` in the search box and focuses it, ready for Enter
    /// (klogg: `setSearchPattern`; searching at once is an option there,
    /// off by default).
    fn set_search_pattern(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// klogg: `addToSearch`, `excludeFromSearch`, `replaceSearch`.
    pub(crate) fn search_from_view(
        &mut self,
        op: SearchOp,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = self.search_mode();
        let current = self.search_text(cx);
        let search = match op {
            SearchOp::Add => compose::combine(&current, &compose::escape(text, false, mode), mode),
            SearchOp::Replace => compose::escape(text, false, mode),
            SearchOp::Exclude => {
                let (search, mode) = compose::exclude(&current, text, mode);
                self.options.boolean = mode.boolean;
                search
            }
        };
        self.set_search_pattern(search, window, cx);
    }

    fn toggle_filter(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let filters = SavedSearches::get(cx).filters;
        let mode = self.search_mode();
        let mut checked = compose::checked_filters(&self.search_text(cx), &filters, mode);
        let Some(on) = checked.get_mut(index) else {
            return;
        };
        *on = !*on;
        let search = compose::from_filters(
            filters
                .iter()
                .zip(&checked)
                .filter(|(_, on)| **on)
                .map(|(f, _)| f),
            mode,
        );
        self.set_search_pattern(search, window, cx);
    }

    fn toggle(&mut self, f: impl FnOnce(&mut SearchOptions), cx: &mut Context<Self>) {
        f(&mut self.options);
        cx.notify();
    }

    /// The search history (klogg: the search combo box's list), with
    /// klogg's "Clear search history" and "Edit search history".
    fn history_button(&self, _: &mut Context<Self>) -> impl IntoElement {
        Button::new("history")
            .icon(IconName::ChevronDown)
            .tooltip("Recent searches")
            .small()
            .ghost()
            .dropdown_menu(|menu, _, cx| {
                let history = SavedSearches::get(cx).history;
                let mut menu = menu.max_h(px(420.));
                if history.is_empty() {
                    menu = menu.label("No recent searches");
                }
                for text in history {
                    let label: String = text.chars().take(80).collect();
                    menu = menu.menu(label, Box::new(UseSearch { text }));
                }
                menu.separator()
                    .menu("Clear search history", Box::new(ClearSearchHistory))
                    .menu("Edit search history…", Box::new(EditSearchHistory))
            })
    }

    /// klogg's predefined filters drop-down: check filters to search for
    /// them combined.
    fn filters_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Button::new("predefined-filters")
            .label("Filters")
            .tooltip("Predefined filters")
            .small()
            .outline()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, cx| {
                let filters = SavedSearches::get(cx).filters;
                let checked = this
                    .upgrade()
                    .map(|c| {
                        let c = c.read(cx);
                        compose::checked_filters(&c.search_text(cx), &filters, c.search_mode())
                    })
                    .unwrap_or_default();
                let mut menu = menu.max_h(px(420.));
                if filters.is_empty() {
                    menu = menu.label("No predefined filters");
                }
                for (index, f) in filters.iter().enumerate() {
                    let on = checked.get(index).copied().unwrap_or(false);
                    menu = menu.menu_with_check(
                        f.name.clone(),
                        on,
                        Box::new(TogglePredefinedFilter { index }),
                    );
                }
                menu.separator()
                    .menu("Save search as filter…", Box::new(SaveSearchAsFilter))
                    .menu("Edit predefined filters…", Box::new(EditPredefinedFilters))
            })
    }

    fn render_search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let o = self.options;
        let visibility = self.visibility(cx);
        let filtered_focus = self.filtered.focus_handle(cx);
        let option = |id: &'static str, label: &'static str, tip: &'static str, on: bool| {
            Toggle::new(id)
                .label(label)
                .tooltip(tip)
                .checked(on)
                .small()
        };
        let action_button = if self.searching {
            Button::new("stop")
                .label("Stop")
                .small()
                .danger()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stop_search();
                    cx.notify();
                }))
        } else {
            Button::new("search")
                .label("Search")
                .small()
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.start_search(window, cx)))
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .child(
                Button::new("visibility")
                    .label(visibility.label())
                    .tooltip("Lines shown below (v / shift-v)")
                    .small()
                    .outline()
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        // Actions go to the filtered view, the pane they change;
                        // the menu shows their 1/2/3 shortcuts.
                        let item = |menu: PopupMenu, v: Visibility, action: Box<dyn Action>| {
                            menu.menu_with_check(v.label(), v == visibility, action)
                        };
                        let menu = menu.action_context(filtered_focus.clone());
                        let menu = item(
                            menu,
                            Visibility::MarksAndMatches,
                            Box::new(ShowMarksAndMatches),
                        );
                        let menu = item(menu, Visibility::Marks, Box::new(ShowMarks));
                        item(menu, Visibility::Matches, Box::new(ShowMatches))
                    }),
            )
            .child(div().flex_1().child(Input::new(&self.input).small()))
            .child(self.history_button(cx))
            .child(self.filters_button(cx))
            .child(
                option("match-case", "Aa", "Match case (Alt+4)", o.match_case).on_click(
                    cx.listener(|this, on: &bool, _, cx| this.toggle(|o| o.match_case = *on, cx)),
                ),
            )
            .child(
                option("regex", ".*", "Use regex (Alt+5)", o.regex).on_click(
                    cx.listener(|this, on: &bool, _, cx| this.toggle(|o| o.regex = *on, cx)),
                ),
            )
            .child(
                option("inverse", "!", "Inverse match (Alt+6)", o.inverse).on_click(
                    cx.listener(|this, on: &bool, _, cx| this.toggle(|o| o.inverse = *on, cx)),
                ),
            )
            .child(
                option(
                    "boolean",
                    "&|",
                    "Combine quoted patterns with and, or, not (Alt+7)",
                    o.boolean,
                )
                .on_click(
                    cx.listener(|this, on: &bool, _, cx| this.toggle(|o| o.boolean = *on, cx)),
                ),
            )
            .child(
                option(
                    "auto-refresh",
                    "⟳",
                    "Auto-refresh: search new lines (Alt+8)",
                    o.auto_refresh,
                )
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.toggle(|o| o.auto_refresh = *on, cx);
                    this.refresh_search(cx);
                })),
            )
            .child(
                option(
                    "keep-results",
                    "Keep",
                    "Keep these results and show the next search in a new tab (Alt+9)",
                    self.keep_results,
                )
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.keep_results = *on;
                    cx.notify();
                })),
            )
            .child(action_button)
            .child(
                Toggle::new("follow")
                    .label("Follow")
                    .tooltip("Follow the end of the file (f, F10)")
                    .checked(self.following)
                    .small()
                    .on_click(cx.listener(|this, on: &bool, _, cx| this.set_follow(*on, cx))),
            )
    }
}

impl Crawler {
    /// The tabs of kept results; hidden while there is one (klogg:
    /// `setTabBarAutoHide`).
    fn render_results_tabs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.results.len() < 2 {
            return None;
        }
        let tabs = self.results.iter().enumerate().map(|(index, r)| {
            Tab::new()
                .label(r.title.clone())
                .suffix(
                    Button::new(("results-close", index))
                        .label("×")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.close_results(index, cx))),
                )
                .on_mouse_down(
                    gpui::MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| this.close_results(index, cx)),
                )
        });
        Some(
            TabBar::new("results")
                .small()
                .selected_index(self.current_results)
                .children(tabs)
                .on_click(cx.listener(|this, index: &usize, _, cx| this.show_results(*index, cx))),
        )
    }
}

impl Drop for Crawler {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn matches_text(n: usize) -> String {
    if n == 1 {
        "1 match".into()
    } else {
        format!("{n} matches")
    }
}

impl Render for Crawler {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, danger, muted_fg) = (theme.border, theme.danger, theme.muted_foreground);
        let info = (!self.info.text.is_empty()).then(|| {
            div()
                .px_2()
                .text_sm()
                .text_color(if self.info.error { danger } else { muted_fg })
                .child(self.info.text.clone())
        });
        let bottom = div()
            .size_full()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(border)
            .child(self.render_search_bar(cx))
            .children(info)
            .children(self.render_results_tabs(cx))
            .child(div().flex_1().min_h_0().child(self.filtered.clone()));
        div()
            .id("crawler")
            .key_context(CONTEXT)
            .size_full()
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let focus = this.input.focus_handle(cx);
                window.focus(&focus, cx);
            }))
            // Edit > Copy and Select All while an input has focus: the view
            // that last had it (klogg: `MainWindow::copy`, `selectAll`).
            .on_action(cx.listener(|this, _: &log_view::Copy, window, cx| {
                let view = this.target_view(this.target).clone();
                view.update(cx, |v, cx| v.copy_selection(false, window, cx));
            }))
            .on_action(cx.listener(|this, _: &log_view::SelectAll, _, cx| {
                let view = this.target_view(this.target).clone();
                view.update(cx, |v, cx| v.select_all(cx));
            }))
            .on_action(cx.listener(|this, _: &VisibilityForward, _, cx| {
                let v = this.visibility(cx).next();
                this.set_visibility(v, cx)
            }))
            .on_action(cx.listener(|this, _: &VisibilityBackward, _, cx| {
                let v = this.visibility(cx).prev();
                this.set_visibility(v, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowMarksAndMatches, _, cx| {
                this.set_visibility(Visibility::MarksAndMatches, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowMarks, _, cx| {
                    this.set_visibility(Visibility::Marks, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ShowMatches, _, cx| {
                this.set_visibility(Visibility::Matches, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleMatchCase, _, cx| {
                this.toggle(|o| o.match_case = !o.match_case, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleRegex, _, cx| this.toggle(|o| o.regex = !o.regex, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleInverse, _, cx| {
                this.toggle(|o| o.inverse = !o.inverse, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBoolean, _, cx| {
                this.toggle(|o| o.boolean = !o.boolean, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleAutoRefresh, _, cx| {
                this.toggle(|o| o.auto_refresh = !o.auto_refresh, cx);
                this.refresh_search(cx);
            }))
            .on_action(cx.listener(|this, _: &IncreaseTopView, window, cx| {
                this.change_top_view_size(1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseTopView, window, cx| {
                this.change_top_view_size(-1., window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleKeepResults, _, cx| {
                this.keep_results = !this.keep_results;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &quick_find::Open, window, cx| {
                this.open_quick_find(Direction::Forward, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &quick_find::OpenForward, window, cx| {
                    this.open_quick_find(Direction::Forward, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &quick_find::OpenBackward, window, cx| {
                    this.open_quick_find(Direction::Backward, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &quick_find::FindNext, window, cx| {
                this.find_next(Direction::Forward, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &quick_find::FindPrevious, window, cx| {
                    this.find_next(Direction::Backward, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &quick_find::FindSelectionNext, window, cx| {
                    this.find_selection(Direction::Forward, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &quick_find::FindSelectionPrevious, window, cx| {
                    this.find_selection(Direction::Backward, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &quick_find::Close, window, cx| {
                this.close_quick_find(window, cx)
            }))
            .on_action(cx.listener(|this, _: &follow::ToggleFollow, _, cx| {
                this.set_follow(!this.following, cx)
            }))
            .on_action(cx.listener(|this, a: &UseSearch, window, cx| {
                this.set_search_pattern(a.text.clone(), window, cx);
                this.start_search(window, cx);
            }))
            .on_action(cx.listener(|this, a: &TogglePredefinedFilter, window, cx| {
                this.toggle_filter(a.index, window, cx)
            }))
            .on_action(cx.listener(|_, _: &ClearSearchHistory, _, cx| {
                SavedSearches::update(cx, |s| s.history.clear())
            }))
            .on_action(cx.listener(|_, _: &EditSearchHistory, window, cx| {
                saved_searches::edit_history(window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveSearchAsFilter, window, cx| {
                let text = this.search_text(cx);
                if !text.is_empty() {
                    saved_searches::save_as_filter(text, this.options.regex, window, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &EditPredefinedFilters, window, cx| {
                saved_searches::edit_filters(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleFreezeMain, _, cx| this.toggle_freeze(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleFreezeFiltered, _, cx| this.toggle_freeze(true, cx)),
            )
            .on_action(cx.listener(|this, _: &go_to_line::Open, window, cx| {
                this.open_go_to_line(window, cx)
            }))
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("crawler-split")
                        .with_state(&self.split)
                        .child(resizable_panel().child(self.main.clone()))
                        .child(
                            resizable_panel()
                                .size(px(self.filtered_height))
                                .child(bottom),
                        ),
                ),
            )
            .children(self.qf.open.then(|| {
                div()
                    .id("quick-find")
                    // Esc in the bar's input closes the bar (klogg: Cancel).
                    .capture_action(cx.listener(
                        |this, _: &gpui_component::input::Escape, window, cx| {
                            this.close_quick_find(window, cx);
                            cx.stop_propagation();
                        },
                    ))
                    .child(self.render_quick_find(cx))
            }))
            .children(self.go_to.open.then(|| {
                div()
                    .id("go-to-line")
                    .capture_action(cx.listener(
                        |this, _: &gpui_component::input::Escape, window, cx| {
                            this.close_go_to_line(window, cx);
                            cx.stop_propagation();
                        },
                    ))
                    .child(self.render_go_to_line(cx))
            }))
    }
}

impl Focusable for Crawler {
    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.main.focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use kloggr_core::Selection;
    use std::path::PathBuf;

    struct TempLog(PathBuf);

    impl TempLog {
        /// Line `i` is "line i", with " ERROR" when `i % 3 == 0` and
        /// " error" when `i % 3 == 1`.
        fn new(name: &str, lines: usize) -> Self {
            let path =
                std::env::temp_dir().join(format!("kloggr-crawler-{}-{name}", std::process::id()));
            let text: String = (0..lines)
                .map(|i| match i % 3 {
                    0 => format!("line {i} ERROR\n"),
                    1 => format!("line {i} error\n"),
                    _ => format!("line {i} a.b\n"),
                })
                .collect();
            std::fs::write(&path, text).unwrap();
            Self(path)
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn crawler<'a>(
        cx: &'a mut TestAppContext,
        log: &TempLog,
    ) -> (Entity<Crawler>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(log_view::key_bindings());
            cx.bind_keys(key_bindings());
            cx.bind_keys(go_to_line::key_bindings());
            cx.bind_keys(follow::key_bindings());
        });
        let file = Arc::new(LogFile::open(&log.0).unwrap());
        let (crawler, cx) = cx.add_window_view(Crawler::new);
        crawler.update_in(cx, |c, window, cx| {
            c.set_log(Some(file), cx);
            let focus = c.main.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.run_until_parked();
        (crawler, cx)
    }

    fn search(
        crawler: &Entity<Crawler>,
        cx: &mut VisualTestContext,
        text: &str,
        options: SearchOptions,
    ) -> Vec<usize> {
        crawler.update_in(cx, |c, window, cx| {
            c.options = options;
            c.set_search_text(text, window, cx);
            c.start_search(window, cx);
        });
        cx.run_until_parked();
        matches(crawler, cx)
    }

    fn matches(crawler: &Entity<Crawler>, cx: &mut VisualTestContext) -> Vec<usize> {
        crawler.read_with(cx, |c, cx| c.filter().read(cx).matches().iter().collect())
    }

    fn info(crawler: &Entity<Crawler>, cx: &mut VisualTestContext) -> SearchInfo {
        crawler.read_with(cx, |c, _| c.info().clone())
    }

    fn selection(view: &Entity<LogView>, cx: &mut VisualTestContext) -> Selection {
        view.read_with(cx, |v, _| v.selection().clone())
    }

    #[gpui::test]
    fn search_fills_filtered_view(cx: &mut TestAppContext) {
        let log = TempLog::new("fill", 30);
        let (crawler, cx) = crawler(cx, &log);
        let found = search(&crawler, cx, "ERROR", SearchOptions::default());
        assert_eq!(found, (0..30).step_by(3).collect::<Vec<_>>());
        assert_eq!(
            info(&crawler, cx),
            SearchInfo {
                text: "10 matches found".into(),
                error: false
            }
        );
        assert!(!crawler.read_with(cx, |c, _| c.is_searching()));
        let rows = crawler.read_with(cx, |c, cx| c.filter().read(cx).row_count());
        assert_eq!(rows, 10);
    }

    #[gpui::test]
    fn search_options(cx: &mut TestAppContext) {
        let log = TempLog::new("options", 9);
        let (crawler, cx) = crawler(cx, &log);
        let o = SearchOptions::default();
        let ignore_case = SearchOptions {
            match_case: false,
            ..o
        };
        assert_eq!(
            search(&crawler, cx, "error", ignore_case),
            [0, 1, 3, 4, 6, 7]
        );
        // Regex: `.` matches any character; plain text: only a dot.
        assert_eq!(search(&crawler, cx, "4.e", o), [4]);
        let plain = SearchOptions { regex: false, ..o };
        assert_eq!(search(&crawler, cx, "4.e", plain), Vec::<usize>::new());
        assert_eq!(search(&crawler, cx, "a.b", plain), [2, 5, 8]);
        let inverse = SearchOptions { inverse: true, ..o };
        assert_eq!(search(&crawler, cx, "error|ERROR", inverse), [2, 5, 8]);
        let boolean = SearchOptions {
            boolean: true,
            match_case: false,
            ..o
        };
        assert_eq!(
            search(&crawler, cx, r#""error" and not("line [0-3] ")"#, boolean),
            [4, 6, 7]
        );
        assert_eq!(info(&crawler, cx).text, "3 matches found");
        assert_eq!(search(&crawler, cx, "a.b", plain).len(), 3);
        assert_eq!(search(&crawler, cx, "line 8 ", o), [8]);
        assert_eq!(info(&crawler, cx).text, "1 match found");
    }

    #[gpui::test]
    fn invalid_pattern_and_empty_search(cx: &mut TestAppContext) {
        let log = TempLog::new("invalid", 9);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        assert!(search(&crawler, cx, "a(b", SearchOptions::default()).is_empty());
        let info = info(&crawler, cx);
        assert!(info.error, "{info:?}");
        assert!(info.text.starts_with("Error in expression: "), "{info:?}");

        search(&crawler, cx, "ERROR", SearchOptions::default());
        assert!(search(&crawler, cx, "", SearchOptions::default()).is_empty());
        assert_eq!(super::tests::info(&crawler, cx), SearchInfo::default());
    }

    #[gpui::test]
    fn filtered_selection_shows_line_in_main_view(cx: &mut TestAppContext) {
        let log = TempLog::new("sync", 300);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        let (main, filtered) = crawler.read_with(cx, |c, _| {
            (c.main_view().clone(), c.filtered_view().clone())
        });
        cx.update(|window, cx| {
            let focus = filtered.focus_handle(cx);
            window.focus(&focus, cx);
        });
        // After the search, the row of the current line (0) is selected.
        assert_eq!(selection(&filtered, cx), Selection::Line(0));
        cx.simulate_keystrokes("down down down");
        assert_eq!(selection(&filtered, cx), Selection::Line(3));
        assert_eq!(selection(&main, cx), Selection::Line(9));
        cx.simulate_keystrokes("ctrl-end");
        assert_eq!(selection(&main, cx), Selection::Line(297));
        let vp = main.read_with(cx, |v, _| v.viewport());
        assert!(vp.visible_lines().contains(&297), "{vp:?}");

        // Copy from the filtered view copies the file's lines.
        cx.simulate_keystrokes("shift-up secondary-shift-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("295: line 294 ERROR\n298: line 297 ERROR")
        );

        // A new search keeps the filtered view near the line last selected.
        search(&crawler, cx, "line \\d+ [eE]", SearchOptions::default());
        let row = filtered.read_with(cx, |v, _| v.selection().clone());
        let line = crawler.read_with(cx, |c, cx| {
            c.filter()
                .read(cx)
                .line_at(row.lines().map_or(0, |r| *r.start()))
        });
        assert_eq!(line, Some(294));
    }

    #[gpui::test]
    fn marks(cx: &mut TestAppContext) {
        let log = TempLog::new("marks", 30);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        let main = crawler.read_with(cx, |c, _| c.main_view().clone());
        let marks = |cx: &mut VisualTestContext| -> Vec<usize> {
            crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().iter().collect())
        };
        let rows = |cx: &mut VisualTestContext| -> Vec<usize> {
            crawler.read_with(cx, |c, cx| c.filter().read(cx).rows().iter().collect())
        };

        // Select lines 1..=2 in the main view and mark them.
        cx.simulate_keystrokes("down down shift-down m");
        assert_eq!(marks(cx), [1, 2]);
        assert_eq!(rows(cx)[..4], [0, 1, 2, 3]);
        cx.simulate_keystrokes("ctrl-end m");
        assert_eq!(marks(cx), [1, 2, 29]);

        // Visibility: Alt+2 = marks, Alt+3 = matches, Alt+1 = both; v cycles.
        cx.simulate_keystrokes("alt-2");
        assert_eq!(rows(cx), [1, 2, 29]);
        cx.simulate_keystrokes("alt-3");
        assert_eq!(rows(cx).len(), 10);
        cx.simulate_keystrokes("v");
        assert_eq!(rows(cx).len(), 13);
        cx.simulate_keystrokes("shift-v");
        assert_eq!(rows(cx).len(), 10);

        // [ and ] jump between marks.
        cx.simulate_keystrokes("[");
        assert_eq!(selection(&main, cx), Selection::Line(2));
        cx.simulate_keystrokes("[ [");
        assert_eq!(selection(&main, cx), Selection::Line(1));
        cx.simulate_keystrokes("]");
        assert_eq!(selection(&main, cx), Selection::Line(2));

        // m on marked lines unmarks them; a new search keeps marks.
        cx.simulate_keystrokes("m");
        assert_eq!(marks(cx), [1, 29]);
        search(&crawler, cx, "a.b", SearchOptions::default());
        assert_eq!(marks(cx), [1, 29]);
    }

    #[gpui::test]
    fn option_keys_and_focus_search(cx: &mut TestAppContext) {
        let log = TempLog::new("keys", 3);
        let (crawler, cx) = crawler(cx, &log);
        cx.simulate_keystrokes("alt-4 alt-5 alt-6 alt-7");
        assert_eq!(
            crawler.read_with(cx, |c, _| c.options()),
            SearchOptions {
                match_case: false,
                regex: false,
                inverse: true,
                boolean: true,
                auto_refresh: false
            }
        );
        cx.simulate_keystrokes("secondary-s");
        let focused = crawler.update_in(cx, |c, window, cx| {
            c.input.focus_handle(cx).is_focused(window)
        });
        assert!(focused);
        // Keys typed in the search box are text, not shortcuts; enter searches.
        cx.simulate_keystrokes("4 5 6 7 1 enter");
        assert_eq!(
            crawler.read_with(cx, |c, cx| c.input.read(cx).value().to_string()),
            "45671"
        );
        cx.simulate_keystrokes("backspace backspace backspace backspace backspace l i n e enter");
        cx.run_until_parked();
        // Boolean mode is on (Alt+7), so the pattern must be quoted.
        let info = info(&crawler, cx);
        assert!(info.error && info.text.contains("quotes"), "{info:?}");
    }

    #[gpui::test]
    fn reopening_reruns_the_search(cx: &mut TestAppContext) {
        let log = TempLog::new("rerun", 9);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        crawler.update(cx, |c, cx| c.toggle_marks_for_test(4, cx));
        let other = TempLog::new("rerun2", 6);
        let file = Arc::new(LogFile::open(&other.0).unwrap());
        crawler.update(cx, |c, cx| c.set_log(Some(file), cx));
        cx.run_until_parked();
        assert_eq!(matches(&crawler, cx), [0, 3]);
        let marks = crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().len());
        assert_eq!(marks, 0);
    }

    #[gpui::test]
    fn digits_then_g_jump_to_a_line(cx: &mut TestAppContext) {
        let log = TempLog::new("digits", 300);
        let (crawler, cx) = crawler(cx, &log);
        let main = crawler.read_with(cx, |c, _| c.main_view().clone());
        // Digits are a line number, not the visibility shortcuts.
        cx.simulate_keystrokes("1 2 3 g");
        assert_eq!(selection(&main, cx), Selection::Line(122));
        assert_eq!(
            crawler.read_with(cx, |c, cx| c.filter().read(cx).visibility()),
            Visibility::MarksAndMatches
        );
        // A count before j / k.
        cx.simulate_keystrokes("1 0 j");
        assert_eq!(selection(&main, cx), Selection::Line(132));
        cx.simulate_keystrokes("3 k");
        assert_eq!(selection(&main, cx), Selection::Line(129));
        cx.simulate_keystrokes("j");
        assert_eq!(selection(&main, cx), Selection::Line(130));
        // g alone: the first line; past the end: the last.
        cx.simulate_keystrokes("g");
        assert_eq!(selection(&main, cx), Selection::Line(0));
        cx.simulate_keystrokes("9 9 9 9 g");
        assert_eq!(selection(&main, cx), Selection::Line(299));
    }

    #[gpui::test]
    fn digits_in_the_filtered_view_are_file_lines(cx: &mut TestAppContext) {
        let log = TempLog::new("digits-filtered", 30);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        let (main, filtered) = crawler.read_with(cx, |c, _| {
            (c.main_view().clone(), c.filtered_view().clone())
        });
        crawler.update_in(cx, |c, window, cx| {
            let focus = c.filtered.focus_handle(cx);
            window.focus(&focus, cx);
        });
        // Matches are lines 0, 3, 6, ...; line 8 (8g) is not one, so the
        // row of line 6 (row 2) is selected, and the main view follows.
        cx.simulate_keystrokes("8 g");
        assert_eq!(selection(&filtered, cx), Selection::Line(2));
        assert_eq!(selection(&main, cx), Selection::Line(6));
    }

    #[gpui::test]
    fn ctrl_l_jumps_in_both_views(cx: &mut TestAppContext) {
        let log = TempLog::new("ctrl-l", 30);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        let (main, filtered) = crawler.read_with(cx, |c, _| {
            (c.main_view().clone(), c.filtered_view().clone())
        });
        cx.simulate_keystrokes("secondary-l");
        assert!(crawler.read_with(cx, |c, _| c.go_to.open));
        // Not a number: an error, the box stays open.
        cx.simulate_keystrokes("x enter");
        assert!(crawler.read_with(cx, |c, _| c.go_to.open && c.go_to.error.is_some()));
        cx.simulate_keystrokes("backspace 1 3 enter");
        assert!(crawler.read_with(cx, |c, _| !c.go_to.open));
        assert_eq!(selection(&main, cx), Selection::Line(12));
        assert_eq!(selection(&filtered, cx), Selection::Line(4));
        // The focus is back in the view: keys are view commands again.
        cx.simulate_keystrokes("j");
        assert_eq!(selection(&main, cx), Selection::Line(13));
        // Esc closes without jumping.
        cx.simulate_keystrokes("secondary-l 5 escape");
        assert!(crawler.read_with(cx, |c, _| !c.go_to.open));
        assert_eq!(selection(&main, cx), Selection::Line(13));
    }

    #[gpui::test]
    fn search_matches_are_highlighted(cx: &mut TestAppContext) {
        let log = TempLog::new("highlight", 9);
        let (crawler, cx) = crawler(cx, &log);
        let highlighted = |cx: &mut VisualTestContext| {
            crawler.read_with(cx, |c, cx| {
                (
                    c.main_view().read(cx).has_search_highlight(),
                    c.filtered_view().read(cx).has_search_highlight(),
                )
            })
        };
        assert_eq!(highlighted(cx), (false, false));
        search(&crawler, cx, "ERROR", SearchOptions::default());
        assert_eq!(highlighted(cx), (true, true));
        // As in klogg: no highlight for excluding or boolean patterns.
        let inverse = SearchOptions {
            inverse: true,
            ..SearchOptions::default()
        };
        search(&crawler, cx, "ERROR", inverse);
        assert_eq!(highlighted(cx), (false, false));
        let boolean = SearchOptions {
            boolean: true,
            ..SearchOptions::default()
        };
        search(&crawler, cx, "\"ERROR\" or \"a\"", boolean);
        assert_eq!(highlighted(cx), (false, false));
        // An invalid pattern or an empty search clears it.
        search(&crawler, cx, "ERROR", SearchOptions::default());
        search(&crawler, cx, "(", SearchOptions::default());
        assert_eq!(highlighted(cx), (false, false));
        search(&crawler, cx, "ERROR", SearchOptions::default());
        search(&crawler, cx, "", SearchOptions::default());
        assert_eq!(highlighted(cx), (false, false));
    }

    fn append(log: &TempLog, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log.0)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// What the watcher does when the file changes.
    fn file_changed(crawler: &Entity<Crawler>, cx: &mut VisualTestContext) {
        let tx = crawler.read_with(cx, |c, _| c.watch.changed.clone().unwrap());
        tx.try_send(()).unwrap();
        cx.run_until_parked();
    }

    fn line_count(crawler: &Entity<Crawler>, cx: &mut VisualTestContext) -> usize {
        crawler.read_with(cx, |c, _| c.log().unwrap().line_count())
    }

    #[gpui::test]
    fn growth_keeps_the_view_and_follow_sticks_to_the_bottom(cx: &mut TestAppContext) {
        let log = TempLog::new("grow", 200);
        let (crawler, cx) = crawler(cx, &log);
        let main = crawler.read_with(cx, |c, _| c.main_view().clone());
        cx.simulate_keystrokes("down down down");
        let top = main.read_with(cx, |v, _| v.viewport().top());
        append(&log, "new 200\nnew 201\n");
        file_changed(&crawler, cx);
        assert_eq!(line_count(&crawler, cx), 202);
        assert_eq!(selection(&main, cx), Selection::Line(2));
        assert_eq!(main.read_with(cx, |v, _| v.viewport().top()), top);

        // f: follow; the view goes to the bottom and stays there.
        cx.simulate_keystrokes("f");
        assert!(crawler.read_with(cx, |c, _| c.following));
        let at_bottom = |cx: &mut VisualTestContext| {
            main.read_with(cx, |v, _| {
                let vp = v.viewport();
                vp.top() >= vp.max_top() - 1e-6 && vp.max_top() > 0.
            })
        };
        append(&log, &"more\n".repeat(100));
        file_changed(&crawler, cx);
        cx.run_until_parked();
        assert_eq!(line_count(&crawler, cx), 302);
        assert!(at_bottom(cx));
        // Scrolling up leaves follow mode.
        cx.simulate_keystrokes("pageup");
        assert!(!crawler.read_with(cx, |c, _| c.following));
        // Shift+G twice: to the bottom, then following again (klogg).
        cx.simulate_keystrokes("shift-g");
        assert!(!crawler.read_with(cx, |c, _| c.following));
        cx.simulate_keystrokes("shift-g");
        assert!(crawler.read_with(cx, |c, _| c.following));
        assert!(main.read_with(cx, |v, _| v.is_following()));
        // Ctrl+L jumps elsewhere: not following any more.
        crawler.update(cx, |c, cx| c.go_to_line(5, cx));
        assert!(!crawler.read_with(cx, |c, _| c.following));
    }

    #[gpui::test]
    fn auto_refresh_searches_new_lines(cx: &mut TestAppContext) {
        let log = TempLog::new("refresh", 6);
        let (crawler, cx) = crawler(cx, &log);
        assert_eq!(
            search(&crawler, cx, "ERROR", SearchOptions::default()),
            [0, 3]
        );
        // Off (klogg's default): new lines are not searched.
        append(&log, "x ERROR\n");
        file_changed(&crawler, cx);
        assert_eq!(matches(&crawler, cx), [0, 3]);
        // On: the lines added since the last search are.
        let on = SearchOptions {
            auto_refresh: true,
            ..SearchOptions::default()
        };
        assert_eq!(search(&crawler, cx, "ERROR", on), [0, 3, 6]);
        append(&log, "y\nz ERR");
        file_changed(&crawler, cx);
        assert_eq!(matches(&crawler, cx), [0, 3, 6]);
        // The incomplete last line is searched again once it grows.
        append(&log, "OR\n");
        file_changed(&crawler, cx);
        assert_eq!(matches(&crawler, cx), [0, 3, 6, 8]);
        assert_eq!(info(&crawler, cx).text, "4 matches found");
    }

    #[cfg_attr(
        windows,
        ignore = "Windows refuses to truncate a mapped file (docs/decisions.md)"
    )]
    #[gpui::test]
    fn search_limits(cx: &mut TestAppContext) {
        let log = TempLog::new("limits", 12);
        let (crawler, cx) = crawler(cx, &log);
        // From the selected line 4 to line 7 (1-based), both included.
        cx.simulate_keystrokes("4 g");
        cx.dispatch_action(log_view::SetSearchStart);
        cx.simulate_keystrokes("7 g");
        cx.dispatch_action(log_view::SetSearchEnd);
        assert_eq!(crawler.read_with(cx, |c, _| c.search_limits.clone()), 3..7);
        assert_eq!(
            search(&crawler, cx, "ERROR", SearchOptions::default()),
            [3, 6]
        );

        // A start after the end drops the end.
        cx.simulate_keystrokes("1 0 g");
        cx.dispatch_action(log_view::SetSearchStart);
        assert_eq!(search(&crawler, cx, "ERROR", SearchOptions::default()), [9]);

        cx.dispatch_action(log_view::ClearSearchLimits);
        assert_eq!(
            search(&crawler, cx, "ERROR", SearchOptions::default()),
            [0, 3, 6, 9]
        );
    }

    #[gpui::test]
    fn keep_results_opens_a_tab_per_search(cx: &mut TestAppContext) {
        let log = TempLog::new("keep", 9);
        let (crawler, cx) = crawler(cx, &log);
        let tabs = |cx: &mut VisualTestContext| {
            crawler.read_with(cx, |c, _| {
                let titles: Vec<String> = c.results.iter().map(|r| r.title.clone()).collect();
                (titles, c.current_results)
            })
        };
        assert_eq!(
            search(&crawler, cx, "ERROR", SearchOptions::default()),
            [0, 3, 6]
        );
        crawler.update(cx, |c, cx| c.toggle_marks_for_test(1, cx));

        // Alt+9, then a search: a new tab, with its own matches and marks.
        cx.simulate_keystrokes("alt-9");
        assert!(crawler.read_with(cx, |c, _| c.keep_results));
        assert_eq!(
            search(&crawler, cx, "a.b", SearchOptions::default()),
            [2, 5, 8]
        );
        assert_eq!(
            tabs(cx),
            (vec!["Find \"ERROR\"".into(), "Find \"a.b\"".into()], 1)
        );
        assert!(crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().is_empty()));
        // Keep Results applies to one search.
        assert!(crawler.read_with(cx, |c, _| !c.keep_results));
        assert_eq!(
            search(&crawler, cx, "line 1", SearchOptions::default()),
            [1]
        );
        assert_eq!(tabs(cx).0.len(), 2);

        // Back to the first tab: its results, marks and pattern.
        crawler.update(cx, |c, cx| c.show_results(0, cx));
        assert_eq!(matches(&crawler, cx), [0, 3, 6]);
        let (marks, pattern) = crawler.read_with(cx, |c, cx| {
            let marks: Vec<usize> = c.filter().read(cx).marks().iter().collect();
            (marks, c.last_pattern.as_ref().map(|p| p.text.clone()))
        });
        assert_eq!((marks, pattern.as_deref()), (vec![1], Some("ERROR")));

        // Closing the current tab shows the next one; the last one stays.
        crawler.update(cx, |c, cx| c.close_results(0, cx));
        assert_eq!(tabs(cx), (vec!["Find \"line 1\"".into()], 0));
        assert_eq!(matches(&crawler, cx), [1]);
        crawler.update(cx, |c, cx| c.close_results(0, cx));
        assert_eq!(tabs(cx).0.len(), 1);

        // Alt+8 toggles auto-refresh.
        cx.simulate_keystrokes("alt-8");
        assert!(crawler.read_with(cx, |c, _| c.options().auto_refresh));
    }

    #[gpui::test]
    fn split_between_the_views(cx: &mut TestAppContext) {
        let log = TempLog::new("split", 9);
        let (crawler, cx) = crawler(cx, &log);
        let sizes = |cx: &mut VisualTestContext| {
            crawler.read_with(cx, |c, cx| {
                c.split
                    .read(cx)
                    .sizes()
                    .iter()
                    .map(|s| f32::from(*s))
                    .collect::<Vec<_>>()
            })
        };
        let before = sizes(cx);
        assert_eq!(before.len(), 2);
        cx.simulate_keystrokes("- - +");
        cx.run_until_parked();
        let after = sizes(cx);
        assert!(
            (after[0] - (before[0] - 10.)).abs() < 0.5,
            "{before:?} {after:?}"
        );

        // Save splitter position: the default for new tabs.
        cx.dispatch_action(log_view::SaveSplitterPosition);
        let saved = cx.update(|_, cx| Preferences::get(cx).view.filtered_height);
        assert!((saved - after[1]).abs() < 0.5);
        // Remembered per file in the session.
        let height = crawler.read_with(cx, |c, cx| c.context(cx).filtered_height);
        assert_eq!(height, Some(after[1] as u32));
    }

    #[gpui::test]
    fn selection_start_end_and_exit_view(cx: &mut TestAppContext) {
        let log = TempLog::new("selstart", 9);
        let (crawler, cx) = crawler(cx, &log);
        let main = crawler.read_with(cx, |c, _| c.main.clone());
        // Set selection start on line 2, Set selection end on line 6.
        cx.simulate_keystrokes("2 g");
        cx.dispatch_action(log_view::SetSelectionStart);
        cx.simulate_keystrokes("6 g");
        cx.dispatch_action(log_view::SetSelectionEnd);
        assert_eq!(
            selection(&main, cx),
            Selection::Range {
                start: 1,
                end: 5,
                anchor: 1
            }
        );

        // Space moves the focus to the other view and back.
        let filtered_focused = |cx: &mut VisualTestContext| {
            let view = crawler.read_with(cx, |c, _| c.filtered.clone());
            cx.update(|window, cx| view.focus_handle(cx).is_focused(window))
        };
        cx.simulate_keystrokes("space");
        assert!(filtered_focused(cx));
        cx.simulate_keystrokes("space");
        assert!(!filtered_focused(cx));
    }

    #[gpui::test]
    fn hovering_a_result_flashes_the_overview(cx: &mut TestAppContext) {
        let log = TempLog::new("hover", 9);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        let (main, filtered) = crawler.read_with(cx, |c, _| (c.main.clone(), c.filtered.clone()));
        let flashed = |cx: &mut VisualTestContext| main.read_with(cx, |v, _| v.overview_flash());
        // The second result row is line 3.
        let (margin, text) = filtered.read_with(cx, |v, _| v.margin_and_text_points(1.));
        cx.simulate_mouse_move(margin, None, Modifiers::none());
        assert_eq!(flashed(cx), Some(3));
        cx.simulate_mouse_move(text, None, Modifiers::none());
        assert_eq!(flashed(cx), None);
        // Below the last result.
        let (below, _) = filtered.read_with(cx, |v, _| v.margin_and_text_points(5.));
        cx.simulate_mouse_move(below, None, Modifiers::none());
        assert_eq!(flashed(cx), None);
    }

    #[gpui::test]
    fn hiding_ansi_sequences(cx: &mut TestAppContext) {
        let log = TempLog::new("ansi", 1);
        std::fs::write(&log.0, "\x1b[31mERROR\x1b[0m: a\nERROR: b\n").unwrap();
        let (crawler, cx) = crawler(cx, &log);
        let regex = SearchOptions::default();
        assert_eq!(search(&crawler, cx, "^ERROR:", regex), [1]);
        // Options > Hide ANSI colour sequences: shown and searched without.
        cx.update(|window, cx| {
            let mut options = Preferences::get(cx).clone();
            options.view.hide_ansi = true;
            Preferences::set(options, Some(window), cx);
        });
        cx.run_until_parked();
        assert_eq!(matches(&crawler, cx), [0, 1]);
        let text = crawler.read_with(cx, |c, _| {
            c.log().unwrap().line_text(0).unwrap().into_owned()
        });
        assert_eq!(text, "ERROR: a");
    }

    #[gpui::test]
    fn truncation_drops_marks_and_results(cx: &mut TestAppContext) {
        let log = TempLog::new("truncate", 9);
        let (crawler, cx) = crawler(cx, &log);
        search(&crawler, cx, "ERROR", SearchOptions::default());
        crawler.update(cx, |c, cx| c.toggle_marks_for_test(4, cx));
        std::fs::write(&log.0, "a ERROR\nb\n").unwrap();
        file_changed(&crawler, cx);
        assert_eq!(line_count(&crawler, cx), 2);
        let (marks, rows) = crawler.read_with(cx, |c, cx| {
            let f = c.filter().read(cx);
            (f.marks().len(), f.row_count())
        });
        assert_eq!((marks, rows), (0, 0));
        assert!(crawler.read_with(cx, |c, _| c.last_pattern.is_none()));

        // With auto-refresh, the search runs again on the new content.
        let on = SearchOptions {
            auto_refresh: true,
            ..SearchOptions::default()
        };
        search(&crawler, cx, "ERROR", on);
        std::fs::write(&log.0, "ERROR\nERROR\nx\n").unwrap();
        file_changed(&crawler, cx);
        assert_eq!(matches(&crawler, cx), [0, 1]);
    }

    #[gpui::test]
    fn color_labels(cx: &mut TestAppContext) {
        use kloggr_core::search::find::Found;
        let log = TempLog::new("labels", 9);
        let (crawler, cx) = crawler(cx, &log);
        cx.update(|_, cx| cx.bind_keys(crate::highlighters::key_bindings()));
        let (main, filtered) = crawler.read_with(cx, |c, _| (c.main.clone(), c.filtered.clone()));
        let select = |cx: &mut VisualTestContext, row: usize, cols: std::ops::Range<usize>| {
            main.update(cx, |v, cx| v.show_found(&Found { row, cols }, cx));
        };
        let label = |cx: &mut VisualTestContext, view: &Entity<LogView>, text: &str| {
            view.read_with(cx, |v, _| v.color_labels().label_of(text))
        };

        // "ERROR" in "line 0 ERROR": Ctrl+Shift+2 gives it label 2, in both
        // views; the same key again removes it.
        select(cx, 0, 7..12);
        cx.simulate_keystrokes("ctrl-shift-2");
        assert_eq!(label(cx, &main, "ERROR"), Some(1));
        assert_eq!(label(cx, &filtered, "ERROR"), Some(1));
        cx.simulate_keystrokes("ctrl-shift-5");
        assert_eq!(label(cx, &main, "ERROR"), Some(4));
        cx.simulate_keystrokes("ctrl-shift-5");
        assert_eq!(label(cx, &main, "ERROR"), None);

        // Ctrl+D: the next label of the cycle.
        cx.simulate_keystrokes("ctrl-d");
        assert_eq!(label(cx, &main, "ERROR"), Some(0));
        select(cx, 0, 0..4);
        cx.simulate_keystrokes("ctrl-d");
        assert_eq!(label(cx, &main, "line"), Some(1));

        // A single selected line labels the whole line; several lines don't.
        cx.simulate_keystrokes("down ctrl-shift-3");
        assert_eq!(label(cx, &main, "line 1 error"), Some(2));
        cx.simulate_keystrokes("shift-down ctrl-shift-4");
        assert!(crawler.read_with(cx, |c, _| c.color_labels.words()[3].is_empty()));

        // What X11 and Wayland send for Ctrl+Shift+1 and Ctrl+Shift+0 on a
        // US layout (the aliases are only bound on Linux).
        if cfg!(target_os = "linux") {
            select(cx, 0, 0..4);
            cx.simulate_keystrokes("ctrl-!");
            assert_eq!(label(cx, &main, "line"), Some(0));
            cx.simulate_keystrokes("ctrl-)");
            assert!(crawler.read_with(cx, |c, _| c.color_labels.is_empty()));
        }

        // Ctrl+Shift+0 clears every label.
        select(cx, 0, 0..4);
        cx.simulate_keystrokes("ctrl-shift-1");
        cx.simulate_keystrokes("ctrl-shift-0");
        assert!(crawler.read_with(cx, |c, _| c.color_labels.is_empty()));
        assert!(filtered.read_with(cx, |v, _| v.color_labels().is_empty()));
    }

    #[gpui::test]
    fn selection_into_search_filters_and_history(cx: &mut TestAppContext) {
        use kloggr_core::search::find::Found;
        use kloggr_core::searches::PredefinedFilter;
        let log = TempLog::new("compose", 9);
        let (crawler, cx) = crawler(cx, &log);
        cx.update(|_, cx| SavedSearches::init(None, cx));
        let main = crawler.read_with(cx, |c, _| c.main.clone());
        let select = |cx: &mut VisualTestContext, row: usize, cols: std::ops::Range<usize>| {
            main.update_in(cx, |v, window, cx| {
                v.show_found(&Found { row, cols }, cx);
                let focus = v.focus_handle(cx);
                window.focus(&focus, cx);
            });
        };
        let text = |cx: &mut VisualTestContext| crawler.read_with(cx, |c, cx| c.search_text(cx));

        // klogg: Shift+A adds, Shift+R replaces, Shift+E excludes.
        select(cx, 0, 7..12);
        cx.simulate_keystrokes("shift-a");
        assert_eq!(text(cx), "ERROR");
        select(cx, 2, 7..10);
        cx.simulate_keystrokes("shift-a");
        assert_eq!(text(cx), r"ERROR|a\.b");
        select(cx, 1, 7..12);
        cx.simulate_keystrokes("shift-r");
        assert_eq!(text(cx), "error");
        select(cx, 0, 0..4);
        cx.simulate_keystrokes("shift-e");
        assert_eq!(text(cx), r#""error" and not("line")"#);
        assert!(crawler.read_with(cx, |c, _| c.options.boolean));

        // Predefined filters: checking them builds the search.
        cx.update(|_, cx| {
            SavedSearches::update(cx, |s| {
                s.filters = vec![
                    PredefinedFilter {
                        name: "Errors".into(),
                        pattern: "ERROR".into(),
                        regex: true,
                    },
                    PredefinedFilter {
                        name: "Dots".into(),
                        pattern: "a.b".into(),
                        regex: false,
                    },
                ]
            })
        });
        crawler.update_in(cx, |c, window, cx| {
            c.options.boolean = false;
            c.set_search_pattern(String::new(), window, cx);
            c.toggle_filter(0, window, cx);
            assert_eq!(c.search_text(cx), "ERROR");
            c.toggle_filter(1, window, cx);
            assert_eq!(c.search_text(cx), r"ERROR|a\.b");
            c.toggle_filter(0, window, cx);
            assert_eq!(c.search_text(cx), r"a\.b");
        });

        // Searches go into the history; picking one runs it.
        crawler.update_in(cx, |c, window, cx| c.start_search(window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| SavedSearches::get(cx).history), [r"a\.b"]);
        cx.dispatch_action(UseSearch {
            text: "ERROR".into(),
        });
        cx.run_until_parked();
        assert_eq!(matches(&crawler, cx), [0, 3, 6]);
        assert_eq!(
            cx.update(|_, cx| SavedSearches::get(cx).history),
            ["ERROR", r"a\.b"]
        );
    }
}
