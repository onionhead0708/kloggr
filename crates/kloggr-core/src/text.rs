//! Display text for the log view: tab expansion, control characters and
//! horizontal windowing of long lines.
//!
//! Ports klogg's `untabify` (`src/logdata/include/linetypes.h`, GPLv3): tabs
//! expand to the next multiple of [`TAB_STOP`] columns and NUL shows as a
//! space. Other C0 control characters (e.g. the ESC of ANSI color codes) are
//! shown as Unicode control pictures (`␛`) so they never reach the shaper.
//!
//! Text is laid out on a grid of fixed-width columns. Unlike klogg, which
//! gives every UTF-16 code unit one column, a character takes its display
//! width ([`width`]): two columns for CJK and most emoji, none for combining
//! marks (see `docs/decisions.md`). [`window`] decodes a line incrementally
//! and stops after the visible columns, so the cost of drawing a line
//! depends on how far it is scrolled, not on its length.

use std::ops::Range;

use crate::encoding::Encoding;

/// Columns per tab stop (klogg: `TabStop`).
pub const TAB_STOP: usize = 8;

/// The character drawn for `c` (tabs are handled by the caller).
pub fn display_char(c: char) -> char {
    match c {
        '\0' => ' ',
        '\u{1}'..='\u{1f}' => char::from_u32(0x2400 + c as u32).unwrap_or(c),
        '\u{7f}' => '\u{2421}',
        c => c,
    }
}

/// Columns taken by a displayed character (not a tab): 2 for East Asian
/// wide and emoji, 0 for combining marks and other zero-width characters,
/// otherwise 1.
pub fn width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
}

/// Calls `emit` with each displayed character of `chars` and its width,
/// expanding tabs to spaces.
fn for_each_char(chars: impl Iterator<Item = char>, mut emit: impl FnMut(char, usize) -> bool) {
    let mut col = 0usize;
    for c in chars {
        if c == '\t' {
            let spaces = TAB_STOP - col % TAB_STOP;
            for _ in 0..spaces {
                if !emit(' ', 1) {
                    return;
                }
            }
            col += spaces;
        } else {
            let shown = display_char(c);
            let w = width(shown);
            if !emit(shown, w) {
                return;
            }
            col += w;
        }
    }
}

/// The start column of each character of a displayed (expanded) text, and
/// its width.
pub fn cells(display: &str) -> impl Iterator<Item = (usize, usize, char)> + '_ {
    let mut col = 0;
    display.char_indices().map(move |(i, c)| {
        let w = width(c);
        let start = col;
        col += w;
        (start, i, c)
    })
}

/// Byte offset in a displayed text of the first character starting at or
/// after column `col` (the text's length if none).
pub fn byte_at_column(display: &str, col: usize) -> usize {
    cells(display)
        .find(|&(start, _, c)| start >= col && width(c) > 0)
        .map_or(display.len(), |(_, i, _)| i)
}

/// Splits a displayed text into pieces to draw at their grid column, as
/// (column, byte range): runs of single-width characters, and each wide
/// character alone (with the zero-width characters that follow), so that a
/// glyph wider or narrower than its columns can't shift the rest.
pub fn grid_segments(display: &str) -> Vec<(usize, Range<usize>)> {
    let mut out: Vec<(usize, Range<usize>)> = Vec::new();
    let mut wide_open = false;
    for (col, i, c) in cells(display) {
        let end = i + c.len_utf8();
        match (width(c), out.last_mut()) {
            (0, Some(last)) => last.1.end = end,
            (1, Some(last)) if !wide_open => last.1.end = end,
            (w, _) => {
                out.push((col, i..end));
                wide_open = w > 1;
            }
        }
    }
    out
}

/// The whole line as displayed (klogg: `getExpandedLineString`).
pub fn expand(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for_each_char(line.chars(), |c, _| {
        out.push(c);
        true
    });
    out
}

/// Number of display columns of `line`.
pub fn columns(line: &str) -> usize {
    let mut n = 0;
    for_each_char(line.chars(), |_, w| {
        n += w;
        true
    });
    n
}

/// Maps sorted, non-overlapping byte ranges of `line` (e.g. regex matches)
/// to display column ranges, in one pass. Range ends must be char
/// boundaries.
pub fn column_ranges(line: &str, bytes: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut wanted = bytes.iter().peekable();
    let mut start_col = None;
    let mut col = 0usize;
    let mut chars = line.char_indices();
    loop {
        let (at, c) = chars
            .next()
            .map_or((line.len(), None), |(i, c)| (i, Some(c)));
        // Resolve every boundary at this byte (an empty range has two).
        while let Some(r) = wanted.peek() {
            match start_col {
                None if r.start <= at => start_col = Some(col),
                Some(start) if r.end <= at => {
                    out.push(start..col);
                    start_col = None;
                    wanted.next();
                }
                _ => break,
            }
        }
        let Some(c) = c else { break };
        col += if c == '\t' {
            TAB_STOP - col % TAB_STOP
        } else {
            width(display_char(c))
        };
    }
    out
}

/// A horizontal slice of a line, ready to shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayWindow {
    /// Display text starting at column `first_col`. A wide character cut by
    /// the window's start shows as spaces; one cut by its end is included.
    pub text: String,
    /// Total columns of the line, if the whole line was decoded (it was
    /// shorter than the window end).
    pub total_cols: Option<usize>,
}

/// Bytes decoded per step by [`window`].
const DECODE_STEP: usize = 4096;

/// Decodes `bytes` (one line, no terminator) and returns display columns
/// `first_col..first_col + max_cols`.
pub fn window(
    encoding: Encoding,
    bytes: &[u8],
    first_col: usize,
    max_cols: usize,
) -> DisplayWindow {
    let end_col = first_col.saturating_add(max_cols);
    let mut decoder = encoding.new_decoder();
    let mut text = String::new();
    let mut col = 0usize;
    // Whether the last character with a width went into `text`.
    let mut shown_last = false;
    let mut buf = String::new();
    let mut chunks = bytes.chunks(DECODE_STEP).peekable();
    'decode: while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        buf.clear();
        buf.reserve(
            decoder
                .max_utf8_buffer_length(chunk.len())
                .unwrap_or(chunk.len() * 3),
        );
        // Capacity is reserved, so the whole chunk is always consumed.
        let _ = decoder.decode_to_string(chunk, &mut buf, last);
        for c in buf.chars() {
            let (shown, w, cells) = match c {
                // A tab is that many single-width spaces.
                '\t' => (' ', 1, TAB_STOP - col % TAB_STOP),
                c => {
                    let shown = display_char(c);
                    (shown, width(shown), 1)
                }
            };
            if w == 0 {
                // Attached to the character before, if it is shown.
                if shown_last {
                    text.push(shown);
                }
                continue;
            }
            for _ in 0..cells {
                if col >= end_col {
                    return DisplayWindow {
                        text,
                        total_cols: None,
                    };
                }
                if col >= first_col {
                    text.push(shown);
                    shown_last = true;
                } else {
                    // The part of a wide character inside the window.
                    let inside = (col + w).saturating_sub(first_col);
                    text.extend(std::iter::repeat_n(' ', inside));
                    shown_last = false;
                }
                col += w;
            }
        }
        if col >= end_col && chunks.peek().is_some() {
            break 'decode;
        }
    }
    let total_cols = (col < end_col || chunks.peek().is_none()).then_some(col);
    DisplayWindow { text, total_cols }
}

/// Word under display column `col` of an expanded line, as a column range
/// (klogg: `selectWordAtPosition`): letters, digits and `_`, with the
/// zero-width characters (e.g. combining accents) that follow them.
pub fn word_at(expanded: &str, col: usize) -> Option<Range<usize>> {
    // (start, end, is part of a word) of each character with a width.
    let mut cells: Vec<(usize, usize, bool)> = Vec::new();
    for (start, _, c) in self::cells(expanded) {
        let w = width(c);
        if w == 0 {
            continue;
        }
        cells.push((start, start + w, c.is_alphanumeric() || c == '_'));
    }
    let at = cells.iter().position(|&(s, e, _)| s <= col && col < e)?;
    if !cells[at].2 {
        return None;
    }
    let first = cells[..at].iter().rposition(|c| !c.2).map_or(0, |i| i + 1);
    let last = cells[at..]
        .iter()
        .position(|c| !c.2)
        .map_or(cells.len(), |i| at + i)
        - 1;
    Some(cells[first].0..cells[last].1)
}

/// Columns `range` of an expanded line, as text: the characters starting
/// in it, with the zero-width characters attached to them.
pub fn slice_columns(expanded: &str, range: Range<usize>) -> String {
    let mut out = String::new();
    let mut taken = false;
    for (start, _, c) in cells(expanded) {
        if width(c) > 0 {
            taken = range.contains(&start);
        }
        if taken {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn column_ranges_expand_tabs() {
        // "a\tbc": a=0, tab=1..8, b=8, c=9.
        assert_eq!(column_ranges("a\tbc", &[0..1, 2..4]), [0..1, 8..10]);
        assert_eq!(column_ranges("a\tbc", &[1..2]), [1..8]);
        assert_eq!(column_ranges("été x", &[2..5, 6..7]), [1..3, 4..5]);
        assert_eq!(column_ranges("abc", &[1..1, 3..3]), [1..1, 3..3]);
        assert_eq!(column_ranges("", &[0..0]), [0..0]);
        assert!(column_ranges("abc", &[]).is_empty());
    }

    fn win(s: &str, first: usize, n: usize) -> DisplayWindow {
        window(Encoding::UTF_8, s.as_bytes(), first, n)
    }

    #[test]
    fn expands_tabs_to_tab_stops() {
        assert_eq!(expand("a\tb"), "a       b");
        assert_eq!(expand("12345678\tx"), format!("12345678{}x", " ".repeat(8)));
        assert_eq!(expand("\t"), " ".repeat(8));
        assert_eq!(columns("ab\tc"), 9);
    }

    #[test]
    fn control_characters_are_visible() {
        assert_eq!(expand("a\0b"), "a b");
        assert_eq!(expand("\u{1b}[31mred"), "\u{241b}[31mred");
        assert_eq!(expand("\u{7f}"), "\u{2421}");
        assert_eq!(expand("\r"), "\u{240d}");
    }

    #[test]
    fn window_slices_expanded_columns() {
        let w = win("abcdef", 2, 3);
        assert_eq!(w.text, "cde");
        assert_eq!(w.total_cols, None);
        let w = win("abcdef", 2, 10);
        assert_eq!(w.text, "cdef");
        assert_eq!(w.total_cols, Some(6));
        // Starting inside a tab's expansion.
        assert_eq!(win("a\tb", 3, 10).text, "     b");
        assert_eq!(
            win("", 0, 10),
            DisplayWindow {
                text: String::new(),
                total_cols: Some(0)
            }
        );
        assert_eq!(win("abc", 10, 5).text, "");
    }

    #[test]
    fn window_matches_expand_on_long_lines() {
        let line: String = (0..20_000)
            .map(|i| match i % 13 {
                0 => '\t',
                5 => '\u{65e5}',
                _ => char::from(b'a' + (i % 26) as u8),
            })
            .collect();
        // One entry per column: a character, or `None` for the second
        // column of a wide one.
        let grid: Vec<Option<char>> = expand(&line)
            .chars()
            .flat_map(|c| {
                std::iter::once(Some(c))
                    .chain(std::iter::repeat_n(None, width(c).saturating_sub(1)))
            })
            .collect();
        for (first, n) in [(0, 100), (4090, 20), (12_345, 500), (grid.len() - 3, 10)] {
            let w = win(&line, first, n);
            let expected: String = grid
                .iter()
                .enumerate()
                .skip(first)
                .take(n)
                .filter_map(|(i, c)| match c {
                    Some(c) => Some(*c),
                    // A wide character cut by the window's start.
                    None if i == first => Some(' '),
                    None => None,
                })
                .collect();
            assert_eq!(w.text, expected, "first={first}");
        }
        assert_eq!(win(&line, 0, usize::MAX).total_cols, Some(grid.len()));
    }

    #[test]
    fn window_decodes_utf16_across_steps() {
        let text = "\u{65e5}\u{672c}".repeat(3000);
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        // Two columns per character: column 4094 starts character 2047.
        let w = window(Encoding::UTF_16LE, &bytes, 4094, 8);
        assert_eq!(w.text, "\u{672c}\u{65e5}\u{672c}\u{65e5}");
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn wide_and_zero_width_characters() {
        // 日 and 本 take two columns, the combining accent none.
        assert_eq!(width('日'), 2);
        assert_eq!(width('\u{301}'), 0);
        assert_eq!(width('a'), 1);
        assert_eq!(columns("a日本b"), 6);
        assert_eq!(columns("e\u{301}x"), 2);
        // Tabs stop at multiples of 8 display columns.
        assert_eq!(expand("日\tx"), format!("日{}x", " ".repeat(6)));
        assert_eq!(column_ranges("a日本b", &[1..4, 7..8]), [1..3, 5..6]);
        assert_eq!(column_ranges("e\u{301}x", &[3..4]), [1..2]);

        // A wide character cut by the window's start shows as a space.
        assert_eq!(win("a日本b", 1, 10).text, "日本b");
        assert_eq!(win("a日本b", 2, 10).text, " 本b");
        assert_eq!(win("a日本b", 3, 2).text, "本");
        assert_eq!(win("a日本b", 3, 1).text, "本", "overflows the end");
        assert_eq!(win("a日本b", 0, 10).total_cols, Some(6));
        // Zero-width characters stay with their base.
        assert_eq!(win("e\u{301}x", 0, 1).text, "e\u{301}");
        assert_eq!(win("e\u{301}x", 1, 5).text, "x");

        assert_eq!(byte_at_column("a日本b", 1), 1);
        assert_eq!(byte_at_column("a日本b", 2), 4, "inside 日: the next char");
        assert_eq!(byte_at_column("a日本b", 6), 8);
        assert_eq!(
            grid_segments("ab日c\u{301}d本"),
            [(0, 0..2), (2, 2..5), (4, 5..9), (6, 9..12)]
        );
        assert_eq!(grid_segments("x\u{301}"), [(0, 0..3)]);
        assert!(grid_segments("").is_empty());

        assert_eq!(word_at("ab 日本語 c", 5), Some(3..9));
        assert_eq!(word_at("ab 日本語 c", 4), Some(3..9));
        assert_eq!(word_at("ab 日本語 c", 9), None);
        assert_eq!(word_at("cafe\u{301} x", 2), Some(0..4));
        assert_eq!(slice_columns("a日本b", 1..5), "日本");
        assert_eq!(slice_columns("a日本b", 2..6), "本b", "starts inside 日");
        assert_eq!(slice_columns("cafe\u{301} x", 0..4), "cafe\u{301}");
    }

    #[test]
    fn words() {
        let s = "foo bar_baz,42 x";
        assert_eq!(word_at(s, 0), Some(0..3));
        assert_eq!(word_at(s, 6), Some(4..11));
        assert_eq!(word_at(s, 3), None);
        assert_eq!(word_at(s, 13), Some(12..14));
        assert_eq!(word_at(s, 15), Some(15..16));
        assert_eq!(word_at(s, 99), None);
        assert_eq!(slice_columns(s, 4..11), "bar_baz");
    }
}
