//! Rows of the filtered view: search matches and marks (klogg:
//! `LogFilteredData`, `Marks`).

use super::LineSet;

/// Which lines the filtered view shows (klogg: `LogFilteredData::Visibility`,
/// in the order of klogg's visibility box and its 1/2/3 shortcuts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    MarksAndMatches,
    Marks,
    Matches,
}

impl Visibility {
    pub const ALL: [Self; 3] = [Self::MarksAndMatches, Self::Marks, Self::Matches];

    pub fn label(self) -> &'static str {
        match self {
            Self::MarksAndMatches => "Marks and matches",
            Self::Marks => "Marks",
            Self::Matches => "Matches",
        }
    }

    /// The next entry, wrapping (klogg: `v`).
    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % 3]
    }

    /// The previous entry, wrapping (klogg: `shift-v`).
    pub fn prev(self) -> Self {
        Self::ALL[(self as usize + 2) % 3]
    }

    fn shows_matches(self) -> bool {
        self != Self::Marks
    }
}

/// How a line is drawn in the bullet margin (klogg: `LineTypeFlags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineKind {
    pub matched: bool,
    pub marked: bool,
}

/// Search matches and marks of one file, and the rows they make in the
/// filtered view.
#[derive(Debug, Clone, Default)]
pub struct FilteredLines {
    matches: LineSet,
    marks: LineSet,
    /// `matches ∪ marks`, kept up to date (klogg: `marks_and_matches_`).
    both: LineSet,
    visibility: Visibility,
}

impl FilteredLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    pub fn set_visibility(&mut self, visibility: Visibility) {
        self.visibility = visibility;
    }

    pub fn matches(&self) -> &LineSet {
        &self.matches
    }

    pub fn marks(&self) -> &LineSet {
        &self.marks
    }

    /// The lines shown, one per row, with the current visibility.
    pub fn rows(&self) -> &LineSet {
        match self.visibility {
            Visibility::MarksAndMatches => &self.both,
            Visibility::Marks => &self.marks,
            Visibility::Matches => &self.matches,
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows().len()
    }

    /// The file line shown in `row` (klogg: `getMatchingLineNumber`).
    pub fn line_at(&self, row: usize) -> Option<usize> {
        self.rows().nth(row)
    }

    /// The row showing `line`, or else the row of the closest line before
    /// it, or 0 (klogg: `getLineIndexNumber`).
    pub fn row_of(&self, line: usize) -> usize {
        self.rows().rank(line + 1).saturating_sub(1)
    }

    pub fn kind(&self, line: usize) -> LineKind {
        LineKind {
            matched: self.matches.contains(line),
            marked: self.marks.contains(line),
        }
    }

    /// Drops the matches, keeping the marks (klogg: `clearSearch`). A new
    /// search in "Marks" visibility switches back to show its matches.
    pub fn clear_matches(&mut self) {
        self.matches.clear();
        self.both = self.marks.clone();
        if !self.visibility.shows_matches() {
            self.visibility = Visibility::MarksAndMatches;
        }
    }

    /// Adds a batch of matches: sorted, and after those already added.
    pub fn add_matches(&mut self, lines: &[usize]) {
        self.matches.append_sorted(lines);
        self.both.extend(lines.iter().copied());
    }

    /// Marks `lines`, or unmarks them if they are all marked already (klogg:
    /// `markLinesFromMain`). Lines `>= line_count` are ignored.
    pub fn toggle_marks(&mut self, lines: impl IntoIterator<Item = usize>, line_count: usize) {
        let lines: Vec<usize> = lines.into_iter().filter(|&l| l < line_count).collect();
        if lines.iter().all(|&l| self.marks.contains(l)) {
            for l in lines {
                self.marks.remove(l);
                if !self.matches.contains(l) {
                    self.both.remove(l);
                }
            }
        } else {
            for l in lines {
                self.marks.insert(l);
                self.both.insert(l);
            }
        }
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
        self.both = self.matches.clone();
    }

    /// First mark after `line` (klogg: `getMarkAfter`, `]`).
    pub fn next_mark(&self, line: usize) -> Option<usize> {
        self.marks.next_after(line)
    }

    /// Last mark before `line` (klogg: `getMarkBefore`, `[`).
    pub fn prev_mark(&self, line: usize) -> Option<usize> {
        self.marks.prev_before(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(f: &FilteredLines) -> Vec<usize> {
        f.rows().iter().collect()
    }

    #[test]
    fn visibility_cycles() {
        use Visibility::*;
        assert_eq!(MarksAndMatches.next(), Marks);
        assert_eq!(Matches.next(), MarksAndMatches);
        assert_eq!(MarksAndMatches.prev(), Matches);
        assert_eq!(Marks.prev(), MarksAndMatches);
    }

    #[test]
    fn rows_follow_visibility() {
        let mut f = FilteredLines::new();
        f.add_matches(&[2, 5]);
        f.add_matches(&[9]);
        f.toggle_marks([5, 7], 100);
        assert_eq!(rows(&f), [2, 5, 7, 9]);
        f.set_visibility(Visibility::Marks);
        assert_eq!(rows(&f), [5, 7]);
        f.set_visibility(Visibility::Matches);
        assert_eq!(rows(&f), [2, 5, 9]);
        assert_eq!(f.row_count(), 3);
        assert_eq!(f.line_at(1), Some(5));
        assert_eq!(f.line_at(3), None);
    }

    #[test]
    fn row_of_finds_nearest_row_before() {
        let mut f = FilteredLines::new();
        f.add_matches(&[10, 20, 30]);
        assert_eq!(f.row_of(20), 1);
        assert_eq!(f.row_of(25), 1);
        assert_eq!(f.row_of(5), 0);
        assert_eq!(f.row_of(1000), 2);
    }

    #[test]
    fn kinds() {
        let mut f = FilteredLines::new();
        f.add_matches(&[1, 2]);
        f.toggle_marks([2, 3], 10);
        assert_eq!(f.kind(0), LineKind::default());
        assert_eq!(
            f.kind(1),
            LineKind {
                matched: true,
                marked: false
            }
        );
        assert_eq!(
            f.kind(2),
            LineKind {
                matched: true,
                marked: true
            }
        );
        assert_eq!(
            f.kind(3),
            LineKind {
                matched: false,
                marked: true
            }
        );
    }

    #[test]
    fn toggle_marks_like_klogg() {
        let mut f = FilteredLines::new();
        f.add_matches(&[4]);
        // Some unmarked: mark them all.
        f.toggle_marks([3, 4], 10);
        f.toggle_marks([4, 5], 10);
        assert_eq!(f.marks().iter().collect::<Vec<_>>(), [3, 4, 5]);
        // All marked: unmark them all. A matched line stays visible.
        f.toggle_marks([3, 4], 10);
        assert_eq!(f.marks().iter().collect::<Vec<_>>(), [5]);
        assert_eq!(rows(&f), [4, 5]);
        // Out of range lines are ignored.
        f.toggle_marks([10, 11], 10);
        assert_eq!(f.marks().len(), 1);
    }

    #[test]
    fn clear_matches_keeps_marks() {
        let mut f = FilteredLines::new();
        f.add_matches(&[1, 2]);
        f.toggle_marks([7], 10);
        f.set_visibility(Visibility::Marks);
        f.clear_matches();
        assert!(f.matches().is_empty());
        assert_eq!(f.visibility(), Visibility::MarksAndMatches);
        assert_eq!(rows(&f), [7]);
        f.add_matches(&[3]);
        assert_eq!(rows(&f), [3, 7]);
        f.clear_marks();
        assert_eq!(rows(&f), [3]);
    }

    #[test]
    fn mark_navigation() {
        let mut f = FilteredLines::new();
        f.toggle_marks([3, 8], 10);
        assert_eq!(f.next_mark(0), Some(3));
        assert_eq!(f.next_mark(3), Some(8));
        assert_eq!(f.next_mark(8), None);
        assert_eq!(f.prev_mark(8), Some(3));
        assert_eq!(f.prev_mark(3), None);
    }
}
