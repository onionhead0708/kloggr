//! The log view: a custom GPUI element drawing the visible part of a
//! [`LogFile`] (klogg: `AbstractLogView`).
//!
//! Only visible rows are decoded and shaped, and only their visible columns
//! (see [`kloggr_core::text::window`]), so a multi-GB file or a multi-MB line
//! costs the same per frame as a small one. Scroll state lives in a
//! [`Viewport`] counted in lines and columns, not pixels.
//!
//! Interaction follows klogg: click selects a line, drag on the same line
//! selects a portion, drag across lines selects whole lines, shift+click
//! extends, double-click selects a word. Keys are bound in [`key_bindings`].
//! Digits typed before `g`, `j` or `k` give a line number or a count, as in
//! klogg (`123g`, `5j`); see `docs/decisions.md`.
//!
//! The same view serves klogg's two panes: the main view shows every line,
//! the filtered view (klogg: `FilteredView`) shows the rows of a
//! [`FilteredLines`]. Both draw klogg's bullet margin: a dot on matching
//! lines, an arrow on marked ones; clicking the margin toggles a mark.
//!
//! Lines are coloured by the active highlighter sets and the tab's color
//! labels (see [`crate::highlighters`]), under the search-match, quick-find
//! and selection highlights.
//!
//! The main view also draws klogg's overview bar between the text and the
//! scrollbar: a tick per pixel row holding matches (red) or marks (blue),
//! and the visible part of the file. Clicking or dragging on it scrolls
//! there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, Font, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollWheelEvent,
    ShapedLine, SharedString, Style, Styled, Subscription, TextAlign, TextRun, Window, actions,
    div, fill, point, px, relative, size,
};
use gpui_component::{ActiveTheme, WindowExt};

use crate::archives::human_size;
use gpui_component::menu::{ContextMenuExt, PopupMenu};
use kloggr_core::highlight::{ColorLabels, Colors, CompiledSet};
use kloggr_core::search::find::{self, Direction, Found};
use kloggr_core::search::overview::{self, Overview};
use kloggr_core::search::{FilteredLines, LineKind, Matcher};
use kloggr_core::settings::FrozenView;
use kloggr_core::text;
use kloggr_core::{DigitsBuffer, Drag, LogFile, Position, Selection, Viewport};

use crate::highlighters::{
    self, ClearColorLabels, Highlighters, NextColorLabel, RemoveColorLabel, SetColorLabel, hsla,
};
use crate::preferences::Preferences;
use crate::shortcuts::Shortcut;

actions!(
    log_view,
    [
        SelectUp,
        SelectDown,
        ExtendUp,
        ExtendDown,
        PageUp,
        PageDown,
        ScrollLeft,
        ScrollRight,
        StartOfLine,
        EndOfLine,
        RightOfScreen,
        Top,
        Bottom,
        SelectAll,
        Copy,
        CopyWithLineNumbers,
        Mark,
        NextMark,
        PrevMark,
        JumpToLineNumber,
        ToggleFreezeColumns,
        AddToSearch,
        ExcludeFromSearch,
        ReplaceSearch,
        SendToScratchpad,
        ReplaceScratchpad,
        SetSearchStart,
        SetSearchEnd,
        ClearSearchLimits,
        SaveToFile,
        SaveSelectedToFile,
        SaveSplitterPosition,
        SetSelectionStart,
        SetSelectionEnd,
        ExitView,
    ]
);

pub const CONTEXT: &str = "LogView";

/// klogg's default log view shortcuts (`settings/src/shortcuts.cpp`).
pub fn shortcuts() -> Vec<Shortcut> {
    let c = Some(CONTEXT);
    vec![
        Shortcut::new(
            "logview.selection_up",
            "Move selection up",
            c,
            &["up", "k"],
            SelectUp,
        ),
        Shortcut::new(
            "logview.selection_down",
            "Move selection down",
            c,
            &["down", "j"],
            SelectDown,
        ),
        Shortcut::new(
            "logview.select_lines_up",
            "Extend selection up",
            c,
            &["shift-up"],
            ExtendUp,
        ),
        Shortcut::new(
            "logview.select_lines_down",
            "Extend selection down",
            c,
            &["shift-down"],
            ExtendDown,
        ),
        Shortcut::new(
            "logview.scroll_up",
            "Page up",
            c,
            &["pageup", "ctrl-up"],
            PageUp,
        ),
        Shortcut::new(
            "logview.scroll_down",
            "Page down",
            c,
            &["pagedown", "ctrl-down"],
            PageDown,
        ),
        Shortcut::new(
            "logview.scroll_left",
            "Scroll left",
            c,
            &["left", "h"],
            ScrollLeft,
        ),
        Shortcut::new(
            "logview.scroll_right",
            "Scroll right",
            c,
            &["right", "l"],
            ScrollRight,
        ),
        Shortcut::new(
            "logview.jump_to_start_of_line",
            "Jump to start of line",
            c,
            &["home", "^"],
            StartOfLine,
        ),
        Shortcut::new(
            "logview.jump_to_end_of_line",
            "Jump to end of line",
            c,
            &["$"],
            EndOfLine,
        ),
        Shortcut::new(
            "logview.jump_to_right",
            "Jump to right of screen",
            c,
            &["end"],
            RightOfScreen,
        ),
        Shortcut::new("logview.jump_to_top", "Jump to top", c, &["ctrl-home"], Top),
        Shortcut::new(
            "logview.jump_to_bottom",
            "Jump to bottom",
            c,
            &["ctrl-end", "shift-g"],
            Bottom,
        ),
        Shortcut::new(
            "mainwindow.select_all",
            "Select all",
            c,
            &["secondary-a"],
            SelectAll,
        ),
        Shortcut::new(
            "mainwindow.copy_selection",
            "Copy selection",
            c,
            &["secondary-c"],
            Copy,
        ),
        Shortcut::new(
            "logview.copy_with_line_numbers",
            "Copy with line numbers",
            c,
            &["secondary-shift-c"],
            CopyWithLineNumbers,
        ),
        Shortcut::new("logview.mark", "Mark selected lines", c, &["m"], Mark),
        Shortcut::new(
            "logview.exit_view",
            "Release focus from view",
            c,
            &["space"],
            ExitView,
        ),
        Shortcut::new(
            "logview.next_mark",
            "Jump to next mark",
            c,
            &["]"],
            NextMark,
        ),
        Shortcut::new(
            "logview.prev_mark",
            "Jump to previous mark",
            c,
            &["["],
            PrevMark,
        ),
        Shortcut::new(
            "logview.add_to_search",
            "Add selection to search pattern",
            c,
            &["shift-a"],
            AddToSearch,
        ),
        Shortcut::new(
            "logview.exclude_from_search",
            "Exclude selection from search pattern",
            c,
            &["shift-e"],
            ExcludeFromSearch,
        ),
        Shortcut::new(
            "logview.replace_search",
            "Replace search pattern with selection",
            c,
            &["shift-r"],
            ReplaceSearch,
        ),
        Shortcut::new(
            "logview.send_selection_to_scratchpad",
            "Send selection to scratchpad",
            c,
            &["ctrl-z"],
            SendToScratchpad,
        ),
        Shortcut::new(
            "logview.replace_scratchpad_with_selection",
            "Replace scratchpad with selection",
            c,
            &["ctrl-shift-z"],
            ReplaceScratchpad,
        ),
        Shortcut::new(
            "logview.jump_to_line_number",
            "Jump to line number typed before",
            c,
            &["g"],
            JumpToLineNumber,
        ),
    ]
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

/// Selection sizes (raw bytes) above which copying changes. klogg has no
/// limits; see `docs/decisions.md`.
#[derive(Debug, Clone, Copy)]
struct CopyLimits {
    /// Larger copies ask first and are built on a worker thread.
    ask: u64,
    /// Larger selections aren't copied: the clipboard holds the whole text
    /// in memory, often more than once.
    max: u64,
    /// The primary selection, updated on every mouse selection.
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    primary: u64,
    /// Sending to the scratchpad.
    scratchpad: u64,
}

impl Default for CopyLimits {
    fn default() -> Self {
        Self {
            ask: 64 << 20,
            max: 1 << 30,
            primary: 8 << 20,
            scratchpad: 8 << 20,
        }
    }
}

fn too_large(bytes: u64, max: u64, what: &'static str, window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Selection too large").description(format!(
            "The selection is {}. At most {} can be {what}.",
            human_size(bytes),
            human_size(max)
        ))
    });
}

/// Emitted when the user selects a line (klogg: `newSelection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogViewEvent {
    /// The file line under the keyboard cursor, after a click or a key.
    LineSelected(usize),
    /// The user changed the frozen columns: toggled them, or dragged the
    /// separator (klogg: `frozenColumnsChanged`).
    FreezeChanged(FrozenView),
    /// The selected text goes into the search pattern.
    Search(SearchOp, String),
    /// A color label change for the selected text (klogg: `addColorLabel`,
    /// `clearColorLabels`). The text is empty for [`LabelOp::ClearAll`].
    ColorLabel(LabelOp, String),
    /// The user left the bottom while following, or asked to follow again
    /// by going to the bottom (klogg: `followModeChanged`).
    FollowChanged(bool),
    /// Set search start / end, Clear search limits (klogg:
    /// `changeSearchLimits`, `clearSearchLimits`).
    SearchLimit(LimitOp),
    /// Progress of Save to file, for the status line; `None` when done.
    Saving(Option<u8>),
    /// Save splitter position (klogg: `saveDefaultSplitterSizes`).
    SaveSplitterPosition,
    /// Space: focus the other view (klogg: `exitView`).
    ExitView,
    /// The mouse is over this file line in the filtered view's margin, or
    /// left it (klogg: `mouseHoveredOverLine`, `mouseLeftHoveringZone`).
    HoverLine(Option<usize>),
}

/// The lines Save to file writes.
enum SaveLines {
    /// File lines.
    Range(std::ops::Range<usize>),
    /// Rows of the filtered view: the lines of `rows` with ranks in the
    /// range.
    Rows(kloggr_core::search::LineSet, std::ops::Range<usize>),
}

/// A change to the lines a search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitOp {
    /// Searches start at this file line (`None`: the first).
    Start(Option<usize>),
    /// Searches end with this file line (`None`: the last).
    End(Option<usize>),
    Clear,
}

/// What the selection does to the search (klogg: `addToSearch`,
/// `excludeFromSearch`, `replaceSearch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOp {
    Add,
    Exclude,
    Replace,
}

/// What a color label action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelOp {
    Set(usize),
    Remove,
    Next,
    ClearAll,
}

const SCROLLBAR: Pixels = px(12.);
const MIN_THUMB: Pixels = px(24.);
const GUTTER_PAD: Pixels = px(8.);
/// klogg: `BulletAreaWidth`.
const BULLET_WIDTH: Pixels = px(11.);
/// Lines longer than this (in bytes) get no quick-find highlights: finding
/// them would decode the whole line on every frame.
const MAX_HIGHLIGHT_LINE: usize = 256 << 10;
/// klogg: `OverviewWidth`, `OverviewWidget::LINE_MARGIN`.
const OVERVIEW_WIDTH: Pixels = px(27.);
const OVERVIEW_MARGIN: Pixels = px(4.);
/// klogg: `FrozenSeparatorGrabDistance`.
const SEPARATOR_GRAB: Pixels = px(3.);

/// Geometry of the last frame, used to map mouse positions.
#[derive(Debug, Clone, Copy)]
struct Layout {
    text: Bounds<Pixels>,
    /// The bullet margin; a click there toggles a mark.
    bullets: Bounds<Pixels>,
    line_height: Pixels,
    char_width: Pixels,
    vtrack: Bounds<Pixels>,
    vthumb: Bounds<Pixels>,
    htrack: Bounds<Pixels>,
    hthumb: Bounds<Pixels>,
    /// The overview bar (main view only).
    overview: Option<Bounds<Pixels>>,
    /// x of the frozen columns separator, when columns are frozen.
    separator: Option<Pixels>,
}

impl Layout {
    fn over_separator(&self, at: Point<Pixels>) -> bool {
        self.separator.is_some_and(|x| {
            (at.x - x).abs() <= SEPARATOR_GRAB
                && at.y >= self.text.top()
                && at.y < self.text.bottom()
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum MouseGrab {
    Text(Drag),
    /// Dragging a scrollbar thumb; offset of the grab point in the thumb.
    VThumb(Pixels),
    HThumb(Pixels),
    /// Pressed on the overview bar: scrolls while dragging.
    Overview,
    /// Dragging the frozen columns separator.
    Separator,
}

pub struct LogView {
    focus: FocusHandle,
    log: Option<Arc<LogFile>>,
    limits: CopyLimits,
    /// Matches and marks: bullets in both views, the rows of the filtered one.
    filter: Option<Entity<FilteredLines>>,
    /// Whether rows are the filter's rows (the filtered view) or all lines.
    filtered: bool,
    _observe_filter: Option<Subscription>,
    selection: Selection,
    /// Line moved to by the keyboard (klogg: `selectionCurrentEndPos_`).
    cursor: usize,
    viewport: Viewport,
    layout: Option<Layout>,
    grab: Option<MouseGrab>,
    /// Sub-column horizontal wheel motion not applied yet.
    hscroll_carry: Pixels,
    /// Ctrl+wheel scrolled, in lines, until it makes a font size step.
    zoom_carry: f32,
    /// Quick-find pattern: its occurrences on visible rows are highlighted
    /// (klogg: `QuickFindPattern`).
    quick_find: Option<Arc<Matcher>>,
    /// Search pattern: its matches on visible rows are highlighted (klogg:
    /// `searchPattern_`, "main search highlight").
    search_highlight: Option<Arc<Matcher>>,
    /// File lines a search covers; the others are drawn dimmed (klogg:
    /// `searchStart_`, `searchEnd_`).
    search_limits: std::ops::Range<usize>,
    /// Stops a running Save to file.
    save_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Row from Set selection start, for Set selection end (klogg:
    /// `selectionStart_`).
    selection_start: Option<usize>,
    /// Filtered view: the line last reported by [`LogViewEvent::HoverLine`].
    hovered: Option<usize>,
    /// Main view: the line flashed on the overview bar, and the animation
    /// step (klogg: `OverviewWidget::highlightLine`).
    overview_flash: Option<(usize, u8)>,
    _flash: Option<gpui::Task<()>>,
    /// Text wrap was on in the last frame (klogg: `useTextWrap_`).
    wrapping: bool,
    /// With text wrap, the screen rows of the last frame: (y, row, part of
    /// the wrapped line), for mouse hits.
    wrap_rows: Vec<(Pixels, usize, usize)>,
    /// The tab's color labels (klogg: `quickHighlighters_`), and as
    /// highlighters.
    color_labels: ColorLabels,
    labels_compiled: Arc<CompiledSet>,
    /// Follow mode: the view stays at the bottom as the file grows (klogg:
    /// `followElasticHook_`).
    following: bool,
    /// Digits typed before a command (klogg: `digitsBuffer_`).
    digits: DigitsBuffer,
    /// Frozen columns (klogg: `frozenColumnsEnabled_`, `frozenColumn_`).
    freeze: FrozenView,
    /// Overview ticks, with the (filter generation, line count, height) they
    /// were computed for.
    overview: Option<((u64, usize, usize), Arc<Overview>)>,
}

impl LogView {
    /// The main view: one row per line.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            log: None,
            limits: CopyLimits::default(),
            filter: None,
            filtered: false,
            _observe_filter: None,
            selection: Selection::None,
            cursor: 0,
            viewport: Viewport::default(),
            layout: None,
            grab: None,
            hscroll_carry: px(0.),
            zoom_carry: 0.,
            quick_find: None,
            search_highlight: None,
            search_limits: 0..usize::MAX,
            wrapping: false,
            wrap_rows: Vec::new(),
            save_cancel: None,
            selection_start: None,
            hovered: None,
            overview_flash: None,
            _flash: None,
            color_labels: ColorLabels::new(),
            labels_compiled: Arc::default(),
            following: false,
            digits: DigitsBuffer::new(),
            freeze: FrozenView::default(),
            overview: None,
        }
    }

    /// The filtered view: one row per line of `filter.rows()`.
    pub fn filtered(cx: &mut Context<Self>) -> Self {
        Self {
            filtered: true,
            ..Self::new(cx)
        }
    }

    pub fn set_log(&mut self, log: Option<Arc<LogFile>>, cx: &mut Context<Self>) {
        self.log = log;
        self.reset(cx);
    }

    /// Shows a newer snapshot of the same file (it grew): keeps the
    /// selection and the scroll position.
    pub fn update_log(&mut self, log: Arc<LogFile>, cx: &mut Context<Self>) {
        self.viewport
            .grow_max_cols(log.index().max_line_len() as usize);
        self.log = Some(log);
        cx.notify();
    }

    /// Turns follow mode on or off; on, the view jumps to the bottom and
    /// stays there.
    pub fn set_following(&mut self, following: bool, cx: &mut Context<Self>) {
        self.following = following;
        if following {
            self.viewport.set_top(f64::MAX);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn is_following(&self) -> bool {
        self.following
    }

    fn at_bottom(&self) -> bool {
        self.viewport.top() >= self.viewport.max_top() - 1e-6
    }

    /// After the user moved the view: leaving the bottom ends follow mode
    /// (klogg: `disableFollow`).
    fn check_follow(&mut self, cx: &mut Context<Self>) {
        if self.following && !self.at_bottom() {
            self.following = false;
            cx.emit(LogViewEvent::FollowChanged(false));
        }
    }

    /// Clears the selection and scrolls back to the top, e.g. when the rows
    /// are replaced by a new search.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.viewport = Viewport::default();
        self.viewport.set_freeze(if self.freeze.enabled {
            self.freeze.column
        } else {
            0
        });
        if let Some(log) = &self.log {
            self.viewport
                .grow_max_cols(log.index().max_line_len() as usize);
        }
        self.selection = Selection::None;
        self.cursor = 0;
        self.grab = None;
        cx.notify();
    }

    /// Draws `filter`'s matches and marks; the filtered view also takes its
    /// rows from it. Redraws whenever it changes.
    pub fn set_filter(&mut self, filter: Entity<FilteredLines>, cx: &mut Context<Self>) {
        self._observe_filter = Some(cx.observe(&filter, |this, _, cx| {
            let rows = this.row_count(cx);
            this.selection = this.selection.cropped(rows);
            cx.notify();
        }));
        self.filter = Some(filter);
        cx.notify();
    }

    #[cfg(test)]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Window points in the margin and in the text of screen row `row`.
    #[cfg(test)]
    pub fn margin_and_text_points(&self, row: f32) -> (Point<Pixels>, Point<Pixels>) {
        let l = self.layout.expect("drawn");
        let y = l.text.top() + l.line_height * (row + 0.5);
        (
            point(l.bullets.left() + px(2.), y),
            point(l.text.left() + l.char_width * 2., y),
        )
    }

    #[cfg(test)]
    pub fn overview_flash(&self) -> Option<usize> {
        self.overview_flash.map(|(line, _)| line)
    }

    #[cfg(test)]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn row_count(&self, cx: &App) -> usize {
        match (&self.log, &self.filter) {
            (None, _) => 0,
            (Some(_), Some(filter)) if self.filtered => filter.read(cx).row_count(),
            (Some(_), _) if self.filtered => 0,
            (Some(log), _) => log.line_count(),
        }
    }

    /// The file line shown in `row`.
    fn line_of(&self, row: usize, cx: &App) -> Option<usize> {
        if self.filtered {
            self.filter.as_ref()?.read(cx).line_at(row)
        } else {
            (row < self.row_count(cx)).then_some(row)
        }
    }

    /// The row showing `line`, or the closest one before it.
    fn row_of(&self, line: usize, cx: &App) -> usize {
        match &self.filter {
            Some(filter) if self.filtered => filter.read(cx).row_of(line),
            _ => line,
        }
    }

    /// Changes the selection on user input, and tells the crawler.
    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if selection != self.selection {
            self.selection = selection;
            cx.notify();
        }
        if let Some(line) = self.line_of(self.cursor, cx) {
            cx.emit(LogViewEvent::LineSelected(line));
        }
    }

    /// Selects `row` and scrolls to it (klogg: `selectAndDisplayLine`).
    fn select_and_reveal(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(last) = self.row_count(cx).checked_sub(1) else {
            return;
        };
        let row = row.min(last);
        self.cursor = row;
        self.select(Selection::Line(row), cx);
        self.viewport.reveal_line(row);
        self.check_follow(cx);
        cx.notify();
    }

    /// Selects and reveals the row of file line `line` (or the closest row
    /// before it), without emitting [`LogViewEvent`]: for syncing the views.
    pub fn show_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(last) = self.row_count(cx).checked_sub(1) else {
            return;
        };
        let row = self.row_of(line, cx).min(last);
        self.cursor = row;
        self.selection = Selection::Line(row);
        self.viewport.reveal_line(row);
        cx.notify();
    }

    #[cfg(test)]
    pub fn color_labels(&self) -> &ColorLabels {
        &self.color_labels
    }

    #[cfg(test)]
    pub fn has_search_highlight(&self) -> bool {
        self.search_highlight.is_some()
    }

    pub fn set_search_highlight(&mut self, matcher: Option<Arc<Matcher>>, cx: &mut Context<Self>) {
        self.search_highlight = matcher;
        cx.notify();
    }

    pub fn set_search_limits(&mut self, limits: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.search_limits = limits;
        cx.notify();
    }

    /// Save to file / Save selected to file (klogg: `saveToFile`,
    /// `saveSelectedToFile`): the view's lines (in the filtered view, its
    /// rows) go to a file chosen in a dialog, written on a worker thread.
    fn save_to_file(&mut self, selected: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(log), Some(lines)) = (self.log.clone(), self.lines_to_save(selected, cx)) else {
            return;
        };
        let source = log.path().to_path_buf();
        let dir = source.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = source
            .file_stem()
            .map_or_else(|| "log".into(), |s| s.to_string_lossy().into_owned());
        let ext = source
            .extension()
            .map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
        let what = if self.filtered { "filtered" } else { "saved" };
        let name = format!("{stem}-{what}{ext}");
        let chosen = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            this.update_in(cx, |v, window, cx| {
                v.start_save(log, path, lines, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// The lines Save to file writes: the view's, or the selected ones.
    fn lines_to_save(&self, selected: bool, cx: &App) -> Option<SaveLines> {
        let rows = if selected {
            let r = self.selection.lines()?;
            *r.start()..r.end() + 1
        } else {
            0..self.row_count(cx)
        };
        Some(match &self.filter {
            Some(filter) if self.filtered => SaveLines::Rows(filter.read(cx).rows().clone(), rows),
            _ => SaveLines::Range(rows),
        })
    }

    fn start_save(
        &mut self,
        log: Arc<LogFile>,
        path: PathBuf,
        lines: SaveLines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The file is mapped: writing over it would pull it from under the
        // views.
        let same = |a: &Path, b: &Path| {
            a == b || std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
        };
        if path.exists() && same(&path, log.path()) {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Save to file")
                    .description("The open file can't be saved over itself.")
            });
            return;
        }
        self.cancel_save();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.save_cancel = Some(cancel.clone());
        enum SaveEvent {
            Progress(u8),
            Done(std::io::Result<()>),
        }
        let (tx, rx) = async_channel::unbounded();
        let target = path.clone();
        crate::background::spawn_blocking(
            "kloggr-save",
            move || {
                let progress = |p| {
                    tx.send_blocking(SaveEvent::Progress(p)).ok();
                };
                let result = match lines {
                    SaveLines::Range(r) => {
                        let count = r.len();
                        log.save_lines(&target, r, count, &cancel, progress)
                    }
                    SaveLines::Rows(set, r) => {
                        let count = r.len();
                        let lines = set.iter().skip(r.start).take(count);
                        log.save_lines(&target, lines, count, &cancel, progress)
                    }
                };
                tx.send_blocking(SaveEvent::Done(result)).ok();
            },
            cx,
        );
        cx.emit(LogViewEvent::Saving(Some(0)));
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update_in(cx, |v, window, cx| match event {
                    SaveEvent::Progress(p) => cx.emit(LogViewEvent::Saving(Some(p))),
                    SaveEvent::Done(result) => {
                        v.save_cancel = None;
                        cx.emit(LogViewEvent::Saving(None));
                        if let Err(e) = result
                            && e.kind() != std::io::ErrorKind::Interrupted
                        {
                            let message = format!("Cannot save {}: {e}", path.display());
                            window.open_alert_dialog(cx, move |alert, _, _| {
                                alert.title("Save to file").description(message.clone())
                            });
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Stops a running Save to file; the target file is left as it was.
    pub fn cancel_save(&mut self) {
        if let Some(cancel) = self.save_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The file line of the selection's first or last line, for Set search
    /// start / end (klogg: `selection_.selectedLine()`; nothing selected
    /// means the start or end of the file).
    fn selected_line(&self, last: bool, cx: &App) -> Option<usize> {
        let row = match self.selection {
            Selection::None => return None,
            Selection::Line(row) => row,
            Selection::Range { start, end, .. } => {
                if last {
                    end
                } else {
                    start
                }
            }
            Selection::Portion { line, .. } => line,
        };
        self.line_of(row, cx)
    }

    pub fn freeze(&self) -> FrozenView {
        self.freeze
    }

    /// Freezes the first `freeze.column` columns, if enabled (klogg:
    /// `setFrozenColumns`).
    pub fn set_freeze(&mut self, freeze: FrozenView, cx: &mut Context<Self>) {
        self.freeze = freeze;
        self.viewport
            .set_freeze(if freeze.enabled { freeze.column } else { 0 });
        cx.notify();
    }

    /// Context menu "Freeze columns".
    fn toggle_freeze(&mut self, cx: &mut Context<Self>) {
        let freeze = FrozenView {
            enabled: !self.freeze.enabled,
            ..self.freeze
        };
        self.set_freeze(freeze, cx);
        cx.emit(LogViewEvent::FreezeChanged(freeze));
    }

    pub fn set_color_labels(
        &mut self,
        labels: ColorLabels,
        compiled: Arc<CompiledSet>,
        cx: &mut Context<Self>,
    ) {
        self.color_labels = labels;
        self.labels_compiled = compiled;
        cx.notify();
    }

    /// The text a color label applies to (klogg: enabled for a portion or a
    /// single selected line): the selected part, or the whole line.
    pub fn label_text(&self, cx: &App) -> Option<String> {
        match (&self.selection, self.selection.lines()) {
            (Selection::Portion { .. }, _) => self.selected_text(false, cx),
            (_, Some(rows)) if rows.start() == rows.end() => self.selected_text(false, cx),
            _ => None,
        }
    }

    /// Emits a color label change for the selected text, if a label can
    /// apply to the selection.
    fn color_label(&mut self, op: LabelOp, cx: &mut Context<Self>) {
        let text = match op {
            LabelOp::ClearAll => String::new(),
            _ => match self.label_text(cx) {
                Some(text) if !text.is_empty() => text,
                _ => return,
            },
        };
        cx.emit(LogViewEvent::ColorLabel(op, text));
    }

    /// klogg: `sendSelectionToScratchpad`, `replaceScratchpadWithSelection`.
    fn send_to_scratchpad(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Its text editor holds and lays out all of it.
        let bytes = self.selection_bytes(cx);
        if bytes > self.limits.scratchpad {
            too_large(
                bytes,
                self.limits.scratchpad,
                "sent to the scratchpad",
                window,
                cx,
            );
            return;
        }
        let Some(text) = self.selected_text(false, cx).filter(|t| !t.is_empty()) else {
            return;
        };
        crate::scratchpad::show(cx, move |pad, window, cx| {
            if replace {
                pad.replace_data(&text, window, cx)
            } else {
                pad.add_data(&text, window, cx)
            }
        });
    }

    fn search_op(&mut self, op: SearchOp, cx: &mut Context<Self>) {
        if let Some(text) = self.label_text(cx).filter(|t| !t.is_empty()) {
            cx.emit(LogViewEvent::Search(op, text));
        }
    }

    pub fn set_quick_find(&mut self, matcher: Option<Arc<Matcher>>, cx: &mut Context<Self>) {
        self.quick_find = matcher;
        cx.notify();
    }

    /// Where a quick find starts, as a row and display column (klogg:
    /// `Selection::getNextPosition`, `getPreviousPosition`): after the
    /// selection going forward, at its start going backward. Without a
    /// selection, from the top of the screen.
    pub fn find_origin(&self, direction: Direction) -> Position {
        let at = |line, col| Position { line, col };
        let forward = direction == Direction::Forward;
        match (&self.selection, self.selection.lines()) {
            (Selection::Portion { line, start, end }, _) => {
                if forward {
                    at(*line, end + 1)
                } else {
                    at(*line, *start)
                }
            }
            (_, Some(rows)) if forward => at(rows.end() + 1, 0),
            (_, Some(rows)) => at(*rows.start(), 0),
            (_, None) => at(self.viewport.first_line(), 0),
        }
    }

    /// Selects a quick-find occurrence and scrolls to it.
    pub fn show_found(&mut self, found: &Found, cx: &mut Context<Self>) {
        self.cursor = found.row;
        let last = found.cols.end.max(found.cols.start + 1) - 1;
        self.select(Selection::portion(found.row, found.cols.start, last), cx);
        self.viewport.reveal_line(found.row);
        self.viewport.reveal_col(last);
        self.viewport.reveal_col(found.cols.start);
        self.check_follow(cx);
        cx.notify();
    }

    /// The selected part of a line, for "find selection" (`*`, `/`).
    pub fn selected_portion(&self, cx: &App) -> Option<String> {
        match self.selection {
            Selection::Portion { .. } => self.selected_text(false, cx),
            _ => None,
        }
    }

    /// Toggles marks on the selected lines (klogg: `markSelected`, `m`).
    fn mark_selection(&mut self, cx: &mut Context<Self>) {
        let (Some(filter), Some(rows), Some(log)) =
            (self.filter.clone(), self.selection.lines(), &self.log)
        else {
            return;
        };
        let line_count = log.line_count();
        let lines: Vec<usize> = rows.filter_map(|r| self.line_of(r, cx)).collect();
        filter.update(cx, |f, cx| {
            f.toggle_marks(lines, line_count);
            cx.notify();
        });
    }

    fn toggle_mark(&mut self, line: usize, cx: &mut Context<Self>) {
        let (Some(filter), Some(log)) = (self.filter.clone(), &self.log) else {
            return;
        };
        let line_count = log.line_count();
        filter.update(cx, |f, cx| {
            f.toggle_marks([line], line_count);
            cx.notify();
        });
    }

    /// Jumps to the next or previous mark (klogg: `]` and `[`).
    fn jump_to_mark(&mut self, next: bool, cx: &mut Context<Self>) {
        let Some(filter) = self.filter.clone() else {
            return;
        };
        let current = self.line_of(self.cursor, cx).unwrap_or(0);
        let target = {
            let f = filter.read(cx);
            if next {
                f.next_mark(current)
            } else {
                f.prev_mark(current)
            }
        };
        if let Some(line) = target {
            let row = self.row_of(line, cx);
            self.select_and_reveal(row, cx);
        }
    }

    /// Moves the selection one line, or as many as the digits typed before
    /// (klogg: `moveSelectionUp`, `moveSelectionDown`).
    fn move_selection(&mut self, up: bool, cx: &mut Context<Self>) {
        let count = self.take_number().unwrap_or(1).max(1);
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let line = match self.selection.lines() {
            None => self.viewport.first_line(),
            Some(lines) if up => lines.start().saturating_sub(count),
            Some(lines) => lines.end().saturating_add(count),
        };
        self.select_and_reveal(line, cx);
    }

    fn take_number(&mut self) -> Option<u64> {
        self.digits.take(std::time::Instant::now())
    }

    /// A digit key: adds it to the number being typed, or, for a lone `0`,
    /// scrolls to the start of the line (klogg: `keyPressEvent`).
    fn on_key_down(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let mut chars = keystroke.key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return;
        };
        if keystroke.modifiers.modified() || !c.is_ascii_digit() {
            return;
        }
        let now = std::time::Instant::now();
        if self.digits.add(c, now) {
            cx.stop_propagation();
        } else if c == '0' {
            self.viewport.set_left(0);
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// `[number] g`: selects file line `number` (1-based), or the first
    /// line without a number (klogg: `LogViewJumpToLineNumber`). In the
    /// filtered view, the row of that line or of the closest one before it.
    fn jump_to_line_number(&mut self, cx: &mut Context<Self>) {
        let number = self.take_number().unwrap_or(0);
        let line = usize::try_from(number.saturating_sub(1)).unwrap_or(usize::MAX);
        let row = self.row_of(line, cx);
        self.select_and_reveal(row, cx);
    }

    fn extend_selection(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(last) = self.row_count(cx).checked_sub(1) else {
            return;
        };
        let line = if up {
            self.cursor.saturating_sub(1)
        } else {
            (self.cursor + 1).min(last)
        };
        self.cursor = line;
        self.select(self.selection.extended_to(line), cx);
        self.viewport.reveal_line(line);
        self.check_follow(cx);
        cx.notify();
    }

    fn selected_text(&self, line_numbers: bool, cx: &App) -> Option<String> {
        let log = self.log.as_ref()?;
        if self.selection.is_empty() {
            return None;
        }
        Some(if self.filtered {
            self.selection
                .text_mapped(log, line_numbers, |r| self.line_of(r, cx))
        } else {
            self.selection.text(log, line_numbers)
        })
    }

    /// About the size of the selected text, from the line index. Counts up
    /// to just past [`CopyLimits::max`] in the filtered view.
    fn selection_bytes(&self, cx: &App) -> u64 {
        let Some(log) = &self.log else { return 0 };
        if self.filtered {
            self.selection
                .byte_len_mapped(log, |r| self.line_of(r, cx), self.limits.max)
        } else {
            self.selection.byte_len(log)
        }
    }

    /// klogg copies any selection at once, on the UI thread. Here a big one
    /// asks first and is built on a worker thread; above
    /// [`CopyLimits::max`] it isn't copied (`docs/decisions.md`).
    /// Selects every row (klogg: `selectAll`).
    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(last) = self.row_count(cx).checked_sub(1) {
            self.select(Selection::range(0, last), cx);
        }
    }

    pub(crate) fn copy_selection(
        &mut self,
        line_numbers: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = self.selection_bytes(cx);
        if bytes <= self.limits.ask {
            if let Some(text) = self.selected_text(line_numbers, cx) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        } else if bytes > self.limits.max {
            too_large(bytes, self.limits.max, "copied", window, cx);
        } else {
            let this = cx.entity().downgrade();
            window.open_alert_dialog(cx, move |alert, _, _| {
                let this = this.clone();
                alert
                    .title("Copy")
                    .description(format!(
                        "The selection is {}. Copy it to the clipboard?",
                        human_size(bytes)
                    ))
                    .show_cancel(true)
                    .ok_text("Copy")
                    .on_ok(move |_, _, cx| {
                        this.update(cx, |v, cx| v.copy_in_background(line_numbers, cx))
                            .ok();
                        true
                    })
            });
        }
    }

    /// Builds the selected text on a worker thread, then puts it on the
    /// clipboard.
    fn copy_in_background(&mut self, line_numbers: bool, cx: &mut Context<Self>) {
        let (Some(log), Some(rows)) = (self.log.clone(), self.selection.lines()) else {
            return;
        };
        let selection = self.selection.clone();
        let first = *rows.start();
        // The filtered view's rows are resolved here: the filter lives on
        // the UI thread.
        let lines: Option<Vec<Option<usize>>> = self
            .filtered
            .then(|| rows.map(|r| self.line_of(r, cx)).collect());
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let text =
                crate::background::run_blocking("kloggr-copy", &executor, move || match lines {
                    Some(lines) => selection.text_mapped(&log, line_numbers, |r| {
                        lines.get(r.checked_sub(first)?).copied().flatten()
                    }),
                    None => selection.text(&log, line_numbers),
                })
                .await;
            if let Some(text) = text {
                this.update(cx, |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                })
                .ok();
            }
        })
        .detach();
    }

    /// X11/Wayland primary selection (klogg: `updateGlobalSelection`). Set
    /// on every mouse selection, so a big one is left out.
    /// Linux and FreeBSD only: other systems have no primary selection.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn update_primary(&self, cx: &mut Context<Self>) {
        if self.selection_bytes(cx) > self.limits.primary {
            return;
        }
        if let Some(text) = self.selected_text(false, cx) {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn update_primary(&self, _: &mut Context<Self>) {}

    /// Widest of `rows`, in display columns (expanded).
    fn rows_width(&self, rows: std::ops::Range<usize>, cx: &App) -> usize {
        let Some(log) = &self.log else { return 0 };
        rows.filter_map(|r| self.line_of(r, cx))
            .filter_map(|l| log.line_text(l))
            .map(|l| text::columns(&l))
            .max()
            .unwrap_or(0)
    }

    fn selection_width(&self, cx: &App) -> usize {
        self.selection
            .lines()
            .map_or(0, |rows| self.rows_width(*rows.start()..rows.end() + 1, cx))
    }

    fn visible_width(&self, cx: &App) -> usize {
        self.rows_width(self.viewport.visible_lines(), cx)
    }

    fn position_at(&self, at: Point<Pixels>, layout: &Layout, cx: &App) -> Option<Position> {
        let last = self.row_count(cx).checked_sub(1)?;
        if self.wrapping && !self.wrap_rows.is_empty() {
            let i = self
                .wrap_rows
                .partition_point(|(y, ..)| *y <= at.y)
                .saturating_sub(1);
            let (_, row, part) = self.wrap_rows[i];
            let cols = ((at.x - layout.text.left()) / layout.char_width).max(0.0) as usize;
            return Some(Position {
                line: row.min(last),
                col: part * self.viewport.cols() + cols.min(self.viewport.cols()),
            });
        }
        let rows = (at.y - layout.text.top()) / layout.line_height;
        let line = (self.viewport.top() + f64::from(rows)).max(0.0) as usize;
        let cols = ((at.x - layout.text.left()) / layout.char_width).max(0.0);
        Some(Position {
            line: line.min(last),
            col: self.viewport.file_col(cols as usize),
        })
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(layout) = self.layout else { return };
        let at = event.position;
        if layout.overview.is_some_and(|o| o.contains(&at)) {
            self.grab = Some(MouseGrab::Overview);
            self.scroll_to_overview(at, &layout);
        } else if layout.vtrack.contains(&at) {
            if layout.vthumb.contains(&at) {
                self.grab = Some(MouseGrab::VThumb(at.y - layout.vthumb.top()));
            } else {
                let page = self.viewport.page();
                let up = at.y < layout.vthumb.top();
                self.viewport.scroll_lines(if up { -page } else { page });
            }
        } else if layout.htrack.contains(&at) {
            if layout.hthumb.contains(&at) {
                self.grab = Some(MouseGrab::HThumb(at.x - layout.hthumb.left()));
            } else {
                let page = self.viewport.scroll_page_cols() as isize;
                let left = at.x < layout.hthumb.left();
                self.viewport.scroll_cols(if left { -page } else { page });
            }
        } else if layout.over_separator(at) {
            self.grab = Some(MouseGrab::Separator);
        } else if layout.bullets.contains(&at) {
            // klogg marks a line clicked in the left margin.
            if let Some(line) = self
                .position_at(at, &layout, cx)
                .and_then(|pos| self.line_of(pos.line, cx))
            {
                self.toggle_mark(line, cx);
            }
        } else if let Some(pos) = self.position_at(at, &layout, cx) {
            self.cursor = pos.line;
            if event.modifiers.shift {
                self.select(self.selection.extended_to(pos.line), cx);
            } else if event.click_count == 2 {
                if let Some(word) = self
                    .line_of(pos.line, cx)
                    .zip(self.log.as_ref())
                    .and_then(|(line, log)| log.line_text(line))
                    .and_then(|line| text::word_at(&text::expand(&line), pos.col))
                {
                    self.select(Selection::portion(pos.line, word.start, word.end - 1), cx);
                }
                self.update_primary(cx);
            } else {
                let (drag, selection) = Drag::start(pos);
                self.grab = Some(MouseGrab::Text(drag));
                self.select(selection, cx);
            }
        }
        self.check_follow(cx);
        cx.notify();
    }

    /// klogg (`considerMouseHovering`): the filtered view reports the line
    /// under the mouse in its margin, for the overview bar.
    fn hover_margin(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let in_margin = at.x >= layout.bullets.left()
            && at.x < layout.text.left()
            && at.y >= layout.text.top()
            && at.y < layout.text.bottom();
        // Below the last row is not over a line (`position_at` clamps).
        let past_end = !self.wrapping && {
            let rows = (at.y - layout.text.top()) / layout.line_height;
            self.viewport.top() + f64::from(rows) >= self.row_count(cx) as f64
        };
        let line = (in_margin && !past_end)
            .then(|| self.position_at(at, &layout, cx))
            .flatten()
            .and_then(|p| self.line_of(p.line, cx));
        if line != self.hovered {
            self.hovered = line;
            cx.emit(LogViewEvent::HoverLine(line));
        }
    }

    /// Main view: flashes `line` on the overview bar, or stops (klogg:
    /// `highlightLine`, `removeHighlight`). The box shrinks in a few 30 ms
    /// steps, as klogg's pixmaps do.
    pub fn flash_overview(&mut self, line: Option<usize>, cx: &mut Context<Self>) {
        self.overview_flash = line.map(|l| (l, 0));
        self._flash = line.map(|_| {
            cx.spawn(async move |this, cx| {
                for _ in 0..OVERVIEW_FLASH_STEPS {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(30))
                        .await;
                    let alive = this.update(cx, |v, cx| {
                        if let Some((_, step)) = &mut v.overview_flash {
                            *step = (*step + 1).min(OVERVIEW_FLASH_STEPS);
                        }
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.filtered && event.pressed_button.is_none() {
            self.hover_margin(event.position, cx);
        }
        let (Some(layout), Some(grab)) = (self.layout, self.grab.as_mut()) else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let at = event.position;
        match grab {
            MouseGrab::VThumb(offset) => {
                let free = layout.vtrack.size.height - layout.vthumb.size.height;
                if free > px(0.) {
                    let ratio = ((at.y - *offset - layout.vtrack.top()) / free).clamp(0., 1.);
                    self.viewport
                        .set_top(f64::from(ratio) * self.viewport.max_top());
                }
            }
            MouseGrab::HThumb(offset) => {
                let free = layout.htrack.size.width - layout.hthumb.size.width;
                if free > px(0.) {
                    let ratio = ((at.x - *offset - layout.htrack.left()) / free).clamp(0., 1.);
                    self.viewport
                        .set_left((ratio * self.viewport.max_left() as f32).round() as usize);
                }
            }
            MouseGrab::Overview => self.scroll_to_overview(at, &layout),
            MouseGrab::Separator => {
                // klogg: the nearest column boundary, leaving some columns
                // to scroll.
                let col = ((at.x - layout.text.left()) / layout.char_width)
                    .round()
                    .max(0.) as usize;
                let max = self
                    .viewport
                    .cols()
                    .saturating_sub(kloggr_core::viewport::MIN_SCROLLABLE_COLUMNS);
                let column = col.min(max);
                if column != self.freeze.column {
                    self.set_freeze(
                        FrozenView {
                            column,
                            ..self.freeze
                        },
                        cx,
                    );
                }
            }
            MouseGrab::Text(drag) => {
                let mut drag = *drag;
                // Auto-scroll while dragging outside the text (klogg uses a
                // timer; here each mouse move scrolls by the overshoot).
                let text = layout.text;
                if at.y < text.top() {
                    self.viewport
                        .scroll_lines(-f64::from((text.top() - at.y) / layout.line_height).ceil());
                } else if at.y > text.bottom() {
                    self.viewport.scroll_lines(
                        f64::from((at.y - text.bottom()) / layout.line_height).ceil(),
                    );
                }
                if at.x > text.right() {
                    self.viewport
                        .scroll_cols(((at.x - text.right()) / layout.char_width).ceil() as isize);
                } else if at.x < text.left() && self.viewport.left() > 0 {
                    self.viewport
                        .scroll_cols(-(((text.left() - at.x) / layout.char_width).ceil() as isize));
                }
                let clamped = point(
                    at.x.clamp(text.left(), text.right()),
                    at.y.clamp(text.top(), text.bottom() - px(1.)),
                );
                if let Some(pos) = self.position_at(clamped, &layout, cx) {
                    self.cursor = pos.line;
                    if let Some(selection) = drag.update(pos) {
                        self.selection = selection;
                    }
                }
                self.grab = Some(MouseGrab::Text(drag));
            }
        }
        self.check_follow(cx);
        cx.notify();
    }

    /// Centers the line under `at` on the overview bar (klogg: `lineClicked`
    /// → `jumpToLine`; the selection doesn't change).
    fn scroll_to_overview(&mut self, at: Point<Pixels>, layout: &Layout) {
        let Some(bar) = layout.overview else { return };
        let height = f32::from(bar.size.height).floor().max(1.) as usize;
        let y = f32::from((at.y - bar.top()).max(px(0.))) as usize;
        let line = overview::line_at_y(y, self.viewport.line_count(), height);
        self.viewport
            .set_top(line as f64 - (self.viewport.rows() / 2.).floor());
    }

    /// Overview ticks for a bar `height` pixels tall, cached until the
    /// matches, marks, file or height change.
    fn overview_ticks(&mut self, height: usize, cx: &App) -> Option<Arc<Overview>> {
        let filter = self.filter.as_ref()?.read(cx);
        let line_count = self.log.as_ref()?.line_count();
        let key = (filter.generation(), line_count, height);
        match &self.overview {
            Some((k, ticks)) if *k == key => Some(ticks.clone()),
            _ => {
                let ticks = Arc::new(Overview::new(filter, line_count, height));
                self.overview = Some((key, ticks.clone()));
                Some(ticks)
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.grab.take() {
            Some(MouseGrab::Text(_)) => self.update_primary(cx),
            Some(MouseGrab::Separator) => cx.emit(LogViewEvent::FreezeChanged(self.freeze)),
            _ => {}
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let delta = event.delta.pixel_delta(layout.line_height);
        // klogg: Ctrl+wheel (Cmd on macOS) changes the font size, one step
        // per wheel notch (which X11 reports as 3 lines); a trackpad's small
        // scrolls add up to a line first.
        if event.modifiers.secondary() {
            self.zoom_carry += match event.delta {
                gpui::ScrollDelta::Lines(lines) if lines.y > 0. => 1.,
                gpui::ScrollDelta::Lines(lines) if lines.y < 0. => -1.,
                gpui::ScrollDelta::Lines(_) => 0.,
                gpui::ScrollDelta::Pixels(_) => delta.y / layout.line_height,
            };
            let steps = self.zoom_carry.trunc();
            if steps != 0. {
                self.zoom_carry -= steps;
                change_font_size(steps, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        // Shift + wheel scrolls sideways (the usual convention), and so does
        // Alt as in klogg (Qt's scroll areas); see `docs/decisions.md`.
        let sideways = event.modifiers.shift || event.modifiers.alt;
        let (dx, dy) = if sideways && delta.x == px(0.) {
            (delta.y, px(0.))
        } else {
            (delta.x, delta.y)
        };
        let lines = -f64::from(dy / layout.line_height);
        // klogg (`allowFollowOnScroll`): scrolling down at the bottom
        // starts following.
        if lines > 0.
            && self.at_bottom()
            && !self.following
            && Preferences::get(cx).file.follow_on_scroll
        {
            cx.emit(LogViewEvent::FollowChanged(true));
        }
        self.viewport.scroll_lines(lines);
        self.check_follow(cx);
        self.hscroll_carry -= dx;
        let cols = (self.hscroll_carry / layout.char_width).trunc();
        if cols != 0. {
            self.hscroll_carry -= layout.char_width * cols;
            self.viewport.scroll_cols(cols as isize);
        }
        cx.notify();
    }
}

/// Changes the log views' font size by `steps` points (klogg:
/// `CrawlerWidget::changeFontSize`), within the options' 4 to 72.
fn change_font_size(steps: f32, window: &mut Window, cx: &mut App) {
    let mut options = Preferences::get(cx).clone();
    let current = options
        .view
        .font_size
        .unwrap_or_else(|| f32::from(cx.theme().mono_font_size));
    let size = (current.round() + steps).clamp(4., 72.);
    if size != current {
        options.view.font_size = Some(size);
        Preferences::set(options, Some(window), cx);
    }
}

impl EventEmitter<LogViewEvent> for LogView {}

impl Focusable for LogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("log-view")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|v, _: &SelectUp, _, cx| v.move_selection(true, cx)))
            .on_action(cx.listener(|v, _: &SelectDown, _, cx| v.move_selection(false, cx)))
            .on_action(cx.listener(|v, _: &ExtendUp, _, cx| v.extend_selection(true, cx)))
            .on_action(cx.listener(|v, _: &ExtendDown, _, cx| v.extend_selection(false, cx)))
            .on_action(cx.listener(|v, _: &PageUp, _, cx| {
                v.viewport.scroll_lines(-v.viewport.page());
                v.check_follow(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| {
                v.viewport.scroll_lines(v.viewport.page());
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollLeft, _, cx| {
                v.viewport
                    .scroll_cols(-(v.viewport.scroll_page_cols() as isize));
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollRight, _, cx| {
                v.viewport
                    .scroll_cols(v.viewport.scroll_page_cols() as isize);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ToggleFreezeColumns, _, cx| v.toggle_freeze(cx)))
            .on_action(cx.listener(|v, _: &AddToSearch, _, cx| v.search_op(SearchOp::Add, cx)))
            .on_action(cx.listener(|v, _: &SetSearchStart, _, cx| {
                let line = v.selected_line(false, cx);
                cx.emit(LogViewEvent::SearchLimit(LimitOp::Start(line)));
            }))
            .on_action(cx.listener(|v, _: &SetSearchEnd, _, cx| {
                let line = v.selected_line(true, cx);
                cx.emit(LogViewEvent::SearchLimit(LimitOp::End(line)));
            }))
            .on_action(
                cx.listener(|v, _: &SaveToFile, window, cx| v.save_to_file(false, window, cx)),
            )
            .on_action(
                cx.listener(|v, _: &SaveSelectedToFile, window, cx| {
                    v.save_to_file(true, window, cx)
                }),
            )
            .on_action(cx.listener(|v, _: &SetSelectionStart, _, _| {
                // klogg: `setSelectionStart`, the selected line.
                v.selection_start = (v.selection != Selection::None).then_some(v.cursor);
            }))
            .on_action(cx.listener(|v, _: &SetSelectionEnd, _, cx| {
                // klogg: `setSelectionEnd` selects from the start to here.
                if let Some(start) = v.selection_start.take()
                    && v.selection != Selection::None
                {
                    let end = v.cursor;
                    v.select(Selection::range(start, end), cx);
                }
            }))
            .on_action(cx.listener(|_, _: &ExitView, _, cx| cx.emit(LogViewEvent::ExitView)))
            .on_action(cx.listener(|_, _: &SaveSplitterPosition, _, cx| {
                cx.emit(LogViewEvent::SaveSplitterPosition)
            }))
            .on_action(cx.listener(|_, _: &ClearSearchLimits, _, cx| {
                cx.emit(LogViewEvent::SearchLimit(LimitOp::Clear));
            }))
            .on_action(
                cx.listener(|v, _: &SendToScratchpad, w, cx| v.send_to_scratchpad(false, w, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ReplaceScratchpad, w, cx| v.send_to_scratchpad(true, w, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ExcludeFromSearch, _, cx| v.search_op(SearchOp::Exclude, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ReplaceSearch, _, cx| v.search_op(SearchOp::Replace, cx)),
            )
            .on_action(cx.listener(|v, _: &StartOfLine, _, cx| {
                v.viewport.set_left(0);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &EndOfLine, _, cx| {
                let width = v.selection_width(cx);
                v.viewport.grow_max_cols(width);
                v.viewport.set_left(width.saturating_sub(v.viewport.cols()));
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &RightOfScreen, _, cx| {
                let width = v.visible_width(cx);
                v.viewport.grow_max_cols(width);
                v.viewport.set_left(width.saturating_sub(v.viewport.cols()));
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &Top, _, cx| v.select_and_reveal(0, cx)))
            .on_action(cx.listener(|v, _: &Bottom, _, cx| {
                // klogg: at the bottom already, Shift+G starts following.
                let was_at_bottom = v.at_bottom();
                v.select_and_reveal(usize::MAX, cx);
                v.viewport.set_top(f64::MAX);
                if was_at_bottom && !v.following {
                    cx.emit(LogViewEvent::FollowChanged(true));
                }
            }))
            .on_action(cx.listener(|v, _: &SelectAll, _, cx| v.select_all(cx)))
            .on_action(cx.listener(|v, _: &Copy, w, cx| v.copy_selection(false, w, cx)))
            .on_action(
                cx.listener(|v, _: &CopyWithLineNumbers, w, cx| v.copy_selection(true, w, cx)),
            )
            .on_action(cx.listener(|v, _: &Mark, _, cx| v.mark_selection(cx)))
            .on_action(cx.listener(|v, _: &NextMark, _, cx| v.jump_to_mark(true, cx)))
            .on_action(cx.listener(|v, _: &PrevMark, _, cx| v.jump_to_mark(false, cx)))
            .on_action(cx.listener(|v, _: &JumpToLineNumber, _, cx| v.jump_to_line_number(cx)))
            .on_action(
                cx.listener(|v, a: &SetColorLabel, _, cx| v.color_label(LabelOp::Set(a.label), cx)),
            )
            .on_action(
                cx.listener(|v, _: &RemoveColorLabel, _, cx| v.color_label(LabelOp::Remove, cx)),
            )
            .on_action(cx.listener(|v, _: &NextColorLabel, _, cx| v.color_label(LabelOp::Next, cx)))
            .on_action(
                cx.listener(|v, _: &ClearColorLabels, _, cx| v.color_label(LabelOp::ClearAll, cx)),
            )
            .on_key_down(cx.listener(|v, event, _, cx| v.on_key_down(event, cx)))
            .child(LogElement { view: cx.entity() })
            .context_menu({
                let view = cx.entity().downgrade();
                move |menu, window, cx| {
                    let Some(view) = view.upgrade() else {
                        return menu;
                    };
                    LogView::context_menu(&view, menu, window, cx)
                }
            })
    }
}

impl LogView {
    /// klogg's log view popup menu (`AbstractLogView::createMenu`): copy,
    /// color labels for the selected text, and the highlighter sets.
    fn context_menu(
        view: &Entity<Self>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let this = view.read(cx);
        let text = this.label_text(cx).filter(|t| !t.is_empty());
        let current = text.as_deref().and_then(|t| this.color_labels.label_of(t));
        let no_text = text.is_none();
        let (focus, nothing_selected) = (this.focus.clone(), this.selection == Selection::None);
        let frozen = this.freeze.enabled;
        let limited = this.search_limits != (0..usize::MAX);
        let selection_started = this.selection_start.is_some();
        let styles = Highlighters::color_label_styles(cx);
        let sets: Vec<(String, String, bool)> = cx
            .try_global::<Highlighters>()
            .map(|h| {
                h.sets
                    .sets
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone(), h.sets.is_active(&s.id)))
                    .collect()
            })
            .unwrap_or_default();
        // klogg's order (`AbstractLogView::createMenu`).
        menu.action_context(focus)
            .menu_with_disabled("Mark", Box::new(Mark), nothing_selected)
            .separator()
            .menu_with_disabled("Copy", Box::new(Copy), nothing_selected)
            .menu_with_disabled(
                "Copy with line numbers",
                Box::new(CopyWithLineNumbers),
                nothing_selected,
            )
            .menu_with_disabled(
                "Send to scratchpad",
                Box::new(SendToScratchpad),
                nothing_selected,
            )
            .menu_with_disabled(
                "Replace scratchpad",
                Box::new(ReplaceScratchpad),
                nothing_selected,
            )
            .separator()
            .menu_with_disabled(
                "Find next",
                Box::new(crate::quick_find::FindSelectionNext),
                no_text,
            )
            .menu_with_disabled(
                "Find previous",
                Box::new(crate::quick_find::FindSelectionPrevious),
                no_text,
            )
            .separator()
            .menu_with_disabled("Replace search", Box::new(ReplaceSearch), no_text)
            .menu_with_disabled("Add to search", Box::new(AddToSearch), no_text)
            .menu_with_disabled("Exclude from search", Box::new(ExcludeFromSearch), no_text)
            .separator()
            .menu("Set search start", Box::new(SetSearchStart))
            .menu("Set search end", Box::new(SetSearchEnd))
            .menu_with_disabled("Clear search limits", Box::new(ClearSearchLimits), !limited)
            .separator()
            .menu_with_disabled(
                "Set selection start",
                Box::new(SetSelectionStart),
                nothing_selected,
            )
            .menu_with_disabled(
                "Set selection end",
                Box::new(SetSelectionEnd),
                nothing_selected || !selection_started,
            )
            .separator()
            .menu("Save to file…", Box::new(SaveToFile))
            .menu_with_disabled(
                "Save selected to file…",
                Box::new(SaveSelectedToFile),
                nothing_selected,
            )
            .separator()
            .menu_with_check_and_disabled(
                "Freeze columns",
                frozen,
                Box::new(ToggleFreezeColumns),
                this.wrapping,
            )
            .separator()
            .menu("Save splitter position", Box::new(SaveSplitterPosition))
            .separator()
            .submenu("Color labels", window, cx, move |menu, _, _| {
                let mut menu = menu
                    .menu_with_check_and_disabled(
                        "None",
                        current.is_none(),
                        Box::new(RemoveColorLabel),
                        no_text,
                    )
                    .separator();
                for (label, style) in styles.iter().enumerate() {
                    menu = menu.menu_with_check_and_disabled(
                        style.name.clone(),
                        current == Some(label),
                        Box::new(SetColorLabel { label }),
                        no_text,
                    );
                }
                menu.separator()
                    .menu("Clear all", Box::new(ClearColorLabels))
            })
            .submenu("Highlighters", window, cx, move |menu, _, _| {
                let any_active = sets.iter().any(|s| s.2);
                let mut menu = menu.menu_with_disabled(
                    "None",
                    Box::new(highlighters::NoHighlighters),
                    !any_active,
                );
                if !sets.is_empty() {
                    menu = menu.separator();
                }
                for (id, name, active) in &sets {
                    menu = menu.menu_with_check(
                        name.clone(),
                        *active,
                        Box::new(highlighters::ToggleHighlighterSet { id: id.clone() }),
                    );
                }
                menu
            })
    }
}

/// Text style of the view, from the theme's monospace settings.
struct Metrics {
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    char_width: Pixels,
}

impl Metrics {
    fn new(window: &mut Window, cx: &App) -> Self {
        let theme = cx.theme();
        let font = gpui::font(theme.mono_font_family.clone());
        let font_size = theme.mono_font_size;
        let ts = window.text_system();
        let font_id = ts.resolve_font(&font);
        let char_width = ts
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        Self {
            font,
            font_size,
            line_height: (font_size * 1.4).round(),
            char_width,
        }
    }

    /// Shapes `text` with a colour per byte range: `runs` are (length,
    /// colour) pairs covering the text.
    fn shape_runs(
        &self,
        window: &mut Window,
        text: SharedString,
        runs: &[(usize, Hsla)],
    ) -> ShapedLine {
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|&(len, color)| TextRun {
                len,
                font: self.font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        window
            .text_system()
            .shape_line(text, self.font_size, &runs, None)
    }

    fn shape(&self, window: &mut Window, text: SharedString, color: Hsla) -> ShapedLine {
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text, self.font_size, &[run], None)
    }
}

struct Row {
    y: Pixels,
    /// The line number, unless hidden.
    number: Option<ShapedLine>,
    /// The text, in pieces drawn at their grid column (see
    /// [`text::grid_segments`]).
    text: Vec<(usize, ShapedLine)>,
    /// Selected screen columns (up to two parts with frozen columns).
    highlight: Vec<(usize, usize)>,
    /// Quick-find occurrences, relative to the first visible column.
    found: Vec<(usize, usize)>,
    /// Search pattern matches, relative to the first visible column.
    searched: Vec<(usize, usize)>,
    /// Background of the whole row, from a whole-line highlighter.
    line_back: Option<Hsla>,
    /// Highlighter and color label backgrounds, relative to the first
    /// visible column.
    colored: Vec<(usize, usize, Hsla)>,
    kind: LineKind,
}

pub struct Frame {
    hitbox: Hitbox,
    layout: Layout,
    gutter: Bounds<Pixels>,
    rows: Vec<Row>,
    focused: bool,
    overview: Option<OverviewFrame>,
}

pub struct OverviewFrame {
    ticks: Arc<Overview>,
    /// Pixel rows of the first and last visible lines.
    view: (usize, usize),
    /// Pixel row of the flashed line, and the animation step.
    flash: Option<(usize, u8)>,
}

/// Steps of the overview flash (klogg: `INITIAL_TTL_VALUE` pixmaps).
const OVERVIEW_FLASH_STEPS: u8 = 5;

struct LogElement {
    view: Entity<LogView>,
}

impl IntoElement for LogElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for LogElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let metrics = Metrics::new(window, cx);
        let (fg, muted) = {
            let theme = cx.theme();
            (theme.foreground, theme.muted_foreground)
        };
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let focused = self.view.read(cx).focus.is_focused(window);

        self.view.update(cx, |view, cx| {
            // Line numbers are the file's, so the gutter fits the file's.
            let file_lines = view.log.as_ref().map_or(0, |l| l.line_count());
            let line_count = view.row_count(cx);
            let digits = file_lines.max(1).to_string().len().max(3);
            let options = Preferences::get(cx);
            // klogg: `lineNumbersVisibleInMain`, `...InFiltered`.
            let numbers = if view.filtered {
                options.view.line_numbers_filtered
            } else {
                options.view.line_numbers_main
            };
            let gutter_width = if numbers {
                BULLET_WIDTH + metrics.char_width * digits + GUTTER_PAD * 2.
            } else {
                BULLET_WIDTH + GUTTER_PAD
            };
            let highlight_matches = options.search.highlight_matches;
            let text_wrap = options.view.text_wrap;
            let show_overview = options.view.overview;
            let gutter = Bounds::new(
                bounds.origin,
                size(gutter_width, bounds.size.height - SCROLLBAR),
            );
            let bullets = Bounds::new(
                bounds.origin,
                size(BULLET_WIDTH, bounds.size.height - SCROLLBAR),
            );
            let with_overview = show_overview && !view.filtered && view.filter.is_some();
            let overview_bounds = with_overview.then(|| {
                Bounds::from_corners(
                    point(bounds.right() - SCROLLBAR - OVERVIEW_WIDTH, bounds.top()),
                    point(bounds.right() - SCROLLBAR, bounds.bottom() - SCROLLBAR),
                )
            });
            let text_right = overview_bounds.map_or(bounds.right() - SCROLLBAR, |o| o.left());
            let text_bounds = Bounds::from_corners(
                point(gutter.right() + GUTTER_PAD / 2., bounds.top()),
                point(text_right, bounds.bottom() - SCROLLBAR),
            );
            let rows = f64::from(text_bounds.size.height / metrics.line_height);
            let cols = (text_bounds.size.width / metrics.char_width).max(0.) as usize;
            view.wrapping = text_wrap;
            // With text wrap, the scroll range ends where the last lines fit
            // at the bottom (klogg: `getNbBottomWrappedVisibleLines`).
            let mut view_rows = rows;
            if text_wrap && let Some(log) = view.log.clone() {
                let mut probe = view.viewport;
                probe.set_size(line_count, rows, cols);
                let screen_rows = rows.floor().max(1.) as usize;
                let (mut used, mut fit) = (0, 0);
                for row in (0..line_count).rev() {
                    let Some(bytes) = view.line_of(row, cx).and_then(|l| log.text_line(l)) else {
                        break;
                    };
                    used += probe.wrapped_rows(wrap_width(&log, &bytes, rows, cols));
                    if used > screen_rows && fit > 0 {
                        break;
                    }
                    fit += 1;
                }
                view_rows = fit as f64;
                // No horizontal scrolling.
                view.viewport.set_left(0);
            }
            view.viewport.set_size(line_count, view_rows, cols);
            if view.following {
                view.viewport.set_top(f64::MAX);
            }

            // Visible rows: decode and shape only the visible columns.
            let mut out = Vec::new();
            if let Some(log) = view.log.clone() {
                let vp = view.viewport;
                let lh = metrics.line_height;
                // The visible screen rows.
                let mut shown: Vec<ScreenRow> = Vec::new();
                if text_wrap {
                    let mut row = vp.first_line();
                    // The scrolled-out fraction of the first line.
                    let frac = vp.top() - row as f64;
                    let mut y = None;
                    while row < line_count {
                        let Some((line, bytes)) = view
                            .line_of(row, cx)
                            .and_then(|l| Some((l, log.text_line(l)?)))
                        else {
                            break;
                        };
                        let parts = vp.wrapped_rows(wrap_width(&log, &bytes, rows, cols));
                        let y0: Pixels = *y.get_or_insert_with(|| {
                            text_bounds.top() - lh * (frac * parts as f64) as f32
                        });
                        for part in 0..parts {
                            let y = y0 + lh * part as f32;
                            if y >= text_bounds.bottom() {
                                break;
                            }
                            shown.push(ScreenRow {
                                row,
                                line,
                                bytes: bytes.clone(),
                                part,
                                vp: vp.wrapped(part),
                                y,
                            });
                        }
                        let next = y0 + lh * parts as f32;
                        if next >= text_bounds.bottom() {
                            break;
                        }
                        y = Some(next);
                        row += 1;
                    }
                } else {
                    let visible = vp.visible_lines();
                    let y = |row: usize| text_bounds.top() + lh * (row as f64 - vp.top()) as f32;
                    if view.filtered {
                        for row in visible {
                            if let Some(line) = view.line_of(row, cx)
                                && let Some(bytes) = log.text_line(line)
                            {
                                shown.push(ScreenRow {
                                    row,
                                    line,
                                    bytes,
                                    part: 0,
                                    vp,
                                    y: y(row),
                                });
                            }
                        }
                    } else {
                        let first = visible.start;
                        for (i, bytes) in log.text_lines(visible).enumerate() {
                            let row = first + i;
                            shown.push(ScreenRow {
                                row,
                                line: row,
                                bytes,
                                part: 0,
                                vp,
                                y: y(row),
                            });
                        }
                    }
                }
                view.wrap_rows = if text_wrap {
                    shown.iter().map(|r| (r.y, r.row, r.part)).collect()
                } else {
                    Vec::new()
                };
                let filter = view.filter.as_ref().map(|f| f.read(cx));
                let highlighters = Highlighters::active(cx);
                let labels = view.labels_compiled.clone();
                let colouring = !highlighters.is_empty() || !labels.is_empty();
                for ScreenRow {
                    row,
                    line,
                    bytes,
                    part,
                    vp,
                    y,
                } in shown
                {
                    let kind = filter.map_or_else(LineKind::default, |f| f.kind(line));
                    // klogg: lines outside the search limits are drawn in
                    // the disabled text colour, without highlighters.
                    let outside = !view.search_limits.contains(&line);
                    let (shown_text, seen) = screen_text(&log, &bytes, &vp);
                    if !text_wrap {
                        view.viewport.grow_max_cols(seen);
                    }
                    let highlight = if view.selection.is_line_selected(row) {
                        vec![(0, vp.cols() + 1)]
                    } else {
                        view.selection
                            .portion_of(row)
                            .map(|cols| vp.screen_ranges(*cols.start()..*cols.end() + 1))
                            .unwrap_or_default()
                    };
                    // Visible columns of a matcher's matches in the line.
                    let visible_matches = |m: &Matcher| -> Vec<(usize, usize)> {
                        find::match_columns(&log, m, line)
                            .into_iter()
                            .flat_map(|c| vp.screen_ranges(c))
                            .collect()
                    };
                    let short = bytes.len() <= MAX_HIGHLIGHT_LINE;
                    let found = match &view.quick_find {
                        Some(m) if short => visible_matches(m),
                        _ => Vec::new(),
                    };
                    // Only matched lines can hold search matches.
                    let searched = match &view.search_highlight {
                        Some(m) if highlight_matches && short && kind.matched => visible_matches(m),
                        _ => Vec::new(),
                    };
                    // Highlighters and color labels (klogg: not on lines
                    // selected whole, which are drawn reversed).
                    let (line_colors, spans) = if colouring
                        && short
                        && !outside
                        && !view.selection.is_line_selected(row)
                    {
                        colored_columns(&log, line, &highlighters, &labels, &vp)
                    } else {
                        (None, Vec::new())
                    };
                    let line_fg = if outside {
                        muted
                    } else {
                        line_colors.and_then(|c| c.fore).map_or(fg, hsla)
                    };
                    let runs = text_runs(&shown_text, line_fg, &spans);
                    // A wrapped line's number and bullet are on its first row.
                    let first_part = part == 0;
                    out.push(Row {
                        y,
                        number: (numbers && first_part)
                            .then(|| metrics.shape(window, (line + 1).to_string().into(), muted)),
                        text: text::grid_segments(&shown_text)
                            .into_iter()
                            .map(|(col, bytes)| {
                                let runs = slice_runs(&runs, bytes.clone());
                                let piece = shown_text[bytes].to_owned();
                                (col, metrics.shape_runs(window, piece.into(), &runs))
                            })
                            .collect(),
                        highlight,
                        found,
                        searched,
                        line_back: line_colors.and_then(|c| c.back).map(hsla),
                        colored: spans
                            .iter()
                            .filter_map(|(from, to, c)| Some((*from, *to, hsla(c.back?))))
                            .collect(),
                        kind: if first_part {
                            kind
                        } else {
                            LineKind::default()
                        },
                    });
                }
            }

            // Scrollbars: tracks on the right and bottom, thumbs sized by the
            // visible fraction.
            let vp = view.viewport;
            let vtrack = Bounds::from_corners(
                point(bounds.right() - SCROLLBAR, bounds.top()),
                point(bounds.right(), bounds.bottom() - SCROLLBAR),
            );
            let total = vp.line_count() as f64;
            let vthumb_h = if total > 0. {
                (vtrack.size.height * (vp.rows() / total).min(1.) as f32).max(MIN_THUMB)
            } else {
                vtrack.size.height
            };
            let vratio = if vp.max_top() > 0. {
                (vp.top() / vp.max_top()) as f32
            } else {
                0.
            };
            let vthumb = Bounds::new(
                point(
                    vtrack.left(),
                    vtrack.top() + (vtrack.size.height - vthumb_h) * vratio,
                ),
                size(SCROLLBAR, vthumb_h),
            );
            let htrack = Bounds::from_corners(
                point(text_bounds.left(), bounds.bottom() - SCROLLBAR),
                point(bounds.right() - SCROLLBAR, bounds.bottom()),
            );
            let total_cols = if text_wrap {
                vp.cols().max(1) as f32
            } else {
                vp.max_cols().max(vp.cols()).max(1) as f32
            };
            let hthumb_w =
                (htrack.size.width * (vp.cols() as f32 / total_cols).min(1.)).max(MIN_THUMB);
            let hratio = if vp.max_left() > 0 {
                vp.left() as f32 / vp.max_left() as f32
            } else {
                0.
            };
            let hthumb = Bounds::new(
                point(
                    htrack.left() + (htrack.size.width - hthumb_w) * hratio,
                    htrack.top(),
                ),
                size(hthumb_w, SCROLLBAR),
            );

            let layout = Layout {
                text: text_bounds,
                bullets,
                line_height: metrics.line_height,
                char_width: metrics.char_width,
                vtrack,
                vthumb,
                htrack,
                hthumb,
                overview: overview_bounds,
                // klogg: no frozen columns with text wrap.
                separator: (view.freeze.enabled && !text_wrap)
                    .then(|| text_bounds.left() + metrics.char_width * vp.frozen()),
            };
            view.layout = Some(layout);
            let overview = overview_bounds.and_then(|bar| {
                let height = f32::from(bar.size.height).floor().max(0.) as usize;
                let ticks = view.overview_ticks(height, cx)?;
                // The visible part of the file (klogg: `getViewLines`).
                let lines = vp.line_count();
                let top = overview::y_of_line(vp.first_line(), lines, height);
                let shown = (vp.rows().ceil() as usize).min(lines);
                let bottom = top + overview::y_of_line(shown, lines, height);
                let flash = view
                    .overview_flash
                    .map(|(line, step)| (overview::y_of_line(line, lines, height), step));
                Some(OverviewFrame {
                    ticks,
                    view: (top, bottom.min(height.saturating_sub(1))),
                    flash,
                })
            });
            Frame {
                hitbox,
                layout,
                gutter,
                rows: out,
                focused,
                overview,
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = cx.theme();
        let (bg, gutter_bg, border, selection, selection_unfocused, track, thumb) = (
            theme.background,
            theme.muted,
            theme.border,
            theme.selection,
            theme.selection.opacity(0.5),
            theme.scrollbar,
            theme.scrollbar_thumb,
        );
        let found_bg = Preferences::quickfind_highlight(theme.yellow, cx);
        let fg = theme.foreground;
        // klogg's default `mainSearchBackColor` is light gray; by default a
        // translucent foreground gives a gray in light and dark themes alike.
        let search_bg = Preferences::search_highlight(fg, cx);
        let bullet_colors = BulletColors {
            matched: theme.red,
            marked: theme.blue,
            both: theme.magenta,
        };
        let layout = frame.layout;
        let lh = layout.line_height;

        window.paint_quad(fill(bounds, bg));
        window.paint_quad(fill(frame.gutter, gutter_bg));
        for x in [layout.bullets.right(), frame.gutter.right()] {
            window.paint_quad(fill(
                Bounds::new(
                    point(x, bounds.top()),
                    size(px(1.), frame.gutter.size.height),
                ),
                border,
            ));
        }

        let highlight = if frame.focused {
            selection
        } else {
            selection_unfocused
        };
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: Bounds::from_corners(bounds.origin, layout.text.bottom_right()),
            }),
            |window| {
                for row in &frame.rows {
                    if let Some(back) = row.line_back {
                        let w = layout.text.size.width;
                        window.paint_quad(fill(
                            Bounds::new(point(layout.text.left(), row.y), size(w, lh)),
                            back,
                        ));
                    }
                    for &(from, to, back) in &row.colored {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window.paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), back));
                    }
                    for &(from, to) in &row.searched {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window
                            .paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), search_bg));
                    }
                    for &(from, to) in &row.found {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window
                            .paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), found_bg));
                    }
                    for &(from, to) in &row.highlight {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window
                            .paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), highlight));
                    }
                    paint_bullet(window, &layout, row, &bullet_colors);
                    if let Some(number) = &row.number {
                        let x = frame.gutter.right() - GUTTER_PAD - number.width;
                        number
                            .paint(point(x, row.y), lh, TextAlign::Left, None, window, cx)
                            .ok();
                    }
                }
            },
        );
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: layout.text,
            }),
            |window| {
                for row in &frame.rows {
                    for (col, piece) in &row.text {
                        piece
                            .paint(
                                point(layout.text.left() + layout.char_width * *col, row.y),
                                lh,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                    }
                }
            },
        );

        // klogg draws the frozen part and the scrolled part side by side;
        // a line marks where scrolling starts.
        if let Some(x) = layout.separator {
            window.paint_quad(fill(
                Bounds::new(
                    point(x, layout.text.top()),
                    size(px(1.), layout.text.size.height),
                ),
                fg.opacity(0.4),
            ));
        }

        if let (Some(bar), Some(o)) = (layout.overview, &frame.overview) {
            paint_overview(window, bar, o, &bullet_colors, gutter_bg, border, fg);
        }

        window.paint_quad(fill(layout.vtrack, track));
        window.paint_quad(fill(layout.htrack, track));
        window.paint_quad(fill(layout.vthumb, thumb));
        window.paint_quad(fill(layout.hthumb, thumb));

        if frame.hitbox.is_hovered(window) {
            let over_text = window.mouse_position();
            let style = if layout.over_separator(over_text)
                || matches!(self.view.read(cx).grab, Some(MouseGrab::Separator))
            {
                CursorStyle::ResizeLeftRight
            } else if layout.text.contains(&over_text) {
                CursorStyle::IBeam
            } else {
                CursorStyle::Arrow
            };
            window.set_cursor_style(style, &frame.hitbox);
        }

        // Mouse handling is registered on the window, not the hitbox, so that
        // drags keep working outside the view.
        let hitbox = frame.hitbox.clone();
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && e.button == MouseButton::Left && hitbox.is_hovered(window) {
                view.update(cx, |v, cx| v.on_mouse_down(e, window, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                view.update(cx, |v, cx| v.on_mouse_move(e, window, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase.bubble() && e.button == MouseButton::Left {
                view.update(cx, |v, cx| v.on_mouse_up(e, window, cx));
            }
        });
        let hitbox = frame.hitbox.clone();
        let view = self.view.clone();
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase.bubble() && hitbox.should_handle_scroll(window) {
                view.update(cx, |v, cx| v.on_scroll(e, window, cx));
            }
        });
    }
}

/// Highlighter and color-label colours of `line`: the whole line's colours,
/// and the visible coloured parts as screen columns (klogg: the
/// `highlighterMatches` of `drawTextArea`).
fn colored_columns(
    log: &LogFile,
    line: usize,
    highlighters: &CompiledSet,
    labels: &CompiledSet,
    vp: &Viewport,
) -> (Option<Colors>, Vec<(usize, usize, Colors)>) {
    let Some(text) = log.line_text(line) else {
        return (None, Vec::new());
    };
    let mut found = highlighters.match_line(&text);
    labels.add_matches(&text, &mut found.spans);
    let columns = text::column_ranges(&text, &found.spans.ranges());
    let spans = found
        .spans
        .with_ranges(columns)
        .into_vec()
        .into_iter()
        .flat_map(|s| {
            vp.screen_ranges(s.range)
                .into_iter()
                .map(move |(from, to)| (from, to, s.colors))
        })
        .collect();
    (found.line, spans)
}

/// The text drawn for a line, one char per screen column: the frozen
/// columns (padded to their width), then the scrolled ones. Also returns
/// the line's width in columns as far as known, to bound scrolling.
fn screen_text(log: &LogFile, bytes: &[u8], vp: &Viewport) -> (String, usize) {
    let frozen = vp.frozen();
    let start = frozen + vp.left();
    let scrolled = text::window(log.encoding(), bytes, start, vp.window_end() - start);
    let seen = scrolled
        .total_cols
        .unwrap_or(start + text::columns(&scrolled.text) + 1);
    if frozen == 0 {
        return (scrolled.text, seen);
    }
    let mut out = text::window(log.encoding(), bytes, 0, frozen).text;
    // A wide character cut by the separator would overlap the scrolled part.
    while text::columns(&out) > frozen {
        out.pop();
    }
    let pad = frozen.saturating_sub(text::columns(&out));
    out.extend(std::iter::repeat_n(' ', pad));
    out.push_str(&scrolled.text);
    (out, seen)
}

/// A screen row of a log view: a line, or with text wrap a part of one.
struct ScreenRow<'a> {
    row: usize,
    /// The file line.
    line: usize,
    /// As shown (see [`LogFile::text_line`]).
    bytes: std::borrow::Cow<'a, [u8]>,
    /// Which part of the wrapped line (0 without wrap).
    part: usize,
    /// The viewport the part is drawn with (see [`Viewport::wrapped`]).
    vp: Viewport,
    y: Pixels,
}

/// Columns of `bytes` that matter for wrapping: up to what fills the view
/// (`rows` × `cols`), so that a huge line costs no more than a screen.
fn wrap_width(log: &LogFile, bytes: &[u8], rows: f64, cols: usize) -> usize {
    let cap = (rows.ceil() as usize + 1) * cols.max(1);
    let w = text::window(log.encoding(), bytes, 0, cap);
    w.total_cols.unwrap_or(cap).min(cap)
}

/// Text runs for `text` (one char per column): `fg`, except in `spans`
/// (column ranges) with a foreground colour.
fn text_runs(text: &str, fg: Hsla, spans: &[(usize, usize, Colors)]) -> Vec<(usize, Hsla)> {
    let byte = |col: usize| text::byte_at_column(text, col);
    let mut runs = Vec::new();
    let mut at = 0;
    for &(from, to, colors) in spans {
        let Some(fore) = colors.fore else { continue };
        let (start, end) = (byte(from), byte(to));
        if start >= end {
            continue;
        }
        if start > at {
            runs.push((start - at, fg));
        }
        runs.push((end - start, hsla(fore)));
        at = end;
    }
    if at < text.len() || runs.is_empty() {
        runs.push((text.len() - at, fg));
    }
    runs
}

/// The part of `runs` (lengths covering a text) inside byte range `bytes`.
fn slice_runs(runs: &[(usize, Hsla)], bytes: std::ops::Range<usize>) -> Vec<(usize, Hsla)> {
    let mut out = Vec::new();
    let mut at = 0;
    for &(len, color) in runs {
        let (start, end) = (at.max(bytes.start), (at + len).min(bytes.end));
        if start < end {
            out.push((end - start, color));
        }
        at += len;
    }
    out
}

struct BulletColors {
    matched: Hsla,
    marked: Hsla,
    /// Marked and matched (klogg: `markedMatchBrush`).
    both: Hsla,
}

/// klogg's overview bar (`OverviewWidget::paintEvent`): a separator, the
/// match and mark ticks inset by a margin, and lines around the visible part.
fn paint_overview(
    window: &mut Window,
    bar: Bounds<Pixels>,
    frame: &OverviewFrame,
    colors: &BulletColors,
    bg: Hsla,
    border: Hsla,
    fg: Hsla,
) {
    window.paint_quad(fill(bar, bg));
    window.paint_quad(fill(
        Bounds::new(bar.origin, size(px(1.), bar.size.height)),
        border,
    ));
    let tick = |window: &mut Window, y: usize, color: Hsla| {
        let left = bar.left() + px(1.) + OVERVIEW_MARGIN;
        let width = bar.size.width - px(2.) - OVERVIEW_MARGIN * 2.;
        window.paint_quad(fill(
            Bounds::new(point(left, bar.top() + px(y as f32)), size(width, px(1.))),
            color,
        ));
    };
    for t in &frame.ticks.matches {
        tick(window, t.y, colors.matched.opacity(t.opacity()));
    }
    for t in &frame.ticks.marks {
        tick(window, t.y, colors.marked.opacity(t.opacity()));
    }
    // The visible part: a light band between two lines.
    let (top, bottom) = frame.view;
    let y = |row: usize| bar.top() + px(row as f32);
    window.paint_quad(fill(
        Bounds::from_corners(
            point(bar.left() + px(1.), y(top)),
            point(bar.right(), y(bottom) + px(1.)),
        ),
        fg.opacity(0.08),
    ));
    for row in [top, bottom] {
        window.paint_quad(fill(
            Bounds::new(
                point(bar.left() + px(1.), y(row)),
                size(bar.size.width - px(1.), px(1.)),
            ),
            fg.opacity(0.7),
        ));
    }
    // klogg: a purple box around the line hovered in the filtered view,
    // shrinking from the bar's width.
    if let Some((row, step)) = frame.flash {
        let color: Hsla = gpui::rgb(0x572F80).into();
        let inset = px(1. + f32::from(step) * 1.5).min(bar.size.width / 3.);
        let half_h = px(4. - f32::from(step).min(2.));
        let rect = Bounds::from_corners(
            point(bar.left() + inset, y(row) - half_h),
            point(bar.right() - inset, y(row) + half_h + px(1.)),
        );
        let t = px(2.);
        for edge in [
            Bounds::new(rect.origin, size(rect.size.width, t)),
            Bounds::new(
                point(rect.left(), rect.bottom() - t),
                size(rect.size.width, t),
            ),
            Bounds::new(rect.origin, size(t, rect.size.height)),
            Bounds::new(
                point(rect.right() - t, rect.top()),
                size(t, rect.size.height),
            ),
        ] {
            window.paint_quad(fill(edge, color));
        }
    }
}

/// klogg's bullets (`AbstractLogView::drawTextArea`): an arrow on marked
/// lines, blue or, if the line also matches, magenta; a red dot on lines
/// that only match.
fn paint_bullet(window: &mut Window, layout: &Layout, row: &Row, colors: &BulletColors) {
    let left = layout.bullets.left();
    let mid_x = left + BULLET_WIDTH / 2.;
    let mid_y = row.y + layout.line_height / 2.;
    if row.kind.marked {
        let color = if row.kind.matched {
            colors.both
        } else {
            colors.marked
        };
        // Shaft from the left edge, head pointing at the line number.
        let (shaft, head) = (px(2.), px(4.5));
        let mut arrow = gpui::PathBuilder::fill();
        arrow.move_to(point(left + px(1.), mid_y - shaft));
        arrow.line_to(point(mid_x, mid_y - shaft));
        arrow.line_to(point(mid_x, mid_y - head));
        arrow.line_to(point(layout.bullets.right() - px(1.), mid_y));
        arrow.line_to(point(mid_x, mid_y + head));
        arrow.line_to(point(mid_x, mid_y + shaft));
        arrow.line_to(point(left + px(1.), mid_y + shaft));
        arrow.close();
        if let Ok(path) = arrow.build() {
            window.paint_path(path, color);
        }
    } else if row.kind.matched {
        let r = px(3.);
        let dot = Bounds::from_corners(point(mid_x - r, mid_y - r), point(mid_x + r, mid_y + r));
        window.paint_quad(fill(dot, colors.matched).corner_radii(r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Modifiers, ScrollDelta, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    fn temp_log(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("kloggr-view-{}-{name}", std::process::id()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        path
    }

    fn numbered(n: usize) -> String {
        (0..n).map(|i| format!("line {i} word_{i}\n")).collect()
    }

    fn open<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
        content: &str,
    ) -> (Entity<LogView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(key_bindings());
        });
        let path = temp_log(name, content);
        let log = LogFile::open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| LogView::new(cx));
        view.update_in(cx, |v, window, cx| {
            v.set_log(Some(Arc::new(log)), cx);
            window.focus(&v.focus, cx);
        });
        cx.run_until_parked();
        (view, cx)
    }

    /// Window coordinates of the middle of a display cell.
    fn layout(view: &Entity<LogView>, cx: &mut VisualTestContext) -> Layout {
        view.read_with(cx, |v, _| v.layout.expect("drawn"))
    }

    fn cell(layout: &Layout, row: f32, col: f32) -> Point<Pixels> {
        point(
            layout.text.left() + layout.char_width * (col + 0.5),
            layout.text.top() + layout.line_height * (row + 0.5),
        )
    }

    fn selection(view: &Entity<LogView>, cx: &mut VisualTestContext) -> Selection {
        view.read_with(cx, |v, _| v.selection.clone())
    }

    fn viewport(view: &Entity<LogView>, cx: &mut VisualTestContext) -> Viewport {
        view.read_with(cx, |v, _| v.viewport)
    }

    #[gpui::test]
    fn keyboard_navigation(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "keys.log", &numbered(1000));
        let page = viewport(&view, cx).page();
        assert!(page >= 5.0, "window too small: {page}");

        cx.simulate_keystrokes("down");
        assert_eq!(selection(&view, cx), Selection::Line(0));
        cx.simulate_keystrokes("j j k down");
        assert_eq!(selection(&view, cx), Selection::Line(2));
        cx.simulate_keystrokes("shift-down shift-down");
        assert_eq!(selection(&view, cx), Selection::range(2, 4));
        cx.simulate_keystrokes("shift-up shift-up shift-up");
        assert_eq!(selection(&view, cx), Selection::range(2, 1));

        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        assert_eq!(selection(&view, cx), Selection::Line(999));
        let vp = viewport(&view, cx);
        assert_eq!(vp.top(), vp.max_top());
        assert_eq!(vp.visible_lines().end, 1000);

        cx.simulate_keystrokes("ctrl-home");
        assert_eq!(selection(&view, cx), Selection::Line(0));
        assert_eq!(viewport(&view, cx).top(), 0.0);
        cx.simulate_keystrokes("pagedown pagedown pageup");
        assert_eq!(viewport(&view, cx).top(), page);
        cx.simulate_keystrokes("shift-g");
        assert_eq!(selection(&view, cx), Selection::Line(999));

        // Moving past the bottom edge recenters (klogg `displayLine`).
        cx.simulate_keystrokes("ctrl-home");
        let rows = viewport(&view, cx).rows().floor() as usize;
        for _ in 0..rows {
            cx.simulate_keystrokes("down");
        }
        let vp = viewport(&view, cx);
        assert!(vp.visible_lines().contains(&rows), "{vp:?}");
        assert!(vp.top() > 0.0);
    }

    #[gpui::test]
    fn mouse_selection(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "mouse.log", &numbered(100));
        let l = layout(&view, cx);

        cx.simulate_click(cell(&l, 3., 2.), Modifiers::none());
        assert_eq!(selection(&view, cx), Selection::Line(3));

        // Drag on the same line: portion; to another line: whole lines.
        let start = cell(&l, 5., 1.);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(cell(&l, 5., 4.), MouseButton::Left, Modifiers::none());
        assert_eq!(selection(&view, cx), Selection::portion(5, 1, 4));
        cx.simulate_mouse_move(cell(&l, 8., 4.), MouseButton::Left, Modifiers::none());
        assert_eq!(selection(&view, cx), Selection::range(5, 8));
        cx.simulate_mouse_up(cell(&l, 8., 4.), MouseButton::Left, Modifiers::none());

        cx.simulate_click(cell(&l, 10., 0.), Modifiers::shift());
        assert_eq!(selection(&view, cx), Selection::range(5, 10));

        // Double click on "word_2" (columns 7..=12 of "line 2 word_2").
        let at = cell(&l, 2., 9.);
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        assert_eq!(selection(&view, cx), Selection::portion(2, 7, 12));

        cx.simulate_keystrokes("secondary-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("word_2")
        );
    }

    #[gpui::test]
    fn bullet_click_toggles_mark(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "bullets.log", &numbered(20));
        let filter = cx.new(|_| FilteredLines::new());
        view.update(cx, |v, cx| v.set_filter(filter.clone(), cx));
        cx.run_until_parked();
        let l = layout(&view, cx);
        let at = point(l.bullets.center().x, cell(&l, 3., 0.).y);
        cx.simulate_click(at, Modifiers::none());
        assert_eq!(
            filter.read_with(cx, |f, _| f.marks().iter().collect::<Vec<_>>()),
            [3]
        );
        // A margin click marks without selecting.
        assert_eq!(selection(&view, cx), Selection::None);
        cx.simulate_click(at, Modifiers::none());
        assert!(filter.read_with(cx, |f, _| f.marks().is_empty()));
    }

    #[gpui::test]
    fn overview_bar(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "overview.log", &numbered(10_000));
        // No filter: no bar.
        assert!(layout(&view, cx).overview.is_none());
        let filter = cx.new(|_| FilteredLines::new());
        view.update(cx, |v, cx| v.set_filter(filter.clone(), cx));
        filter.update(cx, |f, cx| {
            f.add_matches(&[100, 5000]);
            f.toggle_marks([9000], 10_000);
            cx.notify();
        });
        cx.run_until_parked();
        let l = layout(&view, cx);
        let bar = l.overview.expect("main view with a filter has a bar");
        assert!(l.text.right() <= bar.left() && bar.right() <= l.vtrack.left());
        let ticks = view.read_with(cx, |v, _| v.overview.clone().unwrap().1);
        let height = f32::from(bar.size.height).floor() as usize;
        let y = |line| overview::y_of_line(line, 10_000, height);
        assert_eq!(
            ticks.matches.iter().map(|t| t.y).collect::<Vec<_>>(),
            [y(100), y(5000)]
        );
        assert_eq!(
            ticks.marks.iter().map(|t| t.y).collect::<Vec<_>>(),
            [y(9000)]
        );

        // A click scrolls so that the line under it is centered, without
        // selecting; dragging keeps scrolling.
        let at = |frac: f32| point(bar.center().x, bar.top() + bar.size.height * frac);
        cx.simulate_mouse_down(at(0.5), MouseButton::Left, Modifiers::none());
        let vp = viewport(&view, cx);
        let middle = vp.top() + (vp.rows() / 2.).floor();
        assert!((middle - 5000.).abs() < 20., "{middle}");
        assert_eq!(selection(&view, cx), Selection::None);
        cx.simulate_mouse_move(at(0.25), MouseButton::Left, Modifiers::none());
        let vp = viewport(&view, cx);
        let middle = vp.top() + (vp.rows() / 2.).floor();
        assert!((middle - 2500.).abs() < 20., "{middle}");
        cx.simulate_mouse_up(at(0.25), MouseButton::Left, Modifiers::none());

        // New marks are drawn: the cache follows the filter.
        filter.update(cx, |f, cx| {
            f.toggle_marks([0], 10_000);
            cx.notify();
        });
        cx.run_until_parked();
        let ticks = view.read_with(cx, |v, _| v.overview.clone().unwrap().1);
        assert_eq!(ticks.marks.first().map(|t| t.y), Some(0));
    }

    #[gpui::test]
    fn copy_lines(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "copy.log", "a\tb\nsecond\r\nthird\n");
        let l = layout(&view, cx);
        cx.simulate_click(cell(&l, 0., 0.), Modifiers::none());
        cx.simulate_keystrokes("shift-down secondary-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a\tb\nsecond")
        );
        cx.simulate_keystrokes("secondary-shift-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("1: a\tb\n2: second")
        );
        cx.simulate_keystrokes("secondary-a secondary-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a\tb\nsecond\nthird")
        );
    }

    #[gpui::test]
    fn big_copy_runs_in_background(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, "unused.log", "");
        let path = temp_log("bigcopy.log", "a\nb ERR\nc\nd ERR\ne\n");
        let log = Arc::new(LogFile::open(&path).unwrap());
        std::fs::remove_file(path).unwrap();
        let filter = cx.new(|_| FilteredLines::new());
        filter.update(cx, |f, _| f.add_matches(&[1, 3]));
        let view = cx.new(|cx| {
            let mut v = LogView::filtered(cx);
            v.set_log(Some(log.clone()), cx);
            v.set_filter(filter.clone(), cx);
            v
        });
        view.update(cx, |v, cx| {
            v.selection = Selection::range(0, 1);
            assert_eq!(v.selection_bytes(cx), 12);
            v.copy_in_background(true, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("2: b ERR\n4: d ERR")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[gpui::test]
    fn big_selection_skips_primary(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "primary.log", &numbered(10));
        view.update(cx, |v, cx| {
            v.limits.primary = 20;
            v.selection = Selection::range(0, 9);
            v.update_primary(cx);
        });
        assert_eq!(cx.update(|_, cx| cx.read_from_primary()), None);
        view.update(cx, |v, cx| {
            v.selection = Selection::Line(1);
            v.update_primary(cx);
        });
        assert_eq!(
            cx.update(|_, cx| cx.read_from_primary())
                .and_then(|c| c.text())
                .as_deref(),
            Some("line 1 word_1")
        );
    }

    #[gpui::test]
    fn wheel_sideways_with_shift_or_alt(cx: &mut TestAppContext) {
        let text: String = (0..100)
            .map(|i| format!("{i} {}\n", "x".repeat(500)))
            .collect();
        let (view, cx) = open(cx, "wide.log", &text);
        let l = layout(&view, cx);
        let at = cell(&l, 1., 1.);
        // Shift or Alt + wheel scrolls sideways (klogg: Alt).
        for modifiers in [Modifiers::shift(), Modifiers::alt()] {
            cx.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), -l.char_width * 4.)),
                modifiers,
                ..Default::default()
            });
            assert_eq!(viewport(&view, cx).top(), 0.0, "not scrolled down");
            assert_eq!(viewport(&view, cx).left(), 4);
            cx.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), l.char_width * 4.)),
                modifiers,
                ..Default::default()
            });
            assert_eq!(viewport(&view, cx).left(), 0);
        }
    }

    #[gpui::test]
    fn wheel_and_scrollbar(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "wheel.log", &numbered(10_000));
        let l = layout(&view, cx);
        let at = cell(&l, 1., 1.);
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
        assert_eq!(viewport(&view, cx).top(), 3.0);
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), l.line_height * 1.5)),
            ..Default::default()
        });
        assert_eq!(viewport(&view, cx).top(), 1.5);

        // Ctrl (Cmd) + wheel changes the font size instead of scrolling.
        let size =
            |cx: &mut VisualTestContext| cx.update(|_, cx| f32::from(cx.theme().mono_font_size));
        let before = size(cx);
        let zoom = |cx: &mut VisualTestContext, lines: f32| {
            cx.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Lines(point(0., lines)),
                modifiers: gpui::Modifiers::secondary_key(),
                ..Default::default()
            })
        };
        // A wheel notch is one step, however many lines it scrolls.
        zoom(cx, 3.);
        assert_eq!(size(cx), before + 1.);
        assert_eq!(viewport(&view, cx).top(), 1.5, "not scrolled");
        zoom(cx, -3.);
        zoom(cx, -3.);
        zoom(cx, -3.);
        assert_eq!(size(cx), before - 2.);
        // A trackpad's pixels add up to a line per step.
        let pixels = |cx: &mut VisualTestContext, lines: f32| {
            cx.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), l.line_height * lines)),
                modifiers: gpui::Modifiers::secondary_key(),
                ..Default::default()
            })
        };
        pixels(cx, 0.6);
        assert_eq!(size(cx), before - 2., "not a line yet");
        pixels(cx, 0.6);
        assert_eq!(size(cx), before - 1.);
        cx.update(|_, cx| assert_eq!(Preferences::get(cx).view.font_size, Some(before - 1.)));

        // Drag the vertical thumb to the bottom of its track.
        cx.run_until_parked();
        let layout = layout(&view, cx);
        let thumb = layout.vthumb.center();
        cx.simulate_mouse_down(thumb, MouseButton::Left, Modifiers::none());
        let below = point(thumb.x, layout.vtrack.bottom() + px(50.));
        cx.simulate_mouse_move(below, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(below, MouseButton::Left, Modifiers::none());
        let vp = viewport(&view, cx);
        assert_eq!(vp.top(), vp.max_top());
        assert_eq!(selection(&view, cx), Selection::None);
    }

    #[gpui::test]
    fn long_lines_scroll_horizontally(cx: &mut TestAppContext) {
        let long: String = (0..200_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let (view, cx) = open(cx, "long.log", &format!("short\n{long}\n"));
        let l = layout(&view, cx);
        let vp = viewport(&view, cx);
        assert!(vp.max_left() >= 200_000 - vp.cols(), "{vp:?}");

        cx.simulate_keystrokes("end");
        cx.run_until_parked();
        let vp = viewport(&view, cx);
        assert_eq!(vp.left(), 200_000 - vp.cols());
        cx.simulate_keystrokes("home");
        assert_eq!(viewport(&view, cx).left(), 0);
        cx.simulate_keystrokes("right l");
        assert_eq!(viewport(&view, cx).left(), 2 * vp.cols());
        cx.simulate_keystrokes("h");
        assert_eq!(viewport(&view, cx).left(), vp.cols());

        // `$` scrolls to the end of the selected line.
        cx.simulate_click(cell(&l, 0., 0.), Modifiers::none());
        cx.simulate_keystrokes("$");
        assert_eq!(viewport(&view, cx).left(), 0);
        cx.simulate_keystrokes("down $");
        assert_eq!(viewport(&view, cx).left(), 200_000 - vp.cols());
    }

    #[gpui::test]
    fn save_to_file(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, "save.log", &numbered(5));
        let out = std::env::temp_dir().join(format!("kloggr-view-{}-saved", std::process::id()));
        let save = |selected: bool, cx: &mut VisualTestContext| {
            let target = out.clone();
            view.update_in(cx, |v, window, cx| {
                let lines = v.lines_to_save(selected, cx).unwrap();
                let log = v.log.clone().unwrap();
                v.start_save(log, target, lines, window, cx);
            });
            cx.run_until_parked();
            std::fs::read_to_string(&out).unwrap()
        };
        assert_eq!(save(false, cx), numbered(5));
        cx.simulate_keystrokes("down shift-down");
        assert_eq!(save(true, cx), "line 0 word_0\nline 1 word_1\n");
        assert!(view.read_with(cx, |v, _| v.save_cancel.is_none()));
        std::fs::remove_file(out).unwrap();
    }

    #[gpui::test]
    fn text_wrap(cx: &mut TestAppContext) {
        let long: String = (0..200_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let (view, cx) = open(cx, "wrap.log", &format!("short\n{long}\nlast\n"));
        cx.update(|window, cx| {
            let mut options = Preferences::get(cx).clone();
            options.view.text_wrap = true;
            Preferences::set(options, Some(window), cx);
        });
        view.update(cx, |v, cx| {
            v.set_freeze(
                FrozenView {
                    enabled: true,
                    column: 3,
                },
                cx,
            )
        });
        cx.run_until_parked();
        let l = layout(&view, cx);
        let cols = viewport(&view, cx).cols();
        let parts = |cx: &mut VisualTestContext| {
            view.read_with(cx, |v, _| {
                v.wrap_rows
                    .iter()
                    .map(|&(_, row, part)| (row, part))
                    .collect::<Vec<_>>()
            })
        };
        // The long line fills the rest of the screen; no frozen columns.
        let shown = parts(cx);
        assert_eq!(shown[..3], [(0, 0), (1, 0), (1, 1)]);
        assert!(shown[1..].iter().all(|&(row, _)| row == 1));
        assert!(l.separator.is_none());

        // A click on the third screen row is in the long line's second part.
        let at = view.read_with(cx, |v, cx| v.position_at(cell(&l, 2., 3.), &l, cx));
        assert_eq!(
            at,
            Some(Position {
                line: 1,
                col: cols + 3
            })
        );

        // No horizontal scrolling; the last line can be reached.
        cx.simulate_keystrokes("end");
        assert_eq!(viewport(&view, cx).left(), 0);
        cx.simulate_keystrokes("shift-g");
        cx.run_until_parked();
        assert_eq!(parts(cx).last(), Some(&(2, 0)));
    }

    #[gpui::test]
    fn frozen_columns(cx: &mut TestAppContext) {
        let long: String = (0..2000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let (view, cx) = open(cx, "frozen.log", &format!("{long}\nshort\n"));
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, e: &LogViewEvent, _| {
                events.borrow_mut().push(e.clone())
            })
            .detach()
        });
        let on = |column| FrozenView {
            enabled: true,
            column,
        };
        view.update(cx, |v, cx| {
            v.set_freeze(on(10), cx);
            // A new search or file resets the view, not the frozen columns.
            v.reset(cx)
        });
        cx.run_until_parked();
        let vp = viewport(&view, cx);
        assert_eq!(vp.frozen(), 10);
        // A page scrolls the columns that aren't frozen.
        cx.simulate_keystrokes("right");
        let vp = viewport(&view, cx);
        assert_eq!(vp.left(), vp.cols() - 10);

        // Rows show the frozen columns, then the scrolled ones; short lines
        // are padded.
        let log = view.read_with(cx, |v, _| v.log.clone().unwrap());
        let (text, _) = screen_text(&log, &log.line(0).unwrap(), &vp);
        assert!(text.starts_with(&long[..10]));
        assert_eq!(&text[10..20], &long[10 + vp.left()..20 + vp.left()]);
        let (text, _) = screen_text(&log, &log.line(1).unwrap(), &vp);
        assert_eq!(text, "short     ");
        // A wide character cut by the separator is left out of the frozen
        // part.
        let path = temp_log("frozen-wide", "abcdefghi日本\n");
        let wide = LogFile::open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let (text, seen) = screen_text(&wide, &wide.line(0).unwrap(), &vp);
        assert!(text.starts_with("abcdefghi "), "{text:?}");
        assert_eq!(seen, 13);

        // Clicks map screen columns to file columns.
        let l = layout(&view, cx);
        cx.simulate_click(cell(&l, 0., 3.), Modifiers::none());
        cx.simulate_keystrokes("secondary-c");
        let at = |cx: &mut VisualTestContext, col: f32| {
            view.read_with(cx, |v, cx| {
                v.position_at(cell(&l, 0., col), &l, cx).unwrap().col
            })
        };
        assert_eq!(at(cx, 3.), 3);
        assert_eq!(at(cx, 12.), 12 + vp.left());

        // Dragging the separator moves it to the nearest column boundary.
        let x = l.separator.unwrap();
        assert_eq!(x, l.text.left() + l.char_width * 10.);
        let y = cell(&l, 1., 0.).y;
        cx.simulate_mouse_down(point(x + px(1.), y), MouseButton::Left, Modifiers::none());
        let to = point(l.text.left() + l.char_width * 4.2, y);
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        assert_eq!(view.read_with(cx, |v, _| v.freeze()), on(4));
        assert_eq!(
            events.borrow().last(),
            Some(&LogViewEvent::FreezeChanged(on(4)))
        );
        // The separator press selected nothing.
        assert_eq!(selection(&view, cx), Selection::Line(0));

        // The context menu's toggle keeps the column.
        view.update(cx, |v, cx| v.toggle_freeze(cx));
        assert_eq!(viewport(&view, cx).frozen(), 0);
        assert_eq!(
            events.borrow().last(),
            Some(&LogViewEvent::FreezeChanged(FrozenView {
                enabled: false,
                column: 4
            }))
        );
    }

    #[test]
    fn text_runs_colour_spans() {
        use kloggr_core::highlight::Color;
        let fg = gpui::black();
        let red = Colors {
            fore: Some(Color::rgb(255, 0, 0)),
            back: None,
        };
        let back_only = Colors {
            fore: None,
            back: Some(Color::rgb(0, 0, 255)),
        };
        let r = hsla(Color::rgb(255, 0, 0));
        assert_eq!(
            text_runs("abcdef", fg, &[(1, 3, red), (3, 4, back_only)]),
            [(1, fg), (2, r), (3, fg)]
        );
        // Columns are chars: "é" is two bytes.
        assert_eq!(text_runs("éab", fg, &[(0, 2, red)]), [(3, r), (1, fg)]);
        // Spans past the end are clipped; empty text gets one run.
        assert_eq!(text_runs("ab", fg, &[(1, 9, red)]), [(1, fg), (1, r)]);
        assert_eq!(text_runs("", fg, &[]), [(0, fg)]);
        // A wide character takes two columns; a span can't split it.
        assert_eq!(
            text_runs("a日b", fg, &[(1, 3, red)]),
            [(1, fg), (3, r), (1, fg)]
        );
        assert_eq!(text_runs("a日b", fg, &[(2, 3, red)]), [(5, fg)]);
        // Runs cut to a piece of the text.
        assert_eq!(slice_runs(&[(1, fg), (3, r), (1, fg)], 1..4), [(3, r)]);
        assert_eq!(
            slice_runs(&[(1, fg), (3, r), (1, fg)], 0..2),
            [(1, fg), (1, r)]
        );
    }

    #[test]
    fn colored_columns_map_bytes_to_visible_columns() {
        use kloggr_core::highlight::{Highlighter, HighlighterSet};
        let path = temp_log("colored", "\tERROR é x\nplain\n");
        let log = LogFile::open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let mut set = HighlighterSet::new("s");
        set.highlighters.push(Highlighter {
            pattern: "ERROR".into(),
            match_only: true,
            ..Highlighter::default()
        });
        set.highlighters.push(Highlighter {
            pattern: "plain".into(),
            ..Highlighter::default()
        });
        let (compiled, _) = CompiledSet::compile(&set.highlighters);
        let mut labels = ColorLabels::new();
        labels.set(0, "x");
        let styles = kloggr_core::highlight::ColorLabelStyle::defaults();
        let labels = labels.compile(&styles);

        // The tab expands to 8 columns: ERROR is 8..13, "x" (after "é ") 16..17.
        let viewport = |left: usize, cols: usize, freeze: usize| {
            let mut v = Viewport::default();
            v.grow_max_cols(1000);
            v.set_size(10, 5.0, cols);
            v.set_left(left);
            v.set_freeze(freeze);
            v
        };
        let (line, spans) = colored_columns(&log, 0, &compiled, &labels, &viewport(0, 99, 0));
        assert_eq!(line, None);
        let cols: Vec<_> = spans.iter().map(|s| (s.0, s.1)).collect();
        assert_eq!(cols, [(8, 13), (16, 17)]);
        assert_eq!(spans[1].2, styles[0].colors());
        // Scrolled to column 10, 4 columns wide.
        let (_, spans) = colored_columns(&log, 0, &compiled, &labels, &viewport(10, 3, 0));
        let cols: Vec<_> = spans.iter().map(|s| (s.0, s.1)).collect();
        assert_eq!(cols, [(0, 3)]);
        // 6 frozen columns, scrolled by 2: file column 8 is screen column 6.
        let (_, spans) = colored_columns(&log, 0, &compiled, &labels, &viewport(2, 20, 6));
        let cols: Vec<_> = spans.iter().map(|s| (s.0, s.1)).collect();
        assert_eq!(cols, [(6, 11), (14, 15)]);
        // A whole-line highlighter.
        let (line, spans) = colored_columns(&log, 1, &compiled, &labels, &viewport(0, 99, 0));
        assert_eq!(line, Some(Highlighter::default().colors()));
        assert!(spans.is_empty());
    }
}
