# Workflow package #7a "Restart, task-run state and execution status" — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to carry out this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a person with `workflow:manage` can restart a `FAILED`/`PAUSED` execution in place, set one task run's state, and change a finished execution's status, from the API and the execution page, with Kestra's semantics (spec D1–D14).

**Architecture:** two pure executor functions, `executor::restart::restart` and `executor::restart::mark_as`. Each returns `Command`s that reset runs in memory: a run Kestra removes is emptied, never deleted. Three worker entry points apply those commands under the execution's lock and `settle`, the way `resume_by_hand` does. `Loop` iterations and `Subflow` children are restarted in place by the same entry point. Three routes, a migration that relaxes one CHECK, three frontend actions, and docs.

**Tech stack:** Rust (axum, sqlx, Postgres), `crates/egret-workflow`; React + orval client in `frontend/`.

Spec: `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md`. Its Q1–Q2 and D1–D14 are binding, and the spec's file:line citations are its evidence. Branch `feat/workflow-execution-ops`, one PR. Tasks: T0, A1–A5 (engine), B1 (routes), C1 (frontend), Z (docs, gates, live check).

**Review is not a task of this plan.** The controller reviews each task between tasks, and runs one whole-branch review after Z. Fixes are recorded in spec §7 as fix waves.

## Global constraints

- Spec Q1–Q2 and D1–D14 are binding. Kestra source: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` at `96893b58`. When the spec and Kestra disagree, the spec wins. When the spec and egret's code disagree, stop and report; do not improvise.
- **Code search:** use CodeGraph first, `codegraph_explore` with `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Grep only for literal strings. `rg` is not installed.
- Conventional Commits. Trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **No network** for subagents: no `git push`/`fetch`/`pull`, no `gh`, no `npm ci`. The controller runs `npm ci` / `npm run gen:api` when C1 needs them.
- Never touch port 5432 or the `egret-dev-*`/`egret-demo-*` containers. Scratch files and `.superpowers/` never go in git.
- **Build and test environment:**
  - `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target`
  - Test DB: `DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test`, container `egret-execops-postgres`.
  - Shared store: `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465`, container `egret-execops-valkey`.
  - Never run two cargo commands at once.
  - Run `df -h` before a full build (84 GiB free at design time). Clean `target/debug/incremental` rather than starting a second target dir.
- **Tests per task:** each task runs only the tests for what it touches. The full suites run once, at Z.
- **Red first:** the test that shows the gap exists before the fix.
- Every new `std::env::var` must be added to `egret-config`'s `ENV_OUTSIDE_CONFIG` allow-list. Run `cargo test -p egret-config` when one is added. None is expected.
- **The executor ratchet must not move:** **301 / 25 / 25 / 0**, census `LIVE` 295 / `VALIDATE_REFUSED` 25 / `UNKNOWN_TYPE_REFUSED` 21 (spec §4). Any change is a regression to report, not a bank.
- **Words in code and messages:** the user-facing refusal codes are exactly
  - `nothing_to_restart`
  - `not_restartable`
  - `after_execution_running`
  - `files_pruned`
  - `state_unchanged`
  - `restart_revision_unsupported`

  The audit actions are exactly `workflow_execution:restart`, `workflow_execution:set_task_run_state` and `workflow_execution:set_status`. The engine label is `system.restarted` with value `"true"`.

## T0. Environment and baseline

- [ ] Start the test containers. Follow the `docker run` lines used by earlier packages, which a grep for `egret-loopout-postgres` finds in the memory notes or shell history. Use image `postgres:16` on host port **5465**, user/password/db `egret`/`egret`/`egret_test`, and a Valkey container on host port **6465**.
- [ ] Run the executor ratchet and census: `cargo test -p egret-workflow --test executor -- --nocapture`. Record the tuple.
- [ ] Re-read migration numbers: `git ls-tree --name-only origin/main migrations/ | tail -1` and every local branch. `0225` must be free (spec D12).
- **Accept:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; the highest migration is `0219`. Report all three, as measured.

## A. Engine

### A1. The pure reset functions (spec D2, D3, D4, D6, D11)

**Files:**
- Create: `crates/egret-workflow/src/executor/restart.rs` (with `#[cfg(test)] mod tests`).
- Modify:
  - `crates/egret-workflow/src/executor/command.rs`: new commands.
  - `crates/egret-workflow/src/executor/state.rs`: the `apply` arms.
  - `crates/egret-workflow/src/executor/mod.rs`: `pub mod restart;` and re-exports.

**Interfaces produced:**
```rust
pub enum RestartRefusal { NothingToRestart, NotRestartable(RunState) }
/// Spec D2–D4, D6. `now` is the restart's tick.
pub fn restart(plan: &Plan, state: &ExecutionState, now: Tick)
    -> Result<RestartCommands, RestartRefusal>;
/// Spec D11 (Kestra `markAs`). `target` is one of SUCCESS/WARNING/FAILED/CANCELLED.
pub fn mark_as(plan: &Plan, state: &ExecutionState, run: RunIx, target: RunState, now: Tick)
    -> Result<RestartCommands, RestartRefusal>;
pub struct RestartCommands {
    pub commands: Vec<Command>,
    /// Runs emptied by D4/D3 (WorkingDirectory descendants): the worker renews their ids.
    pub emptied: Vec<RunIx>,
    /// Runs reset to CREATED/RUNNING: the worker clears their outputs/message/secret_outputs.
    pub reset: Vec<RunIx>,
    /// `Loop` runs set back to RUNNING whose last failing iteration the worker restarts (D8).
    pub loops: Vec<RunIx>,
    /// `Subflow` runs reset to CREATED: the worker looks up the earlier child (D9).
    pub subflows: Vec<RunIx>,
}
// command.rs
Command::EmptyRun { run: RunIx }              // as RestartLoop empties a descendant, for one run
Command::ReopenFlowable { run: RunIx, at: Tick } // push RUNNING and open a new Attempt at `at` (D6)
Command::OverwriteLastAttempt { run: RunIx, state: RunState } // mark_as: set the run's state AND its last attempt's, no new attempt
```
- **Reuse, do not duplicate.** `EmptyRun` must clear exactly what `RestartLoop` clears on a descendant (`state.rs:998-1041`): history, attempts, renders, `not_before`, `resume_with`, `working_dir`. Factor the descendant-clearing body of `RestartLoop` into one private fn that both arms call.
- **The rules (spec, quoted):**
  - **Reset set.** Every run whose current state is `FAILED` or `PAUSED`, plus every ancestor of one. Other states are kept unless they are an ancestor.
  - **State per kind (D3 table):**
    - a runnable leaf or `Subflow` → `SetRunState{Created}`;
    - a flowable ancestor and `Pause` → `ReopenFlowable` (D6: a new attempt at `now`);
    - `WorkingDirectory` → `SetRunState{Created}`, every descendant `EmptyRun`;
    - `LoopUntil` → `ReopenFlowable`;
    - `Loop` → `ReopenFlowable` and listed in `loops`.
  - **End-hook set (D4).** Every run whose task is in the flow's `errors:`/`finally:`, or in an `errors:`/`finally:` of any flowable (with children), or in `afterExecution:` → `EmptyRun` and listed in `emptied`. A run that is already empty is skipped. Use the plan's own phase membership: `after_execution::in_phase` and the flowable `errors`/`finally` lists that `current_list` reads. Do not match task ids by string.
  - **Execution.** `SetExecutionState{Restarted}` last. `restart` refuses `NotRestartable` unless the execution's current state is `FAILED` or `PAUSED`, and refuses `NothingToRestart` when the reset set is empty.
  - **`mark_as`.** The target run gets `SetRunState{target}`, and its last attempt's state is overwritten with no new attempt. This needs a third command, `Command::OverwriteLastAttempt { run, state }`; add it beside the others. The ancestors get `ReopenFlowable`, the end-hook set is emptied, and then:
    - target `CANCELLED` → `SetExecutionState{Cancelled}`;
    - else if any run is still `PAUSED` → no execution state;
    - else → `SetExecutionState{Restarted}`.

    `mark_as` refuses `NotRestartable` unless the execution is terminal and not `KILLED`.
- [ ] **Tests (pure, hand-built states as in `state.rs`'s tests and `render.rs:1474-1498`).** For each case: build the plan with `build_plan`, drive it to the failed shape with `apply`, call the function, `apply` its commands, and assert. Then assert that one `advance` round on the result does the next right thing (CREATED leaves get opened, not `Waiting`).
  1. A sequence `a, b(fails), c`: only `b` is reset to CREATED. `a` is untouched. `c` has no run. The execution history ends `FAILED, RESTARTED`, and one `advance` appends `RUNNING`.
  2. Flow-level `errors:` and `finally:` runs are emptied and listed in `emptied`. After one `advance`, the walk is in the Tasks phase, not Errors (this is the roadmap-note-7 regression).
  3. A local `errors:` under a `Sequential` is emptied. The `Sequential` is reopened with a new attempt whose `started_at == now`.
  4. `afterExecution:` runs are emptied.
  5. A `Parallel` with `a` FAILED and `b` failed by rule 1: both are reset, and the `Parallel` is reopened.
  6. A `WorkingDirectory` with a failed child: the block is CREATED, every descendant is empty, and `working_dir` is `None`.
  7. A `LoopUntil` whose iteration child failed: the `LoopUntil` is reopened with `attempt_started_at() == Some(now)` (D6).
  8. A `PAUSED` execution with a `Pause` run: the `Pause` is reopened, its history still contains `PAUSED`, and one `advance` ends it `SUCCESS` (Kestra `Pause.java:269-272`).
  9. A `Loop` run FAILED: it is reopened, listed in `loops`, and not CREATED.
  10. A `Subflow` run FAILED: it is CREATED and listed in `subflows`.
  11. Refusals: a `SUCCESS` execution gives `NotRestartable`; a `FAILED` execution with no FAILED/PAUSED run gives `NothingToRestart`.
  12. `mark_as` SUCCESS on a FAILED first task of `a(fails), b`: `a` is SUCCESS, its last attempt is SUCCESS, the attempt count is unchanged, the execution is RESTARTED, and one `advance` creates `b`.
  13. `mark_as` with an error branch: the error runs are emptied (Kestra `changeStateWithErrorBranch`).
  14. `mark_as` CANCELLED: the execution is CANCELLED.
  15. `mark_as` on a KILLED execution gives `NotRestartable`.
- [ ] Run: `cargo test -p egret-workflow --lib executor::restart`.
- [ ] Commit: `feat(workflow): pure restart and mark-as commands (package 7a)`.
- **Accept:** the 15 tests pass; `--lib` passes; the ratchet is unchanged (`--test executor`).

### A2. Restart in place: worker entry point and migration (spec D1, D5, D7, D10, D12)

**Files:**
- Create:
  - `migrations/0225_workflow_restart_attempt_number.sql`
  - `crates/egret-workflow/src/worker/restart.rs`
  - `crates/egret-workflow/tests/restart.rs`
  - `crates/egret-workflow/tests/migration_0225_restart_attempt.rs`
- Modify:
  - `crates/egret-workflow/src/worker/mod.rs`: re-exports.
  - `crates/egret-workflow/src/worker/claim.rs`: only if a helper must become `pub(crate)`.

**Interfaces:**
- Consumes: A1's `restart`, `RestartCommands`, `RestartRefusal`.
- Produces:
```rust
pub enum Restarted { Done, Refused(RestartRefused) }
pub enum RestartRefused { NotRestartable(String /*state*/), NothingToRestart,
                          AfterExecutionRunning, FilesPruned, LoopIteration { root: Uuid } }
/// Unscoped by workspace, like every worker entry point; the route scopes first.
pub async fn restart_execution(pool: &PgPool, id: Uuid) -> egret_core::Result<Restarted>;
```
- **Migration 0225.** Drop and re-add `workflow_executions_metadata_belongs_to_a_replay` (0139:141-146) as
  ```sql
  CHECK (attempt_number >= 1 AND (origin_offset_ms = 0 OR (attempt_number > 1 AND parent_execution_id IS NOT NULL AND parent_run_ix IS NULL)))
  ```
  Add a comment explaining that a restart raises `attempt_number` on any execution (spec D7) while `origin_offset_ms` stays the replay's alone.
- **Shape.** Copy `resume_by_hand` (`claim.rs:4469-4485`): `transaction!`, `load` (takes the lock), the checks under the lock, `before = state.clone()`, `apply` A1's commands, the side writes, then `settle`. Lock the run rows `FOR UPDATE` in `run_ix` order as `kill_loaded` does. Retry on deadlock as `kill_execution_reporting` does (`DEADLOCK_RETRIES`, `worker/kill.rs`).
- **Checks under the lock, in this order:**
  1. a loop iteration (`parent_execution_id` with `loop_index`) → `LoopIteration{root}`;
  2. `files_pruned_at IS NOT NULL` → `FilesPruned`;
  3. the execution is terminal but not idle (`workflow.execution_is_idle`, 0210, false) → `AfterExecutionRunning`;
  4. A1's refusals.

  A refusal writes nothing.
- **Side writes, in the same transaction, after `apply` and before `settle`:**
  - `UPDATE workflow.executions SET attempt_number = attempt_number + 1`;
  - add the label `system.restarted: "true"` if absent, through the same JSON shape `labels` already has (read `labels.rs` and the `labels` column writer first);
  - for each `reset` and `emptied` run: `outputs = NULL, message = NULL, secret_outputs = NULL, outputs_bytes = NULL`;
  - give each `emptied` run a new id the way `renew_restarted_ids` does (`claim.rs:5266-5279`). Reuse it; widen it only if its signature is tied to `RestartLoop`.
- **`loops` and `subflows`** are empty here. A2 refuses (`NothingToRestart`-like, a temporary internal error) when either is non-empty, and A3/A4 remove that refusal. State this in a code comment naming A3/A4.
- [ ] **Tests** (`tests/restart.rs`, `#[sqlx::test]`, helpers as in `tests/kill_route.rs` and `tests/end_hooks.rs`: `start`, `tick`, `finish`, `runs_of`, `exec_history`). Use a registry that fails a named task once and then succeeds, so a test can fail a run and then pass it after restart.
  1. `restartSimple`: same execution id; the same `task_runs.id` for the reset run; history `… FAILED RESTARTED RUNNING`; label present; `attempt_number` 2.
  2. `restartFailedThenSuccess`: runs 3 → 4 after ticking to the end; the failed run has 2 attempts; execution SUCCESS.
  3. Global `errors:` emptied and run again on a new failure; local `errors:`; `finally:`; `afterExecution:`. The emptied runs have new ids, and their old ids still own their old `task_logs` rows.
  4. `restartMultiple`: restart, fail, restart, succeed.
  5. A `Parallel` with two failed branches ends SUCCESS after restart.
  6. A `WorkingDirectory` re-runs in a new directory: its `working_dir` phase starts again (skip if Docker is not available, the same gate as `working_dir_claims.rs`).
  7. A kept container name is not reused: the new attempt's index is `attempts.len()`, greater than the old. Assert this on the claimed `TaskInput`'s attempt, not on Docker.
  8. Flow-trigger outbox rows: one each for RESTARTED, RUNNING and the new terminal state.
  9. Refusals write nothing: a SUCCESS execution, a KILLED one, one with `files_pruned_at` set, one with a live `afterExecution:` run, and a loop iteration. Assert the row's `history`, `attempt_number` and `labels` are unchanged.
  10. A retry: a run whose `retry: maxAttempts: 2` was spent fails again after restart without retrying (D5 consequence).
  11. `restartOrReplayLoopUntil` (Kestra `RestartCaseTest:463-505`): a `LoopUntil` whose child failed waits again from the restart; its `maxDuration` is measured from the restart's tick, not the first start (D6).
  12. D7 side effects: an `EXECUTION_ASSERTION` SLA is evaluated again while the restarted execution runs; flow `outputs:` are re-evaluated at the new terminal state (the old value stays visible until then).
- [ ] `tests/migration_0225_restart_attempt.rs`:
  - a top-level row may now hold `attempt_number = 3, origin_offset_ms = 0`;
  - `origin_offset_ms > 0` without the replay shape is still refused.
- [ ] Commit: `feat(workflow): restart an execution in place (package 7a)`.
- **Accept:** the new tests pass; `--test kill_route --test end_hooks --test worker` pass; the ratchet is unchanged.

### A3. `Loop`: restart the last failing iteration in place (spec D8)

**Files:** modify `crates/egret-workflow/src/worker/restart.rs` and `crates/egret-workflow/src/worker/spawn.rs` (only if the counters need it); extend `tests/restart.rs`.

- For each `loops` run: find the iteration rows `parent_execution_id = id AND parent_run_ix = run`, pick the highest `loop_index` whose state is `FAILED` or `PAUSED`, and restart it with the same inner function. Factor `restart_execution`'s body into `restart_locked(conn, id, as_iteration: bool)`, which skips the loop-iteration refusal for the inner call. Lock order is parent then iteration.
- If no iteration qualifies: append `SetRunState{Failed}` to the loop run and write the task log line `No restartable loop sub-execution found` on that run (find the existing writer for executor log lines; do not add a new one).
- **Counters and outputs list (#6).** First, red: a test where an iteration that ends a second time is double-counted in `terminatedIterations`, or appears twice in `outputs.loop.outputs`. Then fix `on_execution_terminal` (`spawn.rs:539-728`) so a re-terminated iteration replaces its index's entry and is counted once. Counters are derived per index, never incremented blindly.
- [ ] **Tests:**
  1. `restartLoop` (Kestra `RestartCaseTest:224-311`): three restarts, each continuing from the failing iteration. The `Loop` run id is the same each time; exactly N iteration rows exist (no re-spawn).
  2. The counters and outputs list after a restarted iteration succeeds.
  3. No iteration is restartable → the Loop is FAILED with the log line.
  4. Iterations already SUCCESS are not touched.
- [ ] Commit: `feat(workflow): restart a Loop from its failing iteration (package 7a)`.
- **Accept:** the tests pass; `--test restart --test worker` pass; the ratchet is unchanged.

### A4. `Subflow`: restart the failed child in place (spec D9, Q1)

**Files:** modify `crates/egret-workflow/src/worker/subflow.rs` (where the walk's `SpawnSubflow` is carried out) and `crates/egret-workflow/src/worker/restart.rs`; extend `tests/restart.rs`.

- When a `Subflow` run is opened in an execution labelled `system.restarted`, look up earlier children by `trigger_execution_id = parent AND trigger_run_ix = run` with the same `value` and iteration.
  - **Exactly one match:**
    - child `FAILED` or `PAUSED` → `restart_locked(child)`; link it as the run's live child, so the run waits for it as for a fresh spawn;
    - otherwise → the run ends `SUCCESS`.
  - **Zero or several matches** → spawn as today.
- Lock order is parent then child (`worker/kill.rs:82-104`); use kill's deadlock retry.
- `on_child_terminal` (`subflow.rs:661+`) must accept the restarted child's report, because the parent run is non-terminal again. Add a red test first.
- [ ] **Tests:**
  1. `restartSubflow`: the failed child is restarted, never replaced; exactly one child row; parent SUCCESS.
  2. `restartSubflowWithLoop` (Kestra `:359-395`): three restarts and exactly three children.
  3. A child that is SUCCESS while its parent FAILED elsewhere: the `Subflow` run ends SUCCESS, and no child is restarted or spawned.
  4. Two children match: a new child is spawned.
  5. A deadlock between a restart and the child's own settle is retried and both finish. Model it on `a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it`.
- [ ] Commit: `feat(workflow): restart a failed Subflow child in place (package 7a)`.
- **Accept:** the tests pass; `--test restart --test kill_route --test worker` pass; the ratchet is unchanged.

### A5. Task-run state and execution status (spec D11)

**Files:** modify `crates/egret-workflow/src/worker/restart.rs`; create `crates/egret-workflow/tests/mark_as.rs`.

**Interfaces produced:**
```rust
pub enum StateChangeRefused { NotChangeable(String), StateUnchanged, LoopIteration { root: Uuid },
                              NoSuchTaskRun, AfterExecutionRunning, FilesPruned }
pub async fn set_task_run_state(pool: &PgPool, id: Uuid, task_run_id: Uuid, target: RunState)
    -> egret_core::Result<Result<(), StateChangeRefused>>;
pub async fn set_execution_status(pool: &PgPool, id: Uuid, target: RunState)
    -> egret_core::Result<Result<(), StateChangeRefused>>;
```
- `set_task_run_state`:
  - resolve `task_run_id` to a `RunIx` in this execution only; an unknown id or one in another execution gives `NoSuchTaskRun`;
  - a target equal to the run's current state gives `StateUnchanged`;
  - then A1's `mark_as`, and the A2 side writes: attempt+1, the label, outputs cleared on `reset`/`emptied` runs (**not** on the marked run: Kestra keeps them), and ids renewed;
  - `loops`/`subflows` are handled as in A3/A4.
- `set_execution_status`:
  - terminal and not KILLED, else `NotChangeable`;
  - the same state gives `StateUnchanged`;
  - else `SetExecutionState{target}`, then `settle`. No run is touched, and there is no label or attempt change (Kestra `ES:1385-1392`).
- Both functions take the four targets only. The type is checked by the route (B1); assert it here too, with `debug_assert!` plus an `Err`.
- [ ] **Tests:**
  1. `ChangeStateTestCase:28-40`: FAILED first task marked SUCCESS → execution SUCCESS with 2 runs.
  2. `changeStateWithErrorBranch`: error runs emptied; `attempt_number` 2.
  3. Mark CANCELLED → execution CANCELLED, with nothing run.
  4. Status SUCCESS → WARNING: history +1; runs unchanged; one outbox row.
  5. The refusals write nothing: KILLED, RUNNING, the same state, a foreign task-run id, a loop iteration.
  6. A Subflow child's status changed while its parent's `Subflow` run is terminal: the parent is unchanged (spec D11; Kestra's FIXME).
- [ ] Commit: `feat(workflow): set a task run's state and an execution's status (package 7a)`.
- **Accept:** the tests pass; the ratchet is unchanged.

## B1. Routes, OpenAPI and audit (spec D13)

**Files:** modify `crates/egret-workflow/src/routes.rs` (handlers beside `kill_execution`, router lines beside `:4062-4070`, the module doc's route list at `:1-37`, the OpenAPI registration wherever `kill_execution` is registered); create `crates/egret-workflow/tests/restart_routes.rs`.

- **Routes:**
  - `POST /ws/{workspace}/workflows/executions/{id}/restart`, query `revision` (any value) → 400 with code `restart_revision_unsupported` and a message naming package 7b;
  - `POST /ws/{workspace}/workflows/executions/{id}/task-runs/{task_run_id}/state`, body `{"state": "SUCCESS"|"WARNING"|"FAILED"|"CANCELLED"}`;
  - `POST /ws/{workspace}/workflows/executions/{id}/status`, same body.

  Other states, or an unknown field, give 400 (`#[serde(deny_unknown_fields)]`).
- **Each handler:** `require(WF_MANAGE)`, then scoped `executions.get` (404 for a foreign id), then the cheap state check, then the worker call. The outcomes map to:
  - 409 `not_restartable` / `nothing_to_restart` / `after_execution_running` / `files_pruned` / `state_unchanged`;
  - 400 for a loop iteration, naming its root (as replay's message does);
  - 404 for `NoSuchTaskRun`.

  Then the audit event with `{from_state}` (plus `task_run_id` and `to_state` where they apply), and 200 with `ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)`.
- **Coded 409:** use the same `ErrorBody{error, code}` shape as `KillError`. Define one `ExecOpError` enum for the three routes rather than three copies.
- `utoipa::path` docs for every status, written like the kill route's (what the body is, what each 409 means).
- [ ] **Tests:**
  - the 200 paths;
  - 403 with only `workflow:read`;
  - 404 for a foreign workspace and for a foreign task run;
  - each coded 409;
  - 400 for `revision`, for `RUNNING` as a target, and for a loop iteration;
  - an audit record per action (`common::audit_mutations`);
  - the entitlement gate (`the_workflows_entitlement_gates_these_routes` covers new routes automatically; check it does).
- [ ] Run `python3 scripts/handler_surface.py --check` (ceiling 290) if it exists on this branch, and keep under the ceiling.
- [ ] Commit: `feat(workflow): restart, task-run state and status routes (package 7a)`.
- **Accept:** `--test restart_routes --test executions_routes --test routes` pass; `cargo clippy -p egret-workflow --all-targets -- -D warnings` is clean.

## C1. Frontend (spec D14)

**Files:**
- Modify:
  - `frontend/src/components/workflows/executions/actions.ts` (+ its test);
  - `frontend/src/routes/workflows/ExecutionDetail.tsx` (`ExecutionActions` `:358-524`, and the task-run rows);
  - `frontend/src/api/generated/*`, regenerated by the controller with `npm run gen:api`.
- Create: a `ChangeStateDialog.tsx` beside `actions.ts` if the dialog exceeds ~80 lines.

Read `docs/en/frontend-rules.md` (the frontend RULEBOOK) first; every rule applies.

- **Action predicates:**
  - `showsRestart`: state `FAILED` or `PAUSED`; `canRestart`: not a loop iteration;
  - `showsChangeStatus` and `showsChangeTaskRunState`: terminal and not `KILLED`.
- **Restart:** `ConfirmAction` → `mutateAsync` → `invalidateQueries(workflowsKeys.execution(id))` and `executions(ws)` → toast. Polling restarts because the state is live again (`polling.ts`).
- **Change status:** a dialog with a select of the four states minus the current one.
- **Change state:** a menu item per task-run row. It edits the run's last attempt; the dialog says so.
- **Coded 409s** map to sentences through `ApiError.code`, as the kill route's `kill_not_applied` is mapped.
- [ ] **Tests (vitest):**
  - the predicates for every state in `StateChip`'s table;
  - each mutation invalidates both keys;
  - a coded 409 shows its sentence;
  - the select never offers the current state.
- [ ] Run `npm run typecheck`, `npm run lint` and the touched vitest files. The controller provides `node_modules`; borrow, never install.
- [ ] Commit: `feat(workflows): restart, change status and change task-run state on the execution page (package 7a)`.
- **Accept:** typecheck, lint and vitest pass.

## Z. Docs, bookkeeping, gates and live check

- [ ] **Spec §7 "As built":** the measured numbers, the migration number, every deviation from this plan with its reason.
- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`:
  - a dated note under row #7 for the split and 7a done;
  - Appendix §5's table rows for restart, change-status and state marked supported, with route evidence;
  - note 7 marked closed.
- [ ] **Vietnamese** `docs/vi/specs/…` and `docs/vi/plans/…` copies (bilingual convention). A Sonnet subagent may translate.
- [ ] **`egret-data/docs`** (Platform → Workflows → executions): the three actions, the permission, the refusals, and fresh screenshots. The controller opens that PR.
- [ ] **Gates** (one at a time):
  - `cargo fmt --check`;
  - clippy `-D warnings --all-targets` on egret-workflow, egret-server, egret-config, egret-metastore;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow` (report passed, failed, ignored; only `sql_starrocks_live` may fail, for want of an engine);
  - `cargo test -p egret-config -p egret-server -p egret-metastore`;
  - the frontend typecheck, lint and vitest.
- [ ] **Live check** on an own compose project (never the shared demo stack's DB; memory "demo stack shared state"). In a browser, with `admin@egret.local`:
  1. Run a flow that fails once, press Restart, and watch it go SUCCESS on the same execution id.
  2. Change status SUCCESS → WARNING.
  3. Mark a FAILED task SUCCESS and watch the next task run.

  Take screenshots for the docs. Before the live check, grep the image binary for a new route string ("docker build cache ships a stale binary").
- **Accept:** every gate passes; the ratchet is 301 / 25 / 25 / 0; the live check passes with screenshots.
