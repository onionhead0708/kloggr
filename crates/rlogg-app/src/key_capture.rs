//! "Press new key combination" (klogg: `KeySequencePresenter::showEditor`,
//! a `QKeySequenceEdit` with Clear, Cancel and OK).
//!
//! While the key box has focus, every key combination goes to it instead of
//! the app's shortcuts: a keystroke interceptor sees keys before GPUI
//! matches bindings. Escape alone still cancels the dialog.

use std::cell::Cell;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    Keystroke, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window,
    div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};

use crate::shortcuts;

/// The dialog's result: the key, or `None` when cleared.
pub struct Done(pub Option<String>);

impl EventEmitter<Done> for KeyCapture {}

pub struct KeyCapture {
    focus: FocusHandle,
    key: Option<String>,
    _intercept: Subscription,
}

/// Modifier keys alone are reported as keys named like this.
const MODIFIERS: [&str; 5] = ["shift", "control", "alt", "platform", "function"];

impl KeyCapture {
    pub fn new(key: Option<String>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let this = cx.entity().downgrade();
        let box_focus = focus.clone();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if !box_focus.is_focused(window) {
                return;
            }
            let stroke = &event.keystroke;
            let m = stroke.modifiers;
            if MODIFIERS.contains(&stroke.key.as_str()) {
                return;
            }
            // Escape alone cancels, as in a dialog.
            if stroke.key == "escape" && !(m.control || m.alt || m.shift || m.platform) {
                return;
            }
            let key = canonical(stroke);
            this.update(cx, |this, cx| {
                this.key = Some(key);
                cx.notify();
            })
            .ok();
            cx.stop_propagation();
        });
        Self {
            focus,
            key,
            _intercept: intercept,
        }
    }

    #[cfg(test)]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
}

/// The keystroke as a binding: on Linux GPUI reports Ctrl+Shift+1 as
/// `ctrl-!`; store it as `ctrl-shift-1`, as klogg shows it (the symbol is
/// bound too, see [`shortcuts`]).
fn canonical(stroke: &Keystroke) -> String {
    const US_SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];
    let m = stroke.modifiers;
    let mut stroke = stroke.clone();
    stroke.key_char = None;
    if !m.shift
        && (m.control || m.alt || m.platform)
        && let Some(digit) = stroke
            .key
            .chars()
            .next()
            .filter(|_| stroke.key.chars().count() == 1)
            .and_then(|c| US_SHIFTED_DIGITS.iter().position(|d| *d == c))
    {
        stroke.modifiers.shift = true;
        stroke.key = digit.to_string();
    }
    stroke.unparse()
}

impl Render for KeyCapture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let focused = self.focus.is_focused(window);
        let border = if focused { theme.ring } else { theme.input };
        let (muted, fg) = (theme.muted_foreground, theme.foreground);
        let shown = self.key.as_deref().map(shortcuts::display);
        v_flex()
            .gap_2()
            .child(div().text_sm().child("Press new key combination"))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("key-box")
                            .track_focus(&self.focus)
                            .flex_1()
                            .h(px(30.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .text_color(if shown.is_some() { fg } else { muted })
                            .child(shown.unwrap_or_else(|| "none".into()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.focus, cx);
                            })),
                    )
                    .child(
                        Button::new("key-clear")
                            .label("Clear")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.key = None;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("key-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("key-ok")
                            .label("OK")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.emit(Done(this.key.clone()));
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}

/// Opens the dialog over the current one; `done` gets the new key when the
/// user presses OK. Keep the returned entity until then: closing the dialog
/// drops its own handle before the result is delivered.
pub fn open(
    key: Option<String>,
    window: &mut Window,
    cx: &mut App,
    done: impl Fn(Option<String>, &mut Window, &mut App) + 'static,
) -> Entity<KeyCapture> {
    let capture = cx.new(|cx| KeyCapture::new(key, cx));
    window
        .subscribe(&capture, cx, move |_, Done(key), window, cx| {
            done(key.clone(), window, cx)
        })
        .detach();
    let first = Cell::new(true);
    let dialog_capture = capture.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let focus = dialog_capture.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        dialog
            .title("Shortcut")
            .w(px(380.))
            .child(dialog_capture.clone())
    });
    capture
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{KeyBinding, TestAppContext, VisualTestContext, actions};
    use std::cell::RefCell;
    use std::rc::Rc;

    actions!(key_capture_test, [CloseFile]);

    /// A window whose root handles Ctrl+W, to check that the dialog takes it.
    struct Host {
        closed: Rc<Cell<bool>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let closed = self.closed.clone();
            div()
                .size_full()
                .on_action(cx.listener(move |_, _: &CloseFile, _, _| closed.set(true)))
        }
    }

    #[gpui::test]
    fn records_keys_before_shortcuts(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([KeyBinding::new("ctrl-w", CloseFile, None)]);
        });
        let closed = Rc::new(Cell::new(false));
        let window = cx.add_window({
            let closed = closed.clone();
            move |window, cx| {
                let host = cx.new(|_| Host { closed });
                gpui_component::Root::new(host, window, cx)
            }
        });
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let result = Rc::new(RefCell::new(None));
        let capture = cx.update(|window, cx| {
            let result = result.clone();
            open(Some("m".into()), window, cx, move |key, _, _| {
                *result.borrow_mut() = Some(key)
            })
        });
        cx.run_until_parked();
        assert_eq!(
            capture.read_with(cx, |c, _| c.key().map(str::to_owned)),
            Some("m".into())
        );

        cx.simulate_keystrokes("ctrl-w");
        assert!(!closed.get(), "the key went to the dialog, not the app");
        assert_eq!(
            capture.read_with(cx, |c, _| c.key().map(str::to_owned)),
            Some("ctrl-w".into())
        );
        // Modifiers alone don't count; the last combination wins.
        cx.simulate_keystrokes("ctrl-shift-f3");
        assert_eq!(
            capture.read_with(cx, |c, _| c.key().map(str::to_owned)),
            Some("ctrl-shift-f3".into())
        );
        capture.update_in(cx, |c, window, cx| {
            cx.emit(Done(c.key.clone()));
            window.close_dialog(cx);
        });
        assert_eq!(*result.borrow(), Some(Some("ctrl-shift-f3".into())));
    }

    #[test]
    fn linux_shifted_digits_become_shift_digit() {
        let k = |s: &str| canonical(&Keystroke::parse(s).unwrap());
        assert_eq!(k("ctrl-!"), "ctrl-shift-1");
        assert_eq!(k("ctrl-)"), "ctrl-shift-0");
        assert_eq!(k("alt-("), "alt-shift-9");
        // Without Ctrl, Alt or Cmd, a symbol is typed as such.
        assert_eq!(k("!"), "!");
        assert_eq!(k("ctrl-shift-a"), "ctrl-shift-a");
    }
}
