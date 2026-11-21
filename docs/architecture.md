# Architecture

## Fixed decisions

| Area | Decision | Why |
|---|---|---|
| Purpose | Remote access to work machines for small studios and teams; stable and comfortable work first, color matters but not at grading level (decided 2026-11-08; originally "remote color work, not games") | Sharpness and stability, then latency |
| Platform | Windows 10/11, NVIDIA on host and client | One NVENC/NVDEC stack, predictable hardware |
| Format | 4:2:0, **10-bit**, BT.709, **full range** | Compatibility and bitrate; 10 bits against banding |
| Chroma siting | "left" (HEVC type 0, AV1 CSP_VERTICAL) | Default standard; filter [1,2,1]×[1,1] |
| Codec | HEVC Main10 | No AV1 (decided 2026-11-04) |
| HDR | No | SDR content; groundwork — capture in R10G10B10A2/FP16 |
| Target scenario | Office: up to 40 hosts, office and VPS links of 1 Gbit/s each, users at 100 Mbit/s; 10 × 4K, 10 × 2K, 20 × FHD, 30/60 Hz by choice | The main use case (decided 2026-10-23) |
| Bitrate | Static bitrate cap per session (server config, by resolution and frame rate); pacing for the client's link | 4K60 in motion with CQP does not fit into 100 Mbit/s; the sum of sessions is limited by the office/VPS link |
| Loss recovery | Reference frame invalidation (RFI, DPB 16): the next P-frame references the last intact frame at the client; IDR — at session start | No frame size spikes (4K IDR ≈ 1.2 MB ≈ 100 ms at 100 Mbit/s); bit-exact recovery from the first frame. NVENC intra refresh was tested and rejected: it does not recover deterministically |
| Network | The user's WireGuard or Tailscale; host and client connect directly by their address in that network (decided 2026-10-24) | No own relay or NAT traversal: a WG server at the host or client gives a direct path, on a VPS — the same relay done by WG |
| Server | VPS (Ubuntu), Docker, domain + Let's Encrypt (or a local network with its own certificate): device registration, access, busy state, signaling (TLS), web admin (HTTPS) | Video does not go through the server |
| Access | Devices register themselves; one token per host registration (the machine); a client's first access to a host is approved by the administrator in the admin | No manual accounts; knowing the token without approval gives nothing |
| Security | Noise `IKpsk2`: `psk` is derived from the token, the server does not know the token; the host accepts only sessions announced by the server; the "device ↔ server" channel is TLS | The server can deny access but cannot grant it alone |
| UI | React: host and client apps on Tauri v2, the admin is web; the video window is native | Details — `docs/decisions.md` |
| Language | Rust | Memory safety in a network service running as SYSTEM |

## Host pipeline

```
DXGI Desktop Duplication (BGRA8, VRAM)
  └─ CopyResource → own BGRA texture
       └─ shader BGRA → P010 (2 passes: luma R16, chroma R16G16)
            └─ NVENC (registered D3D11 texture, no copies)
                 └─ HEVC access unit → transport
```

The cursor is not part of the image: Desktop Duplication returns it separately, and the client draws it itself
(this removes the cursor latency from what the user feels).

## Stages

Stages 1–3 are the **first working version (MVP)**: video + keyboard/mouse between two machines over the
internet P2P, with authorization on the server.

1. **Quality stand** — done: ΔE/bitrate/latency figures on real content.
2. **Transport + client** (one machine, loopback): UDP protocol
   (fragmentation, FEC, pacing), NVDEC client with a 10-bit swapchain and 1:1 output,
   mouse/keyboard, cursor on the client. The host is a regular process in the user's session.
3. **Server + P2P + E2E**:
   - `apps/server` in Rust in a Docker container (Dockerfile + docker-compose, SQLite in a volume)
     on an Ubuntu VPS. Ports: TCP — signaling over TLS (Let's Encrypt certificate for the domain;
     how to obtain and renew it in the container — to be decided when planning the stage), UDP — relay.
   - Accounts: `lumen-server user add <login>` issues a token; host and client are different accounts;
     the "client → host" binding is set on the server by hand.
   - Host and client config: server address (domain), login, token.
   - Each device has its own key pair; the host admits only keys from a local allowlist —
     the server introduces devices but does not grant access. P2P traffic is encrypted (Noise, `snow`).
   - Connection through the relay: after authorization the server creates a session and gives both sides one-time
     tokens; each side registers its address on the relay's UDP port and keeps the NAT open with keepalives; the relay
     forwards the session's datagrams to the other side. Foreign packets are dropped by E2E at the ends.
   - Before implementing — measure the VPS link and latency (iperf3 / ping from both sides).
   - Bitrate cap and intra refresh (office scenario): NVENC — VBR with `targetQuality` and `maxBitRate`
     (quality like CQP until the cap is hit), a small VBV; on loss — `nvEncInvalidateRefFrames`
     instead of IDR. 30/60 Hz chosen on the client.
   - The relay must sustain ≈ 1 Gbit/s each way (≈ 100k datagrams/s): batched I/O,
     a container with `network_mode: host` (UDP through docker-proxy will not do); VPS pps measured with `iperf3 -u`.
   - "Direct address" mode (like the then `--connect`): connecting by IP without the server — for debugging.
   - One client per host at a time; automatic reconnection.
   - **Done when:** a client on a second machine in another network, after authorization on the server, connects
     to the host through the relay and works (video + input); latency and VPS load — in figures.

After the MVP:

4. **Product: access, admin, UI**: relay dropped in favor of WG/Tailscale, self-registration
   of devices, access approval by the administrator, web admin, host and client apps, a single
   installer.
5. **Host service**: Windows Service (SYSTEM) + an agent in the user's session, the logon screen and UAC
   (switching between the Default/Winlogon desktops).
6. **Static quality**: bringing static content to a low QP (QP delta map from dirty rects). Client EDID/ICC and 4:4:4 —
   not done (decided 2026-11-04).
7. ~~**Network in bad conditions**~~ — cancelled (decided 2026-11-04): no automatic degradation, the frame rate and
   bitrate cap are chosen by the client's user.

Stages 1–3 are described above in their original form (relay on a VPS); the relay and the direct mode were removed at stage 4.

**State:** stages 1–6 are done; stage 7 is cancelled (the frame rate and bitrate are chosen by the client's user).
Beyond the plan: audio, clipboard, files and pictures through the clipboard, multiple monitors and virtual monitors
(Lumen VDD), host without monitors, auto-update, new UIs for all applications, overlays behind an API. Current state — `CLAUDE.md`.

## Crates and applications

The project map with all crates, applications and modules — `docs/MAP.md`.
