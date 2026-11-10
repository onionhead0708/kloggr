//! The scratchpad (klogg: `TabbedScratchPad`, `ScratchPad`): a separate
//! window with tabs of free text, buttons that transform the text (or the
//! selected part) in place, and read-outs of it. The log views send their
//! selection to it (klogg: Ctrl+Z, Ctrl+Shift+Z).

use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, Focusable, Global, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, SharedString, Styled, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, actions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, Root, Sizable, TitleBar, h_flex, v_flex};
use kloggr_core::scratch::{self, Readouts};

actions!(scratchpad, [NewTab]);

const CONTEXT: &str = "Scratchpad";

/// klogg: Ctrl+N adds a tab.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("secondary-n", NewTab, Some(CONTEXT))]
}

struct PadTab {
    name: String,
    text: Entity<TextareaState>,
}

pub struct Scratchpad {
    tabs: Vec<PadTab>,
    active: usize,
    /// klogg: `tabCounter_`, for "Scratchpad N".
    counter: usize,
    /// klogg's status bar: "Copied to clipboard", "Empty transformation".
    status: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl Scratchpad {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            counter: 0,
            status: None,
            _subscriptions: Vec::new(),
        };
        this.add_tab(window, cx);
        this
    }

    pub fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.counter += 1;
        let text = cx.new(|cx| TextareaState::new(window, cx));
        // The read-outs follow the text.
        self._subscriptions
            .push(cx.subscribe(&text, |this, _, _: &InputEvent, cx| {
                this.status = None;
                cx.notify();
            }));
        self.tabs.push(PadTab {
            name: format!("Scratchpad {}", self.counter),
            text,
        });
        self.active = self.tabs.len() - 1;
        let focus = self.tabs[self.active].text.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
        }
        if self.tabs.is_empty() {
            self.add_tab(window, cx);
        }
        self.active = self.active.min(self.tabs.len() - 1);
        cx.notify();
    }

    fn current(&self) -> &Entity<TextareaState> {
        &self.tabs[self.active].text
    }

    /// The selected text, or all of it (klogg: `transformText`).
    fn subject(&self, cx: &App) -> String {
        let state = self.current().read(cx);
        let value = state.value();
        let range = state.selected_range();
        match value.get(range) {
            Some(selected) if !selected.is_empty() => selected.to_owned(),
            _ => value.to_string(),
        }
    }

    /// Replaces the selection, or all the text, with `f` of it, and copies
    /// the result (klogg: `transformTextInPlace`).
    pub fn transform(&mut self, f: Transform, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.current().clone();
        let selected = !state.read(cx).selected_range().is_empty();
        match f(&self.subject(cx)) {
            Some(result) => {
                state.update(cx, |s, cx| {
                    if selected {
                        s.replace(result.clone(), window, cx);
                    } else {
                        s.set_value(result.clone(), window, cx);
                    }
                });
                cx.write_to_clipboard(ClipboardItem::new_string(result));
                self.status = Some("Copied to clipboard");
            }
            None => self.status = Some("Empty transformation"),
        }
        cx.notify();
    }

    /// klogg: `addData` (appended as a new line).
    pub fn add_data(&mut self, data: &str, window: &mut Window, cx: &mut Context<Self>) {
        if data.is_empty() {
            return;
        }
        self.current().clone().update(cx, |s, cx| {
            let old = s.value().to_string();
            let new = if old.is_empty() {
                data.to_owned()
            } else {
                format!("{old}\n{data}")
            };
            s.set_value(new, window, cx);
        });
        cx.notify();
    }

    /// klogg: `replaceData`.
    pub fn replace_data(&mut self, data: &str, window: &mut Window, cx: &mut Context<Self>) {
        if data.is_empty() {
            return;
        }
        let data = data.to_owned();
        self.current()
            .clone()
            .update(cx, |s, cx| s.set_value(data, window, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub fn text(&self, cx: &App) -> String {
        self.current().read(cx).value().to_string()
    }
}

/// A transformation of the text; `None` when it doesn't apply.
type Transform = fn(&str) -> Option<String>;

/// The toolbar's transformations (klogg's order).
const TRANSFORMS: [(&str, Transform); 7] = [
    ("From base64", scratch::from_base64),
    ("To base64", scratch::to_base64),
    ("From hex", scratch::from_hex),
    ("To hex", scratch::to_hex),
    ("Decode url", scratch::decode_url),
    ("Format json", scratch::format_json),
    ("Format xml", scratch::format_xml),
];

impl Render for Scratchpad {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted) = (theme.background, theme.border, theme.muted_foreground);
        let readouts = Readouts::of(&self.subject(cx));
        let toolbar = h_flex()
            .gap_1()
            .flex_wrap()
            .children(TRANSFORMS.iter().enumerate().map(|(i, (label, f))| {
                let f = *f;
                Button::new(("transform", i))
                    .label(*label)
                    .small()
                    .ghost()
                    .on_click(cx.listener(move |this, _, window, cx| this.transform(f, window, cx)))
            }));
        let readout = |label: &'static str, value: String| {
            v_flex()
                .gap_0p5()
                .child(div().text_xs().text_color(muted).child(label))
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .min_h(px(28.))
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .text_sm()
                        .child(value),
                )
        };
        let tabs = TabBar::new("scratch-tabs")
            .small()
            .selected_index(self.active)
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                Tab::new().label(tab.name.clone()).suffix(
                    Button::new(("scratch-close", i))
                        .label("×")
                        .xsmall()
                        .ghost()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.close_tab(i, window, cx)),
                        ),
                )
            }))
            .suffix(
                Button::new("scratch-add")
                    .label("+")
                    .xsmall()
                    .ghost()
                    .tooltip("New tab (Ctrl+N)")
                    .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx))),
            )
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                this.active = *index;
                cx.notify();
            }));
        let body = v_flex()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(tabs)
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .child(Textarea::new(self.current()).h_full()),
                    )
                    .child(
                        v_flex()
                            .w(px(260.))
                            .gap_2()
                            .child(readout("CRC32 hex", readouts.crc32_hex))
                            .child(readout("CRC32 dec", readouts.crc32_dec))
                            .child(readout("File time (UTC)", readouts.file_time))
                            .child(readout("Dec->Hex", readouts.dec_to_hex))
                            .child(readout("Hex->Dec", readouts.hex_to_dec))
                            .child(readout("Time (UTC)", readouts.time)),
                    ),
            )
            .child(
                div()
                    .h(px(18.))
                    .text_xs()
                    .text_color(muted)
                    .children(self.status.map(SharedString::from)),
            );
        // GNOME (Wayland) draws no decorations: the title bar is ours.
        v_flex()
            .key_context(CONTEXT)
            .size_full()
            .bg(bg)
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.add_tab(window, cx)))
            .child(TitleBar::new().child(div().flex_1().text_sm().child("kloggr - scratchpad")))
            .child(body)
    }
}

/// The scratchpad window, opened once and then shown again (klogg: the
/// main window's `scratchPad_`).
#[derive(Default)]
struct ScratchpadWindow {
    window: Option<(WindowHandle<Root>, Entity<Scratchpad>)>,
}

impl Global for ScratchpadWindow {}

/// Whether `window` is the scratchpad's (it doesn't keep the app open).
pub fn is_scratchpad_window(window: gpui::AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<ScratchpadWindow>()
        .and_then(|s| s.window.as_ref())
        .is_some_and(|(handle, _)| window == (*handle).into())
}

/// Shows the scratchpad, opening its window if needed, and runs `f` on it
/// (klogg: `showScratchPad`).
pub fn show(
    cx: &mut App,
    f: impl FnOnce(&mut Scratchpad, &mut Window, &mut Context<Scratchpad>) + 'static,
) {
    // Not from inside another window's update.
    cx.defer(move |cx| {
        let existing = cx
            .try_global::<ScratchpadWindow>()
            .and_then(|s| s.window.clone());
        let (handle, pad) = match existing {
            Some((handle, pad)) if handle.update(cx, |_, _, _| ()).is_ok() => (handle, pad),
            _ => {
                let mut options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(980.), px(640.)),
                        cx,
                    ))),
                    app_id: Some(crate::APP_ID.into()),
                    ..crate::window_base_options()
                };
                if !cfg!(target_os = "linux") {
                    options.titlebar = Some(TitlebarOptions {
                        title: Some("kloggr - scratchpad".into()),
                        ..Default::default()
                    });
                }
                let mut pad = None;
                let Ok(handle) = cx.open_window(options, |window, cx| {
                    let p = cx.new(|cx| Scratchpad::new(window, cx));
                    pad = Some(p.clone());
                    cx.new(|cx| Root::new(p, window, cx))
                }) else {
                    return;
                };
                let pad = pad.expect("built");
                cx.set_global(ScratchpadWindow {
                    window: Some((handle, pad.clone())),
                });
                (handle, pad)
            }
        };
        handle
            .update(cx, |_, window, cx| {
                window.activate_window();
                pad.update(cx, |p, cx| f(p, window, cx));
            })
            .ok();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn transforms_and_data(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (pad, cx): (Entity<Scratchpad>, &mut VisualTestContext) =
            cx.add_window_view(Scratchpad::new);
        pad.update_in(cx, |p, window, cx| {
            p.replace_data("aGVsbG8=", window, cx);
            p.transform(scratch::from_base64, window, cx);
            assert_eq!(p.text(cx), "hello");
            assert_eq!(p.status, Some("Copied to clipboard"));
            p.add_data("world", window, cx);
            assert_eq!(p.text(cx), "hello\nworld");
            p.transform(scratch::format_json, window, cx);
            assert_eq!(p.status, Some("Empty transformation"));
            assert_eq!(p.text(cx), "hello\nworld", "unchanged");
            // A new tab starts empty; closing the last tab leaves a new one.
            p.add_tab(window, cx);
            assert_eq!(p.text(cx), "");
            assert_eq!(p.tabs[1].name, "Scratchpad 2");
            p.close_tab(1, window, cx);
            p.close_tab(0, window, cx);
            assert_eq!(p.tabs.len(), 1);
            assert_eq!(p.tabs[0].name, "Scratchpad 3");
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some("hello".into())
        );
    }
}
