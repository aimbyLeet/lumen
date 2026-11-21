#!/bin/bash
# A server installed by the old guide.html (0.6.7: the archive unpacked into /opt, nginx by hand) is
# updated by install.sh: the mode found, the database kept, the old files and image gone.
set -u
. /t/t-common.sh
prepare nginx docker.io docker-compose-v2
other_site
(dockerd >/var/log/dockerd.log 2>&1 &)
for _ in $(seq 1 20); do docker info >/dev/null 2>&1 && break; sleep 1; done

echo "=== the old way (guide.html of 0.6.7)"
D=old.test
mkdir -p /etc/letsencrypt/live/$D /var/www/$D
openssl req -x509 -newkey rsa:2048 -nodes -days 90 -subj "/CN=$D" -addext "subjectAltName=DNS:$D" \
    -keyout /etc/letsencrypt/live/$D/privkey.pem -out /etc/letsencrypt/live/$D/fullchain.pem 2>/dev/null
tar xzf /dist/lumen-server-0.6.7.tar.gz -C /opt
cd /opt/lumen-server
cp .env.example .env
sed -i "s/^LUMEN_DOMAIN=.*/LUMEN_DOMAIN=$D/; s/^LUMEN_ADMIN_PASSWORD=.*/LUMEN_ADMIN_PASSWORD=old-password-1/" .env
docker load -i lumen-server-image.tar >/dev/null
docker compose up -d >/dev/null 2>&1
cp -r site/. /var/www/$D/
cat >/etc/nginx/sites-available/$D <<EOF
server {
    listen 80;
    server_name $D;
    location / { return 301 https://\$host\$request_uri; }
}
server {
    listen 443 ssl;
    server_name $D;
    ssl_certificate     /etc/letsencrypt/live/$D/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/$D/privkey.pem;
    location / { root /var/www/$D; index index.html; try_files \$uri \$uri/ =404; }
    location /admin/ { proxy_pass http://127.0.0.1:7480; proxy_set_header Host \$host; proxy_set_header X-Real-IP \$remote_addr; }
}
EOF
ln -sf /etc/nginx/sites-available/$D /etc/nginx/sites-enabled/$D
nginx -s reload; sleep 3
check "old server runs (0.6.7)" "docker inspect -f '{{.Config.Image}}' lumen-server | grep -q 'lumen-server:0.6.7'"
check "old admin answers" "[ \"\$(https_code $D /admin/)\" = 200 ]"
docker exec lumen-server lumen-server override global 30 >/dev/null

echo "=== update with install.sh"
cd /tmp && tar xzf /arch/lumen-server-0.6.9-test.tar.gz
printf '\n' | /tmp/lumen-server-0.6.9-test/install.sh --lang ru 2>&1 | tail -14
check "0.6.9 runs" "docker inspect -f '{{.Config.Image}}' lumen-server | grep -q 0.6.9-test"
check "image 0.6.7 removed" "! docker image ls | grep -q '0.6.7'"
check "mode written: public" "grep -q '^LUMEN_MODE=public' /opt/lumen-server/.env"
check "the old password kept" "grep -q '^LUMEN_ADMIN_PASSWORD=old-password-1' /opt/lumen-server/.env"
check "database kept (30 fps)" "docker exec lumen-server lumen-server override show | grep -q '30 fps'"
check "old leftovers gone" "[ ! -e /opt/lumen-server/lumen-server-image.tar ] && [ ! -e /opt/lumen-server/site ] && [ ! -e /opt/lumen-server/.env.example ]"
check "admin after update" "[ \"\$(https_code $D /admin/)\" = 200 ]"
check "admin login with the old password" "curl -sk --resolve $D:443:127.0.0.1 -H 'Content-Type: application/json' -d '{\"user\":\"admin\",\"password\":\"old-password-1\"}' https://$D/admin/api/login | grep -q ok"
check "landing updated" "curl -sk --resolve $D:443:127.0.0.1 https://$D/ | grep -q lumen-setup-"
check "the hand-written nginx config untouched" "! grep -q 'install.sh' /etc/nginx/sites-available/$D"
check "the other site untouched" "[ \"\$(curl -s http://other.test/)\" = OTHER ]"
echo "RESULT: $pass passed, $fail failed"
