//! Highlighter sets and color labels in the app (klogg:
//! `HighlighterSetCollection` as a global, `HighlightersMenu`, the color
//! label shortcuts of `CrawlerWidget`).
//!
//! The sets and which ones are active are global, as in klogg: activating a
//! set colours every open file. Color labels belong to one tab (see
//! [`Crawler`](crate::crawler::Crawler)) and are not saved, as in klogg.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Action, App, Global, Hsla, Menu, MenuItem, Rgba, actions};
use rlogg_core::highlight::{COLOR_LABELS, Color, CompiledSet, HighlighterSets};

use crate::log_view;
use crate::shortcuts::Shortcut;

actions!(
    highlighters,
    [
        EditHighlighters,
        NoHighlighters,
        NextColorLabel,
        ClearColorLabels,
        RemoveColorLabel
    ]
);

/// Turns a highlighter set on or off (Tools > Highlighters).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = highlighters, no_json)]
pub struct ToggleHighlighterSet {
    pub id: String,
}

/// Gives the selected text color label `label` (0-based), or removes it if
/// it already has that label (klogg: `addColorLabel`).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = highlighters, no_json)]
pub struct SetColorLabel {
    pub label: usize,
}

/// klogg's defaults: Ctrl+Shift+1…9, Ctrl+D, Ctrl+Shift+0.
/// On Linux the shifted symbols are bound too (see [`crate::shortcuts`]).
pub fn shortcuts() -> Vec<Shortcut> {
    let view = Some(log_view::CONTEXT);
    let keys = |digit: usize| vec![format!("ctrl-shift-{digit}")];
    const IDS: [&str; COLOR_LABELS] = [
        "logview.add_color_label_1",
        "logview.add_color_label_2",
        "logview.add_color_label_3",
        "logview.add_color_label_4",
        "logview.add_color_label_5",
        "logview.add_color_label_6",
        "logview.add_color_label_7",
        "logview.add_color_label_8",
        "logview.add_color_label_9",
    ];
    const LABELS: [&str; COLOR_LABELS] = [
        "Highlight text with color 1",
        "Highlight text with color 2",
        "Highlight text with color 3",
        "Highlight text with color 4",
        "Highlight text with color 5",
        "Highlight text with color 6",
        "Highlight text with color 7",
        "Highlight text with color 8",
        "Highlight text with color 9",
    ];
    let mut out: Vec<Shortcut> = (0..COLOR_LABELS)
        .map(|label| Shortcut {
            id: IDS[label],
            label: LABELS[label],
            context: view,
            keys: keys(label + 1),
            action: Box::new(SetColorLabel { label }),
        })
        .collect();
    out.push(Shortcut::new(
        "logview.add_next_color_label",
        "Highlight text with next color",
        view,
        &["ctrl-d"],
        NextColorLabel,
    ));
    out.push(Shortcut {
        id: "logview.clear_color_labels",
        label: "Clear all color labels",
        context: view,
        keys: keys(0),
        action: Box::new(ClearColorLabels),
    });
    out.push(Shortcut::new(
        "mainwindow.highlighters",
        "Highlighters…",
        None,
        &[],
        EditHighlighters,
    ));
    out
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

/// The global highlighter state.
pub struct Highlighters {
    pub sets: HighlighterSets,
    /// The active sets' highlighters, compiled.
    active: Arc<CompiledSet>,
    config_dir: Option<PathBuf>,
}

impl Global for Highlighters {}

impl Highlighters {
    /// Loads the sets from `config_dir` and installs the global.
    pub fn init(config_dir: Option<PathBuf>, cx: &mut App) {
        let sets = config_dir
            .as_deref()
            .map(HighlighterSets::load)
            .unwrap_or_default();
        let mut this = Self {
            sets,
            active: Arc::default(),
            config_dir,
        };
        this.recompile();
        cx.set_global(this);
    }

    /// The active highlighters; empty if the global isn't installed (tests).
    pub fn active(cx: &App) -> Arc<CompiledSet> {
        cx.try_global::<Self>()
            .map(|h| h.active.clone())
            .unwrap_or_default()
    }

    /// Changes the sets, then recompiles, saves and redraws.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut HighlighterSets)) {
        if !cx.has_global::<Self>() {
            Self::init(None, cx);
        }
        let this = cx.global_mut::<Self>();
        f(&mut this.sets);
        this.recompile();
        if let Some(dir) = &this.config_dir
            && let Err(e) = this.sets.save(dir)
        {
            tracing::warn!("cannot save highlighters: {e}");
        }
        cx.refresh_windows();
    }

    fn recompile(&mut self) {
        let (set, errors) = self.sets.compile_active();
        for (pattern, e) in errors {
            tracing::warn!("highlighter {pattern:?} skipped: {e}");
        }
        self.active = Arc::new(set);
    }

    /// The Tools > Highlighters menu (klogg: `HighlightersMenu`): "None",
    /// then each set, checked when active.
    pub fn menu(cx: &App) -> Menu {
        let mut items = vec![MenuItem::action("None", NoHighlighters)];
        if let Some(this) = cx.try_global::<Self>() {
            if !this.sets.sets.is_empty() {
                items.push(MenuItem::separator());
            }
            items.extend(this.sets.sets.iter().map(|set| {
                MenuItem::action(
                    set.name.clone(),
                    ToggleHighlighterSet { id: set.id.clone() },
                )
                .checked(this.sets.is_active(&set.id))
            }));
        }
        Menu::new("Highlighters").items(items)
    }

    /// The styles of the color labels.
    pub fn color_label_styles(cx: &App) -> Vec<rlogg_core::highlight::ColorLabelStyle> {
        cx.try_global::<Self>()
            .map_or_else(rlogg_core::highlight::ColorLabelStyle::defaults, |h| {
                h.sets.color_labels.clone()
            })
    }
}

/// A highlight colour for GPUI.
pub fn hsla(c: Color) -> Hsla {
    Rgba {
        r: f32::from(c.r) / 255.,
        g: f32::from(c.g) / 255.,
        b: f32::from(c.b) / 255.,
        a: f32::from(c.a) / 255.,
    }
    .into()
}
