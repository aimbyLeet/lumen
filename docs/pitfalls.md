# Pitfalls

Each item is a trap found in practice, with its cause and workaround.
Before changing a subsystem, find its item (search by word: `NVDEC`, `clipboard`, `cursor`, `pacing`, `Happ`, `SYSTEM` …);
a topic index is in `CLAUDE.md`. A new trap goes here, briefly: symptom, cause, what to do.

- Desktop Duplication (`DuplicateOutput1`) requires per-monitor DPI awareness — call `capture::init_process()`
  first thing.
- `DXGI_ERROR_ACCESS_LOST` (mode change, UAC, a fullscreen application) — the duplicator recreates itself.
- P010 requires even frame dimensions.
- The wrapper is built for NVENC API 13.0 (driver R570+). 13.1 is not used: Pascal (GTX 10xx) is stuck on
  the 580 branch with API 13.0. The version is checked at initialization.
- On Optimus laptops the monitor is often attached to the iGPU — bench will refuse to work (this is expected).
- The cursor is not part of the host frame (by design): it is sent separately and drawn on the client.
- On the client decode through NVDEC (CUDA + D3D11 interop), not D3D11VA; output strictly 1:1 without scaling.
  The video window (decided 2026-10-24): it stretches and maximizes, the picture always fits while keeping
  its aspect ratio (up and down), black bars; borderless (`Ctrl+Alt+Shift+F` in a session, `--borderless` at
  start) — full monitor, also fitted. Exactly 1:1 — only when the client area = the picture size
  (the window opens that way if it fits). How it works: `layout.rs` — a swapchain buffer with the window's aspect ratio and
  the picture's scale, the picture in it 1:1 at `origin`, `DXGI_SCALING_STRETCH` stretches the buffer onto the window
  (buffer = window → no scaling). Renderers draw 1:1 into `Presenter::render_target` — the buffer itself if it is exactly
  the picture, otherwise a texture of the picture's size that `present` copies into the black buffer (exact; the
  `--verify-every` check reads the picture). The window size comes from `WM_SIZE` into a static (`window::current_layout`), the show
  loop and the modal loop timer do `ResizeBuffers` themselves. Scaling — DWM bilinear.
  Over the black bars (`BarFilter`): movement goes to the host (the cursor stops at the edge), presses and the wheel do not;
  a button pressed over the picture is released on the host wherever it is; the system cursor over the bars is visible.
- NVDEC bindings are generated for the **Windows** target: the headers have `unsigned long` (32 bits on Windows,
  64 on Linux) — with a Linux target the cuvid structs would not match.
- The decoder returns a frame as two textures `R16_UNORM` (Y) + `R16G16_UNORM` (CbCr), not one P010:
  that way they can be registered in CUDA interop. The 10-bit code is in the high bits, as in P010.
- On Windows the timeouts of `mpsc::recv_timeout` and the like run on the system tick (~15.6 ms), while `thread::sleep`
  is high resolution but wakes on the 0.5 ms timer tick (a request to sleep 100–450 µs wakes 230–680 µs late,
  p50). Precise waits — `transport::sleep_until`, but it **spins** for the last 1.5 ms: it is not in the host's hot path
  (measured on synthetic 4K60: host 33 % of a core → 7.8 %). The paced sender sleeps until the next
  burst, and the pacing bucket is no smaller than what the pace accumulates per tick (0.6 ms: at 150 Mbit/s bursts ≈ 14 KB instead of 8);
  the host waits for the frame start with a regular sleep (`source.rs`, `nap_until`). Tests and the emulator — `sleep_until`.
- `thread::sleep` may **never wake up**: it waits on a high-resolution timer (`IRTimer`) without a
  limit, and on one client machine such a timer did not fire — the audio thread fell asleep for 2 ms forever, audio
  disappeared, the video window did not close (`Drop` waited on `join`). In long loops — `desktop::nap::Nap` (the same timer,
  but the wait is bounded by the deadline + 50 ms by tick); `join` — with a limit; the video process exits by itself
  5 s after the session (`exit_guard`). The host and `transport` still use `thread::sleep`.
- Tests that measure time (sockets, sleep) live in `crates/transport/tests/` (a separate binary):
  in one binary with heavy unit tests they catch foreign CPU load and flap.
- **CUDA↔D3D11 interop only in the render thread.** Decoding in a separate thread (map/unmap of the output textures)
  while the immediate context worked in parallel lost the device within a few frames even with
  `SetMultithreadProtected`. Now: the network has its own thread, decode + drawing — the render thread.
- The client needs a device with multithread protection: take `capture::GpuDevice::open_nvidia()`.
- IDR: the host ignores a request if it has already sent an IDR newer than the lost frame (otherwise an IDR storm:
  the client repeats the request every 150 ms). A 4K IDR ≈ 1.2 MB ≈ 1000+ packets — with losses ≥ 20 % and 15 % FEC it
  does not get through at all (outside the target links; parity is at most 15 %, see "FEC is chosen by the client"). Now IDR is sent only at start and when
  RFI is impossible; with the cap it is limited by the VBV (103–212 KiB) and "develops" to QP 16 in ≈ 25 frames.
- Loss recovery — RFI (`RequestRecovery`): DPB 16, the client feeds nothing into the decoder until the `RECOVERY`
  frame. NVENC intra refresh (Pascal) does not recover the stream deterministically — do not return to it without
  a new measurement. ffmpeg stops decoding after a skipped reference frame — test loss only on NVDEC.
- NVENC `targetQuality` (Pascal, ULTRA_LOW_LATENCY) is ignored. Rate control with a 1-frame VBV undershoots the set
  bitrate by 30–45 % — the cap is held by `transport::CapController` from the actual rate (reconfiguration without IDR).
- Client: `decode_pending` decodes ≤ 4 AUs per call and wakes the loop itself if more remain; otherwise, when the decode
  falls behind, the window stops responding (Windows kills it as AppHang). `--frame-hashes` is slower than real time
  (4K readback per frame) — do not measure latency in this mode.
- A Windows socket with `SO_RCVTIMEO` sometimes returns `ERROR_IO_PENDING` (997) instead of a timeout — it is a timeout.
- Encryption: all datagrams except `Handshake` are ChaCha20-Poly1305 with the header as AD and `seq` as the nonce;
  never repeat `seq` under one key (on the host all datagrams take `seq` from one `SeqCounter`, the client has its own
  counter). Encrypt new packet types the same way; the receiver drops unencrypted ones. Session keys — the raw split of
  `snow` (feature `risky-raw-split`): the `snow` transport cannot do AD.
- The host serves sessions one after another, by `Incoming` from the server: a socket on its port (9000, busy — up to 9009)
  is opened for the time of waiting for the handshake and the session, it accepts only the client key from `Incoming`;
  `--seconds N` — one session and exit (that is how the check scripts work). The host sends `Bye` only when it closes the
  stream itself (the client then exits); if the server ended the session — no `Bye`, the client reconnects by itself.
- The client sends the first handshake message to all host addresses at once: on one machine the copies arrive at its different
  addresses — the host ignores copies within 150 ms after the reply (otherwise extra `StreamStart` and IDR). The host considers
  the client local (no input injection) if its address is loopback or one of the host's addresses.
- IKpsk2: the host cannot check the client's psk (it enters only message 2) — a client with a foreign token gets
  a reply but cannot decrypt it; in this case the host recreates the keys on every attempt and closes the session after
  10 s of silence. This happens only with a hostile server (key approved, token wrong).
- With an RTT larger than the DPB window (16 frames ≈ 266 ms at 60 Hz) RFI is impossible — the host sends IDR (seen on a test VPS
  through a VPN in the USA).
- **Defender detects `lumen-host.exe` as `Trojan:Win32/Bearfoos.A!ml`** (ML heuristics + cloud): the host looks
  like a RAT (screen capture + `SendInput` + network, no signature or reputation). The verdict is **inconsistent**: the same
  file (same hash, same signatures) in `target\release` is sometimes clean and sometimes not, while in `Program Files` it is clean. Switching
  the static CRT to the dynamic one (`vcruntime140.dll` next to the exe) coincided with a "clean" scan but does not remove
  the cause. The fix — code signing (+ submitting a false positive to Microsoft) — was dropped from the plan: with the
  stage-4 installer Defender did not trigger on the test machines. If it comes back — check with
  `MpCmdRun -Scan -ScanType 3 -File <exe>` and be ready for quarantine on users' machines.
- **A third-party VPN with a TUN adapter (Happ — adapter `happ-xray`, `172.19.0.1`; v2rayN etc.) on either of the two
  machines breaks the path to the host**: its `0.0.0.0/0` (metric 0) takes everything that has no more specific route; UDP goes into
  the TUN, and `xray` resends it "directly" with losses and from another port. Subnet exclusions and choosing an `.exe` in Happ
  do not create Windows routes — they do not help. Symptom: both apps are "on the server", the host announces the session, the handshake
  reaches the host, the reply does not (client: `no answer from the host`), the status flickers. Therefore **the host listens on the port
  with a socket on each of its addresses** (`Station::listen`): Windows (strong host model) sends from a socket bound to an
  address only through that address's interface — bypassing the TUN (measured with Happ on: 3000 UDP from `0.0.0.0` → into
  `happ-xray`, from the Wi-Fi address → 3030 through Wi-Fi and 32 into the TUN). The client follows Windows routes: with WG on the client
  the route to the host's network is more specific, the VPN does not interfere. The client with `--status-json` (launched
  by Lumen Client) reports the session state (`apps/client/src/status.rs`): the adapter of the route to each host
  address and the session's /24 subnets (here — the host addresses, on the host — our address); a private host address routed
  "to the internet" (same source as to 1.1.1.1) — has no route of its own, its address is not taken into the subnet.
- **The frame send pace (pacing) is chosen by the client** (`apps/client/src/pace.rs`, `ControlMessage::SetPacing`
  once a second; `--pace auto|host|<Mbit/s>,<KB>`): start at 150 Mbit/s with 8 KB bursts; packets within frames arrive
  slower than 0.85 of the pace (a bottleneck on the path) — pace = 0.9 × arrival rate; ≥ 2 frames with loss bursts (≥ 3 in a row)
  within a second — × 0.85; 10 s clean — +10 %, at most 150; at least 60. A host without the client's request — also 150 / 8 KB.
  The actual burst is no smaller than what the pace accumulates per sleep tick (see `thread::sleep` above): at 150 Mbit/s ≈ 14 KB.
  Through WG (one minute each, interleaved): 150 (bursts 13–15 KB) — 4 runs, 2.3 loss bursts/min, 7 frames
  lost; 100 (7–9 KB) — 4 runs, 2.0/min, 11 frames; 120 — 2 runs, 2.6/min, 3 frames: the difference is within the link noise
  (one variant from run to run: 0 → 8 bursts/min), 150 was kept. The measurements below are still with 8 KB and scrolling.
  Earlier a 64 KB burst went out at LAN speed (through WG a frame arrived at 200–440 Mbit/s) and overflowed the queue
  of the client's Wi-Fi access point: the frame tail was lost in bursts, FEC did not help. Measurements through WG (4K60, video, one minute each):
  64 KB — 4.7 freezes/min, 0.80 % of the time, p50 39 ms; 150/8 KB — 2.4/min, 0.44 %, p50 42; 120/8 KB — 2.4/min,
  but p50 46, p95 55; 200/8 KB — no gain. Auto through WG holds 150: 2.1–3.0/min, p50 39–41 ms.
  Losses through WG on the test link are mostly single (the client's Wi-Fi).
  A frame is declared lost as soon as it cannot be assembled (`REORDER_GRACE` 3 ms), not by the next frame:
  freeze p50 104 → 81 ms. Analysis — `lumen-client --packet-log` (every datagram) and `--frame-log`.
- **FEC (the parity share) is chosen by the client** (`apps/client/src/autofec.rs`, `ControlMessage::SetFec` once a second and
  immediately when raising; `--fec auto|host|<%>`; the frame as it arrived — `arrival.rs`, shared with pacing). Levels 3 / 5 / 10 / 15 %,
  start at 15 (a host without a request — `--parity`, 15). The host sends a block "data, then parity" in order, so for each
  block with losses the client knows exactly what would have happened with less parity (`Loss::fails_at`; above the sent amount — losses
  proportionally). One level down — if over 20 s the lower level would not have lost a single frame beyond the current one
  (at most once every 10 s, with ≥ 5000 datagrams in the window); a frame lost that 15 % would have saved — straight to 15 %, right on
  that frame, not on the tick (a link that is degrading loses several frames per second: when raising one level per
  second, all the "extra" losses of a run were within 2 s of the collapse). There is no 25 % level: in a run with it, 15.8k extra parity
  datagrams did not save a single frame — long bursts (queue overflow) are cured by pacing. Measurements through WG (4K60, video,
  2.5 min): auto — parity 15 → 3 % in 40 s, holds 3 % on a clean link, on average 7.7 % of data, datagrams −7 %
  against 15 % (−10 % in steady state), 1 frame lost beyond what 15 % would have saved (estimated from the packet log);
  locally, emulated: 0 % loss → 3 %, 1 % → 5–10 %, 3 % → 15 %, no frames lost beyond 15 %. Compare
  variants through WG by the packet log (`--packet-log`, recomputing "what would have happened at p %"), not by the number of freezes:
  a real link changes many-fold within minutes (loss bursts 5 → 59 per minute between neighboring runs), and
  "freezes" in `--frame-log` include show skips (frames in pairs, 13 ms decode) unrelated to the network.
- A VPN with a TUN adapter on the work machine sometimes returns `WSAEACCES` (10013) to new processes for a few seconds
  on outgoing connections (e.g. to a VPS).
- Server: TLS — `rustls` with the `ring` provider (aws-lc requires cmake/nasm on Windows). The development certificate is
  a CA + a separate server certificate (`dev-cert`), devices trust the CA (`--server-ca`) — the same
  "root → server certificate" scheme as Let's Encrypt. Sessions are bound to signaling connections: a drop or a repeated login ends them.
- Server tests with timeouts live in `apps/server/tests/` (a separate binary, short timeouts via `Config`).
- Server (signaling v5): the server sees administrator changes in the database by itself (the CLI is another process) — the `recheck` thread
  checks running sessions and connected devices against the database every 0.5 s. A client can have several connections
  (the app and the video window), a host — one (a new one closes the old one). The `SessionEnd` of a previous session may arrive
  on the same connection after the reply to a new `Connect` — wait for the reply, skipping foreign `SessionEnd`s.
- **System shortcuts to the host** (`apps/client/src/shortcuts.rs`, `--shortcuts host|local`, host by default;
  in Lumen Client — the `shortcuts_to_host` checkbox): `WH_KEYBOARD_LL` in its own thread (the hook holds up every key
  of the system, and the render thread is sometimes busy decoding for ~13 ms). While the video window is `GetForegroundWindow`, the hook takes
  **all** keys and sends `WM_HOOK_KEY` to the window — by a single path, otherwise posted messages overtake the queue's input messages
  and the "Alt, Tab" order breaks. The hook tracks modifiers itself (for swallowed keys `GetKeyState` does not
  change), Ctrl+Alt+Shift+Q/F — `WM_HOOK_SHORTCUT`. The hook does not see Ctrl+Alt+Del and Win+L. Check —
  `lumen-client --test-shortcuts <file>` (SendInput: the hook sees injected keys too; Win+L — never) + comparing
  the `Key` lines with `lumen-host --input-log`; with `--shortcuts local` — only Win and Alt+Tab (control: Win "leaks").
- **Clipboard** (protocol v5; `protocol::clipboard`, `desktop::clipboard`, `apps/host|client/src/clipboard.rs`):
  text is cut into 1155-byte pieces; client → host — `InputEvent::Clipboard` in the input stream (the host writes the text into
  the clipboard **before** the next event — Ctrl+V right after copying finds it: `Clipboard::set` waits for the write), host →
  client — `ControlMessage::Clipboard` / `ClipboardAck` (its own `ReliableSender` in the host control thread, whose
  `recv_timeout` is 20 ms). The `ReliableSender::due_within` window: at most 64 KB of text in flight (a megabyte does not go out in one
  burst); window input does not consume it but queues behind the text. The Windows clipboard has its own thread with a hidden window
  (`HWND_MESSAGE`: writing requires an owner window, and the owner must answer messages), polling the change number
  every 100 ms, the number of its own write is skipped; `ClipSync` remembers the last transferred text (it does not go back or
  repeat). Not written (clipboard busy; on a locked host — always) — the thread retries every 100 ms until it
  writes or something new is copied here (seen in practice: the client's clipboard was lost when connecting to a lock
  screen). A host on the same machine as the client — `--clipboard auto` = dry-run (the clipboard is shared), like input. The host takes only
  texts copied after the session start (`Clipboard::start(false)`), the client — after connecting (also `false`).
  Check: `--clipboard-script N` (both sides) + `--clipboard-log` (lines `S|R <bytes> <hash>`), client
  `--clipboard dry-run`; the real clipboard — `cargo test -p desktop -- --ignored clipboard` (touches this machine's clipboard,
  restores the text). When comparing input logs — `--clipboard off` at the client (otherwise its clipboard is in the input stream too).
- **Audio** (protocol v7): host — the `audio` thread in the session (`--audio auto|loopback|tone|off`,
  auto: client on this machine — off, otherwise loopback); Opus 128 kbit/s, 10 ms frame, CELT `RESTRICTED_LOWDELAY`,
  each packet carries the current and the previous frame (Opus inband FEC does not work for music — SILK only); audio packets bypass
  the video queue (`NowSender`). The time in a loopback packet is the moment it sounds on the host (~8 ms later than the packet
  can be read). Client — an `audio` thread for the whole process (`--audio on|off|null`, null — a silent device for
  checks; audio is always on, there is no checkbox — owner's decision), a 40 ms buffer including the device, the speed is not
  touched within ±10 ms, beyond that — a PI controller (≤ 0.5 %). Pitfalls: an empty queue is an underrun, not a loss
  (concealing "forward" moves playback ahead of the host); the null device model is like a real one (on
  underrun it does not go negative); while nothing plays, loopback returns no packets — the host fills silence by the clock
  (`Pauses`: no losses and no double-counting of time), frame numbers are not reset when the device is reopened; the client's
  buffer target is 50 ms (10 in the device), audio threads are `TIME_CRITICAL` (otherwise the device ran dry on a loaded
  machine); `local-session.ps1` kills all `lumen-host` processes — run a probe from a copy of the exe;
  loopback under load on one machine (4K encode + decode + show) occasionally slips by 1–2 samples (Windows).
- **The Lumen Client hub over the video window** (`to_front` in `apps/client-app/src/main.rs`): Windows does not execute `set_focus` from a background process (it only flashes the taskbar), and the hub shown by a repeated launch lay under the video. It is raised by `always_on_top` on → off (does not require the foreground); the second instance calls `AllowSetForegroundWindow(ASFW_ANY)` before handing over.
- **Overlay**: a web page (`web/client/src/overlay`) in a Lumen Client window over the picture of the main video window; the core — `apps/client/src/ui_api.rs`: state as `@overlay` lines on stdout, JSON commands on stdin, the window position. Only with `--status-json`: without the hub (check scripts) there are no overlays.
  The window has no owner on purpose (input queues would merge, and a busy hub would hold input in the video), the window order is kept by `keep_above`: **`HWND_TOP` from this thread does not raise the window** (the overlay lay under the video, clicks "did not get through", while `IsWindowVisible` said all was well; look at screenshots). While nothing is over the main window and it is in front — `HWND_TOPMOST`, otherwise directly above it and below whatever covers it (other monitors' windows too: they have no overlay).
  Click-through — `WS_EX_TRANSPARENT`, removed when the mouse is over `data-interactive`. Under CSS `zoom` percentages do not scale: `left: 50%`, not `calc(50% / k)`. Block size — by the picture's scale, no more than 1:1 (decided 2026-10-27); over the whole client window was tried — the owner cancelled it.
  Show latency with the overlay is the same (p50 18.44 vs 18.47 ms without it), but the metric ends at `Present` and **does not see Windows composition** (that needs PresentMon with administrator rights). Check: `node tools/overlay-check/gate.mjs work|monitors|files|rest|errors|tray|cost` — **only on a free screen** (real mouse, screenshots); mock — `npm run dev:mock` in `web\client`, http://localhost:5175/overlay.html.
- **Ctrl+Alt+Del to the host** (stage 5): on the client `Ctrl+Alt+Shift+Del` (Windows does not intercept it — verified) →
  `InputEvent::SecureAttention` (protocol v4; over the input channel, in order with keys) → the service host asks the service
  (`Request::SecureAttention`, accepted only from SYSTEM) → `SendSAS` from `sas.dll` (works only in a service and
  only with the `SoftwareSASGeneration` policy with bit 1 — set by the installer). A host without the service cannot send it.
- **One video window per device** (`one_window_per_device`, `apps/client/src/app.rs`): the server keeps one session
  per client, a new `Connect` ends the old one — two copies with one account took the host from each other in a loop
  (each reconnected). A second copy with the same account file (a named mutex by its path) exits with an error,
  the first is not interrupted. Check — `tools\local-session.ps1 -SecondClientArgs "--frames 600"` (`client2.log`).
- **A SYSTEM process does not see in the clipboard what was put there through OLE** (`OleSetClipboard`: Explorer files — always,
  text — from some programs): only the `DataObject` format. What was put with plain
  `SetClipboardData` is visible. Therefore the host clipboard goes through a helper running as the user.
- **The host (SYSTEM on behalf of the user) does not see the user's network drive letters**: `Z:\…` — "The system cannot find
  the path specified" (os error 3), although Explorer sees it. The `clip-agent` helper translates
  such paths into `\\server\share\…` (`desktop::netpath`). But the network path with the `WTSQueryUserToken` token also gives "Access
  is denied" (os error 5): it is not the logon session Explorer works in. Since 0.3.3 the host acts with the user's `explorer.exe`
  token (`user_files::shell_token`); not yet verified on a real network drive setup.
- **Do not use delayed clipboard rendering (`SetClipboardData(fmt, NULL)`) to "wait" for data**: programs
  that watch the clipboard (Happ) request the format immediately, and while the owner waits, they hold the clipboard open — it is busy for the whole
  system (verified: a paste by another process — empty within 0.01 s). A unit test with open retries hides this.
- **RTT by itself does not mean a queue** (`BulkSender`, 0.5.7): through WG it wanders by 100 ms within seconds at any pace. RTT growth cuts the pace, but if after the cut the RTT did not drop and
  sending did not outrun arrival — it is path delay, the base moves (`moved` in the log). A full queue cannot be told apart by the "sent / arrived" ratio: the excess is dropped, the ratio
  is only 1.04; therefore the base moves only after a cut, otherwise video next to a file loses half its frames. When changing the controller — look at `lumen-netsim --file-mb 256 --video-mbps 10|40 --queue-kb 512`.
- **Do not detect a lost file chunk by time in the middle of the data** (`BulkSender`, 0.5.8): an RTT jump makes everything in flight "lost". By time — only the tail; an acknowledgment faster than the minimum round trip
  is the first copy of an already resent chunk, it does not move the order. Compare: the host's `resent` against the client's `lost` (`client: N lost`, counts all datagrams) and `chunks twice`.
- **`BulkSender` after idling** starts the ramp-up over (`start_over`): otherwise the first chunk is a false timeout, and every
  transfer after a pause crawled from 1 Mbit/s (fixed in 0.3.3; test `small_copies_with_pauses_keep_their_speed`).
- **Do not inject input on one machine:** a host click would land in the client window → a loop. `lumen-host --input auto`
  (default) does a dry-run for a client on the same machine. In the client during a session, local shortcuts
  (Esc, Alt+F4) go to the host; close the client with Ctrl+Alt+Shift+Q.
- Input — `ControlMessage::Input { id, .. }` / `Inputs` (v11): end-to-end numbering, the host applies strictly in order,
  cumulative `InputAck` (one per `Inputs`), retransmit by an RTO from the RTT (RFC 6298, 20 ms … 1 s, initial 1 s; `transport::reliable`); input events
  are also copied after 10 ms and ride in the following datagrams (`push_small`, `due_datagrams`), clipboard and files are not.
  Delay for checks is given by `lumen-client --sim-delay <ms>` (on incoming datagrams, the RTT grows by it). All host datagrams (video, cursor, ack) take `seq`
  from one `SeqCounter` — it is both the loss counter on the client and the encryption nonce.
- Cursor: the `CursorPosition` position is the hotspot point; the client draws the cursor (`draw_pointer`, one function for all windows) at the position
  of the local mouse while the user moves it (no network delay), otherwise — where the host reported. Host positions arriving
  within the input RTO + 40 ms after a local movement are the echo of that movement, do not draw them (otherwise the cursor
  jumps between the current position and the one an RTT behind — a "trail"; `pointer_position`). Inverting pixels
  (I-beam) are drawn with a real inversion (blend INV_DEST_COLOR). The client's system cursor was tried and rejected by the owner;
  Windows reads a 32-bit image as a color cursor with a mask — `DrawIconEx` does not show this.
- The client shader quantizes itself and writes `code / 1023`: the float → UNORM10 conversion in D3D allows
  an error, while an integer value converts exactly.
- Swapchain: `DXGI_SCALING_NONE` + the color space `RGB_FULL_G22_NONE_P709`; there is no 8-bit path
  (owner's decision: always R10G10B10A2).
- The client in a session by default: decode on frame arrival + `Present(0)` (`apps/client/src/live.rs`), a single
  `MsgWaitForMultipleObjects` wait on the network event, window messages and the waitable swapchain (the latter —
  only when there is something to show: the wait takes a slot). The old "wait for vsync, then decode" loop (`--vsync-loop`)
  was removed in a refactoring; `--check-cursor` runs in this loop.
  **A decoded frame is shown before the next one is decoded** (`Decode::UntilPicture` / `Hold`, `HOLD_MAX` 25 ms):
  decoding 4K real video on Pascal ≈ 13 ms, frames through WG arrive in bursts, and previously a burst was decoded
  in one call — the first frame was not shown (through WG skips 4.8–14.6 % → 0 % after the change; p50 +1 ms,
  p95 +2–3 ms). Behind by more than a frame (start, a network freeze — then frames arrive at once) — the frame does not wait for
  the swapchain: it is shown if the swapchain is free right now, otherwise the next one is decoded (previously catching up skipped everything
  to the freshest: through WG 0–5 frames in 2.5 min; the age of the picture on screen did not change). Freezes for checks —
  `lumen-client --sim-stall <every ms>,<length ms>`.
  Analysis of skips and delays — `lumen-client --frame-log <file>` (D/P/R lines, format — in `--help`).
- The `input` test `inject_cursor_and_key` moves the real mouse and fails on `SetCursorPos` while an elevated window is in front (for example `tools\dev-service.ps1`): UIPI
  (error code 0 in the test, 203 outside it). Not a defect: switch to another window.
- Frame rate limits distort latency measurements: a 90 fps limit in the NVIDIA control panel on a 120 Hz monitor made
  `Present` block for 11.0 ms, and latencies measured under it were inflated (4K p50 in mode 3: 24–29 → 18.9 ms without it). A frame limit at the client gives a "sticky" cursor on
  the main window of mode 3. Host at 60 fps and the monitor's rate: the phase between them is fixed within a run and random between runs —
  end-to-end latency "jumps" by 5–10 ms from launch to launch. Compare variants with several runs.
- Modal window loops (dragging, the system menu) do not return control to the client loop — decode and show
  during that time are done by the window timer (`Window::pump_with`, `MODAL_TIMER`). Check — `lumen-client --test-move-loop`
  (the cursor jumps to the window's title bar).
- Monitor capture: one frame per 1/fps interval (`Source::grab`) — the first image change in the interval or
  a duplicate at its end. `AcquireNextFrame` also returns on a mere pointer movement (`PointerOnly`) — do not make a frame
  from it (previously the host produced up to 110 fps on a 90 Hz monitor and spent the bitrate cap).
- Decode latency: the first AU ~40 ms (decoder creation), then p50 ~5 ms on synthetic 4K, ~13 ms on 4K
  real video ≈ 40 Mbit/s (GTX 1080 Ti) — almost the whole frame interval. The frame comes out of the same
  `decode()` call (display delay 0, `CUVID_PKT_ENDOFPICTURE`).
- Mock snapshots (`tools/overlay-check/snapshots.mjs`) and focus: Chrome draws the `:focus-visible` ring on an element focused from
  a script (the default button in `Dialog`) or not **depending on the history of previous pages in the same tab** —
  the code may not change while a snapshot changes because of edits in scenes captured earlier (seen on the client monitor dialog,
  4 English snapshots). Check with a CDP probe: `document.activeElement` and `matches(':focus-visible')`. Clean up a probe with its own
  vite: a live `esbuild.exe` blocks `npm ci` in `deploy\build-installer.ps1` (EPERM).
- `install.sh` and nginx: `nginx -s reload` (and `systemctl reload`) are not instant — new workers pick up the config a little later;
  a request right after the reload goes to the old config (the default site, 404 on the Let's Encrypt check). The script
  waits 2 s and before certbot checks by itself that nginx serves a file from the webroot (`lumen-probe`).
- `install.sh` in a container (checks in Docker) and in WSL — no systemd: `docker.io` on Ubuntu 24.04 does not install an init script
  for `service` — the script starts `dockerd`/`nginx` itself. Check — a privileged `ubuntu:22.04/24.04` with a volume on
  `/var/lib/docker` (otherwise overlay2 on overlay does not work); certbot — a stub that checks the webroot through nginx.
- `install.sh` and `set -o pipefail`: not `cmd | grep -q` — `grep -q` exits on the first match, the writer gets
  SIGPIPE, and the whole check is "false" (happened in 0.6.8: `docker logs | grep -q` in `start_server`). Compare against output:
  `[[ "$(cmd)" == *…* ]]`, `[ -n "$(cmd)" ]`, `grep -q … <<<"$x"`. Caught by `koalaman/shellcheck:latest` (SC2337), `:stable` — not.
  Check scenarios in Docker — `tools/install-check/`.
- `printf '%-22s'` in bash counts **bytes**: Cyrillic labels get misaligned. In `install.sh` — `LC_ALL=C.UTF-8` and `row`
  (alignment by `${#s}` — characters).
- Trust by fingerprint (`signal::pin`): the "trust?" window only for a self-signed certificate at a **local** address
  (IP, a name without dots, `.lan`, `.local`, `.internal`, `.home.arpa`); a domain with a broken certificate — an error, otherwise users
  get trained to accept a substitution. The fingerprint is SHA-256 of the SPKI (not of the certificate): reissuing for a new IP does not change it.
- **Windows Sandbox: the host does not work** (0.7.0): the user there is in a remote session, there is no console one
  (`WTSGetActiveConsoleSessionId` = `u32::MAX`) — the service does not start the worker process. The installer, the service and the client
  are checked in the sandbox (`tools/installer-sandbox`), the host update — only on real Windows.
- `localhost` in tests resolves to `::1` first, and `SignalClient::connect` takes the first address: a server on `127.0.0.1` gives
  "connection refused" before the certificate check. In tests — `127.0.0.1`.
- **Installer: restarting elevated for the host** (0.7.0): `ShellExec('runas', …)` from an Inno script does not pass an owner window
  and asks for a shell without UI — UAC was not shown, "Next" did nothing (not visible in the sandbox). Call `ShellExecuteW`
  with `WizardForm.Handle`; code 5 — UAC declined, anything else — the `ElevateFailed` message.
