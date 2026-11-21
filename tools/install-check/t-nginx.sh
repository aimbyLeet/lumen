#!/bin/bash
# A VPS with nginx and another site (Ubuntu 22.04): install, status, update, removal.
set -u
. /t/t-common.sh
prepare nginx docker.io docker-compose-v2
other_site
cd /tmp && tar xzf /arch/lumen-server-0.6.8-test.tar.gz && tar xzf /arch/lumen-server-0.6.9-test.tar.gz
SETUP="$(basename /tmp/lumen-server-0.6.8-test/site/lumen-setup-*.exe)"

echo "=== a domain nginx already serves: refused"
check "other.test refused" "! /tmp/lumen-server-0.6.8-test/install.sh --mode nginx --domain other.test --yes"
check "nothing installed" "[ ! -e /opt/lumen-server/.env ]"

echo "=== certbot fails: nothing of ours stays"
touch /tmp/certbot.fail
check "install fails" "! /tmp/lumen-server-0.6.8-test/install.sh --mode nginx --domain lumen.test --yes"
rm -f /tmp/certbot.fail
check "no site of ours left" "[ ! -e /etc/nginx/sites-enabled/lumen.test ]"
check "nothing installed after the failure" "[ ! -e /opt/lumen-server/.env ]"
check "the other site fine after the failure" "[ \"\$(curl -s http://other.test/)\" = OTHER ] && nginx -t"

echo "=== install (nginx)"
/tmp/lumen-server-0.6.8-test/install.sh --mode nginx --domain Lumen.Test --email admin@example.com --yes --lang ru 2>&1 | grep -v debconf | tail -16
check "the challenge went through nginx" "grep -q 'challenge ok for lumen.test' /tmp/certbot.log"
check "certbot got the e-mail and the hook" "grep -q -- '--email admin@example.com' /tmp/certbot.log && grep -q -- '--deploy-hook' /tmp/certbot.log"
check "landing https 200" "[ \"\$(https_code lumen.test /)\" = 200 ]"
check "landing names the installer" "curl -sk --resolve lumen.test:443:127.0.0.1 https://lumen.test/ | grep -q $SETUP"
check "admin https 200 (through nginx)" "[ \"\$(https_code lumen.test /admin/)\" = 200 ]"
check "admin login through nginx" "curl -sk --resolve lumen.test:443:127.0.0.1 -H 'Content-Type: application/json' -d \"{\\\"user\\\":\\\"admin\\\",\\\"password\\\":\\\"\$(sed -n 's/^LUMEN_ADMIN_PASSWORD=//p' /opt/lumen-server/.env)\\\"}\" https://lumen.test/admin/api/login | grep -q ok"
check "http redirects to https" "curl -s -D - -o /dev/null http://lumen.test/ | grep -qi '^location: https://lumen.test/'"
check "the other site untouched" "[ \"\$(curl -s http://other.test/)\" = OTHER ]"
check "nginx -t ok" "nginx -t"
check "our config is marked" "grep -q '^# Lumen server (install.sh)' /etc/nginx/sites-available/lumen.test"
check "signaling uses the domain's certificate" "echo | openssl s_client -connect 127.0.0.1:7443 -servername lumen.test 2>/dev/null | grep -q 'CN *= *lumen.test'"
check "admin port only on 127.0.0.1" "ss -Hltn 'sport = :7480' | grep -q '127.0.0.1:7480' && ! ss -Hltn 'sport = :7480' | grep -q '0.0.0.0:7480'"

echo "=== status"
/opt/lumen-server/install.sh --mode status --lang ru 2>&1 | head -9

echo "=== update"
/tmp/lumen-server-0.6.9-test/install.sh --mode update --yes 2>&1 | tail -5
check "0.6.9 runs" "docker inspect -f '{{.Config.Image}}' lumen-server | grep -q 0.6.9-test"
check "admin after update" "[ \"\$(https_code lumen.test /admin/)\" = 200 ]"
check "the other site still untouched" "[ \"\$(curl -s http://other.test/)\" = OTHER ]"

echo "=== remove"
/opt/lumen-server/install.sh --mode remove --delete-data --yes 2>&1 | tail -5
check "our site gone" "[ ! -e /etc/nginx/sites-enabled/lumen.test ] && [ ! -e /etc/nginx/sites-available/lumen.test ] && [ ! -e /var/www/lumen.test ]"
check "the other site still works" "[ \"\$(curl -s http://other.test/)\" = OTHER ]"
check "nginx -t ok after removal" "nginx -t"
check "the certificate kept (default: no)" "[ -d /etc/letsencrypt/live/lumen.test ]"
check "/opt/lumen-server gone" "[ ! -e /opt/lumen-server ]"
echo "RESULT: $pass passed, $fail failed"
