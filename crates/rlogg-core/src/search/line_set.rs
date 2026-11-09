//! A compact, sorted set of line numbers: search results and marks
//! (klogg: `SearchResultArray`, a `roaring::Roaring64Map`).

use roaring::RoaringTreemap;

/// Sorted set of line numbers, stored as a Roaring bitmap: dense runs (e.g. a
/// search that matches most lines) cost about a bit per line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineSet(RoaringTreemap);

impl LineSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.0.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, line: usize) -> bool {
        self.0.contains(line as u64)
    }

    /// Adds `line`; returns whether it was new.
    pub fn insert(&mut self, line: usize) -> bool {
        self.0.insert(line as u64)
    }

    /// Removes `line`; returns whether it was present.
    pub fn remove(&mut self, line: usize) -> bool {
        self.0.remove(line as u64)
    }

    /// Adds `lines`, which must be sorted and all after the current maximum,
    /// as search batches are. Faster than inserting one by one.
    pub fn append_sorted(&mut self, lines: &[usize]) {
        debug_assert!(lines.is_sorted());
        debug_assert!(match (self.max(), lines.first()) {
            (Some(max), Some(&first)) => first > max,
            _ => true,
        });
        if self.0.append(lines.iter().map(|&l| l as u64)).is_err() {
            self.0.extend(lines.iter().map(|&l| l as u64));
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// The `n`-th line of the set (0-based), i.e. row `n` of a filtered view.
    pub fn nth(&self, n: usize) -> Option<usize> {
        self.0.select(n as u64).map(|l| l as usize)
    }

    /// Number of lines in the set that are `< line`: the row of `line` in a
    /// filtered view if it is in the set, otherwise the row it would take.
    pub fn rank(&self, line: usize) -> usize {
        match line.checked_sub(1) {
            Some(prev) => self.0.rank(prev as u64) as usize,
            None => 0,
        }
    }

    /// First line of the set after `line` (klogg: next match / next mark).
    pub fn next_after(&self, line: usize) -> Option<usize> {
        self.nth(self.rank(line + 1))
    }

    /// Last line of the set before `line`.
    pub fn prev_before(&self, line: usize) -> Option<usize> {
        self.rank(line).checked_sub(1).and_then(|n| self.nth(n))
    }

    pub fn min(&self) -> Option<usize> {
        self.0.min().map(|l| l as usize)
    }

    pub fn max(&self) -> Option<usize> {
        self.0.max().map(|l| l as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().map(|l| l as usize)
    }

    /// Lines in either set (klogg: `marks_and_matches_`).
    pub fn union(&self, other: &LineSet) -> LineSet {
        LineSet(&self.0 | &other.0)
    }

    /// Bytes used by the bitmap, roughly.
    pub fn serialized_size(&self) -> usize {
        self.0.serialized_size()
    }
}

impl FromIterator<usize> for LineSet {
    fn from_iter<I: IntoIterator<Item = usize>>(iter: I) -> Self {
        LineSet(iter.into_iter().map(|l| l as u64).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut s = LineSet::new();
        assert!(s.is_empty());
        assert!(s.insert(5));
        assert!(!s.insert(5));
        s.insert(2);
        s.insert(9);
        assert_eq!(s.len(), 3);
        assert!(s.contains(2) && !s.contains(3));
        assert_eq!(s.iter().collect::<Vec<_>>(), [2, 5, 9]);
        assert_eq!((s.min(), s.max()), (Some(2), Some(9)));
        assert!(s.remove(5));
        assert_eq!(s.iter().collect::<Vec<_>>(), [2, 9]);
    }

    #[test]
    fn nth_and_rank() {
        let s: LineSet = [3, 10, 11, 500].into_iter().collect();
        assert_eq!(s.nth(0), Some(3));
        assert_eq!(s.nth(3), Some(500));
        assert_eq!(s.nth(4), None);
        assert_eq!(s.rank(0), 0);
        assert_eq!(s.rank(3), 0);
        assert_eq!(s.rank(4), 1);
        assert_eq!(s.rank(11), 2);
        assert_eq!(s.rank(10_000), 4);
    }

    #[test]
    fn neighbours() {
        let s: LineSet = [3, 10, 11].into_iter().collect();
        assert_eq!(s.next_after(0), Some(3));
        assert_eq!(s.next_after(3), Some(10));
        assert_eq!(s.next_after(10), Some(11));
        assert_eq!(s.next_after(11), None);
        assert_eq!(s.prev_before(11), Some(10));
        assert_eq!(s.prev_before(4), Some(3));
        assert_eq!(s.prev_before(3), None);
        assert_eq!(s.prev_before(0), None);
    }

    #[test]
    fn append_sorted() {
        let mut s = LineSet::new();
        s.append_sorted(&[1, 4, 7]);
        s.append_sorted(&[]);
        s.append_sorted(&[8, 1 << 40]);
        assert_eq!(s.iter().collect::<Vec<_>>(), [1, 4, 7, 8, 1 << 40]);
    }

    #[test]
    fn union() {
        let a: LineSet = [1, 5].into_iter().collect();
        let b: LineSet = [2, 5].into_iter().collect();
        assert_eq!(a.union(&b).iter().collect::<Vec<_>>(), [1, 2, 5]);
    }

    #[test]
    fn dense_sets_are_compact() {
        let s: LineSet = (0..1_000_000).collect();
        // 8 KiB bitmap containers per 65536 lines: ~1 bit per line.
        assert!(s.serialized_size() < 1_000_000 * 12 / 80);
    }
}
