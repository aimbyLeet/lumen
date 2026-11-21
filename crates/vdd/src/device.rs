//! The Lumen VDD device: found by its interface, opened by SYSTEM or an
//! administrator. Monitors added through a [`Device`] belong to it: dropped
//! (or the process gone), the driver removes them.

use std::ffi::c_void;
use std::fmt;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW,
    CM_Get_Device_Interface_ListW, CR_SUCCESS,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_BUSY, ERROR_NOT_FOUND,
    ERROR_NOT_READY, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{GUID, PCWSTR};

use crate::contract::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No Lumen VDD device on this machine.
    NotInstalled,
    /// Only SYSTEM and administrators may open the device.
    AccessDenied,
    /// The driver speaks another major version of the contract.
    Contract { major: u16, minor: u16 },
    /// The driver lacks a capability bit.
    Unsupported(&'static str),
    /// A call failed with a Win32 error.
    Call { what: &'static str, code: u32 },
    /// Windows' display configuration is not as expected.
    Layout(String),
}

impl Error {
    /// Worth a retry shortly (`ERROR_BUSY`, `ERROR_NOT_READY`).
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Call { code, .. }
            if *code == ERROR_BUSY.0 || *code == ERROR_NOT_READY.0)
    }

    pub fn code(&self) -> Option<u32> {
        match self {
            Error::Call { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotInstalled => f.write_str("Lumen VDD is not installed"),
            Error::AccessDenied => {
                f.write_str("Lumen VDD: access denied (only SYSTEM and administrators)")
            }
            Error::Contract { major, minor } => write!(
                f,
                "Lumen VDD speaks contract {major}.{minor}, this program {CONTRACT_MAJOR}.x"
            ),
            Error::Unsupported(what) => write!(f, "Lumen VDD: this driver cannot {what}"),
            Error::Layout(what) => write!(f, "display layout: {what}"),
            Error::Call { what, code } => {
                let why = match *code {
                    c if c == ERROR_ALREADY_EXISTS.0 => " (the monitor exists)".to_string(),
                    c if c == ERROR_NOT_FOUND.0 => " (no such monitor)".to_string(),
                    c if c == ERROR_ACCESS_DENIED.0 => {
                        " (the monitor belongs to another handle)".to_string()
                    }
                    c if c == ERROR_BUSY.0 => " (the monitor is changing)".to_string(),
                    c if c == ERROR_NOT_READY.0 => " (the adapter is not ready)".to_string(),
                    _ => String::new(),
                };
                write!(f, "Lumen VDD {what}: error {code}{why}")
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn interface_guid() -> GUID {
    GUID::from_u128(INTERFACE_GUID)
}

/// Paths of the present Lumen VDD interfaces (anyone may ask).
fn interfaces() -> Vec<String> {
    let guid = interface_guid();
    let mut len = 0u32;
    unsafe {
        if CM_Get_Device_Interface_List_SizeW(
            &mut len,
            &guid,
            PCWSTR::null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        ) != CR_SUCCESS
            || len <= 1
        {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(
            &guid,
            PCWSTR::null(),
            &mut buf,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        ) != CR_SUCCESS
        {
            return Vec::new();
        }
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

/// The driver is installed and its device is working (no rights needed).
pub fn installed() -> bool {
    !interfaces().is_empty()
}

/// An open Lumen VDD device; its monitors go when it is dropped.
pub struct Device {
    handle: HANDLE,
    info: Info,
}

// The handle is a plain kernel handle; IOCTLs from any thread are fine.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

impl Device {
    /// Opens the device and checks the contract's major version.
    pub fn open() -> Result<Device> {
        let path = interfaces().into_iter().next().ok_or(Error::NotInstalled)?;
        let name: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| {
            if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                Error::AccessDenied
            } else {
                Error::Call {
                    what: "open",
                    code: (e.code().0 & 0xffff) as u32,
                }
            }
        })?;
        let mut dev = Device {
            handle,
            info: Info::default(),
        };
        let mut info = Info::default();
        dev.ioctl(IOCTL_GET_INFO, "get info", None::<&()>, Some(&mut info))?;
        if info.contract_major != CONTRACT_MAJOR {
            return Err(Error::Contract {
                major: info.contract_major,
                minor: info.contract_minor,
            });
        }
        dev.info = info;
        Ok(dev)
    }

    pub fn info(&self) -> Info {
        self.info
    }

    fn need(&self, cap: u32, what: &'static str) -> Result<()> {
        if self.info.caps & cap == 0 {
            return Err(Error::Unsupported(what));
        }
        Ok(())
    }

    /// One buffered IOCTL; returns the bytes written to `output`.
    fn ioctl<I, O>(
        &self,
        code: u32,
        what: &'static str,
        input: Option<&I>,
        output: Option<&mut O>,
    ) -> Result<u32> {
        self.ioctl_raw(
            code,
            what,
            input.map(|i| (i as *const I as *const c_void, size_of::<I>())),
            output.map(|o| (o as *mut O as *mut c_void, size_of::<O>())),
        )
    }

    fn ioctl_raw(
        &self,
        code: u32,
        what: &'static str,
        input: Option<(*const c_void, usize)>,
        output: Option<(*mut c_void, usize)>,
    ) -> Result<u32> {
        let mut done = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                input.map(|i| i.0),
                input.map_or(0, |i| i.1 as u32),
                output.map(|o| o.0),
                output.map_or(0, |o| o.1 as u32),
                Some(&mut done),
                None,
            )
        }
        .map_err(|e| Error::Call {
            what,
            code: (e.code().0 & 0xffff) as u32,
        })?;
        Ok(done)
    }

    /// Adds monitor `id` (0 .. max_monitors-1) with `modes` (the first is
    /// preferred) and the driver's EDID, or `edid`. Returns where Windows
    /// exposes it.
    pub fn add(&self, id: u32, modes: &[Mode], edid: Option<&[u8]>) -> Result<MonitorAdded> {
        self.need(caps::ADD_REMOVE, "add monitors")?;
        let mut req = AddMonitor {
            id,
            ..AddMonitor::default()
        };
        req.mode_count = copy_modes(&mut req.modes, modes)?;
        if let Some(edid) = edid {
            self.need(caps::EDID, "take an EDID")?;
            if edid.len() > MAX_EDID {
                return Err(Error::Call {
                    what: "add monitor",
                    code: windows::Win32::Foundation::ERROR_INVALID_PARAMETER.0,
                });
            }
            req.edid[..edid.len()].copy_from_slice(edid);
            req.edid_size = edid.len() as u32;
        }
        let mut out = MonitorAdded::default();
        self.ioctl(IOCTL_ADD_MONITOR, "add monitor", Some(&req), Some(&mut out))?;
        Ok(out)
    }

    pub fn remove(&self, id: u32) -> Result<()> {
        self.need(caps::ADD_REMOVE, "remove monitors")?;
        self.ioctl(
            IOCTL_REMOVE_MONITOR,
            "remove monitor",
            Some(&RemoveMonitor { id }),
            None::<&mut ()>,
        )?;
        Ok(())
    }

    /// New modes for monitor `id`: the driver plugs it out and back in.
    pub fn set_modes(&self, id: u32, modes: &[Mode]) -> Result<Monitor> {
        self.need(caps::SET_MODES, "change modes")?;
        let mut req = SetModes {
            id,
            ..SetModes::default()
        };
        req.mode_count = copy_modes(&mut req.modes, modes)?;
        let mut out = Monitor::default();
        self.ioctl(IOCTL_SET_MODES, "set modes", Some(&req), Some(&mut out))?;
        Ok(out)
    }

    /// All monitors of the driver, whoever owns them.
    pub fn list(&self) -> Result<Vec<Monitor>> {
        self.need(caps::LIST_MONITORS, "list monitors")?;
        let mut out = vec![Monitor::default(); self.info.max_monitors.max(1) as usize];
        let bytes = std::mem::size_of_val(out.as_slice());
        let done = self.ioctl_raw(
            IOCTL_LIST_MONITORS,
            "list monitors",
            None,
            Some((out.as_mut_ptr() as *mut c_void, bytes)),
        )?;
        out.truncate(done as usize / size_of::<Monitor>());
        Ok(out)
    }

    /// The GPU that should render the monitors (a preference); zero: none.
    pub fn set_render_adapter(&self, luid: Luid) -> Result<()> {
        self.need(caps::RENDER_ADAPTER, "choose the render adapter")?;
        self.ioctl(
            IOCTL_SET_RENDER_ADAPTER,
            "set render adapter",
            Some(&SetRenderAdapter { luid }),
            None::<&mut ()>,
        )?;
        Ok(())
    }
}

fn copy_modes(to: &mut [Mode; MAX_MODES], modes: &[Mode]) -> Result<u32> {
    if modes.is_empty() || modes.len() > MAX_MODES {
        return Err(Error::Call {
            what: "modes",
            code: windows::Win32::Foundation::ERROR_INVALID_PARAMETER.0,
        });
    }
    to[..modes.len()].copy_from_slice(modes);
    Ok(modes.len() as u32)
}
