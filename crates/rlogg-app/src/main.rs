//! rlogg desktop app: opens the file given on the command line in a log view.
//! See CLAUDE.md for the roadmap.

mod log_view;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui::{
    App, AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Render, Styled, Task,
    Window, WindowOptions, div,
};
use gpui_component::Root;
use rlogg_core::file::OpenError;
use rlogg_core::{IndexProgress, LogFile, OpenOptions};

/// Messages from the indexing thread to the view.
enum IndexEvent {
    Progress(IndexProgress),
    Done(Result<LogFile, OpenError>),
}

use log_view::LogView;

/// Main window content: the log view and a status line.
struct Workspace {
    view: Entity<LogView>,
    status: String,
    /// Stops the indexing thread when the view goes away.
    cancel: Arc<AtomicBool>,
    _indexing: Option<Task<()>>,
}

impl Workspace {
    fn new(path: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            view: cx.new(LogView::new),
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
        self.view.update(cx, |v, cx| v.set_log(None, cx));
        self.status = format!("{path} — indexing…");

        let (tx, rx) = async_channel::unbounded();
        let cancel = self.cancel.clone();
        let thread_path = path.clone();
        std::thread::Builder::new()
            .name("rlogg-index".into())
            .spawn(move || {
                let result =
                    LogFile::open_with(&thread_path, OpenOptions::default(), &cancel, |p| {
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
                            view.status =
                                format!("{path} — {} lines — {}", log.line_count(), log.encoding());
                            view.view.update(cx, |v, cx| v.set_log(Some(log), cx));
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
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(self.view.clone()))
            .child(div().px_2().py_1().text_sm().child(self.status.clone()))
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let path = std::env::args().nth(1);

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.bind_keys(log_view::key_bindings());
        cx.open_window(WindowOptions::default(), |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(path, cx));
            let focus = workspace.read(cx).view.focus_handle(cx);
            window.focus(&focus, cx);
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
