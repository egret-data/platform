# W13 — Flow triggers: a flow that starts because another one ended

**Status:** design, awaiting review
**Depends on:** W6 (`trigger::flow::evaluate`, pure), W10 (runtime), W12 (`scheduler::sync`, `reconcile`, the scheduler ticker, `start_execution_in`) — all merged
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (row W13)
**Decided with the user, 2026-09-25:**
- **Both arms.** The window-free arm (no `dependsOn`) and the `dependsOn` arm with its persistent window.
- **A draft listener fires**, as cron, webhooks and API start already run the draft.
- **Asynchronous through an outbox**, not nested inside `settle` (see §3 for why the first design was withdrawn).

**Delivery:** one docs PR, then **W13a** (plumbing + window-free arm) and **W13b** (`dependsOn` + windows), stacked.

## 1. What exists and what does not

| piece | state on `origin/main` (`006563ee`) |
|---|---|
| `trigger::flow::evaluate(listener, flow, source, when, max_depth)` | **built, pure** (W6). The artifact's filter order: workspace, entry state, same flow, kind, flow disabled, draft, depth, trigger disabled, `states`, `when` |
| `WindowFreeFlowTrigger::resolve` | **built**. Refuses `dependsOn` with `UnsupportedDependsOn` |
| `models::FlowTrigger` | `inputs`, `states` (default: terminated states + `PAUSED`), `dependsOn`, `mode`, `minSatisfied`. **`window` is dropped**, so a declared `window:` parses and is silently lost |
| `executions.trigger_execution_id` | **exists** (migration 0138). `workflow_executions_one_link_kind` allows one link kind per row |
| a persisted execution depth | **none**. `ExecutionCtx::depth` counts *nested `settle` frames on the stack*, not Kestra's `executionDepth` |
| anything that calls `evaluate` | **nothing** |

*This table is the state at `006563ee`, before W13, and is kept as that record. Since W13b (2026-09-26): `window` is modelled with seven fields and refused rather than dropped when it is not implemented; `WindowFreeFlowTrigger::resolve` still refuses `dependsOn` with `UnsupportedDependsOn`, by design, because the processor routes a `dependsOn` listener to its own arm (`trigger::depends_on`, `trigger::window`, `flow_trigger::process::listen_depends_on`) before `resolve` is called. Of the corpus's 20 `dependsOn` declarations, 18 now resolve; see `oracle-exec/README.md`, W13b.*

## 2. What judges W13

**Not the ratchet: expected 109 before and after, for both PRs.** W6 measured why: `harvest.py`'s settle walks `parent_id`, and a flow-fired execution has `parentId = null` on 210 of 210 measured executions. It carries only the trigger link. The corpus has 17 listener flows (21 declarations, 20 with `dependsOn`) and **zero** reachable baselined entries. Teaching the oracle to follow `trigger_execution_id` would need a re-harvest. That is out of scope and recorded in §9.

W13 is judged by its own tests (§7). Each must be seen to fail under a named mutation, as in W12.

## 3. Why the evaluation is asynchronous

The first design evaluated listeners inside the source's `settle` and started fired executions there, nested. The code refuted it:

- `settle` already nests child executions for `Subflow`, and caps that at **`subflow::MAX_SPAWN_CHAIN = 8`** because every level costs worker stack (tokio workers have 2 MiB). A nested flow-trigger chain A→B→C… would be cut near 8, where Kestra allows **100** (`DEFAULT_MAX_EXECUTION_DEPTH`).
- One source with N listeners would pull N executions, and everything *their* first settles fire, into the worker's transaction.
- Kestra itself is asynchronous here: `DefaultExecutor.processFlowTriggers` consumes executions from a queue.

So `settle` only **records** the transition, and a separate loop evaluates it.

## 4. W13a — plumbing and the window-free arm

### 4.1 Schema (migration `0157`; SP4a holds 0154/0155)

```sql
-- One row per Flow trigger a flow declares, kept by scheduler::sync.
CREATE TABLE workflow.flow_listeners (
  flow_id      uuid    NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id   text    NOT NULL,
  workspace_id uuid    NOT NULL,
  states       text[]  NOT NULL,  -- Flow.states after defaults
  depends_on   boolean NOT NULL,  -- which arm
  disabled     boolean NOT NULL,  -- flow YAML disabled OR trigger disabled
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_flow_listeners_ws ON workflow.flow_listeners (workspace_id) WHERE NOT disabled;

-- The outbox: one row per source transition that some listener could hear.
CREATE TABLE workflow.flow_trigger_events (
  id                  bigserial   PRIMARY KEY,
  source_execution_id uuid        NOT NULL REFERENCES workflow.executions (id) ON DELETE CASCADE,
  workspace_id        uuid        NOT NULL,
  state               text        NOT NULL,
  done                text[]      NOT NULL DEFAULT '{}', -- listeners already handled
  attempts            integer     NOT NULL DEFAULT 0,
  next_attempt_at     timestamptz NOT NULL DEFAULT now(), -- a kept event's backoff
  created_at          timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE workflow.executions ADD COLUMN execution_depth integer NOT NULL DEFAULT 0;
-- The outbox event a fire consumed; set iff the trigger is a Flow trigger.
ALTER TABLE workflow.executions ADD COLUMN flow_trigger_event_id bigint;
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_flow_trigger_event_iff_flow
  CHECK ((flow_trigger_event_id IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Flow'));
-- A backstop, as W12's one-per-tick index: a listener fires at most once per
-- outbox event.
CREATE UNIQUE INDEX workflow_executions_one_per_flow_trigger_event
  ON workflow.executions (flow_trigger_event_id, flow_id, (execution_trigger->>'id'))
  WHERE flow_trigger_event_id IS NOT NULL;
```

The unique index keys on the outbox event, not on the source's state, because a source that re-enters a state (`PAUSED`, `RUNNING`, `PAUSED`) makes a new transition, and so a new event, which Kestra fires on again. A key on `(source, state)` would let a listener hear only the first. Event ids are `bigserial` and never reused. `flow_trigger_event_id` is not a foreign key, because the processor deletes the event in the transaction that fires (§4.4).

### 4.2 Keeping `flow_listeners` in step

- **`scheduler::sync` writes these rows** in the same pass as the `Schedule` rows. So every `FlowRepo` writer, `reconcile` at start and the writer scanner cover it with no new mechanism.
- A declaration that does not resolve gets no row, and the reason is logged.
- A `dependsOn` declaration gets a row with `depends_on = true` from W13a onward, but W13a's processor skips it. Its refusal message says W13b owns it. W13b drops the skip.

### 4.3 The hook in `settle`

At the end of `settle`, one statement writes one event for every state the settle **appended** to the execution's history (`state.history[before.history.len()..]`), except `CREATED`, in order:

```sql
INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state)
SELECT $1, $2, t.s
  FROM unnest($3::text[]) WITH ORDINALITY AS t(s, n)   -- the appended states
 WHERE EXISTS (SELECT 1 FROM workflow.flow_listeners l
                WHERE l.workspace_id = $2 AND NOT l.disabled AND t.s = ANY (l.states))
 ORDER BY t.n
```

- **Every transition, not only terminal ones.** `PAUSED` is in the default `states` and is not terminal. The existing parent-notification hook fires on terminal states only, so this cannot reuse it.
- **Every appended state, not only the last one.** One settle can append several: a bounded `Pause` start goes `CREATED → RUNNING → PAUSED`, and the settle that ends one pause and enters the next goes `PAUSED → RESTARTED → RUNNING → PAUSED`, whose first and last states are equal. Kestra runs `processFlowTriggers` once per transition, so each appended state is an event. The slice is sound because execution history is append-only across a settle.
- **The `EXISTS` keeps the outbox empty** for workspaces with no listener. The cost on the hot path is one indexed probe.
- **Atomic with the transition.** A transition that commits has its event; one that rolls back has none.

### 4.4 The processor

`flow_trigger::process_events_with(&PgPool, &PassLimits, &CancellationToken) -> Result<EventReport>` runs from the **W12 scheduler ticker**, after `fire_due`, behind the same `scheduler_enabled` switch. (`process_events(&PgPool)` is the same pass with the default limits and no cancellation.) A pass claims the outbox **one event per transaction**, and it is **capped**:

```text
cursor := 0
loop:
  cancel fired -> return
  BEGIN ISOLATION LEVEL REPEATABLE READ
    SELECT id, source_execution_id, workspace_id, state, done, attempts
      FROM workflow.flow_trigger_events
     WHERE id > cursor AND next_attempt_at <= now()
     ORDER BY id LIMIT 1
       FOR UPDATE SKIP LOCKED                   -- none -> return; 40001 -> claim again
    cursor := event.id
    SELECT listeners: same workspace, state in states, not disabled,
           listener flow not deleted, state <> 'disabled', workflows feature on,
           workspace active                     -- the six W12 off switches
    drop the listeners already in event.done
    load the source execution (workspace, namespace, flow key, kind, execution_depth,
                               render surface), in a SAVEPOINT
    per listener, in a SAVEPOINT:
      parse listener source; WindowFreeFlowTrigger::resolve
      render `when` against the SOURCE's context
      trigger::flow::evaluate(.., max_depth = 100)
      Fired -> render `inputs` (a failure -> born FAILED, §4.5);
               start_execution_in(tx, NewExecution { trigger_execution_id: source,
               execution_depth: source + 1, flow_trigger_event_id: event.id,
               trigger: {id, type: Flow, variables}, .. })
      fired, refused, already fired, broken -> done
      a possibly transient error -> savepoint rolled back, pending
    nothing pending, or attempts + 1 = 5 -> DELETE the event (warn for what is pending)
    otherwise -> UPDATE the event SET done, attempts + 1,
                                      next_attempt_at = now() + backoff
  COMMIT
  the transaction failed -> rolled back whole; then, outside it:
    UPDATE ... SET attempts + 1, next_attempt_at = now() + backoff
     WHERE id = (SELECT .. FOR UPDATE SKIP LOCKED); at 5 -> DELETE + the same warn
  500 events this pass, or 1 s spent -> return
```

**Why each line is the way it is:**

1. **The event is deleted, or kept, in the transaction that fires**, so an event's fires and its bookkeeping commit together or not at all. A crash rolls back both, and a later pass retries. The unique index is the backstop.
   - **A `SELECT … FOR UPDATE`, not a `DELETE … RETURNING`.** An earlier draft of this spec deleted the event in the claim. Per-listener retry sometimes KEEPS the event, so the claim locks it and the event's last statement either deletes it or updates it, in the same transaction.
   - **Retries are per listener, not per event.** A listener that hits a transient error rolls back only its own savepoint, and its siblings' fires commit.
   - The event is then kept with those siblings recorded in `done`, `attempts + 1` and a `next_attempt_at` one backoff ahead (the scheduler's tick interval). A later pass evaluates only the listeners not yet in `done`.
   - After 5 attempts the event is dropped and the still-failing listeners are logged at `warn`. A failing listener can therefore neither undo nor block the others.
   - **A transaction that fails as a whole** (a savepoint, the final write or the `COMMIT`) rolls back its `attempts + 1` too, so the failure path counts the attempt again in an autocommit statement of its own. It takes the row `SKIP LOCKED`, so it never waits on a replica that claimed the event since. An event whose transaction always fails is given up at 5 with the same `warn`, rather than retried for ever.
2. **`SKIP LOCKED` partitions events across replicas**, as the W12 claim does. Two events for one listener can be processed concurrently. Only the window arm needs them serialised, and W13b does that with the window's row lock.
3. **The source's render context is read at processing time, not at transition time.** So are the trigger record's `endDate` and `lastTaskId`. For a terminal state they are usually the same. They differ when the source moved on before its event was processed: a terminal source that was replayed or restarted meanwhile renders its later context and later `endDate`/`lastTaskId`. For `PAUSED`, a source that resumed before its event was processed renders a later context. This is a stated divergence from Kestra, which renders the execution it was handed. The alternative is snapshotting the whole render context into each event row. That is not worth the size for a divergence that exists only when the source moves on before its event is processed.
4. **Depth is data, not stack.** `evaluate` reads the source's persisted `execution_depth`, and a fired execution is written with depth + 1. Each fire is its own `start_execution_in` in the processor's loop, not a nested call, so no stack grows, and the cap is Kestra's 100. A chain advances **at least** one hop per pass: an event that a fired execution's FIRST settle writes (a listener on `RUNNING`, an execution born FAILED, a flow that ends in its first settle) is committed with that fire and has a higher id than the cursor, so it is claimed later in the SAME pass, up to the cap. Depth still bounds it.
   - **Every other link carries the depth too, as Kestra's do** (javap on the pinned artifact). A `Subflow` child is written with its parent's depth + 1 and is refused past 100: its task fails with "cannot execute flow '…'.'…': the execution chain exceeded the maximum depth of 100" (`ExecutableUtils.subflowExecution`). A `CREATE_NEW_EXECUTION` replay keeps its original's depth (`ExecutionService.replay` keeps the metadata). A `Loop` iteration keeps its parent's depth and is of kind `LOOP` (`Execution.loopExecution`), which `computeFlowTriggers` refuses as a source, so egret refuses an iteration `NotNormalKind` (its row carries a `loop_index`). Before this, all three were written at depth 0, and a cycle through a loop never reached 100.
5. **Latency.** A hop whose listener runs tasks waits for the worker and then for the next scheduler tick, 1 s by default, so a 10-hop chain of such flows takes about 10 s or more. Hops that end or re-emit in their first settle can take less, as point 4 says. This is accepted and stated.
6. **With the scheduler off, the outbox grows.** `settle` writes events whenever a listener exists, whatever `scheduler_enabled` says, because `settle` does not read config. Turning the scheduler on then processes the backlog, the flow-trigger analogue of cron's `ALL` recovery, at most one capped pass per tick (point 7). An operator who keeps the scheduler off while flows declare flow triggers accumulates one row per heard transition. The backlog is also **uneven**: with the scheduler off, `reconcile` has not run, so only flows saved after migration 0157 have listener rows, and only their triggers' transitions are recorded until the scheduler first starts. This is stated in the chart comment next to `schedulerEnabled`.
7. **A pass is bounded.** It stops after 500 events or once it has run 1 s, checked after each event, and it checks the shutdown token before each claim. The rest waits for the next tick, so a backlog holds neither `fire_due` nor shutdown back. "Each event at most once per pass" is the **cursor**: the claim takes the oldest due event with `id > cursor`, so neither a kept event nor one whose transaction failed can take the pass again. An event that commits with an id lower than the cursor after the cursor passed it (ids are handed out before commit, so commits can land out of order) is the next pass's.
8. **Listeners are selected again on every attempt.** A kept event's retry re-runs the listener `SELECT`, minus `done`. So a listener created, or switched on, after the transition but before the retry can hear that transition. Kestra evaluates the flows that exist when it processes the transition, once. This is a small, stated divergence: it needs a transient failure of another listener on the same event, and it can only add a fire, never lose one.
9. **The transaction runs at `REPEATABLE READ`**, so the source's row, its task runs and its render surface are read in one snapshot. The price is a serialization failure on a write, and the fire's foreign-key check on the source (`FOR KEY SHARE`) is such a write. It succeeds against a source that a worker settled after the snapshot **because the worker locks the execution `FOR NO KEY UPDATE`** (`claim::load_with`), which conflicts with every other writer exactly as `FOR UPDATE` did but not with a key-share check. Under `FOR UPDATE` that check failed `40001`, and a busy source's listeners failed and were given up.

`NewExecution` (W12 Task 3) gains four fields: `trigger_execution_id: Option<Uuid>`, `execution_depth: i32`, `flow_trigger_event_id: Option<i64>` and `born_failed: bool` (§4.5).

### 4.5 The fired execution

- `execution_trigger = { id, type: "io.kestra.plugin.core.trigger.Flow", variables }`. W6 measured part of `variables` (`executionId`, `namespace`, `flowId`, `state`, `flowRevision`, …). **W13a Task 1 measures the whole object** on the pinned artifact, as W12 Task 1 did, and the code reproduces it from that file.
- `inputs` are rendered against the source's context. **A render failure does NOT refuse the fire.** Kestra, measured (W13a Task 1, `listener_run1_FAILED_bad_inputs`), still creates the listener's execution, `CREATED → FAILED`, with `taskRunList: []`, no `inputs`, the full trigger record and depth + 1. egret does the same: the execution is started with `born_failed`, which applies `SetExecutionState { FAILED }` before its first `settle`, so no run is created and the outbox hook emits its `FAILED` event like any other. `inputs` is stored as `{}` (the column is `NOT NULL`). `workflow.executions` has no execution-level message column, so the render error is logged, not stored. Only an unrenderable `when` is a pure refusal with no execution (W6 measured it).
- **Not done, and named:** the `system.correlationId` label (egret has no labels column), and publishing `trigger.*` into the render context. The same holds for cron.

## 5. W13b — `dependsOn` and the window

This follows `FlowTriggerService.processMultipleConditionWindow` and `AbstractJdbcMultipleConditionStateStore.process` (Kestra checkout `96893b589`).

### 5.1 Model

- `FlowTrigger` gains `window`. Kestra 2.x declares it as `Window { deadline | from+to | every+offset | lookback }` and maps it to a `TimeWindow`:
  - `deadline` gives `DAILY_TIME_DEADLINE`;
  - `from` + `to` gives `DAILY_TIME_WINDOW`;
  - `lookback` gives `SLIDING_WINDOW`;
  - anything else gives `DURATION_WINDOW`, with `every` (default 1 day) as the window and `offset` as its advance.
- A declaration that mixes exclusive groups is refused at save.
- `Duration` values are parsed as the ISO-8601 subset the corpus uses. Anything else is refused, not guessed.
- **`timezone` is a seventh field.** It was measured in W13b Task 1: the image's `Window` lists exactly `offset`, `lookback`, `deadline`, `timezone`, `from`, `to` and `every`. It is honoured for every kind except `lookback`.
  - egret supports it (user decision D2). It takes an IANA zone, and an invalid zone is refused at save.
  - With no `timezone`, the artifact aligned on UTC (measured). The container's zone is also UTC, so the claim that Kestra uses the *server* zone comes from the Java and its schema text; it is not measured.
  - egret has no server zone, so the default is UTC. That is chosen, and it equals what the artifact did.
  - Any other field, such as `fireOnce`, is refused at save, as the artifact refuses it with 422.
  - The Flow trigger has no `resetOnSuccess` field, so a fire always resets.
- **Bounds** (`oracle-exec/w13b-windows.json`, `bounds_rule`, 26 cases). There are two `now`s, and they must not be confused.
  - The **bounds** are computed from `now` in the zone, **truncated to the second**.
  - **Validity** uses the **untruncated** `now` (Postgres `now()`, µs): an event counts only if `start < now < end`. Both ends are strict, as in Java's `isValid`.
  - So the event that opens a `SLIDING_WINDOW` or a sub-minute `DURATION_WINDOW` counts. For example, `PT45S` with `now` 04:51:50.560775 gives start 04:51:50.000000, and `event_counted` is true.
  - The artifact cannot place an event on a bound, so strictness is **chosen** from the Java.
- **`DURATION_WINDOW`** (user decision D3). The rule is:
  - If `every` ≥ 1 day, set the hour to 0.
  - If `every` ≥ 1 hour, set the minute to 0.
  - If `every` ≥ 1 minute, set the second to 0 and floor the minute to a multiple of `every`'s whole minutes.
  - Then add `offset`, and `end = start + every − 1 ms`.

  This is **measured** for `every` ∈ {P1D, PT6H, PT90M, PT1H, PT2M, PT45S}, with and without `offset`:
  - `PT6H` at 04:51 gives 04:00–09:59:59.999. It is **not** the midnight grid of 00:00/06:00 that the javadoc describes.
  - `PT2M` at 04:53 gives 04:52.
  - `PT45S` is not aligned at all.

  Other durations use the same rule and are **chosen from the Java, consistent with the measured cases**; they are not refused. Examples are `PT7M` (sub-hour, not dividing 60, so the window restarts at :00), `PT90S` (1 whole minute) and `P2D`.
- **`offset`** can put `start` in the future (measured). An event before `start` is ignored.
- **`SLIDING_WINDOW`** is `[now truncated to the second, + lookback]`, with an exact end. It is fixed once created and **does not slide**, both on the artifact (measured) and in egret.
- **`DAILY_TIME_WINDOW`** is `[midnight + from, midnight + to]` and **`DAILY_TIME_DEADLINE`** is `[midnight, midnight + deadline]`. Both ends are exact (measured).
- **DST: chosen from the Java, not measured.** Task 1's container was UTC, and the only other zones it measured have no DST (Asia/Kolkata, Asia/Ho_Chi_Minh). egret follows `java.time` (controller ruling, consistent with D3):
  - the local edits (truncate to the day, set the hour, minute or second) are re-resolved in the zone;
  - every addition (`from`, `to`, `deadline`, `every`, `offset`, `lookback`) is a **fixed duration added to that instant from local midnight**, not a wall-clock reading. So "`midnight + from`" above means elapsed time.

  On a DST change day this is visible to users:
  - a daily `from: 10:00` window opens at **11:00 local on a spring-forward day** (and at 09:00 local on a fall-back day);
  - a one-day window (`P1D`, the default) is always exactly 24 hours, so on a 23-hour day it **ends an hour into the next local day**.

### 5.2 Schema (migration `0158`)

```sql
CREATE TABLE workflow.flow_trigger_windows (
  flow_id      uuid        NOT NULL,
  trigger_id   text        NOT NULL,
  window_start timestamptz NOT NULL,
  window_end   timestamptz NOT NULL,
  results      jsonb       NOT NULL,   -- {condition -> true}; only true is stored
  outputs      jsonb,
  PRIMARY KEY (flow_id, trigger_id),
  FOREIGN KEY (flow_id, trigger_id) REFERENCES workflow.flow_listeners ON DELETE CASCADE
);
```

**Deleting a listener cascades its window row.** Kestra keeps the row after the listener flow is deleted (measured: `prior/log-fast.json[8]`, finding `listener_delete_keeps_row`). egret's `ON DELETE CASCADE` is a deliberate divergence (user decision D1): with Kestra's behaviour, a flow re-created with the same id would inherit stale results.

### 5.3 Processing a `dependsOn` listener

This runs inside §4.4's per-listener savepoint:

1. **Only an event that matches at least one of the listener's dependencies touches its window.** An event that matches none of them neither opens a window nor purges one.
2. Compute a fresh window at Postgres `now()`, run `INSERT … ON CONFLICT DO NOTHING`, then run `SELECT … FOR UPDATE`.
   - If the row found has **expired** (`end <= now`), replace it with the fresh window's bounds and empty `results`, and evaluate the event against the fresh window. The event counts.
   - For `SLIDING_WINDOW`, this means the window is anchored on the first *matching* event.
3. If `now` is outside the (possibly fresh) window, the event is ignored for this listener and is not stored for later. Examples are `DAILY_TIME_WINDOW` off-hours, a future `offset` start, and a passed `deadline`.
4. Evaluate each `Dependency`: namespace, flowId, `states`, `labels`, and `when` rendered on the source. Merge the true results. A condition already met stays met.
   - **`labels` are matched against the source flow's declared `labels:`, not the source execution's.** egret has no execution labels (§4.5), so this is the same stand-in `executionLabels` uses. It is a divergence from Kestra, which matches the execution's labels, and it fails closed: a dependency that requires a label existing only at run time (`system.correlationId`, or a label an API start adds) never matches.
5. If the trigger's own `when` holds and `mode` is satisfied (`ALL`, `ANY`, or `AT_LEAST` ≥ `minSatisfied`), fire through §4.4's path, then delete the window.
   - The artifact deletes it under `ALL`, `ANY` and `AT_LEAST` alike (measured). `ANY` therefore fires on every matching event, and `AT_LEAST` starts again from nothing.
6. The scheduler pass deletes expired windows (`sweep_expired_windows`). No purge runs after each event.
7. A met condition never carries into the next window (measured, and the same in egret).

**Kestra, as measured: a deliberate divergence (user decision D1).** Steps 1, 2 and 6 differ from the artifact (`oracle-exec/w13b-windows.json`, findings `expiry_drops_the_next_event`, `purge_is_tenant_wide_on_any_evaluation`, `unrelated_event_opens_a_window` and `listener_delete_keeps_row`).

- **An expired window is not replaced on the artifact.** The event that finds it is ignored, and the row is purged afterwards, so only the *next* event opens a fresh window (`q8_expired_unpurged_then_event`).
- **Every execution the listener hears opens a window**, even one matching no dependency. Its `results` are `{}`, and for `SLIDING_WINDOW` that execution becomes the anchor (`q5` part B).
- **The purge runs at the end of every `dependsOn` evaluation**, including one that matched nothing. So whether the first matching event after an expiry is lost depends on whether an unrelated execution came in between (`q8_expired_purged_by_unrelated_execution`).
  - It is measured that the listener's own expired row is purged by an unrelated execution.
  - That the purge covers the whole tenant comes from the Java (`expired(tenant)`); it was not measured.
- egret does not reproduce this, because the outcome would depend on unrelated traffic.

**Concurrency.** Two sources that satisfy two conditions of one listener, processed on two replicas, serialise on the window row, and the listener fires once. The loser of the race fails with a serialisation error (`40001`), rolls its savepoint back, and its event is evaluated again **on a later pass, at that pass's `now()`, without spending an attempt**. It then reads the first one's result. If the window expired meanwhile, the event opens the next one (and for a daily window it can fall outside the window and be ignored, as in step 3). `ON CONFLICT DO NOTHING` + `FOR UPDATE` means a missing row cannot be created twice.

### 5.4 Measured before coded

W13b Task 1 fires real flows on the pinned artifact and saves the results verbatim. The Java above does not settle these questions. **The answers are in `oracle-exec/w13b-windows.json`.** §5.1 and §5.3 above state egret's behaviour. Each marks what is measured and what is chosen, and names the deliberate divergences from the artifact's window lifecycle. A fired execution's `trigger.variables` are the window-free shape unchanged: the 9 keys W13a measured, describing the source that completed the condition.

- `DURATION_WINDOW` bounds: midnight in which zone, and the effect of `offset`.
- How `SLIDING_WINDOW`'s start is set.
- Whether a met condition that did not fire carries into the next window.
- `resetOnSuccess` under `ANY` and `AT_LEAST`.
- An event exactly at `end`.

## 6. Off switches

A listener is not heard (window-free) or does not accumulate (window) when any of these holds:

- its flow is deleted;
- its flow's `state` is `'disabled'`;
- its flow YAML says `disabled`;
- the trigger says `disabled`;
- the workspace's `workflows` feature is off;
- the workspace is not `active`.

These are the six W12 switches. Events that arrive while a listener is off are **dropped for that listener, not queued**. That matches Kestra, where a disabled flow is never a candidate. It is the one place the flow trigger deliberately differs from cron's freeze-and-recover, because an event, unlike a clock, cannot be replayed.

## 7. Tests

Every row names the mutation that must turn it red.

### 7.1 W13a

| property | mutation it must kill |
|---|---|
| `sync` writes, updates and deletes listener rows; `reconcile` fills them for flows saved earlier | drop the flow-trigger branch of `sync` |
| a source reaching SUCCESS produces one event, and a pass fires one execution with the right link, depth, trigger record and inputs | drop the hook |
| PAUSED fires a listener with default `states` | emit only on terminal states |
| no event is written in a workspace without listeners | drop the `EXISTS` |
| refused: same flow, other workspace, non-normal kind, falsy `when`, unrenderable `when` | drop each filter in turn |
| the six off switches | drop each clause in turn |
| a chain A→B→C fires over three passes; a cycle A→B→A stops at depth 100 | drop the depth check; write depth 0 |
| a `Subflow` child is its parent's depth + 1 and refused past 100; a `Loop` iteration and a replay keep their parent's depth | drop the + 1; drop the refusal; write 0 for either copy |
| a `Loop` iteration fires no listener (`NotNormalKind`) | pass kind `None` for every source |
| a source settled between the claim and the fire still fires | lock the execution `FOR UPDATE` in `load_with` |
| a pass stops at 500 events or 1 s and the next pass goes on; a backlog does not hold the cron ticks back; a cancelled pass claims nothing | drop the cap; drop the time cap; drop the cancel check |
| a kept event waits for its backoff, so attempts count ticks | ignore `next_attempt_at` in the claim; set no backoff |
| an event whose transaction always fails is given up after 5 attempts | drop the failure path's count; never give up there |
| the source's workspace is read from its execution row | read the event's |
| a broken listener neither blocks the event's other listeners nor loses the event for them | drop the savepoint |
| an event is processed once across two processors (counted by insert attempts, as W12 did) | drop `SKIP LOCKED` and the index |
| a fault after the fire rolls back the fire and keeps the event | delete the event in its own transaction |
| a draft listener fires | map draft to Kestra's `isDraft` |
| an `inputs` render failure creates a born-FAILED execution, as Kestra does, not a refusal | refuse instead |

### 7.2 W13b

| property | mutation it must kill |
|---|---|
| `ALL` over two dependencies: A alone does not fire; A then B fires once and resets the window | do not merge results |
| an event that finds an expired window replaces it (fresh bounds, empty results) and counts: the deliberate divergence from Kestra's drop | ignore the event, or keep the expired results |
| an event matching no dependency opens no window and purges none | open the window before matching |
| the scheduler sweep deletes expired windows; the event path does not | drop the sweep |
| deleting a listener deletes its window row | drop the cascade |
| `ANY`; `AT_LEAST` with `minSatisfied` | swap the mode comparison |
| two sources on two processors satisfying `ALL` together fire exactly once | drop the window's `FOR UPDATE` |
| an expired window's results do not count | drop the `end` check |
| an off-hours event is ignored for `DAILY_TIME_WINDOW` | drop the in-window check |
| the four `TimeWindow` kinds' bounds match Task 1's measurements (pure tests on `(window, now)` fed from that file) | — |
| a conflicting `window:` is refused at save | drop validation |

## 8. Deliberately not in W13

- The oracle following `trigger_execution_id` (a re-harvest).
- The `system.correlationId` label.
- Publishing `trigger.*` into the render context.
- Backfill.
- `stopAfter`, which W8 found parsed and read by nothing.
- A UI.

## 9. Delivery

- **Docs PR:** this spec (EN + VI), the plan, and the programme spec §9 rows for W13a/W13b.
- **W13a:**
  1. Measure the Flow trigger's `execution_trigger` on the artifact.
  2. Migration 0157 and `sync` for listeners.
  3. `NewExecution` link + depth, and the `settle` hook.
  4. The processor and its tests.
  5. The coverage statement, then the full gate.
- **W13b:**
  1. Measure window bounds on the artifact.
  2. The `window` model, `Duration`, and validation.
  3. The pure `TimeWindow` port.
  4. Migration 0158 and the `dependsOn` processing.
  5. Concurrency tests, the coverage statement, the full gate.

Each task is reviewed. Each PR gets a whole-branch review. Tests run on a dedicated Postgres, as in W12.
