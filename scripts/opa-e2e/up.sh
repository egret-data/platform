#!/usr/bin/env bash
# E2E ONLY. Bring up compose project `egret-opa-e2e` (deploy/opa-e2e) and wire it:
# warehouse/catalog `lake`, Trino profile `opa_e2e`, the PROVISIONED engine `trino-opa`
# (egret's Docker provisioner), the EXTERNAL engine `trino-ranger` on the Ranger plugin,
# seed data, then switch the profile to OPA. Idempotent: every create checks first.
#
# Never acts on another compose project: every compose call names the file AND -p.
set -euo pipefail
# NB: never `cmd | grep -q` under pipefail: grep exits at the first match, the writer
# dies of SIGPIPE and the pipeline reads as false. `grep … >/dev/null` reads it all.

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$REPO/deploy/opa-e2e/docker-compose.yml" -p egret-opa-e2e)
export WORK="${WORK:-${TMPDIR:-/tmp}/egret-opa-e2e}"
export EGRET_OPA_E2E_PORT="${EGRET_OPA_E2E_PORT:-28081}"
BASE="http://127.0.0.1:${EGRET_OPA_E2E_PORT}"
TRINO_IMAGE="trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
IMAGE_VERSION="483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
PROFILE=opa_e2e
TAG_PROFILE=opa_e2e_tag
CATALOG=lake
OPA_ENGINE=trino-opa
OPA_CONTAINER=egret-engine-egret-engines-${OPA_ENGINE}-coordinator
ADMIN_EMAIL=admin@egret.local
ADMIN_PASS='Admin12345!'

log() { printf '→ %s\n' "$*" >&2; }
die() { printf 'up.sh: %s\n' "$*" >&2; exit 1; }
. "$REPO/scripts/opa-e2e/trino-auth.sh"
# jq-free JSON: `jget <python expr over d>` reads stdin.
jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }

# --- 0. trino-ranger's /etc/trino, from the image's own defaults --------------------------
ETC="$WORK/trino-ranger/etc"
mkdir -p "$ETC/catalog"
if [ ! -f "$ETC/config.properties" ]; then
  log "seeding $ETC from the image's /etc/trino"
  docker run --rm --entrypoint tar "$TRINO_IMAGE" -C /etc/trino -cf - . | tar -C "$ETC" -xf -
fi
chmod -R a+rX "$WORK"

# --- 1. the stack -----------------------------------------------------------------------
if [ -n "$(docker ps -a --filter "name=^${OPA_CONTAINER}$" --filter "label=com.docker.compose.project" -q)" ]; then
  die "$OPA_CONTAINER exists but carries a compose label: refusing to collide with it"
fi
log "compose up (builds egret-opa-e2e-backend:dev only when it is missing; rebuild with: compose build egret)"
"${COMPOSE[@]}" up -d --wait postgres rustfs docker-proxy egret
for _ in $(seq 1 90); do
  curl -fsS "$BASE/healthz" >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS "$BASE/healthz" >/dev/null || die "egret /healthz did not answer on $BASE"

# --- 2. admin session -------------------------------------------------------------------
JAR="$WORK/admin.jar"
curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
  -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }
# api_any: print the body whatever the status; the status goes to fd 3 callers read.
api_code() { curl -sS -o "$WORK/last.json" -w '%{http_code}' -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }

WS_ID=$(api "$BASE/api/me/workspaces" | jget "d[0]['id']")
[ -n "$WS_ID" ] || die "no workspace (egret init should have created one)"

# --- 3. storage + warehouse `lake` (its registry catalog is `lake`) ---------------------
CAT_ID=$(api "$BASE/api/control-plane/catalogs" | jget "next((c['id'] for c in d if c['name']=='$CATALOG'), None)")
if [ -z "$CAT_ID" ]; then
  log "storage connection + warehouse $CATALOG"
  SC_ID=$(api "$BASE/api/control-plane/storage" | jget "next((c['id'] for c in d if c['name']=='opa-e2e-rustfs'), None)")
  if [ -z "$SC_ID" ]; then
    SC_ID=$(api -X POST "$BASE/api/control-plane/storage" -d '{
      "name":"opa-e2e-rustfs","provider":"s3","endpoint":"http://rustfs:9000",
      "region":"us-east-1","bucket":"warehouse","access_key":"egret",
      "new_secret":{"name":"opa-e2e-rustfs-secret","value":"egretsecret"},"path_style":true}' | jget "d['id']")
  fi
  api -X POST "$BASE/api/control-plane/warehouses" \
    -d "{\"name\":\"$CATALOG\",\"workspaceId\":\"$WS_ID\",\"storageConnectionId\":\"$SC_ID\"}" >/dev/null
  CAT_ID=$(api "$BASE/api/control-plane/catalogs" | jget "next((c['id'] for c in d if c['name']=='$CATALOG'), None)")
fi
[ -n "$CAT_ID" ] || die "catalog $CATALOG was not created with its warehouse"
log "catalog $CATALOG = $CAT_ID"

# --- 4. profile, with a tag service ---------------------------------------------------------
# Without a `tagService` the compile skips every tag rule and clearance on this profile (golden
# G10), on Ranger and OPA alike, and Layer 2 (Task 21) would measure no classification at all.
# The tag service can only be named at create time: PUT .../tag-service refuses a profile an
# engine binds or the permission model compiles (409).
# The profile list, as a JSON list whether the route answers a list or {services|items: [...]}.
services() { api "$BASE/api/control-plane/policies/services" | python3 -c \
  "import sys,json; d=json.load(sys.stdin); print(json.dumps(d if isinstance(d,list) else d.get('services', d.get('items', []))))"; }
if [ -z "$(services | jget "next((p['id'] for p in d if p['name']=='$TAG_PROFILE'), None)")" ]; then
  log "tag profile $TAG_PROFILE"
  api -X POST "$BASE/api/control-plane/policies/services" -d "{\"name\":\"$TAG_PROFILE\",\"type\":\"tag\"}" >/dev/null
fi
PROFILE_ID=$(services | jget "next((p['id'] for p in d if p['name']=='$PROFILE'), None)")
if [ -z "$PROFILE_ID" ]; then
  log "profile $PROFILE"
  PROFILE_ID=$(api -X POST "$BASE/api/control-plane/policies/services" \
    -d "{\"name\":\"$PROFILE\",\"type\":\"trino\",\"config\":{\"tagService\":\"$TAG_PROFILE\"}}" | jget "d['id']")
fi
[ "$(services | jget "next((p.get('tag_service') for p in d if p['name']=='$PROFILE'), None)")" = "$TAG_PROFILE" ] \
  || die "profile $PROFILE has no tag service $TAG_PROFILE (made by an older up.sh?): run down.sh, then up.sh"
log "profile $PROFILE = $PROFILE_ID"

engine_id() { api "$BASE/api/control-plane/engines" | jget "next((e['id'] for e in (d if isinstance(d,list) else d.get('items',[])) if e['name']=='$1'), None)"; }

# --- 5. trino-opa: PROVISIONED through egret's Docker provisioner --------------------------
OPA_ID=$(engine_id "$OPA_ENGINE")
if [ -z "$OPA_ID" ]; then
  if docker ps -a --format '{{.Names}}' | grep -x "$OPA_CONTAINER" >/dev/null; then
    die "a container named $OPA_CONTAINER already exists and egret has no engine for it"
  fi
  log "provisioning $OPA_ENGINE"
  OPA_ID=$(api -X POST "$BASE/api/control-plane/engines" -d "{
    \"name\":\"$OPA_ENGINE\",\"imageVersion\":\"$IMAGE_VERSION\",
    \"coordinatorReplicas\":1,\"workerReplicas\":0,\"cpuMin\":\"500m\",\"cpuMax\":\"2\",\"memory\":\"2Gi\",
    \"trinoConfig\":{\"coordinator\":{},\"delegationMode\":\"impersonate\",\"auth\":{\"password\":{}}},
    \"authzServiceId\":$PROFILE_ID,
    \"catalogs\":[{\"catalogId\":\"$CAT_ID\",\"primary\":true}]}" | jget "d['id']")
fi
log "$OPA_ENGINE = $OPA_ID"
wait_opa_ready() {
  for _ in $(seq 1 90); do
    if docker exec "$OPA_CONTAINER" curl -fsS http://127.0.0.1:8080/v1/info 2>/dev/null | grep '"starting":false' >/dev/null; then
      return 0
    fi
    sleep 3
  done
  die "$OPA_CONTAINER did not become ready"
}
wait_opa_ready
TRINO_DELEGATOR_PASSWORD=$(delegator_password "$OPA_ENGINE") || die "reading $OPA_ENGINE's delegator password"
export TRINO_DELEGATOR_PASSWORD

# --- 6/7. trino-ranger: the Ranger plugin on THIS profile's Ranger download ----------------
# The brief registered trino-ranger as an external engine bound to `opa_e2e`. egret refuses
# that: one profile per engine (engines.rs, I2). An external engine on a second profile is
# no oracle either: its profile gets no infrastructure rows (`qualifies_for_infra`), so
# Trino 483's per-query `execute` on queryid and self-`impersonate` checks deny every query,
# and a compiled profile refuses operator policies. So trino-ranger runs the Ranger plugin
# with trino-opa's OWN plugin files (service `opa_e2e`, trino-opa's engine credential),
# copied while trino-opa is still on Ranger: it evaluates exactly the artifact S8 keeps as
# the oracle, the Ranger download of the profile trino-opa answers through OPA.
if [ -f "$ETC/ranger-trino-security.xml" ]; then
  log "keeping the installed Ranger plugin files"
else
  docker exec "$OPA_CONTAINER" grep -x 'access-control.name=ranger' /etc/trino/access-control.properties >/dev/null \
    || die "$OPA_CONTAINER is no longer on Ranger and $ETC has no plugin files: run down.sh, then up.sh"
  for f in access-control.properties ranger-trino-security.xml; do
    docker exec "$OPA_CONTAINER" cat "/etc/trino/$f" > "$ETC/$f"
    chmod 0644 "$ETC/$f"
  done
  # The plugin's cache dir is trino-opa's path; point it at one trino-ranger has.
  sed -i.bak -E 's#(<name>ranger.plugin.trino.policy.cache.dir</name><value>)[^<]*#\1/tmp/ranger-cache#' \
    "$ETC/ranger-trino-security.xml" && rm -f "$ETC/ranger-trino-security.xml.bak"
  log "installed trino-opa's Ranger plugin files (service $PROFILE) for trino-ranger"
fi

# --- 7b. trino-ranger authenticates exactly as egret renders trino-opa -------------------
# Port-8080 hardening (A5): trino-opa refuses authentication over plain HTTP. The oracle gets the
# same authentication, so the harness reaches both engines the same way (the delegator over
# Basic, trino-auth.sh) and the identity check asserts the same refusals on both. Copied from
# trino-opa on every run: the authentication lines of its config.properties (the internal shared
# secret replaced by one of trino-ranger's own), its password file (bcrypt hashes of
# egret_delegator and egret-monitor) and its TLS keystore (HTTPS 8443, for the Trino CLI). E2E
# only: the files are world-readable under $WORK so trino-ranger's `trino` user can read them.
mkdir -p "$ETC/auth/password" "$ETC/tls"
docker exec "$OPA_CONTAINER" cat /etc/trino/config.properties > "$WORK/opa-config.properties"
docker exec "$OPA_CONTAINER" cat /etc/trino/password-authenticator.properties > "$ETC/password-authenticator.properties"
docker exec "$OPA_CONTAINER" cat /etc/trino/auth/password/password.db > "$ETC/auth/password/password.db"
docker exec "$OPA_CONTAINER" cat /etc/trino/tls/keystore.pem > "$ETC/tls/keystore.pem"
grep -x 'http-server.authentication.type=PASSWORD' "$WORK/opa-config.properties" >/dev/null \
  || die "$OPA_CONTAINER does not render PASSWORD authentication"
python3 -I "$REPO/scripts/opa-e2e/copy_auth.py" "$ETC/config.properties" "$WORK/opa-config.properties"
rm -f "$WORK/opa-config.properties"
chmod -R a+rX "$ETC"

# --- 8. trino-ranger's catalog file: the provisioned engine's own, ENV refs resolved -------
docker exec "$OPA_CONTAINER" cat /etc/trino/catalog/$CATALOG.properties > "$WORK/opa-catalog.properties"
docker exec "$OPA_CONTAINER" env > "$WORK/opa-env.txt"
python3 - "$WORK/opa-catalog.properties" "$WORK/opa-env.txt" \
  "$REPO/deploy/opa-e2e/trino-ranger/catalog/lake.properties.tmpl" "$ETC/catalog/$CATALOG.properties" <<'PY'
import re, sys
props, envf, tmpl, out = sys.argv[1:]
env = dict(l.rstrip("\n").split("=", 1) for l in open(envf) if "=" in l)
body = re.sub(r"\$\{ENV:([A-Za-z0-9_]+)\}", lambda m: env[m.group(1)], open(props).read())
open(out, "w").write(open(tmpl).read().replace("@PROPERTIES@", body.rstrip("\n")))
PY
chmod 0644 "$ETC/catalog/$CATALOG.properties"
rm -f "$WORK/opa-env.txt"

# --- 9. (re)start trino-ranger with its plugin + catalog -----------------------------------
log "restarting trino-ranger"
"${COMPOSE[@]}" up -d --force-recreate --wait trino-ranger

RANGER_CONTAINER=$("${COMPOSE[@]}" ps -q trino-ranger)
trino_ranger() { trino_cli "$RANGER_CONTAINER" "$1" "$2"; }

# --- users ---------------------------------------------------------------------------------
for u in ann bob; do
  if ! api "$BASE/api/control-plane/users" | grep "\"$u@egret.local\"" >/dev/null; then
    api -X POST "$BASE/api/control-plane/users" \
      -d "{\"email\":\"$u@egret.local\",\"name\":\"$u\",\"password\":\"E2e-password-1!\"}" >/dev/null
  fi
done
ADMIN_USER=$(api "$BASE/api/control-plane/users" | jget "next((u['username'] for u in (d if isinstance(d,list) else d.get('items',[])) if u['email']=='$ADMIN_EMAIL'), None)")
log "platform admin's Trino user = $ADMIN_USER"

# --- 10. seed lake.s1.{t1,t2} through trino-ranger as the platform admin ------------
for _ in $(seq 1 30); do
  trino_ranger "$ADMIN_USER" "SHOW SCHEMAS FROM $CATALOG" >/dev/null 2>&1 && break
  sleep 3
done
if ! trino_ranger "$ADMIN_USER" "SELECT count(*) FROM $CATALOG.s1.t1" 2>/dev/null | grep -x 1000 >/dev/null; then
  log "seeding $CATALOG.s1.t1 / t2"
  trino_ranger "$ADMIN_USER" "CREATE SCHEMA IF NOT EXISTS $CATALOG.s1"
  trino_ranger "$ADMIN_USER" "DROP TABLE IF EXISTS $CATALOG.s1.t1"
  trino_ranger "$ADMIN_USER" "DROP TABLE IF EXISTS $CATALOG.s1.t2"
  trino_ranger "$ADMIN_USER" "CREATE TABLE $CATALOG.s1.t1 AS
    SELECT n AS id,
           'user' || CAST(n AS varchar) || '@example.com' AS email,
           lpad(CAST(n AS varchar), 9, '0') AS ssn,
           CASE WHEN n % 2 = 0 THEN 'us' ELSE 'eu' END AS region,
           'name' || CAST(n AS varchar) AS name
    FROM UNNEST(sequence(1, 1000)) AS t(n)"
  trino_ranger "$ADMIN_USER" "CREATE TABLE $CATALOG.s1.t2 AS
    SELECT n AS id, 'v' || CAST(n AS varchar) AS v FROM UNNEST(sequence(1, 1000)) AS t(n)"
fi
trino_ranger "$ADMIN_USER" "SELECT count(*) FROM $CATALOG.s1.t1" | grep -x 1000 >/dev/null || die "seed failed"

# --- 11. one grant: bob reads t2 (the profile is generated from here on) -------------------
if ! api "$BASE/api/control-plane/perm/grants" | grep '"ref":"bob"' >/dev/null; then
  api -X POST "$BASE/api/control-plane/perm/grants" -d "{
    \"effect\":\"allow\",\"actions\":[\"read\"],
    \"scope\":{\"catalog_id\":\"$CAT_ID\",\"schema\":\"s1\",\"table\":\"t2\"},
    \"principals\":{\"include\":[{\"kind\":\"user\",\"ref\":\"bob\"}],\"exclude\":[]}}" >/dev/null
fi

# --- 11b. identity: plain-HTTP spoofing is refused, trino-opa still on the Ranger plugin -----
log "identity check: trino-opa (Ranger plugin) and trino-ranger"
identity_check "$OPA_CONTAINER" trino-opa/ranger || die "identity check failed on trino-opa (Ranger plugin)"
identity_check "$RANGER_CONTAINER" trino-ranger || die "identity check failed on trino-ranger"

# --- 12. switch the profile to OPA ----------------------------------------------------------
MECH=$(api "$BASE/api/control-plane/policies/services" | jget "next((p.get('mechanism') for p in (d if isinstance(d,list) else d.get('services',d.get('items',[]))) if p['name']=='$PROFILE'), None)")
if [ "$MECH" != "opa" ]; then
  log "PATCH $PROFILE mechanism=opa"
  code=$(api_code -X PATCH "$BASE/api/control-plane/policies/services/$PROFILE" -d '{"mechanism":"opa"}')
  cat "$WORK/last.json"; echo
  [ "$code" = 200 ] || die "PATCH mechanism answered $code"
  python3 - "$WORK/last.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
# trino-ranger is no engine row (see step 6/7), so no external engine is bound to the profile.
ok = d.get("engines_reconfigured") == ["trino-opa"] and d.get("external_engines_unchanged") == []
print(("PASS" if ok else "FAIL") + f" switch: engines_reconfigured={d.get('engines_reconfigured')} external_engines_unchanged={d.get('external_engines_unchanged')}")
sys.exit(0 if ok else 1)
PY
  sleep 5
  wait_opa_ready
fi
docker exec "$OPA_CONTAINER" grep -q '^access-control.name=opa' /etc/trino/access-control.properties \
  || die "$OPA_CONTAINER is not on the OPA plugin"
echo "up.sh: ready (egret $BASE, trino-opa $OPA_CONTAINER, trino-ranger :${TRINO_RANGER_PORT:-28082})"
