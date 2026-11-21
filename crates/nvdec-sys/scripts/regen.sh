#!/usr/bin/env bash
# Regenerates src/bindings.rs from FFmpeg's nv-codec-headers (CUDA driver API
# subset + NVDEC / cuvid parser). Requires: git, bindgen-cli, libclang.
# After regenerating, re-check NVDECAPI_* and the loaded symbol list in src/lib.rs.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-n13.0.19.1}"
TMP="$(mktemp -d)"
git clone --depth 1 --branch "$TAG" https://github.com/FFmpeg/nv-codec-headers.git "$TMP/nvh"
D="$TMP/nvh/include/ffnvcodec"
printf '#include "dynlink_cuda.h"\n#include "dynlink_nvcuvid.h"\n' > "$D/lumen_nvdec.h"
RES="$(clang -print-resource-dir 2>/dev/null || true)"
EXTRA=()
[ -n "$RES" ] && EXTRA+=(-isystem "$RES/include")

# Windows target on purpose: the headers use `unsigned long`, which is 32-bit
# on Windows (LLP64) but 64-bit on Linux.
bindgen "$D/lumen_nvdec.h" -o "$HERE/src/bindings.rs" \
  --allowlist-type 'CU.*|cuda.*|tcu.*|PFN.*|_CU.*|CUVID.*|_CUVID.*' \
  --allowlist-var 'CU.*|cuda.*|NVDEC.*' \
  --no-prepend-enum-name --default-enum-style consts \
  --no-doc-comments --use-core --no-layout-tests \
  -- -x c --target=x86_64-pc-windows-msvc "${EXTRA[@]}"

sed -i 's/\r$//' "$HERE/src/bindings.rs"
cargo fmt -p nvdec-sys
echo "Done."
