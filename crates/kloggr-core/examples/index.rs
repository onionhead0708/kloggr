//! Indexes a file and prints timing:
//! `cargo run --release -p kloggr-core --example index -- <file> [runs]`
//!
//! The last line is machine-readable (used by `scripts/bench-vs-klogg.sh`):
//! `best_ms=<f64> lines=<n> index_bytes=<n> size=<n>`.

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: index <file> [runs]")?;
    let runs: usize = args.next().map_or(Ok(1), |r| r.parse())?;
    let mut best = f64::INFINITY;
    let mut last = None;
    for _ in 0..runs.max(1) {
        let start = Instant::now();
        let log = kloggr_core::LogFile::open(&path)?;
        let elapsed = start.elapsed();
        let size = log.byte_len() as f64;
        println!(
            "{} lines, {:.1} MiB in {elapsed:.2?} ({:.0} MiB/s), index {:.1} MiB, longest line {}, {}",
            log.line_count(),
            size / 1048576.0,
            size / 1048576.0 / elapsed.as_secs_f64(),
            log.index().allocated_bytes() as f64 / 1048576.0,
            log.index().max_line_len(),
            log.encoding(),
        );
        best = best.min(elapsed.as_secs_f64() * 1000.0);
        last = Some(log);
    }
    let log = last.expect("at least one run");
    println!(
        "best_ms={best:.1} lines={} index_bytes={} size={}",
        log.line_count(),
        log.index().allocated_bytes(),
        log.byte_len()
    );
    Ok(())
}
