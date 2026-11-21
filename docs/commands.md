# Commands: build, checks, server, scripts

The most frequent ones are also in `CLAUDE.md`, "Commands". All paths are relative to the repository root.

```powershell
cargo build --release
tools\check.ps1                     # everything before a commit, output — only errors and the summary (-Web, -NoWeb, -Full)
cargo test --workspace
cargo nextest run --workspace --status-level fail --final-status-level fail   # faster, output — only failures and the summary;
                                    # timing-sensitive tests run one at a time (.config/nextest.toml)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# Test stand (on a host with NVIDIA):
target\release\lumen-bench --list
target\release\lumen-bench --codec hevc --rc cqp --qp 16 --seconds 10 --verify
ffmpeg -i bench-out\stream.hevc -fps_mode passthrough -pix_fmt yuv420p10le -f rawvideo bench-out\decoded.yuv
target\release\lumen-compare capture --dir bench-out
target\release\lumen-decode --reference bench-out\decoded.yuv   # client decoder: must be BIT-EXACT
target\release\lumen-netsim --loss 1 --out bench-out\netsim.hevc   # transport: IDENTICAL to input
target\release\lumen-decode --rgb-every 120 --source-dir bench-out   # client shader vs reference + ΔE
target\release\lumen-client --play bench-out\stream.hevc --borderless --verify-every 60   # 0 px beyond ±1

# A session always goes through the server (there is no direct mode). Host: --server --server-ca, registers itself (host.creds),
# token — `lumen-host token` (host.token; --new — change it). Client: --server --server-ca --token <host token>,
# registers itself (client.creds), files a request and waits for the admin's approval. Files are in the data folder
# (LUMEN_DATA overrides it: several devices on one machine).
# Everything on one machine at once (local server, host, client, approval; logs and data — bench-out\local):
tools\local-session.ps1 -HostArgs "--source synthetic" -ClientArgs "--borderless --frames 900 --barcode-every 30 --sim-bandwidth 100"
# Bit-exact recovery: client frame hashes = offline decode of the host stream (full-screen window, slower than real time)
tools\local-session.ps1 -HostArgs "--source synthetic --seconds 18 --dump-stream bench-out\host.hevc" -ClientArgs "--borderless --frames 900 --sim-loss 5 --frame-hashes bench-out\client.hash"
target\release\lumen-decode --input bench-out\host.hevc --hashes bench-out\host.hash   # compare by frame number
# Reference frame loss without a network: a second stream without frames A..B + RFI
target\release\lumen-bench --source synthetic --drop 290..300 --invalidate-at 300
# Static QP map: the "mixed" scene, codec PSNR inside and outside the window (line D)
target\release\lumen-bench --source synthetic --size 1920x1080 --scene mixed --rc cq --qp 10 --max-mbps 30 --cap-control --static-qp-map --dump-every 60 --out-dir bench-out\qpmap\m
target\release\lumen-compare capture --dir bench-out\qpmap\m --no-heatmaps --region 865,325,1825,865
target\release\lumen-bench --source desktop --no-dumps --seconds 30   # share of changed blocks on a real desktop, no pictures on disk
tools\local-session.ps1 -HostArgs "--source synthetic --scene mixed" -ClientArgs "--borderless --frames 600"   # map on by default (--static-qp 10); --static-qp off — without it
# Input: the logs must match byte for byte
tools\local-session.ps1 -HostArgs "--source synthetic --input-log host-input.log" -ClientArgs "--borderless --frames 1200 --input-script 3000 --clipboard off"
fc /b client-input.log host-input.log
# Clipboard both ways (5 % loss): client S lines = host R lines and vice versa (absolute log paths)
tools\local-session.ps1 -HostArgs "--source synthetic --clipboard-script 120 --clipboard-log $PWD\bench-out\clip\host.log" -ClientArgs "--borderless --frames 900 --sim-loss 5 --clipboard dry-run --clipboard-script 120 --clipboard-log $PWD\bench-out\clip\client.log"

# Server (local): development certificate, accounts, start
target\release\lumen-server --db bench-out\srv\lumen.db dev-cert --out bench-out\srv
target\release\lumen-server --db bench-out\srv\lumen.db serve --cert bench-out\srv\cert.pem --key bench-out\srv\key.pem
# Administrator (without the web admin): devices register themselves, access is by request
target\release\lumen-server --db bench-out\srv\lumen.db access                  # requests (pending) first
target\release\lumen-server --db bench-out\srv\lumen.db approve <client> <host>  # ids from `clients` / `hosts`
target\release\lumen-server --db bench-out\srv\lumen.db unlink <client> <host>   # a running session breaks within ≤ 0.5 s
target\release\lumen-server --db bench-out\srv\lumen.db override global 30 40     # or client <id> 60 auto; cap only — off 40; frame rate only — 30; clear — off
# Server on a local network (no domain): own certificate (key once, prints the fingerprint), devices — --server-pin
target\release\lumen-server self-cert --dir bench-out\srv\tls 192.168.1.10 127.0.0.1
target\release\lumen-server fingerprint bench-out\srv\tls\key.pem
target\release\lumen-server --db bench-out\srv\lumen.db serve --cert bench-out\srv\tls\cert.pem --key bench-out\srv\tls\key.pem `
    --admin-listen 0.0.0.0:443 --redirect-listen 0.0.0.0:80 --site <landing page folder>   # + LUMEN_ADMIN_USER/PASSWORD
target\release\lumen-host --server 192.168.1.10:7443 --server-pin "<fingerprint>" ...     # client: the same; Lumen Client keeps it in servers\<server>.pin

# Release: the installer (host or client, chosen while installing) and the server archive
deploy\build-installer.ps1                                               # dist\lumen-setup-<v>.exe; Lumen VDD from third_party\lumen-vdd (SHA256SUMS checked)
deploy\build-installer.ps1 -VddPackage <lumen-vdd>\build\package         # with a Lumen VDD package built from its own repository
# Server archive: install.sh (menu on the server), compose\public.yml and lan.yml, image, landing page
deploy\build-server.ps1                                                  # dist\lumen-server-<v>.tar.gz, workspace version; build after the installer
deploy\build-server.ps1 -Version 0.6.8-test -Dist bench-out\srv-test     # trial build: the release in dist\ is not touched
# On the server: sudo ./lumen-server-<v>/install.sh (menu) or --mode clean|nginx|lan|update|status|remove [--yes]; --help
# User manuals: manual\manual_EN.html, manual\manual_RU.html (not copied to dist)
# Linux check of the cross-platform crates (Docker Desktop; run from the repository root)
# (RUSTUP_TOOLCHAIN — the image's compiler: otherwise rust-toolchain.toml makes rustup download the latest stable)
docker run --rm -v "${PWD}:/src:ro" -v lumen-cargo:/usr/local/cargo/registry -v lumen-target:/target -e CARGO_TARGET_DIR=/target -e RUSTUP_TOOLCHAIN=1.99.1 -w /src rust:1.99 sh -c "rustup component add clippy && cargo clippy --locked -p lumen-server -p signal -p protocol -p transport --all-targets -- -D warnings"

# Application icon (shortcuts, taskbar, tray, video window, installer, admin tab) — from icons\main_icon.png:
tools\make-icons.ps1      # then rebuild: the installer (deploy\build-installer.ps1), for the admin tab — the server archive

# Without a GPU: 4:2:0 loss on any picture
target\release\lumen-compare image --png shot.png
```

Before a commit: `cargo fmt`, `cargo clippy` without warnings, `cargo test` green (`tools\check.ps1` does all of it).
