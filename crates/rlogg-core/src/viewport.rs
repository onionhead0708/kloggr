//! Scroll state of a log view, independent of pixels.
//!
//! Vertical position is a fractional line number rather than a pixel offset:
//! with tens of millions of lines a pixel offset would exceed what `f32`
//! layout can represent exactly. Horizontal position is a whole display
//! column, since text is drawn in a fixed-width grid (as in klogg).

/// Visible area and scroll position, in lines and columns.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Viewport {
    /// First visible line; the fraction is how much of it is scrolled out.
    top: f64,
    /// First visible display column.
    left: usize,
    /// Visible lines (fractional: a partially visible last row counts).
    rows: f64,
    /// Visible columns.
    cols: usize,
    line_count: usize,
    /// Widest line seen, in columns: bounds horizontal scrolling.
    max_cols: usize,
}

impl Viewport {
    pub fn top(&self) -> f64 {
        self.top
    }

    pub fn first_line(&self) -> usize {
        self.top as usize
    }

    /// Lines with at least one visible pixel.
    pub fn visible_lines(&self) -> std::ops::Range<usize> {
        let first = self.first_line();
        let last = (self.top + self.rows).ceil() as usize;
        first..last.min(self.line_count)
    }

    pub fn left(&self) -> usize {
        self.left
    }

    pub fn rows(&self) -> f64 {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn max_cols(&self) -> usize {
        self.max_cols
    }

    /// Updates the size of the content and of the visible area.
    pub fn set_size(&mut self, line_count: usize, rows: f64, cols: usize) {
        self.line_count = line_count;
        self.rows = rows.max(0.0);
        self.cols = cols;
        self.clamp();
    }

    /// Raises the horizontal scroll limit to `cols` (the widest line known).
    pub fn grow_max_cols(&mut self, cols: usize) {
        self.max_cols = self.max_cols.max(cols);
    }

    /// Largest valid `top`: the last line is at the bottom of the view.
    pub fn max_top(&self) -> f64 {
        (self.line_count as f64 - self.rows.floor()).max(0.0)
    }

    pub fn max_left(&self) -> usize {
        self.max_cols.saturating_sub(self.cols)
    }

    pub fn set_top(&mut self, top: f64) {
        self.top = top;
        self.clamp();
    }

    pub fn set_left(&mut self, left: usize) {
        self.left = left;
        self.clamp();
    }

    pub fn scroll_lines(&mut self, delta: f64) {
        self.set_top(self.top + delta);
    }

    pub fn scroll_cols(&mut self, delta: isize) {
        self.set_left(self.left.saturating_add_signed(delta));
    }

    /// One page: the number of fully visible lines, at least 1.
    pub fn page(&self) -> f64 {
        self.rows.floor().max(1.0)
    }

    /// Brings `line` into view, centering it if it is off screen (klogg:
    /// `displayLine` / `jumpToLine`).
    pub fn reveal_line(&mut self, line: usize) {
        let first = self.top.ceil() as usize;
        let last_full = (self.top + self.rows).floor() as usize;
        if line < first || line >= last_full {
            self.set_top(line as f64 - (self.rows / 2.0).floor());
        }
    }

    /// Scrolls horizontally so that column `col` is visible (klogg shows the
    /// end of a selected portion this way).
    pub fn reveal_col(&mut self, col: usize) {
        if col < self.left {
            self.set_left(col);
        } else if col >= self.left + self.cols {
            self.grow_max_cols(col + 1);
            self.set_left(col + 1 - self.cols);
        }
    }

    fn clamp(&mut self) {
        self.top = self.top.clamp(0.0, self.max_top());
        self.left = self.left.min(self.max_left());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(lines: usize, rows: f64) -> Viewport {
        let mut v = Viewport::default();
        v.set_size(lines, rows, 80);
        v
    }

    #[test]
    fn scrolling_is_clamped() {
        let mut v = vp(100, 10.5);
        v.scroll_lines(-5.0);
        assert_eq!(v.top(), 0.0);
        v.scroll_lines(1000.0);
        assert_eq!(v.top(), 90.0);
        assert_eq!(v.visible_lines(), 90..100);
        v.scroll_lines(-0.25);
        assert_eq!(v.visible_lines(), 89..100);
        // Fewer lines than rows: no scrolling.
        let mut v = vp(3, 10.0);
        v.scroll_lines(5.0);
        assert_eq!((v.top(), v.visible_lines()), (0.0, 0..3));
    }

    #[test]
    fn shrinking_content_clamps_position() {
        let mut v = vp(1000, 10.0);
        v.set_top(900.0);
        v.set_size(50, 10.0, 80);
        assert_eq!(v.top(), 40.0);
    }

    #[test]
    fn horizontal_scroll_bounded_by_widest_line() {
        let mut v = vp(10, 5.0);
        v.scroll_cols(10);
        assert_eq!(v.left(), 0);
        v.grow_max_cols(100);
        v.scroll_cols(500);
        assert_eq!(v.left(), 20);
        v.scroll_cols(-7);
        assert_eq!(v.left(), 13);
        v.reveal_col(5);
        assert_eq!(v.left(), 5);
        v.reveal_col(200);
        assert_eq!(v.left(), 121);
    }

    #[test]
    fn reveal_centers_off_screen_lines() {
        let mut v = vp(1000, 20.0);
        v.reveal_line(10);
        assert_eq!(v.top(), 0.0);
        v.reveal_line(20);
        assert_eq!(v.top(), 10.0);
        v.reveal_line(15);
        assert_eq!(v.top(), 10.0);
        v.reveal_line(999);
        assert_eq!(v.top(), 980.0);
    }
}
