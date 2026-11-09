//! Engine benchmarks: `cargo bench -p rlogg-core`.
//!
//! Indexing runs on generated files (see `support/generate.rs`), cached in
//! `target/bench-data` (or `$RLOGG_BENCH_DIR`). `RLOGG_BENCH_SIZE_MB` sets their size (default
//! 1024; use 10240 for the 10 GB runs from the roadmap). The page cache is
//! warm after the first iteration, so indexing numbers measure CPU, not disk.
//! For a comparison with klogg, see `scripts/bench-vs-klogg.sh`.

#[path = "support/generate.rs"]
mod generate;

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use generate::Profile;
use rlogg_core::line_index::CompressedOffsets;
use rlogg_core::search::{self, Matcher, Pattern};
use rlogg_core::{Encoding, LineIndex, LogFile, encoding, text};
use std::sync::atomic::AtomicBool;

fn size_mb() -> u64 {
    std::env::var("RLOGG_BENCH_SIZE_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024)
}

/// `$RLOGG_BENCH_DIR`, else `target/bench-data` (shared with the
/// comparison script; on disk, as multi-GB files do not fit in tmpfs).
fn data_dir() -> PathBuf {
    std::env::var_os("RLOGG_BENCH_DIR").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
                .parent()
                .expect("target dir")
                .join("bench-data")
        },
        PathBuf::from,
    )
}

/// Full open: map, detect the encoding, build the index (klogg:
/// `FullIndexOperation`).
fn index(c: &mut Criterion) {
    let mut group = c.benchmark_group("index");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(20));
    for profile in Profile::ALL {
        let path = generate::cached(&data_dir(), profile, size_mb());
        let len = std::fs::metadata(&path).unwrap().len();
        group.throughput(Throughput::Bytes(len));
        group.bench_function(
            BenchmarkId::new(profile.name(), format!("{}MiB", size_mb())),
            |b| b.iter(|| LogFile::open(black_box(&path)).unwrap()),
        );
    }
    group.finish();
}

/// Full-file search (klogg: `SearchOperation`), on the typical file unless
/// the case names another profile.
fn search(c: &mut Criterion) {
    let cases: [(&str, Profile, Pattern); 8] = [
        ("plain_rare", Profile::Typical, Pattern::plain("id=123456 ")),
        ("plain_common", Profile::Typical, Pattern::plain("ERROR")),
        (
            "plain_case_insensitive",
            Profile::Typical,
            Pattern::plain("error").case_insensitive(),
        ),
        ("regex", Profile::Typical, Pattern::regex(r"took=\d{4}ms")),
        (
            "exclude",
            Profile::Typical,
            Pattern::plain("DEBUG").excluding(),
        ),
        (
            "boolean",
            Profile::Typical,
            Pattern::regex(r#""ERROR" and "timeout" and not("worker-1\b")"#).boolean(),
        ),
        ("plain_short_lines", Profile::Short, Pattern::plain("cache")),
        ("plain_utf16", Profile::Utf16, Pattern::plain("ERROR")),
    ];
    let mut group = c.benchmark_group("search");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(20));
    let never = AtomicBool::new(false);
    for (name, profile, pattern) in cases {
        let path = generate::cached(&data_dir(), profile, size_mb());
        let log = LogFile::open(&path).unwrap();
        let matcher = Matcher::new(&pattern).unwrap();
        group.throughput(Throughput::Bytes(log.bytes().len() as u64));
        group.bench_function(BenchmarkId::new(name, format!("{}MiB", size_mb())), |b| {
            b.iter(|| search::search_all(&log, &matcher, 0..usize::MAX, &never).unwrap())
        });
    }
    group.finish();
}

/// Reading lines the way the view does: a screenful at a random position.
fn line_access(c: &mut Criterion) {
    let path = generate::cached(&data_dir(), Profile::Typical, size_mb());
    let log = LogFile::open(&path).unwrap();
    let count = log.line_count();
    let mut group = c.benchmark_group("lines");
    let mut seed = 1u64;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 33) as usize % count
    };
    group.bench_function("screen_of_60_at_random", |b| {
        b.iter(|| {
            let first = next();
            log.lines_text(first..first + 60)
                .map(|l| l.len())
                .sum::<usize>()
        })
    });
    group.bench_function("single_line_at_random", |b| {
        b.iter(|| log.line(next()).map(<[u8]>::len))
    });
    group.finish();
}

/// Horizontal windowing of a 10 MB line, scrolled to the start and the end.
fn long_line_window(c: &mut Criterion) {
    let line: String = (0..10_000_000)
        .map(|i| {
            if i % 17 == 0 {
                '\t'
            } else {
                char::from(b'a' + (i % 26) as u8)
            }
        })
        .collect();
    let cols = text::columns(&line);
    let mut group = c.benchmark_group("window_10MB_line");
    for (name, first) in [("start", 0), ("middle", cols / 2), ("end", cols - 200)] {
        group.bench_function(name, |b| {
            b.iter(|| text::window(Encoding::UTF_8, black_box(line.as_bytes()), first, 200))
        });
    }
    group.finish();
}

fn detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("detect_encoding_1MiB");
    for profile in [Profile::Typical, Profile::Utf16] {
        let path = generate::cached(&data_dir(), profile, size_mb());
        let mut sample = std::fs::read(&path).unwrap();
        sample.truncate(encoding::DETECTION_SAMPLE);
        group.bench_function(profile.name(), |b| {
            b.iter(|| encoding::detect(black_box(&sample)))
        });
    }
    let latin1: Vec<u8> = b"Fran\xe7ois a \xe9t\xe9 \xe0 la plage, caf\xe9 cr\xe8me.\n"
        .iter()
        .copied()
        .cycle()
        .take(encoding::DETECTION_SAMPLE)
        .collect();
    group.bench_function("latin1_chardetng", |b| {
        b.iter(|| encoding::detect(black_box(&latin1)))
    });
    group.finish();
}

fn compressed_offsets(c: &mut Criterion) {
    let mut offsets = CompressedOffsets::new();
    let mut v = 0;
    for i in 0..10_000_000u64 {
        v += 40 + i % 120;
        offsets.push(v);
    }
    let mut group = c.benchmark_group("compressed_offsets_10M");
    group.bench_function("get", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = (i + 7_919) % 10_000_000;
            offsets.get(i)
        })
    });
    group.bench_function("range_60", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = (i + 7_919) % 9_999_900;
            offsets.range(i..i + 60)
        })
    });
    group.finish();

    // Incremental extension (follow mode): append 1 MiB to an indexed buffer.
    let base = std::fs::read(generate::cached(&data_dir(), Profile::Typical, 64)).unwrap();
    let mut group = c.benchmark_group("extend");
    group.throughput(Throughput::Bytes(1 << 20));
    group.bench_function("append_1MiB_to_63MiB", |b| {
        let split = base.len() - (1 << 20);
        let indexed = {
            let mut idx = LineIndex::default();
            idx.extend(&base[..split]);
            idx
        };
        b.iter_batched(
            || indexed.clone(),
            |mut idx| {
                idx.extend(&base);
                idx
            },
            criterion::BatchSize::LargeInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    index,
    search,
    line_access,
    long_line_window,
    detection,
    compressed_offsets
);
criterion_main!(benches);
