#!/usr/bin/env bash
#
# Reset egret LOCAL DEV test data to a clean baseline.
#
# Keeps users + the seeded demo/eng workspaces; wipes all embedded-catalog
# objects, the lakekeeper catalog schema, every other (test) workspace, and all
# MinIO objects. See deploy/reset-testdata.sql for the exact SQL.
#
# Usage:  just reset-testdata      (or: ./deploy/reset-testdata.sh [--yes])
set -euo pipefail

cd "$(dirname "$0")/.."
# Load .env for DATABASE_URL / MinIO creds when run standalone.
if [ -f .env ]; then set -a; . ./.env; set +a; fi

PGURL="${DATABASE_URL:-postgres://egret:egret@localhost:5432/egret}"
MINIO_CONTAINER="${MINIO_CONTAINER:-egret-dev-minio-1}"
MINIO_USER="${MINIO_ROOT_USER:-egret}"
MINIO_PASS="${MINIO_ROOT_PASSWORD:-egretsecret}"

# Confirmation (skip with --yes).
if [ "${1:-}" != "--yes" ]; then
  echo "⚠️  This WIPES all catalog data, the lakekeeper schema, every test"
  echo "    workspace, and all MinIO objects. Keeps users + demo/eng workspaces."
  printf "    Type 'reset' to confirm: "
  read -r reply
  [ "$reply" = "reset" ] || { echo "Aborted."; exit 1; }
fi

# 1. Stop a running serve so it isn't holding the lakekeeper schema mid-drop.
if pkill -f "target/debug/egret serve" 2>/dev/null; then
  echo "→ stopped running serve"
  sleep 1
fi

# 2. Postgres: wipe catalog + test workspaces + drop lakekeeper schema.
echo "→ resetting Postgres…"
psql "$PGURL" -v ON_ERROR_STOP=1 -f deploy/reset-testdata.sql

# 3. MinIO: clear every bucket's objects (keep the empty buckets).
echo "→ clearing MinIO objects…"
if docker exec "$MINIO_CONTAINER" sh -c '
    mc alias set lo "http://localhost:9000" "'"$MINIO_USER"'" "'"$MINIO_PASS"'" >/dev/null 2>&1 || exit 1
    for b in $(mc ls lo | awk "{print \$NF}"); do
      mc rm --recursive --force "lo/$b" >/dev/null 2>&1 || true
    done
  ' 2>/dev/null; then
  echo "  cleared MinIO buckets"
else
  echo "  (MinIO container '$MINIO_CONTAINER' not reachable — skipped)"
fi

echo ""
echo "✅ Reset done. Next:"
echo "     just serve        # re-migrates the empty catalog"
echo "     just seed-demo    # (optional) seed a ready-to-test demo catalog"
