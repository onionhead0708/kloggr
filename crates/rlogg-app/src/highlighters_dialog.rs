//! The highlighters dialog (klogg: `HighlightersDialog`, `HighlighterSetEdit`,
//! `HighlighterEdit`): edit highlighter sets, their highlighters and the
//! color label styles; import and export klogg `.conf` files.
//!
//! Edits go to a working copy of the global [`HighlighterSets`]; Apply and
//! OK install it (klogg: `resolveDialog`), Cancel drops it.

use std::path::PathBuf;

use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, Hsla, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};
use rlogg_core::highlight::{
    COLOR_LABELS, Color, Highlighter, HighlighterSet, HighlighterSets, klogg_conf,
};

use crate::highlighters::{Highlighters, hsla};

/// klogg: `DEFAULT_NAME`, `DEFAULT_PATTERN`.
const NEW_SET_NAME: &str = "New Highlighter set";
const NEW_PATTERN: &str = "New Highlighter";

/// Emitted when the dialog should close.
pub struct Close;

impl EventEmitter<Close> for HighlightersEditor {}

/// One color picker bound to an optional colour.
struct ColorField {
    state: Entity<ColorPickerState>,
}

impl ColorField {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            state: cx.new(|cx| ColorPickerState::new(window, cx)),
        }
    }

    fn set(&self, color: Option<Color>, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |s, cx| match color {
            Some(c) => s.set_value(hsla(c), window, cx),
            None => s.clear_value(window, cx),
        });
    }
}

/// Which part of the edited data a field belongs to.
#[derive(Clone, Copy)]
enum Target {
    HighlighterFore,
    HighlighterBack,
    LabelFore(usize),
    LabelBack(usize),
}

pub struct HighlightersEditor {
    sets: HighlighterSets,
    tab: usize,
    set: Option<usize>,
    highlighter: Option<usize>,
    set_name: Entity<InputState>,
    pattern: Entity<InputState>,
    variance: Entity<InputState>,
    fore: ColorField,
    back: ColorField,
    label_names: Vec<Entity<InputState>>,
    label_colors: Vec<(ColorField, ColorField)>,
    /// Why the selected pattern doesn't compile.
    pattern_error: Option<String>,
    /// Result of the last import or export.
    message: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl HighlightersEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sets = cx
            .try_global::<Highlighters>()
            .map(|h| h.sets.clone())
            .unwrap_or_default();
        let input =
            |window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let set_name = input(window, cx);
        let pattern = input(window, cx);
        let variance = input(window, cx);
        let fore = ColorField::new(window, cx);
        let back = ColorField::new(window, cx);
        let label_names: Vec<_> = (0..COLOR_LABELS).map(|_| input(window, cx)).collect();
        let label_colors: Vec<_> = (0..COLOR_LABELS)
            .map(|_| (ColorField::new(window, cx), ColorField::new(window, cx)))
            .collect();

        let mut subs = vec![
            cx.subscribe(&set_name, |this, input, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let name = input.read(cx).value().to_string();
                    if let Some(set) = this.current_set_mut() {
                        set.name = name;
                    }
                    cx.notify();
                }
            }),
            cx.subscribe(&pattern, |this, input, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.edit_highlighter(|h| h.pattern = text, cx);
                }
            }),
            cx.subscribe(&variance, |this, input, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change)
                    && let Ok(v) = input.read(cx).value().trim().parse::<u32>()
                {
                    this.edit_highlighter(|h| h.color_variance = v.min(100), cx);
                }
            }),
        ];
        let mut watch_color =
            |state: &Entity<ColorPickerState>, target: Target, cx: &mut Context<Self>| {
                subs.push(
                    cx.subscribe(state, move |this, _, e: &ColorPickerEvent, cx| {
                        let ColorPickerEvent::Change(value) = e;
                        this.set_color(target, value.map(to_color), cx);
                    }),
                );
            };
        watch_color(&fore.state, Target::HighlighterFore, cx);
        watch_color(&back.state, Target::HighlighterBack, cx);
        for (i, (f, b)) in label_colors.iter().enumerate() {
            watch_color(&f.state, Target::LabelFore(i), cx);
            watch_color(&b.state, Target::LabelBack(i), cx);
        }
        for (i, name) in label_names.iter().enumerate() {
            subs.push(cx.subscribe(name, move |this, input, e: &InputEvent, cx| {
                // klogg ignores an empty name.
                let text = input.read(cx).value().to_string();
                if matches!(e, InputEvent::Change) && !text.is_empty() {
                    this.sets.color_labels[i].name = text;
                }
            }));
        }

        let mut this = Self {
            set: (!sets.sets.is_empty()).then_some(0),
            sets,
            tab: 0,
            highlighter: None,
            set_name,
            pattern,
            variance,
            fore,
            back,
            label_names,
            label_colors,
            pattern_error: None,
            message: None,
            _subscriptions: subs,
        };
        this.highlighter = this
            .current_set()
            .and_then(|s| (!s.highlighters.is_empty()).then_some(0));
        this.load_set(window, cx);
        this.load_labels(window, cx);
        this
    }

    /// The edited copy.
    #[cfg(test)]
    pub fn sets(&self) -> &HighlighterSets {
        &self.sets
    }

    fn current_set(&self) -> Option<&HighlighterSet> {
        self.sets.sets.get(self.set?)
    }

    fn current_set_mut(&mut self) -> Option<&mut HighlighterSet> {
        self.sets.sets.get_mut(self.set?)
    }

    fn current_highlighter(&self) -> Option<&Highlighter> {
        self.current_set()?.highlighters.get(self.highlighter?)
    }

    fn edit_highlighter(&mut self, f: impl FnOnce(&mut Highlighter), cx: &mut Context<Self>) {
        let index = self.highlighter;
        if let Some(h) = self
            .current_set_mut()
            .and_then(|s| s.highlighters.get_mut(index?))
        {
            f(h);
        }
        self.check_pattern();
        cx.notify();
    }

    fn check_pattern(&mut self) {
        self.pattern_error = self
            .current_highlighter()
            .and_then(|h| h.compile().err())
            .map(|e| e.to_string());
    }

    fn set_color(&mut self, target: Target, color: Option<Color>, cx: &mut Context<Self>) {
        match target {
            Target::HighlighterFore => self.edit_highlighter(|h| h.fore_color = color, cx),
            Target::HighlighterBack => self.edit_highlighter(|h| h.back_color = color, cx),
            Target::LabelFore(i) => self.sets.color_labels[i].fore_color = color,
            Target::LabelBack(i) => self.sets.color_labels[i].back_color = color,
        }
        cx.notify();
    }

    /// Fills the set fields from the selected set, then its highlighter.
    fn load_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .current_set()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        self.set_name
            .update(cx, |s, cx| s.set_value(name, window, cx));
        self.load_highlighter(window, cx);
    }

    fn load_highlighter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let h = self.current_highlighter().cloned().unwrap_or_default();
        self.pattern
            .update(cx, |s, cx| s.set_value(h.pattern.clone(), window, cx));
        self.variance.update(cx, |s, cx| {
            s.set_value(h.color_variance.to_string(), window, cx)
        });
        self.fore.set(h.fore_color, window, cx);
        self.back.set(h.back_color, window, cx);
        self.check_pattern();
        cx.notify();
    }

    fn load_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (i, label) in self.sets.color_labels.iter().enumerate() {
            let name = label.name.clone();
            self.label_names[i].update(cx, |s, cx| s.set_value(name, window, cx));
            self.label_colors[i].0.set(label.fore_color, window, cx);
            self.label_colors[i].1.set(label.back_color, window, cx);
        }
    }

    fn select_set(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set = Some(index);
        self.highlighter = self
            .current_set()
            .and_then(|s| (!s.highlighters.is_empty()).then_some(0));
        self.load_set(window, cx);
    }

    fn select_highlighter(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighter = Some(index);
        self.load_highlighter(window, cx);
    }

    pub fn add_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sets.sets.push(HighlighterSet::new(NEW_SET_NAME));
        self.select_set(self.sets.sets.len() - 1, window, cx);
    }

    fn remove_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.set else { return };
        let removed = self.sets.sets.remove(i);
        self.sets.deactivate(&removed.id);
        self.set = (!self.sets.sets.is_empty()).then(|| i.min(self.sets.sets.len() - 1));
        match self.set {
            Some(i) => self.select_set(i, window, cx),
            None => {
                self.highlighter = None;
                self.load_set(window, cx);
            }
        }
    }

    fn move_set(&mut self, up: bool, cx: &mut Context<Self>) {
        if let Some(to) = move_item(&mut self.sets.sets, self.set, up) {
            self.set = Some(to);
            cx.notify();
        }
    }

    pub fn add_highlighter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.current_set_mut() else {
            return;
        };
        set.highlighters.push(Highlighter {
            pattern: NEW_PATTERN.into(),
            ..Highlighter::default()
        });
        let last = set.highlighters.len() - 1;
        self.select_highlighter(last, window, cx);
    }

    fn remove_highlighter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.highlighter;
        let Some(set) = self.current_set_mut() else {
            return;
        };
        let Some(i) = index.filter(|&i| i < set.highlighters.len()) else {
            return;
        };
        set.highlighters.remove(i);
        let len = set.highlighters.len();
        self.highlighter = (len > 0).then(|| i.min(len - 1));
        self.load_highlighter(window, cx);
    }

    fn move_highlighter(&mut self, up: bool, cx: &mut Context<Self>) {
        let index = self.highlighter;
        if let Some(set) = self.current_set_mut()
            && let Some(to) = move_item(&mut set.highlighters, index, up)
        {
            self.highlighter = Some(to);
            cx.notify();
        }
    }

    /// Installs the edited sets (klogg: Apply).
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let sets = self.sets.clone();
        Highlighters::update(cx, |s| *s = sets);
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.import_files(&paths, window, cx))
                .ok();
        })
        .detach();
    }

    /// Adds the new sets of klogg `.conf` files (klogg:
    /// `importHighlighters`).
    pub fn import_files(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let mut added = 0;
        let mut errors = Vec::new();
        for path in paths {
            match std::fs::read_to_string(path) {
                Ok(text) => added += self.sets.merge_imported(klogg_conf::import(&text)),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        self.message = Some(if errors.is_empty() {
            format!("Imported {added} highlighter set(s)")
        } else {
            errors.join("; ")
        });
        if self.set.is_none() && !self.sets.sets.is_empty() {
            self.select_set(0, window, cx);
        }
        cx.notify();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = std::env::home_dir().unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some("highlighters.conf"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update(cx, |this, cx| this.export_to(path, cx)).ok();
        })
        .detach();
    }

    /// Writes the edited sets as a klogg `.conf` file (klogg adds the
    /// extension if missing).
    pub fn export_to(&mut self, mut path: PathBuf, cx: &mut Context<Self>) {
        if path.extension().is_none_or(|e| e != "conf") {
            path.as_mut_os_string().push(".conf");
        }
        self.message = Some(
            match std::fs::write(&path, klogg_conf::export(&self.sets)) {
                Ok(()) => format!("Exported to {}", path.display()),
                Err(e) => format!("{}: {e}", path.display()),
            },
        );
        cx.notify();
    }
}

/// Moves `items[index]` up or down one place; returns its new index.
fn move_item<T>(items: &mut [T], index: Option<usize>, up: bool) -> Option<usize> {
    let i = index?;
    let to = if up { i.checked_sub(1)? } else { i + 1 };
    if to >= items.len() || i >= items.len() {
        return None;
    }
    items.swap(i, to);
    Some(to)
}

fn to_color(c: Hsla) -> Color {
    let rgba = c.to_rgb();
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Color {
        r: channel(rgba.r),
        g: channel(rgba.g),
        b: channel(rgba.b),
        a: channel(rgba.a),
    }
}

/// A selectable row of a list.
fn list_row(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    swatch: Option<(Option<Hsla>, Option<Hsla>)>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let theme = cx.theme();
    let (fg, bg) = swatch.unwrap_or((None, None));
    div()
        .id(id.into())
        .px_2()
        .py_0p5()
        .rounded_sm()
        .cursor_pointer()
        .text_sm()
        .border_1()
        .when(selected, |d| d.bg(theme.accent).border_color(theme.primary))
        .when(!selected, |d| {
            d.border_color(gpui::transparent_black())
                .hover(|d| d.bg(theme.muted))
        })
        .child(
            div()
                .px_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .when_some(bg, |d, bg| d.bg(bg))
                .when_some(fg, |d, fg| d.text_color(fg))
                .child(label.into()),
        )
}

fn list_buttons(
    prefix: &'static str,
    can_edit: bool,
    can_up: bool,
    can_down: bool,
    on: impl Fn(&'static str, &ClickEvent, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let button = |name: &'static str, label: &'static str, enabled: bool, tooltip: &'static str| {
        let on = on.clone();
        Button::new(SharedString::from(format!("{prefix}-{name}")))
            .label(label)
            .xsmall()
            .ghost()
            .tooltip(tooltip)
            .disabled(!enabled)
            .on_click(move |e, window, cx| on(name, e, window, cx))
    };
    h_flex()
        .gap_1()
        .child(button("add", "+", true, "Add"))
        .child(button("remove", "−", can_edit, "Remove"))
        .child(button("up", "↑", can_up, "Move up"))
        .child(button("down", "↓", can_down, "Move down"))
}

impl HighlightersEditor {
    fn render_sets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let rows = self.sets.sets.iter().enumerate().map(|(i, set)| {
            list_row(("set", i), set.name.clone(), self.set == Some(i), None, cx)
                .on_click(cx.listener(move |this, _, window, cx| this.select_set(i, window, cx)))
        });
        let n = self.sets.sets.len();
        let entity = cx.entity();
        v_flex()
            .w(px(200.))
            .gap_1()
            .child(div().text_sm().child("Highlighter sets"))
            .child(
                v_flex()
                    .id("sets")
                    .h(px(360.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_1()
                    .children(rows),
            )
            .child(list_buttons(
                "set",
                self.set.is_some(),
                self.set.is_some_and(|i| i > 0),
                self.set.is_some_and(|i| i + 1 < n),
                move |name, _, window, cx| {
                    entity.update(cx, |this, cx| match name {
                        "add" => this.add_set(window, cx),
                        "remove" => this.remove_set(window, cx),
                        "up" => this.move_set(true, cx),
                        _ => this.move_set(false, cx),
                    })
                },
            ))
    }

    fn render_set(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, danger, muted) = (theme.border, theme.danger, theme.muted_foreground);
        let Some(set) = self.current_set() else {
            return v_flex().flex_1().child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No highlighter set"),
            );
        };
        let rows: Vec<_> = set
            .highlighters
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let label = if h.pattern.is_empty() {
                    "(empty)".to_owned()
                } else {
                    h.pattern.clone()
                };
                let swatch = (h.fore_color.map(hsla), h.back_color.map(hsla));
                list_row(
                    ("hl", i),
                    label,
                    self.highlighter == Some(i),
                    Some(swatch),
                    cx,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.select_highlighter(i, window, cx)),
                )
            })
            .collect();
        let n = set.highlighters.len();
        let entity = cx.entity();
        let h = self.current_highlighter().cloned();
        let check = |id: &'static str,
                     label: &'static str,
                     value: bool,
                     enabled: bool,
                     f: fn(&mut Highlighter, bool),
                     cx: &mut Context<Self>| {
            Checkbox::new(id)
                .label(label)
                .checked(value)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    let checked = *checked;
                    this.edit_highlighter(|h| f(h, checked), cx)
                }))
        };
        let editor = h.map(|h| {
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().w(px(90.)).text_sm().child("Pattern"))
                        .child(div().flex_1().child(Input::new(&self.pattern).small())),
                )
                .children(self.pattern_error.clone().map(|e| {
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(e.lines().last().unwrap_or_default().to_owned())
                }))
                .child(
                    h_flex()
                        .gap_4()
                        .child(check(
                            "ignore-case",
                            "Ignore case",
                            h.ignore_case,
                            true,
                            |h, v| h.ignore_case = v,
                            cx,
                        ))
                        .child(check(
                            "use-regex",
                            "Regular expression",
                            h.use_regex,
                            true,
                            |h, v| h.use_regex = v,
                            cx,
                        ))
                        .child(check(
                            "match-only",
                            "Highlight only match",
                            h.match_only,
                            true,
                            |h, v| h.match_only = v,
                            cx,
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(check(
                            "variate",
                            "Variate colors",
                            h.variate_colors,
                            h.match_only,
                            |h, v| h.variate_colors = v,
                            cx,
                        ))
                        .child(div().text_sm().text_color(muted).child("by ±"))
                        .child(div().w(px(56.)).child(Input::new(&self.variance).xsmall()))
                        .child(div().text_sm().text_color(muted).child("%")),
                )
                .child(
                    h_flex()
                        .gap_4()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().text_sm().child("Foreground"))
                                .child(ColorPicker::new(&self.fore.state).small()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().text_sm().child("Background"))
                                .child(ColorPicker::new(&self.back.state).small()),
                        ),
                )
        });
        v_flex()
            .flex_1()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(90.)).text_sm().child("Name"))
                    .child(div().flex_1().child(Input::new(&self.set_name).small())),
            )
            .child(div().text_sm().child("Highlighters (the first match wins)"))
            .child(
                v_flex()
                    .id("highlighters")
                    .h(px(140.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_1()
                    .children(rows),
            )
            .child(list_buttons(
                "hl",
                self.highlighter.is_some(),
                self.highlighter.is_some_and(|i| i > 0),
                self.highlighter.is_some_and(|i| i + 1 < n),
                move |name, _, window, cx| {
                    entity.update(cx, |this, cx| match name {
                        "add" => this.add_highlighter(window, cx),
                        "remove" => this.remove_highlighter(window, cx),
                        "up" => this.move_highlighter(true, cx),
                        _ => this.move_highlighter(false, cx),
                    })
                },
            ))
            .children(editor)
    }

    fn render_labels(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let header = h_flex()
            .gap_3()
            .text_sm()
            .text_color(muted)
            .child(div().w(px(260.)).child("Name"))
            .child(div().w(px(90.)).child("Foreground"))
            .child(div().w(px(90.)).child("Background"))
            .child(div().child("Ctrl+D cycle"));
        let rows = (0..COLOR_LABELS).map(|i| {
            let (fore, back) = &self.label_colors[i];
            let cycle = self.sets.color_labels[i].cycle;
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(px(260.))
                        .child(Input::new(&self.label_names[i]).small()),
                )
                .child(
                    div()
                        .w(px(90.))
                        .child(ColorPicker::new(&fore.state).small()),
                )
                .child(
                    div()
                        .w(px(90.))
                        .child(ColorPicker::new(&back.state).small()),
                )
                .child(
                    Checkbox::new(("cycle", i))
                        .checked(cycle)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.sets.color_labels[i].cycle = *checked;
                            cx.notify();
                        })),
                )
        });
        v_flex().gap_2().child(header).children(rows)
    }
}

impl Render for HighlightersEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let body = if self.tab == 0 {
            h_flex()
                .items_start()
                .gap_4()
                .child(self.render_sets(cx))
                .child(self.render_set(cx))
                .into_any_element()
        } else {
            self.render_labels(cx).into_any_element()
        };
        v_flex()
            .id("highlighters-editor")
            .gap_3()
            .child(
                TabBar::new("highlighters-tabs")
                    .underline()
                    .small()
                    .selected_index(self.tab)
                    .child(Tab::new().label("Highlighter sets"))
                    .child(Tab::new().label("Color labels"))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.tab = *index;
                        cx.notify();
                    })),
            )
            .child(div().min_h(px(440.)).child(body))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("import")
                            .label("Import…")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
                    )
                    .child(
                        Button::new("export")
                            .label("Export…")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    )
                    .children(
                        self.message
                            .clone()
                            .map(|m| div().text_xs().text_color(muted).child(m)),
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
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    )
                    .child(
                        Button::new("ok")
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

/// Opens the dialog (klogg: Tools > Highlighters…).
pub fn open(window: &mut Window, cx: &mut App) {
    let editor = cx.new(|cx| HighlightersEditor::new(window, cx));
    window
        .subscribe(&editor, cx, |_, _: &Close, window, cx| {
            window.close_dialog(cx)
        })
        .detach();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Highlighters")
            .w(px(900.))
            .child(editor.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn editor(cx: &mut TestAppContext) -> (Entity<HighlightersEditor>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Highlighters::init(None, cx);
        });
        cx.add_window_view(HighlightersEditor::new)
    }

    fn global(cx: &mut VisualTestContext) -> HighlighterSets {
        cx.update(|_, cx| cx.global::<Highlighters>().sets.clone())
    }

    #[gpui::test]
    fn edit_apply_and_cancel(cx: &mut TestAppContext) {
        let (editor, cx) = editor(cx);
        editor.update_in(cx, |e, window, cx| {
            e.add_set(window, cx);
            e.add_highlighter(window, cx);
            e.add_highlighter(window, cx);
            e.edit_highlighter(|h| h.pattern = "(".into(), cx);
            assert!(e.pattern_error.is_some(), "invalid regex is reported");
            e.edit_highlighter(|h| h.pattern = "ERROR".into(), cx);
            assert!(e.pattern_error.is_none());
            e.move_highlighter(true, cx);
            e.set_color(Target::LabelBack(0), Color::parse("#123456"), cx);
        });
        // Nothing changes until Apply.
        assert!(global(cx).sets.is_empty());
        editor.update(cx, |e, cx| e.apply(cx));
        let sets = global(cx);
        assert_eq!(sets.sets[0].name, NEW_SET_NAME);
        let patterns: Vec<_> = sets.sets[0]
            .highlighters
            .iter()
            .map(|h| h.pattern.as_str())
            .collect();
        assert_eq!(patterns, ["ERROR", NEW_PATTERN]);
        assert_eq!(sets.color_labels[0].back_color, Color::parse("#123456"));

        // Removing an active set deactivates it.
        let id = sets.sets[0].id.clone();
        cx.update(|_, cx| Highlighters::update(cx, |s| s.activate(&id)));
        let editor = cx.update(|window, cx| cx.new(|cx| HighlightersEditor::new(window, cx)));
        editor.update_in(cx, |e, window, cx| {
            assert_eq!(e.sets().active, std::slice::from_ref(&id));
            e.remove_set(window, cx);
            assert!(e.sets().sets.is_empty() && e.sets().active.is_empty());
        });
    }

    #[gpui::test]
    fn export_then_import(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("rlogg-hl-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (editor, cx) = editor(cx);
        editor.update_in(cx, |e, window, cx| {
            e.add_set(window, cx);
            e.add_highlighter(window, cx);
            e.export_to(dir.join("mine"), cx);
            assert!(e.message.as_ref().unwrap().starts_with("Exported"));
            // The same set again is skipped (same id and name).
            e.import_files(&[dir.join("mine.conf")], window, cx);
            assert_eq!(e.sets().sets.len(), 1);
            e.remove_set(window, cx);
            e.import_files(
                &[dir.join("mine.conf"), dir.join("missing.conf")],
                window,
                cx,
            );
            assert_eq!(e.sets().sets.len(), 1);
            assert_eq!(e.sets().sets[0].highlighters[0].pattern, NEW_PATTERN);
            assert!(e.message.as_ref().unwrap().contains("missing.conf"));
            assert_eq!(e.set, Some(0));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
