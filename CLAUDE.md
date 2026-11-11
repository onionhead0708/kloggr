# kloggr

A Rust rewrite of [klogg](https://github.com/variar/klogg), a fast, cross-platform
GUI log viewer for very large files (multi-GB), with a GPUI interface.
It was called rlogg until 2026-11-10 (renamed to avoid github.com/jmichaelsturm/rlogg).

License: **GPL-3.0-or-later**, the same as klogg (`LICENSE`). Code ported from klogg
stays GPLv3. Keep klogg's copyright notices where code is translated closely.

## References

| What | Where | Use it for |
|---|---|---|
| klogg source (C++/Qt) | `~/dev/klogg` (local checkout), upstream github.com/variar/klogg | Behavior and algorithms to reproduce. **The spec of record.** |
| klogg user docs | `~/dev/klogg/DOCUMENTATION.md` | Feature list, search syntax, shortcuts |
| kloggr user guide | `docs/user-guide.md` | Shown by Help > Documentation; update it with each user-visible change |
| kloggr decisions | `docs/decisions.md` | Deliberate differences from klogg and why. Read before changing a behavior to match klogg, and add an entry for each new difference |
| klogg test data | `~/dev/klogg/test_data`, `~/dev/klogg/tests` | Port the test cases, reuse the sample files |
| dbdelve | https://github.com/ShayanAbbas1/dbdelve (GPL-3.0, GPUI) | Working GPUI app: bootstrap, keymaps, theming, virtualized grids, Linux packaging, its `AGENTS.md` GPUI rules |
| gpui-component | https://github.com/longbridge/gpui-component | Widgets: dock, tabs, table, inputs, menus. Read its examples |
| GPUI | Zed repo `crates/gpui` (republished on crates.io as `gpui-pre`) | Framework source. Docs are thin, so read the code |

### klogg → kloggr module map

| klogg (`~/dev/klogg/src/…`) | kloggr | Rust crates |
|---|---|---|
| `logdata/` FileHolder, LogData, LinePositionArray, CompressedLineStorage, LogDataWorker | `kloggr-core::{file, line_index}` | memmap2, rayon |
| `logdata/` EncodingDetector | `kloggr-core::encoding` | encoding_rs, chardetng |
| `logdata/` LogFilteredData(Worker) | `kloggr-core::search` (search driver, `LineSet`, `FilteredLines` done) | regex, rayon, roaring |
| `regex/` (Hyperscan / QRegularExpression, boolean combos; exprtk) | `kloggr-core::search` (done; own boolean parser instead of exprtk) | regex |
| `filewatch/` (efsw) | `kloggr-core::watch` (done), reload in `kloggr-core::file`, app `follow` | notify |
| `settings/`, `ui/session*`, `ui/recentfiles*` | `kloggr-core::settings` (recent files, open files and per-file state done), `kloggr-core::options` (done) | serde, toml, directories |
| `ui/optionsdialog`, `settings/shortcuts` | `kloggr-app` options_dialog, shortcuts, preferences (done) | gpui-component |
| `ui/abstractlogview`, `logmainview`, `filteredview`, `overview*` | `kloggr-app` log view widget | gpui |
| `ui/frozencolumns` | `kloggr-core::viewport` (column mapping), `settings::FrozenFile` (done) | |
| `ui/crawlerwidget`, `tabbedcrawlerwidget`, `mainwindow`, `menu` | `kloggr-app` workspace, tabs, menus | gpui-component |
| `ui/quickfind*` (QuickFind engine) | `kloggr-core::search::find`, `kloggr-app` quick_find | rayon |
| `ui/highlighterset`, `colorlabelsmanager`, `highlightedmatch` | `kloggr-core::highlight` (done), app `highlighters` | regex |
| `ui/highlightersdialog`, `highlighterset(edit)`, `highlighteredit` | `kloggr-app` highlighters_dialog (done), `kloggr-core::highlight::klogg_conf` | gpui-component |
| `ui/predefinedfilters*`, `savedsearches` | `kloggr-core::searches`, app `saved_searches` (done) | |
| `ui/scratchpad`, `tabbedscratchpad` | `kloggr-core::scratch`, app `scratchpad` (done) | base64, serde_json, quick-xml, crc32fast |
| `versioncheck/`, `logging/`, `crash_handler/` | `kloggr-core::{crash, version_check}` (done), app `crash_report`, `update_check` | ureq (rustls), tracing, panic hook |

## Architecture

```
crates/
  kloggr-core/   engine: mmap, line index, encodings, search, filtering, watching
  kloggr-app/    GPUI desktop app (binary `kloggr`)
```

Rules:
- **`kloggr-core` must never depend on GPUI** or any UI crate. All logic that can be
  tested headless goes there. This keeps GPUI churn out of the engine.
- Long-running work (indexing, search) runs off the UI thread and reports progress.
  Don't block GPUI's executor, and don't put tokio on it.
- The UI renders only visible rows. Never build per-line UI state for the whole file.
- Very long lines must be truncated or windowed horizontally before text shaping.

## GPUI version policy

- GPUI is pre-1.0 and breaks on minor bumps. `gpui-pre`, `gpui-pre-platform` and
  `gpui-component` are **pinned exactly** in the root `Cargo.toml`, and must be bumped
  together (gpui-component pins its gpui-pre version). `gpui-kit-assets` (the icons
  gpui-component draws, registered in `main.rs`) follows gpui-component's version. Current set: `gpui-pre =0.3.9`
  and `gpui-component =0.8.0`, the same set dbdelve uses.
- The `gpui` crate on crates.io is a stale Oct 2025 snapshot. Use `gpui-pre`, renamed
  to `gpui` in Cargo.
- `[profile.dev.package."*"] opt-level = 3` is required, or debug builds drop frames.

## Commands

```bash
cargo build                      # debug build
cargo run -p kloggr-app -- <file> # run the app on a log file
cargo nextest run                # tests (or: cargo test)
cargo clippy --all-targets -- -D warnings
cargo fmt
bacon                            # watch mode
cargo deny check                 # license / advisory check (GPL-compatible deps only)
cargo bench -p kloggr-core        # criterion benches (KLOGGR_BENCH_SIZE_MB=10240 for 10 GB)
scripts/bench-vs-klogg.sh 1024   # indexing time vs klogg on the same generated files
scripts/package-linux.sh         # tarball, .deb (and AppImage with appimagetool) in target/package/
scripts/package-windows.sh       # Windows .zip and .msi (Git Bash on Windows, WiX 5); CI runs it
scripts/package-macos.sh         # macOS universal .dmg (on a Mac); CI runs it
```

### Visual check (headless)

To see the real app without touching the desktop, run it in a virtual X display
(packages `xvfb xdotool imagemagick`; Vulkan falls back to software rendering):

```bash
Xvfb :99 -screen 0 1400x900x24 &
env -u WAYLAND_DISPLAY DISPLAY=:99 KLOGGR_CONFIG_DIR=<scratch dir> target/debug/kloggr <file> &
DISPLAY=:99 xdotool mousemove 300 60 click --repeat 2 1   # input: clicks, keys (xdotool key ctrl+d)
DISPLAY=:99 import -window root shot.png                  # screenshot
pkill -x kloggr                                            # not `pkill -f`: it matches the shell
```

Simulated keystrokes in `gpui::test` don't go through the platform's key translation:
check new shortcuts in the real app too (see the color-label entry in `docs/decisions.md`).

## Environment (Ubuntu 26.04, Linux first)

- Rust is pinned by `rust-toolchain.toml` (1.99.1), installed through rustup.
- System packages: `build-essential pkg-config cmake libfontconfig-dev libxkbcommon-dev
  libxkbcommon-x11-dev libwayland-dev libdbus-1-dev libvulkan-dev mesa-vulkan-drivers`
- Cargo tools: `bacon cargo-nextest cargo-deny`
- For headless visual checks: `xvfb xdotool imagemagick`
- For packaging: `dpkg-dev fakeroot`, and `appimagetool` (github.com/AppImage/appimagetool releases) on PATH
- Platforms: develop and test on Linux (X11 and Wayland). Keep code portable; Windows
  and macOS are built and tested by CI (`.github/workflows/ci.yml`) on every push.

## Roadmap

### Phase 0: scaffold ✅
- [x] Workspace, GPUI window, mmap and a simple line index, virtualized list

### Phase 1: core viewer (MVP)
- [x] Parallel, compressed line index with progress reporting (klogg `LogDataWorker`, `CompressedLineStorage`)
- [x] Encoding detection and decoding: UTF-8, UTF-16LE/BE, Latin-1, etc. (`EncodingDetector`)
- [x] Custom log view element: line numbers, selection, copy, horizontal scroll, long-line handling
- [x] Open file (dialog, drag and drop, CLI), recent files
- [x] Benchmarks (criterion) for indexing and search on 1–10 GB files, compared with klogg (indexing and search done, 1 GiB results in `docs/benchmarks.md`; 10 GB runs still to do)

### Phase 2: search and filtering ✅
- [x] Regex, plain-text and case-insensitive search, plus klogg's boolean combinations (`regex/`): engine in `kloggr-core::search`; the search bar UI comes with the filtered view
- [x] Filtered view (bottom pane) synced with the main view; marks (search bar, bullets, `m` `[` `]`, visibility `v` `1`–`3`; match highlighting in the text still todo)
- [x] Quick find (incremental, next/previous): Ctrl+F, `'` `"`, `n` `N` F3 Ctrl+G, `*` `.` `/` `,`; occurrences highlighted in both views
- [x] Jump to line: `[number] g`, `[number] j/k`, and Ctrl+L with a small input box (klogg `digitsBuffer_`, `LogViewJumpToLine`). klogg's `1`–`7` shortcuts are Alt+1…7 (`docs/decisions.md`)
- [x] Overview bar showing match and mark positions (main view; click or drag to scroll)
- [x] Follow-up: highlight the search pattern's matches in the text of both views (on by default, unlike klogg: `docs/decisions.md`)

### Phase 3: live files and workspace ✅
- [x] Follow mode / tail: incremental reindex on growth, detect truncation and rotation (`filewatch/`); `f` / F10, search auto-refresh
- [x] Tabs and multiple files, sessions (open tabs restored at startup with search options, follow and marks), tab renaming and "Open containing folder" (klogg `TabNameMapping`)
- [x] Highlighters and highlighter sets, color labels: engine, line colouring, Tools and context menus, color labels `Ctrl+Shift+1`–`9` / `Ctrl+D` / `Ctrl+Shift+0`, the highlighters dialog (sets, highlighters, label styles), klogg `.conf` import/export
- [x] Frozen columns (klogg `frozencolumns`): View menu and context menu toggles per view, draggable separator, remembered per file in the session
- [x] Wide characters (CJK, emoji): two columns per wide character (Unicode East Asian Width), zero for combining marks, each wide glyph drawn at its column
- [x] Settings and options dialog (Tools > Options…: session, search defaults, match and quick-find highlight colours, font, style, overview, line numbers, follow on scroll, recent files) and editable shortcuts with klogg's defaults (`settings.toml`)

### Phase 4: extras and release
- [x] Predefined filters, saved searches, scratchpad (search history and Filters drop-downs, Tools > Predefined Filters…, Add/Exclude/Replace search, scratchpad window with transforms)
- [x] Archive/compressed file opening (klogg `decompressor`): gzip, bzip2, xz, lzma, zip, 7z, tar (also compressed), extracted to a temporary folder
- [x] Packaging: Linux tarball, .deb and AppImage (`scripts/package-linux.sh`; `appimagetool` in `~/.local/bin`), Windows .zip and .msi (`scripts/package-windows.sh`, WiX 5), macOS universal .dmg (`scripts/package-macos.sh`), unsigned; built by `.github/workflows/release.yml` on `v*` tags. Windows/macOS are only checked by CI, never run here
- [x] Version check against GitHub releases (`kloggr-core::version_check`, app `update_check`), crash/panic reporting (`kloggr-core::crash`, report shown at the next start with a GitHub issue link)
- [x] Copying large selections: size from the line index, ask above 64 MiB and copy on a worker thread, refuse above 1 GiB (`docs/decisions.md`)

### Phase 5: viewing and search parity with klogg ✅
Features of the log views and the search that klogg has and kloggr lacks (found by comparing menus, context menus and `shortcuts.h` on 2026-11-10).
- [x] Reload and Stop: `mainwindow.reload` (F5) reloads the file, `mainwindow.stop` cancels indexing or a search; progress shown when a replaced file is reindexed
- [x] Encoding choice: View > Encoding menu overriding the detected encoding, remembered per file in the session
- [x] Search limits: log view context menu Set search start, Set search end, Clear search limits; the search only scans that line range
- [x] Search bar: Keep results toggle (`crawler.enable_keep_results`), shortcut for auto-refresh (`crawler.enable_auto_refresh`), "Save as Filter" in the search box context menu
- [x] Text wrap (`mainwindow.text_wrap`), still windowing very long lines before shaping
- [x] Save to file and Save selected to file (log view context menu), streamed from the mapping on a worker thread
- [x] Split between the views: `crawler.increase_top_view_size` / `decrease_top_view_size`, "Save splitter position" as the default
- [x] Log view context menu completeness: Mark, Find next, Find previous, Set selection start, Set selection end; `logview.exit_view`
- [x] Overview bar flashes the line when hovering a filtered row (klogg overview)
- [x] Option to hide ANSI colour escape sequences (Options > View)

### Phase 6: application, window and platform ✅
Menus, window management and platform integration still missing, plus open items from earlier phases.
- [x] Edit menu: Copy, Select All, Find, Go to Line, Copy Path (`mainwindow.copy_path_to_clipboard`), Open Containing Folder, Open in Editor, Clear File (`mainwindow.clear_file`)
- [x] Help menu: About, Documentation, Report Issue
- [x] File menu: Open from Clipboard, Open from URL, New Window, Opened Files switcher (`mainwindow.select_open_file`)
- [x] Favorites: add, remove and open favorite files (klogg favorites menu and toolbar button)
- [x] Single instance: a second launch passes its files to the running window
- [x] Window size and position remembered in the session; Fullscreen, Minimize, Maximize shortcuts
- [x] Windows: truncated mapped files (read by blocks instead of mapping, re-enable the skipped tests)
- [x] macOS: open files from Finder (*Open With*): GPUI open-URL events and document types in `Info.plist`
- [x] Benchmarks: the 10 GB runs, also against klogg, and the short-line search speed-up (`docs/benchmarks.md`)

### Phase 7: remaining klogg parity
Gaps found by comparing kloggr 0.2.0 with klogg's shortcuts, menus, options, command line and documentation on 2026-11-11.
- [x] File watching by polling, with an interval option (klogg `filewatch.usePolling`, `pollingIntervalMs`): network shares, sftp mounts and volumes where notifications don't arrive
- [x] Search box autocompletion from the search history as you type (klogg `QCompleter`, case-sensitive with the search option)
- [x] Predefined filters: Import and Export in the dialog (klogg `PredefinedFiltersDialog::importFilters`, `exportFilters`)
- [x] File size and modification date in the status line (klogg toolbar `sizeField`, `dateField`)
- [ ] Ctrl+mouse wheel changes the font size in the log views (klogg `changeFontSize`)
- [ ] Drag tabs to reorder them (klogg `setMovable`)
- [ ] Default encoding for new files (klogg `defaultView.encodingMib`), instead of always detecting
- [ ] Command line: `-f/--follow`, `-s/--load-session`, `-n/--new-session`; like klogg, files given on the command line start a new session (or record the difference in `docs/decisions.md`)
- [ ] Log file with a verbosity level (klogg `-l/--log`, `-d/--debug`, Options > Advanced logging)

Optional:
- [ ] Translations (klogg ships Simplified and Traditional Chinese)
- [ ] Minimize to tray (klogg `view.minimizeToTray`)
- [ ] Font bold and antialiasing options (klogg `mainFont.bold`, `mainFont.antialiasing`)
- [ ] Mouse wheel modifiers: klogg scrolls sideways with Alt and faster with Shift, kloggr sideways with Shift; match or record in `docs/decisions.md`
- [ ] A command-line search tool like `klogg_grep`, which also lets the benchmarks compare search with klogg
- [ ] Indexing files larger than memory: read-ahead hints (`madvise`) or sequential reads, where klogg wins on UTF-16 at 10 GiB; tab widths while indexing (the `long` benchmark row)

## Conventions

- Edition 2024. `cargo fmt` and clippy must be clean with `-D warnings`.
- Errors: `thiserror` in `kloggr-core`, `anyhow` at the app boundary.
- Every core feature ports the relevant klogg tests or behaviors and adds unit tests.
- When behavior is unclear, check what klogg does in `~/dev/klogg` before inventing.
- Record every deliberate difference from klogg in `docs/decisions.md` (date, klogg, kloggr, why).
