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

/// Bytes compared at the start and at the end of the indexed part to tell
/// whether a file was rewritten (klogg: `fastModificationDetection`, which
/// hashes a header and a tail).
const SAMPLE: usize = 64 << 10;

/// What happened to the file on disk since it was opened (klogg:
/// `MonitoredFileStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Unchanged,
    /// Data was appended: [`LogFile::reload_grown`] indexes only the new part.
    Grew,
    /// Truncated, rewritten, or replaced by another file under the same name
    /// (log rotation): the file must be opened again (klogg: `Truncated`).
    Replaced,
}

/// Identity of the file behind a path: changes when a rotated log is
/// recreated (klogg: `FileId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl FileId {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: meta.dev(),
                ino: meta.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            Self {}
        }
    }
}

/// A log file mapped into memory together with its line index.
///
/// A `LogFile` is a snapshot: when the file grows,
/// [`reload_grown`](Self::reload_grown) makes a new one that shares the
/// already-indexed part, and readers of the old one keep a valid mapping.
pub struct LogFile {
    path: PathBuf,
    options: OpenOptions,
    /// `None` for an empty file: mapping 0 bytes fails on some platforms.
    map: Option<Mmap>,
    index: LineIndex,
    encoding: Encoding,
    detected_encoding: Encoding,
    id: FileId,
    /// The first and last [`SAMPLE`] bytes of the mapping, to detect
    /// rewrites.
    head: Vec<u8>,
    tail: Vec<u8>,
}

/// Maps `path` (`None` for an empty file) and returns its identity.
fn map_file(path: &Path) -> Result<(Option<Mmap>, FileId), OpenError> {
    let io_err = |source| OpenError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = File::open(path).map_err(io_err)?;
    let meta = file.metadata().map_err(io_err)?;
    let map = if meta.len() == 0 {
        None
    } else {
        // SAFETY: the file may be modified by another process while mapped;
        // reads of a truncated region raise SIGBUS. Accepted risk, same as
        // klogg; follow mode checks for truncation (`LogFile::check`) and
        // opens the file again before reading past a change.
        Some(unsafe { Mmap::map(&file) }.map_err(io_err)?)
    };
    Ok((map, FileId::of(&meta)))
}

fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let n = std::os::windows::fs::FileExt::seek_read(
                file,
                &mut buf[done..],
                offset + done as u64,
            )?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
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
        let (map, id) = map_file(&path)?;
        let data = map.as_deref().unwrap_or_default();
        let detected_encoding =
            encoding::detect(&data[..data.len().min(encoding::DETECTION_SAMPLE)]);
        let encoding = options.encoding.unwrap_or(detected_encoding);
        let mut index = LineIndex::new(encoding.line_feed(), encoding.bom_len(data));
        index.extend_with(data, cancel, on_progress)?;
        Ok(Self::assemble(
            path,
            options,
            map,
            index,
            encoding,
            detected_encoding,
            id,
        ))
    }

    fn assemble(
        path: PathBuf,
        options: OpenOptions,
        map: Option<Mmap>,
        index: LineIndex,
        encoding: Encoding,
        detected_encoding: Encoding,
        id: FileId,
    ) -> Self {
        let data = map.as_deref().unwrap_or_default();
        let head = data[..data.len().min(SAMPLE)].to_vec();
        let tail = data[data.len().saturating_sub(SAMPLE)..].to_vec();
        Self {
            path,
            options,
            map,
            index,
            encoding,
            detected_encoding,
            id,
            head,
            tail,
        }
    }

    /// Opens the file again from scratch with the same options, e.g. after
    /// [`FileChange::Replaced`] (klogg: `FullReindexOperation`).
    pub fn reopen(
        &self,
        cancel: &AtomicBool,
        on_progress: impl FnMut(IndexProgress),
    ) -> Result<Self, OpenError> {
        Self::open_with(&self.path, self.options, cancel, on_progress)
    }

    /// Compares the file on disk with this snapshot (klogg:
    /// `CheckFileChangesOperation`). Reads at most 2 × 64 KiB. Errors (the
    /// file is gone, e.g. during a rotation) are for the caller to retry.
    pub fn check(&self) -> std::io::Result<FileChange> {
        let file = File::open(&self.path)?;
        let meta = file.metadata()?;
        let len = self.bytes().len() as u64;
        if FileId::of(&meta) != self.id || meta.len() < len || (meta.len() == 0 && len > 0) {
            return Ok(FileChange::Replaced);
        }
        let mut buf = vec![0; self.head.len()];
        read_exact_at(&file, &mut buf, 0)?;
        if buf != self.head {
            return Ok(FileChange::Replaced);
        }
        buf.resize(self.tail.len(), 0);
        read_exact_at(&file, &mut buf, len - self.tail.len() as u64)?;
        if buf != self.tail {
            return Ok(FileChange::Replaced);
        }
        Ok(if meta.len() > len {
            FileChange::Grew
        } else {
            FileChange::Unchanged
        })
    }

    /// A new snapshot of the file after data was appended: maps it again
    /// and indexes only the new bytes (klogg: `PartialIndexOperation`). The
    /// index of the old part is shared, not copied. Call it after
    /// [`check`](Self::check) returned [`FileChange::Grew`].
    pub fn reload_grown(
        &self,
        cancel: &AtomicBool,
        on_progress: impl FnMut(IndexProgress),
    ) -> Result<Self, OpenError> {
        let (map, id) = map_file(&self.path)?;
        let data = map.as_deref().unwrap_or_default();
        let mut index = self.index.clone();
        index.extend_with(data, cancel, on_progress)?;
        Ok(Self::assemble(
            self.path.clone(),
            self.options,
            map,
            index,
            self.encoding,
            self.detected_encoding,
            id,
        ))
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

    fn append(path: &Path, content: &[u8]) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(content)
            .unwrap();
    }

    #[test]
    fn grows_incrementally() {
        let path = temp_file("grow.log", b"one\ntw");
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Unchanged);
        append(&path, b"o\nthree\n");
        assert_eq!(log.check().unwrap(), FileChange::Grew);
        let grown = log.reload_grown(&AtomicBool::new(false), |_| {}).unwrap();
        // The unterminated last line got longer.
        assert_eq!(
            grown.lines_text(0..9).collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
        assert_eq!(grown.check().unwrap(), FileChange::Unchanged);
        // The old snapshot still reads its own content.
        assert_eq!(log.lines_text(0..9).collect::<Vec<_>>(), ["one", "tw"]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn detects_truncation_and_rewrites() {
        let path = temp_file("truncate.log", b"aaaa\nbbbb\n");
        let log = LogFile::open(&path).unwrap();
        std::fs::write(&path, b"aaaa\n").unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
        // Same size, different content.
        std::fs::write(&path, b"aaaa\nbbbc\n").unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
        // Rewritten and longer: the old part changed, so not "grew".
        std::fs::write(&path, b"xaaa\nbbbb\nmore\n").unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
        let again = log.reopen(&AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(again.line_count(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn detects_rotation() {
        let path = temp_file("rotate.log", b"old\n");
        let log = LogFile::open(&path).unwrap();
        let rotated = path.with_extension("log.1");
        std::fs::rename(&path, &rotated).unwrap();
        assert!(log.check().is_err(), "gone during the rotation");
        // A new file with the same content and name is another file.
        std::fs::write(&path, b"old\nnew\n").unwrap();
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(rotated).unwrap();
    }

    #[test]
    fn empty_file_grows() {
        let path = temp_file("empty-grow.log", b"");
        let log = LogFile::open(&path).unwrap();
        append(&path, b"first\n");
        assert_eq!(log.check().unwrap(), FileChange::Grew);
        let grown = log.reload_grown(&AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(grown.line_text(0).as_deref(), Some("first"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(LogFile::open("/nonexistent/rlogg.log").is_err());
    }
}
