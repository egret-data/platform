# Pause as a full flowable (package 9a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `Pause` the full Kestra flowable — `tasks:` (body run after resume), `errors:`, `finally:`, `onPause:` (single task at pause entry) and `behavior:` (4-valued at the deadline) — lifting the `unsupported_pause_property` refusal.

**Architecture:** egret-workflow (Rust, sqlx/Postgres); the executor is a pure `advance()` state machine driven by `settle`. Flowables with a body (`Sequential`/`AllowFailure`/`WorkingDirectory`) carry `tasks: Vec<TaskIx>` in their plan node and are driven by `flowable::sequential::drive(children, errors, finally, parent)`, which already handles errors/finally and the PAUSED door. 9a extends `TaskKind::Pause` to carry the body and makes the pause driver delegate to that sequential machinery after resume.

**Tech Stack:** Rust, sqlx; tests are `#[sqlx::test(migrations = "../../migrations")]` in crates/egret-workflow/tests/.

## Global Constraints
- Spec: docs/en/specs/2026-11-24-workflow-pause-flowable-design.md (VI mirror). Decisions D1–D8 bind every task.
- **No migration, no new API route.** Pause topology/UI already renders; FE change only if a Pause-with-body needs graph display (assess in the last task; likely none).
- Chat VI; code/comments/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Stage by explicit path; never `git add -A`; never stage `target*/`. No secrets/scratch.
- `cargo fmt --all --check` before push. Scoped egret-workflow runs MUST include `--test routes --test execution_timestamps`. ONE heavy build at a time.
- Invalid behavior: literal → refuse at validate (D2); expression → FAIL at runtime, never hang.
- Stored bare Pause must keep re-planning as bare (D8: pause_gap vs pause_plan_gap split, like the existing finally: handling).

## File Structure
- `executor/plan.rs` — `TaskKind::Pause` gains tasks/errors/finally/on_pause/behavior; `children()` returns them; planner builds them; `pause_gap`/`pause_plan_gap` stop refusing (keep literal-invalid-behavior).
- `spec/validate.rs` — lift `unsupported_pause_property` for the five props; add `unsupported_pause_behavior`; recurse into the Pause's child lists; keep onResume + afterExecution-Pause refusal.
- `executor/flowable/pause.rs` — onPause at entry; delegate to sequential body after resume; behavior at deadline.
- `executor/flowable/sequential.rs` / `flowable/mod.rs` — reuse the sequential walk for the Pause body (the PAUSED door already lives in sequential::walk).
- `executor/replay.rs`, `executor/restart.rs` — Pause-with-body replay/restart arms.
- `tests/` — pause flowable tests (likely a new tests/pause_flowable.rs + additions to executor/replay tests).

---

### Task 0: Worktree + baseline
Create worktree `egret-pause` off origin/main (post-8c), branch `feat/workflow-pause-flowable`; containers egret-pause-postgres :5469 / valkey :6469 (adjust if taken). Baseline: `cargo test -p egret-workflow --test executor --test pause*` green.

### Task 1: Plan node — carry the Pause body
- Extend `TaskKind::Pause` to `{ duration/bound, tasks, errors, finally, on_pause, behavior }`.
- `children()` returns tasks (+errors/finally/on_pause) so planning recurses.
- Planner builds the child lists (mirror Sequential/AllowFailure at plan.rs ~1454).
- `pause_gap`/`pause_plan_gap`: remove the tasks/errors/behavior/onPause refusals; KEEP a literal-invalid-behavior refusal; keep the stored-vs-new split (D8).
- Tests: a Pause-with-tasks now PLANS (build_plan returns a plan); a stored bare Pause still plans bare (plan_stored); a literal invalid behavior is still refused by the gap predicate.
- This is a pure-plan task; unit-test against build_plan/plan_stored.

### Task 2: Validate — lift refusals + invalid-behavior
- Lift `unsupported_pause_property` for tasks/errors/finally/behavior/onPause.
- Add `unsupported_pause_behavior` (coded) for a literal behavior outside {RESUME,WARN,CANCEL,FAIL}.
- Recurse validation into the Pause's child lists (dedup rules, unknown task types, etc.).
- Keep onResume checks (7c) + the afterExecution-Pause refusal (validate.rs:861).
- Update `the_not_yet_enforced_list_is_pinned_in_both_directions` and any pause-property pinning tests.
- Tests: a full Pause validates clean; a literal invalid behavior → `unsupported_pause_behavior`; a Pause in afterExecution still refused.

### Task 3: Driver — onPause at entry
- When the Pause run is first RUNNING and about to pause, run `on_pause` (single task) once before PAUSED, if set. Respect the once-per-batch rule (a Parallel of two pauses enters both in one batch — reuse the existing guard pattern).
- Tests: onPause runs exactly once at pause entry; two pauses in one Parallel each run their onPause once.

### Task 4: Driver — run the body after resume (the deep one)
- On resume (manual `release_now`, or deadline with behavior RESUME), the Pause run drives its `tasks:` body via `sequential::drive(tasks, errors, finally, parent=pause_run)` instead of ending SUCCESS immediately. Pause run terminal = aggregate(tasks).
- The seam: flowable/pause.rs `drive` + the PAUSED door in sequential::walk. Find where a resumed Pause run transitions from PAUSED to running its body. HIGH RISK (like 8c T4) — escalate if intractable, do NOT ship a wrong trace.
- Tests: Pause with a body → PAUSED, manual resume → body runs in order, terminal = aggregate; a failing body child → errors: runs; finally: runs; a body that reads `{{ outputs.<pause>.onResume.* }}`.

### Task 5: Driver — behavior at the deadline
- `deadline()`/`drive` apply `behavior` on the pauseDuration/timeout expiry path (D3): RESUME → run body; WARN → WARNING (no body); CANCEL → cancel execution; FAIL → FAILED. Manual resume before deadline ignores behavior (always body/SUCCESS).
- Expression behavior rendered by the caller (like pauseDuration); out-of-enum render → FAIL the Pause run (D2), never hang.
- Tests: each behavior at deadline → its terminal; expression→valid honoured; expression→invalid → Pause FAILED (not RUNNING past the deadline); manual resume before deadline ignores behavior.

### Task 6: Replay / restart with a Pause body
- Update the Pause arms in replay.rs (replay.rs:347) and restart.rs so replay/restart of an execution containing a Pause-with-body reopens correctly (body re-runs/resumes per replay rules, as a Sequential body does).
- Tests: replay from a task inside a Pause body; restart a failed Pause body.

### Task 7: Docs + gates + live + PR
- egret-data/docs: a Pause flowable page/section (tasks/errors/finally/onPause/behavior, the invalid-behavior rule, the manual-resume-ignores-behavior rule).
- Gates: fmt/clippy/full scoped suite (incl routes + execution_timestamps + executor + replay + the new pause tests).
- Live check (optional): a Pause-with-approval flow in the browser.
- PR; green CI; merge rules.

## Self-review checklist (run when formalizing)
- Spec D1–D8 each → a task. D2 (invalid behavior) → T1 (gap) + T2 (validate) + T5 (runtime). D4 (body) → T4. D5 (onPause) → T3. D8 (stored) → T1.
- No placeholder; each task names real files + real predicates (pause_gap, sequential::drive, release_now, deadline).
- Type consistency: TaskKind::Pause field names fixed in T1, consumed by name in T3–T6.
