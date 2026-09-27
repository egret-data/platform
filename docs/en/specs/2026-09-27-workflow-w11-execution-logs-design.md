# W11 — Execution logs: every line a task prints, kept and readable

**Status:** design, awaiting review
**Depends on:** W10 (runtime, execution REST surface), all merged. W13b (#598) is the base.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (row W11) and §11 open question 1
**Decided with the user, 2026-09-27:**
- **Scope:** persist every line: `Log`/`Fail` and handler messages, **and every stdout/stderr line of container and python tasks**. Read by **polling** a cursor API. **No SSE.**
- **Retention:** **one global setting**, default 30 days, pruned by a background ticker.
- **Write path:** a **background batch writer**. When its channel is full the handler **waits**; no line is dropped silently.
- **Limits:** a line is **cut at 64 KiB**, and one task-run attempt keeps at most **10,000 lines or 10 MiB**. Over the cap, storing stops, the task keeps running, and **one WARN summary line** says how much was not stored. All three limits are global settings.

**Delivery:** one PR (docs + code), tasks in §8.

## 1. What exists and what does not

| piece | state on `origin/main` (`6749a37b`) |
|---|---|
| `workflow.task_logs` | **does not exist**. Schema `workflow` has no log table |
| `LogSink` (`worker/handlers/mod.rs:64`) | a **synchronous** trait with three implementations: `TracingSink` (the shipping default, writes to `tracing`), a `#[cfg(test)]` `Recorder`, and `NameHintSink` (annotates and forwards). **None persists a line** |
| sink wiring | every handler calls `default_sink()` in its own `::new()` (`log.rs`, `fail.rs`, `container.rs`, `python.rs`). Nothing in `egret-bin` builds a shared sink |
| container / python output | `worker/commands.rs` reads stdout and stderr **line by line as they arrive** and passes each line to `ingest_line`, which **counts** it (`stdOutLineCount`/`stdErrLineCount`), parses `::{"outputs":…}::` markers, and returns a level (stdout INFO, stderr WARN). The text goes to `tracing` and nowhere else |
| handler transaction | `claim::run_one` runs the handler **before** `pool.begin()`; the transaction only wraps `apply_outcome`. Container and python handlers have **no default time budget** |
| precedents | `egret-audit`: a sync `try_send` into a bounded `mpsc` and a spawned batch writer (it **drops** on a full channel). `egret-automation`: `log_retention_days` per schedule, pruned inline in its scheduler tick. W10/W12 tickers: `interval` + `MissedTickBehavior::Skip` + `CancellationToken` |
| read surface | W10's execution routes (`routes.rs`), offset pagination (`DEFAULT_PAGE_LIMIT` 50, max 200). **No SSE endpoint exists anywhere in egret** |
| latest migration | `0158`; W11 takes **`0159`** |

## 2. What Kestra does, and where egret differs

Kestra (checkout `96893b589`, image `kestra/kestra:v2.0.0-rc13`):
- **Model:** `LogEntry` holds tenant, namespace, flow, task, execution, task run, attempt, trigger, timestamp, level, thread, message, execution kind. On Postgres it is stored in a `logs` table as JSONB with generated columns, indexed on `(execution_id, task_id)`, `(execution_id, taskrun_id)` and time/level, plus a full-text GIN index.
- **Read API:** `LogController` serves `GET /{executionId}` (every line, unpaginated), `/download` (plain text), `/follow` (SSE live tail), `/search` (paged) and deletes.
- **Retention:** there is **no automatic retention**. A user writes a `PurgeLogs` task in a flow.
- **Script output:** stdout and stderr lines become log lines through `AbstractLogConsumer`. `##kestra:log:debug##` markers bracket DEBUG output. The source reads stderr as **ERROR**, and egret today says **WARN** (§5).
- **Limits:** there is **no line or volume cap**.

Deliberate differences, all chosen with the user:

| Kestra | egret W11 | why |
|---|---|---|
| no retention; a user-authored `PurgeLogs` | global retention, pruned by a ticker | a table that only grows is a Postgres outage deferred; egret has one shared database |
| no size cap | 64 KiB per line, 10,000 lines / 10 MiB per task-run attempt | a script printing in a loop must not fill the shared database |
| SSE `follow` | cursor polling (§6) | egret has no SSE anywhere; polling reaches the same "see it while it runs" at a ~2 s delay |
| `thread`, `triggerId`, `executionKind` | not stored | egret has no such concepts |
| unpaginated `GET /{executionId}` | paginated by cursor | one execution may hold 10 MiB per attempt |

## 3. Storage

Migration `0159_workflow_task_logs.sql`:

```sql
CREATE TABLE workflow.task_logs (
    id            bigserial   PRIMARY KEY,
    workspace_id  uuid        NOT NULL,
    execution_id  uuid        NOT NULL REFERENCES workflow.executions(id) ON DELETE CASCADE,
    task_run_id   uuid        NULL,      -- NULL: a line about the execution, not one run
    task_id       text        NULL,
    attempt       integer     NULL,
    level         smallint    NOT NULL CHECK (level BETWEEN 0 AND 4),  -- TRACE..ERROR
    ts            timestamptz NOT NULL,  -- when the line was produced
    inserted_at   timestamptz NOT NULL DEFAULT now(),
    message       text        NOT NULL,
    truncated     boolean     NOT NULL DEFAULT false
);
CREATE INDEX task_logs_execution_id ON workflow.task_logs (execution_id, id);
CREATE INDEX task_logs_ts ON workflow.task_logs (ts);
```

*Shapes to verify, not text to paste.* The plan checks:
- the real type of `executions.id`;
- whether a `workspace_id` FK is the house pattern;
- whether `task_run_id` should reference `task_runs` or stay unconstrained. An FK makes every insert take a lock on `task_runs`, and the worker holds `task_runs` rows `FOR UPDATE`.

`ts` is the time the line was produced (the worker's clock when `log` is called). `inserted_at` is the database's clock, and only §6's cursor rule reads it.

## 4. Write path

### 4.1 `LogSink` becomes async

```rust
#[async_trait]
pub trait LogSink: Send + Sync + std::fmt::Debug {
    async fn write(&self, line: LogLine);
}
```

The trait moves from `fn log(&self, task, level, message)` to an async method taking an owned line. This is the only way to have both of the user's choices:
- the handler **waits** on a full channel;
- nothing **blocks a tokio thread**.

A sync `log` could only drop a line (`try_send`) or block the runtime (`blocking_send`). Every current call site is already inside an async handler.

### 4.2 `RunLog`: one handle per task-run attempt

The worker builds a `RunLog` for each claimed run, in `claim::run_one` before the handler starts. It carries:
- the run's identity: workspace, execution, task run, task id, attempt;
- the shared sink;
- the counters for §5's caps.

Handlers write through it (`run_log.info(msg)`, `run_log.line(level, msg)`). `NameHintSink` becomes a wrapper around `RunLog`. After the handler returns, `run_one` calls `RunLog::finish()` on every path: success, failure, `Err`, timeout and cancellation. `finish()` writes §5's summary line if anything was not stored. Where a handle reaches handlers (on `TaskInput` or as a separate argument) is decided in the plan against `cancel::guarded`'s signature.

### 4.3 `PgLogSink` and its writer

- The sink sends into a **bounded** `mpsc` channel, capacity 10,000 lines, with `send().await`. A full channel slows the handler that is writing; it never drops a line.
- A writer task, spawned in `egret-bin` next to the W10 tickers, drains the channel. It inserts a batch when it holds **500 lines** or when **1 s** has passed since the first unflushed line, whichever comes first. One statement per batch: `INSERT … SELECT * FROM unnest($1::uuid[], …)`.
- **Write failure:** retry with backoff (100 ms doubling, capped at 5 s) for up to 30 s per batch. The channel fills during the retries, so handlers slow down rather than lines vanishing. If the batch still fails, `tracing::error!` names the execution ids and the line count lost, the batch is dropped, and the writer continues. **A log write never changes a run's outcome.**
- **Shutdown:** on cancellation the writer stops accepting, drains what is in the channel, flushes, and exits within a bounded time (10 s). The runtime waits for it after the worker ticker has stopped.
- **Wiring:** `Handlers` (the registry) holds the sink. Handler constructors take it instead of calling `default_sink()`. `TracingSink` stays for unit tests and for the case where the worker runs without a database.

The writer runs whenever `worker_enabled` is true, because the worker is the only producer.

### 4.4 Sources of lines

| source | level |
|---|---|
| `core.log.Log` | declared `level:` (measured in W3) |
| `core.execution.Fail` | as today |
| container / python stdout | INFO (`##kestra:log:debug##` bracket → DEBUG, if Task 1 measures that it applies) |
| container / python stderr | **per Task 1's measurement** (§5) |
| handler messages already emitted through the sink | unchanged |
| §5's cap summary | WARN |

Lines consumed as `::{"outputs":…}::` markers are stored or not **per Task 1's measurement**.

## 5. Limits, and levels by measurement

**Truncation.** A line longer than `log_line_max_bytes` (default 65,536) is cut at the last UTF-8 character boundary at or below the limit, and stored with `truncated = true`.

**Per-attempt cap.** `RunLog` counts the lines and bytes it has sent. The first line that would exceed `log_run_max_lines` (default 10,000) or `log_run_max_bytes` (default 10 MiB) switches storing off for that attempt. Counting continues. At `finish()`, if any line was not stored, exactly one WARN line is written, **outside** the cap:

> *N lines (M bytes) of this attempt were not stored: over the per-run log cap (10000 lines / 10485760 bytes).*

The task keeps running and its outcome is unchanged. The limits apply per attempt, so a retry starts fresh.

**Levels.** egret's comment in `commands.rs` reads Kestra's `warningOnStdErr` as the knob for the stderr *log level*. The research reading of Kestra's source is different: `warningOnStdErr` sets the task *state* to WARNING, and `AbstractLogConsumer` logs stderr at **ERROR**. The comment's own defence, *"nothing here reads a level back"*, stops being true in W11, because `min_level` reads it back. **Task 1 measures on the artifact:**
1. the level of a stderr line;
2. whether an `outputs` marker line appears in the log;
3. whether Kestra adds framing lines around a task, for example on start or end;
4. what `attempt` a line carries after a retry;
5. whether the `##kestra:log:debug##` bracket applies to egret's script tasks.

The code follows the measurement, and any disagreement with the Java is written down.

## 6. Read API

```
GET /api/ws/{workspace}/workflows/executions/{id}/logs
    ?after=<id>&limit=<n>&task_id=<s>&task_run_id=<uuid>&min_level=<TRACE..ERROR>
→ 200 { items: [ {id, task_id, task_run_id, attempt, level, ts, message, truncated} ],
        next_after: <id|null>, execution_state: <State> }
```

- **Authorisation:** identical to W10's `GET /executions/{id}`: `WsContext`, `authz::require`, and the `workflows` feature gate. The query filters on **both** `execution_id` and `workspace_id`, so another workspace's execution is a **404**. A test proves that.
- **Paging:** `limit` defaults to 500, max 2,000. Rows come in `id` order. `next_after` is the last returned `id`, or `null` when nothing was returned.
- **End of stream:** when `execution_state` is terminal and a call returns no items, the client has everything and stops polling.
- **Why a cursor, not W10's offset:** the log is append-only and grows while it is read. Offset paging over a growing set repeats or skips rows.

**The out-of-order commit trap, and the settle lag.** `bigserial` ids are allocated at insert and committed later. With two writers (two egret replicas, or one writer's retry overlapping the next batch), id 100 can commit after id 101. A client that has already read 101 and moved its cursor past it would never see 100. So the query returns only rows with `inserted_at < now() - interval '2 seconds'`.
- A batch is one short transaction, so 2 s bounds the window generously.
- The cost is that a line shows up about 2 s after it is written.
- A test stages two transactions committing in reverse id order and asserts that a polling client reads both.
- The lag and its reason are written into the route's doc comment.

utoipa-documented; the frontend client is regenerated with `npm run gen:api`. **The logs panel is W7's**; W11 ships the API only.

## 7. Retention

- Setting: `workflow.log_retention_days`, default **30**; `0` means keep forever.
- A ticker in `egret_workflow::runtime`, started when `worker_enabled`:
  - every **hour**, `MissedTickBehavior::Skip`, cancellation-aware;
  - deletes in batches of **10,000** rows (`DELETE … WHERE id IN (SELECT id … WHERE ts < now() - make_interval(days => $1) LIMIT 10000)`);
  - loops until a batch deletes fewer rows or its 30 s pass budget runs out.
- No single statement holds locks for long. Concurrent replicas are harmless, because deleting a row twice is a no-op.
- `ON DELETE CASCADE` removes an execution's lines with the execution.

Configuration (`egret_config::WorkflowConfig`, env `EGRET_WORKFLOW__…`): `log_retention_days` (30), `log_line_max_bytes` (65536), `log_run_max_lines` (10000), `log_run_max_bytes` (10485760).

## 8. Tasks

1. **Measure on the artifact** (§5 questions 1–5); fixture `oracle-exec/w11-logs.json`.
2. **Migration `0159` and `LogRepo`:** batch insert, cursor read with the settle lag, batch prune.
3. **Pure logging:**
   - async `LogSink`, `LogLine`, `RunLog` (truncation, caps, summary on `finish`);
   - the four handlers and `ingest_line` move onto `RunLog`;
   - the registry carries the sink;
   - levels per Task 1.
4. **`PgLogSink` writer and the retention ticker:** backpressure, batching, retry, shutdown drain; wired into `egret-bin`; the config keys.
5. **Read API:** route, utoipa, `gen:api`.
6. **Coverage statement, EN/VI sweep, full local CI.** The sweep finds claims W11 makes false, for example *"none of the three persists a line"* and *"W7 ships without a logs panel"* in the programme spec §9, and the `commands.rs` stderr comment.

## 9. Testing

Against real Postgres, each with a mutation that must die:

| test | the mutation it kills |
|---|---|
| end to end: publish and run a flow with `Log` + a container that prints stdout and stderr, with no harness driving the tick; read it back through the API in order and at the measured levels | the container handler not writing through `RunLog` |
| a full channel with a slowed writer: the handler waits and **no line is lost** | `try_send` that drops instead of `send().await` |
| over the cap: stored lines stop at the cap, exactly **one** WARN summary, the task succeeds | summary written inside the cap / not written on the `Err` path |
| truncation at a multi-byte boundary | cutting at a byte count |
| out-of-order commit: two transactions commit in reverse id order and a polling client reads both | removing the settle lag |
| shutdown drains the channel | exit without draining |
| retention deletes only lines older than the window; `0` deletes nothing | dropping the `ts` predicate |
| another workspace's execution → 404 | dropping `workspace_id` from the query |
| `finish()` runs on timeout and cancellation | calling it only on success |

**Ratchet:** expected **109 → 109**, measured, not asserted. The oracle compares traces, not log lines.

## 10. Out of scope

SSE / `follow`; `/download`; full-text search; manual log deletion; per-flow `logLevel`; a per-workspace retention override (the global setting leaves room for it without a schema change); object storage (W9); `PurgeLogs`.

## 11. Known limits, stated

- **Lines can be lost** if the process dies: those still in the channel, and those in a batch whose retries run out. Both are named in `tracing` where a write fails; a process kill cannot report its own loss.
- **A run that executes twice** (reaped while its handler still ran, a hazard W4 documented) writes both runs' lines under the same attempt.
- A line appears about **2 s** after it is written (§6).
- **Production runs no container or python task yet.** `egret-bin` registers `Handlers::with_core_tasks()` (`Log`, `Return`, `Fail`, `Sleep`), and its own comment says nine of thirteen publishable types hang in production. W11 stores and tests stdout/stderr through test registries; in production today the stored lines come from `Log`/`Fail` and handler messages. Wiring container, python and sql into production is a separate package, because how the server may run containers is a security decision of its own (user decision, 2026-09-27).
