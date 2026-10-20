#!/usr/bin/env bash
# DEMO ONLY. Wait until the demo answers, then print how to use it.
set -euo pipefail
cd "$(dirname "$0")/../.."
# Values already exported by the caller (for example DEMO_PORT) win over demo.env.
CALLER_PORT="${DEMO_PORT:-}"; CALLER_URL="${DEMO_URL:-}"
set -a; . deploy/demo/demo.env; set +a
[ -z "$CALLER_PORT" ] || DEMO_PORT="$CALLER_PORT"
[ -z "$CALLER_URL" ] || DEMO_URL="$CALLER_URL"
URL="http://${DEMO_HOST}:${DEMO_PORT}"
# DEMO_URL and DEMO_PORT are separate values; changing only one silently breaks SSO.
if [ -n "${DEMO_URL:-}" ] && [ "$DEMO_URL" != "$URL" ]; then
  echo "DEMO_URL ($DEMO_URL) must equal $URL: change DEMO_PORT and DEMO_URL together (see docs/en/guides/demo-stack.md)" >&2
  exit 1
fi
echo -n "waiting for $URL "
for _ in $(seq 1 180); do
  # /api/auth/config is answered by egret itself (through edge and frontend); / alone is served by nginx.
  if curl -fsS -m 5 -o /dev/null "$URL/api/auth/config" 2>/dev/null; then
    echo " ready"
    cat <<EOT

  Egret      $URL
  Admin      $EGRET_ADMIN_EMAIL / $EGRET_ADMIN_PASSWORD   (SSO users: admin-sso / Admin12345!, member-sso / Member12345!)
             An SSO user must sign in once before an admin can add them to a workspace.
  Mail       http://localhost:8025    (Mailpit: every mail the demo sends)
  Trino      database "trino-demo" in workspace "default" (catalogs tpch, memory)

  DEMO ONLY: public development secrets, never expose this to a network.
EOT
    exit 0
  fi
  echo -n "."; sleep 5
done
echo; echo "timed out; see: just demo-logs" >&2; exit 1
