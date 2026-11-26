# Workflow trigger lifecycle parity (package 11d) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honour `allowConcurrent` on the Schedule trigger (serialise its executions) and `stopAfter` on the Flow trigger; keep `failOnTriggerError` warned (Task 0 measured that Kestra honours it for no trigger type egret has).

**Architecture:** All three keys are already fields on `models::Trigger`. `allowConcurrent` holds a due Schedule tick at `scheduler/fire.rs` while a live execution of that trigger exists (derived from execution state, no stored lock → unlock-on-restart is implicit). `stopAfter` on the Flow trigger opens the `worker/stop_after.rs::on_terminal` type-gate and disables the listener via `workflow.flow_listeners`. Behaviour lands in Tasks 1–2 without touching validate-time treatment; Task 3 reconciles the census/warnings in one move.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first (DONE, Task 0):** semantics were pinned against the local Kestra source at the cross-reference checkout `96893b589` (= pinned artifact `kestra/kestra@sha256:28f59bf1…`). Findings in `.superpowers/sdd/2026-11-26-workflow-trigger-lifecycle/task-0-report.md`.
- **D1** `allowConcurrent` = derived lock, migration-free (Task 0 verdict: DERIVABLE). No stored lock column; egret derives the hold from "any non-terminal execution of this (flow, trigger)". Honoured on **Schedule only** (Kestra: Schedule + polling; egret has no polling trigger).
- **D2** Honour each key only where Kestra honours it. Exception (user-approved): `stopAfter` on Flow is a **deliberate egret divergence**, consistent with egret already honouring `stopAfter` on Webhook (11c) where Kestra also no-ops it (`DefaultExecutor.sendTriggerExecutionTerminated` excludes Webhook + Flow + Subflow).
- **D6 `failOnTriggerError` is OUT of scope as an honoured key.** Task 0: Kestra reads it only in `WorkerTriggerProcessor` (worker/polling triggers); egret has none, so Schedule/Webhook/Flow never consult it. It stays a `not_honoured` warning, but Task 3 corrects its wording and `lift_of` target — it is a Kestra-divergence that no egret package will lift, NOT a "package 11 will honour it" case.
- **D4** `dependsOn`/listener conditions are already implemented (W13a/W13b); 11d does not touch them (Task 0: no gap).
- **D5 lift discipline:** before lifting any warning, grep code + message + **fixture YAML + warning-assertion tests** across `src/ tests/ oracle-exec/`.
- **Census:** `property-treatment.json` is probe-generated (`EGRET_WRITE_PROPERTY_TREATMENT=1`) and golden-tested. It must NOT move in Tasks 1–2; it moves exactly once, in Task 3. Baseline after 11c: **1224 honoured / 3 warned / 358 ignored / 117 refused / 1702 total** (the 9 lifecycle cells are all `ignored` today).
- **Scoped test runs** must include `--test routes --test execution_timestamps`; for stopAfter add `--test disabled_triggers --test stop_after`; for the scheduler add `--test schedule`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Conventional Commits; `Closes #N` in the PR body only.
- One heavy Rust build at a time, foreground. `cargo fmt --all --check` before any push. New `std::env::var` keys go in egret-config's `ENV_OUTSIDE_CONFIG` allow-list.

---

### Task 0: Measure — Kestra semantics + census + lift map (load-bearing, report only) — COMPLETE

Done. Verdicts (controller-verified in Kestra source `96893b589`): **allowConcurrent** honoured Schedule-only, default `false` = HOLD serial, **DERIVABLE / migration-free**; **failOnTriggerError** read only at `WorkerTriggerProcessor.java:181` → no egret type honours it → stays warned (D6); **stopAfter** excluded for Flow+Webhook in `DefaultExecutor.java:861-868` → Flow stopAfter is an egret divergence (D2), and the Flow listener's disable is read from `workflow.flow_listeners.disabled` (so `on_terminal` must UPDATE that table, not just `workflow.triggers`); **dependsOn** no gap. Census 1224/3/358/117/1702; next migration 0244. Full detail + lift map in `task-0-report.md`.

---

### Task 1: `allowConcurrent` — hold a due Schedule tick while the trigger's execution is live

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`Fate` enum ~:88; `fire_one` ~:220; `PassReport` ~:44)
- Test: `crates/egret-workflow/tests/schedule.rs` (or the scheduler test file the repo uses — confirm with `git grep -l fire_due tests/`)

**Interfaces:**
- Consumes: Task 0 verdict — hold semantics DERIVABLE, keyed on "any live execution of this (flow, trigger)"; default `false` serialises (holds). The missed tick is HELD and worked off serially (one tick per completion).
- Produces: a new `Fate::Held` variant and a `PassReport.held: usize` counter.

- [ ] **Step 1 — Write the failing test.** Drive two due ticks of one Schedule trigger declaring `allowConcurrent: false`, with a task that stays RUNNING. Use a file-local tick-serialising mutex (worker `tick` claims globally; hold a `tokio::Mutex` across the whole test, as webhook_route.rs `TICK` in 11c). Assert: tick 1 fires an execution; while that execution is non-terminal, a second `fire_due` pass does NOT fire a second execution and does NOT advance `next_evaluation_date` past the held tick; after the first execution reaches a terminal state, the next `fire_due` fires the held tick exactly once. Pin the held/advance behaviour to Task 0.

```rust
#[tokio::test]
async fn allow_concurrent_false_holds_a_tick_while_the_execution_is_live() {
    let _g = TICK.lock().await; // file-local serialiser; worker tick claims globally
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    allowConcurrent: false\n",
    ).await;
    let r1 = fire_due(&pool).await.unwrap();
    assert_eq!(r1.fired, 1);
    let exec1 = latest_execution(&pool, flow).await;
    make_due_again(&pool, flow, "s").await; // set next_evaluation_date <= now()
    let date_before = next_eval_date(&pool, flow, "s").await;
    let r2 = fire_due(&pool).await.unwrap();
    assert_eq!(r2.fired, 0, "held while exec1 is live");
    assert_eq!(r2.held, 1);
    assert_eq!(next_eval_date(&pool, flow, "s").await, date_before, "tick not advanced");
    drive_to_terminal(&pool, exec1).await;
    let r3 = fire_due(&pool).await.unwrap();
    assert_eq!(r3.fired, 1);
}
```

- [ ] **Step 2 — Run it to verify it fails.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds -- --nocapture`. Expected: FAIL (`Fate::Held`/`PassReport.held` do not exist).

- [ ] **Step 3 — Add the `Held` fate + counter.** In `fire.rs`: add `Held` to `enum Fate`, add `held: usize` to `PassReport`, and in the pass accounting map `Fate::Held` to increment `held` and leave `next_evaluation_date` unchanged (no `last_error`, no advance) — the silent twin of the "dates left alone, next pass retries" path.

- [ ] **Step 4 — Hold in `fire_one`.** After the trigger spec is resolved and before `start()`, read `allow_concurrent` from the resolved trigger. When it is `false` AND a live execution exists for this trigger, return `Fate::Held`. Query (under the claim's held trigger-row lock):

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

- [ ] **Step 5 — Run the test to verify it passes.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds`. Expected: PASS. Add a test that `allowConcurrent: true` does NOT hold — both ticks fire.

- [ ] **Step 6 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` green; `cargo fmt --all --check`.

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour allowConcurrent — hold a due Schedule tick while its execution is live"
```

---

### Task 2: `stopAfter` on the Flow trigger (deliberate egret divergence)

**Files:**
- Modify: `crates/egret-workflow/src/worker/stop_after.rs` (add `FLOW_TYPE`, open the type-gate ~:40; add the `workflow.flow_listeners` disable)
- Test: `crates/egret-workflow/tests/stop_after.rs` and/or `crates/egret-workflow/tests/disabled_triggers.rs`

**Interfaces:**
- Consumes: Task 0 — Kestra no-ops Flow stopAfter (`DefaultExecutor.java:861-868`); egret honours it as a divergence, consistent with Webhook (11c). The Flow listener's disable is read from `workflow.flow_listeners.disabled` (derived from `disabled_triggers` only at sync time), NOT `workflow.triggers` (a Flow trigger has no row there, so the existing UPDATE is a no-op). So the type-gate alone is NOT enough — `on_terminal` must also disable the listener row.
- Produces: nothing other tasks consume.

- [ ] **Step 1 — Write the failing test.** A listening flow with a Flow trigger declaring `stopAfter: [SUCCESS]`. Fire the listener (publish a source execution that terminates in a listened state), drive the listener's execution to SUCCESS, run the settle, then assert the listener is disabled (a `workflow.disabled_triggers` row exists AND `workflow.flow_listeners.disabled = true` for `(listener_flow, trigger_id)`) and a second matching source event does NOT fire the listener again.

```rust
#[tokio::test]
async fn flow_trigger_stop_after_disables_the_listener() {
    let _g = TICK.lock().await;
    let (pool, listener) = seed_listener_flow(
        "triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n    stopAfter: [SUCCESS]\n",
    ).await;
    fire_source_and_settle_listener_to(&pool, listener, "SUCCESS").await;
    assert!(disabled_trigger_exists(&pool, listener, "l").await);
    assert!(flow_listener_disabled(&pool, listener, "l").await);
    let fired = fire_source_event(&pool, listener).await;
    assert_eq!(fired, 0, "disabled listener does not fire");
}
```

- [ ] **Step 2 — Run it to verify it fails.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after -- --nocapture`. Expected: FAIL — `on_terminal`'s type-gate excludes Flow, so no disable is written and the second event fires.

- [ ] **Step 3 — Open the type-gate + disable the listener.** In `stop_after.rs`, add `const FLOW_TYPE: &str = "io.kestra.plugin.core.trigger.Flow";` and include it in the `matches!` gate:

```rust
if !matches!(
    trigger.get("type").and_then(|t| t.as_str()),
    Some(crate::scheduler::SCHEDULE_TYPE | WEBHOOK_TYPE | FLOW_TYPE)
) {
    return Ok(());
}
```

Then, for the Flow type, also disable the listener row so firing stops at once (the existing `disabled_triggers` INSERT survives sync; the `workflow.triggers` UPDATE stays a no-op for Flow). Add, in the same settle transaction:

```rust
// A Flow trigger fires off `workflow.flow_listeners`, whose `disabled` is
// re-derived from `disabled_triggers` only at sync. Disable it now so the
// listener stops firing within this settle, as sync would on the next save.
sqlx::query(
    "UPDATE workflow.flow_listeners SET disabled = true \
       WHERE flow_id = $1 AND trigger_id = $2",
)
.bind(flow_id)
.bind(trigger_id)
.execute(&mut *conn)
.await
.map_err(db_err)?;
```

Confirm the exact column/table names against `flow_trigger/process.rs` (the listener query ~:584-590) and the `workflow.flow_listeners` migration — use whatever the listener-fire path actually reads for `disabled`. Update the module doc to "Schedule, Webhook and Flow `stopAfter`" and note the Flow divergence (Kestra no-ops it).

- [ ] **Step 4 — Run the test to verify it passes.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after`. Expected: PASS.

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test stop_after --test disabled_triggers --test routes --test execution_timestamps` green; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/worker/stop_after.rs
git commit -m "feat(workflow): honour stopAfter on the Flow trigger (egret divergence, as Webhook)"
```

---

### Task 3: Reconcile — lift warnings, correct failOnTriggerError wording, census golden, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (`lift_of` ~:176; warn logic ~:714–730; warning-assertion tests ~:2071, ~:2170–2200)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs` (`trigger_base_read` ~:99–112)
- Modify: `crates/egret-workflow/src/spec/schema/property-treatment.json` (regenerated) and `crates/egret-workflow/src/spec/schema/README.md` (~:216, ~:261–263)
- Create: docs additions in the egret-data/docs repo (a separate docs PR, per CLAUDE.md docs rule), en + vi

**Interfaces:**
- Consumes: Task 0 lift map. Honoured now: `(Schedule, allowConcurrent)` and `(Flow, stopAfter)`. Stays warned: `failOnTriggerError` (all types) with corrected wording; `allowConcurrent` on Flow/Webhook; `stopAfter` on Schedule/Webhook already honoured by 11b/11c.

- [ ] **Step 1 — Update the read-set.** In `measure.rs` `trigger_base_read`, add `allowConcurrent` to the Schedule read-set and `stopAfter` to the Flow read-set (and confirm `stopAfter` is already in the Schedule + Webhook read-sets from 11b/11c). Do NOT add `failOnTriggerError` anywhere, and do NOT add `allowConcurrent` to Flow/Webhook.

- [ ] **Step 2 — Lift the two warnings; correct failOnTriggerError.** In `keys.rs`: make the warn/`lift_of` logic type-aware so `(Schedule, allowConcurrent)` and `(Flow, stopAfter)` no longer warn. Keep the `not_honoured` warning for `allowConcurrent` on Flow/Webhook. For `failOnTriggerError`, change its `lift_of` so it is NOT reported as "package 11 will honour it": make the warning say it is a Kestra-divergence egret does not act on because Kestra only consults it for worker/polling triggers, which egret has none of. Remove the special `allowConcurrent != false` skip if the type-aware logic makes it redundant.

- [ ] **Step 3 — Update the warning-assertion tests.** Update every test Task 0 §Step 1 listed (keys.rs ~:2071, ~:2170–2200, incl. `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not`) to assert the new treatment: `(Schedule, allowConcurrent)` + `(Flow, stopAfter)` no longer warn; `failOnTriggerError` + `(Flow/Webhook, allowConcurrent)` still warn (with the corrected wording).

- [ ] **Step 4 — Regenerate the census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow <the property_treatment golden test>` (the probe at measure.rs ~:807), then re-run without the env var to confirm the golden matches. Honoured rises by the 2 lifted cells (expect **1226** honoured; total stays 1702). Update `README.md`'s Schedule row (drop `allowConcurrent`) and Flow row (drop `stopAfter`) from the not-honoured column.

- [ ] **Step 5 — Run the validate/census suite.** `cargo test -p egret-workflow --test routes --test execution_timestamps`, the spec/keys unit tests (`cargo test -p egret-workflow spec::keys`), and the census golden test — all green. `cargo fmt --all --check`.

- [ ] **Step 6 — Docs (egret-data/docs).** In a separate docs worktree/PR, document: `allowConcurrent` on Schedule (derived hold, serialised executions, the §7 divergences — derived vs stored lock, held-tick worked off serially); `stopAfter` on the Flow trigger (egret divergence — Kestra no-ops it, as with Webhook); and a note that `failOnTriggerError` is accepted but inert (Kestra acts on it only for polling triggers egret does not have). Bilingual (en + vi).

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/spec/keys.rs crates/egret-workflow/src/spec/schema/measure.rs \
        crates/egret-workflow/src/spec/schema/property-treatment.json crates/egret-workflow/src/spec/schema/README.md
git commit -m "feat(workflow): lift allowConcurrent/stopAfter warnings + reconcile census for package 11d"
```

---

## Self-Review notes

- **Spec coverage:** allowConcurrent (Schedule) → Task 1 (+ Task 3 warning lift); stopAfter-Flow → Task 2 (+ Task 3); failOnTriggerError stays warned → Task 3 wording; dependsOn no-gap → Task 0; census/lift map → Task 0 + Task 3; divergences/docs → Task 3 §Step 6.
- **Scope correction (Task 0):** the plan's original premise that all three keys are honoured was wrong. Measured truth: only `allowConcurrent` (Schedule) is Kestra parity; `stopAfter`-Flow is a deliberate egret divergence (user-approved, consistent with 11c Webhook); `failOnTriggerError` is honoured by no egret type and stays warned.
- **Migration:** migration-free (D1 derived lock; stopAfter reuses existing tables; no stored lock).
- **Census discipline:** Tasks 1–2 touch only `fire.rs`/`stop_after.rs` + tests (behaviour), never `keys.rs`/`measure.rs`/`property-treatment.json`; the golden cannot move mid-feature. Task 3 is the single census move (honoured 1224 → 1226).
