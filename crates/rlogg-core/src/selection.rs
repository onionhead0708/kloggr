//! Selection in a log view.
//!
//! Port of klogg's `Selection` (`src/ui/src/selection.cpp`, GPLv3) and of the
//! drag rules in `AbstractLogView::mouseMoveEvent`. A selection is one whole
//! line, a range of whole lines, or a portion of a single line. Portion
//! columns are display columns (tabs expanded), as in klogg.

use std::ops::RangeInclusive;

use crate::LogFile;
use crate::text;

/// A line and display column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    None,
    Line(usize),
    /// Whole lines `start..=end`; `anchor` is where it began (for
    /// shift+click), one of `start` or `end`.
    Range {
        start: usize,
        end: usize,
        anchor: usize,
    },
    /// Display columns `start..=end` of `line` (both included, as in klogg).
    Portion {
        line: usize,
        start: usize,
        end: usize,
    },
}

impl Selection {
    pub fn range(anchor: usize, other: usize) -> Self {
        Self::Range {
            start: anchor.min(other),
            end: anchor.max(other),
            anchor,
        }
    }

    pub fn portion(line: usize, a: usize, b: usize) -> Self {
        Self::Portion {
            line,
            start: a.min(b),
            end: a.max(b),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::None
    }

    /// Shift+click: a range from the current line, or from line 0 (klogg:
    /// `selectRangeFromPrevious`).
    pub fn extended_to(&self, line: usize) -> Self {
        let anchor = match *self {
            Self::None => 0,
            Self::Line(l) | Self::Portion { line: l, .. } => l,
            Self::Range { anchor, .. } => anchor,
        };
        Self::range(anchor, line)
    }

    /// Lines touched by the selection, in order.
    pub fn lines(&self) -> Option<RangeInclusive<usize>> {
        match *self {
            Self::None => None,
            Self::Line(l) | Self::Portion { line: l, .. } => Some(l..=l),
            Self::Range { start, end, .. } => Some(start..=end),
        }
    }

    /// Whether `line` is selected as a whole (klogg: `isLineSelected`).
    pub fn is_line_selected(&self, line: usize) -> bool {
        match *self {
            Self::Line(l) => l == line,
            Self::Range { start, end, .. } => (start..=end).contains(&line),
            _ => false,
        }
    }

    /// Selected display columns of `line`, if a portion of it is selected.
    pub fn portion_of(&self, line: usize) -> Option<RangeInclusive<usize>> {
        match *self {
            Self::Portion {
                line: l,
                start,
                end,
            } if l == line => Some(start..=end),
            _ => None,
        }
    }

    /// Drops the parts beyond `last_line` (klogg: `crop`), e.g. after the
    /// file was truncated.
    pub fn cropped(&self, line_count: usize) -> Self {
        let Some(last) = line_count.checked_sub(1) else {
            return Self::None;
        };
        match *self {
            Self::Line(l) | Self::Portion { line: l, .. } if l > last => Self::None,
            Self::Range { start, .. } if start > last => Self::None,
            Self::Range { start, end, anchor } => Self::Range {
                start,
                end: end.min(last),
                anchor: anchor.min(last),
            },
            ref s => s.clone(),
        }
    }

    /// Selected text (klogg: `getSelectedText`). Whole lines are copied raw,
    /// portions from the expanded line. Lines are joined with `\n`; with
    /// `line_numbers`, each is prefixed by its 1-based number.
    pub fn text(&self, log: &LogFile, line_numbers: bool) -> String {
        let prefix = |n: usize| {
            if line_numbers {
                format!("{}: ", n + 1)
            } else {
                String::new()
            }
        };
        match *self {
            Self::None => String::new(),
            Self::Portion { line, start, end } => {
                let expanded = text::expand(&log.line_text(line).unwrap_or_default());
                prefix(line) + &text::slice_columns(&expanded, start..end + 1)
            }
            Self::Line(_) | Self::Range { .. } => {
                let lines = self.lines().expect("not empty");
                let first = *lines.start();
                let mut out = String::new();
                for (i, line) in log.lines_text(first..lines.end() + 1).enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    out += &prefix(first + i);
                    out += &line;
                }
                out
            }
        }
    }
}

/// Mouse-drag selection (klogg: `selectionStarted_`, `selectionStartPos_`,
/// `selectionCurrentEndPos_` in `AbstractLogView`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drag {
    start: Position,
    current: Position,
}

impl Drag {
    /// Mouse press: selects the clicked line and starts a drag.
    pub fn start(at: Position) -> (Self, Selection) {
        (
            Self {
                start: at,
                current: at,
            },
            Selection::Line(at.line),
        )
    }

    /// Mouse move to `to`. Returns the new selection if it changed:
    /// - on another line than the start: a range of lines;
    /// - on the start line, moving horizontally: a portion;
    /// - back on the start line at the start column: that line.
    pub fn update(&mut self, to: Position) -> Option<Selection> {
        if to == self.current {
            return None;
        }
        let selection = if to.line != self.start.line {
            if to.line == self.current.line {
                // Only the column changed on another line: keep the range.
                self.current = to;
                return None;
            }
            Selection::range(self.start.line, to.line)
        } else if to.col != self.current.col {
            Selection::portion(to.line, self.start.col, to.col)
        } else {
            Selection::Line(to.line)
        };
        self.current = to;
        Some(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> Position {
        Position { line, col }
    }

    #[test]
    fn range_is_sorted_and_keeps_anchor() {
        assert_eq!(
            Selection::range(5, 2),
            Selection::Range {
                start: 2,
                end: 5,
                anchor: 5
            }
        );
        assert_eq!(
            Selection::portion(1, 9, 3),
            Selection::Portion {
                line: 1,
                start: 3,
                end: 9
            }
        );
    }

    #[test]
    fn shift_click_extends_from_previous() {
        assert_eq!(Selection::None.extended_to(4), Selection::range(0, 4));
        assert_eq!(Selection::Line(7).extended_to(4), Selection::range(7, 4));
        assert_eq!(
            Selection::range(7, 9).extended_to(2),
            Selection::range(7, 2)
        );
        assert_eq!(
            Selection::range(9, 7).extended_to(12),
            Selection::range(9, 12)
        );
        assert_eq!(
            Selection::portion(3, 1, 2).extended_to(1),
            Selection::range(3, 1)
        );
    }

    #[test]
    fn queries() {
        let r = Selection::range(2, 4);
        assert!(r.is_line_selected(3) && !r.is_line_selected(5));
        assert_eq!(r.lines(), Some(2..=4));
        let p = Selection::portion(3, 4, 2);
        assert!(!p.is_line_selected(3));
        assert_eq!(p.portion_of(3), Some(2..=4));
        assert_eq!(p.portion_of(2), None);
        assert_eq!(Selection::None.lines(), None);
    }

    #[test]
    fn crop() {
        assert_eq!(Selection::Line(5).cropped(5), Selection::None);
        assert_eq!(Selection::Line(4).cropped(5), Selection::Line(4));
        assert_eq!(
            Selection::range(8, 2).cropped(5),
            Selection::Range {
                start: 2,
                end: 4,
                anchor: 4
            }
        );
        assert_eq!(Selection::range(7, 6).cropped(5), Selection::None);
        assert_eq!(Selection::Line(0).cropped(0), Selection::None);
    }

    #[test]
    fn drag_rules() {
        let (mut d, s) = Drag::start(pos(3, 5));
        assert_eq!(s, Selection::Line(3));
        assert_eq!(d.update(pos(3, 5)), None);
        assert_eq!(d.update(pos(3, 9)), Some(Selection::portion(3, 5, 9)));
        assert_eq!(d.update(pos(3, 1)), Some(Selection::portion(3, 1, 5)));
        assert_eq!(d.update(pos(6, 1)), Some(Selection::range(3, 6)));
        assert_eq!(d.update(pos(6, 8)), None);
        assert_eq!(d.update(pos(1, 8)), Some(Selection::range(3, 1)));
        // Back on the start line: a portion again.
        assert_eq!(d.update(pos(3, 2)), Some(Selection::portion(3, 5, 2)));
    }

    mod text {
        use super::*;
        use std::io::Write;

        fn log(content: &str) -> (LogFile, std::path::PathBuf) {
            let path = std::env::temp_dir().join(format!(
                "rlogg-sel-{}-{}.log",
                std::process::id(),
                content.len()
            ));
            std::fs::File::create(&path)
                .unwrap()
                .write_all(content.as_bytes())
                .unwrap();
            (LogFile::open(&path).unwrap(), path)
        }

        #[test]
        fn selected_text() {
            let (log, path) = log("one\ta\ntwo\r\nthree\n");
            assert_eq!(Selection::None.text(&log, false), "");
            assert_eq!(Selection::Line(1).text(&log, false), "two");
            assert_eq!(
                Selection::range(2, 0).text(&log, false),
                "one\ta\ntwo\nthree"
            );
            assert_eq!(Selection::range(0, 1).text(&log, true), "1: one\ta\n2: two");
            // Portions use display columns: the tab expands to col 3..8.
            assert_eq!(Selection::portion(0, 2, 8).text(&log, false), "e     a");
            assert_eq!(Selection::portion(2, 1, 99).text(&log, true), "3: hree");
            std::fs::remove_file(path).unwrap();
        }
    }
}
