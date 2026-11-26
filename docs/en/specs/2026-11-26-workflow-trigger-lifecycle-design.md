# Workflow trigger lifecycle parity (package 11d) — design

**Status:** approved (brainstorm 2026-11-26)
**Roadmap:** egret = Kestra OSS engine; package #11 (triggers) part 4 of 4.
**Predecessors:** 11a trigger.* binding (#915), 11b Schedule parity (#916),
11c Webhook parity (#917). This package closes the three remaining
`lift_of → Package(11)` trigger keys.

## 1. Goal

Honour the three cross-cutting trigger lifecycle properties that
`models::Trigger` already parses but that `spec::keys::lift_of` still reports
as `not_honoured` warnings pointing at package 11:

- `allowConcurrent` — serialise a trigger's executions (do not fire a new one
  while the trigger's previous execution is still running), **plus** release a
  stale hold when the engine restarts (Kestra's "unlock on restart").
- `failOnTriggerError` — surface an error raised while evaluating a trigger as
  a failed execution, rather than only logging it.
- `stopAfter` on the **Flow** trigger — extend the existing
  `worker/stop_after.rs::on_terminal` hook (Schedule 11b, Webhook 11c) to the
  Flow trigger type.

Out of scope, by the roadmap and by measurement:

- **Flow-trigger listener conditions / `dependsOn` / MultipleCondition** are
  **already implemented** (W13a window-free, W13b `dependsOn` with the
  `workflow.flow_trigger_windows` table, modes `ALL`/`ANY`/`AT_LEAST`,
  `minSatisfied` partial-satisfaction, window `fireOnce`, `sweep_expired_windows`).
  11d does not touch them. Task 0 confirms no hidden gap remains before we
  state they are done.
- `logToFile`, `logLevel`, `version`, `assets` → package 13 (stay warned).
- `workerSelector`, `policyRefs` → Enterprise (stay warned).

## 2. Architecture

All three keys are already fields on `models::Trigger`
(`fail_on_trigger_error: bool`, `allow_concurrent: bool`, `stop_after`). The
package is: **honour each at its existing seam, lift its warning, measure the
exact Kestra semantics first.** No new model shape.

| Key | Seam | Change |
|---|---|---|
| `allowConcurrent` | `scheduler/fire.rs` (Schedule fire path) | Before firing a due tick, if the trigger declares `allowConcurrent: false` and a live (non-terminal) execution already exists for this `(flow_id, trigger_id)`, **hold** the tick (do not fire, do not advance past it) until that execution terminates. Derived from live execution state — **no stored lock**. |
| `failOnTriggerError` | `scheduler/fire.rs` / `flow_trigger/process.rs` error paths | When trigger evaluation/fire raises an error and `failOnTriggerError: true`, create a FAILED execution carrying the error, in addition to writing `last_error`. Default `false` keeps today's log-to-`last_error` behaviour. |
| `stopAfter` (Flow) | `worker/stop_after.rs::on_terminal` | Open the type-gate from `{Schedule, Webhook}` to also include `Flow`; disable via the `workflow.disabled_triggers` table (a Flow trigger has no `workflow.triggers` row, same as Webhook in 11c). |

### 2.1 allowConcurrent: derived lock, migration-free (D1)

Kestra stores the hold in `TriggerState.executionId`/`locked` and therefore
needs an **unlock-on-restart** reconciliation pass: on scheduler start it must
clear locks whose execution no longer exists. egret drops those columns (see
`trigger/mod.rs` `TriggerState` doc) and instead **derives** the hold:

> "Is a non-terminal execution already present for this `(flow_id,
> trigger_id)`?" — a query against `workflow.executions` filtered by the
> trigger record, under the same per-flow advisory lock the fire path already
> takes.

Consequence: **there is no stored lock to leak, so unlock-on-restart is a
non-issue** — a restart re-derives the hold from live execution state, which is
always current. This removes the migration and the reconciliation pass that the
stored-lock design would need. It matches egret's established style (package 8a
derived "limit = live-non-QUEUED" count under `pg_advisory_xact_lock`).

**Fallback:** if Task 0 measures a Kestra semantic that cannot be derived — for
example, the hold must remember a *specific* prior `executionId` and ignore
other live executions of the same flow — we fall back to a stored
`locked_execution_id` column on `workflow.triggers` plus an explicit
unlock-on-restart pass. The spec carries this as a contingency; the default is
migration-free.

### 2.2 Hold vs skip-and-advance

11b's `when` support **skips and advances** (a non-matching tick is consumed).
`allowConcurrent: false` is the opposite: the tick must be **held** (not
advanced) so it fires once the lock clears — otherwise the scheduled run is
lost. Task 0 measures Kestra's exact behaviour: whether the held tick fires
retroactively when the lock clears, how it interacts with
`recoverMissedSchedules`, and whether a long-running execution that spans many
ticks collapses them to one or queues them. The implementation pins to the
measured behaviour, not a guess.

## 3. Task 0 — load-bearing measurement (on the pinned Kestra artifact)

No key is honoured until Task 0 pins its semantics. Measure and record:

1. **allowConcurrent**
   - a. Its **default** (keys.rs:733 comment claims `false`; confirm — if the
     default is `false`, egret over-fires today even when the key is unset, and
     the honoured default changes, not just the explicit case).
   - b. Which **trigger types** Kestra applies it to (Schedule/polling only, or
     Flow and Webhook too). Event-driven Flow and synchronous Webhook may be
     inert.
   - c. **Hold semantics**: when locked, is the due tick held or dropped? On
     unlock, does the held tick fire, or only the next due tick? Interaction
     with `recoverMissedSchedules` and backfill.
   - d. The **lock key** (per-trigger, per-flow, or per-execution identity) —
     decides whether the derived query suffices or the stored fallback is
     needed.
2. **failOnTriggerError**
   - a. Its **default** and which trigger types it applies to.
   - b. On error with `true`: does Kestra create a FAILED execution, emit a log
     only, or both? What does the failed execution carry (the error message,
     the trigger context)?
   - c. What counts as a "trigger error" (evaluation throw, render failure,
     fire failure) vs. a normal no-fire.
3. **stopAfter (Flow)**: confirm Kestra disables a Flow trigger on a terminal
   state in `stopAfter` the same way it disables a Schedule, and that the
   disable is per-trigger (not per-flow).
4. **dependsOn confirmation**: a quick census + fixture check that the existing
   W13a/W13b machinery leaves no `dependsOn`/condition-type gap before we state
   it is complete.
5. **Census baseline + lift map**: the current `property-treatment.json` counts
   (1224/3/358/117 after 11c) and the exact set of `(trigger-type, key)` pairs
   whose warning lifts, plus a **lift-refusal grep** (code + message + fixture
   YAML across `tests/`) for every key about to change, per the 9a/9b lesson.

## 4. Type scope (D2)

**Honour each key only on the trigger types Kestra honours it on** (Task 0,
§3.1b/§3.2a). A type where Kestra does not act on the key keeps its
`not_honoured` warning with accurate wording (or is documented inert where
Kestra is also inert). We do not implement a key for a type Kestra ignores it
on — that would diverge from Kestra and over-engineer.

Expected (pending Task 0): `allowConcurrent` and `failOnTriggerError` are
scheduler-evaluated concepts → primarily **Schedule**; Flow (event-driven) and
Webhook (synchronous HTTP) may remain warned/inert. `stopAfter` applies to all
three (Schedule/Webhook done; Flow added here).

## 5. Error handling

- `allowConcurrent` hold: taken under the existing per-flow advisory lock; a
  lost race or deadlock reuses the fire path's existing retry discipline. A
  held tick must never be silently dropped — if it cannot be held it is left
  due (fires next pass).
- `failOnTriggerError` must itself never raise: a failure while *creating* the
  failed execution falls back to writing `last_error` (today's behaviour), so
  the key can only add surfacing, never remove the existing log.
- `stopAfter` reuses 11c's webhook mechanism verbatim (INSERT
  `disabled_triggers`); the existing `became_terminal` guard applies.

## 6. Testing

- Scoped egret-workflow runs **must** include `--test routes --test
  execution_timestamps` and, for stopAfter, `--test disabled_triggers --test
  stop_after`, plus the trigger suites (`--test schedule` / the flow-trigger
  process tests). Per [[egret-workflow-scoped-tests-miss-routes-and-timestamps]].
- `allowConcurrent`: a test that fires tick 1, holds tick 2 while execution 1 is
  live, then fires the held tick once execution 1 terminates. Uses the file-local
  tick-serialising mutex pattern (worker `tick` claims globally — 11c `TICK`
  mutex lesson).
- `failOnTriggerError`: a trigger whose fire raises → with `false`, `last_error`
  only; with `true`, a FAILED execution carrying the error.
- `stopAfter` (Flow): a Flow trigger that fires an execution terminating in a
  `stopAfter` state → the trigger is disabled in `disabled_triggers` and does
  not fire again.
- Census golden regenerated (`EGRET_WRITE_PROPERTY_TREATMENT=1`); the lifted
  warnings move warned→honoured, total 1702 unchanged.

## 7. Divergences (to document in egret-data/docs)

- allowConcurrent uses a **derived** hold (live-execution query) rather than
  Kestra's stored `executionId` lock; unlock-on-restart is therefore implicit,
  not an explicit pass. Any behavioural difference Task 0 surfaces (held-tick
  retroactive fire, multi-tick collapse) is recorded here.
- Any trigger type where Kestra applies a key but egret keeps it warned (if
  Task 0 finds a type we cannot honour without new machinery) is listed as a
  carried divergence with the package that would lift it.

## 8. Decisions

- **D1** allowConcurrent = derived lock, migration-free; stored-lock +
  explicit unlock-on-restart is the contingency only if Task 0 proves a
  non-derivable semantic.
- **D2** Honour each key only on the types Kestra honours it on; elsewhere keep
  an accurate `not_honoured` warning. No guessing — Task 0 measures.
- **D3** All-in-one, one PR, subagent-driven (as 11c). Task 0 measures all
  three keys before any is built.
- **D4** dependsOn/listener conditions are already done (W13a/W13b); 11d does
  not touch them; Task 0 confirms no hidden gap.
- **D5** Lift-refusal discipline: grep code + message + **fixture YAML** across
  `tests/` before lifting any warning (9a×2 / 9b×1 lesson).
