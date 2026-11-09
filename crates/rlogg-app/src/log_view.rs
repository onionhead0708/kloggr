//! The log view: a custom GPUI element drawing the visible part of a
//! [`LogFile`] (klogg: `AbstractLogView`).
//!
//! Only visible rows are decoded and shaped, and only their visible columns
//! (see [`rlogg_core::text::window`]), so a multi-GB file or a multi-MB line
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
//! The main view also draws klogg's overview bar between the text and the
//! scrollbar: a tick per pixel row holding matches (red) or marks (blue),
//! and the visible part of the file. Clicking or dragging on it scrolls
//! there.

use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, Font, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    ScrollWheelEvent, ShapedLine, SharedString, Style, Styled, Subscription, TextAlign, TextRun,
    Window, actions, div, fill, point, px, relative, size,
};
use gpui_component::ActiveTheme;
use rlogg_core::search::find::{self, Direction, Found};
use rlogg_core::search::overview::{self, Overview};
use rlogg_core::search::{FilteredLines, LineKind, Matcher};
use rlogg_core::text;
use rlogg_core::{DigitsBuffer, Drag, LogFile, Position, Selection, Viewport};

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
    ]
);

pub const CONTEXT: &str = "LogView";

/// klogg's default log view shortcuts (`settings/src/shortcuts.cpp`).
pub fn key_bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![
        KeyBinding::new("up", SelectUp, c),
        KeyBinding::new("k", SelectUp, c),
        KeyBinding::new("down", SelectDown, c),
        KeyBinding::new("j", SelectDown, c),
        KeyBinding::new("shift-up", ExtendUp, c),
        KeyBinding::new("shift-down", ExtendDown, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("ctrl-up", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("ctrl-down", PageDown, c),
        KeyBinding::new("left", ScrollLeft, c),
        KeyBinding::new("h", ScrollLeft, c),
        KeyBinding::new("right", ScrollRight, c),
        KeyBinding::new("l", ScrollRight, c),
        KeyBinding::new("home", StartOfLine, c),
        KeyBinding::new("^", StartOfLine, c),
        KeyBinding::new("$", EndOfLine, c),
        KeyBinding::new("end", RightOfScreen, c),
        KeyBinding::new("ctrl-home", Top, c),
        KeyBinding::new("ctrl-end", Bottom, c),
        KeyBinding::new("shift-g", Bottom, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-shift-c", CopyWithLineNumbers, c),
        KeyBinding::new("m", Mark, c),
        KeyBinding::new("]", NextMark, c),
        KeyBinding::new("[", PrevMark, c),
        KeyBinding::new("g", JumpToLineNumber, c),
    ]
}

/// Emitted when the user selects a line (klogg: `newSelection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogViewEvent {
    /// The file line under the keyboard cursor, after a click or a key.
    LineSelected(usize),
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
}

#[derive(Debug, Clone, Copy)]
enum MouseGrab {
    Text(Drag),
    /// Dragging a scrollbar thumb; offset of the grab point in the thumb.
    VThumb(Pixels),
    HThumb(Pixels),
    /// Pressed on the overview bar: scrolls while dragging.
    Overview,
}

pub struct LogView {
    focus: FocusHandle,
    log: Option<Arc<LogFile>>,
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
    /// Quick-find pattern: its occurrences on visible rows are highlighted
    /// (klogg: `QuickFindPattern`).
    quick_find: Option<Arc<Matcher>>,
    /// Search pattern: its matches on visible rows are highlighted (klogg:
    /// `searchPattern_`, "main search highlight").
    search_highlight: Option<Arc<Matcher>>,
    /// Digits typed before a command (klogg: `digitsBuffer_`).
    digits: DigitsBuffer,
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
            filter: None,
            filtered: false,
            _observe_filter: None,
            selection: Selection::None,
            cursor: 0,
            viewport: Viewport::default(),
            layout: None,
            grab: None,
            hscroll_carry: px(0.),
            quick_find: None,
            search_highlight: None,
            digits: DigitsBuffer::new(),
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

    /// Clears the selection and scrolls back to the top, e.g. when the rows
    /// are replaced by a new search.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.viewport = Viewport::default();
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
    pub fn has_search_highlight(&self) -> bool {
        self.search_highlight.is_some()
    }

    pub fn set_search_highlight(&mut self, matcher: Option<Arc<Matcher>>, cx: &mut Context<Self>) {
        self.search_highlight = matcher;
        cx.notify();
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

    fn copy_selection(&self, line_numbers: bool, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text(line_numbers, cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// X11/Wayland primary selection (klogg: `updateGlobalSelection`).
    fn update_primary(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text(false, cx) {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

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
        let rows = (at.y - layout.text.top()) / layout.line_height;
        let line = (self.viewport.top() + f64::from(rows)).max(0.0) as usize;
        let cols = ((at.x - layout.text.left()) / layout.char_width).max(0.0);
        Some(Position {
            line: line.min(last),
            col: self.viewport.left() + cols as usize,
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
                let page = self.viewport.cols().max(1) as isize;
                let left = at.x < layout.hthumb.left();
                self.viewport.scroll_cols(if left { -page } else { page });
            }
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
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        if let Some(MouseGrab::Text(_)) = self.grab.take() {
            self.update_primary(cx);
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let delta = event.delta.pixel_delta(layout.line_height);
        let (dx, dy) = if event.modifiers.shift && delta.x == px(0.) {
            (delta.y, px(0.))
        } else {
            (delta.x, delta.y)
        };
        self.viewport
            .scroll_lines(-f64::from(dy / layout.line_height));
        self.hscroll_carry -= dx;
        let cols = (self.hscroll_carry / layout.char_width).trunc();
        if cols != 0. {
            self.hscroll_carry -= layout.char_width * cols;
            self.viewport.scroll_cols(cols as isize);
        }
        cx.notify();
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
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| {
                v.viewport.scroll_lines(v.viewport.page());
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollLeft, _, cx| {
                v.viewport.scroll_cols(-(v.viewport.cols().max(1) as isize));
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollRight, _, cx| {
                v.viewport.scroll_cols(v.viewport.cols().max(1) as isize);
                cx.notify();
            }))
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
                v.select_and_reveal(usize::MAX, cx);
                v.viewport.set_top(f64::MAX);
            }))
            .on_action(cx.listener(|v, _: &SelectAll, _, cx| {
                if let Some(last) = v.row_count(cx).checked_sub(1) {
                    v.select(Selection::range(0, last), cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Copy, _, cx| v.copy_selection(false, cx)))
            .on_action(cx.listener(|v, _: &CopyWithLineNumbers, _, cx| v.copy_selection(true, cx)))
            .on_action(cx.listener(|v, _: &Mark, _, cx| v.mark_selection(cx)))
            .on_action(cx.listener(|v, _: &NextMark, _, cx| v.jump_to_mark(true, cx)))
            .on_action(cx.listener(|v, _: &PrevMark, _, cx| v.jump_to_mark(false, cx)))
            .on_action(cx.listener(|v, _: &JumpToLineNumber, _, cx| v.jump_to_line_number(cx)))
            .on_key_down(cx.listener(|v, event, _, cx| v.on_key_down(event, cx)))
            .child(LogElement { view: cx.entity() })
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
    number: ShapedLine,
    text: ShapedLine,
    /// Selected columns, relative to the first visible column.
    highlight: Option<(usize, usize)>,
    /// Quick-find occurrences, relative to the first visible column.
    found: Vec<(usize, usize)>,
    /// Search pattern matches, relative to the first visible column.
    searched: Vec<(usize, usize)>,
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
}

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
            let gutter_width = BULLET_WIDTH + metrics.char_width * digits + GUTTER_PAD * 2.;
            let gutter = Bounds::new(
                bounds.origin,
                size(gutter_width, bounds.size.height - SCROLLBAR),
            );
            let bullets = Bounds::new(
                bounds.origin,
                size(BULLET_WIDTH, bounds.size.height - SCROLLBAR),
            );
            let with_overview = !view.filtered && view.filter.is_some();
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
            view.viewport.set_size(line_count, rows, cols);

            // Visible rows: decode and shape only the visible columns.
            let mut out = Vec::new();
            if let Some(log) = view.log.clone() {
                let vp = view.viewport;
                let visible = vp.visible_lines();
                // (row, file line, content) of the visible rows.
                let shown: Vec<(usize, usize, &[u8])> = if view.filtered {
                    visible
                        .filter_map(|row| {
                            let line = view.line_of(row, cx)?;
                            Some((row, line, log.line(line)?))
                        })
                        .collect()
                } else {
                    let first = visible.start;
                    log.lines(visible)
                        .enumerate()
                        .map(|(i, bytes)| (first + i, first + i, bytes))
                        .collect()
                };
                let filter = view.filter.as_ref().map(|f| f.read(cx));
                for (row, line, bytes) in shown {
                    let kind = filter.map_or_else(LineKind::default, |f| f.kind(line));
                    let win = text::window(log.encoding(), bytes, vp.left(), vp.cols() + 1);
                    let seen = win
                        .total_cols
                        .unwrap_or(vp.left() + win.text.chars().count() + 1);
                    view.viewport.grow_max_cols(seen);
                    let highlight = if view.selection.is_line_selected(row) {
                        Some((0, vp.cols() + 1))
                    } else {
                        view.selection.portion_of(row).and_then(|cols| {
                            let start = (*cols.start()).max(vp.left());
                            let end = (*cols.end() + 1).min(vp.left() + vp.cols() + 1);
                            (start < end).then(|| (start - vp.left(), end - vp.left()))
                        })
                    };
                    let window_end = vp.left() + vp.cols() + 1;
                    // Visible columns of a matcher's matches in the line.
                    let visible_matches = |m: &Matcher| -> Vec<(usize, usize)> {
                        find::match_columns(&log, m, line)
                            .into_iter()
                            .filter_map(|c| {
                                let start = c.start.max(vp.left());
                                let end = c.end.min(window_end);
                                (start < end).then(|| (start - vp.left(), end - vp.left()))
                            })
                            .collect()
                    };
                    let short = bytes.len() <= MAX_HIGHLIGHT_LINE;
                    let found = match &view.quick_find {
                        Some(m) if short => visible_matches(m),
                        _ => Vec::new(),
                    };
                    // Only matched lines can hold search matches.
                    let searched = match &view.search_highlight {
                        Some(m) if short && kind.matched => visible_matches(m),
                        _ => Vec::new(),
                    };
                    let y =
                        text_bounds.top() + metrics.line_height * (row as f64 - vp.top()) as f32;
                    out.push(Row {
                        y,
                        number: metrics.shape(window, (line + 1).to_string().into(), muted),
                        text: metrics.shape(window, win.text.into(), fg),
                        highlight,
                        found,
                        searched,
                        kind,
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
            let total_cols = vp.max_cols().max(vp.cols()).max(1) as f32;
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
                Some(OverviewFrame {
                    ticks,
                    view: (top, bottom.min(height.saturating_sub(1))),
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
        let found_bg = theme.yellow.opacity(0.45);
        let fg = theme.foreground;
        // klogg's default `mainSearchBackColor` is light gray; a translucent
        // foreground gives a gray in light and dark themes alike.
        let search_bg = theme.foreground.opacity(0.18);
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
                    if let Some((from, to)) = row.highlight {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window
                            .paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), highlight));
                    }
                    paint_bullet(window, &layout, row, &bullet_colors);
                    let x = frame.gutter.right() - GUTTER_PAD - row.number.width;
                    row.number
                        .paint(point(x, row.y), lh, TextAlign::Left, None, window, cx)
                        .ok();
                }
            },
        );
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: layout.text,
            }),
            |window| {
                for row in &frame.rows {
                    row.text
                        .paint(
                            point(layout.text.left(), row.y),
                            lh,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                }
            },
        );

        if let (Some(bar), Some(o)) = (layout.overview, &frame.overview) {
            paint_overview(window, bar, o, &bullet_colors, gutter_bg, border, fg);
        }

        window.paint_quad(fill(layout.vtrack, track));
        window.paint_quad(fill(layout.htrack, track));
        window.paint_quad(fill(layout.vthumb, thumb));
        window.paint_quad(fill(layout.hthumb, thumb));

        if frame.hitbox.is_hovered(window) {
            let over_text = window.mouse_position();
            let style = if layout.text.contains(&over_text) {
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
    use std::io::Write;

    fn temp_log(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rlogg-view-{}-{name}", std::process::id()));
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

        cx.simulate_keystrokes("ctrl-c");
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
        cx.simulate_keystrokes("shift-down ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a\tb\nsecond")
        );
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("1: a\tb\n2: second")
        );
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a\tb\nsecond\nthird")
        );
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
}
