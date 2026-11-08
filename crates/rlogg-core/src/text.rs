//! Display text for the log view: tab expansion, control characters and
//! horizontal windowing of long lines.
//!
//! Ports klogg's `untabify` (`src/logdata/include/linetypes.h`, GPLv3): tabs
//! expand to the next multiple of [`TAB_STOP`] columns and NUL shows as a
//! space. Other C0 control characters (e.g. the ESC of ANSI color codes) are
//! shown as Unicode control pictures (`␛`) so they never reach the shaper.
//!
//! A column is one `char` of the expanded text, as in klogg (which assumes a
//! fixed-width font). [`window`] decodes a line incrementally and stops after
//! the visible columns, so the cost of drawing a line depends on how far it is
//! scrolled, not on its length.

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

/// Calls `emit` for each display column of `chars`, expanding tabs.
fn for_each_column(chars: impl Iterator<Item = char>, mut emit: impl FnMut(char) -> bool) {
    let mut col = 0usize;
    for c in chars {
        if c == '\t' {
            let spaces = TAB_STOP - col % TAB_STOP;
            for _ in 0..spaces {
                if !emit(' ') {
                    return;
                }
            }
            col += spaces;
        } else {
            if !emit(display_char(c)) {
                return;
            }
            col += 1;
        }
    }
}

/// The whole line as displayed (klogg: `getExpandedLineString`).
pub fn expand(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for_each_column(line.chars(), |c| {
        out.push(c);
        true
    });
    out
}

/// Number of display columns of `line`.
pub fn columns(line: &str) -> usize {
    let mut n = 0;
    for_each_column(line.chars(), |_| {
        n += 1;
        true
    });
    n
}

/// A horizontal slice of a line, ready to shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayWindow {
    /// Display text of columns `first_col..first_col + text.chars().count()`.
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
            let (shown, width) = match c {
                '\t' => (' ', TAB_STOP - col % TAB_STOP),
                c => (display_char(c), 1),
            };
            for _ in 0..width {
                if col >= end_col {
                    return DisplayWindow {
                        text,
                        total_cols: None,
                    };
                }
                if col >= first_col {
                    text.push(shown);
                }
                col += 1;
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
/// (klogg: `selectWordAtPosition`): letters, digits and `_`.
pub fn word_at(expanded: &str, col: usize) -> Option<Range<usize>> {
    let chars: Vec<char> = expanded.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(col).is_some_and(|&c| is_word(c)) {
        return None;
    }
    let start = chars[..col]
        .iter()
        .rposition(|&c| !is_word(c))
        .map_or(0, |i| i + 1);
    let end = chars[col..]
        .iter()
        .position(|&c| !is_word(c))
        .map_or(chars.len(), |i| col + i);
    Some(start..end)
}

/// Columns `range` of an expanded line, as text.
pub fn slice_columns(expanded: &str, range: Range<usize>) -> String {
    expanded
        .chars()
        .skip(range.start)
        .take(range.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let expanded: Vec<char> = expand(&line).chars().collect();
        for (first, n) in [
            (0, 100),
            (4090, 20),
            (12_345, 500),
            (expanded.len() - 3, 10),
        ] {
            let w = win(&line, first, n);
            let expected: String = expanded.iter().skip(first).take(n).collect();
            assert_eq!(w.text, expected, "first={first}");
        }
        assert_eq!(win(&line, 0, usize::MAX).total_cols, Some(expanded.len()));
    }

    #[test]
    fn window_decodes_utf16_across_steps() {
        let text = "\u{65e5}\u{672c}".repeat(3000);
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let w = window(Encoding::UTF_16LE, &bytes, 2047, 4);
        assert_eq!(w.text, "\u{672c}\u{65e5}\u{672c}\u{65e5}");
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
