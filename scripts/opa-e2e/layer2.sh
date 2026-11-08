#!/usr/bin/env bash
# E2E ONLY. Layer 2 of the OPA target (governance SP2b, Task 21): the end-to-end differential.
# Run after scripts/opa-e2e/up.sh. For every scenario in scenarios.sql.d/ (one per Trino-shaped
# golden scenario of sub-project 1), on BOTH engines:
#   trino-opa    — provisioned by egret, OPA plugin -> egret's OPA surface;
#   trino-ranger — the Ranger plugin on the SAME profile's Ranger download (the oracle, S8;
#                  see up.sh step 6/7).
#
#  1. reset: delete every perm object and what scenarios create; wait until both engines serve it;
#  2. controls and reference, in that POLICY-FREE state: the platform admin's row count of every
#     base table must be what the seed wrote, on both engines; and every SELECT of the scenario
#     is run by the admin on trino-ranger as the clear reference compare.py judges values by;
#  3. setup through egret's API; wait until both engines serve it;
#  4. the scenario's `-- expect:` positive controls first (a freshness barrier), then every
#     statement as every scenario user, on both engines; the controls must hold on both (a
#     broken RustFS, catalog or policy path cannot pass as "both sides agree");
#  5. compare.py compares the two sides and writes layer2-report.json. Exit = compare.py's.
# Any setup failure, failed control or harness error stops the run and names the scenario.
#
# Scenario file format (scenarios.sql.d/*.sql):
#   -- users: ann,bob                         the users every statement runs as
#   -- setup: [VAR =] METHOD /api/path [JSON] an egret API call as the admin; VAR keeps its .id
#   -- setup: VAR = ENSURE-WORKSPACE slug     a workspace by slug (created once), its id in VAR
#   -- refused NNN: METHOD /api/path [JSON]   an API call that must answer exactly status NNN
#   -- setup-sql: SQL                         SQL the platform admin runs on trino-ranger; must succeed
#   -- expect: USER | SQL | EXPECTED          a positive control on both engines: EXPECTED is the
#                                             exact JSON rows, `ok` or `denied`; SQL may be KILL
#   SQL                                       a compared statement, one per line
#   @session-user U: SQL                      run as the scenario user impersonating U
# `@NAME@` in a setup or expect line is replaced by the value of NAME: CAT, WH, WS, ENGINE, ADMIN,
# UID_<user>, or a VAR an earlier setup line set. Nothing in a scenario file is passed to eval.
#
# After the scenario's own statements, every user also runs the compared statements every
# scenario shares (COMMON below): the listings, information_schema, a `$files` table,
# SET SESSION query_max_run_time, kill_query against another user's query, and impersonation
# by the delegator (egret_core::engine_principals::DELEGATOR_PRINCIPAL).
#
# Since the port-8080 hardening both engines refuse authentication over plain HTTP, so every job
# authenticates as the delegator (trino-auth.sh, trino-batch.sh). A `direct` job runs as the
# delegator impersonating the user, as egret's own query path does; an `impersonate` job is the
# same request, kept for its statements; an `@session-user` job still names the scenario user as
# the original user, so Trino still asks whether that user may impersonate U.
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
KILL_OTHER=ann            # whose query the admin's kill targets (never the caller's own)
SYNC_TIMEOUT="${LAYER2_SYNC_TIMEOUT:-120}" # seconds to wait for trino-ranger's plugin to catch up
OUT="${LAYER2_OUT:-$WORK/layer2}"
REPORT="${LAYER2_REPORT:-$REPO/layer2-report.json}"
ONLY="${LAYER2_ONLY:-}"     # a regex, e.g. 'G06|G11': run only the scenarios whose file name it matches at the start
SETUP_ONLY="${LAYER2_SETUP_ONLY:-}" # non-empty: apply each setup and run nothing; the last scenario's state stays
SCENARIO="(start)"          # named by every die

log() { printf '→ %s\n' "$*" >&2; }
die() { printf 'layer2.sh: [%s] %s\n' "$SCENARIO" "$*" >&2; exit 1; }
jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }

RANGER_CONTAINER=$("${COMPOSE[@]}" ps -q trino-ranger)
[ -n "$RANGER_CONTAINER" ] || die "trino-ranger is not running: run up.sh first"
docker exec "$OPA_CONTAINER" grep -q '^access-control.name=opa' /etc/trino/access-control.properties \
  || die "$OPA_CONTAINER is not on the OPA plugin: run up.sh first"

# --- authentication, and the identity check (port-8080 hardening, A5) ----------------------
# Both engines refuse authentication over plain HTTP; the harness authenticates as the delegator
# (trino-auth.sh). First, on both engines, trino-opa now on the OPA plugin: a plain-HTTP
# `X-Trino-User` is refused (403), the same with `X-Forwarded-Proto: https` and no credential is
# refused (401), and the delegator still impersonates (identity-check.sh). up.sh ran the same
# check while trino-opa was on the Ranger plugin.
. "$HERE/trino-auth.sh"
TRINO_DELEGATOR_PASSWORD=$(delegator_password trino-opa) || die "reading trino-opa's delegator password"
# Mask it in CI logs, so a future `set -x` cannot print it.
if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::add-mask::$TRINO_DELEGATOR_PASSWORD"; fi
export TRINO_DELEGATOR_PASSWORD
identity_check "$OPA_CONTAINER" trino-opa/opa || die "identity check failed on trino-opa (OPA plugin)"
identity_check "$RANGER_CONTAINER" trino-ranger || die "identity check failed on trino-ranger"

# egret's OPA surface is on its own port (8082); 8080 answers 404 for /opa/.
opa_port_check "$OPA_CONTAINER" || die "OPA port check failed on trino-opa"

# --- admin session ------------------------------------------------------------------------
JAR="$WORK/admin.jar"
curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
  -d '{"email":"admin@egret.local","password":"Admin12345!"}' -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
api_code() { curl -sS -o "$WORK/last.json" -w '%{http_code}' -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
# list PATH: the body of a list route, or die (a failed list must never read as "nothing to delete").
list() { local body; body=$(api "$BASE$1") || die "GET $1 failed: $body"; printf '%s' "$body"; }
# ids(expr): one `expr` (over d, an item; None skips) per line of a list route's answer.
ids() { python3 -c "
import sys, json
d = json.load(sys.stdin)
if isinstance(d, list):
    rows = d
else:
    keys = [k for k in ('items', 'grants', 'rules', 'data', 'groups', 'roles', 'tags', 'workspaces') if k in d]
    if not keys:
        sys.exit('ids: an answer of unknown shape: ' + ', '.join(sorted(d)))
    rows = d[keys[0]]
for d in rows:
    v = $1
    if v is not None:
        print(v)"; }
# one PATH EXPR: the single value a list route answers for EXPR, or die.
one() { local v; v=$(list "$1" | ids "$2") || die "reading $1 failed"; [ -n "$v" ] || die "$1: nothing matches $2"; printf '%s' "$v" | head -1; }

# Values a setup line can name as @NAME@ live in V_NAME (bash 3.2 has no associative arrays).
V_CAT=$(one /api/control-plane/catalogs "d['id'] if d['name']=='$CATALOG' else None")
V_WH=$(one /api/control-plane/warehouses "d['id'] if d['name']=='$CATALOG' else None")
V_WS=$(list /api/me/workspaces | jget "d[0]['id']")
V_ENGINE=$(one /api/control-plane/engines "d['id'] if d['name']=='trino-opa' else None")
V_ADMIN=$(one /api/control-plane/users "d['username'] if d['email']=='admin@egret.local' else None")
ADMIN=${V_ADMIN}
for u in ann bob carol dan; do
  if [ -z "$(list /api/control-plane/users | ids "d['id'] if d['email']=='$u@egret.local' else None")" ]; then
    api -X POST "$BASE/api/control-plane/users" \
      -d "{\"email\":\"$u@egret.local\",\"name\":\"$u\",\"password\":\"E2e-password-1!\"}" >/dev/null \
      || die "creating user $u failed"
  fi
  uid=$(one /api/control-plane/users "d['id'] if d['email']=='$u@egret.local' else None")
  printf -v "V_UID_$u" '%s' "$uid"
done

# --- the engines --------------------------------------------------------------------------
# Every compared statement goes through trino-batch.sh INSIDE each engine's container (Trino's
# client protocol over curl; see that file for why not the CLI). One docker exec per side.
batch() { # $1 = container, stdin = jobs; stdout = protocol responses
  docker exec -i -e TRINO_DELEGATOR_PASSWORD "$1" sh -c "$(cat "$HERE/trino-batch.sh")" trino-batch "$ADMIN" "$KILL_OTHER"
}
# admin_sql SQL: on trino-ranger as the platform admin; dies, naming the scenario, unless it succeeds.
admin_sql() {
  local out rc=0
  out=$(trino_cli "$RANGER_CONTAINER" "$ADMIN" "$1" 2>&1) || rc=$?
  out=$(grep -v '^Picked up JAVA_TOOL_OPTIONS' <<<"$out" || true)
  if [ "$rc" != 0 ] || grep -q '^Query [^ ]* failed' <<<"$out"; then
    die "admin SQL failed (exit $rc): $1 -> $(head -3 <<<"$out")"
  fi
  printf '%s\n' "$out"
}
# admin_probe SQL: the same, but a failure is an answer (empty output), not an error.
admin_probe() {
  trino_cli "$RANGER_CONTAINER" "$ADMIN" "$1" 2>/dev/null \
    | grep -v '^Picked up JAVA_TOOL_OPTIONS' || true
}

# --- data: lake.s1.{t1,t2} come from up.sh; s2.t3 and the empty scratch table s2.w here -----
if [ "$(admin_probe "SELECT count(*) FROM $CATALOG.s2.t3" | tail -1)" != 100 ]; then
  if [ "$(admin_probe "SELECT count(*) FROM $CATALOG.s2.t3_v2" | tail -1)" = 100 ]; then
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
  for id in $(list /api/control-plane/perm/engine-grants | ids "d['id']"); do
    api -X DELETE "$BASE/api/control-plane/perm/engine-grants/$id" -o /dev/null || die "deleting engine grant $id"
  done
  for id in $(list /api/control-plane/perm/rules | ids "d['id']"); do
    api -X DELETE "$BASE/api/control-plane/perm/rules/$id" -o /dev/null || die "deleting rule $id"
  done
  # Explicit grants only: a `workspace_assignment` grant belongs to the catalog's workspace
  # assignment, which is put back to the default workspace alone.
  for id in $(list /api/control-plane/perm/grants | ids "d['id'] if d.get('origin', {}).get('type') == 'explicit' else None"); do
    api -X DELETE "$BASE/api/control-plane/perm/grants/$id" -o /dev/null || die "deleting grant $id"
  done
  api -X PUT "$BASE/api/control-plane/catalogs/$V_CAT/workspaces" -d "{\"workspaceIds\":[\"$V_WS\"]}" -o /dev/null \
    || die "resetting the catalog's workspace assignment"
  for id in $(list /api/control-plane/tags | ids "d['id'] if d['key'].startswith('l2-') else None"); do
    api -X DELETE "$BASE/api/control-plane/tags/$id" -o /dev/null || die "deleting tag $id"
  done
  for id in $(list /api/control-plane/groups | ids "d['id'] if d['name'].startswith('l2_') else None"); do
    api -X DELETE "$BASE/api/control-plane/groups/$id" -o /dev/null || die "deleting group $id"
  done
  for id in $(list /api/control-plane/access/roles | ids "d['id'] if d['name'].startswith('l2_') else None"); do
    api -X DELETE "$BASE/api/control-plane/access/roles/$id" -o /dev/null || die "deleting role $id"
  done
  if [ "$(admin_probe "SELECT count(*) FROM $CATALOG.s2.t3_v2" | tail -1)" = 100 ]; then
    admin_sql "ALTER TABLE $CATALOG.s2.t3_v2 RENAME TO $CATALOG.s2.t3" >/dev/null
  fi
  # Nothing may survive: a leaked grant or rule would change the next scenario silently.
  [ -z "$(list /api/control-plane/perm/rules | ids "d['id']")" ] || die "rules survived the reset"
  [ -z "$(list /api/control-plane/perm/engine-grants | ids "d['id']")" ] || die "engine grants survived the reset"
  [ -z "$(list /api/control-plane/perm/grants | ids "d['id'] if d.get('origin', {}).get('type') == 'explicit' else None")" ] \
    || die "grants survived the reset"
  # The baseline every scenario shares: listings read lake.information_schema, which both
  # plugins authorize like any table (Task 20, D4).
  api -X POST "$BASE/api/control-plane/perm/grants" -d "{\"effect\":\"allow\",\"actions\":[\"read\"],
    \"scope\":{\"catalog_id\":\"${V_CAT}\",\"schema\":\"information_schema\"},
    \"principals\":{\"include\":[{\"kind\":\"group\",\"ref\":\"public\"}],\"exclude\":[]}}" -o /dev/null \
    || die "the baseline information_schema grant"
}

subst() { # replace every @NAME@ with V_NAME
  local s=$1 name
  while [[ $s =~ @([A-Za-z_][A-Za-z0-9_]*)@ ]]; do
    name=${BASH_REMATCH[1]}
    local ref="V_$name"
    [ -n "${!ref+x}" ] || die "a line names @$name@, which nothing set"
    s=${s//@$name@/${!ref}}
  done
  printf '%s' "$s"
}

setup_line() { # $1 = setup | refused:NNN, $2 = the rest of the line
  local kind=$1 line var="" method path body code
  line=$(subst "$2")
  if [[ $line =~ ^([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=[[:space:]]*(.*)$ ]]; then
    var=${BASH_REMATCH[1]}; line=${BASH_REMATCH[2]}
  fi
  if [[ $line =~ ^ENSURE-WORKSPACE[[:space:]]+([a-z0-9-]+)$ ]]; then
    local slug=${BASH_REMATCH[1]} id
    id=$(list /api/admin/workspaces | ids "d['id'] if d.get('slug')=='$slug' else None" | head -1)
    if [ -z "$id" ]; then
      id=$(api -X POST "$BASE/api/admin/workspaces" -d "{\"slug\":\"$slug\",\"name\":\"$slug\"}" | jget "d['id']") \
        || die "creating workspace $slug"
    fi
    [ -n "$id" ] || die "workspace $slug has no id"
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
  if [[ $kind == refused:* ]]; then
    [ "$code" = "${kind#refused:}" ] \
      || die "$method $path answered $code; the scenario expects ${kind#refused:}: $(cat "$WORK/last.json")"
    log "   refused as expected ($code): $method $path"
    return
  fi
  [[ $code =~ ^2 ]] || die "$method $path answered $code: $(cat "$WORK/last.json")"
  if [ -n "$var" ]; then
    local id
    id=$(jget "d['id']" <"$WORK/last.json") || die "$method $path: no id in the answer"
    [ -n "$id" ] || die "$method $path: an empty id"
    printf -v "V_$var" '%s' "$id"
  fi
}

# --- wait until both engines answer from the current state ----------------------------------
# trino-ranger: the Ranger plugin polls policies and tags every 30 s; it is caught up when the
# policyVersion and tagVersion in its cache equal what egret serves now (read with the plugin's
# own credential from the files up.sh installed; never printed). Both versions are bumped in the
# write's own transaction. trino-opa: egret's refresher rebuilds the snapshot every second
# (opa/cache.rs); the 5 s after the Ranger side is caught up covers it. Its own version is not
# exposed, so the positive controls are what prove trino-opa serves the scenario.
XML="$WORK/trino-ranger/etc/ranger-trino-security.xml"
PLUGIN_USER=$(sed -n 's#.*client.username</name><value>\([^<]*\)<.*#\1#p' "$XML")
PLUGIN_PASS=$(sed -n 's#.*client.password</name><value>\([^<]*\)<.*#\1#p' "$XML")
SERVICE=$(sed -n 's#.*service.name</name><value>\([^<]*\)<.*#\1#p' "$XML")
[ -n "$PLUGIN_USER" ] && [ -n "$PLUGIN_PASS" ] && [ -n "$SERVICE" ] || die "cannot read the plugin files in $XML"
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
  sp=$(served_version policies) || die "reading the served policyVersion"
  st=$(served_version tags) || die "reading the served tagVersion"
  [ -n "$sp" ] && [ -n "$st" ] || die "egret served no policyVersion/tagVersion ('$sp'/'$st')"
  until [ "$(cached_version policies)" = "$sp" ] && [ "$(cached_version tags)" = "$st" ]; do
    [ $((SECONDS - start)) -lt "$SYNC_TIMEOUT" ] || die "trino-ranger did not reach policyVersion $sp / tagVersion $st"
    sleep 2
  done
  sleep 5
  log "   synced: policyVersion $sp, tagVersion $st ($((SECONDS - start)) s)"
}

# --- jobs ---------------------------------------------------------------------------------------
# write_jobs DIR: from `users` and `statements` (sid TAB session-user TAB sql) writes opa.jobs and
# ranger.jobs (the compared statements), ref.jobs (each plain SELECT as the admin, labelled per
# user, for the policy-free reference) and meta.tsv (user/via TAB sid TAB the statement shown).
write_jobs() {
  local dir=$1 user sid su sql via u o shown eng
  : >"$dir/opa.jobs"; : >"$dir/ranger.jobs"; : >"$dir/ref.jobs"; : >"$dir/meta.tsv"
  while read -r user; do
    while IFS=$'\t' read -r sid su sql; do
      [ "$su" = - ] && su=""
      via=direct; u=$user; o=-; shown=$sql
      if [ "$sql" = KILL ]; then
        shown="CALL system.runtime.kill_query(query_id => '<another user''s running query>', message => 'layer2')"
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
        printf 'ref/%s/%s\t%s\t%s\t-\t%s\n' "$user" "$via" "$sid" "$ADMIN" "$sql" >>"$dir/ref.jobs"
      fi
    done <"$dir/statements"
  done <"$dir/users"
}

# add_expect DIR FILE SID USER SQL EXPECTED: a positive control on both engines.
add_expect() {
  local dir=$1 file=$2 sid=$3 user=$4 sql=$5 expected=$6 eng
  for eng in opa ranger; do
    printf '%s/%s/expect\t%s\t%s\t-\t%s\n' "$eng" "$user" "$sid" "$user" "$sql" >>"$dir/$file.$eng.jobs"
    printf '%s/%s/expect\t%s\t%s\t%s\n' "$eng" "$user" "$sid" "$user" "$expected" >>"$dir/$file.expect.tsv"
  done
}

# The policy-free controls of every scenario: what the seed wrote, read by the admin.
BASE_CONTROLS=(
  "SELECT count(*) FROM $CATALOG.s1.t1|[[1000]]"
  "SELECT count(*) FROM $CATALOG.s1.t2|[[1000]]"
  "SELECT count(*) FROM $CATALOG.s2.t3|[[100]]"
  "SELECT count(*) FROM $CATALOG.s2.w|[[0]]"
  "SELECT email, ssn, region FROM $CATALOG.s1.t1 WHERE id = 2|[[\"user2@example.com\", \"000000002\", \"us\"]]"
)

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

run_pair() { # $1 = dir, $2 = job-file prefix ("" for the compared statements)
  local dir=$1 pre=$2 p1 p2
  batch "$OPA_CONTAINER" <"$dir/${pre}opa.jobs" >"$dir/${pre}opa.out" & p1=$!
  batch "$RANGER_CONTAINER" <"$dir/${pre}ranger.jobs" >"$dir/${pre}ranger.out" & p2=$!
  wait "$p1" || die "the trino-opa batch failed"
  wait "$p2" || die "the trino-ranger batch failed"
}
check_expects() { # $1 = dir, $2 = file prefix
  local eng
  for eng in opa ranger; do
    python3 -I "$HERE/collect.py" expect "$1/$2.$eng.out" "$1/$2.expect.tsv" "$SCENARIO" "$eng" \
      || die "positive controls failed (see above)"
  done
}

rm -rf "$OUT"; mkdir -p "$OUT"
RESULTS="$OUT/results.jsonl"; : >"$RESULTS"
for f in "$HERE"/scenarios.sql.d/*.sql; do
  name=$(basename "$f" .sql); sc=${name%%_*}
  [ -n "$ONLY" ] && ! [[ $name =~ ^($ONLY) ]] && continue
  SCENARIO=$name
  log "scenario $name"
  dir="$OUT/$sc"; mkdir -p "$dir"
  users=$(sed -n 's/^-- users: *//p' "$f")
  [ -n "$users" ] || die "$f has no '-- users:' line"
  printf "%s\n" "$(subst "$users")" | tr "," "\n" >"$dir/users"
  : >"$dir/statements"; n=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      "@session-user "*)
        n=$((n+1)); su=${line#@session-user }; su=${su%%:*}; sql=${line#*: }
        printf 'q%02d\t%s\t%s\n' "$n" "$su" "$sql" >>"$dir/statements" ;;
      "--"*|"") ;;
      *) n=$((n+1)); printf 'q%02d\t-\t%s\n' "$n" "$line" >>"$dir/statements" ;;
    esac
  done <"$f"
  n=0
  for s in "${COMMON[@]}"; do n=$((n+1)); printf 'c%02d\t-\t%s\n' "$n" "$s" >>"$dir/statements"; done
  n=0
  for s in "${IMPERSONATED[@]}"; do n=$((n+1)); printf 'd%02d\tDELEGATOR\t%s\n' "$n" "$s" >>"$dir/statements"; done
  write_jobs "$dir"

  # 1-2. reset, then the policy-free controls and the reference.
  reset_policies
  : >"$dir/pre.opa.jobs"; : >"$dir/pre.ranger.jobs"; : >"$dir/pre.expect.tsv"; n=0
  for c in "${BASE_CONTROLS[@]}"; do
    n=$((n+1)); add_expect "$dir" pre "k$(printf %02d $n)" "$ADMIN" "${c%%|*}" "${c#*|}"
  done
  cat "$dir/ref.jobs" >>"$dir/pre.ranger.jobs"
  if [ -z "$SETUP_ONLY" ]; then
    wait_synced
    run_pair "$dir" pre.
    check_expects "$dir" pre
  fi

  # 3. setup.
  : >"$dir/post.opa.jobs"; : >"$dir/post.ranger.jobs"; : >"$dir/post.expect.tsv"; n=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      "-- setup-sql: "*) admin_sql "$(subst "${line#-- setup-sql: }")" >/dev/null ;;
      "-- setup: "*) setup_line setup "${line#-- setup: }" ;;
      "-- refused "[0-9][0-9][0-9]": "*)
        code=${line#-- refused }; code=${code%%:*}
        setup_line "refused:$code" "${line#-- refused [0-9][0-9][0-9]: }" ;;
      "-- refused"*) die "a refused line must pin its status: '-- refused NNN: ...': $line" ;;
      "-- expect: "*)
        spec=$(subst "${line#-- expect: }")
        e_user=${spec%% | *}; rest=${spec#* | }; e_sql=${rest%% | *}; e_want=${rest#* | }
        [ -n "$e_user" ] && [ -n "$e_sql" ] && [ -n "$e_want" ] && [ "$e_sql" != "$rest" ] \
          || die "malformed expect line: $line"
        n=$((n+1)); add_expect "$dir" post "x$(printf %02d $n)" "$e_user" "$e_sql" "$e_want" ;;
    esac
  done <"$f"
  [ -n "$SETUP_ONLY" ] && continue
  [ -s "$dir/post.expect.tsv" ] || die "$f has no '-- expect:' positive control"

  # 4. the scenario's positive controls FIRST, then the compared statements, in one batch per
  # side: the controls fail on the pre-setup state, and a snapshot only moves forward, so
  # controls that hold at the start of the batch prove both engines serve the scenario for every
  # compared statement after them (a freshness barrier).
  for eng in opa ranger; do
    cat "$dir/post.$eng.jobs" "$dir/$eng.jobs" >"$dir/$eng.jobs.ordered"
    mv "$dir/$eng.jobs.ordered" "$dir/$eng.jobs"
  done
  wait_synced
  run_pair "$dir" ""
  for eng in opa ranger; do cp "$dir/$eng.out" "$dir/post.$eng.out"; done
  check_expects "$dir" post

  for side in opa ranger pre.ranger; do
    python3 -I "$HERE/collect.py" records "$dir/$side.out" "$dir/meta.tsv" "$sc" >>"$RESULTS" \
      || die "collecting $side"
  done
done
[ -n "$SETUP_ONLY" ] && { log "setup-only: every setup applied; the last scenario's state is left in place"; exit 0; }
SCENARIO="(end)"
reset_policies

cp "$RESULTS" "$(dirname "$REPORT")/layer2-results.jsonl" 2>/dev/null || true
python3 -I "$HERE/compare.py" "$RESULTS" --report "$REPORT"
