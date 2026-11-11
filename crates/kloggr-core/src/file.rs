//! A log file and its line index (klogg: `FileHolder` + `LogData`).
//!
//! The file is mapped into memory to be indexed. Afterwards its bytes come
//! from the mapping ([`Access::Map`], Linux and macOS) or from reads at an
//! offset ([`Access::Read`], Windows): Windows refuses to truncate a file
//! that is mapped, so a kept mapping would stop the program writing the log
//! from truncating it (`copytruncate` rotation). Reading also never crashes
//! on a file truncated under it, where a mapping raises SIGBUS.

use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use memmap2::Mmap;

use crate::encoding::{self, Encoding};
use crate::line_index::{Cancelled, IndexProgress, LineIndex};

/// Options for [`LogFile::open_with`].
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenOptions {
    /// Use this encoding instead of the detected one (klogg: forced encoding).
    pub encoding: Option<Encoding>,
    /// Remove ANSI colour sequences from the text of lines (klogg:
    /// `hideAnsiColorSequences`, a prefilter on decoded lines).
    pub hide_ansi: bool,
    /// How the bytes are read once the file is indexed.
    pub access: Access,
}

/// How a [`LogFile`]'s bytes are read once it is indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Kept mapped: lines are borrowed from the mapping.
    Map,
    /// Read at an offset when needed, the mapping dropped after indexing.
    Read,
}

impl Default for Access {
    /// [`Read`](Self::Read) on Windows (see the module docs) or with the
    /// `KLOGGR_READ_FILES` environment variable set, else
    /// [`Map`](Self::Map).
    fn default() -> Self {
        if cfg!(windows) || std::env::var_os("KLOGGR_READ_FILES").is_some() {
            Self::Read
        } else {
            Self::Map
        }
    }
}

/// Where a snapshot's bytes come from.
#[derive(Clone)]
enum Source {
    /// The file is empty (mapping 0 bytes fails on some platforms).
    Empty,
    /// Shared by snapshots that only differ by encoding or ANSI hiding.
    Mapped(Arc<Mmap>),
    /// The first `len` bytes of `file`, read when needed.
    Read { file: Arc<File>, len: usize },
}

impl Source {
    fn new(map: Option<Mmap>, file: File, access: Access) -> Self {
        match (map, access) {
            (None, _) => Self::Empty,
            (Some(map), Access::Map) => Self::Mapped(Arc::new(map)),
            (Some(map), Access::Read) => Self::Read {
                file: Arc::new(file),
                len: map.len(),
            },
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Mapped(map) => map.len(),
            Self::Read { len, .. } => *len,
        }
    }

    /// Bytes `range` (clamped) of the snapshot. Read from the file when it
    /// isn't mapped: a file truncated since then gives fewer bytes.
    fn get(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        let end = range.end.min(self.len());
        let start = range.start.min(end);
        match self {
            Self::Empty => Cow::Borrowed(&[]),
            Self::Mapped(map) => Cow::Borrowed(&map[start..end]),
            Self::Read { file, .. } => {
                let mut buf = vec![0; end - start];
                let n = read_at_most(file, &mut buf, start as u64);
                buf.truncate(n);
                Cow::Owned(buf)
            }
        }
    }
}

/// Reads up to `buf.len()` bytes at `offset`; returns how many were read
/// before the end of the file or an error.
fn read_at_most(file: &File, buf: &mut [u8], offset: u64) -> usize {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[done..], offset + done as u64);
        #[cfg(windows)]
        let n =
            std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], offset + done as u64);
        match n {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    done
}

/// `buf[range]`, clamped: lines past a truncation read as empty.
fn clamped(buf: &[u8], range: Range<usize>) -> &[u8] {
    let end = range.end.min(buf.len());
    &buf[range.start.min(end)..end]
}

/// Lines read together: one read for the whole range (see
/// [`LogFile::text_block`]).
pub struct TextBlock<'a> {
    data: Cow<'a, [u8]>,
    /// Line starts relative to `data`, then the end of the last line.
    bounds: Vec<usize>,
    lf: encoding::LineFeed,
    strip: Option<Encoding>,
}

impl TextBlock<'_> {
    /// The block's bytes and each line's start in them, then the end of
    /// the last line; `None` when lines are shown without their ANSI
    /// sequences, so their text differs from the bytes.
    pub fn raw(&self) -> Option<(&[u8], &[usize])> {
        self.strip
            .is_none()
            .then(|| (&self.data[..], &self.bounds[..]))
    }

    /// The lines as shown and searched (see [`LogFile::text_line`]).
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Cow<'_, [u8]>> + '_ {
        (1..self.bounds.len().max(1)).map(move |k| {
            let line = self
                .lf
                .strip_eol(clamped(&self.data, self.bounds[k - 1]..self.bounds[k]));
            match self.strip {
                Some(encoding) => strip_ansi(line, encoding),
                None => Cow::Borrowed(line),
            }
        })
    }
}

/// klogg's `AnsiColorSequenceRegex`: SGR (`m`) and erase-in-line (`K`)
/// sequences.
static ANSI: std::sync::LazyLock<regex::bytes::Regex> = std::sync::LazyLock::new(|| {
    regex::bytes::Regex::new(r"\x1B\[([0-9]{1,4}((;|:)[0-9]{1,3})*)?[mK]").expect("valid")
});

/// `line` without ANSI colour sequences. Only ASCII-compatible encodings:
/// in UTF-16 the escape is two bytes, and lines are left as they are.
fn strip_ansi(line: &[u8], encoding: Encoding) -> Cow<'_, [u8]> {
    if encoding.line_feed().width != 1 || !line.contains(&0x1B) {
        return Cow::Borrowed(line);
    }
    match ANSI.replace_all(line, &b""[..]) {
        Cow::Borrowed(_) => Cow::Borrowed(line),
        Cow::Owned(v) => Cow::Owned(v),
    }
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

/// A log file together with its line index.
///
/// A `LogFile` is a snapshot: when the file grows,
/// [`reload_grown`](Self::reload_grown) makes a new one that shares the
/// already-indexed part, and readers of the old one keep reading theirs.
pub struct LogFile {
    path: PathBuf,
    options: OpenOptions,
    source: Source,
    index: LineIndex,
    encoding: Encoding,
    detected_encoding: Encoding,
    id: FileId,
    /// When the file was last modified, as of this snapshot.
    modified: Option<std::time::SystemTime>,
    /// The first and last [`SAMPLE`] bytes of the mapping, to detect
    /// rewrites.
    head: Vec<u8>,
    tail: Vec<u8>,
}

/// Maps `path` (`None` for an empty file) and returns the open file and its
/// identity.
fn map_file(path: &Path) -> Result<(Option<Mmap>, File, FileId), OpenError> {
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
        // reads of a truncated region raise SIGBUS. Accepted risk with
        // `Access::Map`, same as klogg; follow mode checks for truncation
        // (`LogFile::check`) and opens the file again before reading past a
        // change.
        Some(unsafe { Mmap::map(&file) }.map_err(io_err)?)
    };
    Ok((map, file, FileId::of(&meta)))
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
        let (map, file, id) = map_file(&path)?;
        let data = map.as_deref().unwrap_or_default();
        let detected_encoding =
            encoding::detect(&data[..data.len().min(encoding::DETECTION_SAMPLE)]);
        let encoding = options.encoding.unwrap_or(detected_encoding);
        let mut index = LineIndex::new(encoding.line_feed(), encoding.bom_len(data));
        index.extend_with(data, cancel, on_progress)?;
        Ok(Self::assemble(
            path,
            options,
            (map, file),
            index,
            encoding,
            detected_encoding,
            id,
        ))
    }

    fn assemble(
        path: PathBuf,
        options: OpenOptions,
        (map, file): (Option<Mmap>, File),
        index: LineIndex,
        encoding: Encoding,
        detected_encoding: Encoding,
        id: FileId,
    ) -> Self {
        let data = map.as_deref().unwrap_or_default();
        let head = data[..data.len().min(SAMPLE)].to_vec();
        let tail = data[data.len().saturating_sub(SAMPLE)..].to_vec();
        let modified = file.metadata().and_then(|m| m.modified()).ok();
        Self {
            modified,
            path,
            options,
            source: Source::new(map, file, options.access),
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

    /// The same snapshot decoded with another encoding (`None`: the
    /// detected one), without indexing again. `None` if the line layout
    /// differs (UTF-16 against a single-byte encoding, or a byte-order mark
    /// only one of them skips): the file must then be opened again with
    /// [`OpenOptions::encoding`] (klogg: `setDisplayEncoding`, which also
    /// reindexes when the line feed changes).
    pub fn with_encoding(&self, encoding: Option<Encoding>) -> Option<Self> {
        let new = encoding.unwrap_or(self.detected_encoding);
        // A byte-order mark is at most 3 bytes.
        let data = self.source.get(0..4);
        if new.line_feed() != self.encoding.line_feed()
            || new.bom_len(&data) != self.encoding.bom_len(&data)
        {
            return None;
        }
        Some(Self {
            path: self.path.clone(),
            options: OpenOptions {
                encoding,
                ..self.options
            },
            source: self.source.clone(),
            index: self.index.clone(),
            encoding: new,
            detected_encoding: self.detected_encoding,
            id: self.id,
            modified: self.modified,
            head: self.head.clone(),
            tail: self.tail.clone(),
        })
    }

    /// Writes `lines` (ascending, `count` of them) to `path` as they are in
    /// the file, line endings included (klogg: `saveLinesToFile`, which
    /// encodes the decoded text again). Adjacent lines are written in one
    /// piece. The data goes to a temporary file next to `path`, renamed
    /// once complete, so a cancelled or failed save leaves `path` as it was
    /// (klogg: `QSaveFile`). Cancelling returns an `Interrupted` error.
    pub fn save_lines(
        &self,
        path: &Path,
        lines: impl IntoIterator<Item = usize>,
        count: usize,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(u8),
    ) -> std::io::Result<()> {
        use std::io::Write;
        use std::sync::atomic::Ordering;

        /// Bytes written in one piece at most, so that progress moves.
        const PIECE: usize = 8 << 20;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let part = path.with_file_name(format!(".{name}.kloggr-part"));
        let result = (|| {
            let mut out = std::io::BufWriter::new(File::create(&part)?);
            let mut pending: Option<Range<usize>> = None;
            let mut last_percent = None;
            for (done, line) in lines.into_iter().enumerate() {
                let Some(span) = self.index.line_span(line) else {
                    break;
                };
                pending = match pending {
                    Some(p) if p.end == span.start && p.len() < PIECE => Some(p.start..span.end),
                    Some(p) => {
                        out.write_all(&self.source.get(p))?;
                        Some(span)
                    }
                    None => Some(span),
                };
                let percent = ((done + 1) * 100 / count.max(1)).min(100) as u8;
                if last_percent.replace(percent) != Some(percent) {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(std::io::ErrorKind::Interrupted.into());
                    }
                    on_progress(percent);
                }
            }
            if let Some(p) = pending {
                out.write_all(&self.source.get(p))?;
            }
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            std::fs::rename(&part, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result
    }

    /// The same snapshot with ANSI colour sequences hidden or shown (see
    /// [`OpenOptions::hide_ansi`]); nothing is indexed again.
    pub fn with_hide_ansi(&self, hide: bool) -> Self {
        Self {
            path: self.path.clone(),
            options: OpenOptions {
                hide_ansi: hide,
                ..self.options
            },
            source: self.source.clone(),
            index: self.index.clone(),
            encoding: self.encoding,
            detected_encoding: self.detected_encoding,
            id: self.id,
            modified: self.modified,
            head: self.head.clone(),
            tail: self.tail.clone(),
        }
    }

    /// The options the file was opened with.
    pub fn options(&self) -> OpenOptions {
        self.options
    }

    /// Compares the file on disk with this snapshot (klogg:
    /// `CheckFileChangesOperation`). Reads at most 2 × 64 KiB. Errors (the
    /// file is gone, e.g. during a rotation) are for the caller to retry.
    pub fn check(&self) -> std::io::Result<FileChange> {
        let file = File::open(&self.path)?;
        let meta = file.metadata()?;
        let len = self.source.len() as u64;
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
        let (map, file, id) = map_file(&self.path)?;
        let data = map.as_deref().unwrap_or_default();
        let mut index = self.index.clone();
        index.extend_with(data, cancel, on_progress)?;
        Ok(Self::assemble(
            self.path.clone(),
            self.options,
            (map, file),
            index,
            self.encoding,
            self.detected_encoding,
            id,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When the file was last modified, as of this snapshot (klogg: the
    /// toolbar's date field).
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        self.modified
    }

    /// The size of this snapshot in bytes.
    pub fn byte_len(&self) -> usize {
        self.source.len()
    }

    pub fn line_count(&self) -> usize {
        self.index.line_count()
    }

    /// Raw bytes of line `n` (no terminator; see
    /// [`text_line`](Self::text_line) for the text as shown).
    pub fn line(&self, n: usize) -> Option<Cow<'_, [u8]>> {
        let span = self.index.line_span(n)?;
        let lf = self.encoding.line_feed();
        Some(match self.source.get(span) {
            Cow::Borrowed(b) => Cow::Borrowed(lf.strip_eol(b)),
            Cow::Owned(b) => Cow::Owned(lf.strip_eol(&b).to_vec()),
        })
    }

    /// The lines in `range` (clamped) as shown and searched, read together
    /// in one piece: what searching a block of lines uses.
    pub fn text_block(&self, range: Range<usize>) -> TextBlock<'_> {
        let bounds = self.index.line_bounds(range);
        let (Some(&first), Some(&last)) = (bounds.first(), bounds.last()) else {
            return TextBlock {
                data: Cow::Borrowed(&[]),
                bounds: Vec::new(),
                lf: self.encoding.line_feed(),
                strip: None,
            };
        };
        TextBlock {
            data: self.source.get(first as usize..last as usize),
            bounds: bounds.iter().map(|&b| (b - first) as usize).collect(),
            lf: self.encoding.line_feed(),
            strip: self.options.hide_ansi.then_some(self.encoding),
        }
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

    /// The bytes of line `n` as shown and searched: without ANSI colour
    /// sequences if [`OpenOptions::hide_ansi`] is set, else as
    /// [`line`](Self::line).
    pub fn text_line(&self, n: usize) -> Option<Cow<'_, [u8]>> {
        let line = self.line(n)?;
        Some(match (self.options.hide_ansi, line) {
            (false, line) => line,
            (true, Cow::Borrowed(line)) => strip_ansi(line, self.encoding),
            (true, Cow::Owned(line)) => Cow::Owned(strip_ansi(&line, self.encoding).into_owned()),
        })
    }

    /// [`text_line`](Self::text_line) for the lines in `range` (clamped).
    /// Borrowed from the mapping, or each line its own copy when the file
    /// is read: for many lines, [`text_block`](Self::text_block) reads less.
    pub fn text_lines(&self, range: Range<usize>) -> impl ExactSizeIterator<Item = Cow<'_, [u8]>> {
        let lines: Vec<Cow<'_, [u8]>> = match &self.source {
            Source::Mapped(map) => {
                let (hide, encoding) = (self.options.hide_ansi, self.encoding);
                self.index
                    .lines(map, range)
                    .map(|l| {
                        if hide {
                            strip_ansi(l, encoding)
                        } else {
                            Cow::Borrowed(l)
                        }
                    })
                    .collect()
            }
            _ => {
                let block = self.text_block(range);
                block.iter().map(|l| Cow::Owned(l.into_owned())).collect()
            }
        };
        lines.into_iter()
    }

    /// Decoded text of line `n` (see [`text_line`](Self::text_line)).
    /// Invalid sequences become U+FFFD.
    pub fn line_text(&self, n: usize) -> Option<Cow<'_, str>> {
        self.text_line(n).map(|l| decode_cow(self.encoding, l))
    }

    /// Decoded text of the lines in `range` (clamped).
    pub fn lines_text(&self, range: Range<usize>) -> impl ExactSizeIterator<Item = Cow<'_, str>> {
        let encoding = self.encoding;
        self.text_lines(range).map(move |l| decode_cow(encoding, l))
    }
}

/// Decodes bytes that may be owned (stripped of ANSI sequences).
fn decode_cow(encoding: Encoding, bytes: Cow<'_, [u8]>) -> Cow<'_, str> {
    match bytes {
        Cow::Borrowed(b) => encoding.decode(b),
        Cow::Owned(b) => Cow::Owned(encoding.decode(&b).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kloggr-{}-{name}", std::process::id()));
        File::create(&path).unwrap().write_all(content).unwrap();
        path
    }

    #[test]
    fn opens_and_indexes() {
        let path = temp_file("basic.log", b"first\nsecond\n");
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.line_count(), 2);
        assert_eq!(log.line(1).as_deref(), Some(&b"second"[..]));
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
            ..OpenOptions::default()
        };
        let log = LogFile::open_with(&path, opts, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(log.line_text(0).as_deref(), Some("caf\u{e9}"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn switching_encodings() {
        let path = temp_file("switch.log", b"caf\xc3\xa9\nx\n");
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.encoding(), Encoding::UTF_8);
        // Same layout: decoded again, nothing indexed.
        let latin = log.with_encoding(Encoding::for_label("latin1")).unwrap();
        assert_eq!(latin.line_text(0).as_deref(), Some("caf\u{c3}\u{a9}"));
        assert_eq!(latin.options().encoding, Encoding::for_label("latin1"));
        let auto = latin.with_encoding(None).unwrap();
        assert_eq!(auto.encoding(), Encoding::UTF_8);
        assert_eq!(auto.line_text(0).as_deref(), Some("caf\u{e9}"));
        // UTF-16 lays the lines out differently.
        assert!(log.with_encoding(Some(Encoding::UTF_16LE)).is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn saves_lines_as_they_are() {
        let path = temp_file("save-src.log", b"a\r\nb\nc\nd");
        let log = LogFile::open(&path).unwrap();
        let out = path.with_file_name(format!("kloggr-{}-saved.log", std::process::id()));
        let never = AtomicBool::new(false);
        let mut progress = Vec::new();
        log.save_lines(&out, 0..4, 4, &never, |p| progress.push(p))
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"a\r\nb\nc\nd");
        assert_eq!(progress, [25, 50, 75, 100]);
        log.save_lines(&out, [0, 2, 3], 3, &never, |_| {}).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"a\r\nc\nd");
        // Cancelled: the earlier file stays, no temporary file is left.
        let err = log
            .save_lines(&out, [1], 1, &AtomicBool::new(true), |_| {})
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&out).unwrap(), b"a\r\nc\nd");
        let dir: Vec<_> = std::fs::read_dir(out.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("saved.log.kloggr-part")
            })
            .collect();
        assert!(dir.is_empty());
        std::fs::remove_file(out).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hides_ansi_colour_sequences() {
        let path = temp_file(
            "ansi.log",
            b"\x1b[1;31mERROR\x1b[0m x\x1b[K\nplain\n\x1b[2Jkept\n",
        );
        // Mapped: lines without a sequence aren't copied.
        let options = OpenOptions {
            access: Access::Map,
            ..OpenOptions::default()
        };
        let log = LogFile::open_with(&path, options, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(
            log.line_text(0).unwrap(),
            "\u{1b}[1;31mERROR\u{1b}[0m x\u{1b}[K"
        );
        let hidden = log.with_hide_ansi(true);
        assert_eq!(hidden.line_text(0).unwrap(), "ERROR x");
        assert!(matches!(hidden.text_line(1), Some(Cow::Borrowed(b"plain"))));
        // Only colour and erase-in-line sequences, as in klogg.
        assert_eq!(hidden.line_text(2).unwrap(), "\u{1b}[2Jkept");
        assert_eq!(
            hidden.lines_text(0..2).collect::<Vec<_>>(),
            ["ERROR x", "plain"]
        );
        // The raw bytes stay as they are.
        assert_eq!(hidden.line(1).as_deref(), Some(&b"plain"[..]));
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

    fn open_read(path: &Path) -> LogFile {
        let options = OpenOptions {
            access: Access::Read,
            ..OpenOptions::default()
        };
        LogFile::open_with(path, options, &AtomicBool::new(false), |_| {}).unwrap()
    }

    #[test]
    fn reading_gives_what_mapping_gives() {
        let content = b"\x1b[31mred\x1b[0m\r\nplain\n\nlast";
        let path = temp_file("read.log", content);
        let read = open_read(&path);
        let mapped = LogFile::open(&path).unwrap();
        for log in [&read, &mapped] {
            assert_eq!(log.line_count(), 4);
            assert_eq!(log.byte_len(), content.len());
            assert_eq!(log.line(1).as_deref(), Some(&b"plain"[..]));
            assert_eq!(log.line(3).as_deref(), Some(&b"last"[..]));
            assert_eq!(log.line(4), None);
        }
        let all = |log: &LogFile| -> Vec<Vec<u8>> {
            log.text_block(0..10)
                .iter()
                .map(|l| l.into_owned())
                .collect()
        };
        assert_eq!(all(&read), all(&mapped));
        assert_eq!(
            read.text_lines(1..3).collect::<Vec<_>>(),
            mapped.text_lines(1..3).collect::<Vec<_>>()
        );
        let hidden = read.with_hide_ansi(true);
        assert_eq!(hidden.text_line(0).as_deref(), Some(&b"red"[..]));
        assert_eq!(all(&hidden)[0], b"red");
        assert_eq!(
            read.with_encoding(Some(Encoding::WINDOWS_1252))
                .unwrap()
                .line_count(),
            4
        );
        assert!(read.text_block(9..12).iter().next().is_none());

        let out = path.with_extension("saved");
        read.save_lines(&out, [1, 3], 2, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"plain\nlast");
        let _ = std::fs::remove_file(out);
        std::fs::remove_file(path).unwrap();
    }

    /// A snapshot that reads its file survives the file being truncated
    /// (with a mapping, that is SIGBUS; on Windows, the truncation fails).
    #[test]
    fn reading_a_truncated_file() {
        let path = temp_file("read-truncated.log", b"aaaa\nbbbb\ncccc\n");
        let log = open_read(&path);
        std::fs::write(&path, b"aaaa\nbb").unwrap();
        assert_eq!(log.line(0).as_deref(), Some(&b"aaaa"[..]));
        assert_eq!(log.line(1).as_deref(), Some(&b"bb"[..]));
        assert_eq!(log.line(2).as_deref(), Some(&b""[..]));
        let lines: Vec<_> = log
            .text_block(0..3)
            .iter()
            .map(|l| l.into_owned())
            .collect();
        assert_eq!(lines, [&b"aaaa"[..], b"bb", b""]);
        assert_eq!(log.check().unwrap(), FileChange::Replaced);
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
        assert!(LogFile::open("/nonexistent/kloggr.log").is_err());
    }
}
