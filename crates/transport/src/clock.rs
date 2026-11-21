//! Wall clock in microseconds, comparable across processes on one machine.
//!
//! `Instant` is process-local and opaque; latency from host capture to client
//! display needs a clock both processes share. On Windows `SystemTime` is
//! `GetSystemTimePreciseAsFileTime` (sub-microsecond). Across two machines the
//! values are only as aligned as their clocks.

use std::time::{SystemTime, UNIX_EPOCH};

/// Microseconds since the Unix epoch.
pub fn wall_clock_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}
