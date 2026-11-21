# Quality bench

Answers "what does the capture → 4:2:0 → NVENC pipeline do to the picture, and what does it cost in bitrate and
latency". Use it when changing color math, shaders or encoder settings (`crates/color` is the source of truth; see
the code rules in `CLAUDE.md`). Needs FFmpeg in PATH for decoding.

## Running it

```powershell
cargo build --release

# 1. The test chart. Open it on the host full screen at 100 % scale
#    (a viewer without smoothing; the Windows scale does not matter — 1:1 pixels does).
target\release\lumen-patterns --width 2560 --height 1440

# 2. Which monitors there are
target\release\lumen-bench --list

# 3. Record: 10 seconds, HEVC, constant QP 16, check the shader against the reference
target\release\lumen-bench --adapter 0 --output 0 --codec hevc --rc cqp --qp 16 --seconds 10 --verify

# 4. Decode (FFmpeg gives the frames without any color conversion)
ffmpeg -i bench-out\stream.hevc -fps_mode passthrough -pix_fmt yuv420p10le -f rawvideo bench-out\decoded.yuv

# 5. Analyse
target\release\lumen-compare capture --dir bench-out
```

`--codec av1` (RTX 40xx and newer) writes `stream.ivf`, the FFmpeg command is the same. AV1 exists only in the bench
for comparison; the product uses HEVC Main10.

## What `lumen-compare capture` prints

Three lines for every saved frame:

| Line | Compared | Meaning |
|---|---|---|
| **A. 4:2:0 only** | original ↔ reference CPU conversion to 4:2:0 and back | The ceiling of the format. The codec cannot do better. |
| **B. 4:2:0 + codec** | original ↔ decoded stream | What the client really sees. B − A is the codec's share. |
| **C. GPU vs reference** | shader output ↔ reference codes | 0, or a few ±1 code differences (float32 vs f64). More means a shader bug. |

ΔE00: < 1 — indistinguishable, 1–2 — visible to a trained eye side by side, > 3 — visible at once.
Next to them: `de_NNNNNN.png` (heat map: black < 0.5, green < 1, yellow ≈ 2, red ≥ 3) and `decoded_NNNNNN.png`.

## Without a GPU

`lumen-compare image --png screenshot.png` shows the loss of 4:2:0 alone on any picture. Run it on screenshots of
real work (Photoshop, Resolve, Nuke with their UI): the fastest way to see whether 4:2:0 is acceptable for that content.

## Worth measuring

| Experiment | Why |
|---|---|
| `--qp 10 / 14 / 18 / 22` | quality (B) ↔ bitrate curve on static and moving content |
| `--codec hevc` vs `av1` | AV1's bitrate gain at equal ΔE |
| `--preset 1 / 4 / 7` | the quality of a preset against its encode time |
| `--rc cbr --mbps 50 / 100 / 150` | behaviour under a capped channel |
| static screen / scrolling / video in a window | bitrate and latency spread per scenario |

The final summary of `lumen-bench` splits the bitrate into frames with screen changes and static repeats — it shows
directly what the static refinement costs.

## Further checks of the chain

`docs/commands.md` lists the decoder, transport and client checks (`lumen-decode --reference`, `lumen-netsim`,
`lumen-client --play … --verify-every`).

## Updating the NVENC headers

`crates/nvenc-sys/scripts/regen.sh [tag]` regenerates the bindings from FFmpeg nv-codec-headers. Then compare the
`*_VER` constants in `crates/nvenc-sys/src/lib.rs` with the script's output.
