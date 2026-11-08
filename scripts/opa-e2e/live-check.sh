#!/usr/bin/env bash
# E2E ONLY. The live check for Trino 483 on egret's OPA surface (governance SP2b, Task 20).
# Run after scripts/opa-e2e/up.sh. Writes policies as the admin, then runs the same
# statements as `ann` on trino-opa (PROVISIONED, OPA plugin -> egret's OPA surface) and on
# trino-ranger (Ranger plugin -> the same profile's Ranger download, the oracle; see up.sh
# step 6/7), and prints both sides. Assertions are on trino-opa; each also gets a SAME /
# STRICTER / LOOSER comparison with the oracle, and LOOSER fails. Exit 1 on any FAIL.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$REPO/deploy/opa-e2e/docker-compose.yml" -p egret-opa-e2e)
export WORK="${WORK:-${TMPDIR:-/tmp}/egret-opa-e2e}"
export EGRET_OPA_E2E_PORT="${EGRET_OPA_E2E_PORT:-28081}"
BASE="http://127.0.0.1:${EGRET_OPA_E2E_PORT}"
OPA_CONTAINER=egret-engine-egret-engines-trino-opa-coordinator
CATALOG=lake
FAILS=0

jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }

JAR="$WORK/admin.jar"
curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
  -d '{"email":"admin@egret.local","password":"Admin12345!"}' -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }

CAT_ID=$(api "$BASE/api/control-plane/catalogs" | jget "next((c['id'] for c in d if c['name']=='$CATALOG'), None)")
[ -n "$CAT_ID" ] || { echo "no catalog $CATALOG: run up.sh first" >&2; exit 1; }

# --- 1. policies (idempotent: each is created once, by name / by its exact body) -----------
grants=$(api "$BASE/api/control-plane/perm/grants")
grant_once() { # $1 = effect, $2 = table or "", $3 = column or "", $4 = user, $5 = schema (s1)
  local schema=${5:-s1} col_json=null tbl_json=null
  [ -n "$3" ] && col_json="\"$3\""
  [ -n "$2" ] && tbl_json="\"$2\""
  if ! GRANTS="$grants" python3 - "$1" "$2" "$3" "$4" "$schema" <<'PY'
import json, os, sys
eff, table, col, user, schema = sys.argv[1:]
d = json.loads(os.environ["GRANTS"])
rows = d if isinstance(d, list) else d.get("grants", d.get("items", []))
for g in rows:
    s = g.get("scope", {})
    inc = [p.get("ref") for p in g.get("principals", {}).get("include", [])]
    if g.get("effect") == eff and s.get("schema") == schema and (s.get("table") or "") == table \
       and (s.get("column") or "") == col and user in inc:
        sys.exit(0)
sys.exit(1)
PY
  then
    api -X POST "$BASE/api/control-plane/perm/grants" -d "{
      \"effect\":\"$1\",\"actions\":[\"read\"],
      \"scope\":{\"catalog_id\":\"$CAT_ID\",\"schema\":\"$schema\",\"table\":$tbl_json,\"column\":$col_json},
      \"principals\":{\"include\":[{\"kind\":\"user\",\"ref\":\"$4\"}],\"exclude\":[]}}" >/dev/null
    echo "   + grant $1 read $4 on $schema${2:+.$2}${3:+.$3}"
  fi
}
rules=$(api "$BASE/api/control-plane/perm/rules")
rule_once() { # $1 = name, $2 = kind json, $3 = column or ""
  local col_json=null; [ -n "$3" ] && col_json="\"$3\""
  if ! grep -q "\"$1\"" <<<"$rules"; then
    api -X POST "$BASE/api/control-plane/perm/rules" -d "{
      \"name\":\"$1\",\"kind\":$2,
      \"selector\":{\"type\":\"path\",\"catalog_id\":\"$CAT_ID\",\"schema\":\"s1\",\"table\":\"t1\",\"column\":$col_json},
      \"exempt\":{\"include\":[],\"exclude\":[]}}" >/dev/null
    echo "   + rule $1"
  fi
}
echo "== 1. policies"
grant_once allow t1 "" ann
grant_once allow t1 "" bob
grant_once deny  t1 ssn ann
# SHOW COLUMNS / SHOW TABLES read lake.information_schema, which Trino authorizes like any
# table: without a read there both plugins refuse them outright (measured).
grant_once allow "" "" ann information_schema
rule_once e2e-mask-email '{"type":"mask","mask_type":"MASK_HASH"}' email
rule_once e2e-mask-name-custom '{"type":"mask","mask_type":"CUSTOM","value_expr":"concat('"'"'x-'"'"', {col})"}' name
rule_once e2e-filter-region '{"type":"row_filter","filter_expr":"region = '"'"'us'"'"'"}' ""

# --- 2. egret's refresher, then the Ranger plugin's poll -----------------------------------
echo "== 2. wait: OPA refresher (2 s) and the Ranger plugin's policy poll (30 s)"
sleep "${LIVE_CHECK_WAIT:-35}"

# --- 3/4. the same statement on both engines, through each one's own Trino CLI ----------
# trino-opa is reached with `docker exec` (a provisioned engine publishes no port). Both engines
# refuse authentication over plain HTTP (port-8080 hardening), so the CLI goes over HTTPS 8443 as
# the delegator impersonating the user, as egret's own query path does (trino-auth.sh).
. "$REPO/scripts/opa-e2e/trino-auth.sh"
TRINO_DELEGATOR_PASSWORD=$(delegator_password trino-opa) || { echo "cannot read trino-opa's delegator password" >&2; exit 1; }
# Mask it in CI logs, so a future `set -x` cannot print it.
if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::add-mask::$TRINO_DELEGATOR_PASSWORD"; fi
export TRINO_DELEGATOR_PASSWORD
echo "== 2b. the OPA surface is on its own port"
opa_port_check "$OPA_CONTAINER" || { echo "FAIL opa-port"; FAILS=$((FAILS+1)); }
RANGER_CONTAINER=$("${COMPOSE[@]}" ps -q trino-ranger)
q() { # $1 = opa|ranger, $2 = user, $3 = sql  -> rows as TSV, or the CLI's error line
  local out
  if [ "$1" = opa ]; then
    out=$(trino_cli "$OPA_CONTAINER" "$2" "$3" 2>&1 || true)
  else
    out=$(trino_cli "$RANGER_CONTAINER" "$2" "$3" 2>&1 || true)
  fi
  grep -v '^Picked up JAVA_TOOL_OPTIONS' <<<"$out" | sed -E 's/^Query [0-9a-z_]+ failed: /ERROR: /'
}

declare -a NAMES SQLS
add() { NAMES+=("$1"); SQLS+=("$2"); }
add email_masked      "SELECT email FROM $CATALOG.s1.t1 LIMIT 1"
add region_filtered   "SELECT DISTINCT region FROM $CATALOG.s1.t1"
add t2_star_denied    "SELECT * FROM $CATALOG.s1.t2"
add t2_count_denied   "SELECT count(*) FROM $CATALOG.s1.t2"
add show_columns      "SHOW COLUMNS FROM $CATALOG.s1.t1"
add info_schema_cols  "SELECT column_name FROM $CATALOG.information_schema.columns WHERE table_schema = 's1' AND table_name = 't1' ORDER BY column_name"
add email_still_masked "SELECT email FROM $CATALOG.s1.t1 ORDER BY id LIMIT 1"
add custom_mask_col   "SELECT name FROM $CATALOG.s1.t1 ORDER BY id LIMIT 1"
add ssn_denied        "SELECT ssn FROM $CATALOG.s1.t1 LIMIT 1"
add count_t1_filtered "SELECT count(*) FROM $CATALOG.s1.t1"
add show_tables       "SHOW TABLES FROM $CATALOG.s1"

check() { # $1 = name, $2 = output
  local o="$2"
  case "$1" in
    email_masked|email_still_masked) grep -Eqx '[0-9a-fA-F]{64}' <<<"$o" ;;
    region_filtered)   [ "$o" = us ] ;;
    t2_star_denied|t2_count_denied|ssn_denied) grep -q 'Access Denied' <<<"$o" ;;
    show_columns)      ! grep -q '^ssn' <<<"$o" && grep -q '^email' <<<"$o" ;;
    info_schema_cols)  ! grep -qx ssn <<<"$o" && grep -qx email <<<"$o" ;;
    custom_mask_col)   [ "$o" = x-name2 ] ;;  # filter keeps even ids; id 2 is the first
    count_t1_filtered) [ "$o" = 500 ] ;;
    show_tables)       grep -qx t1 <<<"$o" && ! grep -qx t2 <<<"$o" ;;
    *) return 1 ;;
  esac
}

# OPA against the oracle: SAME, STRICTER (OPA shows a subset, or refuses what Ranger
# serves) or LOOSER (OPA shows or serves something Ranger does not), which is a finding.
compare() { # $1 = opa output, $2 = ranger output
  python3 - "$1" "$2" <<'PY'
import sys
o, r = sys.argv[1], sys.argv[2]
oe, re_ = o.startswith("ERROR:"), r.startswith("ERROR:")
if o == r:
    print("SAME")
elif oe and re_:
    print("SAME (both refused; the plugins word the message differently)")
elif oe:
    print("STRICTER (OPA refuses, Ranger serves)")
elif re_:
    print("LOOSER (OPA serves, Ranger refuses)")
else:
    so, sr = set(o.splitlines()), set(r.splitlines())
    if so < sr:
        print("STRICTER (OPA shows a subset; Ranger also shows: " + ", ".join(sorted(sr - so)) + ")")
    else:
        print("LOOSER (OPA shows rows Ranger does not)" if so - sr else "DIFFERENT")
PY
}

echo "== 3/4. ann on trino-opa (OPA) and trino-ranger (Ranger)"
for i in "${!NAMES[@]}"; do
  n=${NAMES[$i]}; s=${SQLS[$i]}
  o_opa=$(q opa ann "$s"); o_rgr=$(q ranger ann "$s")
  echo "--- $n: $s"
  echo "  trino-opa   : $(tr '\n' '|' <<<"$o_opa")"
  echo "  trino-ranger: $(tr '\n' '|' <<<"$o_rgr")"
  # The assertions are on trino-opa. trino-ranger is the oracle: its own result against the
  # same assertion is printed, never counted; the comparison is what counts.
  if check "$n" "$o_opa"; then echo "PASS opa      $n"; else echo "FAIL opa      $n"; FAILS=$((FAILS+1)); fi
  if check "$n" "$o_rgr"; then echo "     oracle   $n: the Ranger side meets the same assertion"
  else echo "     oracle   $n: the Ranger side does NOT meet this assertion"; fi
  verdict=$(compare "$o_opa" "$o_rgr")
  case "$verdict" in
    SAME*|STRICTER*) echo "PASS compare $n: $verdict" ;;
    *) echo "FAIL compare $n: $verdict"; FAILS=$((FAILS+1)) ;;
  esac
done

echo "== egret logs: opa: denied"
# tracing colours its field names: strip the ANSI escapes before matching `reason=`.
denied=$("${COMPOSE[@]}" logs --no-color egret 2>/dev/null | sed -E $'s/\x1b\\[[0-9;]*m//g' | grep 'opa: denied' || true)
tail -n 8 <<<"$denied" | cut -c1-400
if grep -q 'reason=' <<<"$denied"; then echo "PASS logs   opa: denied lines carry reason="
else echo "FAIL logs   no 'opa: denied' line with reason="; FAILS=$((FAILS+1)); fi

echo "== result: $FAILS failure(s)"
[ "$FAILS" = 0 ]
