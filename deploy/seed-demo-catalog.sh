#!/usr/bin/env bash
#
# Seed a ready-to-test demo embedded catalog against a RUNNING egret server:
# a MinIO storage connection, a provisioned "demo-catalog" warehouse in the
# `demo` workspace, an engine role + write grant, a Catalog Client, and a
# `demo_ns.events` Iceberg table. Idempotent: exits early if already seeded.
#
# Run AFTER `just reset-testdata` + `just serve`.  Usage: just seed-demo
set -euo pipefail

cd "$(dirname "$0")/.."
if [ -f .env ]; then set -a; . ./.env; set +a; fi

BASE="${EGRET_BASE_URL:-http://localhost:8080}"
PGURL="${DATABASE_URL:-postgres://egret:egret@localhost:5432/egret}"
S3_ENDPOINT="${SEED_S3_ENDPOINT:-http://localhost:9000}"
BUCKET="${SEED_BUCKET:-warehouse}"
MINIO_CONTAINER="${MINIO_CONTAINER:-egret-dev-minio-1}"
AK="${MINIO_ROOT_USER:-egret}"
SK="${MINIO_ROOT_PASSWORD:-egretsecret}"
ADMIN_EMAIL="${EGRET_ADMIN_EMAIL:-admin@egret.local}"
ADMIN_PASS="${EGRET_ADMIN_PASSWORD:-Admin12345!}"

get() { python3 -c "import sys,json;print(json.load(sys.stdin).get('$1',''))"; }

# 0. Make sure the target bucket exists (reset clears objects but keeps buckets).
docker exec "$MINIO_CONTAINER" sh -c \
  "mc alias set lo http://localhost:9000 '$AK' '$SK' >/dev/null 2>&1 && mc mb --ignore-existing lo/$BUCKET >/dev/null 2>&1" \
  2>/dev/null || echo "  (minio '$MINIO_CONTAINER' not reachable — assuming bucket '$BUCKET' exists)"

# 1. Admin session.
J=$(mktemp)
trap 'rm -f "$J"' EXIT
curl -sf -c "$J" -X POST "$BASE/api/auth/login" -H "Content-Type: application/json" \
  -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$J" | tail -1)
api() { curl -sf -b "$J" -H "X-Egret-CSRF: $CSRF" -H "Content-Type: application/json" "$@"; }

# 2. Resolve the demo workspace by slug (stable across machines).
DEMO_WS=$(psql "$PGURL" -tAc "SELECT id FROM workspaces WHERE slug = 'demo' LIMIT 1")
[ -n "$DEMO_WS" ] || { echo "No 'demo' workspace found — run migrations first."; exit 1; }

# Idempotency: bail if the demo catalog is already present.
if api "$BASE/api/control-plane/warehouses" | grep -q '"name":"demo-catalog"'; then
  echo "✓ demo-catalog already seeded — nothing to do."
  exit 0
fi

echo "→ storage connection (demo-minio)…"
SC_ID=$(api -X POST "$BASE/api/control-plane/storage" -d "{
  \"name\":\"demo-minio\",\"provider\":\"s3\",\"endpoint\":\"$S3_ENDPOINT\",
  \"region\":\"us-east-1\",\"bucket\":\"$BUCKET\",\"access_key\":\"$AK\",
  \"new_secret\":{\"name\":\"demo-minio-secret\",\"value\":\"$SK\"},\"path_style\":true}" | get id)

echo "→ warehouse (demo-catalog) — provisioning lakekeeper…"
WH=$(api -X POST "$BASE/api/control-plane/warehouses" \
  -d "{\"name\":\"demo-catalog\",\"workspaceId\":\"$DEMO_WS\",\"storageConnectionId\":\"$SC_ID\"}")
WH_ID=$(echo "$WH" | get id)
LK=$(echo "$WH" | get lakekeeperWarehouseId)

echo "→ engine role + write grant + client…"
RID=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/roles" \
  -d '{"name":"demo-engine","consumerType":"engine","description":"Demo engine role"}' | get id)
api -o /dev/null -X POST "$BASE/api/control-plane/roles/$RID/grants" \
  -d '{"securableType":"catalog","privilege":"write"}'
CL=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/clients" \
  -d "{\"name\":\"demo-client\",\"consumerType\":\"engine\",\"workspaceId\":\"$DEMO_WS\",\"catalogRoleId\":\"$RID\"}")
CID=$(echo "$CL" | get clientId)
CSEC=$(echo "$CL" | get clientSecret)

echo "→ demo_ns.events table (via catalog REST)…"
TOK=$(curl -sf -X POST "$BASE/oauth/token" \
  -d "grant_type=client_credentials&client_id=$CID&client_secret=$CSEC" | get access_token)
curl -sf -o /dev/null -X POST "$BASE/catalog/v1/$LK/namespaces" \
  -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
  -d '{"namespace":["demo_ns"],"properties":{}}'
curl -sf -o /dev/null -X POST "$BASE/catalog/v1/$LK/namespaces/demo_ns/tables" \
  -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
  -d '{"name":"events","schema":{"type":"struct","schema-id":0,"fields":[
        {"id":1,"name":"id","required":true,"type":"long"},
        {"id":2,"name":"name","required":false,"type":"string"},
        {"id":3,"name":"ts","required":false,"type":"timestamptz"}]}}'

cat <<EOF

✅ Demo catalog seeded (workspace: demo):
   Warehouse : demo-catalog
   Namespace : demo_ns
   Table     : demo_ns.events  (id long, name string, ts timestamptz)
   Client    : $CID
   Secret    : $CSEC

   Browse it: Control Plane → Warehouses → demo-catalog → Browse
EOF
