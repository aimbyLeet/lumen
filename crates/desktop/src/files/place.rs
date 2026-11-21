//! Where a session's files go and as whom: the signed-in user's downloads,
//! the thread impersonating them on the host (SYSTEM).

use std::io;
use std::path::PathBuf;
use std::time::Instant;

use protocol::FileCancel;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{ImpersonateLoggedOnUser, RevertToSelf};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

use super::{PLACE_EVERY, Worker};

/// A user's token (from `WTSQueryUserToken`), closed on drop.
pub struct UserToken(crate::Handle);

impl UserToken {
    /// # Safety
    /// `h` must be a token handle this takes over.
    pub unsafe fn from_raw(h: HANDLE) -> UserToken {
        // SAFETY: the caller's promise.
        UserToken(unsafe { crate::Handle::from_raw(h) })
    }

    fn handle(&self) -> HANDLE {
        self.0.get()
    }
}

/// `<Downloads>\Lumen` of the user `token` is for (this process's user
/// without one), wherever they moved their Downloads.
pub fn downloads(token: Option<&UserToken>) -> io::Result<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(
            &FOLDERID_Downloads,
            KF_FLAG_DEFAULT,
            token.map(|t| t.handle()),
        )
        .map_err(io::Error::other)?;
        let path = PathBuf::from(p.to_string().unwrap_or_default());
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(path.join("Lumen"))
    }
}

/// Where the files of a session go, and as whom: downloads into `folder`,
/// what goes onto the clipboard under `temp`.
pub enum Place {
    /// As this process's user.
    Here { folder: PathBuf, temp: PathBuf },
    /// As the user of the token (the host's thread impersonates them).
    As {
        user: UserToken,
        folder: PathBuf,
        temp: PathBuf,
    },
    /// Nobody is signed in (the host): no transfers.
    Nobody,
    /// Windows is locked (the host): no transfers until a sign-in.
    Locked,
}

/// Where a session's files go, asked again while it goes (the host: a sign-in
/// or a lock mid-session).
pub type PlaceFn = Box<dyn Fn() -> Place + Send>;

impl Worker {
    pub(super) fn unimpersonate(&mut self) {
        if self.impersonating.take().is_some() {
            unsafe {
                let _ = RevertToSelf();
            }
        }
    }

    /// Asks where files go again (at most once per `PLACE_EVERY`).
    pub(super) fn refresh_place(&mut self) {
        if self
            .place_checked
            .is_some_and(|t| t.elapsed() < PLACE_EVERY)
        {
            return;
        }
        self.place_checked = Some(Instant::now());
        if let Some(f) = self.place_fn.take() {
            let place = self.ask_place(&f);
            self.place_fn = Some(f);
            self.apply_place(place);
        }
    }

    /// Asks where files go now, however recently it was asked. Nothing else
    /// asks while nothing goes: a session opened on the lock screen would
    /// still act as SYSTEM after the sign-in.
    pub(super) fn refresh_place_now(&mut self) {
        self.place_checked = None;
        self.refresh_place();
    }

    /// `f` asked as ourselves, not as the user we act for: the host's asks
    /// as SYSTEM (its own token, `WTSQueryUserToken`), which the user may not
    /// do — asked as them, every other answer was "not SYSTEM", and files
    /// were read and written as SYSTEM until the next ask (the owner's host,
    /// 2026-10-28: the network share denied, every first paste).
    pub(super) fn ask_place(&mut self, f: &PlaceFn) -> Place {
        self.unimpersonate();
        f()
    }

    pub(super) fn apply_place(&mut self, place: Place) {
        self.unimpersonate();
        let locked = matches!(place, Place::Locked);
        if locked != self.locked {
            println!(
                "files: {}",
                if locked {
                    "Windows is locked: no transfers until a sign-in"
                } else {
                    "Windows is not locked"
                }
            );
        }
        self.locked = locked;
        (self.folder, self.temp) = match place {
            Place::Here { folder, temp } => (Some(folder), Some(temp)),
            Place::As { user, folder, temp } => {
                if let Err(e) = unsafe { ImpersonateLoggedOnUser(user.handle()) } {
                    eprintln!("files: impersonation failed: {e}");
                    (None, None)
                } else {
                    self.impersonating = Some(user);
                    (Some(folder), Some(temp))
                }
            }
            Place::Nobody | Place::Locked => (None, None),
        };
    }

    /// Why files cannot come or go on this side now, if they cannot.
    pub(super) fn unavailable(&mut self) -> Option<FileCancel> {
        self.refresh_place();
        if self.locked {
            Some(FileCancel::Locked)
        } else if self.folder.is_none() {
            Some(FileCancel::NoUser)
        } else {
            None
        }
    }
}
