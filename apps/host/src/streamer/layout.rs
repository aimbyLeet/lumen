//! Which monitors are streamed, and where they are.

use anyhow::{Result, bail};
use input::Rect;

use crate::app::Args;
use crate::source::parse_size;

/// Where a monitor is: DXGI's adapter and output, and its place on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spot {
    pub adapter: u32,
    pub output: u32,
    pub rect: Rect,
}

/// The monitors to stream, of `outputs`: the primary one, the other real ones
/// by their number in Windows, then Lumen VDD's (`virtuals`: id, GDI name) in
/// the driver's order; `vdd::layout::MAX_MONITORS` at most (`protocol::MAX_STREAMS`
/// is the protocol's limit). A real one on another GPU than NVIDIA's cannot be
/// encoded: left out, or it would end the whole session.
fn pick_monitors<'a>(
    outputs: &'a [capture::OutputInfo],
    virtuals: &[(u8, String)],
) -> Vec<&'a capture::OutputInfo> {
    let Some(primary) = outputs.iter().find(|o| o.left == 0 && o.top == 0) else {
        return Vec::new();
    };
    let is_virtual = |o: &capture::OutputInfo| virtuals.iter().any(|(_, n)| *n == o.device_name);
    let mut real: Vec<&capture::OutputInfo> = outputs
        .iter()
        .filter(|o| o.device_name != primary.device_name && !is_virtual(o) && o.is_nvidia)
        .collect();
    real.sort_by_key(|o| vdd::display::display_number(&o.device_name));
    let mut made: Vec<&(u8, String)> = virtuals.iter().collect();
    made.sort_by_key(|(id, _)| *id);
    let made = made.into_iter().filter_map(|(_, name)| {
        outputs
            .iter()
            .find(|o| o.device_name == *name && o.device_name != primary.device_name)
    });
    std::iter::once(primary)
        .chain(real)
        .chain(made)
        .take(vdd::layout::MAX_MONITORS)
        .collect()
}

/// The monitors to stream, monitor 1 first: as given (`--adapter` /
/// `--output`: that one), else [`pick_monitors`].
pub(crate) fn desktop_layout(args: &Args) -> Vec<Spot> {
    let outputs = capture::list_outputs().unwrap_or_default();
    let spot = |o: &capture::OutputInfo| Spot {
        adapter: o.adapter_index,
        output: o.output_index,
        rect: Rect {
            left: o.left,
            top: o.top,
            width: o.width as i32,
            height: o.height as i32,
        },
    };
    if args.adapter.is_some() || args.output.is_some() {
        let (adapter, output) = (args.adapter.unwrap_or(0), args.output.unwrap_or(0));
        let found = outputs
            .iter()
            .find(|o| o.adapter_index == adapter && o.output_index == output);
        return vec![found.map_or(
            Spot {
                adapter,
                output,
                rect: Rect {
                    left: 0,
                    top: 0,
                    width: 0,
                    height: 0,
                },
            },
            spot,
        )];
    }
    let virtuals: Vec<(u8, String)> = vdd::display::active()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| Some((a.vdd_id?, a.gdi_name)))
        .collect();
    // No physical monitor (or as if): only Lumen VDD's. What else Windows
    // shows then — another driver's virtual display (ParsecVDA), a GPU's
    // output with no monitor behind it (1024×768) — is not streamed: none
    // of Lumen's up, the client is asked (the problem host, 2026-11-12).
    let outputs: Vec<capture::OutputInfo> = if !crate::headless::physical() {
        outputs
            .into_iter()
            .filter(|o| virtuals.iter().any(|(_, n)| *n == o.device_name))
            .collect()
    } else {
        outputs
    };
    // None at all (no monitor on the desktop): the session asks for one.
    pick_monitors(&outputs, &virtuals)
        .into_iter()
        .map(spot)
        .collect()
}

/// Synthetic monitors: `--size 3840x2160,2560x1440`, side by side from (0, 0).
pub(super) fn synthetic_layout(args: &Args) -> Result<Vec<Rect>> {
    let mut left = 0;
    let mut rects = Vec::new();
    for size in args.size.split(',') {
        let (w, h) = parse_size(size.trim())?;
        rects.push(Rect {
            left,
            top: 0,
            width: w as i32,
            height: h as i32,
        });
        left += w as i32;
    }
    if rects.is_empty() || rects.len() > usize::from(protocol::MAX_STREAMS) {
        bail!("--size: 1 to {} sizes", protocol::MAX_STREAMS);
    }
    Ok(rects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture::OutputInfo;

    fn output(name: &str, left: i32, is_nvidia: bool) -> OutputInfo {
        OutputInfo {
            adapter_index: u32::from(!is_nvidia),
            output_index: 0,
            adapter_name: String::new(),
            is_nvidia,
            device_name: name.to_string(),
            left,
            top: 0,
            width: 1920,
            height: 1080,
        }
    }

    fn names(picked: Vec<&OutputInfo>) -> Vec<&str> {
        picked.iter().map(|o| o.device_name.as_str()).collect()
    }

    fn vdd(list: &[(u8, &str)]) -> Vec<(u8, String)> {
        list.iter().map(|(i, n)| (*i, (*n).to_string())).collect()
    }

    #[test]
    fn every_real_monitor_goes_primary_first() {
        let o = [
            output("left", -1920, true),
            output("primary", 0, true),
            output("right", 1920, true),
        ];
        assert_eq!(names(pick_monitors(&o, &[])), ["primary", "left", "right"]);
    }

    #[test]
    fn three_monitors_at_most() {
        let o = [
            output("primary", 0, true),
            output("b", 1920, true),
            output("c", 3840, true),
            output("d", 5760, true),
            output("e", 7680, true),
        ];
        assert_eq!(names(pick_monitors(&o, &[])), ["primary", "b", "c"]);
    }

    #[test]
    fn windows_numbers_order_them_not_the_gpus_list() {
        let o = [
            output(r"\\.\DISPLAY12", 1920, true),
            output(r"\\.\DISPLAY3", 0, true),
            output(r"\\.\DISPLAY2", 3840, true),
            output(r"\\.\DISPLAY1", 5760, true),
            output(r"\\.\DISPLAY4", 7680, true),
        ];
        // The primary (number 3) first, then numbers 1 and 2.
        assert_eq!(
            names(pick_monitors(&o, &[])),
            [r"\\.\DISPLAY3", r"\\.\DISPLAY1", r"\\.\DISPLAY2"]
        );
    }

    #[test]
    fn real_monitors_before_virtual_ones_in_the_drivers_order() {
        let o = [
            output("primary", 0, true),
            output("v1", 1920, true),
            output("v0", 3840, true),
            output("real", 5760, true),
        ];
        let v = vdd(&[(1, "v1"), (0, "v0")]);
        assert_eq!(names(pick_monitors(&o, &v)), ["primary", "real", "v0"]);
        let o = [output("primary", 0, true), o[1].clone(), o[2].clone()];
        assert_eq!(names(pick_monitors(&o, &v)), ["primary", "v0", "v1"]);
    }

    #[test]
    fn a_real_monitor_on_another_gpu_is_left_out() {
        let o = [
            output("primary", 0, true),
            output("igpu", 1920, false),
            output("second", 3840, true),
        ];
        assert_eq!(names(pick_monitors(&o, &[])), ["primary", "second"]);
    }

    #[test]
    fn a_virtual_monitor_stays_whatever_its_adapter_says() {
        let o = [output("primary", 0, true), output("v0", 1920, false)];
        let v = vdd(&[(0, "v0")]);
        assert_eq!(names(pick_monitors(&o, &v)), ["primary", "v0"]);
    }

    #[test]
    fn a_virtual_primary_counts_once() {
        let o = [output("v0", 0, true), output("real", 1920, true)];
        let v = vdd(&[(0, "v0")]);
        assert_eq!(names(pick_monitors(&o, &v)), ["v0", "real"]);
    }

    #[test]
    fn no_primary_nothing() {
        assert!(pick_monitors(&[output("a", 100, true)], &[]).is_empty());
    }
}
