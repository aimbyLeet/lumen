#!/usr/bin/env bash
# Regenerates src/bindings.rs and src/guids.rs from FFmpeg's nv-codec-headers.
# Requires: git, python3, bindgen-cli (`cargo install bindgen-cli`), libclang.
# After regenerating, re-check the *_VER constants in src/lib.rs.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-master}"
TMP="$(mktemp -d)"
git clone --depth 1 --branch "$TAG" https://github.com/FFmpeg/nv-codec-headers.git "$TMP/nvh"
H="$TMP/nvh/include/ffnvcodec/nvEncodeAPI.h"
# The header includes <stdlib.h> but uses nothing from it; an empty stub lets
# this run on hosts without Linux libc headers (e.g. Windows + Git Bash).
mkdir -p "$TMP/stub" && : > "$TMP/stub/stdlib.h"
# libclang loaded by bindgen does not always find clang's builtin headers
# (stdint.h, stddef.h); point at them explicitly when clang is on PATH.
RES="$(clang -print-resource-dir 2>/dev/null || true)"
EXTRA=(-isystem "$TMP/stub")
[ -n "$RES" ] && EXTRA+=(-isystem "$RES/include")

bindgen "$H" -o "$HERE/src/bindings.rs" \
  --allowlist-type 'NV_ENC.*|_NV_ENC.*|NVENC.*|PNVENC.*|GUID|NV_ENCODE.*|_NVENC.*' \
  --allowlist-var 'NVENC.*|NV_ENC.*|NV_MAX.*' \
  --blocklist-item 'NV_ENC_.*_GUID' \
  --no-prepend-enum-name --default-enum-style consts \
  --no-doc-comments --use-core --no-layout-tests \
  -- -x c --target=x86_64-unknown-linux-gnu "${EXTRA[@]}"

python3 "$HERE/scripts/gen_guids.py" "$H" > "$HERE/src/guids.rs"
# Python on Windows writes CRLF; the repo keeps LF. Committed files are rustfmt'ed.
sed -i 's/$//' "$HERE/src/bindings.rs" "$HERE/src/guids.rs"
cargo fmt -p nvenc-sys
grep -n "_VER " "$H" | grep define
echo "Done. Compare the *_VER macros above with src/lib.rs."
