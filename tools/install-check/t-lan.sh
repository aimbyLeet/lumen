#!/bin/bash
# Inside a fresh ubuntu container (privileged, /var/lib/docker a volume): the LAN mode end to end.
set -u
A=/arch
pass=0 fail=0
check() { if eval "$2" >/dev/null 2>&1; then echo "OK   $1"; pass=$((pass+1)); else echo "FAIL $1"; fail=$((fail+1)); fi; }

export DEBIAN_FRONTEND=noninteractive
cd /tmp || exit 1
tar xzf $A/lumen-server-0.6.8-test.tar.gz
tar xzf $A/lumen-server-0.6.9-test.tar.gz
# The installer the archives carry (the workspace's version).
SETUP="$(basename /tmp/lumen-server-0.6.8-test/site/lumen-setup-*.exe)"

echo "=== install (lan, docker from Ubuntu's repository)"
sh /tmp/lumen-server-0.6.8-test/install.sh --mode lan --address 10.20.30.40,lumen.lan --yes --lang ru 2>&1 | tail -22
check "docker installed by the script" "command -v docker"
check "container running" "[ \"\$(docker inspect -f '{{.State.Status}}' lumen-server)\" = running ]"
check ".env is 600" "[ \"\$(stat -c %a /opt/lumen-server/.env)\" = 600 ]"
check "landing over https on 443" "curl -sk https://127.0.0.1/ | grep -q lumen-setup-"
check "installer downloadable" "[ \"\$(curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1/$SETUP)\" = 200 ]"
check "admin over https" "[ \"\$(curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1/admin/)\" = 200 ]"
check "http 80 redirects" "curl -s -D - -o /dev/null http://10.20.30.40/ --connect-to 10.20.30.40:80:127.0.0.1:80 | grep -qi '^location: https://10.20.30.40/'"
check "signaling TLS on 7443" "echo | openssl s_client -connect 127.0.0.1:7443 2>/dev/null | grep -q 'CN *= *Lumen server'"
check "cert names: IP and lumen.lan" "echo | openssl s_client -connect 127.0.0.1:7443 2>/dev/null | openssl x509 -noout -ext subjectAltName | grep -q 'IP Address:10.20.30.40, DNS:lumen.lan'"
FP1=$(docker exec lumen-server lumen-server fingerprint /data/tls/key.pem)
FPO=$(echo | openssl s_client -connect 127.0.0.1:7443 2>/dev/null | openssl x509 -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -hex | awk '{print toupper($2)}' | sed 's/.\{4\}/& /g; s/ $//')
check "fingerprint = openssl's ($FP1)" "[ \"$FP1\" = \"$FPO\" ]"
docker exec lumen-server lumen-server override global 30 40 >/dev/null
check "install.sh copied to /opt" "[ -x /opt/lumen-server/install.sh ]"

echo "=== status"
/opt/lumen-server/install.sh --mode status --lang en 2>&1 | head -12

echo "=== update 0.6.8 -> 0.6.9"
/tmp/lumen-server-0.6.9-test/install.sh --mode update --yes 2>&1 | tail -8
check "image 0.6.9 runs" "docker inspect -f '{{.Config.Image}}' lumen-server | grep -q 0.6.9-test"
check "old image removed" "! docker image ls | grep -q 0.6.8-test"
check "same fingerprint after update" "[ \"\$(docker exec lumen-server lumen-server fingerprint /data/tls/key.pem)\" = \"$FP1\" ]"
check "database kept (override 30 fps)" "docker exec lumen-server lumen-server override show | grep -q '30 fps'"
check "landing still there" "curl -sk https://127.0.0.1/ | grep -q lumen-setup-"

echo "=== install again over it: refused"
check "a second install is refused" "! /tmp/lumen-server-0.6.9-test/install.sh --mode lan --address 10.20.30.40 --yes"

echo "=== remove, keep data"
/opt/lumen-server/install.sh --mode remove --keep-data --yes 2>&1 | tail -4
check "container gone" "! docker inspect lumen-server"
check "images gone" "! docker image ls | grep -q lumen-server"
check "data kept" "[ -f /opt/lumen-server/data/tls/key.pem ] && [ ! -f /opt/lumen-server/.env ]"

echo "=== install again: the same key"
/tmp/lumen-server-0.6.9-test/install.sh --mode lan --address 10.20.30.40 --admin-password 'secret-pass-1' --yes 2>&1 | tail -3
check "same fingerprint after reinstall" "[ \"\$(docker exec lumen-server lumen-server fingerprint /data/tls/key.pem)\" = \"$FP1\" ]"
check "chosen admin password works" "curl -sk -H 'Content-Type: application/json' -d '{\"user\":\"admin\",\"password\":\"secret-pass-1\"}' https://127.0.0.1/admin/api/login | grep -q ok"
check "bad password refused" "! /opt/lumen-server/install.sh --mode lan --admin-password 'a\$b' --yes"

echo "=== remove with data"
/opt/lumen-server/install.sh --mode remove --delete-data --yes 2>&1 | tail -3
check "/opt/lumen-server gone" "[ ! -e /opt/lumen-server ]"
check "nothing listens on 443" "! ss -Hltn 'sport = :443' | grep -q ."

echo "RESULT: $pass passed, $fail failed"
