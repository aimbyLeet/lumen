//! The QP delta map for still regions.
//!
//! NVENC bounds the QP after the map's delta by `minQP`, so the session's
//! `minQP` is the QP still blocks may reach (`still_qp`, finer than today's
//! 16), and blocks that changed lately get a delta on top so that moving
//! content is coded as it always was (`moving_qp`). The encoder's rate control
//! picks the base QP itself — at `minQP` while the cap is far, higher on a low
//! cap even when the stream is well under it — so the delta follows it:
//! `moving_qp - base`, between 0 (the base is already that coarse: exactly
//! today's coding) and `moving_qp - still_qp`. Plain logic, no Windows: tested
//! everywhere.

use crate::regions::Changes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QpMapParams {
    /// The session's `minQP`: what still blocks may reach.
    pub still_qp: u8,
    /// What moving blocks are coded at, as long as the cap allows (the
    /// `minQP` of a session without the map).
    pub moving_qp: u8,
    /// A block unchanged for this many frames counts as still.
    pub still_after: u16,
}

impl QpMapParams {
    pub fn max_delta(&self) -> i8 {
        self.moving_qp
            .saturating_sub(self.still_qp)
            .min(i8::MAX as u8) as i8
    }
}

/// The delta is revisited after this many frames (half a second at 60 fps).
pub const ADAPT_FRAMES: u32 = 30;

/// Per-block age (frames since the block last changed) and the map built
/// from it, one value per `block`-pixel block in raster order.
pub struct StaticQpMap {
    width: u32,
    height: u32,
    block: u32,
    params: QpMapParams,
    ages: Vec<u16>,
    changed: Vec<bool>,
    /// The delta of moving blocks now.
    delta: i8,
    /// The last map's mean delta.
    mean_delta: f64,
    /// Base QP estimates since the delta was last revisited.
    base_sum: f64,
    base_frames: u32,
}

/// What [`StaticQpMap::update`] made of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapStats {
    /// Blocks the frame's changes touched.
    pub changed: usize,
    /// Blocks coded as moving (`+delta`).
    pub moving: usize,
    /// Blocks in the map.
    pub blocks: usize,
    /// The delta of moving blocks.
    pub delta: i8,
    /// The map's mean delta: the encoder's average QP over the frame is its
    /// base QP plus this.
    pub mean_delta: f64,
}

impl MapStats {
    /// The share of blocks coded as moving, 0..=1.
    pub fn moving_share(&self) -> f64 {
        self.moving as f64 / self.blocks.max(1) as f64
    }
}

impl StaticQpMap {
    /// Every block starts as moving (the first frame is new throughout), the
    /// delta at its largest.
    pub fn new(width: u32, height: u32, block: u32, params: QpMapParams) -> Self {
        let n = (width.div_ceil(block) * height.div_ceil(block)) as usize;
        StaticQpMap {
            width,
            height,
            block,
            params,
            ages: vec![0; n],
            changed: vec![false; n],
            delta: params.max_delta(),
            mean_delta: 0.0,
            base_sum: 0.0,
            base_frames: 0,
        }
    }

    pub fn params(&self) -> QpMapParams {
        self.params
    }

    /// The delta of moving blocks now.
    pub fn delta(&self) -> i8 {
        self.delta
    }

    /// Blocks in the map (the size the encoder's map must have).
    pub fn len(&self) -> usize {
        self.ages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ages.is_empty()
    }

    /// Takes the changes since the previous frame and writes the next frame's
    /// deltas into `out` (`len()` values).
    pub fn update(&mut self, changes: &Changes, out: &mut [i8]) -> MapStats {
        assert_eq!(out.len(), self.ages.len(), "map size");
        let changed = changes.mark_blocks(self.width, self.height, self.block, &mut self.changed);
        let (delta, still_after) = (self.delta, self.params.still_after);
        let mut moving = 0usize;
        for ((age, &ch), d) in self.ages.iter_mut().zip(&self.changed).zip(out.iter_mut()) {
            *age = if ch { 0 } else { age.saturating_add(1) };
            *d = if *age < still_after {
                moving += 1;
                delta
            } else {
                0
            };
        }
        self.mean_delta = f64::from(delta) * moving as f64 / self.ages.len().max(1) as f64;
        MapStats {
            changed,
            moving,
            blocks: self.ages.len(),
            delta,
            mean_delta: self.mean_delta,
        }
    }

    /// The encoded frame's average QP (for the map `update` made last).
    /// Returns the base QP the encoder chose — what the cap controller should
    /// compare with `still_qp`: above it, more rate lets still blocks get
    /// finer while the delta keeps moving ones at `moving_qp`. Every
    /// [`ADAPT_FRAMES`] the delta is set anew from the base.
    pub fn observe(&mut self, avg_qp: u32) -> f64 {
        let base = (f64::from(avg_qp) - self.mean_delta).max(f64::from(self.params.still_qp));
        self.base_sum += base;
        self.base_frames += 1;
        if self.base_frames >= ADAPT_FRAMES {
            let base = self.base_sum / f64::from(self.base_frames);
            let want = (f64::from(self.params.moving_qp) - base).round();
            self.delta = want.clamp(0.0, f64::from(self.params.max_delta())) as i8;
            self.base_sum = 0.0;
            self.base_frames = 0;
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::Rect;

    const P: QpMapParams = QpMapParams {
        still_qp: 10,
        moving_qp: 16,
        still_after: 30,
    };

    fn block_change(bx: i32, by: i32) -> Changes {
        let mut c = Changes::default();
        c.add(Rect {
            left: bx * 32,
            top: by * 32,
            right: bx * 32 + 32,
            bottom: by * 32 + 32,
        });
        c
    }

    /// 128x64: 4x2 blocks.
    fn map() -> (StaticQpMap, Vec<i8>) {
        let m = StaticQpMap::new(128, 64, 32, P);
        let out = vec![0; m.len()];
        (m, out)
    }

    #[test]
    fn starts_moving_and_goes_still_after_the_threshold() {
        let (mut m, mut out) = map();
        let s = m.update(&Changes::whole(), &mut out);
        assert_eq!((s.changed, s.moving), (8, 8));
        assert!(out.iter().all(|&d| d == 6));
        // Unchanged frames 1..=29: still moving (age < 30).
        for _ in 1..30 {
            m.update(&Changes::default(), &mut out);
            assert!(out.iter().all(|&d| d == 6));
        }
        let s = m.update(&Changes::default(), &mut out);
        assert_eq!(s.moving, 0);
        assert!(out.iter().all(|&d| d == 0));
        assert_eq!(s.mean_delta, 0.0);
    }

    #[test]
    fn a_moving_block_stays_moving_while_the_rest_goes_still() {
        let (mut m, mut out) = map();
        m.update(&Changes::whole(), &mut out);
        let mut s = MapStats::default();
        for _ in 0..40 {
            s = m.update(&block_change(1, 1), &mut out);
        }
        assert_eq!((s.changed, s.moving), (1, 1));
        assert_eq!(out, vec![0, 0, 0, 0, 0, 6, 0, 0]);
        assert_eq!(s.mean_delta, 6.0 / 8.0);
    }

    #[test]
    fn a_block_that_stops_goes_still_after_the_threshold() {
        let (mut m, mut out) = map();
        for _ in 0..40 {
            m.update(&block_change(2, 0), &mut out);
        }
        for i in 1..=30 {
            m.update(&Changes::default(), &mut out);
            assert_eq!(out[2], if i < 30 { 6 } else { 0 }, "frame {i}");
        }
    }

    #[test]
    fn a_whole_change_makes_everything_move_again() {
        let (mut m, mut out) = map();
        for _ in 0..40 {
            m.update(&Changes::default(), &mut out);
        }
        assert!(out.iter().all(|&d| d == 0));
        let s = m.update(&Changes::whole(), &mut out);
        assert_eq!(s.moving, 8);
        assert_eq!(s.mean_delta, 6.0);
    }

    #[test]
    fn a_blinking_caret_spends_most_of_the_time_moving() {
        // A caret blinking every 32 frames: its block is moving 30 frames of
        // 32 (coded as today), still for 2 — only its own block.
        let (mut m, mut out) = map();
        m.update(&Changes::whole(), &mut out);
        let (mut moving, mut frames) = (0, 0);
        for f in 1..=320 {
            let c = if f % 32 == 0 {
                block_change(0, 0)
            } else {
                Changes::default()
            };
            m.update(&c, &mut out);
            if f > 64 {
                frames += 1;
                moving += usize::from(out[0] == 6);
                assert!(out[1..].iter().all(|&d| d == 0));
            }
        }
        assert_eq!((moving, frames), (240, 256));
    }

    #[test]
    fn ages_do_not_overflow() {
        let (mut m, mut out) = map();
        for _ in 0..70_000 {
            m.update(&Changes::default(), &mut out);
        }
        assert!(out.iter().all(|&d| d == 0));
    }

    /// Feeds `frames` frames whose base QP is `base` (moving blocks at base +
    /// the delta the map gave them).
    fn run(m: &mut StaticQpMap, out: &mut [i8], c: &Changes, base: f64, frames: u32) -> MapStats {
        let mut s = MapStats::default();
        for _ in 0..frames {
            s = m.update(c, out);
            m.observe((base + s.mean_delta).round() as u32);
        }
        s
    }

    #[test]
    fn the_delta_stays_largest_while_the_base_is_at_the_floor() {
        let (mut m, mut out) = map();
        let s = run(&mut m, &mut out, &block_change(0, 0), 10.0, 120);
        assert_eq!(s.delta, 6);
        assert_eq!(out[0], 6);
    }

    #[test]
    fn the_delta_follows_a_coarser_base() {
        // A low cap: the encoder holds the base at 13 — moving blocks get +3,
        // 16 as without the map.
        let (mut m, mut out) = map();
        let s = run(&mut m, &mut out, &Changes::whole(), 13.0, 60);
        assert_eq!(s.delta, 3);
        assert_eq!(m.observe(16), 13.0);
        // The cap presses: the base is already above 16 — no delta at all.
        let s = run(&mut m, &mut out, &Changes::whole(), 24.0, 60);
        assert_eq!((s.delta, s.mean_delta), (0, 0.0));
        assert!(out.iter().all(|&d| d == 0));
        // And back.
        let s = run(&mut m, &mut out, &Changes::whole(), 10.0, 60);
        assert_eq!(s.delta, 6);
    }

    #[test]
    fn the_delta_waits_for_a_whole_window() {
        let (mut m, mut out) = map();
        run(&mut m, &mut out, &Changes::whole(), 20.0, ADAPT_FRAMES - 1);
        assert_eq!(m.delta(), 6);
        run(&mut m, &mut out, &Changes::whole(), 20.0, 1);
        assert_eq!(m.delta(), 0);
    }
}
