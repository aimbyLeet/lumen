//! The session's bitrate ceiling, shared by its streams.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use transport::CapController;

/// The session's bitrate ceiling, shared by its streams: one controller
/// sees what all encoders produce and sets every encoder to the same nominal
/// rate. A still monitor takes little whatever its nominal rate, so the one in
/// motion gets most of the cap; several in motion share it.
pub(crate) struct SessionRate {
    ctl: Mutex<Option<CapController>>,
    nominal: AtomicU32,
}

impl SessionRate {
    pub(crate) fn new(ctl: Option<CapController>) -> Self {
        let nominal = ctl.as_ref().map_or(0, CapController::nominal_bps);
        SessionRate {
            ctl: Mutex::new(ctl),
            nominal: AtomicU32::new(nominal),
        }
    }

    /// What every encoder is set to, bit/s; 0: no cap.
    pub(crate) fn nominal_bps(&self) -> u32 {
        self.nominal.load(Ordering::Relaxed)
    }

    pub(super) fn on_frame(&self, bytes: usize, qp: u32) {
        let mut ctl = self.ctl.lock().unwrap();
        if let Some(c) = ctl.as_mut() {
            c.on_frame(bytes, qp);
            if let Some(bps) = c.poll(Instant::now()) {
                self.nominal.store(bps, Ordering::Relaxed);
            }
        }
    }

    /// A new ceiling (lowered while files go out).
    pub(crate) fn set_cap(&self, cap_bps: u32) {
        if let Some(c) = self.ctl.lock().unwrap().as_mut() {
            let bps = c.set_cap(cap_bps, Instant::now());
            self.nominal.store(bps, Ordering::Relaxed);
        }
    }
}
