# Decisions

Places where kloggr deliberately differs from klogg, and why. klogg stays the
spec of record (see `CLAUDE.md`). Every intentional difference is recorded
here so it isn't "fixed" back later by mistake.

Newest first. Each entry gives the date, what klogg does, what kloggr does, and why.

---

## 2026-11-11: Reload keeps showing the file until it is indexed again

**klogg:** View > Reload (F5) clears the search results and the marks, empties the views and indexes the file again. Stop (Esc, toolbar only) interrupts the search and the indexing; an interrupted load closes the tab. A file replaced on disk is indexed again with its progress in the status bar.

**kloggr:**
- Reload and a replaced file go through the same full reindex. The views keep the old snapshot until the new one is ready, then the marks and results are dropped (auto-refresh runs the search again, as for a replaced file).
- The status line shows "indexing N%" during the reindex.
- Stop is also in the View menu. Stopping the indexing closes the tab, as in klogg; stopping only a search keeps the matches found so far.
- Reload is also bound to Ctrl+R (Cmd+R on macOS): `QKeySequence::Refresh` has both on X11 and macOS.

**Why:** an empty view during the reindex shows nothing useful, and keeping the old snapshot until the swap avoids a blank flash for a small file.

**Code:** `Crawler::{reindex, stop}` in `follow.rs`, `Workspace::{reload, stop}`.

## 2026-11-10: Archives: type from content, a file list, nothing temporary remembered

**klogg:** finds an archive's type from its MIME type (content and name) and asks "Extract archive to temp folder?". It decompresses gzip, bzip2 and xz/lzma to a temporary file, and extracts zip, 7z and tar, also compressed, to a temporary folder. Then it opens a file dialog on that folder to choose files. Extracted files go into recent files and the session like any other.

**kloggr:**
- The type comes from the magic bytes, then the name (`.tar.gz`, `.tgz`, `.lzma`). A `.gz` that holds a tar is extracted as an archive.
- After extracting, a list of the extracted files with check boxes replaces the file dialog. An archive with one file opens it directly.
- Progress shows in the status line. There's no cancel button; quitting stops the extraction.
- Recent files keep the archive. Extracted files are not saved in the session, and the temporary folder is removed when kloggr quits.
- Paths that would leave the folder (zip slip) are skipped.

**Why:** GPUI's file dialog can't start in a given folder. Remembering temporary files would leave broken entries after a restart.

**Code:** `kloggr_core::archive`, `Workspace::{open, extract}`, `crate::archives`.

## 2026-11-10: Scratchpad: UTC times, JSON key order

**klogg:** the scratchpad shows a Unix time in a time zone picked from a list, and "Format json" sorts the object keys (`QJsonObject` keeps them sorted). A tab of instructions ("You can add tabs by pressing +") comes first.

**kloggr:** times (Unix seconds and Windows FILETIME) are shown in UTC only, "Format json" keeps the keys in their order, and there is no instructions tab (the `+` button has a tooltip, and Ctrl+N works as in klogg). The scratchpad is its own window, as in klogg, and closes with the app.

**Why:** time zone data would need another dependency, and UTC is how logs are usually compared. Keeping the key order shows the JSON as it was logged.

**Code:** `kloggr_core::scratch`, `crate::scratchpad`.

## 2026-11-10: Search history and predefined filters

**klogg:** the search box is an editable combo box that lists the history. Predefined filters are a check-list combo box, and checking one replaces the search with the checked filters joined by `|` (or `or`). In boolean mode, pieces are quoted without escaping their quotes, and "Exclude from search" on an empty search gives `"" and not(...)`. The predefined filters dialog can import and export `.conf` files.

**kloggr:**
- The history is a drop-down button next to the search box: a recent search runs at once. It also has "Clear search history" and "Edit search history…".
- The filters are a "Filters" drop-down with check marks. It also has "Save search as filter…" (klogg has this in the search box's context menu) and "Edit predefined filters…".
- In boolean mode, quotes inside a piece are escaped (`\"`), and excluding from an empty search gives just `not("...")`.
- A filter counts as checked when it is exactly one of the alternatives of the search.
- Importing and exporting predefined filters is not done.
- History, history size and filters are saved in `searches.toml`.

**Why:** GPUI has no editable combo box. Escaping the quotes keeps the boolean search valid. Filter import and export can follow if it's asked for.

**Code:** `kloggr_core::searches`, `crate::saved_searches`, `Crawler::{history_button, filters_button, search_from_view}`.

## 2026-11-10: Options dialog: line numbers on by default, fewer options

**klogg:** the main view hides line numbers by default (`lineNumbersVisibleInMain_ = false`). The options dialog also has text wrap, language, High DPI, ANSI colour hiding, archives, https verification, the regex engine, read buffers, the search cache, logging and version checking.

**kloggr:**
- Line numbers show in both views by default, as kloggr always did. View > Line Numbers in Main View turns them off.
- Shortcuts work as in klogg: a primary and a secondary key per action, each set by pressing it in a small dialog, with duplicates shown in red and OK disabled. kloggr's "Find next" and "Find previous" are split in two actions, the F3 / Ctrl+G keys and the `n` / `N` keys of the log views, so that each fits in two keys.
- Changed shortcuts apply at once. GPUI's keymap can only grow, so a key that no longer applies is bound to `NoAction` in its context.
- Options for features kloggr doesn't have yet are left out, and come with those features. The font is any installed font, typed by name.
- Options are saved in `settings.toml` next to `session.toml`.

**Why:** the line numbers are what `[number] g` and Ctrl+L refer to. Options that do nothing would be misleading.

**Code:** `kloggr_core::options`, `crate::preferences`, `crate::shortcuts`, `crate::options_dialog`, `crate::key_capture`.

## 2026-11-10: Wide characters take two columns

**klogg:** a column is one UTF-16 code unit of the line (`QString`), drawn on a fixed-width grid. CJK characters and emoji are wider than a column in most fonts, so the text after them drifts from the grid. Selections, quick-find and highlights then cover the wrong characters, and characters outside the BMP count as two columns.

**kloggr:** a character takes its display width, from Unicode's East Asian Width (`unicode-width`): 2 columns for CJK and most emoji, 0 for combining marks and other zero-width characters (they stay with the character before), 1 otherwise. Tab stops count these columns. Each wide character is drawn alone at its column, so its glyph can't shift the rest of the line. A wide character cut by the left edge (or the frozen-columns separator) shows as blank.

**Why:** logs with CJK text or emoji stay aligned, and the mouse, selections and highlights land on the right characters. "Ambiguous" width characters (e.g. some Greek and box drawing in CJK locales) count as 1, as in most terminals.

**Code:** `kloggr_core::text::{width, window, grid_segments}`, `LogView` row pieces.

## 2026-11-10: Color label keys on Linux are also bound to the shifted symbols

**klogg:** `Ctrl+Shift+1`…`9` set color labels 1–9 and `Ctrl+Shift+0` clears them. Qt matches these by key, whatever the layout.

**kloggr:** binds `ctrl-shift-1`…`9` and `ctrl-shift-0` as klogg does. On Linux, any shortcut with Shift and a digit is also bound as the US layout's shifted symbol: `ctrl-!` `ctrl-@` … `ctrl-(` for the labels and `ctrl-)` to clear. The shortcut editor records such a key as `ctrl-shift-1`, as klogg shows it.

**Why:** GPUI's X11 and Wayland backends report Ctrl+Shift+1 as `ctrl-!`: the shifted character, without `shift`. Their keyboard mapper is a no-op, so `ctrl-shift-1` never matches a real key press (it only matches in tests). The extra bindings make the keys work on US-style layouts. On other layouts (e.g. German, where Shift+3 is `§`) the keys may not work; the log view's right-click menu sets labels on any layout. Revisit when GPUI gains a Linux keyboard mapper.

**Code:** `shortcuts::shifted_digit_alias`, `key_capture::canonical`.

## 2026-11-10: Highlighters: TOML settings, empty patterns, colour variance and layering

**klogg:** highlighter sets and color label styles are kept in `QSettings` (`HighlighterSetCollection`), exported and imported as `.conf` files. A highlighter with an empty pattern matches every line (an empty `QRegularExpression` matches anywhere), so an empty whole-line highlighter colours the whole file. "Variate colours" seeds `std::minstd_rand0` with the CRC32 of the matched text and draws a factor from `uniform_int_distribution`. Color labels are drawn over the search-match highlight.

**kloggr:**
- Sets, active sets and color label styles are saved in `highlighters.toml` next to `session.toml`, with colours as klogg's `#aarrggbb` strings. The highlighters dialog imports and exports klogg's `.conf` files (`highlight::klogg_conf`), so sets move between klogg and kloggr.
- A highlighter with an empty pattern never matches.
- "Variate colours" hashes the matched text with FNV-1a for the factor (same range, `100 ± variance`, and Qt's `darker`). The shades differ from klogg's, but each string still always gets the same colour.
- Highlighter and color label colours are opaque and drawn first. The search-match highlight (translucent gray), quick-find occurrences and the selection are drawn over them, so the search highlight tints a labelled word instead of hiding under it.

**Why:** TOML matches the rest of kloggr's settings. An empty pattern is almost always a highlighter that hasn't been filled in yet, and colouring every line would be surprising. The standard library's distribution algorithm isn't specified, so klogg's exact shades can't be reproduced portably anyway. The layering keeps the translucent search highlight (see the 2026-11-09 entry) visible everywhere.

**Code:** `kloggr_core::highlight`, `crate::highlighters` (app), `LogView::context_menu`, `colored_columns`.

## 2026-11-09: Changed files are detected by sampling, not by hashing the whole file

**klogg:** when the file changes on disk, klogg hashes the whole indexed part again by default (`fastModificationDetection_ = false`) to tell appended data from a rewrite. A faster header-and-tail hash is an option.

**kloggr:** compares the file's identity (device and inode), its size, and the first and last 64 KiB of the indexed part with the file on disk (`LogFile::check`). This always reads at most 128 KiB.

**Why:** hashing a multi-GB file on every write makes following a busy log impossible. A rewrite of the same size that leaves both ends unchanged goes unnoticed, which is the same limit as klogg's fast mode. If that becomes a problem, add a setting for a full check, as klogg has.

## 2026-11-09: Follow mode starts again from a simple scroll at the bottom

**klogg:** while you're at the bottom, follow mode starts again after you "pull" the view past the end by a set distance (`followElasticHook_`, `HookThreshold`), and the view draws the pull.

**kloggr:** any wheel scroll down while already at the bottom starts follow mode again, as does Shift+G at the bottom (also in klogg). Scrolling up, Page Up, a jump (marks, `g`, Ctrl+L, quick find) or selecting a row in the filtered view stops it.

**Why:** this is simpler, and it doesn't need the elastic animation. Revisit it if starting follow by accident turns out to be annoying.

## 2026-11-09: Auto-refresh searches the incomplete last line again

**klogg:** `updateSearch` continues from the number of lines already processed.

**kloggr:** continues from the last searched line, so a line that was still being written during the last search is searched again once it's complete. Duplicate matches are dropped. Auto-refresh is off by default, as in klogg.

**Why:** logs are often read while a line is half written. Without this, such a line would never match.

## 2026-11-09: Search matches are highlighted in the text by default

**klogg:** the main search pattern's matches can be highlighted in both views. This option ("main search highlight", `enableMainSearchHighlight_`) is **off** by default, and its colour is light gray. Excluding and boolean patterns are never highlighted.

**kloggr:** highlighting is **on**, with a translucent gray (the theme's text colour at 18 %) so it works in both light and dark themes. Like klogg, kloggr highlights nothing for excluding or boolean patterns. Quick-find occurrences (yellow) and the selection are drawn on top of it.

**Why:** the user asked for it as the Phase 2 follow-up. There is no options dialog yet; when Phase 3 adds settings, this becomes a setting that defaults to on, with a colour chooser for the highlight that applies while it is on (klogg: `mainSearchBackColor`).

**Code:** `Crawler::set_search_highlight`, `LogView::search_highlight`.

## 2026-11-09: Digits in a log view always type a line number; 1–7 shortcuts move to Alt+1…Alt+7

**klogg:** the bare keys `1`–`7` are crawler shortcuts:
- `1` / `2` / `3` set the filtered view's visibility (marks and matches / marks / matches).
- `4` / `5` / `6` / `7` toggle match case / regex / inverse / boolean combining.

klogg also has a digits buffer: `[number] g` jumps to a line, and `[number] j/k` moves by that many lines. The Qt shortcuts take the key before the view's `keyPressEvent` sees it, so a number that starts with 1–7 never reaches the buffer. Typing `123g` switches visibility three times and then jumps to line 1. Only numbers starting with 8 or 9 work.

**kloggr:**
- Digits typed in a log view always go to the number buffer, so `123g` jumps to line 123 and `5j` moves down 5 lines.
- The seven shortcuts are **Alt+1 … Alt+7**, in the same order.
- The visibility dropdown and the option toggles still work with the mouse, and their tooltips show the Alt keys.

**Why:** klogg's behavior makes `[number] g` unusable for most numbers. The user chose this option over matching klogg exactly or a mixed scheme.

**Code:** `crawler::key_bindings`, `LogView::on_key_down`, `kloggr_core::digits`.

## 2026-11-09: `[number] g` in the filtered view goes to a file line, not a row

**klogg:** `LogViewJumpToLineNumber` selects *row* `number - 1` of whichever view has focus. In the filtered view, that row usually holds a different file line than the one typed.

**kloggr:** the number is always a file line, as shown in the line-number gutter. In the filtered view, kloggr selects the row that shows that line, or the closest row before it. Ctrl+L uses the same rule, and klogg's `goToLine` does too (`getLineIndexNumber`).

**Why:** the filtered view shows file line numbers, so a number typed there should mean the same thing as the number shown.

**Code:** `LogView::jump_to_line_number`.

## 2026-11-10: Large selections: size limits for copying

**klogg:** copies any selection at once on the UI thread, and also puts every mouse selection into the X11 primary selection. Selecting all of a multi-GB file and copying freezes the window while the whole text is built.

**kloggr:**
- The selection's size is found from the line index, without reading the file.
- Up to 64 MiB is copied at once. Above that, kloggr asks "The selection is … Copy it to the clipboard?" and builds the text on a worker thread; the window stays usable.
- Above 1 GiB nothing is copied: a message says the selection is too large.
- The primary selection is only set for selections up to 8 MiB. Sending to the scratchpad stops at 8 MiB too, as its text editor lays out all of it.

**Why:** GPUI's clipboard takes one string, so the text can't be streamed in chunks. The clipboard and the receiving app hold the whole text in memory, often more than once.

**Code:** `Selection::{byte_len, byte_len_mapped}`, `LogView::{copy_selection, copy_in_background}`, `CopyLimits`.

## 2026-11-10: Crash reports stay local

**klogg:** Crashpad writes a minidump on a crash. At the next start klogg asks whether to send it to Sentry, and on an exception it offers to open a pre-filled GitHub issue.

**kloggr:**
- A panic hook writes a plain-text report (message, place, thread, version, OS, backtrace) to `<config dir>/crashes/new/`. It covers panics on any thread; native crashes (signals) aren't caught.
- At the next start a dialog shows the first line and where the report is, with a "Copy report" button. The report then moves to `crashes/`, which keeps the last 10.
- Nothing is sent automatically. "Report issue" copies the report and opens a new GitHub issue on github.com/onionhead0708/kloggr with the start of the report filled in (klogg's `IssueReporter` does the same for exceptions).

**Why:** there is no crash server. A text report is enough for a bug report and needs no extra dependencies.

**Code:** `kloggr_core::crash`, `crate::crash_report`.

## 2026-11-10: Linux packages from a script, no RPM yet

**klogg:** CMake's CPack builds .deb and .rpm (dependencies found with shlibdeps), and linuxdeployqt builds an AppImage that bundles Qt and other libraries.

**kloggr:**
- `scripts/package-linux.sh` builds a tarball (a `bin/` + `share/` tree to extract into `~/.local` or `/usr/local`) and a .deb with `dpkg-deb`. The .deb's dependencies come from `dpkg-shlibdeps`, plus `libvulkan1` and `libwayland-client0`, which GPUI loads at run time.
- An AppImage is built only when `appimagetool` is installed. It bundles nothing besides the binary: kloggr links only libc, libxcb and libxkbcommon, which desktops have.
- No .rpm yet.
- The icon is kloggr's own (`packaging/linux/kloggr.svg`), not klogg's. Windows use app id `kloggr`, matching `kloggr.desktop`.

**Why:** a Rust binary has few shared-library dependencies, so plain tools are enough and no extra cargo tools are needed. Using klogg's icon would make the two apps look the same.

## 2026-11-10: Renamed from rlogg to kloggr

**Before:** the project, crates, binary and config folder were called rlogg.

**Now:** kloggr (crates `kloggr-core`, `kloggr-app`, binary `kloggr`, `~/.config/kloggr`, `KLOGGR_*` variables). On first start, settings in `~/.config/rlogg` move to `~/.config/kloggr`. Older entries in this file were updated to the new name.

**Why:** github.com/jmichaelsturm/rlogg is another Rust viewer for very large text files. The new name keeps the link to klogg and avoids confusing the two.

## 2026-11-10: Version check against GitHub releases

**klogg:** when "check for new versions" is on (the default), downloads a version file from klogg's site at most once every 7 days and announces a newer version with its changes.

**kloggr:**
- Same option (on by default, Options > General) and the same 7-day interval, saved in `version_check.toml`. A failed check also waits a week.
- Asks GitHub's API for the latest release of github.com/onionhead0708/kloggr, on a worker thread with a 20 s timeout. The request carries only a `kloggr/<version>` user agent.
- A newer version shows its version, the start of the release notes and "Open download page".
- HTTPS uses rustls with the system's certificate store (`rustls-platform-verifier`), not a bundled CA list.
- Debug builds read `KLOGGR_RELEASE_URL` to try the notice against another repository.

**Why:** GitHub releases are where the packages are published, so no separate version file is needed.

**Code:** `kloggr_core::version_check`, `crate::update_check`.

## 2026-11-10: Unsigned Windows and macOS packages

**klogg:** ships an .msi and a portable .zip for Windows, and a .dmg for macOS. Its macOS app is unsigned too.

**kloggr:**
- Windows: `kloggr.exe` carries the icon and has no console window in release builds. Packages are a portable .zip and an .msi made with WiX 5 (per-machine, Start menu shortcut, upgrades in place). Not code-signed.
- macOS: one universal `kloggr.app` (Apple Silicon and Intel) in a .dmg, ad-hoc signed only: no Developer ID, no notarization. The README explains how to allow it.
- The X11/Wayland primary selection only exists on Linux and FreeBSD; elsewhere selecting text no longer touches it.
- macOS doesn't yet open files from Finder (*Open With*): it would need GPUI's open-URL events and document types in `Info.plist`.
- Known Windows limitation: kloggr memory-maps the files it shows, and Windows refuses to truncate a file while another program has it mapped (error 1224, found by CI). A program that truncates or rewrites its log in place gets an error while the log is open in kloggr. Appending works. klogg reads files in blocks instead of mapping them, so it doesn't have this problem. The two truncation tests are skipped on Windows until this is fixed (by reading in blocks, or by mapping and releasing on each read).

**Why:** signing needs a paid Apple Developer account and a Windows code-signing certificate. GitHub's Windows and macOS runners build and test the code on each push, since it can't be run on those systems here.
