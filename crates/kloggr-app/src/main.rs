//! kloggr desktop app. See CLAUDE.md for the roadmap.

// No console window next to the app on Windows (debug builds keep it for
// logs and panics).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod archives;
mod background;
mod crash_report;
mod crawler;
mod follow;
mod go_to_line;
mod help;
mod highlighters;
mod highlighters_dialog;
mod key_capture;
mod log_view;
mod options_dialog;
mod preferences;
mod quick_find;
mod results;
mod saved_searches;
mod scratchpad;
mod shortcuts;
mod switcher;
mod update_check;
mod workspace;

use std::path::PathBuf;

use gpui::{App, AppContext, WindowOptions};
use gpui_component::Root;
use workspace::{Quit, Workspace};

/// Window app id (Wayland) and class (X11): the `.desktop` file's name, so
/// desktops show the launcher's icon for kloggr's windows.
pub const APP_ID: &str = "kloggr";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    if let Some(dir) = kloggr_core::settings::config_dir() {
        kloggr_core::crash::install_hook(dir);
    }

    gpui_platform::application()
        // SVG icons used by gpui-component (menu check marks, chevrons).
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            let config_dir = kloggr_core::settings::config_dir();
            // Options first: they bind the shortcuts.
            preferences::Preferences::init(config_dir.clone(), cx);
            cx.bind_keys(scratchpad::key_bindings());
            preferences::Preferences::apply_style(None, cx);
            highlighters::Highlighters::init(config_dir.clone(), cx);
            saved_searches::SavedSearches::init(config_dir.clone(), cx);
            // klogg: the last session's windows, then the file given in
            // the first one.
            let mut windows = if preferences::Preferences::get(cx).general.load_last_session {
                config_dir
                    .as_deref()
                    .map(|dir| kloggr_core::settings::Session::load(dir).all_windows())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if windows.is_empty() {
                windows.push(Default::default());
            }
            let mut path = path;
            let mut first = None;
            for files in windows {
                let handle = open_main_window(config_dir.clone(), Some(files), path.take(), cx);
                first = first.or(handle);
            }
            let main_window = first.expect("failed to open window");
            if let Some(dir) = config_dir.clone() {
                main_window
                    .update(cx, |_, window, cx| {
                        window.defer(cx, move |window, cx| {
                            crash_report::show_pending(&dir, window, cx)
                        })
                    })
                    .ok();
            }
            if let Some(dir) = config_dir {
                update_check::start(dir, main_window, cx);
            }
            // The scratchpad and documentation windows don't keep the app
            // open (klogg's are children of the main window).
            cx.on_window_closed(|cx, _| {
                let windows = cx.windows();
                if windows.iter().all(|w| {
                    scratchpad::is_scratchpad_window(*w, cx)
                        || help::is_documentation_window(*w, cx)
                }) {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}

/// Opens a main window: a workspace with `files` from the session, and
/// `path` (klogg: `MainWindow`, File > New Window).
pub(crate) fn open_main_window(
    config_dir: Option<PathBuf>,
    files: Option<kloggr_core::settings::WindowFiles>,
    path: Option<PathBuf>,
    cx: &mut App,
) -> Option<gpui::AnyWindowHandle> {
    let options = WindowOptions {
        app_id: Some(APP_ID.into()),
        ..window_base_options()
    };
    cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(config_dir, window, cx);
            if let Some(files) = files {
                workspace.restore(files, window, cx);
            }
            if let Some(path) = path {
                workspace.open(path, window, cx);
            }
            workspace
        });
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .inspect_err(|e| tracing::error!("cannot open a window: {e}"))
    .ok()
    .map(Into::into)
}

/// Window options shared by every window. Linux draws its own title bar
/// (GNOME on Wayland has no server decorations); Windows and macOS keep the
/// native one.
pub(crate) fn window_base_options() -> WindowOptions {
    if cfg!(target_os = "linux") {
        gpui_component::TitleBar::window_options()
    } else {
        WindowOptions::default()
    }
}
