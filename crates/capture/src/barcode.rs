//! Frame-number barcode: a single row of black/white square cells at the
//! top-left corner of the frame, robust against 4:2:0 + lossy HEVC.
//!
//! Layout (left to right, [`CELL`] pixels per cell):
//!   * 4 start cells: white, black, white, black (lets a reader validate the
//!     position and polarity of the code);
//!   * 32 cells: the frame number, most significant bit first (1 = white);
//!   * 8 cells: CRC-8 of the four big-endian frame-number bytes
//!     (polynomial 0x07, init 0xFF, no reflection, no final XOR), MSB first.
//!
//! A CRC-8 detects every single-cell flip and all burst errors up to 8 cells.

/// Cell edge in pixels (luma). A multiple of 2 so cells align with chroma samples.
pub const CELL: u32 = 16;

const START: [bool; 4] = [true, false, true, false];
const FRAME_BITS: usize = 32;
const CRC_BITS: usize = 8;
/// Total number of cells in the barcode.
pub const CELL_COUNT: usize = START.len() + FRAME_BITS + CRC_BITS;

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// The full cell sequence for `frame` (start cells, 32 data bits, 8 checksum bits). `true` = white.
pub fn cells(frame: u32) -> Vec<bool> {
    let crc = crc8(&frame.to_be_bytes());
    let mut v = Vec::with_capacity(CELL_COUNT);
    v.extend_from_slice(&START);
    v.extend((0..FRAME_BITS).map(|i| (frame >> (FRAME_BITS - 1 - i)) & 1 != 0));
    v.extend((0..CRC_BITS).map(|i| (crc >> (CRC_BITS - 1 - i)) & 1 != 0));
    v
}

/// The cells packed into 32-bit words (cell `i` = bit `i % 32` of word `i / 32`), for shader constants.
pub fn packed_cells(frame: u32) -> [u32; 4] {
    let mut w = [0u32; 4];
    for (i, white) in cells(frame).into_iter().enumerate() {
        if white {
            w[i / 32] |= 1 << (i % 32);
        }
    }
    w
}

/// Width and height in pixels of the area covered at (0, 0).
pub fn region() -> (u32, u32) {
    (CELL_COUNT as u32 * CELL, CELL)
}

/// Reads the frame number back from 10-bit luma codes (full range) of a decoded
/// picture, row-major, `width` pixels per row. Each cell is sampled as the mean
/// of its central 8×8 pixels and thresholded at mid-grey. Returns `None` if the
/// picture is too small, the start cells or the checksum do not match.
pub fn read_from_luma(luma: &[u16], width: usize) -> Option<u32> {
    let cell = CELL as usize;
    if width < CELL_COUNT * cell || luma.len() < width * cell {
        return None;
    }
    let (lo, hi) = (cell / 4, cell / 4 + cell / 2);
    let sample = |i: usize| -> bool {
        let mut sum = 0u32;
        for y in lo..hi {
            for x in (i * cell + lo)..(i * cell + hi) {
                sum += luma[y * width + x] as u32;
            }
        }
        sum / ((hi - lo) * (hi - lo)) as u32 >= 512
    };
    let bits: Vec<bool> = (0..CELL_COUNT).map(sample).collect();
    if bits[..START.len()] != START {
        return None;
    }
    let read =
        |range: std::ops::Range<usize>| bits[range].iter().fold(0u32, |a, &b| (a << 1) | b as u32);
    let frame = read(START.len()..START.len() + FRAME_BITS);
    let crc = read(START.len() + FRAME_BITS..CELL_COUNT) as u8;
    (crc == crc8(&frame.to_be_bytes())).then_some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(frame: u32, flip: Option<usize>, width: usize) -> Vec<u16> {
        let mut c = cells(frame);
        if let Some(i) = flip {
            c[i] = !c[i];
        }
        let cell = CELL as usize;
        let mut img = vec![512u16; width * cell];
        for y in 0..cell {
            for (i, &w) in c.iter().enumerate() {
                for x in 0..cell {
                    img[y * width + i * cell + x] = if w { 1023 } else { 0 };
                }
            }
        }
        img
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    fn frames() -> Vec<u32> {
        let mut rng = Rng(42);
        let mut v = vec![0, 1, 2, 255, 256, 0x8000_0000, u32::MAX - 1, u32::MAX];
        v.extend((0..500).map(|_| (rng.next() << 1) ^ rng.next()));
        v
    }

    #[test]
    fn layout() {
        assert_eq!(cells(5).len(), CELL_COUNT);
        assert_eq!(region(), (44 * 16, 16));
        assert_eq!(&cells(0)[..4], &[true, false, true, false]);
    }

    #[test]
    fn round_trip() {
        let width = region().0 as usize + 100;
        for f in frames() {
            assert_eq!(read_from_luma(&render(f, None, width), width), Some(f));
        }
    }

    #[test]
    fn survives_noise() {
        let width = region().0 as usize;
        let mut rng = Rng(7);
        for f in frames() {
            let mut img = render(f, None, width);
            for p in img.iter_mut() {
                let n = (rng.next() % 401) as i32 - 200;
                *p = (*p as i32 + n).clamp(0, 1023) as u16;
            }
            assert_eq!(read_from_luma(&img, width), Some(f));
        }
    }

    #[test]
    fn rejects_flipped_cell() {
        let width = region().0 as usize;
        for f in [0u32, 1, 12345, u32::MAX] {
            for i in 0..CELL_COUNT {
                assert_eq!(read_from_luma(&render(f, Some(i), width), width), None);
            }
        }
    }

    #[test]
    fn rejects_grey_and_small() {
        let width = region().0 as usize;
        assert_eq!(read_from_luma(&vec![512u16; width * 16], width), None);
        assert_eq!(read_from_luma(&vec![1023u16; width * 16], width), None);
        assert_eq!(read_from_luma(&[0u16; 10], 10), None);
    }

    #[test]
    fn packed_matches_cells() {
        let f = 0xDEAD_BEEF;
        let p = packed_cells(f);
        for (i, c) in cells(f).into_iter().enumerate() {
            assert_eq!((p[i / 32] >> (i % 32)) & 1 != 0, c);
        }
    }
}
