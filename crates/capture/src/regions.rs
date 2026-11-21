//! Which parts of the picture changed: rectangles gathered from the frame
//! source (Desktop Duplication's dirty and move rects, or the synthetic
//! source's own), turned into a mask of encoder blocks. Plain logic, no
//! Windows: tested everywhere.

/// A rectangle in pixels, `left..right` × `top..bottom` (exclusive ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// More rectangles than this and the change counts as the whole picture: a
/// bound for a caller that does not take the changes for a while.
const MAX_RECTS: usize = 4096;

/// What changed since the changes were last taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// All of it (a new duplication, no metadata, too many rectangles...).
    pub whole: bool,
    pub rects: Vec<Rect>,
}

impl Changes {
    pub fn whole() -> Self {
        Changes {
            whole: true,
            rects: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.whole && self.rects.is_empty()
    }

    pub fn set_whole(&mut self) {
        self.whole = true;
        self.rects.clear();
    }

    /// Adds a changed rectangle (empty ones are ignored).
    pub fn add(&mut self, r: Rect) {
        if self.whole || r.right <= r.left || r.bottom <= r.top {
            return;
        }
        if self.rects.len() >= MAX_RECTS {
            self.set_whole();
        } else {
            self.rects.push(r);
        }
    }

    /// Adds everything `other` holds.
    pub fn merge(&mut self, other: &Changes) {
        if other.whole {
            self.set_whole();
        } else {
            for &r in &other.rects {
                self.add(r);
            }
        }
    }

    /// Marks in `out` (`cols × rows` blocks of `block` pixels over a
    /// `width × height` picture, raster order) the blocks any change touches;
    /// the rest are cleared. Returns how many are marked.
    pub fn mark_blocks(&self, width: u32, height: u32, block: u32, out: &mut [bool]) -> usize {
        let (cols, rows) = (width.div_ceil(block), height.div_ceil(block));
        assert_eq!(out.len(), (cols * rows) as usize, "mask size");
        if self.whole {
            out.fill(true);
            return out.len();
        }
        out.fill(false);
        let (w, h) = (width as i32, height as i32);
        for r in &self.rects {
            let (l, t) = (r.left.max(0), r.top.max(0));
            let (rr, b) = (r.right.min(w), r.bottom.min(h));
            if rr <= l || b <= t {
                continue;
            }
            let block = block as i32;
            for by in t / block..=(b - 1) / block {
                let row = (by as u32 * cols) as usize;
                for bx in l / block..=(rr - 1) / block {
                    out[row + bx as usize] = true;
                }
            }
        }
        out.iter().filter(|&&m| m).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn mask(c: &Changes, w: u32, h: u32, block: u32) -> (Vec<bool>, usize) {
        let mut m = vec![false; (w.div_ceil(block) * h.div_ceil(block)) as usize];
        let n = c.mark_blocks(w, h, block, &mut m);
        (m, n)
    }

    #[test]
    fn nothing_changed_marks_nothing() {
        let (m, n) = mask(&Changes::default(), 100, 70, 32);
        assert_eq!(n, 0);
        assert_eq!(m.len(), 4 * 3);
    }

    #[test]
    fn whole_marks_everything() {
        let (_, n) = mask(&Changes::whole(), 1920, 1080, 32);
        assert_eq!(n, 60 * 34);
    }

    #[test]
    fn a_rect_marks_the_blocks_it_touches() {
        let mut c = Changes::default();
        // Columns 31..33 touch blocks 0 and 1; rows 0..1 — block row 0 only.
        c.add(rect(31, 0, 33, 1));
        let (m, n) = mask(&c, 128, 64, 32);
        assert_eq!(n, 2);
        assert!(m[0] && m[1] && !m[2]);
        // Exactly one block: its own edges are exclusive.
        let mut c = Changes::default();
        c.add(rect(32, 32, 64, 64));
        let (m, n) = mask(&c, 128, 64, 32);
        assert_eq!(n, 1);
        assert!(m[4 + 1]);
    }

    #[test]
    fn rects_past_the_edges_are_clipped() {
        let mut c = Changes::default();
        c.add(rect(-50, -50, 10, 10));
        c.add(rect(97, 65, 500, 500)); // only the last, partial block
        c.add(rect(200, 0, 300, 10)); // wholly outside
        // 100x70: 4x3 blocks, the last column and row partial.
        let (m, n) = mask(&c, 100, 70, 32);
        assert_eq!(n, 2);
        assert!(m[0] && m[4 * 2 + 3]);
    }

    #[test]
    fn empty_rects_are_ignored() {
        let mut c = Changes::default();
        c.add(rect(10, 10, 10, 20));
        c.add(rect(10, 20, 30, 5));
        assert!(c.is_empty());
    }

    #[test]
    fn too_many_rects_become_whole() {
        let mut c = Changes::default();
        for i in 0..=MAX_RECTS as i32 {
            c.add(rect(i, 0, i + 1, 1));
        }
        assert!(c.whole && c.rects.is_empty());
    }

    #[test]
    fn merge_keeps_whole() {
        let mut a = Changes::default();
        a.add(rect(0, 0, 1, 1));
        a.merge(&Changes::whole());
        assert!(a.whole);
        a.add(rect(0, 0, 1, 1));
        assert!(a.rects.is_empty());
        let mut b = Changes::default();
        b.merge(&a);
        assert!(b.whole);
    }
}
