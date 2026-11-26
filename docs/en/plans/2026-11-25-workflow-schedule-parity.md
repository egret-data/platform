# Schedule trigger parity (package 11b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make egret's Schedule trigger match Kestra for `when`, `lateMaximumDelay`, `recoverMissedSchedules`, `labels`, and `stopAfter`.

**Architecture:** Lift the fire-time refusals (`when`, `lateMaximumDelay`) and honor the parsed-but-ignored fields by hooking each at its measured seam: `when` evaluated in the fire/IO half (`fire.rs`), `lateMaximumDelay` via the already-ported `handle_max_delay`, `labels` at `start()`, `recoverMissedSchedules` via a scheduler-startup pass, `stopAfter` via the execution-terminal seam. Builds on 11a's `trigger.*` binding.

**Tech Stack:** Rust, `crates/egret-workflow`; sqlx/Postgres; `scheduler/` (IO) + `trigger/` (pure).

## Global Constraints
- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A`/`target*/`.
- ONE heavy Rust build at a time, foreground in subagents. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- **Scoped `egret-workflow` runs MUST include `--test routes --test execution_timestamps`.**
- **LIFT-REFUSAL DISCIPLINE (bit 9a×2, 9b):** before lifting the `when` or `lateMaximumDelay` refusal, grep its code AND message text AND fixture YAML across `src/ tests/ oracle-exec/` (error codes `UnsupportedWhenCondition`/`UnsupportedLateMaximumDelay`, the message strings at `schedule.rs:1294-1301`, `schedule_not_resolvable` in validate.rs, and any corpus flow keyed on a refused Schedule). A fixture-keyed test that greps clean on code/message still breaks CI.
- Migration numbers shared — if a column is needed (recover guard), take the next free number at build time (11a took one for the replay index; reconfirm) and renumber above main + open PRs.
- `secret()`/10a untouched.

## File Structure
- `crates/egret-workflow/src/trigger/schedule.rs` — un-refuse `when` (~1353-1356) and `lateMaximumDelay` (~1367-1368); `handle_max_delay` (~1500) already ported.
- `crates/egret-workflow/src/scheduler/fire.rs` — `when` evaluation in `fire_one`/`resolve` (~245), `labels` in `start()` (~473-494).
- `crates/egret-workflow/src/scheduler/` (startup) — the `recoverMissedSchedules` onStart pass (Task 0 pins the exact startup fn).
- `crates/egret-workflow/src/trigger/switch.rs` + `trigger/mod.rs` — reuse `RecoverMissed` + `resume_after_backfill`/`last_tick_at_or_before` logic for the onStart pass.
- the execution-terminal/settle path (Task 0 pins it) — `stopAfter` → set `workflow.triggers.disabled`.
- Tests: `tests/schedule_when.rs`, `tests/schedule_parity.rs` (or extend existing schedule tests); unit tests in schedule.rs.

---

### Task 0: Measurement — census + pin the two unmeasured seams (load-bearing)

- [ ] **Step 1 — Census/Banks + lift-refusal grep.** From `crates/egret-workflow`:
```bash
grep -rn "when:\|lateMaximumDelay" oracle-exec/corpus/ | grep -iE "schedule|cron"
grep -rln "UnsupportedWhenCondition\|UnsupportedLateMaximumDelay\|schedule_not_resolvable" src/ tests/
grep -rn "schedule_not_resolvable\|not supported" oracle-exec/ tests/ | head
```
Record which corpus Schedule flows sit in `VALIDATE_REFUSED`/warned `schedule_not_resolvable` because of `when`/`lateMaximumDelay`, and the census constants in `oracle-exec/driver.rs` (`LIVE`, `VALIDATE_REFUSED`, `DRAFT_ONLY`) they will move. Note the w12 schedule oracle (`oracle-exec/w12-schedule-trigger.json`) — the driver DOES fire Schedule triggers there, so a `when`/`lateMaximumDelay` change may move an executor-oracle entry. Decide reconciliation (updated constants vs narrow guard) before code.
- [ ] **Step 2 — Pin the recover onStart seam.** Find where the scheduler starts/first runs (grep `scheduler` module for a `run`/`start`/`spawn` entry, a startup fn before `fire_due` begins looping). Determine whether a once-per-startup guard can key off existing state (`workflow.triggers.evaluated_at` + the startup boundary) or needs a new column (a `recover_applied`/scheduler-epoch marker). Record the chosen mechanism; if a column is needed, name the next-free migration number.
- [ ] **Step 3 — Pin the stopAfter settle seam.** Find where an execution reaches a terminal state and is committed (the settle/`claim` terminal path). Confirm it can see, for a trigger-started execution, the trigger id (`execution_trigger->>'id'` / `_context.id`) + the flow, to look up `(flow_id, trigger_id)` in `workflow.triggers` and set `disabled`. Confirm `stopAfter` is reachable there (re-parse the flow trigger spec).
- [ ] **Step 4 — Pin the Duration parser.** Find egret's existing ISO-8601 `Duration` parser (used elsewhere for `PT…`), or decide the parse for `lateMaximumDelay`. Record the exact function.
- [ ] **Step 5 — Write findings to the task report.** No production code. These pin Tasks 2/4/5.

---

### Task 1: Schedule `labels` on the fired execution

**Files:** `scheduler/fire.rs` `start()` (~473-494).

- [ ] **Step 1: Failing test** — a Schedule trigger with `labels: [{k: v}]` fires an execution carrying that label (rendered against `{{ trigger.* }}` if templated), plus any backfill labels, in order. (Mirror existing fire tests; grep `fn ` tests in fire.rs + schedule tests.)
- [ ] **Step 2: Run — FAIL** (trigger labels dropped today).
- [ ] **Step 3: Implement.** In `start()`, render the Schedule trigger's own `labels` (the `Vec<Label>` at `models/trigger.rs:51`) against the fire's schedule record (`{{ trigger.* }}`), and PREPEND them to the backfill labels already assembled (`fire.rs:473-494`). Order = trigger labels, then backfill labels (Kestra's append order). Flow labels unchanged.
- [ ] **Step 4: Run — PASS** (+ scoped binaries). **Step 5: Commit** `feat(workflow): Schedule trigger labels on the fired execution`.

---

### Task 2: `lateMaximumDelay` — parse + un-refuse

**Files:** `trigger/schedule.rs` `from_schedule` (~1367-1368 refusal) + `handle_max_delay` (~1500, already ported).

- [ ] **Step 1: Failing test** — a Schedule whose due tick is older than `now - lateMaximumDelay` is SKIPPED (fires the next tick within its delay); a tick within the delay fires; the skip is bypassed while a backfill runs. Use egret's measured example (`schedule.rs:1272-1274`: `0 2 * * *` five days behind → `06-16` with `PT1H`).
- [ ] **Step 2: Run — FAIL** (refused today).
- [ ] **Step 3: Implement.** Parse the `lateMaximumDelay` Duration with the parser Task 0 pinned; remove the `return Err(ScheduleError::UnsupportedLateMaximumDelay)` at `from_schedule` (~1367-1368) and carry the parsed `Option<Duration>` into the resolved trigger so `next_evaluation_date` reaches the existing `handle_max_delay` (~1500). Keep the `backfill == null` bypass (match Kestra). LIFT-REFUSAL grep first (code+message+fixture).
- [ ] **Step 4: Run — PASS** (+ scoped). **Step 5: Commit** `feat(workflow): honor Schedule lateMaximumDelay (skip over-late occurrences)`.

---

### Task 3: `when` — skip-and-advance, evaluated in the fire/IO half

**Files:** `trigger/schedule.rs` `resolve` (~1353-1356 refusal); `scheduler/fire.rs` `fire_one` (~209-279).

- [ ] **Step 1: Failing tests** — (a) a Schedule with `when: "{{ trigger.date | …weekday… }}"` (or a literal-false-on-some-ticks expression) fires ONLY on matching ticks, advancing past non-matching ones; (b) a date-helper case, e.g. `when: "{{ isLastWorkingDay(trigger.date) }}"`; (c) a `when` that errors at render fails SAFE (does not fire; records `last_error`), mirroring runIf. (Grep existing schedule-fire tests + the date-helper fns in `runners/functions.rs`.)
- [ ] **Step 2: Run — FAIL** (refused today).
- [ ] **Step 3: Implement.** Remove the `UnsupportedWhenCondition` refusal (`schedule.rs:1353-1356`). In the fire/IO half (`fire_one`, which has a render context), when the resolved trigger has a non-"true" `when`: after computing a candidate tick, render `when` against `{schedule, trigger}` = the candidate tick's `{date,next,previous}` record (reuse `schedule_trigger_record`'s shape) using the confined render engine (the three date helpers already exist); if falsy, advance to the next cron candidate and repeat, bounded (match Kestra's now+10y bound); fire the first matching tick and set `next_evaluation_date` past it. A render error → do not fire, set `last_error`, leave `next_evaluation_date` for retry (fail-safe). Keep the pure `tick` path for the no-`when` case unchanged. LIFT-REFUSAL grep first.
- [ ] **Step 4: Run — PASS** (+ scoped). **Step 5: Commit** `feat(workflow): honor Schedule when: (skip non-matching ticks)`.

---

### Task 4: `recoverMissedSchedules` — Kestra-faithful onStart hook

**Files:** the scheduler startup seam (Task 0); reuse `RecoverMissed` (`schedule.rs:1312`) + `last_tick_at_or_before` + `resume_after_backfill` (`trigger/mod.rs:328`). Migration only if Task 0 found a column is needed.

- [ ] **Step 1: Failing tests** — simulate a scheduler restart with a trigger whose `next_evaluation_date` is in the past (missed): **NONE** → `next_evaluation_date` reseeded to the first tick after now (no missed replays); **LAST** → set to the most recent missed tick; **ALL** (default) → unchanged (replays each). Assert the once-per-startup guard: a second `fire_due` pass does NOT re-apply the recover reseed.
- [ ] **Step 2: Run — FAIL** (normal path ignores the setting today).
- [ ] **Step 3: Implement.** Add a scheduler startup pass (the seam Task 0 pinned) that runs once before/at scheduler start: for each enabled trigger with `next_evaluation_date < now`, resolve the trigger's `recoverMissedSchedules` (absent = ALL) and apply: NONE → reseed from now (reuse the first-tick-after-now computation); LAST → `last_tick_at_or_before(now)`; ALL → leave. Use the once-per-startup guard mechanism Task 0 chose (existing `evaluated_at`+boundary, or the new column). Do NOT change backfill or the manual re-enable route (they keep current semantics).
- [ ] **Step 4: Run — PASS** (+ scoped). **Step 5: Commit** `feat(workflow): recoverMissedSchedules NONE/LAST on scheduler start`.

---

### Task 5: `stopAfter` — disable the trigger on a matching terminal

**Files:** the execution-terminal/settle seam (Task 0); `workflow.triggers` (`disabled`).

- [ ] **Step 1: Failing tests** — a Schedule-started execution reaching a state in `stopAfter` (e.g. `[FAILED]`) sets the trigger `disabled` (no further fires); a terminal state NOT in `stopAfter` leaves it enabled; a non-trigger-started execution never disables anything.
- [ ] **Step 2: Run — FAIL** (stopAfter read by nothing today).
- [ ] **Step 3: Implement.** At the terminal/settle seam Task 0 pinned, when an execution reaches a terminal state AND it is trigger-started (`execution_trigger` non-null), re-parse the flow's trigger spec for that `execution_trigger._context.id`, and if the terminal state ∈ its `stopAfter`, `UPDATE workflow.triggers SET disabled = true WHERE flow_id = $ AND trigger_id = $`. Idempotent; within the settle transaction or a follow-up consistent with how egret writes trigger state.
- [ ] **Step 4: Run — PASS** (+ scoped). **Step 5: Commit** `feat(workflow): stopAfter disables the trigger after a matching terminal`.

---

### Task 6: Docs (repo egret-data/docs, new branch docs/workflow-schedule-parity)

- [ ] **Step 1:** Document the five Schedule features in `platform/workflows/flows/triggers`/Schedule doc: `when` (+ date helpers, skip-and-advance), `lateMaximumDelay` (skip over-late), `recoverMissedSchedules` (ALL default / LAST / NONE on restart), `labels`, `stopAfter`. Note any egret divergence recorded in Task 0.
- [ ] **Step 2:** Build docs (or rely on the docs PR `build` gate). **Step 3: Commit** `docs(workflow): Schedule trigger parity (when/lateMaximumDelay/recover/labels/stopAfter)`.

---

## Self-Review
- **Spec coverage:** D1 when → Task 3; D2 lateMaximumDelay → Task 2; D3 recover → Task 4; D4 labels → Task 1; D5 stopAfter → Task 5; census → Task 0; docs → Task 6. No gap.
- **Placeholder scan:** fire-path seams carry measured line numbers; the two genuinely-unmeasured seams (recover startup, stopAfter settle) + the Duration parser + the recover-guard-column decision are explicitly Task 0 measurements, not "TBD" in an impl step.
- **Type consistency:** `RecoverMissed{All,None,Last}`, `handle_max_delay`, `schedule_trigger_record` (the `{date,next,previous}` shape reused for `when`'s context), `last_tick_at_or_before` used consistently.
- **Order:** easy→risky (labels, lateDelay, when, then recover + stopAfter which need Task-0-pinned seams), docs last.
