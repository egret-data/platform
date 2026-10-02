#!/usr/bin/env bash
# Builds the image and proves it starts in edit and run mode against a fake egret,
# runs as uid 1000, and refuses to start when a config file shadows the locked one.
set -euo pipefail
cd "$(dirname "$0")"
IMG="egret-apps:smoke"
NAMES=(egret-apps-smoke-edit egret-apps-smoke-run egret-apps-smoke-shadow egret-apps-smoke-hardened)
FAKE_PID=""
TMP=$(mktemp -d); JAR="$TMP/jar"; PAGE="$TMP/page.html"
cleanup() {
  for n in "${NAMES[@]}"; do docker rm -f "$n" >/dev/null 2>&1 || true; done
  rm -rf "$TMP"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT
fail() { echo "FAIL: $*"; for n in "${NAMES[@]}"; do docker logs "$n" 2>&1 | tail -20 || true; done; exit 1; }

docker build -t "$IMG" .

if [ ! -x .venv/bin/python ]; then
  python3.12 -m venv .venv
  .venv/bin/pip install -q -r requirements.txt -r requirements-dev.txt -c constraints.txt -e .
fi

PORT="${FAKE_PORT:-18770}"
APP=11111111-2222-4333-8444-555555555555
VER=v1
POD_POD=smokepod
POD_TOKEN=smoke-pod-token
MARIMO_TOKEN=smoke-marimo-token
SRC=$'import marimo\napp = marimo.App()\n\n@app.cell\ndef _():\n    return\n\nif __name__ == "__main__":\n    app.run()\n'
# Bind 0.0.0.0: the container reaches the host through host-gateway (the bridge IP on Linux).
EGRET_SRC="$SRC" APP="$APP" VER="$VER" POD_TOKEN="$POD_TOKEN" PORT="$PORT" \
  .venv/bin/python - <<'PY' &
import os, time, uvicorn
import sys; sys.path.insert(0, "tests")
import fake_egret
s = fake_egret.STATE
src = os.environ["EGRET_SRC"]
s.pod_tokens[os.environ["POD_TOKEN"]] = {}
s.drafts[os.environ["APP"]] = {"source": src, "revision": 1}
s.versions[(os.environ["APP"], os.environ["VER"])] = {"source": src}
uvicorn.run(fake_egret.app, host="0.0.0.0", port=int(os.environ["PORT"]), log_level="warning")
PY
FAKE_PID=$!
for _ in $(seq 1 30); do curl --max-time 5 -fsS -o /dev/null "http://127.0.0.1:$PORT/api/apps/pod/apps/$APP/draft" -H "Authorization: Bearer $POD_TOKEN" && break; sleep 1; done
curl --max-time 5 -fsS -o /dev/null "http://127.0.0.1:$PORT/api/apps/pod/apps/$APP/draft" -H "Authorization: Bearer $POD_TOKEN" || fail "fake egret did not start"

COMMON=(--add-host=host.docker.internal:host-gateway
  -e "EGRET_URL=http://host.docker.internal:$PORT" -e "EGRET_POD_TOKEN=$POD_TOKEN" -e "EGRET_MARIMO_TOKEN=$MARIMO_TOKEN")

wait_page() { # url
  for _ in $(seq 1 120); do
    code=$(curl --max-time 5 -sL -c "$JAR" -b "$JAR" -o "$PAGE" -w '%{http_code}' "$1" || true)
    [ "$code" = 200 ] && return 0
    sleep 1
  done
  return 1
}

docker run -d --name egret-apps-smoke-edit -p 127.0.0.1:18718:2718 "${COMMON[@]}" \
  -e "EGRET_BASE_URL=/e/$POD_POD" "$IMG" edit >/dev/null
wait_page "http://127.0.0.1:18718/e/$POD_POD/?access_token=$MARIMO_TOKEN" || fail "edit mode did not serve /e/$POD_POD/"
grep -qi marimo "$PAGE" || fail "edit page is not marimo's"
[ "$(docker exec egret-apps-smoke-edit id -u)" = 1000 ] || fail "edit container does not run as uid 1000"
[ "$(docker exec egret-apps-smoke-edit cat /proc/1/comm)" = tini ] || fail "PID 1 is not tini"
echo "edit OK (PID 1 is tini)"

# Exactly the Pod's hardening (egret-bin apps_pod_k8s::build_pod): read-only root filesystem, every
# capability dropped, no privilege escalation, and the only writable places are three volumes owned
# by uid 1000. Nobody had run marimo like that: if it writes outside them, the pod would crash-loop
# in production and no unit test would say so.
docker run -d --name egret-apps-smoke-hardened -p 127.0.0.1:18720:2718 "${COMMON[@]}" \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /work:uid=1000,gid=1000,mode=0755,size=256m \
  --tmpfs /home/egret:uid=1000,gid=1000,mode=0755,size=256m \
  --tmpfs /tmp:uid=1000,gid=1000,mode=1777,size=256m \
  -e "EGRET_BASE_URL=/e/$POD_POD" "$IMG" edit >/dev/null
wait_page "http://127.0.0.1:18720/e/$POD_POD/?access_token=$MARIMO_TOKEN" || fail "edit mode does not run with a read-only root filesystem"
grep -qi marimo "$PAGE" || fail "hardened edit page is not marimo's"
echo "hardened edit OK (read-only root, no capabilities)"

docker run -d --name egret-apps-smoke-run -p 127.0.0.1:18719:2719 "${COMMON[@]}" \
  -e "EGRET_BASE_URL=/r/$VER" -e "EGRET_APP_ID=$APP" -e "EGRET_VERSION_ID=$VER" "$IMG" run >/dev/null
wait_page "http://127.0.0.1:18719/r/$VER/?access_token=$MARIMO_TOKEN" || fail "run mode did not serve /r/$VER/"
grep -qi marimo "$PAGE" || fail "run page is not marimo's"
[ "$(docker exec egret-apps-smoke-run id -u)" = 1000 ] || fail "run container does not run as uid 1000"
echo "run OK"

# marimo prints its startup URL with ?access_token=<marimo token> unless run with -q.
for n in egret-apps-smoke-edit egret-apps-smoke-run; do
  logs=$(docker logs "$n" 2>&1)
  grep -q "egret-apps: marimo" <<<"$logs" || fail "$n: launcher startup line missing from the log"
  for secret in "$POD_TOKEN" "$MARIMO_TOKEN" "access_token="; do
    if grep -qF -- "$secret" <<<"$logs"; then fail "$n: '$secret' is in the container log"; fi
  done
done
echo "no tokens in logs OK"

# A config file in /work would shadow the locked marimo config: the image must refuse (exit 65).
for f in .marimo.toml pyproject.toml; do
  d=$(mktemp -d); : > "$d/$f"; chmod -R a+rX "$d"
  rc=0
  docker run --rm --name egret-apps-smoke-shadow -v "$d:/work:ro" "${COMMON[@]}" \
    -e "EGRET_BASE_URL=/e/$POD_POD" "$IMG" edit >/dev/null 2>"$TMP/shadow.err" || rc=$?
  rm -rf "$d"
  [ "$rc" = 65 ] || fail "$f in /work: expected exit 65, got $rc: $(cat "$TMP/shadow.err")"
  grep -qF "egret-apps: refusing to start: /work/$f would override the locked marimo config" "$TMP/shadow.err" \
    || fail "$f in /work: exit 65 without the launcher's refusal: $(cat "$TMP/shadow.err")"
  echo "refuses $f OK"
done

echo "SMOKE OK"
