#!/usr/bin/env bash
# E2E ONLY (governance SP2b, Task 22). The latency of egret's OPA surface as Trino 483 sees it,
# against the Ranger baseline. Run after scripts/opa-e2e/up.sh, on the `egret-opa-e2e` stack:
#
#   scripts/opa-e2e/up.sh && scripts/opa-e2e/latency.sh > latency.out; scripts/opa-e2e/down.sh
#
# 1. Setup (idempotent): 1 000 tables `lake.s1.b0000…b0999` (two columns each) and one table
#    `lake.s1.wide` with 500 columns, created as the platform admin on trino-ranger; ann reads
#    `lake.s1` and `lake.information_schema`; a MASK_HASH rule on `wide.c000`. Then a wait until
#    both engines answer from those policies.
# 2. For each engine (trino-ranger = the Ranger baseline, trino-opa = egret's OPA surface) and
#    each statement below, LATENCY_RUNS (30) timed runs as ann, after LATENCY_WARMUP (2) runs
#    that are not counted:
#      SHOW TABLES FROM lake.s1
#      SELECT * FROM lake.s1.wide LIMIT 1
#      SELECT count(*) FROM lake.information_schema.columns            (Trino #25628)
#    Per run: the client's wall time (taken inside the container), and queued / analysis /
#    planning / elapsed from `system.runtime.queries`. Then LATENCY_RUNS runs of
#    `EXPLAIN ANALYZE <statement>`, whose Queued/Analysis/Planning/Execution line is parsed.
#    On trino-opa, every plain run also reads egret's `GET /metrics` before and after, and the
#    delta of egret_opa_{allow,deny,error,unread}_total is the number of requests the plugin sent.
# 3. trino-opa again with `opa.policy.batched-uri` removed (sed in the container, docker
#    restart of that container), then the file restored by re-sending
#    `PATCH …/services/opa_e2e {"mechanism":"opa"}`, which re-applies the engine.
# 4. Restart window: `SELECT 1` as ann every 100 ms on both engines while
#    `docker compose … restart egret` runs; the first and last failure on each side.
#
# Raw outputs go to $LATENCY_OUT (default $WORK/latency): latency.json and restart-*.txt.
# Never acts on another compose project: every compose call names the file AND -p.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$REPO/deploy/opa-e2e/docker-compose.yml" -p egret-opa-e2e)
export WORK="${WORK:-${TMPDIR:-/tmp}/egret-opa-e2e}"
export EGRET_OPA_E2E_PORT="${EGRET_OPA_E2E_PORT:-28081}"
BASE="http://127.0.0.1:${EGRET_OPA_E2E_PORT}"
OPA_CONTAINER=egret-engine-egret-engines-trino-opa-coordinator
CATALOG=lake
OUT="${LATENCY_OUT:-$WORK/latency}"
mkdir -p "$OUT"

jget() { python3 -c "import sys,json; d=json.load(sys.stdin); v=$1; print('' if v is None else v)"; }
die() { printf 'latency.sh: %s\n' "$*" >&2; exit 1; }

JAR="$WORK/latency-admin.jar"
curl -fsS -c "$JAR" -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' \
  -d '{"email":"admin@egret.local","password":"Admin12345!"}' -o /dev/null
CSRF=$(awk '/egret_csrf/{print $7}' "$JAR" | tail -1)
api() { curl -sS --fail-with-body -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'Content-Type: application/json' "$@"; }

CAT_ID=$(api "$BASE/api/control-plane/catalogs" | jget "next((c['id'] for c in d if c['name']=='$CATALOG'), None)")
[ -n "$CAT_ID" ] || die "no catalog $CATALOG: run up.sh first"
ADMIN_USER=$(api "$BASE/api/control-plane/users" | jget "next((u['username'] for u in (d if isinstance(d,list) else d.get('items',[])) if u['email']=='admin@egret.local'), None)")
[ -n "$ADMIN_USER" ] || die "no platform admin user"
RANGER_CONTAINER=$("${COMPOSE[@]}" ps -q trino-ranger)
[ -n "$RANGER_CONTAINER" ] || die "trino-ranger is not running"
docker exec "$OPA_CONTAINER" true || die "$OPA_CONTAINER is not running"

# --- policies (idempotent) ------------------------------------------------------------------
grants=$(api "$BASE/api/control-plane/perm/grants")
grant_schema_once() { # $1 = schema: allow read to ann on the whole schema
  if ! GRANTS="$grants" python3 -I - "$1" <<'PY'
import json, os, sys
schema = sys.argv[1]
d = json.loads(os.environ["GRANTS"])
rows = d if isinstance(d, list) else d.get("grants", d.get("items", []))
for g in rows:
    s = g.get("scope", {})
    inc = [p.get("ref") for p in g.get("principals", {}).get("include", [])]
    if g.get("effect") == "allow" and s.get("schema") == schema and not s.get("table") \
       and not s.get("column") and "ann" in inc:
        sys.exit(0)
sys.exit(1)
PY
  then
    api -X POST "$BASE/api/control-plane/perm/grants" -d "{
      \"effect\":\"allow\",\"actions\":[\"read\"],
      \"scope\":{\"catalog_id\":\"$CAT_ID\",\"schema\":\"$1\",\"table\":null,\"column\":null},
      \"principals\":{\"include\":[{\"kind\":\"user\",\"ref\":\"ann\"}],\"exclude\":[]}}" >/dev/null
    echo "setup: + grant allow read ann on $CATALOG.$1"
  fi
}
grant_schema_once s1
grant_schema_once information_schema
if ! api "$BASE/api/control-plane/perm/rules" | grep '"lat-mask-wide"' >/dev/null; then
  api -X POST "$BASE/api/control-plane/perm/rules" -d "{
    \"name\":\"lat-mask-wide\",\"kind\":{\"type\":\"mask\",\"mask_type\":\"MASK_HASH\"},
    \"selector\":{\"type\":\"path\",\"catalog_id\":\"$CAT_ID\",\"schema\":\"s1\",\"table\":\"wide\",\"column\":\"c000\"},
    \"exempt\":{\"include\":[],\"exclude\":[]}}" >/dev/null
  echo "setup: + rule lat-mask-wide (MASK_HASH on $CATALOG.s1.wide.c000)"
fi

# --- everything else: one Python driver -----------------------------------------------------
export REPO OUT BASE OPA_CONTAINER RANGER_CONTAINER ADMIN_USER JAR CSRF CATALOG
COMPOSE_JSON=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${COMPOSE[@]}")
export COMPOSE_JSON
python3 -I - <<'PY'
import concurrent.futures as cf
import json
import math
import os
import re
import statistics
import subprocess
import sys
import time
import urllib.request

REPO = os.environ["REPO"]
OUT = os.environ["OUT"]
BASE = os.environ["BASE"]
OPA = os.environ["OPA_CONTAINER"]
RANGER = os.environ["RANGER_CONTAINER"]
ADMIN = os.environ["ADMIN_USER"]
CATALOG = os.environ["CATALOG"]
COMPOSE = json.loads(os.environ["COMPOSE_JSON"])
RUNS = int(os.environ.get("LATENCY_RUNS", "30"))
WARMUP = int(os.environ.get("LATENCY_WARMUP", "2"))
PHASES = os.environ.get("LATENCY_PHASES", "setup,batched,unbatched,restart").split(",")
RESTART_SECONDS = int(os.environ.get("LATENCY_RESTART_SECONDS", "90"))
SCRIPT = open(f"{REPO}/scripts/opa-e2e/latency-run.sh").read()
TABLES = 1000
WIDE = 500
QUERIES = [
    ("show_tables", f"SHOW TABLES FROM {CATALOG}.s1"),
    ("select_wide", f"SELECT * FROM {CATALOG}.s1.wide LIMIT 1"),
    ("info_schema_columns", f"SELECT count(*) FROM {CATALOG}.information_schema.columns"),
]
RESULTS = {"runs": RUNS, "warmup": WARMUP, "sets": [], "restart": None}


def say(*a):
    print(*a, flush=True)


def once(container, user, sql):
    p = subprocess.run(
        ["docker", "exec", container, "sh", "-c", SCRIPT, "latency-run", "once", user, sql],
        capture_output=True, text=True, timeout=900,
    )
    t0 = t1 = None
    resps = []
    for line in p.stdout.splitlines():
        if line.startswith("@@T0 "):
            t0 = int(line[5:])
        elif line.startswith("@@T1 "):
            t1 = int(line[5:])
        elif line.strip():
            try:
                resps.append(json.loads(line))
            except json.JSONDecodeError:
                resps.append({"error": {"errorName": "HARNESS", "message": "not JSON: " + line[:200]}})
    err = next((r["error"] for r in resps if "error" in r), None)
    if p.returncode != 0 and err is None:
        err = {"errorName": "HARNESS", "message": f"docker exec exit {p.returncode}: {p.stderr[:200]}"}
    return {
        "qid": resps[0].get("id") if resps else None,
        "wall_ms": (t1 - t0) / 1e6 if t0 and t1 else None,
        "error": err,
        "rows": [row for r in resps for row in r.get("data", [])],
    }


def must(container, user, sql):
    r = once(container, user, sql)
    if r["error"]:
        sys.exit(f"latency.sh: {sql!r} as {user} failed: {r['error'].get('message', r['error'])}")
    return r["rows"]


def metrics():
    txt = urllib.request.urlopen(BASE + "/metrics", timeout=10).read().decode()
    out = {}
    for line in txt.splitlines():
        m = re.match(r'egret_opa_(allow|deny|error|unread)_total\{route="([a-z_]+)"\} (\d+)', line)
        if m:
            out[(m[1], m[2])] = int(m[3])
    return out


def delta(m0, m1):
    d = {}
    for k in set(m0) | set(m1):
        v = m1.get(k, 0) - m0.get(k, 0)
        if v:
            d[f"{k[0]}:{k[1]}"] = v
    return d


UNIT = {"ns": 1e-6, "us": 1e-3, "µs": 1e-3, "ms": 1.0, "s": 1e3, "m": 6e4, "h": 3.6e6, "d": 8.64e7}


def dur_ms(v, u):
    return float(v) * UNIT[u]


EA = re.compile(
    r"Queued: ([\d.]+)(\w+), Analysis: ([\d.]+)(\w+), Planning: ([\d.]+)(\w+), Execution: ([\d.]+)(\w+)"
)


def pct(xs, p):
    s = sorted(xs)
    return s[max(0, math.ceil(p / 100 * len(s)) - 1)]


def summary(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return "n=0"
    return (f"n={len(xs)} min={min(xs):.1f} p50={statistics.median(xs):.1f} "
            f"p99={pct(xs, 99):.1f} max={max(xs):.1f} mean={statistics.fmean(xs):.1f}")


def wait_ready(container):
    for _ in range(120):
        p = subprocess.run(["docker", "exec", container, "curl", "-fsS", "http://127.0.0.1:8080/v1/info"],
                           capture_output=True, text=True)
        if '"starting":false' in p.stdout:
            # The coordinator answers before it accepts queries as ann; wait for one.
            if not once(container, "ann", "SELECT 1")["error"]:
                return
        time.sleep(2)
    sys.exit(f"latency.sh: {container} did not become ready")


def api(method, path, body=None):
    cmd = ["curl", "-sS", "-o", "-", "-w", "\n%{http_code}", "-b", os.environ["JAR"],
           "-H", "X-Egret-CSRF: " + os.environ["CSRF"], "-H", "Content-Type: application/json",
           "-X", method, BASE + path]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    p = subprocess.run(cmd, capture_output=True, text=True)
    text, _, code = p.stdout.rpartition("\n")
    return int(code or 0), text


# --- 1. setup ------------------------------------------------------------------------------
def setup():
    have = {r[0] for r in must(RANGER, ADMIN, f"SHOW TABLES FROM {CATALOG}.s1")}
    missing = [f"b{i:04d}" for i in range(TABLES) if f"b{i:04d}" not in have]
    if missing:
        say(f"setup: creating {len(missing)} tables {CATALOG}.s1.b0000…b{TABLES - 1:04d}")
        t = time.time()
        with cf.ThreadPoolExecutor(8) as ex:
            for r in ex.map(lambda n: once(RANGER, ADMIN,
                                           f"CREATE TABLE IF NOT EXISTS {CATALOG}.s1.{n} (id bigint, v varchar)"),
                            missing):
                if r["error"]:
                    sys.exit(f"latency.sh: create failed: {r['error'].get('message')}")
        say(f"setup: created in {time.time() - t:.1f} s")
    if "wide" not in have:
        cols = ", ".join(f"'v{i:03d}' AS c{i:03d}" for i in range(WIDE))
        must(RANGER, ADMIN, f"CREATE TABLE {CATALOG}.s1.wide AS SELECT {cols}")
        say(f"setup: created {CATALOG}.s1.wide with {WIDE} columns and one row")
    n = must(RANGER, ADMIN, f"SELECT count(*) FROM {CATALOG}.information_schema.tables WHERE table_schema = 's1'")
    say(f"setup: {CATALOG}.s1 holds {n[0][0]} tables (admin, trino-ranger)")
    want = int(n[0][0])
    # Both engines answer from the policies: ann lists every table and sees c000 masked.
    t = time.time()
    while True:
        ok = True
        for c in (RANGER, OPA):
            a = once(c, "ann", f"SELECT count(*) FROM {CATALOG}.information_schema.tables WHERE table_schema = 's1'")
            b = once(c, "ann", f"SELECT length(c000) FROM {CATALOG}.s1.wide")
            ok &= not a["error"] and int(a["rows"][0][0]) == want and not b["error"] and b["rows"][0][0] == 64
        if ok:
            break
        if time.time() - t > 180:
            sys.exit("latency.sh: the engines did not converge on the setup's policies in 180 s")
        time.sleep(5)
    say(f"setup: both engines list {want} tables to ann and mask wide.c000 (waited {time.time() - t:.0f} s)")


# --- 2/3. timed sets -----------------------------------------------------------------------
def runtime_stats(container, qids):
    ids = ", ".join(f"'{q}'" for q in qids)
    rows = must(container, ADMIN,
                'SELECT query_id, queued_time_ms, analysis_time_ms, planning_time_ms, '
                'to_milliseconds("end" - created) FROM system.runtime.queries '
                f"WHERE query_id IN ({ids})")
    return {r[0]: {"queued_ms": r[1], "analysis_ms": r[2], "planning_ms": r[3], "elapsed_ms": r[4]}
            for r in rows}


def measure(engine, container, mode, count_requests):
    for label, sql in QUERIES:
        for _ in range(WARMUP):
            once(container, "ann", sql)
        runs = []
        for i in range(RUNS):
            m0 = metrics() if count_requests else None
            r = once(container, "ann", sql)
            m1 = metrics() if count_requests else None
            runs.append({"qid": r["qid"], "wall_ms": r["wall_ms"], "rows": len(r["rows"]),
                         "error": r["error"] and r["error"].get("message"),
                         "requests": delta(m0, m1) if count_requests else None})
        stats = runtime_stats(container, [x["qid"] for x in runs if x["qid"]])
        for x in runs:
            x.update(stats.get(x["qid"], {}))
        explain = []
        for i in range(RUNS):
            r = once(container, "ann", "EXPLAIN ANALYZE " + sql)
            text = "\n".join(str(c) for row in r["rows"] for c in row)
            m = EA.search(text)
            explain.append({
                "error": r["error"] and r["error"].get("message"),
                "queued_ms": m and dur_ms(m[1], m[2]), "analysis_ms": m and dur_ms(m[3], m[4]),
                "planning_ms": m and dur_ms(m[5], m[6]), "execution_ms": m and dur_ms(m[7], m[8]),
                "wall_ms": r["wall_ms"],
            })
        s = {"engine": engine, "mode": mode, "query": label, "sql": sql, "runs": runs, "explain": explain}
        RESULTS["sets"].append(s)
        errors = [x["error"] for x in runs if x["error"]] + [x["error"] for x in explain if x["error"]]
        say(f"\n== {engine} [{mode}] {label}: {sql}")
        say(f"   rows returned (each run): {sorted({x['rows'] for x in runs})}; errors: {len(errors)}"
            + (f" first: {errors[0]}" if errors else ""))
        for k in ("wall_ms", "queued_ms", "analysis_ms", "planning_ms", "elapsed_ms"):
            say(f"   query    {k:<12} {summary([x.get(k) for x in runs])}")
        for k in ("queued_ms", "analysis_ms", "planning_ms", "execution_ms", "wall_ms"):
            say(f"   explain  {k:<12} {summary([x.get(k) for x in explain])}")
        if count_requests:
            totals = [sum(v for k, v in x["requests"].items() if k.split(":")[0] in ("allow", "deny"))
                      for x in runs]
            say(f"   requests per query (allow+deny delta): {summary(totals)}")
            shapes = {}
            for x in runs:
                key = json.dumps(dict(sorted(x["requests"].items())))
                shapes[key] = shapes.get(key, 0) + 1
            for k, v in sorted(shapes.items(), key=lambda kv: -kv[1]):
                say(f"   requests by kind:route, {v} run(s): {k}")


def set_batched(on):
    path = "/etc/trino/access-control.properties"
    if not on:
        subprocess.run(["docker", "exec", OPA, "sed", "-i", "/^opa.policy.batched-uri=/d", path], check=True)
        subprocess.run(["docker", "restart", OPA], check=True, capture_output=True)
    else:
        code, body = api("PATCH", "/api/control-plane/policies/services/opa_e2e", {"mechanism": "opa"})
        say(f"restore: PATCH opa_e2e mechanism=opa answered {code}: {body}")
        if code != 200:
            sys.exit("latency.sh: the restoring PATCH failed")
        time.sleep(5)
    wait_ready(OPA)
    p = subprocess.run(["docker", "exec", OPA, "grep", "-c", "^opa.policy.batched-uri=", path],
                       capture_output=True, text=True)
    n = int(p.stdout.strip() or 0)
    say(f"trino-opa: opa.policy.batched-uri lines in {path}: {n}")
    if (n == 1) != on:
        sys.exit("latency.sh: the batched-uri edit did not take")


# --- 4. restart window ---------------------------------------------------------------------
def restart_window():
    procs = {}
    for name, c in (("trino-opa", OPA), ("trino-ranger", RANGER)):
        f = open(f"{OUT}/restart-{name}.txt", "w")
        procs[name] = (subprocess.Popen(
            ["docker", "exec", c, "sh", "-c", SCRIPT, "latency-run", "loop", "ann", "SELECT 1",
             str(RESTART_SECONDS), "0.1"], stdout=f), f)
    time.sleep(10)
    t0 = time.time()
    p = subprocess.run(COMPOSE + ["restart", "egret"], capture_output=True, text=True)
    t1 = time.time()
    say(f"\n== restart window: `docker compose … restart egret` exit {p.returncode}, took {t1 - t0:.2f} s (host clock)")
    healthy = None
    while time.time() - t1 < 120:
        try:
            urllib.request.urlopen(BASE + "/healthz", timeout=2)
            healthy = time.time()
            break
        except Exception:
            time.sleep(0.1)
    say("   egret /healthz answered " + (f"{healthy - t0:.2f} s after the restart began (host clock)"
                                         if healthy else "NOT within 120 s"))
    out = {"compose_restart_s": t1 - t0, "healthz_after_s": healthy and healthy - t0}
    for name, (proc, f) in procs.items():
        proc.wait()
        f.close()
        rows = [l.split() for l in open(f"{OUT}/restart-{name}.txt") if l.strip()]
        rows = [(int(a), s, int(b)) for a, s, b in rows]
        fails = [r for r in rows if r[1] == "fail"]
        gaps = [(rows[i + 1][0] - rows[i][0]) / 1e6 for i in range(len(rows) - 1)]
        line = f"   {name}: {len(rows)} statements, {len(fails)} failed"
        res = {"statements": len(rows), "failed": len(fails),
               "start_gap_ms": {"p50": gaps and statistics.median(gaps), "max": gaps and max(gaps)}}
        if fails:
            first, last = fails[0], fails[-1]
            after = next((r for r in rows if r[0] > last[0] and r[1] == "ok"), None)
            before = [r for r in rows if r[0] < first[0] and r[1] == "ok"]
            res.update({
                "first_to_last_failure_s": (last[0] - first[0]) / 1e9,
                "last_ok_before_to_first_ok_after_s":
                    (after[0] - before[-1][2]) / 1e9 if after and before else None,
            })
            line += (f"; first to last failure (statement starts): {res['first_to_last_failure_s']:.2f} s; "
                     f"end of last ok before -> start of first ok after: "
                     + (f"{res['last_ok_before_to_first_ok_after_s']:.2f} s" if after and before else "n/a"))
        if gaps:
            line += f"; start-to-start gap p50 {statistics.median(gaps):.0f} ms, max {max(gaps):.0f} ms"
        say(line)
        out[name] = res
    RESULTS["restart"] = out


if "setup" in PHASES:
    setup()
if "batched" in PHASES:
    measure("trino-ranger", RANGER, "Ranger plugin", False)
    measure("trino-opa", OPA, "batched-uri on", True)
if "unbatched" in PHASES:
    set_batched(False)
    measure("trino-opa", OPA, "batched-uri off", True)
    set_batched(True)
if "restart" in PHASES:
    restart_window()
json.dump(RESULTS, open(f"{OUT}/latency.json", "w"), indent=1)
say(f"\nlatency.sh: raw results in {OUT}/latency.json")
bad = [s for s in RESULTS["sets"] if any(x["error"] for x in s["runs"])]
sys.exit(1 if bad else 0)
PY
