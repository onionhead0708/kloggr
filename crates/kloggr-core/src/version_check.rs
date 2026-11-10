//! Checking for a newer kloggr (klogg: `versioncheck/`).
//!
//! klogg downloads a version file at most once every 7 days when the
//! option is on, and announces a newer version. kloggr asks GitHub for the
//! repository's latest release ([`LATEST_RELEASE_URL`]) on the same
//! schedule. This module holds what can be tested without a network: the
//! reply, the version comparison and the schedule (`version_check.toml`).
//! The app does the request.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::settings::{self, SettingsError};

/// GitHub's "latest release" of the kloggr repository (drafts and
/// pre-releases are left out by GitHub).
pub const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/onionhead0708/kloggr/releases/latest";

/// klogg: `CHECK_INTERVAL_S`.
pub const CHECK_INTERVAL_SECS: u64 = 7 * 24 * 3600;

const FILE: &str = "version_check.toml";

/// When to check next (klogg: `VersionChecker/nextDeadline`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    /// Seconds since 1970; 0 means now.
    pub next_check: u64,
}

impl Schedule {
    pub fn load(config_dir: &Path) -> Self {
        settings::load_toml(config_dir, FILE).unwrap_or_default()
    }

    pub fn due(&self, now: u64) -> bool {
        self.next_check <= now
    }

    /// After a check, successful or not, the next one is a week later
    /// (klogg extends the deadline either way).
    pub fn checked(now: u64) -> Self {
        Self {
            next_check: now + CHECK_INTERVAL_SECS,
        }
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), SettingsError> {
        settings::save_toml(self, config_dir, FILE)
    }
}

/// A release, from GitHub's JSON.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// The tag, e.g. `v0.2.0`.
    pub tag_name: String,
    /// The release's page.
    pub html_url: String,
    /// Release notes (Markdown).
    #[serde(default)]
    pub body: Option<String>,
}

impl Release {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The version without the tag's `v`.
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches(['v', 'V'])
    }

    /// Whether this release is newer than `current`.
    pub fn is_newer_than(&self, current: &str) -> bool {
        is_newer(current, self.version())
    }
}

/// Compares dotted versions number by number (klogg: `isVersionNewer`).
/// A pre-release (`1.0.0-rc1`) is older than its release; text that isn't
/// a version is never newer.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse(current), parse(candidate)) {
        (Some(current), Some(candidate)) => candidate > current,
        _ => false,
    }
}

/// `(numbers, is_release)`: a release sorts after its pre-releases.
fn parse(version: &str) -> Option<(Vec<u64>, bool)> {
    let version = version.trim().trim_start_matches(['v', 'V']);
    let (numbers, pre) = match version.split_once(['-', '+']) {
        Some((n, rest)) => (n, version[n.len()..].starts_with('-') && !rest.is_empty()),
        None => (version, false),
    };
    let mut parts: Vec<u64> = numbers
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    // 1.2 == 1.2.0
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Some((parts, !pre))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.9", "0.1.10"));
        assert!(is_newer("0.1.0", "v1.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2"));
        assert!(!is_newer("0.2.0", "0.1.99"));
        // Pre-releases come before the release.
        assert!(is_newer("1.0.0-rc1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0-rc1"));
        // Build metadata doesn't count.
        assert!(!is_newer("1.0.0", "1.0.0+build5"));
        assert!(!is_newer("0.1.0", "nightly"));
        assert!(!is_newer("garbage", "1.0.0"));
    }

    #[test]
    fn parses_a_github_release() {
        let json = r#"{"url":"x","tag_name":"v0.3.1","name":"kloggr v0.3.1",
            "html_url":"https://github.com/onionhead0708/kloggr/releases/tag/v0.3.1",
            "body":"Fixes","draft":false,"assets":[]}"#;
        let release = Release::parse(json).unwrap();
        assert_eq!(release.version(), "0.3.1");
        assert_eq!(release.body.as_deref(), Some("Fixes"));
        assert!(release.is_newer_than("0.3.0"));
        assert!(!release.is_newer_than("0.3.1"));
        assert!(Release::parse(r#"{"message":"Not Found"}"#).is_err());
    }

    #[test]
    fn schedule() {
        let dir = std::env::temp_dir().join(format!("kloggr-vcheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Schedule::load(&dir);
        assert!(s.due(0));
        let next = Schedule::checked(1000);
        assert!(!next.due(1000 + CHECK_INTERVAL_SECS - 1));
        assert!(next.due(1000 + CHECK_INTERVAL_SECS));
        next.save(&dir).unwrap();
        assert_eq!(Schedule::load(&dir), next);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
