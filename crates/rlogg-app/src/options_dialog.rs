//! The options dialog (klogg: `OptionsDialog`, Tools > Options…): General,
//! View, File and Shortcuts tabs, with OK, Apply and Cancel.
//!
//! Edits go to a copy of the options; Apply and OK install it through
//! [`Preferences::set`].

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputState};
use gpui_component::radio::{Radio, RadioGroup};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};
use rlogg_core::highlight::Color;
use rlogg_core::options::{Options, Style};

use crate::highlighters::hsla;
use crate::preferences::Preferences;
use crate::shortcuts::{self, Shortcut};

/// Emitted when the dialog should close.
pub struct Close;

impl EventEmitter<Close> for OptionsEditor {}

const STYLES: [(Style, &str); 3] = [
    (Style::System, "System"),
    (Style::Light, "Light"),
    (Style::Dark, "Dark"),
];

/// One row of the Shortcuts tab.
struct ShortcutRow {
    shortcut: Shortcut,
    input: Entity<InputState>,
}

pub struct OptionsEditor {
    options: Options,
    tab: usize,
    font_family: Entity<InputState>,
    font_size: Entity<InputState>,
    recent_files: Entity<InputState>,
    search_color: Entity<ColorPickerState>,
    quickfind_color: Entity<ColorPickerState>,
    shortcuts: Vec<ShortcutRow>,
    /// Why the last Apply was refused.
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl OptionsEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let options = Preferences::get(cx).clone();
        let input =
            |text: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let placeholder = SharedString::from(placeholder.to_owned());
                cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(placeholder);
                    s.set_value(text, window, cx);
                    s
                })
            };
        let theme_font = cx.theme().mono_font_family.to_string();
        let theme_size = f32::from(cx.theme().mono_font_size).to_string();
        let font_family = input(
            options.view.font_family.clone().unwrap_or_default(),
            &theme_font,
            window,
            cx,
        );
        let font_size = input(
            options
                .view
                .font_size
                .map(|s| s.to_string())
                .unwrap_or_default(),
            &theme_size,
            window,
            cx,
        );
        let recent_files = input(options.file.recent_files.to_string(), "", window, cx);
        let picker = |color: Option<Color>, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = ColorPickerState::new(window, cx);
                if let Some(c) = color {
                    s.set_value(hsla(c), window, cx);
                }
                s
            })
        };
        let search_color = picker(options.search.highlight_color, window, cx);
        let quickfind_color = picker(options.search.quickfind_color, window, cx);
        let shortcuts = shortcuts::all()
            .into_iter()
            .map(|shortcut| {
                let keys = shortcut.keys(&options.shortcuts).join(" ");
                let input = input(keys, "none", window, cx);
                ShortcutRow { shortcut, input }
            })
            .collect();
        let subscriptions = vec![
            cx.subscribe(&search_color, |this, _, e: &ColorPickerEvent, cx| {
                let ColorPickerEvent::Change(c) = e;
                this.options.search.highlight_color = c.map(to_color);
                cx.notify();
            }),
            cx.subscribe(&quickfind_color, |this, _, e: &ColorPickerEvent, cx| {
                let ColorPickerEvent::Change(c) = e;
                this.options.search.quickfind_color = c.map(to_color);
                cx.notify();
            }),
        ];
        Self {
            options,
            tab: 0,
            font_family,
            font_size,
            recent_files,
            search_color,
            quickfind_color,
            shortcuts,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// The options as edited, with the text fields read; an error if one
    /// is invalid.
    pub fn edited(&self, cx: &App) -> Result<Options, String> {
        let mut o = self.options.clone();
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let family = text(&self.font_family);
        o.view.font_family = (!family.is_empty()).then_some(family);
        let size = text(&self.font_size);
        o.view.font_size = if size.is_empty() {
            None
        } else {
            match size.parse::<f32>() {
                Ok(s) if (4.0..=72.0).contains(&s) => Some(s),
                _ => return Err(format!("Font size: {size:?} is not a size from 4 to 72")),
            }
        };
        let recent = text(&self.recent_files);
        o.file.recent_files = match recent.parse::<usize>() {
            Ok(n) if (1..=rlogg_core::settings::MAX_RECENT_FILES).contains(&n) => n,
            _ => {
                return Err(format!(
                    "Recent files: {recent:?} is not a number from 1 to {}",
                    rlogg_core::settings::MAX_RECENT_FILES
                ));
            }
        };
        o.shortcuts.clear();
        for row in &self.shortcuts {
            let keys: Vec<String> = text(&row.input)
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            if let Some(bad) = keys.iter().find(|k| !shortcuts::is_valid_key(k)) {
                return Err(format!("{}: {bad:?} is not a key", row.shortcut.label));
            }
            if keys != row.shortcut.keys {
                o.shortcuts.insert(row.shortcut.id.to_owned(), keys);
            }
        }
        Ok(o)
    }

    /// Installs the edited options; false if a field is invalid.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.edited(cx) {
            Ok(options) => {
                self.options = options.clone();
                self.error = None;
                Preferences::set(options, Some(window), cx);
                true
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                false
            }
        }
    }

    fn restore_default_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for row in &self.shortcuts {
            let keys = row.shortcut.keys.join(" ");
            row.input.update(cx, |s, cx| s.set_value(keys, window, cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn set_shortcut_text(&mut self, id: &str, text: &str, window: &mut Window, cx: &mut App) {
        if let Some(row) = self.shortcuts.iter().find(|r| r.shortcut.id == id) {
            let text = text.to_owned();
            row.input.update(cx, |s, cx| s.set_value(text, window, cx));
        }
    }

    #[cfg(test)]
    pub fn options_mut(&mut self) -> &mut Options {
        &mut self.options
    }

    fn check(
        &self,
        id: &'static str,
        label: &'static str,
        value: bool,
        f: fn(&mut Options, bool),
        cx: &mut Context<Self>,
    ) -> Checkbox {
        Checkbox::new(id)
            .label(label)
            .checked(value)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                f(&mut this.options, *checked);
                cx.notify();
            }))
    }

    fn color_row(
        &self,
        id: &'static str,
        label: &'static str,
        state: &Entity<ColorPickerState>,
        clear: fn(&mut Options),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state_for_reset = state.clone();
        h_flex()
            .gap_2()
            .child(div().w(px(160.)).text_sm().child(label))
            .child(ColorPicker::new(state).small())
            .child(
                Button::new(id)
                    .label("Default")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        clear(&mut this.options);
                        state_for_reset.update(cx, |s, cx| s.clear_value(window, cx));
                        cx.notify();
                    })),
            )
    }

    fn render_general(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let o = &self.options;
        let heading = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(text)
        };
        v_flex()
            .gap_2()
            .child(heading("Session"))
            .child(self.check(
                "load-last",
                "Load last session",
                o.general.load_last_session,
                |o, v| o.general.load_last_session = v,
                cx,
            ))
            .child(self.check(
                "follow-on-load",
                "Follow file on load",
                o.general.follow_on_load,
                |o, v| o.general.follow_on_load = v,
                cx,
            ))
            .child(heading("Search options for new files"))
            .child(
                h_flex()
                    .gap_4()
                    .child(self.check(
                        "match-case",
                        "Case sensitive",
                        o.search.match_case,
                        |o, v| o.search.match_case = v,
                        cx,
                    ))
                    .child(self.check(
                        "regex",
                        "Regular expressions",
                        o.search.regex,
                        |o, v| o.search.regex = v,
                        cx,
                    ))
                    .child(self.check(
                        "boolean",
                        "Logical combining",
                        o.search.boolean,
                        |o, v| o.search.boolean = v,
                        cx,
                    ))
                    .child(self.check(
                        "auto-refresh",
                        "Auto refresh",
                        o.search.auto_refresh,
                        |o, v| o.search.auto_refresh = v,
                        cx,
                    )),
            )
            .child(self.check(
                "highlight",
                "Highlight matches",
                o.search.highlight_matches,
                |o, v| o.search.highlight_matches = v,
                cx,
            ))
            .child(self.color_row(
                "search-color-default",
                "Match highlight colour",
                &self.search_color,
                |o| o.search.highlight_color = None,
                cx,
            ))
            .child(heading("Quick find"))
            .child(
                h_flex()
                    .gap_4()
                    .child(self.check(
                        "qf-incremental",
                        "Incremental",
                        o.search.quickfind_incremental,
                        |o, v| o.search.quickfind_incremental = v,
                        cx,
                    ))
                    .child(self.check(
                        "qf-match-case",
                        "Case sensitive",
                        o.search.quickfind_match_case,
                        |o, v| o.search.quickfind_match_case = v,
                        cx,
                    ))
                    .child(self.check(
                        "qf-regex",
                        "Regular expressions",
                        o.search.quickfind_regex,
                        |o, v| o.search.quickfind_regex = v,
                        cx,
                    )),
            )
            .child(self.color_row(
                "qf-color-default",
                "Occurrence colour",
                &self.quickfind_color,
                |o| o.search.quickfind_color = None,
                cx,
            ))
    }

    fn render_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let o = &self.options;
        let row = |label: &'static str, child: gpui::AnyElement| {
            h_flex()
                .gap_2()
                .child(div().w(px(160.)).text_sm().child(label))
                .child(child)
        };
        let style_index = STYLES.iter().position(|(s, _)| *s == o.view.style);
        v_flex()
            .gap_2()
            .child(row(
                "Font family",
                div()
                    .w(px(280.))
                    .child(Input::new(&self.font_family).small())
                    .into_any_element(),
            ))
            .child(row(
                "Font size",
                div()
                    .w(px(80.))
                    .child(Input::new(&self.font_size).small())
                    .into_any_element(),
            ))
            .child(row(
                "Style",
                RadioGroup::horizontal("style")
                    .children(
                        STYLES
                            .iter()
                            .map(|(_, label)| Radio::new(*label).label(*label)),
                    )
                    .selected_index(style_index)
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.options.view.style = STYLES[*index].0;
                        cx.notify();
                    }))
                    .into_any_element(),
            ))
            .child(self.check(
                "overview",
                "Show the overview bar",
                o.view.overview,
                |o, v| o.view.overview = v,
                cx,
            ))
            .child(self.check(
                "numbers-main",
                "Line numbers in main view",
                o.view.line_numbers_main,
                |o, v| o.view.line_numbers_main = v,
                cx,
            ))
            .child(self.check(
                "numbers-filtered",
                "Line numbers in filtered view",
                o.view.line_numbers_filtered,
                |o, v| o.view.line_numbers_filtered = v,
                cx,
            ))
    }

    fn render_file(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let o = &self.options;
        v_flex()
            .gap_2()
            .child(self.check(
                "follow-on-scroll",
                "Turn on follow on scroll past the end",
                o.file.follow_on_scroll,
                |o, v| o.file.follow_on_scroll = v,
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child("Show the number of recently opened files"),
                    )
                    .child(
                        div()
                            .w(px(64.))
                            .child(Input::new(&self.recent_files).small()),
                    ),
            )
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let rows = self.shortcuts.iter().map(|row| {
            h_flex()
                .gap_2()
                .py_0p5()
                .child(
                    v_flex()
                        .w(px(300.))
                        .child(div().text_sm().child(row.shortcut.label))
                        .child(div().text_xs().text_color(muted).child(row.shortcut.id)),
                )
                .child(div().flex_1().child(Input::new(&row.input).small()))
        });
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Keys separated by spaces, e.g. \"ctrl-shift-f f3\". \"secondary\" is Ctrl, or Cmd on macOS."),
            )
            .child(
                v_flex()
                    .id("shortcut-rows")
                    .h(px(360.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_2()
                    .children(rows),
            )
            .child(
                h_flex().child(
                    Button::new("restore-shortcuts")
                        .label("Restore defaults")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.restore_default_shortcuts(window, cx)
                        })),
                ),
            )
    }
}

fn to_color(c: gpui::Hsla) -> Color {
    let rgba = c.to_rgb();
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Color {
        r: channel(rgba.r),
        g: channel(rgba.g),
        b: channel(rgba.b),
        a: channel(rgba.a),
    }
}

impl Render for OptionsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let body = match self.tab {
            0 => self.render_general(cx).into_any_element(),
            1 => self.render_view(cx).into_any_element(),
            2 => self.render_file(cx).into_any_element(),
            _ => self.render_shortcuts(cx).into_any_element(),
        };
        v_flex()
            .id("options-editor")
            .gap_3()
            .child(
                TabBar::new("options-tabs")
                    .underline()
                    .small()
                    .selected_index(self.tab)
                    .child(Tab::new().label("General"))
                    .child(Tab::new().label("View"))
                    .child(Tab::new().label("File"))
                    .child(Tab::new().label("Shortcuts"))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.tab = *index;
                        cx.notify();
                    })),
            )
            .child(div().min_h(px(440.)).child(body))
            .child(
                h_flex()
                    .gap_2()
                    .children(
                        self.error
                            .clone()
                            .map(|e| div().text_sm().text_color(danger).child(e)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Close))),
                    )
                    .child(
                        Button::new("apply")
                            .label("Apply")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply(window, cx);
                            })),
                    )
                    .child(
                        Button::new("ok")
                            .label("OK")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.apply(window, cx) {
                                    cx.emit(Close);
                                }
                            })),
                    ),
            )
    }
}

/// Opens the dialog (klogg: Tools > Options…).
pub fn open(window: &mut Window, cx: &mut App) {
    let editor = cx.new(|cx| OptionsEditor::new(window, cx));
    window
        .subscribe(&editor, cx, |_, _: &Close, window, cx| {
            window.close_dialog(cx)
        })
        .detach();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Options").w(px(860.)).child(editor.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crawler::Crawler;
    use gpui::{Focusable, TestAppContext, VisualTestContext};
    use rlogg_core::LogFile;
    use std::sync::Arc;

    fn setup(cx: &mut TestAppContext) -> (Entity<Crawler>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Preferences::init(None, cx);
            crate::highlighters::Highlighters::init(None, cx);
        });
        let path = std::env::temp_dir().join(format!("rlogg-options-{}.log", std::process::id()));
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let log = Arc::new(LogFile::open(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
        let (crawler, cx) = cx.add_window_view(Crawler::new);
        crawler.update_in(cx, |c, window, cx| {
            c.set_log(Some(log), cx);
            let focus = c.main.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.run_until_parked();
        (crawler, cx)
    }

    fn marks(crawler: &Entity<Crawler>, cx: &mut VisualTestContext) -> Vec<usize> {
        crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().iter().collect())
    }

    #[gpui::test]
    fn changed_shortcuts_apply_at_once(cx: &mut TestAppContext) {
        let (crawler, cx) = setup(cx);
        cx.simulate_keystrokes("down m");
        assert_eq!(marks(&crawler, cx), [0]);

        let editor = cx.update(|window, cx| cx.new(|cx| OptionsEditor::new(window, cx)));
        editor.update_in(cx, |e, window, cx| {
            e.set_shortcut_text("logview.mark", "x", window, cx);
            assert!(e.apply(window, cx));
        });
        let options = cx.update(|_, cx| Preferences::get(cx).clone());
        assert_eq!(options.shortcuts["logview.mark"], ["x"]);
        // Focus is back on the view; `m` no longer marks, `x` does.
        crawler.update_in(cx, |c, window, cx| {
            let focus = c.main.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("down m");
        assert_eq!(marks(&crawler, cx), [0]);
        cx.simulate_keystrokes("x");
        assert_eq!(marks(&crawler, cx), [0, 1]);

        // Back to the default: no override is kept.
        editor.update_in(cx, |e, window, cx| {
            e.restore_default_shortcuts(window, cx);
            assert!(e.apply(window, cx));
        });
        assert!(cx.update(|_, cx| Preferences::get(cx).shortcuts.is_empty()));
        cx.simulate_keystrokes("down m");
        assert_eq!(marks(&crawler, cx), [0, 1, 2]);
    }

    #[gpui::test]
    fn invalid_fields_are_refused(cx: &mut TestAppContext) {
        let (_, cx) = setup(cx);
        let editor = cx.update(|window, cx| cx.new(|cx| OptionsEditor::new(window, cx)));
        editor.update_in(cx, |e, window, cx| {
            e.set_shortcut_text("logview.mark", "ctrl-", window, cx);
            assert!(!e.apply(window, cx));
            assert!(e.error.as_ref().unwrap().contains("Mark selected lines"));
            e.set_shortcut_text("logview.mark", "m", window, cx);
            e.font_size
                .update(cx, |s, cx| s.set_value("900", window, cx));
            assert!(!e.apply(window, cx));
            e.font_size.update(cx, |s, cx| s.set_value("", window, cx));
            e.options_mut().search.match_case = false;
            e.options_mut().view.line_numbers_main = false;
            assert!(e.apply(window, cx));
        });
        let options = cx.update(|_, cx| Preferences::get(cx).clone());
        assert!(!options.search.match_case && !options.view.line_numbers_main);
        assert!(!options.new_file_context().match_case);
    }
}
