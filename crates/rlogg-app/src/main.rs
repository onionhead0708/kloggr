//! rlogg desktop app. See CLAUDE.md for the roadmap.

mod archives;
mod background;
mod crash_report;
mod crawler;
mod follow;
mod go_to_line;
mod highlighters;
mod highlighters_dialog;
mod key_capture;
mod log_view;
mod options_dialog;
mod preferences;
mod quick_find;
mod saved_searches;
mod scratchpad;
mod shortcuts;
mod workspace;

use std::path::PathBuf;

use gpui::{App, AppContext, WindowOptions};
use gpui_component::Root;
use workspace::{Quit, Workspace};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    if let Some(dir) = rlogg_core::settings::config_dir() {
        rlogg_core::crash::install_hook(dir);
    }

    gpui_platform::application()
        // SVG icons used by gpui-component (menu check marks, chevrons).
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            let config_dir = rlogg_core::settings::config_dir();
            // Options first: they bind the shortcuts.
            preferences::Preferences::init(config_dir.clone(), cx);
            cx.bind_keys(scratchpad::key_bindings());
            preferences::Preferences::apply_style(None, cx);
            highlighters::Highlighters::init(config_dir.clone(), cx);
            saved_searches::SavedSearches::init(config_dir.clone(), cx);
            cx.open_window(WindowOptions::default(), |window, cx| {
                let crash_dir = config_dir.clone();
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::new(config_dir, window, cx);
                    // klogg: the last session first, then the file given.
                    if preferences::Preferences::get(cx).general.load_last_session {
                        workspace.restore_session(window, cx);
                    }
                    if let Some(path) = path {
                        workspace.open(path, window, cx);
                    }
                    workspace
                });
                if let Some(dir) = crash_dir {
                    window.defer(cx, move |window, cx| {
                        crash_report::show_pending(&dir, window, cx)
                    });
                }
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("failed to open window");
            // The scratchpad window doesn't keep the app open (klogg's is
            // a child of the main window).
            cx.on_window_closed(|cx, _| {
                let windows = cx.windows();
                if windows
                    .iter()
                    .all(|w| scratchpad::is_scratchpad_window(*w, cx))
                {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
