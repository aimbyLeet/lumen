//! Per-session datagram sequence numbers shared by every sending thread.
//!
//! One counter per sender and session: the receiver derives packet loss from
//! gaps, and `seq` becomes the AEAD nonce once encryption is on, so it must
//! never repeat — two counters for one key would reuse nonces.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Default)]
pub struct SeqCounter(Arc<AtomicU64>);

impl SeqCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the next number and advances the counter.
    pub fn take(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}
