//! Short sleeps that cannot hang. `std::thread::sleep` waits on a
//! high-resolution waitable timer with no time limit; on the owner's client
//! (2026-11-13, Windows 11 26200) one such timer never fired and the audio
//! thread slept in a 2 ms sleep for good: no sound, and the video window could
//! not close (its `Drop` joined that thread). [`Nap`] waits on the same kind of
//! timer, but no longer than the sleep plus [`GRACE`]: that wait is by the
//! system tick, a separate mechanism. A timer that missed is made afresh.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimer, Sleep,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// How late a timer may be before the wait gives up on it.
pub const GRACE: Duration = Duration::from_millis(50);

/// A thread's sleeper (one timer, made once).
pub struct Nap {
    timer: Option<crate::Handle>,
    missed: u64,
}

impl Default for Nap {
    fn default() -> Self {
        Self::new()
    }
}

impl Nap {
    pub fn new() -> Nap {
        Nap {
            timer: high_resolution_timer(),
            missed: 0,
        }
    }

    /// Sleeps for `d` (high resolution, as `thread::sleep`); `false`: the timer
    /// did not fire and the wait ended after `d + GRACE`.
    pub fn sleep(&mut self, d: Duration) -> bool {
        self.wait(d, true)
    }

    /// Times the timer missed so far.
    pub fn missed(&self) -> u64 {
        self.missed
    }

    /// `armed: false` leaves the timer unset: a timer that never fires (tests).
    #[doc(hidden)]
    pub fn wait(&mut self, d: Duration, armed: bool) -> bool {
        let deadline = Instant::now() + d;
        let Some(timer) = self.timer.as_ref().map(crate::Handle::get) else {
            // No timer: the system tick's sleep (~15.6 ms steps), never forever.
            tick_sleep(d);
            return true;
        };
        // Relative due time, in 100 ns units.
        let due = -i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX).max(1);
        // SAFETY: `timer` is a live timer handle owned by `self`; `due` outlives the call.
        let set = !armed || unsafe { SetWaitableTimer(timer, &due, 0, None, None, false) }.is_ok();
        if set {
            let limit = (d + GRACE).as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: as above.
            if unsafe { WaitForSingleObject(timer, limit) } == WAIT_OBJECT_0 {
                return true;
            }
        }
        self.missed += 1;
        // That timer may be broken for good: closed, then a new one.
        self.timer = None;
        self.timer = high_resolution_timer();
        let left = deadline.saturating_duration_since(Instant::now());
        if !left.is_zero() {
            tick_sleep(left);
        }
        false
    }
}

fn high_resolution_timer() -> Option<crate::Handle> {
    // SAFETY: no pointers but the empty name.
    unsafe {
        CreateWaitableTimerExW(
            None,
            PCWSTR::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        )
        .ok()
        // SAFETY: just made, ours alone.
        .map(|h| crate::Handle::from_raw(h))
    }
}

/// `Sleep`: by the system tick (~15.6 ms steps), with no waitable timer.
fn tick_sleep(d: Duration) {
    let ms = d.as_millis().clamp(1, u128::from(u32::MAX - 1)) as u32;
    // SAFETY: a plain kernel32 call.
    unsafe { Sleep(ms) };
}
