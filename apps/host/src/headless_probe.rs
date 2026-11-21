//! `lumen-host displays probe`: step 0 of the headless-host work, run once by
//! hand from an administrator's console on a free screen.
//!
//! A. A Lumen VDD monitor with id 2 and every resolution of the list: does
//!    Windows offer them all, and does each one take with
//!    `ChangeDisplaySettingsEx` alone (no reconnect)?
//! B. SET_MODES as it failed earlier: what Windows offers
//!    after each one.
//! C. The physical monitor turned off for now: only virtual ones; are they on
//!    the NVIDIA GPU, does Desktop Duplication give a picture, before and
//!    after a resolution change, with three of them?
//!
//! Then everything goes back: the monitors are removed (closing the driver's
//! handle removes them anyway, also if this process dies) and Windows's
//! saved layout brings the physical monitor back.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use capture::{Acquired, DesktopDuplicator};
use vdd::contract::Mode;
use vdd::device::Device;
use vdd::display::{self, Target};
use vdd::layout::{REFRESH_HZ, RESOLUTIONS};

const ACTIVE_WITHIN: Duration = Duration::from_secs(10);

struct Out {
    file: File,
    t0: Instant,
}

impl Out {
    fn line(&mut self, s: impl AsRef<str>) {
        let l = format!("{:>7.2} {}", self.t0.elapsed().as_secs_f64(), s.as_ref());
        println!("{l}");
        let _ = writeln!(self.file, "{l}");
    }

    fn desktop(&mut self, what: &str) {
        self.line(format!("desktop ({what}):"));
        match display::active() {
            Ok(all) => {
                for a in all {
                    self.line(format!(
                        "  {:<14} {:>5},{:<5} {}x{}  vdd {:?}  target {}  {}",
                        a.gdi_name,
                        a.rect.x,
                        a.rect.y,
                        a.rect.width,
                        a.rect.height,
                        a.vdd_id,
                        a.target.id,
                        a.name
                    ))
                }
            }
            Err(e) => self.line(format!("  error: {e}")),
        }
    }
}

fn mode(width: u32, height: u32) -> Mode {
    Mode {
        width,
        height,
        refresh_hz: REFRESH_HZ,
    }
}

/// `first`, then every other resolution of the list.
fn all_modes(width: u32, height: u32) -> Vec<Mode> {
    std::iter::once(mode(width, height))
        .chain(
            RESOLUTIONS
                .iter()
                .filter(|r| (r.width, r.height) != (width, height))
                .map(|r| mode(r.width, r.height)),
        )
        .collect()
}

fn gdi_of(t: Target) -> Result<String> {
    display::active()?
        .into_iter()
        .find(|a| a.target == t)
        .map(|a| a.gdi_name)
        .context("the monitor is not active")
}

fn size_of_target(t: Target) -> Option<(u32, u32)> {
    display::active()
        .ok()?
        .into_iter()
        .find(|a| a.target == t)
        .map(|a| (a.rect.width, a.rect.height))
}

fn offered(o: &mut Out, t: Target) -> Result<()> {
    let name = gdi_of(t)?;
    let modes: Vec<String> = display::offered(&name)
        .into_iter()
        .filter(|m| m.2 == REFRESH_HZ)
        .map(|(w, h, _)| format!("{w}x{h}"))
        .collect();
    o.line(format!(
        "  Windows offers {} at {REFRESH_HZ} Hz ({} modes): {}",
        name,
        modes.len(),
        modes.join(" ")
    ));
    Ok(())
}

fn added(o: &mut Out, what: &str, a: vdd::contract::MonitorAdded) -> Result<Target> {
    let t = Target {
        adapter: a.adapter,
        id: a.target_id,
    };
    let at = Instant::now();
    display::wait_active(&[t], ACTIVE_WITHIN)?;
    o.line(format!(
        "{what}: target {}, active in {} ms, {:?}",
        t.id,
        at.elapsed().as_millis(),
        size_of_target(t)
    ));
    Ok(t)
}

/// Sets `t` to width × height and says whether it took.
fn set_size(o: &mut Out, t: Target, width: u32, height: u32) -> bool {
    let at = Instant::now();
    let r = display::set_resolutions(&[(t, width, height, REFRESH_HZ)]);
    let got = size_of_target(t);
    let ok = r.is_ok() && got == Some((width, height));
    o.line(format!(
        "  {width}x{height}: {} in {} ms, now {:?}{}",
        if ok { "ok" } else { "FAILED" },
        at.elapsed().as_millis(),
        got,
        r.err().map_or(String::new(), |e| format!(", {e}"))
    ));
    ok
}

/// Duplicates the monitor `gdi` for `secs`: frames, and how much of the
/// picture is not black.
fn grab(o: &mut Out, gdi: &str, secs: f64) -> Result<()> {
    let outs = capture::list_outputs()?;
    let Some(oi) = outs.iter().find(|x| x.device_name == gdi) else {
        bail!("DXGI does not list {gdi}");
    };
    o.line(format!(
        "  capture {gdi}: adapter {} [{}], nvidia {}, {}x{} at {},{}",
        oi.adapter_index, oi.adapter_name, oi.is_nvidia, oi.width, oi.height, oi.left, oi.top
    ));
    let mut d = DesktopDuplicator::new(oi.adapter_index, oi.output_index)?;
    let tex = d.create_frame_texture()?;
    let (mut frames, mut pointer, mut first) = (0u32, 0u32, None);
    let until = Instant::now() + Duration::from_secs_f64(secs);
    let at = Instant::now();
    while Instant::now() < until {
        match d.acquire_into(100, &tex)? {
            Acquired::NewFrame { .. } => {
                frames += 1;
                first.get_or_insert(at.elapsed());
            }
            Acquired::PointerOnly => pointer += 1,
            Acquired::Timeout => {}
        }
    }
    let lit = if frames > 0 {
        let px = capture::read_bgra(d.gpu(), &tex)?;
        let n = px.len() / 4;
        let lit = px
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
            .count();
        format!("{:.1} % not black", 100.0 * lit as f64 / n.max(1) as f64)
    } else {
        "no picture".into()
    };
    o.line(format!(
        "  {frames} frame(s), first after {:?}, {pointer} pointer-only; {lit}",
        first.map(|d| d.as_millis())
    ));
    Ok(())
}

fn phases(o: &mut Out, dev: &Device) -> Result<()> {
    let info = dev.info();
    o.line(format!(
        "driver {}, contract {}.{}, caps {:#x}, max monitors {}, max modes {}",
        info.driver_version(),
        info.contract_major,
        info.contract_minor,
        info.caps,
        info.max_monitors,
        info.max_modes
    ));
    for m in dev.list()? {
        o.line(format!("  already there: id {} (state {})", m.id, m.state));
    }

    o.line("== A: id 2 with every resolution, changed in place");
    let all = all_modes(2560, 1440);
    let a = added(o, "id 2 added", dev.add(2, &all, None)?)?;
    offered(o, a)?;
    let mut failed = 0;
    for r in RESOLUTIONS.iter().chain([&RESOLUTIONS[1]]) {
        if !set_size(o, a, r.width, r.height) {
            failed += 1;
        }
    }
    let still = dev.list()?.into_iter().find(|m| m.id == 2);
    o.line(format!(
        "A: {failed} failed; the driver's monitor 2 now: target {:?} (same: {})",
        still.map(|m| m.target_id),
        still.is_some_and(|m| m.target_id == a.id)
    ));

    o.line("== B: SET_MODES on id 3");
    let b = added(
        o,
        "id 3 added, 1920x1080",
        dev.add(3, &[mode(1920, 1080)], None)?,
    )?;
    offered(o, b)?;
    for (w, h, modes) in [
        (2560, 1440, vec![mode(2560, 1440), mode(1920, 1080)]),
        (3440, 1440, vec![mode(3440, 1440), mode(2560, 1080)]),
        (3840, 2160, all_modes(3840, 2160)),
    ] {
        let at = Instant::now();
        match dev.set_modes(3, &modes) {
            Ok(m) => {
                let t = Target {
                    adapter: m.adapter,
                    id: m.target_id,
                };
                let active = display::wait_active(&[t], ACTIVE_WITHIN);
                o.line(format!(
                    "set_modes {w}x{h} (+{}): {} ms, target {}, active {:?}, size {:?}",
                    modes.len() - 1,
                    at.elapsed().as_millis(),
                    t.id,
                    active.as_ref().map(|_| ()).map_err(|e| e.to_string()),
                    size_of_target(t)
                ));
                if active.is_ok() {
                    offered(o, t)?;
                    set_size(o, t, w, h);
                }
            }
            Err(e) => o.line(format!("set_modes {w}x{h}: {e}")),
        }
    }
    dev.remove(3)?;
    std::thread::sleep(Duration::from_secs(1));

    o.line("== C: no physical monitor");
    set_size(o, a, 2560, 1440);
    let gdi = gdi_of(a)?;
    o.line("  with the physical monitor:");
    grab(o, &gdi, 1.0)?;
    let at = Instant::now();
    display::keep_only(&[a])?;
    o.line(format!("physical off in {} ms", at.elapsed().as_millis()));
    std::thread::sleep(Duration::from_secs(2));
    o.desktop("only the virtual one");
    let gdi = gdi_of(a)?;
    grab(o, &gdi, 2.0)?;
    o.line("  resolution of the only monitor:");
    set_size(o, a, 3440, 1440);
    std::thread::sleep(Duration::from_secs(1));
    grab(o, &gdi_of(a)?, 2.0)?;
    o.line("  three virtual monitors:");
    let m0 = added(o, "id 0 added", dev.add(0, &all_modes(1920, 1080), None)?)?;
    let m1 = added(o, "id 1 added", dev.add(1, &all_modes(1920, 1200), None)?)?;
    // A new monitor makes Windows pick a layout: the physical one may be on again.
    display::keep_only(&[a, m0, m1])?;
    std::thread::sleep(Duration::from_secs(1));
    o.desktop("three virtual");
    for t in [a, m0, m1] {
        grab(o, &gdi_of(t)?, 1.5)?;
    }
    Ok(())
}

/// Removes our monitors and waits for a physical monitor on the desktop;
/// asks Windows for its "extend" layout if none comes.
fn restore(o: &mut Out, dev: Device, physical: &[Target]) {
    o.line("== restore");
    for id in [0, 1, 3, 2] {
        let _ = dev.remove(id);
    }
    drop(dev);
    let back = |within: Duration| {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if display::active().is_ok_and(|a| a.iter().any(|a| physical.contains(&a.target))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    };
    let at = Instant::now();
    if back(Duration::from_secs(5)) {
        o.line(format!(
            "the physical monitor is back by itself in {} ms",
            at.elapsed().as_millis()
        ));
    } else {
        o.line(format!(
            "not back by itself; extend: {:?}",
            display::extend()
        ));
        o.line(format!("back: {}", back(Duration::from_secs(5))));
    }
    o.desktop("after");
}

pub fn run(log: &Path) -> Result<()> {
    capture::init_process();
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = Out {
        file: File::create(log).with_context(|| format!("{}", log.display()))?,
        t0: Instant::now(),
    };
    o.desktop("before");
    let dev = Device::open()?;
    // Right after a monitor goes, Windows still lists it for a moment, as
    // not ours: the physical ones are told by their targets.
    let physical: Vec<Target> = display::real()?.iter().map(|a| a.target).collect();
    let r = phases(&mut o, &dev);
    if let Err(e) = &r {
        o.line(format!("PROBE FAILED: {e:#}"));
    }
    restore(&mut o, dev, &physical);
    o.line(format!("log: {}", log.display()));
    r
}
