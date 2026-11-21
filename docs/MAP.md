# Lumen project map

The goal is to find what you need without reading files one after another. Each module is described in one line (from its
own doc comment); details are in the code and in `docs/architecture.md`, `docs/decisions.md`, `docs/pitfalls.md`. Update
this map when adding a crate, module or script.

## How it works (one paragraph)

The **host** (`apps/host`, a Windows service + a SYSTEM worker process in the console session) captures a monitor (DXGI Desktop
Duplication, `crates/capture`), converts BGRA → P010 with a shader (`crates/gpu-convert`), encodes with NVENC (`crates/nvenc`),
cuts frames into datagrams with Reed–Solomon FEC and sends them paced (`crates/transport`) over UDP, encrypted with
ChaCha20-Poly1305 after a Noise IKpsk2 handshake. The **client** (`apps/client`, the video window) reassembles frames, decodes
with NVDEC (`crates/decoder`), converts 4:2:0 → RGB10 with a shader and shows the picture 1:1; input, clipboard, files, cursor
and audio go over the same channel (`crates/protocol` is the format). The **server** (`apps/server`, Linux/Docker) does only
signaling over TLS (`crates/signal`): device registration, access, session announcement, the web admin; video never goes
through it. **Lumen Host** and **Lumen Client** (`apps/host-app`, `apps/client-app`, Tauri) are the windows for people, their
UI is React in `web/`; the core returns state and codes, the words live in the UI.

## Top level

| Path | What |
|---|---|
| `CLAUDE.md` | rules, summary of decisions, current state — read first |
| `README.md`, `LICENSE-MIT` | project readme; license (MIT) |
| `docs/` | architecture, decisions, pitfalls, commands, this map |
| `crates/` | libraries (below) |
| `apps/` | `host`, `client`, `server`, `host-app`, `client-app` (below) |
| `web/` | React UIs: `client` (hub + overlays + shared styles and `ui.tsx`), `host`, `admin` |
| `tools/` | tool binaries (`bench`, `decode`, `netsim`, `compare`, `patterns`) and check scripts `*.ps1`, `overlay-check/`, `install-check/`, `installer-sandbox/` |
| `deploy/` | installer (`installer/lumen.iss`, `build-installer.ps1`), server (`server/`: Dockerfile, `entrypoint.sh` — modes `public`/`lan`, `compose/public.yml` and `lan.yml`, **`install.sh` — the setup menu on the server** (clean VPS, VPS with nginx, local network, update, status, removal), `site/` — landing page; `build-server.ps1 [-Version] [-Dist]`) |
| `manual/` | user manuals: `manual_EN.html`, `manual_RU.html` |
| `third_party/lumen-vdd/` | Lumen VDD virtual monitor driver: signed release package v0.2.3 of https://github.com/aimbyLeet/lumen-vdd (driver contract — `include/lumen_vdd_ioctl.h` there); `SHA256SUMS` is checked by `deploy/build-installer.ps1` |
| `icons/` | icon source (`main_icon.png` → `tools\make-icons.ps1`) |
| `bench-out/` | check outputs and test stands (not in git): `local` (`local-session.ps1`), `srv` (local server), `svc` (dev service flags), `sandbox` |
| `dist/` | releases: `lumen-setup-<v>.exe`, `lumen-server-<v>.tar.gz` (not in git) |
| `target/` | Cargo build output |

## Crates (`crates/`)

| Crate | What | Key files |
|---|---|---|
| `color` | **reference** color math (the specification for the shaders), any OS | `ycbcr.rs` BT.709 full 10-bit; `subsample.rs` 4:2:0; `lab.rs` CIEDE2000; `metrics.rs` PSNR/ΔE |
| `protocol` | datagram and message format, any OS | `header.rs` header; `control.rs` control messages (Hello, StreamStart, input, `MonitorWanted`…); `fragment.rs` FEC layout; `format.rs` video format; `signal.rs` signaling; **`update.rs` the update door (format frozen)**; `clipboard.rs`, `file.rs`, `audio.rs`, `cursor.rs`; `wire.rs` |
| `transport` | FEC, pacing, UDP, channel emulator, reliable delivery, encryption, bitrate cap controller, any OS | `fec.rs`; `pacing.rs`; `sender.rs` background sender; `net.rs` UDP, `sleep_until`; `reliable.rs` input/clipboard (retransmit by RTO, copy after 10 ms); `bulk.rs` file stream; `rate.rs` `CapController`; `secure.rs` Noise IKpsk2; `token.rs` host token → psk/lookup; `emulator.rs`; `seq.rs`; `config.rs` data folder; `input_stall.rs` input stall model |
| `signal` | signaling TLS client (rustls + ring) | `lib.rs` login and requests, `Trust`; `update.rs` the update door: `query`, `download` (resume, SHA-256); `pin.rs` trust by key fingerprint (`Trust::Pinned`, `CertProblem`: `err_cert_untrusted` / `err_cert_changed`); `agent.rs` long-lived host/client agents; `contract.rs` core ↔ UI contract tests |
| `capture` | DXGI Desktop Duplication, synthetic source, frame-number barcode; changed regions and the static QP map | `win.rs`; `synthetic.rs`; `barcode.rs`; `regions.rs` (regions → blocks); `qpmap.rs` (QP map, stage 6) |
| `gpu-convert` | D3D11 shaders: host BGRA8 → P010, client 4:2:0 → RGB10; shared D3D11 plumbing | `win.rs`; `display.rs`; `d3d.rs` (`fullscreen_raster`, `texture_desc`, `read_rows` — also for `capture`, `decoder`, the client); `*.hlsl` |
| `nvenc-sys` / `nvenc` | NVENC 13.0 bindings (generated) / encoder wrapper | `nvenc/src/lib.rs` |
| `nvdec-sys` / `decoder` | CUDA + NVDEC bindings (generated) / HEVC Main10 decoder → D3D11 via CUDA interop | `decoder/src/win.rs`; `annexb.rs`; `readback.rs` (checks only) |
| `input` | input: window messages → events; `SendInput` on the host | `window.rs`; `inject.rs`; `held.rs` held keys; `paste.rs` Ctrl+V |
| `audio` | audio: WASAPI loopback and output, Opus, resampler, playout buffer | `wasapi.rs`; `opus.rs`; `framer.rs`; `playout.rs`; `resample.rs`; `tone.rs`, `wav.rs` (checks) |
| `opus-sys` | libopus 1.5.2, built with `cc` | `libopus/` |
| `vdd` | Lumen VDD on the host side: driver contract, device, display configuration, monitor layout | `contract.rs`; `device.rs`; `display.rs` (`set_resolutions`, `keep_only`, `physical_connected`); `layout.rs` (`RESOLUTIONS`, `SCALES`, `place`, `ClientSet`) |
| `desktop` | shared by Lumen Host and Lumen Client (Windows): log, network, clipboard, files, service pipe, secrets | `log.rs`; `handle.rs` `Handle` — ownership of a `HANDLE` (also in the host, via `winsec`); `net.rs` adapters and routes; `hostsvc.rs` **requests to the service** (`Request`, `State`); `clipboard/` Windows clipboard (`thread.rs`, `sync.rs`, `formats.rs`, `agent.rs`); `files/` file transfer (`outgoing.rs`, `incoming.rs`, `clip.rs`, `place.rs`, `views.rs`); `netpath.rs` `Z:\` → UNC; `secret.rs` DPAPI; `cpu.rs`; `nap.rs` bounded sleep (`Nap`); `lang.rs` window language (installer choice from the registry); `update.rs` update: installer line (`installer_line`), footer (`UpdateView`) |

## Applications (`apps/`)

**`apps/host` — `lumen-host.exe`** (service, worker process, command line `lumen-host …`):

| Module | What |
|---|---|
| `app.rs` | command line and entry point (`token`, `registration`, `service`, `displays`, `audio-probe`, `clip-agent`…) |
| `station.rs` | what lives between sessions: capture, keys, server registration, accepting a client, locking after a session (`LockAfter`) |
| `session/` | one session: `mod.rs` the `stream()` loop and client events; `start.rs` startup (handshake, control thread, input, files, audio, sender); `rate.rs` bitrate cap; `monitors.rs` monitor announcement, layout, waiting for a monitor (`headless::Asking`); `pacing.rs` pacing and FEC as the client asks; `report.rs` per-second report |
| `streamer/` | monitor threads: `layout.rs` session monitors (`pick_monitors`, up to 3, `desktop_layout`); `screen.rs` `Screen`, `open_screens`; `worker.rs` monitor thread (capture → P010 → NVENC → FEC); `rate.rs` `SessionRate`; `cursor.rs` cursor, `renumber`; `mod.rs` `Stream`, `Cmd`, `Setup`, `Window` |
| `source.rs` | the picture: monitor or synthetic, one frame per interval |
| `control.rs` | the control thread and its datagrams |
| `remote_input.rs` | client input: ordering, applying |
| `cursor.rs` | cursor for the client: shape and position |
| `clipboard.rs`, `clip_agent.rs`, `user_files.rs` | session clipboard; helper running as the user (SYSTEM does not see OLE); user files (token of `explorer.exe`) |
| `audio.rs`, `audio_probe.rs` | session audio; audio probe |
| `service.rs` | SCM service: install / uninstall / run |
| `worker.rs` | the service worker process: SYSTEM in the console session, launching as the user |
| `registry.rs` | machine registration `%ProgramData%\Lumen\host`, **pipe `\\.\pipe\lumen-host`** (handles `hostsvc::Request`) |
| `monitors/` | the service's virtual monitors: `mod.rs` `VirtualMonitors` (driver); `shape.rs` `Shape`; `arrange.rs` plan and `displays arrange`; `keeper.rs` `Keeper` (machine / client set / question); `cli.rs` the other `displays …` |
| `headless.rs`, `headless_probe.rs` | host without monitors: what the host tells the service (`Reporter`), the question to the client (`Asking`), `no-physical.flag` simulation; step-0 probe |
| `awake.rs` | sleep prevention (`power.json`) |
| `update.rs` | update from the server: worker process (`Updater`: door, download, "updating"), service (installer outside the job, Lumen Host for the user afterwards), the `update.json` checkbox |
| `netinfo.rs`, `files.rs`, `winsec.rs` | info for the server; data files (key, token); SID, SDDL, `SendSAS`, launching on a desktop |
| `reboot.rs` | machine reboot at the client's request (`RebootWanted`): `InitiateShutdownW`, forced, without updates |

**`apps/client` — `lumen-client.exe`** (the video window; launched by Lumen Client):

| Module | What |
|---|---|
| `app.rs` | command line, connecting through the server, one window per device |
| `net/` | session with the host: `mod.rs` types (`Options`, `Counters`, `Decoded`); `handshake.rs` handshake; `connection.rs` `Connection`, decoder queue; `thread.rs` network thread `NetLoop` (frame assembly, input, control); `recovery.rs` loss recovery state machine; `files.rs` files and clipboard; `checks.rs` check-only code (`--sim-*`, `--packet-log`, `--test-paste`, `--dump-streams`) |
| `live.rs`, `present.rs`, `window.rs`, `layout.rs` | show loop (decode on arrival); swapchain; window; where the picture sits in the window |
| `render/` | render steps: `mod.rs` `Renderer`, `ViewCache`; `checks.rs` check-only (`TestPattern`, `Clear`, `VideoFile`, `InputScript`); `network/` live video — `mod.rs` `NetworkVideo` (decode, show, reconnect), `views.rs` monitors and modes, `pointer.rs` cursor (`draw_pointer`), `hashes.rs` `--frame-hashes` |
| `scene.rs`, `composite.rs`, `extra.rs`, `memory.rs` | host monitors in the window: modes 1–3, mode-3 windows, view memory per machine card |
| `pace.rs`, `autofec.rs`, `arrival.rs`, `tune.rs`, `clock.rs` | pacing and FEC chosen by the client; RTT and host clock |
| `status.rs` | `@status` for Lumen Client (`--status-json`) |
| `ui_api.rs`, `overlay.rs` | overlay API: `@overlay` state, hub commands from stdin, the window above the picture; numbers for the overlay |
| `monitor_pick.rs` | host without monitors: the question, a proposal based on the client's monitor, the answer (`MonitorPick`, `HostWaits`) |
| `shortcuts.rs`, `paste.rs`, `clipboard.rs`, `audio.rs`, `cursor.rs` | system shortcuts (hook); holding input after Ctrl+V; clipboard; audio; cursor |

**`apps/server` — `lumen-server`** (Linux/Docker, also builds on Windows): `lib.rs` signaling over TLS (`protocol::signal::SIGNAL_VERSION`), sessions, `recheck`;
`db.rs` SQLite (devices, access, quality overrides); `update.rs` installer for the update door (`--update-dir`, at most 16 downloads); `admin.rs` web admin (HTTPS API; on a local network also the landing page `--site` and HTTP→HTTPS `--redirect-listen`); `main.rs` administrator CLI (`self-cert` — own certificate for a local network, `fingerprint`).
Tests with timeouts — `apps/server/tests/`.

**`apps/host-app` — Lumen Host** (Tauri, `io.github.aimbyleet.lumen.host`): `main.rs` — window, tray, UI commands → requests to the service (`desktop::hostsvc`).
**`apps/client-app` — Lumen Client** (Tauri, `io.github.aimbyleet.lumen.client`): `main.rs` commands, `watch_video` (hub aside / above the video — `Hub::after`);
`card.rs` machine cards (token); `server.rs` connection to the client's single server; `update.rs` update from the server (door, installer, restart); `video.rs` launching the video window; `state.rs` state for the UI.

## UIs (`web/`)

The only link to the core is `src/api.ts` in each. Mocks without the core — `src/mock` (`npm run dev:mock`), not part of the build.
Contract with the core — `src/contract/*.example.ts`: written by the core tests (`signal::contract`) from real responses, `tsc` checks them against the types.

| Folder | What |
|---|---|
| `web/client/src` | Lumen Client: `App.tsx`, `Machines.tsx` (cards), `Preferences.tsx`, `Help.tsx`, `MonitorDialog.tsx` (host without monitors), **`TrustDialog.tsx` server trust-by-fingerprint dialog (shared with Lumen Host)**; `Welcome.tsx` first run: name and server (`useIdentity` — also in preferences); `Update.tsx` yellow "version outdated" footer, confirmation, update progress; `status.ts` status words; **`errors.ts` words for core codes (shared)**; **`i18n.ts` UI language (shared): `words(en, ru)`, `setLang`**; **`units.ts` units and numbers with them (shared): `fps`, `mbps`, `cap`, `hoursMinutes`, `locale`**; **`ui.tsx` shared components** (icons, frame, `Dialog`, `Confirm`, `TokenInput`, `LangToggle` — the globe, `LogSetting`, `byAspect`); `components.css`, `tokens.css`, `style.css`; `overlay/` overlays (`Overlay.tsx`, `model.ts`) |
| `web/host/src` | Lumen Host: `Machine.tsx`, `Monitors.tsx` (machine monitors), `ClientMonitors.tsx` (client set), `MonitorDesk.tsx` + `useDesk.ts` (diagram, dragging), `MonitorPanel.tsx`, `layout.ts` (layout = `vdd::layout::place`), `Preferences.tsx`; styles and `ui.tsx` come from `web/client` |
| `web/admin/src` | admin: `Clients.tsx`, `Machines.tsx`, `Requests.tsx`, `Settings.tsx`, `common.tsx` |

## Tools and scripts (`tools/`)

| What | For |
|---|---|
| `bench` → `lumen-bench` | capture → codec test stand (quality, latency, `--verify`, RFI) |
| `decode` → `lumen-decode` | decoding a file with our decoder, bit-exact comparison, frame hashes |
| `netsim` → `lumen-netsim` | stream and files through the transport over loopback with emulated loss, delay, bandwidth |
| `compare`, `patterns` | ΔE2000/PSNR and heat maps; PNG test chart |
| `check.ps1` | short pre-commit checks: fmt, clippy, nextest (`-Full` — `cargo test`), tsc and `npm test` if `web/` changed (`-Web` — always) |
| `local-session.ps1` | server + host + client on this machine, approval (data — `bench-out\local`) |
| `docker-session.ps1` | the same with the server image in Docker (kills processes running from `target\release`, including a local server) |
| `headless-client.ps1`, `headless-hub-path.ps1` | client to a dev-service host without monitors; the hub path (`--status-json`, a command on stdin) |
| `dev-service.ps1` | watcher of the flag files `bench-out\svc\stop.flag` / `start.flag` for the dev service (run elevated) |
| `install-check/` | `install.sh` in Docker: `run.sh build` (two test archives), `run.sh lan\|nginx\|clean\|old <ubuntu>` — scenarios with OK/FAIL checks |
| `installer-sandbox/` | installer in Windows Sandbox: `build.ps1` (test versions, ISCC only), `lumen.wsb` (started by hand) → `run-tests.ps1` at logon: client as a regular user, host as SYSTEM; `build-client-update.ps1` (client of two versions + server) and `client-update.wsb` → `run-client-update.ps1`: the client updates itself, a broken installer is refused; `build-host-update.ps1` (three versions to check the host update on real Windows: the sandbox has no console session); `purge.wsb` → `run-purge.ps1`: "Delete user data" on uninstall (run `build.ps1 -Versions 0.6.90 -Out purge` first); every check first calls `prep-sandbox.ps1` (Smart App Control off, the WebView2 registry entry), and at the end `finished.ps1` (a FINISHED window with the result); the `build*.ps1` builders share `common.ps1` (ISCC, CRT, `Build-Setup`, `Invoke-AsVersions` — version for the build, then `Cargo.toml`/`Cargo.lock` restored byte for byte); the client is installed by the regular user `lumtest` (WebView2 for all users is not installed in the sandbox, `0x80040C02`); OK/FAIL in `bench-out\sandbox\results` (`purge` — in `bench-out\sandbox\purge`) |
| `overlay-check/` | `snapshots.mjs` snapshots of all mocks (byte-for-byte comparison); `gate.mjs` overlay checks on real windows (free screen only) |
| `make-icons.ps1` | icons of all applications from `icons\main_icon.png` |

## Runtime data

| Where | What |
|---|---|
| `%APPDATA%\Lumen` (`LUMEN_DATA`) | client: `client-app.json` (cards, tokens — DPAPI), `servers\<server>.creds`, logs |
| `%ProgramData%\Lumen\host` | service host: `host.token`, `host.creds`, `host.json`, `host.key`, `server-ca.pem`, `logs\` (host logs per session) |
| `%ProgramData%\Lumen` | service: `monitors.json` (machine monitors), `client-monitors.json` (client sets), `power.json`; `logs\` — SYSTEM only |
| `\\.\pipe\lumen-host` | Lumen Host ↔ service pipe (`desktop::hostsvc`) |

## Where to start by topic

| Topic | Read |
|---|---|
| color, shaders | `crates/color`, `crates/gpu-convert`; `docs/architecture.md` |
| codec, cap, recovery | `crates/nvenc`, `crates/transport/src/rate.rs`, `apps/client/src/net/recovery.rs`; `docs/pitfalls.md` (IDR, RFI) |
| network, pacing, FEC | `crates/transport`, `apps/client/src/{pace,autofec,arrival}.rs`; `docs/pitfalls.md` |
| server, access, admin | `apps/server`, `crates/signal`, `web/admin`; `docs/decisions.md` (access, server) |
| Windows service | `apps/host/src/{service,worker,registry,winsec}.rs`; `docs/decisions.md` (Windows service) |
| clipboard, files | `crates/desktop/src/{clipboard,files}`, `crates/transport/src/bulk.rs`; `docs/pitfalls.md` (clipboard, `BulkSender`) |
| audio | `crates/audio`, `apps/*/src/audio.rs`; `docs/pitfalls.md` (audio) |
| multiple monitors, Lumen VDD | `crates/vdd`, `apps/host/src/monitors/`, `apps/client/src/{scene,composite,extra}.rs`, `third_party/lumen-vdd/`; `docs/decisions.md` (virtual monitors) |
| host without monitors | `apps/host/src/headless.rs`, `monitors/keeper.rs` (`Keeper`), `apps/client/src/monitor_pick.rs`, `web/host/src/ClientMonitors.tsx`; `docs/decisions.md` (host without monitors) |
| cursor | `apps/client/src/render/network/pointer.rs`, `apps/host/src/cursor.rs`; `docs/pitfalls.md` (cursor) |
| UIs, overlays, API | `web/*`, `apps/client/src/ui_api.rs`; `docs/decisions.md` (UIs, overlays) |
| update | `crates/protocol/src/update.rs`, `crates/signal/src/update.rs`, `apps/host/src/update.rs`, `apps/client-app/src/update.rs`, `apps/server/src/update.rs`; `docs/decisions.md` (auto-update) |
| release | `deploy/*`; `docs/commands.md`, `CLAUDE.md` (commands) |
| languages (English, Russian) | `web/client/src/i18n.ts`, `units.ts`, `LangToggle` in `ui.tsx`, `crates/desktop/src/lang.rs`, `deploy/installer/lumen.iss` (`[CustomMessages]`), `deploy/server/site/index.html`; `docs/decisions.md` |

## Documents (`docs/`)

| File | About |
|---|---|
| `architecture.md` | architecture and stages |
| `decisions.md` | owner's decisions, full text |
| `pitfalls.md` | pitfalls: symptom, cause, fix |
| `commands.md` | all commands |
| `quality-bench.md` | the color/codec quality bench (`lumen-bench`, `lumen-compare`) |
| `MAP.md` | this map |
