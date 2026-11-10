//! Search history and predefined filters in the app (klogg: `SavedSearches`,
//! `PredefinedFiltersCollection`, `PredefinedFiltersDialog`), shared by
//! every tab and saved in `searches.toml`.

use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Focusable, Global, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};
use rlogg_core::searches::{PredefinedFilter, Searches};

pub struct SavedSearches {
    pub searches: Searches,
    config_dir: Option<PathBuf>,
}

impl Global for SavedSearches {}

impl SavedSearches {
    pub fn init(config_dir: Option<PathBuf>, cx: &mut App) {
        let searches = config_dir
            .as_deref()
            .map(Searches::load)
            .unwrap_or_default();
        cx.set_global(Self {
            searches,
            config_dir,
        });
    }

    /// The searches; empty if the global isn't installed (tests).
    pub fn get(cx: &App) -> Searches {
        cx.try_global::<Self>()
            .map(|s| s.searches.clone())
            .unwrap_or_default()
    }

    /// Changes the searches, saves them and redraws.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Searches)) {
        if !cx.has_global::<Self>() {
            Self::init(None, cx);
        }
        let this = cx.global_mut::<Self>();
        f(&mut this.searches);
        if let Some(dir) = &this.config_dir
            && let Err(e) = this.searches.save(dir)
        {
            tracing::warn!("cannot save searches: {e}");
        }
        cx.refresh_windows();
    }
}

/// Emitted by the dialogs here when they should close.
pub struct Close;

/// klogg: "Edit search history" (`QInputDialog::getMultiLineText`), one
/// search per line, most recent first.
pub fn edit_history(window: &mut Window, cx: &mut App) {
    let text = SavedSearches::get(cx).history.join("\n");
    let state = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx).rows(14);
        s.set_value(text, window, cx);
        s
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let state = state.clone();
        dialog
            .title("Search history")
            .w(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child("One search per line, most recent first"),
                    )
                    .child(Textarea::new(&state).h(px(300.))),
            )
            .footer(ok_cancel("history", move |_, cx| {
                let lines = state.read(cx).value().to_string();
                SavedSearches::update(cx, |s| s.set_history(&lines));
            }))
    });
}

/// klogg: "Save as predefined filter" from the search box: asks for a
/// name and adds the search as a filter.
pub fn save_as_filter(pattern: String, regex: bool, window: &mut Window, cx: &mut App) {
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
    let first = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            name.focus_handle(cx).focus(window, cx);
        }
        let (name, pattern) = (name.clone(), pattern.clone());
        dialog
            .title("Save as predefined filter")
            .w(px(480.))
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(format!("Pattern: {pattern}")))
                    .child(Input::new(&name)),
            )
            .footer(ok_cancel("save-filter", move |_, cx| {
                let name = name.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    return;
                }
                let filter = PredefinedFilter {
                    name,
                    pattern: pattern.clone(),
                    regex,
                };
                SavedSearches::update(cx, |s| s.filters.push(filter));
            }))
    });
}

/// Cancel and OK buttons for a dialog; OK runs `ok` and closes it.
fn ok_cancel(id: &'static str, ok: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    h_flex()
        .justify_end()
        .gap_2()
        .child(
            Button::new(SharedString::from(format!("{id}-cancel")))
                .label("Cancel")
                .small()
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-ok")))
                .label("OK")
                .small()
                .primary()
                .on_click(move |_, window, cx| {
                    ok(window, cx);
                    window.close_dialog(cx);
                }),
        )
}

/// One row of the predefined filters dialog.
struct FilterRow {
    name: Entity<InputState>,
    pattern: Entity<InputState>,
    regex: bool,
}

/// klogg: `PredefinedFiltersDialog` (Tools > Predefined filters…): a table
/// of name, pattern and "regex"; add, remove, move up and down.
pub struct FiltersEditor {
    rows: Vec<FilterRow>,
}

impl EventEmitter<Close> for FiltersEditor {}

impl FiltersEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filters = SavedSearches::get(cx).filters;
        let mut this = Self { rows: Vec::new() };
        for f in filters {
            this.push(f, window, cx);
        }
        this
    }

    fn push(&mut self, f: PredefinedFilter, window: &mut Window, cx: &mut Context<Self>) {
        let input = |text: String,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(placeholder);
                s.set_value(text, window, cx);
                s
            })
        };
        self.rows.push(FilterRow {
            name: input(f.name, "Name", window, cx),
            pattern: input(f.pattern, "Pattern", window, cx),
            regex: f.regex,
        });
    }

    /// The filters as edited; rows without a name or pattern are dropped.
    pub fn filters(&self, cx: &App) -> Vec<PredefinedFilter> {
        self.rows
            .iter()
            .map(|r| PredefinedFilter {
                name: r.name.read(cx).value().trim().to_owned(),
                pattern: r.pattern.read(cx).value().to_string(),
                regex: r.regex,
            })
            .filter(|f| !f.name.is_empty() && !f.pattern.is_empty())
            .collect()
    }

    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push(
            PredefinedFilter {
                name: String::new(),
                pattern: String::new(),
                regex: true,
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let filters = self.filters(cx);
        SavedSearches::update(cx, |s| s.filters = filters);
    }
}

impl Render for FiltersEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted) = (cx.theme().border, cx.theme().muted_foreground);
        let n = self.rows.len();
        let rows = self.rows.iter().enumerate().map(|(i, row)| {
            let id = |what: &str| SharedString::from(format!("filter-{what}-{i}"));
            h_flex()
                .gap_2()
                .py_0p5()
                .child(div().w(px(180.)).child(Input::new(&row.name).small()))
                .child(div().flex_1().child(Input::new(&row.pattern).small()))
                .child(
                    Checkbox::new(id("regex"))
                        .label("Regex")
                        .checked(row.regex)
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            this.rows[i].regex = *on;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(id("up"))
                        .label("↑")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i > 0 {
                                this.rows.swap(i, i - 1);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new(id("down"))
                        .label("↓")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i + 1 < n {
                                this.rows.swap(i, i + 1);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new(id("remove"))
                        .label("−")
                        .xsmall()
                        .ghost()
                        .tooltip("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.rows.remove(i);
                            cx.notify();
                        })),
                )
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(muted)
                    .child(div().w(px(180.)).child("Name"))
                    .child(div().flex_1().child("Pattern")),
            )
            .child(
                v_flex()
                    .id("filter-rows")
                    .h(px(300.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_2()
                    .children(rows),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("filter-add")
                            .label("Add")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("filters-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Close))),
                    )
                    .child(
                        Button::new("filters-ok")
                            .label("OK")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply(cx);
                                cx.emit(Close);
                            })),
                    ),
            )
    }
}

/// Opens the predefined filters dialog.
pub fn edit_filters(window: &mut Window, cx: &mut App) {
    let editor = cx.new(|cx| FiltersEditor::new(window, cx));
    window
        .subscribe(&editor, cx, |_, _: &Close, window, cx| {
            window.close_dialog(cx)
        })
        .detach();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Predefined filters")
            .w(px(760.))
            .child(editor.clone())
    });
}
