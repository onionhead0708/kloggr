//! rlogg desktop app. See CLAUDE.md for the roadmap.

mod background;
mod crawler;
mod follow;
mod go_to_line;
mod highlighters;
mod highlighters_dialog;
mod log_view;
mod quick_find;
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

    gpui_platform::application()
        // SVG icons used by gpui-component (menu check marks, chevrons).
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.bind_keys(log_view::key_bindings());
            cx.bind_keys(crawler::key_bindings());
            cx.bind_keys(quick_find::key_bindings());
            cx.bind_keys(go_to_line::key_bindings());
            cx.bind_keys(follow::key_bindings());
            cx.bind_keys(highlighters::key_bindings());
            cx.bind_keys(workspace::key_bindings());
            cx.on_action(|_: &Quit, cx| cx.quit());
            let config_dir = rlogg_core::settings::config_dir();
            highlighters::Highlighters::init(config_dir.clone(), cx);
            cx.open_window(WindowOptions::default(), |window, cx| {
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::new(config_dir, window, cx);
                    // klogg: the last session first, then the file given.
                    workspace.restore_session(window, cx);
                    if let Some(path) = path {
                        workspace.open(path, window, cx);
                    }
                    workspace
                });
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("failed to open window");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
