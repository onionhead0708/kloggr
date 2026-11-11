//! Quick find: the next or previous occurrence of a pattern from a position
//! (klogg: `QuickFind::doSearchForward`, `doSearchBackward`).
//!
//! klogg scans line by line on one thread; kloggr checks blocks of rows in
//! parallel and keeps the first (or last) block with a match, so a find that
//! crosses a multi-GB file runs at search speed. Positions are display
//! columns, as in [`Selection`](crate::Selection) portions.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use super::{BLOCK_LINES, LineSet, Matcher};
use crate::encoding::Encoding;
use crate::file::LogFile;
use crate::line_index::Cancelled;
use crate::selection::Position;
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// The rows of a view: every line of the file, or the lines of a filtered
/// view.
#[derive(Debug, Clone, Copy)]
pub enum Rows<'a> {
    All,
    Lines(&'a LineSet),
}

impl Rows<'_> {
    pub fn count(&self, log: &LogFile) -> usize {
        match self {
            Rows::All => log.line_count(),
            Rows::Lines(set) => set.len(),
        }
    }

    pub fn line_of(&self, row: usize) -> Option<usize> {
        match self {
            Rows::All => Some(row),
            Rows::Lines(set) => set.nth(row),
        }
    }
}

/// An occurrence: display columns `cols` of `row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub row: usize,
    pub cols: Range<usize>,
}

/// Display column ranges of the non-empty matches of `matcher` in `line`.
pub fn match_columns(log: &LogFile, matcher: &Matcher, line: usize) -> Vec<Range<usize>> {
    let Some(text) = log.line_text(line) else {
        return Vec::new();
    };
    text::column_ranges(&text, &matcher.match_ranges(&text))
}

/// Whether `line`, whose content is `bytes`, has a non-empty match.
fn has_match(log: &LogFile, matcher: &Matcher, line: usize, bytes: &[u8]) -> bool {
    // Cheap test on the raw line first; most lines don't match.
    let candidate = if log.encoding() == Encoding::UTF_8 {
        matcher.is_match(bytes)
    } else {
        matcher.is_match(log.encoding().decode(bytes).as_bytes())
    };
    candidate && !match_columns(log, matcher, line).is_empty()
}

/// The first (or, `backward`, last) row of `block` with a match.
fn find_in_block(
    log: &LogFile,
    rows: Rows<'_>,
    matcher: &Matcher,
    block: Range<usize>,
    backward: bool,
) -> Option<usize> {
    let first = block.start;
    let hit =
        |row: usize, line: usize, bytes: &[u8]| has_match(log, matcher, line, bytes).then_some(row);
    match rows {
        // Decode the block's line offsets once, as `search` does.
        Rows::All => {
            let lines: Vec<_> = log.text_lines(block).collect();
            let test =
                |(k, bytes): (usize, &std::borrow::Cow<'_, [u8]>)| hit(first + k, first + k, bytes);
            if backward {
                lines.iter().enumerate().rev().find_map(test)
            } else {
                lines.iter().enumerate().find_map(test)
            }
        }
        Rows::Lines(_) => {
            let test = |row: usize| {
                let line = rows.line_of(row)?;
                hit(row, line, &log.text_line(line)?)
            };
            if backward {
                block.rev().find_map(test)
            } else {
                block.into_iter().find_map(test)
            }
        }
    }
}

/// Finds the first occurrence at or after `from` (forward), or the last
/// one starting before `from` (backward). `from.line` is a row of `rows`.
/// Calls `on_progress` with the percentage of rows scanned; checks `cancel`
/// between batches. Returns `Ok(None)` at the end (or start) of the rows,
/// as klogg's "Reached end of file". Blocks; call it off the UI thread.
pub fn find(
    log: &LogFile,
    rows: Rows<'_>,
    matcher: &Matcher,
    from: Position,
    direction: Direction,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u8),
) -> Result<Option<Found>, Cancelled> {
    let count = rows.count(log);
    let in_row = |row: usize| {
        rows.line_of(row)
            .map(|line| match_columns(log, matcher, line))
            .unwrap_or_default()
    };
    let batch = BLOCK_LINES * rayon::current_num_threads().max(1) * 4;

    match direction {
        Direction::Forward => {
            if from.line < count
                && let Some(cols) = in_row(from.line).into_iter().find(|c| c.start >= from.col)
            {
                return Ok(Some(Found {
                    row: from.line,
                    cols,
                }));
            }
            let start = from.line.saturating_add(1).min(count);
            let mut lo = start;
            while lo < count {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Cancelled);
                }
                let hi = lo.saturating_add(batch).min(count);
                let first = (lo..hi)
                    .step_by(BLOCK_LINES)
                    .collect::<Vec<_>>()
                    .into_par_iter()
                    .filter_map(|b| {
                        find_in_block(log, rows, matcher, b..(b + BLOCK_LINES).min(hi), false)
                    })
                    .min();
                if let Some(row) = first {
                    let cols = in_row(row).swap_remove(0);
                    return Ok(Some(Found { row, cols }));
                }
                on_progress(percent(hi - start, count - start));
                lo = hi;
            }
            Ok(None)
        }
        Direction::Backward => {
            if from.line < count
                && let Some(cols) = in_row(from.line)
                    .into_iter()
                    .rev()
                    .find(|c| c.start < from.col)
            {
                return Ok(Some(Found {
                    row: from.line,
                    cols,
                }));
            }
            let end = from.line.min(count);
            let mut hi = end;
            while hi > 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Cancelled);
                }
                let lo = hi.saturating_sub(batch);
                let last = (lo..hi)
                    .step_by(BLOCK_LINES)
                    .collect::<Vec<_>>()
                    .into_par_iter()
                    .filter_map(|b| {
                        find_in_block(log, rows, matcher, b..(b + BLOCK_LINES).min(hi), true)
                    })
                    .max();
                if let Some(row) = last {
                    let cols = in_row(row).pop().expect("row has a match");
                    return Ok(Some(Found { row, cols }));
                }
                on_progress(percent(end - lo, end));
                hi = lo;
            }
            Ok(None)
        }
    }
}

fn percent(done: usize, total: usize) -> u8 {
    (done * 100)
        .checked_div(total)
        .map_or(100, |p| p.min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Pattern;
    use std::io::Write;
    use std::path::PathBuf;

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, content: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("kloggr-find-{}-{name}", std::process::id()));
            std::fs::File::create(&path)
                .unwrap()
                .write_all(content.as_bytes())
                .unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn at(line: usize, col: usize) -> Position {
        Position { line, col }
    }

    fn run(log: &LogFile, rows: Rows, p: &str, from: Position, d: Direction) -> Option<Found> {
        let m = Matcher::new(&Pattern::plain(p)).unwrap();
        find(log, rows, &m, from, d, &AtomicBool::new(false), |_| {}).unwrap()
    }

    fn found(row: usize, cols: Range<usize>) -> Option<Found> {
        Some(Found { row, cols })
    }

    #[test]
    fn forward_within_and_across_lines() {
        let f = TempFile::new("fwd", "ab ab\nxx\n\tab\n");
        let log = LogFile::open(&f.0).unwrap();
        let fwd = Direction::Forward;
        assert_eq!(run(&log, Rows::All, "ab", at(0, 0), fwd), found(0, 0..2));
        assert_eq!(run(&log, Rows::All, "ab", at(0, 1), fwd), found(0, 3..5));
        // Columns are display columns: the tab spans 0..8.
        assert_eq!(run(&log, Rows::All, "ab", at(0, 4), fwd), found(2, 8..10));
        assert_eq!(run(&log, Rows::All, "ab", at(3, 0), fwd), None);
        assert_eq!(run(&log, Rows::All, "zz", at(0, 0), fwd), None);
    }

    #[test]
    fn backward_within_and_across_lines() {
        let f = TempFile::new("bwd", "ab ab\nxx\n\tab\n");
        let log = LogFile::open(&f.0).unwrap();
        let bwd = Direction::Backward;
        assert_eq!(run(&log, Rows::All, "ab", at(2, 8), bwd), found(0, 3..5));
        assert_eq!(run(&log, Rows::All, "ab", at(2, 9), bwd), found(2, 8..10));
        assert_eq!(run(&log, Rows::All, "ab", at(0, 3), bwd), found(0, 0..2));
        assert_eq!(run(&log, Rows::All, "ab", at(0, 0), bwd), None);
        // Past the end: starts from the last line.
        assert_eq!(run(&log, Rows::All, "ab", at(99, 0), bwd), found(2, 8..10));
    }

    #[test]
    fn many_blocks() {
        let n = BLOCK_LINES * 9 + 5;
        let mut text = String::new();
        for i in 0..n {
            text += if i == 7 || i == n - 3 {
                "needle\n"
            } else {
                "hay\n"
            };
        }
        let f = TempFile::new("blocks", &text);
        let log = LogFile::open(&f.0).unwrap();
        let fwd = run(&log, Rows::All, "needle", at(8, 0), Direction::Forward);
        assert_eq!(fwd, found(n - 3, 0..6));
        let bwd = run(&log, Rows::All, "needle", at(n - 3, 0), Direction::Backward);
        assert_eq!(bwd, found(7, 0..6));
    }

    #[test]
    fn filtered_rows() {
        let f = TempFile::new("rows", "a x\nb x\nc x\nd y\n");
        let log = LogFile::open(&f.0).unwrap();
        let set: LineSet = [0, 2, 3].into_iter().collect();
        let rows = Rows::Lines(&set);
        // Row 1 is line 2 ("c x").
        assert_eq!(
            run(&log, rows, "x", at(0, 3), Direction::Forward),
            found(1, 2..3)
        );
        assert_eq!(
            run(&log, rows, "y", at(0, 0), Direction::Forward),
            found(2, 2..3)
        );
        assert_eq!(run(&log, rows, "b", at(0, 0), Direction::Forward), None);
    }

    #[test]
    fn regex_and_case() {
        let f = TempFile::new("regex", "Error 42\nerror 7\n");
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::regex(r"error \d+").case_insensitive()).unwrap();
        let never = AtomicBool::new(false);
        let r = find(
            &log,
            Rows::All,
            &m,
            at(0, 1),
            Direction::Forward,
            &never,
            |_| {},
        );
        assert_eq!(r.unwrap(), found(1, 0..7));
        // Empty matches are not occurrences.
        let m = Matcher::new(&Pattern::regex("z*")).unwrap();
        let r = find(
            &log,
            Rows::All,
            &m,
            at(0, 0),
            Direction::Forward,
            &never,
            |_| {},
        );
        assert_eq!(r.unwrap(), None);
    }

    #[test]
    fn cancelled() {
        let text = "x\n".repeat(BLOCK_LINES * 2);
        let f = TempFile::new("cancel", &text);
        let log = LogFile::open(&f.0).unwrap();
        let m = Matcher::new(&Pattern::plain("y")).unwrap();
        let r = find(
            &log,
            Rows::All,
            &m,
            at(0, 0),
            Direction::Forward,
            &AtomicBool::new(true),
            |_| {},
        );
        assert_eq!(r, Err(Cancelled));
    }
}
