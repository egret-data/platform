# Workflow follow-ups, group A: a timed-out StarRocks statement does stop; `http()` is refused for good

**Status:** designed 2026-10-17, not built. Every claim about egret's code was checked in the worktree `egret-grpa` at `4f6cafec` (main, after #683) and is cited `path:line` from the repository root.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (the note on names owned by no row) and §11 question 3.

**Decided with the user, 2026-10-17:**
- **StarRocks: no session variable; correct the docs and pin the measured behaviour** (§1.3, decided after the measurement in §1.2). No `KILL QUERY` here: an early cancel is part of the kill package (group B).
- **Plugin jars stay operator-curated.** This is the W5b ruling of 2026-09-29, already recorded in §11 question 3 of the programme spec: jars enter only through a reviewed PR to `plugin-host/plugins.lock` (Maven coordinate + sha256, checked at image build), the platform admin narrows them with `allowed-types`, and a workspace turns them on with `workflow_kestra_plugins`. A workspace never uploads a jar. Nothing is built for this item; it is listed so the decision is not reopened.
- **`http()` is refused permanently**, not "waiting for W9".

## 1. StarRocks: the docs say a timed-out statement keeps running, and it does not

### 1.1 What the code and docs claim

- `crates/egret-query/src/execution/connector/starrocks.rs:251` (`execute`) and `:306` (`execute_with_params`) never read `limits.timeout`. Every caller bounds the call with `tokio::time::timeout` at the same deadline and drops the future when it fires: `crates/egret-query/src/execution/port_impl.rs:366` (`run_bounded`, the Data API serving path and `egret.sql`), `crates/egret-query/src/execution/job.rs:222` (the JobManager), `crates/egret-query/src/execution/connector/mod.rs:189`, `crates/egret-query/src/registry/tester.rs:47`.
- Four places say that, after the drop, the statement **runs to completion on the engine**: `crates/egret-workflow/src/worker/handlers/sql.rs:557-565` (the `cancel()` doc) and `:579-582` (its error text: *"on StarRocks … it is not bounded by anything"*), `crates/egret-workflow/src/routes.rs:2225-2231` (the kill finding, which quotes that text), `starrocks.rs:300-305`, and `crates/egret-dataapi/src/serving/router.rs:45-49`.

### 1.2 What was measured

On the pinned `starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4…`, a private container, 2026-10-17 (plan Task 1; raw output in the task report):

- **egret's real path, unmodified connector:** `tokio::time::timeout(3 s, c.execute(LONG_SQL, …))` fired, the future was dropped, and a fresh connection found **no** row for the statement in `SHOW FULL PROCESSLIST` at +1 s, +5 s or +20 s. The same for `INSERT INTO probe.t SELECT …` through `execute`, and for `execute_with_params` (`WHERE ? = 1`, `Param::I64(1)`). After the drop the statement disappears from the processlist, consistent with the connection closing (the connector opens and disconnects its own `Conn` per call, `starrocks.rs:258`). The same happened when the `mysql` CLI client was killed.
- `LONG_SQL` = `SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000))`: it runs for longer than 70 s when nothing ends it.
- For reference, not adopted (§1.3): `query_timeout` (default 300) and `insert_timeout` (default 14400) both exist; `SET query_timeout = 2` ends `LONG_SQL` at 2 s with `ERROR 5024 (53400): Query reached its timeout of 2 seconds, …`; an `INSERT … SELECT` obeys only `insert_timeout` (`query_timeout = 2` alone let it run 159 s).
- Positive control: the three tests each assert they saw the statement running before the drop, so a probe that cannot see it cannot pass. Needles: `egret-deadline-probe` (SELECT), `INSERT INTO egret_drop_probe.t` (INSERT), `egret-params-deadline-probe` (parameterised SELECT, binary protocol).
- Probe hygiene: StarRocks strips `/* */` comments from `Info`, so a marker must be in the SQL text itself; a raw `mysql_async` probe connection needs `prefer_socket(false)`.

So the claim in §1.1 is false on 4.1.6, for every path egret uses. The objection that kept the session variable out (*"would begin failing queries that succeed today"*, `handlers/sql.rs:562-565`) is moot as well.

### 1.3 Decision and change

**Decided with the user, 2026-10-17: no session variable.** It would cost one more round trip on every StarRocks statement, the Data API serving path included, to guard against a leak that does not happen. What remains uncovered is a connection that is never closed cleanly (a network partition, a lost node), where the engine's own defaults bound the statement: 300 s for a query, 14400 s for an `INSERT`. That residue is stated, not fixed.

- **Correct every text listed in §1.1** to the measured behaviour: when egret stops waiting it drops the connection, and StarRocks 4.1.6 cancels the statement on its own. `handlers/sql.rs`'s `cancel()` still returns `Err` (it cannot stop a statement *before* the deadline, and that remains group B's), but its message no longer says the statement is unbounded. The error text must keep the words `QueryExecPort` and `StarRocks`: `cancel_refuses_rather_than_claiming_to_have_stopped_the_statement` (`handlers/sql.rs:1428`) asserts both. In `routes.rs`, the kill finding's point 7 is rewritten: an abandoned statement is not left running on StarRocks; the handler still cannot stop one early.
- **Pin the behaviour with a live test** so a StarRocks or `mysql_async` upgrade that changes it goes red instead of silently making the docs false again.

### 1.4 Tests

- Live, in `crates/egret-query/tests/connector_starrocks.rs` (gated on `EGRET_TEST_STARROCKS_URL`), three tests: `LONG_SQL` through `execute`, an `INSERT … SELECT` from `generate_series` into a scratch table through `execute`, and a `WHERE ? = 1` SELECT through `execute_with_params`. Each wraps the call in `tokio::time::timeout(3 s, …)`, asserts the outer timeout fired, then polls `SHOW FULL PROCESSLIST` on a fresh connection for up to 10 s and asserts the statement's text is gone. Each test also asserts it **saw** the statement in the processlist while it was running (polled before the deadline, on the fresh connection), so a probe that cannot see the statement cannot pass.
- The test is the instrument for a property that holds today, so its red run is shown by **mutation of the probe, not of the code**: point the processlist match at a string that is never in `Info` and the "saw it running" assertion must fail. A green run must show the test actually ran (not the `skip` line), because engine-gated tests skip silently.

## 2. `http()`: refused because of where it would run

### 2.1 What is true today

`crates/egret-workflow/src/runners/functions.rs:285` registers `http` as `Refusal::NotYet { needs: "an outbound HTTP client", package: "W9" }`, so an author reads "egret's W9 package provides" it. W9 shipped without it, and the programme spec says `http` is "still owned by no row" (`docs/en/specs/2026-09-01-workflow-programme-design.md:515`, `:541`).

Expressions render inside the egret server process. A request made from a render would leave from egret's control plane, so a flow author could reach anything egret's network reaches. That is server-side request forgery, and an allow-list would be a new security surface for a function no corpus flow uses.

### 2.2 Change

- New variant `Refusal::Unsupported(&'static str)` beside `NotYet` and `Confined` (`functions.rs:215`), rendered the same way as `Confined` (`pebble_error`, `functions.rs:206`).
- `http` uses it with this sentence: *"The 'http' function is not supported: an expression renders inside egret's control plane, so a request made from it would leave from egret's own network. Make the request in a script task, which runs in the task's container."*
- The test `the_refused_list_is_six_and_kv_is_not_in_it` (`functions.rs:3561`) moves `http` from the not-yet list to a checked `Unsupported` row, with all three argument shapes. The count stays six.
- The programme spec (EN and VI) records the decision at the two sentences cited above: *"`http` is refused on purpose (2026-10-17): it would run in the control plane."*

## 3. Scope

No migration, no frontend, no new config key. One PR with two commits, one per section. Out of scope: `KILL QUERY` and the kill route (group B), the remaining Kestra tasks (group C).
