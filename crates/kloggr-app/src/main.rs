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
    let args = match Args::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            println!("{message}");
            return;
        }
    };
    // klogg: a second kloggr hands its files to the first, unless
    // -m/--multi.
    let single = !args.multi;
    if let Some(dir) = kloggr_core::settings::config_dir().filter(|_| single) {
        match kloggr_core::instance::send_to_running(&dir, &args.files) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => tracing::warn!("cannot reach the running kloggr: {e}"),
        }
    }
    if let Some(dir) = kloggr_core::settings::config_dir() {
        kloggr_core::crash::install_hook(dir);
    }
    let paths = args.files;

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
            let mut paths = paths;
            let mut first = None;
            for files in windows {
                let paths = std::mem::take(&mut paths);
                let handle = open_main_window(config_dir.clone(), Some(files), paths, cx);
                first = first.or(handle);
            }
            if single && let Some(dir) = config_dir.clone() {
                receive_files(dir, cx);
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

/// The command line (klogg: `CliParameters`).
#[derive(Debug, Default, PartialEq)]
struct Args {
    files: Vec<PathBuf>,
    /// -m, --multi: don't hand the files to a running kloggr.
    multi: bool,
}

impl Args {
    const HELP: &str = "Usage: kloggr [options] [files…]

A fast log viewer for very large files.

Options:
  -m, --multi    allow several kloggr to run, instead of opening the files
                 in the one already running
  -h, --help     print this help and exit
  -v, --version  print the version and exit";

    /// The arguments, or the text to print and exit with.
    fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, String> {
        let mut out = Self::default();
        let mut only_files = false;
        for arg in args {
            match arg.to_str() {
                _ if only_files => out.files.push(arg.into()),
                Some("--") => only_files = true,
                Some("-m" | "--multi") => out.multi = true,
                Some("-h" | "--help") => return Err(Self::HELP.to_owned()),
                Some("-v" | "--version") => {
                    return Err(format!("kloggr {}", env!("CARGO_PKG_VERSION")));
                }
                // Older macOS passes a process serial number to apps
                // started from Finder.
                Some(a) if a.starts_with("-psn_") => {}
                Some(a) if a.starts_with('-') && a.len() > 1 => {
                    return Err(format!("Unknown option {a}\n\n{}", Self::HELP));
                }
                _ => out.files.push(arg.into()),
            }
        }
        Ok(out)
    }
}

/// Files sent by later launches open in the last active window (klogg:
/// `MessageReceiver`, `loadFileNonInteractive`).
fn receive_files(config_dir: PathBuf, cx: &mut App) {
    let server = match kloggr_core::instance::Server::start(&config_dir) {
        Ok(server) => std::sync::Arc::new(server),
        Err(e) => {
            tracing::warn!("other launches won't find this kloggr: {e}");
            return;
        }
    };
    let (tx, rx) = async_channel::unbounded();
    let serving = server.clone();
    std::thread::Builder::new()
        .name("kloggr-instance".into())
        .spawn(move || {
            serving.serve(|files| {
                tx.send_blocking(files).ok();
            })
        })
        .expect("failed to spawn a thread");
    cx.on_app_quit(move |_| {
        server.stop();
        async {}
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(files) = rx.recv().await {
            let dir = config_dir.clone();
            cx.update(|cx| workspace::open_from_another_launch(files, dir, cx));
        }
    })
    .detach();
}

/// Opens a main window: a workspace with `files` from the session, then
/// `paths` (klogg: `MainWindow`, File > New Window).
pub(crate) fn open_main_window(
    config_dir: Option<PathBuf>,
    files: Option<kloggr_core::settings::WindowFiles>,
    paths: Vec<PathBuf>,
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
            for path in paths {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn command_line() {
        assert_eq!(parse(&[]), Ok(Args::default()));
        assert_eq!(
            parse(&["a.log", "-m", "/var/log/b.log"]),
            Ok(Args {
                files: vec!["a.log".into(), "/var/log/b.log".into()],
                multi: true,
            })
        );
        assert_eq!(parse(&["--", "-m"]).unwrap().files, [PathBuf::from("-m")]);
        assert_eq!(parse(&["-"]).unwrap().files, [PathBuf::from("-")]);
        assert_eq!(parse(&["-psn_0_1234"]), Ok(Args::default()));
        assert!(parse(&["--help"]).unwrap_err().starts_with("Usage: kloggr"));
        assert!(parse(&["-v"]).unwrap_err().starts_with("kloggr 0."));
        assert!(parse(&["-x"]).unwrap_err().starts_with("Unknown option -x"));
    }
}
