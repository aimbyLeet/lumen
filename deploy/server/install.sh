#!/usr/bin/env bash
# Lumen server @VERSION@: install, update, status, removal (Ubuntu 22.04 / 24.04, root).
#
#   tar xzf lumen-server-@VERSION@.tar.gz -C /tmp && sudo /tmp/lumen-server-@VERSION@/install.sh
#
# The menu asks everything; the same without questions: install.sh --help.
# The server lives in /opt/lumen-server (docker-compose.yml, .env, data/);
# this script is copied there too (status, removal without the archive).

# Run as `sh install.sh` too: the rest is bash.
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail
# Lengths in characters (the Russian texts), whatever the session's locale.
export LC_ALL=C.UTF-8

VERSION="@VERSION@"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIR=/opt/lumen-server
ENV_FILE="$DIR/.env"
MARK="# Lumen server (install.sh)"

# ---------------------------------------------------------------- arguments

MODE="" DOMAIN="" EMAIL="" ADDRESS="" ADMIN_USER="" ADMIN_PASSWORD="" YES=0 DATA="" UI_LANG=""
usage() {
    cat <<'EOF'
Lumen server: install, update, status, removal.

  install.sh                         the menu
  install.sh --mode <mode> [...]     without the menu

Modes:
  clean    a clean VPS: Docker, nginx, Let's Encrypt for your domain
  nginx    a VPS where nginx already serves other sites: only our site is added
  lan      a local network, no domain: the server's own certificate, by IP
  update   update the installed server (its settings and database stay)
  status   what is installed and how it is doing
  remove   remove the server

Options:
  --domain <name>            clean, nginx: the server's domain (its DNS A record points here)
  --email <address>          clean, nginx: for Let's Encrypt (renewal notices); empty: none
  --address <ip[,name...]>   lan: the address(es) devices use (default: this machine's IP)
  --admin-user <login>       the web admin's login (default: admin)
  --admin-password <pass>    at least 8 characters, no spaces, $ ` ' " \ # (default: generated)
  --keep-data | --delete-data  remove: keep the database and the key (default) or delete them
  --yes                      do not ask: defaults everywhere
  --lang en|ru               the language of this script (default: from LANG)
EOF
}
while [ $# -gt 0 ]; do
    case "$1" in
        --mode) MODE="${2:-}"; shift ;;
        --domain) DOMAIN="${2:-}"; shift ;;
        --email) EMAIL="${2:-}"; shift ;;
        --address) ADDRESS="${2:-}"; shift ;;
        --admin-user) ADMIN_USER="${2:-}"; shift ;;
        --admin-password) ADMIN_PASSWORD="${2:-}"; shift ;;
        --keep-data) DATA=keep ;;
        --delete-data) DATA=delete ;;
        --yes | -y) YES=1 ;;
        --lang) UI_LANG="${2:-}"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown option: $1 (install.sh --help)" >&2; exit 2 ;;
    esac
    shift
done

if [ -z "$UI_LANG" ]; then
    case "${LC_ALL:-${LANG:-}}" in ru*) UI_LANG=ru ;; *) UI_LANG=en ;; esac
fi

# ---------------------------------------------------------------- output

if [ -t 1 ]; then
    B=$'\e[1m' G=$'\e[32m' Y=$'\e[33m' R=$'\e[31m' D=$'\e[2m' N=$'\e[0m'
else
    B="" G="" Y="" R="" D="" N=""
fi
# t "English" "Русский": the text in the script's language.
t() { if [ "$UI_LANG" = ru ]; then printf '%s' "$2"; else printf '%s' "$1"; fi; }
say() { printf '%s\n' "$(t "$1" "$2")"; }
step() { printf '\n%s==> %s%s\n' "$B" "$(t "$1" "$2")" "$N"; }
ok() { printf '%s✓%s %s\n' "$G" "$N" "$(t "$1" "$2")"; }
warn() { printf '%s! %s%s\n' "$Y" "$(t "$1" "$2")" "$N" >&2; }
# row "label" "value": a line of a table, labels aligned by characters.
row() { printf '  %s%*s %s\n' "$1" $((24 - ${#1})) "" "$2"; }
die() { printf '%s✗ %s%s\n' "$R" "$(t "$1" "$2")" "$N" >&2; exit 1; }

# ask VAR "en" "ru" [default]: a line from the user; --yes takes the default.
ask() {
    local var=$1 prompt default=${4:-} answer
    prompt="$(t "$2" "$3")"
    if [ "$YES" = 1 ]; then
        printf -v "$var" '%s' "$default"
        return
    fi
    if [ -n "$default" ]; then prompt="$prompt [$default]"; fi
    read -r -p "$prompt: " answer || answer=""
    printf -v "$var" '%s' "${answer:-$default}"
}

# yes_no "en" "ru" y|n: true for yes.
yes_no() {
    local default=$3 answer hint
    if [ "$YES" = 1 ]; then [ "$default" = y ]; return; fi
    if [ "$default" = y ]; then hint="Y/n"; else hint="y/N"; fi
    read -r -p "$(t "$1" "$2") [$hint]: " answer || answer=""
    answer="${answer:-$default}"
    case "$answer" in [yYдД]*) return 0 ;; *) return 1 ;; esac
}

# ---------------------------------------------------------------- the machine

need_root() {
    [ "$(id -u)" = 0 ] || die "Run as root: sudo $0" "Запустите от root: sudo $0"
}

check_system() {
    local id="" version=""
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        id="$(. /etc/os-release && echo "${ID:-}")"
        # shellcheck disable=SC1091
        version="$(. /etc/os-release && echo "${VERSION_ID:-}")"
    fi
    if [ "$id" != ubuntu ]; then
        warn "This is not Ubuntu ($id): the script is made for Ubuntu 22.04 and 24.04." \
            "Это не Ubuntu ($id): скрипт сделан для Ubuntu 22.04 и 24.04."
        yes_no "Go on anyway?" "Всё равно продолжить?" n || exit 1
    elif [ "$version" != 22.04 ] && [ "$version" != 24.04 ]; then
        warn "Ubuntu $version: checked on 22.04 and 24.04." "Ubuntu $version: проверено на 22.04 и 24.04."
    fi
    [ "$(uname -m)" = x86_64 ] || die "The server's image is for x86_64, this machine is $(uname -m)." \
        "Образ сервера — для x86_64, а эта машина — $(uname -m)."
    # What the script itself uses (a minimal Ubuntu may lack them).
    local missing=()
    command -v ss >/dev/null 2>&1 && command -v ip >/dev/null 2>&1 || missing+=(iproute2)
    command -v curl >/dev/null 2>&1 || missing+=(curl)
    command -v openssl >/dev/null 2>&1 || missing+=(openssl)
    if [ ${#missing[@]} -gt 0 ]; then apt_install "${missing[@]}"; fi
}

with_systemd() { [ -d /run/systemd/system ]; }

APT_UPDATED=0
apt_install() {
    if [ "$APT_UPDATED" = 0 ]; then
        DEBIAN_FRONTEND=noninteractive apt-get update -q >/dev/null
        APT_UPDATED=1
    fi
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$@" >/dev/null
}

start_service() {
    if with_systemd; then
        systemctl enable --now "$1" >/dev/null 2>&1 || systemctl start "$1"
        return
    fi
    # No systemd (a container, WSL): the init script, else the daemon itself.
    service "$1" start >/dev/null 2>&1 && return
    case "$1" in
        docker) pgrep -x dockerd >/dev/null || (nohup dockerd >/var/log/dockerd.log 2>&1 &) ;;
        nginx) pgrep -x nginx >/dev/null || nginx ;;
    esac
}

# A TCP port someone listens on (any address).
port_busy() { [ -n "$(ss -Hltn "sport = :$1" 2>/dev/null)" ]; }

# Who listens on a port, for a message.
port_owner() { ss -Hltnp "sport = :$1" 2>/dev/null | sed -n 's/.*users:(("\([^"]*\)".*/\1/p' | head -1; }

ensure_docker() {
    step "Docker" "Docker"
    if ! command -v docker >/dev/null 2>&1; then
        say "Installing Docker (docker.io from Ubuntu's repository)…" "Ставлю Docker (docker.io из репозитория Ubuntu)…"
        apt_install docker.io
    fi
    if ! docker compose version >/dev/null 2>&1; then
        say "Installing Docker Compose…" "Ставлю Docker Compose…"
        if dpkg -s docker-ce >/dev/null 2>&1; then apt_install docker-compose-plugin; else apt_install docker-compose-v2; fi
    fi
    start_service docker
    for _ in $(seq 1 20); do docker info >/dev/null 2>&1 && break; sleep 1; done
    docker info >/dev/null 2>&1 || die "Docker does not run: systemctl status docker" "Docker не запускается: systemctl status docker"
    ok "$(docker --version | head -1), $(docker compose version | head -1)" "$(docker --version | head -1), $(docker compose version | head -1)"
}

ufw_active() { command -v ufw >/dev/null 2>&1 && [[ "$(ufw status 2>/dev/null)" == *"Status: active"* ]]; }

# Opens the ports in ufw if it is on; prints the rules added (kept in .env for removal).
open_ports() {
    local added="" p
    if ufw_active; then
        for p in "$@"; do
            ufw allow "$p" >/dev/null
            added="$added $p"
        done
    fi
    printf '%s' "${added# }"
}

# ---------------------------------------------------------------- the installation

installed() { [ -f "$ENV_FILE" ]; }

env_get() {
    [ -f "$ENV_FILE" ] || return 0
    sed -n "s/^$1=//p" "$ENV_FILE" | tail -1
}

env_set() {
    if grep -q "^$1=" "$ENV_FILE" 2>/dev/null; then
        local tmp
        tmp="$(mktemp)"
        awk -v k="$1" -v v="$2" 'index($0, k "=") == 1 { print k "=" v; next } { print }' "$ENV_FILE" >"$tmp"
        cat "$tmp" >"$ENV_FILE"
        rm -f "$tmp"
    else
        printf '%s=%s\n' "$1" "$2" >>"$ENV_FILE"
    fi
}

installed_version() { sed -n 's/^ *image: *lumen-server:\(.*\)$/\1/p' "$DIR/docker-compose.yml" 2>/dev/null | head -1; }

# The mode in words.
mode_name() {
    if [ "$(installed_mode)" = lan ]; then t "local network" "локальная сеть"; else t "domain, behind nginx" "домен, за nginx"; fi
}

installed_mode() {
    local m
    m="$(env_get LUMEN_MODE)"
    printf '%s' "${m:-public}"
}

need_archive() {
    [ -f "$HERE/lumen-server-image.tar" ] && [ -d "$HERE/site" ] && [ -d "$HERE/compose" ] ||
        die "No image here ($HERE): run install.sh from the unpacked archive lumen-server-<version>.tar.gz." \
            "Здесь нет образа ($HERE): запускайте install.sh из распакованного архива lumen-server-<версия>.tar.gz."
}

load_image() {
    step "The server's image" "Образ сервера"
    docker load -i "$HERE/lumen-server-image.tar" >/dev/null
    ok "lumen-server:$VERSION" "lumen-server:$VERSION"
}

write_compose() {
    install -m 644 "$HERE/compose/$1.yml" "$DIR/docker-compose.yml"
    install -m 755 "$HERE/install.sh" "$DIR/install.sh"
}

compose() { (cd "$DIR" && docker compose "$@"); }

# The server's images (lumen-server:<version>), one per line.
server_images() { docker image ls --format '{{.Repository}}:{{.Tag}}' | grep '^lumen-server:' || true; }

# Old images of the server (the current one stays).
remove_old_images() {
    local img
    for img in $(server_images); do
        [ "$img" = "lumen-server:$VERSION" ] || docker rmi "$img" >/dev/null 2>&1 || true
    done
}

# Starts the server and waits until it listens (its log says so).
start_server() {
    step "Starting the server" "Запуск сервера"
    compose up -d --remove-orphans >/dev/null 2>&1 || compose up -d --remove-orphans
    local state
    for _ in $(seq 1 60); do
        # Not `| grep -q`: with pipefail, grep leaving early fails the pipe (SIGPIPE).
        if [[ "$(docker logs lumen-server 2>&1)" == *"lumen-server: signaling on"* ]]; then
            ok "The server runs." "Сервер работает."
            return
        fi
        state="$(docker inspect -f '{{.State.Status}}' lumen-server 2>/dev/null || echo missing)"
        if [ "$state" = exited ] || [ "$state" = dead ] || [ "$state" = missing ]; then break; fi
        sleep 1
    done
    docker logs --tail 20 lumen-server 2>&1 || true
    die "The server did not start: the log is above (docker logs lumen-server)." \
        "Сервер не запустился: журнал выше (docker logs lumen-server)."
}

check_password() {
    local p=$1
    [ "${#p}" -ge 8 ] || return 1
    case "$p" in *[[:space:]\$\`\'\"\\#]*) return 1 ;; esac
}

ask_admin() {
    ask ADMIN_USER "The web admin's login" "Логин админки" "${ADMIN_USER:-admin}"
    case "$ADMIN_USER" in "" | *[!A-Za-z0-9._@-]*)
        die "The login: letters, digits, . _ @ -" "Логин: буквы, цифры, . _ @ -" ;;
    esac
    if [ -z "$ADMIN_PASSWORD" ] && [ "$YES" = 0 ]; then
        read -r -s -p "$(t "The web admin's password (Enter: generate one)" "Пароль админки (Enter — сгенерировать)"): " ADMIN_PASSWORD || true
        echo
    fi
    if [ -z "$ADMIN_PASSWORD" ]; then
        ADMIN_PASSWORD="$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 16 || true)"
        GENERATED=1
    fi
    check_password "$ADMIN_PASSWORD" || die "The password: at least 8 characters, no spaces and none of \$ \` ' \" \\ #" \
        "Пароль: от 8 символов, без пробелов и без \$ \` ' \" \\ #"
}
GENERATED=0

refuse_if_installed() {
    if installed; then
        die "The server is already installed in $DIR (version $(installed_version)): choose Update, or Remove first." \
            "Сервер уже установлен в $DIR (версия $(installed_version)): выберите «Обновить» или сначала «Удалить»."
    fi
}

new_env() {
    mkdir -p "$DIR/data"
    (
        umask 077
        {
            echo "# Lumen server settings, written by install.sh. After a change:"
            echo "#   cd $DIR && docker compose up -d"
            echo "# Values without \$ (docker compose takes it for a variable)."
        } >"$ENV_FILE"
    )
    chmod 600 "$ENV_FILE"
}

# ---------------------------------------------------------------- nginx and Let's Encrypt

nginx_conf() {
    if [ -d /etc/nginx/sites-available ] && grep -q "sites-enabled" /etc/nginx/nginx.conf 2>/dev/null; then
        printf '/etc/nginx/sites-available/%s' "$1"
    else
        printf '/etc/nginx/conf.d/%s.conf' "$1"
    fi
}

nginx_enable() {
    case "$1" in /etc/nginx/sites-available/*)
        ln -sf "$1" "/etc/nginx/sites-enabled/$(basename "$1")" ;;
    esac
}

nginx_disable() {
    rm -f "/etc/nginx/sites-enabled/$(basename "$1")" "$1"
}

reload_nginx() {
    if with_systemd; then systemctl reload nginx; else nginx -s reload; fi
}

reload_hook() {
    if with_systemd; then printf 'systemctl reload nginx'; else printf 'nginx -s reload'; fi
}

# A listen line for IPv6 only where the machine has it (nginx fails otherwise).
listen6() { if [ -f /proc/net/if_inet6 ]; then printf '    listen [::]:%s;' "$1"; fi; }

# The site's config with `body`; nginx -t and a reload, or the old config back.
apply_site() {
    local conf=$1 body=$2 backup="" out
    if [ -f "$conf" ]; then
        backup="$(mktemp)"
        cp "$conf" "$backup"
    fi
    printf '%s\n' "$body" >"$conf"
    nginx_enable "$conf"
    if out="$(nginx -t 2>&1)" && reload_nginx; then
        rm -f "$backup"
        # A reload is not at once: new workers take the config a moment later.
        sleep 2
        return 0
    fi
    if [ -n "$backup" ]; then
        cat "$backup" >"$conf"
        rm -f "$backup"
    else
        nginx_disable "$conf"
    fi
    reload_nginx >/dev/null 2>&1 || true
    printf '%s\n' "$out" >&2
    die "nginx does not accept the site's config (above); the old config is back, other sites are untouched." \
        "nginx не принял конфиг сайта (выше); вернул прежний, остальные сайты не затронуты."
}

acme_site() {
    cat <<EOF
$MARK: until the certificate is there (Let's Encrypt's check).
server {
    listen 80;
$(listen6 80)
    server_name $1;
    location /.well-known/acme-challenge/ {
        root /var/www/$1;
    }
    location / {
        return 404;
    }
}
EOF
}

full_site() {
    local domain=$1 cert=$2 admin_port=$3
    cat <<EOF
$MARK: the landing page, /admin/ to the server's admin.
server {
    listen 80;
$(listen6 80)
    server_name $domain;
    location /.well-known/acme-challenge/ {
        root /var/www/$domain;
    }
    location / {
        return 301 https://\$host\$request_uri;
    }
}
server {
    listen 443 ssl;
$(listen6 "443 ssl")
    server_name $domain;
    ssl_certificate     /etc/letsencrypt/live/$cert/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/$cert/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    location / {
        root /var/www/$domain;
        index index.html;
        try_files \$uri \$uri/ =404;
    }
    location = /admin {
        return 301 /admin/;
    }
    location /admin/ {
        proxy_pass http://127.0.0.1:$admin_port;
        proxy_set_header Host            \$host;
        proxy_set_header X-Real-IP       \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_read_timeout 30s;
    }
}
EOF
}

check_domain() {
    [ -n "$1" ] || die "No domain given." "Домен не указан."
    grep -Eq '^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)+$' <<<"$1" ||
        die "Not a domain: $1" "Это не домен: $1"
}

# Where the domain points, against this machine's addresses (behind NAT they differ: asks).
check_dns() {
    local resolved mine ip match=0
    resolved="$(getent ahostsv4 "$1" 2>/dev/null | awk '{print $1}' | sort -u | tr '\n' ' ')"
    [ -n "$resolved" ] || die "The domain $1 does not resolve: make its DNS A record point to this server and wait a few minutes." \
        "Домен $1 не разрешается: укажите в DNS A-запись на этот сервер и подождите несколько минут."
    mine="$(ip -o -4 addr show 2>/dev/null | awk '{split($4, a, "/"); print a[1]}' | tr '\n' ' ')"
    for ip in $resolved; do
        case " $mine " in *" $ip "*) match=1 ;; esac
    done
    if [ "$match" = 1 ]; then
        ok "$1 → $resolved" "$1 → $resolved"
    else
        warn "$1 points to $resolved; this machine's addresses: $mine. Fine if the server is behind NAT (the provider's public IP), otherwise Let's Encrypt will fail." \
            "$1 указывает на $resolved; адреса этой машины: $mine. Это нормально, если сервер за NAT (публичный IP провайдера), иначе Let's Encrypt не выдаст сертификат."
        yes_no "Go on?" "Продолжить?" y || exit 1
    fi
}

get_certificate() {
    local domain=$1
    step "Let's Encrypt certificate" "Сертификат Let's Encrypt"
    command -v certbot >/dev/null 2>&1 || apt_install certbot
    local who=(--register-unsafely-without-email)
    if [ -n "$EMAIL" ]; then who=(--email "$EMAIL"); fi
    # Let's Encrypt asks for a file over plain HTTP: is nginx serving the webroot yet?
    local probe="/var/www/$domain/.well-known/acme-challenge/lumen-probe" token _
    token="$(tr -dc 'a-z0-9' </dev/urandom | head -c 16 || true)"
    mkdir -p "$(dirname "$probe")"
    printf '%s' "$token" >"$probe"
    for _ in $(seq 1 10); do
        [ "$(curl -s --resolve "$domain:80:127.0.0.1" "http://$domain/.well-known/acme-challenge/lumen-probe" || true)" = "$token" ] && break
        sleep 1
    done
    rm -f "$probe"
    if ! certbot certonly --webroot -w "/var/www/$domain" -d "$domain" --cert-name "$domain" \
        --keep-until-expiring --non-interactive --agree-tos "${who[@]}" \
        --deploy-hook "$(reload_hook)"; then
        # Nothing of ours stays in nginx.
        nginx_disable "$(nginx_conf "$domain")"
        reload_nginx >/dev/null 2>&1 || true
        die "certbot did not get the certificate (above). Check the DNS record and that ports 80 and 443 are open (ufw and the provider's firewall)." \
            "certbot не получил сертификат (выше). Проверьте DNS-запись и что порты 80 и 443 открыты (ufw и фаервол хостера)."
    fi
    [ -s "/etc/letsencrypt/live/$domain/fullchain.pem" ] ||
        die "No certificate in /etc/letsencrypt/live/$domain" "Нет сертификата в /etc/letsencrypt/live/$domain"
    ok "/etc/letsencrypt/live/$domain" "/etc/letsencrypt/live/$domain"
}

copy_site_public() {
    local root="/var/www/$1"
    mkdir -p "$root"
    rm -f "$root"/lumen-setup-*.exe
    cp -R "$HERE/site/." "$root/"
    chmod -R a+rX "$root"
}

# ---------------------------------------------------------------- 1, 2: a VPS with a domain

install_public() {
    local kind=$1 # clean | nginx
    refuse_if_installed
    need_archive
    if [ "$kind" = clean ] && command -v nginx >/dev/null 2>&1; then
        say "nginx is already installed here: going on as «a VPS with nginx» (only our site is added)." \
            "nginx здесь уже установлен: продолжаю как «VPS с nginx» (добавлю только наш сайт)."
        kind=nginx
    fi
    if [ "$kind" = nginx ]; then
        command -v nginx >/dev/null 2>&1 || die "nginx is not installed: choose «a clean VPS»." \
            "nginx не установлен: выберите «чистый VPS»."
        local out
        out="$(nginx -t 2>&1)" || {
            printf '%s\n' "$out" >&2
            die "nginx's config has an error already (above): fix it first, so it is clear whose error it is." \
                "В конфиге nginx уже есть ошибка (выше): сначала исправьте её, чтобы было понятно, чья она."
        }
    else
        local p
        for p in 80 443; do
            port_busy "$p" && die "Port $p is taken by $(port_owner "$p"): choose «a VPS with nginx» if it is nginx, or free the port." \
                "Порт $p занят ($(port_owner "$p")): если это nginx — выберите «VPS с nginx», иначе освободите порт."
        done
    fi

    step "The domain" "Домен"
    ask DOMAIN "The server's domain (its DNS A record points to this VPS)" "Домен сервера (A-запись в DNS указывает на этот VPS)" "$DOMAIN"
    DOMAIN="$(printf '%s' "$DOMAIN" | tr '[:upper:]' '[:lower:]' | sed 's#^https\?://##; s#/.*##')"
    check_domain "$DOMAIN"
    local conf
    conf="$(nginx_conf "$DOMAIN")"
    if command -v nginx >/dev/null 2>&1; then
        local other
        other="$(grep -rlE "server_name[^;]*[[:space:]]$DOMAIN([[:space:];]|$)" /etc/nginx 2>/dev/null | grep -v "^$conf$" | head -1 || true)"
        [ -z "$other" ] || die "$DOMAIN is already served by nginx: $other. Use another domain (a subdomain), or remove that site first." \
            "$DOMAIN уже обслуживает nginx: $other. Возьмите другой домен (поддомен) или сначала уберите тот сайт."
    fi
    check_dns "$DOMAIN"
    ask EMAIL "E-mail for Let's Encrypt (notices; Enter: none)" "E-mail для Let's Encrypt (уведомления; Enter — без него)" "$EMAIL"

    local signal=7443 admin=7480
    port_busy "$signal" && die "Port $signal (for the programs) is taken by $(port_owner "$signal")." \
        "Порт $signal (для программ) занят ($(port_owner "$signal"))."
    port_busy "$admin" && die "Port $admin (the admin, 127.0.0.1) is taken by $(port_owner "$admin")." \
        "Порт $admin (админка, 127.0.0.1) занят ($(port_owner "$admin"))."
    step "The web admin" "Админка"
    ask_admin

    ensure_docker
    step "nginx" "nginx"
    if ! command -v nginx >/dev/null 2>&1; then
        say "Installing nginx…" "Ставлю nginx…"
        apt_install nginx
    fi
    start_service nginx
    mkdir -p "/var/www/$DOMAIN"
    apply_site "$conf" "$(acme_site "$DOMAIN")"
    ok "$conf" "$conf"
    get_certificate "$DOMAIN"
    copy_site_public "$DOMAIN"
    apply_site "$conf" "$(full_site "$DOMAIN" "$DOMAIN" "$admin")"
    ok "https://$DOMAIN/ → /var/www/$DOMAIN, /admin/ → 127.0.0.1:$admin" "https://$DOMAIN/ → /var/www/$DOMAIN, /admin/ → 127.0.0.1:$admin"

    load_image
    new_env
    env_set LUMEN_MODE public
    env_set LUMEN_DOMAIN "$DOMAIN"
    env_set LUMEN_CERT_NAME "$DOMAIN"
    env_set LUMEN_SIGNAL_PORT "$signal"
    env_set LUMEN_ADMIN_PORT "$admin"
    env_set LUMEN_ADMIN_USER "$ADMIN_USER"
    env_set LUMEN_ADMIN_PASSWORD "$ADMIN_PASSWORD"
    env_set LUMEN_UFW "$(open_ports 80/tcp 443/tcp "$signal/tcp")"
    write_compose public
    start_server
    verify_public
    summary
}

# check_pages <base shown> <base asked> [curl options]: the landing page and the admin answer 200.
check_pages() {
    local shown=$1 asked=$2 path code
    shift 2
    for path in / /admin/; do
        code="$(curl -sk -o /dev/null -w '%{http_code}' "$@" "$asked$path" || true)"
        if [ "$code" = 200 ]; then
            ok "$shown$path: 200" "$shown$path: 200"
        else
            warn "$shown$path answers $code (expected 200)" "$shown$path отвечает $code (ожидался 200)"
        fi
    done
}

verify_public() {
    local domain
    domain="$(env_get LUMEN_DOMAIN)"
    check_pages "https://$domain" "https://$domain" --resolve "$domain:443:127.0.0.1"
}

# ---------------------------------------------------------------- 3: a local network

default_address() {
    local a
    a="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") { print $(i + 1); exit }}')"
    if [ -z "$a" ]; then a="$(hostname -I 2>/dev/null | awk '{print $1}')"; fi
    printf '%s' "$a"
}

check_addresses() {
    local a
    [ -n "$1" ] || die "No address given." "Адрес не указан."
    for a in $(printf '%s' "$1" | tr ',' ' '); do
        grep -Eq '^([0-9]{1,3}(\.[0-9]{1,3}){3}|[0-9A-Fa-f:]+|[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?)$' <<<"$a" ||
            die "Not an address or a name: $a" "Это не адрес и не имя: $a"
    done
}

# A free port: `want`, or the user's choice when it is taken; "off" allowed if `can_off`.
pick_port() {
    local var=$1 want=$2 en=$3 ru=$4 fallback=$5 can_off=${6:-}
    local p=$want
    while port_busy "$p"; do
        warn "Port $p ($en) is taken by $(port_owner "$p")." "Порт $p ($ru) занят ($(port_owner "$p"))."
        [ "$YES" = 0 ] || die "Free port $p or run without --yes to choose another." \
            "Освободите порт $p или запустите без --yes, чтобы выбрать другой."
        if [ -n "$can_off" ]; then
            ask p "Another port, or off" "Другой порт или off" "$fallback"
            [ "$p" != off ] || break
        else
            ask p "Another port" "Другой порт" "$fallback"
        fi
        case "$p" in "" | *[!0-9]*) die "Not a port: $p" "Это не порт: $p" ;; esac
    done
    printf -v "$var" '%s' "$p"
}

install_lan() {
    refuse_if_installed
    need_archive
    step "The address" "Адрес"
    say "Devices reach the server by this address (an IP in the local network or VPN; several: comma-separated, names too)." \
        "По этому адресу устройства находят сервер (IP в локальной сети или VPN; несколько — через запятую, можно и имена)."
    ask ADDRESS "The server's address" "Адрес сервера" "${ADDRESS:-$(default_address)}"
    ADDRESS="$(printf '%s' "$ADDRESS" | tr -d ' ')"
    check_addresses "$ADDRESS"
    local signal https http
    pick_port signal 7443 "for the programs" "для программ" 7444
    pick_port https 443 "HTTPS: the admin and the landing page" "HTTPS: админка и лендинг" 8443
    pick_port http 80 "HTTP, redirects to HTTPS" "HTTP, перенаправляет на HTTPS" 8080 can_off
    step "The web admin" "Админка"
    ask_admin

    ensure_docker
    load_image
    new_env
    env_set LUMEN_MODE lan
    env_set LUMEN_ADDRESS "$ADDRESS"
    env_set LUMEN_SIGNAL_PORT "$signal"
    env_set LUMEN_HTTPS_PORT "$https"
    env_set LUMEN_HTTP_PORT "$http"
    env_set LUMEN_ADMIN_USER "$ADMIN_USER"
    env_set LUMEN_ADMIN_PASSWORD "$ADMIN_PASSWORD"
    local ports=("$signal/tcp" "$https/tcp")
    [ "$http" = off ] || ports+=("$http/tcp")
    env_set LUMEN_UFW "$(open_ports "${ports[@]}")"
    copy_site_lan
    write_compose lan
    start_server
    verify_lan
    summary
}

# The first of the addresses (a local network): the one shown.
first_address() { env_get LUMEN_ADDRESS | cut -d, -f1; }

# The landing page and the installer of a local network, served by the server itself.
copy_site_lan() {
    rm -rf "$DIR/site"
    cp -R "$HERE/site" "$DIR/site"
    chmod -R a+rX "$DIR/site"
}

https_base() {
    local first port
    first="$(first_address)"
    case "$first" in *:*) first="[$first]" ;; esac
    port="$(env_get LUMEN_HTTPS_PORT)"
    if [ "${port:-443}" = 443 ]; then printf 'https://%s' "$first"; else printf 'https://%s:%s' "$first" "$port"; fi
}

fingerprint() {
    docker exec lumen-server lumen-server fingerprint /data/tls/key.pem 2>/dev/null || true
}

verify_lan() {
    local port
    port="$(env_get LUMEN_HTTPS_PORT)"
    check_pages "$(https_base)" "https://127.0.0.1:${port:-443}"
}

# ---------------------------------------------------------------- what to tell the user

program_address() {
    local host=$1 port
    port="$(env_get LUMEN_SIGNAL_PORT)"
    if [ "${port:-7443}" = 7443 ]; then printf '%s' "$host"; else printf '%s:%s' "$host" "$port"; fi
}

summary() {
    local mode
    mode="$(installed_mode)"
    printf '\n%s%s%s\n' "$B" "$(t "Lumen server $(installed_version) is ready" "Сервер Lumen $(installed_version) готов")" "$N"
    if [ "$mode" = lan ]; then
        row "$(t "Admin:" "Админка:")" "$(https_base)/admin/"
        row "$(t "Download page:" "Страница загрузки:")" "$(https_base)/"
        row "$(t "Server in the programs:" "Сервер в программах:")" "$(program_address "$(first_address)")"
        row "$(t "Fingerprint:" "Отпечаток:")" "$B$(fingerprint)$N"
    else
        local domain
        domain="$(env_get LUMEN_DOMAIN)"
        row "$(t "Admin:" "Админка:")" "https://$domain/admin/"
        row "$(t "Download page:" "Страница загрузки:")" "https://$domain/"
        row "$(t "Server in the programs:" "Сервер в программах:")" "$(program_address "$domain")"
    fi
    row "$(t "Admin's login:" "Логин админки:")" "$(env_get LUMEN_ADMIN_USER)"
    if [ "$GENERATED" = 1 ]; then
        row "$(t "Admin's password:" "Пароль админки:")" "$B$(env_get LUMEN_ADMIN_PASSWORD)$N  $(t "(generated: save it)" "(сгенерирован — сохраните)")"
    else
        row "$(t "Admin's password:" "Пароль админки:")" "$(t "as entered (also in $ENV_FILE)" "введённый (он же в $ENV_FILE)")"
    fi
    local fw
    fw="$(env_get LUMEN_UFW)"
    if [ -n "$fw" ]; then
        row "ufw:" "$(t "opened $fw" "открыты $fw")"
    elif ! ufw_active; then
        row "$(t "Firewall:" "Фаервол:")" "$(t "ufw is off; if the provider has a firewall, open the ports there" "ufw выключен; если у хостера есть свой фаервол — откройте порты там")"
    fi
    printf '\n'
    if [ "$mode" = lan ]; then
        say "The browser warns once about the certificate (it is the server's own, not a public one): that is expected." \
            "Браузер один раз предупредит о сертификате (он собственный у сервера, не публичный) — так и должно быть."
        say "Lumen Host and Lumen Client show the fingerprint the first time: it must be the same as above." \
            "Lumen Host и Lumen Client в первый раз покажут отпечаток: он должен совпасть с тем, что выше."
    fi
    say "Status, update, removal: sudo $DIR/install.sh" "Состояние, обновление, удаление: sudo $DIR/install.sh"
}

# ---------------------------------------------------------------- 4: update

update() {
    installed || die "No server in $DIR: install it first." "В $DIR нет сервера: сначала установите его."
    need_archive
    local mode was
    mode="$(installed_mode)"
    was="$(installed_version)"
    step "Update $was → $VERSION ($mode)" "Обновление $was → $VERSION ($mode)"
    if [ -n "$was" ] && [ "$was" != "$VERSION" ] &&
        [ "$(printf '%s\n%s\n' "$was" "$VERSION" | sort -V | tail -1)" = "$was" ]; then
        yes_no "The installed version $was is newer than $VERSION: install the older one?" \
            "Установленная версия $was новее, чем $VERSION: поставить более старую?" n || exit 1
    fi
    ensure_docker
    load_image
    env_set LUMEN_MODE "$mode"
    if [ "$mode" = lan ]; then
        copy_site_lan
    else
        copy_site_public "$(env_get LUMEN_DOMAIN)"
        # What the archive used to leave here (before install.sh).
        rm -rf "$DIR/site" "$DIR/lumen-server-image.tar" "$DIR/.env.example" "$DIR/guide.html"
    fi
    write_compose "$mode"
    start_server
    remove_old_images
    if [ "$mode" = lan ]; then verify_lan; else verify_public; fi
    if [ "$mode" = lan ]; then
        ok "Updated: the database, the settings and the key (the fingerprint) are as they were." \
            "Обновлено: база, настройки и ключ (отпечаток) — прежние."
    else
        ok "Updated: the database and the settings are as they were." "Обновлено: база и настройки — прежние."
    fi
}

# ---------------------------------------------------------------- 5: status

status() {
    if ! installed; then
        say "The server is not installed (no $DIR)." "Сервер не установлен (нет $DIR)."
        return
    fi
    local mode state
    mode="$(installed_mode)"
    state="$(docker inspect -f '{{.State.Status}}, {{.State.StartedAt}}' lumen-server 2>/dev/null | sed 's/\.[0-9]*Z$/ UTC/; s/T/ /' || true)"
    printf '%s%s%s\n' "$B" "$(t "Lumen server" "Сервер Lumen")" "$N"
    row "$(t "Version:" "Версия:")" "$(installed_version)"
    row "$(t "Mode:" "Режим:")" "$(mode_name)"
    row "$(t "Container:" "Контейнер:")" "${state:-$(t "not running" "не запущен")}"
    if [ "$mode" = lan ]; then
        row "$(t "Addresses:" "Адреса:")" "$(env_get LUMEN_ADDRESS)"
        row "$(t "Admin:" "Админка:")" "$(https_base)/admin/"
        row "$(t "Server in the programs:" "Сервер в программах:")" "$(program_address "$(first_address)")"
        row "$(t "Fingerprint:" "Отпечаток:")" "$B$(fingerprint)$N"
    else
        local domain cert until=""
        domain="$(env_get LUMEN_DOMAIN)"
        cert="$(env_get LUMEN_CERT_NAME)"
        cert="/etc/letsencrypt/live/${cert:-$domain}/fullchain.pem"
        if [ -r "$cert" ] && command -v openssl >/dev/null 2>&1; then
            until="$(openssl x509 -noout -enddate -in "$cert" | cut -d= -f2)"
        fi
        row "$(t "Admin:" "Админка:")" "https://$domain/admin/"
        row "$(t "Server in the programs:" "Сервер в программах:")" "$(program_address "$domain")"
        row "$(t "Certificate until:" "Сертификат до:")" "${until:--}"
    fi
    row "$(t "Admin's login:" "Логин админки:")" "$(env_get LUMEN_ADMIN_USER)"
    row "$(t "Settings:" "Настройки:")" "$ENV_FILE"
    printf '\n%s\n' "$(t "The log's end:" "Конец журнала:")"
    docker logs --tail 8 lumen-server 2>&1 | sed 's/^/  /' || true
}

# ---------------------------------------------------------------- 6: removal

remove() {
    installed || die "No server in $DIR." "В $DIR нет сервера."
    local mode domain
    mode="$(installed_mode)"
    domain="$(env_get LUMEN_DOMAIN)"
    step "Removal" "Удаление"
    [ "$YES" = 1 ] || yes_no "Remove the Lumen server from this machine?" "Удалить сервер Lumen с этой машины?" n || exit 1
    if [ -z "$DATA" ]; then
        if yes_no "Delete its database (devices, access) and key too? (No: they stay in $DIR/data, a new installation picks them up)" \
            "Удалить и его базу (устройства, доступы) и ключ? (Нет — останутся в $DIR/data, новая установка их подхватит)" n; then
            DATA=delete
        else
            DATA=keep
        fi
    fi
    compose down >/dev/null 2>&1 || docker rm -f lumen-server >/dev/null 2>&1 || true
    local img
    for img in $(server_images); do
        docker rmi "$img" >/dev/null 2>&1 || true
    done
    ok "The container and the images are removed." "Контейнер и образы удалены."
    if [ "$mode" != lan ] && [ -n "$domain" ]; then
        local conf
        conf="$(nginx_conf "$domain")"
        if [ -f "$conf" ]; then
            if grep -q "^$MARK" "$conf" ||
                yes_no "$conf was not written by install.sh: remove it?" "$conf написан не install.sh: удалить его?" y; then
                nginx_disable "$conf"
                if command -v nginx >/dev/null 2>&1 && nginx -t >/dev/null 2>&1; then reload_nginx || true; fi
                ok "nginx: the site $domain is removed, other sites are untouched." "nginx: сайт $domain удалён, остальные сайты не затронуты."
            fi
        fi
        rm -rf "/var/www/$domain"
        if [ -d "/etc/letsencrypt/live/$domain" ] && command -v certbot >/dev/null 2>&1 &&
            yes_no "Delete the Let's Encrypt certificate of $domain?" "Удалить сертификат Let's Encrypt для $domain?" n; then
            certbot delete --cert-name "$domain" --non-interactive >/dev/null 2>&1 || true
            ok "The certificate is deleted." "Сертификат удалён."
        fi
    fi
    local rule
    for rule in $(env_get LUMEN_UFW); do
        ufw delete allow "$rule" >/dev/null 2>&1 || true
    done
    if [ "$DATA" = delete ]; then
        rm -rf "$DIR"
        ok "$DIR is removed with the database." "$DIR удалён вместе с базой."
    else
        find "$DIR" -mindepth 1 -maxdepth 1 ! -name data -exec rm -rf {} +
        ok "Removed; the database and the key stay in $DIR/data." "Удалено; база и ключ остались в $DIR/data."
    fi
    say "Docker and nginx stay installed." "Docker и nginx остаются установленными."
}

# ---------------------------------------------------------------- the menu

menu() {
    local current="" choice
    if installed; then current="$(installed_version), $(mode_name)"; fi
    printf '\n%s%s%s\n' "$B" "$(t "Lumen server $VERSION" "Сервер Lumen $VERSION")" "$N"
    if [ -n "$current" ]; then
        printf '%s%s%s\n' "$D" "$(t "Installed: $current" "Установлен: $current")" "$N"
    fi
    cat <<EOF

  1) $(t "Install on a clean VPS (domain, nginx, Let's Encrypt)" "Установить на чистый VPS (домен, nginx, Let's Encrypt)")
  2) $(t "Install on a VPS that has nginx and other sites" "Установить на VPS, где уже есть nginx и сайты")
  3) $(t "Install in a local network (by IP, no domain)" "Установить в локальной сети (по IP, без домена)")
  4) $(t "Update" "Обновить")
  5) $(t "Status" "Состояние")
  6) $(t "Remove" "Удалить")
  0) $(t "Exit" "Выход")

EOF
    local default=1
    if installed; then default=4; fi
    read -r -p "$(t "Choose" "Выберите") [$default]: " choice || choice=0
    case "${choice:-$default}" in
        1) MODE=clean ;;
        2) MODE=nginx ;;
        3) MODE=lan ;;
        4) MODE=update ;;
        5) MODE=status ;;
        6) MODE=remove ;;
        *) exit 0 ;;
    esac
}

need_root
if [ -z "$MODE" ]; then
    [ "$YES" = 0 ] || die "--yes needs --mode" "--yes нужен вместе с --mode"
    menu
fi
case "$MODE" in
    clean | nginx | lan | update) check_system ;;
esac
case "$MODE" in
    clean) install_public clean ;;
    nginx) install_public nginx ;;
    lan) install_lan ;;
    update) update ;;
    status) status ;;
    remove) remove ;;
    *) die "Unknown mode: $MODE (install.sh --help)" "Неизвестный режим: $MODE (install.sh --help)" ;;
esac
