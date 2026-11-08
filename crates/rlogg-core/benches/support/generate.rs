//! Deterministic synthetic log files for benchmarks.
//!
//! Shared by the criterion benches and the `genlog` example (included with
//! `#[path]`), so `scripts/bench-vs-klogg.sh` measures the same files.

#![allow(dead_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Shape of the generated lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Application log: timestamp, level, thread, message; 60–200 bytes.
    Typical,
    /// Very short lines (8–30 bytes): stresses per-line costs.
    Short,
    /// Long lines (1–16 KiB): stresses scanning.
    Long,
    /// `Typical`, encoded as UTF-16LE with a BOM.
    Utf16,
}

impl Profile {
    pub const ALL: [Self; 4] = [Self::Typical, Self::Short, Self::Long, Self::Utf16];

    pub fn name(self) -> &'static str {
        match self {
            Self::Typical => "typical",
            Self::Short => "short",
            Self::Long => "long",
            Self::Utf16 => "utf16",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const LEVELS: [&str; 5] = ["TRACE", "DEBUG", "INFO ", "WARN ", "ERROR"];
const WORDS: [&str; 16] = [
    "request",
    "handled",
    "user",
    "session",
    "timeout",
    "cache",
    "miss",
    "connection",
    "opened",
    "closed",
    "retry",
    "payload",
    "bytes",
    "queue",
    "worker",
    "database",
];

fn line(rng: &mut Rng, n: u64, profile: Profile, out: &mut String) {
    use std::fmt::Write as _;
    out.clear();
    match profile {
        Profile::Typical | Profile::Utf16 => {
            let secs = n / 1000;
            let _ = write!(
                out,
                "2026-11-08 {:02}:{:02}:{:02}.{:03} {} [worker-{}] ",
                secs / 3600 % 24,
                secs / 60 % 60,
                secs % 60,
                n % 1000,
                LEVELS[rng.below(5) as usize],
                rng.below(32)
            );
            for _ in 0..3 + rng.below(18) {
                out.push_str(WORDS[rng.below(16) as usize]);
                out.push(' ');
            }
            let _ = write!(
                out,
                "id={} took={}ms",
                rng.next() % 1_000_000,
                rng.below(5000)
            );
        }
        Profile::Short => {
            for _ in 0..1 + rng.below(4) {
                out.push_str(WORDS[rng.below(16) as usize]);
                out.push(' ');
            }
        }
        Profile::Long => {
            let len = 1024 + rng.below(15 * 1024) as usize;
            while out.len() < len {
                out.push_str(WORDS[rng.below(16) as usize]);
                out.push(if rng.below(8) == 0 { '\t' } else { ' ' });
            }
        }
    }
    out.push('\n');
}

/// Writes about `size` bytes of `profile` lines to `path`.
pub fn generate(path: &Path, profile: Profile, size: u64) -> std::io::Result<()> {
    let tmp = path.with_extension("partial");
    let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut buf = String::new();
    let mut written = 0u64;
    if profile == Profile::Utf16 {
        w.write_all(&[0xFF, 0xFE])?;
        written += 2;
    }
    let mut units = Vec::new();
    let mut n = 0u64;
    while written < size {
        line(&mut rng, n, profile, &mut buf);
        n += 1;
        if profile == Profile::Utf16 {
            units.clear();
            units.extend(buf.encode_utf16().flat_map(u16::to_le_bytes));
            w.write_all(&units)?;
            written += units.len() as u64;
        } else {
            w.write_all(buf.as_bytes())?;
            written += buf.len() as u64;
        }
    }
    w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(tmp, path)
}

/// Returns a cached file of `profile` and `size_mb` MiB in `dir`, generating
/// it on first use.
pub fn cached(dir: &Path, profile: Profile, size_mb: u64) -> PathBuf {
    std::fs::create_dir_all(dir).expect("create bench data dir");
    let path = dir.join(format!("{}-{size_mb}MiB.log", profile.name()));
    if !path.exists() {
        eprintln!("generating {} …", path.display());
        generate(&path, profile, size_mb << 20).expect("generate bench file");
    }
    path
}
