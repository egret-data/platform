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

`flow_trigger::process_events(&PgPool) -> Result<EventReport>` runs from the **W12 scheduler ticker**, after `fire_due`, behind the same `scheduler_enabled` switch. A pass drains the outbox **one event per transaction**:

```text
BEGIN
  DELETE FROM workflow.flow_trigger_events
   WHERE id = (SELECT id FROM workflow.flow_trigger_events ORDER BY id
               LIMIT 1 FOR UPDATE SKIP LOCKED)
  RETURNING *                                    -- none -> pass ends
  load the source execution (namespace, flow key, kind, execution_depth, render surface)
  SELECT listeners: same workspace, state in states, flow <> source, not disabled,
         listener flow not deleted, state <> 'disabled', workflows feature on,
         workspace active                       -- the six W12 off switches
  per listener, in a SAVEPOINT:
    parse listener source; WindowFreeFlowTrigger::resolve
    render `when` and `inputs` against the SOURCE's context
    trigger::flow::evaluate(.., max_depth = 100)
    Fired -> start_execution_in(tx, NewExecution { trigger_execution_id: source,
             execution_depth: source + 1, flow_trigger_event_id: event.id,
             trigger: {id, type: Flow, variables}, .. })
    error -> classify as W12 does; a listener's failure never undoes another's fire
COMMIT
```

**Why each line is the way it is:**

1. **The event is deleted in the transaction that fires**, so an event is processed exactly once or not at all. A crash rolls back both the fire and the delete, and the next pass retries. The unique index is the backstop.
   - **Retries are per listener, not per event.** A listener that hits a transient error rolls back only its own savepoint, and its siblings' fires commit.
   - The event is then kept with those siblings recorded in `done` and `attempts + 1`. The next pass evaluates only the listeners not yet in `done`.
   - After 5 attempts the event is dropped and the still-failing listeners are logged at `warn`. A failing listener can therefore neither undo nor block the others.
2. **`SKIP LOCKED` partitions events across replicas**, as the W12 claim does. Two events for one listener can be processed concurrently. Only the window arm needs them serialised, and W13b does that with the window's row lock.
3. **The source's render context is read at processing time, not at transition time.** For a terminal state they are the same. For `PAUSED`, a source that resumed before its event was processed renders a later context. This is a stated divergence from Kestra, which renders the execution it was handed. The alternative is snapshotting the whole render context into each event row. That is not worth the size for a divergence that exists only on non-terminal states.
4. **Depth is data, not stack.** `evaluate` reads the source's persisted `execution_depth`, and a fired execution is written with depth + 1. A chain runs one hop per pass, so no stack grows, and the cap is Kestra's 100.
5. **Latency.** One hop per scheduler tick, 1 s by default. A 10-hop chain takes about 10 s. This is accepted and stated.
6. **With the scheduler off, the outbox grows.** `settle` writes events whenever a listener exists, whatever `scheduler_enabled` says, because `settle` does not read config. Turning the scheduler on then processes the backlog, the flow-trigger analogue of cron's `ALL` recovery. An operator who keeps the scheduler off while flows declare flow triggers accumulates one row per heard transition. This is stated in the chart comment next to `schedulerEnabled`.

`NewExecution` (W12 Task 3) gains three fields: `trigger_execution_id: Option<Uuid>`, `execution_depth: i32` and `flow_trigger_event_id: Option<i64>`.

### 4.5 The fired execution

- `execution_trigger = { id, type: "io.kestra.plugin.core.trigger.Flow", variables }`. W6 measured part of `variables` (`executionId`, `namespace`, `flowId`, `state`, `flowRevision`, …). **W13a Task 1 measures the whole object** on the pinned artifact, as W12 Task 1 did, and the code reproduces it from that file.
- `inputs` are rendered against the source's context. A render failure refuses the fire, as `WhenUnrenderable` does.
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

### 5.3 Processing a `dependsOn` listener

This runs inside §4.4's per-listener savepoint:

1. Run `INSERT … ON CONFLICT DO NOTHING` for a fresh window computed at Postgres `now()`, then `SELECT … FOR UPDATE`. An expired window is replaced.
2. If `now` is outside the window (for example `DAILY_TIME_WINDOW` off-hours), the event is ignored for this listener.
3. Evaluate each `Dependency`: namespace, flowId, `states`, `labels`, and `when` rendered on the source. Merge the true results; a condition already met stays met.
4. If the trigger's own `when` holds and `mode` is satisfied (`ALL`, `ANY`, or `AT_LEAST` ≥ `minSatisfied`), fire through §4.4's path. Then delete the window when it is satisfied (`resetOnSuccess`, default true).
5. The scheduler ticker also deletes expired windows once per pass.

**Concurrency.** Two sources that satisfy two conditions of one listener, processed on two replicas, serialise on the window row. The second one reads the first one's result and fires once. `ON CONFLICT DO NOTHING` + `FOR UPDATE` means a missing row cannot be created twice.

### 5.4 Measured before coded

W13b Task 1 fires real flows on the pinned artifact and saves the results verbatim. The Java above does not settle these questions:

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
| a broken listener neither blocks the event's other listeners nor loses the event for them | drop the savepoint |
| an event is processed once across two processors (counted by insert attempts, as W12 did) | drop `SKIP LOCKED` and the index |
| a fault after the fire rolls back the fire and keeps the event | delete the event in its own transaction |
| a draft listener fires | map draft to Kestra's `isDraft` |

### 7.2 W13b

| property | mutation it must kill |
|---|---|
| `ALL` over two dependencies: A alone does not fire; A then B fires once and resets the window | do not merge results |
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
