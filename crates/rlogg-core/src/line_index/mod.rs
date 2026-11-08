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
//! TODO(encoding): only single-byte line feeds for now. UTF-16 needs klogg's
//! `lineFeedWidth` handling, and line lengths will need tab expansion.

mod compressed;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

pub use compressed::CompressedOffsets;

/// Bytes scanned by one rayon task.
const CHUNK: usize = 4 << 20;

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
#[derive(Debug, Default, Clone)]
pub struct LineIndex {
    /// Offset of the first byte of each line.
    starts: CompressedOffsets,
    /// Number of bytes of the buffer covered by the index.
    len: u64,
    /// Longest line among all lines except the last, in bytes.
    max_terminated_len: u64,
    /// Length of the last line, which may still grow on [`extend`](Self::extend).
    last_len: u64,
}

impl LineIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the index of `data` (parallel, not cancellable).
    pub fn build(data: &[u8]) -> Self {
        let mut index = Self::new();
        index.extend(data);
        index
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
        let total = data.len() as u64;
        assert!(total >= self.len, "buffer shrank: reindex from scratch");
        while self.len < total {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled);
            }
            let end = (self.len as usize).saturating_add(batch).min(data.len());
            self.extend_to(data, end);
            on_progress(IndexProgress {
                done: self.len,
                total,
            });
        }
        Ok(())
    }

    /// Indexes `data[self.len..end]`.
    fn extend_to(&mut self, data: &[u8], end: usize) {
        let old = self.len as usize;
        debug_assert!(old < end);

        // A line starts at `old` if the old buffer was empty or ended on `\n`:
        // a start is only recorded once there is a byte after the terminator.
        if old == 0 || data[old - 1] == b'\n' {
            self.push_start(data, old as u64);
        }

        // Positions after each `\n` in the new bytes, chunk by chunk in parallel.
        let chunks: Vec<Vec<u64>> = (old..end)
            .step_by(CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|from| {
                let to = (from + CHUNK).min(end);
                memchr::memchr_iter(b'\n', &data[from..to])
                    .map(|i| (from + i + 1) as u64)
                    .filter(|&s| s < end as u64)
                    .collect()
            })
            .collect();
        for starts in chunks {
            for s in starts {
                self.push_start(data, s);
            }
        }

        self.len = end as u64;
        if let Some(last) = self.starts.last() {
            self.last_len = content_len(data, last as usize, end);
        }
    }

    fn push_start(&mut self, data: &[u8], start: u64) {
        if let Some(prev) = self.starts.last() {
            let len = content_len(data, prev as usize, start as usize);
            self.max_terminated_len = self.max_terminated_len.max(len);
        }
        self.starts.push(start);
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Number of bytes of the buffer covered by the index.
    pub fn indexed_len(&self) -> u64 {
        self.len
    }

    /// Length in bytes of the longest line, without terminator (klogg:
    /// `getMaxLength`, which also expands tabs; not done yet).
    pub fn max_line_len(&self) -> u64 {
        self.max_terminated_len.max(self.last_len)
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
        self.line_span(n).map(|span| strip_eol(&data[span]))
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
        (1..bounds.len().max(1))
            .map(move |k| strip_eol(&data[bounds[k - 1] as usize..bounds[k] as usize]))
    }

    fn check_buffer(&self, data: &[u8]) {
        debug_assert!(
            data.len() as u64 >= self.len,
            "index built from another buffer"
        );
    }
}

fn strip_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Length of the line occupying `data[start..end]`, without terminator.
fn content_len(data: &[u8], start: usize, end: usize) -> u64 {
    strip_eol(&data[start..end]).len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

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
        body.split(|&b| b == b'\n').map(strip_eol).collect()
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
        let mut idx = LineIndex::new();
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
        let mut idx = LineIndex::new();
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
}
