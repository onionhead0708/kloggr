//! The Help menu (klogg: `MainWindow::documentation`, `about`, and
//! `IssueReporter::reportIssue`).
//!
//! Documentation shows `docs/user-guide.md`, built into the binary, in its
//! own window as klogg shows its built-in `documentation.html`. Report Issue
//! opens a new GitHub issue with klogg's bug template, the version and the
//! system filled in.

use gpui::{
    App, AppContext, Bounds, Context, Global, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, actions, div, px, size,
};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme, Root, TitleBar, WindowExt, v_flex};
use kloggr_core::crash;

actions!(help, [ShowDocumentation, ReportIssue, About]);

/// The user guide shown by Help > Documentation.
pub const GUIDE: &str = include_str!("../../../docs/user-guide.md");

/// The documentation window's content.
pub struct Documentation;

impl Render for Documentation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let titlebar = cfg!(target_os = "linux")
            .then(|| TitleBar::new().child(div().text_sm().child("kloggr documentation")));
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .children(titlebar)
            .child(
                div().flex_1().min_h_0().px_6().py_4().child(
                    TextView::markdown("user-guide", GUIDE)
                        .selectable(true)
                        .scrollable(true)
                        .size_full(),
                ),
            )
    }
}

/// The open documentation window, if any.
struct DocumentationWindow(Option<WindowHandle<Root>>);

impl Global for DocumentationWindow {}

/// Whether `window` is the documentation's (it doesn't keep the app open).
pub fn is_documentation_window(window: gpui::AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<DocumentationWindow>()
        .and_then(|d| d.0)
        .is_some_and(|handle| window == handle.into())
}

/// Help > Documentation: shows the user guide, opening its window if needed.
pub fn show_documentation(cx: &mut App) {
    // Not from inside another window's update.
    cx.defer(|cx| {
        let open = cx
            .try_global::<DocumentationWindow>()
            .and_then(|d| d.0)
            .filter(|handle| handle.update(cx, |_, w, _| w.activate_window()).is_ok());
        if open.is_some() {
            return;
        }
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(820.), px(860.)),
                cx,
            ))),
            app_id: Some(crate::APP_ID.into()),
            ..crate::window_base_options()
        };
        if !cfg!(target_os = "linux") {
            options.titlebar = Some(TitlebarOptions {
                title: Some("kloggr documentation".into()),
                ..Default::default()
            });
        }
        let handle = cx
            .open_window(options, |window, cx| {
                let docs = cx.new(|_| Documentation);
                cx.new(|cx| Root::new(docs, window, cx))
            })
            .ok();
        cx.set_global(DocumentationWindow(handle));
    });
}

/// Help > Report Issue (klogg: `IssueReporter::reportIssue` with
/// `IssueTemplate::Bug`).
pub fn report_issue(cx: &mut App) {
    cx.open_url(&crash::bug_report_url());
}

/// Help > About (klogg: `MainWindow::about`).
pub fn about(window: &mut Window, cx: &mut App) {
    let text = format!(
        "**kloggr {}**\n\n\
         A fast log viewer for very large files.\n\n\
         [{repo}]({repo})\n\n\
         kloggr is a Rust rewrite of [klogg](https://github.com/variar/klogg) \
         by Anton Filimonov and contributors, itself a fork of \
         [glogg](https://glogg.bonnefon.org) by Nicolas Bonnefon.\n\n\
         You may modify and redistribute the program under the terms of the \
         GPL (version 3 or later).",
        env!("CARGO_PKG_VERSION"),
        repo = kloggr_core::REPOSITORY,
    );
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title("About kloggr")
            .description(TextView::markdown("about", text.clone()).selectable(true))
            .ok_text("Close")
    });
}
