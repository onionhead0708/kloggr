//! Coloured ranges of a line, where a range added later is drawn over the
//! ones before it.
//!
//! Ports klogg's `HighlightedMatchRanges` (`src/ui/include/highlightedmatch.h`,
//! GPLv3, Copyright (C) 2020 Anton Filimonov and other contributors).

use std::ops::Range;

use super::Color;

/// Colours of a highlight. `None` keeps the colour underneath (klogg: an
/// invalid `QColor`, as colour label 9's foreground).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colors {
    pub fore: Option<Color>,
    pub back: Option<Color>,
}

/// A coloured range of a line (klogg: `HighlightedMatch`). The unit of the
/// range (bytes or display columns) is up to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub colors: Colors,
}

/// Sorted, non-overlapping spans (klogg: `HighlightedMatchRanges`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spans(Vec<Span>);

impl Spans {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[Span] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<Span> {
        self.0
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Adds `span` over the existing ones: the parts of older spans it
    /// covers are dropped (klogg: `addMatch`). Empty spans are ignored.
    pub fn add(&mut self, span: Span) {
        let new = span.range.clone();
        if new.is_empty() {
            return;
        }
        let mut out = Vec::with_capacity(self.0.len() + 2);
        for old in self.0.drain(..) {
            if old.range.end <= new.start || new.end <= old.range.start {
                out.push(old);
                continue;
            }
            if old.range.start < new.start {
                out.push(Span {
                    range: old.range.start..new.start,
                    colors: old.colors,
                });
            }
            if new.end < old.range.end {
                out.push(Span {
                    range: new.end..old.range.end,
                    colors: old.colors,
                });
            }
        }
        let at = out.partition_point(|s| s.range.start < new.start);
        out.insert(at, span);
        self.0 = out;
    }

    pub fn extend(&mut self, spans: impl IntoIterator<Item = Span>) {
        for span in spans {
            self.add(span);
        }
    }

    /// The ranges, to map them to another unit (e.g. bytes to columns).
    pub fn ranges(&self) -> Vec<Range<usize>> {
        self.0.iter().map(|s| s.range.clone()).collect()
    }

    /// Replaces the ranges, in order, keeping the colours. `ranges` must
    /// have one entry per span.
    pub fn with_ranges(self, ranges: Vec<Range<usize>>) -> Self {
        debug_assert_eq!(ranges.len(), self.0.len());
        Self(
            self.0
                .into_iter()
                .zip(ranges)
                .map(|(s, range)| Span {
                    range,
                    colors: s.colors,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: u8) -> Colors {
        Colors {
            fore: None,
            back: Some(Color::rgb(n, n, n)),
        }
    }

    fn span(range: Range<usize>, n: u8) -> Span {
        Span {
            range,
            colors: c(n),
        }
    }

    fn spans(list: &[(Range<usize>, u8)]) -> Spans {
        let mut s = Spans::new();
        for (r, n) in list {
            s.add(span(r.clone(), *n));
        }
        s
    }

    #[test]
    fn disjoint_spans_are_sorted() {
        let s = spans(&[(10..12, 1), (0..2, 2), (5..6, 3)]);
        assert_eq!(s.ranges(), [0..2, 5..6, 10..12]);
    }

    #[test]
    fn later_span_wins() {
        // New covers old entirely.
        assert_eq!(spans(&[(2..4, 1), (0..6, 2)]).as_slice(), [span(0..6, 2)]);
        // New inside old: old is split.
        assert_eq!(
            spans(&[(0..10, 1), (3..5, 2)]).as_slice(),
            [span(0..3, 1), span(3..5, 2), span(5..10, 1)]
        );
        // Overlap on the right and on the left.
        assert_eq!(
            spans(&[(0..5, 1), (3..8, 2)]).as_slice(),
            [span(0..3, 1), span(3..8, 2)]
        );
        assert_eq!(
            spans(&[(3..8, 1), (0..5, 2)]).as_slice(),
            [span(0..5, 2), span(5..8, 1)]
        );
        // Spanning several.
        assert_eq!(
            spans(&[(0..2, 1), (3..5, 2), (6..9, 3), (1..7, 4)]).as_slice(),
            [span(0..1, 1), span(1..7, 4), span(7..9, 3)]
        );
        // Same range.
        assert_eq!(spans(&[(1..3, 1), (1..3, 2)]).as_slice(), [span(1..3, 2)]);
    }

    #[test]
    fn empty_spans_are_ignored() {
        assert!(spans(&[(3..3, 1)]).is_empty());
        assert_eq!(spans(&[(0..5, 1), (2..2, 2)]).as_slice(), [span(0..5, 1)]);
    }

    #[test]
    fn with_ranges_keeps_colours() {
        let s = spans(&[(0..2, 1), (4..8, 2)]).with_ranges(vec![0..1, 2..4]);
        assert_eq!(s.as_slice(), [span(0..1, 1), span(2..4, 2)]);
    }
}
