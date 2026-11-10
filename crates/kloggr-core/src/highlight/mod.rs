//! Highlighters, highlighter sets and color labels: colours for the lines,
//! or the parts of lines, that match a pattern.
//!
//! Ports klogg's `Highlighter`, `HighlighterSet`, `HighlighterSetCollection`
//! (`src/ui/src/highlighterset.cpp`) and `ColorLabelsManager`
//! (`src/ui/src/colorlabelsmanager.cpp`), GPLv3:
//! Copyright (C) 2009, 2010, 2011 Nicolas Bonnefon and other contributors,
//! Copyright (C) 2019, 2021 Anton Filimonov and other contributors.
//!
//! - [`Highlighter`]: a pattern and its colours, for the whole line or only
//!   the matching text (or its capture groups).
//! - [`HighlighterSet`]: a named, ordered list of highlighters. Any number of
//!   sets can be active; their highlighters are combined in collection order.
//! - [`HighlighterSets`]: every set, which are active, and the styles of the
//!   nine color labels. Saved in `highlighters.toml`.
//! - [`ColorLabels`]: the strings marked with each color label in one tab.
//!
//! Matching works on decoded text and returns byte ranges; the caller maps
//! them to display columns.

pub(crate) mod color;
pub mod klogg_conf;
mod ranges;

use std::hash::{BuildHasher, Hasher};
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

pub use color::Color;
pub use ranges::{Colors, Span, Spans};

use crate::settings::{self, SettingsError};

/// Default colour variance in percent (klogg: `colorVariance_`).
pub const DEFAULT_COLOR_VARIANCE: u32 = 15;

/// A pattern and the colours of what it matches (klogg: `Highlighter`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlighter {
    pub pattern: String,
    pub ignore_case: bool,
    /// Regular expression, or plain text.
    pub use_regex: bool,
    /// Colour only the matches (or their capture groups), not the line.
    pub match_only: bool,
    /// Vary the colours slightly with the matched text (only with
    /// `match_only`).
    pub variate_colors: bool,
    /// How much the colours vary, in percent.
    pub color_variance: u32,
    #[serde(with = "color::optional")]
    pub fore_color: Option<Color>,
    #[serde(with = "color::optional")]
    pub back_color: Option<Color>,
}

impl Default for Highlighter {
    /// klogg's new highlighter (`HighlighterSetEdit::addHighlighter`).
    fn default() -> Self {
        Self {
            pattern: String::new(),
            ignore_case: false,
            use_regex: true,
            match_only: false,
            variate_colors: false,
            color_variance: DEFAULT_COLOR_VARIANCE,
            fore_color: Some(Color::rgb(0, 0, 0)),
            back_color: Some(Color::rgb(255, 255, 255)),
        }
    }
}

impl Highlighter {
    /// A highlighter for the literal `text`, colouring only the matches
    /// (klogg: the highlighters made for color labels).
    pub fn plain_match(text: impl Into<String>, colors: Colors) -> Self {
        Self {
            pattern: text.into(),
            use_regex: false,
            match_only: true,
            fore_color: colors.fore,
            back_color: colors.back,
            ..Self::default()
        }
    }

    pub fn colors(&self) -> Colors {
        Colors {
            fore: self.fore_color,
            back: self.back_color,
        }
    }

    /// Compiles the pattern. An empty pattern never matches.
    pub fn compile(&self) -> Result<CompiledHighlighter, regex::Error> {
        let source = if self.use_regex {
            self.pattern.clone()
        } else {
            regex::escape(&self.pattern)
        };
        let regex = if self.pattern.is_empty() {
            None
        } else {
            Some(
                RegexBuilder::new(&source)
                    .case_insensitive(self.ignore_case)
                    .build()?,
            )
        };
        Ok(CompiledHighlighter {
            regex,
            match_only: self.match_only,
            variance: (self.variate_colors && self.match_only).then_some(self.color_variance),
            colors: self.colors(),
        })
    }
}

/// A compiled [`Highlighter`].
#[derive(Debug, Clone)]
pub struct CompiledHighlighter {
    regex: Option<Regex>,
    match_only: bool,
    variance: Option<u32>,
    colors: Colors,
}

impl CompiledHighlighter {
    fn is_match(&self, line: &str) -> bool {
        self.regex.as_ref().is_some_and(|re| re.is_match(line))
    }

    /// The matches in `line` (klogg: `Highlighter::matchLine`): with capture
    /// groups, the text of each group; otherwise the whole match.
    fn matches(&self, line: &str, out: &mut Vec<Span>) {
        let Some(re) = &self.regex else { return };
        let mut push = |m: regex::Match<'_>| {
            if !m.is_empty() {
                out.push(Span {
                    range: m.range(),
                    colors: self.colors_for(m.as_str()),
                });
            }
        };
        if re.captures_len() > 1 {
            for caps in re.captures_iter(line) {
                caps.iter().skip(1).flatten().for_each(&mut push);
            }
        } else {
            re.find_iter(line).for_each(push);
        }
    }

    /// klogg's `vairateColors`: darker or lighter by up to `variance`
    /// percent, depending on a hash of the matched text, so equal strings
    /// get equal colours.
    fn colors_for(&self, text: &str) -> Colors {
        let Some(variance) = self.variance else {
            return self.colors;
        };
        let spread = u64::from(variance) * 2 + 1;
        let factor = (100 - variance.min(99)) + (fnv1a(text.as_bytes()) % spread) as u32;
        Colors {
            fore: self.colors.fore.map(|c| c.darker(factor)),
            back: self.colors.back.map(|c| c.darker(factor)),
        }
    }
}

/// FNV-1a: a fixed hash, so a string keeps its colour across runs.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// How a line is highlighted (klogg: `HighlighterMatchType` and the
/// matches it fills).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineHighlights {
    /// Colours of the whole line, if a whole-line highlighter matched.
    pub line: Option<Colors>,
    /// Coloured parts, in bytes, drawn over the line colours.
    pub spans: Spans,
}

/// Compiled highlighters, applied in order.
#[derive(Debug, Clone, Default)]
pub struct CompiledSet {
    list: Vec<CompiledHighlighter>,
}

impl CompiledSet {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Compiles `highlighters`, skipping the ones whose pattern is invalid.
    /// Returns the set and the errors, with the pattern of each.
    pub fn compile<'a>(
        highlighters: impl IntoIterator<Item = &'a Highlighter>,
    ) -> (Self, Vec<(String, regex::Error)>) {
        let mut list = Vec::new();
        let mut errors = Vec::new();
        for h in highlighters {
            match h.compile() {
                Ok(c) => list.push(c),
                Err(e) => errors.push((h.pattern.clone(), e)),
            }
        }
        (Self { list }, errors)
    }

    /// Highlights of `line` (klogg: `HighlighterSet::matchLine`). The
    /// highlighters are tried from last to first, each drawn over the ones
    /// before, so the first in the list wins. A whole-line match replaces
    /// the parts found so far.
    pub fn match_line(&self, line: &str) -> LineHighlights {
        let mut out = LineHighlights::default();
        let mut found = Vec::new();
        for h in self.list.iter().rev() {
            if !h.is_match(line) {
                continue;
            }
            if h.match_only {
                found.clear();
                h.matches(line, &mut found);
                out.spans.extend(found.drain(..));
            } else {
                out.line = Some(h.colors);
                out.spans.clear();
            }
        }
        out
    }

    /// Adds the matches of every highlighter in `line` to `spans`, in list
    /// order, whatever their `match_only` (used for color labels and the
    /// search highlight).
    pub fn add_matches(&self, line: &str, spans: &mut Spans) {
        let mut found = Vec::new();
        for h in &self.list {
            found.clear();
            h.matches(line, &mut found);
            spans.extend(found.drain(..));
        }
    }
}

/// A named list of highlighters (klogg: `HighlighterSet`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlighterSet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub highlighters: Vec<Highlighter>,
}

impl HighlighterSet {
    /// An empty set with a new unique id (klogg: `createNewSet`).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            highlighters: Vec::new(),
        }
    }
}

/// A random id in UUID form (klogg: `generateIdFromUuid`).
pub fn new_id() -> String {
    let random = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    let (a, b) = (random(), random());
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xffff,
        b >> 48,
        b & 0xffff_ffff_ffff
    )
}

/// The style of a color label (klogg: `QuickHighlighter`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorLabelStyle {
    pub name: String,
    #[serde(with = "color::optional", default)]
    pub fore_color: Option<Color>,
    #[serde(with = "color::optional", default)]
    pub back_color: Option<Color>,
    /// Used by "next color label" (Ctrl+D).
    #[serde(default = "yes")]
    pub cycle: bool,
}

fn yes() -> bool {
    true
}

impl ColorLabelStyle {
    pub fn colors(&self) -> Colors {
        Colors {
            fore: self.fore_color,
            back: self.back_color,
        }
    }

    /// klogg's nine color labels.
    pub fn defaults() -> Vec<Self> {
        let label = |i: usize, fore: Option<&str>, back: &str| Self {
            name: format!("Color label {i}"),
            fore_color: fore.and_then(Color::parse),
            back_color: Color::parse(back),
            cycle: true,
        };
        vec![
            label(1, Some("#001e80"), "#a1b7ff"),
            label(2, Some("#80005d"), "#ffa1c6"),
            label(3, Some("#0f8000"), "#acffa1"),
            label(4, Some("#806000"), "#ffe8a1"),
            label(5, Some("#420080"), "#d2a1ff"),
            label(6, Some("#007f80"), "#a1feff"),
            label(7, Some("#004e80"), "#a1dbff"),
            label(8, Some("#120080"), "#a29ccf"),
            // Qt::gray, and klogg's invalid (default) foreground.
            label(9, None, "#a0a0a4"),
        ]
    }
}

/// Number of color labels (klogg: 9, `Ctrl+Shift+1`…`9`).
pub const COLOR_LABELS: usize = 9;

const HIGHLIGHTERS_FILE: &str = "highlighters.toml";
const HIGHLIGHTERS_VERSION: u32 = 1;

/// All highlighter sets, which are active, and the color label styles
/// (klogg: `HighlighterSetCollection`). Saved in `highlighters.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HighlighterSets {
    version: u32,
    /// Ids of the active sets.
    pub active: Vec<String>,
    pub sets: Vec<HighlighterSet>,
    pub color_labels: Vec<ColorLabelStyle>,
}

impl Default for HighlighterSets {
    fn default() -> Self {
        Self {
            version: HIGHLIGHTERS_VERSION,
            active: Vec::new(),
            sets: Vec::new(),
            color_labels: ColorLabelStyle::defaults(),
        }
    }
}

impl HighlighterSets {
    pub fn is_active(&self, id: &str) -> bool {
        self.active.iter().any(|a| a == id)
    }

    /// klogg: `activateSet`. Unknown ids are ignored.
    pub fn activate(&mut self, id: &str) {
        if !self.is_active(id) && self.sets.iter().any(|s| s.id == id) {
            self.active.push(id.to_owned());
        }
    }

    pub fn deactivate(&mut self, id: &str) {
        self.active.retain(|a| a != id);
    }

    pub fn deactivate_all(&mut self) {
        self.active.clear();
    }

    /// The highlighters of the active sets, in collection order (klogg:
    /// `updateCombinedSet`), compiled.
    pub fn compile_active(&self) -> (CompiledSet, Vec<(String, regex::Error)>) {
        CompiledSet::compile(
            self.sets
                .iter()
                .filter(|s| self.is_active(&s.id))
                .flat_map(|s| &s.highlighters),
        )
    }

    /// Restores invariants after loading: nine color labels (klogg pads
    /// with the defaults and drops extras), no duplicate set ids, and only
    /// existing sets active.
    fn sanitize(&mut self) {
        let defaults = ColorLabelStyle::defaults();
        self.color_labels.truncate(COLOR_LABELS);
        let have = self.color_labels.len();
        self.color_labels.extend(defaults.into_iter().skip(have));
        let mut ids = std::collections::HashSet::new();
        for set in &mut self.sets {
            if set.id.is_empty() || !ids.insert(set.id.clone()) {
                set.id = new_id();
                ids.insert(set.id.clone());
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.active
            .retain(|id| ids.contains(id) && seen.insert(id.clone()));
    }

    /// Loads from `dir`; a missing, unreadable or unknown-version file gives
    /// the defaults (logged), as for the session.
    pub fn load(dir: &Path) -> Self {
        let mut sets = settings::load_toml::<Self>(dir, HIGHLIGHTERS_FILE)
            .filter(|s| {
                let known = s.version == HIGHLIGHTERS_VERSION;
                if !known {
                    tracing::warn!(
                        "ignoring {HIGHLIGHTERS_FILE}: unknown version {}",
                        s.version
                    );
                }
                known
            })
            .unwrap_or_default();
        sets.sanitize();
        sets
    }

    pub fn save(&self, dir: &Path) -> Result<(), SettingsError> {
        settings::save_toml(self, dir, HIGHLIGHTERS_FILE)
    }
}

/// The strings marked with each color label in one tab (klogg:
/// `ColorLabelsManager`). A string has at most one label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorLabels {
    words: [Vec<String>; COLOR_LABELS],
    /// The label last applied by [`set_next`](Self::set_next).
    current: Option<usize>,
}

impl ColorLabels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(Vec::is_empty)
    }

    /// The strings of each label.
    pub fn words(&self) -> &[Vec<String>; COLOR_LABELS] {
        &self.words
    }

    /// The label of `text`, if any.
    pub fn label_of(&self, text: &str) -> Option<usize> {
        self.words.iter().position(|w| w.iter().any(|t| t == text))
    }

    /// Gives `text` the label `label` (klogg: `setColorLabel`). If it
    /// already has that label, the label is removed instead (a toggle).
    pub fn set(&mut self, label: usize, text: &str) {
        self.update(label, text, true);
    }

    /// Removes `text`'s label.
    pub fn remove(&mut self, text: &str) {
        for words in &mut self.words {
            words.retain(|t| t != text);
        }
    }

    /// Gives `text` the next label of the cycle (klogg: `setNextColorLabel`):
    /// labels whose `cycle[i]` is true, after the last one applied, wrapping.
    /// If `text` already has a label, it is removed instead.
    pub fn set_next(&mut self, text: &str, cycle: &[bool]) {
        let in_cycle: Vec<usize> = (0..COLOR_LABELS)
            .filter(|&i| cycle.get(i).copied().unwrap_or(false))
            .collect();
        let Some(&first) = in_cycle.first() else {
            return;
        };
        let next = self
            .current
            .and_then(|c| in_cycle.iter().copied().find(|&i| i > c))
            .unwrap_or(first);
        self.current = Some(next);
        self.update(next, text, false);
    }

    /// klogg: `updateColorLabel`. `text` loses any label it has; it gets
    /// `label` if it had none, or if `replace` and it had another one.
    fn update(&mut self, label: usize, text: &str, replace: bool) {
        if label >= COLOR_LABELS || text.is_empty() {
            return;
        }
        let had = self.label_of(text);
        self.remove(text);
        if had.is_none() || (replace && had != Some(label)) {
            self.words[label].push(text.to_owned());
        }
    }

    /// Removes every label (klogg: `clear`).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Highlighters for the labelled strings: case-sensitive plain text,
    /// matches only, in label order.
    pub fn compile(&self, styles: &[ColorLabelStyle]) -> CompiledSet {
        let highlighters: Vec<Highlighter> = self
            .words
            .iter()
            .zip(styles)
            .flat_map(|(words, style)| {
                words
                    .iter()
                    .map(|w| Highlighter::plain_match(w.clone(), style.colors()))
            })
            .collect();
        CompiledSet::compile(&highlighters).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(n: u8) -> Colors {
        Colors {
            fore: Some(Color::rgb(n, 0, 0)),
            back: Some(Color::rgb(0, 0, n)),
        }
    }

    fn hl(pattern: &str, match_only: bool, n: u8) -> Highlighter {
        Highlighter {
            pattern: pattern.into(),
            match_only,
            fore_color: colors(n).fore,
            back_color: colors(n).back,
            ..Highlighter::default()
        }
    }

    fn compile(list: &[Highlighter]) -> CompiledSet {
        let (set, errors) = CompiledSet::compile(list);
        assert!(errors.is_empty());
        set
    }

    fn spans(h: &LineHighlights) -> Vec<(std::ops::Range<usize>, Colors)> {
        h.spans
            .as_slice()
            .iter()
            .map(|s| (s.range.clone(), s.colors))
            .collect()
    }

    #[test]
    fn whole_line_and_match_only() {
        // The word highlighter is above the line one, so it is drawn over it.
        let set = compile(&[hl("id=\\d+", true, 2), hl("ERROR", false, 1)]);
        let h = set.match_line("ERROR id=42 failed");
        assert_eq!(h.line, Some(colors(1)));
        assert_eq!(spans(&h), [(6..11, colors(2))]);

        let h = set.match_line("INFO id=7 id=8");
        assert_eq!(h.line, None);
        assert_eq!(spans(&h), [(5..9, colors(2)), (10..14, colors(2))]);

        assert_eq!(set.match_line("nothing"), LineHighlights::default());
    }

    #[test]
    fn first_highlighter_wins() {
        // Tried from the bottom up, so the top one is drawn last.
        let set = compile(&[hl("WARN|ERROR", false, 1), hl("ERROR", false, 2)]);
        assert_eq!(set.match_line("ERROR").line, Some(colors(1)));
        let set = compile(&[hl("ab", true, 1), hl("abc", true, 2)]);
        assert_eq!(
            spans(&set.match_line("abcd")),
            [(0..2, colors(1)), (2..3, colors(2))]
        );
        // A whole-line highlighter above a word highlighter clears its words.
        let set = compile(&[hl("x", false, 1), hl("y", true, 2)]);
        let h = set.match_line("x y");
        assert_eq!(h.line, Some(colors(1)));
        assert!(h.spans.is_empty());
    }

    #[test]
    fn capture_groups_only() {
        let set = compile(&[hl("user=(\\w+) id=(\\d+)?", true, 1)]);
        let h = set.match_line("user=bob id= user=al id=3");
        assert_eq!(
            spans(&h),
            [(5..8, colors(1)), (18..20, colors(1)), (24..25, colors(1))]
        );
    }

    #[test]
    fn plain_text_case_and_empty() {
        let mut h = hl("a.b", true, 1);
        h.use_regex = false;
        let set = compile(&[h.clone()]);
        assert_eq!(spans(&set.match_line("axb a.b")), [(4..7, colors(1))]);
        h.ignore_case = true;
        h.pattern = "A.B".into();
        assert_eq!(spans(&compile(&[h]).match_line("a.b")), [(0..3, colors(1))]);
        // An empty pattern never matches, even for the whole line.
        let set = compile(&[hl("", false, 1)]);
        assert_eq!(set.match_line("anything"), LineHighlights::default());
        // Zero-length matches colour nothing.
        let set = compile(&[hl("x*", true, 1)]);
        assert!(set.match_line("abc").spans.is_empty());
    }

    #[test]
    fn invalid_patterns_are_reported_and_skipped() {
        let (set, errors) = CompiledSet::compile(&[hl("(", false, 1), hl("ok", false, 2)]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "(");
        assert_eq!(set.match_line("ok").line, Some(colors(2)));
    }

    #[test]
    fn variance_is_stable_per_text() {
        let mut h = hl("\\w+", true, 200);
        h.variate_colors = true;
        h.color_variance = 30;
        let set = compile(&[h]);
        let s = spans(&set.match_line("alpha beta alpha"));
        assert_eq!(s[0].1, s[2].1);
        // Not tied to the position.
        assert_eq!(s.len(), 3);
        // Without match_only there is no variance.
        let mut h = hl("x", false, 200);
        h.variate_colors = true;
        assert_eq!(compile(&[h]).match_line("x").line, Some(colors(200)));
    }

    #[test]
    fn active_sets_combine_in_collection_order() {
        let mut sets = HighlighterSets::default();
        let mut a = HighlighterSet::new("a");
        a.highlighters.push(hl("x", false, 1));
        let mut b = HighlighterSet::new("b");
        b.highlighters.push(hl("x", false, 2));
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        sets.sets = vec![a, b];
        assert!(sets.compile_active().0.is_empty());
        // Activation order doesn't matter, collection order does.
        sets.activate(&b_id);
        sets.activate(&a_id);
        sets.activate(&a_id);
        sets.activate("missing");
        assert_eq!(sets.active, [b_id.clone(), a_id.clone()]);
        assert_eq!(
            sets.compile_active().0.match_line("x").line,
            Some(colors(1))
        );
        sets.deactivate(&a_id);
        assert_eq!(
            sets.compile_active().0.match_line("x").line,
            Some(colors(2))
        );
        sets.deactivate_all();
        assert!(sets.active.is_empty());
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<_> = (0..100).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 100);
        assert_eq!(new_id().len(), 36);
    }

    #[test]
    fn save_load_and_sanitize() {
        let dir = std::env::temp_dir().join(format!("kloggr-hl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(HighlighterSets::load(&dir), HighlighterSets::default());

        let mut sets = HighlighterSets::default();
        let mut set = HighlighterSet::new("errors");
        set.highlighters.push(hl("ERROR", false, 1));
        let mut h = hl("id=(\\d+)", true, 2);
        h.fore_color = None;
        set.highlighters.push(h);
        let id = set.id.clone();
        sets.sets.push(set);
        sets.activate(&id);
        sets.color_labels[0].name = "Mine".into();
        sets.save(&dir).unwrap();
        assert_eq!(HighlighterSets::load(&dir), sets);

        // Hand-edited: missing labels, duplicate ids, unknown active set.
        let text = format!(
            "version = 1\nactive = [\"{id}\", \"gone\", \"{id}\"]\n\
             [[sets]]\nid = \"{id}\"\nname = \"a\"\n\
             [[sets]]\nid = \"{id}\"\nname = \"b\"\n\
             [[color_labels]]\nname = \"one\"\nback_color = \"#ff0000\"\n"
        );
        std::fs::write(dir.join(HIGHLIGHTERS_FILE), text).unwrap();
        let loaded = HighlighterSets::load(&dir);
        assert_eq!(loaded.active, std::slice::from_ref(&id));
        assert_ne!(loaded.sets[0].id, loaded.sets[1].id);
        assert_eq!(loaded.color_labels.len(), COLOR_LABELS);
        assert_eq!(loaded.color_labels[0].name, "one");
        assert!(loaded.color_labels[0].cycle);
        assert_eq!(loaded.color_labels[0].fore_color, None);
        assert_eq!(loaded.color_labels[1], ColorLabelStyle::defaults()[1]);

        std::fs::write(dir.join(HIGHLIGHTERS_FILE), "version = 7\n").unwrap();
        assert_eq!(HighlighterSets::load(&dir), HighlighterSets::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn color_labels_set_toggle_and_replace() {
        let mut labels = ColorLabels::new();
        labels.set(0, "foo");
        assert_eq!(labels.label_of("foo"), Some(0));
        // Another label replaces it.
        labels.set(2, "foo");
        assert_eq!(labels.label_of("foo"), Some(2));
        assert!(labels.words()[0].is_empty());
        // The same label removes it.
        labels.set(2, "foo");
        assert_eq!(labels.label_of("foo"), None);
        labels.set(1, "bar");
        labels.set(1, "baz");
        assert_eq!(labels.words()[1], ["bar", "baz"]);
        labels.set(1, "");
        labels.set(COLOR_LABELS, "x");
        assert_eq!(labels.label_of(""), None);
        labels.clear();
        assert!(labels.is_empty());
    }

    #[test]
    fn color_labels_next_cycles() {
        let mut cycle = [true; COLOR_LABELS];
        cycle[1] = false;
        let mut labels = ColorLabels::new();
        labels.set_next("a", &cycle);
        labels.set_next("b", &cycle);
        labels.set_next("c", &cycle);
        assert_eq!(labels.label_of("a"), Some(0));
        assert_eq!(labels.label_of("b"), Some(2));
        assert_eq!(labels.label_of("c"), Some(3));
        // Next on a labelled string removes its label (and still advances).
        labels.set_next("a", &cycle);
        assert_eq!(labels.label_of("a"), None);
        for t in ["d", "e", "f", "g"] {
            labels.set_next(t, &cycle);
        }
        assert_eq!(labels.label_of("d"), Some(5));
        assert_eq!(labels.label_of("g"), Some(8));
        labels.set_next("h", &cycle);
        assert_eq!(labels.label_of("h"), Some(0), "wraps");
        // No label in the cycle: nothing happens.
        labels.set_next("z", &[false; COLOR_LABELS]);
        assert_eq!(labels.label_of("z"), None);
        // Clear resets the cycle.
        labels.clear();
        labels.set_next("a", &cycle);
        assert_eq!(labels.label_of("a"), Some(0));
    }

    #[test]
    fn color_labels_compile() {
        let styles = ColorLabelStyle::defaults();
        let mut labels = ColorLabels::new();
        labels.set(0, "a.c");
        labels.set(8, "ABC");
        let set = labels.compile(&styles);
        let mut spans = Spans::new();
        set.add_matches("abc a.c ABC", &mut spans);
        let got: Vec<_> = spans
            .as_slice()
            .iter()
            .map(|s| (s.range.clone(), s.colors))
            .collect();
        assert_eq!(
            got,
            [(4..7, styles[0].colors()), (8..11, styles[8].colors())]
        );
        assert_eq!(styles[8].colors().fore, None);
    }
}
