//! Opening compressed files and archives (klogg: `Decompressor`,
//! `src/ui/src/decompressor.cpp`, GPLv3, Copyright (C) 2021 Anton Filimonov
//! and other contributors).
//!
//! A compressed file (gzip, bzip2, xz, lzma) is decompressed to a temporary
//! file, which is then opened as a log. An archive (zip, 7z, tar, also
//! compressed) is extracted to a temporary directory, and the user picks
//! files from it. klogg finds the type from the MIME type, then the file
//! name; kloggr reads the magic bytes, then the file name.

use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// A stream compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Gzip,
    Bzip2,
    Xz,
    Lzma,
}

/// What a file is (klogg: `Archive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A plain file: open it as it is.
    Plain,
    /// One compressed file (klogg: `DecompressAction::Decompress`).
    Compressed(Compression),
    /// Several files (klogg: `DecompressAction::Extract`).
    Zip,
    SevenZip,
    Tar(Option<Compression>),
}

impl Kind {
    pub fn is_plain(self) -> bool {
        self == Kind::Plain
    }

    /// Whether this holds several files to choose from.
    pub fn is_archive(self) -> bool {
        matches!(self, Kind::Zip | Kind::SevenZip | Kind::Tar(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Format(String),
    #[error("cancelled")]
    Cancelled,
}

/// Bytes read to find the type: enough for the tar header's "ustar".
const SNIFF: usize = 512;

fn compression_of(head: &[u8]) -> Option<Compression> {
    if head.starts_with(&[0x1f, 0x8b]) {
        Some(Compression::Gzip)
    } else if head.starts_with(b"BZh") {
        Some(Compression::Bzip2)
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Some(Compression::Xz)
    } else {
        None
    }
}

fn is_tar(head: &[u8]) -> bool {
    head.get(257..262) == Some(b"ustar")
}

/// The extension says a compressed tar (`.tar.gz`, `.tgz`, …).
fn tar_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.contains(".tar.")
        || [".tgz", ".tbz", ".tbz2", ".txz"]
            .iter()
            .any(|e| name.ends_with(e))
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The type of the file at `path` (klogg: `archiveType`).
pub fn kind(path: &Path) -> io::Result<Kind> {
    let mut head = Vec::with_capacity(SNIFF);
    File::open(path)?
        .take(SNIFF as u64)
        .read_to_end(&mut head)?;
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Ok(Kind::Zip);
    }
    if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Ok(Kind::SevenZip);
    }
    if is_tar(&head) {
        return Ok(Kind::Tar(None));
    }
    let compression = compression_of(&head).or_else(|| {
        // Legacy .lzma has no magic number; trust the extension.
        (extension(path) == "lzma").then_some(Compression::Lzma)
    });
    let Some(compression) = compression else {
        return Ok(Kind::Plain);
    };
    if tar_name(path) || sniff_tar(path, compression) {
        return Ok(Kind::Tar(Some(compression)));
    }
    Ok(Kind::Compressed(compression))
}

/// Whether the decompressed content starts with a tar header.
fn sniff_tar(path: &Path, compression: Compression) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(SNIFF);
    match decoder(compression, BufReader::new(file)) {
        Ok(reader) => reader.take(SNIFF as u64).read_to_end(&mut head).is_ok() && is_tar(&head),
        Err(_) => false,
    }
}

/// A reader decompressing `input`.
fn decoder<'a>(
    compression: Compression,
    input: impl io::BufRead + 'a,
) -> io::Result<Box<dyn Read + 'a>> {
    Ok(match compression {
        Compression::Gzip => Box::new(flate2::bufread::MultiGzDecoder::new(input)),
        Compression::Bzip2 => Box::new(bzip2::bufread::MultiBzDecoder::new(input)),
        Compression::Xz | Compression::Lzma => {
            // lzma-rs decodes into a writer: run it on a pipe-like buffer.
            Box::new(LzmaReader::new(compression, input)?)
        }
    })
}

/// lzma-rs only decodes whole streams into a `Write`; this decodes the
/// stream into memory in one go and reads from there. Logs compressed this
/// way are rare and klogg reads them whole too.
struct LzmaReader {
    data: io::Cursor<Vec<u8>>,
}

impl LzmaReader {
    fn new(compression: Compression, mut input: impl io::BufRead) -> io::Result<Self> {
        let mut out = Vec::new();
        let result = match compression {
            Compression::Xz => lzma_rs::xz_decompress(&mut input, &mut out),
            _ => lzma_rs::lzma_decompress(&mut input, &mut out),
        };
        result.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self {
            data: io::Cursor::new(out),
        })
    }
}

impl Read for LzmaReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.read(buf)
    }
}

/// Counts bytes read from the archive, for progress, and stops when
/// cancelled.
struct Progress<'a, R> {
    inner: R,
    read: u64,
    total: u64,
    cancel: &'a AtomicBool,
    on_progress: &'a mut dyn FnMut(u8),
    last: u8,
}

impl<R: Read> Read for Progress<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        let percent = (self.read * 100)
            .checked_div(self.total)
            .unwrap_or(100)
            .min(100) as u8;
        if percent != self.last {
            self.last = percent;
            (self.on_progress)(percent);
        }
        Ok(n)
    }
}

fn cancelled(e: io::Error, cancel: &AtomicBool) -> ArchiveError {
    if cancel.load(Ordering::Relaxed) {
        ArchiveError::Cancelled
    } else {
        ArchiveError::Io(e)
    }
}

/// The name of the decompressed file: the archive's without its extension
/// (`app.log.gz` → `app.log`).
fn decompressed_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or(path.as_os_str());
    PathBuf::from(stem)
}

/// Decompresses the compressed file at `path` into `dir` (klogg:
/// `Decompressor::decompress`) and returns the new file. Calls
/// `on_progress` with the percentage of the input read; stops when
/// `cancel` is set. Blocks: call it off the UI thread.
pub fn decompress(
    path: &Path,
    compression: Compression,
    dir: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u8),
) -> Result<PathBuf, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    let input = File::open(path)?;
    let total = input.metadata()?.len();
    let reader = BufReader::new(Progress {
        inner: input,
        read: 0,
        total,
        cancel,
        on_progress,
        last: 0,
    });
    let out_path = dir.join(decompressed_name(path));
    let mut out = io::BufWriter::new(File::create(&out_path)?);
    let result = decoder(compression, reader)
        .and_then(|mut d| io::copy(&mut d, &mut out))
        .and_then(|_| out.flush());
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out_path);
        return Err(cancelled(e, cancel));
    }
    Ok(out_path)
}

/// Extracts the archive at `path` into `dir` (klogg:
/// `Decompressor::extract`) and returns the extracted files, sorted.
/// Entries whose path would leave `dir` are skipped. Blocks.
pub fn extract(
    path: &Path,
    kind: Kind,
    dir: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u8),
) -> Result<Vec<PathBuf>, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    let input = File::open(path)?;
    let total = input.metadata()?.len();
    let progress = Progress {
        inner: input,
        read: 0,
        total,
        cancel,
        on_progress,
        last: 0,
    };
    match kind {
        Kind::Tar(compression) => {
            let reader: Box<dyn Read> = match compression {
                Some(c) => {
                    decoder(c, BufReader::new(progress)).map_err(|e| cancelled(e, cancel))?
                }
                None => Box::new(progress),
            };
            let mut archive = tar::Archive::new(reader);
            for entry in archive.entries().map_err(|e| cancelled(e, cancel))? {
                let mut entry = entry.map_err(|e| cancelled(e, cancel))?;
                // `unpack_in` refuses paths outside `dir`.
                entry.unpack_in(dir).map_err(|e| cancelled(e, cancel))?;
            }
        }
        Kind::Zip => {
            drop(progress);
            let mut zip = zip::ZipArchive::new(File::open(path)?)
                .map_err(|e| ArchiveError::Format(e.to_string()))?;
            let count = zip.len();
            for i in 0..count {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ArchiveError::Cancelled);
                }
                let mut file = zip
                    .by_index(i)
                    .map_err(|e| ArchiveError::Format(e.to_string()))?;
                let Some(name) = file.enclosed_name() else {
                    continue;
                };
                let target = dir.join(name);
                if file.is_dir() {
                    std::fs::create_dir_all(&target)?;
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                io::copy(&mut file, &mut File::create(&target)?)?;
                on_progress(((i + 1) * 100 / count.max(1)) as u8);
            }
        }
        Kind::SevenZip => {
            drop(progress);
            sevenz_rust2::decompress_file(path, dir)
                .map_err(|e| ArchiveError::Format(e.to_string()))?;
            on_progress(100);
        }
        Kind::Plain | Kind::Compressed(_) => {
            return Err(ArchiveError::Format("not an archive".into()));
        }
    }
    let mut files = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// A new directory under the system temporary directory for files taken
/// out of `archive` (klogg: `QTemporaryDir` in its temp dir).
pub fn temp_dir_for(root: &Path, archive: &Path) -> PathBuf {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into());
    let mut n = 0;
    loop {
        let dir = root.join(if n == 0 {
            name.clone()
        } else {
            format!("{name}-{n}")
        });
        if !dir.exists() {
            return dir;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kloggr-archive-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const TEXT: &[u8] = b"line 1\nline 2 ERROR\nline 3\n";

    fn no_progress() -> impl FnMut(u8) {
        |_| {}
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn detects_kinds() {
        let t = Temp::new("kinds");
        let write = |name: &str, data: &[u8]| {
            let p = t.0.join(name);
            std::fs::write(&p, data).unwrap();
            p
        };
        assert_eq!(kind(&write("a.log", TEXT)).unwrap(), Kind::Plain);
        assert_eq!(
            kind(&write("a.log.gz", &gzip(TEXT))).unwrap(),
            Kind::Compressed(Compression::Gzip)
        );
        // Magic bytes win over the name.
        assert_eq!(
            kind(&write("misnamed.txt", &gzip(TEXT))).unwrap(),
            Kind::Compressed(Compression::Gzip)
        );
        let tar = tar_of(&[("x.log", TEXT)]);
        assert_eq!(kind(&write("a.tar", &tar)).unwrap(), Kind::Tar(None));
        assert_eq!(
            kind(&write("a.tgz", &gzip(&tar))).unwrap(),
            Kind::Tar(Some(Compression::Gzip))
        );
        // A gzipped tar found by content.
        assert_eq!(
            kind(&write("b.gz", &gzip(&tar))).unwrap(),
            Kind::Tar(Some(Compression::Gzip))
        );
        assert_eq!(kind(&write("z.zip", b"PK\x03\x04rest")).unwrap(), Kind::Zip);
        assert_eq!(
            kind(&write("s.7z", b"7z\xbc\xaf\x27\x1c rest")).unwrap(),
            Kind::SevenZip
        );
        assert_eq!(kind(&write("e", b"")).unwrap(), Kind::Plain);
    }

    #[test]
    fn decompresses_streams() {
        let t = Temp::new("decompress");
        let cancel = AtomicBool::new(false);
        let gz = t.0.join("app.log.gz");
        std::fs::write(&gz, gzip(TEXT)).unwrap();
        let mut seen = Vec::new();
        let out = decompress(
            &gz,
            Compression::Gzip,
            &t.0.join("out"),
            &cancel,
            &mut |p| seen.push(p),
        )
        .unwrap();
        assert_eq!(out.file_name().unwrap(), "app.log");
        assert_eq!(std::fs::read(&out).unwrap(), TEXT);
        assert_eq!(seen.last(), Some(&100));

        let bz = t.0.join("b.log.bz2");
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        e.write_all(TEXT).unwrap();
        std::fs::write(&bz, e.finish().unwrap()).unwrap();
        let out = decompress(&bz, Compression::Bzip2, &t.0, &cancel, &mut no_progress()).unwrap();
        assert_eq!(std::fs::read(out).unwrap(), TEXT);

        let xz = t.0.join("c.log.xz");
        let mut data = Vec::new();
        lzma_rs::xz_compress(&mut io::Cursor::new(TEXT), &mut data).unwrap();
        std::fs::write(&xz, data).unwrap();
        assert_eq!(kind(&xz).unwrap(), Kind::Compressed(Compression::Xz));
        let out = decompress(&xz, Compression::Xz, &t.0, &cancel, &mut no_progress()).unwrap();
        assert_eq!(std::fs::read(out).unwrap(), TEXT);

        // Corrupt input fails and leaves nothing behind.
        let bad = t.0.join("bad.log.gz");
        std::fs::write(&bad, b"\x1f\x8bnot gzip").unwrap();
        let dir = t.0.join("bad");
        assert!(decompress(&bad, Compression::Gzip, &dir, &cancel, &mut no_progress()).is_err());
        assert!(!dir.join("bad.log").exists());
        // Cancelled.
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            decompress(
                &gz,
                Compression::Gzip,
                &t.0.join("c"),
                &cancel,
                &mut no_progress()
            ),
            Err(ArchiveError::Cancelled)
        ));
    }

    #[test]
    fn extracts_archives() {
        let t = Temp::new("extract");
        let cancel = AtomicBool::new(false);
        let tgz = t.0.join("logs.tar.gz");
        std::fs::write(
            &tgz,
            gzip(&tar_of(&[("a.log", TEXT), ("sub/b.log", b"b\n")])),
        )
        .unwrap();
        let dir = t.0.join("tgz");
        let files = extract(
            &tgz,
            Kind::Tar(Some(Compression::Gzip)),
            &dir,
            &cancel,
            &mut no_progress(),
        )
        .unwrap();
        assert_eq!(files, [dir.join("a.log"), dir.join("sub/b.log")]);
        assert_eq!(std::fs::read(&files[0]).unwrap(), TEXT);

        let zip_path = t.0.join("logs.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("one.log", opts).unwrap();
            w.write_all(TEXT).unwrap();
            w.add_directory("d/", opts).unwrap();
            w.start_file("d/two.log", opts).unwrap();
            w.write_all(b"2\n").unwrap();
            // A path leaving the directory is skipped (zip slip).
            w.start_file("../evil.log", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(kind(&zip_path).unwrap(), Kind::Zip);
        let dir = t.0.join("zip");
        let files = extract(&zip_path, Kind::Zip, &dir, &cancel, &mut no_progress()).unwrap();
        assert_eq!(files, [dir.join("d/two.log"), dir.join("one.log")]);
        assert!(!t.0.join("evil.log").exists());

        assert!(extract(&zip_path, Kind::Plain, &dir, &cancel, &mut no_progress()).is_err());
        let root = t.0.join("tmp");
        assert_eq!(temp_dir_for(&root, &zip_path), root.join("logs.zip"));
        std::fs::create_dir_all(root.join("logs.zip")).unwrap();
        assert_eq!(temp_dir_for(&root, &zip_path), root.join("logs.zip-1"));
    }
}
