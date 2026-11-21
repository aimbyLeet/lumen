//! The `lumen-host displays ...` commands.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use vdd::display::{self, Target};
use vdd::layout::{self, ClientSet, Settings};

use super::VirtualMonitors;
use super::arrange::{Part, Plan, arrange};
use super::shape::Shape;

/// `lumen-host displays service [--set <file> | --reset]`.
pub fn ask_service(set: Option<&std::path::Path>, reset: bool) -> Result<()> {
    use desktop::hostsvc::{Request, call};
    let req = match (set, reset) {
        (Some(path), _) => {
            let text =
                std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
            Request::SetMonitors {
                settings: serde_json::from_str(&text).context("monitor settings")?,
            }
        }
        (None, true) => Request::ResetMonitors,
        (None, false) => Request::State,
    };
    let t = Instant::now();
    let r = call(&req);
    let took = t.elapsed();
    match r {
        Ok(state) => {
            println!("{}", serde_json::to_string_pretty(&state.monitors)?);
            println!("answered in {took:?}");
            Ok(())
        }
        Err(e) => bail!("{e} (after {took:?})"),
    }
}

/// `lumen-host displays info`.
pub fn print_info() -> Result<()> {
    if !vdd::device::installed() {
        bail!("Lumen VDD is not installed");
    }
    let m = VirtualMonitors::open()?;
    let i = m.device.info();
    println!(
        "Lumen VDD {} (contract {}.{}), caps {:#x}, up to {} monitors of {} modes, IddCx {:#x}",
        i.driver_version(),
        i.contract_major,
        i.contract_minor,
        i.caps,
        i.max_monitors,
        i.max_modes,
        i.iddcx_version
    );
    for mon in m.device.list().unwrap_or_default() {
        println!(
            "  id {}: state {}, {}, target {:08x}:{:08x}/{}, {} modes, first {}x{}@{}",
            mon.id,
            mon.state,
            if mon.owned() {
                "ours"
            } else {
                "another handle's"
            },
            mon.adapter.high,
            mon.adapter.low,
            mon.target_id,
            mon.mode_count,
            mon.modes()[0].width,
            mon.modes()[0].height,
            mon.modes()[0].refresh_hz
        );
    }
    Ok(())
}

/// `lumen-host displays list`: the desktop's monitors (no rights needed).
pub fn print_desktop() -> Result<()> {
    match display::available() {
        Ok(all) => {
            println!("monitors Windows has (on or off):");
            for a in all {
                println!(
                    "  {:<12} target {:08x}:{:08x}/{}  {}  monitor {}  adapter {}",
                    format!("{:?}", a.kind),
                    a.target.adapter.high,
                    a.target.adapter.low,
                    a.target.id,
                    a.name,
                    a.path,
                    a.adapter_path
                );
            }
        }
        Err(e) => println!("monitors Windows has: {e}"),
    }
    for a in display::active()? {
        println!(
            "{:<14} {:>5},{:<5} {}x{}  scale {}  target {:08x}:{:08x}/{}  {}",
            a.gdi_name,
            a.rect.x,
            a.rect.y,
            a.rect.width,
            a.rect.height,
            a.scale.map_or("?".into(), |s| format!("{s} %")),
            a.target.adapter.high,
            a.target.adapter.low,
            a.target.id,
            a.name
        );
    }
    Ok(())
}

/// A settings file: a client's own set (monitor 1 virtual too) or the
/// machine's settings.
fn read_plan_file(path: &std::path::Path) -> Result<Shape> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    Ok(match serde_json::from_str::<ClientSet>(&text) {
        Ok(c) => {
            c.rects()?;
            Shape::Client(c)
        }
        Err(_) => {
            let s: Settings = serde_json::from_str(&text).context("monitor settings")?;
            layout::check(&s)?;
            Shape::machine(&s)
        }
    })
}

/// Makes the monitors as the file says and arranges them.
fn hold_one(m: &mut VirtualMonitors, path: &std::path::Path) -> Result<()> {
    let shape = read_plan_file(path)?;
    let t = Instant::now();
    m.apply(&shape.wanted())?;
    println!(
        "{}: driver: {} monitor(s) in {:?}",
        path.display(),
        m.up().len(),
        t.elapsed()
    );
    let t = Instant::now();
    let plan = Plan {
        up: m.up().to_vec(),
        shape,
        park: false,
        wake: false,
    };
    for note in arrange(&plan, Part::Both)? {
        println!("{note}");
    }
    println!("arranged in {:?}", t.elapsed());
    print_desktop()
}

/// `lumen-host displays up <settings.json> [--then <settings.json>]`: the
/// monitors up and arranged; with `--then`, a few seconds later changed to
/// the second file's (the monitors that stay are not plugged out and in);
/// until Enter.
pub fn hold(settings: &std::path::Path, then: Option<&std::path::Path>) -> Result<()> {
    let mut m = VirtualMonitors::open()?;
    let first: Vec<Target> = {
        hold_one(&mut m, settings)?;
        m.up().iter().map(|u| u.target).collect()
    };
    if let Some(then) = then {
        std::thread::sleep(Duration::from_secs(3));
        hold_one(&mut m, then)?;
        let kept = m.up().iter().filter(|u| first.contains(&u.target)).count();
        println!("{kept} monitor(s) kept their target (not plugged out and in)");
    }
    println!("Enter: remove the monitors and exit");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    Ok(())
}
