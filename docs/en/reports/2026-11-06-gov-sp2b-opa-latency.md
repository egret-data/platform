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

1. **Latency.** Inside the egret process, the OPA path costs about as much as Ranger's
   in-JVM plugin. On `SHOW TABLES` over 1 003 tables, analysis takes 19.0 ms p50 on OPA and
   10.5 ms on Ranger. Wall time is 292.6 ms against 274.5 ms. On the other two statements,
   the wall times are within the run-to-run spread.
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

**Quiet machine.** No other test suite or build ran during the benches or the end-to-end
run. Other people's compose projects (`egret-demo`, `egret-dev-*`, the docs Trino) stayed up,
as the Docker rules require. `docker stats --no-stream` before the runs showed their largest
CPU use at 11.72 % (the docs Trino coordinator). The Docker VM process itself was at
189.1 % CPU in `ps` at that moment.

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

Every bench asks about the same objects at both sizes. The value shown is criterion's middle
estimate, with its interval in brackets.

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

The 500 columns of `wide` cost one `/batch_column_masks` request, not 500. The 1 018
`/batch` requests on `information_schema.columns` are one `FilterColumns` per table.

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
- egret answered every request it received: no `busy`, `in flight` or `being parsed` line
  in its log, and `egret_opa_error_total` and `egret_opa_unread_total` stayed absent;
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

Start-to-start gaps:

| Run | trino-opa p50 | trino-opa max | trino-ranger p50 | trino-ranger max |
|---|---|---|---|---|
| 1 | 241 ms | 1273 ms | 235 ms | 307 ms |
| 2 | 242 ms | 1700 ms | 233 ms | 426 ms |
| 3 | 234 ms | 1224 ms | 233 ms | 291 ms |

"First to last failure" is 0.00 s whenever there was a failure: only one statement failed.
The failed statement failed within 86 ms and 76 ms. In trino-opa's log of run 1
(`RAW/repro-unbatched-trino-opa.log`) it shows
`Server refused connection: http://egret:8080/opa/v1/data/egret/trino/allow` with
`java.net.ConnectException: Connection refused`.

The statement after it took 1160 ms and 1116 ms. In run 2 no statement failed, but two took
1591 ms and 1234 ms (from `awk` over `RAW/*/restart-trino-opa.txt`). So egret's restart shows up
as one refused statement, or as a stalled one (1591 ms). trino-ranger, whose plugin
evaluates its own cached policies, saw neither.

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
