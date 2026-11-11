//! Switch to an open file, Ctrl+Shift+O (klogg: `MainWindow::selectOpenedFile`,
//! a dialog listing the open files' paths with a wildcard filter).
//!
//! Typing filters the list (`*` and `?` are wildcards, case is ignored, the
//! text may match anywhere in the path); Up and Down move the selection,
//! Enter or a click shows the file's tab, Esc closes.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};

/// The tab chosen, by index.
pub struct Chosen(pub usize);

pub struct FileSwitcher {
    files: Vec<PathBuf>,
    input: Entity<InputState>,
    /// Indexes in `files` that match the filter.
    pub(crate) shown: Vec<usize>,
    /// Index in `shown`.
    pub(crate) selected: usize,
    _subscription: Subscription,
}

impl EventEmitter<Chosen> for FileSwitcher {}

impl Focusable for FileSwitcher {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl FileSwitcher {
    pub fn new(files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filter(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.choose(window, cx),
                _ => {}
            },
        );
        let shown = (0..files.len()).collect();
        Self {
            files,
            input,
            shown,
            selected: 0,
            _subscription: subscription,
        }
    }

    fn filter(&mut self, cx: &App) {
        let pattern = self.input.read(cx).value().to_lowercase();
        self.shown = (0..self.files.len())
            .filter(|&i| {
                let path = self.files[i].display().to_string().to_lowercase();
                wildcard_match(&pattern, &path)
            })
            .collect();
        // klogg selects the first match after each edit.
        self.selected = 0;
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&index) = self.shown.get(self.selected) {
            cx.emit(Chosen(index));
            window.close_dialog(cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let last = self.shown.len().saturating_sub(1);
        match event.keystroke.key.as_str() {
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(last),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for FileSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, selected_bg, muted) =
            (theme.border, theme.list_active, theme.muted_foreground);
        let rows = self.shown.iter().enumerate().map(|(row, &index)| {
            div()
                .id(("open-file", index))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .when(row == self.selected, |d| d.bg(selected_bg))
                .child(self.files[index].display().to_string())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.selected = row;
                    this.choose(window, cx);
                }))
        });
        let empty = self.shown.is_empty();
        v_flex()
            .gap_2()
            .capture_key_down(cx.listener(|this, event, _, cx| this.on_key_down(event, cx)))
            .child(Input::new(&self.input).small())
            .child(
                v_flex()
                    .id("open-files")
                    .h(px(360.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_1()
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p_2()
                                .text_sm()
                                .text_color(muted)
                                .child("No open file matches"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("switch-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("switch-ok")
                            .label("Switch")
                            .small()
                            .primary()
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                    ),
            )
    }
}

/// Whether `text` contains a match of `pattern`, where `*` stands for any
/// run of characters and `?` for one (Qt's unanchored wildcard filter).
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // Unanchored: as if the pattern were `*pattern*`.
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = Some((0, 0));
    loop {
        if pi == p.len() {
            return true;
        }
        if ti < t.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if p[pi] == '*' {
            pi += 1;
            star = Some((pi, ti));
        } else if let Some((sp, st)) = star
            && st < t.len()
        {
            pi = sp;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
}

/// Shows the switcher; `chosen` gets the tab index. Keep the returned
/// entity until then (see `key_capture::open`).
pub fn open(
    files: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut App,
    chosen: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Entity<FileSwitcher> {
    let switcher = cx.new(|cx| FileSwitcher::new(files, window, cx));
    window
        .subscribe(&switcher, cx, move |_, Chosen(index), window, cx| {
            chosen(*index, window, cx)
        })
        .detach();
    let dialog_switcher = switcher.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Switch to file")
            .w(px(800.))
            .child(dialog_switcher.clone())
    });
    let focus = switcher.focus_handle(cx);
    window.focus(&focus, cx);
    switcher
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("", "/var/log/a.log"));
        assert!(wildcard_match("log/a", "/var/log/a.log"));
        assert!(wildcard_match("var*a.log", "/var/log/a.log"));
        assert!(wildcard_match("a.?og", "/var/log/a.log"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("b.log", "/var/log/a.log"));
        assert!(!wildcard_match("a.log?", "/var/log/a.log"));
        assert!(
            wildcard_match("aab", "xaaab"),
            "backtracks after a partial match"
        );
    }
}
