//! Main window content: menu bar, log view and status line (klogg:
//! `MainWindow`).
//!
//! Files are opened from the command line, File > Open (Ctrl+O), File > Open
//! Recent, or by dropping them on the window. Recent files are kept in the
//! session file (see [`rlogg_core::settings`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui::{
    Action, AppContext, Context, Entity, ExternalPaths, Focusable, InteractiveElement, IntoElement,
    KeyBinding, Menu, MenuItem, ParentElement, PathPromptOptions, PromptLevel, Render, Styled,
    Task, Window, actions, div, px,
};
use gpui_component::menu::AppMenuBar;
use gpui_component::{ActiveTheme, GlobalState};
use rlogg_core::file::OpenError;
use rlogg_core::settings::Session;
use rlogg_core::{IndexProgress, LogFile, OpenOptions};

use crate::background;
use crate::crawler::Crawler;

actions!(workspace, [Open, Close, Quit, ClearRecent]);

/// Opens a file from File > Open Recent.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

/// klogg's defaults: `QKeySequence::Open`, `Close`, `Quit`.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("secondary-o", Open, None),
        KeyBinding::new("secondary-w", Close, None),
        KeyBinding::new("secondary-q", Quit, None),
    ]
}

const IDLE_STATUS: &str = "Open a file with Ctrl+O, or drop it here";

/// Messages from the indexing thread.
enum IndexEvent {
    Progress(IndexProgress),
    Done(Result<Arc<LogFile>, OpenError>),
}

pub struct Workspace {
    crawler: Entity<Crawler>,
    /// In-window menu bar; `None` on macOS, which uses the native one.
    menu_bar: Option<Entity<AppMenuBar>>,
    status: String,
    /// Where the session is saved; `None` if there is no config directory.
    config_dir: Option<PathBuf>,
    session: Session,
    /// Stops the current indexing thread.
    cancel: Arc<AtomicBool>,
    _indexing: Option<Task<()>>,
}

impl Workspace {
    pub fn new(config_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = config_dir.as_deref().map(Session::load).unwrap_or_default();
        let crawler = cx.new(|cx| Crawler::new(window, cx));
        let focus = crawler.focus_handle(cx);
        window.focus(&focus, cx);
        let mut this = Self {
            crawler,
            menu_bar: (!cfg!(target_os = "macos")).then(|| AppMenuBar::new(cx)),
            status: IDLE_STATUS.into(),
            config_dir,
            session,
            cancel: Arc::default(),
            _indexing: None,
        };
        this.update_menus(cx);
        this
    }

    fn menus(&self) -> Vec<Menu> {
        let recent = self.session.recent_files.shown();
        let mut recent_items: Vec<MenuItem> = recent
            .iter()
            .enumerate()
            .map(|(i, path)| {
                MenuItem::action(recent_label(i, path), OpenRecent { path: path.clone() })
            })
            .collect();
        recent_items.push(MenuItem::separator());
        recent_items.push(MenuItem::action("Clear List", ClearRecent));
        vec![
            Menu::new("File").items([
                MenuItem::action("Open…", Open),
                MenuItem::submenu(
                    Menu::new("Open Recent")
                        .items(recent_items)
                        .disabled(recent.is_empty()),
                ),
                MenuItem::separator(),
                MenuItem::action("Close", Close),
                MenuItem::action("Quit", Quit),
            ]),
        ]
    }

    /// Publishes the menus: native on macOS, the in-window bar elsewhere.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        cx.set_menus(self.menus());
        let owned = self.menus().into_iter().map(Menu::owned).collect();
        GlobalState::global_mut(cx).set_app_menus(owned);
        if let Some(bar) = &self.menu_bar {
            bar.update(cx, |bar, cx| bar.reload(cx));
        }
    }

    fn save_session(&self) {
        if let Some(dir) = &self.config_dir
            && let Err(e) = self.session.save(dir)
        {
            tracing::warn!("{e}");
        }
    }

    /// Opens `path`: maps and indexes it on a dedicated thread (indexing
    /// itself runs on rayon), streaming progress to the status line. Replaces
    /// the current file (klogg opens a tab; tabs come later).
    pub fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_indexing();
        self.crawler.update(cx, |c, cx| c.set_log(None, cx));
        let shown = path.display().to_string();
        self.status = format!("{shown} — indexing…");
        window.set_window_title(&format!("{} — rlogg", file_name(&path)));

        let (tx, rx) = async_channel::unbounded();
        let cancel = self.cancel.clone();
        let thread_path = path.clone();
        background::spawn_blocking(
            "rlogg-index",
            move || {
                let result =
                    LogFile::open_with(&thread_path, OpenOptions::default(), &cancel, |p| {
                        tx.send_blocking(IndexEvent::Progress(p)).ok();
                    });
                tx.send_blocking(IndexEvent::Done(result.map(Arc::new)))
                    .ok();
            },
            cx,
        );

        let started = Instant::now();
        self._indexing = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    match event {
                        IndexEvent::Progress(p) => {
                            this.status = format!("{shown} — indexing {}%", p.percent());
                        }
                        IndexEvent::Done(Ok(log)) => {
                            tracing::info!(
                                lines = log.line_count(),
                                index_bytes = log.index().allocated_bytes(),
                                elapsed = ?started.elapsed(),
                                "indexed {shown}"
                            );
                            this.status = format!(
                                "{shown} — {} lines — {}",
                                log.line_count(),
                                log.encoding()
                            );
                            this.crawler.update(cx, |c, cx| c.set_log(Some(log), cx));
                            this.add_recent(&path, cx);
                        }
                        IndexEvent::Done(Err(OpenError::Cancelled(_))) => {}
                        IndexEvent::Done(Err(e)) => {
                            this.status = e.to_string();
                            window.set_window_title("rlogg");
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn add_recent(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.session.recent_files.add(path);
        self.save_session();
        self.update_menus(cx);
    }

    fn stop_indexing(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self._indexing = None;
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_indexing();
        self.crawler.update(cx, |c, cx| c.set_log(None, cx));
        self.status = IDLE_STATUS.into();
        window.set_window_title("rlogg");
        cx.notify();
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update_in(cx, |this, window, cx| this.open(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Opens a recent file, offering to forget it if it is gone (klogg:
    /// `openFileFromRecent`).
    fn open_recent(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if std::fs::File::open(&path).is_ok() {
            self.open(path, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!(
                "Could not read file {}. Remove it from recent files?",
                path.display()
            ),
            None,
            &["Yes", "No"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    this.session.recent_files.remove(&path);
                    this.save_session();
                    this.update_menus(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn clear_recent(&mut self, cx: &mut Context<Self>) {
        self.session.recent_files.clear();
        self.save_session();
        self.update_menus(cx);
    }

    /// Files dropped on the window (klogg: `dropEvent`). Until tabs exist,
    /// only the first one is opened.
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = paths.paths().iter().find(|p| p.is_file()) {
            self.open(path.clone(), window, cx);
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, drop_bg) = (theme.border, theme.drop_target);
        div()
            .id("workspace")
            .key_context("Workspace")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Open, window, cx| this.prompt_open(window, cx)))
            .on_action(cx.listener(|this, a: &OpenRecent, window, cx| {
                this.open_recent(a.path.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearRecent, _, cx| this.clear_recent(cx)))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close(window, cx)))
            .on_drop(cx.listener(Self::drop_paths))
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_bg))
            .children(self.menu_bar.clone().map(|bar| {
                div()
                    .h(px(32.))
                    .px_1()
                    .border_b_1()
                    .border_color(border)
                    .child(bar)
            }))
            .child(div().flex_1().min_h_0().child(self.crawler.clone()))
            .child(div().px_2().py_1().text_sm().child(self.status.clone()))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// "1  app.log — /var/log" (klogg numbers recent files 1..9, then a, b, …).
fn recent_label(index: usize, path: &Path) -> String {
    let key = match index {
        0..=8 => char::from(b'1' + index as u8),
        _ => char::from(b'a' + (index - 9) as u8),
    };
    let dir = path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    format!("{key}  {} — {dir}", file_name(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FileDropEvent, TestAppContext, VisualTestContext, point};

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rlogg-ws-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn log(&self, name: &str, lines: usize) -> PathBuf {
            let path = self.dir.join(name);
            let text: String = (0..lines).map(|i| format!("{name} {i}\n")).collect();
            std::fs::write(&path, text).unwrap();
            path.canonicalize().unwrap()
        }

        fn config(&self) -> PathBuf {
            self.dir.join("config")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn workspace(
        cx: &mut TestAppContext,
        config: PathBuf,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(crate::log_view::key_bindings());
            cx.bind_keys(crate::crawler::key_bindings());
            cx.bind_keys(key_bindings());
        });
        cx.add_window_view(|window, cx| Workspace::new(Some(config), window, cx))
    }

    /// Indexing runs on a real thread: let it finish, then run the UI tasks.
    fn wait_indexed(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
        for _ in 0..500 {
            cx.run_until_parked();
            let lines = ws.read_with(cx, |ws, cx| {
                ws.crawler.read(cx).log().map(LogFile::line_count)
            });
            if let Some(lines) = lines {
                return lines;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("indexing did not finish");
    }

    fn recent(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<PathBuf> {
        ws.read_with(cx, |ws, _| ws.session.recent_files.files().to_vec())
    }

    fn menu_labels(cx: &mut VisualTestContext) -> Vec<String> {
        fn walk(items: &[gpui::OwnedMenuItem], out: &mut Vec<String>) {
            for item in items {
                match item {
                    gpui::OwnedMenuItem::Action { name, .. } => out.push(name.clone()),
                    gpui::OwnedMenuItem::Submenu(menu) => walk(&menu.items, out),
                    _ => {}
                }
            }
        }
        cx.update(|_, cx| {
            let mut out = Vec::new();
            for menu in GlobalState::global(cx).app_menus() {
                walk(&menu.items, &mut out);
            }
            out
        })
    }

    #[gpui::test]
    fn open_dialog_indexes_and_remembers(cx: &mut TestAppContext) {
        let fx = Fixture::new("dialog");
        let a = fx.log("a.log", 10);
        let (ws, cx) = workspace(cx, fx.config());
        assert!(menu_labels(cx).contains(&"Open…".to_string()));

        cx.simulate_keystrokes("ctrl-o");
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| Some(vec![a.clone()]));
        assert_eq!(wait_indexed(&ws, cx), 10);
        assert_eq!(cx.window_title().as_deref(), Some("a.log — rlogg"));
        assert!(ws.read_with(cx, |ws, _| ws.status.contains("10 lines")));

        assert_eq!(recent(&ws, cx), std::slice::from_ref(&a));
        assert_eq!(
            Session::load(&fx.config()).recent_files.files(),
            std::slice::from_ref(&a)
        );
        let labels = menu_labels(cx);
        assert!(
            labels.iter().any(|l| l.starts_with("1  a.log — ")),
            "{labels:?}"
        );

        // A new workspace starts with the saved recent files.
        let config = fx.config();
        let ws2 = cx.update(|window, cx| cx.new(|cx| Workspace::new(Some(config), window, cx)));
        assert_eq!(recent(&ws2, cx), [a]);
    }

    #[gpui::test]
    fn open_recent_and_forget_missing(cx: &mut TestAppContext) {
        let fx = Fixture::new("recent");
        let (a, b) = (fx.log("a.log", 3), fx.log("b.log", 4));
        let (ws, cx) = workspace(cx, fx.config());
        for path in [&a, &b] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        assert_eq!(recent(&ws, cx), [b.clone(), a.clone()]);

        cx.dispatch_action(OpenRecent { path: a.clone() });
        assert_eq!(wait_indexed(&ws, cx), 3);
        assert_eq!(recent(&ws, cx), [a.clone(), b.clone()]);

        // A recent file that disappeared: klogg asks before forgetting it.
        std::fs::remove_file(&b).unwrap();
        cx.dispatch_action(OpenRecent { path: b.clone() });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Yes");
        cx.run_until_parked();
        assert_eq!(recent(&ws, cx), std::slice::from_ref(&a));
        assert_eq!(Session::load(&fx.config()).recent_files.files(), [a]);

        cx.dispatch_action(ClearRecent);
        assert!(recent(&ws, cx).is_empty());
        assert!(!menu_labels(cx).iter().any(|l| l.contains("a.log")));
    }

    #[gpui::test]
    fn drop_opens_file_and_close_clears(cx: &mut TestAppContext) {
        let fx = Fixture::new("drop");
        let a = fx.log("dropped.log", 7);
        let (ws, cx) = workspace(cx, fx.config());
        let at = point(gpui::px(200.), gpui::px(200.));
        cx.simulate_event(FileDropEvent::Entered {
            position: at,
            paths: ExternalPaths(vec![a.clone()].into()),
        });
        cx.simulate_event(FileDropEvent::Submit { position: at });
        assert_eq!(wait_indexed(&ws, cx), 7);
        assert_eq!(recent(&ws, cx), [a]);

        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();
        assert!(ws.read_with(cx, |ws, cx| ws.crawler.read(cx).log().is_none()));
        assert_eq!(ws.read_with(cx, |ws, _| ws.status.clone()), IDLE_STATUS);
        assert_eq!(cx.window_title().as_deref(), Some("rlogg"));
    }
}
