# W11 — Execution logs, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every log line a task produces is stored in `workflow.task_logs` and can be read back through a paginated cursor API while the execution runs. That includes `Log`/`Fail` lines, handler messages, and container/python stdout and stderr.

**Architecture:**
- `LogSink` becomes async. The worker builds one `RunLog` handle per task-run attempt and hands it to the handler on `TaskInput`. The handle carries the run's identity, the truncation rule and the per-attempt caps.
- A `PgLogSink` sends lines into a bounded channel with `send().await`. A background writer in the runtime inserts them in batches.
- An hourly ticker prunes lines older than the retention window.
- A cursor route serves the lines, with a 2 s settle lag.

**Tech Stack:** Rust, tokio (`mpsc`, `CancellationToken`), sqlx runtime queries, Postgres, axum + utoipa, `async_trait`.

**Spec:** `docs/{en,vi}/specs/2026-09-27-workflow-w11-execution-logs-design.md`. **Section numbers below (§n) are the spec's.**

**A warning about this document.** Code blocks are shapes to verify, not text to paste. The plan is wrong wherever it disagrees with the code, and an implementer who finds that says so in the report.

## Global Constraints

- **Migration `0159`** (`migrations/0159_workflow_task_logs.sql`). `origin/main` ends at `0158`. Re-check `main` and every open PR branch before naming the file. **Never edit a migration that has already been applied anywhere shared.**
- **Limits and defaults, verbatim from the spec:**
  - line cut at **65,536 bytes** on a UTF-8 boundary, with `truncated = true`;
  - per-attempt cap of **10,000 lines** or **10,485,760 bytes**;
  - exactly **one WARN summary** line, written outside the cap;
  - channel capacity **10,000**;
  - batch of **500 lines** or **1 s**;
  - write retry at 100 ms doubling, capped at 5 s, for **30 s** per batch;
  - shutdown drain bounded at **10 s**;
  - retention default **30 days** (`0` means keep forever);
  - prune every **hour**, in batches of **10,000**, with a **30 s** pass budget;
  - API `limit` default **500**, max **2,000**;
  - settle lag **2 s**.
- **Config keys** on `egret_config::WorkflowConfig`: `log_retention_days`, `log_line_max_bytes`, `log_run_max_lines`, `log_run_max_bytes`. The writer and the pruner run when `worker_enabled` is true.
- **A log write never changes a run's outcome.** A write that fails is reported through `tracing::error!`, never through the `Outcome`.
- **No line is dropped silently.** A full channel makes the writer wait (`send().await`), never `try_send`. Only an exhausted retry drops a batch, and that is logged with its count.
- **Handler registry scope is unchanged** (user decision, 2026-09-27). Production's `Handlers::with_core_tasks()` stays `Log`/`Return`/`Fail`/`Sleep`. Container and python are exercised through test registries, and the spec says production does not run them yet.
- **Levels follow Task 1's measurement.** Where the measurement and Kestra's Java disagree, the measurement wins and the disagreement is written down.
- **The ratchet cannot judge this package. Expected: 109 before and after, measured.**
- **Mutation discipline:**
  - Every mutation shows evidence that it applied (`git diff --stat` plus a token count), a run count, an unmutated control, and the assertion that killed it.
  - A mutant that fails to compile proves nothing; it prints no `test result:` line.
  - A surviving mutant is reported as unproved.
  - Never commit a mutant, and never apply one while any build or test is running.
- **A zero is reported beside a number that can disagree.** A sweep is reported as N examined / M corrected, per population, using `/usr/bin/grep` with file arguments.
- **Commits:**
  - Conventional Commits, made with `git -c user.email=hoangquocvietbk2.13@gmail.com`.
  - Trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - Check `git branch --show-current` before every commit; never `git checkout` a commit.
  - Never commit `.superpowers/`. No PRs; the controller opens them.
- **Environment:**
  - The controller sets up a dedicated Postgres for this branch on its own port.
  - `DATABASE_URL` points at that Postgres.
  - `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030`, `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` (shared).
  - Never run two cargo test processes at once, and check `pgrep -lx cargo` first.
  - Run long cargo commands in the background and poll them.
  - If free disk drops under 10 G, stop and report.

## File Structure

```
migrations/0159_workflow_task_logs.sql                NEW   the table (§3)
crates/egret-config/src/…                             MOD   four WorkflowConfig keys (§7)
crates/egret-workflow/src/
  logs/mod.rs                                         NEW   LogLine, LogLevel, LogLimits; re-exports
  logs/run_log.rs                                     NEW   RunLog: identity, truncation, caps, finish() (pure, no sqlx)
  logs/repo.rs                                        NEW   LogRepo: insert_batch, page (cursor + lag), prune_batch
  logs/writer.rs                                      NEW   PgLogSink + writer task (channel, batch, retry, drain)
  logs/prune.rs                                       NEW   the retention ticker
  worker/handlers/mod.rs                              MOD   LogSink async; TracingSink; Recorder
  worker/handlers/{log,fail,container,python}.rs      MOD   write through task.log
  worker/commands.rs                                  MOD   ingest_line's lines go to task.log; stderr level per Task 1
  worker/mod.rs                                       MOD   TaskInput gains `log: RunLog`
  worker/claim.rs                                     MOD   run_one builds the RunLog, finish() on every path
  worker/registry.rs                                  MOD   Handlers carries the sink
  runtime.rs                                          MOD   spawn the writer and the pruner; shutdown ordering
  routes.rs                                           MOD   GET …/executions/{id}/logs (§6)
crates/egret-bin/src/main.rs                          MOD   pass the sink and the config
crates/egret-workflow/tests/
  task_logs.rs                                        NEW   repo, writer, pruner, route, end to end
crates/egret-workflow/oracle-exec/w11-logs.json       NEW   Task 1's measurements
frontend/src/api/generated/…                          MOD   `npm run gen:api` (Task 5)
```

`logs/` is a new module because none of today's modules owns log storage. `worker/` owns running tasks and `routes.rs` owns HTTP. If the implementer finds a better home by the codebase's own pattern, they say so in the report.

---

## Task 1: Measure log behaviour on the artifact

**Files:**
- Create `crates/egret-workflow/oracle-exec/w11-logs.json`.
- Amend spec §4.4 and §5 (EN and VI) only where a measurement contradicts them.

**Interfaces:** produces the fixture that Task 3's tests load with `include_str!`, and the level table Task 3 implements.

- [ ] **Step 1:** Bring up the pinned artifact (`crates/egret-workflow/oracle-exec`, `docker compose up -d`; the API base and auth are in `harvest.py`).
  - **Known hazard, measured in W13b:** `docker exec … psql` running as a child of the postmaster died of SIGPIPE and crashed Postgres. To read Kestra's database, use a separate container on the compose network.
- [ ] **Step 2: Deploy flows and read each execution's logs** through Kestra's `GET /api/v1/{tenant}/logs/{executionId}`. Record the raw JSON for each question:
  1. A script task (`io.kestra.plugin.scripts.shell.Commands`, or whatever script task the image carries) that prints one stdout line and one stderr line: **the level of each**. Repeat with `warningOnStdErr: false` if the property exists.
  2. The same task printing `::{"outputs":{"k":"v"}}::`: **does the marker line appear in the log?** Does `k` appear in outputs?
  3. `##kestra:log:debug##` / `##kestra:log:info##` brackets around a line: **the level of the bracketed line**, and whether the markers themselves appear.
  4. A `core.log.Log` task, and a `Fail` task: **every line** the execution's log holds, including any framing lines the worker adds (for example "started"/"ended"). For each line record `taskId`, `taskRunId` and `attemptNumber`.
  5. A script task with `retry: {type: constant, interval: PT1S, maxAttempts: 2}` that fails once: **the `attemptNumber` on each attempt's lines**.
  6. A line of 100,000 characters: stored whole or cut? (Kestra is expected not to cut; record what happens.)
- [ ] **Step 3: Save** every observation verbatim with timestamps, the image digest, and a `findings` array that quotes the value each finding came from. Add an `egret_expected` entry per finding that later tasks assert: level per stream, whether marker lines are stored, whether framing lines exist, how `attempt` numbers. If something cannot be measured, say so and name what egret will do instead; the spec is amended to "chosen".
- [ ] **Step 4:** `docker compose down -v`, then commit: `docs(workflow): W11 Task 1 — log behaviour on the artifact`.
- [ ] **Step 5: Baseline gate** at the branch base, after the artifact is down.
  - Run `cargo test -p egret-workflow` and save the name/result list to the SDD workspace as `baseline-gate.txt`.
  - Report passed/failed/ignored and the failing names. The expected failures are the StarRocks-live environment ones.

---

## Task 2: Migration `0159` and `LogRepo`

**Files:**
- Create `migrations/0159_workflow_task_logs.sql` and `crates/egret-workflow/src/logs/{mod.rs,repo.rs}`.
- Tests go in `crates/egret-workflow/tests/task_logs.rs`.

**Interfaces:**

```rust
// logs/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub enum LogLevel { Trace = 0, Debug = 1, Info = 2, Warn = 3, Error = 4 }   // replaces handlers::Level

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub workspace_id: Uuid, pub execution_id: Uuid,
    pub task_run_id: Option<Uuid>, pub task_id: Option<String>, pub attempt: Option<i32>,
    pub level: LogLevel, pub ts: DateTime<Utc>, pub message: String, pub truncated: bool,
}

// logs/repo.rs
pub struct LogRepo { pool: PgPool }
impl LogRepo {
    pub fn new(pool: PgPool) -> Self;
    pub async fn insert_batch(&self, lines: &[LogLine]) -> Result<u64>;          // one INSERT … SELECT unnest(…)
    pub async fn page(&self, ws: Uuid, execution: Uuid, q: &LogQuery) -> Result<Vec<StoredLine>>;
    pub async fn prune_batch(&self, older_than_days: i32, batch: i64) -> Result<u64>;
}
pub struct LogQuery { pub after: Option<i64>, pub limit: i64, pub task_id: Option<String>,
                      pub task_run_id: Option<Uuid>, pub min_level: Option<LogLevel> }
pub struct StoredLine { pub id: i64, /* LogLine's public fields minus workspace/execution */ }
```

`page` returns rows with `workspace_id = $ws AND execution_id = $execution AND id > $after AND inserted_at < now() - interval '2 seconds'` plus the optional filters, ordered by `id`, limited.

- [ ] **Step 1: Verify the spec's schema against the code** and write down each decision in the migration's header comment:
  - the real type of `workflow.executions.id`;
  - whether an FK on `workspace_id` is the house pattern (look at `0157`/`0158`);
  - `task_run_id`: **no FK**. The worker holds `task_runs` rows `FOR UPDATE`/`FOR NO KEY UPDATE` while the writer inserts, and an FK check would take `FOR KEY SHARE` on them. W13a measured a `40001` from exactly that shape. Confirm the lock argument, or refute it with a measurement.
- [ ] **Step 2: Failing tests**, using `egret_test_db` the way `tests/flow_trigger_windows.rs` does:
  - `insert_batch_round_trips_every_field`: including `NULL` `task_run_id`/`task_id`/`attempt`, a multi-byte message, and `truncated = true`.
  - `page_returns_nothing_younger_than_the_settle_lag`: insert, read immediately, get 0 rows. Backdate `inserted_at` by 3 s with SQL, read again, get 1 row.
  - `page_is_ordered_by_id_and_honours_after_and_limit`.
  - `page_filters_by_task_id_task_run_id_and_min_level`.
  - `page_never_returns_another_workspaces_lines`: same execution id queried with the wrong workspace gives 0 rows.
  - `out_of_order_commits_are_not_skipped_by_a_polling_reader`:
    - tx A inserts and holds; tx B inserts (higher id) and commits;
    - the reader polls with the lag disabled by backdating B and gets B, with its cursor now past A's id;
    - A commits.
    - The test then shows that **with the lag**, a reader that polls only after both are settled gets both in order.
    - Write the scenario so the mutation "drop the lag predicate" makes it fail.
  - `prune_deletes_only_lines_older_than_the_window`, and `prune_with_zero_days_is_never_called_by_the_ticker` (the latter lives in Task 4; here, `prune_batch(0, …)` is documented as *deletes everything older than now*, so the ticker must not call it with `0`).
  - `deleting_an_execution_cascades_its_lines`.
- [ ] **Step 3: Implement.** Run the tests to green.
- [ ] **Step 4: Mutations:**

| mutation | must kill |
|---|---|
| drop `inserted_at < now() - 2s` | `page_returns_nothing_younger_than_the_settle_lag`, `out_of_order_commits_…` |
| drop `workspace_id = $ws` | `page_never_returns_another_workspaces_lines` |
| `id >= $after` | `page_is_ordered_by_id_and_honours_after_and_limit` |
| drop the `ts <` predicate in prune | `prune_deletes_only_lines_older_than_the_window` |

- [ ] **Step 5: Commit:** `feat(workflow): W11 — workflow.task_logs and LogRepo`.

---

## Task 3: `RunLog`, an async `LogSink`, and the handlers on it

**Files:**
- Create `crates/egret-workflow/src/logs/run_log.rs`.
- Modify `worker/handlers/{mod,log,fail,container,python}.rs`, `worker/commands.rs`, `worker/mod.rs` (`TaskInput`), `worker/claim.rs` (`run_one`) and `worker/registry.rs`.

**Interfaces:**

```rust
// worker/handlers/mod.rs
#[async_trait::async_trait]
pub trait LogSink: Send + Sync + std::fmt::Debug {
    async fn write(&self, line: LogLine);
}
pub struct TracingSink;            // unchanged role: writes to `tracing`
// testing::Recorder keeps its role; it now records LogLine.

// logs/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogLimits { pub line_max_bytes: usize, pub run_max_lines: u64, pub run_max_bytes: u64 }
impl Default for LogLimits { /* 65_536, 10_000, 10_485_760 */ }

// logs/run_log.rs
#[derive(Clone)]
pub struct RunLog { /* identity + Arc<dyn LogSink> + LogLimits + Arc<Mutex<Counters>> */ }
impl RunLog {
    pub fn new(identity: RunIdentity, sink: Arc<dyn LogSink>, limits: LogLimits) -> Self;
    pub fn discard() -> Self;                                   // TracingSink-backed; tests and non-run callers
    pub async fn line(&self, level: LogLevel, message: &str);
    pub async fn info(&self, message: &str);
    pub async fn finish(&self);                                 // writes the summary once, if anything was withheld
}
impl std::fmt::Debug for RunLog { /* identity only */ }
impl PartialEq for RunLog { /* identity only: TaskInput derives PartialEq */ }

// worker/mod.rs
pub struct TaskInput { /* … */ pub log: RunLog }

// worker/registry.rs
impl Handlers {
    pub fn with_core_tasks() -> Self;                            // unchanged set
    pub fn with_log_sink(self, sink: Arc<dyn LogSink>, limits: LogLimits) -> Self;
    pub fn log_sink(&self) -> (Arc<dyn LogSink>, LogLimits);     // run_one reads it through Registry
}
// Registry gains: fn log_sink(&self) -> (Arc<dyn LogSink>, LogLimits); default: TracingSink + defaults.
```

**Decision taken here: the handle rides on `TaskInput`.** `cancel::guarded(reg, &input, budget)` and `Registry::run(&TaskInput)` then need no new parameter, and the `sink` field plus the `with_sink` test hook on each handler go away. A handler writes with `task.log.line(level, msg).await`. `NameHintSink` becomes a thin observer: python's runner passes each line to the observer and then to `task.log`. Keep its behaviour and its tests.

**Truncation:** cut at the greatest `char` boundary ≤ `line_max_bytes` and set `truncated`. The cut line counts toward the byte cap at its **stored** length.

**Cap:** the first line that would make `lines > run_max_lines` or `bytes > run_max_bytes` flips `withholding = true`. Later lines only increment `withheld_lines`/`withheld_bytes`. `finish()` writes, once, if `withheld_lines > 0`:

> *{N} lines ({M} bytes) of this attempt were not stored: over the per-run log cap ({run_max_lines} lines / {run_max_bytes} bytes).*

It is written at `WARN`, bypassing the cap. A second `finish()` is a no-op.

**`run_one`:**
- Build `RunLog` from `reg.log_sink()` and the run's identity (`workspace_id`, `execution_id`, `run_id`, `task_id`, `attempt`), inside `input_for` or right after it.
- Call `input.log.finish().await` after the `match budget {…}`, **before** `pool.begin()`, on every path. That includes the `Budget::Refused` arm and the `?` error path: make the `Err` path call `finish()` too (for example by capturing the `Result` and finishing before returning it).

**Levels:** apply Task 1's `egret_expected` table in `commands::ingest_line` (stderr level, debug bracket) and in the marker-line rule. Rewrite the `commands.rs` stderr comment to cite the measurement instead of `warningOnStdErr`.

- [ ] **Step 1: Failing unit tests** in `run_log.rs`, with `Recorder`:
  - `a_line_is_written_with_the_runs_identity`.
  - `a_long_line_is_cut_on_a_char_boundary_and_marked` (a 4-byte char straddling the limit).
  - `the_cap_stops_storing_and_finish_writes_one_summary`: `run_max_lines = 3`, 5 lines, so 3 are stored plus 1 WARN summary naming 2 lines.
  - `the_byte_cap_counts_stored_length`.
  - `finish_is_idempotent_and_silent_under_the_cap`.
  - `a_retry_attempt_starts_fresh`.
- [ ] **Step 2: Failing tests for the wiring:**
  - `run_one_finishes_the_log_on_error_timeout_and_cancel`: a fake registry whose handler logs past a 1-line cap, then returns `Err`, then expires under a 1 ms budget; the summary appears in all three.
  - A container test (the fake `CommandRunner` style already in `container.rs` tests) asserting stdout and stderr arrive on `task.log` at Task 1's levels, and marker lines per Task 1.
  - The existing `log.rs`/`fail.rs`/`python.rs` sink tests, moved onto `Recorder` through `TaskInput.log`.
- [ ] **Step 3: Implement.** Keep `src/trigger/` untouched. Run `cargo test -p egret-workflow --lib` plus `worker`, `container_docker_live` (if docker is up) and `end_to_end`.
- [ ] **Step 4: Mutations:**

| mutation | must kill |
|---|---|
| cut at `line_max_bytes` bytes, not a char boundary | `a_long_line_is_cut_on_a_char_boundary_and_marked` (panics or mismatches) |
| summary counted inside the cap (written only if under cap) | `the_cap_stops_storing_…` |
| `finish()` only on the `Ok` path of `run_one` | `run_one_finishes_the_log_on_error_timeout_and_cancel` |
| container handler writes to `TracingSink` instead of `task.log` | the container wiring test |
| stderr level swapped to Task 1's other value | the container wiring test |

- [ ] **Step 5: Commit:** `feat(workflow): W11 — RunLog, an async LogSink, and every handler writing through it`.

---

## Task 4: `PgLogSink`, its writer, the retention ticker, and wiring

**Files:**
- Create `logs/writer.rs` and `logs/prune.rs`.
- Modify `runtime.rs`, `egret-config`'s `WorkflowConfig`, and `crates/egret-bin/src/main.rs`.
- Tests go in `tests/task_logs.rs`.

**Interfaces:**

```rust
// logs/writer.rs
#[derive(Clone, Debug)]
pub struct PgLogSink { tx: mpsc::Sender<LogLine> }
#[async_trait] impl LogSink for PgLogSink { async fn write(&self, line: LogLine) { /* tx.send(line).await; on Err (closed) → tracing::warn once */ } }
pub struct WriterConfig { pub capacity: usize, pub batch: usize, pub flush_every: Duration,
                          pub retry_for: Duration, pub drain_within: Duration }   // 10_000, 500, 1s, 30s, 10s
pub fn spawn_writer(repo: LogRepo, cfg: WriterConfig, cancel: CancellationToken) -> (PgLogSink, JoinHandle<()>);

// logs/prune.rs
pub struct PruneConfig { pub retention_days: i32, pub every: Duration, pub batch: i64, pub pass_budget: Duration }
pub fn spawn_pruner(repo: LogRepo, cfg: PruneConfig, cancel: CancellationToken) -> Option<JoinHandle<()>>;  // None when retention_days == 0

// runtime.rs: RuntimeConfig gains `log_limits: LogLimits`, `log_retention_days: i32`;
// spawn() builds the writer + pruner when `enabled`, passes the sink into the registry,
// and on cancel stops the worker ticker FIRST, then lets the writer drain.
```

**Registry plumbing:** `runtime::spawn` takes `Arc<dyn Registry>` today. Wrapping the sink into an already-built `Arc<Handlers>` is not possible. Choose the smallest change that keeps `egret-bin` the one place deciding the registry. For example, `egret-bin` builds the writer through a `runtime` helper and calls `Handlers::with_core_tasks().with_log_sink(sink, limits)` before `Arc::new`. Say which in the report.

**Shutdown order:** the worker ticker must stop claiming before the writer drains. Otherwise a run still in flight writes into a closed channel. `PgLogSink::write` on a closed channel logs once and drops that line; this is the "process is stopping" case in spec §11.

- [ ] **Step 1: Failing tests:**
  - `lines_reach_postgres_in_order_within_a_flush`: write 3 lines; after `flush_every` plus the 2 s settle lag, `page` returns them in order.
  - `a_full_channel_makes_the_writer_wait_and_loses_nothing`:
    - capacity 4, and a repo whose insert is slowed. Slow it with a `pg_advisory_lock` held by the test, which the insert statement waits on through a trigger, or with an injected `Inserter` trait: pick the smaller.
    - Write 20 lines from a task.
    - Assert the task's `write` calls were pending (not all done) while the lock was held.
    - Assert all 20 stored after release.
  - `a_failing_batch_is_retried_then_reported_not_fatal`: the insert fails (for example with the table renamed in a transaction the test controls, or an injected failing `Inserter`); the writer keeps running and later batches land.
  - `shutdown_drains_what_is_in_the_channel`: write 50 lines, cancel immediately, await the handle; all 50 are stored.
  - `the_pruner_deletes_old_lines_in_batches_and_stops_on_cancel`, and `the_pruner_is_not_started_at_zero_days`.
  - `end_to_end_a_flow_logs_through_the_real_runtime`:
    - publish a flow with a `Log` task (INFO) and a `Fail` task;
    - start it through the runtime (`worker_enabled`, no harness driving the tick), with the pattern `tests/runtime.rs` uses;
    - read the lines through `LogRepo::page` after the lag;
    - both tasks' lines are there with the right `task_id`, `attempt` and level.
- [ ] **Step 2: Implement.** Add the four config keys with doc comments in the house style (see `worker_enabled`'s) and their defaults, and wire them in `egret-bin`.
- [ ] **Step 3: Mutations:**

| mutation | must kill |
|---|---|
| `try_send` that drops instead of `send().await` | `a_full_channel_makes_the_writer_wait_and_loses_nothing` |
| exit on cancel without draining | `shutdown_drains_what_is_in_the_channel` |
| a failed batch ends the writer loop | `a_failing_batch_is_retried_then_reported_not_fatal` |
| pruner started at `0` days | `the_pruner_is_not_started_at_zero_days` |
| writer not passed into the registry (TracingSink stays) | `end_to_end_a_flow_logs_through_the_real_runtime` |

- [ ] **Step 4: Full crate gate** (`cargo test -p egret-workflow`), reconciled by test-name-set diff against `baseline-gate.txt`. Commit: `feat(workflow): W11 — the log writer, the retention ticker, and their wiring`.

---

## Task 5: The read API

**Files:**
- Modify `crates/egret-workflow/src/routes.rs`.
- Modify the OpenAPI registration wherever W10's execution routes are registered (find `executions/{id}` in the utoipa `paths(…)`).
- Update the committed `openapi.json` if the repo commits one (`committed_openapi_json_is_current` exists in `egret-server`).
- Regenerate `frontend/src/api/generated/…` with `npm run gen:api`, **never bare orval**.
- Tests go in `tests/task_logs.rs`, or `tests/executions_routes.rs` if that file's harness is the natural one.

**Interfaces:**

```
GET /api/ws/{workspace}/workflows/executions/{id}/logs
    ?after=<i64>&limit=<1..=2000, default 500>&task_id=<str>&task_run_id=<uuid>&min_level=<TRACE|DEBUG|INFO|WARN|ERROR>
200 { "items": [ {"id","task_id","task_run_id","attempt","level","ts","message","truncated"} ],
      "next_after": <i64|null>, "execution_state": <State> }
404 unknown or foreign execution; 400 bad limit/min_level; 403/404 per W10's feature gate and authz
```

Authorisation, feature gate and 404-for-foreign are **exactly** W10's `GET /executions/{id}`: reuse its extractor chain and its `ExecutionRepo::get` for the existence and ownership check, then `LogRepo::page`. The route's doc comment states the 2 s settle lag and the reason (spec §6).

- [ ] **Step 1: Failing tests:**
  - `logs_route_pages_with_after_and_limit_and_reports_execution_state`.
  - `logs_route_filters_by_min_level_and_task`.
  - `logs_route_answers_404_for_another_workspaces_execution` (the id exists in workspace B, the request is in A).
  - `logs_route_is_gated_by_the_workflows_feature` (reuse W10's gate test shape).
  - `logs_route_rejects_limit_over_2000`.
- [ ] **Step 2: Implement; regenerate the client** (`npm run gen:api` in `frontend/`); run `egret-server`'s OpenAPI currency test.
- [ ] **Step 3: Mutations:** drop the ownership check (use `LogRepo::page` alone with a guessed workspace) must kill the 404 test; drop the `limit` clamp must kill `…rejects_limit_over_2000`.
- [ ] **Step 4: Commit:** `feat(workflow): W11 — GET …/executions/{id}/logs`.

---

## Task 6: Coverage statement, sweep, full gate

- [ ] **Step 1: Coverage statement** in `crates/egret-workflow/oracle-exec/README.md`, following W13b's section shape:
  - ratchet 109 → 109, measured;
  - what W11's tests cover and what they do not: production registry has no container/python, so no production run produces stdout; SSE is absent; the lag is 2 s.
- [ ] **Step 2: Sweep, EN and VI:**
  - the programme spec §9 W11 row and §6.1 `task_logs` line ("NOT BUILT");
  - §11 open question 1;
  - W7's row ("ships without a logs panel");
  - `handlers/mod.rs`'s "egret has no durable workflow-log store yet";
  - the `commands.rs` stderr comment, if Task 3 has not already rewritten it;
  - any "none of the three persists a line".

  Report N examined / M corrected per population, with the commands.
- [ ] **Step 3: Full local CI** per `.github/workflows/ci.yml`:
  - fmt, clippy (the known pre-existing denials only), and workspace tests;
  - the OpenAPI check; frontend `lint`/`typecheck`/`build` (the generated client changed).
  - Name-set diff against the base: removed names must each be explained.
- [ ] **Step 4: Commit:** `docs(workflow): W11 coverage statement and the sweep`.

---

## Final review

A whole-branch review on the most capable model. Point it at:
1. Any path where a line is **silently** lost: a closed channel, a batch dropped without a count, `finish()` skipped, the shutdown order.
2. Any path where logging changes a run's outcome or blocks the tokio runtime.
3. The settle lag against out-of-order commits, including the writer's own retries overlapping a later batch.
4. Cross-workspace reads through the route or `LogRepo::page`.
5. Caps: truncation on UTF-8 boundaries, the summary outside the cap, per attempt.
6. Levels and marker lines against Task 1's `egret_expected`.
7. Pruning: batch size, pass budget, `0` days, and deletion of a live execution's recent lines.
