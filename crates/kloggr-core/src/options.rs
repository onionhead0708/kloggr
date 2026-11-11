//! User options (klogg: `Configuration`, edited in the options dialog).
//!
//! Saved in `settings.toml` next to the session. Defaults are klogg's
//! (`settings/include/configuration.h`), except where `docs/decisions.md`
//! says otherwise. Options for features kloggr doesn't have (text wrap,
//! archives, regex engine, buffers) are left out until they exist.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::highlight::Color;
use crate::settings::{self, FileContext, SettingsError};

const OPTIONS_FILE: &str = "settings.toml";
const OPTIONS_VERSION: u32 = 1;

/// Light or dark colours (klogg: `view.style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Follow the desktop's appearance.
    #[default]
    System,
    Light,
    Dark,
}

/// The options dialog's General tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Reopen the files of the last session (klogg: `session.loadLast`).
    pub load_last_session: bool,
    /// Follow files when they are opened (klogg: `session.followOnLoad`).
    pub follow_on_load: bool,
    /// Ask GitHub for a newer release once a week (klogg:
    /// `versionchecker.enabled`, also on by default).
    pub check_for_updates: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            load_last_session: true,
            follow_on_load: false,
            check_for_updates: true,
        }
    }
}

/// Search options, and the defaults of the search bar for a new file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    /// klogg: `!defaultView.searchIgnoreCase`.
    pub match_case: bool,
    /// Regular expressions, or plain text (klogg: `regexpType.main`).
    pub regex: bool,
    /// klogg: `defaultView.searchLogicalCombining`.
    pub boolean: bool,
    /// klogg: `defaultView.searchAutoRefresh`.
    pub auto_refresh: bool,
    /// Highlight the search pattern's matches in the text (klogg:
    /// `regexpType.mainHighlight`, off there; on in kloggr).
    pub highlight_matches: bool,
    /// Their background (klogg: `regexpType.mainBackColor`); `None`: a
    /// translucent gray that suits light and dark styles.
    #[serde(with = "crate::highlight::color::optional")]
    pub highlight_color: Option<Color>,
    /// Quick find searches as you type (klogg: `quickfind.incremental`).
    pub quickfind_incremental: bool,
    /// Quick find takes regular expressions (klogg: `regexpType.quickfind`,
    /// plain text by default).
    pub quickfind_regex: bool,
    /// klogg: `!quickfind.ignore_case`.
    pub quickfind_match_case: bool,
    /// Background of quick-find occurrences (klogg:
    /// `regexpType.quickfindBackColor`); `None`: the theme's yellow.
    #[serde(with = "crate::highlight::color::optional")]
    pub quickfind_color: Option<Color>,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            match_case: true,
            regex: true,
            boolean: false,
            auto_refresh: false,
            highlight_matches: true,
            highlight_color: None,
            quickfind_incremental: true,
            quickfind_regex: false,
            quickfind_match_case: true,
            quickfind_color: None,
        }
    }
}

/// The View tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// Monospace font of the log views; `None`: the theme's.
    pub font_family: Option<String>,
    /// In points; `None`: the theme's.
    pub font_size: Option<f32>,
    pub style: Style,
    /// klogg: `view.overviewVisible`.
    pub overview: bool,
    /// klogg: `view.lineNumbersVisibleInMain`, off there.
    pub line_numbers_main: bool,
    /// klogg: `view.lineNumbersVisibleInFiltered`.
    pub line_numbers_filtered: bool,
    /// Long lines wrap at the view's width (klogg: `view.textWrap`).
    pub text_wrap: bool,
    /// Height of the filtered view in a new tab, in pixels (klogg: "Save
    /// splitter position", `splitterSizes`).
    pub filtered_height: f32,
    /// klogg: `view.hideAnsiColorSequences`.
    pub hide_ansi: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: None,
            style: Style::System,
            overview: true,
            line_numbers_main: true,
            line_numbers_filtered: true,
            text_wrap: false,
            filtered_height: 260.,
            hide_ansi: false,
        }
    }
}

/// The File tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct File {
    /// Scrolling down at the bottom of a file starts following it (klogg:
    /// `filewatch.allowFollowOnScroll`).
    pub follow_on_scroll: bool,
    /// Recent files shown in the File menu (klogg: `RecentFiles`'
    /// "Show the number of recently opened files").
    pub recent_files: usize,
    /// Open compressed files and archives by extracting them to a
    /// temporary folder (klogg: `archives.extract`).
    pub extract_archives: bool,
    /// Without asking first (klogg: `archives.extractAlways`).
    pub extract_without_confirmation: bool,
    /// Watch files with the system's change notifications (klogg:
    /// `filewatch.useNative`).
    pub native_watch: bool,
    /// Also check files every [`poll_interval_ms`](Self::poll_interval_ms)
    /// (klogg: `filewatch.usePolling`, on by default on Windows): for network
    /// shares and mounts where notifications don't arrive.
    pub poll: bool,
    /// klogg: `filewatch.pollingIntervalMs`.
    pub poll_interval_ms: u32,
}

/// Polling intervals allowed, in milliseconds (klogg: `PollIntervalMin`,
/// `PollIntervalMax`).
pub const POLL_INTERVAL_MS: std::ops::RangeInclusive<u32> = 10..=3_600_000;

impl Default for File {
    fn default() -> Self {
        Self {
            follow_on_scroll: true,
            recent_files: settings::DEFAULT_RECENT_SHOWN,
            extract_archives: true,
            extract_without_confirmation: false,
            native_watch: true,
            poll: cfg!(windows),
            poll_interval_ms: 2000,
        }
    }
}

/// Everything in `settings.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    version: u32,
    pub general: General,
    pub search: Search,
    pub view: View,
    pub file: File,
    /// Keys of the actions whose shortcuts were changed, by action id
    /// (klogg: `shortcuts.mapping`). An empty list removes the shortcut.
    pub shortcuts: BTreeMap<String, Vec<String>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            version: OPTIONS_VERSION,
            general: General::default(),
            search: Search::default(),
            view: View::default(),
            file: File::default(),
            shortcuts: BTreeMap::new(),
        }
    }
}

impl Options {
    /// The search bar's state for a newly opened file (klogg:
    /// `defaultView.*`).
    pub fn new_file_context(&self) -> FileContext {
        let s = &self.search;
        FileContext {
            match_case: s.match_case,
            regex: s.regex,
            boolean: s.boolean,
            auto_refresh: s.auto_refresh,
            follow: self.general.follow_on_load,
            ..FileContext::default()
        }
    }

    /// Loads from `dir`; a missing, unreadable or unknown-version file
    /// gives the defaults (logged).
    pub fn load(dir: &Path) -> Self {
        let mut options = settings::load_toml::<Self>(dir, OPTIONS_FILE)
            .filter(|o| {
                let known = o.version == OPTIONS_VERSION;
                if !known {
                    tracing::warn!("ignoring {OPTIONS_FILE}: unknown version {}", o.version);
                }
                known
            })
            .unwrap_or_default();
        options.view.font_size = options
            .view
            .font_size
            .filter(|s| s.is_finite() && (4.0..=72.0).contains(s));
        options.view.font_family = options.view.font_family.filter(|f| !f.trim().is_empty());
        options.file.recent_files = options
            .file
            .recent_files
            .clamp(1, settings::MAX_RECENT_FILES);
        options
    }

    pub fn save(&self, dir: &Path) -> Result<(), SettingsError> {
        settings::save_toml(self, dir, OPTIONS_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_klogg() {
        let o = Options::default();
        assert!(o.general.load_last_session && !o.general.follow_on_load);
        assert!(o.general.check_for_updates);
        assert!(o.search.match_case && o.search.regex && !o.search.boolean);
        assert!(!o.search.auto_refresh);
        assert!(o.search.quickfind_incremental && !o.search.quickfind_regex);
        assert!(o.view.overview && o.view.line_numbers_filtered);
        assert!(o.file.follow_on_scroll);
        assert!(o.file.extract_archives && !o.file.extract_without_confirmation);
        assert_eq!(o.new_file_context(), FileContext::default());
    }

    #[test]
    fn new_file_context_uses_the_defaults() {
        let mut o = Options::default();
        o.search.match_case = false;
        o.search.boolean = true;
        o.general.follow_on_load = true;
        let c = o.new_file_context();
        assert!(!c.match_case && c.boolean && c.follow && c.regex);
    }

    #[test]
    fn save_load_and_sanitize() {
        let dir = std::env::temp_dir().join(format!("kloggr-options-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Options::load(&dir), Options::default());
        let mut o = Options::default();
        o.view.font_family = Some("JetBrains Mono".into());
        o.view.font_size = Some(13.0);
        o.view.style = Style::Dark;
        o.search.highlight_color = Color::parse("#80c0c0c0");
        o.shortcuts
            .insert("logview.mark".into(), vec!["ctrl-m".into(), "m".into()]);
        o.shortcuts.insert("quickfind.open".into(), vec![]);
        o.save(&dir).unwrap();
        assert_eq!(Options::load(&dir), o);

        // Partial and out-of-range values.
        std::fs::write(
            dir.join(OPTIONS_FILE),
            "version = 1\n[view]\nfont_size = 900.0\nfont_family = \" \"\n[search]\nregex = false\n",
        )
        .unwrap();
        let o = Options::load(&dir);
        assert_eq!(o.view.font_size, None);
        assert_eq!(o.view.font_family, None);
        assert!(!o.search.regex && o.search.match_case);
        std::fs::write(dir.join(OPTIONS_FILE), "version = 3\n").unwrap();
        assert_eq!(Options::load(&dir), Options::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
