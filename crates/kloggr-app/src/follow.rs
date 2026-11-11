//! Live files: reloading a file that changes on disk, and follow mode
//! (klogg: `LogData::fileChangedOnDisk`, `CrawlerWidget::fileChangedHandler`,
//! `loadingFinishedHandler`, and the "Follow File" action).
//!
//! A [`Watcher`] reports that the file may have changed; a worker thread then
//! [checks](LogFile::check) it and makes a new snapshot:
//! - appended data: the new snapshot shares the old index and the views keep
//!   their place; with auto-refresh on, the new lines are searched;
//! - truncated, rewritten or rotated: the file is indexed again, and marks
//!   and search results are dropped (klogg does the same); with auto-refresh
//!   on, the search runs again.
//!
//! View > Reload (F5) indexes the file again the same way (klogg:
//! `CrawlerWidget::reload`), and View > Stop (Esc) cancels a search or the
//! indexing (klogg: `stopLoading`). Indexing shows its progress in the status
//! line.
//!
//! In follow mode both views stay at the bottom. Scrolling up or jumping
//! elsewhere leaves it; Shift+G or scrolling down at the bottom comes back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, Task, actions};
use kloggr_core::file::OpenError;
use kloggr_core::search::FilteredLines;
use kloggr_core::{FileChange, LogFile, OpenOptions, Watcher};

use crate::background;
use crate::crawler::{self, Crawler, CrawlerEvent, SearchInfo};
use crate::preferences::Preferences;
use crate::shortcuts::Shortcut;

actions!(follow, [ToggleFollow, Reload, Stop]);

/// klogg: `MainWindowFollowFile` (F and F10), `MainWindowReload`
/// (`QKeySequence::Refresh`) and `MainWindowStop` (`QKeySequence::Cancel`).
pub fn shortcuts() -> Vec<Shortcut> {
    vec![
        Shortcut::new(
            "mainwindow.follow_file",
            "Follow file",
            Some(crawler::CONTEXT),
            &["f", "f10"],
            ToggleFollow,
        ),
        Shortcut::new(
            "mainwindow.reload",
            "Reload file",
            None,
            &["f5", "secondary-r"],
            Reload,
        ),
        Shortcut::new(
            "mainwindow.stop",
            "Stop file loading",
            None,
            &["escape"],
            Stop,
        ),
    ]
}

#[cfg(test)]
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    crate::shortcuts::default_bindings(shortcuts())
}

/// The watcher of the open file and the task that applies its changes.
#[derive(Default)]
pub struct Watch {
    _watcher: Option<Watcher>,
    /// Wakes the reload task; tests send on it directly.
    pub(crate) changed: Option<async_channel::Sender<()>>,
    _task: Option<Task<()>>,
}

/// The file is indexed again from the start (klogg: `FullReindexOperation`).
pub(crate) struct Reindex {
    /// Progress shown in the status line.
    pub(crate) percent: u8,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Reindex {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

enum ReindexEvent {
    Progress(u8),
    Done(Result<Arc<LogFile>, OpenError>),
}

/// What changed on disk.
enum Change {
    /// A newer snapshot with the appended data.
    Grew(Arc<LogFile>),
    /// The file must be indexed again.
    Replaced,
}

/// Checks the file and, if it grew, makes the new snapshot (on a worker
/// thread).
fn reload(log: &LogFile) -> Option<Change> {
    match log.check() {
        Ok(FileChange::Unchanged) => None,
        // Gone, e.g. between the two steps of a rotation: the next event
        // will tell.
        Err(_) => None,
        Ok(FileChange::Grew) => log
            .reload_grown(&AtomicBool::new(false), |_| {})
            .inspect_err(|e| tracing::warn!("cannot reload {}: {e}", log.path().display()))
            .ok()
            .map(|l| Change::Grew(Arc::new(l))),
        Ok(FileChange::Replaced) => Some(Change::Replaced),
    }
}

impl Crawler {
    /// Watches the open file for changes; replaces the previous watch.
    pub(crate) fn start_watching(&mut self, cx: &mut Context<Self>) {
        self.watch = Watch::default();
        let Some(log) = &self.log else { return };
        let (tx, rx) = async_channel::unbounded::<()>();
        // GPUI's test scheduler can't be woken from the watcher's thread:
        // tests send changes through `watch.changed`.
        #[cfg(not(test))]
        {
            let tx = tx.clone();
            match Watcher::new(log.path(), move || {
                tx.try_send(()).ok();
            }) {
                Ok(w) => self.watch._watcher = Some(w),
                Err(e) => tracing::warn!("{e}; the file won't be reloaded"),
            }
        }
        #[cfg(test)]
        let _ = log;
        self.watch.changed = Some(tx);
        self.watch._task = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                // One check covers a burst of events.
                while rx.try_recv().is_ok() {}
                let Ok((log, reindexing)) =
                    this.read_with(cx, |c, _| (c.log.clone(), c.reindex.is_some()))
                else {
                    break;
                };
                // A running reindex checks again when it is done.
                let Some(log) = log.filter(|_| !reindexing) else {
                    continue;
                };
                let executor = cx.background_executor().clone();
                let result =
                    background::run_blocking("kloggr-reload", &executor, move || reload(&log))
                        .await
                        .flatten();
                if let Some(result) = result
                    && this.update(cx, |c, cx| c.apply_reload(result, cx)).is_err()
                {
                    break;
                }
                // At most ~10 reloads a second for a busy log.
                #[cfg(not(test))]
                executor.timer(std::time::Duration::from_millis(100)).await;
            }
        }));
    }

    fn apply_reload(&mut self, change: Change, cx: &mut Context<Self>) {
        match change {
            Change::Grew(log) => {
                cx.emit(CrawlerEvent::NewData);
                self.log = Some(log.clone());
                for view in [&self.main, &self.filtered] {
                    let log = log.clone();
                    view.update(cx, |v, cx| v.update_log(log, cx));
                }
                self.refresh_search(cx);
            }
            Change::Replaced => self.reindex(cx),
        }
        cx.notify();
    }

    /// Indexes the file again from the start, with progress in the status
    /// line (View > Reload, or the file was replaced). The views keep the
    /// old snapshot until it is done.
    pub(crate) fn reindex(&mut self, cx: &mut Context<Self>) {
        let Some(log) = self.log.clone() else { return };
        let path = log.path().to_path_buf();
        let options = OpenOptions {
            encoding: self.encoding,
            hide_ansi: Preferences::get(cx).view.hide_ansi,
        };
        let (tx, rx) = async_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        background::spawn_blocking(
            "kloggr-reindex",
            move || {
                let mut last = None;
                let result = LogFile::open_with(&path, options, &thread_cancel, |p| {
                    // One event per percent: progress comes per block.
                    if last.replace(p.percent()) != Some(p.percent()) {
                        tx.send_blocking(ReindexEvent::Progress(p.percent())).ok();
                    }
                });
                tx.send_blocking(ReindexEvent::Done(result.map(Arc::new)))
                    .ok();
            },
            cx,
        );
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |c, cx| c.on_reindex_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // Replacing a running reindex cancels it.
        self.reindex = Some(Reindex {
            percent: 0,
            cancel,
            _task: task,
        });
        cx.emit(CrawlerEvent::Status);
        cx.notify();
    }

    fn on_reindex_event(&mut self, event: ReindexEvent, cx: &mut Context<Self>) {
        match event {
            ReindexEvent::Progress(p) => {
                if let Some(r) = &mut self.reindex {
                    r.percent = p;
                }
            }
            ReindexEvent::Done(result) => {
                self.reindex = None;
                match result {
                    Ok(log) => {
                        self.apply_replaced(log, cx);
                        cx.emit(CrawlerEvent::NewData);
                    }
                    Err(OpenError::Cancelled(_)) => {}
                    Err(e) => tracing::warn!("cannot reload: {e}"),
                }
                // The file may have changed again while it was indexed.
                if let Some(changed) = &self.watch.changed {
                    changed.try_send(()).ok();
                }
            }
        }
        cx.emit(CrawlerEvent::Status);
        cx.notify();
    }

    /// Shows a new snapshot of a replaced file. klogg (`fileChangedHandler`,
    /// `reload`): the marks and the search results are dropped;
    /// auto-refresh runs the search again.
    fn apply_replaced(&mut self, log: Arc<LogFile>, cx: &mut Context<Self>) {
        self.stop_search();
        self.drop_kept_results();
        self.filter.update(cx, |f, cx| {
            *f = FilteredLines::new();
            cx.notify();
        });
        self.log = Some(log.clone());
        self.current_line = 0;
        self.searched = None;
        // klogg: `loadingFinishedHandler` clears the search limits.
        self.set_search_limits(0..usize::MAX, cx);
        for view in [&self.main, &self.filtered] {
            let log = log.clone();
            view.update(cx, |v, cx| v.set_log(Some(log), cx));
        }
        let following = self.following;
        self.set_follow(following, cx);
        self.info = SearchInfo::default();
        match self.last_pattern.clone() {
            Some(pattern) if self.options.auto_refresh => self.run(pattern, cx),
            _ => self.last_pattern = None,
        }
    }

    /// View > Stop (klogg: `stopLoading`): stops the search, keeping the
    /// matches found so far, a Save to file, and the indexing. Returns whether indexing was
    /// stopped: the caller closes the tab, as klogg does.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) -> bool {
        self.stop_search();
        for view in [&self.main, &self.filtered] {
            view.update(cx, |v, _| v.cancel_save());
        }
        let stopped = self.reindex.take().is_some();
        cx.emit(CrawlerEvent::Status);
        cx.notify();
        stopped
    }

    /// Edit > Clear File (klogg: `MainWindow::clearLog`): empties the file
    /// on disk, then shows it again. Everything that reads the file lets go
    /// of it first: reading a mapping past the end of a truncated file
    /// crashes (SIGBUS), and Windows refuses to truncate a mapped file.
    pub(crate) fn clear_file(&mut self, cx: &mut Context<Self>) -> Task<std::io::Result<()>> {
        let Some(log) = self.log.take() else {
            return Task::ready(Ok(()));
        };
        let path = log.path().to_path_buf();
        let mapped = Arc::downgrade(&log);
        drop(log);
        self.stop(cx);
        self.qf.stop();
        self.set_log(None, cx);
        cx.emit(CrawlerEvent::Status);
        let options = OpenOptions {
            encoding: self.encoding,
            hide_ansi: Preferences::get(cx).view.hide_ansi,
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            // Cancelled workers stop at their next block.
            for _ in 0..250 {
                if mapped.strong_count() == 0 {
                    break;
                }
                executor.timer(std::time::Duration::from_millis(20)).await;
            }
            let busy = mapped.strong_count() > 0;
            let Some((cleared, log)) =
                background::run_blocking("kloggr-clear", &executor, move || {
                    let cleared = if busy {
                        Err(std::io::Error::other("the file is still being read"))
                    } else {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .open(&path)
                            .and_then(|f| f.set_len(0))
                    };
                    let log = LogFile::open_with(&path, options, &AtomicBool::new(false), |_| {});
                    (cleared, log)
                })
                .await
            else {
                return Err(std::io::Error::other("clearing the file failed"));
            };
            this.update(cx, |c, cx| {
                match log {
                    Ok(log) => c.set_log(Some(Arc::new(log)), cx),
                    Err(e) => tracing::warn!("cannot reopen after clearing: {e}"),
                }
                cx.emit(CrawlerEvent::Status);
            })
            .ok();
            cleared
        })
    }

    /// With auto-refresh on, searches the lines added since the last search
    /// (klogg: `updateSearch`). The last searched line is checked again: it
    /// may have been incomplete.
    pub(crate) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let (Some(pattern), Some(log), Some((lines, bytes))) =
            (self.last_pattern.clone(), &self.log, self.searched)
        else {
            return;
        };
        // More bytes, not only more lines: the last line may have grown.
        if self.options.auto_refresh && !self.is_searching() && log.bytes().len() > bytes {
            self.run_from(pattern, Some(lines.saturating_sub(1)), cx);
        }
    }

    /// Turns follow mode on or off in both views (klogg: `followSet`).
    pub(crate) fn set_follow(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.following != on {
            cx.emit(CrawlerEvent::Status);
        }
        self.following = on;
        for view in [&self.main, &self.filtered] {
            view.update(cx, |v, cx| v.set_following(on, cx));
        }
        cx.notify();
    }
}
