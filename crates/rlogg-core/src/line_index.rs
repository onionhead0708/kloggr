//! Line start offsets for a byte buffer.
//!
//! First, simple version: a `Vec<u64>` of line-start offsets. klogg stores these
//! compressed (`CompressedLineStorage`) and builds them in parallel
//! (`LogDataWorker`); do the same once benchmarks on multi-GB files exist.

/// Index of line boundaries in a buffer.
///
/// Lines are terminated by `\n`; a `\r` before it is not part of the line.
/// A final line without a terminator still counts. An empty buffer has 0 lines.
#[derive(Debug, Default, Clone)]
pub struct LineIndex {
    /// Offset of the first byte of each line.
    starts: Vec<u64>,
    /// Length of the indexed buffer.
    len: u64,
}

impl LineIndex {
    /// Builds the index by scanning `data` for `\n`.
    pub fn build(data: &[u8]) -> Self {
        let mut starts = Vec::new();
        if !data.is_empty() {
            starts.push(0);
        }
        starts.extend(
            data.iter()
                .enumerate()
                .filter(|&(i, &b)| b == b'\n' && i + 1 < data.len())
                .map(|(i, _)| i as u64 + 1),
        );
        Self {
            starts,
            len: data.len() as u64,
        }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Content of line `n` in `data` (the buffer this index was built from),
    /// without its `\n` / `\r\n` terminator.
    pub fn line<'a>(&self, data: &'a [u8], n: usize) -> Option<&'a [u8]> {
        debug_assert_eq!(
            data.len() as u64,
            self.len,
            "index built from another buffer"
        );
        let start = *self.starts.get(n)? as usize;
        let end = self.starts.get(n + 1).map_or(data.len(), |&e| e as usize);
        let mut line = &data[start..end];
        if let Some(rest) = line.strip_suffix(b"\n") {
            line = rest;
        }
        if let Some(rest) = line.strip_suffix(b"\r") {
            line = rest;
        }
        Some(line)
    }
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

    #[test]
    fn empty_buffer_has_no_lines() {
        assert_eq!(LineIndex::build(b"").line_count(), 0);
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
    fn out_of_range_line_is_none() {
        let data = b"x\n";
        assert!(LineIndex::build(data).line(data, 1).is_none());
    }
}
