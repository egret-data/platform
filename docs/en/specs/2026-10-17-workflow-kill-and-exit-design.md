# Workflow follow-ups, group B: kill an execution, and the `Exit` task

**Status:** designed 2026-10-17, not built. Every claim about egret's code was checked in the worktree `egret-kill` at `6e995c1f` (main, after #686) and is cited `path:line` from `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`; its paths are relative to it.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (W7's "kill/restart" left out of W7; W8's execution-level kill never shipped).

**Decided with the user, 2026-10-17:**
- **Kill goes through `KILLING`, then `KILLED`, as in Kestra.** `KILLED` is written only once the work has stopped. A worker that dies while killing is replaced by one that finishes the kill, so no container or pod is left running.
- **The design below (§3-§7) was approved as presented.**

**Default decisions (2026-10-17),** under the rule "follow Kestra unless egret has a reason; keep correctness tight":
- D1. `cascade` defaults to `true` and kills child executions (subflow and loop children), as Kestra's `isOnKillCascade` does.
- D2. A paused execution is killed without a worker: nothing is running, so its waiting runs go straight to `KILLED` (Kestra resumes it into `KILLING`, `core/…/services/ExecutionService.java:1007-1013`; the outcome is the same).
- D3. Killing a terminal execution is `409 execution_terminated`. Kestra answers 409 only when `isOnKillCascade` is false (`webserver/…/ExecutionController.java:1757`); egret has nothing to emit for a terminal execution, so it refuses in both cases.
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
- `202` with the execution DTO: it is now `KILLING`, or `KILLED` if nothing was running.
- `404` if the execution is not in the workspace; `409 execution_terminated` if it is terminal (D3).
- `WF_MANAGE` (D4). utoipa-documented, regenerated into the frontend client with `npm run gen:api`.

### 3.2 One transaction

Lock the execution row (`FOR UPDATE`), then, for the execution and, with `cascade`, every descendant execution reached through `workflow.executions.parent_execution_id` (`migrations/0135_workflow_loop_sub_executions.sql:21`) and the subflow parent link (the plan names its column after reading `worker/subflow.rs`):
1. A non-terminal leaf run that is **leased** (`claimed_at IS NOT NULL`, state `RUNNING`) → `KILLING`. `claimed_at` is left as it is.
2. A non-terminal leaf run that is **not leased** (waiting to be claimed, `RETRYING`, `PAUSED`, `CREATED`) → `KILLED`, with `ended_at`.
3. A non-terminal flowable run is left to `advance()` (§5).
4. The execution → `KILLING`. If step 1 changed nothing, `advance()` closes it on the next tick; the route also runs one `advance()` in the same transaction so a kill with nothing running returns `KILLED` at once.

Executions are locked parent first, then children in id order, to avoid deadlocking against a concurrent kill of a child.

## 4. The worker stops the handler

### 4.1 Fences

- `renew_lease` (`worker/claim.rs:1845-1861`): `WHERE id = $1 AND claimed_at = $2` gains `AND state = 'RUNNING'`.
- `apply_outcome_with`'s fence (`worker/claim.rs:2031`): `WHERE id = $2 AND claimed_at = $3` gains `AND state = 'RUNNING'`. An outcome for a `KILLING` run is refused as `LeaseLost` and records nothing.

### 4.2 Detection and cancel

`under_lease_with` (`worker/claim.rs:1769`) gets a third wake-up beside the renewal and the deadline: every `kill_poll_ms`, `SELECT state, claimed_at FROM workflow.task_runs WHERE id = $1`. If the row is `KILLING` with this worker's `claimed_at`:
1. the handler future is dropped;
2. `handler.cancel(task, grace)` is called, the same call the timeout path makes (`worker/cancel.rs`, `worker/timeout.rs`), with the configured grace (`workflow.plugin_host.kill_grace_s` for plugin tasks, the container grace for containers);
3. the run is written `KILLED` (with `ended_at`, the attempt closed) `WHERE id = $1 AND claimed_at = $held AND state = 'KILLING'`, and `advance()` runs in the same transaction, as `apply_outcome` does.

A handler whose `cancel` returns `Err` (`handlers/sql.rs`) is still written `KILLED` after the drop; the error goes to `tracing`, as the timeout path does (`worker/cancel.rs`, fact 2).

A renewal that fails because the state is no longer `RUNNING` is not a lost lease: the worker reads the state and takes the kill path above instead of `Leased::Lost` (which drops the handler with no cancel on purpose, `worker/claim.rs:1715-1721`).

### 4.3 A worker that dies while killing

- The reaper (`worker/reaper.rs:42-44`) clears an expired lease on a `KILLING` run exactly as on a `RUNNING` one.
- `claim` (`worker/claim.rs:477-485`) also takes `state = 'KILLING' AND claimed_at IS NULL`. A claimed `KILLING` run does not run its handler: the worker builds the handler, calls `cancel` (a container is named by run and attempt since W5a, so the new holder reaches the old container), then writes `KILLED` as in §4.2 step 3.

## 5. `advance()` while the execution is `KILLING`

`advance(plan, state, now)` (`executor/mod.rs:175`) stays pure. When the execution's last state is `Killing`:
- it creates no run and starts no retry;
- a flowable run that is not terminal becomes `KILLING`, then `KILLED` once every child run is terminal (Kestra `executor/…/ExecutorService.java:313-335`);
- the execution becomes `KILLED` once every run is terminal.

## 6. The `Exit` task

`io.kestra.plugin.core.execution.Exit`, property `state` (default `SUCCESS`; one of `SUCCESS`, `WARNING`, `KILLED`, `FAILED`, `CANCELLED`). In Kestra it is an `ExecutionUpdatableTask` run by the executor (`core/…/plugin/core/execution/Exit.java`). In egret it is executor-level too: registered in `spec::registry` with no worker handler, and `advance()` handles its run when it is created:
- `KILLED`: the execution becomes `KILLING` and §5 takes over.
- any other state: the `Exit` run, each of its ancestor runs, and the execution are set to that state (Kestra's `update`: the last not-terminated run, then its parents, then the execution). Running sibling branches keep running (D5).
- `state` is a property that may be templated; it is rendered with the run's render context. A value outside the five is a run failure naming the value.

The refusal of `Exit` (`executor/mod.rs:4235-4250`, the four `valids/exit*` entries) is removed.

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
