# W12 — The trigger loop: a `cron:` that fires

**Status:** design, awaiting review
**Depends on:** W6 (the pure `trigger::tick` and the cron parser), W10 (the runtime tickers and `start_execution_with_trigger`) — both merged
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §6.1, §6.3, §9 (row W12)
**Scope decided with the user, 2026-09-24:** schedule triggers only; missed schedules recovered as Kestra's default `ALL`.

## 1. What exists and what does not

W6 shipped the part of a scheduler that is hard to get right, and W10 shipped the
part that runs things. Nothing connects them.

| piece | state on `origin/main` (`6fb7520e`) |
|---|---|
| cron parser, timezone, `withSeconds`, nicknames, backfill arithmetic | **built** — `trigger/schedule.rs`, measured against the pinned artifact |
| `trigger::tick(trigger, state, inputs, clock)` | **built, pure** — `TriggerScheduler.processSchedulableTrigger`'s order, `TickOutcome::{NotDue, Skipped, NotSchedulable, Fired}` |
| `TriggerState::initialise` | **one caller, and it is `#[cfg(test)]`** |
| `workflow.triggers` | **no migration creates it** (latest migration is `0153`) |
| a loop that claims due triggers | **none** — `runtime::spawn` starts two tickers, claim and reaper |
| creating an execution with an `ExecutionTrigger` | **built** — `worker::start_execution_with_trigger`, used by the webhook route |

So W12 adds a table, a way to keep that table in step with flow YAML, and a
third ticker, and reuses the rest. **It does not touch `tick`'s semantics.**

Two facts about the write side, checked in `repo.rs` rather than assumed, shape
§3:

- `workflow.flows` is written by exactly **four** statements, all in `FlowRepo`:
  `create` (INSERT), `update` (UPDATE incl. `source`), `soft_delete` (UPDATE
  `deleted_at`) and `set_live` (UPDATE `live_version_id`, `state`). All four run
  on `&self.db.pool` directly, **not in a transaction**.
- Nothing in `crates/egret-workflow/src` writes `state = 'disabled'`, and no path
  clears `deleted_at`. Both are read by the start paths; neither is written by an
  API today.

## 2. What judges W12

**Not the ratchet.** A `Schedule`-fired execution carries no link `harvest.py`'s
settle can walk (W6 Task 8's coverage statement, `oracle-exec/README.md`), so
`baseline.txt` is expected to read **109 before and 109 after**, and it would read
109 if every line of W12 were wrong. This is stated here so the number is not read
as "no progress" or as "verified".

W12 is judged by its own tests (§6), each of which must be **seen to fail** under
a named mutation before it counts — the discipline of
`assertions-that-survive-mutation`. The DB-gated suites must not self-skip: a
missing `DATABASE_URL` is a failure, not an `ok`.

## 3. Schema and sync

### 3.1 The table

Migration `NNNN_workflow_triggers.sql`, where `NNNN` is the next free number **when the task runs**: `0154` on `origin/main` today, but the unmerged SP4a branch (`feat/sp4a-permission-model`) already holds `0154` and `0155`, so whichever merges second renumbers:

```sql
CREATE TABLE workflow.triggers (
  flow_id              uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id           text        NOT NULL,   -- the trigger's `id:` in the YAML
  workspace_id         uuid        NOT NULL,   -- copied from the flow; see below
  spec_key            text        NOT NULL,   -- canonical "cron|withSeconds|timezone", not a digest
  disabled             boolean     NOT NULL,   -- flow YAML `disabled` OR trigger `disabled`
  next_evaluation_date timestamptz,            -- NULL = not queued (see 3.3)
  evaluated_at         timestamptz,
  last_error           text,
  updated_at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_triggers_due
  ON workflow.triggers (next_evaluation_date)
  WHERE next_evaluation_date IS NOT NULL AND NOT disabled;
```

- **One row per `Schedule` trigger**, not per trigger of any kind. Webhooks are
  pushed by HTTP and need no row; flow triggers are out of scope (§7).
- `workspace_id` is copied so that logs and later admin views can filter without a
  join. It is not an authority: firing reads the flow row in the same statement.
- **`disabled` is a column because it cannot be a predicate at fire time.**
  Kestra's `DefaultSchedulableTriggerFetcher` drops a trigger whose flow *or*
  whose own declaration is disabled **before** evaluation (the bytecode is quoted
  on `TickOutcome::NotSchedulable`): the state is frozen, not consumed. A loop
  that discovered this only after claiming would leave the row due, claim it
  again next pass, and — with `ORDER BY next_evaluation_date LIMIT 1` — claim it
  **first** every pass, starving every trigger behind it. Both flags live in the
  YAML, so `sync` writes them and the claim filters on them.
- **No lease column.** §4 holds the row lock for the whole fire, so there is
  nothing to reclaim.
- `TriggerState`'s other fields map one to one; `backfill` is not stored, because
  no W12 path creates one (§7). `namespace`/`flow_id` strings are read from the
  flow at fire time rather than copied, so a rename cannot leave them stale.

### 3.2 A second column on executions, so a double fire cannot be written

Same migration:

```sql
ALTER TABLE workflow.executions ADD COLUMN scheduled_date timestamptz;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL;
```

plus a CHECK that `scheduled_date IS NOT NULL` **iff** `execution_trigger->>'type'`
is `io.kestra.plugin.core.trigger.Schedule`.

§4's `SKIP LOCKED` is what prevents a double fire; this index is what makes one
**unrepresentable** if §4 is ever wrong (`unrepresentable-beats-unforgettable`).
It is a typed column rather than an index over `execution_trigger->'variables'`
because the text form of those variables is not yet measured (§5), and a
uniqueness guarantee must not depend on a formatting decision.

### 3.3 Sync: one function, called at every write that changes what fires

The DB-facing code lives in a **new module, `src/scheduler/`**, not in
`src/trigger/`: `trigger/mod.rs`'s own doc promises that nothing in that tree
reaches a database, a container or a network, and its tests rely on it. `trigger/`
stays pure; `scheduler/` is its only I/O caller.

`scheduler::sync(conn, flow: &FlowRow, gone: bool)` (it parses `flow.source` itself unless `gone`), which reads `now()`
from the same transaction — the loop's clock (§4 point 4), so a first
`next_evaluation_date` is never computed on one clock and compared on another:

1. Collect the flow's `Schedule` triggers (none if `gone`, or if the
   source does not parse, or the flow is being deleted).
2. **Delete** rows whose `trigger_id` is no longer declared.
3. For each declared schedule, compute `spec_key` and resolve it with
   `ScheduleTrigger::resolve`:
   - **new row, or `spec_key` changed** → upsert with
     `TriggerState::initialise` (the **next** cron tick after `now`, never `now`),
     `evaluated_at = NULL`, `last_error = NULL`;
   - in every case, `disabled` = the flow's `disabled:` OR the trigger's
     `disabled:`; toggling it does **not** touch the dates, so re-enabling
     recovers the missed ticks under `ALL`;
   - **`spec_key` unchanged** → leave `next_evaluation_date` and `evaluated_at`
     **untouched**. Editing a task must not reset a schedule;
   - **resolve refused** (bad cron, `lateMaximumDelay`, a `when` it cannot
     render) → upsert with `next_evaluation_date = NULL` and the refusal in
     `last_error`;
   - **resolved, but no future tick** (`0 0 30 2 *`: a cron that never fires) →
     the same: `next_evaluation_date = NULL`, and `last_error` says the schedule
     never fires (final review I4).

   The NULL-and-reason rule applies when the row is (re-)initialised. A refusal
   whose cause is not part of `spec_key`, met by a row that is already queued
   under an unchanged key, keeps that row's date; the claim finds the refusal at
   fire time and takes the row off the queue then (§4 point 6).

The four writers become transactional and call it:

| writer | sync call |
|---|---|
| `create` | `sync(tx, row, false)` |
| `update` | `sync(tx, row, false)` — also when only `namespace`/`folder_id` changed; it is a no-op then |
| `soft_delete` | `sync(tx, row, true)` — deletes every row. Kestra deletes a deleted flow's trigger state; keeping it would make a future restore replay the whole time in the trash under `ALL` |
| `set_live` | **exempt**, named: it changes neither `source` nor `deleted_at`, and `state` is read at fire time (§4) |

**The guard against a fifth writer that forgets:** a test scans the non-test half
of `crates/**/src` for statements that `INSERT INTO`, `UPDATE` or `DELETE FROM`
`workflow.flows`, and asserts each one sits in a function that calls
`scheduler::sync` or is on an exemption list whose entries carry a reason. It has a
positive control (it must find exactly the four above today), so it cannot pass
by scanning nothing.

**Flows that existed before the table (final review I1).** The migration creates
`workflow.triggers` empty, and only the writers above call `sync`, so a flow saved
before it and never saved again would never get a row. The scheduler ticker
therefore runs `sync` over every non-deleted flow once when it starts, before its
first pass, each flow in its own transaction. It is idempotent — an unchanged
`spec_key` keeps its dates — and a flow whose `sync` fails is logged and skipped,
never an aborted start. Like a save, it re-queues a row `fire_due` took off the
queue, and removes the rows of a flow whose source no longer parses.

A flow whose `state` becomes `disabled` keeps its rows and its
`next_evaluation_date`; §4 simply does not fire it. When it is enabled again, the
missed ticks are recovered under `ALL`. That is Kestra's `recoverMissedSchedules`
default and the user's decision; no path writes `disabled` today, so this is the
rule for the path that eventually will.

## 4. The loop

A third ticker in `runtime::spawn`, `trigger_interval` apart. **A pass evaluates
every due trigger once**, which is `TriggerScheduler`'s own cycle, **one trigger
per transaction**, and keeps the `(flow_id, trigger_id)` pairs it has already
handled in `seen`:

```text
BEGIN
  SELECT t.*, f.id, f.workspace_id, f.source, now() AS db_now
    FROM workflow.triggers t
    JOIN workflow.flows f ON f.id = t.flow_id
   WHERE t.next_evaluation_date <= now()
     AND NOT t.disabled
     AND f.deleted_at IS NULL
     AND f.state <> 'disabled'
     AND NOT EXISTS (SELECT 1 FROM workspace_features wf
                      WHERE wf.workspace_id = f.workspace_id
                        AND wf.feature = 'workflows' AND NOT wf.enabled)
     AND (t.flow_id, t.trigger_id) NOT IN (SELECT * FROM unnest($seen_flows, $seen_ids))
   ORDER BY t.next_evaluation_date
   LIMIT 1
   FOR UPDATE OF t SKIP LOCKED
  -- none -> ROLLBACK, pass ends
  parse f.source; find the Schedule whose id = t.trigger_id; ScheduleTrigger::resolve
  -- any failure -> UPDATE t SET next_evaluation_date = NULL, last_error = <why>; COMMIT
  tick(trigger, state_from(t), schedule.inputs, FixedClock(db_now))
  Fired(e) -> start_execution_in(&mut tx, ws, f.id, f.source, inputs, trigger_record(e),
                                scheduled_date = e.scheduled_date)
  UPDATE t SET next_evaluation_date, evaluated_at, last_error = NULL/<skip reason>
COMMIT
```

**Why each line is the way it is:**

1. **One transaction around fire and advance.** `start_execution_with_trigger`
   opens its own transaction from the pool today. W12 extracts its body into
   `start_execution_in(conn, …)` and keeps the old function as a wrapper, so the
   webhook, the API start and every test keep their behaviour. A crash anywhere
   before `COMMIT` rolls back both the execution and the state change, and the
   next pass fires the same tick again. **A tick is fired exactly once or not at
   all** — never fired-and-not-advanced, never advanced-and-not-fired.
2. **`SKIP LOCKED` is the exclusion, and it covers this statement only.** N
   replicas partition the due set exactly as the worker's claim does. The other
   statements in the transaction run under that row lock. `runtime.rs`'s rule —
   one safety statement per SQL statement, not one per loop — gets a W12 section.
3. **No lease.** The row lock is held for the whole fire; the work inside it is a
   parse, a pure function and the first `settle` of a new execution — no handler,
   no network. There is nothing slow enough to need a lease, so there is nothing
   for a reaper to reclaim.
4. **One clock, and it is Postgres's.** The due predicate and `tick` both use the
   `now()` the claim returned, handed to `tick` as a `FixedClock`. Replicas with
   skewed clocks therefore agree, and W6's rule that `SystemClock::now` is the only
   wall-clock reader under `src/trigger/` still holds — `FixedClock` joins
   `TestClock` in that module.
5. **All six ways to switch a trigger off are honoured, and none consumes a
   tick.** `deleted_at` and the `state` column are read from the flow in the
   `WHERE`; the flow's YAML `disabled: true` and the trigger's own are the
   `disabled` column `sync` wrote (§3.1); the workspace's `workflows`
   feature, switched off, is a `NOT EXISTS` over `workspace_features` — the
   table, and the `enabled = false` reading, that the route gate in `WsContext`
   uses, so the two cannot disagree (final review I3; the user's decision of
   2026-09-25); and the workspace's own `status`, an `EXISTS` over `workspaces`
   requiring `status = 'active'` — the same column and value `WsContext`
   (`crates/egret-workspace/src/context.rs`) and the webhook route
   (`webhook_routes.rs`) already refuse a non-active workspace on, so an
   archived workspace cannot disagree with them either (the user's decision of
   2026-09-25). All six leave the state frozen, which is `NotSchedulable`'s
   measured behaviour, so switching any of them back on recovers the missed
   ticks under `ALL`. **The YAML is re-checked after `parse` anyway**, because
   the source read under the lock is the authority and the column is a copy: if
   they disagree — the trigger is off, no longer declared, or declares a cron
   whose `spec_key` is not the row's — the loop does not fire, leaves the dates
   alone, and re-runs `sync` for that flow — the same third door
   `subflow::spawn_one` closes (the W10 Critical). The `spec_key` comparison is
   not hypothetical (final review I2, measured): a claim that waited on a save
   re-reads the newest trigger row under `FOR UPDATE OF t` but keeps its
   snapshot's flow row, so without it a pass could fire the old cron and write
   its dates under the new key — permanently, since a same-key save keeps dates.
   `sync`'s own statement reads the committed source.
6. **A broken trigger is taken off the queue, not retried each pass.** Setting
   `next_evaluation_date = NULL` stops a flow whose draft no longer parses from
   costing one failed transaction per second for ever. The next `sync` — i.e. the
   next save of that flow — puts it back.

**Why `seen`, and not a `batch` limit.** A trigger that is days behind always
holds the oldest `next_evaluation_date`. Under `ORDER BY … LIMIT n` it would be
claimed first on every iteration — its advanced date is *still* the oldest — so
it would take the whole batch of every pass and on-time triggers would wait until
it caught up. `seen` makes "once per pass" a property of the query rather than of
luck, and processing the whole due set bounds a pass by the number of triggers,
not by a tuning knob. There is therefore no `batch` for this loop.

**Catch-up under `ALL`.** `tick` advances one tick per call and a pass calls it
once per trigger. Consequence, derived rather than measured: a `* * * * *` cron
after a three-day outage owes ~4,320 executions and, at the default 1 s interval,
takes ~72 minutes to catch up, during which every other trigger still fires once
per pass. A flow author who does not
want a backlog replayed uses `lateMaximumDelay`, which W12 **still refuses** (§7);
until that lands, the only lever is to disable and re-save.

**Config.** `WorkflowConfig` gains `scheduler_enabled` (default **false**, the same
reasoning as `worker_enabled`) and `trigger_interval_secs` (default 1). The gate
lives in `RuntimeConfig`, as W10's does, so a test can reach it. Scheduler on with
worker off logs a warning at start: executions will be created and nothing will
run them.

## 5. The execution a schedule creates

- `execution_trigger = { id: <trigger id>, type: "io.kestra.plugin.core.trigger.Schedule", variables: { date, next, previous } }`.
  The **text form** of those three values is `Schedule.Output.toMap(ZoneId)`
  through Kestra's Jackson mapper, which `ScheduledExecution`'s doc records as
  **unmeasured**. Task 1 measures it on the pinned artifact
  (`kestra/kestra@sha256:28f59bf1…`) by firing a real schedule and reading the
  stored execution. If the measurement cannot be made, the spec is amended to say
  the format is chosen, and which.
- `scheduled_date` = `ScheduledExecution.scheduled_date` (§3.2).
- `inputs` = `Schedule.inputs`, **verbatim**. Kestra renders them through a
  `RunContext`; egret has no render context at this seam. A value that contains
  template syntax (`{{` or `{%`) is **refused** — the tick is consumed and the
  reason goes to `last_error` — rather than run with the literal string, which
  would be over-acceptance in the direction the oracle cannot see.
  `spec::validate`'s `schedule_missing_inputs` already refuses a schedule that
  leaves a required input unset, at write time.
- `trigger.*` is stored and **not** published into the render context — the same
  state migration `0140` left the webhook in. W12 does not change it.

## 6. Tests

Every row names the mutation that must turn it red. A row whose mutation survives
is reported as unproved, not as covered.

| property | test | mutation it must kill |
|---|---|---|
| sync creates, deletes and preserves | add/remove a `Schedule`; edit an unrelated task → `next_evaluation_date` unchanged; change the cron → re-initialised to the next tick | drop the `spec_key` comparison |
| sync on delete | `soft_delete` removes every row | drop the `soft_delete` call |
| every writer syncs | the scanner in §3.3, positive control = 4 | delete the `sync` call in `update` |
| fire exactly once | `next_evaluation_date` one tick in the past, two passes → one execution with a whole `execution_trigger` and `scheduled_date` | drop the state `UPDATE` |
| replicas never double fire | two tickers, one DB, N due triggers → each `(trigger, date)` has exactly one execution; asserted on executions, not counts | drop `SKIP LOCKED` |
| the unique index is a real backstop | hand-insert a second execution for the same `(flow, trigger, date)` → refused | drop the index |
| fire + advance are atomic | a fault injected after `start_execution_in`, before `COMMIT` → no execution, state unchanged | split into two transactions |
| `ALL` catch-up | `next_evaluation_date` five ticks in the past → five passes → five executions, `scheduled_date` ascending, one per pass | tick until idle within one pass |
| a trigger that is behind does not starve the rest | trigger A five ticks behind, trigger B one tick due → the first pass fires one of each | drop the `seen` exclusion |
| a broken trigger does not jam the loop | unparseable draft → `last_error` set, `next_evaluation_date` NULL; a sibling trigger in the same pass still fires | drop the take-off-queue branch |
| every off switch | `state='disabled'`, YAML `disabled: true`, trigger `disabled: true`, `deleted_at`, workspace feature `workflows` off, workspace `status='archived'` → zero executions **and** `next_evaluation_date` unchanged; the switch back on → the missed ticks fire | drop each clause in turn |
| a disabled trigger does not starve the queue | the oldest due row is disabled, a younger one is enabled → the younger fires, and the disabled row is never claimed | drop `NOT t.disabled` from the claim |
| re-enable recovers under `ALL` | disable, set `next_evaluation_date` three ticks in the past, enable → three passes, three executions | reset the dates when `disabled` toggles |
| templated inputs refused | `inputs: {x: "{{ now() }}"}` → no execution, reason recorded | drop the check |
| default off | `scheduler_enabled = false` → no statement issued | flip the default |
| end to end | `tests/runtime.rs`: a flow with `*/1 * * * *`, worker and scheduler on, its row's `next_evaluation_date` written one minute in the past → the execution reaches `SUCCESS` with nothing calling `tick` by hand | — |

The clock in the DB tests: the claim's `now()` is Postgres's, so tests that need a
due trigger write `next_evaluation_date` in the past rather than moving a Rust
clock. `TestClock` stays for the pure tests of `tick`.

## 7. Deliberately not in W12

Recorded in the programme spec's §9 so each has an owner or an explicit "none":

- **Flow triggers** (`trigger/flow.rs`, window-free arm) — event-driven, belongs
  in the worker's settle transaction, not in a claim table. Proposed **W13**.
- **Backfill API** — `tick` supports it; no endpoint creates one and the corpus has
  zero live declarations.
- **`lateMaximumDelay`** — still refused at `ScheduleTrigger::resolve`, because
  parsing `java.time.Duration` is unmeasured.
- **`stopAfter`**, and `concurrency` / `allowConcurrent` between consecutive cron
  runs.
- **Publishing `trigger.*` into the render context.**
- **A UI for triggers** — W7.

## 8. Delivery

One docs PR (this spec, en + vi, plus the programme-spec §6.1/§6.3/§9 updates),
then one code PR in six tasks:

1. Measure the `Schedule` output format on the artifact (§5).
2. Migration `NNNN` (§3.1) + `scheduler::sync` + transactional `FlowRepo` writers + the
   writer scanner.
3. `start_execution_in` extracted; the old function becomes its wrapper.
4. The fire transaction (`scheduler::fire_due`), `FixedClock`, the off switches and
   the refusal paths.
5. The ticker, `WorkflowConfig`, the safety sections in `runtime.rs`, the
   concurrency and end-to-end tests.
6. Whole-branch review, mutation table filled in, full local CI gate.
