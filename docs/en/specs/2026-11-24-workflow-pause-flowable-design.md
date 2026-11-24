# Workflow Pause as a full flowable (package 9a)

**Status:** design
**Date:** 2026-11-24
**Depends on:** 8c (concurrency gate — in flight), 7c (Pause onResume + manual pause/resume, #868), #2a/#2b (finally/afterExecution).
**Roadmap:** package #9 split into **9a (this: full Pause flowable)** and **9b (AllowFailure block retry)** — user decision 2026-11-24.

## 1. Goal

Make `io.kestra.plugin.core.flow.Pause` the full Kestra flowable. Today egret
supports only a **bare** pause: enter `PAUSED`, wait for `pauseDuration:`/
`timeout:`/manual resume, end the run; plus `onResume:` inputs (7c). A Pause
that declares `tasks:`, `errors:`, `finally:`, `onPause:` or `behavior:` is
**refused** at publish/start with `unsupported_pause_property`
(`executor::plan::pause_gap`). 9a implements all five so a Pause behaves as it
does in Kestra.

## 2. Kestra semantics (measured from vendored Pause.java)

`Pause extends Task implements FlowableTask<Output>`:

- **`tasks:`** — the flowable body. It runs **after the pause is resumed** (like
  a `Sequential` body gated behind the pause). The Pause run's terminal is the
  aggregate of `tasks:` (standard flowable rule).
- **`errors:`** / **`finally:`** — standard flowable error/finally handlers for
  the body (the sequential driver already handles both for Sequential).
- **`onPause:`** — a **single task** (not a list) run **when the pause begins**.
- **`behavior:`** (default `RESUME`) — what happens when the pause reaches its
  **`pauseDuration:`/`timeout:` deadline** (NOT a manual resume; a manual resume
  before the deadline always succeeds and ignores behavior). Enum →
  terminal state:
  - `RESUME` → continue (run the `tasks:` body);
  - `WARN` → the Pause run ends `WARNING`, execution continues;
  - `CANCEL` → the execution is `CANCELLED`;
  - `FAIL` → the Pause run ends `FAILED`.
- **`onResume:`** — inputs collected on resume (DONE in 7c; its output feeds the
  `tasks:` body).
- Bounds: `pauseDuration:` (own field) wins over `timeout:` (inherited). An
  unbounded pause waits for manual resume forever.

## 3. Decisions

**D1 — Split (user 2026-11-24).** 9a = full Pause flowable (tasks/errors/finally/
onPause/behavior). 9b = AllowFailure block retry (separate package).

**D2 — Invalid `behavior:` value (user 2026-11-24).** A **literal** behavior
outside `{RESUME, WARN, CANCEL, FAIL}` is **refused at validate**
(`unsupported_pause_behavior` or similar coded error) at publish/start — it is
known at plan time. An **expression** that renders to an out-of-enum value at
runtime **FAILS the Pause task** (not a hang). This diverges from Kestra, which
leaves the execution `RUNNING` forever for an invalid value — a worker-resource
footgun 9a refuses to reproduce. Record the divergence in the spec §divergences.

**D3 — `behavior` applies only at the deadline.** A manual resume (via the 7c
resume route) before the deadline always succeeds and runs the body, ignoring
`behavior` (Kestra `Pause.java`). `behavior` maps a terminal only on the
`pauseDuration:`/`timeout:` expiry path.

**D4 — `tasks:` run after resume.** On RESUME (manual or deadline-with-RESUME),
the Pause's `tasks:` run as a sequential body with the Pause run as their
parent, with `errors:`/`finally:` via the existing sequential driver. The Pause
run's terminal is `aggregate(tasks)`.

**D5 — `onPause:` runs once, at pause entry**, before the run goes `PAUSED`
(Kestra `resolveTasks(List.of(onPause))` at pause begin). A single task.

**D6 — `finally:` (carried from 7c Q5).** A Pause `finally:` is part of 9a (the
sequential driver already runs a list's `finally:`).

**D7 — No migration.** The Pause plan node gains fields but the execution row /
task_run schema is unchanged (tasks/errors/finally are plan structure; PAUSED
state and the resume machinery already exist).

**D8 — Stored executions (D4-style, plan_stored).** A Pause stored before 9a
(bare, or one that pre-#2a dropped its `finally:`) re-plans by the rules it
started under: `pause_plan_gap` / `plan_stored` must keep an old bare Pause a
bare Pause. Only NEW starts/publishes get the full flowable. (Mirror the
`pause_gap` vs `pause_plan_gap` split that already exists for `finally:`.)

## 4. Architecture / where the work falls

### 4.1 Plan (`executor/plan.rs`)
- `TaskKind::Pause` currently carries only `{ duration }`. Extend it to carry the
  body: `tasks: Vec<TaskIx>`, `errors: Vec<TaskIx>`, `finally: Vec<TaskIx>`,
  `on_pause: Option<TaskIx>`, `behavior: Option<String>` (literal or
  expression; None = RESUME), alongside the existing bound.
- `TaskKind::children()` must return the Pause's `tasks` (+ errors/finally/
  on_pause as the other flowables do) so planning recurses into them. Today it
  returns `Vec::new()` for Pause.
- The planner builds those children (as it does for `Sequential`/`AllowFailure`
  at plan.rs ~1454-1468).
- `pause_gap` / `pause_plan_gap`: stop refusing tasks/errors/behavior/onPause/
  finally. Keep refusing a **literal invalid behavior** (D2). Keep the
  stored-vs-new split so `plan_stored` doesn't retro-flowable an old bare Pause.

### 4.2 Validate (`spec/validate.rs`)
- Remove the `unsupported_pause_property` refusals for tasks/errors/behavior/
  onPause/finally (the `NOT_YET_ENFORCED`/`pause_gap` mirror).
- Add `unsupported_pause_behavior` (D2) for a literal behavior outside the enum.
- Keep the 7c `onResume` input checks and the afterExecution-Pause refusal
  (validate.rs:861 — a Pause inside afterExecution stays refused).
- Validate recurses into the Pause's `tasks:`/`errors:`/`finally:`/`onPause:` as
  it does for the other flowables' child lists.

### 4.3 Driver (`executor/flowable/pause.rs` + `flowable/mod.rs`/`sequential.rs`)
- **onPause (D5):** when the Pause run first becomes RUNNING and is about to
  pause, if `on_pause` is set, run it once before entering PAUSED.
- **Pause then body (D4):** the Pause run enters PAUSED (as today). On resume
  (manual `release_now`, or deadline with behavior RESUME), instead of just
  ending the run `SUCCESS`, it must now DRIVE its `tasks:` body via the
  sequential machinery (the Pause run is the parent of its children), with
  `errors:`/`finally:`. The Pause run's terminal = `aggregate(tasks)`.
- **behavior at deadline (D2/D3):** `deadline()` currently returns
  `Resume::Terminal{SUCCESS}` for pauseDuration and the allow-flag-mapped state
  for timeout. Extend so the deadline path applies `behavior`:
  RESUME → run the body; WARN → run terminal WARNING (no body); CANCEL →
  cancel the execution; FAIL → FAILED. An expression behavior is rendered by
  the caller (like pauseDuration) and an out-of-enum render → FAIL (D2).
- Reuse `sequential::drive(children, errors, finally, parent=pause_run)` for the
  body — do NOT duplicate the sequential walk.

### 4.4 Replay / restart (`executor/replay.rs`, `executor/restart.rs`)
- replay.rs already special-cases Pause (replay.rs:347:
  `TaskKind::Pause{..} => !errors.is_empty() || !finally.is_empty()`). Update
  the Pause arms for the new flowable shape so a replay/restart of an execution
  with a Pause-with-body reopens correctly (the body re-runs or resumes per the
  replay rules, as a Sequential body would).

### 4.5 Outputs
- The Pause `Output` carries `onResume` (7c). Confirm the body tasks can read
  `{{ outputs.<pause>.onResume.* }}` as Kestra does.

## 5. Out of scope (9a)
- AllowFailure block retry → 9b.
- Any manual-pause/resume API change (7c shipped it; 9a reuses it).
- Namespace/tenant anything.

## 6. Testing (sketch, grows in the plan)
- A Pause with `tasks:` → PAUSED, manual resume → body runs, Pause terminal =
  aggregate(body).
- `onPause:` runs at pause entry (once; a Parallel of two pauses enters both in
  one batch — reuse the `two_pauses_in_one_parallel` guard).
- `behavior: WARN|CANCEL|FAIL` at deadline → the mapped terminal; `RESUME` →
  body runs. Literal invalid → refused at validate; expression → invalid →
  Pause FAILS.
- `errors:`/`finally:` on the Pause body behave as on a Sequential.
- Manual resume before deadline ignores behavior (always runs body / SUCCESS).
- plan_stored: an old bare Pause (and a pre-#2a Pause with dropped finally)
  re-plans as before (D8).
- Replay/restart of an execution with a Pause-with-body.
- Scoped runs MUST include `--test routes --test execution_timestamps`.

## 7. Risks
- The driver integration (pause → body via sequential) is the deepest change
  (like 8c Task 4). The machinery exists, but wiring the PAUSED→body transition
  through the sequential walk with the Pause run as parent needs care. The
  implementer finds the seam in flowable/pause.rs + sequential::walk's PAUSED
  door; fallback is NOT soft-shipping a wrong trace — escalate if intractable.
- onPause ordering (before PAUSED) and the once-per-batch RESTARTED rule.
