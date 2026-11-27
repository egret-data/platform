# Workflow Subflow scheduleDate (package #12b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use `- [ ]`.
> DRAFT in scratchpad — formalize to `docs/{en,vi}/plans/2026-11-27-workflow-subflow-scheduledate.md` on the #12b worktree (off main-after-#12 `4796619ea`) before executing.

**Goal:** Honour Subflow `scheduleDate` — a future `scheduleDate` parks the spawned child in `CREATED` and a waker starts it when the instant is due; a `wait:true` parent waits across the delay.

**Architecture:** A dedicated deferred-start mechanism. A new `workflow.executions.scheduled_start timestamptz` column (migration 0246) holds the instant; `worker::subflow::spawn_one` parks the child (insert CREATED, do NOT drive the first settle) instead of starting it when `scheduleDate` is future; a waker hooked into the existing worker poll cadence (the `reopen_due`/`spawn_due` seam) drives the first settle of children whose `scheduled_start` is due, exactly once. Independent of `scheduled_date` (the scheduler tick-identity column) and its CHECK.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first:** Task 0 pins Kestra `scheduleDate` semantics on the local pinned source `/Users/1pro-l4r8/Documents/github/data-portal/kestra` @ `96893b589`, and the exact egret spawn/waker seams, before any code.
- **D1** dedicated `scheduled_start` column (migration **0246**), NOT an overload of `scheduled_date` (no touching the `workflow_executions_scheduled_date_iff_schedule` CHECK that 11e's 0244 narrowed). **D2** the waker reuses the existing worker poll cadence (the `executor::retry::reopen_due` / `worker::replay::spawn_due` / `worker::claim` loop), no new timer. **D3** future `scheduleDate` → park CREATED; past/now → start immediately; unrenderable → fail the Subflow run (as other Subflow render errors). **D4** the parked child sits `CREATED`; `wait:true` parent waits across the delay unchanged. **D5** census: `scheduleDate` lifts from `Lift::Deferred` → honoured (1261 → **1262**). **D6** migration 0246 (next free after 0245).
- **Census:** `property-treatment.json` golden-tested; it moves once, in the reconcile task (Task 2). The subsystem (Task 1) is behaviour + migration only.
- **Scoped runs** include the subflow/executor/restart/routes/execution_timestamps suites + the waker's test file; tick/waker-driving tests hold the file-local serialising mutex (the worker poll claims globally).
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` in PR body only. Before done per task: `cargo fmt --all --check`, **`cargo clippy --all-targets -- -D warnings`** (CI runs full-workspace clippy; `cargo check` alone missed `large_enum_variant` in 11e — run clippy), `cargo check -p egret-workflow`. Foreground builds, one at a time. New env vars → egret-config `ENV_OUTSIDE_CONFIG`.

---

### Task 0: Measure — Kestra scheduleDate semantics + egret spawn/waker seams (load-bearing, report only)

**Files:** none (report).

- [ ] **Step 1 — Kestra semantics.** On the pinned source (`ExecutableUtils.java:228-250`, `Execution.newExecution`): confirm `scheduleDate` is rendered against the PARENT runContext; the child execution's STATE while deferred (CREATED); what the executor does with it (holds until the instant); the handling of a PAST `scheduleDate` (start now, or clamp?) and an unrenderable/invalid one (fail vs ignore); how `wait:true` + `transmitFailed` interact with a deferred child.
- [ ] **Step 2 — egret spawn seam.** In `worker::subflow::spawn_one` (`worker/subflow.rs:284`), find the EXACT point where the child's first `settle` is driven inside the caller's transaction, so the implementer can branch "park (insert CREATED, set `scheduled_start`, skip settle) vs settle now". Record how the child row is inserted (the INSERT around subflow.rs:~450-525) so a `scheduled_start` bind can be added.
- [ ] **Step 3 — egret waker seam.** Identify the exact worker poll loop to hook (`executor::retry::reopen_due` + `worker::claim::due_retries` / `worker::replay::spawn_due` — read `worker/claim.rs:596-666`, `worker/replay.rs:110,362`): its cadence, how it claims due work, and how it drives a run's reopen/settle OUTSIDE any caller transaction. Determine how to drive a parked child's FIRST settle when woken later (the child never had its first settle — `spawn_one` normally does it in the parent txn; the waker must do the equivalent now, standalone). Record the idempotency/row-lock guard that makes a due parked child start EXACTLY once under concurrent pollers.
- [ ] **Step 4 — migration + census.** Confirm `scheduled_start` needs only a plain nullable column + a partial index (`WHERE scheduled_start IS NOT NULL`) and NO CHECK/trigger coupling; next free number is 0246 (disk at 0245). Record the census baseline (post-#12: 1261/3/363/120/1747), the `scheduleDate` lift site (`keys.rs` the `(Subflow, scheduleDate) => Lift::Deferred(...)` arm) and the warning-assertion tests that pin it (grep `Lift::Deferred` / "package #12b" across `src/ tests/`), and the #12 docs "coming later" note to update. Record a BASE commit.

---

### Task 1: The deferred-start subsystem — migration 0246 + spawn park + waker

**Files:**
- Create: `crates/egret-workflow/migrations/0246_execution_scheduled_start.sql`
- Modify: `crates/egret-workflow/src/worker/subflow.rs` (`spawn_one` park branch + the child INSERT)
- Modify: `crates/egret-workflow/src/models/trigger.rs`/`models/task.rs` — parse `scheduleDate` on the `Subflow` struct if not already present (grep; #12 left it unparsed since it was deferred)
- Modify: the waker seam Task 0 §3 names (likely `crates/egret-workflow/src/worker/claim.rs` or a new `worker/deferred_start.rs` + its hook in the worker loop)
- Test: `crates/egret-workflow/tests/subflow_schedule_date.rs`

**Interfaces:**
- Consumes: Task 0 §2 (spawn seam) + §3 (waker seam, idempotency guard). If Task 0 finds driving a child's first settle standalone is a larger change than hooking the existing waker, STOP and report (it may warrant splitting park vs waker into two tasks).
- Produces: `workflow.executions.scheduled_start` (NULL normally); a parked child is `CREATED` with `scheduled_start` set and no settle driven; the waker drives its first settle when due.

- [ ] **Step 1 — Migration.** `migrations/0246_execution_scheduled_start.sql`:

```sql
-- Subflow scheduleDate (package #12b): the instant a deferred child execution
-- should start. NULL for every normal execution; set on a Subflow child whose
-- scheduleDate is in the future. Independent of scheduled_date (the scheduler
-- tick identity). The waker (worker loop) starts a CREATED child once this is due.
ALTER TABLE workflow.executions ADD COLUMN scheduled_start timestamptz;
CREATE INDEX workflow_executions_scheduled_start
  ON workflow.executions (scheduled_start)
  WHERE scheduled_start IS NOT NULL;
```

- [ ] **Step 2 — Failing test (park).** A parent flow whose Subflow declares `scheduleDate` a short future instant → after the parent's spawn, the child row exists in `CREATED` with `scheduled_start` ≈ the instant, and has NO task runs / is not RUNNING. Pin the deferred state to Task 0 §1.
- [ ] **Step 3 — Run → fail** (`scheduled_start` column/parse/park missing).
- [ ] **Step 4 — Parse + park.** Parse `schedule_date` on the `Subflow` model. In `spawn_one`, render `scheduleDate` against the parent context (Task 0 §1): a FUTURE instant → insert the child with `scheduled_start = T`, `state = CREATED`, and do NOT drive the first settle (return before the settle-drive point Task 0 §2 named); PAST/now → unchanged (settle now); a render error → fail the Subflow run via the existing Subflow render-error path.
- [ ] **Step 5 — Run → pass** the park test.
- [ ] **Step 6 — Failing test (waker).** A parked child whose `scheduled_start` is now in the past → one worker poll starts it (first settle driven), it runs to terminal, and the `wait:true` parent completes. A second poll does NOT double-start it. Use the file-local serialising mutex.
- [ ] **Step 7 — The waker.** Hook the existing worker poll cadence (Task 0 §3): claim CREATED children with `scheduled_start <= now()` under a row lock, drive each one's first settle standalone (the equivalent of `spawn_one`'s first settle, now outside any parent txn), and clear `scheduled_start` (or rely on `state != CREATED`) so it is not re-woken. Idempotent single-start per Task 0 §3.
- [ ] **Step 8 — Run → pass** the waker test + the no-double-start assertion.
- [ ] **Step 9 — Scoped + fmt + clippy.** `cargo test -p egret-workflow --test subflow_schedule_date --test executor --test routes --test execution_timestamps` green; `cargo fmt --all --check`; `cargo clippy --all-targets -- -D warnings`; `cargo check -p egret-workflow`.
- [ ] **Step 10 — Commit.** `feat(workflow): Subflow scheduleDate — defer the child start (migration 0246 + waker)`

---

### Task 2: Reconcile — lift scheduleDate to honoured, census, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (drop the `(Subflow, scheduleDate) => Lift::Deferred` arm → None), `crates/egret-workflow/src/spec/schema/property-treatment.json` (regen), `crates/egret-workflow/src/spec/schema/README.md`
- Create: docs in egret-data/docs (separate PR, controller-handled)

**Interfaces:** Consumes Task 0 §4 lift map.

- [ ] **Step 1 — Lift.** In `keys.rs`, `(Subflow, scheduleDate)` no longer warns (drop the `Lift::Deferred` arm → None). If `Lift::Deferred` now has no remaining user, remove the variant + its `short()`/`sentence()` arms; otherwise leave it. Grep + update the warning-assertion tests (the `flowable_keys…` test that expects the #12b message on `scheduleDate`).
- [ ] **Step 2 — Regen census.** `EGRET_WRITE_PROPERTY_TREATMENT=1` then re-run without. `Subflow.scheduleDate` flips ignored→honoured; **expect 1262 honoured / 3 / 362 ignored / 120 / 1747**. Update README Subflow row + Result line.
- [ ] **Step 3 — Run** `cargo test -p egret-workflow spec::keys` + census golden + `--test routes --test execution_timestamps` green; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all --check`.
- [ ] **Step 4 — Docs (egret-data/docs, separate PR).** Update the Subflow doc: `scheduleDate` now honoured (remove the "#12 coming later" note), document the deferred-start behaviour + the poll-cadence divergence. Bilingual.
- [ ] **Step 5 — Commit.** `feat(workflow): lift Subflow scheduleDate to honoured + reconcile census (package #12b)`

---

## Self-Review notes

- **Spec coverage:** scheduleDate semantics/park/waker → Task 1; census/lift/docs → Task 2; measurement → Task 0. D1–D6 are Global Constraints.
- **Risk:** Task 1 is the subsystem; the sharpest unknown is driving a parked child's FIRST settle standalone from the waker (spawn_one normally does it in the parent txn). Task 0 §3 pins this; if it is a larger change than a hook, Task 1 splits into 1a (migration + park) and 1b (waker) — flagged in Task 1's Interfaces.
- **Migration:** 0246, plain nullable column + partial index, no CHECK — the whole point of D1 (avoid re-broadening the scheduled_date CHECK).
- **Census discipline:** Task 1 is behaviour + migration only (no keys.rs/measure.rs/property-treatment.json); Task 2 is the single census move (1261→1262).
