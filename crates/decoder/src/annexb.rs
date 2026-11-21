//! Splits an HEVC Annex B byte stream into access units.
//!
//! The live path gets whole access units from NVENC; this is for files
//! (`bench-out\stream.hevc`) so the decoder is fed exactly as it will be live.
//!
//! A new AU starts at (H.265 7.4.2.4.4): an AUD, a VPS/SPS/PPS or prefix SEI
//! following a VCL NAL of the current AU, or a VCL NAL with
//! `first_slice_segment_in_pic_flag = 1` following a VCL NAL.

/// VPS, SPS, PPS, AUD, prefix SEI, reserved 41..=44, unspecified 48..=55.
fn opens_au(t: u8) -> bool {
    matches!(t, 32..=35 | 39 | 41..=44 | 48..=55)
}

fn is_vcl(t: u8) -> bool {
    t < 32
}

/// Start offsets of NAL units (positions of their start codes).
fn nal_starts(data: &[u8]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            // Include a leading zero of a 4-byte start code.
            starts.push(if i > 0 && data[i - 1] == 0 { i - 1 } else { i });
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
}

/// Returns the access units of `data` as sub-slices, in order.
pub fn split_access_units(data: &[u8]) -> Vec<&[u8]> {
    let starts = nal_starts(data);
    let mut aus = Vec::new();
    let mut au_start = match starts.first() {
        Some(&s) => s,
        None => return aus,
    };
    let mut seen_vcl = false;
    for &s in &starts {
        let hdr = s + if data[s + 2] == 1 { 3 } else { 4 };
        let Some(&b0) = data.get(hdr) else { break };
        let t = (b0 >> 1) & 0x3F;
        let boundary = if is_vcl(t) {
            // first_slice_segment_in_pic_flag: MSB of the byte after the 2-byte header.
            let first_slice = data.get(hdr + 2).is_some_and(|b| b & 0x80 != 0);
            seen_vcl && first_slice
        } else {
            seen_vcl && opens_au(t)
        };
        if boundary {
            aus.push(&data[au_start..s]);
            au_start = s;
            seen_vcl = false;
        }
        if is_vcl(t) {
            seen_vcl = true;
        }
    }
    aus.push(&data[au_start..]);
    aus
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(t: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 1, t << 1, 1];
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn splits_param_sets_and_slices() {
        let first = [0x80, 0xAA];
        let next = [0x00, 0xBB];
        let mut s = Vec::new();
        // AU 0: VPS SPS PPS SEI IDR(first) IDR(second slice)
        for t in [32, 33, 34, 39] {
            s.extend(nal(t, &[0x11]));
        }
        s.extend(nal(19, &first));
        s.extend(nal(19, &next));
        let au1 = s.len();
        // AU 1: TRAIL_R first slice
        s.extend(nal(1, &first));
        let au2 = s.len();
        // AU 2: AUD + TRAIL_R + suffix SEI (40 stays in the AU)
        s.extend(nal(35, &[0x50]));
        s.extend(nal(1, &first));
        s.extend(nal(40, &[0x22]));

        let aus = split_access_units(&s);
        assert_eq!(aus.len(), 3);
        assert_eq!(aus[0], &s[..au1]);
        assert_eq!(aus[1], &s[au1..au2]);
        assert_eq!(aus[2], &s[au2..]);
    }

    #[test]
    fn three_byte_start_codes() {
        let mut s = vec![0, 0, 1, 1 << 1, 1, 0x80];
        s.extend([0, 0, 1, 1 << 1, 1, 0x80]);
        assert_eq!(split_access_units(&s).len(), 2);
    }

    #[test]
    fn empty_and_garbage() {
        assert!(split_access_units(&[]).is_empty());
        assert!(split_access_units(&[1, 2, 3, 4]).is_empty());
        assert_eq!(split_access_units(&[0, 0, 1]).len(), 1);
    }
}
