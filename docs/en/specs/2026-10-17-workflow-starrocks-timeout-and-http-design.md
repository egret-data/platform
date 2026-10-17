# Workflow follow-ups, group A: StarRocks honours the timeout; `http()` is refused for good

**Status:** designed 2026-10-17, not built. Every claim about egret's code was checked in the worktree `egret-grpa` at `4f6cafec` (main, after #683) and is cited `path:line` from the repository root.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (the note on names owned by no row) and §11 question 3.

**Decided with the user, 2026-10-17:**
- **StarRocks gets a server-side deadline, on every caller, as a session variable.** No `KILL QUERY` here: a real cancel is part of the kill package (group B).
- **Plugin jars stay operator-curated.** This is the W5b ruling of 2026-09-29, already recorded in §11 question 3 of the programme spec: jars enter only through a reviewed PR to `plugin-host/plugins.lock` (Maven coordinate + sha256, checked at image build), the platform admin narrows them with `allowed-types`, and a workspace turns them on with `workflow_kestra_plugins`. A workspace never uploads a jar. Nothing is built for this item; it is listed so the decision is not reopened.
- **`http()` is refused permanently**, not "waiting for W9".

## 1. StarRocks: the engine stops a statement egret has given up on

### 1.1 What is true today

- `crates/egret-query/src/execution/connector/starrocks.rs:251` (`execute`) and `:306` (`execute_with_params`) never read `limits.timeout`; the doc at `:300` says so.
- Every caller already bounds the call with the same deadline, so egret stops waiting at `limits.timeout`:
  - `crates/egret-query/src/execution/port_impl.rs:366` (`run_bounded`, behind `execute_as` and `execute_as_with_params`: the Data API serving path and `egret.sql`);
  - `crates/egret-query/src/execution/job.rs:222` (the JobManager: SQL Lab, charts, alerts);
  - `crates/egret-query/src/execution/connector/mod.rs:189` and `crates/egret-query/src/registry/tester.rs:47`.
- Dropping the future drops the MySQL connection, which tells StarRocks nothing: the statement runs to completion on the engine (`crates/egret-workflow/src/worker/handlers/sql.rs:557-565`).
- Trino already sends `query_max_run_time` from the same `limits.timeout` (`crates/egret-query/src/execution/connector/trino.rs:490`).

### 1.2 Why the old objection no longer holds

`handlers/sql.rs:562-565` says the session variable was not set because the connector is shared with the Data API serving path "and would begin failing queries that succeed today". That is false on current main: every caller listed above already reports a timeout at `limits.timeout`. egret's clock starts before the connect and the engine's starts when the statement arrives, so the engine's deadline is never earlier than egret's. The variable can only stop work egret has already abandoned.

### 1.3 Change

- A private helper in `connector/starrocks.rs` sends `SET query_timeout = N` (and `insert_timeout = N`, see below) on the connection, where `N = ceil(limits.timeout in seconds)` with a minimum of 1.
- It runs in `execute` and `execute_with_params`, after the connect and after `switch_identity`, before the statement. Each call opens its own `Conn` (`starrocks.rs:258`) and disconnects it at the end, so the variable cannot reach another statement.
- If the `SET` fails, the call returns `EngineError::Engine` and the statement is not sent. Running without a deadline is what this fixes, so failing open is not acceptable.
- `insert_timeout`: StarRocks uses it in place of `query_timeout` for `INSERT` in recent versions. Task 1 of the plan measures on the pinned StarRocks 4.1.6 whether the variable exists and whether an `INSERT … SELECT` obeys it. If it does not exist, only `query_timeout` is set and the measurement is recorded here.
- **Measured 2026-10-17 on 4.1.6:** `insert_timeout` exists (default 14400; `query_timeout` default 300), and `SET query_timeout = 2, insert_timeout = 2` in one statement succeeds.
- **Measured 2026-10-17 on 4.1.6:** `LONG_SQL` = `SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000))`. With `query_timeout = 70` it ran the full 70 s and was ended by the deadline (so it runs for more than 60 s); with `query_timeout = 2` it ended after 2 s. `SELECT sleep(120) AS s, 'egret-deadline-probe' AS marker` behaves the same (2 s).
- **Measured 2026-10-17 on 4.1.6:** the error text is `ERROR 5024 (53400): Query reached its timeout of 2 seconds, please increase the 'query_timeout' session variable and retry`. For an `INSERT` it is `ERROR 5024 (53400): Insert reached its timeout of 2 seconds, please increase the 'insert_timeout' session variable or the 'timeout' property for insert statement and retry`.
- **Measured 2026-10-17 on 4.1.6:** the engine ends the statement: with `query_timeout = 2` the marker is in `SHOW FULL PROCESSLIST` at 1 s and gone at 5 s.
- **Measured 2026-10-17 on 4.1.6:** `INSERT INTO ... SELECT` is governed by `insert_timeout`, not `query_timeout`. With `query_timeout = 600, insert_timeout = 2` it stopped after about 3 s; with `query_timeout = 2, insert_timeout = 600` it ignored the 2 s and ran to completion (159 s). Both variables must therefore be set.
- **Measured 2026-10-17 on 4.1.6 (caveat):** when the client process is killed with no `SET` (a clean socket close), the statement also disappeared from `SHOW FULL PROCESSLIST` within 2 s. The "statement keeps running" failure therefore does not reproduce on a closed socket; it only applies while the connection stays open (for example a pooled connection whose future was dropped).
- `describe` and `describe_with_params` are out of scope: they wrap their own `tokio::time::timeout` (`starrocks.rs:389`, `:451`) and send result-metadata queries only.
- Docs that state the old behaviour are corrected in the same commit: `handlers/sql.rs` (`cancel()`'s doc and its error text), `starrocks.rs:300-305`, and the module doc of `crates/egret-dataapi/src/serving/router.rs:45-49`.

### 1.4 Tests

- Unit: `N` rounds up (1.2 s gives 2), never goes below 1 (200 ms gives 1), and a whole second stays the same (30 s gives 30).
- Live, in `crates/egret-query/tests/connector_starrocks.rs` (gated on `EGRET_TEST_STARROCKS_URL`): start a statement that takes far longer than 2 s, call it with `timeout = 2 s` directly on the connector (no outer wrapper, so only the engine can end it), and assert that it returns an engine timeout error. Then, on a second connection, check `SHOW PROCESSLIST` for the statement's text and assert it is gone within the deadline plus a margin.
- Mutation: remove the `SET`; the live test must go red. A green run must show the test actually ran (not the `skip` line), because engine-gated tests skip silently.

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
