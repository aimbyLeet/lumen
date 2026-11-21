//! Virtual monitors of Lumen VDD.
//!
//! The service keeps them ([`Keeper`]): the driver's monitors belong to its
//! handle, so they live while the service runs. Their settings are the
//! machine's, `%ProgramData%\Lumen\monitors.json` (written by the service
//! only). Where they sit and their scale are set on the console's desktop:
//! the service (session 0) runs `lumen-host displays arrange` there — the
//! place as SYSTEM (Windows keeps it for the machine), the scale as the
//! signed-in user (Windows keeps it per user), again for each user who signs
//! in.
//!
//! A host without a physical monitor shows the client
//! in session its own monitors instead: monitor 1 virtual too, as the client
//! chose them (`client-monitors.json`, by the client's key); the machine's
//! are not up then. The host tells the service what it sees
//! ([`Keeper::host_report`]).
//!
//! `lumen-host displays ...` tries it by hand (from an administrator's
//! console: only SYSTEM and administrators may open the driver).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use vdd::device::Device;
use vdd::display::Target;
use vdd::layout;

mod arrange;
mod cli;
mod keeper;
mod shape;

pub use arrange::{Part, arrange_plan};
pub use cli::{ask_service, hold, print_desktop, print_info};
pub use keeper::Keeper;

/// The driver's id of monitor `n`: stable, Windows keeps its scale and place
/// by it. The machine's monitors 2 and 3 are 0 and 1; a client's own
/// monitor 1 is 2 (its 2 and 3 are 0 and 1 again: the machine's are not up
/// then).
fn driver_id(monitor: usize) -> u32 {
    match monitor {
        1 => 2,
        n => n as u32 - 2,
    }
}

/// A virtual monitor that is up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Up {
    pub monitor: usize,
    pub target: Target,
    /// The resolution asked for (`arrange` gives it).
    pub width: u32,
    pub height: u32,
}

/// The driver's device and the monitors this process has added through it.
pub struct VirtualMonitors {
    pub(super) device: Device,
    pub(super) up: Vec<Up>,
}

/// Retries a driver call while the monitor is changing.
fn retry<T>(mut f: impl FnMut() -> vdd::device::Result<T>) -> vdd::device::Result<T> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match f() {
            Err(e) if e.is_transient() && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(200))
            }
            r => return r,
        }
    }
}

impl VirtualMonitors {
    pub fn open() -> Result<VirtualMonitors> {
        Ok(VirtualMonitors {
            device: Device::open()?,
            up: Vec::new(),
        })
    }

    pub fn up(&self) -> &[Up] {
        &self.up
    }

    /// Adds and removes monitors to match `wanted`. Each one comes with every
    /// resolution of the list: a new one for a monitor that is up is set in
    /// place by [`arrange`], without plugging it out and in. Returns whether
    /// one was added (Windows turns a new one on itself).
    pub fn apply(&mut self, wanted: &[(usize, u32, u32)]) -> Result<bool> {
        // Gone first: their places may be taken by the others.
        let (keep, gone): (Vec<Up>, Vec<Up>) = std::mem::take(&mut self.up)
            .into_iter()
            .partition(|u| wanted.iter().any(|w| w.0 == u.monitor));
        self.up = keep;
        let mut added_any = false;
        for u in gone {
            if let Err(e) = retry(|| self.device.remove(driver_id(u.monitor))) {
                self.up.push(u.clone());
                return Err(e).with_context(|| format!("remove monitor {}", u.monitor));
            }
        }
        for &(monitor, width, height) in wanted {
            match self.up.iter_mut().find(|u| u.monitor == monitor) {
                Some(u) => (u.width, u.height) = (width, height),
                None => {
                    added_any = true;
                    let modes = layout::modes(width, height);
                    let added = retry(|| self.device.add(driver_id(monitor), &modes, None))
                        .with_context(|| format!("add monitor {monitor}"))?;
                    self.up.push(Up {
                        monitor,
                        target: Target {
                            adapter: added.adapter,
                            id: added.target_id,
                        },
                        width,
                        height,
                    });
                }
            }
        }
        self.up.sort_by_key(|u| u.monitor);
        Ok(added_any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_ids_stay_apart() {
        // The machine's 2 and 3 keep their ids; a client's monitor 1 has its own.
        assert_eq!([1, 2, 3].map(driver_id), [2, 0, 1]);
    }
}
