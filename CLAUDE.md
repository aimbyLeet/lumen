# CLAUDE.md — Lumen

Open source remote access to work machines for small studios and teams. The goal is **stable and comfortable remote
work**; color matters, but not at a color-grading level. Host and client: Windows 10/11 + NVIDIA. Server: a VPS in
Docker or a machine on the local network. Code, identifiers, comments and agent documents are in English; the apps'
UI is English and Russian.

If `CLAUDE.local.md` exists next to this file, it holds the maintainer's personal working rules and the current state
of the work — read it too; it overrides this file where they differ.

**Where to look** (do not read everything):
- `docs/MAP.md` — **the project map**: every crate, app, module and script in one line; where to start per topic.
- `docs/architecture.md` — how the pieces fit: the pipeline, the threads, the network, the server.
- `docs/decisions.md` — the design decisions in full; `docs/pitfalls.md` — the known traps in full (read by topic).
- `docs/commands.md` — all commands; `docs/quality-bench.md` — the color/codec quality bench.
- `manual/manual_EN.html`, `manual/manual_RU.html` — the user manual (install, server, admin panel, host, client).
- `README.md` — the public overview (English, then Russian).

## Design decisions — summary (do not change without the maintainer's consent; full text in `docs/decisions.md`)

- **Platform:** Windows 10/11 + NVIDIA. **Format:** 4:2:0, 10 bit, BT.709, full range, chroma siting "left"; no
  EDID/ICC, no 4:4:4. **Codec:** HEVC Main10 (no AV1 in the product); no B-frames, infinite GOP. No HDR.
- **Network:** the user's WireGuard or Tailscale, direct connection; no relay, NAT traversal, STUN or UPnP of our own.
- **Server:** a VPS behind nginx (signaling TLS 7443, certbot certificate read-only; admin `127.0.0.1:7480` → `/admin/`;
  the landing page at the root) or a local network (`self-cert`, trust by fingerprint after a confirmation window).
  Video never goes through the server. `deploy/server/install.sh` installs and updates it; no backups.
- **Access:** devices register themselves; host = machine (one token), client = installation; first access is a
  request an administrator approves; "Unlink" ends the session. There is no direct mode without a server.
- **Security:** E2E Noise `IKpsk2` (`snow`), the `psk` comes from the host's token, the server never knows the token;
  the host accepts only sessions announced by the server. Device ↔ server: TLS (`rustls`).
- **Windows service:** the host is a service (LocalSystem), the worker process runs as SYSTEM in the console session;
  one session per machine; `Ctrl+Alt+Shift+Del` → Ctrl+Alt+Del. After a session Windows locks (window closed — at
  once, connection lost — after 10 s), unless `keep_unlocked`.
- **Clipboard:** text both ways, up to 1 MB; files and pictures only by Ctrl+C → Ctrl+V (≤ 32 MB as clipboard, larger —
  into `Downloads\Lumen`); the switch and the speed cap are in the admin panel.
- **Monitors:** virtual monitors through Lumen VDD (prebuilt in `third_party/lumen-vdd`, source
  https://github.com/aimbyLeet/lumen-vdd); a client gets at most 3 monitors; hosts without monitors are supported.
- **UI:** React; Lumen Host and Lumen Client are Tauri v2 apps (identifiers `io.github.aimbyleet.lumen.host` /
  `.client`), the admin panel is a web app; the video window is native Win32. The core returns `err_…` / `note_…`
  codes, the words live in `web/client/src/errors.ts`. The client draws the cursor. Languages: English and Russian
  (globe button).
- **Quality:** 30/60 fps and a bitrate cap (Auto or 10…100 Mbit/s), set by the client and the admin; the cap is static,
  no automatic degradation. Losses → RFI instead of IDR. NVENC P1; static content → QP map.
- **Versions:** one version for the whole project (`Cargo.toml` workspace); a client connects only to a host of the
  same version. Auto-update from the user's server; a client has one server.
- **Release — 2 files:** `dist\lumen-setup-<version>.exe` and `dist\lumen-server-<version>.tar.gz` (build after the
  installer). The manual lives only in the repository.

## How to work

- **Iteratively, step by step.** Before a step: what is done and a measurable acceptance criterion. A step is done
  only when checked. Commit after each step; do not push, branch or merge unless asked.
- Before larger architectural changes, propose a short plan and wait for an answer.
- Measurements (ΔE, bitrate, latency) — in numbers. Say honestly what was checked on hardware and what was only
  compiled; two-machine checks need a human.
- Administrator rights (UAC, installing the service) belong to the human. Tests that move the real mouse, screenshots
  and blanking the monitor (`no-physical.flag`) — only on a free screen; use `tools\check.ps1 -NoMouse` otherwise.
- Refactoring: after a feature, as a separate pass; UI mock snapshots before and after compared byte for byte
  (`tools/overlay-check/snapshots.mjs`).

## Code rules

- **`crates/color` is the source of truth.** Formulas change there first, with tests, then in the HLSL in sync
  (host `bgra_to_p010.hlsl`, client `yuv420_to_rgb10.hlsl`); check with `lumen-bench --verify` + `lumen-compare`.
- Windows code — `#[cfg(any(windows, lumen_check))]`; `color`, `protocol`, `compare`, `patterns` do not depend on
  `windows`.
- `nvenc-sys/src/bindings.rs`, `guids.rs`, `nvdec-sys/src/bindings.rs` are **generated**, never edit by hand
  (`crates/*-sys/scripts/regen.sh`).
- `unsafe` only in the thin layers (`capture`, `gpu-convert`, `nvenc`, `decoder`); safe APIs outward. Win32 for
  windows and services lives in the apps' own modules (`window.rs`, `ui_api.rs`, `shortcuts.rs`, `winsec.rs`,
  `service.rs`); `HANDLE` ownership — `desktop::Handle`.
- Errors: `anyhow` in apps, typed errors in libraries. A new error code needs its words in `web/client/src/errors.ts`
  (English and Russian), otherwise `npm test` fails.
- No CPU copies of frames on the hot path. New dependencies only with a clear benefit, licenses MIT/Apache-2.0 (or
  compatible permissive). The project itself is MIT.
- `third_party/lumen-vdd` is a signed package, byte for byte (`binary` in `.gitattributes`); update it only from a
  Lumen VDD release and refresh `SHA256SUMS`.
- Before a commit — `tools\check.ps1`; before a release — `tools\check.ps1 -Full`.

## Commands (all of them — `docs/commands.md`)

```powershell
tools\check.ps1               # fmt + clippy + nextest (+ web if changed): errors and the summary only
cd web\host; npm run build    # the UI — BEFORE cargo build -p lumen-host-app (same for web\client)
npm run dev:mock              # mocks: web\client :5175, web\host :5174, web\admin :5176/admin/
tools\local-session.ps1 -HostArgs "--source synthetic" -ClientArgs "--borderless --frames 900"
deploy\build-installer.ps1; deploy\build-server.ps1   # release (a running service holds target\release — stop it)
```

## The most dangerous pitfalls (the rest — `docs/pitfalls.md`, by topic)

- CUDA↔D3D11 interop — only on the render thread. Windows timers: 15.6 ms tick, `sleep_until` spins — not on the hot
  path.
- Never inject input on a single machine (host and client on one PC). Defender touches `lumen-host.exe`. A VPN with a
  TUN adapter can break the path — see `Station::listen`.
- The client picks pacing and FEC. Encryption and `seq`; IKpsk2 with a wrong token.
- `install.sh`: an nginx reload is not instant, no systemd, `pipefail` with `grep -q`. Trust by fingerprint — only
  for local addresses.
