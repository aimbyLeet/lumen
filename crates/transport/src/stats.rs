//! Small statistics shared by the tools and the client.

/// The `p`-quantile (0.0..=1.0) of an ascending-sorted slice, nearest rank:
/// the element at index `round((len - 1) * p)`. NaN for an empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// How many per-frame samples a session keeps for its closing report: an
/// hour of 60 fps frames. Kept with [`push_recent`], a vector holds at most
/// twice as many (3.5 MB of `f64`).
pub const RECENT_SAMPLES: usize = 216_000;

/// Pushes a sample to a vector that would otherwise grow all session: once
/// it holds twice `keep`, the older half goes. The order stays (the newest
/// last), so the report is of the last `keep`..`2 * keep` samples.
pub fn push_recent<T>(v: &mut Vec<T>, x: T, keep: usize) {
    if keep > 0 && v.len() >= 2 * keep {
        v.drain(..v.len() - keep);
    }
    v.push(x);
}

#[cfg(test)]
mod tests {
    use super::{percentile, push_recent};

    #[test]
    fn recent_samples_are_bounded_and_in_order() {
        let mut v = Vec::new();
        for i in 0..5 {
            push_recent(&mut v, i, 3);
        }
        // Not full yet: everything is there.
        assert_eq!(v, [0, 1, 2, 3, 4]);
        for i in 5..7 {
            push_recent(&mut v, i, 3);
        }
        // Reached 6: the older half went before 6 came.
        assert_eq!(v, [3, 4, 5, 6]);
        for i in 7..100 {
            push_recent(&mut v, i, 3);
            assert!(v.len() <= 6 && v.len() > 3);
            assert_eq!(*v.last().unwrap(), i);
            assert!(v.windows(2).all(|w| w[0] + 1 == w[1]));
        }
    }

    #[test]
    fn empty_is_nan() {
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn one_element_is_itself() {
        for p in [0.0, 0.5, 1.0] {
            assert_eq!(percentile(&[7.5], p), 7.5);
        }
    }

    #[test]
    fn ten_elements() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.0), 1.0);
        // Index round(4.5) = 5: halves round away from zero.
        assert_eq!(percentile(&v, 0.5), 6.0);
        assert_eq!(percentile(&v, 0.95), 10.0);
        assert_eq!(percentile(&v, 0.99), 10.0);
        assert_eq!(percentile(&v, 1.0), 10.0);
    }
}
