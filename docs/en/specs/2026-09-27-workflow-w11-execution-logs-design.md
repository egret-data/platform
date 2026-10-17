# W11 — Execution logs: every line a task prints, kept and readable

**Status:** design, awaiting review
**Depends on:** W10 (runtime, execution REST surface), all merged. W13b (#598) is the base.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (row W11) and §11 open question 1
**Decided with the user, 2026-09-27:**
- **Scope:** persist every line: `Log`/`Fail` and handler messages, **and every stdout/stderr line of container and python tasks**. Read by **polling** a cursor API. **No SSE.**
- **Retention:** **one global setting**, default 30 days, pruned by a background ticker.
- **Write path:** a **background batch writer**. When its channel is full the handler **waits**; no line is dropped silently. **Amended with the user, 2026-09-28:** while the writer is failing (its breaker is open), a write that finds the channel full drops the line and counts it instead of waiting, so logging can never push a run past its `timeout:` or stall the worker; the dropped count is reported through `tracing::error!` when the writer recovers or drains at shutdown. While the writer is healthy the handler still waits.
- **Limits:** one task-run attempt keeps at most **10,000 lines or 10 MiB**. Over the cap, storing stops, the task keeps running, and **one WARN summary line** says how much was not stored. Both caps are global settings.
- **Long lines (D6, decided after Task 1 measured Kestra):** a line longer than **15,360 characters is split** into several stored lines, as Kestra splits it; no text is lost. This replaces the first decision, a cut at 64 KiB. The piece length is a constant, not a setting.

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

Kestra (Java checkout `96893b589`; the measured image is `kestra/kestra@sha256:28f59bf1…`, tagged `v2.0.0-rc13` but labelled `version=nightly`, revision `169ca7f2`):
- **Model:** `LogEntry` holds tenant, namespace, flow, task, execution, task run, attempt, trigger, timestamp, level, thread, message, execution kind. On Postgres it is stored in a `logs` table as JSONB with generated columns, indexed on `(execution_id, task_id)`, `(execution_id, taskrun_id)` and time/level, plus a full-text GIN index.
- **Read API:** `LogController` serves `GET /{executionId}` (every line, unpaginated), `/download` (plain text), `/follow` (SSE live tail), `/search` (paged) and deletes.
- **Retention:** there is **no automatic retention**. A user writes a `PurgeLogs` task in a flow.
- **Script output:** stdout and stderr lines become log lines through `AbstractLogConsumer`. `##kestra:log:debug##` markers bracket DEBUG output. The source reads stderr as **ERROR**, Task 1 measured **ERROR**, and egret today says **WARN** (§5).
- **Limits:** there is **no volume cap**. A line longer than 15,360 characters is **split** into several lines with one shared timestamp (`RunContextLogger`, measured in Task 1), which the API may then return out of order (it did on 2 of 2 runs).

Deliberate differences, all chosen with the user:

| Kestra | egret W11 | why |
|---|---|---|
| no retention; a user-authored `PurgeLogs` | global retention, pruned by a ticker | a table that only grows is a Postgres outage deferred; egret has one shared database |
| no volume cap | 10,000 lines / 10 MiB per task-run attempt; long lines split exactly as Kestra splits them (§5) | a script printing in a loop must not fill the shared database |
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
    message       text        NOT NULL
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

- The sink sends into a **bounded** `mpsc` channel, capacity 10,000 lines. While the writer is healthy, a full channel makes the handler that is writing **wait** for room; it never drops a line. While the writer is **failing** (the breaker, below), a full channel drops the line and counts it instead.
- A writer task, spawned in `egret-bin` next to the W10 tickers, drains the channel. It inserts a batch when it holds **500 lines** or when **1 s** has passed since the first unflushed line, whichever comes first. One statement per batch: `INSERT … SELECT * FROM unnest($1::uuid[], …)`.
- **Write failure:** a batch that started while the writer was healthy is retried with backoff (100 ms doubling, capped at 5 s) for up to 30 s. If it still fails, `tracing::error!` names the execution ids and the line count lost, the batch is dropped, and the writer continues. A batch that starts while the writer is failing gets **one** attempt. SQLSTATE 23503 (an execution deleted mid-statement) is re-run once at once, never retried.
- **A line Postgres refuses** (SQLSTATE class 22, for example a U+0000 byte, 22021): no retry can fix a value, and retrying would hold the batch's other lines, from any execution and any workspace, hostage. So the batch is re-inserted **one line at a time**; only the refused line(s) are dropped, with a counted `tracing::error!`. `RunLog` removes U+0000 from every line it builds, as Kestra does (`JdbcJsonbUtils.java:13`), so a line reaches this path only if it bypassed `RunLog`.
- **The breaker.** The writer and every sink share a **failing** flag. It is set by an insert that fails with an ordinary error and by a flush barrier that timed out; it is cleared by the next insert that succeeds. While it is set: the flush barrier returns at once (counted, one `warn` per failing spell), and a barrier already waiting is released; a `write` that finds the channel full drops its line with a count instead of waiting (one `error!` when dropping starts, the total when the writer recovers or stops); each new batch gets one attempt, so the channel keeps draining. In normal operation it changes nothing.
- **What a log write can and cannot do to a run.** It never changes a run's outcome through the `Outcome`: a failed write is reported through `tracing`. It can **delay** the outcome by at most the barrier's bound (5 s), once per failing spell, and by the time an insert attempt takes to fail before the breaker is set. It cannot run a task into its `timeout:` through logging while the writer is failing, because `write` does not wait then. A write **can** still wait while the writer is healthy but slower than the handler (D3's backpressure), and that wait counts against the task's `timeout:` like any other work the task does.
- **Shutdown:** on cancellation the writer stops accepting, drains what is in the channel, flushes, and exits within a bounded time (10 s). The runtime waits for it after the worker ticker has stopped.
- **Flush barrier:** `LogSink::flush()` (a no-op by default, so `TracingSink` and test sinks return at once). `PgLogSink` sends a marker behind every line it has accepted. The channel carries `Line(LogLine) | Flush(oneshot)`. The writer stores its current batch, with its normal retries and 23503 re-run, and then acknowledges. `RunLog::finish()` awaits the barrier after the cap summary. `claim::run_one` calls `finish()` before it opens the outcome's transaction, so **a run's lines are inserted before its outcome, and so the execution's terminal state, commits**. The wait is bounded by **5 s** (`WriterConfig::flush_within`: a healthy barrier waits for everything queued ahead of it, up to the whole channel — about 21 inserts of 500 lines). Past it, or with the writer gone, `finish()` logs and returns, and the timeout sets the breaker. While the breaker is set the barrier does not wait at all. So during a log-write fault a run's outcome can commit **before its last lines are stored, or without them**, and `complete` can then be true while those lines are missing (§11).
- **Shutdown during a log-write fault.** The claim loop's tick is not cancel-aware, so a tick in flight at SIGTERM runs to its end. Logging adds to that tick at most one barrier bound (5 s) and one failing insert attempt per failing spell, not 30 s per run, so a tick's length is its handlers' time as it was before W11.
- **Wiring:** `Handlers` (the registry) holds the sink. Handler constructors take it instead of calling `default_sink()`. `TracingSink` stays for unit tests and for the case where the worker runs without a database.

The writer runs whenever `worker_enabled` is true, because the worker is the only producer.

### 4.4 Sources of lines

| task kind / path | written by | lines (level, text) |
|---|---|---|
| `core.log.Log` | the handler | one line per message, at the declared `level:` (measured in W3 and Task 1) |
| `core.execution.Fail`, no `condition` | `run_one` only (the handler writes nothing) | one line: ERROR, the `errorMessage`. Kestra writes the same one ERROR line (measured) |
| `core.execution.Fail`, `condition` true | the handler, then `run_one` | two lines: ERROR `errorMessage` (handler), then ERROR `Fail on a condition` (`run_one`), which are Kestra's two (`Fail.java:131-136` and `exceptionHandler`) |
| `core.execution.Fail`, `condition` false | nobody | none |
| container / python stdout | the handler (`ingest_line`) | INFO; **DEBUG** between `##kestra:log:debug##` and `##kestra:log:info##` (measured; the marker lines are not stored) |
| container / python stderr | the handler (`ingest_line`) | **ERROR** (measured, §5), also inside a debug bracket |
| invalid whole-line `::{…}::` marker | the handler (`ingest_line`) | WARN `Invalid outputs '…'` (measured shape, §5) |
| any task whose attempt ends `Failed` with a message (handler `Ok(Failed)`, `Budget::Refused`, timeout expiry) | `run_one` (`RunLog::failure`) | **the failed-attempt line**: one ERROR line, the `Outcome`'s message. It **bypasses the cap** and comes after the handler's lines and before the summary. Not written for a handler `Err` (egret's outage, chosen), a dropped future (Kestra writes nothing when killed), or a `Failed` outcome with no message |
| worker framing ("started"/"ended") | nobody | none (measured: Kestra stores none) |
| handler messages already emitted through the sink | the handler | unchanged |
| §5's cap summary | `RunLog::finish` | WARN, after the failed-attempt line, outside the cap |

**The failed-attempt line against Kestra.** Kestra's `AbstractWorkerCallable.exceptionHandler` logs every throwing task's `e.getMessage()` at ERROR unless it was killed. `RunContextLogger.logEntries` then adds a second ERROR line with the cause-chain messages when the exception has a cause, and a DEBUG stack trace. The q4 (`Fail`) and q5 (script) records show both shapes. egret has no cause chain, so it writes **only the first line**; the cause line and the stack trace are a stated divergence. The text matches Kestra for `Fail` (`errorMessage`, `Fail on a condition`) and for a timeout (`Timeout after HH:mm:ss.SSS`). For every other handler it is egret's own failure message (chosen). For a script, Kestra's is `io.kestra…TaskException: Command failed with exit code N` followed by the cause line.

Lines consumed as whole-line `::{…}::` markers are **not stored** (measured, §5).

## 5. Limits, and levels by measurement

**Long lines are split (D6).** `RunLog` splits every line longer than **15,360 characters** into consecutive pieces of 15,360 characters, the last one shorter, exactly as Kestra's `RunContextLogger` does (`MAX_MESSAGE_LENGTH = 1024 * 15`, `Splitter.fixedLength`; measured in Task 1: a 100,000-character line became 6 × 15,360 + 7,840). Each piece is its own stored line with the same `ts`, written in order, so the pieces get ascending ids and the API returns them in order. Their concatenation equals the input. Each piece counts toward the per-attempt cap as one line and at its own byte length. The piece length is the constant `LOG_PIECE_CHARS = 15_360`, not a setting.

**What a character is.** egret counts **Unicode scalar values** (Rust `char`), so a split never falls inside a character and every piece is valid UTF-8. Kestra's Java counts **UTF-16 code units** (`String.length()`). The two agree on every character in the Basic Multilingual Plane. They differ for characters outside it (emoji, some CJK): such a character is 1 in egret's count and 2 in Kestra's, so egret's pieces can hold more of them, and Kestra can split a surrogate pair where egret cannot. This is a deliberate difference and it is **not measured**: Task 1's long line was ASCII only.

**Per-attempt cap.** `RunLog` counts the lines and bytes it has sent. The first line that would exceed `log_run_max_lines` (default 10,000) or `log_run_max_bytes` (default 10 MiB) switches storing off for that attempt. Counting continues. At `finish()`, if any line was not stored, exactly one WARN line is written, **outside** the cap:

> *N lines (M bytes) of this attempt were not stored: over the per-run log cap (10000 lines / 10485760 bytes).*

The task keeps running and its outcome is unchanged. The limits apply per attempt, so a retry starts fresh.

**Levels, measured.** Task 1 measured these on the artifact (fixture `crates/egret-workflow/oracle-exec/w11-logs.json`, shell plugin 1.9.8, Process and Docker runners). egret's comment in `commands.rs` read Kestra's `warningOnStdErr` as the knob for the stderr *log level*, and its own defence, *"nothing here reads a level back"*, stops being true in W11, because `min_level` reads it back. The measurement:
1. **stderr is ERROR**, not WARN, whatever `warningOnStdErr` says (default, `false` and `true` all logged ERROR). egret changes WARN to ERROR and still does not offer `warningOnStdErr`.
2. **A whole-line `::{json}::` marker is not stored**, on stdout or stderr, and its `outputs` still reach the task's outputs. This holds for any JSON object, not only one with `outputs` (a `::{"metrics":[]}::` line is not stored either). A line that merely contains a marker but is not exactly one (`x ::{…}::`, or a marker followed by a space) is an ordinary line and is stored. A whole-line marker whose JSON does not parse is not stored; Kestra writes a WARN `Invalid outputs '<error>'` instead, and egret writes one WARN starting `Invalid outputs '` with serde's error text (chosen: no stack-trace line). egret's comment "consumed as an output AND logged" was wrong about Kestra.
3. **There are no worker framing lines** in an execution's log. `Type {} started` and `Flow completed with state {}` go to the server's own logger. Kestra's script runner adds its own DEBUG/TRACE diagnostics (`Starting command with pid …`, `Command succeed with exit code 0`, container and volume ids); egret does **not** replicate them (chosen).
4. **`attemptNumber` is 0-based**: the first attempt's lines carry 0, the retry's 1, under the same task run id. It is the index into `task_runs.attempts`. egret's `TaskInput.attempt` is 1-based, so `task_logs.attempt = TaskInput.attempt - 1` (chosen mapping, measured numbering).
5. **The `##kestra:log:debug##` bracket applies**, on both runners: stdout lines after it are DEBUG until `##kestra:log:info##`, and neither marker line is stored. A stderr line inside the bracket stays ERROR. One mode per command, toggled by a marker on either stream.
6. **A long line is split, not cut:** Kestra stores a 100,000-character line as 15,360-character pieces with one shared timestamp, and read them back out of order. egret splits the same way (above, D6) and keeps the pieces in order.

The code follows the measurement, and any disagreement with the Java is written down.

## 6. Read API

```
GET /api/ws/{workspace}/workflows/executions/{id}/logs
    ?after=<id>&limit=<n>&task_id=<s>&task_run_id=<uuid>&min_level=<TRACE..ERROR>
→ 200 { items: [ {id, task_id, task_run_id, attempt, level, ts, message} ],
        next_after: <id|null>, execution_state: <State>, complete: <bool> }
```

- **Authorisation:** identical to W10's `GET /executions/{id}`: `WsContext`, `authz::require`, and the `workflows` feature gate. The query filters on **both** `execution_id` and `workspace_id`, so another workspace's execution is a **404**. A test proves that.
- **Paging:** `limit` defaults to 500, max 2,000. Rows come in `id` order. `next_after` is the last returned `id`, or `null` when nothing was returned.
- **End of stream:** `complete` is true when the execution's state is terminal **and** `now() - executions.updated_at > 2 s` (the settle lag). Both are computed in SQL from the same row. **The client stops polling on `complete && items` empty, never on a terminal `execution_state` alone.** A terminal state is committed after the run's lines are inserted (§4.3's flush barrier). So once the settle lag has also passed since that commit, every line is visible. Without the barrier, a terminal state could be read while the last lines were still in the channel, in an unflushed batch (≤ 1 s) or being retried (≤ 30 s), and a client would stop and silently lose the tail. The barrier gives this guarantee only while the writer is healthy; §11 lists when it does not hold. The route reads the execution through a one-row query (existence, ownership, state, `complete`). It does not load the frozen source or the task runs, because a panel polls it.
- **Why a cursor, not W10's offset:** the log is append-only and grows while it is read. Offset paging over a growing set repeats or skips rows.

**The out-of-order commit trap, and the settle lag.** `bigserial` ids are allocated at insert and committed later. With two writers (two egret replicas; one writer is sequential and cannot overlap itself), id 100 can commit after id 101, and a statement that started later can hold an id below some of an earlier statement's ids. A client that has already read 101 and moved its cursor past it would never see 100. So the page is a **watermark**: it returns rows in id order and stops before the first row after the cursor whose `inserted_at` is not older than `now() - interval '2 seconds'`, whatever the filters. A per-row filter is not enough: it can serve a settled higher id while a lower id is still settling.
- **The bound:** a write transaction must last less than **half** the lag (1 s). An uncommitted row is invisible to the watermark; it can hold an id below a served row only if its transaction started before that row's ended, so it commits within two write-durations of that row's start. A batch is one short `INSERT`.
- The cost is that a line shows up about 2 s after it is written.
- Two tests: two transactions committing in reverse id order, read by a polling client that must get both; and three committed rows of which only the lowest and the highest are settled, where the reader must not advance past the middle one.
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

Configuration (`egret_config::WorkflowConfig`, env `EGRET_WORKFLOW__…`): `log_retention_days` (30), `log_run_max_lines` (10000), `log_run_max_bytes` (10485760).

## 8. Tasks

1. **Measure on the artifact** (§5 questions 1–5); fixture `oracle-exec/w11-logs.json`.
2. **Migration `0159` and `LogRepo`:** batch insert, cursor read with the settle lag, batch prune.
3. **Pure logging:**
   - async `LogSink`, `LogLine`, `RunLog` (splitting long lines, caps, summary on `finish`);
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
| end to end: publish and run a flow with `Log` + `Fail` through the real runtime, with no harness driving the tick, and read the lines back from Postgres in order and at their levels. **Container and python stdout/stderr are tested separately** (`tests/container_docker_live.rs`, against a real Docker daemon) reaching `RunLog` at the measured levels, recorded by a test sink; no test stores a container line in Postgres | the `Log` / failure path not writing through `RunLog` |
| a full channel with a slowed writer: the handler waits and **no line is lost** | `try_send` that drops instead of `send().await` |
| over the cap: stored lines stop at the cap, exactly **one** WARN summary, the task succeeds | summary written inside the cap / not written on the `Err` path |
| a failed attempt over the cap: the handler's stored lines, then exactly one ERROR failure line, then the summary; an unconditional `Fail` stores one ERROR line and a conditional one two | failure line counted inside the cap / written after the summary / written on the `Err` path / `Fail` logging `errorMessage` twice |
| a 40,000-character line with multi-byte characters at and around the 15,360th character → 3 stored lines in order (15,360 / 15,360 / 9,280 characters), their concatenation equals the input | splitting at a byte count instead of a `char` count |
| out-of-order commit: two transactions commit in reverse id order and a polling client reads both | removing the settle lag |
| the watermark: of three committed rows only the lowest and the highest are settled; the page serves the lowest and stops | a per-row settled filter instead of the watermark |
| a line holding U+0000 is stored without it; a batch holding a refused line stores every other line, of any workspace, without a retry window | not stripping U+0000 / retrying a data error |
| a failing writer: five runs' `finish()` return at once; a `write` on a full channel does not wait; the barrier waits again once an insert succeeds | the barrier ignoring the breaker |
| a barrier behind a dropped batch, and one queued at shutdown, are acknowledged | not acknowledging after a dropped batch / in the drain |
| shutdown drains the channel | exit without draining |
| retention deletes only lines older than the window; `0` deletes nothing | dropping the `ts` predicate |
| another workspace's execution → 404 | dropping `workspace_id` from the query |
| `finish()` runs on timeout and cancellation | calling it only on success |

**Ratchet:** expected **109 → 109**, measured, not asserted. The oracle compares traces, not log lines.

## 10. Out of scope

SSE / `follow`; `/download`; full-text search; manual log deletion; per-flow `logLevel`; a per-workspace retention override (the global setting leaves room for it without a schema change); object storage (W9); `PurgeLogs`.

## 11. Known limits, stated

- **Lines can be lost:** those still in the channel when the process dies; a batch whose retries run out, or whose one attempt fails while the writer is failing; a line Postgres refuses (only that line); a line dropped at a full channel while the writer is failing. Each is counted in `tracing` where it happens, except the process dying, which cannot report its own loss.
- **`complete` can be true with lines missing** in exactly these cases: (1) the process died with lines unstored; (2) a batch was dropped (above) before the terminal commit; (3) a flush barrier did not wait for the run's lines, because it timed out after its **5 s** bound or because the writer was failing (the breaker); the run's outcome then commits before its last lines are stored, or without them, and lines stored after the terminal commit may be served after `complete` was already reported; (4) a run that executes twice (reaped while its handler still ran): the stale handler can write after the terminal commit. `complete` also assumes nothing else writes after the terminal state, which holds while egret has no kill route. *(Note, 2026-10-18: egret now has a kill route (`docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`). On the kill path the worker drops the handler and runs the run log's `finish()` before it commits `KILLED` (`worker/claim.rs`, `run_one`'s `Leased::Killed` arm). The kill was not measured against `complete` separately.)*
- **Delivery is at least once:** a batch retried after a failure whose commit the writer could not see (the connection lost after the commit) is stored twice.
- **A run that executes twice** (reaped while its handler still ran, a hazard W4 documented) writes both runs' lines under the same attempt.
- A line appears about **2 s** after it is written (§6).
- **Production runs no container or python task yet.** `egret-bin` registers `Handlers::with_core_tasks()` (`Log`, `Return`, `Fail`, `Sleep`), and its own comment says nine of thirteen publishable types hang in production. W11 stores and tests stdout/stderr through test registries; in production today the stored lines come from `Log`/`Fail` and handler messages. Wiring container, python and sql into production is a separate package, because how the server may run containers is a security decision of its own (user decision, 2026-09-27). *(W5a, 2026-09-28: that package is W5a. With `workflow.container_runner` set to `docker` or `kubernetes`, production runs Kestra's four script task types and the two aliases, and stores their stdout at INFO and stderr at ERROR; the default `none` leaves this item true. Seven of fifteen publishable types still hang, not nine of thirteen; `egret.sql` and `egret.python` are still not in production.)*
