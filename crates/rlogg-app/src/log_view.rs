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

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, Entity, FocusHandle,
    Focusable, Font, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollWheelEvent,
    ShapedLine, SharedString, Style, Styled, TextAlign, TextRun, Window, actions, div, fill, point,
    px, relative, size,
};
use gpui_component::ActiveTheme;
use rlogg_core::text;
use rlogg_core::{Drag, LogFile, Position, Selection, Viewport};

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
    ]
);

const CONTEXT: &str = "LogView";

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
    ]
}

const SCROLLBAR: Pixels = px(12.);
const MIN_THUMB: Pixels = px(24.);
const GUTTER_PAD: Pixels = px(8.);

/// Geometry of the last frame, used to map mouse positions.
#[derive(Debug, Clone, Copy)]
struct Layout {
    text: Bounds<Pixels>,
    line_height: Pixels,
    char_width: Pixels,
    vtrack: Bounds<Pixels>,
    vthumb: Bounds<Pixels>,
    htrack: Bounds<Pixels>,
    hthumb: Bounds<Pixels>,
}

#[derive(Debug, Clone, Copy)]
enum MouseGrab {
    Text(Drag),
    /// Dragging a scrollbar thumb; offset of the grab point in the thumb.
    VThumb(Pixels),
    HThumb(Pixels),
}

pub struct LogView {
    focus: FocusHandle,
    log: Option<LogFile>,
    selection: Selection,
    /// Line moved to by the keyboard (klogg: `selectionCurrentEndPos_`).
    cursor: usize,
    viewport: Viewport,
    layout: Option<Layout>,
    grab: Option<MouseGrab>,
    /// Sub-column horizontal wheel motion not applied yet.
    hscroll_carry: Pixels,
}

impl LogView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            log: None,
            selection: Selection::None,
            cursor: 0,
            viewport: Viewport::default(),
            layout: None,
            grab: None,
            hscroll_carry: px(0.),
        }
    }

    pub fn set_log(&mut self, log: Option<LogFile>, cx: &mut Context<Self>) {
        self.viewport = Viewport::default();
        if let Some(log) = &log {
            self.viewport
                .grow_max_cols(log.index().max_line_len() as usize);
        }
        self.log = log;
        self.selection = Selection::None;
        self.cursor = 0;
        self.grab = None;
        cx.notify();
    }

    fn line_count(&self) -> usize {
        self.log.as_ref().map_or(0, LogFile::line_count)
    }

    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if selection != self.selection {
            self.selection = selection;
            cx.notify();
        }
    }

    /// Selects `line` and scrolls to it (klogg: `selectAndDisplayLine`).
    fn select_and_reveal(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(last) = self.line_count().checked_sub(1) else {
            return;
        };
        let line = line.min(last);
        self.cursor = line;
        self.select(Selection::Line(line), cx);
        self.viewport.reveal_line(line);
        cx.notify();
    }

    fn move_selection(&mut self, up: bool, cx: &mut Context<Self>) {
        let line = match self.selection.lines() {
            None => self.viewport.first_line(),
            Some(lines) if up => lines.start().saturating_sub(1),
            Some(lines) => lines.end() + 1,
        };
        self.select_and_reveal(line, cx);
    }

    fn extend_selection(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.line_count() == 0 {
            return;
        }
        let line = if up {
            self.cursor.saturating_sub(1)
        } else {
            (self.cursor + 1).min(self.line_count() - 1)
        };
        self.cursor = line;
        self.select(self.selection.extended_to(line), cx);
        self.viewport.reveal_line(line);
        cx.notify();
    }

    fn copy_selection(&self, line_numbers: bool, cx: &mut Context<Self>) {
        if let Some(log) = &self.log
            && !self.selection.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.selection.text(log, line_numbers),
            ));
        }
    }

    /// X11/Wayland primary selection (klogg: `updateGlobalSelection`).
    fn update_primary(&self, cx: &mut Context<Self>) {
        if let Some(log) = &self.log
            && !self.selection.is_empty()
        {
            cx.write_to_primary(ClipboardItem::new_string(self.selection.text(log, false)));
        }
    }

    /// Display columns of `line` (expanded), longest of the selection.
    fn selection_width(&self) -> usize {
        let (Some(log), Some(lines)) = (&self.log, self.selection.lines()) else {
            return 0;
        };
        log.lines_text(*lines.start()..lines.end() + 1)
            .map(|l| text::columns(&l))
            .max()
            .unwrap_or(0)
    }

    fn visible_width(&self) -> usize {
        let Some(log) = &self.log else { return 0 };
        log.lines_text(self.viewport.visible_lines())
            .map(|l| text::columns(&l))
            .max()
            .unwrap_or(0)
    }

    fn position_at(&self, at: Point<Pixels>, layout: &Layout) -> Option<Position> {
        let last = self.line_count().checked_sub(1)?;
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
        if layout.vtrack.contains(&at) {
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
        } else if let Some(pos) = self.position_at(at, &layout) {
            self.cursor = pos.line;
            if event.modifiers.shift {
                self.select(self.selection.extended_to(pos.line), cx);
            } else if event.click_count == 2 {
                if let Some(word) = self
                    .log
                    .as_ref()
                    .and_then(|log| log.line_text(pos.line))
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
                if let Some(pos) = self.position_at(clamped, &layout) {
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
                let width = v.selection_width();
                v.viewport.grow_max_cols(width);
                v.viewport.set_left(width.saturating_sub(v.viewport.cols()));
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &RightOfScreen, _, cx| {
                let width = v.visible_width();
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
                if let Some(last) = v.line_count().checked_sub(1) {
                    v.select(Selection::range(0, last), cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Copy, _, cx| v.copy_selection(false, cx)))
            .on_action(cx.listener(|v, _: &CopyWithLineNumbers, _, cx| v.copy_selection(true, cx)))
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
}

pub struct Frame {
    hitbox: Hitbox,
    layout: Layout,
    gutter: Bounds<Pixels>,
    rows: Vec<Row>,
    focused: bool,
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

        self.view.update(cx, |view, _| {
            let line_count = view.line_count();
            let digits = line_count.max(1).to_string().len().max(3);
            let gutter_width = metrics.char_width * digits + GUTTER_PAD * 2.;
            let gutter = Bounds::new(
                bounds.origin,
                size(gutter_width, bounds.size.height - SCROLLBAR),
            );
            let text_bounds = Bounds::from_corners(
                point(gutter.right() + GUTTER_PAD / 2., bounds.top()),
                point(bounds.right() - SCROLLBAR, bounds.bottom() - SCROLLBAR),
            );
            let rows = f64::from(text_bounds.size.height / metrics.line_height);
            let cols = (text_bounds.size.width / metrics.char_width).max(0.) as usize;
            view.viewport.set_size(line_count, rows, cols);

            // Visible rows: decode and shape only the visible columns.
            let mut out = Vec::new();
            if let Some(log) = &view.log {
                let vp = view.viewport;
                let visible = vp.visible_lines();
                let first = visible.start;
                for (i, bytes) in log.lines(visible).enumerate() {
                    let line = first + i;
                    let win = text::window(log.encoding(), bytes, vp.left(), vp.cols() + 1);
                    let seen = win
                        .total_cols
                        .unwrap_or(vp.left() + win.text.chars().count() + 1);
                    view.viewport.grow_max_cols(seen);
                    let highlight = if view.selection.is_line_selected(line) {
                        Some((0, vp.cols() + 1))
                    } else {
                        view.selection.portion_of(line).and_then(|cols| {
                            let start = (*cols.start()).max(vp.left());
                            let end = (*cols.end() + 1).min(vp.left() + vp.cols() + 1);
                            (start < end).then(|| (start - vp.left(), end - vp.left()))
                        })
                    };
                    let y =
                        text_bounds.top() + metrics.line_height * (line as f64 - vp.top()) as f32;
                    out.push(Row {
                        y,
                        number: metrics.shape(window, (line + 1).to_string().into(), muted),
                        text: metrics.shape(window, win.text.into(), fg),
                        highlight,
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
                line_height: metrics.line_height,
                char_width: metrics.char_width,
                vtrack,
                vthumb,
                htrack,
                hthumb,
            };
            view.layout = Some(layout);
            Frame {
                hitbox,
                layout,
                gutter,
                rows: out,
                focused,
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
        let layout = frame.layout;
        let lh = layout.line_height;

        window.paint_quad(fill(bounds, bg));
        window.paint_quad(fill(frame.gutter, gutter_bg));
        window.paint_quad(fill(
            Bounds::new(
                point(frame.gutter.right(), bounds.top()),
                size(px(1.), frame.gutter.size.height),
            ),
            border,
        ));

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
                    if let Some((from, to)) = row.highlight {
                        let x = layout.text.left() + layout.char_width * from;
                        let w = layout.char_width * (to - from);
                        window
                            .paint_quad(fill(Bounds::new(point(x, row.y), size(w, lh)), highlight));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, ScrollDelta, TestAppContext, VisualTestContext};
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
            v.set_log(Some(log), cx);
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
