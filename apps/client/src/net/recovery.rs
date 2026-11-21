//! A stream's loss recovery: which frames go to the decoder, and what to ask
//! the host for after a loss or a decode failure.

use std::time::{Duration, Instant};

/// While waiting for an IDR or a recovery frame, repeat the request this often.
const REQUEST_RETRY: Duration = Duration::from_millis(150);

/// A stream's loss recovery: what it waits for, its newest frame given to the
/// decoder, its newest lost one, when it last asked the host.
#[derive(Default)]
pub(super) struct Recovery {
    awaiting: Awaiting,
    last_fed: Option<u32>,
    last_lost: u32,
    last_request: Option<Instant>,
    /// It lost a frame or its decoder failed: a keyframe is asked for (a
    /// stream's first IDR comes unasked).
    broken: bool,
}

/// What the network thread waits for before frames may go to the decoder again.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Awaiting {
    Nothing,
    #[default]
    Keyframe,
    /// A frame flagged `RECOVERY` newer than `lost`.
    Recovery {
        last_good: u32,
        lost: u32,
    },
}

/// What to ask the host for (`RequestKeyframe` / `RequestRecovery`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Ask {
    Keyframe { frame: u32 },
    Recovery { last_good: u32, lost: u32 },
}

impl Recovery {
    /// A complete frame: whether it goes to the decoder (false: dropped).
    pub(super) fn on_frame(&mut self, frame: u32, keyframe: bool, recovery: bool) -> bool {
        let resumes = match self.awaiting {
            Awaiting::Nothing => true,
            Awaiting::Keyframe => keyframe,
            Awaiting::Recovery { lost, .. } => keyframe || (recovery && frame > lost),
        };
        if !resumes {
            // Frames come but their IDR went by: it came before
            // the host's new `Monitors` (another path), which made
            // this stream wait for one. Ask for it (this frame is
            // newer than that IDR: the host does not take the
            // request for a repeat). Monitors stayed black, 2026-10-30.
            if self.awaiting == Awaiting::Keyframe {
                self.last_lost = frame;
                if !std::mem::replace(&mut self.broken, true) {
                    self.last_request = None;
                }
            }
            return false;
        }
        self.awaiting = Awaiting::Nothing;
        self.last_fed = Some(frame);
        self.broken = false;
        true
    }

    /// Frames up to `last` were lost.
    pub(super) fn on_lost(&mut self, last: u32) {
        self.awaiting = match (self.awaiting, self.last_fed) {
            (Awaiting::Keyframe, _) | (_, None) => Awaiting::Keyframe,
            (_, Some(last_good)) => Awaiting::Recovery {
                last_good,
                lost: last,
            },
        };
        self.last_lost = last;
        self.last_request = None;
        self.broken = true;
    }

    /// The decoder failed: a keyframe is asked for.
    pub(super) fn on_decode_failed(&mut self) {
        self.awaiting = Awaiting::Keyframe;
        self.last_request = None;
        self.broken = true;
        // The newest frame given to the decoder: the host's last IDR
        // may be newer than the last loss (its streams started over).
        if let Some(fed) = self.last_fed {
            self.last_lost = self.last_lost.max(fed);
        }
    }

    /// Whether it may ask the host for anything (without reading the clock).
    #[inline]
    pub(super) fn may_ask(&self) -> bool {
        match self.awaiting {
            Awaiting::Nothing => false,
            // The first IDR is sent by the host unasked; only request after a loss.
            Awaiting::Keyframe => self.broken,
            Awaiting::Recovery { .. } => true,
        }
    }

    /// What to ask the host for now, if anything: at once after a loss or a
    /// decode failure, then every `REQUEST_RETRY` until it comes.
    pub(super) fn ask(&mut self, now: Instant) -> Option<Ask> {
        if self.awaiting == Awaiting::Nothing
            || self
                .last_request
                .is_some_and(|t| now.duration_since(t) < REQUEST_RETRY)
        {
            return None;
        }
        let ask = match self.awaiting {
            // The first IDR is sent by the host unasked; only request after a loss.
            Awaiting::Keyframe if self.broken => Ask::Keyframe {
                frame: self.last_lost,
            },
            Awaiting::Recovery { last_good, lost } => Ask::Recovery { last_good, lost },
            _ => return None,
        };
        self.last_request = Some(now);
        Some(ask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn frames_before_the_first_keyframe_are_dropped_and_asked_for_once_broken() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        // The first IDR comes unasked.
        assert!(!r.may_ask());
        assert_eq!(r.ask(t0), None);
        assert!(!r.on_frame(5, false, false));
        assert!(r.broken);
        assert!(r.may_ask());
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 5 }));
        assert!(!r.on_frame(6, false, true));
        assert!(r.on_frame(7, true, false));
        assert_eq!(r.awaiting, Awaiting::Nothing);
        assert_eq!(r.last_fed, Some(7));
        assert!(!r.broken);
        assert!(!r.may_ask());
        assert_eq!(r.ask(t0 + ms(1000)), None);
    }

    #[test]
    fn a_keyframe_first_resumes_without_asking() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        assert!(r.on_frame(1, false, false));
        assert_eq!(r.ask(t0), None);
    }

    #[test]
    fn a_non_key_frame_while_awaiting_a_keyframe_marks_it_broken_and_asks_at_once() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        r.on_lost(10);
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 10 }));
        // Already broken: the request is not repeated before `REQUEST_RETRY`.
        assert!(!r.on_frame(12, false, false));
        assert_eq!(r.last_lost, 12);
        assert_eq!(r.ask(t0 + ms(10)), None);
        assert_eq!(r.ask(t0 + ms(150)), Some(Ask::Keyframe { frame: 12 }));

        // Not broken yet (a new generation of the host's monitors): at once.
        let mut r = Recovery {
            last_request: Some(t0),
            ..Default::default()
        };
        assert!(!r.on_frame(40, false, false));
        assert!(r.broken);
        assert_eq!(r.last_lost, 40);
        assert_eq!(r.last_request, None);
        assert_eq!(r.ask(t0 + ms(1)), Some(Ask::Keyframe { frame: 40 }));
    }

    #[test]
    fn a_loss_after_fed_frames_asks_for_recovery() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        assert!(r.on_frame(1, false, false));
        r.on_lost(3);
        assert!(r.broken);
        assert_eq!(
            r.ask(t0),
            Some(Ask::Recovery {
                last_good: 1,
                lost: 3
            })
        );
    }

    #[test]
    fn a_loss_before_any_fed_frame_asks_for_a_keyframe() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        r.on_lost(4);
        assert_eq!(r.awaiting, Awaiting::Keyframe);
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 4 }));

        // Awaiting a keyframe already: a loss keeps it so, fed frames or not.
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        r.on_decode_failed();
        r.on_lost(9);
        assert_eq!(r.awaiting, Awaiting::Keyframe);
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 9 }));
    }

    #[test]
    fn recovery_resumes_on_a_keyframe_or_a_newer_recovery_frame() {
        let fed_then_lost = || {
            let mut r = Recovery::default();
            assert!(r.on_frame(0, true, false));
            assert!(r.on_frame(1, false, false));
            r.on_lost(3);
            r
        };
        let mut r = fed_then_lost();
        assert!(!r.on_frame(4, false, false));
        // A dropped frame while awaiting a recovery changes nothing else.
        assert_eq!(r.last_lost, 3);
        assert!(!r.on_frame(3, false, true));
        assert!(r.on_frame(4, false, true));
        assert_eq!(r.awaiting, Awaiting::Nothing);
        assert_eq!(r.last_fed, Some(4));
        assert!(!r.broken);

        let mut r = fed_then_lost();
        assert!(r.on_frame(2, true, false));
        assert_eq!(r.awaiting, Awaiting::Nothing);
    }

    #[test]
    fn a_decode_failure_asks_for_a_keyframe_after_the_newest_frame() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        assert!(r.on_frame(8, false, false));
        r.on_decode_failed();
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 8 }));

        // An older loss: the newest fed frame. A newer loss: that one.
        let mut r = Recovery::default();
        assert!(r.on_frame(20, true, false));
        r.on_lost(25);
        r.on_decode_failed();
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 25 }));
        let mut r = Recovery::default();
        r.on_lost(25);
        assert!(r.on_frame(30, true, false));
        r.on_decode_failed();
        assert_eq!(r.ask(t0), Some(Ask::Keyframe { frame: 30 }));
    }

    #[test]
    fn a_request_repeats_only_after_the_retry_interval() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        r.on_lost(2);
        let ask = Some(Ask::Recovery {
            last_good: 0,
            lost: 2,
        });
        assert_eq!(r.ask(t0), ask);
        assert_eq!(r.ask(t0), None);
        assert_eq!(r.ask(t0 + REQUEST_RETRY - ms(1)), None);
        assert_eq!(r.ask(t0 + REQUEST_RETRY), ask);
        assert_eq!(r.ask(t0 + REQUEST_RETRY + ms(1)), None);
        assert_eq!(r.ask(t0 + 2 * REQUEST_RETRY), ask);
        // A decode failure asks again at once.
        r.on_decode_failed();
        assert_eq!(
            r.ask(t0 + 2 * REQUEST_RETRY),
            Some(Ask::Keyframe { frame: 2 })
        );
    }

    #[test]
    fn a_second_loss_while_awaiting_recovery_keeps_last_good_and_moves_lost() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert!(r.on_frame(0, true, false));
        assert!(r.on_frame(5, false, false));
        r.on_lost(7);
        assert_eq!(
            r.ask(t0),
            Some(Ask::Recovery {
                last_good: 5,
                lost: 7
            })
        );
        r.on_lost(11);
        assert_eq!(
            r.ask(t0 + ms(1)),
            Some(Ask::Recovery {
                last_good: 5,
                lost: 11
            })
        );
        assert!(!r.on_frame(10, false, true));
        assert!(r.on_frame(12, false, true));
    }
}
