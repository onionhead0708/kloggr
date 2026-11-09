//! Searching a log file (klogg: `src/regex/`, `LogFilteredDataWorker`).
//!
//! - [`Pattern`] / [`Matcher`]: regex, plain-text, case-insensitive, inverse and
//!   boolean patterns (klogg: `RegularExpressionPattern`, `PatternMatcher`)
//! - [`LineSet`]: compact set of matching lines (klogg: `SearchResultArray`)
//! - [`FilteredLines`]: matches and marks shown by the filtered view (klogg:
//!   `LogFilteredData`)
//! - [`search`]: parallel search of a line range, with progress, cancellation
//!   and incremental results (klogg: `SearchOperation`)

mod boolean;
mod filtered;
mod line_set;
mod matcher;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

pub use boolean::BooleanError;
pub use filtered::{FilteredLines, LineKind, Visibility};
pub use line_set::LineSet;
pub use matcher::{Matcher, Pattern, PatternError};

use crate::encoding::Encoding;
use crate::file::LogFile;
use crate::line_index::Cancelled;

/// Lines matched by one rayon task.
const BLOCK_LINES: usize = 8192;

/// Search progress, reported after each batch of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProgress {
    /// Lines searched so far, out of `total`.
    pub done: usize,
    pub total: usize,
    /// Matching lines found so far.
    pub matches: usize,
}

impl SearchProgress {
    /// Progress in percent, 0..=100.
    pub fn percent(&self) -> u8 {
        (self.done * 100)
            .checked_div(self.total)
            .map_or(100, |p| p.min(100) as u8)
    }
}

/// Searches `lines` of `log` (clamped to its line count) in parallel. After
/// each batch, calls `on_batch` with the progress and the batch's matching
/// lines, in ascending order, so a view can show results as they come.
/// Checks `cancel` between batches. Blocks; call it off the UI thread.
///
/// To search lines appended to a growing file, search just the new range and
/// append the results (klogg: `UpdateSearchOperation`).
pub fn search(
    log: &LogFile,
    matcher: &Matcher,
    lines: Range<usize>,
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(SearchProgress, &[usize]),
) -> Result<(), Cancelled> {
    // Enough blocks per batch to keep every thread busy.
    let batch = BLOCK_LINES * rayon::current_num_threads().max(1) * 4;
    let end = lines.end.min(log.line_count());
    let start = lines.start.min(end);
    let mut progress = SearchProgress {
        done: 0,
        total: end - start,
        matches: 0,
    };
    let mut from = start;
    while from < end {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let to = from.saturating_add(batch).min(end);
        let blocks: Vec<Vec<usize>> = (from..to)
            .step_by(BLOCK_LINES)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|b| search_block(log, matcher, b..(b + BLOCK_LINES).min(to)))
            .collect();
        let found = blocks.concat();
        progress.done += to - from;
        progress.matches += found.len();
        on_batch(progress, &found);
        from = to;
    }
    Ok(())
}

/// Like [`search`], collecting every matching line.
pub fn search_all(
    log: &LogFile,
    matcher: &Matcher,
    lines: Range<usize>,
    cancel: &AtomicBool,
) -> Result<LineSet, Cancelled> {
    let mut set = LineSet::new();
    search(log, matcher, lines, cancel, |_, found| {
        set.append_sorted(found)
    })?;
    Ok(set)
}

fn search_block(log: &LogFile, matcher: &Matcher, range: Range<usize>) -> Vec<usize> {
    let encoding = log.encoding();
    let first = range.start;
    let mut found = Vec::new();
    for (k, line) in log.lines(range).enumerate() {
        let matched = if encoding == Encoding::UTF_8 {
            // Invalid sequences are left as they are: `.` doesn't match them
            // but literals around them still do.
            matcher.is_match(line)
        } else {
            matcher.is_match(encoding.decode(line).as_bytes())
        };
        if matched {
            found.push(first + k);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, content: &[u8]) -> Self {
            let path =
                std::env::temp_dir().join(format!("rlogg-search-{}-{name}", std::process::id()));
            std::fs::File::create(&path)
                .unwrap()
                .write_all(content)
                .unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn run(log: &LogFile, pattern: Pattern) -> Vec<usize> {
        let m = Matcher::new(&pattern).unwrap();
        search_all(log, &m, 0..usize::MAX, &AtomicBool::new(false))
            .unwrap()
            .iter()
            .collect()
    }

    #[test]
    fn finds_matching_lines() {
        let f = TempFile::new("basic", b"info a\nerror b\r\ninfo c\nERROR d\nlast error");
        let log = LogFile::open(&f.0).unwrap();
        assert_eq!(run(&log, Pattern::plain("error")), [1, 4]);
        assert_eq!(
            run(&log, Pattern::plain("error").case_insensitive()),
            [1, 3, 4]
        );
        assert_eq!(run(&log, Pattern::plain("error").excluding()), [0, 2, 3]);
        // `$` sees lines without their `\r\n` terminator.
        assert_eq!(run(&log, Pattern::regex("b$")), [1]);
    }

    #[test]
    fn many_blocks_and_batches() {
        let mut content = String::new();
        let n = BLOCK_LINES * 3 + 17;
        for i in 0..n {
            content.push_str(&format!("line {i}\n"));
        }
        let f = TempFile::new("many", content.as_bytes());
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::regex(r"7$")).unwrap();

        let mut batches = Vec::new();
        let mut all = Vec::new();
        let never = AtomicBool::new(false);
        // A small range spanning block boundaries, then the whole file.
        let from = BLOCK_LINES - 20;
        search(&log, &m, from..from + 40, &never, |p, found| {
            batches.push(p);
            all.extend_from_slice(found);
        })
        .unwrap();
        let expected: Vec<usize> = (from..from + 40).filter(|i| i % 10 == 7).collect();
        assert_eq!(all, expected);
        assert_eq!(batches.last().unwrap().done, 40);
        assert_eq!(batches.last().unwrap().matches, expected.len());

        let set = search_all(&log, &m, 0..usize::MAX, &never).unwrap();
        assert_eq!(set.len(), (0..n).filter(|i| i % 10 == 7).count());
        assert!(set.iter().all(|i| i % 10 == 7));
    }

    #[test]
    fn progress_is_monotonic_and_complete() {
        let content = "x\n".repeat(BLOCK_LINES * 2);
        let f = TempFile::new("progress", content.as_bytes());
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::plain("x")).unwrap();
        let mut last = 0;
        search(&log, &m, 0..usize::MAX, &AtomicBool::new(false), |p, _| {
            assert!(p.done > last);
            last = p.done;
            assert_eq!(p.total, BLOCK_LINES * 2);
        })
        .unwrap();
        assert_eq!(last, BLOCK_LINES * 2);
    }

    #[test]
    fn cancelled() {
        let f = TempFile::new("cancel", b"a\nb\n");
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::plain("a")).unwrap();
        let r = search_all(&log, &m, 0..2, &AtomicBool::new(true));
        assert_eq!(r, Err(Cancelled));
    }

    #[test]
    fn empty_range_and_file() {
        let f = TempFile::new("empty", b"");
        let log = LogFile::open(&f.0).unwrap();
        assert!(run(&log, Pattern::regex("")).is_empty());
        let f = TempFile::new("range", b"a\na\n");
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::plain("a")).unwrap();
        let never = AtomicBool::new(false);
        assert!(search_all(&log, &m, 5..9, &never).unwrap().is_empty());
        assert_eq!(search_all(&log, &m, 1..9, &never).unwrap().len(), 1);
    }

    #[test]
    fn decodes_other_encodings() {
        let text = "début\nÉTÉ chaud\nfin\n";
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let f = TempFile::new("utf16", &utf16);
        let log = LogFile::open(&f.0).unwrap();
        assert_eq!(log.encoding(), Encoding::UTF_16LE);
        assert_eq!(run(&log, Pattern::plain("été").case_insensitive()), [1]);
        assert_eq!(run(&log, Pattern::regex("^d.but$")), [0]);

        let (latin1, _, _) = encoding_rs::WINDOWS_1252.encode(text);
        let f = TempFile::new("latin1", &latin1);
        let options = OpenOptions {
            encoding: Some(Encoding::WINDOWS_1252),
        };
        let log = LogFile::open_with(&f.0, options, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(run(&log, Pattern::plain("ÉTÉ")), [1]);
    }

    #[test]
    fn boolean_search() {
        let f = TempFile::new(
            "boolean",
            b"conn open id=1\nconn close id=1\nconn open id=2 timeout\nother\n",
        );
        let log = LogFile::open(&f.0).unwrap();
        let p = Pattern::plain(r#""conn" and not("timeout")"#).boolean();
        assert_eq!(run(&log, p), [0, 1]);
        let p = Pattern::regex(r#""open" xor "id=2""#).boolean();
        assert_eq!(run(&log, p), [0]);
    }
}
