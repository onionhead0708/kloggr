//! Main window content: menu bar, tabs of open files, and status line
//! (klogg: `MainWindow`, `TabbedCrawlerWidget`).
//!
//! Files are opened from the command line, File > Open (Ctrl+O), File > Open
//! Recent, or by dropping them on the window, each in its tab. Recent files
//! are kept in the session file (see [`kloggr_core::settings`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gpui::{
    Action, App, AppContext, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Menu, MenuItem, MouseButton, ParentElement, PathPromptOptions,
    PromptLevel, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{AppMenuBar, ContextMenuExt};
use gpui_component::tab::{Tab as GuiTab, TabBar};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, GlobalState, Sizable, WindowExt};
use kloggr_core::archive;
use kloggr_core::file::OpenError;
use kloggr_core::settings::{FileContext, OpenFile, Session};
use kloggr_core::{IndexProgress, LogFile, OpenOptions};

use crate::archives;
use crate::background;
use crate::crawler::{
    Crawler, CrawlerEvent, EditPredefinedFilters, ToggleFreezeFiltered, ToggleFreezeMain,
};
use crate::highlighters::{EditHighlighters, Highlighters, NoHighlighters, ToggleHighlighterSet};
use crate::highlighters_dialog;
use crate::options_dialog;
use crate::preferences::Preferences;
use crate::shortcuts::Shortcut;

actions!(
    workspace,
    [
        Open,
        Close,
        Quit,
        ClearRecent,
        NextTab,
        PrevTab,
        LastTab,
        CloseAllTabs,
        ShowOptions,
        OpenScratchpad,
        ToggleOverview,
        ToggleLineNumbersMain,
        ToggleLineNumbersFiltered
    ]
);

/// Opens a file from File > Open Recent.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

/// Tab actions, from the keyboard (Ctrl+1…8) and the tab context menu.
macro_rules! tab_action {
    ($($name:ident),*) => {$(
        #[derive(Clone, PartialEq, Debug, Action)]
        #[action(namespace = workspace, no_json)]
        pub struct $name {
            pub index: usize,
        }
    )*};
}
tab_action!(
    ActivateTab,
    CloseTab,
    CloseOtherTabs,
    CloseTabsLeft,
    CloseTabsRight,
    CopyTabPath,
    OpenTabFolder,
    RenameTab,
    ResetTabName
);

/// klogg's defaults: `QKeySequence::Open`, `Close`, `Quit`, and the tab keys
/// of `TabbedCrawlerWidget::keyPressEvent`.
pub fn shortcuts() -> Vec<Shortcut> {
    let mut out = vec![
        Shortcut::new(
            "mainwindow.open_file",
            "Open file",
            None,
            &["secondary-o"],
            Open,
        ),
        Shortcut::new(
            "mainwindow.close_file",
            "Close file",
            None,
            &["secondary-w"],
            Close,
        ),
        Shortcut::new(
            "mainwindow.close_all",
            "Close all files",
            None,
            &[],
            CloseAllTabs,
        ),
        Shortcut::new(
            "mainwindow.quit",
            "Exit application",
            None,
            &["secondary-q"],
            Quit,
        ),
        Shortcut::new(
            "mainwindow.preference",
            "Preferences",
            None,
            &[],
            ShowOptions,
        ),
        Shortcut::new(
            "mainwindow.scratchpad",
            "Open scratchpad",
            None,
            &[],
            OpenScratchpad,
        ),
        Shortcut::new(
            "mainwindow.next_tab",
            "Next tab",
            None,
            &["ctrl-tab", "ctrl-pagedown"],
            NextTab,
        ),
        Shortcut::new(
            "mainwindow.prev_tab",
            "Previous tab",
            None,
            &["ctrl-shift-tab", "ctrl-pageup"],
            PrevTab,
        ),
        Shortcut::new(
            "mainwindow.last_tab",
            "Last tab",
            None,
            &["secondary-9"],
            LastTab,
        ),
    ];
    const TABS: [(&str, &str); 8] = [
        ("mainwindow.tab_1", "Tab 1"),
        ("mainwindow.tab_2", "Tab 2"),
        ("mainwindow.tab_3", "Tab 3"),
        ("mainwindow.tab_4", "Tab 4"),
        ("mainwindow.tab_5", "Tab 5"),
        ("mainwindow.tab_6", "Tab 6"),
        ("mainwindow.tab_7", "Tab 7"),
        ("mainwindow.tab_8", "Tab 8"),
    ];
    for (index, (id, label)) in TABS.into_iter().enumerate() {
        out.push(Shortcut {
            id,
            label,
            context: None,
            keys: vec![format!("secondary-{}", index + 1)],
            action: Box::new(ActivateTab { index }),
        });
    }
    out
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

const IDLE_STATUS: &str = "Open a file with Ctrl+O, or drop it here";

/// Messages from the indexing thread.
enum ExtractEvent {
    Progress(u8),
    Done(Result<Vec<PathBuf>, String>),
}

enum IndexEvent {
    Progress(IndexProgress),
    Done(Result<Arc<LogFile>, OpenError>),
}

/// One open file (klogg: a `CrawlerWidget` in `TabbedCrawlerWidget`).
struct Tab {
    path: PathBuf,
    crawler: Entity<Crawler>,
    status: String,
    /// The file changed while the tab was in the background (klogg: the
    /// "new data" tab icon).
    new_data: bool,
    /// Stops the tab's indexing thread.
    cancel: Arc<AtomicBool>,
    _indexing: Option<Task<()>>,
    _events: Subscription,
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    /// In-window menu bar; `None` on macOS, which uses the native one.
    menu_bar: Option<Entity<AppMenuBar>>,
    /// Where the session is saved; `None` if there is no config directory.
    config_dir: Option<PathBuf>,
    session: Session,
    tab_scroll: ScrollHandle,
    /// Focused when no tab is open, so that the window's keys still work.
    focus: FocusHandle,
    /// Why the last file could not be opened, when no tab is left to show it.
    error: Option<String>,
    /// Enter in the rename dialog's input.
    _rename: Option<gpui::Subscription>,
    /// "Extracting app.log.gz… 40%", while an archive is opened.
    extracting: Option<String>,
    extract_cancel: Arc<AtomicBool>,
    _extract: Option<Task<()>>,
    /// The open archive file picker (see `archives::pick`).
    _picker: Option<Entity<archives::ArchivePicker>>,
}

impl Workspace {
    pub fn new(config_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut session = config_dir.as_deref().map(Session::load).unwrap_or_default();
        session
            .recent_files
            .set_shown(Preferences::get(cx).file.recent_files);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Marks and options change without a tab change: save them on exit.
        cx.on_app_quit(|this, cx| {
            this.save_open_files(cx);
            this.extract_cancel.store(true, Ordering::Relaxed);
            let _ = std::fs::remove_dir_all(Self::temp_root());
            async {}
        })
        .detach();
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            menu_bar: (!cfg!(target_os = "macos")).then(|| AppMenuBar::new(cx)),
            config_dir,
            session,
            tab_scroll: ScrollHandle::new(),
            focus,
            error: None,
            _rename: None,
            extracting: None,
            extract_cancel: Arc::default(),
            _extract: None,
            _picker: None,
        };
        this.update_menus(cx);
        // Set names and active sets show in the menus.
        cx.observe_global::<Highlighters>(|this, cx| this.update_menus(cx))
            .detach();
        cx.observe_global::<Preferences>(|this, cx| {
            let shown = Preferences::get(cx).file.recent_files;
            this.session.recent_files.set_shown(shown);
            this.update_menus(cx);
        })
        .detach();
        this
    }

    /// The crawler of the current tab.
    pub fn crawler(&self) -> Option<&Entity<Crawler>> {
        self.tabs.get(self.active).map(|t| &t.crawler)
    }

    fn menus(&self, cx: &App) -> Vec<Menu> {
        let view = &Preferences::get(cx).view;
        let no_tab = self.crawler().is_none();
        let frozen = self
            .crawler()
            .map(|c| c.read(cx).frozen_columns(cx))
            .unwrap_or_default();
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
                MenuItem::action("Close All", CloseAllTabs),
                MenuItem::action("Quit", Quit),
            ]),
            // klogg: View > Freeze columns in main / filtered view.
            Menu::new("View").items([
                MenuItem::action("Overview", ToggleOverview).checked(view.overview),
                MenuItem::action("Line Numbers in Main View", ToggleLineNumbersMain)
                    .checked(view.line_numbers_main),
                MenuItem::action("Line Numbers in Filtered View", ToggleLineNumbersFiltered)
                    .checked(view.line_numbers_filtered),
                MenuItem::separator(),
                MenuItem::action("Freeze Columns in Main View", ToggleFreezeMain)
                    .checked(frozen.0.enabled)
                    .disabled(no_tab),
                MenuItem::action("Freeze Columns in Filtered View", ToggleFreezeFiltered)
                    .checked(frozen.1.enabled)
                    .disabled(no_tab),
            ]),
            Menu::new("Tools").items([
                MenuItem::action("Options…", ShowOptions),
                MenuItem::action("Predefined Filters…", EditPredefinedFilters),
                MenuItem::action("Scratchpad", OpenScratchpad),
                MenuItem::separator(),
                MenuItem::action("Highlighters…", EditHighlighters),
                MenuItem::submenu(Highlighters::menu(cx)),
            ]),
        ]
    }

    /// Publishes the menus: native on macOS, the in-window bar elsewhere.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        cx.set_menus(self.menus(cx));
        let owned = self.menus(cx).into_iter().map(Menu::owned).collect();
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

    /// Opens `path` in a new tab, or shows its tab if it is open already
    /// (klogg: `loadFile`). The file is mapped and indexed on a dedicated
    /// thread (indexing itself runs on rayon), with progress in the status
    /// line.
    pub fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let options = Preferences::get(cx).file.clone();
        let kind = if options.extract_archives {
            archive::kind(&path).unwrap_or(archive::Kind::Plain)
        } else {
            archive::Kind::Plain
        };
        if kind.is_plain() {
            self.open_with(path, None, window, cx);
        } else if options.extract_without_confirmation {
            self.extract(path, kind, window, cx);
        } else {
            // klogg: "Extract archive to temp folder?"; No opens nothing.
            // Deferred: a file given on the command line is opened while
            // the window (and its dialog layer) is still being built.
            let this = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let (this, path) = (this.clone(), path.clone());
                    alert
                        .title("Open archive")
                        .description(format!(
                            "Extract {} to a temporary folder?",
                            file_name(&path)
                        ))
                        .show_cancel(true)
                        .ok_text("Extract")
                        .on_ok(move |_, window, cx| {
                            let path = path.clone();
                            this.update(cx, |ws, cx| ws.extract(path, kind, window, cx))
                                .ok();
                            true
                        })
                });
            });
        }
    }

    /// Where files taken out of archives go; removed when kloggr quits
    /// (klogg: `tempDir_`).
    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("kloggr-{}", std::process::id()))
    }

    /// Files taken out of an archive aren't remembered: they are gone
    /// after quitting.
    /// Paths are compared resolved too: opened files are canonical, and the
    /// temporary directory may not be (`/var` is `/private/var` on macOS,
    /// Windows has short names and the `\\?\` prefix).
    fn is_temporary(path: &Path) -> bool {
        let root = Self::temp_root();
        let resolved = |p: &Path| std::fs::canonicalize(p).ok();
        path.starts_with(&root)
            || resolved(&root).is_some_and(|root| {
                path.starts_with(&root) || resolved(path).is_some_and(|path| path.starts_with(root))
            })
    }

    /// Decompresses or extracts `path` on a thread, with progress in the
    /// status line, then opens the result (klogg: `extractAndLoadFile`).
    pub fn extract(
        &mut self,
        path: PathBuf,
        kind: archive::Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let dir = archive::temp_dir_for(&Self::temp_root(), &path);
        let (tx, rx) = async_channel::unbounded();
        let cancel = self.extract_cancel.clone();
        let (thread_path, thread_dir) = (path.clone(), dir.clone());
        background::spawn_blocking(
            "kloggr-extract",
            move || {
                let mut progress = |p: u8| {
                    tx.send_blocking(ExtractEvent::Progress(p)).ok();
                };
                let result = match kind {
                    archive::Kind::Compressed(c) => {
                        archive::decompress(&thread_path, c, &thread_dir, &cancel, &mut progress)
                            .map(|f| vec![f])
                    }
                    _ => archive::extract(&thread_path, kind, &thread_dir, &cancel, &mut progress),
                };
                tx.send_blocking(ExtractEvent::Done(result.map_err(|e| e.to_string())))
                    .ok();
            },
            cx,
        );
        let name = file_name(&path);
        self.extracting = Some(format!("Extracting {name}…"));
        self.error = None;
        cx.notify();
        self._extract = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    match event {
                        ExtractEvent::Progress(p) => {
                            this.extracting = Some(format!("Extracting {name}… {p}%"));
                        }
                        ExtractEvent::Done(result) => {
                            this.extracting = None;
                            this.extracted(&path, kind, &dir, result, window, cx);
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    fn extracted(
        &mut self,
        archive_path: &Path,
        kind: archive::Kind,
        dir: &Path,
        result: Result<Vec<PathBuf>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = match result {
            Ok(files) => files,
            Err(e) => {
                // klogg: "Failed to extract %1".
                self.error = Some(format!("Failed to extract {}: {e}", archive_path.display()));
                return;
            }
        };
        self.add_recent(archive_path, cx);
        match files.as_slice() {
            [] => self.error = Some(format!("{} is empty", archive_path.display())),
            [file] => self.open_with(file.clone(), None, window, cx),
            _ if !kind.is_archive() => self.open_with(files[0].clone(), None, window, cx),
            _ => {
                let this = cx.entity().downgrade();
                let picker = archives::pick(
                    dir.to_path_buf(),
                    files,
                    window,
                    cx,
                    move |chosen, window, cx| {
                        this.update(cx, |ws, cx| {
                            for file in chosen {
                                ws.open_with(file, None, window, cx);
                            }
                        })
                        .ok();
                    },
                );
                self._picker = Some(picker);
            }
        }
    }

    /// Like [`open`](Self::open), restoring the file's state from a session.
    fn open_with(
        &mut self,
        path: PathBuf,
        context: Option<FileContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if let Some(index) = self.tabs.iter().position(|t| t.path == path) {
            self.activate(index, window, cx);
            return;
        }
        let (main_frozen, filtered_frozen) = self.session.frozen_columns(&path);
        let crawler = cx.new(|cx| {
            let mut crawler = Crawler::new(window, cx);
            // klogg: a new file gets the default search options
            // (`defaultView.*`), a session's file its own.
            let context = context.unwrap_or_else(|| Preferences::get(cx).new_file_context());
            crawler.set_context(context, cx);
            crawler.set_frozen_columns(main_frozen, filtered_frozen, cx);
            crawler
        });
        let events = cx.subscribe(&crawler, |this, crawler, event: &CrawlerEvent, cx| {
            if let CrawlerEvent::FrozenChanged = event {
                // klogg: `saveFrozenColumns`.
                if let Some(tab) = this.tabs.iter().find(|t| t.crawler == crawler) {
                    let (main, filtered) = crawler.read(cx).frozen_columns(cx);
                    this.session.set_frozen_columns(&tab.path, main, filtered);
                    this.save_session();
                }
                this.update_menus(cx);
                return;
            }
            if let Some(tab) = this.tabs.iter_mut().find(|t| t.crawler == crawler) {
                tab.status = status_of(&tab.path, crawler.read(cx).log.as_deref());
            }
            let active = this.crawler().cloned();
            if let Some(tab) = this.tabs.iter_mut().find(|t| t.crawler == crawler)
                && active.as_ref() != Some(&crawler)
            {
                tab.new_data = true;
            }
            cx.notify();
        });
        let shown = path.display().to_string();
        let (tx, rx) = async_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let thread_path = path.clone();
        background::spawn_blocking(
            "kloggr-index",
            move || {
                let result =
                    LogFile::open_with(&thread_path, OpenOptions::default(), &thread_cancel, |p| {
                        tx.send_blocking(IndexEvent::Progress(p)).ok();
                    });
                tx.send_blocking(IndexEvent::Done(result.map(Arc::new)))
                    .ok();
            },
            cx,
        );

        let started = Instant::now();
        let tab_crawler = crawler.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    let Some(index) = this.tabs.iter().position(|t| t.crawler == tab_crawler)
                    else {
                        return;
                    };
                    match event {
                        IndexEvent::Progress(p) => {
                            this.tabs[index].status =
                                format!("{shown} — indexing {}%", p.percent());
                        }
                        IndexEvent::Done(Ok(log)) => {
                            tracing::info!(
                                lines = log.line_count(),
                                index_bytes = log.index().allocated_bytes(),
                                elapsed = ?started.elapsed(),
                                "indexed {shown}"
                            );
                            let tab = &mut this.tabs[index];
                            tab.status = status_of(&tab.path, Some(&log));
                            let path = tab.path.clone();
                            tab.crawler.update(cx, |c, cx| c.set_log(Some(log), cx));
                            this.add_recent(&path, cx);
                        }
                        IndexEvent::Done(Err(OpenError::Cancelled(_))) => {}
                        IndexEvent::Done(Err(e)) => {
                            // klogg: the tab of a file that can't be read is
                            // closed, with an error.
                            this.close_tab(index, window, cx);
                            this.error = Some(e.to_string());
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.tabs.push(Tab {
            status: status_of(&path, None),
            path,
            crawler,
            new_data: false,
            cancel,
            _indexing: Some(task),
            _events: events,
        });
        self.error = None;
        let index = self.tabs.len() - 1;
        self.activate(index, window, cx);
    }

    fn add_recent(&mut self, path: &Path, cx: &mut Context<Self>) {
        if Self::is_temporary(path) {
            return;
        }
        self.session.recent_files.add(path.to_path_buf());
        self.save_session();
        self.update_menus(cx);
    }

    /// Reopens the files of the last session, each with its state, and
    /// shows the tab that was current (klogg: `reloadSession`). Files that
    /// are gone are skipped.
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = std::mem::take(&mut self.session.open_files);
        let current = self.session.current_file;
        let mut current_tab = None;
        for (i, file) in files.into_iter().enumerate() {
            if !file.path.is_file() {
                continue;
            }
            self.open_with(file.path, Some(file.context), window, cx);
            if i <= current {
                current_tab = Some(self.tabs.len() - 1);
            }
        }
        if let Some(index) = current_tab {
            self.activate(index, window, cx);
        }
        self.save_open_files(cx);
    }

    /// Stores the open tabs and their state in the session file (klogg:
    /// `writeSettings`, which does it on exit; here also on every tab change,
    /// so a crash keeps the tabs).
    fn save_open_files(&mut self, cx: &App) {
        self.session.open_files = self
            .tabs
            .iter()
            .filter(|t| !Self::is_temporary(&t.path))
            .map(|t| OpenFile {
                path: t.path.clone(),
                context: t.crawler.read(cx).context(cx),
            })
            .collect();
        self.session.current_file = self.active;
        self.save_session();
    }

    /// Shows tab `index` (klogg: `currentTabChanged`).
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        self.active = index;
        self.error = None;
        tab.new_data = false;
        let focus = tab.crawler.focus_handle(cx);
        window.focus(&focus, cx);
        self.tab_scroll.scroll_to_item(index);
        self.update_title(window);
        self.save_open_files(cx);
        self.update_menus(cx);
        cx.notify();
    }

    fn update_title(&self, window: &mut Window) {
        match self.tabs.get(self.active) {
            Some(tab) => window.set_window_title(&format!("{} — kloggr", file_name(&tab.path))),
            None => window.set_window_title("kloggr"),
        }
    }

    /// Closes tab `index`; the next one, or else the previous one, becomes
    /// current (as in Qt's tab widget).
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
            window.focus(&self.focus, cx);
            self.save_open_files(cx);
            self.update_title(window);
            cx.notify();
            return;
        }
        let active = if index < self.active || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(active, window, cx);
    }

    /// Closes the tabs for which `close(index)` is true.
    fn close_tabs(
        &mut self,
        close: impl Fn(usize) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for index in (0..self.tabs.len()).rev() {
            if close(index) {
                self.close_tab(index, window, cx);
            }
        }
    }

    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        if n > 0 {
            let next = if forward {
                (self.active + 1) % n
            } else {
                (self.active + n - 1) % n
            };
            self.activate(next, window, cx);
        }
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                for path in paths {
                    this.open(path, window, cx);
                }
            })
            .ok();
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

    /// Files dropped on the window, each in its tab (klogg: `dropEvent`).
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths.paths().iter().filter(|p| p.is_file()) {
            self.open(path.clone(), window, cx);
        }
    }

    fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if let Some(extracting) = &self.extracting {
            return extracting.clone();
        }
        self.tabs
            .get(self.active)
            .map_or_else(|| IDLE_STATUS.to_owned(), |t| t.status.clone())
    }

    /// The tab's text: the name the user gave it, else the file name
    /// (klogg: `TabNameMapping`).
    fn tab_label(&self, tab: &Tab) -> String {
        self.session
            .tab_name(&tab.path)
            .map_or_else(|| file_name(&tab.path), str::to_owned)
    }

    pub fn rename_tab(&mut self, index: usize, name: &str, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(index) {
            self.session.set_tab_name(&tab.path, name);
            self.save_session();
            cx.notify();
        }
    }

    /// Asks for the tab's new name (klogg: "Rename tab", `QInputDialog`).
    fn prompt_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let current = self.tab_label(tab);
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(current, window, cx);
            s
        });
        let this = cx.entity().downgrade();
        // Enter confirms.
        self._rename = Some(cx.subscribe_in(
            &input,
            window,
            move |ws, input, e: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let name = input.read(cx).value().to_string();
                    ws.rename_tab(index, &name, cx);
                    window.close_dialog(cx);
                }
            },
        ));
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let ok = {
                let (this, input) = (this.clone(), input.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let name = input.read(cx).value().to_string();
                    this.update(cx, |ws, cx| ws.rename_tab(index, &name, cx))
                        .ok();
                    window.close_dialog(cx);
                }
            };
            dialog
                .title("Rename tab")
                .w(px(420.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child("Tab name (empty for the file name)"))
                        .child(Input::new(&input)),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rename-cancel")
                                .label("Cancel")
                                .small()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rename-ok")
                                .label("OK")
                                .small()
                                .primary()
                                .on_click(ok),
                        ),
                )
        });
    }

    /// The tab bar, shown with two tabs or more (klogg hides it for one).
    fn render_tabs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.tabs.len() < 2 {
            return None;
        }
        let accent = cx.theme().blue;
        let last = self.tabs.len() - 1;
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let label = div()
                .id(("tab-label", index))
                .flex()
                .items_center()
                .gap_1()
                .children(
                    tab.new_data
                        .then(|| div().size(px(6.)).rounded_full().bg(accent)),
                )
                .child(self.tab_label(tab))
                .tooltip({
                    let path: SharedString = tab.path.display().to_string().into();
                    move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                })
                .context_menu(move |menu, _, _| {
                    menu.menu("Close this", Box::new(CloseTab { index }))
                        .menu("Close others", Box::new(CloseOtherTabs { index }))
                        .menu_with_disabled(
                            "Close to the left",
                            Box::new(CloseTabsLeft { index }),
                            index == 0,
                        )
                        .menu_with_disabled(
                            "Close to the right",
                            Box::new(CloseTabsRight { index }),
                            index == last,
                        )
                        .menu("Close all", Box::new(CloseAllTabs))
                        .separator()
                        .menu("Copy full path", Box::new(CopyTabPath { index }))
                        .menu("Open containing folder", Box::new(OpenTabFolder { index }))
                        .separator()
                        .menu("Rename tab", Box::new(RenameTab { index }))
                        .menu("Reset tab name", Box::new(ResetTabName { index }))
                });
            GuiTab::new()
                .child(label)
                .suffix(
                    Button::new(("tab-close", index))
                        .label("×")
                        .xsmall()
                        .ghost()
                        .tooltip("Close (Ctrl+W)")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_tab(index, window, cx);
                        })),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)),
                )
        });
        Some(
            TabBar::new("tabs")
                .track_scroll(&self.tab_scroll)
                .menu(true)
                .selected_index(self.active)
                .children(tabs)
                .on_click(
                    cx.listener(|this, index: &usize, window, cx| {
                        this.activate(*index, window, cx)
                    }),
                ),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, drop_bg, muted) = (theme.border, theme.drop_target, theme.muted_foreground);
        let content = match self.crawler() {
            Some(crawler) => div().size_full().child(crawler.clone()),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .child(IDLE_STATUS),
        };
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Open, window, cx| this.prompt_open(window, cx)))
            .on_action(cx.listener(|this, a: &OpenRecent, window, cx| {
                this.open_recent(a.path.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearRecent, _, cx| this.clear_recent(cx)))
            // klogg: `HighlightersMenu::saveCurrentHighlighterFromAction`.
            .on_action(cx.listener(|this, a: &ToggleHighlighterSet, _, cx| {
                let id = a.id.clone();
                Highlighters::update(cx, |sets| {
                    if sets.is_active(&id) {
                        sets.deactivate(&id)
                    } else {
                        sets.activate(&id)
                    }
                });
                this.update_menus(cx);
            }))
            .on_action(
                cx.listener(|_, _: &ShowOptions, window, cx| options_dialog::open(window, cx)),
            )
            .on_action(
                cx.listener(|_, _: &OpenScratchpad, _, cx| {
                    crate::scratchpad::show(cx, |_, _, _| {})
                }),
            )
            .on_action(cx.listener(|_, _: &EditPredefinedFilters, window, cx| {
                crate::saved_searches::edit_filters(window, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleOverview, window, cx| {
                toggle_view_option(|v| &mut v.overview, window, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleLineNumbersMain, window, cx| {
                toggle_view_option(|v| &mut v.line_numbers_main, window, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleLineNumbersFiltered, window, cx| {
                toggle_view_option(|v| &mut v.line_numbers_filtered, window, cx)
            }))
            .on_action(cx.listener(|_, _: &EditHighlighters, window, cx| {
                highlighters_dialog::open(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NoHighlighters, _, cx| {
                Highlighters::update(cx, |sets| sets.deactivate_all());
                this.update_menus(cx);
            }))
            .on_action(cx.listener(|this, _: &Close, window, cx| {
                let index = this.active;
                this.close_tab(index, window, cx)
            }))
            .on_action(
                cx.listener(|this, a: &CloseTab, window, cx| this.close_tab(a.index, window, cx)),
            )
            .on_action(cx.listener(|this, a: &CloseOtherTabs, window, cx| {
                let keep = a.index;
                this.close_tabs(|i| i != keep, window, cx)
            }))
            .on_action(cx.listener(|this, a: &CloseTabsLeft, window, cx| {
                let keep = a.index;
                this.close_tabs(|i| i < keep, window, cx)
            }))
            .on_action(cx.listener(|this, a: &CloseTabsRight, window, cx| {
                let keep = a.index;
                this.close_tabs(|i| i > keep, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseAllTabs, window, cx| {
                this.close_tabs(|_| true, window, cx)
            }))
            .on_action(cx.listener(|this, a: &CopyTabPath, _, cx| {
                if let Some(tab) = this.tabs.get(a.index) {
                    let path = tab.path.display().to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(path));
                }
            }))
            .on_action(
                cx.listener(|this, a: &ActivateTab, window, cx| this.activate(a.index, window, cx)),
            )
            .on_action(cx.listener(|this, a: &RenameTab, window, cx| {
                this.prompt_rename(a.index, window, cx)
            }))
            .on_action(
                cx.listener(|this, a: &ResetTabName, _, cx| this.rename_tab(a.index, "", cx)),
            )
            .on_action(cx.listener(|this, a: &OpenTabFolder, _, cx| {
                if let Some(tab) = this.tabs.get(a.index) {
                    cx.reveal_path(&tab.path);
                }
            }))
            .on_action(cx.listener(|this, _: &LastTab, window, cx| {
                let last = this.tabs.len().saturating_sub(1);
                this.activate(last, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PrevTab, window, cx| this.cycle_tab(false, window, cx)),
            )
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
            .children(self.render_tabs(cx))
            .child(div().flex_1().min_h_0().child(content))
            .child(div().px_2().py_1().text_sm().child(self.status()))
    }
}

/// Flips a View menu option (klogg: View > Overview, Line numbers…).
fn toggle_view_option(
    field: fn(&mut kloggr_core::options::View) -> &mut bool,
    window: &mut Window,
    cx: &mut App,
) {
    let mut options = Preferences::get(cx).clone();
    let value = field(&mut options.view);
    *value = !*value;
    Preferences::set(options, Some(window), cx);
}

/// "path — N lines — encoding".
fn status_of(path: &Path, log: Option<&LogFile>) -> String {
    let shown = path.display();
    match log {
        Some(log) => format!("{shown} — {} lines — {}", log.line_count(), log.encoding()),
        None => format!("{shown} — indexing…"),
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
            let dir = std::env::temp_dir().join(format!("kloggr-ws-{}-{name}", std::process::id()));
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
                ws.crawler()
                    .and_then(|c| c.read(cx).log().map(LogFile::line_count))
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

        cx.simulate_keystrokes("secondary-o");
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| Some(vec![a.clone()]));
        assert_eq!(wait_indexed(&ws, cx), 10);
        assert_eq!(cx.window_title().as_deref(), Some("a.log — kloggr"));
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("10 lines")));

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

        // Already open: its tab is shown; like klogg, the list stays as is.
        cx.dispatch_action(OpenRecent { path: a.clone() });
        assert_eq!(wait_indexed(&ws, cx), 3);
        assert_eq!(recent(&ws, cx), [b.clone(), a.clone()]);

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

        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        assert!(ws.read_with(cx, |ws, _| ws.crawler().is_none()));
        assert_eq!(ws.read_with(cx, |ws, _| ws.status()), IDLE_STATUS);
        assert_eq!(cx.window_title().as_deref(), Some("kloggr"));
    }

    fn tab_names(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> (Vec<String>, usize) {
        ws.read_with(cx, |ws, _| {
            (
                ws.tabs.iter().map(|t| file_name(&t.path)).collect(),
                ws.active,
            )
        })
    }

    #[gpui::test]
    fn tabs(cx: &mut TestAppContext) {
        let fx = Fixture::new("tabs");
        let paths: Vec<PathBuf> = ["a.log", "b.log", "c.log", "d.log"]
            .iter()
            .enumerate()
            .map(|(i, n)| fx.log(n, i + 1))
            .collect();
        let (ws, cx) = workspace(cx, fx.config());
        for path in &paths {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            tab_names(&ws, cx),
            (names(&["a.log", "b.log", "c.log", "d.log"]), 3)
        );
        assert_eq!(cx.window_title().as_deref(), Some("d.log — kloggr"));

        // Opening an open file shows its tab.
        ws.update_in(cx, |ws, window, cx| ws.open(paths[1].clone(), window, cx));
        assert_eq!(tab_names(&ws, cx).1, 1);
        assert_eq!(wait_indexed(&ws, cx), 2);

        // klogg's keys: Ctrl+Tab / Ctrl+Shift+Tab cycle, Ctrl+N picks.
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(tab_names(&ws, cx).1, 2);
        cx.simulate_keystrokes("ctrl-shift-tab ctrl-shift-tab ctrl-shift-tab");
        assert_eq!(tab_names(&ws, cx).1, 3);
        cx.simulate_keystrokes("secondary-1");
        assert_eq!(tab_names(&ws, cx).1, 0);
        cx.simulate_keystrokes("secondary-9");
        assert_eq!(tab_names(&ws, cx).1, 3);
        cx.simulate_keystrokes("ctrl-pageup");
        assert_eq!(tab_names(&ws, cx).1, 2);
        // The keys reach the tab's views: the focus follows the tab.
        let focused = ws.update_in(cx, |ws, window, cx| {
            ws.crawler()
                .unwrap()
                .focus_handle(cx)
                .contains_focused(window, cx)
        });
        assert!(focused);

        // Ctrl+W closes the current tab; the next one takes its place.
        cx.simulate_keystrokes("secondary-w");
        assert_eq!(tab_names(&ws, cx), (names(&["a.log", "b.log", "d.log"]), 2));
        assert_eq!(cx.window_title().as_deref(), Some("d.log — kloggr"));

        // Context menu actions.
        cx.dispatch_action(CopyTabPath { index: 0 });
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some(paths[0].display().to_string())
        );
        cx.dispatch_action(CloseTabsRight { index: 0 });
        assert_eq!(tab_names(&ws, cx), (names(&["a.log"]), 0));
        for path in &paths[2..] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
        }
        cx.dispatch_action(CloseTabsLeft { index: 1 });
        assert_eq!(tab_names(&ws, cx).0, names(&["c.log", "d.log"]));
        cx.dispatch_action(CloseOtherTabs { index: 1 });
        assert_eq!(tab_names(&ws, cx), (names(&["d.log"]), 0));
        cx.dispatch_action(CloseAllTabs);
        assert!(tab_names(&ws, cx).0.is_empty());
        assert_eq!(cx.window_title().as_deref(), Some("kloggr"));
    }

    #[gpui::test]
    fn dropping_several_files_opens_a_tab_each(cx: &mut TestAppContext) {
        let fx = Fixture::new("drop-many");
        let (a, b) = (fx.log("a.log", 2), fx.log("b.log", 3));
        let (ws, cx) = workspace(cx, fx.config());
        let at = point(gpui::px(200.), gpui::px(200.));
        cx.simulate_event(FileDropEvent::Entered {
            position: at,
            paths: ExternalPaths(vec![a, b].into()),
        });
        cx.simulate_event(FileDropEvent::Submit { position: at });
        assert_eq!(wait_indexed(&ws, cx), 3);
        assert_eq!(tab_names(&ws, cx).0, ["a.log", "b.log"]);
    }

    #[gpui::test]
    fn unreadable_file_closes_its_tab(cx: &mut TestAppContext) {
        let fx = Fixture::new("unreadable");
        let a = fx.log("a.log", 2);
        let (ws, cx) = workspace(cx, fx.config());
        ws.update_in(cx, |ws, window, cx| ws.open(a.clone(), window, cx));
        wait_indexed(&ws, cx);
        let missing = fx.dir.join("missing.log");
        ws.update_in(cx, |ws, window, cx| ws.open(missing, window, cx));
        for _ in 0..500 {
            cx.run_until_parked();
            if ws.read_with(cx, |ws, _| ws.tabs.len()) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(tab_names(&ws, cx).0, ["a.log"]);
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("missing.log")));
    }

    #[gpui::test]
    fn background_tab_shows_new_data(cx: &mut TestAppContext) {
        let fx = Fixture::new("newdata");
        let (a, b) = (fx.log("a.log", 2), fx.log("b.log", 3));
        let (ws, cx) = workspace(cx, fx.config());
        for path in [&a, &b] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&a)
            .unwrap()
            .write_all(b"more\n")
            .unwrap();
        let tx = ws.read_with(cx, |ws, cx| {
            ws.tabs[0].crawler.read(cx).watch.changed.clone().unwrap()
        });
        tx.try_send(()).unwrap();
        cx.run_until_parked();
        let new_data = |cx: &mut VisualTestContext| {
            ws.read_with(cx, |ws, _| {
                ws.tabs.iter().map(|t| t.new_data).collect::<Vec<_>>()
            })
        };
        assert_eq!(new_data(cx), [true, false]);
        cx.simulate_keystrokes("secondary-1");
        assert_eq!(new_data(cx), [false, false]);
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("3 lines")));
    }

    #[gpui::test]
    fn session_restores_tabs_and_their_state(cx: &mut TestAppContext) {
        let fx = Fixture::new("session");
        let (a, b, c) = (fx.log("a.log", 5), fx.log("b.log", 6), fx.log("c.log", 7));
        let (ws, cx) = workspace(cx, fx.config());
        for path in [&a, &b, &c] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        // a: marks and options; b: follow; then c is deleted; b is current.
        ws.update(cx, |ws, cx| {
            ws.tabs[0].crawler.update(cx, |c, cx| {
                c.toggle_marks_for_test(1, cx);
                c.toggle_marks_for_test(3, cx);
                c.options.match_case = false;
                c.options.auto_refresh = true;
            });
            ws.tabs[1]
                .crawler
                .update(cx, |c, cx| c.set_follow(true, cx));
        });
        cx.simulate_keystrokes("secondary-2");
        // What quitting does.
        ws.update(cx, |ws, cx| ws.save_open_files(cx));
        std::fs::remove_file(&c).unwrap();

        let config = fx.config();
        let ws2 = cx.update(|window, cx| {
            cx.new(|cx| {
                let mut ws = Workspace::new(Some(config), window, cx);
                ws.restore_session(window, cx);
                ws
            })
        });
        assert_eq!(
            tab_names(&ws2, cx),
            (vec!["a.log".into(), "b.log".into()], 1)
        );
        assert_eq!(wait_indexed(&ws2, cx), 6);
        ws2.update_in(cx, |ws, window, cx| ws.activate(0, window, cx));
        assert_eq!(wait_indexed(&ws2, cx), 5);
        let (marks, options, following) = ws2.read_with(cx, |ws, cx| {
            let a = ws.tabs[0].crawler.read(cx);
            let b = ws.tabs[1].crawler.read(cx);
            (
                a.filter().read(cx).marks().iter().collect::<Vec<_>>(),
                a.options(),
                (a.following, b.following),
            )
        });
        assert_eq!(marks, [1, 3]);
        assert!(!options.match_case && options.auto_refresh && options.regex);
        assert_eq!(following, (false, true));
        // The missing file is dropped from the saved session.
        assert_eq!(Session::load(&fx.config()).open_files.len(), 2);
    }

    #[gpui::test]
    fn highlighter_sets_menu(cx: &mut TestAppContext) {
        use kloggr_core::highlight::{Highlighter, HighlighterSet, HighlighterSets};
        let fx = Fixture::new("highlighters");
        let mut set = HighlighterSet::new("Errors");
        set.highlighters.push(Highlighter {
            pattern: "ERROR".into(),
            ..Highlighter::default()
        });
        let id = set.id.clone();
        let mut sets = HighlighterSets::default();
        sets.sets.push(set);
        sets.save(&fx.config()).unwrap();
        cx.update(|cx| Highlighters::init(Some(fx.config()), cx));
        let (_ws, cx) = workspace(cx, fx.config());
        assert!(menu_labels(cx).contains(&"Errors".to_string()));
        let active =
            |cx: &mut VisualTestContext| cx.update(|_, cx| !Highlighters::active(cx).is_empty());
        assert!(!active(cx));

        cx.dispatch_action(ToggleHighlighterSet { id: id.clone() });
        assert!(active(cx));
        // Saved, and checked in the menu.
        assert_eq!(
            HighlighterSets::load(&fx.config()).active,
            std::slice::from_ref(&id)
        );
        let checked = cx.update(|_, cx| {
            GlobalState::global(cx).app_menus().iter().any(|m| {
                m.items.iter().any(|i| match i {
                    gpui::OwnedMenuItem::Submenu(sub) => sub.items.iter().any(|i| {
                        matches!(i, gpui::OwnedMenuItem::Action { name, checked: true, .. } if name == "Errors")
                    }),
                    _ => false,
                })
            })
        });
        assert!(checked);

        cx.dispatch_action(ToggleHighlighterSet { id: id.clone() });
        assert!(!active(cx));
        cx.dispatch_action(ToggleHighlighterSet { id });
        cx.dispatch_action(NoHighlighters);
        assert!(!active(cx));
        assert!(HighlighterSets::load(&fx.config()).active.is_empty());
    }

    #[gpui::test]
    fn frozen_columns_are_remembered_per_file(cx: &mut TestAppContext) {
        let fx = Fixture::new("frozen");
        let a = fx.log("a.log", 5);
        let (ws, cx) = workspace(cx, fx.config());
        ws.update_in(cx, |ws, window, cx| ws.open(a.clone(), window, cx));
        wait_indexed(&ws, cx);
        let frozen = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.read_with(cx, |ws, cx| {
                ws.crawler().unwrap().read(cx).frozen_columns(cx)
            })
        };
        assert!(!frozen(&ws, cx).0.enabled);

        cx.dispatch_action(ToggleFreezeMain);
        assert!(frozen(&ws, cx).0.enabled);
        assert!(!frozen(&ws, cx).1.enabled);
        assert!(Session::load(&fx.config()).frozen_columns(&a).0.enabled);

        // Reopened, the file gets its frozen columns back.
        cx.dispatch_action(Close);
        ws.update_in(cx, |ws, window, cx| ws.open(a.clone(), window, cx));
        wait_indexed(&ws, cx);
        assert!(frozen(&ws, cx).0.enabled);
        cx.dispatch_action(ToggleFreezeFiltered);
        cx.dispatch_action(ToggleFreezeMain);
        let (main, filtered) = Session::load(&fx.config()).frozen_columns(&a);
        assert!(!main.enabled && filtered.enabled);
    }

    #[gpui::test]
    fn rename_tab(cx: &mut TestAppContext) {
        let fx = Fixture::new("rename");
        let (a, b) = (fx.log("a.log", 3), fx.log("b.log", 3));
        // Dialogs need gpui-component's Root around the window.
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(key_bindings());
        });
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let window = cx.add_window({
            let (slot, config) = (slot.clone(), fx.config());
            move |window, cx| {
                let ws = cx.new(|cx| Workspace::new(Some(config), window, cx));
                *slot.borrow_mut() = Some(ws.clone());
                gpui_component::Root::new(ws, window, cx)
            }
        });
        let ws: Entity<Workspace> = slot.borrow().clone().unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        for path in [&a, &b] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        let label = |ws: &Entity<Workspace>, cx: &mut VisualTestContext, i: usize| {
            ws.read_with(cx, |ws, _| ws.tab_label(&ws.tabs[i]))
        };
        assert_eq!(label(&ws, cx, 0), "a.log");

        // klogg: "Rename tab" asks for the name; Enter confirms.
        cx.dispatch_action(RenameTab { index: 0 });
        cx.run_until_parked();
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("API server");
        cx.simulate_keystrokes("enter");
        assert_eq!(label(&ws, cx, 0), "API server");
        assert_eq!(Session::load(&fx.config()).tab_name(&a), Some("API server"));
        // The name stays with the file.
        ws.update_in(cx, |ws, window, cx| {
            ws.close_tab(0, window, cx);
            ws.open(a.clone(), window, cx)
        });
        wait_indexed(&ws, cx);
        assert_eq!(label(&ws, cx, 1), "API server");

        ws.update(cx, |ws, cx| ws.rename_tab(1, "", cx));
        assert_eq!(label(&ws, cx, 1), "a.log");
        assert_eq!(Session::load(&fx.config()).tab_name(&a), None);
    }

    #[gpui::test]
    fn opens_compressed_files(cx: &mut TestAppContext) {
        use std::io::Write as _;
        let fx = Fixture::new("archive");
        let gz = fx.dir.join("app.log.gz");
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b"one\ntwo ERROR\nthree\n").unwrap();
        std::fs::write(&gz, e.finish().unwrap()).unwrap();
        let gz = gz.canonicalize().unwrap();
        let (ws, cx) = workspace(cx, fx.config());
        cx.update(|_, cx| {
            let mut o = Preferences::get(cx).clone();
            o.file.extract_without_confirmation = true;
            Preferences::set(o, None, cx);
        });
        ws.update_in(cx, |ws, window, cx| ws.open(gz.clone(), window, cx));
        // Decompressing runs on a thread, then indexing.
        let mut lines = None;
        for _ in 0..500 {
            cx.run_until_parked();
            lines = ws.read_with(cx, |ws, cx| {
                ws.crawler()
                    .and_then(|c| c.read(cx).log().map(LogFile::line_count))
            });
            if lines.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(lines, Some(3));
        let tab_path = ws.read_with(cx, |ws, _| ws.tabs[0].path.clone());
        assert!(Workspace::is_temporary(&tab_path), "{tab_path:?}");
        assert_eq!(tab_path.file_name().unwrap(), "app.log");
        // The archive is remembered, not the temporary file.
        assert_eq!(recent(&ws, cx), [gz]);
        ws.update(cx, |ws, cx| ws.save_open_files(cx));
        assert!(Session::load(&fx.config()).open_files.is_empty());
        let _ = std::fs::remove_dir_all(Workspace::temp_root());
    }
}
