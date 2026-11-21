#!/bin/sh
# `serve` in one of two modes (LUMEN_MODE, install.sh writes it into .env):
#   public  the server behind the VPS's nginx: wait for the Let's Encrypt
#           certificate of $LUMEN_DOMAIN (certbot of the VPS gets it), then
#           signaling with it, the admin as plain HTTP on 127.0.0.1 for nginx.
#   lan     a server in a local network, no domain: its own certificate in
#           /data/tls for $LUMEN_ADDRESS (made once, devices trust its key's
#           fingerprint), the admin and the landing page over HTTPS, plain
#           HTTP redirected there.
# Anything else: lumen-server "$@".
set -eu

if [ "${1:-}" != serve ]; then
    exec lumen-server "$@"
fi

signal="0.0.0.0:${LUMEN_SIGNAL_PORT:-7443}"

if [ "${LUMEN_MODE:-public}" = lan ]; then
    : "${LUMEN_ADDRESS:?set LUMEN_ADDRESS in .env}"
    # Addresses separated by commas or spaces: all go into the certificate.
    # shellcheck disable=SC2046
    lumen-server self-cert --dir /data/tls $(echo "$LUMEN_ADDRESS" | tr ',' ' ')
    # Plain HTTP to HTTPS, unless its port is taken by something else ("off").
    http="${LUMEN_HTTP_PORT:-80}"
    if [ "$http" = off ]; then
        set --
    else
        set -- --redirect-listen "0.0.0.0:$http"
    fi
    exec lumen-server serve \
        --listen "$signal" \
        --admin-listen "0.0.0.0:${LUMEN_HTTPS_PORT:-443}" \
        "$@" \
        --site /srv/site \
        --cert /data/tls/cert.pem \
        --key /data/tls/key.pem
fi

: "${LUMEN_DOMAIN:?set LUMEN_DOMAIN in .env}"
live="/etc/letsencrypt/live/${LUMEN_CERT_NAME:-$LUMEN_DOMAIN}"

waited=0
until [ -s "$live/fullchain.pem" ] && [ -s "$live/privkey.pem" ]; do
    if [ $((waited % 60)) -eq 0 ]; then
        echo "waiting for the certificate in $live (certbot on the VPS)"
    fi
    sleep 5
    waited=$((waited + 5))
done

# The admin's login and password come in LUMEN_ADMIN_USER / LUMEN_ADMIN_PASSWORD.
exec lumen-server serve \
    --listen "$signal" \
    --admin-listen "127.0.0.1:${LUMEN_ADMIN_PORT:-7480}" \
    --admin-behind-proxy \
    --cert "$live/fullchain.pem" \
    --key "$live/privkey.pem"
