//! rlogg desktop app. Scaffold: opens the file given on the command line and
//! shows it in a virtualized list. See CLAUDE.md for the roadmap.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, Styled, Task, Window,
    WindowOptions, div, px, uniform_list,
};
use gpui_component::Root;
use rlogg_core::file::OpenError;
use rlogg_core::{IndexProgress, LogFile};

/// Messages from the indexing thread to the view.
enum IndexEvent {
    Progress(IndexProgress),
    Done(Result<LogFile, OpenError>),
}

struct LogView {
    log: Option<LogFile>,
    status: String,
    /// Stops the indexing thread when the view goes away.
    cancel: Arc<AtomicBool>,
    _indexing: Option<Task<()>>,
}

impl LogView {
    fn new(path: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            log: None,
            status: "usage: rlogg <file>".into(),
            cancel: Arc::default(),
            _indexing: None,
        };
        if let Some(path) = path {
            view.open(path, cx);
        }
        view
    }

    /// Maps and indexes `path` on a dedicated thread (indexing itself runs on
    /// rayon), streaming progress back to the UI.
    fn open(&mut self, path: String, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self.log = None;
        self.status = format!("{path} — indexing…");

        let (tx, rx) = async_channel::unbounded();
        let cancel = self.cancel.clone();
        let thread_path = path.clone();
        std::thread::Builder::new()
            .name("rlogg-index".into())
            .spawn(move || {
                let result = LogFile::open_with(&thread_path, &cancel, |p| {
                    tx.send_blocking(IndexEvent::Progress(p)).ok();
                });
                tx.send_blocking(IndexEvent::Done(result)).ok();
            })
            .expect("failed to spawn indexing thread");

        let started = Instant::now();
        self._indexing = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update(cx, |view, cx| {
                    match event {
                        IndexEvent::Progress(p) => {
                            view.status = format!("{path} — indexing {}%", p.percent());
                        }
                        IndexEvent::Done(Ok(log)) => {
                            tracing::info!(
                                lines = log.line_count(),
                                index_bytes = log.index().allocated_bytes(),
                                elapsed = ?started.elapsed(),
                                "indexed {path}"
                            );
                            view.status = format!("{path} — {} lines", log.line_count());
                            view.log = Some(log);
                        }
                        IndexEvent::Done(Err(OpenError::Cancelled(_))) => {}
                        IndexEvent::Done(Err(e)) => view.status = e.to_string(),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    fn render_rows(&mut self, range: Range<usize>) -> Vec<gpui::Div> {
        let Some(log) = &self.log else {
            return Vec::new();
        };
        let first = range.start;
        log.lines(range)
            .enumerate()
            .map(|(i, line)| {
                // TODO: decoding and long-line windowing (Phase 1 log view).
                let text = String::from_utf8_lossy(line).into_owned();
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(70.)).child(format!("{}", first + i + 1)))
                    .child(text)
            })
            .collect()
    }
}

impl Drop for LogView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
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
            let view = cx.new(|cx| LogView::new(path, cx));
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
