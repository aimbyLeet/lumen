//! `desktop::nap`: a sleep ends even when its timer never fires. Timed, so in
//! a test binary of its own.
#![cfg(any(windows, lumen_check))]

use std::time::{Duration, Instant};

use desktop::nap::{GRACE, Nap};

#[test]
fn a_nap_sleeps_its_time() {
    let mut nap = Nap::new();
    let t = Instant::now();
    for _ in 0..10 {
        assert!(nap.sleep(Duration::from_millis(2)));
    }
    let took = t.elapsed();
    assert!(took >= Duration::from_millis(20), "{took:?}");
    assert!(took < Duration::from_millis(200), "{took:?}");
    assert_eq!(nap.missed(), 0);
}

#[test]
fn a_timer_that_never_fires_ends_after_the_grace() {
    let mut nap = Nap::new();
    let t = Instant::now();
    assert!(!nap.wait(Duration::from_millis(2), false));
    let took = t.elapsed();
    assert!(took >= GRACE, "{took:?}");
    assert!(took < GRACE * 4, "{took:?}");
    assert_eq!(nap.missed(), 1);
    // The new timer works.
    assert!(nap.sleep(Duration::from_millis(2)));
}
