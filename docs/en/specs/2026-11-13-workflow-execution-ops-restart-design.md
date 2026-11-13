# Workflow package #7a "Restart, task-run state and execution status"

**Status:** designed 2026-11-13. Branch `feat/workflow-execution-ops`, one PR. Roadmap row #7 and note 7 of `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Row #7 is size L and is split into six sub-packages (user decision, 2026-11-13); this is the first. Plan: `docs/en/plans/2026-11-13-workflow-execution-ops-restart.md`. Kestra is the checkout at `data-portal/kestra`, `96893b58` (2.0.0-SNAPSHOT).

Abbreviations: `EW` = `crates/egret-workflow/`; `ES` = `kestra/core/src/main/java/io/kestra/core/services/ExecutionService.java`; `EC` = `kestra/webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`; `ExS` = `kestra/executor/src/main/java/io/kestra/executor/ExecutorService.java`; `KS` = `kestra/core/src/main/java/io/kestra/core/models/flows/State.java`.

## 1. Problem

egret can start, kill, resume and replay an execution, and nothing else (`EW src/routes.rs:3112-3527`). Three Kestra operations that act on a finished execution are missing:

1. **Restart** (`POST /executions/{id}/actions/restart`, `ES:281-373`). It re-runs a `FAILED` (or `PAUSED`) execution **in place**: same execution id, same task-run ids, the failed task runs and their ancestors reset, the end-hook runs removed, and the executor carries on from there.
2. **Change a task run's state** (`POST /executions/{id}/actions/state`, `ES:521-559`, `ES:603-662`). It overrides one task run's state ("mark as SUCCESS") and then restarts the execution so that what follows the task runs.
3. **Change the execution's status** (`POST /executions/{id}/actions/change-status`, `ES:1385-1392`). It rewrites only the terminal state of a finished execution.

The rest of row #7 goes to sub-packages 7b–7f (§8).

## 2. Decisions

### 2.1 Package split (user, 2026-11-13)

| Sub | Scope |
|---|---|
| **7a** | This spec. |
| 7b | Replay from a chosen task, with new inputs and a revision choice. **Restart with `revision`** goes here too (Q2 below). |
| 7c | Pause by hand; resume with `onResume` inputs; resume/validate. |
| 7d | Delete an execution (logs, storage); labels API; logs download and delete. |
| 7e | SSE follow (execution, logs); expression eval endpoint; export, distinct, average-duration, latest. |
| 7f | Every `by-ids` / `by-query` bulk operation. |

### 2.2 User decisions, 2026-11-13 (asked, answered)

- **Q1. A failed `Subflow` child is restarted in place**, as Kestra's default `restartBehavior: RETRY_FAILED` does (`kestra/core/.../runners/ExecutableUtils.java:91-170`). A child that is not `FAILED` is not restarted, and the parent's `Subflow` run ends `SUCCESS`. The `NEW_EXECUTION` choice and the `restartBehavior` property itself stay in row #12 (D9).
- **Q2. Restart with `revision` is 7b's.** In Kestra it is not in place at all: it creates a new execution on another flow revision (`ES:289`, `ES:367`), which is a replay. 7a refuses the `revision` parameter, naming 7b.

### 2.3 Design decisions (default decisions, 2026-11-13)

Taken under the autonomy rules (follow Kestra unless there is a reason; keep safety and correctness tight).

- **D1. Reset in memory, never delete rows.** The restart is a pure executor function, `executor::restart(plan, &state) -> Vec<Command>`, applied and then settled the way `resume_by_hand` is (`EW src/worker/claim.rs:4469-4485`). A run that Kestra *removes* is **emptied** in memory, as `Command::RestartLoop` already does for a `LoopUntil` iteration (`EW src/executor/state.rs:998-1041`), and the walk rebuilds it with `RecreateRun` when its turn comes. Deleting rows is not an option: `load_with` refuses a gap in `run_ix` (`claim.rs:4771-4782`), `persist` never deletes (`claim.rs:5404-5577`), and `task_logs` / `execution_files` key off the task-run id with no FK (migrations 0159, 0166).
- **D2. Which runs are reset.** Every run whose own state is `FAILED` or `PAUSED`, plus every ancestor of one (`ES:311-314`, `ES:375-391`). `KILLED`, `WARNING`, `CANCELLED`, `RETRIED`, `SKIPPED` and `SUCCESS` runs are kept unless they are an ancestor. If nothing qualifies, the restart is refused (409, "no task to restart from", Kestra's `"No task found to restart execution from!"`). **Every** failed run is reset, not just one: a `Parallel` whose rule 1 failed the live siblings (`EW src/executor/flowable/parallel.rs:192-220`) would otherwise fail them all again at once.
- **D3. New state of a reset run.** Kestra puts `RESTARTED` on a reset leaf and on `WorkingDirectory`/`LoopUntil`/`Loop`, and `RUNNING` on other flowable ancestors (`ES:1227-1256`). egret never puts `RESTARTED` on a run, because `sequential::walk` treats a run it does not know as `Waiting` and would stall (survey §1). Kestra treats `RESTARTED` as "created" (`KS:300-302`), so egret maps it:

  | Run | Kestra | egret |
  |---|---|---|
  | Runnable leaf, `Subflow` | `RESTARTED` | `CREATED`. The walk's `open` adds `SUBMITTED` and a new attempt, as `retry::reopen_due` does (`EW src/executor/retry.rs:511-534`). |
  | Flowable ancestor (`Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `AllowFailure`, …) | `RUNNING` | `RUNNING` |
  | `Pause` (a flowable in Kestra) | `RUNNING` | `RUNNING`. Its history already holds `PAUSED`, so it proceeds as if resumed (Kestra `Pause.java:269-272`). |
  | `WorkingDirectory` | `RESTARTED`, all descendants removed (`ES:335-337`, `ES:1179-1200`) | `CREATED`, all descendants emptied, `working_dir` cleared, so the block re-runs whole in one fresh directory. |
  | `LoopUntil` | `RESTARTED`, children kept; its `maxDuration` clock restarts (`LoopUntil.java:148-153`) | `RUNNING`, children reset by D2. Its clock restarts (D6). |
  | `Loop` | `RESTARTED`; the executor then restarts the last failing iteration (`ExS:735-762`) | `RUNNING`, never `CREATED` (that would re-spawn iteration 0 and collide in `spawn::open_batch`, `EW src/worker/spawn.rs:136-266`). The last failing iteration is restarted in place (D8). |

- **D4. End-hook runs are emptied (roadmap note 7).** Kestra removes every run of the flow's and every flowable's `errors:` and `finally:` (with their children), and every `afterExecution:` run (`ES:339-351`). egret empties the same set and gives each emptied run a new task-run id (`renew_restarted_ids`, `claim.rs:5266-5279`, debt D12), so that its old logs, files and kept containers stay with the old id. Without this, `run_of` (which takes a task's first run, `EW src/executor/flowable/mod.rs:359-366`) makes `current_list` resume the old `errors`/`finally` phase, and `after_execution::terminated` (`EW src/executor/after_execution.rs:236-250`) reads the after-phase as done.
- **D5. Attempts are kept and appended to.** Kestra copies `attempts` unchanged at restart, and the worker appends a new attempt when the task runs again (`TaskRun.java:193-208`; `RestartCaseTest:64`: a restarted run that succeeds has 2 attempts). egret does the same. Three things depend on it: the container or pod name `egret-run-{run_id}-{attempt}` (`EW src/worker/commands.rs:808-810`), the log line's attempt index (`task_logs.attempt`, migration 0159), and the attempt-scoped storage of files. With the attempts kept, the new attempt gets a new name and index and never re-attaches to a kept `delete: false` container of the earlier one.
  - Consequence, as in Kestra: a run's `retry:` counts all its attempts and measures `maxDuration` from its first attempt (`EW src/executor/retry.rs:253-293`), so a restarted run whose retries were spent does not retry again.
- **D6. A restarted flowable's clock restarts.** `Pause` and `LoopUntil` measure timeouts from `attempt_started_at` (`EW src/executor/flowable/pause.rs:132`, `flowable/loop_until.rs:360`). A flowable set back to `RUNNING` gets a new attempt opened at the restart's tick, so the clock restarts as Kestra's `RESTARTED` history entry makes it.
- **D7. Execution-level effects.**
  - **History:** `RESTARTED` is appended, then the existing `Created | Restarted` arm (`EW src/executor/mod.rs:331-346`) appends `RUNNING` in the same settle. The history stays append-only (the outbox asserts it, `claim.rs:3900-3905`).
  - **`attempt_number` goes up by one** (`ES:358`, `ExecutionMetadata.nextAttempt()`). Migration 0139's CHECK `workflow_executions_metadata_belongs_to_a_replay` allows this only on a replay row, so the package's migration (D12) relaxes it. `origin_offset_ms` is unchanged, so a `CREATE_NEW_EXECUTION` retry still measures `maxDuration` from creation.
  - **Label `system.restarted: "true"`** is added if absent (`ES:360-364`). Only the engine may write `system.*` labels (`EW src/labels.rs:52`). The `Subflow` restart (D9) keys off it, as Kestra's does.
  - **Flow triggers fire again.** The outbox writes one event per appended state (`claim.rs:3869-3933`), so `RESTARTED`, `RUNNING` and the new terminal state each reach a listener. Kestra does the same.
  - **SLA** `EXECUTION_ASSERTION` is evaluated again while the execution is live (`claim.rs:3830-3835`). Kestra re-creates the monitors (`ExecutionEventMessageHandler:160`).
  - **Flow outputs** keep their old value until the new terminal state, which re-evaluates them (`became_terminal`, `claim.rs:3713-3722`).
- **D8. `Loop`: restart the last failing iteration in place.** As Kestra (`ES:985-991`, `ExS:735-762`): the iteration with the highest `loop_index` whose state is `FAILED` or `PAUSED` is restarted by the same function, in place, under its own lock. The `Loop` run goes back to `RUNNING` and does not spawn. If no iteration qualifies, the `Loop` run ends `FAILED` with the log line "No restartable loop sub-execution found", as Kestra does. Because `spawn::on_execution_terminal` returns at once while the loop run is terminal (`spawn.rs:634-640`), the loop run must be `RUNNING` before the iteration can end again. The plan pins how the loop's counters and its #6 outputs list treat an iteration that ends a second time (one entry per index, not two).
- **D9. `Subflow`: restart the failed child in place (Q1).** When the walk opens a reset `Subflow` run of an execution labelled `system.restarted`, it looks for the earlier child, linked by `trigger_execution_id`/`trigger_run_ix` (`EW src/worker/subflow.rs`) with the same `value` and iteration; only exactly one match counts.
  - The child is `FAILED` or `PAUSED`: restart it in place with the same function, and the `Subflow` run waits for it as for a fresh child.
  - The child is in any other terminal state: the `Subflow` run ends `SUCCESS`. Kestra does this, with the comment "don't restart it as it's terminated successfully" (`ExS:1311-1318`).
  - No match or several: spawn a new child, as today.
  - Lock order is parent then child, as kill's cascade takes it (`EW src/worker/kill.rs:82-104`). The plan reuses kill's deadlock retry.
- **D10. Refusals beyond Kestra.**
  - An execution whose `afterExecution:` still runs is refused (409). Kestra's restart would remove those live runs; egret's kill cannot even reach a block that is being prepared (`KILL_NOT_APPLIED_CODE`, `routes.rs:3382`), so removing a live run is not safe.
  - An execution whose files were pruned (`files_pruned_at` set, migration 0168) is refused (409): its tasks would read `Unreadable::Pruned` outputs.
  - A loop iteration is restarted through its loop root, never on its own (as replay refuses one, `routes.rs:3253`). Kestra exposes no such call either; its loop restart goes through the parent.
- **D11. Task-run state and execution status.**
  - **Target states.** Both routes take `SUCCESS`, `WARNING`, `FAILED` or `CANCELLED`, the four Kestra's UI offers (`ui/src/components/executions/ChangeStatus.vue:101-121`, `ChangeExecutionStatus.vue:96-97`). Kestra's server takes any state for a task run and any terminal state for an execution. egret keeps the narrower door; a state outside the four is 400.
  - **Source state.** Both require an execution that has ended and is not `KILLED` (`KS:189-191`). A loop iteration is changed through its root.
  - **No-op.** A target equal to the current state is 409. Kestra logs a warning and returns unchanged (`KS:73-77`); egret's resume already answers 409 to a call that changes nothing (`routes.rs:3125`).
  - **Task-run state** follows `markAs` (`ES:603-662`):
    1. The run gets the target state and its last attempt's state is overwritten, with no new attempt.
    2. Its ancestors go `RUNNING` and the end-hook runs are emptied (D4).
    3. The execution gets the `system.restarted` label and `attempt_number + 1`.
    4. The execution then goes `RESTARTED` and the executor carries on.
    5. A `CANCELLED` target makes the execution `CANCELLED` at once.
  - **Execution status** appends one state to the history and touches no run. A parent whose `Subflow` run is already terminal ignores the child's new state (`subflow.rs:731-737`), as Kestra's does in practice (`ChangeStateTestCase:43-68`, whose FIXME records it).
- **D12. One migration**, provisionally **0220** (main is at 0219 and no open PR adds one; re-check before push, per the shared-namespace rule). It relaxes the 0139 CHECK so that `attempt_number > 1` is allowed on any execution, keeping `origin_offset_ms <> 0` for the replay shape only.
- **D13. Routes, permission and audit.** These are egret-style paths, as kill and resume are, not Kestra's `/actions/` paths:
  - `POST /api/ws/{ws}/workflows/executions/{id}/restart`. The `revision` query parameter is refused with 400, naming 7b.
  - `POST /api/ws/{ws}/workflows/executions/{id}/task-runs/{task_run_id}/state`, body `{ "state": … }`.
  - `POST /api/ws/{ws}/workflows/executions/{id}/status`, body `{ "state": … }`.

  All three:
  - require `workflow:manage`;
  - read the execution scoped first, so a foreign id is 404;
  - re-check the state under the execution's lock;
  - answer **200** with the `ExecutionDto`;
  - emit audit events `workflow_execution:restart`, `:set_task_run_state` and `:set_status`, each with `from_state`.

  Kestra answers asynchronously, through a command queue (`EC:3016-3059`). egret answers synchronously, as its resume and kill do.
- **D14. Frontend.** On the execution page (`frontend/src/routes/workflows/ExecutionDetail.tsx:358-524`, `frontend/src/components/workflows/executions/actions.ts`):
  - a **Restart** button, shown for `FAILED` and `PAUSED`;
  - a **Change status** dialog, shown for a terminal non-`KILLED` execution;
  - a **Change state** item on each task run row of a terminal non-`KILLED` execution, which edits the last attempt only, as Kestra's UI does.

  Each action uses a `ConfirmAction` with the reasons for a disabled button, invalidates the execution query so polling resumes, and maps the coded 409s. The client is regenerated with `npm run gen:api`, never bare orval.

## 3. Shape of the change

| Layer | Change |
|---|---|
| `executor` (pure) | New `restart.rs`:<br>• `restart(plan, &state) -> Result<Vec<Command>, NothingToRestart>`<br>• `mark_as(plan, &state, run, target) -> Vec<Command>`<br>• a shared "reset set" (D2), "end-hook set" (D4) and state mapping (D3)<br>• a new `Command::EmptyRun(RunIx)` (or a generalised `RestartLoop`) and `Command::OpenFlowableAttempt` (D6)<br>• unit tests over hand-built states, no DB |
| `worker` | `restart_execution`, `set_task_run_state`, `set_execution_status`:<br>• `transaction!`, then `load` (lock), re-check, apply, `settle`, as `resume_by_hand` does<br>• Loop (D8) and Subflow (D9) in place<br>• `outputs`/`message`/`secret_outputs` cleared on reset and emptied runs<br>• `attempt_number`, label |
| `routes` | Three handlers (D13), OpenAPI, audit; coded 409s (`nothing_to_restart`, `after_execution_running`, `files_pruned`, `not_restartable`, `state_unchanged`) |
| migration | 0220 (D12) |
| frontend | D14; `npm run gen:api` |
| docs | `egret-data/docs` Platform → Workflows → executions page; screenshots of the three actions |

## 4. Tests

The executor oracle drives a corpus of flows, not operations, so the ratchet is **unchanged** (301 / 25 / 25 / 0) and this package banks nothing (roadmap: "0 (operability)"). Each Kestra test scenario becomes an egret test:

| Kestra test | egret test (`EW tests/`) |
|---|---|
| `ExecutionServiceTest.restartSimple` (:73-91) | same ids, `RESTARTED` in history, failed run reset, label set |
| `RestartCaseTest.restartFailedThenSuccess` (:38-71) | runs 3 → 4, failed run ends with 2 attempts, all `SUCCESS` |
| `…WithGlobalErrors` / `…WithLocalErrors` (:73-137) | errors runs emptied, run again on the new failure |
| `restartFailedWithFinally` / `…WithAfterExecution` (:397-461) | finally / afterExecution emptied and run again |
| `restartLoop` (:224-311) | three restarts, each continues from the failing iteration; same Loop run id |
| `restartSubflow` / `restartSubflowWithLoop` (:339-395) | the failed child is restarted, never replaced; exactly N children |
| `restartMultiple` (:313-337) | repeated restarts of one execution |
| `restartOrReplayLoopUntil` (:463-505) | LoopUntil waits again from the restart (D6) |
| `changeStateWithErrorBranch` (ES test :544-560) | mark SUCCESS empties the error branch; `attempt_number` 2 |
| `ChangeStateTestCase` (:28-40) | FAILED first task marked SUCCESS → execution SUCCESS, 2 runs |
| `ExecutionControllerRunnerTest` (:665-684, :1750-1915, :3303-3342) | route status codes: 409 on SUCCESS/KILLED restart, 409 on KILLED change, 200 paths |

egret-only tests:
- `Parallel` with siblings failed by rule 1;
- `WorkingDirectory` re-run in a fresh directory;
- a `delete: false` container is not re-attached (D5);
- the 409 refusals of D10 and D11;
- the 0220 CHECK both ways;
- flow-trigger events per appended state;
- a foreign-workspace id is 404;
- the frontend button logic (vitest);
- a live browser check on the demo stack.

## 5. Follow-ups recorded

- Restart with `revision`, which is a new execution on another revision: 7b.
- `restartBehavior: NEW_EXECUTION` and the property itself: row #12.
- Bulk restart and change-status: 7f.
- A `Pause` reached by a task-run state change (Kestra's resume path through `markAs`): 7c owns resume with inputs.

## 6. Recorded divergences

1. The target states are limited to the four Kestra's UI offers (D11).
2. A no-op state change is 409, not a silent 200 (D11).
3. Restart is refused while `afterExecution:` runs, and after the pruner deleted the files (D10).
4. A reset run's `outputs` are cleared at restart. Kestra keeps them, keyed by the task-run id, until the run writes again.
5. The operations answer synchronously; Kestra uses a command queue (D13).
6. Routes use egret's paths, not `/actions/…` (D13).

## 7. As built

(Filled in when the PR lands.)

## 8. Rest of row #7

See §2.1. Each sub-package gets its own spec, plan and PR.
