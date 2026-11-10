//! The overview bar beside the main view: where matches and marks are in
//! the file (klogg: `Overview`, `OverviewWidget`, GPLv3).
//!
//! The bar is `height` pixel rows tall; file line `l` maps to row
//! `l * height / line_count`. Each row with matches gets a red tick, each row
//! with marks that are not matches a blue one. A tick is darker the more
//! lines fall in its row, in [`WEIGHT_STEPS`] steps.

use super::FilteredLines;

/// klogg: `Overview::WeightedLine::WEIGHT_STEPS`.
pub const WEIGHT_STEPS: usize = 3;

/// A tick on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    /// Pixel row, from the top of the bar.
    pub y: usize,
    /// `0..WEIGHT_STEPS`: one line in the row, two, three or more.
    pub weight: usize,
}

impl Tick {
    /// klogg's opacity for the tick: `(weight + 1) / WEIGHT_STEPS`.
    pub fn opacity(&self) -> f32 {
        (self.weight + 1) as f32 / WEIGHT_STEPS as f32
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    pub matches: Vec<Tick>,
    /// Marked lines that are not matches (klogg draws those in blue).
    pub marks: Vec<Tick>,
}

impl Overview {
    /// Ticks for a bar `height` rows tall over `line_count` lines (klogg:
    /// `recalculatesLines`).
    pub fn new(filter: &FilteredLines, line_count: usize, height: usize) -> Self {
        let ticks = |counts: Vec<(usize, usize)>| {
            counts
                .into_iter()
                .map(|(y, n)| Tick { y, weight: n - 1 })
                .collect()
        };
        let matches = filter.matches();
        Self {
            matches: ticks(matches.bucket_counts(line_count, height, WEIGHT_STEPS, |_| false)),
            marks: ticks(
                filter
                    .marks()
                    .bucket_counts(line_count, height, WEIGHT_STEPS, |l| matches.contains(l)),
            ),
        }
    }
}

/// The row of `line` (klogg: `yFromFileLine`).
pub fn y_of_line(line: usize, line_count: usize, height: usize) -> usize {
    if line_count == 0 {
        return 0;
    }
    (line as u128 * height as u128 / line_count as u128) as usize
}

/// The line at row `y` (klogg: `fileLineFromY`), clamped to the file.
pub fn line_at_y(y: usize, line_count: usize, height: usize) -> usize {
    if height == 0 || line_count == 0 {
        return 0;
    }
    let line = (y as u128 * line_count as u128 / height as u128) as usize;
    line.min(line_count - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(y: usize, weight: usize) -> Tick {
        Tick { y, weight }
    }

    #[test]
    fn ticks_and_weights() {
        let mut f = FilteredLines::new();
        // 1000 lines on 100 rows: 10 lines per row.
        f.add_matches(&[0, 5, 15, 16, 17, 18, 999]);
        f.toggle_marks([18, 40, 41], 1000);
        let o = Overview::new(&f, 1000, 100);
        assert_eq!(o.matches, [tick(0, 1), tick(1, 2), tick(99, 0)]);
        // Line 18 is matched: drawn as a match only.
        assert_eq!(o.marks, [tick(4, 1)]);
        assert!((o.matches[1].opacity() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn more_rows_than_lines() {
        let mut f = FilteredLines::new();
        f.add_matches(&[0, 1, 2]);
        let o = Overview::new(&f, 3, 300);
        assert_eq!(o.matches, [tick(0, 0), tick(100, 0), tick(200, 0)]);
    }

    #[test]
    fn empty() {
        let f = FilteredLines::new();
        assert_eq!(Overview::new(&f, 0, 100), Overview::default());
        assert_eq!(Overview::new(&f, 10, 0), Overview::default());
    }

    #[test]
    fn many_matches_skip_ahead() {
        let mut f = FilteredLines::new();
        let all: Vec<usize> = (0..2_000_000).collect();
        f.add_matches(&all);
        let o = Overview::new(&f, 2_000_000, 500);
        assert_eq!(o.matches.len(), 500);
        assert!(o.matches.iter().all(|t| t.weight == WEIGHT_STEPS - 1));
    }

    #[test]
    fn line_mapping() {
        assert_eq!(y_of_line(500, 1000, 100), 50);
        assert_eq!(line_at_y(50, 1000, 100), 500);
        assert_eq!(line_at_y(100, 1000, 100), 999);
        assert_eq!(line_at_y(3, 0, 100), 0);
        assert_eq!(y_of_line(3, 0, 100), 0);
    }
}
