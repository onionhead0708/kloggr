//! Watching a log file for changes (klogg: `filewatch/`, efsw plus
//! optional polling).
//!
//! The watcher only says "this file may have changed"; what changed is up
//! to [`LogFile::check`](crate::LogFile::check). It watches the file's
//! directory, not the file, so a log that is rotated (renamed, then
//! recreated under the same name) keeps being followed. When the native
//! watcher can't start (e.g. out of inotify watches), it falls back to
//! polling every [`POLL_INTERVAL`] (klogg's default interval).

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, RecursiveMode};

/// klogg: `pollIntervalMs_`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
#[error("cannot watch {path}: {source}")]
pub struct WatchError {
    path: PathBuf,
    #[source]
    source: notify::Error,
}

/// Watches one file while alive.
pub struct Watcher {
    _inner: Box<dyn notify::Watcher + Send>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

impl Watcher {
    /// Calls `on_change`, from a background thread, whenever `path` may have
    /// changed: written, created, removed or renamed.
    pub fn new(
        path: &Path,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, WatchError> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let name = path.file_name().map(|n| n.to_os_string());
        let on_change = std::sync::Arc::new(on_change);
        let handler = {
            let on_change = on_change.clone();
            move |event: notify::Result<Event>| {
                let Ok(event) = event else {
                    // Errors (e.g. a queue overflow): check anyway.
                    on_change();
                    return;
                };
                let relevant = event.need_rescan()
                    || event
                        .paths
                        .iter()
                        .any(|p| p.file_name().map(|n| n.to_os_string()) == name);
                if relevant && is_change(&event.kind) {
                    on_change();
                }
            }
        };
        let err = |source| WatchError {
            path: path.clone(),
            source,
        };
        let mut inner: Box<dyn notify::Watcher + Send> =
            match notify::recommended_watcher(handler.clone()) {
                Ok(w) => Box::new(w),
                Err(e) => {
                    tracing::warn!("native file watching unavailable ({e}), polling");
                    Box::new(poll_watcher(handler.clone()).map_err(err)?)
                }
            };
        if let Err(e) = inner.watch(&dir, RecursiveMode::NonRecursive) {
            tracing::warn!("cannot watch {} natively ({e}), polling", dir.display());
            inner = Box::new(poll_watcher(handler).map_err(err)?);
            inner
                .watch(&dir, RecursiveMode::NonRecursive)
                .map_err(err)?;
        }
        Ok(Self { _inner: inner })
    }
}

fn poll_watcher(handler: impl notify::EventHandler) -> notify::Result<notify::PollWatcher> {
    let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
    notify::PollWatcher::new(handler, config)
}

/// Whether an event can mean new or different content. Opening or reading
/// the file (e.g. by rlogg itself) doesn't.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn wait(rx: &mpsc::Receiver<()>) -> bool {
        let got = rx.recv_timeout(Duration::from_secs(5)).is_ok();
        // Drain the burst of events of one change.
        while rx.recv_timeout(Duration::from_millis(100)).is_ok() {}
        got
    }

    #[test]
    fn sees_appends_and_rotation() {
        let dir = std::env::temp_dir().join(format!("rlogg-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        std::fs::write(&path, "a\n").unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let _w = Watcher::new(&path, move || {
            tx.lock().unwrap().send(()).ok();
        })
        .unwrap();

        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"b\n")
            .unwrap();
        assert!(wait(&rx), "append");

        // Another file in the directory: ignored.
        std::fs::write(dir.join("other.log"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        // Rotation: renamed away, then recreated.
        std::fs::rename(&path, dir.join("app.log.1")).unwrap();
        assert!(wait(&rx), "rename");
        std::fs::write(&path, "new\n").unwrap();
        assert!(wait(&rx), "recreate");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
