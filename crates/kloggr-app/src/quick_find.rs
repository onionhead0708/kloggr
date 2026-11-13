//! Quick find bar (klogg: `QuickFindWidget`, `QuickFindMux`, `QuickFind`).
//!
//! Finds occurrences of a pattern in the view that last had focus, the main
//! or the filtered one, starting from its selection. Typing searches
//! incrementally from where the search started; Enter, `n`, F3 or Ctrl+G go
//! to the next occurrence, with Shift to the previous one. Every occurrence
//! on screen is highlighted in both views. The find itself runs on a thread
//! (see [`kloggr_core::search::find`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Styled, Task, Window,
    actions, div,
};
use gpui_component::ActiveTheme;
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants, Toggle};
use gpui_component::input::{Input, InputState};
use kloggr_core::search::find::{self, Direction, Found, Rows};
use kloggr_core::search::{self, Matcher, Pattern};
use kloggr_core::{Cancelled, Position};

use crate::background;
use crate::crawler::{self, Crawler};
use crate::log_view::{self, LogView};
use crate::preferences::Preferences;
use crate::shortcuts::Shortcut;
use crate::{msg, tr};

actions!(
    quick_find,
    [
        Open,
        OpenForward,
        OpenBackward,
        FindNext,
        FindPrevious,
        FindSelectionNext,
        FindSelectionPrevious,
        Close,
    ]
);

/// klogg's defaults (`settings/src/shortcuts.cpp`).
pub fn shortcuts() -> Vec<Shortcut> {
    let view = Some(log_view::CONTEXT);
    let crawler = Some(crawler::CONTEXT);
    vec![
        Shortcut::new(
            "mainwindow.open_qf",
            msg!("Open quick find"),
            crawler,
            &["secondary-f"],
            Open,
        ),
        Shortcut::new(
            "logview.open_qf_forward",
            msg!("Quick find forward"),
            view,
            &["'"],
            OpenForward,
        ),
        Shortcut::new(
            "logview.open_qf_backward",
            msg!("Quick find backward"),
            view,
            &["\""],
            OpenBackward,
        ),
        // klogg: `QKeySequence::FindNext` (F3, Ctrl+G on Linux), and `n` in
        // the log views (`AbstractLogView::keyPressEvent`).
        Shortcut::new(
            "qf.find_next",
            msg!("Find next"),
            crawler,
            &["f3", "secondary-g"],
            FindNext,
        ),
        Shortcut::new(
            "qf.find_prev",
            msg!("Find previous"),
            crawler,
            &["shift-f3", "secondary-shift-g"],
            FindPrevious,
        ),
        Shortcut::new(
            "logview.qf_next",
            msg!("Find next (log view)"),
            view,
            &["n"],
            FindNext,
        ),
        Shortcut::new(
            "logview.qf_prev",
            msg!("Find previous (log view)"),
            view,
            &["shift-n"],
            FindPrevious,
        ),
        Shortcut::new(
            "logview.qf_selected_forward",
            msg!("Find selection forward"),
            view,
            &["*", "."],
            FindSelectionNext,
        ),
        Shortcut::new(
            "logview.qf_selected_backward",
            msg!("Find selection backward"),
            view,
            &["/", ","],
            FindSelectionPrevious,
        ),
    ]
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

/// Which view quick find works on (klogg: `QuickFindMux` selector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Main,
    Filtered,
}

/// An incremental search: typing restarts it from `origin`.
#[derive(Debug, Clone, Copy)]
struct Incremental {
    target: Target,
    origin: Position,
}

enum FindEvent {
    Progress(u8),
    Done(Result<Option<Found>, Cancelled>),
}

pub struct QuickFind {
    pub open: bool,
    pub input: Entity<InputState>,
    /// klogg's quick find ignores case only on request, and searches plain
    /// text unless configured otherwise.
    pub match_case: bool,
    pub regex: bool,
    pub info: String,
    pub error: bool,
    /// The pattern text the matcher was built from.
    text: String,
    direction: Direction,
    incremental: Option<Incremental>,
    matcher: Option<Arc<Matcher>>,
    /// Next/previous presses made while a find runs: done one after the
    /// other, as klogg waits for the running find.
    queued: Vec<Direction>,
    running: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl QuickFind {
    pub fn new(window: &mut Window, cx: &mut Context<Crawler>) -> Self {
        Self {
            open: false,
            input: cx.new(|cx| InputState::new(window, cx).placeholder(tr!("Quick find"))),
            match_case: Preferences::get(cx).search.quickfind_match_case,
            regex: Preferences::get(cx).search.quickfind_regex,
            info: String::new(),
            error: false,
            text: String::new(),
            direction: Direction::Forward,
            incremental: None,
            matcher: None,
            queued: Vec::new(),
            running: false,
            cancel: Arc::default(),
            task: None,
        }
    }

    pub(crate) fn stop(&mut self) {
        self.queued.clear();
        self.running = false;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self.task = None;
    }
}

impl Drop for QuickFind {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Crawler {
    pub(crate) fn target_view(&self, target: Target) -> &Entity<LogView> {
        match target {
            Target::Main => &self.main,
            Target::Filtered => &self.filtered,
        }
    }

    /// Quick find works on the log view that has (or last had) the focus.
    pub(crate) fn update_target(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.filtered.focus_handle(cx).contains_focused(window, cx) {
            self.target = Target::Filtered;
        } else if self.main.focus_handle(cx).contains_focused(window, cx) {
            self.target = Target::Main;
        }
    }

    /// Shows the bar and focuses it (klogg: `displayQuickFindBar`).
    pub fn open_quick_find(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_target(window, cx);
        self.qf.open = true;
        self.qf.direction = direction;
        self.qf.incremental = None;
        let focus = self.qf.input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Hides the bar and gives the focus back to the view. The pattern stays
    /// highlighted and `n` / `N` keep using it.
    pub fn close_quick_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.qf.open = false;
        self.qf.incremental = None;
        self.qf.stop();
        self.qf.info.clear();
        let focus = self.target_view(self.target).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn quick_find_pattern(&self, text: &str) -> Pattern {
        Pattern {
            text: text.to_owned(),
            case_sensitive: self.qf.match_case,
            exclude: false,
            boolean: false,
            plain_text: !self.qf.regex,
        }
    }

    /// Rebuilds the matcher from the bar and highlights it in both views.
    fn update_quick_find_matcher(&mut self, cx: &mut Context<Self>) {
        let text = self.qf.input.read(cx).value().to_string();
        self.qf.text = text.clone();
        self.qf.error = false;
        self.qf.info.clear();
        self.qf.matcher = if text.is_empty() {
            None
        } else {
            match Matcher::new(&self.quick_find_pattern(&text)) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    self.qf.error = true;
                    self.qf.info = e.to_string();
                    None
                }
            }
        };
        let matcher = self.qf.matcher.clone();
        for view in [&self.main, &self.filtered] {
            view.update(cx, |v, cx| v.set_quick_find(matcher.clone(), cx));
        }
    }

    /// The pattern changed: search again from where typing started (klogg:
    /// `incrementallySearchForward`).
    pub(crate) fn quick_find_edited(&mut self, cx: &mut Context<Self>) {
        // The input also reports a change on Enter, with the same text.
        if *self.qf.input.read(cx).value() == *self.qf.text {
            return;
        }
        // klogg: `quickfind.incremental`; otherwise Enter searches.
        if !Preferences::get(cx).search.quickfind_incremental {
            self.update_quick_find_matcher(cx);
            cx.notify();
            return;
        }
        self.quick_find_options_changed(cx);
    }

    /// Rebuilds the pattern and searches again from where typing started.
    pub(crate) fn quick_find_options_changed(&mut self, cx: &mut Context<Self>) {
        self.update_quick_find_matcher(cx);
        if self.qf.matcher.is_none() {
            self.qf.stop();
            cx.notify();
            return;
        }
        let direction = self.qf.direction;
        let incremental = match self.qf.incremental {
            Some(incremental) => incremental,
            None => {
                let incremental = Incremental {
                    target: self.target,
                    origin: self
                        .target_view(self.target)
                        .read(cx)
                        .find_origin(direction),
                };
                self.qf.incremental = Some(incremental);
                incremental
            }
        };
        self.run_find(incremental.target, incremental.origin, direction, cx);
    }

    /// Next or previous occurrence from the selection of the active view.
    pub(crate) fn find_next(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_target(window, cx);
        if self.qf.matcher.is_none() {
            if !self.qf.open {
                self.open_quick_find(direction, window, cx);
            }
            return;
        }
        if self.qf.running {
            self.qf.queued.push(direction);
            return;
        }
        self.find_from_selection(direction, cx);
    }

    fn find_from_selection(&mut self, direction: Direction, cx: &mut Context<Self>) {
        // Confirming ends the incremental search: typing again starts from
        // the occurrence found now.
        self.qf.incremental = None;
        self.qf.direction = direction;
        let target = self.target;
        let origin = self.target_view(target).read(cx).find_origin(direction);
        self.run_find(target, origin, direction, cx);
    }

    /// Uses the selected text as the pattern (klogg: `*`, `.`, `/`, `,`).
    pub(crate) fn find_selection(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_target(window, cx);
        let Some(text) = self.target_view(self.target).read(cx).selected_portion(cx) else {
            return;
        };
        let text = if self.qf.regex {
            search::escape(&text)
        } else {
            text
        };
        self.qf
            .input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.update_quick_find_matcher(cx);
        self.find_next(direction, window, cx);
    }

    fn run_find(
        &mut self,
        target: Target,
        origin: Position,
        direction: Direction,
        cx: &mut Context<Self>,
    ) {
        self.qf.stop();
        let (Some(log), Some(matcher)) = (self.log.clone(), self.qf.matcher.clone()) else {
            return;
        };
        let rows = match target {
            Target::Main => None,
            Target::Filtered => Some(self.filter.read(cx).rows().clone()),
        };
        let (tx, rx) = async_channel::unbounded();
        let cancel = self.qf.cancel.clone();
        self.qf.running = true;
        background::spawn_blocking(
            "kloggr-quick-find",
            move || {
                let rows = rows.as_ref().map_or(Rows::All, Rows::Lines);
                let result = find::find(&log, rows, &matcher, origin, direction, &cancel, |p| {
                    tx.send_blocking(FindEvent::Progress(p)).ok();
                });
                tx.send_blocking(FindEvent::Done(result)).ok();
            },
            cx,
        );
        self.qf.task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match event {
                        FindEvent::Progress(p) => {
                            this.qf.info =
                                tr!("Searching (position {percent} %)", percent = p).to_string();
                        }
                        FindEvent::Done(Ok(Some(found))) => {
                            this.qf.running = false;
                            this.qf.info.clear();
                            this.target_view(target)
                                .clone()
                                .update(cx, |v, cx| v.show_found(&found, cx));
                            if !this.qf.queued.is_empty() {
                                let next = this.qf.queued.remove(0);
                                this.find_from_selection(next, cx);
                            }
                        }
                        FindEvent::Done(Ok(None)) => {
                            this.qf.running = false;
                            this.qf.queued.clear();
                            this.qf.info = match direction {
                                Direction::Forward => {
                                    tr!("Reached end of file, no occurrence found.")
                                }
                                Direction::Backward => {
                                    tr!("Reached beginning of file, no occurrence found.")
                                }
                            }
                            .into();
                        }
                        FindEvent::Done(Err(Cancelled)) => {}
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    pub(crate) fn render_quick_find(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted_fg, border) = (theme.danger, theme.muted_foreground, theme.border);
        let qf = &self.qf;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("qf-close")
                    .label("×")
                    .tooltip(tr!("Close (Esc)"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.close_quick_find(window, cx))),
            )
            .child(div().w_80().child(Input::new(&qf.input).small()))
            .child(
                Toggle::new("qf-match-case")
                    .label(tr!("Aa"))
                    .tooltip(tr!("Match case"))
                    .checked(qf.match_case)
                    .small()
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.qf.match_case = *on;
                        this.quick_find_options_changed(cx);
                    })),
            )
            .child(
                Toggle::new("qf-regex")
                    .label(".*")
                    .tooltip(tr!("Use regex"))
                    .checked(qf.regex)
                    .small()
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.qf.regex = *on;
                        this.quick_find_options_changed(cx);
                    })),
            )
            .child(
                Button::new("qf-prev")
                    .label(tr!("Previous"))
                    .tooltip(tr!("Shift+Enter, N, Shift+F3"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.find_next(Direction::Backward, window, cx)
                    })),
            )
            .child(
                Button::new("qf-next")
                    .label(tr!("Next"))
                    .tooltip(tr!("Enter, n, F3"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.find_next(Direction::Forward, window, cx)
                    })),
            )
            .child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(if qf.error { danger } else { muted_fg })
                    .child(qf.info.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crawler::SearchOptions;
    use gpui::{TestAppContext, VisualTestContext};
    use kloggr_core::{LogFile, Selection};
    use std::path::PathBuf;

    struct TempLog(PathBuf);

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// "line i" plus " ERROR x" on every 10th line, " error" on line 15.
    fn setup<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
    ) -> (Entity<Crawler>, TempLog, &'a mut VisualTestContext) {
        let path = std::env::temp_dir().join(format!("kloggr-qf-{}-{name}", std::process::id()));
        let text: String = (0..100)
            .map(|i| match i {
                15 => format!("line {i} error\n"),
                i if i % 10 == 0 => format!("line {i} ERROR x\n"),
                _ => format!("line {i}\n"),
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(log_view::key_bindings());
            cx.bind_keys(crawler::key_bindings());
            cx.bind_keys(key_bindings());
        });
        let file = Arc::new(LogFile::open(&path).unwrap());
        let (crawler, cx) = cx.add_window_view(Crawler::new);
        crawler.update_in(cx, |c, window, cx| {
            c.set_log(Some(file), cx);
            let focus = c.main.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.run_until_parked();
        (crawler, TempLog(path), cx)
    }

    fn main_selection(c: &Entity<Crawler>, cx: &mut VisualTestContext) -> Selection {
        c.read_with(cx, |c, cx| c.main.read(cx).selection().clone())
    }

    fn info(c: &Entity<Crawler>, cx: &mut VisualTestContext) -> String {
        c.read_with(cx, |c, _| c.qf.info.clone())
    }

    fn keys(cx: &mut VisualTestContext, keys: &str) {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn incremental_then_next_and_previous(cx: &mut TestAppContext) {
        let (c, _f, cx) = setup(cx, "incremental");
        keys(cx, "secondary-f");
        assert!(c.read_with(cx, |c, _| c.qf.open));
        // "line 0 ERROR x": ERROR at columns 7..12.
        keys(cx, "E R");
        assert_eq!(main_selection(&c, cx), Selection::portion(0, 7, 8));
        keys(cx, "R O R");
        assert_eq!(main_selection(&c, cx), Selection::portion(0, 7, 11));
        // Enter: next occurrence; Shift+Enter: previous.
        keys(cx, "enter");
        assert_eq!(main_selection(&c, cx), Selection::portion(10, 8, 12));
        keys(cx, "enter enter");
        assert_eq!(main_selection(&c, cx), Selection::portion(30, 8, 12));
        keys(cx, "shift-enter");
        assert_eq!(main_selection(&c, cx), Selection::portion(20, 8, 12));

        // Esc closes the bar, back in the view: n / N keep working.
        keys(cx, "escape");
        assert!(!c.read_with(cx, |c, _| c.qf.open));
        let focused = c.update_in(cx, |c, window, cx| {
            c.main.focus_handle(cx).is_focused(window)
        });
        assert!(focused);
        keys(cx, "n");
        assert_eq!(main_selection(&c, cx), Selection::portion(30, 8, 12));
        keys(cx, "shift-n shift-n");
        assert_eq!(main_selection(&c, cx), Selection::portion(10, 8, 12));
        keys(cx, "f3");
        assert_eq!(main_selection(&c, cx), Selection::portion(20, 8, 12));
    }

    #[gpui::test]
    fn end_of_file_and_case(cx: &mut TestAppContext) {
        let (c, _f, cx) = setup(cx, "eof");
        keys(cx, "ctrl-end secondary-f e r r o r");
        // Case-sensitive: "error" only on line 15, before the selection.
        assert_eq!(info(&c, cx), "Reached end of file, no occurrence found.");
        assert_eq!(main_selection(&c, cx), Selection::Line(99));
        keys(cx, "shift-enter");
        assert_eq!(main_selection(&c, cx), Selection::portion(15, 8, 12));
        keys(cx, "shift-enter");
        assert_eq!(
            info(&c, cx),
            "Reached beginning of file, no occurrence found."
        );

        // Ignoring case finds ERROR too.
        c.update(cx, |c, cx| {
            c.qf.match_case = false;
            c.quick_find_options_changed(cx);
        });
        cx.run_until_parked();
        // It runs again, backward as the last find, from line 15.
        assert_eq!(main_selection(&c, cx), Selection::portion(10, 8, 12));
    }

    #[gpui::test]
    fn invalid_regex(cx: &mut TestAppContext) {
        let (c, _f, cx) = setup(cx, "regex");
        c.update(cx, |c, _| c.qf.regex = true);
        keys(cx, "secondary-f ( enter");
        let (error, info) = c.read_with(cx, |c, _| (c.qf.error, c.qf.info.clone()));
        assert!(error && info.contains("unclosed group"), "{info}");
        keys(cx, "backspace l i n e space 1 \\ d");
        assert_eq!(main_selection(&c, cx), Selection::portion(10, 0, 6));
        keys(cx, "enter");
        assert_eq!(main_selection(&c, cx), Selection::portion(11, 0, 6));
    }

    #[gpui::test]
    fn find_selection(cx: &mut TestAppContext) {
        let (c, _f, cx) = setup(cx, "selection");
        // Select "ERROR" on line 20, then * finds the next one.
        c.update(cx, |c, cx| {
            c.main.update(cx, |v, cx| {
                v.show_found(
                    &Found {
                        row: 20,
                        cols: 8..13,
                    },
                    cx,
                )
            })
        });
        keys(cx, "*");
        assert_eq!(main_selection(&c, cx), Selection::portion(30, 8, 12));
        assert_eq!(
            c.read_with(cx, |c, cx| c.qf.input.read(cx).value().to_string()),
            "ERROR"
        );
        keys(cx, "/ /");
        assert_eq!(main_selection(&c, cx), Selection::portion(10, 8, 12));
    }

    #[gpui::test]
    fn finds_in_the_filtered_view(cx: &mut TestAppContext) {
        let (c, _f, cx) = setup(cx, "filtered");
        c.update_in(cx, |c, window, cx| {
            c.set_search_text_for_test("ERROR", window, cx);
            c.start_search(window, cx);
        });
        cx.run_until_parked();
        let filtered_focus = c.read_with(cx, |c, cx| c.filtered.focus_handle(cx));
        cx.update(|window, cx| window.focus(&filtered_focus, cx));
        cx.run_until_parked();

        c.update(cx, |c, _| c.qf.match_case = false);
        keys(cx, "secondary-f x");
        // The search left row 0 selected, so the find starts at row 1
        // (line 10, "x" at column 14); the main view follows.
        let filtered = c.read_with(cx, |c, cx| c.filtered.read(cx).selection().clone());
        assert_eq!(filtered, Selection::portion(1, 14, 14));
        assert_eq!(main_selection(&c, cx), Selection::Line(10));
    }

    #[test]
    fn options_unchanged_by_quick_find() {
        // Quick find has its own options; the search bar's are separate.
        assert!(SearchOptions::default().regex);
    }
}
