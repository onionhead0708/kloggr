//! Persistent session state: recent files and the files open in the window
//! with their per-file state (more settings later).
//!
//! klogg keeps these in `QSettings` INI files (`settings/src/persistentinfo.cpp`);
//! kloggr uses one TOML file, `session.toml`, in the platform config directory
//! (`~/.config/kloggr` on Linux). `KLOGGR_CONFIG_DIR` overrides the directory,
//! e.g. for a portable install or tests.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most recent files remembered (klogg: `MAX_RECENT_FILES`).
pub const MAX_RECENT_FILES: usize = 25;
/// Recent files shown in the menu by default (klogg:
/// `DEFAULT_MAX_ITEMS_TO_SHOW`).
pub const DEFAULT_RECENT_SHOWN: usize = 5;

const SESSION_FILE: &str = "session.toml";
const SESSION_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot serialize settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Recently opened files, most recent first (klogg: `RecentFiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentFiles {
    files: Vec<PathBuf>,
    /// How many to show in the menu, 1..=[`MAX_RECENT_FILES`].
    shown: usize,
}

impl Default for RecentFiles {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            shown: DEFAULT_RECENT_SHOWN,
        }
    }
}

impl RecentFiles {
    /// All remembered files, most recent first.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The files to show in the menu (klogg: `getNumberItemsToShow`).
    pub fn shown(&self) -> &[PathBuf] {
        &self.files[..self.files.len().min(self.shown)]
    }

    pub fn set_shown(&mut self, n: usize) {
        if n > 0 {
            self.shown = n.min(MAX_RECENT_FILES);
        }
    }

    /// Moves `path` to the front (klogg: `addRecent`). Like klogg, this also
    /// drops files that no longer exist.
    pub fn add(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        self.remove(&path);
        self.files.insert(0, path);
        self.files.truncate(MAX_RECENT_FILES);
    }

    /// Removes `path`, and any file that no longer exists (klogg:
    /// `removeRecent`).
    pub fn remove(&mut self, path: &Path) {
        self.files.retain(|f| f != path && f.exists());
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Restores invariants after loading a hand-edited or older file.
    fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.files.retain(|f| seen.insert(f.clone()));
        self.files.truncate(MAX_RECENT_FILES);
        self.shown = self.shown.clamp(1, MAX_RECENT_FILES);
    }
}

/// Per-file state kept in the session (klogg: `CrawlerWidgetContext`,
/// saved as the "view context" of each open file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileContext {
    pub match_case: bool,
    pub regex: bool,
    pub inverse: bool,
    pub boolean: bool,
    pub auto_refresh: bool,
    pub follow: bool,
    /// Marked lines, 0-based.
    pub marks: Vec<u64>,
    /// Encoding chosen in the Encoding menu (its name); `None`: detected.
    pub encoding: Option<String>,
    /// Height of the filtered view in pixels (klogg: the splitter `sizes`
    /// of the view context); `None`: the default.
    pub filtered_height: Option<u32>,
}

impl Default for FileContext {
    /// klogg's defaults for a new file.
    fn default() -> Self {
        Self {
            match_case: true,
            regex: true,
            inverse: false,
            boolean: false,
            auto_refresh: false,
            follow: false,
            marks: Vec::new(),
            encoding: None,
            filtered_height: None,
        }
    }
}

/// Frozen columns of one view (klogg: `FrozenColumns::ViewState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FrozenView {
    pub enabled: bool,
    /// Columns frozen when enabled; kept while disabled.
    pub column: usize,
}

/// Frozen columns of a file's main and filtered views (klogg:
/// `FrozenColumns::FileState`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenFile {
    pub path: PathBuf,
    #[serde(default)]
    pub main: FrozenView,
    #[serde(default)]
    pub filtered: FrozenView,
}

/// Files whose frozen columns are remembered (klogg: `MAX_FILES`).
pub const MAX_FROZEN_FILES: usize = 500;

/// A name given to a file's tab (klogg: `TabNameMapping::TabName`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabName {
    pub path: PathBuf,
    pub name: String,
}

/// A file open when the session was saved (klogg: `SessionInfo::OpenFile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenFile {
    pub path: PathBuf,
    #[serde(default)]
    pub context: FileContext,
}

/// The tabs of one window (klogg: a window of `SessionInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFiles {
    /// Index in `open_files` of the current tab.
    pub current_file: usize,
    pub open_files: Vec<OpenFile>,
}

/// Everything stored in `session.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    version: u32,
    /// Index in `open_files` of the current tab. (Plain values come before
    /// tables in TOML.)
    pub current_file: usize,
    /// Favorite files, by path (klogg: `FavoriteFiles`, sorted by name).
    pub favorites: Vec<PathBuf>,
    pub recent_files: RecentFiles,
    /// Files open in the window, in tab order, reopened at startup (klogg:
    /// `loadLastSession`, on by default).
    pub open_files: Vec<OpenFile>,
    /// Frozen columns per file, most recently changed first, kept after the
    /// file is closed (klogg: `FrozenColumns`).
    pub frozen_columns: Vec<FrozenFile>,
    /// Tab names chosen by the user, by file (klogg: `TabNameMapping`).
    pub tab_names: Vec<TabName>,
    /// The windows after the first, whose tabs are `open_files` (klogg:
    /// "Enable multiple windows").
    pub windows: Vec<WindowFiles>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            recent_files: RecentFiles::default(),
            favorites: Vec::new(),
            open_files: Vec::new(),
            current_file: 0,
            frozen_columns: Vec::new(),
            tab_names: Vec::new(),
            windows: Vec::new(),
        }
    }
}

impl Session {
    /// Loads the session from `dir`. A missing file gives the defaults; an
    /// unreadable or unknown-version file is logged and ignored, as klogg
    /// does, so a bad file never prevents startup.
    pub fn load(dir: &Path) -> Self {
        match load_toml::<Self>(dir, SESSION_FILE) {
            Some(mut session) if session.version == SESSION_VERSION => {
                session.recent_files.sanitize();
                session.favorites.sort();
                session.favorites.dedup();
                session
            }
            Some(session) => {
                tracing::warn!(
                    "ignoring {SESSION_FILE}: unknown version {}",
                    session.version
                );
                Self::default()
            }
            None => Self::default(),
        }
    }

    /// Saves to `dir`, creating it if needed.
    pub fn save(&self, dir: &Path) -> Result<(), SettingsError> {
        save_toml(self, dir, SESSION_FILE)
    }

    /// Adds `path` to the favorites, kept sorted (klogg: `FavoriteFiles::add`).
    pub fn add_favorite(&mut self, path: &Path) {
        if let Err(at) = self.favorites.binary_search_by(|f| f.as_path().cmp(path)) {
            self.favorites.insert(at, path.to_path_buf());
        }
    }

    pub fn remove_favorite(&mut self, path: &Path) {
        self.favorites.retain(|f| f != path);
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.favorites.iter().any(|f| f == path)
    }

    /// The tabs of every window, the first one first.
    pub fn all_windows(&self) -> Vec<WindowFiles> {
        let first = WindowFiles {
            current_file: self.current_file,
            open_files: self.open_files.clone(),
        };
        std::iter::once(first)
            .chain(self.windows.iter().cloned())
            .collect()
    }

    /// Stores the tabs of every window, the first one first.
    pub fn set_windows(&mut self, windows: Vec<WindowFiles>) {
        let mut windows = windows.into_iter();
        let first = windows.next().unwrap_or_default();
        self.current_file = first.current_file;
        self.open_files = first.open_files;
        self.windows = windows.collect();
    }

    /// The name given to `path`'s tab, if any (klogg: `tabName`).
    pub fn tab_name(&self, path: &Path) -> Option<&str> {
        self.tab_names
            .iter()
            .find(|t| t.path == path)
            .map(|t| t.name.as_str())
    }

    /// Names `path`'s tab; an empty name goes back to the file name
    /// (klogg: `setTabName`).
    pub fn set_tab_name(&mut self, path: &Path, name: &str) {
        let name = name.trim();
        match self.tab_names.iter_mut().find(|t| t.path == path) {
            Some(_) if name.is_empty() => self.tab_names.retain(|t| t.path != path),
            Some(t) => t.name = name.to_owned(),
            None if !name.is_empty() => self.tab_names.push(TabName {
                path: path.to_path_buf(),
                name: name.to_owned(),
            }),
            None => {}
        }
    }

    /// The frozen columns of `path`'s main and filtered views (klogg:
    /// `fileState`); all off for a file never frozen.
    pub fn frozen_columns(&self, path: &Path) -> (FrozenView, FrozenView) {
        self.frozen_columns
            .iter()
            .find(|f| f.path == path)
            .map_or_default(|f| (f.main, f.filtered))
    }

    /// Remembers `path`'s frozen columns (klogg: `setFileState`): first in
    /// the list, dropping files that no longer exist; a file with both
    /// views unfrozen is forgotten.
    pub fn set_frozen_columns(&mut self, path: &Path, main: FrozenView, filtered: FrozenView) {
        self.frozen_columns
            .retain(|f| f.path != path && f.path.exists());
        if main.enabled || filtered.enabled {
            self.frozen_columns.insert(
                0,
                FrozenFile {
                    path: path.to_path_buf(),
                    main,
                    filtered,
                },
            );
            self.frozen_columns.truncate(MAX_FROZEN_FILES);
        }
    }
}

/// Reads `dir/name` as TOML. `None` if it is missing (silently) or
/// unreadable or invalid (logged).
pub(crate) fn load_toml<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    let path = dir.join(name);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            return None;
        }
    };
    toml::from_str(&text)
        .inspect_err(|e| tracing::warn!("ignoring {}: {e}", path.display()))
        .ok()
}

/// Writes `value` to `dir/name` as TOML, creating `dir` if needed. Writes a
/// temporary file and renames it, so a crash never leaves a truncated file.
pub(crate) fn save_toml<T: Serialize>(
    value: &T,
    dir: &Path,
    name: &str,
) -> Result<(), SettingsError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| SettingsError::Io { path, source }
    };
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let path = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, toml::to_string_pretty(value)?).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io(&path))
}

/// Directory for kloggr's settings: `$KLOGGR_CONFIG_DIR`, else the platform
/// config directory.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KLOGGR_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let dir = directories::ProjectDirs::from("", "", "kloggr")?
        .config_dir()
        .to_path_buf();
    // The project was called rlogg until 2026-11-10: keep its settings.
    if let Some(old) = directories::ProjectDirs::from("", "", "rlogg") {
        move_old_dir(old.config_dir(), &dir);
    }
    Some(dir)
}

/// Renames `old` to `new` when only `old` exists.
fn move_old_dir(old: &Path, new: &Path) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(old, new);
}

#[cfg(test)]
mod tests {

    #[test]
    fn moves_the_old_settings_once() {
        let base = std::env::temp_dir().join(format!("kloggr-move-{}", std::process::id()));
        let (old, new) = (base.join("rlogg"), base.join("kloggr"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("session.toml"), "x").unwrap();
        move_old_dir(&old, &new);
        assert!(!old.exists());
        assert_eq!(
            std::fs::read_to_string(new.join("session.toml")).unwrap(),
            "x"
        );
        // An existing new directory is never replaced.
        std::fs::create_dir_all(&old).unwrap();
        move_old_dir(&old, &new);
        assert!(old.exists());
        std::fs::remove_dir_all(base).unwrap();
    }
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kloggr-settings-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// Creates an empty file and returns its path.
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"").unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn add_moves_to_front_without_duplicates() {
        let dir = TempDir::new("order");
        let (a, b, c) = (dir.file("a"), dir.file("b"), dir.file("c"));
        let mut recent = RecentFiles::default();
        recent.add(&a);
        recent.add(&b);
        recent.add(&c);
        recent.add(&a);
        assert_eq!(recent.files(), [a, c, b]);
    }

    #[test]
    fn add_prunes_missing_files_and_caps_length() {
        let dir = TempDir::new("prune");
        let mut recent = RecentFiles::default();
        let gone = dir.file("gone");
        recent.add(&gone);
        std::fs::remove_file(&gone).unwrap();
        let files: Vec<_> = (0..30).map(|i| dir.file(&format!("f{i}"))).collect();
        for f in &files {
            recent.add(f);
        }
        assert_eq!(recent.files().len(), MAX_RECENT_FILES);
        assert_eq!(recent.files()[0], files[29]);
        assert!(!recent.files().contains(&gone));
    }

    #[test]
    fn shown_is_limited() {
        let dir = TempDir::new("shown");
        let mut recent = RecentFiles::default();
        for i in 0..8 {
            recent.add(dir.file(&i.to_string()));
        }
        assert_eq!(recent.shown().len(), DEFAULT_RECENT_SHOWN);
        recent.set_shown(0);
        assert_eq!(recent.shown().len(), DEFAULT_RECENT_SHOWN);
        recent.set_shown(100);
        assert_eq!(recent.shown().len(), 8);
        recent.clear();
        assert!(recent.shown().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new("roundtrip");
        let mut session = Session::default();
        session.recent_files.add(dir.file("x.log"));
        session.recent_files.add(dir.file("y.log"));
        session.save(&dir.0.join("conf")).unwrap();
        assert_eq!(Session::load(&dir.0.join("conf")), session);
    }

    #[test]
    fn open_files_round_trip() {
        let dir = TempDir::new("open-files");
        let session = Session {
            current_file: 1,
            open_files: vec![
                OpenFile {
                    path: "/var/log/a.log".into(),
                    context: FileContext::default(),
                },
                OpenFile {
                    path: "/var/log/b.log".into(),
                    context: FileContext {
                        match_case: false,
                        follow: true,
                        marks: vec![3, 7],
                        ..FileContext::default()
                    },
                },
            ],
            ..Session::default()
        };
        session.save(&dir.0).unwrap();
        assert_eq!(Session::load(&dir.0), session);
        // A session from before tabs: no open files.
        std::fs::write(dir.0.join(SESSION_FILE), "version = 1\n").unwrap();
        assert!(Session::load(&dir.0).open_files.is_empty());
    }

    #[test]
    fn favorites() {
        let dir = TempDir::new("favorites");
        let mut session = Session::default();
        for p in [
            "/var/log/b.log",
            "/var/log/a.log",
            "/srv/c.log",
            "/var/log/a.log",
        ] {
            session.add_favorite(Path::new(p));
        }
        let expected: Vec<PathBuf> = ["/srv/c.log", "/var/log/a.log", "/var/log/b.log"]
            .map(PathBuf::from)
            .into();
        assert_eq!(session.favorites, expected);
        assert!(session.is_favorite(Path::new("/srv/c.log")));
        session.save(&dir.0).unwrap();
        assert_eq!(Session::load(&dir.0).favorites, expected);
        session.remove_favorite(Path::new("/srv/c.log"));
        assert!(!session.is_favorite(Path::new("/srv/c.log")));
        assert_eq!(session.favorites.len(), 2);
    }

    #[test]
    fn windows_round_trip() {
        let dir = TempDir::new("windows");
        let file = |p: &str| OpenFile {
            path: p.into(),
            context: FileContext::default(),
        };
        let windows = vec![
            WindowFiles {
                current_file: 1,
                open_files: vec![file("/a.log"), file("/b.log")],
            },
            WindowFiles {
                current_file: 0,
                open_files: vec![file("/c.log")],
            },
        ];
        let mut session = Session::default();
        session.set_windows(windows.clone());
        // The first window is where a session without windows has it.
        assert_eq!(session.open_files, windows[0].open_files);
        assert_eq!(session.current_file, 1);
        session.save(&dir.0).unwrap();
        let loaded = Session::load(&dir.0);
        assert_eq!(loaded.all_windows(), windows);
        session.set_windows(Vec::new());
        assert_eq!(session.all_windows(), [WindowFiles::default()]);
    }

    #[test]
    fn bad_files_load_as_default() {
        let dir = TempDir::new("bad");
        assert_eq!(Session::load(&dir.0), Session::default());
        std::fs::write(dir.0.join(SESSION_FILE), "not = [valid").unwrap();
        assert_eq!(Session::load(&dir.0), Session::default());
        std::fs::write(dir.0.join(SESSION_FILE), "version = 99\n").unwrap();
        assert_eq!(Session::load(&dir.0), Session::default());
    }

    #[test]
    fn load_sanitizes() {
        let dir = TempDir::new("sanitize");
        std::fs::write(
            dir.0.join(SESSION_FILE),
            "version = 1\n[recent_files]\nfiles = [\"/a\", \"/a\", \"/b\"]\nshown = 0\n",
        )
        .unwrap();
        let recent = Session::load(&dir.0).recent_files;
        assert_eq!(recent.files(), [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(recent.shown().len(), 1);
    }

    #[test]
    fn frozen_columns_per_file() {
        let dir = TempDir::new("frozen");
        let (a, b) = (dir.file("a.log"), dir.file("b.log"));
        let mut session = Session::default();
        let on = |column| FrozenView {
            enabled: true,
            column,
        };
        assert_eq!(session.frozen_columns(&a), Default::default());
        session.set_frozen_columns(&a, on(10), FrozenView::default());
        session.set_frozen_columns(&b, FrozenView::default(), on(3));
        assert_eq!(session.frozen_columns(&a).0, on(10));
        assert_eq!(session.frozen_columns(&b).1, on(3));
        assert_eq!(session.frozen_columns[0].path, b, "most recent first");
        // Saved and loaded with the session.
        session.save(&dir.0).unwrap();
        assert_eq!(Session::load(&dir.0), session);
        // Unfreezing both views forgets the file; the column is not kept.
        session.set_frozen_columns(
            &a,
            FrozenView {
                enabled: false,
                column: 10,
            },
            FrozenView::default(),
        );
        assert_eq!(session.frozen_columns.len(), 1);
        // Missing files are dropped on the next change.
        std::fs::remove_file(&b).unwrap();
        session.set_frozen_columns(&a, on(1), FrozenView::default());
        assert_eq!(session.frozen_columns.len(), 1);
        assert_eq!(session.frozen_columns[0].path, a);
    }

    #[test]
    fn tab_names() {
        let dir = TempDir::new("tab-names");
        let mut session = Session::default();
        let a = Path::new("/var/log/a.log");
        assert_eq!(session.tab_name(a), None);
        session.set_tab_name(a, "  API server ");
        assert_eq!(session.tab_name(a), Some("API server"));
        session.set_tab_name(a, "API");
        assert_eq!(session.tab_names.len(), 1);
        session.save(&dir.0).unwrap();
        assert_eq!(Session::load(&dir.0).tab_name(a), Some("API"));
        session.set_tab_name(a, "");
        assert_eq!(session.tab_name(a), None);
        session.set_tab_name(Path::new("/x"), " ");
        assert!(session.tab_names.is_empty());
    }
}
