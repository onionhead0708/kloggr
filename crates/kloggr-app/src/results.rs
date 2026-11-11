//! Kept search results: tabs of the bottom pane (klogg: "Keep Results",
//! `tabbedFilteredView_`).
//!
//! With Keep Results checked, the next search opens a new tab with its own
//! matches and marks, and the earlier results stay in theirs. The main view
//! shows the bullets of the current tab. Switching tabs stops the running
//! search (klogg: `changeFilteredView`). The tab bar is hidden while there is
//! one tab (klogg: `setTabBarAutoHide`).

use std::sync::Arc;

use gpui::{AppContext, Context, Entity, Focusable, Subscription, Window};
use kloggr_core::search::{FilteredLines, Matcher, Pattern};

use crate::crawler::{Crawler, CrawlerEvent, SearchInfo};
use crate::log_view::{LogView, LogViewEvent};
use crate::quick_find::Target;

/// One tab of search results.
pub(crate) struct Results {
    pub(crate) filter: Entity<FilteredLines>,
    pub(crate) view: Entity<LogView>,
    /// `Find "pattern"` once searched (klogg's tab text).
    pub(crate) title: String,
    /// The search state of the tab while another one is current.
    pattern: Option<Pattern>,
    searched: Option<(usize, usize)>,
    info: SearchInfo,
    _subscriptions: Vec<Subscription>,
}

impl Results {
    pub(crate) fn new(
        filter: Entity<FilteredLines>,
        view: Entity<LogView>,
        window: &mut Window,
        cx: &mut Context<Crawler>,
    ) -> Self {
        let focus = view.focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &LogViewEvent, window, cx| match event {
                    &LogViewEvent::LineSelected(line) => {
                        // klogg: `jumpToMatchingLine`, which also stops
                        // following.
                        this.current_line = line;
                        this.set_follow(false, cx);
                        this.main.update(cx, |v, cx| v.show_line(line, cx));
                    }
                    &LogViewEvent::FollowChanged(on) => this.set_follow(on, cx),
                    LogViewEvent::ColorLabel(op, text) => this.color_label(*op, text, cx),
                    LogViewEvent::FreezeChanged(_) => cx.emit(CrawlerEvent::FrozenChanged),
                    LogViewEvent::Search(op, text) => this.search_from_view(*op, text, window, cx),
                    &LogViewEvent::SearchLimit(op) => this.change_search_limits(op, cx),
                    LogViewEvent::SaveSplitterPosition => this.save_split(cx),
                    &LogViewEvent::Saving(p) => {
                        this.saving = p;
                        cx.emit(CrawlerEvent::Status);
                    }
                },
            ),
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.on_focus_in(&focus, window, |this, _, _| this.target = Target::Filtered),
        ];
        Self {
            filter,
            view,
            title: String::new(),
            pattern: None,
            searched: None,
            info: SearchInfo::default(),
            _subscriptions: subscriptions,
        }
    }
}

impl Crawler {
    /// Opens a tab for the next search's results, with the current tab's
    /// visibility and frozen columns (klogg: `startNewSearch` with Keep
    /// Results checked).
    pub(crate) fn new_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visibility = self.filter.read(cx).visibility();
        let filter = cx.new(|_| {
            let mut f = FilteredLines::new();
            f.set_visibility(visibility);
            f
        });
        let freeze = self.filtered.read(cx).freeze();
        let log = self.log.clone();
        let view = cx.new(|cx| {
            let mut v = LogView::filtered(cx);
            v.set_filter(filter.clone(), cx);
            v.set_log(log, cx);
            v.set_freeze(freeze, cx);
            v
        });
        let results = Results::new(filter, view, window, cx);
        self.results.push(results);
        self.show_results(self.results.len() - 1, cx);
    }

    /// Makes tab `index` current (klogg: `changeFilteredView`).
    pub(crate) fn show_results(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.results.len() {
            return;
        }
        self.stop_search();
        let current = &mut self.results[self.current_results];
        current.pattern = self.last_pattern.take();
        current.searched = self.searched.take();
        current.info = std::mem::take(&mut self.info);

        self.current_results = index;
        let next = &mut self.results[index];
        self.last_pattern = next.pattern.take();
        self.searched = next.searched.take();
        self.info = std::mem::take(&mut next.info);
        self.filter = next.filter.clone();
        self.filtered = next.view.clone();

        let filter = self.filter.clone();
        self.main.update(cx, |v, cx| v.set_filter(filter, cx));
        // The tab may have missed appended data and option changes.
        if let Some(log) = self.log.clone() {
            self.filtered.update(cx, |v, cx| v.update_log(log, cx));
        }
        let limits = self.search_limits.clone();
        self.filtered
            .update(cx, |v, cx| v.set_search_limits(limits, cx));
        self.show_color_labels(cx);
        let highlight = self
            .last_pattern
            .as_ref()
            .filter(|p| !p.exclude && !p.boolean)
            .and_then(|p| Matcher::new(p).ok())
            .map(Arc::new);
        self.set_search_highlight(highlight, cx);
        cx.notify();
    }

    /// Closes tab `index`; the last tab stays.
    pub(crate) fn close_results(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.results.len() < 2 || index >= self.results.len() {
            return;
        }
        if index == self.current_results {
            let next = if index + 1 < self.results.len() {
                index + 1
            } else {
                index - 1
            };
            self.show_results(next, cx);
        }
        self.results.remove(index);
        if self.current_results > index {
            self.current_results -= 1;
        }
        cx.notify();
    }

    /// Drops the other tabs: their line numbers are wrong once the file is
    /// replaced, reloaded or another file is shown.
    pub(crate) fn drop_kept_results(&mut self) {
        let current = self.current_results;
        let mut i = 0;
        self.results.retain(|_| {
            i += 1;
            i - 1 == current
        });
        self.current_results = 0;
    }
}
