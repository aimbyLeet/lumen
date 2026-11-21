//! Windows' display configuration: where the virtual monitors sit on the
//! desktop (`SetDisplayConfig`) and their scale (per-monitor DPI).
//!
//! Both act on the desktop of the calling process's session: the service
//! (session 0) does this through a process in the console session. The scale
//! is stored per user (`HKCU`): it is set as the signed-in user.

use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, LUID};

use crate::contract::Luid;
use crate::device::{Error, Result};
use crate::layout::Rect;

/// A monitor as `DISPLAYCONFIG` sees it: its target (the driver's answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    pub adapter: Luid,
    pub id: u32,
}

/// An active monitor of the desktop.
#[derive(Clone, Debug)]
pub struct Active {
    pub target: Target,
    pub rect: Rect,
    /// `\\.\DISPLAYn`: DXGI's `DeviceName` of the output.
    pub gdi_name: String,
    /// What the monitor calls itself (EDID).
    pub name: String,
    /// Scale, %, as this user has it.
    pub scale: Option<u16>,
    /// The driver's id (0..) when this is a Lumen VDD monitor (its EDID).
    pub vdd_id: Option<u8>,
}

const MODE_IDX_INVALID: u32 = 0xffff_ffff;
const PATH_ACTIVE: u32 = 1;
/// Scales Windows steps through for per-monitor DPI, %.
const DPI_STEPS: &[u16] = &[100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];
/// Undocumented `DISPLAYCONFIG_DEVICE_INFO_TYPE`s of the DPI scale.
const GET_DPI_SCALE: i32 = -3;
const SET_DPI_SCALE: i32 = -4;

fn ours(l: LUID) -> Luid {
    Luid {
        low: l.LowPart,
        high: l.HighPart,
    }
}

fn failed(what: &'static str, code: u32) -> Error {
    Error::Call { what, code }
}

/// The active paths with their modes.
struct Config {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

impl Config {
    fn query() -> Result<Config> {
        Self::query_with(QDC_ONLY_ACTIVE_PATHS)
    }

    /// With `QDC_ALL_PATHS`: every source × target pair, active or not.
    fn query_with(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<Config> {
        loop {
            let (mut np, mut nm) = (0u32, 0u32);
            let r = unsafe { GetDisplayConfigBufferSizes(flags, &mut np, &mut nm) };
            if r != ERROR_SUCCESS {
                return Err(failed("GetDisplayConfigBufferSizes", r.0));
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let r = unsafe {
                QueryDisplayConfig(
                    flags,
                    &mut np,
                    paths.as_mut_ptr(),
                    &mut nm,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            // ERROR_INSUFFICIENT_BUFFER: a monitor came meanwhile.
            if r.0 == 122 {
                continue;
            }
            if r != ERROR_SUCCESS {
                return Err(failed("QueryDisplayConfig", r.0));
            }
            paths.truncate(np as usize);
            modes.truncate(nm as usize);
            return Ok(Config { paths, modes });
        }
    }

    fn target(p: &DISPLAYCONFIG_PATH_INFO) -> Target {
        Target {
            adapter: ours(p.targetInfo.adapterId),
            id: p.targetInfo.id,
        }
    }

    fn source_mode_index(&self, p: &DISPLAYCONFIG_PATH_INFO) -> Option<usize> {
        let i = unsafe { p.sourceInfo.Anonymous.modeInfoIdx } as usize;
        (i < self.modes.len() && self.modes[i].infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            .then_some(i)
    }

    fn rect(&self, p: &DISPLAYCONFIG_PATH_INFO) -> Option<Rect> {
        let m = unsafe { self.modes[self.source_mode_index(p)?].Anonymous.sourceMode };
        Some(Rect {
            x: m.position.x,
            y: m.position.y,
            width: m.width,
            height: m.height,
        })
    }

    fn find(&self, t: Target) -> Option<&DISPLAYCONFIG_PATH_INFO> {
        self.paths.iter().find(|p| Self::target(p) == t)
    }
}

fn wide_str(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn gdi_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    let mut n = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    n.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
        size: size_of_val(&n) as u32,
        adapterId: p.sourceInfo.adapterId,
        id: p.sourceInfo.id,
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } != 0 {
        return String::new();
    }
    wide_str(&n.viewGdiDeviceName)
}

/// Lumen VDD's EDID: manufacturer `LMN`, product code `0x4C00 + id`
/// (lumen-vdd `driver/edid.cpp`); Windows puts both into the monitor's device
/// path, `\\?\DISPLAY#LMN4C01#...`.
const VDD_PRODUCT_BASE: u16 = 0x4C00;

/// The driver's id of a monitor by its device path, if it is a Lumen VDD one.
fn vdd_id_of(device_path: &str) -> Option<u8> {
    let at = device_path.to_ascii_uppercase().find("#LMN")? + 4;
    let hex = device_path.get(at..at + 4)?;
    let product = u16::from_str_radix(hex, 16).ok()?;
    u8::try_from(product.checked_sub(VDD_PRODUCT_BASE)?)
        .ok()
        .filter(|&id| id < 16)
}

fn monitor_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    target_name(p).map_or_else(String::new, |n| wide_str(&n.monitorFriendlyDeviceName))
}

fn target_name(p: &DISPLAYCONFIG_PATH_INFO) -> Option<DISPLAYCONFIG_TARGET_DEVICE_NAME> {
    let mut n = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    n.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        size: size_of_val(&n) as u32,
        adapterId: p.targetInfo.adapterId,
        id: p.targetInfo.id,
    };
    (unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } == 0).then_some(n)
}

#[repr(C)]
struct DpiGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    min_rel: i32,
    cur_rel: i32,
    max_rel: i32,
}

#[repr(C)]
struct DpiSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    rel: i32,
}

/// The source's DPI steps relative to its recommended one: (min, cur, max).
/// Step 0 of [`DPI_STEPS`] is `-min`.
fn dpi_steps(p: &DISPLAYCONFIG_PATH_INFO) -> Option<(i32, i32, i32)> {
    let mut g = DpiGet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_TYPE(GET_DPI_SCALE),
            size: size_of::<DpiGet>() as u32,
            adapterId: p.sourceInfo.adapterId,
            id: p.sourceInfo.id,
        },
        min_rel: 0,
        cur_rel: 0,
        max_rel: 0,
    };
    (unsafe { DisplayConfigGetDeviceInfo(&mut g.header) } == 0)
        .then_some((g.min_rel, g.cur_rel, g.max_rel))
}

/// Scale % from relative steps; `None` beyond the table.
fn step_scale(min_rel: i32, rel: i32) -> Option<u16> {
    DPI_STEPS.get(usize::try_from(rel - min_rel).ok()?).copied()
}

fn scale_of(p: &DISPLAYCONFIG_PATH_INFO) -> Option<u16> {
    let (min, cur, _) = dpi_steps(p)?;
    step_scale(min, cur)
}

/// The active monitors of this session's desktop.
pub fn active() -> Result<Vec<Active>> {
    let c = Config::query()?;
    Ok(c.paths
        .iter()
        .filter_map(|p| {
            Some(Active {
                target: Config::target(p),
                rect: c.rect(p)?,
                gdi_name: gdi_name(p),
                name: monitor_name(p),
                scale: scale_of(p),
                vdd_id: target_name(p).and_then(|n| vdd_id_of(&wide_str(&n.monitorDevicePath))),
            })
        })
        .collect())
}

/// The number in `\\.\DISPLAYn`: Windows's order of its monitors (none: last).
pub fn display_number(gdi_name: &str) -> u32 {
    gdi_name
        .rsplit("DISPLAY")
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

/// The real monitors of this session's desktop (not Lumen VDD's): the primary
/// one, then the others in Windows's order.
pub fn real() -> Result<Vec<Active>> {
    let mut all: Vec<Active> = active()?
        .into_iter()
        .filter(|a| a.vdd_id.is_none())
        .collect();
    all.sort_by_key(|a| (a.rect.x != 0 || a.rect.y != 0, display_number(&a.gdi_name)));
    Ok(all)
}

/// The primary monitor (at (0, 0)) unless it is one of `virtuals`.
pub fn primary(virtual_adapter: Option<Luid>) -> Result<Active> {
    active()?
        .into_iter()
        .find(|a| a.rect.x == 0 && a.rect.y == 0 && Some(a.target.adapter) != virtual_adapter)
        .ok_or(Error::Layout(
            "the primary monitor is not a physical one (or there is none)".into(),
        ))
}

/// Targets Windows sees connected but keeps off.
pub fn inactive(targets: &[Target]) -> Result<Vec<Target>> {
    let active = Config::query()?;
    let all = Config::query_with(QDC_ALL_PATHS)?;
    Ok(targets
        .iter()
        .copied()
        .filter(|t| {
            active.find(*t).is_none()
                && all
                    .paths
                    .iter()
                    .any(|p| Config::target(p) == *t && p.targetInfo.targetAvailable.as_bool())
        })
        .collect())
}

/// Turns `targets` on (extends the desktop to them), each with a source of
/// its adapter that no active monitor uses; Windows picks modes and places.
pub fn activate(targets: &[Target]) -> Result<()> {
    let all = Config::query_with(QDC_ALL_PATHS)?;
    let Config { mut paths, modes } = Config::query()?;
    for t in targets {
        if paths.iter().any(|p| Config::target(p) == *t) {
            continue;
        }
        let free = |p: &&DISPLAYCONFIG_PATH_INFO| {
            !paths.iter().any(|a| {
                a.sourceInfo.adapterId == p.sourceInfo.adapterId
                    && a.sourceInfo.id == p.sourceInfo.id
            })
        };
        let mut p = *all
            .paths
            .iter()
            .filter(|p| Config::target(p) == *t && p.targetInfo.targetAvailable.as_bool())
            .find(free)
            .ok_or(Error::Layout(format!("monitor {}: no free source", t.id)))?;
        p.flags = PATH_ACTIVE;
        p.sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        p.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        paths.push(p);
    }
    let r = unsafe {
        SetDisplayConfig(
            Some(&paths),
            Some(&modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE | SDC_ALLOW_CHANGES,
        )
    };
    if r != 0 {
        return Err(failed("SetDisplayConfig (turn on)", r as u32));
    }
    Ok(())
}

/// Waits until all `targets` are active on the desktop. Ones Windows keeps
/// off (it does so with a monitor whose saved mode is gone: new modes of the
/// same monitor) are turned on after a while.
pub fn wait_active(targets: &[Target], timeout: std::time::Duration) -> Result<()> {
    let until = std::time::Instant::now() + timeout;
    let turn_on_at = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut turned_on = false;
    loop {
        let c = Config::query()?;
        if targets.iter().all(|t| c.find(*t).is_some()) {
            return Ok(());
        }
        if !turned_on && std::time::Instant::now() > turn_on_at {
            turned_on = true;
            let off = inactive(targets)?;
            if !off.is_empty() {
                activate(&off)?;
            }
        }
        if std::time::Instant::now() > until {
            return Err(Error::Layout(
                "Windows did not turn the virtual monitors on (are they disconnected in its \
                 display settings?)"
                    .into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Gives each target its resolution (width, height, Hz) where Windows chose
/// another: it turns a monitor on in the EDID's preferred mode (1920×1080
/// for Lumen VDD's own EDID), whatever the driver offers first. Each monitor
/// on its own: applying all at once (`ChangeDisplaySettingsEx(NULL)`) brings
/// back Windows's saved layout of every monitor, a physical one turned off
/// too. Returns whether anything changed.
pub fn set_resolutions(wanted: &[(Target, u32, u32, u32)]) -> Result<bool> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::core::PCWSTR;
    let c = Config::query()?;
    let mut changed = false;
    for &(t, width, height, hz) in wanted {
        let p = c
            .find(t)
            .ok_or(Error::Layout(format!("monitor {} is not active", t.id)))?;
        let rect = c
            .rect(p)
            .ok_or(Error::Layout(format!("monitor {}: no source mode", t.id)))?;
        if (rect.width, rect.height) == (width, height) {
            continue;
        }
        let name: Vec<u16> = gdi_name(p).encode_utf16().chain([0]).collect();
        let dm = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY,
            dmPelsWidth: width,
            dmPelsHeight: height,
            dmDisplayFrequency: hz,
            ..Default::default()
        };
        let r = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(name.as_ptr()),
                Some(&dm),
                None,
                CDS_UPDATEREGISTRY,
                None,
            )
        };
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(Error::Layout(format!(
                "monitor {}: Windows refused {width}x{height}@{hz} ({})",
                t.id, r.0
            )));
        }
        changed = true;
    }
    Ok(changed)
}

/// What a monitor is, for the service's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On a real adapter: a GPU's port (a monitor or a dummy plug in it), a
    /// USB dock.
    Physical,
    /// Lumen VDD's, with the driver's id.
    Lumen(u8),
    /// Another virtual display driver's (Parsec VDD, MTT VDD, SudoVDA…): its
    /// adapter is software, `ROOT\…` or `SWD\…`.
    Virtual,
    /// On a real adapter but no monitor behind it: no monitor device at all
    /// (a GPU's output Windows keeps a 1024×768 desktop on when it has no
    /// display — seen on a host without monitors while Parsec swapped its
    /// own), or Windows's `DEFAULT_MONITOR` (no EDID). Nothing draws on it,
    /// so it is not a physical monitor (decision 2026-11-12). A monitor whose
    /// EDID only lacks a name is physical.
    NoEdid,
}

/// A monitor's kind by its device path, its name (EDID) and its adapter's
/// path (empty: unknown, taken as physical, as before kinds were told apart).
/// No device path and no name: a GPU's bare output. A path missing but a
/// name there is a physical monitor coming back (Windows says so for a
/// moment as it reconnects, seen 2026-11-12): still physical, else Lumen
/// would put its own up for that moment.
pub fn kind_of(monitor_path: &str, name: &str, adapter_path: &str) -> Kind {
    if let Some(id) = vdd_id_of(monitor_path) {
        return Kind::Lumen(id);
    }
    let bus = bus_of(adapter_path);
    if bus.eq_ignore_ascii_case("ROOT") || bus.eq_ignore_ascii_case("SWD") {
        Kind::Virtual
    } else if (monitor_path.trim().is_empty() && name.trim().is_empty())
        || monitor_path
            .to_ascii_uppercase()
            .contains("#DEFAULT_MONITOR#")
    {
        Kind::NoEdid
    } else {
        Kind::Physical
    }
}

/// The bus an adapter's device path starts with: `PCI`, `USB`, `ROOT`, `SWD`…
pub fn bus_of(adapter_path: &str) -> &str {
    adapter_path
        .strip_prefix(r"\\?\")
        .unwrap_or(adapter_path)
        .split(['#', '\\'])
        .next()
        .unwrap_or("")
}

/// The device path of a target's adapter (empty when Windows does not say).
fn adapter_path(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    let mut n = DISPLAYCONFIG_ADAPTER_NAME::default();
    n.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME,
        size: size_of_val(&n) as u32,
        adapterId: p.targetInfo.adapterId,
        id: 0,
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } != 0 {
        return String::new();
    }
    wide_str(&n.adapterDevicePath)
}

/// A monitor Windows has, on or off.
#[derive(Clone, Debug)]
pub struct Available {
    pub target: Target,
    pub kind: Kind,
    /// What the monitor calls itself (EDID).
    pub name: String,
    /// The monitor's device path (`\\?\DISPLAY#<EDID id>#…`).
    pub path: String,
    pub adapter_path: String,
}

/// The monitors Windows lists as available, each once: a monitor turned off
/// by [`keep_only`] is still here; one switched off by its button may or may
/// not be, as its cable tells Windows.
pub fn available() -> Result<Vec<Available>> {
    let all = Config::query_with(QDC_ALL_PATHS)?;
    let mut out: Vec<Available> = Vec::new();
    for p in &all.paths {
        if !p.targetInfo.targetAvailable.as_bool() {
            continue;
        }
        let Some(n) = target_name(p) else { continue };
        let t = Config::target(p);
        if out.iter().any(|o| o.target == t) {
            continue;
        }
        let adapter_path = adapter_path(p);
        let path = wide_str(&n.monitorDevicePath);
        let name = wide_str(&n.monitorFriendlyDeviceName);
        out.push(Available {
            target: t,
            kind: kind_of(&path, &name, &adapter_path),
            name,
            path,
            adapter_path,
        });
    }
    Ok(out)
}

/// Whether a physical monitor is plugged in, on or off (see [`available`]).
/// Other virtual display drivers' monitors and Windows's generic monitor (a
/// port without EDID) are not physical ones.
pub fn physical_connected() -> Result<bool> {
    Ok(available()?.iter().any(|a| a.kind == Kind::Physical))
}

/// Puts each target at its rect (pixels; the primary stays at (0, 0)) and
/// saves it as Windows' layout for this set of monitors.
pub fn arrange(placed: &[(Target, Rect)]) -> Result<()> {
    let mut c = Config::query()?;
    for (t, rect) in placed {
        let pi = c
            .paths
            .iter()
            .position(|p| Config::target(p) == *t)
            .ok_or(Error::Layout(format!("monitor {} is not active", t.id)))?;
        let p = c.paths[pi];
        let mi = c
            .source_mode_index(&p)
            .ok_or(Error::Layout(format!("monitor {}: no source mode", t.id)))?;
        let m = unsafe { &mut c.modes[mi].Anonymous.sourceMode };
        m.position.x = rect.x;
        m.position.y = rect.y;
    }
    let r = unsafe {
        SetDisplayConfig(
            Some(&c.paths),
            Some(&c.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE | SDC_ALLOW_CHANGES,
        )
    };
    if r != 0 {
        return Err(failed("SetDisplayConfig", r as u32));
    }
    Ok(())
}

/// Sets the target's scale for this user; returns the scale it has now
/// (the nearest Windows allows).
pub fn set_scale(target: Target, percent: u16) -> Result<u16> {
    let c = Config::query()?;
    let p = *c.find(target).ok_or(Error::Layout(format!(
        "monitor {} is not active",
        target.id
    )))?;
    let (min, _, max) = dpi_steps(&p).ok_or(Error::Layout("no DPI steps".into()))?;
    let want = DPI_STEPS
        .iter()
        .position(|&s| s >= percent)
        .unwrap_or(DPI_STEPS.len() - 1) as i32;
    let rel = (want + min).clamp(min, max);
    let s = DpiSet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_TYPE(SET_DPI_SCALE),
            size: size_of::<DpiSet>() as u32,
            adapterId: p.sourceInfo.adapterId,
            id: p.sourceInfo.id,
        },
        rel,
    };
    let r = unsafe { DisplayConfigSetDeviceInfo(&s.header) };
    if r != 0 {
        return Err(failed("set DPI scale", r as u32));
    }
    let now = Config::query()?;
    Ok(now.find(target).and_then(scale_of).unwrap_or(0))
}

/// The modes Windows offers for `\.\DISPLAYn` (`EnumDisplaySettings`):
/// (width, height, Hz), each once, in Windows's order.
pub fn offered(gdi_name: &str) -> Vec<(u32, u32, u32)> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::core::PCWSTR;
    let name: Vec<u16> = gdi_name.encode_utf16().chain([0]).collect();
    let mut out = Vec::new();
    for i in 0.. {
        let mut dm = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if !unsafe {
            EnumDisplaySettingsW(
                PCWSTR(name.as_ptr()),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut dm,
            )
        }
        .as_bool()
        {
            break;
        }
        let m = (dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency);
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Turns every active monitor but `keep` off for now (not saved: Windows
/// goes back to its saved layout when the set of monitors changes). The
/// first of `keep` becomes the primary one, the others keep their place
/// relative to it.
pub fn keep_only(keep: &[Target]) -> Result<()> {
    let c = Config::query()?;
    let first = keep
        .first()
        .ok_or(Error::Layout("nothing to keep".into()))?;
    let origin = c
        .find(*first)
        .and_then(|p| c.rect(p))
        .ok_or(Error::Layout(format!("monitor {} is not active", first.id)))?;
    let Config { paths, mut modes } = c;
    let paths: Vec<_> = paths
        .into_iter()
        .filter(|p| keep.contains(&Config::target(p)))
        .collect();
    for p in &paths {
        let i = unsafe { p.sourceInfo.Anonymous.modeInfoIdx } as usize;
        if i < modes.len() && modes[i].infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            let m = unsafe { &mut modes[i].Anonymous.sourceMode };
            m.position.x -= origin.x;
            m.position.y -= origin.y;
        }
    }
    let r = unsafe {
        SetDisplayConfig(
            Some(&paths),
            Some(&modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    };
    if r != 0 {
        return Err(failed("SetDisplayConfig (keep only)", r as u32));
    }
    Ok(())
}

/// After a session on a host without a physical monitor: Lumen's monitors
/// `ours` turned off, but kept in the driver (no unplugging: no sound, no
/// waiting next time), and another virtual display driver's (Parsec's
/// fallback display) on again. With no such display ours stay on: Windows
/// would be left with a GPU's bare output. Returns whether ours went off.
pub fn park(ours: &[Target]) -> Result<bool> {
    let others: Vec<Target> = available()?
        .into_iter()
        .filter(|a| a.kind == Kind::Virtual && !ours.contains(&a.target))
        .map(|a| a.target)
        .collect();
    if others.is_empty() {
        return Ok(false);
    }
    let off = inactive(&others)?;
    if !off.is_empty() {
        activate(&off)?;
    }
    keep_only(&others)?;
    Ok(true)
}

/// Windows's own "extend" topology from its saved layouts: turns back on
/// what [`keep_only`] turned off.
pub fn extend() -> Result<()> {
    let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_TOPOLOGY_EXTEND) };
    if r != 0 {
        return Err(failed("SetDisplayConfig (extend)", r as u32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_are_told_apart_by_their_adapter() {
        let gpu = r"\\?\PCI#VEN_10DE&DEV_1B06&SUBSYS_120F10DE&REV_A1#4&2283f625&0&0019#{5b45201d-f2f2-4f3b-85bb-30ff1f953599}";
        let dock = r"\\?\USB#VID_17E9&PID_6006&MI_00#7&1a2b3c4d&0&0000#{5b45201d-f2f2-4f3b-85bb-30ff1f953599}";
        let root = r"\\?\ROOT#DISPLAY#0000#{5b45201d-f2f2-4f3b-85bb-30ff1f953599}";
        let swd = r"\\?\SWD#RemoteDisplayEnum#RdpIdd_IndirectDisplay&SessionId_0002#{5b45201d-f2f2-4f3b-85bb-30ff1f953599}";
        let samsung = r"\\?\DISPLAY#SAM7303#5&x&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        let foreign = r"\\?\DISPLAY#XYZ0001#1&x&0&UID0#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        let lumen = r"\\?\DISPLAY#LMN4C02#1&x&0&UID2#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        const NAMED: &str = "Odyssey G7";
        let generic =
            r"\\?\DISPLAY#Default_Monitor#1&x&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(kind_of(samsung, NAMED, gpu), Kind::Physical);
        assert_eq!(kind_of(samsung, NAMED, dock), Kind::Physical);
        assert_eq!(kind_of(foreign, NAMED, root), Kind::Virtual);
        assert_eq!(
            kind_of(foreign, NAMED, &root.to_ascii_lowercase()),
            Kind::Virtual
        );
        assert_eq!(kind_of(samsung, NAMED, swd), Kind::Virtual);
        assert_eq!(kind_of(lumen, NAMED, root), Kind::Lumen(2));
        // A GPU's port without EDID (the problem host, 2026-11-12).
        assert_eq!(kind_of(generic, NAMED, gpu), Kind::NoEdid);
        assert_eq!(
            kind_of(&generic.to_ascii_lowercase(), NAMED, gpu),
            Kind::NoEdid
        );
        assert_eq!(kind_of(generic, NAMED, root), Kind::Virtual);
        // A GPU's output with no monitor device (the problem host, 0.7.9:
        // `Physical «» (PCI, )`, a 1024×768 desktop).
        assert_eq!(kind_of("", "", gpu), Kind::NoEdid);
        // A physical monitor coming back: its name, no path yet.
        assert_eq!(kind_of("", "Odyssey G7", gpu), Kind::Physical);
        assert_eq!(kind_of("", NAMED, root), Kind::Virtual);
        // Windows did not say: as before, physical.
        assert_eq!(kind_of(samsung, NAMED, ""), Kind::Physical);
        assert_eq!(kind_of(samsung, NAMED, r"ROOT\DISPLAY\0000"), Kind::Virtual);
        assert_eq!(bus_of(gpu), "PCI");
        assert_eq!(bus_of(""), "");
    }

    #[test]
    fn lumen_vdd_monitors_are_known_by_their_edid() {
        let path = r"\\?\DISPLAY#LMN4C01#5&2b7ad4b&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(vdd_id_of(path), Some(1));
        assert_eq!(vdd_id_of(r"\\?\display#lmn4c00#x"), Some(0));
        assert_eq!(vdd_id_of(r"\\?\DISPLAY#SAM7303#5&x"), None);
        assert_eq!(display_number(r"\\.\DISPLAY12"), 12);
        assert_eq!(display_number(r"\\.\DISPLAY2"), 2);
        assert_eq!(display_number("other"), u32::MAX);
        assert_eq!(vdd_id_of(r"\\?\DISPLAY#LMN1234#x"), None);
        assert_eq!(vdd_id_of(r"\\?\DISPLAY#LMN4C"), None);
    }

    #[test]
    fn dpi_steps_are_relative_to_the_recommended_one() {
        // Recommended 150 % (index 2): min = -2.
        assert_eq!(step_scale(-2, 0), Some(150));
        assert_eq!(step_scale(-2, -2), Some(100));
        assert_eq!(step_scale(-2, 1), Some(175));
        assert_eq!(step_scale(0, 11), Some(500));
        assert_eq!(step_scale(0, 12), None);
        assert_eq!(step_scale(0, -1), None);
    }

    /// Reads this desktop's monitors (no change).
    #[test]
    fn this_desktop_has_monitors() {
        let all = active().unwrap();
        assert!(!all.is_empty());
        assert!(
            all.iter().any(|a| a.rect.x == 0 && a.rect.y == 0),
            "{all:?}"
        );
        for a in &all {
            assert!(a.gdi_name.starts_with(r"\\.\DISPLAY"), "{a:?}");
            assert!(a.scale.is_some(), "{a:?}");
        }
    }
}
