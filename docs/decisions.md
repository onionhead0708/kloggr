# Decisions

Places where rlogg deliberately differs from klogg, and why. klogg stays the
spec of record (see `CLAUDE.md`). Every intentional difference is recorded
here so it isn't "fixed" back later by mistake.

Newest first. Each entry gives the date, what klogg does, what rlogg does, and why.

---

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
