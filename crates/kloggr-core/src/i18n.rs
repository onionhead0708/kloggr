//! Translations of the user interface (klogg: Qt `.ts` files loaded with
//! `QTranslator`, chosen in Options > General > Language).
//!
//! A catalog is a TOML file that maps each English message, as written in
//! the source, to its translation:
//!
//! ```toml
//! language = "de"          # BCP 47 tag: "de", "zh-CN", "pt-BR"
//! name = "Deutsch"         # shown in Options > General > Language
//!
//! [messages]
//! "Open…" = "Öffnen…"
//! "{count} lines" = "{count} Zeilen"
//! ```
//!
//! Messages may hold `{name}` placeholders, filled in after the lookup
//! ([`fill`]); a translation may move them but must keep their names.
//! Missing or empty translations fall back to English. The app keeps a
//! template listing every message (`crates/kloggr-app/locales/template.toml`).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// The translations of one language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// BCP 47 language tag, e.g. `"zh-CN"`.
    pub language: String,
    /// The language's name in itself, e.g. `"简体中文"`.
    pub name: String,
    messages: HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    #[error("the catalog has no language tag")]
    NoLanguage,
}

#[derive(Deserialize)]
struct CatalogFile {
    language: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    messages: HashMap<String, String>,
}

impl Catalog {
    /// Parses a catalog. Empty translations are left out.
    pub fn parse(text: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = toml::from_str(text)?;
        let language = file.language.trim().to_owned();
        if language.is_empty() {
            return Err(CatalogError::NoLanguage);
        }
        let name = match file.name.trim() {
            "" => language.clone(),
            name => name.to_owned(),
        };
        let messages = file
            .messages
            .into_iter()
            .filter(|(_, t)| !t.is_empty())
            .collect();
        Ok(Self {
            language,
            name,
            messages,
        })
    }

    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    /// The translation of `message`, if there is one.
    pub fn get(&self, message: &str) -> Option<&str> {
        self.messages.get(message).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Fills the `{name}` placeholders of `template` with `args`. Unknown
/// names stay as they are; `{{` and `}}` are literal braces.
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(t) = tail.strip_prefix("{{") {
            out.push('{');
            rest = t;
        } else if let Some(t) = tail.strip_prefix("}}") {
            out.push('}');
            rest = t;
        } else if let Some(end) = tail.find('}').filter(|_| tail.starts_with('{')) {
            let name = &tail[1..end];
            match args.iter().find(|(n, _)| *n == name) {
                Some((_, value)) => out.push_str(&value.to_string()),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The catalog among `catalogs` for the language `wanted` (a BCP 47 tag,
/// any case, `_` or `-`): the same tag, else the same language without its
/// region (`"zh-TW"` finds `"zh"`, `"de"` finds `"de-DE"`). `None` for
/// English, which needs no catalog.
pub fn choose<'a>(catalogs: &'a [Catalog], wanted: &str) -> Option<&'a Catalog> {
    let norm = |tag: &str| tag.trim().replace('_', "-").to_ascii_lowercase();
    // "zh-Hans-CN.UTF-8" → "zh-hans-cn"
    let wanted = norm(wanted.split(['.', '@']).next().unwrap_or_default());
    let primary = |tag: &str| tag.split('-').next().unwrap_or_default().to_owned();
    if wanted.is_empty() || primary(&wanted) == "en" {
        return None;
    }
    catalogs
        .iter()
        .find(|c| norm(&c.language) == wanted)
        .or_else(|| {
            // zh-Hant-TW / zh-TW should prefer a Traditional catalog.
            let script_or_region = wanted.split('-').skip(1).collect::<Vec<_>>();
            catalogs.iter().find(|c| {
                let tag = norm(&c.language);
                primary(&tag) == primary(&wanted)
                    && tag
                        .split('-')
                        .skip(1)
                        .any(|part| script_or_region.contains(&part))
            })
        })
        .or_else(|| {
            catalogs
                .iter()
                .find(|c| primary(&norm(&c.language)) == primary(&wanted))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DE: &str = r#"
language = "de"
name = "Deutsch"

[messages]
"Open…" = "Öffnen…"
"{count} lines" = "{count} Zeilen"
"Untranslated" = ""
"#;

    fn catalog(language: &str) -> Catalog {
        Catalog {
            language: language.into(),
            name: language.into(),
            messages: HashMap::new(),
        }
    }

    #[test]
    fn parses_and_skips_empty_translations() {
        let c = Catalog::parse(DE).unwrap();
        assert_eq!((c.language.as_str(), c.name.as_str()), ("de", "Deutsch"));
        assert_eq!(c.get("Open…"), Some("Öffnen…"));
        assert_eq!(c.get("Untranslated"), None);
        assert_eq!(c.get("Missing"), None);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn rejects_bad_catalogs() {
        assert!(Catalog::parse("language = \"\"").is_err());
        assert!(Catalog::parse("name = \"x\"").is_err());
        assert!(Catalog::parse("language = ").is_err());
        let c = Catalog::parse("language = \"fr\"").unwrap();
        assert_eq!(c.name, "fr");
        assert!(c.is_empty());
    }

    #[test]
    fn fills_placeholders() {
        let n = 12;
        assert_eq!(fill("{count} lines", &[("count", &n)]), "12 lines");
        assert_eq!(
            fill("{a} of {b}, {a}", &[("a", &1), ("b", &"two")]),
            "1 of two, 1"
        );
        assert_eq!(fill("{unknown} {{x}}", &[]), "{unknown} {x}");
        assert_eq!(fill("odd { and } braces", &[]), "odd { and } braces");
        assert_eq!(fill("", &[]), "");
        assert_eq!(fill("é{x}ü", &[("x", &"ß")]), "éßü");
    }

    #[test]
    fn chooses_a_language() {
        let all = [
            catalog("de-DE"),
            catalog("zh-CN"),
            catalog("zh-TW"),
            catalog("pt"),
        ];
        let pick = |w: &str| choose(&all, w).map(|c| c.language.as_str());
        assert_eq!(pick("de_DE.UTF-8"), Some("de-DE"));
        assert_eq!(pick("de"), Some("de-DE"));
        assert_eq!(pick("zh-CN"), Some("zh-CN"));
        assert_eq!(pick("zh_TW"), Some("zh-TW"));
        assert_eq!(pick("zh-Hant-TW"), Some("zh-TW"));
        assert_eq!(pick("zh-HK"), Some("zh-CN"), "first of the language");
        assert_eq!(pick("pt-BR"), Some("pt"));
        assert_eq!(pick("en-US"), None);
        assert_eq!(pick("fr"), None);
        assert_eq!(pick(""), None);
    }
}
