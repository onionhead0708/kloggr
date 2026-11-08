//! Memory-mapped log file (klogg: `FileHolder` + `LogData`).

use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use memmap2::Mmap;

use crate::encoding::{self, Encoding};
use crate::line_index::{Cancelled, IndexProgress, LineIndex};

/// Options for [`LogFile::open_with`].
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenOptions {
    /// Use this encoding instead of the detected one (klogg: forced encoding).
    pub encoding: Option<Encoding>,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("cannot open {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Cancelled(#[from] Cancelled),
}

/// A log file mapped into memory together with its line index.
///
/// TODO: handle growth/truncation (follow mode), which invalidates the
/// mapping; klogg re-maps and indexes only the appended part.
pub struct LogFile {
    path: PathBuf,
    /// `None` for an empty file: mapping 0 bytes fails on some platforms.
    map: Option<Mmap>,
    index: LineIndex,
    encoding: Encoding,
    detected_encoding: Encoding,
}

impl LogFile {
    /// Maps and fully indexes `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        Self::open_with(
            path,
            OpenOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
    }

    /// Maps `path`, detects its encoding and indexes it, reporting progress
    /// and stopping early with [`OpenError::Cancelled`] once `cancel` is set.
    /// Blocks; call it off the UI thread (klogg: `FullIndexOperation`).
    ///
    /// To switch encodings, open the file again with
    /// [`OpenOptions::encoding`]: the line layout may change (klogg reindexes
    /// too).
    pub fn open_with(
        path: impl AsRef<Path>,
        options: OpenOptions,
        cancel: &AtomicBool,
        on_progress: impl FnMut(IndexProgress),
    ) -> Result<Self, OpenError> {
        let path = path.as_ref().to_path_buf();
        let io_err = |source| OpenError::Io {
            path: path.clone(),
            source,
        };
        let file = File::open(&path).map_err(io_err)?;
        let len = file.metadata().map_err(io_err)?.len();
        let map = if len == 0 {
            None
        } else {
            // SAFETY: the file may be modified by another process while mapped;
            // reads of a truncated region raise SIGBUS. Accepted risk, same as
            // klogg; follow mode must detect truncation before reading.
            Some(unsafe { Mmap::map(&file) }.map_err(io_err)?)
        };
        let data = map.as_deref().unwrap_or_default();
        let detected_encoding =
            encoding::detect(&data[..data.len().min(encoding::DETECTION_SAMPLE)]);
        let encoding = options.encoding.unwrap_or(detected_encoding);
        let mut index = LineIndex::new(encoding.line_feed(), encoding.bom_len(data));
        index.extend_with(data, cancel, on_progress)?;
        Ok(Self {
            path,
            map,
            index,
            encoding,
            detected_encoding,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> &[u8] {
        self.map.as_deref().unwrap_or_default()
    }

    pub fn line_count(&self) -> usize {
        self.index.line_count()
    }

    /// Raw bytes of line `n` (no terminator). Decoding is the caller's job until
    /// encoding support lands.
    pub fn line(&self, n: usize) -> Option<&[u8]> {
        self.index.line(self.bytes(), n)
    }

    /// Raw bytes of the lines in `range` (clamped), for rendering visible rows.
    pub fn lines(&self, range: Range<usize>) -> impl ExactSizeIterator<Item = &[u8]> {
        self.index.lines(self.bytes(), range)
    }

    pub fn index(&self) -> &LineIndex {
        &self.index
    }

    /// Encoding used to index and decode the file.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Encoding guessed from the file content (klogg: `getDetectedEncoding`).
    pub fn detected_encoding(&self) -> Encoding {
        self.detected_encoding
    }

    /// Decoded text of line `n`. Invalid sequences become U+FFFD.
    pub fn line_text(&self, n: usize) -> Option<Cow<'_, str>> {
        self.line(n).map(|l| self.encoding.decode(l))
    }

    /// Decoded text of the lines in `range` (clamped).
    pub fn lines_text(&self, range: Range<usize>) -> impl ExactSizeIterator<Item = Cow<'_, str>> {
        let encoding = self.encoding;
        self.lines(range).map(move |l| encoding.decode(l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rlogg-{}-{name}", std::process::id()));
        File::create(&path).unwrap().write_all(content).unwrap();
        path
    }

    #[test]
    fn opens_and_indexes() {
        let path = temp_file("basic.log", b"first\nsecond\n");
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.line_count(), 2);
        assert_eq!(log.line(1), Some(&b"second"[..]));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_file_has_no_lines() {
        let path = temp_file("empty.log", b"");
        assert_eq!(LogFile::open(&path).unwrap().line_count(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn open_reports_progress_and_can_be_cancelled() {
        let path = temp_file("progress.log", b"a\nb\n");
        let mut last = None;
        let opts = OpenOptions::default();
        LogFile::open_with(&path, opts, &AtomicBool::new(false), |p| last = Some(p)).unwrap();
        assert_eq!(last.map(|p| p.percent()), Some(100));
        let cancelled = LogFile::open_with(&path, opts, &AtomicBool::new(true), |_| {});
        assert!(matches!(cancelled, Err(OpenError::Cancelled(_))));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn detects_and_decodes_utf16() {
        let text = "caf\u{e9}\r\n\u{520A}\u{65e5}\u{672c}\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let path = temp_file("utf16.log", &bytes);
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.encoding(), Encoding::UTF_16LE);
        assert_eq!(
            log.lines_text(0..10).collect::<Vec<_>>(),
            ["caf\u{e9}", "\u{520A}\u{65e5}\u{672c}"]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn forced_encoding_overrides_detection() {
        let path = temp_file("latin1.log", b"caf\xe9\n");
        let opts = OpenOptions {
            encoding: Encoding::for_label("latin1"),
        };
        let log = LogFile::open_with(&path, opts, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(log.line_text(0).as_deref(), Some("caf\u{e9}"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(LogFile::open("/nonexistent/rlogg.log").is_err());
    }
}
