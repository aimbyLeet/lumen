//! SplitMix64 generator; deterministic and dependency-free (loss emulation).

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1) with 53 bits of precision.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [0, n], `n` inclusive.
    pub fn below_inclusive(&mut self, n: u64) -> u64 {
        if n == u64::MAX {
            return self.next_u64();
        }
        let m = n + 1;
        // Rejection sampling to avoid modulo bias.
        let zone = u64::MAX - (u64::MAX % m);
        loop {
            let x = self.next_u64();
            if x < zone {
                return x % m;
            }
        }
    }
}
