# Workflow Subflow parity (package #12) — design

**Status:** brainstormed + decisions locked 2026-11-26 (DRAFT in scratchpad; formalize to
`docs/{en,vi}/specs/2026-11-26-workflow-subflow-design.md` on the #12 worktree off main at
implementation time). Measured against local Kestra source `96893b589`.
**Roadmap:** egret = Kestra OSS engine; 20-package roadmap #12 (Subflow), after #11.

## 1. Goal

egret already runs the `io.kestra.plugin.core.flow.Subflow` task (spawns another flow's
execution; honours namespace/flowId/inputs/wait/transmitFailed/retry/timeout/allowFailure/
allowWarning). Package #12 honours the **five Subflow keys egret currently ignores**
(census: all five `ignored`), which `spec/keys.rs` `lift_of` maps to `Package(12)`:

- **labels** — labels on the spawned child execution (rendered).
- **inheritLabels** — when true, the child also inherits the parent execution's labels.
- **revision** — run a specific revision of the subflow (default: latest).
- **scheduleDate** — delay the child's start until a datetime.
- **restartBehavior** — `RETRY_FAILED` (restart the existing child in place — already egret's
  behaviour since 7a) vs `NEW_EXECUTION` (on parent restart, spawn a fresh child).

Plus the roadmap follow-up: **"LoopUntil Subflow child matched by position only"** (7a note) —
match a restarted/looped Subflow child to its source correctly, not by list position alone.

Not in scope: `logLevel`/`logToFile`/`version`/`assets`/`taskCache` (→ #13),
`workerSelector`/`policyRefs` (Enterprise) — stay ignored/warned.

## 2. Measurement (Kestra `96893b589`, `core/.../flow/Subflow.java`)

`class Subflow extends Task implements ExecutableTask<Output>, ChildFlowInterface`.
- `revision: Integer` (optional; default = most recent revision).
- `labels: List<Label>`; `inheritLabels: Property<Boolean> = false`.
- `scheduleDate: Property<ZonedDateTime>` (delay the child start).
- `restartBehavior: RestartBehavior = RETRY_FAILED` (enum: RETRY_FAILED | NEW_EXECUTION).
- `wait = true`, `transmitFailed = true` (already honoured). `Output{executionId, state, outputs}`.
- Class doc: "If the parent restarts, previously started subflows are restarted too" — governed
  by restartBehavior.

egret today: `TaskKind::Subflow(Subflow)` (models/task.rs:97/687); the five keys parse into the
model but are read by nothing (census `ignored`). `visible_labels` (labels.rs) already computes
flow-then-own label order (used by trigger labels in 11b/11c). Flow revisions exist (7b replay
`source=version:N`). `workflow.executions.scheduled_date` + the scheduler due-gate exist (Schedule).
Restart-in-place machinery exists (7a/7b `restart::open`, Subflow child restart).

## 3. Architecture

Each key is honoured at the Subflow spawn/restart seam (`worker/handlers/subflow.rs` + the
child `NewExecution` build + `restart`). No new trigger/flowable machinery.

### 3.1 labels + inheritLabels (light)
Render the Subflow `labels` (values rendered against the task context) and thread them into the
child `NewExecution.labels`. When `inheritLabels` renders true, prepend the parent execution's
labels (Kestra's order — confirm in Task 0). Reuse `visible_labels`/the 11c labels threading.

### 3.2 revision (reuse 7b flow-version)
When `revision` is set, resolve the subflow's flow source at that revision via the same
version machinery 7b uses (`source=version:N`); default = latest (current behaviour). A revision
that does not exist → the existing Subflow-not-found/refusal path (Task 0 pins the exact error).

### 3.3 scheduleDate (reuse scheduled_date + due-gate)
When `scheduleDate` renders a future datetime, create the child execution with
`scheduled_date = scheduleDate` and let the scheduler/worker due-gate start it only when due
(the Schedule mechanism). `wait:true` parent waits across the delay as it waits for any child.
Task 0 pins where the child start is gated on `scheduled_date`.

### 3.4 restartBehavior (heavy — reuse 7a/7b restart)
- `RETRY_FAILED` (default): the existing behaviour (7a restarts the failed child in place).
- `NEW_EXECUTION`: on a parent restart that re-runs the Subflow task, spawn a FRESH child
  execution instead of restarting the existing one. Reuse `restart::open`'s Subflow seam
  (7a/7b). Task 0 pins Kestra's exact NEW_EXECUTION semantics (does the old child keep running?
  is it killed? what links to the parent?).
- Follow-up: fix "LoopUntil Subflow child matched by position only" so a looped/restarted child
  is matched to its source by identity, not list position (7a note / row #12).

### 3.5 Reconcile
Add the five keys to the Subflow read-set (`measure.rs`), lift their `lift_of` Package(12)
warnings, regenerate the census (five cells ignored→honoured), update README. Lift-refusal
discipline before lifting.

## 4. Decisions

- **D1** #12 = all five keys in one package (user-approved).
- **D2 (scheduleDate)** reuse `executions.scheduled_date` + the scheduler due-gate; no new
  delayed-start infra. Task 0 confirms the seam.
- **D3 (revision)** reuse 7b flow-version machinery; default = latest. Task 0 confirms egret
  stores enough revision history to resolve a subflow at a given revision.
- **D4 (restartBehavior)** RETRY_FAILED stays the default (7a); add NEW_EXECUTION via the
  7a/7b restart seam; include the LoopUntil-position follow-up.
- **D5** migration: expected migration-free (reuse scheduled_date, labels, version, restart);
  Task 0 confirms (fallback: a column at the next free number).
- **D6** census: the five cells move ignored→honoured in ONE reconcile commit (last), census
  moves once; behaviour tasks do not touch keys.rs/measure.rs/property-treatment.json.

## 5. Task 0 — load-bearing measurement

1. labels/inheritLabels: Kestra's exact child-label composition + order (own, inherited,
   system) and render context.
2. revision: how 7b's version machinery resolves a flow at revision N; the not-found error;
   confirm egret stores revision history usable here.
3. scheduleDate: the exact egret seam that gates a child start on `scheduled_date`; how `wait`
   interacts with a delayed child; past/invalid scheduleDate handling in Kestra.
4. restartBehavior: Kestra's NEW_EXECUTION semantics (old child's fate, parent linkage, outputs);
   the `restart::open` Subflow seam to extend; the LoopUntil-position bug's exact locus.
5. Census baseline (post-#11) + the five keys' lift sites + lift-refusal grep + migration verdict.

## 6. Testing

- Scoped runs include the Subflow/executor/restart/routes/execution_timestamps suites.
- labels: child carries rendered labels; inheritLabels merges parent labels.
- revision: a subflow pinned to an older revision runs that revision; default runs latest.
- scheduleDate: a future date delays the child start; the parent waits; a past date starts now.
- restartBehavior: RETRY_FAILED restarts in place; NEW_EXECUTION spawns a fresh child on parent
  restart; the LoopUntil child is matched by identity.
- Census golden regenerated; five cells honoured.

## 7. Divergences (to document)

- Any NEW_EXECUTION or scheduleDate detail where egret's restart/scheduler model differs from
  Kestra's (Task 0 records the exact differences).
- **Nested LoopUntil over a Subflow (approximation):** the child's iteration identity
  (`child_iteration`) uses the NEAREST enclosing loop's iteration, matching Kestra's single
  `taskRunIteration`; an outer loop's iteration is not part of the child identity.
- `scheduleDate` is not honoured (package #12b); it stays accepted and warned.

## Task 0 outcome (2026-11-27) — scope corrected

Measured against Kestra `96893b589` + egret:

- **labels/inheritLabels, revision, restartBehavior**: tractable, migration-free — honoured in #12. Label composition pinned (child own + [inheritLabels ? parent-exec labels filtered : parent system.*] + task labels + correlationId, deduped; task labels rendered in the parent runContext). revision via `FlowRepo::get_version`. restartBehavior NEW_EXECUTION gates `restart_earlier_child`.
- **LoopUntil child-by-identity** (row #12 bug): egret matches a Subflow child by `(trigger_execution_id, trigger_run_ix)`; Kestra by `taskRunId`+value+iteration. egret never sets `TaskRun::value` on a Subflow run → may need one child-identity column (migration 0245) — decided in the restartBehavior task.
- **scheduleDate → split to #12b.** egret has NO seam to defer an already-created execution's start, and the `workflow_executions_scheduled_date_iff_schedule` CHECK (0156/0244) forbids a Subflow child (NULL trigger) from writing `scheduled_date`. Honouring it needs a new deferred-start subsystem (modelled on retry `ExecutionDelay`) + a migration — materially larger than a key wire, so it is its own package #12b (user-approved split 2026-11-27). In #12, `scheduleDate` stays ignored/warned, re-pointed to #12b.

Revised #12: Task 1 labels+inheritLabels, Task 2 revision, Task 3 restartBehavior+LoopUntil-identity, Task 4 reconcile (four keys). Census 1257 → honoured +4.
