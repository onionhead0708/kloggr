# rlogg

A Rust rewrite of [klogg](https://github.com/variar/klogg), a fast, cross-platform
GUI log viewer for very large files (multi-GB), with a GPUI interface.

License: **GPL-3.0-or-later**, the same as klogg (`LICENSE`). Code ported from klogg
stays GPLv3. Keep klogg's copyright notices where code is translated closely.

## References

| What | Where | Use it for |
|---|---|---|
| klogg source (C++/Qt) | `~/dev/klogg` (local checkout), upstream github.com/variar/klogg | Behavior and algorithms to reproduce. **The spec of record.** |
| klogg user docs | `~/dev/klogg/DOCUMENTATION.md` | Feature list, search syntax, shortcuts |
| klogg test data | `~/dev/klogg/test_data`, `~/dev/klogg/tests` | Port the test cases, reuse the sample files |
| dbdelve | https://github.com/ShayanAbbas1/dbdelve (GPL-3.0, GPUI) | Working GPUI app: bootstrap, keymaps, theming, virtualized grids, Linux packaging, its `AGENTS.md` GPUI rules |
| gpui-component | https://github.com/longbridge/gpui-component | Widgets: dock, tabs, table, inputs, menus. Read its examples |
| GPUI | Zed repo `crates/gpui` (republished on crates.io as `gpui-pre`) | Framework source. Docs are thin, so read the code |

### klogg → rlogg module map

| klogg (`~/dev/klogg/src/…`) | rlogg | Rust crates |
|---|---|---|
| `logdata/` FileHolder, LogData, LinePositionArray, CompressedLineStorage, LogDataWorker | `rlogg-core::{file, line_index}` | memmap2, rayon |
| `logdata/` EncodingDetector | `rlogg-core::encoding` | encoding_rs, chardetng |
| `logdata/` LogFilteredData(Worker) | `rlogg-core::filter` (todo) | regex, rayon, crossbeam-channel |
| `regex/` (Hyperscan / QRegularExpression, boolean combos) | `rlogg-core::search` (todo) | regex, regex-automata (RegexSet) |
| `filewatch/` (efsw) | `rlogg-core::watch` (todo) | notify |
| `settings/`, `ui/session*`, `ui/recentfiles*` | `rlogg-core::settings` (todo) | serde, toml, directories |
| `ui/abstractlogview`, `logmainview`, `filteredview`, `frozencolumns`, `overview*` | `rlogg-app` log view widget | gpui |
| `ui/crawlerwidget`, `tabbedcrawlerwidget`, `mainwindow`, `menu` | `rlogg-app` workspace, tabs, menus | gpui-component |
| `ui/quickfind*`, `highlighter*`, `predefinedfilters*`, `savedsearches`, `scratchpad` | `rlogg-app` features | gpui-component |
| `versioncheck/`, `logging/`, `crash_handler/` | later | ureq (rustls), tracing, panic hook |

## Architecture

```
crates/
  rlogg-core/   engine: mmap, line index, encodings, search, filtering, watching
  rlogg-app/    GPUI desktop app (binary `rlogg`)
```

Rules:
- **`rlogg-core` must never depend on GPUI** or any UI crate. All logic that can be
  tested headless goes there. This keeps GPUI churn out of the engine.
- Long-running work (indexing, search) runs off the UI thread and reports progress.
  Don't block GPUI's executor, and don't put tokio on it.
- The UI renders only visible rows. Never build per-line UI state for the whole file.
- Very long lines must be truncated or windowed horizontally before text shaping.

## GPUI version policy

- GPUI is pre-1.0 and breaks on minor bumps. `gpui-pre`, `gpui-pre-platform` and
  `gpui-component` are **pinned exactly** in the root `Cargo.toml`, and must be bumped
  together (gpui-component pins its gpui-pre version). Current set: `gpui-pre =0.3.9`
  and `gpui-component =0.8.0`, the same set dbdelve uses.
- The `gpui` crate on crates.io is a stale Oct 2025 snapshot. Use `gpui-pre`, renamed
  to `gpui` in Cargo.
- `[profile.dev.package."*"] opt-level = 3` is required, or debug builds drop frames.

## Commands

```bash
cargo build                      # debug build
cargo run -p rlogg-app -- <file> # run the app on a log file
cargo nextest run                # tests (or: cargo test)
cargo clippy --all-targets -- -D warnings
cargo fmt
bacon                            # watch mode
cargo deny check                 # license / advisory check (GPL-compatible deps only)
```

## Environment (Ubuntu 26.04, Linux first)

- Rust is pinned by `rust-toolchain.toml` (1.99.1), installed through rustup.
- System packages: `build-essential pkg-config cmake libfontconfig-dev libxkbcommon-dev
  libxkbcommon-x11-dev libwayland-dev libdbus-1-dev libvulkan-dev mesa-vulkan-drivers`
- Cargo tools: `bacon cargo-nextest cargo-deny`
- Platforms: develop and test on Linux (X11 and Wayland). Keep code portable; Windows
  and macOS CI come later.

## Roadmap

### Phase 0: scaffold ✅
- [x] Workspace, GPUI window, mmap and a simple line index, virtualized list

### Phase 1: core viewer (MVP)
- [x] Parallel, compressed line index with progress reporting (klogg `LogDataWorker`, `CompressedLineStorage`)
- [x] Encoding detection and decoding: UTF-8, UTF-16LE/BE, Latin-1, etc. (`EncodingDetector`)
- [x] Custom log view element: line numbers, selection, copy, horizontal scroll, long-line handling
- [ ] Open file (dialog, drag and drop, CLI), recent files
- [ ] Benchmarks (criterion) for indexing and search on 1–10 GB files, compared with klogg

### Phase 2: search and filtering
- [ ] Regex, plain-text and case-insensitive search, plus klogg's boolean combinations (`regex/`)
- [ ] Filtered view (bottom pane) synced with the main view; marks
- [ ] Quick find (incremental, next/previous)
- [ ] Overview bar showing match and mark positions

### Phase 3: live files and workspace
- [ ] Follow mode / tail: incremental reindex on growth, detect truncation (`filewatch/`)
- [ ] Tabs and multiple files, sessions
- [ ] Highlighters and highlighter sets, color labels
- [ ] Frozen columns (klogg `frozencolumns`)
- [ ] Settings and options dialog, keyboard shortcuts matching klogg's defaults

### Phase 4: extras and release
- [ ] Predefined filters, saved searches, scratchpad
- [ ] Archive/compressed file opening (klogg `decompressor`)
- [ ] Packaging: AppImage, .deb, tarball; then Windows (.msi) and macOS (.dmg) CI
- [ ] Version check, crash/panic reporting

## Conventions

- Edition 2024. `cargo fmt` and clippy must be clean with `-D warnings`.
- Errors: `thiserror` in `rlogg-core`, `anyhow` at the app boundary.
- Every core feature ports the relevant klogg tests or behaviors and adds unit tests.
- When behavior is unclear, check what klogg does in `~/dev/klogg` before inventing.
