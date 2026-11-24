# Concurrency: gate every kind of start (package 8c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a flow's `concurrency: {limit, behavior}` apply to replay, restart-on-source, restart-in-place and subflow children — not only fresh starts — matching Kestra's `CREATED | failedThenRestarted` gate, with one uniform drain.

**Architecture:** egret-workflow is a Rust (axum + sqlx) Kestra OSS port. Executions live in `workflow.executions`; the admission gate (`worker/concurrency.rs::concurrency_admit`) decides Run/Queue/Cancel/Fail under a per-flow `pg_advisory_xact_lock`; the drain (`concurrency_drain` + `promote_queued`) promotes the oldest `QUEUED` rows when a slot frees. Fresh starts are gated (8a/8b); this package routes the other start paths through the same gate and generalises `promote_queued` to resume any queued start (including a restart) by popping the trailing `QUEUED` from history.

**Tech Stack:** Rust, sqlx (Postgres), axum, utoipa; tests are `#[sqlx::test]` in `crates/egret-workflow/tests/`.

## Global Constraints

- Spec: `docs/en/specs/2026-11-24-workflow-concurrency-gate-all-starts-design.md` (VI mirror alongside). The spec's decisions D1–D7 bind every task; copy exact behaviour from it.
- **No migration. No new API route. No FE change** beyond the M2 error code (D6).
- Chat in Vietnamese; code, comments, commits, docs in English.
- Conventional Commits; `Closes #N` only in the PR body, never in a commit message. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Stage by explicit path; never `git add -A`; never stage `target*/`. Never commit secrets or scratch files.
- Run `cargo fmt --all --check` before any push. Scoped local test runs for `egret-workflow` **must** include `--test routes --test execution_timestamps` (both guards live in binaries a scoped run skips; they broke CI in 8a and 8b).
- One heavy Rust build at a time.
- Gate decision is uniform: `concurrency_admit(conn, flow_id, limit, behavior) -> Admit` (Run/Queue/Cancel/Fail), taken under the flow's advisory lock in the same transaction as the write that follows.
- The DRIVE (the walk's task-run creation) is what makes a run `RUNNING`/claimable; a QUEUE gate must stop **before** drive so a queued execution leaves no claimable run.

## File Structure

- `crates/egret-workflow/src/worker/claim.rs` — `promote_queued` (generalise, D3), the shared admit→row/settle helper (new), `insert_execution_gated`/`start_execution_in_on` (use the helper), `insert_execution` (add a gated variant returning `Admit`).
- `crates/egret-workflow/src/worker/copy.rs` — `replay_locked` gates its new execution (whole + partial replay, restart-on-source).
- `crates/egret-workflow/src/worker/restart.rs` — restart-in-place gates inside the walk (the risky seam).
- `crates/egret-workflow/src/worker/subflow.rs` — the `Subflow` child spawn gates on the child flow.
- `crates/egret-workflow/src/worker/concurrency.rs` — unchanged in shape; doc note that the drain now covers every kind of start.
- `crates/egret-workflow/src/routes.rs` — `unqueue_target` M2 fix (coded error for an unknown state).
- `crates/egret-workflow/tests/concurrency.rs` — new tests for every path + FIFO + drain equivalence.
- `crates/egret-workflow/tests/routes.rs` — M2 route test, MAX_BULK cap, bulk-over-limit race.
- egret-data/docs: `platform/workflows/flows/concurrency.md` (gated paths + deadlock note), `platform/workflows/executions/actions.md` (unqueue error note).

---

### Task 0: Workspace baseline

**Files:** none (environment only).

- [ ] **Step 1: Confirm the worktree and branch**

Run: `git -C /Users/1pro-l4r8/Documents/github/data-portal/egret-cc-gate rev-parse --abbrev-ref HEAD`
Expected: `feat/workflow-concurrency-gate-all-starts` (off `origin/main`, which has 8b `e183c3403`).

- [ ] **Step 2: Start test containers** (Postgres :5468 / Valkey :6468, names `egret-ccg-postgres` / `egret-ccg-valkey`; adjust ports if taken).

```bash
docker run -d --name egret-ccg-postgres -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret_test -p 5468:5432 postgres:16
docker run -d --name egret-ccg-valkey -p 6468:6379 valkey/valkey:8
export DATABASE_URL=postgres://egret:egret@localhost:5468/egret_test
```

- [ ] **Step 3: Baseline the concurrency suite**

Run: `cargo test -p egret-workflow --test concurrency`
Expected: PASS (the 8a/8b suite is green on a clean branch).

---

### Task 1: Uniform drain (history pop) + shared admit helper

**Files:**
- Modify: `crates/egret-workflow/src/worker/claim.rs` (`promote_queued`; new `admit_row` helper; `insert_execution_gated` + `start_execution_in_on` use it)
- Test: `crates/egret-workflow/tests/concurrency.rs`

**Interfaces:**
- Produces: `promote_queued(conn, execution, stack)` now restores the pre-queue state by popping the trailing `QUEUED` from the stored history (D3). Produces `admit_row(current_history: &[RunState], admit: Admit) -> AdmitRow` where `AdmitRow` says the `(state, history)` to persist and the settle disposition (`Run` / `Queue` / `Terminal(RunState)`). Consumed by Tasks 2–4.

- [ ] **Step 1: Write the failing test — a queued restart-shaped history pops back to RESTARTED**

Add to `tests/concurrency.rs`. This drives `promote_queued` indirectly: insert a row whose history ends `[…,"RESTARTED","QUEUED"]` with a real (restartable) source + a terminal sibling to free the slot, then a tick, and assert it resumes (state no longer `QUEUED`, history's last real state was `RESTARTED` not `CREATED`).

```rust
#[sqlx::test]
async fn a_queued_restart_resumes_to_restarted_not_created(pool: sqlx::PgPool) {
    // Arrange: limit 1. One live execution holds the slot; one QUEUED row whose
    // history ends on RESTARTED (as a restart-in-place gated to QUEUED would).
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let live = start(&pool, ws, Some(flow), &src).await;
    let queued = start(&pool, ws, Some(flow), &src).await;
    assert_eq!(state(&pool, queued).await, "QUEUED");
    // Force the QUEUED row to look like a gated restart.
    sqlx::query("UPDATE workflow.executions SET history = $2 WHERE id = $1")
        .bind(queued)
        .bind(json!(["CREATED", "RUNNING", "FAILED", "RESTARTED", "QUEUED"]))
        .execute(&pool).await.unwrap();

    // Act: terminate the live one, free the slot, drain.
    sqlx::query("UPDATE workflow.executions SET state = 'SUCCESS', \
                 history = history || '[\"SUCCESS\"]'::jsonb WHERE id = $1")
        .bind(live).execute(&pool).await.unwrap();
    egret_workflow::worker::concurrency::concurrency_sweep(&pool).await.unwrap();

    // Then: it left the queue; the history pop restored RESTARTED, so the
    // resumed run's history does NOT reset to ["CREATED"].
    assert_ne!(state(&pool, queued).await, "QUEUED");
    let h = history(&pool, queued).await;
    let arr = h.as_array().unwrap();
    assert!(arr.iter().any(|s| s == "RESTARTED"),
        "pop must keep the RESTARTED history, got {h}");
    assert_ne!(arr, &vec![json!("CREATED")], "must not reset to CREATED");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-workflow --test concurrency a_queued_restart_resumes_to_restarted_not_created`
Expected: FAIL — current `promote_queued` resets history to `["CREATED"]`.

- [ ] **Step 3: Generalise `promote_queued`**

In `claim.rs`, replace the hardcoded reset with a pop of the trailing `QUEUED`. The stored history is the source of truth; pop it, the new last element is the state to restore.

```rust
pub(super) async fn promote_queued(
    conn: &mut PgConnection,
    execution: Uuid,
    stack: Stack,
) -> Result<()> {
    // The drain restores the state the execution would have had if it had not
    // been queued: pop the trailing `QUEUED` the gate appended (package 8c D3),
    // leaving CREATED for a fresh start/replay/subflow child and RESTARTED for a
    // restart. Then `load` reads whatever runs exist (none for a fresh start, a
    // copied prefix for a partial replay, the reset runs for a restart) and the
    // settle resumes identically.
    let history: serde_json::Value =
        sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
    let mut hist: Vec<serde_json::Value> =
        history.as_array().cloned().unwrap_or_default();
    if hist.last().and_then(|v| v.as_str()) == Some("QUEUED") {
        hist.pop();
    }
    let restored = hist
        .last()
        .and_then(|v| v.as_str())
        .unwrap_or("CREATED")
        .to_string();
    sqlx::query(EXECUTION_STATE_UPDATE)
        .bind(&restored)
        .bind(serde_json::Value::Array(hist))
        .bind(execution)
        .bind(false)
        .bind(false)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    let (ctx, state, now) = load(conn, execution, stack).await?;
    settle(conn, &ctx, &state, state.clone(), now, None).await
}
```

- [ ] **Step 4: Add the shared `admit_row` helper and route the fresh-start path through it**

In `claim.rs`, factor the `Admit → (row state, row history, disposition)` decision used by `insert_execution_gated`/`start_execution_in_on` into one helper so Tasks 2–4 reuse it. `current_history` is `["CREATED"]` for a new row; a restart passes its live history.

```rust
/// What a gate outcome means for the row about to be written or reopened
/// (package 8c §5): the state + history to persist, and what the caller does
/// next.
pub(super) enum Disposition {
    /// Settle normally (drive).
    Run,
    /// Leave `QUEUED`; the drain resumes it. Do not settle.
    Queue,
    /// Terminalize through settle with this state, no task run (D5).
    Terminal(RunState),
}

pub(super) struct AdmitRow {
    pub state: String,
    pub history: serde_json::Value,
    pub disposition: Disposition,
}

/// `current_history` is the row's history before the gate (`["CREATED"]` for a
/// new start/replay/subflow child; the live history for a restart-in-place).
pub(super) fn admit_row(current_history: &[serde_json::Value], admit: Admit) -> AdmitRow {
    let push = |s: &str| {
        let mut h = current_history.to_vec();
        h.push(serde_json::json!(s));
        serde_json::Value::Array(h)
    };
    match admit {
        Admit::Run => AdmitRow {
            state: current_history.last().and_then(|v| v.as_str())
                .unwrap_or("CREATED").to_string(),
            history: serde_json::Value::Array(current_history.to_vec()),
            disposition: Disposition::Run,
        },
        Admit::Queue => AdmitRow {
            state: "QUEUED".into(),
            history: push("QUEUED"),
            disposition: Disposition::Queue,
        },
        // CANCEL/FAIL insert the pre-terminal state and let the caller's settle
        // write the terminal (born-FAILED style, D5), so the terminal hooks see
        // an ordinary transition.
        Admit::Cancel => AdmitRow {
            state: current_history.last().and_then(|v| v.as_str())
                .unwrap_or("CREATED").to_string(),
            history: serde_json::Value::Array(current_history.to_vec()),
            disposition: Disposition::Terminal(RunState::Cancelled),
        },
        Admit::Fail => AdmitRow {
            state: current_history.last().and_then(|v| v.as_str())
                .unwrap_or("CREATED").to_string(),
            history: serde_json::Value::Array(current_history.to_vec()),
            disposition: Disposition::Terminal(RunState::Failed),
        },
    }
}
```

Then rewrite `insert_execution_gated`'s `(row_state, row_history)` match and `start_execution_in_on`'s terminalization to go through `admit_row`/`Disposition`, keeping identical behaviour for the fresh-start path (the existing fresh-start tests are the regression guard).

- [ ] **Step 5: Run the whole concurrency suite + the regression guards**

Run: `cargo test -p egret-workflow --test concurrency && cargo test -p egret-workflow --test routes --test execution_timestamps`
Expected: PASS, including the new test and every existing 8a/8b test (drain equivalence: a queued fresh start still produces `["CREATED"]`).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workflow/src/worker/claim.rs crates/egret-workflow/tests/concurrency.rs
git commit -m "feat(workflow): uniform concurrency drain by history pop + shared admit helper (8c)"
```

---

### Task 2: Gate replay and restart-on-source (whole + the new-execution insert)

**Files:**
- Modify: `crates/egret-workflow/src/worker/copy.rs` (`replay_locked`)
- Modify: `crates/egret-workflow/src/worker/claim.rs` (a gated `insert_execution` variant returning `Admit`)
- Test: `crates/egret-workflow/tests/concurrency.rs`

**Interfaces:**
- Consumes: `admit_row`/`Disposition` (Task 1), `concurrency_admit` (8b).
- Produces: `insert_execution` gains a sibling `insert_execution_admitted(conn, gate, new) -> (ExecutionCtx, Admit)` that gates (the screen, then `concurrency_admit`, then the INSERT at the admitted row state). `replay_locked` branches on the `Admit`.

> **Note on partial replay / restart-on-source:** these copy runs and drive through the walk (`restart::copy_job` + `run_job`). Gating the *drive* for them is Task 4. Task 2 gates the **insert** so the new execution is born at the admitted state; for `Admit::Queue` it leaves the row `QUEUED` and must not reach the walk. Whole replay (no copied runs, `settle_new`) is fully handled here.

- [ ] **Step 1: Write the failing tests**

Add to `tests/concurrency.rs`. Helper to replay an execution (mirror the existing route/worker entry; use `egret_workflow::worker::copy::replay_execution` with a whole-replay request). Three cases on `behavior`.

```rust
#[sqlx::test]
async fn a_whole_replay_over_the_limit_is_queued(pool: sqlx::PgPool) {
    // Arrange: limit 1. One live holds the slot; replay a finished execution.
    let src = flow_src(Some((1, "QUEUE")));
    let (ws, flow) = setup(&pool, &src).await;
    let old = start(&pool, ws, Some(flow), &src).await;
    finish_success(&pool, old).await;      // test helper: force terminal SUCCESS
    let _live = start(&pool, ws, Some(flow), &src).await; // holds the one slot

    // Act: whole replay of `old`.
    let replayed = whole_replay(&pool, ws, old).await; // test helper → copy::replay_execution

    // Then: the replay is QUEUED, started no task.
    assert_eq!(state(&pool, replayed).await, "QUEUED");
    assert_eq!(run_count(&pool, replayed).await, 0);
    assert_eq!(history(&pool, replayed).await, json!(["CREATED", "QUEUED"]));
}

#[sqlx::test]
async fn a_whole_replay_over_a_cancel_limit_is_cancelled(pool: sqlx::PgPool) {
    let src = flow_src(Some((1, "CANCEL")));
    let (ws, flow) = setup(&pool, &src).await;
    let old = start(&pool, ws, Some(flow), &src).await;
    finish_success(&pool, old).await;
    let _live = start(&pool, ws, Some(flow), &src).await;

    let replayed = whole_replay(&pool, ws, old).await;

    assert_eq!(state(&pool, replayed).await, "CANCELLED");
    assert_eq!(run_count(&pool, replayed).await, 0);
    assert!(has_ended_at(&pool, replayed).await, "a terminal start stamps ended_at");
}
```

(Add a `FAIL` variant asserting `FAILED`. Add the `finish_success`, `whole_replay` helpers next to the existing helpers, using the public worker/copy entry points.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p egret-workflow --test concurrency a_whole_replay_over`
Expected: FAIL — today a replay bypasses the gate and starts.

- [ ] **Step 3: Add `insert_execution_admitted` and gate `replay_locked`**

In `claim.rs`, expose a gated variant that returns the `Admit` (the existing `insert_execution_gated` already computes it; add a `pub(super)` wrapper that passes `gated=true` and returns `(ctx, admit)`).

In `copy.rs::replay_locked`, replace `claim::insert_execution(...)` with the admitted variant, then branch:

```rust
let (ctx, admit) = claim::insert_execution_admitted(conn, gate, new).await?;
match claim::admit_row(&[serde_json::json!("CREATED")], admit).disposition {
    claim::Disposition::Queue => {
        // The copy's runs (if any) are still copied below so the drain can
        // resume a partial replay; for a whole replay there are none. Skip the
        // settle — the drain (promote_queued) resumes it.
        // ... (copy runs for the partial case, then) return Ok(Ok(ctx.id));
    }
    claim::Disposition::Terminal(state) => {
        // Terminalize the new execution through its first settle, no runs (D5).
        let empty = ExecutionState::new(&ctx.plan);
        let mut terminal = empty.clone();
        apply(&mut terminal, &Command::SetExecutionState { state });
        claim::settle(conn, &ctx, &empty, terminal, Tick(0), None).await?;
        return Ok(Ok(ctx.id));
    }
    claim::Disposition::Run => { /* fall through to today's copy + settle */ }
}
```

For Task 2, implement the **whole-replay** branch fully (the `copied.is_none()` path: Queue → set row QUEUED via `admit_row` state/history and return without `settle_new`; Terminal → terminalize). Leave the partial-replay drive gating (the `copied.is_some()` walk path) to Task 4 — for now a partial replay under Queue may still drive (documented, closed by Task 4).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p egret-workflow --test concurrency a_whole_replay_over`
Expected: PASS.

- [ ] **Step 5: Full suite + guards**

Run: `cargo test -p egret-workflow --test concurrency --test routes --test execution_timestamps`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workflow/src/worker/copy.rs crates/egret-workflow/src/worker/claim.rs crates/egret-workflow/tests/concurrency.rs
git commit -m "feat(workflow): gate whole replay under flow concurrency (8c)"
```

---

### Task 3: Gate the subflow child

**Files:**
- Modify: `crates/egret-workflow/src/worker/subflow.rs` (the child spawn/INSERT ~line 557)
- Test: `crates/egret-workflow/tests/concurrency.rs`

**Interfaces:**
- Consumes: `concurrency_admit`, `admit_row`/`Disposition`.
- Produces: a subflow child born over the child flow's limit is QUEUED (parent waits, drain runs it) or terminal (parent reacts), per behaviour.

- [ ] **Step 1: Write the failing test**

A parent flow whose single task is a `Subflow` calling a child flow that has `concurrency: {limit: 1, behavior: QUEUE}`. Pre-occupy the child flow's one slot with a live execution, run the parent, assert the child is QUEUED and starts no task; then free the slot, tick, assert the child runs.

```rust
#[sqlx::test]
async fn a_subflow_child_over_the_childs_limit_is_queued(pool: sqlx::PgPool) {
    // Arrange: child flow `t.child` limit 1 QUEUE, one live holding its slot.
    let child_src = "id: child\nnamespace: t\nconcurrency:\n  limit: 1\n  behavior: QUEUE\n\
                     tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let (ws, child_flow) = setup(&pool, child_src).await;
    let _hold = start(&pool, ws, Some(child_flow), child_src).await;
    let parent_src = "id: parent\nnamespace: t\ntasks:\n  - id: s\n    \
                      type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n    wait: true\n";
    let (_ws2, parent_flow) = setup(&pool, parent_src).await;

    // Act: run the parent; it spawns the child, which is over the child's limit.
    let parent = start(&pool, ws, Some(parent_flow), parent_src).await;
    tick(&pool, 3).await;

    // Then: a child execution of `child_flow` exists and is QUEUED, no task run.
    let child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE flow_id = $1 AND trigger_execution_id = $2")
        .bind(child_flow).bind(parent).fetch_one(&pool).await.unwrap();
    assert_eq!(state(&pool, child).await, "QUEUED");
    assert_eq!(run_count(&pool, child).await, 0);
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p egret-workflow --test concurrency a_subflow_child_over`. Expected FAIL (child bypasses the gate).

- [ ] **Step 3: Gate the child spawn**

In `subflow.rs`, before/at the child INSERT, resolve the child flow's `concurrency`, call `concurrency_admit(conn, child_flow, limit, behavior)` under the child flow's advisory lock, and use `admit_row(&["CREATED"], admit)` to decide:
- `Run` → today's INSERT (`CREATED`) + child settle.
- `Queue` → INSERT the child at `QUEUED` (history `["CREATED","QUEUED"]`); **do not** run the child settle (`child_ctx` settle). The parent's `Subflow` run reads a non-terminal child and keeps waiting; the drain runs the child later.
- `Terminal(state)` → INSERT `CREATED`, then run the child settle with `SetExecutionState { state }` applied (no task run); the parent sees a terminal child.

The gate's advisory lock is per child flow; it must sit in the same transaction as the child INSERT (the existing savepoint for the FILE-input check is inside this transaction — take the lock before the INSERT as `insert_execution_gated` does).

- [ ] **Step 4: Run to verify it passes** — `cargo test -p egret-workflow --test concurrency a_subflow_child`. Expected PASS. Add CANCEL/FAIL variants (child terminal, parent `Subflow` run FAILED/handled) and a drain-resumes-the-child variant.

- [ ] **Step 5: Full suite + guards** — `cargo test -p egret-workflow --test concurrency --test routes --test execution_timestamps`. Expected PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workflow/src/worker/subflow.rs crates/egret-workflow/tests/concurrency.rs
git commit -m "feat(workflow): gate subflow children under the child flow's concurrency (8c)"
```

---

### Task 4: Gate the walk-based paths (partial replay, restart-on-source, restart-in-place) — HIGH RISK

**Files:**
- Modify: `crates/egret-workflow/src/worker/restart.rs` (the walk's Restart open/drive seam)
- Modify: `crates/egret-workflow/src/worker/copy.rs` (partial-replay drive)
- Test: `crates/egret-workflow/tests/concurrency.rs`

**Interfaces:**
- Consumes: `concurrency_admit`, `admit_row`/`Disposition`, the generalised `promote_queued` (Task 1), which resumes a `[…,"RESTARTED","QUEUED"]` row to `RESTARTED`.

> **This is the one seam inside the walk (spec §4.3, §9).** The walk (`restart::open` → `claim::run_job`/`walk`) persists the `RESTARTED` transition and reset runs, then drives. The gate must fall **after** the resets + `RESTARTED` are persisted and **before** the drive: if over a QUEUE limit, append `QUEUED` and stop (leaving reset runs that are not `RUNNING`, hence not claimable — see Global Constraints); if CANCEL/FAIL, terminalize instead of driving. The drain resumes via `promote_queued`. **Fallback (D1/§9):** if the seam proves intractable, keep partial-replay/restart-in-place at soft-overshoot under QUEUE only (still terminalize under CANCEL/FAIL), ship Tasks 1–3 + 5, and record QUEUE gating for these paths as package 8d. Escalate to the human before taking the fallback.

- [ ] **Step 1: Write the failing tests**

```rust
#[sqlx::test]
async fn a_restart_in_place_over_the_limit_is_queued_then_resumes(pool: sqlx::PgPool) {
    // Arrange: limit 1. A FAILED execution to restart; a live one holding the slot.
    let src = flow_src_failing(Some((1, "QUEUE"))); // a source whose task fails
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick(&pool, 3).await;
    assert_eq!(state(&pool, failed).await, "FAILED");
    let _live = start(&pool, ws, Some(flow), &src).await; // holds the slot

    // Act: restart the failed execution while over the limit.
    egret_workflow::worker::restart::restart_execution(&pool, failed).await.unwrap();

    // Then: it is QUEUED (not RESTARTED/RUNNING), history records the RESTARTED it
    // would have taken, no run is RUNNING.
    assert_eq!(state(&pool, failed).await, "QUEUED");
    let h = history(&pool, failed).await;
    assert_eq!(h.as_array().unwrap().last().unwrap(), "QUEUED");
    assert!(h.as_array().unwrap().iter().any(|s| s == "RESTARTED"));
    assert_eq!(running_run_count(&pool, failed).await, 0);

    // And: freeing the slot drains it back to a real restart.
    finish_success(&pool, _live).await;
    egret_workflow::worker::concurrency::concurrency_sweep(&pool).await.unwrap();
    assert_ne!(state(&pool, failed).await, "QUEUED");
}

#[sqlx::test]
async fn a_restart_in_place_over_a_fail_limit_is_failed(pool: sqlx::PgPool) {
    let src = flow_src_failing(Some((1, "FAIL")));
    let (ws, flow) = setup(&pool, &src).await;
    let failed = start(&pool, ws, Some(flow), &src).await;
    tick(&pool, 3).await;
    let _live = start(&pool, ws, Some(flow), &src).await;
    egret_workflow::worker::restart::restart_execution(&pool, failed).await.unwrap();
    assert_eq!(state(&pool, failed).await, "FAILED"); // re-failed by the gate, no drive
}
```

(Add a partial-replay-over-QUEUE test: replay from a task of a finished execution while over the limit → QUEUED with the copied prefix present, then drained to a resume.)

- [ ] **Step 2: Run to verify they fail** — `cargo test -p egret-workflow --test concurrency a_restart_in_place_over`. Expected FAIL (restart bypasses the gate and drives).

- [ ] **Step 3: Locate the seam and gate it**

In `restart.rs`, the walk opens a `Job::Restart(Opening{ id, prepared })` where `prepared.state` already ends on `RESTARTED` and `prepared.cmds` carry the run resets. Find where the walk persists that transition before `drive` (follow `open` → `walk`/`walk_from`). Insert the gate there (under the flow's advisory lock, same transaction):
- resolve the flow's `concurrency` from `prepared.ctx` (the source is on `ctx`);
- `concurrency_admit(conn, flow_id, limit, behavior)`;
- `admit_row(&prepared.state.history_as_values(), admit)` → persist that state/history (QUEUE appends `QUEUED`; Terminal keeps the pre-terminal and the settle writes the terminal) and set the disposition;
- `Disposition::Queue` → persist the reset runs + the `QUEUED` row, **return before drive**;
- `Disposition::Terminal(state)` → apply `SetExecutionState { state }` and settle without driving;
- `Disposition::Run` → drive (today).

Apply the same gate to the partial-replay drive in `copy.rs` (the `copied.is_some()` path that calls `restart::copy_job` + `run_job`): since both go through the walk, prefer gating once in the walk's Restart handling so the two cannot drift.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p egret-workflow --test concurrency a_restart_in_place a_partial_replay`. Expected PASS.

- [ ] **Step 5: Full suite + guards** — `cargo test -p egret-workflow --test concurrency --test routes --test execution_timestamps`. Expected PASS. Confirm no QUEUED execution has a `RUNNING` task run (the "not claimable" invariant).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workflow/src/worker/restart.rs crates/egret-workflow/src/worker/copy.rs crates/egret-workflow/tests/concurrency.rs
git commit -m "feat(workflow): gate restart and partial replay under flow concurrency (8c)"
```

---

### Task 5: 8b residual minors — M2 error code + the three tests

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (`unqueue_target`)
- Test: `crates/egret-workflow/tests/routes.rs`

**Interfaces:** none new; a coded 400 for an unknown `?state=`.

- [ ] **Step 1: Write the failing test (M2)**

In `tests/routes.rs`, add a test that `DELETE …/executions/{id}/unqueue?state=bogus` returns 400 with the coded body `unqueue_state_not_terminal` (not an uncoded `InvalidInput`). Mirror the existing unqueue route tests for the request shape.

- [ ] **Step 2: Run to verify it fails** — Expected FAIL (today the unknown-state branch returns an uncoded `Error::InvalidInput`).

- [ ] **Step 3: Fix `unqueue_target`**

```rust
let state = crate::worker::from_wire(&raw.to_ascii_uppercase()).ok_or_else(|| {
    ExecOpError::Coded(
        StatusCode::BAD_REQUEST,
        UNQUEUE_STATE_NOT_TERMINAL_CODE,
        format!("`{raw}` is not an execution state; an unqueued execution must end in a terminal one"),
    )
})?;
```

(The openapi doc for the 400 already reads "or not an execution state at all", so the code is consistent; no doc change needed there.)

- [ ] **Step 4: Run to verify it passes** — Expected PASS.

- [ ] **Step 5: Add the MAX_BULK-cap and bulk-over-limit tests**

- A test asserting a by-ids unqueue and a by-ids force-run with >1000 ids return 400 `too_many` (reuse the 7f `MAX_BULK` assertion pattern).
- A test that a bulk force-run crossing the limit respects the behaviour: force-run N queued executions of a limit-M flow; assert the drain's live count never exceeds what the forced bypass allows and no execution is lost (all either run or stay queued correctly). Deterministic (no sleeps): drive via `tick`/`concurrency_sweep`.

- [ ] **Step 6: Add the deterministic deadlock-race test**

In `tests/concurrency.rs`, construct the ABBA that 8b's deadlock-retry absorbs: one task takes the per-flow advisory lock then a row lock (a settle-drain mutator), another takes the row lock then the advisory (force-run/unqueue), run them concurrently against the same flow, and assert both complete (the retry resolves the deadlock) with a correct final state. Keep it deterministic with explicit transaction ordering (two `tokio::spawn`ed transactions synchronised on a barrier), not timing.

- [ ] **Step 7: Full suite + guards + fmt**

Run: `cargo fmt --all --check && cargo test -p egret-workflow --test concurrency --test routes --test execution_timestamps`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-workflow/src/routes.rs crates/egret-workflow/tests/routes.rs crates/egret-workflow/tests/concurrency.rs
git commit -m "fix(workflow): coded error for unknown unqueue state + concurrency race/cap tests (8c)"
```

---

### Task Z: Docs, gates, live check, PR

**Files:**
- egret-data/docs: `platform/workflows/flows/concurrency.md`, `platform/workflows/executions/actions.md`
- No code change beyond what Tasks 1–5 committed.

- [ ] **Step 1: Docs — concurrency.md**

Update the concurrency doc: the limit now applies to replay, restart and subflow children (not only fresh starts); each obeys the flow's behaviour (QUEUE → QUEUED, CANCEL/FAIL → terminal); a queued replay/restart/subflow child can be unqueued or force-run like any queued execution. Add the deadlock note (D2/§7): a self-referential `Subflow` with `wait: true` under a low limit can stall; use force-run/unqueue to break it.

- [ ] **Step 2: Docs — actions.md**

Note that unqueue's `?state=`/`target_state` must be a terminal execution state; an unknown or non-terminal value is refused `unqueue_state_not_terminal` (400).

- [ ] **Step 3: Whole-branch gates**

Run: `cargo fmt --all --check`, `cargo clippy -p egret-workflow --all-targets`, `cargo test -p egret-workflow --test concurrency --test routes --test execution_timestamps`.
Expected: clean.

- [ ] **Step 4: Live check (no image build)**

Per the roadmap recipe: `target/debug/egret serve` on its own port + env file (own DB, dummy storage creds) + a scratch `vite.<name>.config.ts` (untracked) proxying `/api`. Verify in the browser: a flow with `concurrency: {limit: 1}` where a replay/restart/subflow child over the limit shows `QUEUED`, and unqueue/force-run act on it. Screenshots via Playwright in the scratchpad. Never commit the vite config.

- [ ] **Step 5: Push + open the docs PR and the code PR**

- Docs PR against egret-data/docs (merges at once on "merge đi").
- Code PR: branch `feat/workflow-concurrency-gate-all-starts`; body ends with the attribution line and `Closes` the 8c tracking issue if one exists; ticks the Docs box. Wait for green CI, no `--admin`, `cargo fmt --all --check` first.

- [ ] **Step 6: Merge on green CI**, then clean up (worktree `egret-cc-gate`, containers `egret-ccg-*`, SDD workspace, remote branch), update the roadmap memory (8c merged), and notify the user.

## Self-Review

- **Spec coverage:** D1 (four paths) → Tasks 2/3/4; D2 (deadlock doc) → Task Z; D3 (pop drain) → Task 1; D4 (append) → Task 1 `admit_row`; D5 (terminalize) → `Disposition::Terminal` in Tasks 1–4; D6 (no migration/API/FE) → honoured (no such step); D7 (minors) → Task 5. §9 fallback → Task 4 note.
- **Placeholder scan:** the only deliberately-unspecified detail is Task 4's exact walk line (a genuine internal the implementer locates), bounded by concrete tests + a named fallback — not a TODO. Test helpers `finish_success`, `whole_replay`, `flow_src_failing`, `running_run_count` are named and described; the implementer writes them beside the existing `tests/concurrency.rs` helpers.
- **Type consistency:** `Admit` (Run/Queue/Cancel/Fail) is the 8b enum; `admit_row`/`AdmitRow`/`Disposition` are introduced in Task 1 and consumed by name in Tasks 2–4; `promote_queued`/`concurrency_admit`/`concurrency_sweep` signatures match the current code.
