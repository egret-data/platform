# Package 11b — Schedule trigger parity — Design

**Status:** approved design (scope + decisions locked), formalized on the #11b worktree. Next-free migration = 0244 (if the recover guard needs a column; Task 0 decides).
**Date:** 2026-11-25
**Roadmap:** egret-workflow Kestra parity, package **#11 (triggers)**, part **11b**. #11 program: 11a (`trigger.*` binding, merging) → **11b** (Schedule parity, this doc) → 11c (Webhook parity) → 11d (Flow-trigger listener conditions + remaining cross-cutting lifecycle). User chose **all 5 Schedule features in 11b** (so `stopAfter` lives here, not 11d).

## 1. Goal

Make egret's Schedule trigger match Kestra for five features that are currently refused or parsed-but-ignored:
1. **`when:`** — a Pebble truthiness condition on the trigger (refused today, `schedule.rs:1353-1356`).
2. **`lateMaximumDelay`** — skip an occurrence delayed past the duration (refused, `schedule.rs:1367-1368`).
3. **`recoverMissedSchedules`** — ALL/LAST/NONE on scheduler restart (parsed; only the manual re-enable route honors it; the normal path hardcodes ALL-style replay).
4. **`labels`** — the trigger's own labels on the fired execution (dropped today).
5. **`stopAfter`** — disable the trigger after a started execution reaches one of these states (parsed, read by nothing).

Builds on **11a**: `when` and `labels` render against the `{schedule,trigger}` vars that 11a binds.

## 2. Measured facts (egret fire path)

- Scheduler: `scheduler/fire.rs` `fire_due` → `claim_and_fire` (claim `WHERE next_evaluation_date <= now()`; due-ness is **precomputed** into `next_evaluation_date`, no cron re-eval at claim) → `fire_one` (`ScheduleTrigger::resolve` at `fire.rs:245`, where `when`/`lateMaximumDelay` are refused) → pure `tick` (`trigger/mod.rs:467`, DB-free) → `start` (`fire.rs:466`, the single `NewExecution` construction: labels/inputs/trigger/scheduled_date).
- Next-fire computation is pure: `ScheduleTrigger::next_evaluation_date` (`schedule.rs:1566`); `handle_max_delay` (`schedule.rs:1500`) is a faithful lateMaximumDelay port, **dead today** (resolve forces `late_maximum_delay: None`).
- State table `workflow.triggers` (migration 0156): `(flow_id,trigger_id)` PK, `spec_key`, `disabled`, `next_evaluation_date` (NULL=off queue), `evaluated_at`, `last_error`. Backfill in `workflow.trigger_backfills` (0192). Double-fire backstop: unique `(flow_id, execution_trigger->>'id', scheduled_date)`.
- `recoverMissedSchedules`: `RecoverMissed{All,None,Last}` (`schedule.rs:1312`, absent=All); honored only in the switch/re-enable route (`triggers/switch.rs:201-290`) + backfill-end (`resume_after_backfill`, `mod.rs:328`). Normal path never reads it; after downtime it replays every missed tick one-at-a-time (ALL-style) — `schedule.rs:1575-1586,226-229`.
- `labels`: fire path uses only the backfill row's labels (`fire.rs:473-494`); the trigger's own `labels` (`models/trigger.rs:51`) is never applied.
- `stopAfter`: `models/trigger.rs:60`, parsed/serialized, no consumer. Needs the started execution's terminal state — a seam the scheduler doesn't observe (`trigger/mod.rs:235`).

## 3. Kestra semantics (reference) — the parity targets

- **`when`**: skip-and-advance. `nextEvaluationDate` → `findNextDateMatchingConditions` walks cron candidates, returns the first where `when` is truthy; non-matching ticks never become due. Evaluated against `additionalVariables = {schedule: tickOutput, trigger: tickOutput}` where tickOutput = `{date,next,previous}`. Date helpers: `isWeekend`, `isDayWeekInMonth`, `isLastWorkingDay`.
- **`lateMaximumDelay`**: skip (don't fire) any occurrence whose `date + delay < now`; walk forward to the first still within its delay; **disabled while a backfill runs**. (egret's `handle_max_delay` already matches.)
- **`recoverMissedSchedules`**: default **ALL**; enforced at scheduler startup (`TriggerScheduler.onStart`): for a trigger whose stored `next_evaluation_date` is in the past (missed) — ALL = keep (replay each), LAST = jump to the most recent missed, NONE = reseed from now.
- **`stopAfter`**: `TriggerState.updateOnExecutionTerminated` — if the started execution's end state ∈ `stopAfter`, set the trigger `disabled`.
- **`labels`**: fired execution labels = trigger's own labels (rendered with `{{ trigger.* }}`), then backfill labels, then system `from`/`correlationId`; flow's own labels come from the flow separately (unchanged).

## 4. Design & decisions

### D1 — `when` (medium): evaluate in the fire/IO half, skip-and-advance
egret's `tick`/`next_evaluation_date` is pure/DB-free and has no render context, but Kestra evaluates `when` against a rendered context. **Decision:** evaluate `when` in the **fire/IO half** (`fire.rs`, which has a render context), not in the pure tick. When a trigger has a non-"true" `when`, after computing a candidate tick, render `when` against `{schedule,trigger} = {date,next,previous}` of that candidate; if falsy, advance to the next cron candidate and repeat (bounded, as Kestra bounds at now+10y); fire the first matching tick and set `next_evaluation_date` past it. Stop refusing in `resolve` (`schedule.rs:1353-1356`). No new column (a `when`-skip only changes which tick becomes due). The three date helpers already exist in egret's render surface. A `when` render error fails safe (does not fire; records `last_error`) — mirror runIf's posture.

### D2 — `lateMaximumDelay` (small): parse + un-refuse
Parse the ISO-8601 Duration (`PT1H`) — egret's duration parsing for this is unmeasured, so pin it in Task 0 / use the existing duration parser if one exists. Remove the refusal in `from_schedule` (`schedule.rs:1367-1368`). `handle_max_delay` (already ported) then drops occurrences whose `date+delay < now`, and is bypassed while a backfill runs (match Kestra's `backfill == null` guard).

### D3 — `recoverMissedSchedules` (largest): Kestra-faithful onStart hook (USER-CHOSEN)
Add a scheduler **startup pass**: once per scheduler start, for each enabled trigger whose `next_evaluation_date` is already in the past (a miss), apply the resolved recover value: **NONE** → reseed `next_evaluation_date` from now (skip all missed); **LAST** → set it to the most recent missed tick (`last_tick_at_or_before(now)`, reusing the existing helper); **ALL** → leave it (the normal loop replays each, today's behavior). A **once-per-startup guard** prevents re-applying on every fire (the startup pass must run before `fire_due` begins serving, and must be idempotent if the scheduler restarts mid-backlog). Whether this needs a new `workflow.triggers` column (e.g. a `recover_applied_at` / scheduler-epoch marker) vs. can key off existing `evaluated_at` is a **Task 0 decision** — prefer no migration if `evaluated_at` + the startup boundary suffice; add a minimal column only if required. Backfill-in-progress and the manual re-enable route keep their current semantics.

### D4 — `labels` (small): render + prepend
In `start()` (`fire.rs:473`), render the Schedule trigger's own `labels` against `{{ trigger.* }}` (the fire's schedule record) and prepend them to the backfill labels (Kestra's append order: trigger labels, then backfill, then system). Flow labels unchanged.

### D5 — `stopAfter` (cross-seam): disable on terminal
When a trigger-started execution reaches a terminal state, if that state ∈ the trigger's `stopAfter`, set the trigger `disabled` in `workflow.triggers`. **Seam:** hook the settle/terminal path (where an execution finalizes), identify a trigger-started execution via `execution_trigger._context.id` (+ its flow), look up the `(flow_id, trigger_id)` row, and set `disabled = true` when the terminal state matches. Task 0 locates the exact settle hook and confirms `stopAfter` is reachable there (it is stored on the flow's trigger spec, re-parsed from the flow source). A disabled trigger stops firing (existing `disabled` semantics + partial index).

## 5. Census / Banks (Task 0, measure-first)

Lifting `when` and `lateMaximumDelay` moves any corpus Schedule flow currently in `VALIDATE_REFUSED` / warned `schedule_not_resolvable` to valid/LIVE. Task 0 greps the corpus/oracle for Schedule `when:`/`lateMaximumDelay:` flows and records which census constants (`LIVE`, `VALIDATE_REFUSED`, `DRAFT_ONLY`) move, with the reconciled values — decide (accept the ratchet with updated constants, or keep a narrow guard) before code. `labels`/`stopAfter`/`recoverMissedSchedules` are runtime/scheduler behavior and should not move validate/publish census (confirm). Any executor-oracle movement (the driver does fire Schedule triggers for w12 — check `w12-schedule-trigger.json`) must be measured and reconciled.

## 6. Testing
- **Task 0:** census measurement + pin: the Duration format for lateMaximumDelay; the recover once-per-startup guard mechanism (column-or-not); the settle hook for stopAfter; the w12 schedule oracle's exposure.
- `when`: a Schedule with `when` that skips non-matching ticks fires only matching ones (+ a date-helper case, e.g. `isLastWorkingDay`); a `when` render error fails safe.
- `lateMaximumDelay`: a tick delayed past the delay is skipped; within delay fires; bypassed during backfill.
- `recoverMissedSchedules`: NONE reseeds, LAST fires one, ALL replays all — on a simulated restart with a past `next_evaluation_date`.
- `labels`: fired execution carries the trigger's rendered labels + backfill labels in order.
- `stopAfter`: an execution reaching a stopAfter state disables the trigger (no further fires); a non-matching state leaves it enabled.
- Scoped `egret-workflow` runs include `--test routes --test execution_timestamps`. Live-verify the scheduler behavior on kind where feasible.

## 7. Global constraints
- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A`/`target*/`.
- ONE heavy Rust build at a time, foreground in subagents. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- Lift-refusal discipline (bit 9a/9b): before lifting the `when`/`lateMaximumDelay` refusals, grep their code AND message text AND fixture YAML across `src/ tests/ oracle-exec/`.
- Migration numbers shared — if a column is needed (recover guard), take the next free number at plan time and renumber above main + open PRs.
- Scoped `egret-workflow` runs include `--test routes --test execution_timestamps`.
