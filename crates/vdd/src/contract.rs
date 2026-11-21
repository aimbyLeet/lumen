//! The contract with the Lumen VDD driver: a mirror of `lumen_vdd_ioctl.h`
//! (repository `lumen-vdd`, `include/`), which is the source of truth. The
//! tests check the IOCTL codes and struct sizes against the values it states,
//! and against the header itself when it is next to this repository.

pub const CONTRACT_MAJOR: u16 = 1;
pub const CONTRACT_MINOR: u16 = 1;

/// Device interface class `{0F76755D-FA51-4FB9-A19D-39A14E1AEA68}`.
pub const INTERFACE_GUID: u128 = 0x0f76755d_fa51_4fb9_a19d_39a14e1aea68;

/// Capability bits ([`Info::caps`]).
pub mod caps {
    pub const ADD_REMOVE: u32 = 0x01;
    pub const RENDER_ADAPTER: u32 = 0x02;
    pub const SET_MODES: u32 = 0x04;
    pub const LIST_MONITORS: u32 = 0x08;
    pub const EDID: u32 = 0x10;
    pub const DIRECT_FRAMES: u32 = 0x20;
}

pub const MAX_MODES: usize = 32;
pub const MAX_EDID: usize = 1024;

/// `CTL_CODE(device_type, function, method, access)`.
pub const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const FILE_DEVICE_UNKNOWN: u32 = 0x22;
const METHOD_BUFFERED: u32 = 0;
const FILE_ANY_ACCESS: u32 = 0;
const FILE_READ_WRITE: u32 = 3;

const fn ioctl(function: u32, access: u32) -> u32 {
    ctl_code(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, access)
}

pub const IOCTL_GET_INFO: u32 = ioctl(0x800, FILE_ANY_ACCESS);
pub const IOCTL_ADD_MONITOR: u32 = ioctl(0x801, FILE_READ_WRITE);
pub const IOCTL_REMOVE_MONITOR: u32 = ioctl(0x802, FILE_READ_WRITE);
pub const IOCTL_SET_RENDER_ADAPTER: u32 = ioctl(0x803, FILE_READ_WRITE);
pub const IOCTL_SET_MODES: u32 = ioctl(0x804, FILE_READ_WRITE);
pub const IOCTL_LIST_MONITORS: u32 = ioctl(0x805, FILE_ANY_ACCESS);

/// [`Monitor::state`].
pub mod state {
    pub const ACTIVE: u32 = 1;
    pub const SUSPENDED: u32 = 2;
    pub const CHANGING: u32 = 3;
}

/// [`Monitor::flags`].
pub mod flags {
    pub const OWNED: u32 = 0x01;
    pub const OWN_EDID: u32 = 0x02;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub contract_major: u16,
    pub contract_minor: u16,
    pub caps: u32,
    pub max_monitors: u32,
    pub max_modes: u32,
    /// `(major << 16) | (minor << 8) | patch`.
    pub driver_version: u32,
    pub iddcx_version: u32,
}

impl Info {
    pub fn driver_version(&self) -> String {
        let v = self.driver_version;
        format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AddMonitor {
    pub id: u32,
    pub mode_count: u32,
    pub modes: [Mode; MAX_MODES],
    pub edid_size: u32,
    pub edid: [u8; MAX_EDID],
}

impl Default for AddMonitor {
    fn default() -> Self {
        AddMonitor {
            id: 0,
            mode_count: 0,
            modes: [Mode::default(); MAX_MODES],
            edid_size: 0,
            edid: [0; MAX_EDID],
        }
    }
}

/// An adapter LUID as the contract carries it.
#[repr(C)]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Luid {
    pub low: u32,
    pub high: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonitorAdded {
    pub adapter: Luid,
    pub target_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoveMonitor {
    pub id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SetRenderAdapter {
    pub luid: Luid,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SetModes {
    pub id: u32,
    pub flags: u32,
    pub mode_count: u32,
    pub modes: [Mode; MAX_MODES],
}

impl Default for SetModes {
    fn default() -> Self {
        SetModes {
            id: 0,
            flags: 0,
            mode_count: 0,
            modes: [Mode::default(); MAX_MODES],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub id: u32,
    pub state: u32,
    pub flags: u32,
    /// Zero while not active.
    pub adapter: Luid,
    pub target_id: u32,
    pub mode_count: u32,
    pub modes: [Mode; MAX_MODES],
}

impl Default for Monitor {
    fn default() -> Self {
        Monitor {
            id: 0,
            state: 0,
            flags: 0,
            adapter: Luid::default(),
            target_id: 0,
            mode_count: 0,
            modes: [Mode::default(); MAX_MODES],
        }
    }
}

impl Monitor {
    pub fn modes(&self) -> &[Mode] {
        &self.modes[..(self.mode_count as usize).min(MAX_MODES)]
    }

    pub fn owned(&self) -> bool {
        self.flags & flags::OWNED != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn codes_and_sizes_are_the_contracts() {
        // The numbers lumen_vdd_ioctl.h spells out.
        assert_eq!(IOCTL_GET_INFO, 0x0022_2000);
        assert_eq!(IOCTL_ADD_MONITOR, 0x0022_E004);
        assert_eq!(IOCTL_REMOVE_MONITOR, 0x0022_E008);
        assert_eq!(IOCTL_SET_RENDER_ADAPTER, 0x0022_E00C);
        assert_eq!(IOCTL_SET_MODES, 0x0022_E010);
        assert_eq!(IOCTL_LIST_MONITORS, 0x0022_2014);
        // The static_asserts of lumen_vdd_ioctl.h.
        assert_eq!(size_of::<Info>(), 24);
        assert_eq!(size_of::<Mode>(), 12);
        assert_eq!(size_of::<AddMonitor>(), 1420);
        assert_eq!(size_of::<MonitorAdded>(), 12);
        assert_eq!(size_of::<RemoveMonitor>(), 4);
        assert_eq!(size_of::<SetRenderAdapter>(), 8);
        assert_eq!(size_of::<SetModes>(), 396);
        assert_eq!(size_of::<Monitor>(), 412);
    }

    /// The header itself, if the driver's repository sits next to this one
    /// (`LUMEN_VDD_HEADER` points elsewhere).
    #[test]
    fn matches_the_header_if_present() {
        let path = std::env::var_os("LUMEN_VDD_HEADER").map_or_else(
            || {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../lumen-vdd/include/lumen_vdd_ioctl.h")
            },
            std::path::PathBuf::from,
        );
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("{} not found: skipped", path.display());
            return;
        };
        let define = |name: &str| -> u64 {
            let line = text
                .lines()
                .find(|l| l.split_whitespace().nth(1) == Some(name))
                .unwrap_or_else(|| panic!("{name} not in the header"));
            let v = line.split_whitespace().nth(2).unwrap();
            let v = v.trim_end_matches(['u', 'U']);
            match v.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16).unwrap(),
                None => v.parse().unwrap(),
            }
        };
        assert_eq!(define("LUMEN_VDD_CONTRACT_MAJOR"), CONTRACT_MAJOR as u64);
        assert_eq!(define("LUMEN_VDD_CONTRACT_MINOR"), CONTRACT_MINOR as u64);
        assert_eq!(define("LUMEN_VDD_MAX_MODES"), MAX_MODES as u64);
        assert_eq!(define("LUMEN_VDD_MAX_EDID"), MAX_EDID as u64);
        for (name, code) in [
            ("IOCTL_LUMEN_VDD_GET_INFO", IOCTL_GET_INFO),
            ("IOCTL_LUMEN_VDD_ADD_MONITOR", IOCTL_ADD_MONITOR),
            ("IOCTL_LUMEN_VDD_REMOVE_MONITOR", IOCTL_REMOVE_MONITOR),
            (
                "IOCTL_LUMEN_VDD_SET_RENDER_ADAPTER",
                IOCTL_SET_RENDER_ADAPTER,
            ),
            ("IOCTL_LUMEN_VDD_SET_MODES", IOCTL_SET_MODES),
            ("IOCTL_LUMEN_VDD_LIST_MONITORS", IOCTL_LIST_MONITORS),
        ] {
            assert_eq!(define(name), code as u64, "{name}");
        }
        for (name, bit) in [
            ("LUMEN_VDD_CAP_ADD_REMOVE", caps::ADD_REMOVE),
            ("LUMEN_VDD_CAP_RENDER_ADAPTER", caps::RENDER_ADAPTER),
            ("LUMEN_VDD_CAP_SET_MODES", caps::SET_MODES),
            ("LUMEN_VDD_CAP_LIST_MONITORS", caps::LIST_MONITORS),
            ("LUMEN_VDD_CAP_EDID", caps::EDID),
            ("LUMEN_VDD_CAP_DIRECT_FRAMES", caps::DIRECT_FRAMES),
            ("LUMEN_VDD_MONITOR_ACTIVE", state::ACTIVE),
            ("LUMEN_VDD_MONITOR_SUSPENDED", state::SUSPENDED),
            ("LUMEN_VDD_MONITOR_CHANGING", state::CHANGING),
            ("LUMEN_VDD_MONITOR_OWNED", flags::OWNED),
            ("LUMEN_VDD_MONITOR_OWN_EDID", flags::OWN_EDID),
        ] {
            assert_eq!(define(name), bit as u64, "{name}");
        }
        for (name, size) in [
            ("LUMEN_VDD_INFO", size_of::<Info>()),
            ("LUMEN_VDD_MODE", size_of::<Mode>()),
            ("LUMEN_VDD_ADD_MONITOR", size_of::<AddMonitor>()),
            ("LUMEN_VDD_MONITOR_ADDED", size_of::<MonitorAdded>()),
            ("LUMEN_VDD_REMOVE_MONITOR", size_of::<RemoveMonitor>()),
            (
                "LUMEN_VDD_SET_RENDER_ADAPTER",
                size_of::<SetRenderAdapter>(),
            ),
            ("LUMEN_VDD_SET_MODES", size_of::<SetModes>()),
            ("LUMEN_VDD_MONITOR", size_of::<Monitor>()),
        ] {
            let assert = format!("static_assert(sizeof({name}) == {size},");
            assert!(text.contains(&assert), "{assert}");
        }
        let g = INTERFACE_GUID;
        let guid = format!(
            "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
            g >> 96,
            (g >> 80) & 0xffff,
            (g >> 64) & 0xffff,
            (g >> 48) & 0xffff,
            g & 0xffff_ffff_ffff
        );
        assert!(
            text.contains(&format!("Device interface class: {guid}")),
            "{guid}"
        );
    }
}
