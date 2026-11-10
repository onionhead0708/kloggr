//! klogg's highlighter export files (`.conf`): `QSettings` in INI format.
//!
//! Ports `HighlighterSetCollection::saveToStorage` / `retrieveFromStorage`
//! and `HighlighterSet`'s (`src/ui/src/highlighterset.cpp`, GPLv3,
//! Copyright (C) 2019 Anton Filimonov and other contributors), with a minimal
//! reader and writer for Qt's INI dialect: `[group]` sections, `\`-separated
//! keys, arrays as `name\1\key` ... `name\size`, and Qt's value escaping
//! (`QSettingsPrivate::iniEscapedString`, `iniUnescapedStringList`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{
    COLOR_LABELS, Color, ColorLabelStyle, DEFAULT_COLOR_VARIANCE, Highlighter, HighlighterSet,
    HighlighterSets, new_id,
};

/// klogg: `HighlighterSetCollection_VERSION`, `HighlighterSet_VERSION`.
const COLLECTION_VERSION: u32 = 2;
const SET_VERSION: u32 = 3;
/// klogg's older `FilterSet` (glogg filters), still read by klogg.
const FILTER_SET_VERSION: u32 = 2;

/// Settings as a flat map from `group/key/...` to the raw value.
struct Ini(BTreeMap<String, String>);

impl Ini {
    fn parse(text: &str) -> Self {
        let mut map = BTreeMap::new();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = unescape_key(name);
                if section.eq_ignore_ascii_case("general") {
                    section.clear();
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = unescape_key(key.trim()).replace('\\', "/");
            let full = if section.is_empty() {
                key
            } else {
                format!("{section}/{key}")
            };
            map.insert(full, value.trim().to_owned());
        }
        Self(map)
    }

    fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.0
            .get(key)
            .map(|v| unescape_list(v))
            .unwrap_or_default()
    }

    /// A string value (klogg: `value().toString()`): a list of several
    /// strings has no string value.
    fn string(&self, key: &str) -> Option<String> {
        let mut list = self.0.get(key).map(|v| unescape_list(v))?;
        (list.len() == 1).then(|| list.remove(0))
    }

    fn bool(&self, key: &str, default: bool) -> bool {
        match self.string(key).as_deref().map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
            Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
            _ => default,
        }
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.string(key)?.trim().parse().ok()
    }

    fn color(&self, key: &str) -> Option<Color> {
        self.string(key).as_deref().and_then(Color::parse)
    }

    /// Prefixes `prefix/1` ... `prefix/N` of an array (klogg:
    /// `beginReadArray`).
    fn array(&self, prefix: &str) -> Vec<String> {
        let size = self.int(&format!("{prefix}/size")).unwrap_or(0).max(0);
        (1..=size).map(|i| format!("{prefix}/{i}")).collect()
    }
}

/// Qt's key unescaping: `%XX` and `%UXXXX`.
fn unescape_key(key: &str) -> String {
    let mut out = String::new();
    let mut chars = key.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let wide = chars.peek() == Some(&'U');
        if wide {
            chars.next();
        }
        let digits: String = (0..if wide { 4 } else { 2 })
            .filter_map(|_| chars.next_if(char::is_ascii_hexdigit))
            .collect();
        match u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32)
        {
            Some(decoded) => out.push(decoded),
            None => {
                out.push('%');
                out.push_str(&digits);
            }
        }
    }
    out
}

/// Qt's value unescaping (`iniUnescapedStringList`): quoted parts, C-like
/// escapes (`\n`, `\x263a`, octal), and unquoted commas between list items.
/// Also undoes the `@` prefixes of `QVariant` values that matter here.
fn unescape_list(value: &str) -> Vec<String> {
    match value {
        "@Invalid()" => return Vec::new(),
        v if v.starts_with("@String(") && v.ends_with(')') => {
            return vec![v["@String(".len()..v.len() - 1].to_owned()];
        }
        _ => {}
    }
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                items.push(std::mem::take(&mut current));
                while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
            }
            '\\' => match chars.next() {
                Some('a') => current.push('\u{7}'),
                Some('b') => current.push('\u{8}'),
                Some('f') => current.push('\u{c}'),
                Some('n') => current.push('\n'),
                Some('r') => current.push('\r'),
                Some('t') => current.push('\t'),
                Some('v') => current.push('\u{b}'),
                Some('x') => {
                    let mut hex = String::new();
                    while let Some(d) = chars.next_if(char::is_ascii_hexdigit) {
                        hex.push(d);
                    }
                    if let Some(decoded) =
                        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                    {
                        current.push(decoded);
                    }
                }
                Some(d @ '0'..='7') => {
                    let mut oct = String::from(d);
                    while let Some(d) = chars.next_if(|c| ('0'..='7').contains(c)) {
                        oct.push(d);
                    }
                    if let Some(decoded) =
                        u32::from_str_radix(&oct, 8).ok().and_then(char::from_u32)
                    {
                        current.push(decoded);
                    }
                }
                Some(other) => current.push(other),
                None => {}
            },
            c => current.push(c),
        }
    }
    items.push(current);
    // Qt writes a literal leading `@` as `@@`.
    for item in &mut items {
        if item.starts_with("@@") {
            item.remove(0);
        }
    }
    items
}

/// Qt's value escaping (`iniEscapedString`), quoting values that would
/// otherwise be read as lists or lose spaces.
fn escape(value: &str) -> String {
    let mut body = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => body.push_str("\\\\"),
            '"' => body.push_str("\\\""),
            '\n' => body.push_str("\\n"),
            '\r' => body.push_str("\\r"),
            '\t' => body.push_str("\\t"),
            '\0'..='\u{1f}' => {
                let _ = write!(body, "\\x{:x}", c as u32);
            }
            c => body.push(c),
        }
    }
    if value.starts_with('@') {
        body.insert(0, '@');
    }
    let needs_quotes = value.contains([',', ';', '=', '#'])
        || value.starts_with([' ', '\t'])
        || value.ends_with([' ', '\t']);
    if needs_quotes {
        format!("\"{body}\"")
    } else {
        body
    }
}

/// Reads the highlighter sets and color label styles of a klogg export
/// (or klogg's own settings file). Missing or unknown parts are skipped, as
/// klogg does.
pub fn import(text: &str) -> HighlighterSets {
    let ini = Ini::parse(text);
    let mut out = HighlighterSets::default();
    let group = "HighlighterSetCollection";
    let version = ini.int(&format!("{group}/version"));
    if version.is_some_and(|v| v >= 1 && v <= i64::from(COLLECTION_VERSION)) {
        for prefix in ini.array(&format!("{group}/sets")) {
            if let Some(set) = read_set(&ini, &prefix) {
                out.sets.push(set);
            }
        }
        out.active = ini.list(&format!("{group}/active_sets"));
        out.active.retain(|id| !id.is_empty());
        let labels = ini.array(&format!("{group}/quick"));
        for (i, prefix) in labels.iter().enumerate().take(COLOR_LABELS) {
            let default = &ColorLabelStyle::defaults()[i];
            out.color_labels[i] = ColorLabelStyle {
                name: ini
                    .string(&format!("{prefix}/name"))
                    .unwrap_or_else(|| default.name.clone()),
                fore_color: ini.color(&format!("{prefix}/fore_colour")),
                back_color: ini.color(&format!("{prefix}/back_colour")),
                cycle: ini.bool(&format!("{prefix}/cycle"), true),
            };
        }
    }
    // A set saved by older versions, outside the collection.
    if let Some(set) = read_set(&ini, "") {
        out.sets.push(set);
    }
    out.sanitize();
    out
}

/// One set at `prefix` (klogg: `HighlighterSet::retrieveFromStorage`).
fn read_set(ini: &Ini, prefix: &str) -> Option<HighlighterSet> {
    let at = |key: &str| {
        if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}/{key}")
        }
    };
    let (group, list, max) = if ini.contains(&at("HighlighterSet/version")) {
        ("HighlighterSet", "highlighters", SET_VERSION)
    } else if ini.contains(&at("FilterSet/version")) {
        ("FilterSet", "filters", FILTER_SET_VERSION)
    } else {
        return None;
    };
    let key = |k: &str| at(&format!("{group}/{k}"));
    if ini.int(&key("version")).is_none_or(|v| v > i64::from(max)) {
        tracing::warn!("unknown version of {group}, ignoring it");
        return None;
    }
    let highlighters = ini
        .array(&key(list))
        .iter()
        .map(|p| read_highlighter(ini, p))
        .collect();
    Some(HighlighterSet {
        name: ini
            .string(&key("name"))
            .unwrap_or_else(|| "Highlighters set".into()),
        id: ini.string(&key("id")).unwrap_or_else(new_id),
        highlighters,
    })
}

/// klogg: `Highlighter::retrieveFromStorage`.
fn read_highlighter(ini: &Ini, prefix: &str) -> Highlighter {
    let key = |k: &str| format!("{prefix}/{k}");
    Highlighter {
        pattern: ini.string(&key("regexp")).unwrap_or_default(),
        ignore_case: ini.bool(&key("ignore_case"), false),
        use_regex: ini.bool(&key("use_regex"), true),
        match_only: ini.bool(&key("match_only"), false),
        variate_colors: ini.bool(&key("variate_colors"), false),
        color_variance: ini
            .int(&key("color_variance"))
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(DEFAULT_COLOR_VARIANCE),
        fore_color: ini.color(&key("fore_colour")),
        back_color: ini.color(&key("back_colour")),
    }
}

/// Writes `sets` as klogg exports them (`HighlightersDialog::
/// exportHighlighters`), so klogg can import the file.
pub fn export(sets: &HighlighterSets) -> String {
    let mut out = String::from("[HighlighterSetCollection]\n");
    let mut put = |key: &str, value: &str| {
        let _ = writeln!(out, "{}={value}", key.replace('/', "\\"));
    };
    put("version", &COLLECTION_VERSION.to_string());
    let active = if sets.active.is_empty() {
        "@Invalid()".to_owned()
    } else {
        sets.active
            .iter()
            .map(|id| escape(id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    put("active_sets", &active);
    let color = |c: Option<Color>| c.map(Color::to_hex).unwrap_or_default();
    let bool = |b: bool| if b { "true" } else { "false" };
    for (i, set) in sets.sets.iter().enumerate() {
        let p = format!("sets/{}/HighlighterSet", i + 1);
        put(&format!("{p}/version"), &SET_VERSION.to_string());
        put(&format!("{p}/name"), &escape(&set.name));
        put(&format!("{p}/id"), &escape(&set.id));
        for (j, h) in set.highlighters.iter().enumerate() {
            let q = format!("{p}/highlighters/{}", j + 1);
            put(&format!("{q}/regexp"), &escape(&h.pattern));
            put(&format!("{q}/ignore_case"), bool(h.ignore_case));
            put(&format!("{q}/match_only"), bool(h.match_only));
            put(&format!("{q}/use_regex"), bool(h.use_regex));
            put(&format!("{q}/variate_colors"), bool(h.variate_colors));
            put(
                &format!("{q}/color_variance"),
                &h.color_variance.to_string(),
            );
            put(&format!("{q}/fore_colour"), &color(h.fore_color));
            put(&format!("{q}/back_colour"), &color(h.back_color));
        }
        put(
            &format!("{p}/highlighters/size"),
            &set.highlighters.len().to_string(),
        );
    }
    put("sets/size", &sets.sets.len().to_string());
    for (i, label) in sets.color_labels.iter().enumerate() {
        let p = format!("quick/{}", i + 1);
        put(&format!("{p}/name"), &escape(&label.name));
        put(&format!("{p}/fore_colour"), &color(label.fore_color));
        put(&format!("{p}/back_colour"), &color(label.back_color));
        put(&format!("{p}/cycle"), bool(label.cycle));
    }
    put("quick/size", &sets.color_labels.len().to_string());
    out
}

impl HighlighterSets {
    /// Adds the sets of `imported` that are new: klogg skips a set whose id
    /// or name is already present (`importHighlighters`). Returns how many
    /// were added.
    pub fn merge_imported(&mut self, imported: HighlighterSets) -> usize {
        let mut added = 0;
        for set in imported.sets {
            let known = self
                .sets
                .iter()
                .any(|s| s.id == set.id || s.name == set.name);
            if !known {
                self.sets.push(set);
                added += 1;
            }
        }
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As written by klogg (Qt 5 escapes non-ASCII as `\x`).
    const KLOGG: &str = r#"[General]
version=1

[HighlighterSetCollection]
active_sets=a1b2
quick\1\back_colour=#ffa1b7ff
quick\1\cycle=false
quick\1\fore_colour=#ff001e80
quick\1\name=Mine
quick\size=1
sets\1\HighlighterSet\highlighters\1\back_colour=#ffff0000
sets\1\HighlighterSet\highlighters\1\color_variance=15
sets\1\HighlighterSet\highlighters\1\fore_colour=#ff000000
sets\1\HighlighterSet\highlighters\1\ignore_case=true
sets\1\HighlighterSet\highlighters\1\match_only=false
sets\1\HighlighterSet\highlighters\1\regexp="ERROR|FATAL, \\d+"
sets\1\HighlighterSet\highlighters\1\use_regex=true
sets\1\HighlighterSet\highlighters\1\variate_colors=false
sets\1\HighlighterSet\highlighters\2\back_colour=
sets\1\HighlighterSet\highlighters\2\fore_colour=#ff0000ff
sets\1\HighlighterSet\highlighters\2\match_only=true
sets\1\HighlighterSet\highlighters\2\regexp=caf\xe9
sets\1\HighlighterSet\highlighters\size=2
sets\1\HighlighterSet\id=a1b2
sets\1\HighlighterSet\name=Errors
sets\1\HighlighterSet\version=3
sets\2\FilterSet\filters\1\regexp=old
sets\2\FilterSet\filters\size=1
sets\2\FilterSet\id=c3
sets\2\FilterSet\name=Legacy
sets\2\FilterSet\version=2
sets\3\HighlighterSet\name=Future
sets\3\HighlighterSet\version=9
sets\size=3
version=2
"#;

    #[test]
    fn imports_klogg_conf() {
        let sets = import(KLOGG);
        assert_eq!(sets.sets.len(), 2, "the future version is skipped");
        let errors = &sets.sets[0];
        assert_eq!(
            (errors.id.as_str(), errors.name.as_str()),
            ("a1b2", "Errors")
        );
        let h = &errors.highlighters[0];
        assert_eq!(h.pattern, r"ERROR|FATAL, \d+");
        assert!(h.ignore_case && h.use_regex && !h.match_only);
        assert_eq!(h.back_color, Color::parse("#ff0000"));
        let h = &errors.highlighters[1];
        assert_eq!(h.pattern, "café");
        assert!(h.match_only);
        assert_eq!(h.back_color, None);
        assert_eq!(h.color_variance, DEFAULT_COLOR_VARIANCE);
        let legacy = &sets.sets[1];
        assert_eq!(legacy.name, "Legacy");
        assert_eq!(legacy.highlighters[0].pattern, "old");
        assert_eq!(sets.active, ["a1b2"]);
        assert_eq!(sets.color_labels[0].name, "Mine");
        assert!(!sets.color_labels[0].cycle);
        assert_eq!(sets.color_labels[1], ColorLabelStyle::defaults()[1]);
        assert_eq!(sets.color_labels.len(), COLOR_LABELS);
    }

    #[test]
    fn export_round_trips() {
        let mut sets = HighlighterSets::default();
        let mut set = HighlighterSet::new("Mixed, \"quoted\" = set");
        set.highlighters.push(Highlighter {
            pattern: r#"a\b,"c";=d é @x"#.into(),
            ignore_case: true,
            variate_colors: true,
            color_variance: 40,
            fore_color: None,
            ..Highlighter::default()
        });
        set.highlighters.push(Highlighter {
            pattern: "@at start ".into(),
            ..Highlighter::default()
        });
        let id = set.id.clone();
        sets.sets.push(set);
        sets.sets.push(HighlighterSet::new("Empty"));
        sets.activate(&id);
        sets.color_labels[8].name = "Last".into();
        let text = export(&sets);
        assert!(text.contains("sets\\1\\HighlighterSet\\version=3"));
        assert_eq!(import(&text), sets);
    }

    #[test]
    fn values() {
        assert_eq!(unescape_list("a, b,c"), ["a", "b", "c"]);
        assert_eq!(unescape_list(r#""a, b""#), ["a, b"]);
        assert_eq!(unescape_list(r"x\ty\x41\101\\"), ["x\tyAA\\"]);
        assert_eq!(unescape_list("@Invalid()"), Vec::<String>::new());
        assert_eq!(unescape_list("@String(hi)"), ["hi"]);
        assert_eq!(unescape_list("@@x"), ["@x"]);
        assert_eq!(unescape_key("a%20b%U00e9"), "a bé");
        for s in [
            "",
            "plain",
            " pad ",
            "a,b",
            "q\"uote",
            "back\\slash",
            "@at",
            "tab\there",
        ] {
            assert_eq!(unescape_list(&escape(s)), [s], "{s:?}");
        }
        assert_eq!(
            import("garbage\n[x]\nno equals"),
            HighlighterSets::default()
        );
    }

    #[test]
    fn merge_skips_known_ids_and_names() {
        let mut mine = HighlighterSets::default();
        mine.sets.push(HighlighterSet::new("A"));
        let mut theirs = HighlighterSets::default();
        let mut same_id = HighlighterSet::new("B");
        same_id.id = mine.sets[0].id.clone();
        theirs.sets.push(same_id);
        theirs.sets.push(HighlighterSet::new("A"));
        theirs.sets.push(HighlighterSet::new("C"));
        assert_eq!(mine.merge_imported(theirs), 1);
        assert_eq!(mine.sets.len(), 2);
        assert_eq!(mine.sets[1].name, "C");
    }
}
