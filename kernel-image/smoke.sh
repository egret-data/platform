#!/usr/bin/env bash
set -euo pipefail
IMG="egret-kernel:smoke"
PORT="${PORT:-18888}"
NAME="egret-kernel-smoke"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker build -t "$IMG" .
docker run -d --rm --name "$NAME" -p "${PORT}:8888" "$IMG"

# Wait for the gateway (up to ~30s).
for i in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${PORT}/api" >/dev/null 2>&1; then break; fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${PORT}/api" >/dev/null || { echo "FAIL: gateway not alive"; exit 1; }

# python3 kernelspec actually registered? (check the kernelspecs map, not the
# "default" field — "default":"python3" would false-match a plain grep).
curl -fsS "http://127.0.0.1:${PORT}/api/kernelspecs" \
  | python3 -c 'import sys,json; sys.exit(0 if "python3" in json.load(sys.stdin).get("kernelspecs",{}) else 1)' \
  || { echo "FAIL: no python3 kernelspec"; exit 1; }

# Create + delete a kernel (proves the kernel actually launches w/ Daft+helpers).
KID=$(curl -fsS -X POST "http://127.0.0.1:${PORT}/api/kernels" -H 'Content-Type: application/json' -d '{"name":"python3"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
[ -n "$KID" ] || { echo "FAIL: kernel did not start"; exit 1; }
curl -fsS -X DELETE "http://127.0.0.1:${PORT}/api/kernels/${KID}" >/dev/null
echo "SMOKE OK: gateway alive, python3 kernelspec present, kernel ${KID} started + stopped"
