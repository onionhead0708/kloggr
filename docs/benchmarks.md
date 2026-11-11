# Benchmarks

How to reproduce:

```bash
scripts/bench-vs-klogg.sh 1024           # kloggr vs klogg, 1 GiB files, best of 3
cargo bench -p kloggr-core                # criterion suite (1 GiB files)
KLOGGR_BENCH_SIZE_MB=10240 cargo bench -p kloggr-core -- index   # 10 GB
```

Test files are generated deterministically (`crates/kloggr-core/benches/support/generate.rs`)
and cached in `target/bench-data`:

| profile | lines |
|---|---|
| typical | timestamp, level, thread, 3–20 words, ids: 60–200 bytes |
| short | 1–4 words: 8–30 bytes, i.e. many lines per byte |
| long | 1–16 KiB of words, about 1 in 8 separated by a tab |
| utf16 | typical, encoded as UTF-16LE with a BOM |

Both tools run with a warm page cache, so the numbers measure CPU work, not disk.

## 2026-11-08: 1 GiB, kloggr 4862bee + index shrink, klogg 24.11.0.0

Machine: Intel Core i7-7500U (2 cores / 4 threads, 2017 laptop), 14 GB RAM, NVMe,
Ubuntu 26.04, Rust 1.99.1 release build.

### Full indexing vs klogg

kloggr: `LogFile::open` (mmap, encoding detection, parallel index). klogg: the
"Indexing done, took …" time from its own log, headless (`QT_QPA_PLATFORM=offscreen`),
which excludes startup. Best of 3 runs.

| file | kloggr | klogg | speedup | kloggr index | klogg index |
|---|---|---|---|---|---|
| typical, 1 GiB | 274 ms (3.65 GiB/s) | 856 ms (1.17 GiB/s) | 3.1× | 12.3 MiB | 9.6 MiB |
| short, 1 GiB | 911 ms (1.10 GiB/s) | 2743 ms (0.36 GiB/s) | 3.0× | 60.3 MiB | 74.2 MiB |
| long, 1 GiB | 53 ms (18.8 GiB/s) | 158 s (0.01 GiB/s) | not comparable, see below | 0.2 MiB | 0.3 MiB |
| utf16, 1 GiB | 223 ms (4.48 GiB/s) | 666 ms (1.50 GiB/s) | 3.0× | 7.4 MiB | 7.1 MiB |

Notes:
- **long**: klogg expands tabs while indexing (`expandTabsInLine`) to get each line's
  display width, and on tab-heavy long lines this dominates. kloggr doesn't expand tabs
  during indexing yet (the view widens its scroll range as it meets wider lines), so this
  row isn't a like-for-like comparison. Recheck once kloggr computes expanded widths.
- **Index size**: kloggr stores 128-line blocks as a u64 base plus varint deltas (1–2
  bytes per line); klogg uses stream-vbyte relative to the block start. kloggr's index is
  about 25% larger on typical lines, smaller on short ones.

### Criterion (kloggr only)

| benchmark | time |
|---|---|
| index/typical 1 GiB | 298 ms (3.35 GiB/s) |
| index/short 1 GiB | 826 ms (1.21 GiB/s) |
| index/long 1 GiB | 89 ms (11.3 GiB/s) |
| index/utf16 1 GiB | 229 ms (4.36 GiB/s) |
| extend: append 1 MiB to a 63 MiB index (follow mode) | 299 µs (3.26 GiB/s) |
| 60 decoded lines at a random position (one screen) | 4.5 µs |
| one raw line at a random position | 0.79 µs |
| compressed index: `get` / `range(60)`, 10M lines | 252 ns / 638 ns |
| window of 200 columns in a 10 MB line: start / middle / end | 1.3 µs / 24 ms / 48 ms |
| encoding detection, 1 MiB: UTF-8 / UTF-16 BOM / Latin-1 (chardetng) | 0.65 ms / 3 ns / 123 ms |

Observations for later work:
- Drawing a 10 MB line scrolled to its end costs 48 ms per frame, because the window is
  decoded from the line start. Caching column checkpoints per long line would fix this.
- `chardetng` takes 123 ms on a 1 MiB non-UTF-8 sample. That's acceptable once per open,
  but a smaller sample would do.
- Short lines are the slowest case (1.1 GiB/s): the sequential append into the
  compressed index is the bottleneck, not the parallel scan.

## 2026-11-09: search, 1 GiB, kloggr feat/search

Same machine and files. `search::search_all` over the whole file: parallel per-line
matching with the `regex` crate, results in a Roaring bitmap. Criterion, warm cache.

| benchmark | time | throughput |
|---|---|---|
| plain, rare (`id=123456 `) | 286 ms | 3.50 GiB/s |
| plain, ~20% of lines (`ERROR`) | 301 ms | 3.32 GiB/s |
| plain, case-insensitive (`error`) | 406 ms | 2.46 GiB/s |
| regex (`took=\d{4}ms`) | 549 ms | 1.82 GiB/s |
| exclude (`DEBUG`, inverse) | 334 ms | 2.99 GiB/s |
| boolean (`"ERROR" and "timeout" and not("worker-1\b")`) | 412 ms | 2.43 GiB/s |
| plain, short lines (`cache`) | 1996 ms | 0.51 GiB/s |
| plain, UTF-16LE (`ERROR`, decoded per line) | 431 ms | 2.32 GiB/s |

Notes:
- **Boolean**: sub-patterns are matched lazily while the expression is evaluated, so
  `and`/`or` short-circuit. A `RegexSet` that runs every sub-pattern on every line took
  1187 ms.
- **Short lines** are bound by per-line overhead (~55M lines). A possible speed-up for
  UTF-8 files: run the regex over whole blocks and check only the lines where a
  candidate match starts (care needed with `\A`/`\z` and matches that cross lines).
- No klogg comparison yet: klogg has no headless search mode to time it the way
  `scripts/bench-vs-klogg.sh` times indexing.

### Quick find (2026-11-09)

`search::find::find` with no occurrence after the start, i.e. a scan of the whole
1 GiB typical file (klogg: "Reached end of file, no occurrence found").

| benchmark | time | throughput |
|---|---|---|
| forward, line 0 to the end | 291 ms | 3.43 GiB/s |
| backward, the end to line 0 | 289 ms | 3.46 GiB/s |

Blocks of rows are checked in parallel and the first (last) block with a match wins.
Decoding each block's line offsets once, instead of one index lookup per line,
took this from 1383 ms to 291 ms. klogg scans line by line on one thread.

### Overview bar (2026-11-09)

`search::overview::Overview::new`: the ticks for a 1000-pixel bar over 10 M lines,
with marks on every 1000th line.

| matches | time |
|---|---|
| every line (10 M) | 0.33 ms |
| 1 line in 100 (100 k) | 0.56 ms |

Once a pixel row holds 3 lines (the darkest tick), the iterator jumps to the next
row, so the cost depends on the bar height, not the match count. klogg walks every
match and mark. The view caches the ticks until the matches, marks, file or bar
height change.

## 2026-11-11: block search, and 10 GiB runs, kloggr feat/phase-6

Same machine, Rust 1.99.1 release build, criterion.

### Search over whole blocks (1 GiB)

`search_all` now runs a single UTF-8 pattern over a whole block of lines with a
multi-line copy of the regex, and checks a line on its own only when a match runs
past its end (`Matcher::match_block`). When most lines match, the rest of the block
is tested line by line instead, which is cheaper. Boolean patterns, UTF-16 files,
hidden ANSI sequences and blocks with carriage returns keep the per-line path.

| benchmark | 2026-11-09 | now | |
|---|---|---|---|
| plain, rare | 286 ms | 118 ms (8.49 GiB/s) | 2.4× |
| plain, ~20% of lines | 301 ms | 191 ms (5.24 GiB/s) | 1.6× |
| plain, case-insensitive | 406 ms | 290 ms (3.45 GiB/s) | 1.4× |
| regex (`took=\d{4}ms`, dense) | 549 ms | 556 ms (1.80 GiB/s) | same |
| exclude | 334 ms | 253 ms (3.95 GiB/s) | 1.3× |
| boolean | 412 ms | 431 ms | same path |
| plain, short lines | 1996 ms | 664 ms (1.51 GiB/s) | 3.0× |
| plain, UTF-16LE | 431 ms | 478 ms | same path |
| quick find, forward / backward | 291 / 289 ms | 305 / 306 ms | same path |

The "same path" rows didn't change code; their differences are run-to-run noise
on a laptop (quick find also reads lines through `LogFile::text_block` now).

### Reading instead of mapping (Windows)

On Windows the file is no longer kept mapped after indexing (`Access::Read`,
`docs/decisions.md`). With `KLOGGR_READ_FILES=1` on this machine, 256 MiB files
before the block search: search 7–32% slower, quick find about 40%, 60 lines at a
random position 8.1 µs instead of 4.9 µs.

### 10 GiB

`KLOGGR_BENCH_SIZE_MB=10240`. The files are close to this machine's 14 GB of RAM,
so part of each run reads from disk (NVMe); indexing, which touches every byte
once per sample, shows it most.

| benchmark | time | throughput |
|---|---|---|
| index, typical | 5.31 s | 1.88 GiB/s |
| index, short | 12.06 s | 0.83 GiB/s |
| index, long | 0.85 s | 11.8 GiB/s |
| index, UTF-16 | 2.90 s | 3.45 GiB/s |
| search, plain rare | 1.14 s | 8.80 GiB/s |
| search, plain ~20% | 1.95 s | 5.14 GiB/s |
| search, regex | 5.57 s | 1.80 GiB/s |
| search, short lines | 10.75 s | 0.93 GiB/s |

Search scales linearly from 1 GiB. The klogg comparison at 10 GiB is still to do:
klogg wasn't installed when these ran.
