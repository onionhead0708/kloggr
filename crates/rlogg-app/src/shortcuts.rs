//! Keyboard shortcuts: klogg's defaults, and the user's changes from the
//! options dialog (klogg: `ShortcutAction`, `settings/src/shortcuts.cpp`).
//!
//! Each module lists its shortcuts with an id (klogg's, where klogg has the
//! action), a label, a key context and the default keys. Keys are GPUI
//! keystrokes (`ctrl-shift-f`, `secondary-o` for Ctrl, or Cmd on macOS).
//!
//! A key without Ctrl, Alt or Cmd (`n`, `shift-g`, `home`) only works in the
//! log views, whatever the shortcut's context, so it can still be typed in
//! the search box (except F1–F24).
//!
//! Each action has up to two keys, a primary and a secondary one, as in
//! klogg's options dialog.
//!
//! GPUI's Linux backends report Shift+digit as the shifted character
//! (Ctrl+Shift+1 is `ctrl-!`) and have no layout mapping for bindings, so
//! on Linux a key with Shift and a digit is also bound as the US layout's
//! symbol (docs/decisions.md).

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke, NoAction,
};

use crate::log_view;

/// One shortcut (klogg: a `ShortcutAction` and its default keys).
pub struct Shortcut {
    pub id: &'static str,
    pub label: &'static str,
    /// Where the keys work (`None`: anywhere in the window).
    pub context: Option<&'static str>,
    pub keys: Vec<String>,
    pub action: Box<dyn Action>,
}

impl Shortcut {
    pub fn new(
        id: &'static str,
        label: &'static str,
        context: Option<&'static str>,
        keys: &[&str],
        action: impl Action,
    ) -> Self {
        Self {
            id,
            label,
            context,
            keys: keys.iter().map(|k| (*k).to_owned()).collect(),
            action: Box::new(action),
        }
    }

    /// The keys in effect, given the user's changes.
    pub fn keys<'a>(&'a self, overrides: &'a BTreeMap<String, Vec<String>>) -> &'a [String] {
        overrides.get(self.id).unwrap_or(&self.keys)
    }

    /// The context `key` is bound in (see the module docs).
    fn context_for(&self, key: &Keystroke) -> Option<&'static str> {
        let m = key.modifiers;
        let function_key = key
            .key
            .strip_prefix('f')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if m.control || m.alt || m.platform || function_key {
            self.context
        } else {
            Some(log_view::CONTEXT)
        }
    }

    /// Bindings of `key` to `action`: the key, and on Linux its US-layout
    /// alias (see the module docs). The alias comes first, so menus show
    /// the key itself.
    fn binding(&self, key: &str, action: Box<dyn Action>) -> Vec<KeyBinding> {
        let stroke = match Keystroke::parse(key) {
            Ok(stroke) => stroke,
            Err(e) => {
                tracing::warn!("shortcut {}: invalid key {key:?}: {e}", self.id);
                return Vec::new();
            }
        };
        let context = self
            .context_for(&stroke)
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok())
            .map(Rc::new);
        let mut keys = Vec::new();
        if cfg!(target_os = "linux")
            && let Some(alias) = shifted_digit_alias(&stroke)
        {
            keys.push(alias);
        }
        keys.push(key.to_owned());
        keys.iter()
            .filter_map(|k| {
                KeyBinding::load(
                    k,
                    action.boxed_clone(),
                    context.clone(),
                    false,
                    None,
                    &DummyKeyboardMapper,
                )
                .inspect_err(|e| tracing::warn!("shortcut {}: {e}", self.id))
                .ok()
            })
            .collect()
    }

    /// Bindings of `keys` to the action.
    fn bindings(&self, keys: &[String]) -> Vec<KeyBinding> {
        keys.iter()
            .flat_map(|k| self.binding(k, self.action.boxed_clone()))
            .collect()
    }
}

/// Shift+0 … Shift+9 on a US layout.
const US_SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];

/// `ctrl-!` for `ctrl-shift-1`: what GPUI reports on Linux.
fn shifted_digit_alias(stroke: &Keystroke) -> Option<String> {
    let digit = stroke.key.parse::<usize>().ok().filter(|d| *d < 10)?;
    if !stroke.modifiers.shift || stroke.key.len() != 1 {
        return None;
    }
    let mut alias = stroke.clone();
    alias.modifiers.shift = false;
    alias.key = US_SHIFTED_DIGITS[digit].to_string();
    alias.key_char = None;
    Some(alias.unparse())
}

/// A key as klogg shows it: `Ctrl+Shift+F`, `PgDown`, `F3`.
pub fn display(key: &str) -> String {
    let Ok(stroke) = Keystroke::parse(key) else {
        return key.to_owned();
    };
    let m = stroke.modifiers;
    let mut parts: Vec<String> = Vec::new();
    if m.control {
        parts.push("Ctrl".into());
    }
    if m.alt {
        parts.push("Alt".into());
    }
    if m.shift {
        parts.push("Shift".into());
    }
    if m.platform {
        parts.push(
            if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Super"
            }
            .into(),
        );
    }
    if m.function {
        parts.push("Fn".into());
    }
    let name = match stroke.key.as_str() {
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDown".to_owned(),
        "escape" => "Esc".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "delete" => "Del".to_owned(),
        "insert" => "Ins".to_owned(),
        k => {
            let mut chars = k.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    };
    parts.push(name);
    parts.join("+")
}

/// A key in one canonical spelling (`secondary-` resolved, modifiers in
/// order), to find duplicates.
pub fn normalize(key: &str) -> String {
    Keystroke::parse(key).map_or_else(|_| key.to_owned(), |k| k.unparse())
}

impl Shortcut {}

/// Whether `key` is a valid keystroke.
#[cfg(test)]
pub fn is_valid_key(key: &str) -> bool {
    let dangling = key.ends_with('-') && key != "-" && !key.ends_with("--");
    !key.is_empty()
        && !dangling
        && !key.contains(char::is_whitespace)
        && Keystroke::parse(key).is_ok_and(|k| !k.key.is_empty())
}

/// Bindings of `shortcuts` with their default keys.
#[cfg(test)]
pub fn default_bindings(shortcuts: Vec<Shortcut>) -> Vec<KeyBinding> {
    shortcuts.iter().flat_map(|s| s.bindings(&s.keys)).collect()
}

/// Every shortcut of the app, in the options dialog's order.
pub fn all() -> Vec<Shortcut> {
    let mut all = Vec::new();
    all.extend(crate::workspace::shortcuts());
    all.extend(crate::crawler::shortcuts());
    all.extend(crate::quick_find::shortcuts());
    all.extend(crate::go_to_line::shortcuts());
    all.extend(crate::follow::shortcuts());
    all.extend(crate::log_view::shortcuts());
    all.extend(crate::highlighters::shortcuts());
    all
}

/// Binds every shortcut, with the user's changes (at startup).
pub fn bind_all(overrides: &BTreeMap<String, Vec<String>>, cx: &mut App) {
    let bindings: Vec<KeyBinding> = all()
        .iter()
        .flat_map(|s| s.bindings(s.keys(overrides)))
        .collect();
    cx.bind_keys(bindings);
}

/// Applies a change of the user's shortcuts while running. GPUI's keymap
/// only grows, so keys that no longer apply are bound to [`NoAction`] in
/// the same context, which hides the earlier binding, and the new keys are
/// bound after that.
pub fn rebind(
    old: &BTreeMap<String, Vec<String>>,
    new: &BTreeMap<String, Vec<String>>,
    cx: &mut App,
) {
    let mut bindings = Vec::new();
    for s in all() {
        let (before, after) = (s.keys(old), s.keys(new));
        if before == after {
            continue;
        }
        for key in before.iter().filter(|k| !after.contains(k)) {
            bindings.extend(s.binding(key, Box::new(NoAction)));
        }
        bindings.extend(s.bindings(after));
    }
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_defaults_valid() {
        let all = all();
        let mut ids = std::collections::HashSet::new();
        for s in &all {
            assert!(ids.insert(s.id), "duplicate id {}", s.id);
            for k in &s.keys {
                assert!(is_valid_key(k), "{}: {k}", s.id);
            }
        }
        assert!(!is_valid_key("ctrl-"));
        assert!(!is_valid_key("ctrl a"));
        assert!(is_valid_key("ctrl-shift-f"));
        assert!(is_valid_key("ctrl--") && is_valid_key("-"));
        assert!(!is_valid_key(""));
    }

    #[test]
    fn at_most_two_keys_and_no_duplicates() {
        let mut seen = std::collections::HashMap::new();
        for s in all() {
            assert!(s.keys.len() <= 2, "{}: {:?}", s.id, s.keys);
            for k in &s.keys {
                if let Some(other) = seen.insert(normalize(k), s.id) {
                    panic!("{k} is bound to {other} and {}", s.id);
                }
            }
        }
    }

    #[test]
    fn display_and_aliases() {
        assert_eq!(display("ctrl-shift-f"), "Ctrl+Shift+F");
        assert_eq!(display("pagedown"), "PgDown");
        assert_eq!(display("f3"), "F3");
        assert_eq!(display("shift-g"), "Shift+G");
        assert_eq!(display("'"), "'");
        assert_eq!(normalize("shift-ctrl-a"), normalize("ctrl-shift-a"));
        let alias = |k: &str| shifted_digit_alias(&Keystroke::parse(k).unwrap());
        assert_eq!(alias("ctrl-shift-1").as_deref(), Some("ctrl-!"));
        assert_eq!(alias("ctrl-shift-0").as_deref(), Some("ctrl-)"));
        assert_eq!(alias("ctrl-1"), None);
        assert_eq!(alias("ctrl-shift-a"), None);
    }

    #[test]
    fn bare_keys_only_work_in_log_views() {
        let s = Shortcut::new("t", "t", Some("Crawler"), &[], NoAction);
        let ctx = |k: &str| s.context_for(&Keystroke::parse(k).unwrap());
        assert_eq!(ctx("n"), Some(log_view::CONTEXT));
        assert_eq!(ctx("shift-n"), Some(log_view::CONTEXT));
        assert_eq!(ctx("home"), Some(log_view::CONTEXT));
        assert_eq!(ctx("f3"), Some("Crawler"));
        assert_eq!(ctx("ctrl-g"), Some("Crawler"));
        assert_eq!(ctx("alt-1"), Some("Crawler"));
    }
}
