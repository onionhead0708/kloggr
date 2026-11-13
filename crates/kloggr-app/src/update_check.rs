//! Announces a newer kloggr (klogg: `VersionChecker`, `newVersionNotification`).
//! See [`kloggr_core::version_check`].

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{AnyWindowHandle, App};
use gpui_component::WindowExt;
use kloggr_core::version_check::{LATEST_RELEASE_URL, Release, Schedule};

use crate::preferences::Preferences;
use crate::tr;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Checks in the background when the option is on and a week has passed
/// since the last check.
pub fn start(config_dir: PathBuf, window: AnyWindowHandle, cx: &mut App) {
    if !Preferences::get(cx).general.check_for_updates {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if !Schedule::load(&config_dir).due(now) {
        return;
    }
    // Counted as checked even if it fails, as klogg does: no retry on every
    // start while offline.
    if let Err(e) = Schedule::checked(now).save(&config_dir) {
        tracing::warn!("can't save the version check schedule: {e}");
    }
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let release = crate::background::run_blocking("kloggr-version-check", &executor, fetch)
            .await
            .and_then(|r| {
                r.map_err(|e| tracing::info!("version check failed: {e}"))
                    .ok()
            });
        let Some(release) = release.filter(|r| r.is_newer_than(VERSION)) else {
            return;
        };
        window
            .update(cx, |_, window, cx| announce(release, window, cx))
            .ok();
    })
    .detach();
}

fn fetch() -> anyhow::Result<Release> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    // Debug builds can point elsewhere, to try the notice.
    let url = std::env::var("KLOGGR_RELEASE_URL")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .unwrap_or_else(|| LATEST_RELEASE_URL.to_owned());
    let json = agent
        .get(&url)
        .header("User-Agent", concat!("kloggr/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_to_string()?;
    Ok(Release::parse(&json)?)
}

fn announce(release: Release, window: &mut gpui::Window, cx: &mut App) {
    // The start of the release notes, without Markdown heading marks.
    let mut notes = String::new();
    for line in release.body.as_deref().unwrap_or_default().lines() {
        let line = line.trim_start_matches('#').trim();
        if line.is_empty() {
            continue;
        }
        if notes.len() + line.len() > 400 {
            notes += "\n…";
            break;
        }
        if !notes.is_empty() {
            notes.push('\n');
        }
        notes += line;
    }
    let description = if notes.is_empty() {
        tr!("You have version {version}.", version = VERSION)
    } else {
        tr!(
            "You have version {version}.\n\n{notes}",
            version = VERSION,
            notes = notes
        )
    };
    let (title, url) = (
        tr!("kloggr {version} is available", version = release.version()),
        release.html_url,
    );
    window.open_alert_dialog(cx, move |alert, _, _| {
        let url = url.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .ok_text(tr!("Open download page"))
            .show_cancel(true)
            .cancel_text(tr!("Later"))
            .on_ok(move |_, _, cx| {
                cx.open_url(&url);
                true
            })
    });
}
