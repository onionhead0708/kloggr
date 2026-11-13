//! Line start offsets for a byte buffer.
//!
//! Port of the indexing half of klogg's `LogDataWorker`
//! (`src/logdata/src/logdataworker.cpp`, GPLv3): the buffer is scanned for
//! `\n` in fixed-size chunks, chunks are scanned in parallel (rayon instead of
//! klogg's TBB flow graph), and their results are appended in order to a
//! [`CompressedOffsets`] (klogg: `CompressedLinePositionStorage`).
//!
//! Indexing is incremental: [`LineIndex::extend`] indexes only the bytes added
//! since the last call, which is what follow mode needs (klogg:
//! `PartialIndexOperation`). It reports progress and can be cancelled between
//! batches; a cancelled index is still valid for the prefix it covers.
//!
//! Multi-byte line feeds (UTF-16) follow klogg's `lineFeedWidth` /
//! `lineFeedIndex`, but stricter: a `0x0A` byte is a line feed only at its
//! position within an aligned code unit whose other byte is 0, so text such as
//! U+520A is not split. Indexing starts after the byte-order mark.
//!
//! The longest line's length expands tabs to the next multiple of
//! [`TAB_STOP`](crate::text::TAB_STOP) columns, as klogg does while indexing (`expandTabsInLine`).
//! Each chunk measures its lines in the same pass that finds the line feeds;
//! a line that starts in an earlier chunk gets a [`Segment`], which gives
//! its width from any start column, so chunks don't wait for each other.

mod compressed;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

pub use compressed::CompressedOffsets;

use crate::encoding::LineFeed;

/// Bytes scanned by one rayon task.
const CHUNK: usize = 4 << 20;

/// Tab stops, as the views lay lines out (klogg: `TabStop`).
const TAB_STOP: u64 = crate::text::TAB_STOP as u64;

/// The column after a tab at column `col`.
fn tab_stop(col: u64) -> u64 {
    (col / TAB_STOP + 1) * TAB_STOP
}

/// A piece of a line, without line feed, whose start column isn't known
/// yet: its width from any column. After its first tab the columns are
/// those from a tab stop, so the units before that tab and the width after
/// it are enough.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Segment {
    /// It has at least one code unit (a trailing `\r` included).
    nonempty: bool,
    /// Code units before the first tab, or all of them without a tab.
    before_tab: u64,
    tab: bool,
    /// Width after the first tab, from a tab stop.
    after_tab: u64,
    /// It ends with a `\r` that isn't counted: it isn't part of the line if
    /// a `\n` follows, and is one column if anything else does.
    cr: bool,
}

impl Segment {
    /// The column after this segment when it starts at `col`.
    fn end_col(&self, col: u64) -> u64 {
        if self.tab {
            tab_stop(col + self.before_tab) + self.after_tab
        } else {
            col + self.before_tab
        }
    }
}

/// What one chunk found: line starts, and line widths.
struct ChunkScan {
    starts: Vec<u64>,
    /// From the chunk's start to its first line feed, or to its end.
    head: Segment,
    /// The chunk has a line feed, which ends the head's line.
    terminated: bool,
    /// The widest line between two line feeds of the chunk.
    inner_max: u64,
    /// From the last line feed to the chunk's end, starting a line.
    tail: Segment,
}

/// Indexing progress, reported after each batch of chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress {
    /// Bytes indexed so far, out of `total`.
    pub done: u64,
    pub total: u64,
}

impl IndexProgress {
    /// Progress in percent, 0..=100 (klogg: `calculateProgress`).
    pub fn percent(&self) -> u8 {
        (self.done * 100)
            .checked_div(self.total)
            .map_or(100, |p| p.min(100) as u8)
    }
}

/// Indexing was cancelled; the index covers only part of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("indexing cancelled")]
pub struct Cancelled;

/// Index of line boundaries in a buffer.
///
/// Lines are terminated by `\n`; a `\r` before it is not part of the line.
/// A final line without a terminator still counts. An empty buffer has 0 lines.
/// For multi-byte encodings, a trailing partial code unit is not indexed.
#[derive(Debug, Clone)]
pub struct LineIndex {
    lf: LineFeed,
    /// Offset of the first line: the byte-order mark is skipped.
    origin: u64,
    /// Offset of the first byte of each line.
    starts: CompressedOffsets,
    /// Number of bytes of the buffer covered by the index.
    len: u64,
    /// Widest line among the lines ended by a line feed, in columns.
    max_terminated_len: u64,
    /// Width of the line still open at the end of the indexed part, which
    /// may still grow on [`extend`](Self::extend), without a final `\r`.
    open_len: u64,
    /// The open line ends with a `\r` (see [`Segment::cr`]).
    open_cr: bool,
}

impl Default for LineIndex {
    fn default() -> Self {
        Self::new(LineFeed::SINGLE_BYTE, 0)
    }
}

impl LineIndex {
    /// An empty index for text with line feeds `lf`, whose first line starts
    /// at `origin` (the byte-order mark length).
    pub fn new(lf: LineFeed, origin: usize) -> Self {
        assert_eq!(
            origin % lf.width as usize,
            0,
            "origin must be code-unit aligned"
        );
        Self {
            lf,
            origin: origin as u64,
            starts: CompressedOffsets::new(),
            len: 0,
            max_terminated_len: 0,
            open_len: 0,
            open_cr: false,
        }
    }

    /// Builds the index of single-byte-line-feed `data` (parallel, not
    /// cancellable).
    pub fn build(data: &[u8]) -> Self {
        let mut index = Self::default();
        index.extend(data);
        index
    }

    pub fn line_feed(&self) -> LineFeed {
        self.lf
    }

    /// Indexes the bytes appended to `data` since the last call.
    ///
    /// `data` must be the buffer this index was built from, possibly grown;
    /// the already-indexed prefix must be unchanged.
    pub fn extend(&mut self, data: &[u8]) {
        let never = AtomicBool::new(false);
        self.extend_with(data, &never, |_| {})
            .expect("cannot be cancelled");
    }

    /// Like [`extend`](Self::extend), reporting progress and checking `cancel`
    /// between batches. On cancellation the index stays valid and covers a
    /// prefix of `data`; calling this again resumes where it stopped.
    pub fn extend_with(
        &mut self,
        data: &[u8],
        cancel: &AtomicBool,
        on_progress: impl FnMut(IndexProgress),
    ) -> Result<(), Cancelled> {
        // Enough chunks per batch to keep every thread busy.
        let batch = CHUNK * rayon::current_num_threads().max(1) * 2;
        self.extend_batched(data, cancel, on_progress, batch)
    }

    fn extend_batched(
        &mut self,
        data: &[u8],
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(IndexProgress),
        batch: usize,
    ) -> Result<(), Cancelled> {
        let total = self.indexable_len(data);
        assert!(total >= self.len, "buffer shrank: reindex from scratch");
        let batch = batch - batch % self.lf.width as usize;
        while self.len < total {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled);
            }
            let end = (self.len as usize)
                .saturating_add(batch)
                .min(total as usize);
            self.extend_to(data, end);
            on_progress(IndexProgress {
                done: self.len,
                total,
            });
        }
        self.starts.shrink_to_fit();
        Ok(())
    }

    /// Bytes of `data` that can be indexed: 0 if there is nothing after the
    /// BOM but a partial code unit, otherwise up to the last complete unit.
    fn indexable_len(&self, data: &[u8]) -> u64 {
        let len = data.len() as u64;
        let width = u64::from(self.lf.width);
        if len < self.origin + width {
            return 0;
        }
        len - (len - self.origin) % width
    }

    /// Indexes `data[self.len..end]`; `end` is code-unit aligned.
    fn extend_to(&mut self, data: &[u8], end: usize) {
        let origin = self.origin as usize;
        let scan_from = (self.len as usize).max(origin);
        debug_assert!(scan_from < end);

        // A line starts at `scan_from` if it is the first line, or if the
        // indexed part ended on a line feed: a start is only recorded once
        // there is a byte after the terminator.
        if self.starts.is_empty() || self.lf.ends_with_lf(data, scan_from) {
            self.starts.push(scan_from as u64);
        }

        // Positions after each line feed in the new bytes, and line widths,
        // chunk by chunk in parallel. Chunks are code-unit aligned, so units
        // never straddle them.
        let lf = self.lf;
        let chunks: Vec<ChunkScan> = (scan_from..end)
            .step_by(CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|from| scan_chunk(data, from, (from + CHUNK).min(end), end, lf, origin))
            .collect();
        for chunk in chunks {
            self.continue_line(&chunk.head);
            if chunk.terminated {
                self.max_terminated_len = self.max_terminated_len.max(self.open_len);
                (self.open_len, self.open_cr) = (0, false);
                self.max_terminated_len = self.max_terminated_len.max(chunk.inner_max);
                self.continue_line(&chunk.tail);
            }
            for s in chunk.starts {
                self.starts.push(s);
            }
        }
        self.len = end as u64;
    }

    /// Adds `segment` to the open line.
    fn continue_line(&mut self, segment: &Segment) {
        if segment.nonempty {
            // A `\r` followed by more text is one column.
            let col = self.open_len + u64::from(self.open_cr);
            self.open_len = segment.end_col(col);
            self.open_cr = segment.cr;
        }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Number of bytes of the buffer covered by the index.
    pub fn indexed_len(&self) -> u64 {
        self.len
    }

    /// Width of the longest line, without terminator, in code units with
    /// tabs expanded to the next multiple of
    /// [`TAB_STOP`](crate::text::TAB_STOP) (klogg:
    /// `getMaxLength`). For UTF-8 a code unit is a byte, for UTF-16 a 16-bit
    /// unit.
    pub fn max_line_len(&self) -> u64 {
        // A `\r` at the very end is part of the line until a `\n` follows.
        self.max_terminated_len
            .max(self.open_len + u64::from(self.open_cr))
    }

    /// Heap bytes used by the index.
    pub fn allocated_bytes(&self) -> usize {
        self.starts.allocated_bytes()
    }

    /// Byte range of line `n` in the buffer, including its terminator.
    pub fn line_span(&self, n: usize) -> Option<Range<usize>> {
        let start = self.starts.get(n)? as usize;
        let end = self.starts.get(n + 1).unwrap_or(self.len) as usize;
        Some(start..end)
    }

    /// Content of line `n` in `data` (the buffer this index was built from),
    /// without its `\n` / `\r\n` terminator.
    pub fn line<'a>(&self, data: &'a [u8], n: usize) -> Option<&'a [u8]> {
        self.check_buffer(data);
        self.line_span(n).map(|span| self.lf.strip_eol(&data[span]))
    }

    /// Content of the lines in `range` (clamped to the line count), decoding
    /// the index once for the whole range. Use this to render visible rows.
    pub fn lines<'a>(
        &self,
        data: &'a [u8],
        range: Range<usize>,
    ) -> impl ExactSizeIterator<Item = &'a [u8]> + use<'a> {
        self.check_buffer(data);
        let count = self.line_count();
        let end = range.end.min(count);
        let start = range.start.min(end);
        let mut bounds = self.starts.range(start..(end + 1).min(count));
        if start < end && end == count {
            bounds.push(self.len);
        }
        let lf = self.lf;
        (1..bounds.len().max(1))
            .map(move |k| lf.strip_eol(&data[bounds[k - 1] as usize..bounds[k] as usize]))
    }

    /// The start of each line in `range` (clamped to the line count), then
    /// the end of the last one; empty for an empty range.
    pub fn line_bounds(&self, range: Range<usize>) -> Vec<u64> {
        let count = self.line_count();
        let end = range.end.min(count);
        let start = range.start.min(end);
        if start == end {
            return Vec::new();
        }
        let mut bounds = self.starts.range(start..(end + 1).min(count));
        if end == count {
            bounds.push(self.len);
        }
        bounds
    }

    fn check_buffer(&self, data: &[u8]) {
        debug_assert!(
            data.len() as u64 >= self.len,
            "index built from another buffer"
        );
    }
}

/// Scans `data[from..to]` for line feeds and tabs. `to` is the chunk's
/// end and `end` the end of the indexed part; both are code-unit aligned,
/// as is `from` relative to `origin`.
fn scan_chunk(
    data: &[u8],
    from: usize,
    to: usize,
    end: usize,
    lf: LineFeed,
    origin: usize,
) -> ChunkScan {
    let (w, index) = (lf.width as usize, lf.index as usize);
    // Whether the code unit starting at `u` is the ASCII character `c`.
    let unit_is = |u: usize, c: u8| {
        if w == 1 {
            data[u] == c
        } else {
            lf.unit_before(data, u + w) == Some(c)
        }
    };
    let units = |a: usize, b: usize| ((b - a) / w) as u64;
    let mut scan = ChunkScan {
        starts: Vec::new(),
        head: Segment::default(),
        terminated: false,
        inner_max: 0,
        tail: Segment::default(),
    };
    // Where the text not yet measured starts, and the column there: from
    // the line start, or from the head's first tab.
    let mut last = from;
    let mut col = 0u64;
    // Start of the line after the last line feed.
    let mut line_start = from;
    for p in memchr::memchr2_iter(b'\n', b'\t', &data[from..to]) {
        let p = from + p;
        // The unit holding this byte, if the byte is a whole ASCII unit.
        let u = if w == 1 {
            p
        } else {
            if p < origin + index || !(p - index - origin).is_multiple_of(w) {
                continue;
            }
            p - index
        };
        if w > 1 && lf.unit_before(data, u + w) != Some(data[p]) {
            continue;
        }
        if data[p] == b'\t' {
            if !scan.terminated && !scan.head.tab {
                scan.head.before_tab = units(last, u);
                scan.head.tab = true;
                col = 0;
            } else {
                col = tab_stop(col + units(last, u));
            }
            last = u + w;
            continue;
        }
        // A line feed; a `\r` before it isn't part of the line.
        let mut content_end = u;
        if u >= last + w && unit_is(u - w, b'\r') {
            content_end -= w;
        }
        let width = col + units(last, content_end);
        if scan.terminated {
            scan.inner_max = scan.inner_max.max(width);
        } else {
            scan.head.nonempty = u > from;
            if scan.head.tab {
                scan.head.after_tab = width;
            } else {
                scan.head.before_tab = width;
            }
            scan.terminated = true;
        }
        col = 0;
        last = u + w;
        line_start = last;
        if last < end {
            scan.starts.push(last as u64);
        }
    }
    // The rest, up to the chunk's end; a final `\r` waits for what follows.
    let mut content_end = to;
    let cr = to >= last + w && unit_is(to - w, b'\r');
    if cr {
        content_end -= w;
    }
    let width = col + units(last, content_end);
    if scan.terminated {
        scan.tail = Segment {
            nonempty: to > line_start,
            before_tab: width,
            cr,
            ..Segment::default()
        };
    } else {
        scan.head.nonempty = to > from;
        if scan.head.tab {
            scan.head.after_tab = width;
        } else {
            scan.head.before_tab = width;
        }
        scan.head.cr = cr;
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strips a trailing `\r` from a line already split on `\n`.
    fn strip_cr(line: &[u8]) -> &[u8] {
        line.strip_suffix(b"\r").unwrap_or(line)
    }

    fn lines(data: &[u8]) -> Vec<&[u8]> {
        let idx = LineIndex::build(data);
        (0..idx.line_count())
            .map(|n| idx.line(data, n).unwrap())
            .collect()
    }

    /// Reference implementation: split on `\n` like klogg does.
    fn reference(data: &[u8]) -> Vec<&[u8]> {
        if data.is_empty() {
            return Vec::new();
        }
        let body = data.strip_suffix(b"\n").unwrap_or(data);
        body.split(|&b| b == b'\n').map(strip_cr).collect()
    }

    #[test]
    fn empty_buffer_has_no_lines() {
        let idx = LineIndex::build(b"");
        assert_eq!(idx.line_count(), 0);
        assert_eq!(idx.lines(b"", 0..10).count(), 0);
    }

    #[test]
    fn trailing_newline_does_not_add_a_line() {
        assert_eq!(lines(b"a\nbb\n"), vec![&b"a"[..], b"bb"]);
    }

    #[test]
    fn last_line_without_newline_counts() {
        assert_eq!(lines(b"a\nbb"), vec![&b"a"[..], b"bb"]);
    }

    #[test]
    fn crlf_is_stripped_and_empty_lines_kept() {
        assert_eq!(lines(b"a\r\n\r\nc\r\n"), vec![&b"a"[..], b"", b"c"]);
    }

    #[test]
    fn single_newline_is_one_empty_line() {
        assert_eq!(lines(b"\n"), vec![&b""[..]]);
    }

    #[test]
    fn out_of_range_line_is_none() {
        let data = b"x\n";
        assert!(LineIndex::build(data).line(data, 1).is_none());
    }

    #[test]
    fn max_line_len() {
        assert_eq!(LineIndex::build(b"ab\r\nabcd\nx").max_line_len(), 4);
        assert_eq!(LineIndex::build(b"ab\nx\r\nabcdef").max_line_len(), 6);
        assert_eq!(LineIndex::build(b"").max_line_len(), 0);
        // A lone `\r` is part of the line, at the end too.
        assert_eq!(LineIndex::build(b"a\rb\n").max_line_len(), 3);
        assert_eq!(LineIndex::build(b"ab\r").max_line_len(), 3);
    }

    /// Width of `line` with tabs expanded, the way the views lay it out.
    fn expanded(line: &[u8]) -> u64 {
        line.iter().fold(
            0,
            |col, &b| if b == b'\t' { tab_stop(col) } else { col + 1 },
        )
    }

    #[test]
    fn max_line_len_expands_tabs() {
        let max = |data: &[u8]| LineIndex::build(data).max_line_len();
        assert_eq!(max(b"\t"), 8);
        assert_eq!(max(b"a\tb"), 9);
        assert_eq!(max(b"1234567\tx"), 9);
        assert_eq!(max(b"12345678\t"), 16);
        assert_eq!(max(b"\t\t\r\nabc"), 16);
        assert_eq!(max(b"abc\n\t\t\tx\r\nab"), 25);
        assert_eq!(
            max(b"x\ta\r\tb\n"),
            17,
            "a `\\r` inside the line is a column"
        );
    }

    /// The widest line of `data`, tabs expanded; a `\r` is dropped only
    /// before a `\n`.
    fn widest(data: &[u8]) -> u64 {
        let mut lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
        let last = lines.pop().unwrap_or_default();
        lines
            .iter()
            .map(|l| expanded(strip_cr(l)))
            .chain([expanded(last)])
            .max()
            .unwrap_or(0)
    }

    /// Like [`generated`], with tabs.
    fn generated_with_tabs(len: usize) -> Vec<u8> {
        let mut x = 0x2545f4914f6cdd1du64;
        generated(len)
            .into_iter()
            .map(|b| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                if b != b'\n' && b != b'\r' && x.is_multiple_of(11) {
                    b'\t'
                } else {
                    b
                }
            })
            .collect()
    }

    #[test]
    fn tab_widths_across_chunks_and_extends() {
        let data = generated_with_tabs(3 * CHUNK + 12_345);
        let full = LineIndex::build(&data);
        assert_eq!(full.max_line_len(), widest(&data));
        assert_eq!(full.line_count(), reference(&data).len());

        // A long line with tabs across chunk boundaries.
        let mut long = b"x\n".to_vec();
        long.extend(
            generated_with_tabs(2 * CHUNK + 777)
                .iter()
                .map(|&b| match b {
                    b'\n' | b'\r' => b'y',
                    b => b,
                }),
        );
        long.extend(b"\r\nz");
        let want = expanded(&long[2..long.len() - 3]);
        assert_eq!(LineIndex::build(&long).max_line_len(), want);

        // Grown in uneven steps, cutting between `\r` and `\n` and in
        // runs of tabs.
        let data = generated_with_tabs(60_000);
        let full = LineIndex::build(&data);
        let mut idx = LineIndex::default();
        let mut cut = 0;
        let mut step = 1;
        while cut < data.len() {
            cut = (cut + step).min(data.len());
            idx.extend(&data[..cut]);
            assert_eq!(idx.max_line_len(), widest(&data[..cut]), "cut={cut}");
            step = step * 7 % 1013 + 1;
        }
        assert_eq!(idx.max_line_len(), full.max_line_len());
    }

    /// Deterministic pseudo-random log with lines of 0..200 bytes, some CRLF.
    fn generated(len: usize) -> Vec<u8> {
        let mut x = 0x9e3779b97f4a7c15u64;
        let mut out = Vec::with_capacity(len + 256);
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let n = (x % 200) as usize;
            out.extend(std::iter::repeat_n(b'a' + (x % 26) as u8, n));
            if x.is_multiple_of(5) {
                out.push(b'\r');
            }
            out.push(b'\n');
        }
        out.truncate(len);
        out
    }

    #[test]
    fn large_buffer_matches_reference_across_chunks() {
        // Several chunks and more than one batch on small machines.
        let data = generated(3 * CHUNK + 12_345);
        let idx = LineIndex::build(&data);
        let expected = reference(&data);
        assert_eq!(idx.line_count(), expected.len());
        let got: Vec<_> = idx.lines(&data, 0..idx.line_count()).collect();
        assert_eq!(got, expected);
        let max = expected.iter().map(|l| l.len()).max().unwrap() as u64;
        assert_eq!(idx.max_line_len(), max);
    }

    #[test]
    fn incremental_extend_matches_full_build() {
        let data = generated(50_000);
        let full = LineIndex::build(&data);
        // Grow in uneven steps, including cuts right before and after `\n`.
        let mut idx = LineIndex::default();
        let mut cut = 0;
        for step in [1, 2, 3, 97, 1000, 4096, 7, 13_000] {
            cut = (cut + step).min(data.len());
            idx.extend(&data[..cut]);
            assert_eq!(
                idx.lines(&data[..cut], 0..usize::MAX).collect::<Vec<_>>(),
                reference(&data[..cut]),
                "cut={cut}"
            );
        }
        idx.extend(&data);
        assert_eq!(idx.line_count(), full.line_count());
        assert_eq!(idx.max_line_len(), full.max_line_len());
        for n in [0, 1, full.line_count() / 2, full.line_count() - 1] {
            assert_eq!(idx.line(&data, n), full.line(&data, n));
        }
    }

    #[test]
    fn growing_last_line_updates_max_len() {
        let mut idx = LineIndex::build(b"ab\ncd");
        assert_eq!(idx.max_line_len(), 2);
        idx.extend(b"ab\ncdefg");
        assert_eq!((idx.line_count(), idx.max_line_len()), (2, 5));
        idx.extend(b"ab\ncdefg\n");
        assert_eq!((idx.line_count(), idx.max_line_len()), (2, 5));
        idx.extend(b"ab\ncdefg\nh");
        assert_eq!(idx.line_count(), 3);
    }

    #[test]
    fn lines_range_is_clamped() {
        let data = b"a\nb\nc";
        let idx = LineIndex::build(data);
        assert_eq!(idx.lines(data, 1..10).collect::<Vec<_>>(), [b"b", b"c"]);
        assert_eq!(idx.lines(data, 0..2).collect::<Vec<_>>(), [b"a", b"b"]);
        assert_eq!(idx.lines(data, 5..10).count(), 0);
    }

    #[test]
    fn cancel_leaves_a_resumable_prefix() {
        let data = generated(100_000);
        let batch = 30_000;
        let cancel = AtomicBool::new(false);
        let mut idx = LineIndex::default();
        let mut reports = Vec::new();
        let result = idx.extend_batched(
            &data,
            &cancel,
            |p| {
                reports.push(p);
                cancel.store(true, Ordering::Relaxed);
            },
            batch,
        );
        assert_eq!(result, Err(Cancelled));
        assert_eq!(reports.len(), 1);
        assert_eq!(idx.indexed_len(), batch as u64);
        assert_eq!(
            idx.lines(&data, 0..usize::MAX).collect::<Vec<_>>(),
            reference(&data[..batch])
        );

        cancel.store(false, Ordering::Relaxed);
        let mut percents = Vec::new();
        idx.extend_batched(&data, &cancel, |p| percents.push(p.percent()), batch)
            .unwrap();
        assert_eq!(percents, [60, 90, 100]);
        assert_eq!(idx.line_count(), reference(&data).len());
    }

    mod utf16 {
        use super::*;
        use crate::encoding::{Encoding, detect};

        /// Reference: decode the whole buffer, then split into lines.
        fn decoded_reference(enc: Encoding, data: &[u8]) -> Vec<String> {
            let w = enc.line_feed().width as usize;
            let bom = enc.bom_len(data);
            let body = &data[bom..data.len() - (data.len() - bom) % w];
            let text = enc.decode(body);
            if text.is_empty() {
                return Vec::new();
            }
            let text = text.strip_suffix('\n').unwrap_or(&text);
            text.split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
                .collect()
        }

        fn decoded_lines(enc: Encoding, idx: &LineIndex, data: &[u8]) -> Vec<String> {
            idx.lines(data, 0..usize::MAX)
                .map(|l| enc.decode(l).into_owned())
                .collect()
        }

        fn index(enc: Encoding, data: &[u8]) -> LineIndex {
            let mut idx = LineIndex::new(enc.line_feed(), enc.bom_len(data));
            idx.extend(data);
            idx
        }

        fn klogg_sample(name: &str) -> Vec<u8> {
            std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
        }

        #[test]
        fn klogg_samples_match_reference() {
            for name in [
                "Chinese-Lipsum.utf16.txt",
                "Chinese-Lipsum.utf16be.txt",
                "utf16_tab_test.txt",
                "utf16be_tab_test.txt",
                "random_block_100_byte_utf16le.txt",
                "random_block_100_byte_utf16be.txt",
            ] {
                let data = klogg_sample(name);
                let enc = detect(&data);
                let idx = index(enc, &data);
                let expected = decoded_reference(enc, &data);
                assert!(!expected.is_empty());
                assert_eq!(decoded_lines(enc, &idx, &data), expected, "{name}");
            }
        }

        #[test]
        fn le_and_be_samples_agree() {
            let le = klogg_sample("Chinese-Lipsum.utf16.txt");
            let be = klogg_sample("Chinese-Lipsum.utf16be.txt");
            let (il, ib) = (
                index(Encoding::UTF_16LE, &le),
                index(Encoding::UTF_16BE, &be),
            );
            assert_eq!(il.line_count(), ib.line_count());
            assert_eq!(il.max_line_len(), ib.max_line_len());
            assert_eq!(
                decoded_lines(Encoding::UTF_16LE, &il, &le),
                decoded_lines(Encoding::UTF_16BE, &ib, &be)
            );
        }

        /// Multi-chunk UTF-16 text with CRLF and code units containing 0x0A
        /// (U+520A, U+0A0A) that must not split lines.
        fn generated_text() -> String {
            let ascii = String::from_utf8(generated(CHUNK + CHUNK / 2)).unwrap();
            ascii
                .split_inclusive('\n')
                .enumerate()
                .map(|(i, l)| match i % 3 {
                    0 => format!("\u{520A}{l}"),
                    1 => format!("\u{0A0A}\u{FEFF}{l}"),
                    _ => l.to_owned(),
                })
                .collect()
        }

        fn encode(text: &str, be: bool, bom: bool) -> Vec<u8> {
            let mut out = Vec::new();
            for u in std::iter::once(0xFEFF)
                .filter(|_| bom)
                .chain(text.encode_utf16())
            {
                out.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
            }
            out
        }

        #[test]
        fn generated_multi_chunk_matches_reference() {
            let text = generated_text();
            for (be, bom) in [(false, true), (true, true), (false, false), (true, false)] {
                let data = encode(&text, be, bom);
                let enc = if be {
                    Encoding::UTF_16BE
                } else {
                    Encoding::UTF_16LE
                };
                assert_eq!(detect(&data), enc);
                let idx = index(enc, &data);
                assert_eq!(
                    decoded_lines(enc, &idx, &data),
                    decoded_reference(enc, &data),
                    "be={be} bom={bom}"
                );
            }
        }

        #[test]
        fn incremental_with_odd_cuts() {
            let text: String = generated_text().chars().take(20_000).collect();
            for be in [false, true] {
                let data = encode(&text, be, true);
                let enc = if be {
                    Encoding::UTF_16BE
                } else {
                    Encoding::UTF_16LE
                };
                let mut idx = LineIndex::new(enc.line_feed(), 2);
                let mut cut = 0;
                for step in [1, 1, 1, 2, 3, 97, 1001, 4096, 7, 13_001] {
                    cut = (cut + step).min(data.len());
                    idx.extend(&data[..cut]);
                    assert_eq!(
                        decoded_lines(enc, &idx, &data[..cut]),
                        decoded_reference(enc, &data[..cut]),
                        "be={be} cut={cut}"
                    );
                }
                idx.extend(&data);
                assert_eq!(idx.line_count(), index(enc, &data).line_count());
            }
        }

        #[test]
        fn tab_widths_in_code_units() {
            // Tabs, and units holding a 0x09 or 0x0D byte that aren't tabs
            // or carriage returns (U+0909, U+0D0D, U+090D).
            let ascii = String::from_utf8(generated_with_tabs(CHUNK + CHUNK / 3)).unwrap();
            let text: String = ascii
                .split_inclusive('\n')
                .enumerate()
                .map(|(i, l)| match i % 4 {
                    0 => format!("\u{0909}{l}"),
                    1 => format!("\t\u{0D0D}\u{090D}{l}"),
                    _ => l.to_owned(),
                })
                .collect();
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut lines: Vec<&[u16]> = units.split(|&u| u == 0x0A).collect();
            let last = lines.pop().unwrap();
            let width = |l: &[u16]| {
                l.iter()
                    .fold(0, |c, &u| if u == 9 { tab_stop(c) } else { c + 1 })
            };
            let want = lines
                .iter()
                .map(|l| width(l.strip_suffix(&[0x0D]).unwrap_or(l)))
                .chain([width(last)])
                .max()
                .unwrap();
            for be in [false, true] {
                let data = encode(&text, be, true);
                let enc = if be {
                    Encoding::UTF_16BE
                } else {
                    Encoding::UTF_16LE
                };
                assert_eq!(index(enc, &data).max_line_len(), want, "be={be}");
            }
        }

        #[test]
        fn bom_only_has_no_lines() {
            let idx = index(Encoding::UTF_16LE, b"\xFF\xFE");
            assert_eq!(idx.line_count(), 0);
            let idx = index(Encoding::UTF_8, b"\xEF\xBB\xBFa\n");
            assert_eq!(
                idx.lines(b"\xEF\xBB\xBFa\n", 0..1).collect::<Vec<_>>(),
                [b"a"]
            );
        }
    }
}
