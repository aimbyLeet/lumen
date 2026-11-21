# Owner's decisions — full text

Fixed product decisions: **do not change without the owner's explicit consent**. A short summary is in `CLAUDE.md`;
here is the full text with all caveats. A new owner's decision goes here (and as one line into the `CLAUDE.md` summary),
with its date. UI strings are quoted in their English form.

- **Concept** (decided 2026-11-08): open source remote access to work machines for small studios
  and teams. The main thing is stable and comfortable remote work; color matters, but not at color-grading level
  (the former wording "focus on accurate color, colorists and retouchers" no longer applies).
- Host and client: **Windows 10/11 + NVIDIA**. Linux/macOS are not a goal.
- Format: **4:2:0, 10-bit, BT.709, full range**, chroma siting "left"
  (HEVC `chroma_sample_loc_type = 0`, AV1 `CSP_VERTICAL`). The protocol has a chroma format field
  (laid down for 4:4:4), but **4:4:4 is not done** (decided 2026-11-04).
- **Color — the standard profile** (decided 2026-11-04): the client monitor's profile is not passed to the host (no client
  EDID/ICC in the virtual display); host and client work in standard sRGB/BT.709.
- Codec: **HEVC Main10**. No B-frames, infinite GOP. **No AV1** (decided 2026-11-04; previously an
  option for RTX 40+): sessions are always HEVC; AV1 remains only in the `lumen-bench` stand and `crates/nvenc`.
- **HDR is not needed.**
- Network (decided 2026-10-24, replaced the relay on the VPS): **the user's WireGuard or Tailscale, no own
  relay.** Host and client connect directly by their address in that network: a WG server at the host or the client (whoever
  has a public IP) gives a direct path, on a VPS — traffic goes through the VPS by WG. The host reports its addresses to the server
  itself, the client tries them in parallel. Relay, NAT traversal, STUN/UPnP — not done.
- Server on a **remote VPS** (Ubuntu, **Docker**, domain + Let's Encrypt): device registration, access, busy state,
  sessions — signaling over TLS (TCP); a **web admin** at `/admin/` (HTTPS, one administrator), the domain root has a landing
  page with downloads (decided 2026-11-02). Video does not go through the server.
  **Behind the VPS's nginx** (decided 2026-10-28: other sites live on its 443): certificate — the VPS's certbot (`--webroot`),
  admin — HTTP on `127.0.0.1:7480` behind nginx (`--admin-behind-proxy`, password brute-force blocking by `X-Real-IP`),
  signaling — the server itself on 7443 with the same certificate from `/etc/letsencrypt` (ro). No own certbot container.
- **Server installation — `install.sh`** from the archive (decided 2026-11-04): the archive is uploaded to `/tmp`, the script
  (bash, Ubuntu 22.04/24.04) is a menu: clean VPS (Docker, nginx, certbot), VPS with nginx and other sites (only its own
  site, rollback on an `nginx -t` error), **local network**, update, status, removal. Docker — `docker.io` from the
  Ubuntu repository. **No backups** (removed from the guide).
- **Server on a local network** (decided 2026-11-04): no domain — **own certificate** for the IP
  (`lumen-server self-cert`: the key is created once in `data/tls` and never changes, the certificate is valid for 100 years);
  landing page and admin — over HTTPS with the same certificate (the browser warns once), HTTP 80 → HTTPS. Device ↔ server —
  still TLS. The programs trust the server by **key fingerprint** (SHA-256 SPKI): on the first connection to a
  self-signed certificate at a local address — **a window with the fingerprint and the user's confirmation** (option "a"),
  then — a check against the remembered one; the key changed — refusal and the window again. A public domain never gets the window.
- **Access** (stage 4): devices register themselves; a host registration = **the machine** (decided
  2026-10-25, was "machine + Windows account"): the host is installed on the whole PC, one token per PC for all its
  accounts (visible in Lumen Host under any account, any logged-in user can change the settings and the token); a client =
  an installation. A client's first access to a machine is a request, **approved by the administrator** in the admin (an access is the pair
  "client → machine", it has an ID); rejected repeats are ignored; "Unlink" breaks the session, after that — only a new request. The direct
  mode without a server (`--connect`, `mode=direct`) is removed.
- **Windows service** (stage 5; decided 2026-10-25): the host is a service (LocalSystem),
  the worker process is SYSTEM in the console session, on the current input desktop (logon, lock, UAC). The machine's host is
  always available (logon screen, lock screen, any account — the Windows password is needed); a second user of the PC logs into
  their account via "Switch user"; one session per machine ("the host is busy with another connection"). Ctrl+Alt+Del to the host —
  `Ctrl+Alt+Shift+Del` on the client. After a remote session the host locks Windows: the client closed the window — immediately,
  connection lost — after 10 s, unless the same client came back; for another client — before its session (decided 2026-10-25).
  The Lumen Client setting "Do not lock Windows after the session" (decided 2026-11-03, off by default, protocol v12 —
  `keep_unlocked` in `Hello`): after such a client's session the host does not lock Windows either on window close or on a drop;
  before another client's session it locks; there is no switch in the admin.
  Host switch (decided 2026-10-25): "Turn off the host and quit" in the tray stops the service (nobody can connect),
  starting Lumen Host turns it on; the "Start with Windows" checkbox = the service start type (automatic / manual).
- **Clipboard** (decided 2026-10-25): text, both ways, always on, up to 1 MB (UTF-8); over the direct
  channel (not through the server: it must not see the content, and Ctrl+V goes in the same stream as the text). The client
  sends its clipboard only while the video window is in front; both sides send only what was copied after connecting
  (the client's clipboard from before connecting does not go to the host — decided 2026-10-25).
- **Files and pictures through the clipboard** (decided 2026-10-27): only Ctrl+C
  → Ctrl+V, both ways, no drag and drop. ≤ 32 MB — as clipboard; larger — transferred right on pressing Ctrl+V /
  Shift+Insert (not from the "Paste" menu: step 0) into `Downloads\Lumen`, with a transfers overlay (bottom right corner,
  "Cancel" on each, no close cross, disappears after 5 s; `Ctrl+Alt+Shift+D` — show/hide, session history; the overlay does not open
  clipboard files itself, they have one permanent "Clipboard" line; "Cancel all" when more than one job is running — decided 2026-11-02;
  2026-11-12: the downloads list holds the whole session history with mouse wheel scrolling, no line limit; the folder of one paste is
  one line "Folder · N files" with common progress and one "Cancel", single files — their own lines). While a transfer is running, files
  matter more than video; the file speed cap is on the server (100 Mbit/s). The files switch is only in the admin (global and per client),
  it does not affect text and pictures. A host without a logged-in user does not transfer files. Version 0.3.3 (0.3.0 + polishing
  after the owner's check, protocol v9).
- **Virtual monitors** (decided 2026-10-29): the driver is a separate
  repository **Lumen VDD** (https://github.com/aimbyLeet/lumen-vdd; its signed release package is vendored in
  `third_party/lumen-vdd/`), installed as an option of the host installer (without it the behavior is as before; the installer without
  the option and Lumen's auto-update **update** an older driver already installed, and do not install it where it is absent — decided 2026-11-12);
  Lumen Host — the "Monitors" section: monitor 1 is the physical one (a PC with one monitor, on), up to 2 virtual ones with a
  resolution from the 16:9/16:10/21:9 list and a scale, 60 Hz, placement — edges touching; the monitors live as long as
  the host service lives; "Create / Apply / Disable / Reset". At most 3 monitors are passed to the client (decided 2026-11-01):
  the primary, real ones by Windows number, then virtual ones; a monitor not on an NVIDIA GPU is skipped; the "Monitors" section is always
  present with the driver, there can be as many virtual ones as make 3 in total (two real + one virtual; three real — none).
  At the client (decided 2026-10-30): one monitor at the host — as before; 2–3 — three modes via
  `Ctrl+Alt+Shift+L`: 1) all in one window as laid out; 2) one monitor per window + a circle switcher on top, the host
  encodes only the shown one, the mouse stops at the edge; 3) a window per monitor, `Ctrl+Alt+Shift+F` — each borderless on
  its own client monitor (two on one — refused). The mode and window memory belongs to the machine card; the host monitors changed
  (between sessions or in the middle) — mode 1 again. Closing any window ends the session.
- **Host without monitors** (decided 2026-11-04): on a host with the Lumen VDD driver Windows does not
  see any physical monitor — when a client connects, the host creates **that client's set** (monitor 1 is
  virtual too, up to 3 in total); without a set, the client chooses the resolution and scale in a pop-up card in Lumen Client
  (prefilled from its own monitor); the set is remembered per client (per Lumen Client installation: a laptop and a PC are different sets),
  changed in Lumen Host on the remote PC during a session; the client has no reset. End of session: the window closed — the monitors
  are parked immediately, a drop — after 10 s (came back — they stay). "Parked" (decided 2026-11-12, instead of "removed"): they stay
  in the driver, disabled in Windows, a display of another driver is enabled (Parsec's fallback — "each with its own"), if there is none —
  the Lumen monitors stay enabled; the next session enables them right away, without a sound or waiting. Therefore Parsec after
  a Lumen session sees a Lumen monitor and does not bring up its fallback — left as is (decided 2026-11-13): disabling the Lumen
  monitors for Parsec means a bare 1024×768 output and lost window positions; hiding a monitor from other capture with the driver
  is impossible (the Windows desktop is shared). Without a physical monitor the machine's "shared" virtual monitors
  are not created; a physical one turned on in the middle of a session — normal behavior (the client set stays saved), turned off —
  the client set (none — the card over the video). "Physical" — on a real adapter (GPU, USB dock; a dummy plug in the port counts as physical);
  displays of other virtual drivers (Parsec VDD etc., adapter `ROOT\`/`SWD\`) are not physical and go dark for the duration of a Lumen session;
  a GPU output without a monitor (neither a monitor device nor a name — Windows keeps a 1024×768 desktop on it when there are no displays;
  a name without a path is a physical monitor that is coming back) and the "Generic Non-PnP Monitor" (`DISPLAY\DEFAULT_MONITOR`, no EDID)
  are not physical either, nothing is drawn on them (decided 2026-11-12), a monitor with EDID but no name is physical; without a physical
  monitor the "shared" monitors in Lumen Host do not change (decided 2026-11-11). Without the
  driver — the status "No monitors" instead of a black screen. The admin needs nothing.
- **No floating dock in the video window** (decided 2026-11-02): modes and "borderless" — via keyboard shortcuts.
- **Overlays** (decided 2026-11-02): a web page over **the host picture in the main window** (the black bars stay free; mode 1 — on the
  picture of monitor 1; size — by the picture's scale; over the whole client window was tried — the owner cancelled it; no lower size
  limit; the mode-2 circles are always visible; the overlay window lives for the whole session), only on the window with the stream of the
  host's primary screen; the look — as in the apps (a block as in the admin, three line charts, circles with Roman numerals).
- **Codes instead of words** (decided 2026-11-02): the core returns codes `err_…` / `note_…` (`code|detail`), the words are in
  `web/client/src/errors.ts`; a new code without words fails `npm test`. Core responses are checked against the TypeScript types:
  `signal::contract`, `web/*/src/contract/*.example.ts`.
- **Cursor** (decided 2026-11-01, after trying the client's system cursor — not liked): the client draws the arrow
  with the host's shape into the frame at the local mouse, with one function (`draw_pointer`) for all modes and monitors; it is not drawn on
  the host (latency).
- **Quality**: the client chooses 30/60 fps and the cap (Auto — the server's table, or 10/20/30/40/60/80/100 Mbit/s);
  the admin can set the frame rate and the cap **independently of each other** (decided 2026-11-02), globally or per client (the client's
  setting wins, per field): what is set, the client does not change, the rest it chooses itself.
- **Uninstall: "Delete user data"** (decided 2026-11-08): a checkbox in the host and client uninstaller,
  off by default; silently — `/PURGE=1`. Erases keys and settings so that a new installation starts from scratch: for the host —
  `%ProgramData%\Lumen` and Lumen Host files in all profiles; for the client — its files in `%APPDATA%\Lumen`,
  `%LOCALAPPDATA%\Lumen` and this user's WebView2 folder. Files of the other program (host and client on one PC) stay.
  The client is installed into the user profile (not Program Files) — for a silent update without UAC; kept so (2026-11-08).
  UAC — only when the host is chosen in the wizard (the wizard restarts elevated), not right at launch: otherwise a regular
  user without the administrator password would hit UAC, and the client would land in the profile of whoever entered the password (2026-11-09).
- **Auto-update** (decided 2026-11-07, cancels "There will be no auto-update" of 2026-11-03;
  version 0.7.0): the programs compare their version with the server's version, the installer comes from the server.
  Outdated — a yellow footer "Your version is out of date. Click to update" (always by a click: confirmation →
  the program closes → silent update → restart; on the host a running session is broken). The client with the checkbox (on by default)
  updates silently at startup; the host with the checkbox (on by default) — silently, only at the moment a newer client tries to connect
  to it (machine busy — no update, and not after the session either). The installer is installed only if SHA-256 matches and the version
  is higher than its own; a signature is postponed (the server is assumed to be on the studio's local network). The update request is separate, with
  a frozen format. The client is installed per user (no admin). Failed — the old version keeps working, visible at the
  client and in the admin. No switch in the admin.
  Versions before 0.7.0 are considered nonexistent (no settings migration).
  Clarifications (2026-11-07): the installer for updates lives **in the server image**
  (`/usr/share/lumen/update`; a copy in `site/` for the landing page stays — double storage is tolerated for now);
  **at most 16** downloads are served at once; the old 0.6.x client from Program Files is neither removed by the installer nor
  warned about — "uninstall the old Lumen first" is written in the 0.7.0 instructions.
  Machine card in Lumen Client (decided 2026-11-09, 0.7.3, signaling v10): the host reports its
  checkbox to the server at login and on every toggle; the client shows in advance what will happen. The button: same version —
  "Connect"; the host is older and updates itself — "Update and connect"; the host is older, checkbox off — "Updates are off"
  (inactive); the host is newer — "Update the client" (inactive). The host version — the second line of the status badge
  (no dot, centered): `0.7.0 → 0.7.3`, `0.7.0 · the host does not update itself`, `0.7.4 · newer than this client`.
- **Names — Latin only** (decided 2026-11-09): the machine name (Lumen Host), the client name (Lumen Client) and the client's
  nickname in the admin — Latin letters, digits, space and printable characters (ASCII 0x20–0x7E), 1–64 characters. One rule —
  `protocol::signal::name_allowed`, in the UIs — `nameNotLatin`; the server always checks. No compatibility with old
  names: installation and registration start from scratch. A Cyrillic computer name is not prefilled into the field.
- **One server per client** (decided 2026-11-07, 0.7.0): first launch — a welcome window (nickname and server);
  the server is a line in the preferences under the nickname; nickname and server are locked while there are machines; "Add machine" and the machine
  line take only the token.
- **Versions** (decided 2026-10-27): one version for the whole project (`Cargo.toml` workspace, also in the
  installer name), raised **by hand before a release**. A client connects only to a host of **exactly the same** version:
  a device reports its version at login (signaling v5, `APP_VERSION`), the server stores it (`devices.version`) and refuses
  the `Connect` (`ConnectFailure::Version` with the host version); the Lumen Client card shows the mismatch in advance.
- **Lumen Client UI** (decided 2026-11-02): three tabs
  (machines, preferences, help), rows of equal 100 px height on all tabs, in preferences and help — a title and up to 3 lines
  of text up to 440 px wide; a machine has only a short status, without explanations or "More"; shortcuts as plain text, without
  frames; own window frame, dark theme only; settings apply immediately; the log — an "Open folder" button; the add dialog has
  only the server and the token. User name: empty on first launch, a machine cannot be added without it, it does not change while there are machines
  (remove all machines). All hub functionality is behind an API (`web/client/src/api.ts`).
- **Lumen Host UI** (decided 2026-11-02): frame, window size and tab bar as in the client; three tabs (machine, monitors,
  preferences), no help; rows only in preferences (autostart, sleep prevention, log —
  "Open folder"), on the machine and monitors tabs — panels and cards, aligned with the row text. Machine: who is connected, with what settings and since when;
  name and server; token; no manual addresses or certificate, the name does not change after registration. Monitors:
  a diagram, placement — by dragging with snapping to edges, a common "On / Off" status on the diagram, disabled ones dimmed; "+", a cross on a
  monitor, a trash can — delete all; without the driver the tab does not open.
  Sleep prevention is a service setting (`apps/host/src/awake.rs`, `power.json`): only timer sleep; manual "Sleep", the lid and monitor
  blanking work.
- **Admin UI** (decided 2026-11-02): look and tab bar as in the client and host; three tabs (clients, machines, settings);
  a "Requests" block on top of the clients and machines tabs. Each client and machine is a separate block, alphabetically:
  collapsed — only the name and status (session yellow, online green, offline gray; the name and icon color is the color of the status
  next to it), expanded — state, the client's settings as three fields in a row, connections; "Unlink" is always visible; settings and
  connections in identical panels. Client name (decided 2026-11-06, instead of "there is no second one" of 2026-11-02): the administrator can
  give a client their own nickname — it exists **only in the admin** and is the main one everywhere there: "AdminNick [ClientNick]" (list,
  requests, connections, sessions; clarified 2026-11-07 — square brackets); it is not shown to the client or in Lumen Host. The pencil — to the
  right of the name, only in the expanded card; opens a window with a field that has an "erase" cross on the right: an empty field + "Save"
  removes the nickname (no separate eraser — 2026-11-07). Search — by both names, one for both tabs.
- **UIs, changes of 2026-11-09 (0.7.4):** Lumen Host's first launch on a clean machine — the same
  welcome page as in Lumen Client (logo on the left, a "Welcome" form: machine name and server,
  "Register"); the server accepted — "All good, here is your token!": the token as dots and dashes (a click shows and hides it,
  **no eye icon**), "Copy" copies it and opens the regular window. A server refusal — the regular "Machine" tab right away. The service
  is off or silent — the regular window with "Turn on the host"; after "Remove registration" in the same run — also the regular one.
  Status badges — **without a dot** everywhere (as on the client's cards). Admin: the header — at the edges of the central column (not the window,
  aligned with the card frames); a client for whom the administrator set something of their own (frame rate, cap, files) — a figure with a
  gear. **No animations**: anime.js was tried in all three UIs and rolled back (`143669e`).
- **Host and client behave the same (decided 2026-11-09, instead of "Register" grayed out while the server is
  unreachable, of 2026-10-28):** the server is checked **once, on the button** ("Register" in Lumen Host, like
  "Continue" in Lumen Client), not while typing. The button is active when a server is entered and the name is Latin; it shows
  "Checking…"; an own certificate — the trust window; the error and what is wrong with the name — in one line under the fields ("Server X:
  not responding"). Do not complicate the logic without a real reason.
- **UI** — React: the host app (with a tray) and the client app — Tauri v2, the admin — web; the video window — native.
  For the future (decided 2026-10-30, since done for the hub and overlays): all UIs and overlays are behind an API,
  changed without touching the core; the video stream and everything related to content transfer stay inside the core and are not editable from outside.
- **Target scenario — office** (decided 2026-10-23): up to 40 host workstations in an office (one
  per user), office link 1 Gbit/s, VPS 1 Gbit/s, users at home at 100 Mbit/s each. Mix: 10 × 4K,
  10 × 2K (2560×1440), 20 × FHD; frame rate 30 or 60 Hz — by the user's choice. The worst case "everyone in motion
  at 60 Hz" ≈ 1 Gbit/s total (estimate from synthetic content) — sessions must not fall apart.
- **Session bitrate cap** — mandatory: the host does not exceed the set bitrate, pacing — for the client's link.
  In static content quality as with CQP, in motion — coarsening within the cap.
  The caps are **static** (not divided dynamically), chosen so that the office worst case fits
  into 1 Gbit/s; set in the server config by resolution and frame rate. Decided 2026-10-24: the studio's
  upstream is 680 Mbit/s (downstream 390), the worst case of the mix with the current caps ≈ 900 Mbit/s with FEC —
  **do not lower** the caps. A dynamic budget (splitting the studio link between sessions) — **not done, at least
  for now** (decided 2026-10-24): the cap is "no more than", in practice a session spends what it needs (4K: static
  ≈ 0.4, desktop 0.4–10 Mbit/s), the worst case only if everyone is in full-screen motion at once. Discussed:
  a budget from an admin figure or auto-detecting capacity from signs at the site's clients; against other uploads from
  the studio (uploads, backups) only QoS on the studio router helps, not our code. Downloads into the studio do not disturb video
  (the other direction).
  There is **no** "internet speed" setting at the client — only FPS and bitrate; the client picks the send pacing itself.
- **Bad network — the user's choice** (decided 2026-11-04, stage 7 cancelled): no automatic degradation
  of the frame rate (or bitrate); the frame rate (30/60) and the bitrate cap are chosen by the client's user within what the
  admin set.
- **Static refinement — the QP map** (stage 6, decided 2026-11-04): blocks unchanged for half a
  second are refined to QP 10, moving ones are encoded as before (QP 16, the delta adapts to the encoder's base QP);
  change regions — from DDA. On by default, `--static-qp off` turns it off.
- **Loss recovery — reference frame invalidation (RFI)** instead of IDR (decided 2026-10-23,
  replaced intra refresh): no frame size spikes (4K IDR ≈ 1.2 MB ≈ 100 ms at 100 Mbit/s). The client reports the
  last intact frame, the host invalidates the rest, the next P-frame references the intact one. DPB 16 frames; a loss
  longer than the window — NVENC makes an I-frame itself. IDR — at session start and on parameter changes. NVENC intra refresh on
  Pascal does not recover the stream deterministically (measured at stage 3).
- Links: the client has ≥ 100 Mbit/s; the office link (and the VPS, if the WG server is there) — the sum of the streams of all sessions.
- Security: E2E between devices — Noise **`IKpsk2`** (`snow`), `psk` is derived from the host token; the server does not know
  the token (it stores only a `lookup` independent of `psk`). The host accepts only sessions announced by the server, and
  only with the right `psk` — the server can deny access but cannot grant it alone. The
  "device ↔ server" channel — **TLS** (domain + Let's Encrypt, `rustls`).
- **Languages — English and Russian** (decided 2026-11-04, preparation for GitHub): Lumen Client, Lumen Host,
  the admin, the installer and the landing page — in two languages, **English by default**. The installer asks for the language first;
  it is also the programs' starting language until the user picks another. The admin and the landing page, until a language is chosen, follow the
  browser language (Russian → Russian, otherwise English). **Choice — with a globe** (2026-11-04): a click switches to the other language right away
  (no list), the choice is remembered; the tooltip names the language it will switch to. Where: in Client, Host and the admin — at the
  right end of the tab bar, at the admin login — in the card header, on the landing page — in the top right corner; there is no "Language"
  item in the preferences. User manuals — `manual/manual_EN.html` and `manual/manual_RU.html`; developer documentation — in English.
- **Machine reboot from the client** (decided 2026-11-13): in the machine card the ⋯ button (instead of the trash can)
  opens "Restart" and the trash can in place of the main button; available always, also during a session. Confirmation — and an
  immediate reboot: no session checks, applications are closed forcibly, Windows updates are not installed. The path —
  through the server (signaling v11: `Reboot` → `RebootWanted`), approved access and the host online are required; no token signature
  (a server able to reboot machines is acceptable). No switches at the host or in the admin. No animation.
