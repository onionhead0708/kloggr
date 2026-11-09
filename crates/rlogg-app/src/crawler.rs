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
    Action, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, Styled, Subscription, Task, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants, Toggle};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::resizable::{resizable_panel, v_resizable};
use gpui_component::{ActiveTheme, Sizable};
use rlogg_core::search::{self, FilteredLines, Matcher, Pattern, SearchProgress, Visibility};
use rlogg_core::settings::FileContext;
use rlogg_core::{Cancelled, LogFile};

use crate::background;
use crate::follow::{self, Watch};
use crate::go_to_line::{self, GoToLine};
use crate::log_view::{self, LogView, LogViewEvent};
use crate::quick_find::{self, QuickFind, Target};
use rlogg_core::search::find::Direction;

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
    ]
);

pub(crate) const CONTEXT: &str = "Crawler";

/// klogg's defaults (`settings/src/shortcuts.cpp`). Single keys only work
/// in the log views, so they can still be typed in the search box. klogg's
/// bare `1`–`7` are Alt+1…Alt+7 here, so that digits in a view always
/// type a line number (`docs/decisions.md`).
pub fn key_bindings() -> Vec<KeyBinding> {
    let view = Some(log_view::CONTEXT);
    vec![
        KeyBinding::new("secondary-s", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-f", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("v", VisibilityForward, view),
        KeyBinding::new("shift-v", VisibilityBackward, view),
        KeyBinding::new("alt-1", ShowMarksAndMatches, view),
        KeyBinding::new("alt-2", ShowMarks, view),
        KeyBinding::new("alt-3", ShowMatches, view),
        KeyBinding::new("alt-4", ToggleMatchCase, view),
        KeyBinding::new("alt-5", ToggleRegex, view),
        KeyBinding::new("alt-6", ToggleInverse, view),
        KeyBinding::new("alt-7", ToggleBoolean, view),
    ]
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
        let filtered_focus = filtered.focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.start_search(cx);
                }
            }),
            cx.subscribe(&main, |this, _, event: &LogViewEvent, cx| match *event {
                LogViewEvent::LineSelected(line) => this.current_line = line,
                LogViewEvent::FollowChanged(on) => this.set_follow(on, cx),
            }),
            cx.subscribe(
                &filtered,
                |this, _, event: &LogViewEvent, cx| match *event {
                    LogViewEvent::LineSelected(line) => {
                        // klogg: `jumpToMatchingLine`, which also stops
                        // following.
                        this.current_line = line;
                        this.set_follow(false, cx);
                        this.main.update(cx, |v, cx| v.show_line(line, cx));
                    }
                    LogViewEvent::FollowChanged(on) => this.set_follow(on, cx),
                },
            ),
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.on_focus_in(&main_focus, window, |this, _, _| this.target = Target::Main),
            cx.on_focus_in(&filtered_focus, window, |this, _, _| {
                this.target = Target::Filtered
            }),
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
            searched: None,
            search_covers: (0, 0),
            updating: false,
            pending_marks: Vec::new(),
            cancel: Arc::default(),
            _search: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows `log`, dropping the previous file's matches and marks. If a
    /// search was run before, it runs again on the new file.
    pub fn set_log(&mut self, log: Option<Arc<LogFile>>, cx: &mut Context<Self>) {
        self.stop_search();
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
        cx.notify();
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
    pub fn start_search(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.is_empty() {
            self.clear_search(cx);
            return;
        }
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
    fn set_search_highlight(&mut self, matcher: Option<Arc<Matcher>>, cx: &mut Context<Self>) {
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
        background::spawn_blocking(
            "rlogg-search",
            move || {
                let result =
                    search::search(&log, &matcher, from..usize::MAX, &cancel, |p, found| {
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

    fn toggle(&mut self, f: impl FnOnce(&mut SearchOptions), cx: &mut Context<Self>) {
        f(&mut self.options);
        cx.notify();
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
                .on_click(cx.listener(|this, _, _, cx| this.start_search(cx)))
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
                    "Auto-refresh: search new lines",
                    o.auto_refresh,
                )
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.toggle(|o| o.auto_refresh = *on, cx);
                    this.refresh_search(cx);
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
            .child(div().flex_1().min_h_0().child(self.filtered.clone()));
        div()
            .id("crawler")
            .key_context(CONTEXT)
            .size_full()
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let focus = this.input.focus_handle(cx);
                window.focus(&focus, cx);
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
            .on_action(cx.listener(|this, _: &go_to_line::Open, window, cx| {
                this.open_go_to_line(window, cx)
            }))
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("crawler-split")
                        .child(resizable_panel().child(self.main.clone()))
                        .child(resizable_panel().size(px(260.)).child(bottom)),
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
    use gpui::{TestAppContext, VisualTestContext};
    use rlogg_core::Selection;
    use std::path::PathBuf;

    struct TempLog(PathBuf);

    impl TempLog {
        /// Line `i` is "line i", with " ERROR" when `i % 3 == 0` and
        /// " error" when `i % 3 == 1`.
        fn new(name: &str, lines: usize) -> Self {
            let path =
                std::env::temp_dir().join(format!("rlogg-crawler-{}-{name}", std::process::id()));
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
            c.start_search(cx);
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
        cx.simulate_keystrokes("shift-up ctrl-shift-c");
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
        cx.simulate_keystrokes("ctrl-s");
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
        cx.simulate_keystrokes("ctrl-l");
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
        cx.simulate_keystrokes("ctrl-l 5 escape");
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
}
