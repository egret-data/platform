# Workflow Subflow scheduleDate (package #12b) — design

**Status:** brainstormed + decisions locked 2026-11-27 (DRAFT in scratchpad; formalize to
`docs/{en,vi}/specs/2026-11-27-workflow-subflow-scheduledate-design.md` on the #12b worktree
off main-after-#12 `4796619ea`). Measured against Kestra `96893b589`.
**Roadmap:** the piece split out of #12 (Subflow). The other four Subflow keys shipped in #12
(#920); `scheduleDate` was deferred because it needs a new deferred-execution-start subsystem.

## 1. Goal

Honour the Subflow task's `scheduleDate` (a `Property<ZonedDateTime>`): when set to a future
instant, the spawned child execution does not START until that instant; it is created and waits.
A `wait: true` parent waits across the delay as for any child. Currently `scheduleDate` is
accepted-but-inert, warned via `Lift::Deferred("roadmap package #12b lifts it")`.

## 2. Measurement (Kestra `96893b589` + egret)

- **Kestra** (`ExecutableUtils.java:228-250`): `scheduleDate` is rendered against the parent
  runContext and passed to `Execution.newExecution(flow, inputs, labels, scheduleDate, kind)` —
  it is set **on the child execution itself** at creation. Kestra's executor does not start an
  execution before its `scheduledDate`; the child sits CREATED until due. (NOT via
  `ExecutionDelay`, which is Kestra's RESUME_FLOW/pause-resume machinery.) Task 0 pins the
  past/invalid-date handling.
- **egret has no defer-start seam:** `worker::subflow::spawn_one` drives the child's first
  `settle` immediately, inside the caller's transaction (the deep nested-settle design). Nothing
  consults a start time.
- **egret's `scheduled_date` is the wrong column:** it is the scheduler's tick-identity /
  double-fire guard, and the CHECK `workflow_executions_scheduled_date_iff_schedule` (0156,
  narrowed by 0244 in 11e) FORBIDS a Subflow child (NULL `execution_trigger`) from writing it.
- **egret's reusable "wake at time T" pattern:** the retry delay — `executor::retry::reopen_due`
  + `TaskRun::not_before` + the waker pass `worker::claim::due_retries` / `worker::replay::spawn_due`
  (poll runs whose delay has come due, reopen them). #12b models the deferred child-start on this
  poll-due-and-wake pattern.

## 3. Architecture

A **dedicated deferred-start mechanism**, independent of `scheduled_date`:

### 3.1 New column (migration 0246)
`ALTER TABLE workflow.executions ADD COLUMN scheduled_start timestamptz;` — NULL for every normal
execution; set only on a Subflow child whose `scheduleDate` renders to a future instant. No CHECK
coupling to `execution_trigger` (it is a different concept from `scheduled_date`). An index on
`scheduled_start WHERE scheduled_start IS NOT NULL` for the waker's poll.

### 3.2 Defer at spawn (`worker::subflow::spawn_one`)
When the Subflow declares `scheduleDate`, render it against the parent context:
- **future instant** → insert the child row with `scheduled_start = T` and `state = CREATED`, and
  **do NOT drive the first settle**. The child is created but parked.
- **past / now / invalid-but-rendered** → start immediately (current behaviour); Task 0 pins
  Kestra's exact past/invalid rule (likely "start now" for past; a render error fails the Subflow
  run as other Subflow render errors do).

### 3.3 The waker (reuse the existing worker poll cadence)
A new query hooked into the existing worker/scheduler poll loop (the same cadence `reopen_due` /
`spawn_due` run on — Task 0 pins the exact seam): `SELECT parked children WHERE scheduled_start
<= now() AND state = CREATED AND <not yet settled>`, and for each, drive its first `settle`
(the same first-settle `spawn_one` would have driven), then clear/leave `scheduled_start` so it is
not re-woken. Idempotent: a child already past CREATED is skipped; two wakers racing one child are
guarded by a row lock / the settle's own idempotency (Task 0 confirms the guard).

### 3.4 Parent wait + topology
`wait: true` parent waits for the child's terminal exactly as today — the defer only delays
CREATED→RUNNING. The topology/executions UI shows the parked child as CREATED with its
`scheduled_start` (a small DTO addition, optional for this package — Task 0 decides whether to
surface it or defer the UI bit).

### 3.5 Reconcile
Lift `scheduleDate` from `Lift::Deferred` → honoured; `measure.rs` read-set already has it via
`wire_fields_of::<Subflow>()` (the field exists), so regenerating flips `Subflow.scheduleDate`
ignored→honoured. Census 1261 → **1262**. Update README + the #12 docs note ("coming later" →
documented).

## 4. Decisions

- **D1** dedicated `scheduled_start` column (migration 0246), NOT an overload of `scheduled_date`
  (which would re-broaden the CHECK 11e just narrowed and conflate tick-identity with deferral).
- **D2** the waker reuses the existing worker poll cadence (the `reopen_due`/`spawn_due` seam), no
  new timer/loop.
- **D3** future `scheduleDate` → park CREATED; past/now → start immediately; a render error fails
  the Subflow run (Task 0 pins the exact Kestra rule).
- **D4** the parked child sits in `CREATED`; `wait: true` parent waits across the delay unchanged.
- **D5** census: `scheduleDate` lifts to honoured (+1 → 1262); `Lift::Deferred` for it is removed.
- **D6** migration 0246 (next free after 0245).

## 5. Task 0 — load-bearing measurement

1. Kestra: exact `scheduleDate` semantics — rendered against the parent context (confirm); the
   child execution's state while deferred; past-date and invalid/unrenderable-date handling; how a
   `wait:true` parent and `transmitFailed` interact with a deferred child; whether a `scheduleDate`
   in the past is clamped to now or runs immediately.
2. egret: `spawn_one`'s exact first-settle drive point (where to branch to "park vs settle"); the
   exact worker poll seam the waker hooks (`reopen_due`/`spawn_due`/`claim` loop) and its cadence;
   how the child's first settle is driven OUTSIDE the parent's spawn transaction when woken later;
   any idempotency/row-lock guard needed so the waker starts a child exactly once.
3. The migration shape (column + index) and whether any uniqueness/double-start guard is needed;
   next free number (0246); confirm no CHECK on the new column.
4. Census baseline (post-#12: 1261/3/363/120/1747); the `scheduleDate` lift site (`keys.rs`
   `Lift::Deferred`) + the warning-assertion tests to flip; the #12 docs note to update.

## 6. Testing

- A Subflow with `scheduleDate` a short future instant → the child is CREATED and does NOT start
  until the instant; after the waker runs it starts, runs, and the `wait:true` parent completes.
- A past `scheduleDate` → the child starts immediately.
- An unrenderable `scheduleDate` → the Subflow run fails (as other Subflow render errors).
- The waker starts a due parked child exactly once (no double-start under a racing poll).
- Census golden regenerated: `Subflow.scheduleDate` ignored→honoured (1262).
- Scoped runs include the subflow/executor/restart/routes/execution_timestamps suites + the waker's
  test file; tick/waker-driving tests hold the file-local serialising mutex.

## 7. Divergences (to document)

- egret starts a deferred child from a poll cadence (best-effort around the worker loop interval),
  not Kestra's executor-held scheduledDate; the start may lag the instant by up to one poll.
- `scheduleDate` is parsed as RFC3339 only. Kestra's `ZonedDateTime` rendering is more lenient, so a bare date such as `2026-01-01` that Kestra accepts fails the Subflow run in egret.
- `created_at` becomes mutable for a woken parked child: the wake rewrites it to `now()` so the child's clock (`not_before`, attempt timings) starts when it starts, not when it was parked. Only the graph tie-break that orders by `created_at` can notice (a narrow note); a child that already started is never moved.
