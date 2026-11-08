//! rlogg desktop app. Scaffold: opens the file given on the command line and
//! shows it in a virtualized list. See CLAUDE.md for the roadmap.

use std::ops::Range;

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowOptions,
    div, px, uniform_list,
};
use gpui_component::Root;
use rlogg_core::LogFile;

struct LogView {
    log: Option<LogFile>,
    status: String,
}

impl LogView {
    fn new(path: Option<String>) -> Self {
        match path.map(|p| (LogFile::open(&p), p)) {
            None => Self {
                log: None,
                status: "usage: rlogg <file>".into(),
            },
            Some((Ok(log), p)) => Self {
                status: format!("{p} — {} lines", log.line_count()),
                log: Some(log),
            },
            Some((Err(e), _)) => Self {
                log: None,
                status: e.to_string(),
            },
        }
    }

    fn render_rows(&mut self, range: Range<usize>) -> Vec<gpui::Div> {
        let Some(log) = &self.log else {
            return Vec::new();
        };
        range
            .map(|n| {
                let text = String::from_utf8_lossy(log.line(n).unwrap_or_default()).into_owned();
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(70.)).child(format!("{}", n + 1)))
                    .child(text)
            })
            .collect()
    }
}

impl Render for LogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.log.as_ref().map_or(0, LogFile::line_count);
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family("monospace")
            .text_sm()
            .child(
                uniform_list(
                    "log-lines",
                    count,
                    cx.processor(|this, range: Range<usize>, _window, _cx| this.render_rows(range)),
                )
                .flex_1(),
            )
            .child(div().px_2().py_1().child(self.status.clone()))
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let path = std::env::args().nth(1);

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.open_window(WindowOptions::default(), |window, cx| {
            let view = cx.new(|_| LogView::new(path));
            cx.new(|cx| Root::new(view, window, cx))
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
