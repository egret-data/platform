#!/usr/bin/env bash
# E2E ONLY. Tear down compose project `egret-opa-e2e` and the engine egret provisioned
# for it. The provisioned engine goes through egret's API (DELETE /engines/{id}), never
# `docker rm`, so egret removes exactly the containers it labelled for this project.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$REPO/deploy/opa-e2e/docker-compose.yml" -p egret-opa-e2e)
export WORK="${WORK:-${TMPDIR:-/tmp}/egret-opa-e2e}"
export EGRET_OPA_E2E_PORT="${EGRET_OPA_E2E_PORT:-28081}"
BASE="http://127.0.0.1:${EGRET_OPA_E2E_PORT}"
OPA_CONTAINER=egret-engine-egret-engines-trino-opa-coordinator

jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }

if curl -fsS "$BASE/healthz" >/dev/null 2>&1; then
  JAR="$(mktemp)"; trap 'rm -f "$JAR"' EXIT
  curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
    -d '{"email":"admin@egret.local","password":"Admin12345!"}' -o /dev/null
  CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
  api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
  for name in trino-opa trino-ranger; do
    id=$(api "$BASE/api/control-plane/engines" | jget "next((e['id'] for e in (d if isinstance(d,list) else d.get('items',[])) if e['name']=='$name'), None)")
    if [ -n "$id" ]; then
      echo "→ DELETE engine $name ($id)"
      api -X DELETE "$BASE/api/control-plane/engines/$id" -o /dev/null || echo "   (delete answered non-2xx)"
    fi
  done
  for _ in $(seq 1 30); do
    docker ps -a --format '{{.Names}}' | grep -x "$OPA_CONTAINER" >/dev/null || break
    sleep 2
  done
else
  echo "→ egret is not answering on $BASE; skipping the engine DELETE"
fi
if docker ps -a --format '{{.Names}}' | grep -x "$OPA_CONTAINER" >/dev/null; then
  echo "down.sh: $OPA_CONTAINER still exists after DELETE; NOT removing it by hand" >&2
fi

echo "→ compose down -v (project egret-opa-e2e only)"
"${COMPOSE[@]}" down -v --remove-orphans
