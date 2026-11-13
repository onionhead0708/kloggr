//! The user's options in the app (klogg: `Configuration::get()`), and
//! applying them: style, font, shortcuts, and the views that read them on
//! each frame.

use std::path::PathBuf;

use gpui::{App, Global, Hsla, Window, px};
use gpui_component::{Theme, ThemeMode};
use kloggr_core::options::{Options, Style, TextRendering};

use crate::highlighters::hsla;
use crate::shortcuts;

pub struct Preferences {
    pub options: Options,
    config_dir: Option<PathBuf>,
    /// The theme's own monospace font, used when the options name none.
    default_font: Option<(gpui::SharedString, gpui::Pixels)>,
}

impl Global for Preferences {}

impl Preferences {
    /// Loads the options, binds the shortcuts and installs the global.
    pub fn init(config_dir: Option<PathBuf>, cx: &mut App) {
        let options = config_dir.as_deref().map(Options::load).unwrap_or_default();
        shortcuts::bind_all(&options.shortcuts, cx);
        cx.set_global(Self {
            options,
            config_dir,
            default_font: None,
        });
    }

    /// kloggr's configuration folder, if there is one.
    pub fn config_dir(cx: &App) -> Option<PathBuf> {
        cx.try_global::<Self>().and_then(|p| p.config_dir.clone())
    }

    /// The options; the defaults if the global isn't installed (tests).
    pub fn get(cx: &App) -> &Options {
        static DEFAULT: std::sync::OnceLock<Options> = std::sync::OnceLock::new();
        cx.try_global::<Self>()
            .map(|p| &p.options)
            .unwrap_or_else(|| DEFAULT.get_or_init(Options::default))
    }

    /// Replaces the options: saves them, rebinds changed shortcuts and
    /// applies the style and font (klogg: `OptionsDialog::updateConfigFromDialog`).
    pub fn set(options: Options, window: Option<&mut Window>, cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self {
                options: Options::default(),
                config_dir: None,
                default_font: None,
            });
        }
        let old = std::mem::replace(&mut cx.global_mut::<Self>().options, options);
        let this = cx.global::<Self>();
        if old.shortcuts != this.options.shortcuts {
            let new = this.options.shortcuts.clone();
            shortcuts::rebind(&old.shortcuts, &new, cx);
        }
        if let Some(dir) = &cx.global::<Self>().config_dir
            && let Err(e) = cx.global::<Self>().options.save(dir)
        {
            tracing::warn!("cannot save options: {e}");
        }
        Self::apply_style(window, cx);
        cx.refresh_windows();
    }

    /// Applies the style and font to the theme.
    pub fn apply_style(window: Option<&mut Window>, cx: &mut App) {
        let view = Self::get(cx).view.clone();
        let mode = match view.style {
            Style::Light => Some(ThemeMode::Light),
            Style::Dark => Some(ThemeMode::Dark),
            Style::System => None,
        };
        match mode {
            Some(mode) => Theme::change(mode, window, cx),
            None => Theme::sync_system_appearance(window, cx),
        }
        // Remember the theme's font the first time, to go back to it.
        if cx.has_global::<Self>() && cx.global::<Self>().default_font.is_none() {
            let theme = Theme::global(cx);
            let font = (theme.mono_font_family.clone(), theme.mono_font_size);
            cx.global_mut::<Self>().default_font = Some(font);
        }
        cx.set_text_rendering_mode(match view.text_rendering {
            TextRendering::System => gpui::TextRenderingMode::PlatformDefault,
            TextRendering::Subpixel => gpui::TextRenderingMode::Subpixel,
            TextRendering::Grayscale => gpui::TextRenderingMode::Grayscale,
        });
        let default = cx.try_global::<Self>().and_then(|p| p.default_font.clone());
        let theme = Theme::global_mut(cx);
        if let Some((family, size)) = default {
            theme.mono_font_family = view.font_family.map_or(family, Into::into);
            theme.mono_font_size = view.font_size.map_or(size, px);
        }
    }

    /// Background of the search pattern's matches: the option's colour,
    /// else a translucent `fg` (klogg's light gray, in any style).
    pub fn search_highlight(fg: Hsla, cx: &App) -> Hsla {
        Self::get(cx)
            .search
            .highlight_color
            .map_or(fg.opacity(0.18), hsla)
    }

    /// Background of quick-find occurrences (klogg: yellow).
    pub fn quickfind_highlight(yellow: Hsla, cx: &App) -> Hsla {
        Self::get(cx)
            .search
            .quickfind_color
            .map_or(yellow.opacity(0.45), hsla)
    }
}
