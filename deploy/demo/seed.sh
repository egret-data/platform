#!/usr/bin/env bash
# DEMO ONLY. Seed a running demo stack with sample content. Idempotent: every object is looked up
# by name first and created only when missing. Usage: just demo-seed
set -euo pipefail
cd "$(dirname "$0")/../.."
# Values already exported by the caller (for example DEMO_PORT) win over demo.env.
CALLER_PORT="${DEMO_PORT:-}"
set -a; . deploy/demo/demo.env; set +a
[ -z "$CALLER_PORT" ] || DEMO_PORT="$CALLER_PORT"

BASE="${BASE:-http://${DEMO_HOST}:${DEMO_PORT}}"
WS="${WS:-default}"
JAR=$(mktemp); trap 'rm -f "$JAR"' EXIT

curl -fsS -c "$JAR" -H 'content-type: application/json' \
  -d "{\"email\":\"$EGRET_ADMIN_EMAIL\",\"password\":\"$EGRET_ADMIN_PASSWORD\"}" "$BASE/api/auth/login" >/dev/null
CSRF=$(awk '$6=="egret_csrf"{print $7}' "$JAR")

get()  { curl -fsS -b "$JAR" "$BASE$1"; }
call() { curl -fsS -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'content-type: application/json' -X "$1" -d "$3" "$BASE$2"; }
id()   { python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])'; }
find_id() { # find_id <path> <name>: id of the first object with that name, or empty
  get "$1" | python3 -c '
import json,sys
want=sys.argv[1]
rows=json.load(sys.stdin)
rows=rows.get("items",rows) if isinstance(rows,dict) else rows
print(next((r["id"] for r in rows if r.get("name")==want),""))' "$2"
}

# 1. Trino database
DB=$(find_id "/api/ws/$WS/databases" trino-demo)
if [ -z "$DB" ]; then
  DB=$(call POST "/api/ws/$WS/databases" '{"name":"trino-demo","engine":"trino","host":"trino-demo","port":8080,"options":{"user":"egret","http_scheme":"http","catalog":"tpch"}}' | id)
  echo "database trino-demo $DB"
else echo "database trino-demo exists"; fi

# 2. Dataset and metrics (each metric is added when missing, so a partial earlier run is repaired)
DS=$(find_id "/api/ws/$WS/datasets" orders)
if [ -z "$DS" ]; then
  DS=$(call POST "/api/ws/$WS/datasets" "{\"name\":\"orders\",\"database_id\":\"$DB\",\"description\":\"TPC-H sample orders\",\"source\":{\"kind\":\"physical\",\"catalog\":\"tpch\",\"schema\":\"tiny\",\"table\":\"orders\"}}" | id)
  echo "dataset orders $DS"
else echo "dataset orders exists"; fi
metric() { # metric <name> <verbose> <expression>
  if get "/api/ws/$WS/datasets/$DS" | python3 -c 'import json,sys;sys.exit(0 if any(m["metric_name"]==sys.argv[1] for m in json.load(sys.stdin)["metrics"]) else 1)' "$1"; then
    echo "metric $1 exists"
  else
    call POST "/api/ws/$WS/datasets/$DS/metrics" "{\"metric_name\":\"$1\",\"verbose_name\":\"$2\",\"expression\":\"$3\"}" >/dev/null
    echo "metric $1 created"
  fi
}
metric revenue Revenue 'sum(totalprice)'
metric order_count Orders 'count(*)'

# 3. Charts and dashboard
spec() { echo "{\"metrics\":$1,\"dimensions\":$2,\"columns\":[],\"filters\":[],\"orderby\":$3,\"row_limit\":1000,\"rollup_groups\":[]$4}"; }
chart() { # chart <name> <viz> <spec-json> [settings-json]  (cartesian kinds need cartesian.xAxis/yMetrics in settings)
  local c st='{}'; [ -z "${4:-}" ] || st=$4
  c=$(find_id "/api/ws/$WS/charts" "$1")
  if [ -z "$c" ]; then
    c=$(call POST "/api/ws/$WS/charts" "{\"dataset_id\":\"$DS\",\"name\":\"$1\",\"viz_type\":\"$2\",\"params\":{\"version\":2,\"settings\":$st,\"spec\":$3}}" | id)
  fi
  echo "$c"
}
BAR=$(chart "Revenue by order status" bar "$(spec '["revenue"]' '["orderstatus"]' '[]' '')" '{"cartesian.xAxis":"orderstatus","cartesian.yMetrics":["revenue"]}')
LINE=$(chart "Revenue by month" line "$(spec '["revenue"]' '["orderdate"]' '[{"field":"orderdate","desc":false}]' ',"temporal_units":{"orderdate":"month"}')" '{"cartesian.xAxis":"orderdate","cartesian.yMetrics":["revenue"]}')
TBL=$(chart "Orders by priority" table "$(spec '["order_count"]' '["orderpriority"]' '[]' '')")

DASH=$(find_id "/api/ws/$WS/dashboards" "Sales overview")
if [ -z "$DASH" ]; then
  DASH=$(call POST "/api/ws/$WS/dashboards" '{"name":"Sales overview","description":"Demo dashboard"}' | id)
  NEW=1; echo "dashboard Sales overview $DASH"
else
  NEW=0
fi
CARDS=$(get "/api/ws/$WS/dashboards/$DASH" | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["cards"]))')
if [ "$CARDS" = 0 ]; then
  call PUT "/api/ws/$WS/dashboards/$DASH" "{\"name\":\"Sales overview\",\"description\":\"Demo dashboard\",\"parameters\":[],\"width\":\"full\",\"tabs\":[],\"cards\":[
   {\"chart_id\":\"$BAR\",\"card_type\":\"chart\",\"grid_col\":0,\"grid_row\":0,\"size_x\":12,\"size_y\":4,\"options\":{}},
   {\"chart_id\":\"$LINE\",\"card_type\":\"chart\",\"grid_col\":12,\"grid_row\":0,\"size_x\":12,\"size_y\":4,\"options\":{}},
   {\"chart_id\":\"$TBL\",\"card_type\":\"chart\",\"grid_col\":0,\"grid_row\":4,\"size_x\":24,\"size_y\":5,\"options\":{}}]}" >/dev/null
  [ "$NEW" = 1 ] || echo "dashboard Sales overview cards repaired"
else echo "dashboard Sales overview exists"; fi

# 4. Workspace SMTP -> Mailpit (a PUT replaces; safe to repeat). Needs EGRET_SECRETS__MASTER_KEY on the
#    server: without it this call answered 500 during the October 2026 docs run.
call PUT "/api/ws/$WS/notifications/smtp" '{"host":"mailpit","port":1025,"from_address":"egret@demo.local","from_name":"Egret demo","tls_mode":"none"}' >/dev/null
echo "smtp -> mailpit:1025"
echo "seed done: $BASE (workspace $WS)"
