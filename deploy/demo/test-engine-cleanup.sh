#!/usr/bin/env bash
# Acceptance check for the engine cleanup in `just demo-down` / `demo-reset`.
# Touches only containers it creates (names start with egret-demo-cleanup-test-).
set -euo pipefail
cd "$(dirname "$0")/../.."
MINE=egret-demo-cleanup-test-mine
OTHER=egret-demo-cleanup-test-other
IMG=$(docker inspect --format '{{.Config.Image}}' "$(docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml ps -q postgres 2>/dev/null | head -1)" 2>/dev/null || true)
IMG=${IMG:-postgres:16}
cleanup() { docker rm -f "$MINE" "$OTHER" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker image inspect "$IMG" >/dev/null 2>&1 || docker pull -q "$IMG" >/dev/null
docker create --name "$MINE" --label egret.managed=true --label egret.project=egret-demo "$IMG" >/dev/null
docker create --name "$OTHER" --label egret.managed=true --label egret.project=someone-else "$IMG" >/dev/null
./deploy/demo/remove-engines.sh
if docker inspect "$MINE" >/dev/null 2>&1; then echo "FAIL: demo engine container survived" >&2; exit 1; fi
if ! docker inspect "$OTHER" >/dev/null 2>&1; then echo "FAIL: another project's container was removed" >&2; exit 1; fi
echo "PASS: engine cleanup removes only egret-demo containers"
