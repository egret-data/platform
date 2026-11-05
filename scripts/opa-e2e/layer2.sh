#!/usr/bin/env bash
# E2E ONLY. Layer 2 of the OPA target (governance SP2b, Task 21): the end-to-end differential.
# Run after scripts/opa-e2e/up.sh. For every scenario in scenarios.sql.d/ (one per Trino-shaped
# golden scenario of sub-project 1), it resets the profile's policies, applies the scenario's
# admin setup through egret's API, waits for egret's OPA refresher and the Ranger plugin's poll,
# then runs every statement as every scenario user on BOTH engines:
#   trino-opa    — provisioned by egret, OPA plugin -> egret's OPA surface;
#   trino-ranger — the Ranger plugin on the SAME profile's Ranger download (the oracle, S8;
#                  see up.sh step 6/7).
# compare.py then compares the two sides and writes layer2-report.json. Exit = compare.py's.
#
# Scenario file format (scenarios.sql.d/*.sql):
#   -- users: ann,bob                         the users every statement runs as
#   -- setup: [VAR =] METHOD /api/path [JSON] an egret API call as the admin; VAR keeps its .id
#   -- setup: VAR = ENSURE-WORKSPACE slug     a workspace by slug (created once), its id in VAR
#   -- refused: METHOD /api/path [JSON]       an API call that must answer non-2xx
#   -- setup-sql: SQL                         SQL run by the platform admin on trino-ranger
#   SQL                                       a compared statement, one per line
#   @session-user U: SQL                      run as the scenario user impersonating U
# `@NAME@` in a setup line is replaced by the value of NAME: CAT, WH, WS, ENGINE, ADMIN,
# UID_<user>, or a VAR an earlier setup line set. Nothing in a scenario file is passed to eval.
#
# After the scenario's own statements, every user also runs the compared statements every
# scenario shares (COMMON below): the listings, information_schema, a `$files` table,
# SET SESSION query_max_run_time, kill_query against another user's query, and impersonation
# by the delegator (egret_core::engine_principals::DELEGATOR_PRINCIPAL).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$REPO/scripts/opa-e2e"
COMPOSE=(docker compose -f "$REPO/deploy/opa-e2e/docker-compose.yml" -p egret-opa-e2e)
export WORK="${WORK:-${TMPDIR:-/tmp}/egret-opa-e2e}"
export EGRET_OPA_E2E_PORT="${EGRET_OPA_E2E_PORT:-28081}"
BASE="http://127.0.0.1:${EGRET_OPA_E2E_PORT}"
OPA_CONTAINER=egret-engine-egret-engines-trino-opa-coordinator
CATALOG=lake
DELEGATOR=egret_delegator # egret_core::engine_principals::DELEGATOR_PRINCIPAL
SYNC_TIMEOUT="${LAYER2_SYNC_TIMEOUT:-120}" # seconds to wait for trino-ranger's plugin to catch up
OUT="${LAYER2_OUT:-$WORK/layer2}"
REPORT="${LAYER2_REPORT:-$REPO/layer2-report.json}"
ONLY="${LAYER2_ONLY:-}"     # a regex, e.g. 'G06|G11': run only the scenarios whose file name it matches at the start
SETUP_ONLY="${LAYER2_SETUP_ONLY:-}" # non-empty: apply each scenario's setup and run nothing (a dry run of the setups)

log() { printf '→ %s\n' "$*" >&2; }
die() { printf 'layer2.sh: %s\n' "$*" >&2; exit 1; }
jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }

RANGER_CONTAINER=$("${COMPOSE[@]}" ps -q trino-ranger)
[ -n "$RANGER_CONTAINER" ] || die "trino-ranger is not running: run up.sh first"
docker exec "$OPA_CONTAINER" grep -q '^access-control.name=opa' /etc/trino/access-control.properties \
  || die "$OPA_CONTAINER is not on the OPA plugin: run up.sh first"

# --- admin session ------------------------------------------------------------------------
JAR="$WORK/admin.jar"
curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
  -d '{"email":"admin@egret.local","password":"Admin12345!"}' -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
api_code() { curl -sS -o "$WORK/last.json" -w '%{http_code}' -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
# ids(expr): the list a list route answers, one `expr` (over d, an item; None skips) per line.
ids() { python3 -c "
import sys, json
d = json.load(sys.stdin)
rows = d if isinstance(d, list) else next((d[k] for k in ('items', 'grants', 'rules', 'data', 'groups', 'roles', 'tags', 'workspaces') if k in d), [])
for d in rows:
    v = $1
    if v is not None:
        print(v)"; }

# Values a setup line can name as @NAME@ live in V_NAME (bash 3.2 has no associative arrays).
V_CAT=$(api "$BASE/api/control-plane/catalogs" | jget "next((c['id'] for c in d if c['name']=='$CATALOG'), None)")
[ -n "${V_CAT}" ] || die "no catalog $CATALOG: run up.sh first"
V_WH=$(api "$BASE/api/control-plane/warehouses" | jget "next((w['id'] for w in (d if isinstance(d,list) else d.get('items',[])) if w['name']=='$CATALOG'), None)")
[ -n "${V_WH}" ] || die "no warehouse $CATALOG"
V_WS=$(api "$BASE/api/me/workspaces" | jget "d[0]['id']")
V_ENGINE=$(api "$BASE/api/control-plane/engines" | jget "next((e['id'] for e in (d if isinstance(d,list) else d.get('items',[])) if e['name']=='trino-opa'), None)")
[ -n "${V_ENGINE}" ] || die "no engine trino-opa"
V_ADMIN=$(api "$BASE/api/control-plane/users" | jget "next((u['username'] for u in (d if isinstance(d,list) else d.get('items',[])) if u['email']=='admin@egret.local'), None)")
ADMIN=${V_ADMIN}
for u in ann bob carol dan; do
  if ! api "$BASE/api/control-plane/users" | grep "\"$u@egret.local\"" >/dev/null; then
    api -X POST "$BASE/api/control-plane/users" \
      -d "{\"email\":\"$u@egret.local\",\"name\":\"$u\",\"password\":\"E2e-password-1!\"}" >/dev/null
  fi
  printf -v "V_UID_$u" '%s' "$(api "$BASE/api/control-plane/users" | jget "next((u['id'] for u in (d if isinstance(d,list) else d.get('items',[])) if u['email']=='$u@egret.local'), None)")"
done

# --- the engines --------------------------------------------------------------------------
# Every compared statement goes through trino-batch.sh INSIDE each engine's container (Trino's
# client protocol over curl; see that file for why not the CLI). One docker exec per side.
batch() { # $1 = container, stdin = jobs; stdout = protocol responses
  docker exec -i "$1" sh -c "$(cat "$HERE/trino-batch.sh")" trino-batch "$ADMIN"
}
admin_sql() { # on trino-ranger, as the platform admin
  docker exec "$RANGER_CONTAINER" trino --user "$ADMIN" --output-format TSV --execute "$1" 2>&1 \
    | grep -v '^Picked up JAVA_TOOL_OPTIONS' || true
}

# --- data: lake.s1.{t1,t2} come from up.sh; s2.t3 and the empty scratch table s2.w here -----
if [ "$(admin_sql "SELECT count(*) FROM $CATALOG.s2.t3" | tail -1)" != 100 ]; then
  if [ "$(admin_sql "SELECT count(*) FROM $CATALOG.s2.t3_v2" | tail -1)" = 100 ]; then
    admin_sql "ALTER TABLE $CATALOG.s2.t3_v2 RENAME TO $CATALOG.s2.t3" >/dev/null
  else
    log "seeding $CATALOG.s2.t3 / w"
    admin_sql "CREATE SCHEMA IF NOT EXISTS $CATALOG.s2" >/dev/null
    admin_sql "DROP TABLE IF EXISTS $CATALOG.s2.t3" >/dev/null
    admin_sql "CREATE TABLE $CATALOG.s2.t3 AS SELECT n AS id, n * 100 AS salary,
      CASE WHEN n % 3 = 0 THEN 'ops' ELSE 'eng' END AS dept FROM UNNEST(sequence(1, 100)) AS t(n)" >/dev/null
  fi
fi
admin_sql "CREATE TABLE IF NOT EXISTS $CATALOG.s2.w (id bigint)" >/dev/null
[ "$(admin_sql "SELECT count(*) FROM $CATALOG.s2.t3" | tail -1)" = 100 ] || die "seeding s2.t3 failed"

# --- reset: every perm object, and what the scenarios create (l2-* / l2_*) -------------------
reset_policies() {
  local id
  for id in $(api "$BASE/api/control-plane/perm/engine-grants" | ids "d['id']"); do
    api -X DELETE "$BASE/api/control-plane/perm/engine-grants/$id" -o /dev/null
  done
  for id in $(api "$BASE/api/control-plane/perm/rules" | ids "d['id']"); do
    api -X DELETE "$BASE/api/control-plane/perm/rules/$id" -o /dev/null
  done
  # Explicit grants only: a `workspace_assignment` grant belongs to the catalog's workspace
  # assignment, which is put back to the default workspace alone.
  for id in $(api "$BASE/api/control-plane/perm/grants" | ids "d['id'] if d.get('origin', {}).get('type') == 'explicit' else None"); do
    api -X DELETE "$BASE/api/control-plane/perm/grants/$id" -o /dev/null
  done
  api -X PUT "$BASE/api/control-plane/catalogs/$V_CAT/workspaces" -d "{\"workspaceIds\":[\"$V_WS\"]}" -o /dev/null
  for id in $(api "$BASE/api/control-plane/tags" | ids "d['id'] if d['key'].startswith('l2-') else None"); do
    api -X DELETE "$BASE/api/control-plane/tags/$id" -o /dev/null
  done
  for id in $(api "$BASE/api/control-plane/groups" | ids "d['id'] if d['name'].startswith('l2_') else None"); do
    api -X DELETE "$BASE/api/control-plane/groups/$id" -o /dev/null
  done
  for id in $(api "$BASE/api/control-plane/access/roles" | ids "d['id'] if d['name'].startswith('l2_') else None"); do
    api -X DELETE "$BASE/api/control-plane/access/roles/$id" -o /dev/null
  done
  if [ "$(admin_sql "SELECT count(*) FROM $CATALOG.s2.t3_v2" | tail -1)" = 100 ]; then
    admin_sql "ALTER TABLE $CATALOG.s2.t3_v2 RENAME TO $CATALOG.s2.t3" >/dev/null
  fi
  # The baseline every scenario shares: listings read lake.information_schema, which both
  # plugins authorize like any table (Task 20, D4).
  api -X POST "$BASE/api/control-plane/perm/grants" -d "{\"effect\":\"allow\",\"actions\":[\"read\"],
    \"scope\":{\"catalog_id\":\"${V_CAT}\",\"schema\":\"information_schema\"},
    \"principals\":{\"include\":[{\"kind\":\"group\",\"ref\":\"public\"}],\"exclude\":[]}}" -o /dev/null
}

subst() { # replace every @NAME@ with V[NAME]
  local s=$1 name
  while [[ $s =~ @([A-Za-z_][A-Za-z0-9_]*)@ ]]; do
    name=${BASH_REMATCH[1]}
    local ref="V_$name"
    [ -n "${!ref+x}" ] || die "setup names @$name@, which nothing set"
    s=${s//@$name@/${!ref}}
  done
  printf '%s' "$s"
}

setup_line() { # $1 = setup|refused, $2 = the rest of the line
  local kind=$1 line var="" method path body code
  line=$(subst "$2")
  if [[ $line =~ ^([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=[[:space:]]*(.*)$ ]]; then
    var=${BASH_REMATCH[1]}; line=${BASH_REMATCH[2]}
  fi
  if [[ $line =~ ^ENSURE-WORKSPACE[[:space:]]+([a-z0-9-]+)$ ]]; then
    local slug=${BASH_REMATCH[1]} id
    id=$(api "$BASE/api/admin/workspaces" | ids "d['id'] if d.get('slug')=='$slug' else None" | head -1)
    if [ -z "$id" ]; then
      id=$(api -X POST "$BASE/api/admin/workspaces" -d "{\"slug\":\"$slug\",\"name\":\"$slug\"}" | jget "d['id']")
    fi
    printf -v "V_$var" '%s' "$id"
    return
  fi
  method=${line%% *}; line=${line#* }
  path=${line%% *}; body=""
  [ "$path" != "$line" ] && body=${line#* }
  if [ -n "$body" ]; then
    code=$(api_code -X "$method" "$BASE$path" -d "$body")
  else
    code=$(api_code -X "$method" "$BASE$path")
  fi
  if [ "$kind" = refused ]; then
    [[ $code =~ ^2 ]] && die "$path answered $code; the scenario expects a refusal: $(cat "$WORK/last.json")"
    log "   refused as expected ($code): $method $path"
    return
  fi
  if ! [[ $code =~ ^2 ]]; then
    # A member/role already present answers 409 on a re-run; anything else is a broken setup.
    [ "$code" = 409 ] && [[ $path =~ /members$|/roles$ ]] && return
    die "$method $path answered $code: $(cat "$WORK/last.json")"
  fi
  if [ -n "$var" ]; then
    printf -v "V_$var" '%s' "$(jget "d['id']" <"$WORK/last.json")"
  fi
}

# --- wait until both engines answer from the scenario's state -------------------------------
# trino-ranger: the Ranger plugin polls policies and tags every 30 s; it is caught up when the
# policyVersion and tagVersion in its cache equal what egret serves now (read with the plugin's
# own credential from the files up.sh installed; never printed). trino-opa: egret's refresher
# rebuilds the snapshot every 2 s; a further 5 s covers it.
XML="$WORK/trino-ranger/etc/ranger-trino-security.xml"
PLUGIN_USER=$(sed -n 's#.*client.username</name><value>\([^<]*\)<.*#\1#p' "$XML")
PLUGIN_PASS=$(sed -n 's#.*client.password</name><value>\([^<]*\)<.*#\1#p' "$XML")
SERVICE=$(sed -n 's#.*service.name</name><value>\([^<]*\)<.*#\1#p' "$XML")
served_version() { # $1 = policies|tags
  local p="plugins/secure/policies/download/$SERVICE" k=policyVersion
  [ "$1" = tags ] && p="tags/secure/download/$SERVICE" && k=tagVersion
  curl -sS -u "$PLUGIN_USER:$PLUGIN_PASS" "$BASE/ranger/service/$p" | jget "d.get('$k')"
}
cached_version() { # $1 = policies|tags
  local f="trino_${SERVICE}.json" k=policyVersion
  [ "$1" = tags ] && f="trino_${SERVICE}_tag.json" && k=tagVersion
  docker exec "$RANGER_CONTAINER" cat "/tmp/ranger-cache/$f" 2>/dev/null | jget "d.get('$k')" 2>/dev/null || true
}
wait_synced() {
  local sp st start=$SECONDS
  sleep 3
  sp=$(served_version policies); st=$(served_version tags)
  until [ "$(cached_version policies)" = "$sp" ] && [ "$(cached_version tags)" = "$st" ]; do
    [ $((SECONDS - start)) -lt "$SYNC_TIMEOUT" ] || die "trino-ranger did not reach policyVersion $sp / tagVersion $st"
    sleep 2
  done
  sleep 5
  log "   synced: policyVersion $sp, tagVersion $st ($((SECONDS - start)) s)"
}

# --- the jobs of one scenario ---------------------------------------------------------------
# $1 = scenario dir with `users` and `statements` (sid TAB session-user TAB sql); writes
# opa.jobs, ranger.jobs (each with the admin's `raw` reference for plain SELECTs) and meta.tsv
# (label TAB sid TAB the statement as the report shows it).
write_jobs() {
  local dir=$1 user sid su sql via u o shown eng
  : >"$dir/opa.jobs"; : >"$dir/ranger.jobs"; : >"$dir/meta.tsv"
  while read -r user; do
    while IFS=$'\t' read -r sid su sql; do
      [ "$su" = - ] && su=""
      via=direct; u=$user; o=-; shown=$sql
      if [ "$sql" = KILL ]; then
        shown="CALL system.runtime.kill_query(query_id => '<the admin''s running query>', message => 'layer2')"
      elif [ "$su" = DELEGATOR ]; then
        via=impersonate; o=$DELEGATOR
      elif [ -n "$su" ]; then
        u=$su; o=$user; shown="[impersonating $su] $sql"
      fi
      for eng in opa ranger; do
        printf '%s/%s/%s\t%s\t%s\t%s\t%s\n' "$eng" "$user" "$via" "$sid" "$u" "$o" "$sql" >>"$dir/$eng.jobs"
      done
      printf '%s/%s\t%s\t%s\n' "$user" "$via" "$sid" "$shown" >>"$dir/meta.tsv"
      if [ -z "$su" ] && [[ $sql =~ ^SELECT ]]; then
        printf 'raw/%s/%s\t%s\t%s\t-\t%s\n' "$user" "$via" "$sid" "$ADMIN" "$sql" >>"$dir/ranger.jobs"
      fi
    done <"$dir/statements"
  done <"$dir/users"
}

# --- the statements every scenario shares -------------------------------------------------
COMMON=(
  "SHOW CATALOGS"
  "SHOW SCHEMAS FROM $CATALOG"
  "SHOW TABLES FROM $CATALOG.s1"
  "SHOW COLUMNS FROM $CATALOG.s1.t1"
  "SELECT table_name, column_name FROM $CATALOG.information_schema.columns ORDER BY 1, 2"
  "SELECT count(*) FROM $CATALOG.s1.\"t1\$files\""
  "SET SESSION query_max_run_time = '1m'"
  "KILL"
)
IMPERSONATED=("SELECT current_user" "SELECT count(*) FROM $CATALOG.s1.t1")

rm -rf "$OUT"; mkdir -p "$OUT"
RESULTS="$OUT/results.jsonl"; : >"$RESULTS"
for f in "$HERE"/scenarios.sql.d/*.sql; do
  name=$(basename "$f" .sql); sc=${name%%_*}
  [ -n "$ONLY" ] && ! [[ $name =~ ^($ONLY) ]] && continue
  log "scenario $name"
  dir="$OUT/$sc"; mkdir -p "$dir"
  reset_policies
  users=$(sed -n 's/^-- users: *//p' "$f")
  [ -n "$users" ] || die "$f has no '-- users:' line"
  printf "%s\n" "$(subst "$users")" | tr "," "\n" >"$dir/users"
  : >"$dir/statements"; n=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      "-- setup-sql: "*) admin_sql "$(subst "${line#-- setup-sql: }")" >/dev/null ;;
      "-- setup: "*) setup_line setup "${line#-- setup: }" ;;
      "-- refused: "*) setup_line refused "${line#-- refused: }" ;;
      "--"*|"") ;;
      "@session-user "*)
        n=$((n+1)); su=${line#@session-user }; su=${su%%:*}; sql=${line#*: }
        printf 'q%02d\t%s\t%s\n' "$n" "$su" "$sql" >>"$dir/statements" ;;
      *) n=$((n+1)); printf 'q%02d\t-\t%s\n' "$n" "$line" >>"$dir/statements" ;;
    esac
  done <"$f"
  n=0
  for s in "${COMMON[@]}"; do n=$((n+1)); printf 'c%02d\t-\t%s\n' "$n" "$s" >>"$dir/statements"; done
  n=0
  for s in "${IMPERSONATED[@]}"; do n=$((n+1)); printf 'd%02d\tDELEGATOR\t%s\n' "$n" "$s" >>"$dir/statements"; done
  [ -n "$SETUP_ONLY" ] && continue
  write_jobs "$dir"
  wait_synced
  batch "$OPA_CONTAINER" <"$dir/opa.jobs" >"$dir/opa.out" & p1=$!
  batch "$RANGER_CONTAINER" <"$dir/ranger.jobs" >"$dir/ranger.out" & p2=$!
  wait "$p1"; wait "$p2"
done
[ -n "$SETUP_ONLY" ] && { log "setup-only: every setup applied; the last scenario's state is left in place"; exit 0; }
reset_policies

# --- collect -------------------------------------------------------------------------------
python3 -I - "$OUT" "$RESULTS" <<'PY'
import json, os, sys
out, results = sys.argv[1], sys.argv[2]
with open(results, "w") as w:
    for sc in sorted(os.listdir(out)):
        sd = os.path.join(out, sc)
        if not os.path.isfile(os.path.join(sd, "meta.tsv")):
            continue
        shown = {}
        for line in open(os.path.join(sd, "meta.tsv")):
            uv, sid, stmt = line.rstrip("\n").split("\t", 2)
            shown[(uv, sid)] = stmt
        for side in ("opa", "ranger"):
            cur = None
            for line in open(os.path.join(sd, f"{side}.out")):
                line = line.rstrip("\n")
                if line.startswith("@@BEGIN "):
                    label, sid = line.split(" ")[1:3]
                    cur = {"label": label, "sid": sid, "pages": []}
                elif line.startswith("@@END "):
                    engine, user, via = cur["label"].split("/")
                    cols, rows, err = [], [], None
                    for page in cur["pages"]:
                        try:
                            d = json.loads(page)
                        except ValueError:
                            err = err or f"layer2 harness: not JSON: {page[:200]}"
                            continue
                        if d.get("columns"):
                            cols = [c["name"] for c in d["columns"]]
                        rows += d.get("data") or []
                        if d.get("error"):
                            e = d["error"]
                            err = f"{e.get('errorName', '')}: {e.get('message', '')}"
                    if not cur["pages"]:
                        err = "layer2 harness: no response"
                    rec = {"scenario": sc, "user": user, "sid": cur["sid"], "via": via,
                           "engine": engine, "statement": shown[(f"{user}/{via}", cur["sid"])],
                           "ok": err is None, "columns": cols if err is None else [],
                           "rows": rows if err is None else [], "error": err}
                    w.write(json.dumps(rec) + "\n")
                    cur = None
                elif cur is not None and line:
                    cur["pages"].append(line)
PY
cp "$RESULTS" "$(dirname "$REPORT")/layer2-results.jsonl" 2>/dev/null || true
python3 -I "$HERE/compare.py" "$RESULTS" --report "$REPORT"
