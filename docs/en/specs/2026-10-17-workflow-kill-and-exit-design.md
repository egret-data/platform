# Workflow follow-ups, group B: kill an execution, and the `Exit` task

**Status:** built and verified 2026-10-18 on `feat/workflow-kill-exit` (plan Tasks 1-8; dated notes below record where the build differs from this design).
- **Oracle ratchet** (`cargo test -p egret-workflow --test executor`): before, 104 mismatching and 104 baselined; after, 100 mismatching and 100 baselined; 0 broken in both. The refused set went from 45 to 41. The four `valids/exit*` entries (`exit`, `exit-cancelled`, `exit-killed`, `exit-nested`) are banked and removed from `oracle-exec/baseline.txt`; no other entry moved. `golden.json` was not edited.
- **Live, Docker** (`tests/container_docker_live.rs`, each case run alone): the kill of a running `sleep 600` container took 2.00 s, with `KILLED` committed about 0.75 s after the kill was written. The kill whose worker dies took 3.05 s; the second worker finished the kill about 0.3 s after the reaper freed the lease.
- **Mutations** (in `worker/claim.rs::end_kill`): with the cancel removed, the live cases go RED (the `taskRunner.delete: false` case on the daemon, "still running"). With the release removed, the live cases still pass, because on Docker the cancel has already removed the container (§4.2 note); the release on the kill path is pinned by the non-daemon worker tests (`tests/worker.rs`'s `KillRecorder`, `claim.rs` unit tests).

**Designed** 2026-10-17. Every claim about egret's code was checked in the worktree `egret-kill` at `6e995c1f` (main, after #686) and is cited `path:line` from `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`; its paths are relative to it.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (W7's "kill/restart" left out of W7; W8's execution-level kill never shipped).

**Decided with the user, 2026-10-17:**
- **Kill goes through `KILLING`, then `KILLED`, as in Kestra.** `KILLED` is written only once the work has stopped. A worker that dies while killing is replaced by one that finishes the kill, so no container or pod is left running.
- **The design below (§3-§7) was approved as presented.**

**Default decisions (2026-10-17),** under the rule "follow Kestra unless egret has a reason; keep correctness tight":
- D1. `cascade` defaults to `true` and kills child executions (subflow and loop children), as Kestra's `isOnKillCascade` does.
- D2. A paused execution is killed without a worker: nothing is running, so its waiting runs go straight to `KILLED` (Kestra resumes it into `KILLING`, `core/…/services/ExecutionService.java:1007-1013`; the outcome is the same).
- D3. Killing a terminal execution is `409 execution_terminated`. Kestra answers 409 only when `isOnKillCascade` is false (`webserver/…/ExecutionController.java:1757`); egret has nothing to emit for a terminal execution, so it refuses in both cases.
  - *Note, 2026-10-28 (end hooks #2b, the user's answer to Q3; spec `2026-10-26-workflow-end-hooks-design.md` D9 as built).* A terminal execution can now have something to kill: its `afterExecution:` runs. A kill of a terminal execution with a live after-run is **accepted** (202): it stops those runs and never writes the execution's own state, history or `ended_at`; the audit `outcome` is the execution's own state plus `"after_execution": "KILLING"|"KILLED"`. Without a live after-run it is still `409 execution_terminated` (an `Exit` sibling still running does not count). A phase kill that cannot apply yet (a root after-task `WorkingDirectory` being prepared or finished) is a retryable `409 kill_not_applied`. A parent's cascade (D1) also stops a terminal child's live after-phase, where Kestra skips terminal descendants. This is a security divergence (roadmap §3.1): a tenant cannot write a task that no one can kill.
- D4. The kill route needs `WF_MANAGE`, the same permission as resume and replay (`routes.rs:2833`, `:2939`), and writes an audit record.
- D5. `Exit` with `SUCCESS`, `WARNING`, `FAILED` or `CANCELLED` leaves running sibling branches running, as Kestra documents (`core/…/plugin/core/execution/Exit.java`, *"only `KILLED` stops sibling tasks"*).
- D6. The UI gets a Kill button on the execution detail page (W7b), a confirmation dialog with the cascade option, and a `KILLING` badge. No per-task-run kill, no bulk kill.
- D7. Kill detection polls the run's state every `workflow.kill_poll_ms` (default 5000). It does not wait for the lease renewal, which runs every `working_timeout / 3` (`worker/claim.rs:1179-1181`), 20 minutes at the default `working_timeout` of 3600 s (`worker/mod.rs:153`).
- D8 (decided with the user, 2026-10-17). A killed WorkingDirectory block runs its finish claim in discard-only mode: its `_wd/{block}/` objects are deleted, no outputs or cache are written. A retention sweep for crashed blocks remains group D's.

## 1. Why kill was not shipped, and what changed

`routes.rs:2176-2236` records seven obstacles. Re-read against current main:

| # | Obstacle (routes.rs) | Status |
|---|---|---|
| 1 | No command produces `KILLED`; `advance()` takes no operator input | `advance()` reads the execution's state. A kill writes `KILLING` into that state; `advance()` turns it into `KILLED` commands (§5). No new argument. |
| 2 | A killer holds no lease; the worker's outcome fence still matches | The fence gains `AND state = 'RUNNING'` (§4.1), so a run moved to `KILLING` refuses the worker's write. |
| 3 | No channel from HTTP to the worker's `TaskHandler` | The worker polls the run's state (§4.2). |
| 4 | "Kill requested" is unrepresentable: no `Killing` in `executor::RunState` | Added (§2). |
| 5 | Clearing `claimed_at` re-opens the run | The kill does not clear it on a leased run; it changes `state` (§3.2). |
| 6 | The killer is a different process | The signal is in the database and polled (§4.2). |
| 7 | `egret.sql` cannot be cancelled | Dropping its future ends the statement on StarRocks 4.1.6 (measured, #686, `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §1.2) and on Trino through `query_max_run_time`. A kill drops it. |

## 2. States and data

- `executor::RunState` gains `Killing`, **non-terminal**, at `executor/state.rs:447` beside `Killed`. Its wire name is `KILLING`, which `models/state.rs` already has. W8's lesson applies: every predicate that lists a subset of `RunState::ALL` must be re-read, and the variant is added to the round-trip test of `RunState::ALL` against `slot()`.
- `workflow.task_runs.state` and `workflow.executions.state` are plain `text` with no value list; `workflow.state_is_terminal` (`migrations/0183_workflow_execution_timestamps.sql:52`) lists terminals only, and `KILLING` is not one. **No migration is expected.** The plan's first task confirms this against every constraint and index on both tables; if one needs `KILLING`, the migration takes the next free number on main at that time (numbers are a shared namespace).

## 3. The kill route

### 3.1 Contract

`POST /api/v1/workspaces/{ws}/workflows/executions/{id}/kill?cascade=true|false` (default `true`).

*Note, 2026-10-18:* the route as built is `POST /api/ws/{workspace}/workflows/executions/{id}/kill`, the prefix every workflow route has (`routes.rs`), as the plan's Global Constraints say.
- `202` with the execution DTO: it is now `KILLING`, or `KILLED` if nothing was running.
- `404` if the execution is not in the workspace; `409 execution_terminated` if it is terminal (D3).
  - *Note, 2026-10-28 (end hooks #2b):* unless its `afterExecution:` still runs: then `202` with the execution unchanged (e.g. `SUCCESS`), or `409 kill_not_applied` (retry) when the phase kill cannot apply yet. See D3's note.
- `WF_MANAGE` (D4). utoipa-documented, regenerated into the frontend client with `npm run gen:api`.

### 3.2 One transaction

Lock the execution row (`FOR UPDATE`), then, for the execution and, with `cascade`, every descendant execution, recursively, that is a loop child (`parent_execution_id = <parent>` AND `parent_run_ix IS NOT NULL`, `migrations/0135_workflow_loop_sub_executions.sql:21`) or a subflow child (`trigger_execution_id = <parent>` AND `trigger_run_ix IS NOT NULL`, migration 0138). A replay (`parent_run_ix` NULL) and an execution a Flow trigger fired (`trigger_run_ix` NULL) are not children:
1. A non-terminal leaf run that is **leased** (`claimed_at IS NOT NULL`, state `RUNNING`) → `KILLING`. `claimed_at` is left as it is.
2. A non-terminal leaf run that is **not leased** (waiting to be claimed, `RETRYING`, `PAUSED`, `CREATED`) → `KILLED`, with `ended_at`.
3. A non-terminal flowable run is left to `advance()` (§5).
4. The execution → `KILLING`. If step 1 changed nothing, `advance()` closes it on the next tick; the route also runs one `advance()` in the same transaction so a kill with nothing running returns `KILLED` at once.

Executions are locked the execution first, then its descendants by depth and then by id (`worker/kill.rs`, "Lock order"). Depth first is what keeps a kill of a parent and a concurrent kill of a child in one order. One deadlock this order cannot remove: a child that ends inside a worker's transaction locks child, then parent. Postgres aborts one side, and the kill re-runs its transaction up to 3 times on `40P01` (`deadlock_detected`).

*Note, 2026-10-18 (corrections):*
- *The subflow link.* This section first left the subflow column for the plan to name, and the plan's Global Constraints named `execution_trigger->>'type' = 'io.kestra.plugin.core.flow.Subflow'` AND `execution_trigger->'variables'->>'executionId' = <parent>`. That matches no row: `subflow::spawn_one` writes `trigger_execution_id` / `trigger_run_ix` and leaves `execution_trigger` NULL (that column is written only by the webhook and the scheduler). The predicate above is the one built (plan Task 6), and it is the link `subflow::trigger_of` reads on completion.
- *Lock order.* This section first said "parent first, then children in id order". Pure id order can deadlock: a kill of a parent could hold a low-id descendant of a child while waiting for that child. The order and the retry above are what was built.
- *Step 2* also covers every `Subflow` run, which is `KILLED` by the kill and does not wait for a worker; its child execution is killed by the cascade.

*Note, 2026-10-18 (final review):*
- *Step 1 reads no lease, and takes `RUNNING` or `SUBMITTED`.* Step 1 first read "leased (`claimed_at IS NOT NULL`, state `RUNNING`)", and step 2 "not leased (waiting to be claimed, …) → `KILLED`". As built, step 1 took `RUNNING` or `SUBMITTED`. Since the final review (I1) it reads no lease either: every running `Runnable` becomes `KILLING`, leased or not. A run whose worker died and was reaped is `RUNNING` with `claimed_at` NULL, but its container may still be running, because W5a re-attaches to it on the next claim. Writing that run `KILLED` asked nobody to stop it, and the orphan sweep never removes a `taskRunner.delete: false` container. Now the kill-only claim (§4.3) cancels it by run and attempt, then writes `KILLED`.
  - A kill of a running task therefore always answers `KILLING`. The execution is `KILLED` after the next worker tick.
  - Step 2 keeps `CREATED`, `RETRYING`, `PAUSED` and `Subflow` runs, which are `KILLED` at once.
  - This supersedes plan Task 6's "unleased `RUNNING` → `KILLED`".
- *A descendant's replay is a child (M2).* A subflow child retried with `CREATE_NEW_EXECUTION` ends `RETRIED` and continues in a replacement linked `parent_execution_id = <child>` with `parent_run_ix` NULL (`worker/replay.rs`). The cascade follows that link from depth 1 down, so the replacement is killed. The root's own replay is still not a child.
- *The worker's side of the deadlock (M1).* When Postgres aborts the worker's commit instead of the kill, the worker runs that commit once more (`claim::run_one`, and `claim::commit_kill` the same way). The retry waits on the rows the kill holds, so its fence then finds the run `KILLING`, and the worker takes the kill path (§4.2) instead of leaving the run to the reaper.

## 4. The worker stops the handler

### 4.1 Fences

- `renew_lease` (`worker/claim.rs:1845-1861`): `WHERE id = $1 AND claimed_at = $2` gains `AND state = 'RUNNING'`.
- `apply_outcome_with`'s fence (`worker/claim.rs:2031`): `WHERE id = $2 AND claimed_at = $3` gains `AND state = 'RUNNING'`. An outcome for a `KILLING` run is refused as `LeaseLost` and records nothing.

*Note, 2026-10-18 (final review, as built):* there is a third fence, and its state is a parameter. A `WorkingDirectory` block's claim commits through `claim::apply_block_claim_on`, fenced `WHERE id = $3 AND claimed_at = $4 AND state = $6`. `$6` is the state the claim expects its block in: `KILLING` for a discard-only finish (a block killed after it prepared, D8, plan Task 2), and `RUNNING` for every other block claim. `renew_lease` takes the same expected state (`expect`): `RUNNING` for a leaf, the block fence's state for a block's claim. So a discard-only finish, whose row is `KILLING` by design, keeps its lease, while a leaf's `KILLING` still refuses its renewal.

### 4.2 Detection and cancel

`under_lease_with` (`worker/claim.rs:1769`) gets a third wake-up beside the renewal and the deadline: every `kill_poll_ms`, `SELECT state, claimed_at FROM workflow.task_runs WHERE id = $1`. If the row is `KILLING` with this worker's `claimed_at`:
1. the handler future is dropped;
2. the handler's `cancel` is called (`cancel::kill`), the same call the timeout path makes (`worker/cancel.rs`, `worker/timeout.rs`). The kill passes no grace: each handler applies its own inside `cancel`, `workflow.plugin_host.kill_grace_s` for a plugin task and zero for a script task (`handlers/script.rs`);
3. the run is written `KILLED` (with `ended_at`, the attempt closed) `WHERE id = $1 AND claimed_at = $held AND state = 'KILLING'`, and `advance()` runs in the same transaction, as `apply_outcome` does.

A handler whose `cancel` returns `Err` (`handlers/sql.rs`) is still written `KILLED` after the drop; the error goes to `tracing`, as the timeout path does (`worker/cancel.rs`, fact 2).

Once `KILLED` has committed, the worker calls `Registry::release`, as after any committed outcome. A kill whose commit is `LeaseLost` releases nothing.

*Note, 2026-10-18 (corrections):*
- *No grace parameter.* This section first wrote `handler.cancel(task, grace)` with "the container grace for containers". The built `finish_kill(pool, claimed, held, reg, task)` takes no grace, and a script task's container is cancelled with grace zero, as W5a's cancel always was.
- *A killed container is removed, `taskRunner.delete: false` included.* On Docker, `DockerRunner::cancel` is `docker rm --force` (after `docker stop -t <grace>` when the grace is not zero, which is the plugin host's case). The Kubernetes runner's cancel deletes the pod. `delete: false` is read only by `release`, which runs after the cancel and finds nothing left. So a killed `delete: false` container is not kept for inspection. This is existing W5a behaviour, unchanged by the kill. Whether a killed `delete: false` container should be kept is an open product question.

*Note, 2026-10-19 (debt package, PR-1):* the user's ruling of 2026-10-18 closes the open question above: **a killed `taskRunner.delete: false` container is kept.** The kill removed it before; it does not any more. Four decisions of `docs/en/specs/2026-10-19-workflow-debt-design.md` change this section:
- **D1. On Docker, a kill or an expired budget stops the container and does not remove it** (`docker stop -t <grace>`, a grace of zero kills at once; `delete: true` keeps `rm --force`). The budget's expiry and the kill share one `cancel`, so both change together. The runner trait's `cancel` now takes the task's `RunnerSpec`, and the script handler passes the spec it parses, as `release` does. Tests: `a_kept_container_is_stopped_by_cancel_and_not_removed` (unit, with the docker shim), `cancel_passes_the_tasks_delete_to_the_runner` (handler) and `a_killed_delete_false_container_is_stopped_and_kept` (live, `container_docker_live`).
- **D2. Kubernetes keeps today's behaviour: a killed pod is deleted, `delete: false` or not.** A pod's container cannot be stopped without deleting the pod, and the kill was never measured on a cluster (§9). `KubernetesRunner::cancel` takes the spec and does not read `delete`; its doc says so. This is a recorded divergence from Docker.
- **D3. A kept container is removed after a deadline.** The setting is `workflow.kept_container_retention_days` (env `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS`), default 7, where `0` keeps containers for ever. The orphan sweeps of both runners remove a kept container once its run is terminal and the run's end (`COALESCE(ended_at, updated_at)`) is older than the deadline; before, they skipped it for ever. It applies to every kept container whose run is terminal, not only killed ones. Tests: `sweep_decision_removes_kept_work_only_after_the_deadline`, `the_docker_sweep_parses_names_and_the_keep_flag`, `the_kept_container_retention_defaults_to_seven_days`, `the_kept_container_retention_maps_zero_to_for_ever`, `workflow_kept_container_retention_default_and_env` (config) and `a_kept_container_is_swept_after_its_deadline` (live).
- **D4. A kill that cannot load an execution closes it by force, as `KILLED`,** instead of failing and being claimed again for ever (§4.3). The rules are the debt spec's §2.2 (`worker/force_close.rs`). The route's audit detail gains `"forced": true` when the force-close ran. Tests (`kill_route`): `the_route_force_closes_an_execution_it_cannot_load`, `a_kill_only_claim_of_an_unloadable_execution_closes_it`, `a_force_close_leaves_a_held_leaf_to_its_worker`, `a_cascade_force_closes_an_unloadable_child_and_kills_the_parent`, `a_force_closed_child_reports_to_its_parent`; and, from the fix round, `a_cascade_force_closes_a_loop_whose_parent_and_iteration_cannot_load`, `a_held_iteration_of_an_unloadable_loop_is_closed_by_its_kill_only_claim`, `an_unloadable_iterations_report_to_an_unloadable_live_parent_is_dropped`, `a_report_to_a_terminal_parent_does_not_lock_it`, `a_report_that_cannot_lock_the_parent_still_fails_the_kill`, `a_force_close_closes_a_leased_block_claim_at_once`; in `working_dir_claims`, `a_discard_only_block_claim_of_an_unloadable_execution_closes_it`.

The bullet above ("A killed container is removed, `taskRunner.delete: false` included") is left as written: it was true of the code on 2026-10-18.

A renewal that fails because the state is no longer `RUNNING` is not a lost lease: the worker reads the state and takes the kill path above instead of `Leased::Lost` (which drops the handler with no cancel on purpose, `worker/claim.rs:1715-1721`).

### 4.3 A worker that dies while killing

- The reaper (`worker/reaper.rs:42-44`) clears an expired lease on a `KILLING` run exactly as on a `RUNNING` one.
- `claim` (`worker/claim.rs:477-485`) also takes `state = 'KILLING' AND claimed_at IS NULL`. A claimed `KILLING` run does not run its handler: the worker builds the handler, calls `cancel` (a container is named by run and attempt since W5a, so the new holder reaches the old container), then writes `KILLED` as in §4.2 step 3.

## 5. `advance()` while the execution is `KILLING`

`advance(plan, state, now)` (`executor/mod.rs:175`) stays pure. When the execution's last state is `Killing`:
- it creates no run and starts no retry;
- a flowable run that is not terminal becomes `KILLING`, then `KILLED` once every child run is terminal (Kestra `executor/…/ExecutorService.java:313-335`);
- the execution becomes `KILLED` once every run is terminal.

*Note, 2026-10-18 (final review C1):* this section first left every leaf to the route, and `advance_killing` wrote none. That left a gap: a leaf nobody killed stayed live for ever. An `Exit(KILLED)`'s round could create or start one (§6 note), and a `CREATED` leaf under a `KILLING` execution is never claimed. So `advance_killing` now also writes any live leaf that is not `KILLING`, the way §3.2 does: a running `Runnable` (`RUNNING`, `SUBMITTED`) becomes `KILLING` for its holder or the kill-only claim to cancel, and every other live leaf becomes `KILLED`. `WorkingDirectory` blocks and other flowables keep the rules above (`executor/kill.rs`).

## 6. The `Exit` task

`io.kestra.plugin.core.execution.Exit`, property `state` (default `SUCCESS`; one of `SUCCESS`, `WARNING`, `KILLED`, `FAILED`, `CANCELLED`). In Kestra it is an `ExecutionUpdatableTask` run by the executor (`core/…/plugin/core/execution/Exit.java`). In egret it is executor-level too: registered in `spec::registry` with no worker handler, and `advance()` handles its run when it is created:
- `KILLED`: the execution becomes `KILLING` and §5 takes over.
- any other state: the `Exit` run, each of its ancestor runs, and the execution are set to that state (Kestra's `update`: the last not-terminated run, then its parents, then the execution). Running sibling branches keep running (D5).
- `state` is a property that may be templated; it is rendered with the run's render context. A value outside the five is a run failure naming the value.

The refusal of `Exit` (`executor/mod.rs:4235-4250`, the four `valids/exit*` entries) is removed.

*Note, 2026-10-18 (as built, measured against the golden):* `KILLED` does not go through `KILLING` when nothing else is live. `oracle-exec/golden.json`'s `valids/exit-killed` reads `CREATED RUNNING KILLED` for the execution and for the `If`, with no `KILLING`, and the `Exit` run is `KILLED` too. Histories are compared exactly, so the first bullet above would not bank. The built shape is Kestra's own (`ExecutorService.handleExecutionUpdatingTasks`, then `ExecutionService.killParentTaskruns`; `executor/exit.rs`):
- With nothing else live, the `Exit` run, its ancestors and the execution are set `KILLED` at once, with no `KILLING` step.
- With a live sibling, the `Exit` run and its ancestors are `KILLED`, the execution goes `KILLING`, and the `Exit` does the kill route's leaf steps itself, because no route ran: a running `Runnable` goes `KILLING`, every other live leaf goes `KILLED`. Flowables are left to §5.
- `Exit(KILLED)` also kills the execution's child executions (subflow and loop children), as Kestra's `isOnKillCascade(true)` does. `claim::exit_kills` sees the round that set an `Exit` run `KILLED` and runs the kill route's cascade (`worker::kill::cascade`, plan Task 6).
- An `Exit` inside a `WorkingDirectory` is refused at validate with `working_directory_child_not_runnable`.

*Note, 2026-10-18 (final review C1):* an `Exit` that decides `KILLED` is the whole round. Its leaf steps are computed from the state at the start of the round, but a `Parallel` collects every child's commands in one batch. The same round could therefore create a sibling branch's next run, which then stayed `CREATED` under `KILLING` for ever. It could also start a `CREATED` sibling after the kill's `KILLED`, which left a `RUNNING` run under a killed execution. `advance()` now returns only the `Exit`'s commands for that round (`exit::killed_in`), as Kestra runs updating tasks before flowables. The next round, under `KILLING`, belongs to §5. The four `valids/exit*` entries stay banked, and the ratchet is unchanged (100 mismatching / 100 baselined).

## 7. Frontend (W7b's execution page)

- A Kill button on the execution detail page, shown while the execution is not terminal, behind a confirmation dialog with a "also kill subflow executions" checkbox (on by default). It follows `docs/en/frontend-rules.md`.
- `KILLING` gets a badge and a status colour like the other states; the list and the detail refresh as they do today.

## 8. Tests and acceptance

- **Oracle:** `valids/exit`, `valids/exit-cancelled`, `valids/exit-killed`, `valids/exit-nested` are baselined today. They must bank against `oracle-exec/golden.json`; the ratchet's before and after counts are recorded. `golden.json` is never edited to match egret.
- **Unit (pure):** a table test of `advance()` for an execution in `Killing`: leaf runs `Killing` / terminal, a flowable parent with mixed children, nested flowables, a `Pause`d run. A table test of `Exit` for each of the five states, nested in `If` and in `Parallel`.
- **DB tests** (private database): the route's transaction (§3.2 steps 1-4, cascade to a subflow child and a loop child, 409, 404, permission); both fences refusing a `KILLING` run; the reaper and `claim` taking a `KILLING` run; the worker writing `KILLED` only under its lease.
- **Live, Docker** (engine-gated like the existing `container_docker_live`): a container task running `sleep 600` is killed; the run is `KILLED` within `kill_poll_ms + grace + margin`, and no container named for that run and attempt remains. A second live test kills while the worker is stopped, then starts a worker, and checks the same.
- **Frontend:** vitest for the button, the dialog and the badge; the live check in the browser on a private stack.

## 9. Scope

In: §2-§7. Out: kill of one task run, bulk kill, restart, Kubernetes live measurement of the kill (the k8s runner gets the same `cancel` call; the live test runs on Docker), group C's other Kestra tasks.
