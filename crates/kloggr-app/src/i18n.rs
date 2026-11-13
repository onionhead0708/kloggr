//! The interface's language (klogg: `MainWindow::installLanguage`).
//!
//! Every message shown to the user goes through [`tr`], or [`tr!`] when it
//! holds values, with its English text as the key:
//!
//! ```ignore
//! MenuItem::action(tr("Open…"), Open)
//! tr!("{count} lines", count = n)
//! ```
//!
//! The catalog is chosen once at startup (Options > General > Language, else
//! the system's language) and applies from the next start, as in klogg.
//! Catalogs come with the app ([`BUILT_IN`]) or from the `translations`
//! folder of the configuration folder, which lets a translator try a catalog
//! without building kloggr. See `docs/translating.md`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use gpui::SharedString;
use kloggr_core::i18n::{Catalog, choose};

/// Catalogs built into the app: `include_str!("../locales/<tag>.toml")`.
pub const BUILT_IN: &[&str] = &[];

/// The folder of the configuration folder with extra catalogs.
pub const FOLDER: &str = "translations";

struct Active {
    messages: HashMap<&'static str, SharedString>,
}

static ACTIVE: OnceLock<Active> = OnceLock::new();

/// The catalogs to choose from: the built-in ones, then those of the
/// configuration folder's `translations` folder, which replace a built-in
/// catalog of the same language. Unreadable files are logged and skipped.
pub fn available(config_dir: Option<&Path>) -> Vec<Catalog> {
    let mut catalogs: Vec<Catalog> = BUILT_IN
        .iter()
        .filter_map(|text| {
            Catalog::parse(text)
                .inspect_err(|e| tracing::error!("built-in catalog: {e}"))
                .ok()
        })
        .collect();
    let dir = config_dir.map(|d| d.join(FOLDER));
    let mut files: Vec<_> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for path in files {
        match Catalog::load(&path) {
            Ok(c) => {
                catalogs.retain(|old| !old.language.eq_ignore_ascii_case(&c.language));
                catalogs.push(c);
            }
            Err(e) => tracing::warn!("translation {}: {e}", path.display()),
        }
    }
    catalogs
}

/// The system's language, e.g. `"de-DE"`.
pub fn system_language() -> Option<String> {
    sys_locale::get_locale()
}

/// Loads the catalog for `language` (a tag, or `None` for the system's).
/// Only the first call counts.
pub fn init(language: Option<&str>, config_dir: Option<&Path>) {
    let wanted = language
        .map(str::to_owned)
        .or_else(system_language)
        .unwrap_or_default();
    let catalogs = available(config_dir);
    let messages = match choose(&catalogs, &wanted) {
        Some(c) => {
            tracing::info!("language {wanted}: {} ({} messages)", c.language, c.len());
            message_table(c)
        }
        None => HashMap::new(),
    };
    ACTIVE.set(Active { messages }).ok();
}

/// The catalog's messages that the app uses, keyed by the app's own
/// strings so a lookup needs no allocation.
fn message_table(catalog: &Catalog) -> HashMap<&'static str, SharedString> {
    MESSAGES
        .iter()
        .filter_map(|&m| {
            catalog
                .get(m)
                .map(|t| (m, SharedString::from(t.to_owned())))
        })
        .collect()
}

/// `message` in the interface's language.
pub fn tr(message: &'static str) -> SharedString {
    ACTIVE
        .get()
        .and_then(|a| a.messages.get(message).cloned())
        .unwrap_or(SharedString::new_static(message))
}

/// [`tr`](fn@tr) for messages with `{name}` placeholders:
/// `tr!("{count} lines", count = n)`.
#[macro_export]
macro_rules! tr {
    ($message:literal $(,)?) => {
        $crate::i18n::tr($message)
    };
    ($message:literal, $($name:ident = $value:expr),+ $(,)?) => {
        ::gpui::SharedString::from(::kloggr_core::i18n::fill(
            &$crate::i18n::tr($message),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        ))
    };
}

/// Marks a message kept in a constant, to be translated where it is shown
/// with [`tr`](fn@tr): `const STYLES: [&str; 2] = [msg!("Light"), msg!("Dark")]`.
/// It expands to the literal; the template lists it.
#[macro_export]
macro_rules! msg {
    ($message:literal) => {
        $message
    };
}

/// Every message of the app, from `locales/template.toml` (kept up to date
/// by the `template_lists_every_message` test).
static MESSAGES: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| template_messages(include_str!("../locales/template.toml")));

/// The keys of the template's `[messages]` table.
fn template_messages(template: &'static str) -> Vec<&'static str> {
    // The template is generated: one `"key" = ""` per line, keys escaped
    // only for `"` and `\`, so they are found without a TOML parser that
    // would hand back owned strings.
    template
        .lines()
        .filter_map(|l| l.strip_suffix(" = \"\""))
        .filter_map(|k| k.strip_prefix('"')?.strip_suffix('"'))
        .map(|k| {
            if k.contains('\\') {
                // Leaked once per escaped key, at startup.
                &*Box::leak(unescape(k).into_boxed_str())
            } else {
                k
            }
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The messages passed to `tr(…)` and `tr!(…)` in the app's source.
    fn messages_in_source() -> Vec<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            // Not this file: its docs and tests only show examples.
            if path.ends_with("i18n.rs") {
                continue;
            }
            // Git may check sources out with CRLF line ends (Windows).
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            // Not the tests, whose strings aren't shown.
            let text = text
                .find("#[cfg(test)]\nmod tests")
                .map_or(text.as_str(), |i| &text[..i]);
            for start in ["tr(\"", "tr!(\"", "tr!(\n", "msg!(\""] {
                let mut rest = text;
                while let Some(i) = rest.find(start) {
                    // `tr(` as a whole word: not `attr(` or `ctr(`.
                    let before = rest[..i].chars().next_back();
                    rest = &rest[i + start.len()..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let lit = if start.ends_with('\n') {
                        // `tr!(` then the message on the next line.
                        let trimmed = rest.trim_start();
                        let Some(t) = trimmed.strip_prefix('"') else {
                            continue;
                        };
                        t
                    } else {
                        rest
                    };
                    out.push(string_literal(lit));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The Rust string literal at the start of `s` (after its quote).
    fn string_literal(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return out,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('\n') => {
                        // Line continuation: skip the next line's indent.
                        let rest: String = chars.clone().collect();
                        let skipped = rest.len() - rest.trim_start().len();
                        for _ in 0..rest[..skipped].chars().count() {
                            chars.next();
                        }
                    }
                    Some(c) => out.push(c),
                    None => {}
                },
                c => out.push(c),
            }
        }
        panic!("unterminated string literal");
    }

    fn template(messages: &[String]) -> String {
        let mut out = String::from(
            "# kloggr translation template: every message of the interface.\n\
             # Generated by `KLOGGR_UPDATE_TEMPLATE=1 cargo test -p kloggr-app template`.\n\
             # To translate, copy it to <language>.toml; see docs/translating.md.\n\
             \n\
             language = \"\"   # BCP 47 tag, e.g. \"de\" or \"zh-CN\"\n\
             name = \"\"       # the language's name in itself, e.g. \"Deutsch\"\n\
             \n\
             [messages]\n",
        );
        for m in messages {
            let key = m
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\t', "\\t");
            out.push_str(&format!("\"{key}\" = \"\"\n"));
        }
        out
    }

    #[test]
    fn template_lists_every_message() {
        let messages = messages_in_source();
        assert!(messages.len() > 100, "found {}", messages.len());
        let want = template(&messages);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales/template.toml");
        if std::env::var_os("KLOGGR_UPDATE_TEMPLATE").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            have == want,
            "locales/template.toml is out of date: run \
             KLOGGR_UPDATE_TEMPLATE=1 cargo test -p kloggr-app template"
        );
        // It parses as a catalog once given a language, and its keys read
        // back the same.
        let filled = have.replace("language = \"\"", "language = \"xx\"");
        Catalog::parse(&filled).unwrap();
        let keys: Vec<String> = template_messages(Box::leak(have.into_boxed_str()))
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(keys, messages);
    }

    #[test]
    fn placeholders_survive_translation() {
        // Placeholders in messages are plain names, so `fill` finds them.
        for m in messages_in_source() {
            let mut rest = m.as_str();
            while let Some(i) = rest.find('{') {
                let end = rest[i..].find('}').map(|e| i + e).unwrap_or_else(|| {
                    panic!("unclosed placeholder in {m:?}");
                });
                let name = &rest[i + 1..end];
                assert!(
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
                    "placeholder {name:?} in {m:?}"
                );
                rest = &rest[end + 1..];
            }
        }
    }

    #[test]
    fn core_labels_are_in_the_template() {
        use kloggr_core::search::Visibility;
        let groups: Vec<&str> = kloggr_core::encoding::MENU
            .iter()
            .map(|(g, _)| *g)
            .collect();
        assert_eq!(groups, crate::workspace::ENCODING_GROUPS);
        let labels: Vec<&str> = Visibility::ALL.iter().map(|v| v.label()).collect();
        assert_eq!(labels, crate::crawler::VISIBILITY_LABELS);
    }

    #[test]
    fn falls_back_to_english() {
        // No catalog is active in tests.
        assert_eq!(tr("Open…"), "Open…");
        assert_eq!(tr!("{count} lines", count = 3), "3 lines");
    }
}
