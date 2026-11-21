# Shared by the public-mode scenarios: a certbot stand-in, names in /etc/hosts, checks.
pass=0 fail=0
check() { if eval "$2" >/dev/null 2>&1; then echo "OK   $1"; pass=$((pass+1)); else echo "FAIL $1"; fail=$((fail+1)); fi; }
export DEBIAN_FRONTEND=noninteractive

prepare() {
    apt-get update -q >/dev/null && apt-get install -y -q curl openssl iproute2 ca-certificates "$@" >/dev/null 2>&1
    local ip
    ip="$(hostname -I | awk '{print $1}')"
    printf '%s lumen.test other.test old.test\n' "$ip" >>/etc/hosts
    # certbot: the webroot must really be served by nginx for the domain (Let's Encrypt's check), then a
    # self-signed certificate where certbot keeps them.
    cat >/usr/local/bin/certbot <<'EOF'
#!/bin/bash
echo "certbot $*" >>/tmp/certbot.log
cmd=$1; shift
w="" d="" name=""
while [ $# -gt 0 ]; do
    case "$1" in -w) w=$2; shift ;; -d) d=$2; shift ;; --cert-name) name=$2; shift ;; esac
    shift
done
name=${name:-$d}
if [ "$cmd" = delete ]; then rm -rf "/etc/letsencrypt/live/$name"; exit 0; fi
[ ! -f /tmp/certbot.fail ] || { echo "certbot: (made to fail)"; exit 1; }
token=$RANDOM$RANDOM
mkdir -p "$w/.well-known/acme-challenge"
echo "$token" >"$w/.well-known/acme-challenge/probe"
got="$(curl -s "http://$d/.well-known/acme-challenge/probe")"
rm -f "$w/.well-known/acme-challenge/probe"
[ "$got" = "$token" ] || { echo "challenge failed: got '$got'"; exit 1; }
echo "challenge ok for $d" >>/tmp/certbot.log
mkdir -p "/etc/letsencrypt/live/$name"
openssl req -x509 -newkey rsa:2048 -nodes -days 90 -subj "/CN=$d" -addext "subjectAltName=DNS:$d" \
    -keyout "/etc/letsencrypt/live/$name/privkey.pem" -out "/etc/letsencrypt/live/$name/fullchain.pem" 2>/dev/null
EOF
    chmod 755 /usr/local/bin/certbot
}

other_site() {
    mkdir -p /var/www/other
    echo OTHER >/var/www/other/index.html
    cat >/etc/nginx/sites-available/other <<'EOF'
server {
    listen 80;
    server_name other.test;
    root /var/www/other;
}
EOF
    ln -sf /etc/nginx/sites-available/other /etc/nginx/sites-enabled/other
    nginx 2>/dev/null || nginx -s reload
}

https_code() { curl -sk -o /dev/null -w '%{http_code}' --resolve "$1:443:127.0.0.1" "https://$1$2"; }
