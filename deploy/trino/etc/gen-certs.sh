#!/usr/bin/env bash
# F2-P5: generate a self-signed TLS keystore for the DEV Trino coordinator.
# JWT authentication is only ENFORCED over HTTPS (see README.md), so the dev
# stack needs a keystore. This produces `trino-keystore.p12` (gitignored — never
# commit a private key). Password is "changeit" to match config.properties.
# Replace with a real certificate in any non-local environment.
set -euo pipefail
cd "$(dirname "$0")"
openssl req -x509 -newkey rsa:2048 -keyout /tmp/trino-key.pem -out /tmp/trino-cert.pem \
    -days 3650 -nodes -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,DNS:trino,IP:127.0.0.1"
openssl pkcs12 -export -in /tmp/trino-cert.pem -inkey /tmp/trino-key.pem \
    -out trino-keystore.p12 -name trino -passout pass:changeit
rm -f /tmp/trino-key.pem /tmp/trino-cert.pem
echo "wrote $(pwd)/trino-keystore.p12 (password: changeit)"
