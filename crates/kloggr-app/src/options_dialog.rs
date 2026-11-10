//! The options dialog (klogg: `OptionsDialog`, Tools > Options…): General,
//! View, File and Shortcuts tabs, with OK, Apply and Cancel.
//!
//! Edits go to a copy of the options; Apply and OK install it through
//! [`Preferences::set`].

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputState};
use gpui_component::radio::{Radio, RadioGroup};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};
use kloggr_core::highlight::Color;
use kloggr_core::options::{Options, Style};

use crate::highlighters::hsla;
use crate::key_capture;
use crate::preferences::Preferences;
use crate::saved_searches::SavedSearches;
use crate::shortcuts::{self, Shortcut};

/// Emitted when the dialog should close.
pub struct Close;

impl EventEmitter<Close> for OptionsEditor {}

const STYLES: [(Style, &str); 3] = [
    (Style::System, "System"),
    (Style::Light, "Light"),
    (Style::Dark, "Dark"),
];

/// One row of the Shortcuts tab: the action and its primary and secondary
/// keys (klogg: `PRIMARY_COL`, `SECONDARY_COL`).
struct ShortcutRow {
    shortcut: Shortcut,
    keys: [Option<String>; 2],
}

fn slots(keys: &[String]) -> [Option<String>; 2] {
    [keys.first().cloned(), keys.get(1).cloned()]
}

pub struct OptionsEditor {
    options: Options,
    tab: usize,
    font_family: Entity<InputState>,
    font_size: Entity<InputState>,
    recent_files: Entity<InputState>,
    /// klogg: "Search history size" (kept with the history).
    history_size: Entity<InputState>,
    search_color: Entity<ColorPickerState>,
    quickfind_color: Entity<ColorPickerState>,
    shortcuts: Vec<ShortcutRow>,
    /// Why the last Apply was refused.
    error: Option<String>,
    /// The open key dialog (see [`key_capture::open`]).
    capture: Option<Entity<key_capture::KeyCapture>>,
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
        let history_size = input(
            SavedSearches::get(cx).history_size.to_string(),
            "",
            window,
            cx,
        );
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
        // klogg's table is sorted by action name.
        let mut shortcuts: Vec<ShortcutRow> = shortcuts::all()
            .into_iter()
            .map(|shortcut| ShortcutRow {
                keys: slots(shortcut.keys(&options.shortcuts)),
                shortcut,
            })
            .collect();
        shortcuts.sort_by_key(|r| r.shortcut.label.to_lowercase());
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
            history_size,
            search_color,
            quickfind_color,
            shortcuts,
            error: None,
            capture: None,
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
            Ok(n) if (1..=kloggr_core::settings::MAX_RECENT_FILES).contains(&n) => n,
            _ => {
                return Err(format!(
                    "Recent files: {recent:?} is not a number from 1 to {}",
                    kloggr_core::settings::MAX_RECENT_FILES
                ));
            }
        };
        if let Some(key) = self.duplicates().first() {
            return Err(format!(
                "{} is used by more than one action",
                shortcuts::display(key)
            ));
        }
        o.shortcuts.clear();
        for row in &self.shortcuts {
            let keys: Vec<String> = row.keys.iter().flatten().cloned().collect();
            if keys != row.shortcut.keys {
                o.shortcuts.insert(row.shortcut.id.to_owned(), keys);
            }
        }
        Ok(o)
    }

    /// Keys given to more than one slot (klogg: `checkShortcutsOnDuplicate`,
    /// which shows them in red and disables OK and Apply).
    fn duplicates(&self) -> Vec<String> {
        let mut count = std::collections::BTreeMap::<String, usize>::new();
        for key in self.shortcuts.iter().flat_map(|r| r.keys.iter().flatten()) {
            *count.entry(shortcuts::normalize(key)).or_default() += 1;
        }
        count
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(k, _)| k)
            .collect()
    }

    /// Sets one key of a row (`slot` 0: primary, 1: secondary).
    pub fn set_key(&mut self, id: &str, slot: usize, key: Option<String>, cx: &mut Context<Self>) {
        if let Some(row) = self.shortcuts.iter_mut().find(|r| r.shortcut.id == id) {
            row.keys[slot] = key;
            // Keep a lone key in the primary column.
            if row.keys[0].is_none() {
                row.keys.swap(0, 1);
            }
        }
        self.error = None;
        cx.notify();
    }

    /// Opens the key dialog for one slot (klogg: the "..." button).
    fn edit_key(
        &mut self,
        id: &'static str,
        slot: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .shortcuts
            .iter()
            .find(|r| r.shortcut.id == id)
            .and_then(|r| r.keys[slot].clone());
        let this = cx.entity().downgrade();
        let capture = key_capture::open(current, window, cx, move |key, _, cx| {
            this.update(cx, |this, cx| this.set_key(id, slot, key, cx))
                .ok();
        });
        self.capture = Some(capture);
    }

    /// Installs the edited options; false if a field is invalid.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let size = self.history_size.read(cx).value().trim().to_owned();
        let history_size = match size.parse::<usize>() {
            Ok(n) if (1..=1000).contains(&n) => n,
            _ => {
                self.error = Some(format!(
                    "Search history size: {size:?} is not a number from 1 to 1000"
                ));
                cx.notify();
                return false;
            }
        };
        match self.edited(cx) {
            Ok(options) => {
                if SavedSearches::get(cx).history_size != history_size {
                    SavedSearches::update(cx, |s| s.set_history_size(history_size));
                }
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

    fn restore_default_shortcuts(&mut self, cx: &mut Context<Self>) {
        for row in &mut self.shortcuts {
            row.keys = slots(&row.shortcut.keys);
        }
        self.error = None;
        cx.notify();
    }

    /// klogg asks first: "Do you want to restore default shortcuts?"
    fn confirm_restore_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Restore Default Shortcuts")
                .description("Do you want to restore default shortcuts?")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    this.update(cx, |this, cx| this.restore_default_shortcuts(cx))
                        .ok();
                    true
                })
        });
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
            .child(heading("Updates"))
            .child(self.check(
                "check-updates",
                "Check for new versions once a week",
                o.general.check_for_updates,
                |o, v| o.general.check_for_updates = v,
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
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(160.)).text_sm().child("Search history size"))
                    .child(
                        div()
                            .w(px(64.))
                            .child(Input::new(&self.history_size).small()),
                    ),
            )
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
            .child(self.check(
                "extract-archives",
                "Extract archives",
                o.file.extract_archives,
                |o, v| o.file.extract_archives = v,
                cx,
            ))
            .child(self.check(
                "extract-always",
                "Extract archives without confirmation",
                o.file.extract_without_confirmation,
                |o, v| o.file.extract_without_confirmation = v,
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
        let (muted, border, danger) = (theme.muted_foreground, theme.border, theme.danger);
        let duplicates = self.duplicates();
        let header = h_flex()
            .gap_2()
            .px_2()
            .text_sm()
            .text_color(muted)
            .child(div().w(px(330.)).child("Action"))
            .child(div().flex_1().child("Primary shortcut"))
            .child(div().flex_1().child("Secondary shortcut"));
        let rows = self.shortcuts.iter().enumerate().map(|(i, row)| {
            let id = row.shortcut.id;
            let cell = |slot: usize| {
                let key = row.keys[slot].as_deref();
                let duplicate = key.is_some_and(|k| duplicates.contains(&shortcuts::normalize(k)));
                h_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .px_1()
                            .rounded_sm()
                            .text_sm()
                            .when(duplicate, |d| d.bg(danger.opacity(0.35)))
                            .child(key.map(shortcuts::display).unwrap_or_default()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("key-{i}-{slot}")))
                            .label("...")
                            .small()
                            .w(px(40.))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_key(id, slot, window, cx)
                            })),
                    )
            };
            h_flex()
                .gap_2()
                .px_2()
                .py_0p5()
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .w(px(330.))
                        .text_sm()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(row.shortcut.label),
                )
                .child(cell(0))
                .child(cell(1))
        });
        v_flex()
            .gap_2()
            .child(header)
            .child(
                v_flex()
                    .id("shortcut-rows")
                    .h(px(380.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .children(rows),
            )
            .child(
                h_flex().child(
                    Button::new("restore-shortcuts")
                        .label("Restore defaults")
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_restore_defaults(window, cx)
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
        let has_duplicates = !self.duplicates().is_empty();
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
                            .disabled(has_duplicates)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply(window, cx);
                            })),
                    )
                    .child(
                        Button::new("ok")
                            .label("OK")
                            .small()
                            .primary()
                            .disabled(has_duplicates)
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
    use kloggr_core::LogFile;
    use std::sync::Arc;

    fn setup(cx: &mut TestAppContext) -> (Entity<Crawler>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Preferences::init(None, cx);
            crate::highlighters::Highlighters::init(None, cx);
        });
        let path = std::env::temp_dir().join(format!("kloggr-options-{}.log", std::process::id()));
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
            e.set_key("logview.mark", 0, Some("x".into()), cx);
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
            e.restore_default_shortcuts(cx);
            assert!(e.apply(window, cx));
        });
        assert!(cx.update(|_, cx| Preferences::get(cx).shortcuts.is_empty()));
        cx.simulate_keystrokes("down m");
        assert_eq!(marks(&crawler, cx), [0, 1, 2]);
    }

    #[gpui::test]
    fn key_dialog_sets_a_slot(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Preferences::init(None, cx);
        });
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let window = cx.add_window({
            let slot = slot.clone();
            move |window, cx| {
                let editor = cx.new(|cx| OptionsEditor::new(window, cx));
                *slot.borrow_mut() = Some(editor.clone());
                gpui_component::Root::new(editor, window, cx)
            }
        });
        let editor: Entity<OptionsEditor> = slot.borrow().clone().unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        editor.update_in(cx, |e, window, cx| {
            e.edit_key("logview.mark", 1, window, cx)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-shift-m");
        // As the OK button does: the dialog closes in the same update.
        let capture = editor.read_with(cx, |e, _| e.capture.clone().unwrap());
        capture.update_in(cx, |c, window, cx| {
            cx.emit(key_capture::Done(c.key().map(str::to_owned)));
            window.close_dialog(cx);
        });
        drop(capture);
        cx.run_until_parked();
        let keys = editor.read_with(cx, |e, _| {
            e.shortcuts
                .iter()
                .find(|r| r.shortcut.id == "logview.mark")
                .unwrap()
                .keys
                .clone()
        });
        assert_eq!(
            keys,
            [Some("m".to_owned()), Some("ctrl-shift-m".to_owned())]
        );
    }

    #[gpui::test]
    fn invalid_fields_are_refused(cx: &mut TestAppContext) {
        let (_, cx) = setup(cx);
        let editor = cx.update(|window, cx| cx.new(|cx| OptionsEditor::new(window, cx)));
        editor.update_in(cx, |e, window, cx| {
            // A key used twice (klogg: shown in red, OK disabled).
            e.set_key("logview.mark", 1, Some("ctrl-d".into()), cx);
            assert_eq!(e.duplicates(), ["ctrl-d"]);
            assert!(!e.apply(window, cx));
            assert!(e.error.as_ref().unwrap().contains("Ctrl+D"));
            e.set_key("logview.mark", 1, None, cx);
            // Clearing the primary key moves the secondary one up.
            e.set_key("qf.find_next", 0, None, cx);
            let row = e
                .shortcuts
                .iter()
                .find(|r| r.shortcut.id == "qf.find_next")
                .unwrap();
            assert_eq!(row.keys, [Some("secondary-g".to_owned()), None]);
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
