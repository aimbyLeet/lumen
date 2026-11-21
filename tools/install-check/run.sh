#!/bin/bash
# install.sh checked in Docker: privileged Ubuntu containers with Docker inside, two test archives built
# from the working tree. Run from Git Bash on this machine (Docker Desktop); docs/pitfalls.md, install.sh.
#   tools/install-check/run.sh build                 shellcheck + bench-out\srv\lumen-server-0.6.8-test / 0.6.9-test.tar.gz
#   tools/install-check/run.sh <scenario> <ubuntu>   a fresh container runs t-<scenario>.sh; prints OK / FAIL lines
# Scenarios (as checked 2026-11-06): lan 24.04 (25), nginx 24.04 (26), clean 22.04 (9), old 24.04 (13: a server
# installed by the guide of 0.6.7, dist\lumen-server-0.6.7.tar.gz, updated by the test archive).
# Containers stay (lt-<scenario>) for a look: docker rm -f -v lt-lan ...; the test images: docker rmi lumen-server:0.6.8-test ...
S="$(cd "$(dirname "$0")" && pwd)"
cd /c/dev/lumen || exit 1
if [ "$1" = build ]; then
    MSYS_NO_PATHCONV=1 docker run --rm -v "C:\\dev\\lumen\\deploy\\server:/mnt:ro" koalaman/shellcheck -x install.sh entrypoint.sh && echo SHELLCHECK-CLEAN || exit 1
    for v in 0.6.8-test 0.6.9-test; do
        powershell.exe -NoProfile -Command "& .\deploy\build-server.ps1 -Version $v -Dist bench-out\srv *>&1 | Select-Object -Last 1" | grep -a tar.gz
    done
    exit 0
fi
name=lt-$1
docker rm -f -v "$name" >/dev/null 2>&1
MSYS_NO_PATHCONV=1 docker run -d --privileged --name "$name" -v "C:\\dev\\lumen\\bench-out\\srv:/arch:ro" \
    -v "C:\\dev\\lumen\\dist:/dist:ro" -v "$(cygpath -w "$S"):/t:ro" -v /var/lib/docker "ubuntu:$2" sleep infinity >/dev/null
MSYS_NO_PATHCONV=1 docker exec "$name" bash "/t/t-$1.sh" 2>&1 | grep -av debconf
