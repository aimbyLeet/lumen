#!/bin/bash
# A clean VPS (Ubuntu 24.04), ufw on: the script installs Docker and nginx, opens and closes the ports.
set -u
. /t/t-common.sh
prepare ufw
ufw --force enable >/dev/null 2>&1
cd /tmp && tar xzf /arch/lumen-server-0.6.8-test.tar.gz

echo "=== install (clean)"
/tmp/lumen-server-0.6.8-test/install.sh --mode clean --domain lumen.test --yes 2>&1 | tail -14
check "ufw is on" "ufw status | grep -q 'Status: active'"
check "nginx installed by the script" "command -v nginx"
check "docker installed by the script" "command -v docker"
check "landing https 200" "[ \"\$(https_code lumen.test /)\" = 200 ]"
check "admin https 200" "[ \"\$(https_code lumen.test /admin/)\" = 200 ]"
check "ufw: 80, 443, 7443 opened" "ufw status | grep -q '^7443/tcp' && ufw status | grep -q '^443/tcp' && ufw status | grep -q '^80/tcp'"
check "LUMEN_UFW kept" "grep -q '^LUMEN_UFW=80/tcp 443/tcp 7443/tcp' /opt/lumen-server/.env"

echo "=== interactive menu: 5 (status), in Russian"
printf '5\n' | /opt/lumen-server/install.sh --lang ru 2>&1 | head -16

echo "=== remove"
/opt/lumen-server/install.sh --mode remove --yes 2>&1 | tail -4
check "ufw rules closed" "! ufw status | grep -q '7443/tcp'"
check "data kept by default" "[ -f /opt/lumen-server/data/lumen.db ]"
echo "RESULT: $pass passed, $fail failed"
