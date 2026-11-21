//! A Windows handle owned by this process.

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// A kernel handle (event, mutex, timer, token) closed when dropped, unless it
/// is invalid (null or -1). Kept as a number: kernel handles are the
/// process's, any thread may use one.
pub struct Handle(usize);

impl Handle {
    /// # Safety
    /// `h` must be a handle this takes over: valid, closed with `CloseHandle`
    /// and by nobody else.
    pub unsafe fn from_raw(h: HANDLE) -> Handle {
        Handle(h.0 as usize)
    }

    pub fn get(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.get().is_invalid() {
            // SAFETY: owned by `self` (`from_raw`), closed once.
            unsafe {
                let _ = CloseHandle(self.get());
            }
        }
    }
}
