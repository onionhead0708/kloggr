//! Search history and predefined filters in the app (klogg: `SavedSearches`,
//! `PredefinedFiltersCollection`, `PredefinedFiltersDialog`), shared by
//! every tab and saved in `searches.toml`.

use std::path::{Path, PathBuf};

use crate::tr;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Focusable, Global, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};
use kloggr_core::searches::{PredefinedFilter, Searches};

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
            .title(tr!("Search history"))
            .w(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child(tr!("One search per line, most recent first")),
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
    let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("Name")));
    let first = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            name.focus_handle(cx).focus(window, cx);
        }
        let (name, pattern) = (name.clone(), pattern.clone());
        dialog
            .title(tr!("Save as predefined filter"))
            .w(px(480.))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child(tr!("Pattern: {pattern}", pattern = pattern)),
                    )
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
                .label(tr!("Cancel"))
                .small()
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-ok")))
                .label(tr!("OK"))
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
/// of name, pattern and "regex"; add, remove, move up and down, import and
/// export klogg `.conf` files.
pub struct FiltersEditor {
    rows: Vec<FilterRow>,
    /// The result of the last import or export.
    message: Option<String>,
}

impl EventEmitter<Close> for FiltersEditor {}

impl FiltersEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filters = SavedSearches::get(cx).filters;
        let mut this = Self {
            rows: Vec::new(),
            message: None,
        };
        for f in filters {
            this.push(f, window, cx);
        }
        this
    }

    fn push(&mut self, f: PredefinedFilter, window: &mut Window, cx: &mut Context<Self>) {
        let input = |text: String,
                     placeholder: SharedString,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(placeholder);
                s.set_value(text, window, cx);
                s
            })
        };
        self.rows.push(FilterRow {
            name: input(f.name, tr!("Name"), window, cx),
            pattern: input(f.pattern, tr!("Pattern"), window, cx),
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

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr!("Import")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.import_file(&path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Replaces the rows with the filters of a klogg `.conf` file, as klogg
    /// does (`importFilters`); OK keeps them.
    pub fn import_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let filters = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                kloggr_core::searches::import_klogg_filters(&text)
                    .ok_or_else(|| tr!("no predefined filters in it").to_string())
            });
        self.message = Some(match filters {
            Ok(filters) => {
                let n = filters.len();
                self.rows.clear();
                for f in filters {
                    self.push(f, window, cx);
                }
                tr!(
                    "Imported {count} filter(s) from {file}",
                    count = n,
                    file = path.display()
                )
                .to_string()
            }
            Err(e) => format!("{}: {e}", path.display()),
        });
        cx.notify();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = std::env::home_dir().unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some("filters.conf"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update(cx, |this, cx| this.export_to(path, cx)).ok();
        })
        .detach();
    }

    /// Writes the rows as a klogg `.conf` file (klogg adds the extension if
    /// missing).
    pub fn export_to(&mut self, mut path: PathBuf, cx: &mut Context<Self>) {
        if path.extension().is_none_or(|e| e != "conf") {
            path.as_mut_os_string().push(".conf");
        }
        let text = kloggr_core::searches::export_klogg_filters(&self.filters(cx));
        self.message = Some(match std::fs::write(&path, text) {
            Ok(()) => tr!("Exported to {file}", file = path.display()).to_string(),
            Err(e) => format!("{}: {e}", path.display()),
        });
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
                        .label(tr!("Regex"))
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
                        .tooltip(tr!("Remove"))
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
                    .child(div().w(px(180.)).child(tr!("Name")))
                    .child(div().flex_1().child(tr!("Pattern"))),
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
                            .label(tr!("Add"))
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    )
                    .child(
                        Button::new("filter-import")
                            .label(tr!("Import…"))
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
                    )
                    .child(
                        Button::new("filter-export")
                            .label(tr!("Export…"))
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(muted)
                            .truncate()
                            .children(self.message.clone()),
                    )
                    .child(
                        Button::new("filters-cancel")
                            .label(tr!("Cancel"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Close))),
                    )
                    .child(
                        Button::new("filters-ok")
                            .label(tr!("OK"))
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
            .title(tr!("Predefined filters"))
            .w(px(760.))
            .child(editor.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn filters_import_and_export(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            SavedSearches::init(None, cx);
            SavedSearches::update(cx, |s| {
                s.filters = vec![PredefinedFilter {
                    name: "Errors".into(),
                    pattern: "ERROR|FATAL".into(),
                    regex: true,
                }]
            });
        });
        let (editor, cx): (Entity<FiltersEditor>, &mut VisualTestContext) =
            cx.add_window_view(FiltersEditor::new);
        let dir = std::env::temp_dir().join(format!("kloggr-filters-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Export adds klogg's extension.
        editor.update(cx, |e, cx| e.export_to(dir.join("mine"), cx));
        let exported = dir.join("mine.conf");
        let text = std::fs::read_to_string(&exported).unwrap();
        assert!(text.starts_with("[PredefinedFiltersCollection]"));

        // Import replaces the rows (klogg does too); OK would keep them.
        let other = dir.join("other.conf");
        std::fs::write(
            &other,
            "[PredefinedFiltersCollection]\nversion=1\nfilters\\1\\name=Warn\n\
             filters\\1\\filter=WARN\nfilters\\1\\regex=false\nfilters\\size=1\n",
        )
        .unwrap();
        editor.update_in(cx, |e, window, cx| e.import_file(&other, window, cx));
        let filters = editor.read_with(cx, |e, cx| e.filters(cx));
        assert_eq!(
            filters,
            [PredefinedFilter {
                name: "Warn".into(),
                pattern: "WARN".into(),
                regex: false
            }]
        );
        assert!(cx.update(|_, cx| SavedSearches::get(cx).filters[0].name == "Errors"));

        // A file without filters leaves the rows and says why.
        editor.update_in(cx, |e, window, cx| {
            e.import_file(&exported.with_extension("txt"), window, cx)
        });
        assert_eq!(editor.read_with(cx, |e, cx| e.filters(cx)).len(), 1);
        let message = editor.read_with(cx, |e, _| e.message.clone().unwrap());
        assert!(message.contains("mine.txt"), "{message}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
