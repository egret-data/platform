#!/usr/bin/env bash
# DEMO ONLY. Wait until the demo answers, then print how to use it.
set -euo pipefail
cd "$(dirname "$0")/../.."
# Values already exported by the caller (for example DEMO_PORT) win over demo.env.
CALLER_PORT="${DEMO_PORT:-}"
set -a; . deploy/demo/demo.env; set +a
[ -z "$CALLER_PORT" ] || DEMO_PORT="$CALLER_PORT"
URL="http://${DEMO_HOST}:${DEMO_PORT}"
echo -n "waiting for $URL "
for _ in $(seq 1 180); do
  if curl -fsS -o /dev/null "$URL/api/auth/config" 2>/dev/null || curl -fsS -o /dev/null "$URL/" 2>/dev/null; then
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
