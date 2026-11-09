//! Jump to line box, Ctrl+L (klogg: `CrawlerWidget::goToLine`, which asks
//! for the number in a `QInputDialog`).
//!
//! A small bar at the bottom of the crawler, like the quick-find bar. Enter
//! selects the line in the main view and the closest row in the filtered
//! view; Esc closes it. Numbers are 1-based; 0 means line 1 and numbers past
//! the end select the last line, as in klogg.

use gpui::{
    AppContext, Context, Entity, Focusable, IntoElement, KeyBinding, ParentElement, Styled, Window,
    actions, div,
};
use gpui_component::ActiveTheme;
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};

use crate::crawler::{self, Crawler};

actions!(go_to_line, [Open]);

/// klogg's default (`LogViewJumpToLine`).
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("secondary-l", Open, Some(crawler::CONTEXT))]
}

pub struct GoToLine {
    pub open: bool,
    pub input: Entity<InputState>,
    /// The message for invalid input, and the input it is about.
    pub error: Option<String>,
    failed_text: String,
}

impl GoToLine {
    pub fn new(window: &mut Window, cx: &mut Context<Crawler>) -> Self {
        Self {
            open: false,
            input: cx.new(|cx| InputState::new(window, cx).placeholder("Line number")),
            error: None,
            failed_text: String::new(),
        }
    }
}

/// The 0-based line for what was typed, or an error message.
pub fn parse_line(text: &str) -> Result<usize, &'static str> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Not a line number");
    }
    // Too large to parse: past the end anyway.
    let number: u64 = text.parse().unwrap_or(u64::MAX);
    Ok(usize::try_from(number.saturating_sub(1)).unwrap_or(usize::MAX))
}

impl Crawler {
    pub fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.log.is_none() {
            return;
        }
        self.update_target(window, cx);
        self.go_to.open = true;
        self.go_to.error = None;
        let input = self.go_to.input.clone();
        input.update(cx, |state, cx| state.set_value("", window, cx));
        window.focus(&input.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn close_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to.open = false;
        self.go_to.error = None;
        let focus = self.target_view(self.target).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Enter in the box.
    pub fn submit_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.go_to.input.read(cx).value().to_string();
        match parse_line(&text) {
            Ok(line) => {
                self.go_to_line(line, cx);
                self.close_go_to_line(window, cx);
            }
            Err(message) => {
                self.go_to.error = Some(message.to_owned());
                self.go_to.failed_text = text;
                cx.notify();
            }
        }
    }

    /// The input changed: drops the error once the text differs from the
    /// one it was about (the input also reports a change on Enter).
    pub fn go_to_line_edited(&mut self, cx: &mut Context<Self>) {
        if self.go_to.error.is_some()
            && *self.go_to.input.read(cx).value() != self.go_to.failed_text
        {
            self.go_to.error = None;
            cx.notify();
        }
    }

    /// Selects `line` (clamped to the file) in the main view and the closest
    /// row in the filtered view (klogg: `goToLine`).
    pub fn go_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(last) = self
            .log
            .as_ref()
            .and_then(|l| l.line_count().checked_sub(1))
        else {
            return;
        };
        let line = line.min(last);
        self.current_line = line;
        self.main.update(cx, |v, cx| v.show_line(line, cx));
        self.filtered.update(cx, |v, cx| v.show_line(line, cx));
    }

    pub(crate) fn render_go_to_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted_fg, border) = (theme.danger, theme.muted_foreground, theme.border);
        let count = self.log.as_ref().map_or(0, |l| l.line_count());
        let (hint, color) = match &self.go_to.error {
            Some(e) => (e.clone(), danger),
            None => (format!("1 – {count}"), muted_fg),
        };
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
                Button::new("go-to-close")
                    .label("×")
                    .tooltip("Close (Esc)")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.close_go_to_line(window, cx))),
            )
            .child(div().px_1().text_sm().child("Jump to line"))
            .child(div().w_40().child(Input::new(&self.go_to.input).small()))
            .child(div().px_2().text_sm().text_color(color).child(hint))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_like_klogg() {
        assert_eq!(parse_line("1"), Ok(0));
        assert_eq!(parse_line(" 42 "), Ok(41));
        assert_eq!(parse_line("0"), Ok(0));
        // Past the end: clamped when jumping.
        assert!(parse_line("99999999999999999999999").unwrap() > 1 << 40);
        assert!(parse_line("").is_err());
        assert!(parse_line("12a").is_err());
        assert!(parse_line("-3").is_err());
    }
}
