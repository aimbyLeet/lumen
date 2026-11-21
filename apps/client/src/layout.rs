//! Where the picture goes in the window.
//!
//! The swapchain buffer has the window's aspect and the picture's scale: the
//! picture sits in it 1:1 at `origin`, black around it, and the swapchain
//! (`DXGI_SCALING_STRETCH`) stretches the buffer over the client area. When
//! the buffer is the client area's size, nothing is scaled: exact 1:1.

/// The swapchain buffer and the picture in it, in buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub picture: (u32, u32),
    pub buffer: (u32, u32),
    /// The picture's top-left corner in the buffer.
    pub origin: (u32, u32),
    /// The client area the buffer is stretched over.
    pub client: (u32, u32),
}

/// The picture in a `client` area: as large as fits, up or down, keeping its
/// aspect (owner's decision 2026-10-24, framed and borderless alike). Exactly
/// 1:1 only when the area is the picture's size.
pub fn layout(picture: (u32, u32), client: (u32, u32)) -> Layout {
    let (pw, ph) = (picture.0.max(1), picture.1.max(1));
    let (cw, ch) = (client.0.max(1), client.1.max(1));
    let buffer = if u64::from(cw) * u64::from(ph) >= u64::from(ch) * u64::from(pw) {
        // The area is wider than the picture: bars left and right.
        (
            div_round(u64::from(cw) * u64::from(ph), u64::from(ch)).max(pw),
            ph,
        )
    } else {
        (
            pw,
            div_round(u64::from(ch) * u64::from(pw), u64::from(cw)).max(ph),
        )
    };
    Layout {
        picture: (pw, ph),
        buffer,
        origin: ((buffer.0 - pw) / 2, (buffer.1 - ph) / 2),
        client: (cw, ch),
    }
}

fn div_round(a: u64, b: u64) -> u32 {
    ((2 * a + b) / (2 * b)) as u32
}

impl Layout {
    /// No scaling: one client pixel per buffer pixel.
    pub fn is_one_to_one(&self) -> bool {
        self.buffer == self.client
    }

    /// The buffer is exactly the picture (no bars).
    pub fn is_picture_only(&self) -> bool {
        self.buffer == self.picture
    }

    /// Screen pixels per picture pixel (the same both ways: the aspect holds).
    pub fn picture_scale(&self) -> f32 {
        self.client.0 as f32 / self.buffer.0 as f32
    }

    /// Where the picture is in the client area: left, top, width, height.
    pub fn picture_rect(&self) -> (i32, i32, i32, i32) {
        let x = |v: u32| {
            div_round(
                u64::from(v) * u64::from(self.client.0),
                u64::from(self.buffer.0),
            ) as i32
        };
        let y = |v: u32| {
            div_round(
                u64::from(v) * u64::from(self.client.1),
                u64::from(self.buffer.1),
            ) as i32
        };
        let (left, top) = (x(self.origin.0), y(self.origin.1));
        (
            left,
            top,
            x(self.origin.0 + self.picture.0) - left,
            y(self.origin.1 + self.picture.1) - top,
        )
    }

    /// A picture point → where it is in the client area (fractional).
    pub fn picture_to_client_f(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (self.origin.0 as f32 + x) * self.client.0 as f32 / self.buffer.0 as f32,
            (self.origin.1 as f32 + y) * self.client.1 as f32 / self.buffer.1 as f32,
        )
    }

    /// A picture pixel → the client pixel it is drawn at.
    pub fn picture_to_client(&self, x: i32, y: i32) -> (i32, i32) {
        let (fx, fy) = self.picture_to_client_f(x as f32, y as f32);
        (fx.floor() as i32, fy.floor() as i32)
    }

    /// A client-area pixel → the picture pixel under its centre. Outside the
    /// picture (the bars, a drag beyond the window) it stays outside.
    pub fn to_picture(self, x: i32, y: i32) -> (i32, i32) {
        let map = |v: i32, buffer: u32, client: u32, origin: u32| -> i32 {
            let b = (2 * i64::from(v) + 1) * i64::from(buffer);
            (b.div_euclid(2 * i64::from(client)) - i64::from(origin)) as i32
        };
        (
            map(x, self.buffer.0, self.client.0, self.origin.0),
            map(y, self.buffer.1, self.client.1, self.origin.1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_is_one_to_one_without_bars() {
        let l = layout((3840, 2160), (3840, 2160));
        assert_eq!((l.buffer, l.origin), ((3840, 2160), (0, 0)));
        assert!(l.is_one_to_one() && l.is_picture_only());
        assert_eq!(l.to_picture(123, 456), (123, 456));
    }

    #[test]
    fn a_window_of_the_same_aspect_scales_without_bars() {
        let l = layout((3840, 2160), (1920, 1080));
        assert_eq!((l.buffer, l.origin), ((3840, 2160), (0, 0)));
        assert!(!l.is_one_to_one());
        // 2:1 down: client pixel 0 covers picture 0..2, its centre is picture 1.
        assert_eq!(l.to_picture(0, 0), (1, 1));
        assert_eq!(l.to_picture(1919, 1079), (3839, 2159));
    }

    #[test]
    fn a_wider_window_gets_bars_left_and_right() {
        // 16:9 picture in a 2:1 area.
        let l = layout((1920, 1080), (2000, 1000));
        assert_eq!(l.buffer, (2160, 1080));
        assert_eq!(l.origin, (120, 0));
        // The left bar is left of the picture.
        assert!(l.to_picture(0, 500).0 < 0);
        // The centre stays the centre.
        assert_eq!(l.to_picture(1000, 500), (960, 540));
    }

    #[test]
    fn a_taller_window_gets_bars_top_and_bottom() {
        let l = layout((1920, 1080), (1920, 1440));
        assert_eq!((l.buffer, l.origin), ((1920, 1440), (0, 180)));
        assert!(l.is_one_to_one());
        assert_eq!(l.to_picture(10, 180), (10, 0));
        assert_eq!(l.to_picture(10, 179).1, -1);
    }

    #[test]
    fn the_picture_rect_leaves_out_the_bars() {
        let l = layout((1920, 1080), (2000, 1000));
        // Buffer 2160×1080 over 2000×1000: the picture is 1777.8 wide.
        assert_eq!(l.picture_rect(), (111, 0, 1778, 1000));
        assert!((l.picture_scale() - 1000.0 / 1080.0).abs() < 1e-6);
        let one = layout((1920, 1080), (1920, 1440));
        assert_eq!(one.picture_rect(), (0, 180, 1920, 1080));
        assert_eq!(one.picture_scale(), 1.0);
    }

    #[test]
    fn picture_points_come_back_to_the_client() {
        let l = layout((1920, 1080), (2000, 1000));
        let (x, y) = l.picture_to_client(960, 540);
        assert!((x - 1000).abs() <= 1 && (y - 500).abs() <= 1, "{x},{y}");
        // Round trip: client → picture → client, within a pixel of scale.
        for (cx, cy) in [(200, 100), (1500, 900)] {
            let (px, py) = l.to_picture(cx, cy);
            let (bx, by) = l.picture_to_client(px, py);
            assert!((bx - cx).abs() <= 1 && (by - cy).abs() <= 1);
        }
    }

    #[test]
    fn a_larger_window_scales_up() {
        let l = layout((1920, 1080), (3840, 2160));
        assert_eq!(l.buffer, (1920, 1080));
        assert_eq!(l.to_picture(3839, 2159), (1919, 1079));
    }

    #[test]
    fn a_smaller_picture_of_another_aspect_fills_the_height() {
        // 16:10 host on a 16:9 monitor: stretched up, bars left and right.
        let l = layout((1920, 1200), (3840, 2160));
        assert_eq!((l.buffer, l.origin), ((2133, 1200), (106, 0)));
        assert_eq!(l.to_picture(1920, 1080), (960, 600));
        // A larger one is scaled down the same way.
        assert_eq!(layout((3840, 2160), (1920, 1200)).buffer, (3840, 2400));
    }
}
