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
    Action, App, AppContext, ClipboardItem, Context, Entity, EntityId, ExternalPaths, FocusHandle,
    Focusable, Global, InteractiveElement, IntoElement, Menu, MenuItem, MouseButton, ParentElement,
    PathPromptOptions, PromptLevel, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, WeakEntity, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{AppMenuBar, ContextMenuExt};
use gpui_component::tab::{Tab as GuiTab, TabBar};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, GlobalState, Sizable, TitleBar, WindowExt};
use kloggr_core::file::OpenError;
use kloggr_core::settings::{FileContext, OpenFile, Session, WindowFiles, WindowGeometry};
use kloggr_core::{Encoding, IndexProgress, LogFile, OpenOptions, encoding};
use kloggr_core::{archive, download};

use crate::archives;
use crate::background;
use crate::crawler::{
    Crawler, CrawlerEvent, EditPredefinedFilters, ToggleFreezeFiltered, ToggleFreezeMain,
};
use crate::follow::{Reload, Stop, ToggleFollow};
use crate::go_to_line;
use crate::help;
use crate::highlighters::{EditHighlighters, Highlighters, NoHighlighters, ToggleHighlighterSet};
use crate::highlighters_dialog;
use crate::options_dialog;
use crate::preferences::Preferences;
use crate::shortcuts::Shortcut;
use crate::switcher;
use crate::{log_view, quick_find};

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
        ToggleLineNumbersFiltered,
        ToggleTextWrap,
        CopyPath,
        OpenContainingFolder,
        OpenInEditor,
        ClearFile,
        OpenClipboard,
        SelectOpenFile,
        OpenUrl,
        NewWindow,
        AddToFavorites,
        RemoveFromFavorites,
        ToggleFullScreen,
        MaximizeWindow,
        MinimizeWindow
    ]
);

/// Encoding > …: decodes the current file with this encoding (its name),
/// or the detected one (`None`, "Auto").
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = workspace, no_json)]
pub struct SetEncoding {
    pub name: Option<String>,
}

/// Opens a file from File > Open Recent.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

/// Opens a file from the Favorites menu.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenFavorite {
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
            "mainwindow.new_window",
            "Open new window",
            None,
            &[],
            NewWindow,
        ),
        // klogg: no default keys, and Full Screen can't be left; F11 (and
        // macOS's Ctrl+Cmd+F) toggles it here.
        Shortcut::new(
            "mainwindow.fullscreen",
            "Full Screen",
            None,
            if cfg!(target_os = "macos") {
                &["ctrl-cmd-f"]
            } else {
                &["f11"]
            },
            ToggleFullScreen,
        ),
        Shortcut::new(
            "mainwindow.max_window",
            "Maximize window",
            None,
            &[],
            MaximizeWindow,
        ),
        Shortcut::new(
            "mainwindow.min_window",
            "Minimize window",
            None,
            &[],
            MinimizeWindow,
        ),
        Shortcut::new(
            "mainwindow.open_file",
            "Open file",
            None,
            &["secondary-o"],
            Open,
        ),
        // klogg: `QKeySequence::Paste`. Inputs keep Ctrl+V for pasting.
        Shortcut::new(
            "mainwindow.open_from_clipboard",
            "Paste text from clipboard",
            None,
            &["secondary-v"],
            OpenClipboard,
        ),
        Shortcut::new(
            "mainwindow.open_from_url",
            "Open file from URL",
            None,
            &[],
            OpenUrl,
        ),
        Shortcut::new(
            "mainwindow.select_open_file",
            "Switch to file",
            None,
            &["secondary-shift-o"],
            SelectOpenFile,
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
            "mainwindow.copy_path_to_clipboard",
            "Copy file path to clipboard",
            None,
            &[],
            CopyPath,
        ),
        Shortcut::new(
            "mainwindow.open_containing_folder",
            "Open containing folder",
            None,
            &[],
            OpenContainingFolder,
        ),
        Shortcut::new(
            "mainwindow.open_in_editor",
            "Open file in editor",
            None,
            &[],
            OpenInEditor,
        ),
        // klogg: `QKeySequence::Cut`, after a confirmation.
        Shortcut::new(
            "mainwindow.clear_file",
            "Clear file",
            None,
            &["secondary-x"],
            ClearFile,
        ),
        // klogg: W; in the log views, so that W can be typed elsewhere.
        Shortcut::new(
            "mainwindow.text_wrap",
            "Toggle text wrap",
            Some(crate::log_view::CONTEXT),
            &["w"],
            ToggleTextWrap,
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

enum DownloadEvent {
    /// Bytes received, and the size if known.
    Progress(u64, Option<u64>),
    Done(Result<PathBuf, download::DownloadError>),
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

/// The main windows' workspaces, in the order the windows opened: their
/// tabs are saved in that order (klogg: "Enable multiple windows").
#[derive(Default)]
struct MainWindows {
    list: Vec<WeakEntity<Workspace>>,
    /// The main window active last, where files from other launches open.
    last_active: Option<WeakEntity<Workspace>>,
    quitting: bool,
}

impl Global for MainWindows {}

impl MainWindows {
    fn live(cx: &App) -> Vec<Entity<Workspace>> {
        cx.try_global::<Self>()
            .map(|w| w.list.iter().filter_map(WeakEntity::upgrade).collect())
            .unwrap_or_default()
    }
}

/// Files given to a later kloggr launch: they open in the main window
/// active last, which comes to the front (klogg:
/// `KloggApp::loadFileNonInteractive`). Without a main window, one opens.
pub fn open_from_another_launch(files: Vec<PathBuf>, config_dir: PathBuf, cx: &mut App) {
    let target = cx
        .try_global::<MainWindows>()
        .and_then(|w| w.last_active.as_ref()?.upgrade())
        .or_else(|| MainWindows::live(cx).into_iter().next());
    let Some(ws) = target else {
        crate::open_main_window(Some(config_dir), None, files, cx);
        return;
    };
    let handle = ws.read(cx).window;
    handle
        .update(cx, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                for file in files {
                    ws.open(file, window, cx);
                }
            });
            window.activate_window();
        })
        .ok();
}

pub struct Workspace {
    /// This workspace's entity, to find it among the main windows.
    id: EntityId,
    window: gpui::AnyWindowHandle,
    /// The window's size and place, kept up to date for the session.
    geometry: Option<WindowGeometry>,
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
    _prompt: Option<gpui::Subscription>,
    /// "Extracting app.log.gz… 40%", while an archive is opened.
    extracting: Option<String>,
    extract_cancel: Arc<AtomicBool>,
    _extract: Option<Task<()>>,
    /// "Downloading https://…… 40%", while a file is downloaded.
    downloading: Option<String>,
    download_cancel: Arc<AtomicBool>,
    _download: Option<Task<()>>,
    /// The open archive file picker (see `archives::pick`).
    _picker: Option<Entity<archives::ArchivePicker>>,
    /// The open file switcher (Ctrl+Shift+O).
    _switcher: Option<Entity<switcher::FileSwitcher>>,
}

impl Workspace {
    pub fn new(config_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut session = config_dir.as_deref().map(Session::load).unwrap_or_default();
        session
            .recent_files
            .set_shown(Preferences::get(cx).file.recent_files);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let me = cx.entity().downgrade();
        cx.default_global::<MainWindows>().list.push(me);
        cx.on_release(|this, cx| this.released(cx)).detach();
        // Another window may have changed the recent files; the menus are
        // the active window's.
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let me = cx.entity().downgrade();
                cx.default_global::<MainWindows>().last_active = Some(me);
                this.reload_session(cx);
                this.update_menus(cx);
            }
        })
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            let geometry = geometry_of(window);
            let fullscreen_changed =
                this.geometry.map(|g| g.fullscreen) != Some(geometry.fullscreen);
            this.geometry = Some(geometry);
            if fullscreen_changed {
                this.update_menus(cx);
            }
        })
        .detach();
        // Marks and options change without a tab change: save them on exit.
        cx.on_app_quit(|this, cx| {
            cx.default_global::<MainWindows>().quitting = true;
            this.save_open_files(cx);
            this.extract_cancel.store(true, Ordering::Relaxed);
            this.download_cancel.store(true, Ordering::Relaxed);
            let _ = std::fs::remove_dir_all(Self::temp_root());
            async {}
        })
        .detach();
        let mut this = Self {
            id: cx.entity_id(),
            window: window.window_handle(),
            geometry: Some(geometry_of(window)),
            tabs: Vec::new(),
            active: 0,
            menu_bar: (!cfg!(target_os = "macos")).then(|| AppMenuBar::new(cx)),
            config_dir,
            session,
            tab_scroll: ScrollHandle::new(),
            focus,
            error: None,
            _prompt: None,
            extracting: None,
            extract_cancel: Arc::default(),
            _extract: None,
            _picker: None,
            _switcher: None,
            downloading: None,
            download_cancel: Arc::default(),
            _download: None,
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
        let following = self.crawler().is_some_and(|c| c.read(cx).is_following());
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
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open…", Open),
                MenuItem::action("Open from Clipboard", OpenClipboard),
                MenuItem::action("Open from URL…", OpenUrl),
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
            // klogg: the Edit menu, disabled without a file.
            Menu::new("Edit")
                .items([
                    MenuItem::action("Copy", log_view::Copy),
                    MenuItem::action("Select All", log_view::SelectAll),
                    MenuItem::separator(),
                    MenuItem::action("Find…", quick_find::Open),
                    MenuItem::separator(),
                    MenuItem::action("Go to Line…", go_to_line::Open),
                    MenuItem::separator(),
                    MenuItem::action("Copy Path", CopyPath),
                    MenuItem::action("Open Containing Folder", OpenContainingFolder),
                    MenuItem::separator(),
                    MenuItem::action("Open in Editor", OpenInEditor),
                    MenuItem::action("Clear File…", ClearFile),
                ])
                .disabled(no_tab),
            // klogg: View > Freeze columns in main / filtered view.
            Menu::new("View").items([
                MenuItem::action("Overview", ToggleOverview).checked(view.overview),
                MenuItem::action("Line Numbers in Main View", ToggleLineNumbersMain)
                    .checked(view.line_numbers_main),
                MenuItem::action("Line Numbers in Filtered View", ToggleLineNumbersFiltered)
                    .checked(view.line_numbers_filtered),
                MenuItem::separator(),
                MenuItem::action("Text Wrap", ToggleTextWrap).checked(view.text_wrap),
                // klogg: no frozen columns with text wrap.
                MenuItem::action("Freeze Columns in Main View", ToggleFreezeMain)
                    .checked(frozen.0.enabled)
                    .disabled(no_tab || view.text_wrap),
                MenuItem::action("Freeze Columns in Filtered View", ToggleFreezeFiltered)
                    .checked(frozen.1.enabled)
                    .disabled(no_tab || view.text_wrap),
                MenuItem::separator(),
                // klogg: View > Follow File, Reload; Stop is on its toolbar.
                MenuItem::action("Follow File", ToggleFollow)
                    .checked(following)
                    .disabled(no_tab),
                MenuItem::action("Reload", Reload).disabled(no_tab),
                MenuItem::action("Stop", Stop).disabled(no_tab),
                MenuItem::separator(),
                MenuItem::submenu(self.opened_files_menu()),
                MenuItem::separator(),
                MenuItem::action("Full Screen", ToggleFullScreen)
                    .checked(self.geometry.is_some_and(|g| g.fullscreen)),
            ]),
            self.encoding_menu(cx),
            Menu::new("Tools").items([
                MenuItem::action("Options…", ShowOptions),
                MenuItem::action("Predefined Filters…", EditPredefinedFilters),
                MenuItem::action("Scratchpad", OpenScratchpad),
                MenuItem::separator(),
                MenuItem::action("Highlighters…", EditHighlighters),
                MenuItem::submenu(Highlighters::menu(cx)),
            ]),
            self.favorites_menu(),
            // klogg: Help > Documentation, Report Issue, About.
            Menu::new("Help").items([
                MenuItem::action("Documentation", help::ShowDocumentation),
                MenuItem::separator(),
                MenuItem::action("Report Issue…", help::ReportIssue),
                MenuItem::separator(),
                MenuItem::action("About kloggr", help::About),
            ]),
        ]
    }

    /// klogg: the Favorites menu (`updateFavoritesMenu`): add the current
    /// file, remove one, then the favorites.
    fn favorites_menu(&self) -> Menu {
        let current = self.tabs.get(self.active).map(|t| t.path.as_path());
        let can_add =
            current.is_some_and(|p| !self.session.is_favorite(p) && !Self::is_temporary(p));
        let mut items = vec![
            MenuItem::action("Add to Favorites", AddToFavorites).disabled(!can_add),
            MenuItem::action("Remove from Favorites…", RemoveFromFavorites)
                .disabled(self.session.favorites.is_empty()),
        ];
        if !self.session.favorites.is_empty() {
            items.push(MenuItem::separator());
        }
        items.extend(
            self.session.favorites.iter().map(|path| {
                MenuItem::action(path_label(path), OpenFavorite { path: path.clone() })
            }),
        );
        Menu::new("Favorites").items(items)
    }

    /// klogg: View > Opened Files: the switcher, then a tab per file.
    fn opened_files_menu(&self) -> Menu {
        let mut items = vec![
            MenuItem::action("Switch to File…", SelectOpenFile),
            MenuItem::separator(),
        ];
        items.extend(self.tabs.iter().enumerate().map(|(index, tab)| {
            MenuItem::action(tab.path.display().to_string(), ActivateTab { index })
                .checked(index == self.active)
        }));
        Menu::new("Opened Files")
            .items(items)
            .disabled(self.tabs.is_empty())
    }

    /// klogg: the Encoding menu (`EncodingMenu::generate`): Auto, then the
    /// encodings by group. klogg's "System" entry is left out: kloggr
    /// detects legacy encodings itself (`docs/decisions.md`).
    fn encoding_menu(&self, cx: &App) -> Menu {
        let crawler = self.crawler().map(|c| c.read(cx));
        let chosen = crawler.and_then(|c| c.encoding);
        let mut items = vec![
            MenuItem::action("Auto", SetEncoding { name: None })
                .checked(crawler.is_some() && chosen.is_none()),
            MenuItem::separator(),
        ];
        items.extend(encoding::MENU.iter().map(|(group, names)| {
            MenuItem::submenu(Menu::new(*group).items(names.iter().map(|name| {
                let checked = chosen.is_some_and(|e| e.name() == *name);
                MenuItem::action(
                    *name,
                    SetEncoding {
                        name: Some((*name).to_owned()),
                    },
                )
                .checked(checked)
            })))
        }));
        Menu::new("Encoding")
            .items(items)
            .disabled(crawler.is_none())
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

    /// Changes the session and saves it. The file is read again first:
    /// another window may have changed it (recent files, tab names…).
    fn update_session(&mut self, cx: &App, f: impl FnOnce(&mut Session)) {
        self.reload_session(cx);
        f(&mut self.session);
        if let Some(dir) = &self.config_dir
            && let Err(e) = self.session.save(dir)
        {
            tracing::warn!("{e}");
        }
    }

    /// Reads the session file again, for what other windows changed.
    fn reload_session(&mut self, cx: &App) {
        if let Some(dir) = &self.config_dir {
            self.session = Session::load(dir);
            self.session
                .recent_files
                .set_shown(Preferences::get(cx).file.recent_files);
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

    /// File > Open from Clipboard (klogg: `openClipboard`): the clipboard's
    /// text, saved to a temporary file, in a new tab.
    fn open_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|c| c.text());
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            self.error = Some("The clipboard holds no text".to_owned());
            cx.notify();
            return;
        };
        match Self::temp_file("clipboard", ".txt")
            .and_then(|path| std::fs::write(&path, text).map(|_| path))
        {
            Ok(path) => self.open_with(path, None, window, cx),
            Err(e) => {
                self.error = Some(format!("Cannot save the clipboard: {e}"));
                cx.notify();
            }
        }
    }

    /// View > Opened Files > Switch to File… (klogg: `selectOpenedFile`).
    fn select_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let files = self.tabs.iter().map(|t| t.path.clone()).collect();
        let this = cx.entity().downgrade();
        let labels = switcher::Labels {
            title: "Switch to file",
            ok: "Switch",
        };
        self._switcher = Some(switcher::open(
            labels,
            files,
            self.active,
            window,
            cx,
            move |index, window, cx| {
                this.update(cx, |ws, cx| ws.activate(index, window, cx))
                    .ok();
            },
        ));
    }

    /// Downloads `url` to the temporary folder, with progress in the status
    /// line, then opens it like any file (klogg: `openRemoteFile`). Stop
    /// (Esc) cancels it.
    fn open_url(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = download::parse_url(&url) else {
            self.error = Some(format!("{} is not an http or https URL", url.trim()));
            cx.notify();
            return;
        };
        // A new download replaces the running one.
        self.download_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.download_cancel = cancel.clone();
        let dir = Self::temp_root().join("downloads");
        let (tx, rx) = async_channel::unbounded();
        let thread_url = url.clone();
        background::spawn_blocking(
            "kloggr-download",
            move || {
                let mut last = None;
                let result = download::download(&thread_url, &dir, &cancel, |got, total| {
                    // One event per percent, or per MiB without a size.
                    let step = total.map_or(got >> 20, |t| got * 100 / t.max(1));
                    if last.replace(step) != Some(step) {
                        tx.send_blocking(DownloadEvent::Progress(got, total)).ok();
                    }
                });
                tx.send_blocking(DownloadEvent::Done(result)).ok();
            },
            cx,
        );
        self.downloading = Some(format!("Downloading {url}…"));
        self.error = None;
        cx.notify();
        self._download = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    match event {
                        DownloadEvent::Progress(got, total) => {
                            let done = match total {
                                Some(total) => format!("{}%", got * 100 / total.max(1)),
                                None => archives::human_size(got).to_string(),
                            };
                            this.downloading = Some(format!("Downloading {url}… {done}"));
                        }
                        DownloadEvent::Done(result) => {
                            this.downloading = None;
                            match result {
                                Ok(path) => this.open(path, window, cx),
                                Err(download::DownloadError::Cancelled) => {}
                                // klogg: a message box with the error.
                                Err(e) => this.error = Some(format!("Cannot download {url}: {e}")),
                            }
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

    /// A new file name in the temporary folder: `{stem}-1{ext}`, `-2`…
    fn temp_file(stem: &str, ext: &str) -> std::io::Result<PathBuf> {
        let dir = Self::temp_root().join(stem);
        std::fs::create_dir_all(&dir)?;
        Ok((1..)
            .map(|n| dir.join(format!("{stem}-{n}{ext}")))
            .find(|p| !p.exists())
            .expect("a free name"))
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
        // klogg: a new file gets the default search options
        // (`defaultView.*`), a session's file its own.
        let context = context.unwrap_or_else(|| Preferences::get(cx).new_file_context());
        let options = OpenOptions {
            encoding: context.encoding.as_deref().and_then(Encoding::for_label),
            hide_ansi: Preferences::get(cx).view.hide_ansi,
            ..OpenOptions::default()
        };
        let crawler = cx.new(|cx| {
            let mut crawler = Crawler::new(window, cx);
            crawler.set_context(context, cx);
            crawler.set_frozen_columns(main_frozen, filtered_frozen, cx);
            crawler
        });
        let events = cx.subscribe(&crawler, |this, crawler, event: &CrawlerEvent, cx| {
            match event {
                CrawlerEvent::FrozenChanged => {
                    // klogg: `saveFrozenColumns`.
                    if let Some(tab) = this.tabs.iter().find(|t| t.crawler == crawler) {
                        let (main, filtered) = crawler.read(cx).frozen_columns(cx);
                        let path = tab.path.clone();
                        this.update_session(cx, |s| s.set_frozen_columns(&path, main, filtered));
                    }
                    this.update_menus(cx);
                    return;
                }
                CrawlerEvent::Status => {
                    if this.crawler() == Some(&crawler) {
                        this.update_menus(cx);
                    }
                }
                CrawlerEvent::NewData => {
                    let active = this.crawler().cloned();
                    if let Some(tab) = this.tabs.iter_mut().find(|t| t.crawler == crawler)
                        && active.as_ref() != Some(&crawler)
                    {
                        tab.new_data = true;
                    }
                }
            }
            if let Some(tab) = this.tabs.iter_mut().find(|t| t.crawler == crawler) {
                tab.status = status_of(&tab.path, crawler.read(cx));
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
                let result = LogFile::open_with(&thread_path, options, &thread_cancel, |p| {
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
                            let path = tab.path.clone();
                            tab.crawler.update(cx, |c, cx| c.set_log(Some(log), cx));
                            tab.status = status_of(&path, tab.crawler.read(cx));
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
            status: format!("{} — indexing…", path.display()),
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
        self.update_session(cx, |s| s.recent_files.add(path.to_path_buf()));
        self.update_menus(cx);
    }

    /// Reopens the files of the last session, each with its state, and
    /// shows the tab that was current (klogg: `reloadSession`). Files that
    /// are gone are skipped.
    #[cfg(test)]
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = self.session.all_windows().swap_remove(0);
        self.restore(first, window, cx);
    }

    /// Reopens one window's files from the session.
    pub fn restore(&mut self, files: WindowFiles, window: &mut Window, cx: &mut Context<Self>) {
        let current = files.current_file;
        let mut current_tab = None;
        for (i, file) in files.open_files.into_iter().enumerate() {
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
        // Every main window's tabs, in the order the windows opened.
        let id = self.id;
        let windows = MainWindows::live(cx)
            .iter()
            .map(|ws| {
                if ws.entity_id() == id {
                    self.window_files(cx)
                } else {
                    ws.read(cx).window_files(cx)
                }
            })
            .collect::<Vec<_>>();
        let windows = if windows.is_empty() {
            vec![self.window_files(cx)]
        } else {
            windows
        };
        self.update_session(cx, |s| s.set_windows(windows));
    }

    /// This window's tabs and their state, for the session.
    fn window_files(&self, cx: &App) -> WindowFiles {
        let open_files: Vec<OpenFile> = self
            .tabs
            .iter()
            .filter(|t| !Self::is_temporary(&t.path))
            .map(|t| OpenFile {
                path: t.path.clone(),
                context: t.crawler.read(cx).context(cx),
            })
            .collect();
        // The current tab, counted without the temporary files.
        let current_file = self
            .tabs
            .iter()
            .take(self.active)
            .filter(|t| !Self::is_temporary(&t.path))
            .count()
            .min(open_files.len().saturating_sub(1));
        WindowFiles {
            current_file,
            open_files,
            geometry: self.geometry,
        }
    }

    /// The window closed. If other main windows are left, the session
    /// forgets its tabs; if it was the last, they are what the next start
    /// reopens (klogg: the last closed window is saved).
    fn released(&mut self, cx: &mut App) {
        let quitting = cx.try_global::<MainWindows>().is_some_and(|w| w.quitting);
        let id = self.id;
        cx.default_global::<MainWindows>()
            .list
            .retain(|w| w.entity_id() != id);
        if quitting {
            return;
        }
        match MainWindows::live(cx).first() {
            Some(next) => next.update(cx, |ws, cx| ws.save_open_files(cx)),
            None => self.save_open_files(cx),
        }
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
            self.update_menus(cx);
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

    /// Favorites > Add to Favorites (klogg: `addToFavorites`).
    fn add_to_favorites(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.get(self.active).map(|t| t.path.clone()) else {
            return;
        };
        if Self::is_temporary(&path) {
            return;
        }
        self.update_session(cx, |s| s.add_favorite(&path));
        self.update_menus(cx);
    }

    /// Favorites > Remove from Favorites… (klogg: `removeFromFavorites`,
    /// which asks which one, the current file first).
    fn prompt_remove_favorite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.session.favorites.clone();
        if files.is_empty() {
            return;
        }
        let current = self.tabs.get(self.active).map(|t| t.path.as_path());
        let selected = files
            .iter()
            .position(|f| Some(f.as_path()) == current)
            .unwrap_or(0);
        let this = cx.entity().downgrade();
        let labels = switcher::Labels {
            title: "Remove from favorites",
            ok: "Remove",
        };
        let chosen_files = files.clone();
        self._switcher = Some(switcher::open(
            labels,
            files,
            selected,
            window,
            cx,
            move |index, _, cx| {
                let path = chosen_files[index].clone();
                this.update(cx, |ws, cx| {
                    ws.update_session(cx, |s| s.remove_favorite(&path));
                    ws.update_menus(cx);
                })
                .ok();
            },
        ));
    }

    /// Opens a favorite, offering to remove it if it is gone (klogg:
    /// `openFileFromFavorites`).
    fn open_favorite(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if std::fs::File::open(&path).is_ok() {
            self.open(path, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!(
                "Could not read file {}. Remove it from favorites?",
                path.display()
            ),
            None,
            &["Yes", "No"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    this.update_session(cx, |s| s.remove_favorite(&path));
                    this.update_menus(cx);
                })
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
                    this.update_session(cx, |s| s.recent_files.remove(&path));
                    this.update_menus(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn clear_recent(&mut self, cx: &mut Context<Self>) {
        self.update_session(cx, |s| s.recent_files.clear());
        self.update_menus(cx);
    }

    /// Files dropped on the window, each in its tab (klogg: `dropEvent`).
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths.paths().iter().filter(|p| p.is_file()) {
            self.open(path.clone(), window, cx);
        }
    }

    /// View > Reload (klogg: `MainWindow::reload`): indexes the current
    /// file again. Ignored while it is first indexed.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(crawler) = self.crawler().cloned() {
            crawler.update(cx, |c, cx| c.reindex(cx));
        }
    }

    /// Edit > Copy Path, or the tab's Copy full path (klogg:
    /// `copyFullPath`).
    fn copy_path(&self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(index) {
            let path = tab.path.display().to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    /// Edit > Clear File (klogg: `clearLog`): empties the current file on
    /// disk after a warning.
    fn clear_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let crawler = tab.crawler.clone();
        if crawler.read(cx).log.is_none() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!(
                "Clear file {}? Its content will be removed from disk. This can't be undone.",
                tab.path.display()
            ),
            None,
            &["Clear", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |_, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let task = crawler.update(cx, |c, cx| c.clear_file(cx));
            if let Err(e) = task.await {
                let message = format!("Cannot clear the file: {e}");
                if let Ok(shown) = cx.update(|window, cx| {
                    window.prompt(PromptLevel::Critical, &message, None, &["OK"], cx)
                }) {
                    shown.await.ok();
                }
            }
        })
        .detach();
    }

    /// View > Stop (klogg: `stopLoading`): stops the current tab's search,
    /// and its indexing, which closes the tab as in klogg (an interrupted
    /// load leaves no file to show).
    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.downloading.take().is_some() {
            self.download_cancel.store(true, Ordering::Relaxed);
            self._download = None;
            cx.notify();
            return;
        }
        let index = self.active;
        let Some(crawler) = self.crawler().cloned() else {
            return;
        };
        let first_load = crawler.read(cx).log.is_none();
        let stopped = crawler.update(cx, |c, cx| c.stop(cx));
        if first_load || stopped {
            self.close_tab(index, window, cx);
        }
    }

    fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if let Some(extracting) = &self.extracting {
            return extracting.clone();
        }
        if let Some(downloading) = &self.downloading {
            return downloading.clone();
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
            let path = tab.path.clone();
            self.update_session(cx, |s| s.set_tab_name(&path, name));
            cx.notify();
        }
    }

    /// Asks for the tab's new name (klogg: "Rename tab", `QInputDialog`).
    fn prompt_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let current = self.tab_label(tab);
        self.prompt_text(
            "Rename tab",
            "Tab name (empty for the file name)",
            current,
            "OK",
            move |ws, name, _, cx| ws.rename_tab(index, &name, cx),
            window,
            cx,
        );
    }

    /// File > Open from URL… (klogg: `openUrl`, which offers the
    /// clipboard's text if it is a URL).
    fn prompt_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let initial = cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .filter(|t| download::parse_url(t).is_some())
            .map(|t| t.trim().to_owned())
            .unwrap_or_default();
        self.prompt_text(
            "Open from URL",
            "URL to download",
            initial,
            "Open",
            |ws, url, window, cx| ws.open_url(url, window, cx),
            window,
            cx,
        );
    }

    /// A dialog asking for one line of text; Enter or `ok` passes it to
    /// `done`.
    #[allow(clippy::too_many_arguments)]
    fn prompt_text(
        &mut self,
        title: &'static str,
        label: &'static str,
        initial: String,
        ok: &'static str,
        done: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + Clone + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(initial, window, cx);
            s
        });
        let this = cx.entity().downgrade();
        // Enter confirms.
        let on_enter = done.clone();
        self._prompt = Some(cx.subscribe_in(
            &input,
            window,
            move |ws, input, e: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let text = input.read(cx).value().to_string();
                    window.close_dialog(cx);
                    on_enter(ws, text, window, cx);
                }
            },
        ));
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let on_ok = {
                let (this, input, done) = (this.clone(), input.clone(), done.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let text = input.read(cx).value().to_string();
                    window.close_dialog(cx);
                    this.update(cx, |ws, cx| done(ws, text, window, cx)).ok();
                }
            };
            dialog
                .title(title)
                .w(px(480.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child(label))
                        .child(Input::new(&input)),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("prompt-cancel")
                                .label("Cancel")
                                .small()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("prompt-ok")
                                .label(ok)
                                .small()
                                .primary()
                                .on_click(on_ok),
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
            .on_action(cx.listener(|_, _: &ToggleTextWrap, window, cx| {
                toggle_view_option(|v| &mut v.text_wrap, window, cx)
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
            .on_action(cx.listener(|this, a: &CopyTabPath, _, cx| this.copy_path(a.index, cx)))
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
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_path(this.active, cx)))
            .on_action(
                cx.listener(|this, _: &OpenClipboard, window, cx| this.open_clipboard(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenUrl, window, cx| this.prompt_url(window, cx)))
            .on_action(cx.listener(|this, _: &AddToFavorites, _, cx| this.add_to_favorites(cx)))
            .on_action(cx.listener(|_, _: &ToggleFullScreen, window, _| window.toggle_fullscreen()))
            // klogg: `showMaximized`; here it also restores a maximized window.
            .on_action(cx.listener(|_, _: &MaximizeWindow, window, _| window.zoom_window()))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &RemoveFromFavorites, window, cx| {
                this.prompt_remove_favorite(window, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenFavorite, window, cx| {
                this.open_favorite(a.path.clone(), window, cx)
            }))
            // klogg: File > New Window; it starts empty.
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| {
                let config_dir = this.config_dir.clone();
                cx.defer(move |cx| {
                    crate::open_main_window(config_dir, None, Vec::new(), cx);
                });
            }))
            .on_action(
                cx.listener(|this, _: &SelectOpenFile, window, cx| {
                    this.select_open_file(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &OpenContainingFolder, _, cx| {
                if let Some(tab) = this.tabs.get(this.active) {
                    cx.reveal_path(&tab.path);
                }
            }))
            // klogg: `openFileInDefaultApplication`.
            .on_action(cx.listener(|this, _: &OpenInEditor, _, cx| {
                if let Some(tab) = this.tabs.get(this.active) {
                    cx.open_with_system(&tab.path);
                }
            }))
            .on_action(cx.listener(|this, _: &ClearFile, window, cx| this.clear_file(window, cx)))
            .on_action(
                cx.listener(|_, _: &help::ShowDocumentation, _, cx| help::show_documentation(cx)),
            )
            .on_action(cx.listener(|_, _: &help::ReportIssue, _, cx| help::report_issue(cx)))
            .on_action(cx.listener(|_, _: &help::About, window, cx| help::about(window, cx)))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, a: &SetEncoding, _, cx| {
                let encoding = a.name.as_deref().and_then(Encoding::for_label);
                if let Some(crawler) = this.crawler().cloned() {
                    crawler.update(cx, |c, cx| c.set_encoding(encoding, cx));
                    this.save_open_files(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Stop, window, cx| this.stop(window, cx)))
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
            .child(if cfg!(target_os = "linux") {
                // GNOME (Wayland) draws no decorations for GPUI windows: the
                // title bar with the window controls is ours.
                TitleBar::new()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .children(self.menu_bar.clone()),
                    )
                    .into_any_element()
            } else {
                div()
                    .h(px(32.))
                    .px_1()
                    .border_b_1()
                    .border_color(border)
                    .children(self.menu_bar.clone())
                    .into_any_element()
            })
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

/// "path — N lines — encoding", or the indexing progress.
fn status_of(path: &Path, crawler: &Crawler) -> String {
    let shown = path.display();
    if let Some(p) = crawler.saving {
        return format!("{shown} — saving {p}%");
    }
    match (crawler.reindex_percent(), &crawler.log) {
        (Some(p), _) => format!("{shown} — indexing {p}%"),
        (None, Some(log)) => {
            // klogg: "Detected as …" or "Displayed as …".
            let how = if crawler.encoding.is_some() {
                "displayed as"
            } else {
                "detected as"
            };
            let lines = log.line_count();
            format!("{shown} — {lines} lines — {how} {}", log.encoding())
        }
        (None, None) => format!("{shown} — indexing…"),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The window's size and place, and whether it is maximized or full screen.
fn geometry_of(window: &Window) -> WindowGeometry {
    let (bounds, maximized, fullscreen) = match window.window_bounds() {
        gpui::WindowBounds::Windowed(b) => (b, false, false),
        gpui::WindowBounds::Maximized(b) => (b, true, false),
        gpui::WindowBounds::Fullscreen(b) => (b, false, true),
    };
    WindowGeometry {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized,
        fullscreen,
    }
}

/// "1  app.log — /var/log" (klogg numbers recent files 1..9, then a, b, …).
fn recent_label(index: usize, path: &Path) -> String {
    let key = match index {
        0..=8 => char::from(b'1' + index as u8),
        _ => char::from(b'a' + (index - 9) as u8),
    };
    format!("{key}  {}", path_label(path))
}

/// "app.log — /var/log", the folder shortened in the middle when long
/// (klogg: `DisplayFilePath`, `shrinkPath`).
fn path_label(path: &Path) -> String {
    const MAX_DIR: usize = 60;
    let dir = path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let chars: Vec<char> = dir.chars().collect();
    let dir = if chars.len() > MAX_DIR {
        let keep = (MAX_DIR - 1) / 2;
        let head: String = chars[..keep].iter().collect();
        let tail: String = chars[chars.len() - keep..].iter().collect();
        format!("{head}…{tail}")
    } else {
        dir
    };
    format!("{} — {dir}", file_name(path))
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
            cx.bind_keys(crate::follow::key_bindings());
            cx.bind_keys(key_bindings());
        });
        // Dialogs need gpui-component's Root around the window.
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let window = cx.add_window({
            let slot = slot.clone();
            move |window, cx| {
                let ws = cx.new(|cx| Workspace::new(Some(config), window, cx));
                *slot.borrow_mut() = Some(ws.clone());
                gpui_component::Root::new(ws, window, cx)
            }
        });
        let ws = slot.borrow().clone().expect("built");
        (
            ws,
            VisualTestContext::from_window(window.into(), cx).into_mut(),
        )
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

    /// As if kloggr started again: the workspaces made so far are no
    /// longer main windows.
    fn forget_windows(cx: &mut VisualTestContext) {
        cx.update(|_, cx| cx.default_global::<MainWindows>().list.clear());
    }

    fn recent(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<PathBuf> {
        ws.read_with(cx, |ws, _| ws.session.recent_files.files().to_vec())
    }

    fn menu_labels_of(ws: &Workspace, cx: &App) -> Vec<String> {
        ws.menus(cx)
            .into_iter()
            .flat_map(|m| m.items)
            .filter_map(|i| match i {
                MenuItem::Action { name, .. } => Some(name.to_string()),
                _ => None,
            })
            .collect()
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
        forget_windows(cx);
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
        // Open Recent is empty and disabled (Opened Files still lists a.log).
        let open_recent = ws.read_with(cx, |ws, cx| {
            ws.menus(cx)[0].items.iter().find_map(|i| match i {
                MenuItem::Submenu(m) if m.name == "Open Recent" => {
                    Some((m.disabled, m.items.len()))
                }
                _ => None,
            })
        });
        assert_eq!(
            open_recent,
            Some((true, 2)),
            "only the separator and Clear List"
        );
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
    fn reload_and_stop(cx: &mut TestAppContext) {
        let fx = Fixture::new("reload");
        let path = fx.log("a.log", 3);
        let (ws, cx) = workspace(cx, fx.config());
        ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
        assert_eq!(wait_indexed(&ws, cx), 3);
        let crawler = ws.read_with(cx, |ws, _| ws.crawler().unwrap().clone());
        crawler.update(cx, |c, cx| c.toggle_marks_for_test(1, cx));

        // F5 indexes the file again and drops the marks (klogg: `reload`).
        // (Appended: Windows refuses to rewrite a mapped file.)
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"3\n4\n").unwrap();
        drop(file);
        cx.simulate_keystrokes("f5");
        cx.run_until_parked();
        assert_eq!(wait_indexed(&ws, cx), 5);
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("5 lines")));
        assert!(crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().is_empty()));
        assert!(ws.read_with(cx, |ws, cx| {
            menu_labels_of(ws, cx).contains(&"Reload".to_owned())
        }));

        // Esc stops a search and keeps the tab.
        crawler.update_in(cx, |c, window, cx| {
            c.set_search_text("1", window, cx);
            c.start_search(window, cx);
            assert!(c.is_searching());
        });
        cx.simulate_keystrokes("escape");
        assert!(crawler.read_with(cx, |c, _| !c.is_searching()));
        assert_eq!(ws.read_with(cx, |ws, _| ws.tabs.len()), 1);

        // The status line shows the progress of a reindex; stopping it
        // closes the tab, as in klogg.
        crawler.update(cx, |c, cx| c.reindex(cx));
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("indexing 0%")));
        cx.simulate_keystrokes("escape");
        assert_eq!(ws.read_with(cx, |ws, _| ws.tabs.len()), 0);
    }

    #[gpui::test]
    fn edit_menu(cx: &mut TestAppContext) {
        let fx = Fixture::new("edit");
        let path = fx.log("a.log", 3);
        let (ws, cx) = workspace(cx, fx.config());
        assert!(ws.read_with(cx, |ws, cx| {
            ws.menus(cx).iter().any(|m| m.name == "Edit" && m.disabled)
        }));
        ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
        assert_eq!(wait_indexed(&ws, cx), 3);
        assert!(ws.read_with(cx, |ws, cx| {
            ws.menus(cx).iter().any(|m| m.name == "Edit" && !m.disabled)
        }));
        let crawler = ws.read_with(cx, |ws, _| ws.crawler().unwrap().clone());

        // From the search box, Select All and Copy work on the log view.
        cx.dispatch_action(crate::crawler::FocusSearch);
        cx.dispatch_action(log_view::SelectAll);
        cx.dispatch_action(log_view::Copy);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("a.log 0\na.log 1\na.log 2")
        );

        cx.dispatch_action(CopyPath);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some(path.display().to_string())
        );

        // Clear File asks first; Cancel keeps the file.
        cx.dispatch_action(ClearFile);
        cx.run_until_parked();
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 24);

        crawler.update(cx, |c, cx| c.toggle_marks_for_test(1, cx));
        cx.dispatch_action(ClearFile);
        cx.run_until_parked();
        cx.simulate_prompt_answer("Clear");
        assert_eq!(wait_indexed(&ws, cx), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().is_empty()));
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("0 lines")));
    }

    #[gpui::test]
    fn open_from_clipboard(cx: &mut TestAppContext) {
        let fx = Fixture::new("clipboard");
        let (ws, cx) = workspace(cx, fx.config());
        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        cx.dispatch_action(OpenClipboard);
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("no text")));

        cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo\n".into()));
        cx.dispatch_action(OpenClipboard);
        assert_eq!(wait_indexed(&ws, cx), 2);
        cx.dispatch_action(OpenClipboard);
        assert_eq!(wait_indexed(&ws, cx), 2);
        let paths: Vec<PathBuf> =
            ws.read_with(cx, |ws, _| ws.tabs.iter().map(|t| t.path.clone()).collect());
        assert_eq!(paths.len(), 2, "each paste opens a new tab");
        assert!(paths.iter().all(|p| Workspace::is_temporary(p)));
        assert!(paths[0].ends_with("clipboard-1.txt"));
        // Not remembered: the file is gone after quitting.
        assert!(recent(&ws, cx).is_empty());
        let _ = std::fs::remove_dir_all(Workspace::temp_root());
    }

    #[gpui::test]
    fn switch_to_open_file(cx: &mut TestAppContext) {
        let fx = Fixture::new("switch");
        let a = fx.log("app.log", 2);
        let b = fx.log("db.log", 2);
        let c = fx.log("web.log", 2);
        let (ws, cx) = workspace(cx, fx.config());
        for path in [&a, &b, &c] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
        }
        let active = |cx: &mut VisualTestContext| ws.read_with(cx, |ws, _| ws.active);
        assert_eq!(active(cx), 2);
        // View > Opened Files lists the tabs, the current one checked.
        let opened = |cx: &mut VisualTestContext| {
            ws.read_with(cx, |ws, _| ws.opened_files_menu())
                .items
                .into_iter()
                .filter_map(|i| match i {
                    MenuItem::Action { name, checked, .. } => Some((name.to_string(), checked)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(opened(cx)[3], (c.display().to_string(), true));

        // Ctrl+Shift+O, a filter, then Enter.
        cx.simulate_keystrokes("ctrl-shift-o");
        let switcher = ws.read_with(cx, |ws, _| ws._switcher.clone().unwrap());
        cx.simulate_input("ap*log");
        assert_eq!(switcher.read_with(cx, |s, _| s.shown.clone()), vec![0]);
        cx.simulate_keystrokes("enter");
        assert_eq!(active(cx), 0);
        assert!(opened(cx)[1].1);

        // Down moves the selection; the list is filtered anew each time.
        cx.simulate_keystrokes("ctrl-shift-o");
        let switcher = ws.read_with(cx, |ws, _| ws._switcher.clone().unwrap());
        cx.simulate_input(".log");
        cx.simulate_keystrokes("down down down enter");
        assert_eq!(switcher.read_with(cx, |s, _| s.selected), 2);
        assert_eq!(active(cx), 2);

        // Without a tab the menu is disabled.
        cx.dispatch_action(CloseAllTabs);
        assert!(ws.read_with(cx, |ws, _| ws.opened_files_menu().disabled));
        assert!(ws.read_with(cx, |ws, cx| {
            ws.menus(cx).iter().any(|m| m.name == "Edit" && m.disabled)
        }));
    }

    /// Serves one HTTP request with `status` and `body` on a thread.
    fn serve_once(status: &'static str, body: &'static [u8]) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    #[gpui::test]
    fn open_from_url(cx: &mut TestAppContext) {
        let fx = Fixture::new("url");
        let (ws, cx) = workspace(cx, fx.config());
        // The dialog offers the URL on the clipboard; Enter downloads it.
        let url = format!(
            "{}/logs/remote.log",
            serve_once("200 OK", b"one\ntwo\nthree\n")
        );
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        cx.dispatch_action(OpenUrl);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(wait_indexed(&ws, cx), 3);
        let path = ws.read_with(cx, |ws, _| ws.tabs[0].path.clone());
        assert!(path.ends_with("downloads/remote.log"), "{path:?}");
        assert!(Workspace::is_temporary(&path));
        assert!(recent(&ws, cx).is_empty());

        // Errors show in the status line.
        let url = format!("{}/missing.log", serve_once("404 Not Found", b""));
        ws.update_in(cx, |ws, window, cx| ws.open_url(url, window, cx));
        for _ in 0..500 {
            cx.run_until_parked();
            if ws.read_with(cx, |ws, _| ws.downloading.is_none()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let status = ws.read_with(cx, |ws, _| ws.status());
        assert!(
            status.contains("Cannot download") && status.contains("404"),
            "{status}"
        );
        ws.update_in(cx, |ws, window, cx| {
            ws.open_url("ftp://x/y".into(), window, cx)
        });
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("not an http")));
        let _ = std::fs::remove_dir_all(Workspace::temp_root());
    }

    #[gpui::test]
    fn new_window_and_its_session(cx: &mut TestAppContext) {
        let fx = Fixture::new("windows");
        let (a, b) = (fx.log("a.log", 2), fx.log("b.log", 3));
        let (ws, cx) = workspace(cx, fx.config());
        ws.update_in(cx, |ws, window, cx| ws.open(a.clone(), window, cx));
        wait_indexed(&ws, cx);

        // File > New Window opens an empty one.
        cx.dispatch_action(NewWindow);
        cx.run_until_parked();
        let ws2 = cx.update(|_, cx| {
            let live = MainWindows::live(cx);
            assert_eq!(live.len(), 2);
            live[1].clone()
        });
        assert!(ws2.read_with(cx, |ws, _| ws.tabs.is_empty()));
        ws2.update_in(cx, |ws, window, cx| ws.open(b.clone(), window, cx));
        wait_indexed(&ws2, cx);

        // Each window's size and place is saved with its tabs.
        let saved = Session::load(&fx.config()).all_windows();
        assert!(
            saved
                .iter()
                .all(|w| w.geometry.is_some_and(|g| g.width > 0.))
        );
        // Each window's tabs, in order; the recent files of both.
        let paths = |w: &WindowFiles| {
            w.open_files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
        };
        let saved = Session::load(&fx.config());
        let windows = saved.all_windows();
        assert_eq!(
            windows.iter().map(paths).collect::<Vec<_>>(),
            [vec![a.clone()], vec![b.clone()]]
        );
        assert_eq!(saved.recent_files.files(), [b.clone(), a.clone()]);
        // The first window's menus show the second's recent file once it is
        // active again.
        ws.update(cx, |ws, cx| ws.reload_session(cx));
        assert_eq!(recent(&ws, cx), [b.clone(), a.clone()]);

        // Closing a window while another is open forgets its tabs.
        let second = cx.update(|_, cx| cx.windows()[1]);
        cx.update(|_, cx| {
            second
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        });
        drop(ws2);
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| MainWindows::live(cx)).len(), 1);
        let windows = Session::load(&fx.config()).all_windows();
        assert_eq!(
            windows.iter().map(paths).collect::<Vec<_>>(),
            [vec![a.clone()]]
        );
    }

    #[test]
    fn long_folders_are_shortened() {
        assert_eq!(
            path_label(Path::new("/var/log/app.log")),
            "app.log — /var/log"
        );
        let deep = format!("/{}/app.log", "d".repeat(100));
        let label = path_label(Path::new(&deep));
        assert_eq!(label.chars().count(), "app.log — ".chars().count() + 59);
        assert!(label.starts_with("app.log — /ddd") && label.contains('…'));
    }

    #[gpui::test]
    fn favorites(cx: &mut TestAppContext) {
        let fx = Fixture::new("favorites");
        let (a, b) = (fx.log("a.log", 2), fx.log("b.log", 3));
        let (ws, cx) = workspace(cx, fx.config());
        let menu = |cx: &mut VisualTestContext| {
            ws.read_with(cx, |ws, _| ws.favorites_menu())
                .items
                .into_iter()
                .filter_map(|i| match i {
                    MenuItem::Action { name, disabled, .. } => Some((name.to_string(), disabled)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // Nothing to add or remove without a file.
        assert_eq!(
            menu(cx),
            [
                ("Add to Favorites".into(), true),
                ("Remove from Favorites…".into(), true)
            ]
        );
        for path in [&b, &a] {
            ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
            wait_indexed(&ws, cx);
            cx.dispatch_action(AddToFavorites);
        }
        // Sorted, and the current file can't be added twice.
        let items = menu(cx);
        assert_eq!(items[0], ("Add to Favorites".into(), true));
        assert_eq!(items[2].0, path_label(&a));
        assert_eq!(items[3].0, path_label(&b));
        assert_eq!(
            Session::load(&fx.config()).favorites,
            [a.clone(), b.clone()]
        );

        // Remove from Favorites… offers the current file first.
        cx.dispatch_action(RemoveFromFavorites);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(
            Session::load(&fx.config()).favorites,
            std::slice::from_ref(&b)
        );

        // A favorite that disappeared: asked, then removed.
        ws.update_in(cx, |ws, window, cx| ws.activate(0, window, cx));
        cx.dispatch_action(CloseAllTabs);
        std::fs::remove_file(&b).unwrap();
        cx.dispatch_action(OpenFavorite { path: b.clone() });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Yes");
        cx.run_until_parked();
        assert!(Session::load(&fx.config()).favorites.is_empty());
        assert!(ws.read_with(cx, |ws, _| ws.session.favorites.is_empty()));
    }

    #[gpui::test]
    fn files_from_another_launch(cx: &mut TestAppContext) {
        let fx = Fixture::new("launch");
        let (a, b) = (fx.log("a.log", 2), fx.log("b.log", 3));
        let (ws, cx) = workspace(cx, fx.config());
        let config = fx.config();
        // From a task, outside any window's update (as `main` does).
        cx.cx
            .update(|cx| open_from_another_launch(vec![a.clone()], config.clone(), cx));
        assert_eq!(wait_indexed(&ws, cx), 2);

        // Into the window active last.
        cx.dispatch_action(NewWindow);
        cx.run_until_parked();
        let ws2 = cx.update(|_, cx| MainWindows::live(cx)[1].clone());
        cx.cx.update(|cx| {
            cx.default_global::<MainWindows>().last_active = Some(ws2.downgrade());
            open_from_another_launch(vec![b.clone()], config.clone(), cx);
        });
        assert_eq!(wait_indexed(&ws2, cx), 3);
        assert_eq!(ws.read_with(cx, |ws, _| ws.tabs.len()), 1);
    }

    #[gpui::test]
    fn help_menu(cx: &mut TestAppContext) {
        let fx = Fixture::new("help");
        let (_ws, cx) = workspace(cx, fx.config());
        let labels = menu_labels(cx);
        for label in ["Documentation", "Report Issue…", "About kloggr"] {
            assert!(labels.contains(&label.to_owned()), "{label}");
        }
        // Documentation opens one window, and shows it again after that.
        let windows = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.windows().len());
        assert_eq!(windows(cx), 1);
        cx.dispatch_action(help::ShowDocumentation);
        cx.run_until_parked();
        assert_eq!(windows(cx), 2);
        let docs = cx.update(|_, cx| cx.windows()[1]);
        assert!(cx.update(|_, cx| help::is_documentation_window(docs, cx)));
        cx.dispatch_action(help::ShowDocumentation);
        cx.run_until_parked();
        assert_eq!(windows(cx), 2);
        assert!(help::GUIDE.contains("## Edit menu"));
    }

    #[gpui::test]
    fn encoding_menu(cx: &mut TestAppContext) {
        let fx = Fixture::new("encoding");
        let path = fx.dir.join("utf8.log");
        std::fs::write(&path, "caf\u{e9}\nx\n").unwrap();
        let (ws, cx) = workspace(cx, fx.config());
        ws.update_in(cx, |ws, window, cx| ws.open(path.clone(), window, cx));
        assert_eq!(wait_indexed(&ws, cx), 2);
        let crawler = ws.read_with(cx, |ws, _| ws.crawler().unwrap().clone());
        let line0 = |cx: &mut VisualTestContext| {
            crawler.read_with(cx, |c, _| {
                c.log().unwrap().line_text(0).unwrap().into_owned()
            })
        };
        assert!(ws.read_with(cx, |ws, _| ws.status().contains("detected as UTF-8")));
        crawler.update(cx, |c, cx| c.toggle_marks_for_test(1, cx));

        // A single-byte encoding: decoded again, the marks stay.
        cx.dispatch_action(SetEncoding {
            name: Some("windows-1252".into()),
        });
        cx.run_until_parked();
        assert_eq!(line0(cx), "caf\u{c3}\u{a9}");
        assert!(ws.read_with(cx, |ws, _| {
            ws.status().contains("displayed as windows-1252")
        }));
        assert!(crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().contains(1)));
        assert_eq!(
            Session::load(&fx.config()).open_files[0]
                .context
                .encoding
                .as_deref(),
            Some("windows-1252")
        );

        // UTF-16 lays the lines out differently: indexed again.
        cx.dispatch_action(SetEncoding {
            name: Some("UTF-16LE".into()),
        });
        cx.run_until_parked();
        assert_eq!(wait_indexed(&ws, cx), 1);
        assert!(crawler.read_with(cx, |c, cx| c.filter().read(cx).marks().is_empty()));

        // Auto goes back to the detected encoding.
        cx.dispatch_action(SetEncoding { name: None });
        cx.run_until_parked();
        assert_eq!(line0(cx), "caf\u{e9}");

        // Remembered for the file in the session.
        cx.dispatch_action(SetEncoding {
            name: Some("ISO-8859-15".into()),
        });
        cx.run_until_parked();
        let config = fx.config();
        forget_windows(cx);
        let ws2 = cx.update(|window, cx| {
            cx.new(|cx| {
                let mut ws = Workspace::new(Some(config), window, cx);
                ws.restore_session(window, cx);
                ws
            })
        });
        assert_eq!(wait_indexed(&ws2, cx), 2);
        let restored = ws2.read_with(cx, |ws, cx| {
            ws.crawler()
                .unwrap()
                .read(cx)
                .log()
                .unwrap()
                .encoding()
                .name()
        });
        assert_eq!(restored, "ISO-8859-15");
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
        forget_windows(cx);
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
