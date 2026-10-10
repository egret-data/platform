#!/usr/bin/env bash
#
# Seed a ready-to-test demo embedded catalog against a RUNNING egret server:
#   - a RustFS storage connection + a provisioned "demo-catalog" warehouse (demo ws)
#   - an ENGINE role (catalog write) + client that creates the tables
#   - two namespaces with five Iceberg tables
#   - a READER role (external) granted read on ONLY two tables + its client
#   - a live check proving the reader reads its two tables and is denied the rest
#
# Run AFTER `just reset-testdata` + `just serve`.  Usage: just seed-demo
set -euo pipefail

cd "$(dirname "$0")/.."
if [ -f .env ]; then set -a; . ./.env; set +a; fi

BASE="${EGRET_BASE_URL:-http://localhost:8080}"
PGURL="${DATABASE_URL:-postgres://egret:egret@localhost:5432/egret}"
S3_ENDPOINT="${SEED_S3_ENDPOINT:-http://localhost:9000}"
BUCKET="${SEED_BUCKET:-warehouse}"
RUSTFS_CONTAINER="${RUSTFS_CONTAINER:-egret-dev-rustfs-1}"
AK="${RUSTFS_ACCESS_KEY:-egret}"
SK="${RUSTFS_SECRET_KEY:-egretsecret}"
ADMIN_EMAIL="${EGRET_ADMIN_EMAIL:-admin@egret.local}"
ADMIN_PASS="${EGRET_ADMIN_PASSWORD:-Admin12345!}"

get() { python3 -c "import sys,json;print(json.load(sys.stdin).get('$1',''))"; }

# 0. Make sure the target bucket exists (reset clears objects but keeps buckets).
# The RustFS image ships no S3 client, so one runs beside it, in its network namespace.
s3() {
  docker run --rm --network "container:$RUSTFS_CONTAINER" \
    -e AWS_ACCESS_KEY_ID="$AK" -e AWS_SECRET_ACCESS_KEY="$SK" \
    -e AWS_DEFAULT_REGION=us-east-1 -e AWS_ENDPOINT_URL=http://localhost:9000 \
    "${S3_CLIENT_IMAGE:-amazon/aws-cli:2.37.9}" "$@"
}
{ s3 s3api head-bucket --bucket "$BUCKET" || s3 s3api create-bucket --bucket "$BUCKET"; } >/dev/null 2>&1 \
  || echo "  (rustfs '$RUSTFS_CONTAINER' not reachable — assuming bucket '$BUCKET' exists)"

# 1. Admin session.
J=$(mktemp); trap 'rm -f "$J"' EXIT
curl -sf -c "$J" -X POST "$BASE/api/auth/login" -H "Content-Type: application/json" \
  -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$J" | tail -1)
api() { curl -sf -b "$J" -H "X-Egret-CSRF: $CSRF" -H "Content-Type: application/json" "$@"; }

# 2. Resolve the demo workspace by slug; bail early if already seeded.
DEMO_WS=$(psql "$PGURL" -tAc "SELECT id FROM workspaces WHERE slug = 'demo' LIMIT 1")
[ -n "$DEMO_WS" ] || { echo "No 'demo' workspace found — run migrations first."; exit 1; }
if api "$BASE/api/control-plane/warehouses" | grep -q '"name":"demo-catalog"'; then
  echo "✓ demo-catalog already seeded — nothing to do."; exit 0
fi

echo "→ storage connection + warehouse (provisioning lakekeeper)…"
SC_ID=$(api -X POST "$BASE/api/control-plane/storage" -d "{
  \"name\":\"demo-rustfs\",\"provider\":\"s3\",\"endpoint\":\"$S3_ENDPOINT\",
  \"region\":\"us-east-1\",\"bucket\":\"$BUCKET\",\"access_key\":\"$AK\",
  \"new_secret\":{\"name\":\"demo-rustfs-secret\",\"value\":\"$SK\"},\"path_style\":true}" | get id)
WH=$(api -X POST "$BASE/api/control-plane/warehouses" \
  -d "{\"name\":\"demo-catalog\",\"workspaceId\":\"$DEMO_WS\",\"storageConnectionId\":\"$SC_ID\"}")
WH_ID=$(echo "$WH" | get id)
LK=$(echo "$WH" | get lakekeeperWarehouseId)

echo "→ engine role (catalog write) + client…"
ENG_RID=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/roles" \
  -d '{"name":"demo-engine","consumerType":"engine","description":"Full read/write engine"}' | get id)
api -o /dev/null -X POST "$BASE/api/control-plane/roles/$ENG_RID/grants" \
  -d '{"securableType":"catalog","privilege":"write"}'
ENG=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/clients" \
  -d "{\"name\":\"demo-engine-client\",\"consumerType\":\"engine\",\"workspaceId\":\"$DEMO_WS\",\"catalogRoleId\":\"$ENG_RID\"}")
ENG_TOK=$(curl -sf -X POST "$BASE/oauth/token" \
  -d "grant_type=client_credentials&client_id=$(echo "$ENG" | get clientId)&client_secret=$(echo "$ENG" | get clientSecret)" | get access_token)

# Iceberg REST helpers (as the engine).
cat_post() { curl -s -o /dev/null -w "%{http_code}" -X POST "$BASE/catalog/v1/$LK/$1" \
  -H "Authorization: Bearer $ENG_TOK" -H "Content-Type: application/json" -d "$2"; }
mk_ns() { cat_post "namespaces" "{\"namespace\":[\"$1\"],\"properties\":{}}" >/dev/null || true; }
mk_table() { # $1=ns $2=table $3=fields-json
  cat_post "namespaces/$1/tables" "{\"name\":\"$2\",\"schema\":{\"type\":\"struct\",\"schema-id\":0,\"fields\":$3}}" >/dev/null
}

echo "→ creating namespaces + tables…"
mk_ns demo_ns
mk_ns analytics
mk_table demo_ns events    '[{"id":1,"name":"id","required":true,"type":"long"},{"id":2,"name":"name","required":false,"type":"string"},{"id":3,"name":"ts","required":false,"type":"timestamptz"}]'
mk_table demo_ns orders    '[{"id":1,"name":"id","required":true,"type":"long"},{"id":2,"name":"amount","required":false,"type":"double"},{"id":3,"name":"customer_id","required":false,"type":"long"}]'
mk_table demo_ns customers '[{"id":1,"name":"id","required":true,"type":"long"},{"id":2,"name":"email","required":false,"type":"string"},{"id":3,"name":"country","required":false,"type":"string"}]'
mk_table analytics sessions  '[{"id":1,"name":"session_id","required":true,"type":"string"},{"id":2,"name":"user_id","required":false,"type":"long"},{"id":3,"name":"started_at","required":false,"type":"timestamptz"}]'
mk_table analytics pageviews '[{"id":1,"name":"url","required":true,"type":"string"},{"id":2,"name":"ts","required":false,"type":"timestamptz"},{"id":3,"name":"session_id","required":false,"type":"string"}]'

echo "→ reader role (read on demo_ns.events + demo_ns.orders ONLY) + client…"
RD_RID=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/roles" \
  -d '{"name":"demo-reader","consumerType":"external","description":"Read-only on two tables"}' | get id)
api -o /dev/null -X POST "$BASE/api/control-plane/roles/$RD_RID/grants" \
  -d '{"securableType":"table","namespace":["demo_ns"],"tableName":"events","privilege":"read"}'
api -o /dev/null -X POST "$BASE/api/control-plane/roles/$RD_RID/grants" \
  -d '{"securableType":"table","namespace":["demo_ns"],"tableName":"orders","privilege":"read"}'
RD=$(api -X POST "$BASE/api/control-plane/warehouses/$WH_ID/clients" \
  -d "{\"name\":\"demo-reader-client\",\"consumerType\":\"external\",\"workspaceId\":\"$DEMO_WS\",\"catalogRoleId\":\"$RD_RID\"}")
RD_TOK=$(curl -sf -X POST "$BASE/oauth/token" \
  -d "grant_type=client_credentials&client_id=$(echo "$RD" | get clientId)&client_secret=$(echo "$RD" | get clientSecret)" | get access_token)

echo ""
echo "→ verifying the reader's table-level scope…"
check() { # $1=ns $2=table $3=expected(200/403)
  code=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/catalog/v1/$LK/namespaces/$1/tables/$2" -H "Authorization: Bearer $RD_TOK")
  if [ "$code" = "$3" ]; then echo "   ✓ reader $1.$2 → $code (expected $3)";
  else echo "   ✗ reader $1.$2 → $code (EXPECTED $3)"; FAIL=1; fi
}
FAIL=0
check demo_ns   events    200   # granted
check demo_ns   orders    200   # granted
check demo_ns   customers 403   # NOT granted (same namespace)
check analytics sessions  403   # NOT granted (other namespace)

cat <<EOF

✅ Demo catalog seeded (workspace: demo):
   Warehouse : demo-catalog
   Tables    : demo_ns.{events, orders, customers}, analytics.{sessions, pageviews}
   Engine    : role demo-engine (catalog write) — creates/reads everything
   Reader    : role demo-reader (external) — read ONLY demo_ns.events + demo_ns.orders

   Browse it: Control Plane → Warehouses → demo-catalog → Browse
EOF
[ "$FAIL" = 0 ] || { echo ""; echo "⚠️  reader scope check FAILED — see ✗ lines above."; exit 1; }
