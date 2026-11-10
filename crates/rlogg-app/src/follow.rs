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
//! In follow mode both views stay at the bottom. Scrolling up or jumping
//! elsewhere leaves it; Shift+G or scrolling down at the bottom comes back.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::{Context, Task, actions};
use rlogg_core::search::FilteredLines;
use rlogg_core::{FileChange, LogFile, Watcher};

use crate::background;
use crate::crawler::{self, Crawler, CrawlerEvent, SearchInfo};
use crate::shortcuts::Shortcut;

actions!(follow, [ToggleFollow]);

/// klogg: `MainWindowFollowFile`, F and F10.
pub fn shortcuts() -> Vec<Shortcut> {
    vec![Shortcut::new(
        "mainwindow.follow_file",
        "Follow file",
        Some(crawler::CONTEXT),
        &["f", "f10"],
        ToggleFollow,
    )]
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

/// A newer snapshot of the file.
enum Reload {
    Grew(Arc<LogFile>),
    Replaced(Arc<LogFile>),
}

/// Checks the file and makes the new snapshot (on a worker thread).
fn reload(log: &LogFile) -> Option<Reload> {
    let never = AtomicBool::new(false);
    let result = match log.check() {
        Ok(FileChange::Unchanged) => return None,
        // Gone, e.g. between the two steps of a rotation: the next event
        // will tell.
        Err(_) => return None,
        Ok(FileChange::Grew) => log
            .reload_grown(&never, |_| {})
            .map(|l| Reload::Grew(Arc::new(l))),
        Ok(FileChange::Replaced) => log
            .reopen(&never, |_| {})
            .map(|l| Reload::Replaced(Arc::new(l))),
    };
    result
        .inspect_err(|e| tracing::warn!("cannot reload {}: {e}", log.path().display()))
        .ok()
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
                let Ok(Some(log)) = this.read_with(cx, |c, _| c.log.clone()) else {
                    break;
                };
                let executor = cx.background_executor().clone();
                let result =
                    background::run_blocking("rlogg-reload", &executor, move || reload(&log))
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

    fn apply_reload(&mut self, reload: Reload, cx: &mut Context<Self>) {
        cx.emit(CrawlerEvent::NewData);
        match reload {
            Reload::Grew(log) => {
                self.log = Some(log.clone());
                for view in [&self.main, &self.filtered] {
                    let log = log.clone();
                    view.update(cx, |v, cx| v.update_log(log, cx));
                }
                self.refresh_search(cx);
            }
            Reload::Replaced(log) => {
                // klogg (`fileChangedHandler`): drop the marks and the search
                // results; auto-refresh runs the search again.
                self.stop_search();
                self.filter.update(cx, |f, cx| {
                    *f = FilteredLines::new();
                    cx.notify();
                });
                self.log = Some(log.clone());
                self.current_line = 0;
                self.searched = None;
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
        }
        cx.notify();
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
        self.following = on;
        for view in [&self.main, &self.filtered] {
            view.update(cx, |v, cx| v.set_following(on, cx));
        }
        cx.notify();
    }
}
