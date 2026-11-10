//! Highlighter sets and color labels in the app (klogg:
//! `HighlighterSetCollection` as a global, `HighlightersMenu`, the color
//! label shortcuts of `CrawlerWidget`).
//!
//! The sets and which ones are active are global, as in klogg: activating a
//! set colours every open file. Color labels belong to one tab (see
//! [`Crawler`](crate::crawler::Crawler)) and are not saved, as in klogg.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Action, App, Global, Hsla, KeyBinding, Menu, MenuItem, Rgba, actions};
use rlogg_core::highlight::{COLOR_LABELS, Color, CompiledSet, HighlighterSets};

use crate::log_view;

actions!(
    highlighters,
    [
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
pub fn key_bindings() -> Vec<KeyBinding> {
    let view = Some(log_view::CONTEXT);
    let mut keys = Vec::new();
    // GPUI's Linux backends have no layout mapping for bindings: they
    // report Ctrl+Shift+1 as the shifted character, `ctrl-!`, and drop the
    // shift. Bind the US layout's shifted digits too (docs/decisions.md).
    // They come first: menus show the binding added last.
    if cfg!(target_os = "linux") {
        for (label, symbol) in US_SHIFTED_DIGITS[1..].iter().enumerate() {
            keys.push(KeyBinding::new(
                &format!("ctrl-{symbol}"),
                SetColorLabel { label },
                view,
            ));
        }
        keys.push(KeyBinding::new(
            &format!("ctrl-{}", US_SHIFTED_DIGITS[0]),
            ClearColorLabels,
            view,
        ));
    }
    for label in 0..COLOR_LABELS {
        keys.push(KeyBinding::new(
            &format!("ctrl-shift-{}", label + 1),
            SetColorLabel { label },
            view,
        ));
    }
    keys.push(KeyBinding::new("ctrl-d", NextColorLabel, view));
    keys.push(KeyBinding::new("ctrl-shift-0", ClearColorLabels, view));
    keys
}

/// Shift+0 … Shift+9 on a US layout.
const US_SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];

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
