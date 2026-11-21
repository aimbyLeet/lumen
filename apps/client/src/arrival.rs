//! How one frame's video datagrams arrived: what the client judges the link by
//! when it picks the host's pacing ([`crate::pace`]) and FEC parity
//! ([`crate::autofec`]). Every video stream (monitor of the host) has frames of
//! its own; the host's pacer interleaves the streams' datagrams, so a frame's
//! rate counts every video datagram that arrived meanwhile.

use protocol::FragmentHeader;

/// One FEC block of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub data: u16,
    pub parity: u16,
    /// Per shard index (data, then parity): received.
    pub got: Vec<bool>,
}

impl Block {
    /// Indices of the shards that did not arrive.
    pub fn lost(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.got.len() as u16).filter(|&i| !self.got[usize::from(i)])
    }
}

/// One frame, finished when the next frame of its stream starts (frames of a
/// stream arrive in order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameArrival {
    pub number: u32,
    pub first_us: u64,
    pub last_us: u64,
    /// Video bytes of any stream after the frame's first datagram, up to its last.
    pub bytes: u64,
    pub packets: u32,
    /// None: no datagram of that block arrived (its shape is unknown).
    pub blocks: Vec<Option<Block>>,
    /// `Collector::total` at the frame's first datagram.
    total_at_first: u64,
}

#[derive(Default)]
pub struct Collector {
    /// The frame arriving now, per stream.
    frames: [Option<FrameArrival>; protocol::MAX_STREAMS as usize],
    /// Video bytes of every stream so far.
    total: u64,
}

impl Collector {
    /// A video datagram of `stream` of `bytes` arrived at `now_us`; the
    /// stream's previous frame, if this one starts a new frame.
    pub fn on_datagram(
        &mut self,
        now_us: u64,
        stream: u8,
        f: &FragmentHeader,
        bytes: usize,
    ) -> Option<FrameArrival> {
        self.total += bytes as u64;
        let total = self.total;
        let slot = self.frames.get_mut(usize::from(stream))?;
        let done = slot.take_if(|fr| fr.number != f.frame);
        let fr = slot.get_or_insert_with(|| FrameArrival {
            number: f.frame,
            first_us: now_us,
            last_us: now_us,
            bytes: 0,
            packets: 0,
            blocks: vec![None; usize::from(f.block_count)],
            total_at_first: total,
        });
        fr.bytes = total - fr.total_at_first;
        fr.packets += 1;
        fr.last_us = now_us;
        if let Some(slot) = fr.blocks.get_mut(usize::from(f.block)) {
            let b = slot.get_or_insert_with(|| Block {
                data: f.data_shards,
                parity: f.parity_shards,
                got: vec![false; usize::from(f.data_shards + f.parity_shards)],
            });
            if let Some(g) = b.got.get_mut(usize::from(f.index)) {
                *g = true;
            }
        }
        done
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Feeds one single-block frame of `data` + `parity` datagrams of 1200
    /// bytes arriving at `mbps` from `t0_us`; datagrams in `lost` do not arrive.
    /// Returns the frame finished by this one's first datagram.
    #[allow(clippy::too_many_arguments)]
    pub fn feed(
        c: &mut Collector,
        number: u32,
        t0_us: u64,
        data: u16,
        parity: u16,
        mbps: f64,
        lost: &[u16],
    ) -> Vec<FrameArrival> {
        let gap_us = 1200.0 * 8.0 / mbps;
        let mut done = Vec::new();
        for i in 0..data + parity {
            if lost.contains(&i) {
                continue;
            }
            let f = FragmentHeader {
                frame: number,
                total_len: 0,
                capture_time_us: 0,
                block: 0,
                block_count: 1,
                index: i,
                data_shards: data,
                parity_shards: parity,
            };
            let t = t0_us + (f64::from(i) * gap_us) as u64;
            done.extend(c.on_datagram(t, 0, &f, 1200));
        }
        done
    }

    #[test]
    fn a_frame_is_finished_by_the_next() {
        let mut c = Collector::default();
        assert!(feed(&mut c, 1, 0, 10, 2, 100.0, &[3, 11]).is_empty());
        let done = feed(&mut c, 2, 20_000, 10, 2, 100.0, &[]);
        assert_eq!(done.len(), 1);
        let f = &done[0];
        assert_eq!((f.number, f.packets, f.bytes), (1, 10, 9 * 1200));
        let b = f.blocks[0].as_ref().expect("block");
        assert_eq!((b.data, b.parity), (10, 2));
        assert_eq!(b.lost().collect::<Vec<_>>(), vec![3, 11]);
    }

    #[test]
    fn interleaved_streams_count_the_whole_link() {
        // Two streams' frames 1 and 1 sent in turn, a datagram each, at 100
        // Mbit/s together: each frame's rate is the link's, not half of it.
        let mut c = Collector::default();
        let gap_us = 1200.0 * 8.0 / 100.0;
        let header = |frame, index| FragmentHeader {
            frame,
            total_len: 0,
            capture_time_us: 0,
            block: 0,
            block_count: 1,
            index,
            data_shards: 10,
            parity_shards: 0,
        };
        let mut t = 0.0;
        for i in 0..10 {
            for s in 0..2 {
                assert!(c.on_datagram(t as u64, s, &header(1, i), 1200).is_none());
                t += gap_us;
            }
        }
        for s in 0..2 {
            let f = c.on_datagram(t as u64, s, &header(2, 0), 1200).unwrap();
            assert_eq!(f.packets, 10);
            let mbps = f.bytes as f64 * 8.0 / (f.last_us - f.first_us) as f64;
            assert!((mbps - 100.0).abs() < 2.0, "stream {s}: {mbps:.1} Mbit/s");
        }
    }
}
