//! Indexes a file and prints timing: `cargo run --release -p rlogg-core --example index -- <file>`

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: index <file>")?;
    let start = Instant::now();
    let log = rlogg_core::LogFile::open(&path)?;
    let elapsed = start.elapsed();
    let size = log.bytes().len() as f64;
    println!(
        "{} lines, {:.1} MiB in {elapsed:.2?} ({:.0} MiB/s), index {:.1} MiB, longest line {} B",
        log.line_count(),
        size / 1048576.0,
        size / 1048576.0 / elapsed.as_secs_f64(),
        log.index().allocated_bytes() as f64 / 1048576.0,
        log.index().max_line_len(),
    );
    Ok(())
}
