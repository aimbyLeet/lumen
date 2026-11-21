//! CPU time used, for the programs' reports.

use std::time::Duration;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes,
};

/// User + kernel time of the calling thread, or of the whole process.
pub fn cpu_time(thread: bool) -> Duration {
    let mut t = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut t;
    let ok = unsafe {
        if thread {
            GetThreadTimes(GetCurrentThread(), created, exited, kernel, user)
        } else {
            GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user)
        }
    };
    if ok.is_err() {
        return Duration::ZERO;
    }
    // FILETIME: 100 ns units.
    let ticks = |f: &FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((ticks(&t[2]) + ticks(&t[3])) * 100)
}
