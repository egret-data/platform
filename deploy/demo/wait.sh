#!/usr/bin/env bash
# DEMO ONLY. Wait until the demo answers, then print how to use it.
set -euo pipefail
cd "$(dirname "$0")/../.."
# Values already exported by the caller (for example DEMO_PORT) win over demo.env.
CALLER_PORT="${DEMO_PORT:-}"; CALLER_URL="${DEMO_URL:-}"; CALLER_APPS_URL="${DEMO_APPS_URL:-}"
set -a; . deploy/demo/demo.env; set +a
[ -z "$CALLER_PORT" ] || DEMO_PORT="$CALLER_PORT"
[ -z "$CALLER_URL" ] || DEMO_URL="$CALLER_URL"
[ -z "$CALLER_APPS_URL" ] || DEMO_APPS_URL="$CALLER_APPS_URL"
URL="http://${DEMO_HOST}:${DEMO_PORT}"
# DEMO_URL and DEMO_PORT are separate values; changing only one silently breaks SSO.
if [ -n "${DEMO_URL:-}" ] && [ "$DEMO_URL" != "$URL" ]; then
  echo "DEMO_URL ($DEMO_URL) must equal $URL: change DEMO_PORT and DEMO_URL together (see docs/en/guides/demo-stack.md)" >&2
  exit 1
fi
APPS_URL="http://${DEMO_APPS_HOST}:${DEMO_PORT}"
if [ -n "${DEMO_APPS_URL:-}" ] && [ "$DEMO_APPS_URL" != "$APPS_URL" ]; then
  echo "DEMO_APPS_URL ($DEMO_APPS_URL) must equal $APPS_URL: change DEMO_PORT, DEMO_URL and DEMO_APPS_URL together (see docs/en/guides/demo-stack.md)" >&2
  exit 1
fi
echo -n "waiting for $URL "
for _ in $(seq 1 180); do
  # /api/auth/config is answered by egret itself (through edge and frontend); / alone is served by nginx.
  if curl -fsS -m 5 -o /dev/null "$URL/api/auth/config" 2>/dev/null; then
    echo " ready"
    cat <<EOT

  Egret      $URL
  Data Apps  $APPS_URL   (opened from inside Egret; needs *.localtest.me DNS or /etc/hosts entries for egret.localtest.me and apps.localtest.me)
  Admin      $EGRET_ADMIN_EMAIL / $EGRET_ADMIN_PASSWORD   (SSO users: admin-sso / Admin12345!, member-sso / Member12345!)
             An SSO user must sign in once before an admin can add them to a workspace.
  Mail       http://localhost:8025    (Mailpit: every mail the demo sends)
  Trino      database "trino-demo" in workspace "default" (catalogs tpch, memory)
  Engines    "Add engine" is on: it starts Trino containers on this machine's Docker (first use pulls trinodb/trino:483)
             Risk: Data Apps author code runs inside Egret and can call the Docker proxy, so any app author can get root on this
             machine's Docker host. Do not give app access to anyone you do not trust with this machine.
             The read-only socket mount protects nothing; exec start is refused but creating an exec instance still
             reaches the daemon. Details: "Safety" in docs/en/guides/demo-stack.md.

  DEMO ONLY: public development secrets, never expose this to a network.
EOT
    exit 0
  fi
  echo -n "."; sleep 5
done
echo; echo "timed out; see: just demo-logs" >&2; exit 1
