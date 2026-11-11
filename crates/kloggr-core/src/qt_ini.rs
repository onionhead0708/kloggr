//! A minimal reader and writer for Qt's INI dialect (`QSettings` with
//! `IniFormat`), for klogg's `.conf` files: `[group]` sections,
//! `\`-separated keys, arrays as `name\1\key` ... `name\size`, and Qt's
//! value escaping (`QSettingsPrivate::iniEscapedString`,
//! `iniUnescapedStringList`). Ported from klogg's use of `QSettings` (GPLv3).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Settings as a flat map from `group/key/...` to the raw value.
pub(crate) struct Ini(BTreeMap<String, String>);

impl Ini {
    pub(crate) fn parse(text: &str) -> Self {
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

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub(crate) fn list(&self, key: &str) -> Vec<String> {
        self.0
            .get(key)
            .map(|v| unescape_list(v))
            .unwrap_or_default()
    }

    /// A string value (klogg: `value().toString()`): a list of several
    /// strings has no string value.
    pub(crate) fn string(&self, key: &str) -> Option<String> {
        let mut list = self.0.get(key).map(|v| unescape_list(v))?;
        (list.len() == 1).then(|| list.remove(0))
    }

    pub(crate) fn bool(&self, key: &str, default: bool) -> bool {
        match self.string(key).as_deref().map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
            Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
            _ => default,
        }
    }

    pub(crate) fn int(&self, key: &str) -> Option<i64> {
        self.string(key)?.trim().parse().ok()
    }

    /// Prefixes `prefix/1` ... `prefix/N` of an array (klogg:
    /// `beginReadArray`).
    pub(crate) fn array(&self, prefix: &str) -> Vec<String> {
        let size = self.int(&format!("{prefix}/size")).unwrap_or(0).max(0);
        (1..=size).map(|i| format!("{prefix}/{i}")).collect()
    }
}

/// Qt's key unescaping: `%XX` and `%UXXXX`.
pub(crate) fn unescape_key(key: &str) -> String {
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
pub(crate) fn unescape_list(value: &str) -> Vec<String> {
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
pub(crate) fn escape(value: &str) -> String {
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
