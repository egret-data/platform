#!/usr/bin/env bash
# Measures Trino 483 behaviour with allow-insecure-over-http true vs false.
# Own compose project; secrets/keystore are generated per run into a temp dir.
set -uo pipefail
cd "$(dirname "$0")"
export COMPOSE_PROJECT_NAME=trino-insecure-http
WORK=$(mktemp -d); export WORK
trap 'docker compose -p trino-insecure-http down -v >/dev/null 2>&1; rm -rf "$WORK"' EXIT
DELEG_PW=$(openssl rand -hex 8); MON_PW=$(openssl rand -hex 8)
export TRINO_INTERNAL_SECRET=$(openssl rand -hex 16)
{ for u in "egret_delegator:$DELEG_PW" "egret-monitor:$MON_PW"; do
    htpasswd -nbBC 10 "${u%%:*}" "${u#*:}"; done; } | sed 's/\$2[aby]\$/$2y$/' > "$WORK/password.db"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=coordinator \
  -addext subjectAltName=DNS:coordinator,DNS:localhost \
  -keyout "$WORK/k.pem" -out "$WORK/c.pem" 2>/dev/null
cat "$WORK/k.pem" "$WORK/c.pem" > "$WORK/keystore.pem"; chmod 644 "$WORK/keystore.pem" "$WORK/password.db"

H=http://localhost:18080; S=https://localhost:18443
run() { python3 -I ./query.py "$@"; }
cli() { docker exec trino-insecure-http-coordinator-1 trino --server "$1" "${@:2}" 2>&1 | tr '\n' ' ' | cut -c1-300; echo; }

for flag in true false; do
  echo "=== allow-insecure-over-http=$flag ==="
  ALLOW_INSECURE=$flag docker compose -p trino-insecure-http up -d --force-recreate >/dev/null 2>&1
  for _ in $(seq 90); do curl -s $H/v1/info 2>/dev/null | grep -q '"starting":false' && break; sleep 2; done
  for _ in $(seq 60); do
    n=$(run $S "SELECT count(*) FROM system.runtime.nodes" -u "egret_delegator:$DELEG_PW" -H 'X-Trino-User: egret_delegator' 2>/dev/null)
    case $n in *"[[2]]"*) break;; esac; sleep 2; done
  echo "1 plain http, X-Trino-User bob:           $(run $H 'SELECT current_user' -H 'X-Trino-User: bob')"
  echo "2 CLI --user ann --session-user bob:      $(cli http://localhost:8080 --user ann --session-user bob --execute 'SELECT current_user')"
  echo "3 fwd https, no credential:               $(run $H 'SELECT current_user' -H 'X-Trino-User: bob' -H 'X-Forwarded-Proto: https')"
  echo "4 fwd https, monitor as itself:           $(run $H 'SELECT current_user' -u "egret-monitor:$MON_PW" -H 'X-Trino-User: egret-monitor' -H 'X-Forwarded-Proto: https')"
  echo "5 fwd https, monitor claiming bob:        $(run $H 'SELECT current_user' -u "egret-monitor:$MON_PW" -H 'X-Trino-User: bob' -H 'X-Forwarded-Proto: https')"
  echo "6 GET /v1/info:                           $(curl -s -w ' HTTP %{http_code}' $H/v1/info)"
  echo "7 GET /v1/info/state as admin:            $(curl -s -w ' HTTP %{http_code}' -H 'X-Trino-User: admin' $H/v1/info/state)"
  echo "8 nodes over https as delegator->ann:     $(run $S 'SELECT count(*) FROM system.runtime.nodes' -u "egret_delegator:$DELEG_PW" -H 'X-Trino-Original-User: egret_delegator' -H 'X-Trino-User: ann')"
  echo "9 current_user over https delegator->ann: $(run $S 'SELECT current_user' -u "egret_delegator:$DELEG_PW" -H 'X-Trino-Original-User: egret_delegator' -H 'X-Trino-User: ann')"
  docker compose -p trino-insecure-http logs worker 2>&1 | grep -ci 'error' | sed 's/^/worker log lines with error: /'
done
