#!/usr/bin/env bash
#
# Reset egret LOCAL DEV test data to a clean baseline.
#
# Keeps users + the seeded demo/eng workspaces; wipes all embedded-catalog
# objects, the lakekeeper catalog schema, every other (test) workspace, and all
# RustFS objects. See deploy/reset-testdata.sql for the exact SQL.
#
# Usage:  just reset-testdata      (or: ./deploy/reset-testdata.sh [--yes])
set -euo pipefail

cd "$(dirname "$0")/.."
# Load .env for DATABASE_URL / RustFS creds when run standalone.
if [ -f .env ]; then set -a; . ./.env; set +a; fi

PGURL="${DATABASE_URL:-postgres://egret:egret@localhost:5432/egret}"
RUSTFS_CONTAINER="${RUSTFS_CONTAINER:-egret-dev-rustfs-1}"
RUSTFS_USER="${RUSTFS_ACCESS_KEY:-egret}"
RUSTFS_PASS="${RUSTFS_SECRET_KEY:-egretsecret}"
# The RustFS image ships no S3 client, so one runs beside it, in its network namespace.
S3_CLIENT_IMAGE="${S3_CLIENT_IMAGE:-amazon/aws-cli:2.37.9}"
s3() {
  docker run --rm --network "container:$RUSTFS_CONTAINER" \
    -e AWS_ACCESS_KEY_ID="$RUSTFS_USER" -e AWS_SECRET_ACCESS_KEY="$RUSTFS_PASS" \
    -e AWS_DEFAULT_REGION=us-east-1 -e AWS_ENDPOINT_URL=http://localhost:9000 \
    "$S3_CLIENT_IMAGE" "$@"
}

# Confirmation (skip with --yes).
if [ "${1:-}" != "--yes" ]; then
  echo "⚠️  This WIPES all catalog data, the lakekeeper schema, every test"
  echo "    workspace, and all RustFS objects. Keeps users + demo/eng workspaces."
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

# 3. RustFS: clear every bucket's objects (keep the empty buckets).
echo "→ clearing RustFS objects…"
if buckets=$(s3 s3api list-buckets --query 'Buckets[].Name' --output text 2>/dev/null); then
  for b in $buckets; do
    s3 s3 rm "s3://$b" --recursive >/dev/null 2>&1 || true
  done
  echo "  cleared RustFS buckets"
else
  echo "  (RustFS container '$RUSTFS_CONTAINER' not reachable — skipped)"
fi

echo ""
echo "✅ Reset done. Next:"
echo "     just serve        # re-migrates the empty catalog"
echo "     just seed-demo    # (optional) seed a ready-to-test demo catalog"
