//! Shows the crash report left by the last run (klogg: `CrashHandler`'s
//! "send report" dialog at startup). See [`kloggr_core::crash`].

use std::path::Path;

use crate::tr;
use gpui::{App, ClipboardItem, Window};
use gpui_component::WindowExt;
use kloggr_core::crash;

/// Report bytes put in the issue link; GitHub refuses links of about 8 KB.
const ISSUE_BODY_MAX: usize = 4000;

/// If the last run crashed, says so once, with its report to copy.
pub fn show_pending(config_dir: &Path, window: &mut Window, cx: &mut App) {
    let pending = crash::pending(config_dir);
    let Some(newest) = pending.last() else { return };
    let text = std::fs::read_to_string(newest).unwrap_or_default();
    let moved = crash::mark_shown(config_dir);
    let saved = moved.last().unwrap_or(newest).display().to_string();
    let first = text
        .lines()
        .next()
        .map_or_else(|| tr!("kloggr crashed"), |l| l.to_owned().into());
    let others = match pending.len() - 1 {
        0 => gpui::SharedString::default(),
        1 => tr!(" There is 1 older report next to it."),
        n => tr!(" There are {count} older reports next to it.", count = n),
    };
    window.open_alert_dialog(cx, move |alert, _, _| {
        let text = text.clone();
        alert
            .title(tr!("kloggr closed unexpectedly"))
            .description(tr!(
                "{first}\n\nThe report is saved in {saved}.{others} \
                 Report issue opens a new GitHub issue with it, and copies it \
                 to the clipboard.",
                first = first,
                saved = saved,
                others = others,
            ))
            .ok_text(tr!("Report issue"))
            .show_cancel(true)
            .cancel_text(tr!("Close"))
            .on_ok(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                cx.open_url(&crash::issue_url(&text, ISSUE_BODY_MAX));
                true
            })
    });
}
