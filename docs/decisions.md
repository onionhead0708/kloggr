# Decisions

Places where rlogg deliberately differs from klogg, and why. klogg stays the
spec of record (see `CLAUDE.md`). Every intentional difference is recorded
here so it isn't "fixed" back later by mistake.

Newest first. Each entry gives the date, what klogg does, what rlogg does, and why.

---

## 2026-11-10: Color label keys on Linux are also bound to the shifted symbols

**klogg:** `Ctrl+Shift+1`…`9` set color labels 1–9 and `Ctrl+Shift+0` clears them. Qt matches these by key, whatever the layout.

**rlogg:** binds `ctrl-shift-1`…`9` and `ctrl-shift-0` as klogg does. On Linux, it also binds the US layout's shifted digits: `ctrl-!` `ctrl-@` `ctrl-#` `ctrl-$` `ctrl-%` `ctrl-^` `ctrl-&` `ctrl-*` `ctrl-(` for the labels and `ctrl-)` to clear.

**Why:** GPUI's X11 and Wayland backends report Ctrl+Shift+1 as `ctrl-!`: the shifted character, without `shift`. Their keyboard mapper is a no-op, so `ctrl-shift-1` never matches a real key press (it only matches in tests). The extra bindings make the keys work on US-style layouts. On other layouts (e.g. German, where Shift+3 is `§`) the keys may not work; the log view's right-click menu sets labels on any layout. Revisit when GPUI gains a Linux keyboard mapper.

**Code:** `highlighters::key_bindings`.

## 2026-11-10: Highlighters: TOML settings, empty patterns, colour variance and layering

**klogg:** highlighter sets and color label styles are kept in `QSettings` (`HighlighterSetCollection`), exported and imported as `.conf` files. A highlighter with an empty pattern matches every line (an empty `QRegularExpression` matches anywhere), so an empty whole-line highlighter colours the whole file. "Variate colours" seeds `std::minstd_rand0` with the CRC32 of the matched text and draws a factor from `uniform_int_distribution`. Color labels are drawn over the search-match highlight.

**rlogg:**
- Sets, active sets and color label styles are saved in `highlighters.toml` next to `session.toml`, with colours as klogg's `#aarrggbb` strings. The highlighters dialog imports and exports klogg's `.conf` files (`highlight::klogg_conf`), so sets move between klogg and rlogg.
- A highlighter with an empty pattern never matches.
- "Variate colours" hashes the matched text with FNV-1a for the factor (same range, `100 ± variance`, and Qt's `darker`). The shades differ from klogg's, but each string still always gets the same colour.
- Highlighter and color label colours are opaque and drawn first. The search-match highlight (translucent gray), quick-find occurrences and the selection are drawn over them, so the search highlight tints a labelled word instead of hiding under it.

**Why:** TOML matches the rest of rlogg's settings. An empty pattern is almost always a highlighter that hasn't been filled in yet, and colouring every line would be surprising. The standard library's distribution algorithm isn't specified, so klogg's exact shades can't be reproduced portably anyway. The layering keeps the translucent search highlight (see the 2026-11-09 entry) visible everywhere.

**Code:** `rlogg_core::highlight`, `crate::highlighters` (app), `LogView::context_menu`, `colored_columns`.

## 2026-11-09: Changed files are detected by sampling, not by hashing the whole file

**klogg:** when the file changes on disk, klogg hashes the whole indexed part again by default (`fastModificationDetection_ = false`) to tell appended data from a rewrite. A faster header-and-tail hash is an option.

**rlogg:** compares the file's identity (device and inode), its size, and the first and last 64 KiB of the indexed part with the file on disk (`LogFile::check`). This always reads at most 128 KiB.

**Why:** hashing a multi-GB file on every write makes following a busy log impossible. A rewrite of the same size that leaves both ends unchanged goes unnoticed, which is the same limit as klogg's fast mode. If that becomes a problem, add a setting for a full check, as klogg has.

## 2026-11-09: Follow mode starts again from a simple scroll at the bottom

**klogg:** while you're at the bottom, follow mode starts again after you "pull" the view past the end by a set distance (`followElasticHook_`, `HookThreshold`), and the view draws the pull.

**rlogg:** any wheel scroll down while already at the bottom starts follow mode again, as does Shift+G at the bottom (also in klogg). Scrolling up, Page Up, a jump (marks, `g`, Ctrl+L, quick find) or selecting a row in the filtered view stops it.

**Why:** this is simpler, and it doesn't need the elastic animation. Revisit it if starting follow by accident turns out to be annoying.

## 2026-11-09: Auto-refresh searches the incomplete last line again

**klogg:** `updateSearch` continues from the number of lines already processed.

**rlogg:** continues from the last searched line, so a line that was still being written during the last search is searched again once it's complete. Duplicate matches are dropped. Auto-refresh is off by default, as in klogg.

**Why:** logs are often read while a line is half written. Without this, such a line would never match.

## 2026-11-09: Search matches are highlighted in the text by default

**klogg:** the main search pattern's matches can be highlighted in both views. This option ("main search highlight", `enableMainSearchHighlight_`) is **off** by default, and its colour is light gray. Excluding and boolean patterns are never highlighted.

**rlogg:** highlighting is **on**, with a translucent gray (the theme's text colour at 18 %) so it works in both light and dark themes. Like klogg, rlogg highlights nothing for excluding or boolean patterns. Quick-find occurrences (yellow) and the selection are drawn on top of it.

**Why:** the user asked for it as the Phase 2 follow-up. There is no options dialog yet; when Phase 3 adds settings, this becomes a setting that defaults to on, with a colour chooser for the highlight that applies while it is on (klogg: `mainSearchBackColor`).

**Code:** `Crawler::set_search_highlight`, `LogView::search_highlight`.

## 2026-11-09: Digits in a log view always type a line number; 1–7 shortcuts move to Alt+1…Alt+7

**klogg:** the bare keys `1`–`7` are crawler shortcuts:
- `1` / `2` / `3` set the filtered view's visibility (marks and matches / marks / matches).
- `4` / `5` / `6` / `7` toggle match case / regex / inverse / boolean combining.

klogg also has a digits buffer: `[number] g` jumps to a line, and `[number] j/k` moves by that many lines. The Qt shortcuts take the key before the view's `keyPressEvent` sees it, so a number that starts with 1–7 never reaches the buffer. Typing `123g` switches visibility three times and then jumps to line 1. Only numbers starting with 8 or 9 work.

**rlogg:**
- Digits typed in a log view always go to the number buffer, so `123g` jumps to line 123 and `5j` moves down 5 lines.
- The seven shortcuts are **Alt+1 … Alt+7**, in the same order.
- The visibility dropdown and the option toggles still work with the mouse, and their tooltips show the Alt keys.

**Why:** klogg's behavior makes `[number] g` unusable for most numbers. The user chose this option over matching klogg exactly or a mixed scheme.

**Code:** `crawler::key_bindings`, `LogView::on_key_down`, `rlogg_core::digits`.

## 2026-11-09: `[number] g` in the filtered view goes to a file line, not a row

**klogg:** `LogViewJumpToLineNumber` selects *row* `number - 1` of whichever view has focus. In the filtered view, that row usually holds a different file line than the one typed.

**rlogg:** the number is always a file line, as shown in the line-number gutter. In the filtered view, rlogg selects the row that shows that line, or the closest row before it. Ctrl+L uses the same rule, and klogg's `goToLine` does too (`getLineIndexNumber`).

**Why:** the filtered view shows file line numbers, so a number typed there should mean the same thing as the number shown.

**Code:** `LogView::jump_to_line_number`.
