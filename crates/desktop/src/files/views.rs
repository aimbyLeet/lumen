//! What the overlay shows of the transfers, and the log's lines about them.

use std::time::Instant;

use super::{LOG_EVERY, Stage, VIEW_EVERY, View, Worker};

impl Worker {
    pub(super) fn refresh_views(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_view) < VIEW_EVERY {
            return;
        }
        self.last_view = now;
        let acked = |id: u32| {
            self.session
                .as_ref()
                .and_then(|s| s.tx.progress(id))
                .map_or(0, |p| p.acked)
        };
        let mut views: Vec<View> = self
            .out
            .iter()
            .map(|(&id, o)| o.view(id, acked(id)))
            .chain(self.incoming.iter().map(|(&id, i)| i.view(id)))
            .collect();
        self.smooth_speeds(&mut views, now);
        views.sort_by_key(|v| v.started);
        if now.duration_since(self.last_log) >= LOG_EVERY {
            self.last_log = now;
            self.log_running(&views);
        }
        *self.views.lock().unwrap() = views;
    }

    /// Each running transfer's speed, smoothed over about a second.
    fn smooth_speeds(&mut self, views: &mut [View], now: Instant) {
        for v in views {
            let key = (v.outgoing, v.transfer);
            let speed = match self.speed.get(&key) {
                Some(&(t, b, s)) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt > 0.05 {
                        let inst = v.done.saturating_sub(b) as f64 / dt;
                        let a = (dt / 1.0).min(1.0);
                        s + (inst - s) * a
                    } else {
                        s
                    }
                }
                None => 0.0,
            };
            v.speed = if v.stage == Stage::Running {
                speed
            } else {
                0.0
            };
            self.speed.insert(key, (now, v.done, speed));
        }
    }

    fn log_running(&self, views: &[View]) {
        for v in views.iter().filter(|v| v.stage == Stage::Running) {
            println!(
                "files: #{} {} {}: {} of {} bytes, {:.1} Mbit/s",
                v.transfer,
                if v.outgoing { "out" } else { "in" },
                v.name,
                v.done,
                v.len,
                v.speed * 8.0 / 1e6
            );
        }
        if let Some(s) = &self.session {
            let st = s.tx.stats();
            if st.sent_bytes > 0 {
                println!(
                    "files: sender rate {:.1} Mbit/s, arrives {:.1}, rtt {:.1} ms (base {:.1}, moved {}), resent {}, timeouts {}, slow reads {} (longest {} ms)",
                    st.rate_bps as f64 / 1e6,
                    st.delivery_bps as f64 / 1e6,
                    st.srtt.as_secs_f64() * 1e3,
                    st.base_rtt.as_secs_f64() * 1e3,
                    st.rebased,
                    st.resent,
                    st.timeouts,
                    st.slow_reads,
                    st.longest_read.as_millis()
                );
            }
            // Chunks that came twice: resent with no need.
            if s.rx.duplicates() > 0 {
                println!(
                    "files: received {} bytes, {} chunks twice",
                    s.rx.bytes(),
                    s.rx.duplicates()
                );
            }
        }
    }
}
