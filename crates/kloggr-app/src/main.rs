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

/// The log file of this run, if one is written (shown in Help > About).
pub(crate) static LOG_FILE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn main() {
    let args = match Args::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            println!("{message}");
            return;
        }
    };
    let options =
        kloggr_core::settings::config_dir().map(|d| kloggr_core::options::Options::load(&d));
    if let Some(path) = init_logging(&args, options.as_ref()) {
        LOG_FILE.set(path).ok();
    }
    tracing::info!(
        "kloggr {} on {}-{}, files {:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        args.files
    );
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
    let cli = Args {
        files: Vec::new(),
        ..args.clone()
    };
    let follow = cli.follow;
    let paths = args.files;

    // macOS: files opened from Finder (Open With, double-click) arrive as
    // file:// URLs, at launch or while running.
    let (opened_tx, opened_rx) = async_channel::unbounded::<Vec<String>>();
    let app = gpui_platform::application()
        // SVG icons used by gpui-component (menu check marks, chevrons).
        .with_assets(gpui_kit_assets::Assets);
    app.on_open_urls(move |urls| {
        opened_tx.try_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let config_dir = kloggr_core::settings::config_dir();
        // Options first: they bind the shortcuts.
        preferences::Preferences::init(config_dir.clone(), cx);
        cx.bind_keys(scratchpad::key_bindings());
        preferences::Preferences::apply_style(None, cx);
        highlighters::Highlighters::init(config_dir.clone(), cx);
        saved_searches::SavedSearches::init(config_dir.clone(), cx);
        // klogg: the last session's windows, then the files given in
        // the first one. Files given start a new session (unless -s).
        let load_last = preferences::Preferences::get(cx).general.load_last_session;
        let reopen = cli.reopen_session(!paths.is_empty(), load_last);
        let mut windows = if reopen {
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
            let handle = open_main_window(config_dir.clone(), Some(files), paths, follow, cx);
            first = first.or(handle);
        }
        if single && let Some(dir) = config_dir.clone() {
            receive_files(dir, cx);
        }
        let finder_dir = config_dir.clone();
        cx.spawn(async move |cx| {
            while let Ok(urls) = opened_rx.recv().await {
                let files = urls
                    .iter()
                    .filter_map(|u| kloggr_core::instance::path_from_file_url(u))
                    .collect();
                let dir = finder_dir.clone();
                cx.update(|cx| workspace::open_from_another_launch(files, dir, cx));
            }
        })
        .detach();
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
                scratchpad::is_scratchpad_window(*w, cx) || help::is_documentation_window(*w, cx)
            }) {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The command line (klogg: `CliParameters`).
#[derive(Debug, Default, Clone, PartialEq)]
struct Args {
    files: Vec<PathBuf>,
    /// -m, --multi: don't hand the files to a running kloggr.
    multi: bool,
    /// -f, --follow: follow the files given.
    follow: bool,
    /// -s, --load-session: reopen the last session even with files given.
    load_session: bool,
    /// -n, --new-session: don't reopen the last session.
    new_session: bool,
    /// -l, --log: write a log file.
    log: bool,
    /// -d, --debug, once per level above warnings.
    debug: usize,
}

impl Args {
    const HELP: &str = "Usage: kloggr [options] [files…]

A fast log viewer for very large files.

Options:
  -f, --follow       follow the files given
  -s, --load-session reopen the last session (the default without files)
  -n, --new-session  don't reopen the last session (the default with files)
  -l, --log          write a log file in the temporary folder
  -d, --debug        log more: -d info, -dd debug, -ddd trace
  -m, --multi        allow several kloggr to run, instead of opening the files
                     in the one already running
  -h, --help         print this help and exit
  -v, --version      print the version and exit";

    /// The arguments, or the text to print and exit with. Short options
    /// can be combined (`-fm`).
    fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, String> {
        let mut out = Self::default();
        let mut only_files = false;
        for arg in args {
            let text = arg.to_str();
            let flags: Vec<String> = match text {
                _ if only_files => {
                    out.files.push(arg.into());
                    continue;
                }
                Some("--") => {
                    only_files = true;
                    continue;
                }
                // Older macOS passes a process serial number to apps
                // started from Finder.
                Some(a) if a.starts_with("-psn_") => continue,
                Some(a) if a.starts_with("--") => vec![a.to_owned()],
                Some(a) if a.starts_with('-') && a.len() > 1 => {
                    a[1..].chars().map(|c| format!("-{c}")).collect()
                }
                _ => {
                    out.files.push(arg.into());
                    continue;
                }
            };
            for flag in flags {
                match flag.as_str() {
                    "-m" | "--multi" => out.multi = true,
                    "-f" | "--follow" => out.follow = true,
                    "-s" | "--load-session" => out.load_session = true,
                    "-n" | "--new-session" => out.new_session = true,
                    "-l" | "--log" => out.log = true,
                    "-d" | "--debug" => out.debug += 1,
                    "-h" | "--help" => return Err(Self::HELP.to_owned()),
                    "-v" | "--version" => {
                        return Err(format!("kloggr {}", env!("CARGO_PKG_VERSION")));
                    }
                    other => return Err(format!("Unknown option {other}\n\n{}", Self::HELP)),
                }
            }
        }
        Ok(out)
    }

    /// Whether to reopen the last session (klogg: `load_session ||
    /// (filenames.empty() && !new_session && loadLastSession)`).
    fn reopen_session(&self, files_given: bool, load_last_session: bool) -> bool {
        self.load_session || (!files_given && !self.new_session && load_last_session)
    }
}

/// Sets up the log (klogg: `enableLogging`, `enableFileLogging`): errors
/// on the terminal, or more with `-d`; with `-l` or Options > General > "Write
/// a log file", also a file in the temporary folder at the higher of the two
/// levels. Only kloggr's own messages go below warnings. `RUST_LOG`
/// overrides both. Returns the file's path.
fn init_logging(args: &Args, options: Option<&kloggr_core::options::Options>) -> Option<PathBuf> {
    use kloggr_core::options::LogLevel;
    use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

    let env = std::env::var("RUST_LOG").ok().filter(|e| !e.is_empty());
    let filter = |level: Option<LogLevel>| match (&env, level) {
        (Some(directives), _) => EnvFilter::new(directives),
        (None, Some(level)) => EnvFilter::new(format!(
            "warn,kloggr={0},kloggr_core={0}",
            level.directive()
        )),
        (None, None) => EnvFilter::new("error"),
    };
    let cli = (args.debug > 0).then(|| LogLevel::from_debug_flags(args.debug));
    let terminal = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter(cli));
    let file_level = [
        args.log.then(|| cli.unwrap_or(LogLevel::Warning)),
        options
            .filter(|o| o.general.log_file)
            .map(|o| o.general.log_level),
    ]
    .into_iter()
    .flatten()
    .max();
    let mut path = None;
    let file = file_level.and_then(|level| {
        let name = format!(
            "kloggr_{}_{}.log",
            chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"),
            std::process::id()
        );
        let file_path = std::env::temp_dir().join(name);
        let file = std::fs::File::create(&file_path).ok()?;
        path = Some(file_path);
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .with_filter(filter(Some(level))),
        )
    });
    tracing_subscriber::registry()
        .with(terminal)
        .with(file)
        .init();
    path
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
            cx.update(|cx| workspace::open_from_another_launch(files, Some(dir), cx));
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
    follow: bool,
    cx: &mut App,
) -> Option<gpui::AnyWindowHandle> {
    let options = WindowOptions {
        app_id: Some(APP_ID.into()),
        window_bounds: files
            .as_ref()
            .and_then(|f| f.geometry)
            .and_then(|g| window_bounds(g, cx)),
        ..window_base_options()
    };
    cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(config_dir, window, cx);
            if let Some(files) = files {
                workspace.restore(files, window, cx);
            }
            for path in paths {
                workspace.open_following(path, follow, window, cx);
            }
            workspace
        });
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .inspect_err(|e| tracing::error!("cannot open a window: {e}"))
    .ok()
    .map(Into::into)
}

/// Where a window saved in the session opens again (klogg:
/// `restoreGeometry`). A window that would be off every display (one was
/// unplugged) keeps its size but is centred instead.
fn window_bounds(g: kloggr_core::settings::WindowGeometry, cx: &App) -> Option<gpui::WindowBounds> {
    use gpui::{Bounds, point, px, size};
    if !(g.width >= 200. && g.height >= 150.) {
        return None;
    }
    let saved = Bounds::new(point(px(g.x), px(g.y)), size(px(g.width), px(g.height)));
    let visible = cx.displays().iter().any(|d| {
        let shown = d.bounds().intersect(&saved);
        f32::from(shown.size.width) >= 100. && f32::from(shown.size.height) >= 50.
    });
    let bounds = if visible {
        saved
    } else {
        Bounds::centered(None, saved.size, cx)
    };
    Some(if g.fullscreen {
        gpui::WindowBounds::Fullscreen(bounds)
    } else if g.maximized {
        gpui::WindowBounds::Maximized(bounds)
    } else {
        gpui::WindowBounds::Windowed(bounds)
    })
}

/// Whether windows draw their own title bar: only on Linux, where GNOME on
/// Wayland has no server decorations. Windows and macOS keep the native one.
pub(crate) const OWN_TITLE_BAR: bool = cfg!(target_os = "linux");

/// Window options shared by every window (see [`OWN_TITLE_BAR`]).
pub(crate) fn window_base_options() -> WindowOptions {
    if OWN_TITLE_BAR {
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

    #[gpui::test]
    fn saved_bounds(cx: &mut gpui::TestAppContext) {
        use kloggr_core::settings::WindowGeometry;
        let g = WindowGeometry {
            x: 40.,
            y: 30.,
            width: 900.,
            height: 600.,
            ..WindowGeometry::default()
        };
        cx.update(|cx| {
            let display = cx.displays()[0].bounds();
            let bounds = |w: Option<gpui::WindowBounds>| match w {
                Some(gpui::WindowBounds::Windowed(b)) => b,
                other => panic!("{other:?}"),
            };
            let shown = bounds(window_bounds(g, cx));
            assert_eq!(
                (f32::from(shown.origin.x), f32::from(shown.size.width)),
                (40., 900.)
            );
            // Off every display: centred, same size.
            let away = WindowGeometry { x: 90_000., ..g };
            let shown = bounds(window_bounds(away, cx));
            assert!(display.intersect(&shown).size == shown.size, "{shown:?}");
            assert_eq!(f32::from(shown.size.height), 600.);
            assert!(matches!(
                window_bounds(
                    WindowGeometry {
                        maximized: true,
                        ..g
                    },
                    cx
                ),
                Some(gpui::WindowBounds::Maximized(_))
            ));
            // Nonsense sizes are ignored.
            assert_eq!(window_bounds(WindowGeometry::default(), cx), None);
        });
    }

    #[test]
    fn command_line() {
        assert_eq!(parse(&[]), Ok(Args::default()));
        assert_eq!(
            parse(&["a.log", "-m", "/var/log/b.log"]),
            Ok(Args {
                files: vec!["a.log".into(), "/var/log/b.log".into()],
                multi: true,
                ..Args::default()
            })
        );
        // Combined short options, and the session rule (klogg's).
        let args = parse(&["-fn", "--load-session", "x.log"]).unwrap();
        assert!(args.follow && args.new_session && args.load_session && !args.multi);
        assert!(args.reopen_session(true, false), "-s reopens it anyway");
        let none = Args::default();
        assert!(none.reopen_session(false, true));
        assert!(
            !none.reopen_session(true, true),
            "files start a new session"
        );
        assert!(!none.reopen_session(false, false), "the option is off");
        let n = parse(&["-n"]).unwrap();
        assert!(!n.reopen_session(false, true));
        assert!(
            parse(&["-fx"])
                .unwrap_err()
                .starts_with("Unknown option -x")
        );
        assert_eq!(parse(&["--", "-m"]).unwrap().files, [PathBuf::from("-m")]);
        assert_eq!(parse(&["-"]).unwrap().files, [PathBuf::from("-")]);
        assert_eq!(parse(&["-psn_0_1234"]), Ok(Args::default()));
        assert!(parse(&["--help"]).unwrap_err().starts_with("Usage: kloggr"));
        assert!(parse(&["-v"]).unwrap_err().starts_with("kloggr 0."));
        assert!(parse(&["-x"]).unwrap_err().starts_with("Unknown option -x"));
        // klogg: -l writes a file, each -d is one level more than warnings.
        let args = parse(&["-l", "-dd"]).unwrap();
        assert!(args.log);
        assert_eq!(args.debug, 2);
    }
}
