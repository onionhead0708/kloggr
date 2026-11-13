# Benchmarks

How to reproduce:

```bash
scripts/bench-vs-klogg.sh 1024           # kloggr vs klogg, 1 GiB files, best of 3
COLD=1 scripts/bench-vs-klogg.sh 10240   # the same, each file dropped from the page cache first
KLOGG_GREP=… scripts/bench-grep-vs-klogg.sh 1024   # search: kloggr-grep vs klogg_grep
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

Both tools run with a warm page cache, so the numbers measure CPU work, not disk,
except with `COLD=1`.

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

Search scales linearly from 1 GiB.

### Indexing vs klogg, 10 GiB

`KLOGG=~/dev/klogg/build_root/output/klogg PROFILES="typical short utf16" RUNS=2
scripts/bench-vs-klogg.sh 10240` (klogg 24.11 built from source, RelWithDebInfo).
Best of 2. The long-line file is left out: klogg needs about 2.5 minutes per GiB on it.

| file | kloggr | klogg | speedup | kloggr index | klogg index |
|---|---|---|---|---|---|
| typical, 10 GiB | 9.02 s (1.11 GiB/s) | 10.08 s (0.99 GiB/s) | 1.1× | 123.0 MiB | 96.4 MiB |
| short, 10 GiB | 16.45 s (0.61 GiB/s) | 26.91 s (0.37 GiB/s) | 1.6× | 603.1 MiB | 742.3 MiB |
| utf16, 10 GiB | 8.53 s (1.17 GiB/s) | 7.25 s (1.38 GiB/s) | 0.8× | 74.0 MiB | 70.6 MiB |

Unlike the 1 GiB runs (3× faster), these are mostly disk-bound: a 10 GiB file
doesn't stay in the page cache of a 14 GB machine, so both tools wait for the
NVMe drive, and the CPU work where kloggr is ahead matters less. kloggr's times
are also slower than criterion's (5.3 s for typical), which repeats each run with
whatever the previous one left cached.

To investigate: on the UTF-16 file klogg wins. It reads the file sequentially in
blocks, so the kernel's read-ahead keeps the drive busy; kloggr maps the file and
its threads fault pages in at several places at once. Advising the kernel
(`madvise(MADV_SEQUENTIAL)`/`MADV_WILLNEED` on the chunk ahead) or reading
batches with `read` while indexing may close the gap when the file isn't cached.

## 2026-11-13: search vs klogg_grep, 1 GiB, kloggr feat/phase-7-optional

`KLOGG_GREP=~/dev/klogg/build_root/output/klogg_grep scripts/bench-grep-vs-klogg.sh 1024`
(klogg 24.11 built from source, RelWithDebInfo; Intel i7-7500U, 4 threads). Best of 3,
wall clock of the whole process: start, index, search, and write the matching
lines to a file, with a warm page cache. Both tools printed the same lines, byte
for byte, except on UTF-16.

| search | file | kloggr-grep | (index + search) | klogg_grep | speedup | matches |
|---|---|---|---|---|---|---|
| plain, rare (`id=123456`) | typical | 417 ms | 290 + 115 ms | 1726 ms | 4.1× | 3 |
| plain, ~20% (`ERROR`) | typical | 1184 ms | 290 + 878 ms | 5002 ms | 4.2× | 1,470,710 |
| regex (`took=\d{4}ms`, dense) | typical | 2995 ms | 290 + 2667 ms | 14783 ms | 4.9× | 5,884,992 |
| plain, short lines (`cache`) | short | 3391 ms | 933 + 2413 ms | 40944 ms | 12.1× | 8,305,705 |
| plain, UTF-16LE (`ERROR`) | utf16 | 1123 ms | 230 + 852 ms | 1550 ms | – | 735,513 |

kloggr-grep's search time includes writing the lines, which is most of it when
many lines match (criterion's `search_all`, which only collects them, takes 191 ms
for `ERROR`). klogg_grep doesn't decode UTF-16: it found no line with `ERROR` in
that file, so its time there isn't comparable.

## 2026-11-13: read-ahead and tab widths while indexing, kloggr feat/phase-7-optional

Same machine. `scripts/bench-vs-klogg.sh` against klogg 24.11 built from source.

Two changes to indexing:
- **Read-ahead** (Linux and macOS): a thread reads the file sequentially, at
  most 512 MiB ahead of the indexer, and the indexer waits until the next
  128 MiB are read before each batch, so it finds the pages in memory instead of
  faulting them in from the drive. Blocks already in the page cache are skipped
  (`mincore`), so a cached file costs nothing more. `madvise(MADV_WILLNEED)` and
  `MADV_SEQUENTIAL` were tried first and gained under 10%.
- **Tab widths**: the longest line's width now expands tabs, as klogg's does.
  Each chunk measures its lines in the same `memchr2` pass that finds line feeds,
  so this moved per-line work out of the sequential loop and made indexing faster.

### Cold, 10 GiB

`COLD=1 PROFILES="typical short utf16" RUNS=2 scripts/bench-vs-klogg.sh 10240`: the
file is dropped from the page cache before each run. The NVMe drive reads 1.8 GB/s
(1.68 GiB/s) with `dd`.

| file | kloggr before | kloggr now | klogg | now vs klogg |
|---|---|---|---|---|
| typical | 13.4 s | 7.59 s (1.32 GiB/s) | 8.10 s (1.24 GiB/s) | 1.1× |
| short | 19.3 s | 9.24 s (1.08 GiB/s) | 24.29 s (0.41 GiB/s) | 2.6× |
| utf16 | 12.6 s | 7.01 s (1.43 GiB/s) | 6.37 s (1.57 GiB/s) | 0.9× |

"Before" is master's indexer, one cold run each. On UTF-16 klogg is still a little
ahead; both are close to the drive's speed.

### Warm, 1 GiB

`RUNS=3 scripts/bench-vs-klogg.sh 1024`, now with tab widths on both sides, so the
long-line row compares like with like.

| file | kloggr | klogg | speedup |
|---|---|---|---|
| typical | 167 ms (5.99 GiB/s) | 771 ms (1.30 GiB/s) | 4.6× |
| short | 711 ms (1.41 GiB/s) | 2354 ms (0.42 GiB/s) | 3.3× |
| long (tabs) | 173 ms (5.77 GiB/s) | 143.8 s (0.01 GiB/s) | 830× (one run) |
| utf16 | 115 ms (8.70 GiB/s) | 622 ms (1.61 GiB/s) | 5.4× |

Before these changes kloggr took 274 / 911 / 53 / 223 ms. The long file got slower
(53 to 173 ms) because its tabs are now counted; the others got faster.
Windows keeps indexing through the mapping without read-ahead: it has no
`mincore`, and reading a cached file again would cost about a fifth.

