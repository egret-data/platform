# Workflow trigger lifecycle parity (package 11d) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honour the three remaining `lift_of → Package(11)` trigger lifecycle keys — `allowConcurrent`, `failOnTriggerError`, and `stopAfter` on the Flow trigger — on the trigger types Kestra honours them on.

**Architecture:** All three keys are already fields on `models::Trigger`. Each is honoured at its existing seam: `allowConcurrent` holds a due Schedule tick at `scheduler/fire.rs` while a live execution of that trigger exists (derived from execution state, no stored lock → unlock-on-restart is implicit); `failOnTriggerError` surfaces a trigger-evaluation error as a `born_failed` execution; `stopAfter` on Flow opens the `worker/stop_after.rs::on_terminal` type-gate. Behaviour lands in Tasks 1–3 without touching validate-time treatment; Task 4 reconciles the census/warnings in one move.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first:** no key is honoured until Task 0 pins its exact Kestra semantics on the pinned artifact `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` (Java cross-reference checkout `96893b589`). Task 1–3 code pins to Task 0's findings, not to this plan's expected path.
- **D1** `allowConcurrent` = derived lock, migration-free. Stored `locked_execution_id` + explicit unlock-on-restart is the contingency ONLY if Task 0 proves a non-derivable semantic.
- **D2** Honour each key only on the trigger types Kestra honours it on. A type Kestra ignores keeps an accurate `not_honoured` warning (or is documented inert). No guessing.
- **D4** `dependsOn`/listener conditions are already implemented (W13a/W13b); 11d does not touch them.
- **D5 lift discipline:** before lifting any warning, grep code + message + **fixture YAML + warning-assertion tests** across `src/ tests/ oracle-exec/`.
- **Census:** `property-treatment.json` is probe-generated (`EGRET_WRITE_PROPERTY_TREATMENT=1`) and golden-tested. It must NOT move in Tasks 1–3; it moves exactly once, in Task 4. Baseline after 11c: **1224 honoured / 3 warned / 358 ignored / 117 refused / 1702 total**.
- **Scoped test runs** must include `--test routes --test execution_timestamps`; for stopAfter add `--test disabled_triggers --test stop_after`; for the scheduler add `--test schedule`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Conventional Commits; `Closes #N` in the PR body only.
- One heavy Rust build at a time, foreground. `cargo fmt --all --check` before any push. New `std::env::var` keys go in egret-config's `ENV_OUTSIDE_CONFIG` allow-list.

---

### Task 0: Measure — Kestra semantics + census + lift map (load-bearing, report only)

**Files:** none (findings written to the task report).

- [ ] **Step 1 — Census baseline + lift-refusal grep.** Record the current `property-treatment.json` aggregate (expect 1224/3/358/117/1702) and which bucket each `(trigger-type, key)` of `allowConcurrent`/`failOnTriggerError`/`stopAfter` sits in for Flow, Schedule, Webhook. Grep across `src/ tests/ oracle-exec/`: `allowConcurrent`, `failOnTriggerError`, `stopAfter`, `not_honoured`, `lift_of`, and every test asserting these warnings (e.g. `spec/keys.rs` `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not` at ~:2178, the `ALL_LIFT`/base-key tests at ~:2300). List every file+line a warning lift would break.

- [ ] **Step 2 — allowConcurrent on the pinned artifact.** Read Kestra scheduler source at checkout `96893b589` (`AbstractScheduler`, `SchedulerExecutionWithTrigger`, `TriggerState`, `Schedule`) and/or drive the image. Pin:
  - (a) the **default** value of `allowConcurrent` (keys.rs:733's comment claims `false`; confirm — if `false`, egret over-fires today even when unset).
  - (b) which **trigger types** it applies to (Schedule/polling only, or Flow/Webhook too).
  - (c) **hold semantics**: when a prior execution is live, is the due tick HELD (fires later) or DROPPED (advanced)? On unlock, does the held tick fire, or only the next due tick? Interaction with `recoverMissedSchedules` and backfill.
  - (d) the **lock key**: does the hold key on the specific prior `executionId`, or on "any live execution of this (flow, trigger)"? This decides whether egret's derived query (§Task 1) suffices or the D1 stored-lock fallback is needed.
  - Record a verdict: **derivable (default)** or **stored-lock fallback required**, with the reason.

- [ ] **Step 3 — failOnTriggerError on the pinned artifact.** Pin: its **default**; which **trigger types** it applies to; on an evaluation error with `true`, whether Kestra creates a FAILED execution, logs only, or both, and what the failed execution carries (message, trigger context); what counts as a "trigger error" (evaluation throw / render failure / fire failure) vs a normal no-fire.

- [ ] **Step 4 — stopAfter on the Flow trigger.** Confirm Kestra disables a Flow trigger on a terminal state in `stopAfter` the same way as a Schedule, and that the disable is per-trigger (not per-flow). In egret, confirm the Flow-trigger evaluate path reads the disable: trace `flow_trigger/process.rs` → `trigger/flow.rs` `evaluate` `Refusal::TriggerDisabled` and verify `trigger.disabled` is derived from `workflow.disabled_triggers` for Flow triggers (as the webhook route reads it). Record whether `stopAfter` on Flow is fully wired by the `stop_after.rs` type-gate alone, or needs the disable-read confirmed/added.

- [ ] **Step 5 — dependsOn no-gap check.** Confirm the existing W13a/W13b machinery leaves no `dependsOn`/condition-type gap: census shows the Flow `dependsOn` corpus flows handled (fired/refused as measured), no Kestra Flow-trigger condition type beyond `states`/`when`/`dependsOn` is silently dropped. Record "no gap" or list the gap (which would become a new roadmap line, not 11d scope).

- [ ] **Step 6 — Write the lift map + migration verdict.** For each `(trigger-type, key)` honoured per §2–§4, record: the exact `keys.rs` warn site to lift, the `measure.rs` `trigger_base_read` entry to add, the warning-assertion tests to update, and whether the key is honoured or stays warned (with wording). Record "migration-free" (expected) or the stored-lock column + next free migration number (next free is **0244**; 0243 was 11a).

- [ ] **Step 7 — Record a BASE commit** (`git rev-parse HEAD`) for the review package, and write all findings into the report. No production code in Task 0.

---

### Task 1: `allowConcurrent` — hold a due tick while the trigger's execution is live

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`Fate` enum ~:88; `fire_one` ~:220; `claim_and_fire`/`PassReport` ~:44/:158)
- Test: `crates/egret-workflow/tests/schedule.rs` (or the existing scheduler test file Task 0 identifies)

**Interfaces:**
- Consumes: Task 0's verdict on hold semantics (§Step 2) and the derivable/fallback decision. If Task 0 says **stored-lock fallback**, STOP and escalate — this task assumes the derived path (D1 default).
- Produces: a new `Fate::Held` variant and a `PassReport.held: usize` counter; the live-execution predicate used by Task 2's error path is independent (no shared helper required).

- [ ] **Step 1 — Write the failing test.** In the scheduler test file, drive two due ticks of one Schedule trigger declaring `allowConcurrent: false`, with a task that stays RUNNING. Use the file-local tick-serialising mutex pattern (worker `tick` claims globally; hold a `tokio::Mutex` across the whole test, as webhook_route.rs `TICK` in 11c). Assert: tick 1 fires an execution; while that execution is non-terminal, a second `fire_due` pass does NOT fire a second execution and does NOT advance `next_evaluation_date` past the held tick; after the first execution reaches a terminal state, the next `fire_due` fires the held tick exactly once. Pin the exact held/advance/retroactive-fire behaviour to Task 0 §Step 2(c).

```rust
#[tokio::test]
async fn allow_concurrent_false_holds_a_tick_while_the_execution_is_live() {
    let _g = TICK.lock().await; // file-local serialiser; worker tick claims globally
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    allowConcurrent: false\n",
    ).await;
    // tick 1 fires
    let r1 = fire_due(&pool).await.unwrap();
    assert_eq!(r1.fired, 1);
    let exec1 = latest_execution(&pool, flow).await;
    // while exec1 is live, a due tick 2 is HELD, not fired, not advanced
    make_due_again(&pool, flow, "s").await; // set next_evaluation_date <= now()
    let date_before = next_eval_date(&pool, flow, "s").await;
    let r2 = fire_due(&pool).await.unwrap();
    assert_eq!(r2.fired, 0, "held while exec1 is live");
    assert_eq!(r2.held, 1);
    assert_eq!(next_eval_date(&pool, flow, "s").await, date_before, "tick not advanced");
    // exec1 terminates; the held tick now fires once
    drive_to_terminal(&pool, exec1).await;
    let r3 = fire_due(&pool).await.unwrap();
    assert_eq!(r3.fired, 1);
}
```

- [ ] **Step 2 — Run it to verify it fails.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds -- --nocapture`. Expected: FAIL (`Fate::Held`/`PassReport.held` do not exist; tick 2 fires).

- [ ] **Step 3 — Add the `Held` fate + counter.** In `fire.rs`: add `Held` to `enum Fate`, add `held: usize` to `PassReport`, and map `Fate::Held` in the pass accounting (where `Fate::Frozen` etc. are counted) to increment `held` and leave `next_evaluation_date` unchanged (no `last_error`, no advance) — the same "dates left alone, next pass retries" path `Errored` uses, but silent.

- [ ] **Step 4 — Hold in `fire_one`.** After the trigger spec is resolved and before `start()`, read `allow_concurrent` from the resolved trigger. When it is `false` (pin the default to Task 0 §Step 2a) AND a live execution exists for this trigger, return `Fate::Held`. The live-execution predicate (under the claim's held trigger-row lock):

```rust
// Any non-terminal execution this trigger already started. `execution_is_idle`
// (migration 0210) is the terminal test; its negation is "live".
let live: bool = sqlx::query_scalar(
    "SELECT EXISTS (SELECT 1 FROM workflow.executions \
       WHERE flow_id = $1 AND workspace_id = $2 \
         AND execution_trigger ->> 'id' = $3 \
         AND NOT workflow.execution_is_idle(id))",
)
.bind(c.flow_id)
.bind(c.workspace_id)
.bind(&c.trigger_id)
.fetch_one(&mut **at)
.await?;
if !allow_concurrent && live {
    return Ok(Fate::Held);
}
```

Pin the exact lock key (per-trigger vs per-prior-executionId) to Task 0 §Step 2d; the query above keys on "any live execution of this (flow, trigger)".

- [ ] **Step 5 — Run the test to verify it passes.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds`. Expected: PASS. Also add/confirm a test that `allowConcurrent: true` (or unset, per Task 0 default) does NOT hold — both ticks fire.

- [ ] **Step 6 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` green; `cargo fmt --all --check`.

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour allowConcurrent — hold a due tick while the trigger's execution is live"
```

---

### Task 2: `failOnTriggerError` — surface a trigger error as a born-failed execution

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`fire_one` error arm; `start`/`record_error` ~:573/:677)
- Test: `crates/egret-workflow/tests/schedule.rs`

**Interfaces:**
- Consumes: Task 0 §Step 3 (default, type-scope, "both FAILED execution + log" vs log-only, what counts as a trigger error). `NewExecution.born_failed: bool` (worker/claim.rs) — "start the execution already FAILED, with no task run."
- Produces: nothing other tasks consume.

- [ ] **Step 1 — Write the failing test.** Drive a Schedule trigger whose fire raises a trigger-evaluation error (pin the exact error class to Task 0 §Step 3c — e.g. a `when`/inputs render against a missing variable, or whatever Task 0 identifies as the Kestra "trigger error"). Assert: with `failOnTriggerError: false` (default), today's behaviour — `last_error` is set, NO execution is created; with `failOnTriggerError: true`, a `born_failed` execution exists (state FAILED, `taskRunList` empty) AND `last_error` is still recorded.

```rust
#[tokio::test]
async fn fail_on_trigger_error_true_surfaces_a_failed_execution() {
    let _g = TICK.lock().await;
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    failOnTriggerError: true\n    inputs:\n      x: \"{{ missing.field }}\"\n",
    ).await; // pin the error shape to Task 0 §Step 3c
    let r = fire_due(&pool).await.unwrap();
    let exec = latest_execution(&pool, flow).await;
    assert_eq!(state_of(&pool, exec).await, "FAILED");
    assert!(task_runs_of(&pool, exec).await.is_empty(), "no task run");
    assert!(last_error_of(&pool, flow, "s").await.is_some());
}
```

- [ ] **Step 2 — Run it to verify it fails.** `cargo test -p egret-workflow --test schedule fail_on_trigger_error_true -- --nocapture`. Expected: FAIL (no execution created; `fire_one` records `last_error` only).

- [ ] **Step 3 — Surface on the error arm.** In `fire_one`, where a trigger-evaluation error is currently classified into `Fate::Errored`/`record_error` (the arm Task 0 §Step 3c identifies), when `fail_on_trigger_error` is `true` for a type Kestra honours (Task 0 §Step 3a/b), start a born-failed execution before recording `last_error`:

```rust
if fail_on_trigger_error {
    // Surface the trigger error as Kestra does: a CREATED -> FAILED execution,
    // no task run. `born_failed` is W13a's mechanism (worker/claim.rs).
    let _ = start_born_failed(at, c, &resolved /* inputs/trigger as measured */).await;
}
// existing behaviour, unchanged: record the reason on the trigger row
record_error(at, c, &why).await?;
```

Add a small `start_born_failed` next to `start` that calls `start_execution_in_with` with `born_failed: true` and the same `NewExecution` fields `start` uses (trigger record, scheduled_date, inputs as far as they rendered). It must never propagate an error that would roll back the trigger advance: on failure it logs and falls through to `record_error` (Global Constraint: the key can only add surfacing, never remove the existing log). Pin whether the failed execution also carries the rendered inputs/trigger to Task 0 §Step 3b.

- [ ] **Step 4 — Run the test to verify it passes.** `cargo test -p egret-workflow --test schedule fail_on_trigger_error`. Expected: PASS. Add the `false`-default counterpart test (no execution, `last_error` only).

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` green; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour failOnTriggerError — surface a trigger error as a born-failed execution"
```

---

### Task 3: `stopAfter` on the Flow trigger

**Files:**
- Modify: `crates/egret-workflow/src/worker/stop_after.rs` (add `FLOW_TYPE`, open the type-gate ~:40)
- Modify (if Task 0 §Step 4 requires): `crates/egret-workflow/src/trigger/flow.rs` / `flow_trigger/process.rs` (confirm the Flow-trigger evaluate path reads `workflow.disabled_triggers`)
- Test: `crates/egret-workflow/tests/stop_after.rs` and/or `crates/egret-workflow/tests/disabled_triggers.rs`

**Interfaces:**
- Consumes: Task 0 §Step 4 (Flow-trigger disable is per-trigger via `disabled_triggers`; whether the evaluate path already reads it). The existing `on_terminal` already does the `disabled_triggers` INSERT + `triggers` UPDATE (the UPDATE is a no-op for Flow, which has no `workflow.triggers` row — same as Webhook).
- Produces: nothing other tasks consume.

- [ ] **Step 1 — Write the failing test.** A listening flow with a Flow trigger declaring `stopAfter: [SUCCESS]`. Fire the listener (publish a source execution that terminates in a listened state), drive the listener's execution to SUCCESS, run the settle, then assert a `workflow.disabled_triggers` row exists for `(listener_flow, trigger_id)` and a second matching source event does NOT fire the listener again.

```rust
#[tokio::test]
async fn flow_trigger_stop_after_disables_the_listener() {
    let _g = TICK.lock().await;
    let (pool, listener) = seed_listener_flow(
        "triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n    stopAfter: [SUCCESS]\n",
    ).await;
    fire_source_and_settle_listener_to(&pool, listener, "SUCCESS").await;
    assert!(disabled_trigger_exists(&pool, listener, "l").await);
    // a second source event does not fire the disabled listener
    let fired = fire_source_event(&pool, listener).await;
    assert_eq!(fired, 0);
}
```

- [ ] **Step 2 — Run it to verify it fails.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after -- --nocapture`. Expected: FAIL — `on_terminal`'s type-gate excludes Flow, so no `disabled_triggers` row is written.

- [ ] **Step 3 — Open the type-gate.** In `stop_after.rs`, add `const FLOW_TYPE: &str = "io.kestra.plugin.core.trigger.Flow";` and include it in the `matches!` gate:

```rust
if !matches!(
    trigger.get("type").and_then(|t| t.as_str()),
    Some(crate::scheduler::SCHEDULE_TYPE | WEBHOOK_TYPE | FLOW_TYPE)
) {
    return Ok(());
}
```

Update the module doc so it reads "Schedule, Webhook and Flow `stopAfter`". If Task 0 §Step 4 found the Flow-trigger evaluate path does NOT already read `disabled_triggers`, wire that read in `flow_trigger/process.rs`/`trigger/flow.rs` so the disable takes effect (a listener with a `disabled_triggers` row is skipped with `Refusal::TriggerDisabled`).

- [ ] **Step 4 — Run the test to verify it passes.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after`. Expected: PASS.

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test stop_after --test disabled_triggers --test routes --test execution_timestamps` green; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/worker/stop_after.rs
git commit -m "feat(workflow): honour stopAfter on the Flow trigger"
```

---

### Task 4: Reconcile — lift warnings, census golden, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (warn logic ~:725–760; `lift_of` ~:176; warning-assertion tests ~:2178/:2300)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs` (`trigger_base_read` ~:97)
- Modify: `crates/egret-workflow/src/spec/schema/property-treatment.json` (regenerated) and `crates/egret-workflow/src/spec/schema/README.md`
- Create: `docs/en/platform/workflows/triggers/*.md` additions + `docs/vi/...` (egret-data/docs repo — a separate docs PR, per CLAUDE.md docs rule)

**Interfaces:**
- Consumes: Task 0 §Step 6 lift map — the exact `(trigger-type, key)` pairs that are now honoured vs stay warned, and the warning-assertion tests to update.

- [ ] **Step 1 — Update the read-set.** In `measure.rs` `trigger_base_read`, add each honoured `(trigger-type, key)` from the lift map so the firing-path read-set classifies it honoured (e.g. `allowConcurrent`/`failOnTriggerError` on Schedule, `stopAfter` on Flow — exactly per Task 0 §Step 6). Do NOT add a key for a type Task 0 found inert.

- [ ] **Step 2 — Lift the warnings.** In `keys.rs`, make the `lift_of`/warn logic type-aware for these keys: drop the `not_honoured` warning for the honoured `(type, key)` pairs; keep it (with accurate wording) for the pairs that stay warned (D2). Remove the special `allowConcurrent != false` skip only if Task 0's default makes it obsolete.

- [ ] **Step 3 — Update the warning-assertion tests.** Update every test Task 0 §Step 1 listed (e.g. `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not`, the base-key/`ALL_LIFT` tests) to assert the new treatment: honoured pairs no longer warn; still-warned pairs still warn.

- [ ] **Step 4 — Regenerate the census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --test <the golden test> property_treatment` (the probe that writes `property-treatment.json`), then re-run without the env var to confirm the golden matches. Record the new aggregate (honoured goes up by the number of lifted pairs; total stays 1702). Update `README.md`'s Flow/Schedule/Webhook rows to move the lifted keys out of the not-honoured column.

- [ ] **Step 5 — Run the full validate/census suite.** `cargo test -p egret-workflow --test routes --test execution_timestamps` plus the spec/keys unit tests (`cargo test -p egret-workflow spec::keys`) and the census golden test — all green. `cargo fmt --all --check`.

- [ ] **Step 6 — Docs (egret-data/docs).** In a separate docs worktree/PR, document: `allowConcurrent` (derived hold + the §7 divergences — derived vs stored lock, held-tick behaviour as measured), `failOnTriggerError` (born-failed surfacing, default, type-scope), `stopAfter` on the Flow trigger. Include the measured divergences from Task 0. Bilingual (en + vi).

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/spec/keys.rs crates/egret-workflow/src/spec/schema/measure.rs \
        crates/egret-workflow/src/spec/schema/property-treatment.json crates/egret-workflow/src/spec/schema/README.md
git commit -m "feat(workflow): lift trigger lifecycle warnings + reconcile census for package 11d"
```

---

## Self-Review notes

- **Spec coverage:** allowConcurrent → Task 1 (+ Task 4 warning); failOnTriggerError → Task 2 (+ Task 4); stopAfter-Flow → Task 3 (+ Task 4); dependsOn no-gap → Task 0 §Step 5; census/lift map → Task 0 + Task 4; divergences/docs → Task 4 §Step 6. D1/D2/D4/D5 are Global Constraints. All spec sections map to a task.
- **Migration:** expected migration-free (D1 derived lock). If Task 0 §Step 2d forces the stored-lock fallback, Task 1 escalates and a migration task (column on `workflow.triggers` + unlock-on-restart pass, next free **0244**) is inserted before Task 1 — do not proceed on the derived path.
- **Census discipline:** Tasks 1–3 touch only `fire.rs`/`stop_after.rs` + tests (behaviour), never `keys.rs`/`measure.rs`/`property-treatment.json`; the golden cannot move mid-feature. Task 4 is the single census move.
