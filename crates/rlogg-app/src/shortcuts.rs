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

    fn binding(&self, key: &str, action: Box<dyn Action>) -> Option<KeyBinding> {
        let stroke = match Keystroke::parse(key) {
            Ok(stroke) => stroke,
            Err(e) => {
                tracing::warn!("shortcut {}: invalid key {key:?}: {e}", self.id);
                return None;
            }
        };
        let context = self
            .context_for(&stroke)
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok())
            .map(Rc::new);
        KeyBinding::load(key, action, context, false, None, &DummyKeyboardMapper)
            .inspect_err(|e| tracing::warn!("shortcut {}: {e}", self.id))
            .ok()
    }

    /// Bindings of `keys` to the action.
    fn bindings(&self, keys: &[String]) -> Vec<KeyBinding> {
        keys.iter()
            .filter_map(|k| self.binding(k, self.action.boxed_clone()))
            .collect()
    }
}

/// Whether `key` is a valid keystroke (for the options dialog).
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
