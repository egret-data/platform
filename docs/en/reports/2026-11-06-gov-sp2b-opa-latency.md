# Governance SP2b: OPA surface latency, measured (2026-11-06)

This report measures egret's OPA surface: the part of egret that answers Trino 483's OPA
plugin. It compares OPA with the Ranger plugin on the same profile, and it answers spec
question S2: does the decision endpoint need a sidecar?

Every number below was printed by a command. The raw outputs are kept outside the
repository, in the measuring session's scratchpad,
`/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/latency/`
(named `RAW/` below). Each section names the file and the command that produced its
numbers. The commands re-run on any `egret-opa-e2e` stack.

## Answer to S2: no sidecar

**No sidecar is needed.** Two measurements decide it:

1. **Latency.** On `SHOW TABLES` over 1 003 tables, analysis takes 19.0 ms p50 on OPA and
   10.5 ms on Ranger's in-JVM plugin: about 8 ms more, or 1.8 times as long. That is inside
   the wall-time spread: wall p50 is 292.6 ms against 274.5 ms, and Ranger's own wall time
   runs from 258.3 ms to 339.8 ms. On the other two statements, the wall times are within
   the run-to-run spread.
2. **Restart window.** A restart of egret cost trino-opa at most **one** failed statement in
   each of three runs (1, 0 and 1). The gap from the last success to the next success was
   0.30 s and 0.29 s. egret's `/healthz` answered again 1.42 s, 1.31 s and 1.17 s after the
   restart began.

A sidecar would remove that window, but only for an egret restart; it would need its own
restarts for upgrades. The costs that did show up do not depend on which process the
evaluator runs in:
- the plugin's own HTTP queue overflows when batching is off;
- evaluation is linear in the statements of one catalog;
- the stored-overlap scan reads every service on each rebuild.

A sidecar would run the same evaluator and get the same requests. See "What would change
the answer" at the end.

## Machine

From `RAW/machine.txt` (`sysctl -n machdep.cpu.brand_string hw.ncpu hw.perflevel0.physicalcpu hw.memsize`,
`docker info`, `rustc -V`):

| | |
|---|---|
| CPU | Intel(R) Core(TM) i7-1068NG7 CPU @ 2.30GHz; `hw.ncpu` 8, `hw.perflevel0.physicalcpu` 4 |
| RAM | `hw.memsize` 34359738368 bytes |
| OS | macOS 26.7 (25G229) |
| Docker VM | Docker Desktop, server 28.4.0; `NCPU=8`, `MemTotal=25198567424` |
| Rust | rustc 1.99.0 |
| trino-opa | provisioned by egret: `cpuMax` 2, `memory` 2Gi, coordinator only (`scripts/opa-e2e/up.sh`) |
| trino-ranger | compose Trino 483, `JAVA_TOOL_OPTIONS=-Xmx1g` (`deploy/opa-e2e/docker-compose.yml`) |

**Conditions: not an idle machine.** No other test suite or build of mine ran during the
benches or the end-to-end run. Other projects' compose stacks (`egret-demo`, `egret-dev-*`,
the docs Trino coordinator) were running the whole time, as the Docker rules require; their
load was not recorded. Ranger and OPA ran back to back under those same conditions, so
compare them with each other, and treat the absolute times as this machine's under that
background load.

**The image.** `egret-opa-e2e-backend:dev` is `sha256:2c39814c…`, created
2026-11-05T18:15:35Z. That is after `ff0224ce` (01:02 +07), and no Rust under `crates/`,
`migrations/`, `Cargo.lock` or the backend Dockerfile changed between `ff0224ce` and
`fecdd781` (`git diff --stat`: only `crates/egret-perm-compile/tests/…`). As a rebuild check,
`grep -ac "request body not read"` and `grep -ac egret_opa_unread_total` on
`/usr/local/bin/egret` in that image each gave `1` (`RAW/rebuild-check.txt`).

## 1. Microbenchmarks: `egret-perm-eval`

Command: `CARGO_TARGET_DIR=$PWD/target-sp2b cargo bench -p egret-perm-eval --bench eval -- --save-baseline sp2b`.
Output: `RAW/criterion.out`. The bench is `crates/egret-perm-eval/benches/eval.rs`
(criterion 0.5.1, no default features; it was absent from the workspace before).

**The generator.** It lays out 100 catalogs × 10 schemas × 10 tables. On each table there is
one Read grant to `ann` and one other user. On every 20th table there is also a `MASK_HASH`
on `email` and a row filter. One deny and the Trino infrastructure set (`lower_infra`) are
added. The bench printed these statement counts:
- 1 106 statements for 1 000 tables;
- 11 006 statements for 10 000 tables.

**Two layouts.**
- `grid` is the brief's layout. The snapshot indexes path statements **by catalog only**, so a
  question on the grid meets only its own catalog's statements, the same ones at either size.
- `one_catalog` puts the same tables in one catalog, so every statement is a candidate. That
  is the shape of an installation with one big lakehouse catalog.

Every bench asks about the same objects at both sizes. **The bench IDs' `/1000` and `/10000`
are table counts, not statement counts** (1 106 and 11 006 statements, above). The value shown
is criterion's middle estimate, with its interval in brackets.

| Bench | 1 000 tables | 10 000 tables |
|---|---|---|
| `build/snapshot` | 87.418 µs [86.856, 88.049] | 1.4937 ms [1.4783, 1.5097] |
| `decide/select_8_columns` | 227.49 µs [222.11, 233.72] | 221.91 µs [220.25, 223.52] |
| `filter/FilterTables_1000` | 18.911 ms [18.787, 19.048] | 19.019 ms [18.848, 19.229] |
| `column_masks/500_columns` | 1.7343 ms [1.7238, 1.7463] | 1.7951 ms [1.7464, 1.8520] |
| `build/snapshot_one_catalog` | 58.655 µs [58.100, 59.198] | 694.56 µs [684.73, 704.59] |
| `decide/select_8_columns_one_catalog` | 2.6550 ms [2.6409, 2.6700] | 4.1363 ms [4.0994, 4.1749] |
| `filter/FilterTables_1000_one_catalog` | 185.10 ms [183.02, 187.54] | 361.10 ms [358.24, 364.17] |
| `column_masks/500_columns_one_catalog` | 13.603 ms [13.353, 13.916] | 139.34 ms [137.92, 141.08] |

What this shows:

- **On the grid, size does not matter.** The catalog index keeps the cost flat from 1 106 to
  11 006 statements.
- **In one catalog, cost grows with the catalog's statement count.** A `FilterTables` of
  1 000 tables costs 185.10 ms at 1 106 statements and 361.10 ms at 11 006. A 500-column mask
  question goes from 13.603 ms to 139.34 ms.
- **The evaluator is CPU-bound and runs inside the request.** A sidecar would run this same
  code, so these numbers do not argue for one. They argue for a finer index (by schema and
  table) if one-catalog installations grow. See concern 2.
- **The end-to-end run below does not hit this shape.** ann holds one schema-level grant
  there. The worst shape, per-table grants in one catalog, is measured only here.
- **10 000 statements cannot be reached through the API today** (section 5): a profile is
  capped at 1 000 compiled policies. The reachable row is the 1 000-table one.

## 2. End to end: Trino 483 on OPA against Trino 483 on Ranger

Command: `scripts/opa-e2e/up.sh`, then `scripts/opa-e2e/latency.sh`, with
`WORK=RAW/work` and `LATENCY_OUT=RAW/run`. Outputs:
- `RAW/latency.out`: the printed summary;
- `RAW/run/latency.json`: every run;
- `RAW/setup.out`: the setup's first pass.

The stack is `egret-opa-e2e` (Tasks 20 and 21). It runs two engines:
- **trino-opa** is provisioned by egret and runs the OPA plugin against egret on port 28081.
- **trino-ranger** is the Ranger baseline (Task 20 D1). It is a compose Trino 483 running the
  Ranger plugin on the **same profile's** Ranger download.

**Setup** (`latency.sh`, idempotent):
- 1 000 tables `lake.s1.b0000…b0999` (`id bigint, v varchar`), created in 94.4 s with 8
  parallel `CREATE TABLE`s as the platform admin on trino-ranger;
- `lake.s1.wide`, with 500 columns and one row;
- `lake.s1` holds 1 003 tables;
- ann holds an allow-read grant on `lake.s1` and one on `lake.information_schema`;
- a `MASK_HASH` rule on `wide.c000`.

Both engines then listed 1 003 tables to ann and masked `c000`; the script waited for that.

**Per statement.** Each statement ran as ann over Trino's client protocol from inside the
container (`scripts/opa-e2e/latency-run.sh`, `curl`), so `docker exec` is not in the timing.
- 2 warm-up runs are not counted.
- 30 runs are timed. For each, the script records the client's wall time and, from
  `system.runtime.queries`, `queued_time_ms`, `analysis_time_ms`, `planning_time_ms` and
  `end − created`.
- Then 30 runs of `EXPLAIN ANALYZE <statement>`, whose `Queued/Analysis/Planning/Execution`
  line is parsed.

p99 is nearest-rank: with 30 runs it is the largest value. All times are in milliseconds.
Every run below returned without error unless the table says otherwise.

### 2.1 Batched (the configuration egret writes)

**`SHOW TABLES FROM lake.s1`.** Every run returned 1 003 rows.

| | Ranger p50 | Ranger p99 | OPA p50 | OPA p99 |
|---|---|---|---|---|
| client wall | 274.5 | 339.8 | 292.6 | 332.5 |
| analysis (`system.runtime.queries`) | 10.5 | 20.0 | 19.0 | 23.0 |
| planning (`system.runtime.queries`) | 15.0 | 31.0 | 19.0 | 28.0 |
| elapsed (`end − created`) | 233.5 | 284.0 | 246.0 | 281.0 |
| EXPLAIN ANALYZE analysis | 10.4 | 16.0 | 17.8 | 27.6 |
| EXPLAIN ANALYZE planning | 13.7 | 27.6 | 14.5 | 31.0 |

**`SELECT * FROM lake.s1.wide LIMIT 1`.** Each run returned 1 row of 500 columns.

| | Ranger p50 | Ranger p99 | OPA p50 | OPA p99 |
|---|---|---|---|---|
| client wall | 274.8 | 471.7 | 268.1 | 322.7 |
| analysis | 46.0 | 68.0 | 54.0 | 74.0 |
| planning | 72.5 | 144.0 | 63.0 | 89.0 |
| elapsed | 226.0 | 426.0 | 221.0 | 272.0 |
| EXPLAIN ANALYZE analysis | 43.2 | 88.2 | 49.0 | 63.5 |
| EXPLAIN ANALYZE planning | 84.7 | 159.8 | 78.7 | 87.7 |

**`SELECT count(*) FROM lake.information_schema.columns`** (Trino #25628). Each run
returned 1 row.

| | Ranger p50 | Ranger p99 | OPA p50 | OPA p99 |
|---|---|---|---|---|
| client wall | 3785.4 | 4413.6 | 3925.6 | 4570.8 |
| analysis | 2.0 | 2.0 | 6.0 | 16.0 |
| planning | 6.0 | 8.0 | 6.0 | 20.0 |
| elapsed | 3744.0 | 4355.0 | 3878.0 | 4514.0 |
| EXPLAIN ANALYZE execution | 3885.0 | 4970.0 | 3870.0 | 4680.0 |

The access checks of this statement happen at execution time, while columns are listed,
not during analysis. On OPA it is 1 024 requests (below). The execution p50 is 3870.0 ms on
OPA and 3885.0 ms on Ranger.

### 2.2 Requests per query

On trino-opa, every timed run read `GET /metrics` before and after. The count is the delta
of `egret_opa_allow_total + egret_opa_deny_total`; `error` and `unread` stayed at 0. Those
metric names are in `crates/egret-perm-compile/src/opa/surface.rs`, and `/metrics` is
mounted in `crates/egret-server/src/lib.rs`. Each request is counted once, in allow or
deny, whatever a `/batch` holds.

| Statement | batched-uri on: requests per query (30 of 30 runs) | batched-uri off |
|---|---|---|
| `SHOW TABLES FROM lake.s1` | **9**: allow 5, batch 2, batch_column_masks 1, row_filters 1 | **1 011**: allow 1 009, batch_column_masks 1, row_filters 1 |
| `SELECT * FROM … wide LIMIT 1` | **7**: allow 5, batch_column_masks 1, row_filters 1 | **7** (the same) |
| `SELECT count(*) FROM … information_schema.columns` | **1 024**: allow 3, batch 1 018 (+1 denied), batch_column_masks 1, row_filters 1 | between **6 229** and **7 108** (p50 6 921), then the query **failed**, see 2.3 |

The 500 columns of `wide` cost one `/batch_column_masks` request, not 500. On
`information_schema.columns`, `/batch` took 1 019 requests (1 018 allowed, 1 denied) for a
catalog whose `lake.s1` holds 1 003 tables. That they are one `FilterColumns` per table is an
inference; nothing here maps requests to tables.

### 2.3 Without the batched URI

The edit was `docker exec <trino-opa> sed -i '/^opa.policy.batched-uri=/d'
/etc/trino/access-control.properties`, then `docker restart <trino-opa>`. `grep -c` printed
0 lines afterwards.

| | OPA, batched p50 / p99 | OPA, unbatched p50 / p99 |
|---|---|---|
| `SHOW TABLES` client wall | 292.6 / 332.5 | 407.0 / 738.4 |
| `SHOW TABLES` planning | 19.0 / 28.0 | 26.5 / 51.0 |
| `wide` client wall | 268.1 / 322.7 | 305.7 / 398.5 |

**`information_schema.columns` failed in 30 of 30 runs and in all 30 `EXPLAIN ANALYZE` runs**
("errors: 60"), each with `Error listing table columns for catalog lake: Failed to query
OPA backend`. That is the cause the brief asks for. It was reproduced once more on the
same stack, with the stack trace kept (`RAW/repro-unbatched.txt`,
`RAW/repro-unbatched-stack.txt`):

```text
java.util.concurrent.RejectedExecutionException: Max requests queued per destination 1024 exceeded
for HttpDestination[…http://egret:8080…],state=STARTED,queue=1023,
pool=…MultiplexConnectionPool…[…c=0/20/20,a=20,i=0,q=1017…]
```

The failure is in **the plugin's own HTTP client**, not in egret:
- the client has 20 connections and queues at most 1 024 requests per destination;
- egret refused at most one request per run. In every failing run, the `/metrics` delta
  (`RAW/latency.out`, "requests by kind:route") holds exactly one deny (`deny:allow: 1`) and
  no `error` or `unread` key. A 503 `busy` would be counted as a deny, so egret answered
  every other request. The batched runs show the same single deny, on `/batch`
  (`deny:batch: 1`). egret's log was not kept: the stack is gone, so the log is not evidence
  here;
- the plugin sent between 6 229 and 7 108 requests per run (allow+deny delta, almost all
  `/allow`) before its queue overflowed; those failing runs had an elapsed p50 of 4295.0 ms.

A sidecar would receive the same flood through the same client. The only fix is the batched
URI, which egret always writes. The file was then restored with
`PATCH /api/control-plane/policies/services/opa_e2e {"mechanism":"opa"}`. It answered 200
with `engines_reconfigured: ["trino-opa"]`, and `grep -c` printed 1 line again.

## 3. Restart window

`latency.sh`'s last phase, run three times:
- run 1 is the full run (`RAW/latency.out`, `RAW/run/restart-*.txt`);
- runs 2 and 3 are `LATENCY_PHASES=restart` (`RAW/restart2.out`, `RAW/restart3.out`, and
  `RAW/restart{2,3}/`).

Each run loops `SELECT 1` as ann on both engines, sleeping 100 ms between statements, while
`docker compose -f deploy/opa-e2e/docker-compose.yml -p egret-opa-e2e restart egret` runs.
Only egret is restarted. Each statement's own time comes on top of the 100 ms sleep, so
statements start every 234 to 242 ms at p50 (below), not every 100 ms.

| Run | `compose restart` | `/healthz` back after | trino-opa: statements / failed | last ok → next ok | trino-ranger: statements / failed |
|---|---|---|---|---|---|
| 1 | 1.08 s | 1.42 s | 366 / **1** | 0.30 s | 385 / 0 |
| 2 | 0.75 s | 1.31 s | 233 / **0** | n/a | 254 / 0 |
| 3 | 0.79 s | 1.17 s | 249 / **1** | 0.29 s | 256 / 0 |

Start-to-start gaps, counted only among statements that started at least 9 s after the
loop's first statement. The loops run 10 s before the restart begins, and the first statement
of a loop is a cold start: in run 2 it took 1591 ms. Recomputed by
`python3 -I RAW/restart_gaps.py RAW` into `RAW/restart-gaps-recomputed.txt`:

| Run | trino-opa p50 | trino-opa max | trino-ranger p50 | trino-ranger max |
|---|---|---|---|---|
| 1 | 240 ms | 1273 ms | 235 ms | 303 ms |
| 2 | 241 ms | 1339 ms | 233 ms | 426 ms |
| 3 | 234 ms | 1224 ms | 233 ms | 291 ms |

The figures `latency.sh` printed at run time counted every statement, the cold first one
included: run 2's trino-opa max of 1700 ms was that first statement. `latency.sh` now leaves
out the statements before the restart.

"First to last failure" is 0.00 s whenever there was a failure: only one statement failed.
The failed statement failed within 86 ms and 76 ms. In trino-opa's log of run 1
(`RAW/repro-unbatched-trino-opa.log`) it shows
`Server refused connection: http://egret:8080/opa/v1/data/egret/trino/allow` with
`java.net.ConnectException: Connection refused`.

The statement after it took 1160 ms (run 1, line 42) and 1116 ms (run 3, line 45). In run 2
no statement failed, but line 35, started 10.32 s into the loop, took 1234 ms. Line 1 of run 2
(1591 ms) is the cold start, not the restart. All of these are from
`RAW/restart-gaps-recomputed.txt`. So egret's restart shows up as one refused statement, or as
a stalled one (1234 ms). trino-ranger, whose plugin evaluates its own cached policies, saw
neither.

**What this does not cover.** The stack's database is small. After a restart, the refresher
builds each OPA snapshot on its first tick, and every request is refused until that build
finishes. At scale, that build time (section 4, up to 1 152.3 ms) adds to the window.

## 4. Follow-up T7 M1: snapshot rebuild when the key moves

Command (the stack down, nothing else running):

```bash
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --release --features harness --test opa_rebuild_timing \
  -- --ignored --nocapture
```

Output: `RAW/rebuild-timing.out`. The test is `crates/egret-perm-compile/tests/opa_rebuild_timing.rs`.
- Each step moves `policy_version` and times `SnapshotCache::refresh_once`, 10 times,
  asserting the profile was rebuilt each time.
- It then times `stored_overlaps_via` alone, 10 times, in a `REPEATABLE READ, READ ONLY`
  transaction, as a build runs it.

| Step | statements | services in DB | rebuild p50 (min–max) | overlap scan p50 (min–max) |
|---|---|---|---|---|
| `1k_alone`: one compiled profile (984 stored policies) | 902 | 2 | 60.2 ms (57.3–101.0) | 33.5 ms (29.5–52.1) |
| `10k_uncompiled` | 11 000 | 4 | 138.2 ms (127.5–151.9) | 39.1 ms (36.3–50.9) |
| `1k_beside_10_profiles`: plus 9 compiled profiles of 984 policies | 902 | 22 | 1044.8 ms (497.3–1152.3) | 351.5 ms (333.2–365.2) |
| `10k_beside_10_profiles` | 11 000 | 22 | 505.6 ms (469.1–538.0) | 371.7 ms (328.9–502.1) |

The first builds took 88.9 ms (`rt1k`) and 136.2 ms (`rt10k`).

**What this shows (T7 M1).** The overlap scan reads every service's stored units on every
rebuild, so its cost follows the database, not the profile:
- **33.5 ms → 351.5 ms** when nine other compiled profiles of 984 stored policies each were added;
- the profile being rebuilt did not change.

At 22 services the scan is most of a rebuild:
- 371.7 ms of the 505.6 ms for the 11 000-statement profile;
- the 902-statement compiled profile rebuilt in 1044.8 ms p50.

Statement count costs less: alone, 902 statements rebuilt in 60.2 ms and 11 000 in
138.2 ms.

**Not explained.** The 902-statement compiled profile rebuilds slower than the 11 000-statement
uncompiled one at 22 services (1044.8 ms against 505.6 ms), and its samples are bimodal
(min 497.3 ms). It may be the stored-state checks on its own 984 stored policies, which the
uncompiled profile has none of. That is a guess; I did not measure it.

**Effect.** The refresher runs one refresh at a time, every 1 s (`REFRESH_PERIOD`). At 22
services a single moved key cost between 469.1 ms and 1152.3 ms, so two profiles changing in
the same second already exceed the period. Fixing T7 M1 (scan only the rebuilt profile's
units) would remove most of the scan's 351.5 ms p50 from each rebuild at this size.

## 5. Found while measuring

- **A profile cannot exceed 1 000 compiled policies** (`PROFILE_EXPANSION_CEILING`,
  `crates/egret-perm-compile/src/trigger.rs`). The first two timing attempts refused:
  - 1 000 tables gave `ProfileExpansionCeiling { total: 1200, ceiling: 1000 }`
    (`RAW/rebuild-timing-first-attempt-ceiling.out`);
  - 900 tables gave `total: 1080` (`RAW/rebuild-timing-second-attempt-ceiling.out`).

  So a rule on a path compiles to two policies, and a 10 000-statement profile cannot be
  written through the API at all: every write recompiles the Ranger side too. The
  10 000-statement numbers in sections 1 and 4 come from model rows written directly, which
  the OPA snapshot lowers without a compile.
- **egret's own counters agree with Task 20's tcpdump.** Task 20 counted `SHOW TABLES` at
  5 `/allow`, 2 `/batch`, 1 `/batch_column_masks` and 1 `/row_filters` on its 10 000-table
  schema. The counters here gave the same 9 on 1 003 tables.

## What would change the answer

A sidecar becomes worth it if one of these is measured:
- an egret restart window, or a first snapshot build after a restart, that fails more than a
  statement or two at production size;
- egret's own request handling, rather than the evaluator, dominating analysis time under
  concurrent load. This report measured one query at a time; concurrency was not measured.

The cheaper fixes for what was measured are:
- a schema/table index in the snapshot (section 1);
- scanning only the rebuilt profile's units (section 4);
- keeping the batched URI (section 2.3).

## Files

| File | What |
|---|---|
| `crates/egret-perm-eval/benches/eval.rs` | the microbenchmarks |
| `crates/egret-perm-compile/tests/opa_rebuild_timing.rs` | T7 M1, `#[ignore]`d |
| `scripts/opa-e2e/latency.sh` | setup, timed sets, request counts, unbatched run, restart window |
| `scripts/opa-e2e/latency-run.sh` | runs inside each Trino container: one statement, or the restart loop |

## 22F: after the evaluator index and the scoped overlap scan (2026-11-06)

Task 22F fixed the two costs that sections 1 and 4 measured:

- **The path index.** The snapshot now indexes path statements by catalog, schema and table
  (`egret-perm-eval`, commit `1765e77d`). Before, it indexed them by catalog only.
- **The scoped overlap scan.** The refresher's stored-overlap re-check now reads only the
  units of the profile and of its tag service (`stored_overlaps_of_via`, commit `b1df6ee5`).
  Before, it read every service's units.

The raw outputs are in the measuring session's scratchpad,
`/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/latency-22F/`
(named `RAW22F/` below). Every "before" figure is Task 22's, from `RAW/`.

**Conditions.** The machine was not idle. The `egret-demo` and `egret-dev-*` stacks and the
docs Trino ran throughout, as the Docker rules require. `ps` showed Docker's VM process at
74.5 % to 82.5 % of a core, and the load average was 4.73 when the benches started
(`RAW22F/conditions-bench.txt`, `RAW22F/conditions-rebuild.txt`). No other build or test
suite of mine ran during either measurement.

### 22F.1 Microbenchmarks

Command: `CARGO_TARGET_DIR=$PWD/target-sp2b cargo bench -p egret-perm-eval --bench eval -- --baseline sp2b`
at `b1df6ee5`. Output: `RAW22F/criterion.out`. The bench and its generator are unchanged,
and so are the statement counts it printed (1 106 and 11 006). Each value is criterion's
middle estimate. The change column is criterion's own comparison with the `sp2b` baseline.

| Bench | tables | before (`RAW/criterion.out`) | after | criterion's change |
|---|---|---|---|---|
| `build/snapshot` | 1 000 | 87.418 µs | 288.94 µs | +232.47 % |
| | 10 000 | 1.4937 ms | 3.1946 ms | +113.87 % |
| `decide/select_8_columns` | 1 000 | 227.49 µs | 9.0885 µs | −95.928 % |
| | 10 000 | 221.91 µs | 9.1735 µs | −95.843 % |
| `filter/FilterTables_1000` | 1 000 | 18.911 ms | 1.1925 ms | −93.663 % |
| | 10 000 | 19.019 ms | 1.1953 ms | −93.693 % |
| `column_masks/500_columns` | 1 000 | 1.7343 ms | 733.95 µs | −57.987 % |
| | 10 000 | 1.7951 ms | 729.73 µs | −58.260 % |
| `build/snapshot_one_catalog` | 1 000 | 58.655 µs | 297.61 µs | +412.31 % |
| | 10 000 | 694.56 µs | 3.4863 ms | +401.94 % |
| `decide/select_8_columns_one_catalog` | 1 000 | 2.6550 ms | 9.1431 µs | −99.641 % |
| | 10 000 | 4.1363 ms | 9.2305 µs | −99.775 % |
| `filter/FilterTables_1000_one_catalog` | 1 000 | 185.10 ms | 1.2592 ms | −99.331 % |
| | 10 000 | 361.10 ms | 1.2070 ms | −99.668 % |
| `column_masks/500_columns_one_catalog` | 1 000 | 13.603 ms | 742.71 µs | −94.576 % |
| | 10 000 | 139.34 ms | 742.18 µs | −99.470 % |

What this shows:

- **Every question now costs the same at both sizes and in both layouts.** A `FilterTables`
  of 1 000 tables costs about 1.2 ms. In one catalog it cost 185.10 ms at 1 106 statements
  and 361.10 ms at 11 006. A 500-column mask question costs about 0.74 ms; it was 13.603 ms
  and 139.34 ms.
- **The grid also got faster.** There, each question used to scan every statement of its
  catalog.
- **Building the snapshot costs more.** It now takes 3.1946 ms at 11 006 statements (grid),
  up from 1.4937 ms, and 3.4863 ms in one catalog. A build runs once for each key that moves,
  and a rebuild takes at least 58.1 ms (22F.2). The extra build time is under 3 ms of that.

### 22F.2 Snapshot rebuild (T7 M1)

Command, at `b1df6ee5`, with no stack up:

```bash
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --release --features harness --test opa_rebuild_timing \
  -- --ignored --nocapture
```

Output: `RAW22F/rebuild-timing.out`. The test now does three more things:

- it times the scan a build now runs (`overlap_scan_profile`) beside the all-services scan
  (`overlap_scan`);
- it prints every rebuild sample, in run order;
- it prints `pg_stat_user_tables` for `policy.authz_policies` before and after each step.

It also has a fifth step, `1k_beside_10_profiles_again`: the 1k measurement, repeated after
the 10k one.

Rebuild p50, with min to max in brackets:

| Step | services | before (`RAW/rebuild-timing.out`) | after |
|---|---|---|---|
| `1k_alone` (902 statements) | 2 | 60.2 ms (57.3–101.0) | 71.7 ms (58.1–83.4) |
| `10k_uncompiled` (11 000) | 4 | 138.2 ms (127.5–151.9) | 106.9 ms (89.8–115.2) |
| `1k_beside_10_profiles` | 22 | 1044.8 ms (497.3–1152.3) | 739.9 ms (170.5–810.3), see 22F.3 |
| `10k_beside_10_profiles` | 22 | 505.6 ms (469.1–538.0) | 162.9 ms (145.3–197.5) |
| `1k_beside_10_profiles_again` | 22 | not run | 189.5 ms (164.4–369.9) |

The scan, timed alone, p50:

| Step | services | all services (the scan before 22F) | the profile's units (22F) |
|---|---|---|---|
| `1k_alone` | 2 | 34.8 ms | 31.4 ms |
| `10k_uncompiled` | 4 | 39.7 ms | 7.8 ms |
| `1k_beside_10_profiles` | 22 | 351.9 ms | 36.2 ms |
| `10k_beside_10_profiles` | 22 | 364.4 ms | 9.3 ms |
| `1k_beside_10_profiles_again` | 22 | 344.3 ms | 35.1 ms |

What this shows:

- **The scan no longer grows with the database.** At 22 services it takes 36.2 ms for the
  compiled profile and 9.3 ms for the uncompiled one. The all-services scan still measures
  351.9 ms and 364.4 ms, the same as Task 22's 351.5 ms and 371.7 ms.
- **A rebuild at 22 services** now takes 162.9 ms p50 for the 11 000-statement profile,
  against 505.6 ms before. For the 902-statement profile it takes 189.5 ms once Postgres has
  analyzed the table (22F.3), against 1044.8 ms before.
- **A moved key now stays inside the 1 s refresh period.** The largest sample was 810.3 ms,
  in the stale-statistics window of 22F.3. After that window, the largest was 369.9 ms.
- **The 2-service step did not get faster** (60.2 ms before, 71.7 ms after). Alone, the
  profile's units are almost all the units there are.

### 22F.3 The 902-statement profile rebuilt slower than the 11 000 one

Task 22 left this unexplained: at 22 services, the compiled 902-statement profile rebuilt at
1044.8 ms p50, with bimodal samples, while the uncompiled 11 000-statement profile rebuilt at
505.6 ms.

The new samples, in run order (`RAW22F/rebuild-timing.out`, line 28):
`810.3 772.3 747.0 710.7 739.9 742.5 172.7 170.5 175.2 211.2` ms.

- **Before that step**, `policy.authz_policies` had `n_mod_since_analyze=9840` and had never
  been analyzed (`last_autoanalyze=None`). The nine profiles arranged just before had
  inserted 9 × 984 policies.
- **After that step**, it had `n_mod_since_analyze=0` and
  `last_autoanalyze=2026-11-05 22:30:11.910619+00`. Autoanalyze ran during the step, and the
  samples fell from about 750 ms to about 170 ms at that point.
- **The two later steps both started on analyzed statistics.** The 11 000-statement profile
  measured 162.9 ms p50, and the repeated 902-statement step measured 189.5 ms p50.

**Finding.** The slow mode is the order the test runs in, not the profile's shape. The
902-statement step was the first step after a bulk insert of 9 840 policies, and it ran on
planner statistics that had never been taken. Once autoanalyze ran, it rebuilt at a speed
comparable to the 11 000-statement profile. Task 22's 11 000 step ran second, as it does
here, so it probably never met the stale statistics. That part is an inference: Task 22's
test did not print `pg_stat_user_tables`, so the statistics state of its run was not
recorded, only its bimodal samples (min 497.3 ms, p50 1044.8 ms).

**Not measured.** I did not find which query's plan changes, and I did not run the step
with a forced `ANALYZE`. The evidence is the time course, and a correlation in time is not a
proof of mechanism.

**What it means for production.** A bulk write, such as a migration or a large import, can
leave the refresher on stale statistics until autoanalyze runs. In this run, one rebuild
then cost up to 810.3 ms.

## Shared tag service (Task 26) (2026-11-08)

The question: when N profiles share one tag service and its tag version moves, how long
until the OPA refresher serves the last of the N profiles a snapshot at its new key? The
plan's threshold was 1 s at N = 10.

The raw outputs are in the measuring session's scratchpad,
`/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/latency/task26-shared-tag/`
(called `RAWT26/` below), next to `RAW/`.

### Machine and conditions

The machine is the one in "Machine" above (`RAWT26/machine.txt`): Intel i7-1068NG7, 8
logical CPUs, 4 performance cores, 32 GiB, macOS 26.7 (25G229), rustc 1.99.0, and
Postgres `egret_test` on localhost:5432. The harness gives each test process its own
database.

The machine was not idle. Docker's VM ran throughout at 70 % to 85 % of a core.
- The first "before" run started at load average 23.87 (`RAWT26/conditions-before.txt`).
- So I ran the unfixed code again later, at load average 6.54 to 11.44
  (`RAWT26/conditions-before-rerun.txt`).
- The two "after" runs ran at load averages 6.51 and 6.88 to 7.92 (`conditions-after.txt`,
  `conditions-pieces.txt`).

No other build or test suite of mine ran during a measurement. Another session's
`egret-policy` mutant run in the same worktree ended before the "after" runs started. I
waited for it, and checked that `crates/egret-policy` was unmodified.

### Command

```bash
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  CARGO_TARGET_DIR=$PWD/target-t26 \
  cargo test -p egret-perm-compile --release --features harness --test opa_rebuild_timing \
  -- --ignored --nocapture --exact opa_shared_tag_service_refresh_latency
```

The test is `opa_shared_tag_service_refresh_latency` in
`crates/egret-perm-compile/tests/opa_rebuild_timing.rs`, beside section 4's test, and it uses
the same `arrange`.

**Setup.**
- One tag service. N compiled Trino profiles name it as `tagService`; N grows 1, 5, 10, 20.
- Each profile is section 4's 820-table profile: 984 stored policies, on OPA, with its own
  engine and catalog.
- `ANALYZE` runs after each arrange, so no step measures on statistics that were never
  taken (22F.3).

**One run.**
1. One `UPDATE` moves `tag_version` on all N profiles and on the tag service, as
   `egret_catalog_registry::tag_version` does.
2. The clock starts.
3. `refresh_once` is driven directly. A thread polls `SnapshotCache::get` every 100 µs and
   records when each profile is `Ready` with a snapshot whose `key()` equals the new
   `(policy_version, tag_version)`.
4. "Last current" is the last of those moments.

There are 10 runs per N. The 1 s ticker is not running: the period's share is added below.

### Before the fix

At `c5274af1c`, output in `RAWT26/shared-tag-before-rerun.out` (and the busier
`shared-tag-before.out`). Times in ms, p50 with min to max in brackets.

| N | last profile current | first profile current | one rebuild | scoped overlap scan, alone |
|---|---|---|---|---|
| 1 | 60.8 (52.7–79.1) | 60.8 | 60.8 (52.7–79.1) | 36.5 |
| 5 | 1117.6 (926.0–1350.6) | 223.8 | 215.8 (177.5–306.5) | 168.0 |
| 10 | **4291.3 (3715.3–4638.1)** | 388.9 | 418.8 (339.3–632.8) | 346.5 |
| 20 | 18289.5 (16688.2–19658.8) | 873.7 | 887.9 (720.7–1549.1) | 820.4 |

The busier first run gave last-current p50s of 55.1, 1075.7, 3772.4 and 15123.7 ms, with
ranges 49.5–78.7, 1009.5–1380.0, 3708.7–4016.6 and 14869.6–16543.2.

**Why.**
- `stored_overlaps_of_via(p)` (the C8 re-check) resolved `{p, tag(p)}`.
- `units_for_service_via(tag)` then returned one unit per profile naming the tag service,
  and each unit was LOADED: 984 policies plus the tag service's.
- Only then were the pairs filtered down to `p`'s.

So one rebuild loaded N units, and a bump that moves N profiles loaded N² units. The scan
alone grew 36.5 → 168.0 → 346.5 → 820.4 ms, which is most of each rebuild.

### Decision: change the code

4291.3 ms at N = 10 is over the 1 s threshold, so I changed the code.

`stored_overlaps_of_via` now filters the unit ids before loading any of them. It keeps the
units whose pairs carry `p`, meaning the unit's `resource`, else its `tag_service`. That is
the same predicate the pairs were filtered by after loading, so the set and order of pairs
do not change. The filter is in `units_served_by_via`, in
`crates/egret-perm-compile/src/overlap.rs`.

For a profile on a shared tag service, that leaves one unit: `{p, tag(p)}`. Two other
effects:
- A unit that fails to load now fails only the profiles whose pairs it holds. Before, it
  failed every profile sharing the tag service. **This narrowing is deliberate, and no test
  covers it**: there is no seam to make one unit fail to load while the others load. It is
  safe for two reasons:
  - the units it skips hold only other profiles' pairs, which the old code loaded and then
    discarded, so they never changed this profile's verdict;
  - each skipped unit is still loaded by the build of the profile whose pairs it holds, so
    a unit that fails to load still leaves that profile `Unread`, as before.
- `stored_overlaps_via`, the all-services report, is unchanged.

**Tests.** `the_recheck_loads_only_the_profiles_own_unit_of_a_shared_tag_service` in
`crates/egret-perm-compile/tests/opa_cache.rs` checks three things:
- B and C share A's tag service, and each profile's re-check loads exactly its own unit;
- after C names A itself as its tag service, A still loads only `{A, tag(A)}`;
- each profile's scoped pairs equal the all-services scan's pairs for that profile. The
  test checks this first. A stores one pair (two tag masks of the shared tag service meeting
  on its `email`), B stores one (two path masks on one column), and the test asserts that
  neither list is empty.

**Mutant.** Remove the `.filter(…)` in `units_served_by_via`. Two tests fail:
- the new test. In the first round (`RAWT26/mutant-no-prefilter.out`), A's re-check got
  three units. After review fix round 1 (`RAWT26/mutant-no-prefilter-fix1.out`), the
  pairs-equality check fails first: A's scoped pairs also hold B's;
- `a_stored_overlap_counts_only_in_the_profile_whose_download_serves_it`: A was charged
  B's pair.

The full `opa_cache` suite passes with the change: 11 of 11 (`RAWT26/opa-cache-fixed.out`,
and `RAWT26/opa-cache-fix1.out` after fix round 1).

**C8 still refuses** a pair stored in a profile's own unit, including the unit it shares a
tag service with. `a_stored_overlap_counts_only_in_the_profile_whose_download_serves_it`
(same file) proves it:
- A's tag service serves MASK and MASK_NULL on one column, and A goes `NotBuilt`
  (`StoredStateRefused`);
- also when another profile names A as its tag service;
- a pair stored only in B, which shares A's tag service, leaves A `Ready`.

That test passes with the change and fails under the mutant above.

### After the fix

Output in `RAWT26/shared-tag-after.out`. A second run, `shared-tag-after-pieces.out`, also
times two pieces of one build.

| N | last profile current | first profile current | one rebuild | scoped overlap scan, alone |
|---|---|---|---|---|
| 1 | 52.2 (47.2–187.1) | 52.2 | 52.2 (47.2–187.1) | 32.9 |
| 5 | 392.0 (325.8–466.8) | 76.6 | 74.4 (63.1–105.2) | 33.1 |
| 10 | **1041.0 (932.8–1201.6)** | 103.2 | 100.8 (84.1–151.9) | 32.6 |
| 20 | 3507.5 (3052.0–4116.9) | 176.2 | 169.0 (129.4–328.7) | 35.7 |

The second run gave last-current p50s of 53.0, 403.3, **1184.9** and 3094.6 ms, with ranges
47.8–64.5, 358.8–424.2, 1081.1–1349.9 and 2935.8–3223.2.

**What this shows.**
- **N = 10 went from 4291.3 ms to 1041.0 ms** (1184.9 ms in the second run), and N = 20
  from 18289.5 ms to 3507.5 ms.
- **The scan is now flat**: about 33 ms at every N.
- **It is still not inside 1 s at N = 10.** The p50 is 41 ms over in one run and 185 ms over
  in the other.

**What still grows.** One rebuild still grows with N: 52 → 74 → 101 → 169 ms. The second run
breaks one build down (p50):

| N | `stored_state_refusals_via` | `snapshot_parts_via` | scoped scan |
|---|---|---|---|
| 1 | 7.0 | 24.4 | 32.5 |
| 5 | 5.1 | 30.5 | 31.9 |
| 10 | 5.0 | 70.2 | 37.3 |
| 20 | 3.6 | 113.9 | 33.9 |

`snapshot_parts_via` is what grows. Its loader, `ir_load::load_ep`, calls `list_grants_via`
and `list_rules_via`. These read EVERY grant and rule in the installation, because the
`perm` model is installation-wide, and the lowering then keeps only this profile's catalogs.

Here each profile adds 820 grants, so each rebuild reads 820 × N grants, and N rebuilds
still cost O(N²). The main term is now linear, about 100 ms × N at this size.

This cost does not depend on the shared tag service: any N profiles whose keys move
together pay it.

**The N axis also measures installation size.** In this test, N profiles are all the
profiles there are, so a larger N also means more grants and rules in the installation,
and each rebuild reads all of them. The 1 041 ms at N = 10 is therefore this installation's
figure: 10 profiles, 8 200 grants, nothing else.

In production, N = 10 profiles on one tag service can come out above or below 1 041 ms:
- above, in an installation with more profiles and grants beside them;
- below, with fewer or smaller ones.

I did not measure the effect of other profiles' size separately. I did not change it. `load_ep` is the Ranger compile's loader too, so
scoping it is a change to the compiler's input, outside this task. It is the next fix if a
shared tag service of 10 or more large profiles must go current within 1 s.

### Stale window: how long the surface answers from the previous snapshot

During a rebuild, `get` keeps returning the previous `Ready` snapshot. The entry is replaced
only when that profile's build ends (`SnapshotCache::apply`). So the surface keeps answering,
and nothing is denied for lack of a snapshot.

Profiles rebuild one at a time, in id order. Profile k of N therefore answers from its
previous snapshot for about k × (one rebuild). That formula is approximate: it multiplies
the p50 of one rebuild, and single rebuilds vary (the max column). The last profile answers
from its previous snapshot for the whole "last profile current" figure. With the change,
at this size:

| N | one rebuild p50 (each profile's own share) | last profile stale for, p50 (max) |
|---|---|---|
| 1 | 52 ms | 52 ms (187 ms) |
| 5 | 74 ms | 392 ms (467 ms) |
| 10 | 101 ms | 1041 ms (1202 ms) |
| 20 | 169 ms | 3508 ms (4117 ms) |

Before the change, the same last-profile window was 1.1 s at N = 5, 4.3 s at N = 10 and
18.3 s at N = 20.

22F.3 measured up to 810 ms for ONE rebuild, on planner statistics that had never been
taken. This test runs `ANALYZE` first, so its numbers exclude that state. In that state, one
rebuild cost up to 4 to 5 times more (22F.3: 810 ms against about 170 ms). Multiply the
table by about that factor for a refresh that follows a bulk write before autoanalyze.

**What an operator sees, with the 1 s ticker.**
- `refresh_once` was driven directly here. In production it runs on a `tokio` interval of 1 s
  (`REFRESH_PERIOD`, `MissedTickBehavior::Delay`), and a refresh lists the profiles' keys
  when it starts.
- A bump that lands while the refresher is idle waits for the next tick: between 0 and 1 s,
  0.5 s on average. Then the table's time follows.
- A bump that lands during a refresh waits for that refresh to finish, then for the next
  tick. With `Delay`, the next tick comes no sooner than 1 s after the late tick fired.
- So at N = 10 an operator sees the last profile current about 1.0 to 2.0 s after the bump
  (p50 rebuilds), and at N = 20 about 3.5 to 4.5 s. Before the change, that was 4.3 to 5.3 s
  at N = 10.

### Files

| File | What |
|---|---|
| `crates/egret-perm-compile/tests/opa_rebuild_timing.rs` | `opa_shared_tag_service_refresh_latency`, `#[ignore]`d |
| `crates/egret-perm-compile/src/overlap.rs` | `units_served_by_via`: the unit filter before loading |
| `crates/egret-perm-compile/tests/opa_cache.rs` | `the_recheck_loads_only_the_profiles_own_unit_of_a_shared_tag_service` |

## Follow-up 2026-11-09: the loader reads only the profile's catalogs

The section above left one cost that grew with N: `ir_load::load_ep` read every grant and
rule in the installation on every rebuild. This follow-up scopes those two reads. Raw output
is in the measuring session's scratchpad,
`/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/epscope/`
(called `RAWEP/` below). The machine is the same as above.

### Measure first

The harness now also times `load_ep` alone, in the same `REPEATABLE READ, READ ONLY`
transaction as the other pieces, and prints how many grants it read. Before the change,
at `main` 441413651 (`RAWEP/before.out`), p50 in ms:

| N | `snapshot_parts_via` | `load_ep` alone | grants read | last profile current |
|---|---|---|---|---|
| 1 | 12.1 | 9.9 | 820 | 60.9 (50.9–81.6) |
| 5 | 38.4 | 31.9 | 4 100 | 403.3 (353.3–440.1) |
| 10 | 61.2 | 55.9 | 8 200 | 954.2 (915.7–1050.1) |
| 20 | 126.8 | 110.1 | 16 400 | 3112.8 (2996.9–3183.1) |

So `load_ep` was almost all of `snapshot_parts_via`, and it grew with the grants it read,
which is 820 × N here. This run's N = 10 figure, 954 ms, is under the earlier run's 1 041 ms.
Its range still crosses 1 s.

### The change

`load_ep` now reads:
- **Grants:** `list_grants_for_catalogs_via`, the grants whose `catalog_id` is one of the
  catalogs the profile reaches (`catalogs_for_service_via`, read just before).
- **Rules:** `list_rules_for_catalogs_via`, every tag-selected rule, plus the path-selected
  rules on those catalogs.

Both functions are in `crates/egret-perm/src/store.rs`. They share row parsing with
`list_grants_via` and `list_rules_via`, which are unchanged and still serve the API listings.

**Why the compiled output does not change.**
- `lower_grants` skips a grant whose catalog is not in `ep.catalogs`.
- `lower_rules` skips a path rule whose catalog is not in `ep.catalogs`.
- A tag rule names no catalog, so every tag rule is still read.
- No other reader of the loaded grants or rules looks at a row outside those catalogs:
  - `refuse_overlaps` names only rules that produced a desired policy;
  - the tag-rule set reads only tag rules.

**Two side effects.**
- **Fewer usernames minted.** `expand_enumerated_principals` now expands only the principals
  of these rows. A compile therefore mints a username only for someone this profile's policies
  name. Before, it minted one for anyone named anywhere in the model. Anyone else is minted
  by the compile of a profile that does name them, and `premint_model_usernames` still mints
  across the whole model for a multi-profile recompile.
- **Row order.** The reads still have no `ORDER BY`, so rows come back in whatever order the
  plan returns them, as before. Grants on one catalog may now come back through
  `grants_catalog_idx` and not in heap order. The Ranger desired set is keyed by name. In the
  OPA snapshot, order only picks which statement a decision names as its reason; it never
  changes allow or deny.

**Test.** `the_loader_reads_only_the_grants_and_rules_its_catalogs_reach`, in
`crates/egret-perm-compile/tests/metadata_tables.rs`, sets up two Trino profiles, each on
its own catalog, with a grant and a path rule on each catalog and one tag rule. It checks that:
- each profile's load holds its own grant and path rule, and the tag rule;
- it holds neither the other profile's grant nor its path rule.

**Mutants**, each run against that test:

| Mutant | Result |
|---|---|
| M1: `load_ep` back on `list_grants_via` and `list_rules_via` | fails at "not the other catalog's grant" |
| M2: the `selector_kind = 'tag'` clause dropped from the rule read | fails at "every tag rule" |

### After

Output in `RAWEP/after.out`, p50 in ms:

| N | `snapshot_parts_via` | `load_ep` alone | grants read | one rebuild | last profile current |
|---|---|---|---|---|---|
| 1 | 14.8 | 11.9 | 820 | 49.8 | 49.8 (45.0–57.4) |
| 5 | 12.4 | 8.8 | 820 | 50.9 | 257.5 (236.1–273.6) |
| 10 | 14.2 | 9.5 | 820 | 47.4 | **493.0 (437.7–541.8)** |
| 20 | 11.7 | 8.8 | 820 | 46.9 | 971.2 (938.5–1028.7) |

**What this shows.**
- **One rebuild is flat**, at about 47 to 51 ms for every N. The refresh is now linear in N,
  about 50 ms per profile at this size.
- **N = 10 is inside 1 s**: 493 ms p50, 542 ms max. Before this change it was 954 ms in the
  same session.
- **N = 20**: 3 113 ms went to 971 ms.
- With the 1 s ticker (see "What an operator sees" above), an operator now sees the last of
  10 profiles current about 0.5 to 1.5 s after the bump.

### Files

| File | What |
|---|---|
| `crates/egret-perm/src/store.rs` | `list_grants_for_catalogs_via`, `list_rules_for_catalogs_via` |
| `crates/egret-perm-compile/src/ir_load.rs` | `load_ep` reads through them |
| `crates/egret-perm-compile/src/harness_support.rs` | `load_ep_model_ids_for_test` |
| `crates/egret-perm-compile/tests/metadata_tables.rs` | the scoping test |
| `crates/egret-perm-compile/tests/opa_rebuild_timing.rs` | the `load_ep` piece in `opa_shared_tag_service_refresh_latency` |
