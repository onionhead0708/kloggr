//! Search history and predefined filters (klogg: `SavedSearches`,
//! `PredefinedFiltersCollection`), saved in `searches.toml`, and the rules
//! klogg uses to build a search pattern from pieces (`CrawlerWidget::
//! escapeSearchPattern`, `combinePatterns`, `addToSearch`,
//! `excludeFromSearch`; GPLv3, Copyright (C) 2016 Anton Filimonov and other
//! contributors).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::settings::{self, SettingsError};

/// Searches remembered (klogg: `MaxNumberOfRecentSearches`).
pub const DEFAULT_HISTORY_SIZE: usize = 50;

const SEARCHES_FILE: &str = "searches.toml";
const SEARCHES_VERSION: u32 = 1;

/// A named pattern (klogg: `PredefinedFilter`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredefinedFilter {
    pub name: String,
    pub pattern: String,
    /// A regular expression, or plain text.
    #[serde(default = "yes")]
    pub regex: bool,
}

fn yes() -> bool {
    true
}

/// Everything in `searches.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Searches {
    version: u32,
    /// How many searches to remember (klogg: `historySize`).
    pub history_size: usize,
    /// Recent searches, most recent first (klogg: `searchHistory`).
    pub history: Vec<String>,
    pub filters: Vec<PredefinedFilter>,
}

impl Default for Searches {
    fn default() -> Self {
        Self {
            version: SEARCHES_VERSION,
            history_size: DEFAULT_HISTORY_SIZE,
            history: Vec::new(),
            filters: Vec::new(),
        }
    }
}

impl Searches {
    /// Puts `text` first in the history (klogg: `addRecent`); blank
    /// searches are not kept.
    pub fn add_recent(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.history.retain(|t| t != text);
        self.history.insert(0, text.to_owned());
        self.trim();
    }

    /// Replaces the history with `lines`, most recent first, skipping blank
    /// lines (klogg: `editSearchHistory`).
    pub fn set_history(&mut self, lines: &str) {
        let items: Vec<&str> = lines.lines().filter(|l| !l.is_empty()).collect();
        self.history.clear();
        for item in items.iter().rev() {
            self.add_recent(item);
        }
    }

    pub fn set_history_size(&mut self, size: usize) {
        self.history_size = size;
        self.trim();
    }

    fn trim(&mut self) {
        self.history.truncate(self.history_size);
    }

    pub fn load(dir: &Path) -> Self {
        let mut searches = settings::load_toml::<Self>(dir, SEARCHES_FILE)
            .filter(|s| {
                let known = s.version == SEARCHES_VERSION;
                if !known {
                    tracing::warn!("ignoring {SEARCHES_FILE}: unknown version {}", s.version);
                }
                known
            })
            .unwrap_or_default();
        searches.trim();
        searches
    }

    pub fn save(&self, dir: &Path) -> Result<(), SettingsError> {
        settings::save_toml(self, dir, SEARCHES_FILE)
    }
}

/// How the search box reads its text: the state of its regex and boolean
/// toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub regex: bool,
    pub boolean: bool,
}

/// `text` as a piece of a search in `mode` (klogg: `escapeSearchPattern`):
/// plain text is escaped for a regex search, and in boolean mode the piece
/// is quoted (with its quotes escaped, which klogg forgets).
pub fn escape(text: &str, is_regex: bool, mode: Mode) -> String {
    let mut piece = if !is_regex && mode.regex {
        regex::escape(text)
    } else {
        text.to_owned()
    };
    if mode.boolean {
        piece = format!("\"{}\"", piece.replace('"', "\\\""));
    }
    piece
}

/// The separator between alternatives in `mode`: `or` in boolean mode, `|`
/// for a regex; plain text has none (klogg: `combinePatterns`).
fn separator(mode: Mode) -> &'static str {
    if mode.boolean {
        " or "
    } else if mode.regex {
        "|"
    } else {
        ""
    }
}

/// `current`, or `piece`, as one search (klogg: `combinePatterns`).
pub fn combine(current: &str, piece: &str, mode: Mode) -> String {
    if current.is_empty() {
        piece.to_owned()
    } else {
        format!("{current}{}{piece}", separator(mode))
    }
}

/// A search for the lines of `current` that don't contain `text` (klogg:
/// `excludeFromSearch`). It is a boolean search: the returned mode has
/// `boolean` set. An empty `current` gives just `not(...)`, where klogg
/// would write `"" and not(...)`.
pub fn exclude(current: &str, text: &str, mode: Mode) -> (String, Mode) {
    let current = if mode.boolean || current.is_empty() {
        current.to_owned()
    } else {
        format!("\"{}\"", current.replace('"', "\\\""))
    };
    let mode = Mode {
        boolean: true,
        ..mode
    };
    let piece = format!("not({})", escape(text, false, mode));
    let search = if current.is_empty() {
        piece
    } else {
        format!("{current} and {piece}")
    };
    (search, mode)
}

/// The search for the checked predefined filters (klogg:
/// `setSearchPatternFromPredefinedFilters`).
pub fn from_filters<'a>(
    filters: impl IntoIterator<Item = &'a PredefinedFilter>,
    mode: Mode,
) -> String {
    filters.into_iter().fold(String::new(), |search, f| {
        combine(&search, &escape(&f.pattern, f.regex, mode), mode)
    })
}

/// Which `filters` the search `current` is made of, to check them in the
/// dropdown (klogg: `PredefinedFiltersComboBox::showPopup`).
pub fn checked_filters(current: &str, filters: &[PredefinedFilter], mode: Mode) -> Vec<bool> {
    let sep = separator(mode);
    let parts: Vec<&str> = if sep.is_empty() {
        vec![current]
    } else {
        current.split(sep).collect()
    };
    filters
        .iter()
        .map(|f| {
            let piece = escape(&f.pattern, f.regex, mode);
            parts.contains(&piece.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGEX: Mode = Mode {
        regex: true,
        boolean: false,
    };
    const PLAIN: Mode = Mode {
        regex: false,
        boolean: false,
    };
    const BOOLEAN: Mode = Mode {
        regex: true,
        boolean: true,
    };

    fn filter(name: &str, pattern: &str, regex: bool) -> PredefinedFilter {
        PredefinedFilter {
            name: name.into(),
            pattern: pattern.into(),
            regex,
        }
    }

    #[test]
    fn history_is_most_recent_first_and_capped() {
        let mut s = Searches::default();
        s.add_recent("a");
        s.add_recent("b");
        s.add_recent("a");
        s.add_recent("");
        assert_eq!(s.history, ["a", "b"]);
        for i in 0..60 {
            s.add_recent(&i.to_string());
        }
        assert_eq!(s.history.len(), DEFAULT_HISTORY_SIZE);
        assert_eq!(s.history[0], "59");
        s.set_history_size(3);
        assert_eq!(s.history, ["59", "58", "57"]);
        s.set_history("x\n\ny\nx\nz\n");
        assert_eq!(s.history, ["x", "y", "z"]);
    }

    #[test]
    fn escape_and_combine() {
        assert_eq!(escape("a.b", false, REGEX), r"a\.b");
        assert_eq!(escape("a.b", true, REGEX), "a.b");
        assert_eq!(escape("a.b", false, PLAIN), "a.b");
        assert_eq!(escape(r#"say "hi""#, true, BOOLEAN), r#""say \"hi\"""#);
        assert_eq!(combine("", "x", REGEX), "x");
        assert_eq!(combine("x", "y", REGEX), "x|y");
        assert_eq!(combine(r#""x""#, r#""y""#, BOOLEAN), r#""x" or "y""#);
        assert_eq!(combine("x", "y", PLAIN), "xy");
    }

    #[test]
    fn exclude_switches_to_boolean() {
        let (s, m) = exclude("error", "timeout", REGEX);
        assert_eq!(s, r#""error" and not("timeout")"#);
        assert!(m.boolean && m.regex);
        let (s, _) = exclude(r#""error""#, "a.b", BOOLEAN);
        assert_eq!(s, r#""error" and not("a\.b")"#);
        assert_eq!(exclude("", "x", REGEX).0, r#"not("x")"#);
        // The result parses as a boolean search.
        let p = crate::search::Pattern::regex(&s).boolean();
        let m = crate::search::Matcher::new(&p).unwrap();
        assert!(m.is_match(b"error here"));
        assert!(!m.is_match(b"error a.b"));
        assert!(!m.is_match(b"none"));
    }

    #[test]
    fn filters_combine_and_are_recognized() {
        let filters = [
            filter("Errors", "ERROR|FATAL", true),
            filter("Dots", "a.b", false),
            filter("Warn", "WARN", true),
        ];
        let search = from_filters([&filters[0], &filters[1]], REGEX);
        assert_eq!(search, r"ERROR|FATAL|a\.b");
        // A filter's own `|` isn't told apart from the separator in regex
        // mode, as in klogg; Dots is recognized.
        assert_eq!(
            checked_filters(&search, &filters, REGEX),
            [false, true, false]
        );
        let search = from_filters([&filters[0], &filters[2]], BOOLEAN);
        assert_eq!(search, r#""ERROR|FATAL" or "WARN""#);
        assert_eq!(
            checked_filters(&search, &filters, BOOLEAN),
            [true, false, true]
        );
        assert_eq!(checked_filters("", &filters, REGEX), [false, false, false]);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("rlogg-searches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Searches::load(&dir), Searches::default());
        let mut s = Searches::default();
        s.add_recent("one");
        s.filters.push(filter("F", "x", false));
        s.save(&dir).unwrap();
        assert_eq!(Searches::load(&dir), s);
        std::fs::write(
            dir.join(SEARCHES_FILE),
            "version = 1\nhistory_size = 1\nhistory = [\"a\", \"b\"]\n[[filters]]\nname = \"n\"\npattern = \"p\"\n",
        )
        .unwrap();
        let s = Searches::load(&dir);
        assert_eq!(s.history, ["a"]);
        assert!(s.filters[0].regex);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
