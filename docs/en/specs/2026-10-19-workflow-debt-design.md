# Workflow debt package: what groups B, C1, C2 and WorkingDirectory left open

**Status:** designed 2026-10-19. PR-1 built and verified 2026-10-19 on branch `feat/workflow-debt-kill-wd` (PR-1); PR-2 built and verified 2026-10-19. The user's order of 2026-10-18 puts this package after C2 merges and before the Kestra-parity roadmap (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` row 1 waits for it).

**Where the claims come from:** every claim about egret's code was checked in the worktree `egret-coretasks` at `481d9101` (branch `feat/workflow-kestra-executor-tasks`, C2 at HEAD, which is what main looks like once C2 merges). Code is cited `path:line` relative to `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`, and its paths are relative to it. A claim that could not be checked statically says "to measure in Task N" and names the plan task that measures it. Where the debt notes said something the code no longer shows, the item is reported as stale, not planned.

**Where the debt was recorded:**
- the kill debt and the C1 debt: the group B and group C1 paragraphs of the programme memory, and the kill spec's §4.2 note (`docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md:101`);
- the WorkingDirectory debt: the WorkingDirectory spec's open point 2 (`docs/en/specs/2026-10-16-workflow-working-directory-design.md:174`) and the merge note of #683;
- the C2 minors: the "can wait" list in the C2 final review (M3–M8) and the "minor (deferred)" lines of the C2 ledger.

**Decided with the user, 2026-10-18:**
- **This package comes before the parity roadmap.**
- **A killed task whose container was started with `taskRunner.delete: false` keeps its container.** Today the kill removes it. The orphan sweep then removes such containers after a deadline; today it skips them for ever.

**Decided with the user, 2026-10-19** (the two open questions of the first draft):
- **The kept-container deadline (D3) applies to every kept container whose run is terminal**, whatever the run's end state, so runs that finished normally lose their kept container after the deadline too. Kestra keeps them for ever; egret bounds them, because the Docker runner has no disk bound.
- **Publish accepts a `Subflow`/`dependsOn` target that is absent from the caller's workspace, with Kestra's warning, and resolves it in that workspace only** (§5.1). The four security tests are rewritten to keep the no-probe property. §5.1 is built in PR-2.

**Default decisions (2026-10-19).** The rule: follow Kestra unless egret has a reason not to, and keep safety tight.
- **D1. On Docker, a kill or an expired budget stops a `delete: false` container and does not remove it.** The stop is `docker stop -t <grace>`, where a grace of zero kills at once. This is Kestra's `Docker.cleanupContainer`: it calls `kill()`, then `removeContainerCmd` only when `delete` is true (`script/src/main/java/io/kestra/plugin/scripts/runner/docker/Docker.java:662-690`, `kill` at `:777-800`). The budget's expiry and the kill share one `cancel` (`worker/cancel.rs:208`), so both change together, as in Kestra. `delete: true` keeps `rm --force`.
- **D2. Kubernetes keeps today's behaviour: a killed pod is deleted, `delete: false` or not.** A pod's container cannot be stopped without deleting the pod; the only way is an exec of `kill` inside an author's image, which is fragile. The kill was never measured on a cluster (kill spec §9), and WorkingDirectory on Kubernetes is deferred too. The divergence is written in the kill spec.
- **D3. The kept-container deadline is a new setting, `workflow.kept_container_retention_days`, default 7, where `0` keeps containers for ever.**
  - The setting follows `workflow.storage_retention_days` (`crates/egret-config/src/lib.rs:745`, default 30, `0` = for ever).
  - The clock starts at the run's `ended_at` (migration 0183), or at `updated_at` for a row older than 0183.
  - Why 7 days and not the 30 that logs and files keep: a kept container holds its writable layer on the daemon, and the Docker runner has no disk bound (`worker/commands.rs:2590-2597`, "No disk bound"). A week still covers a weekend between a kill and someone inspecting it.
  - The sweep runs on the reaper's tick (`runtime.rs:638-659`, every `reaper_interval_secs`, default 60), so a container goes within one minute of its deadline.
  - Both runners' sweeps apply the deadline: Docker's (`worker/commands.rs:2514-2557`) and the pod sweep (`worker/k8s/runner.rs:1485-1547`). It reaches every kept container whose run is terminal (decided with the user, 2026-10-19).
- **D4. A kill that cannot load an execution closes it by force, as `KILLED`.** The force-close is §2.2. Kestra's executor fails an execution it cannot process (`Execution.failedExecutionFromExecutor`, `core/src/main/java/io/kestra/core/models/executions/Execution.java:860-902`). Egret writes `KILLED` instead of `FAILED`, because the operator asked for a kill.
- **D5. `commit_kill` keeps its single retry of a deadlock victim.** This package adds the test the retry never had (§2.3). There is no behaviour change.
- **D6. A dedicated hourly sweep removes the `_wd/{block}/` snapshots that a crashed `WorkingDirectory` block left behind**, 24 hours after the block's run ended (§3.1). It uses migration **0197** (next free: `ls migrations | tail -1` is `0196_workflow_execution_variables.sql`).
- **D7. The executor suite's oracle setup empties `workflow.flows` before it files the corpus.** A failed first setup then no longer turns every later test into a misleading "left 335" (§3.2).
- **D8. `executions.variables` is capped at 1 MiB of serialised JSON.** That is `runners::env::MAX_OUTPUT_BYTES` (`runners/env.rs:204`), the bound on one render and on a container's output markers (`worker/commands.rs:204`). An update that would exceed it fails the run, with egret's own message.
- **D9. A `Labels` task's labels pass the backfill's per-label caps**: a key of at most 128 characters and a value of at most 1,024 (`labels.rs:7-8`). The merged list holds at most 100 labels: twice a backfill's 50 (`labels.rs:6`), so a flow's own labels and a backfill's fit.
- **D10. (M5) Updates created in the same render pass render against the surface as the pass began, and merge onto the current value.** This aligns with Kestra (§4.2). The C2 spec's "each one sees the result of the one before it" is withdrawn.
- **D11. (M6) A failed update stays a plain run failure.** Kestra's executor-exception path is recorded as a known divergence, not copied (§4.3).
- **D12. (M7) A `LoopUntil` iteration gives each restarted descendant a new task-run id**, as Kestra recreates the task runs (§4.4).
- **D13. (M8) Flow-trigger `executionLabels` are recorded as a divergence, not fixed** (§4.5).

- **D14. The oracle driver resumes a run whose short deadline passed under load.** This is a fix in the oracle driver (test harness), not in production code, added 2026-10-19 after the ratchet-flake investigation (§3.2). It is PR-1 Task 1.6b.
- **D15. A kept container whose run id has no row, because a `LoopUntil` iteration superseded it, is swept once its execution is terminal.** It gets the retention deadline of D3, measured from the execution's end, through the container's `egret.io/execution-id` label. A `delete: true` container is left alone by this rule. A `WorkingDirectory` block inside a `LoopUntil` is prepared again each iteration, and the `_wd` cleanup removes the old ids' prefixes (§4.7). Added 2026-10-19 while building D12 (PR-2 Task 3b).

## 1. Every debt item, verified

| # | Item | Verified at | State |
|---|---|---|---|
| 1 | Kill removes a `delete: false` container; the sweep never removes one | `worker/commands.rs:1617-1643` (`cancel` ends in `rm --force`), `:2563-2577` (`parse_docker_ps` drops `delete=false`), `worker/k8s/runner.rs:1503-1511` | open, §2.1 |
| 2a | `commit_kill`'s deadlock retry has no test | retry at `worker/claim.rs:2414-2427`; the only deadlock test, `tests/kill_route.rs:735`, drives `run_one`'s retry (`claim.rs:1545-1600`), not `commit_kill`'s | open, §2.3 |
| 2b | A kill whose execution cannot be loaded loops | `claim::load` fails `Unplannable`/`Corrupt`/`UnknownState` (`worker/claim.rs:4078-4080`, `:4111-4116`); `commit_kill_with` calls it first (`:2452-2453`); `finish_claimed_kill` routes a row-content fault to `commit_kill` (`:1725-1736`), which fails again; the run keeps its lease until `reap`, then the next claim repeats it every `working_timeout` (3600 s) for ever. The route's `kill_in` (`worker/kill.rs:147-172`) and the cascade's `descendants` (`:191-215`) fail the same way, as a 500 | open, §2.2 |
| 3 | VI §9 lacks the 2026-10-16/17 notes | VI has every dated note EN has: AllowFailure `docs/vi/...programme-design.md:521`, WorkingDirectory `:523-525`, kill `:527`, C1 `:529` | **stale**. Two real gaps instead: VI `:511` lacks EN `:523`'s strike-through and "(retired 2026-10-17 …)"; **neither** EN nor VI has a C2 note (EN `:541` still says "Group C2 … is next") |
| 4 | Two publish over-refusals | `publish.rs:433-450` (Subflow) and `:532-558` (Flow trigger `dependsOn`) refuse `None` as `ref_not_found()` | open, §5.1 (user: accept, 2026-10-19) |
| 5 | `render_map`'s `{% %}` not in the spec; schema README §1 | `models/property.rs:133-137,162-163` handle `{%`…`%}` as a whole expression, and the C1 spec's build note (`docs/en/specs/2026-10-18-workflow-core-tasks-design.md:90-94`) does not say so; `src/spec/schema/README.md:19,29-31` is a 90-word sentence; the recipe still names `w7a-kestra-t1` and `admin@w7a.local` (`:41-54`) | open, §5.2 |
| 6a | `_wd` crash sweep | §3.1 | open |
| 6b | "flake 335" | §3.2 | cause found statically; to reproduce in PR-1 Task 6 |
| 6c | Unticked WorkingDirectory plan boxes | `docs/en/plans/2026-10-16-workflow-working-directory.md`: 35 unticked; the work behind 32 of them is in the code (§3.3) | open, docs only |
| 7a | Size caps | §4.1 | open |
| 7b | M5 | §4.2 | open, align |
| 7c | M6 | §4.3 | record |
| 7d | M7 | §4.4 | open, align |
| 7e | Code minors | `executor/update.rs:94` clones the variables for every op, `Labels` included (the ledger's `:97` has moved); `executor/plan.rs:1365-1367` calls `UpdateOp::of` twice with an `expect`; `spec/unrunnable.rs:144`'s test is named "publish" but drives `validate_for_run` + `unrunnable`; the `update.rs` test gaps listed in the C2 ledger (Task 2) | open, §4.6 |
| 7f | M8 | §4.5 | record |

## 2. Kill (PR-1)

### 2.1 A killed `delete: false` container is kept, and swept later

**Today:**
- `TaskRunner::cancel(task, grace)` (`worker/commands.rs:641`) has no way to know `delete`. Docker's ends in `rm --force` (`:1631`). Kubernetes deletes the pod (`worker/k8s/runner.rs:1417-1438`).
- `release` reads `delete` (`worker/commands.rs:1648-1656`, `k8s/runner.rs:1442-1450`), but the kill path calls it after a cancel that has already removed everything (`worker/claim.rs:2392`; kill spec §4.2 note).
- The script handler re-parses its spec in `release` (`worker/handlers/script.rs:786-797`) but not in `cancel` (`:774-776`).
- The plugin handler always uses `delete: true` (`worker/handlers/kestra.rs:802-822`; publish refuses a `taskRunner:` on a plugin task).

**Change:**
- `TaskRunner::cancel` takes the parsed spec: `cancel(task, grace, spec: &RunnerSpec)`.
- The script handler's `cancel` parses the spec as its `release` does. A task that does not parse, or names another runner, uses the configured runner's defaults (`delete: true`), as today.
- The plugin handler passes `RunnerSpec::defaults(kind)`.
- Docker, `spec.delete == false`:
  - `docker stop -t <ceil(grace)>`, bounded by `grace + CANCEL_WAIT_MARGIN` like the graceful path (`worker/commands.rs:1617-1630`), and no `rm`.
  - A container that is absent is `Ok(())`.
  - A container that is already stopped is `Ok(())`. `docker stop` is believed to exit 0 there; to measure in PR-1 Task 1 on the daemon, as W5a measured `rm --force` (`:1636-1640`).
- Docker, `spec.delete == true`: unchanged.
- Kubernetes ignores `spec.delete` in `cancel` (D2), with a doc comment saying so.
- `release` after the kill already keeps a `delete: false` container (`worker/commands.rs:1649`), so the kill path's release needs no change.

**The sweep:**
- `parse_docker_ps` returns the keep flag instead of dropping kept containers.
- `run_liveness` (`worker/commands.rs:2456-2487`) also returns each terminal run's `COALESCE(ended_at, updated_at)`.
- One pure function, shared by both sweeps, decides:
  - not terminal: keep;
  - terminal and not kept: remove (today's rule);
  - terminal and kept: remove once `now - ended > retention`, where `None` (setting `0`) means never.
- The pod sweep (`worker/k8s/runner.rs:1503-1511`) reads the annotation the same way. A pod whose run has no row is still left alone in both halves (`:1530-1533`, `worker/commands.rs:2504-2511`).
- The retention reaches the runners at construction, in `runtime::handlers_for` (`runtime.rs:750-778`): `DockerRunner::with_kept_retention(Option<Duration>)` and the same on `KubernetesRunner`.
- Config: `WorkflowConfig::kept_container_retention_days: u32`, default 7, documented like `storage_retention_days`, with env `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS`. It is not a chart value, by the rule at `deploy/helm/egret/templates/configmap-env.yaml:44-51`.

**A consequence to state:** a `LoopUntil` body run reuses its task-run id and restarts its attempts at 0 (§4.4), so its container name `egret-run-{run}-{attempt}` (`worker/commands.rs:793-795`) repeats across iterations. `find` treats an exited container as `Started` (`:1717-1719`), so with `delete: false` a second iteration could re-attach to the first one's exited container instead of running. This is believed to happen today; to measure in PR-2 Task 3. D12 removes it.

**(2026-10-19, measured and built.)** The live Docker case `a_loop_until_body_with_delete_false_runs_a_new_container_each_iteration` (`cargo test -p egret-workflow --test container_docker_live`) shows one new container per iteration, named from that iteration's new run id. D15 (§4.7) covers what a superseded iteration's kept container does afterwards.

### 2.2 Force-close

**When:** `claim::load` fails with a row-content fault (`WorkerError::is_row_content`, `worker/mod.rs:822-830`: `Unplannable`, `NotALeaf`, `Corrupt`, `UnknownState`) at one of three sites:
- the route's `kill_in` (`worker/kill.rs:147-172`);
- the cascade's `descendants` (`:191-215`), for one descendant;
- `commit_kill_with` (`worker/claim.rs:2450-2453`).

Any other error stays transient, as `finish_claimed_kill` already rules (`claim.rs:1695-1699`).

**What it writes:** one function, `worker::force_close::close(conn, execution, reason, own: Option<(run_id, claimed_at)>)`. It is plain SQL and never plans the source:
1. It locks the execution row (`FOR NO KEY UPDATE`, as `load`), then its run rows `FOR UPDATE` in `run_ix` order, as `kill_loaded` does (`worker/kill.rs:243-249`).
2. A live run (`NOT workflow.state_is_terminal(state)`) that a worker holds is set to `KILLING`, and keeps its lease. "Holds" means `RUNNING`, `SUBMITTED` or `KILLING` with `claimed_at IS NOT NULL`, and not the caller's own `own` run. That worker has the handler's input, so it can cancel; its `commit_kill` then reaches this function again with `own` set.
3. Every other live run becomes `KILLED`:
   - `KILLED` is appended to `history`;
   - `ended_at = now()` and `claimed_at = NULL`;
   - `message = reason` when no message is set;
   - the last attempt's `history` gets `KILLED` when its last state is not terminal.

   An unleased running run gets no cancel: nothing can build its handler (the fix round 1 rule, `claim.rs:1688-1694`). The orphan sweep owns its container, now including a kept one (§2.1). How the attempt's `ended_at` should be stamped (`stamps::stamp_attempt_ends` writes a tick relative to `created_at`) is to measure in PR-1 Task 4.
4. The execution becomes `KILLED`, with `ended_at`, when no run is left `KILLING`. Otherwise it becomes `KILLING`.
5. A `KILLED` execution reports to its parent through the same calls `settle` makes (`spawn::on_execution_terminal`, `worker/spawn.rs:512`; `subflow::on_child_terminal`, `worker/subflow.rs:602`). They load the parent, not this execution, so a waiting loop or `Subflow` run ends.
6. No Flow-trigger outbox event is written, and no `errors:` or `finally:` runs: there is no plan to run them from. The message says so.

**The message**, stored on each run it closes: `egret could not load this execution (<reason>), so it was killed without running its flow: no errors:, finally: or Flow trigger ran`. `<reason>` is the `WorkerError`'s text.

*Note, 2026-10-19 (as built, PR-1 Task 4):*
- **Rule 2 now holds a leaf only.** A run is held only when it is a leaf (a `Runnable` with a handler) that a worker leases. A leased `WorkingDirectory` block is not held: nobody would cancel it (its claim is never killed), and left `KILLING` it stalled for ever, so it is closed `KILLED` at once and its worker's later commit fails once on the load. A block already `KILLING` or `FINISHING` whose discard-only claim finds the execution unloadable is force-closed by that claim (`apply_block_claim_on`, test `a_discard_only_block_claim_of_an_unloadable_execution_closes_it`); a fault in any other block claim is returned as before. Tests: `a_force_close_closes_a_leased_block_claim_at_once`, `a_force_close_leaves_a_held_leaf_to_its_worker`.
- **Rule 5, the report of a force-closed child, is deferred and guarded.** The report is a deferred job (`Job::Report`), so the parent is loaded only when the job runs; in a cascade the parent can be a descendant whose kill has not settled yet. The report **skips a parent that is already terminal** (read in plain SQL, without its lock; test `a_report_to_a_terminal_parent_does_not_lock_it`), and it **is dropped, with a warning, when loading the parent hits a row-content fault** (an unloadable live parent is closed by its own kill; test `an_unloadable_iterations_report_to_an_unloadable_live_parent_is_dropped`). Any other error from the report is returned and fails the kill (`a_report_that_cannot_lock_the_parent_still_fails_the_kill`). The guard drops a row-content fault from anywhere in the report's calls, not only from the parent's load.
- **The signature gained `caller` and the result `own`.** `close(conn, execution, reason, own, caller) -> Closed`, where `Closed.own` says whether the caller's own fenced run was closed. `commit_kill_with` returns `LeaseLost` when it was not, but the transaction still commits the other runs it closed, so `LeaseLost` no longer means "nothing written" on this path.
- **The attempt's `ended_at` is measured** (the open point of rule 3): `stamps::stamp_attempt_ends` writes an integer tick, milliseconds after the execution's `created_at`, as `started_at`; `close` writes the same key and unit. The audit key `"forced": true` is written only when the route's transaction (or a cascaded descendant) was force-closed.

*Note, 2026-10-19 (final review I3, correcting rule 3's "the orphan sweep owns its container, now including a kept one"):* that sentence overstated what the sweep did. A kept (`delete: false`) container gets no `cancel` on the force-close path, nor when `finish_claimed_kill` cannot build a handler, so it could still be running when its run was already `KILLED`. Before this fix the sweep only removed it after the retention, so author code could run for up to 7 days, or for ever with the setting `0`. As built now:
- **Docker.** The sweep's listing reads `{{.State}}`. For a terminal run whose kept container is still running (`running`, `paused` or `restarting`), it runs `docker stop -t 0`, the same stop as D1, and keeps the container. Removal still waits for the retention (`commands::docker_sweep_action`). The grace is `0` for three reasons. The run is terminal, so nothing waits on a clean shutdown. D1's keep path uses grace 0 in practice, because only script tasks honour `delete`. And the sweep runs inside the reaper tick, one container after another. A stop that fails or times out is logged and retried on the next tick. Tests: `the_docker_sweep_stops_a_running_kept_container_of_a_terminal_run` (unit) and `a_force_closed_kept_container_is_stopped_by_the_sweep` (live Docker).
- **Kubernetes: no change, and a follow-up.** A pod cannot be stopped, only deleted, and a kill deletes it (D2). A kept pod of a force-closed run is therefore cleaned up only by the retention-based pod sweep, and may run until then. The follow-up is to delete such a pod, or otherwise end its process, when its run is terminal and the pod is still running.

**Audit:** the route's existing record (`routes.rs:3085-3096`) gains `"forced": true` when the route's own transaction force-closed. A worker-side force-close finishes a kill the route already audited, so it writes a `tracing::warn!` only.

**Kestra:** Kestra has no kill of an unloadable execution. Its executor answers an exception with `failedExecutionFromExecutor` (above), which fails the last unfinished task run or the execution. D4 keeps the shape and writes `KILLED`.

### 2.3 The `commit_kill` deadlock test

This copies `a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it` (`tests/kill_route.rs:735-830`), which covers `run_one`. Here the worker is in the kill-only claim:
1. A subflow child's run is `KILLING` with no lease.
2. The test transaction holds the parent's row, with `deadlock_timeout = 60s`.
3. The worker's `commit_kill` locks the child and then, through `subflow::on_child_terminal`, waits on the parent.
4. The test transaction then locks the child. That is the cycle, and the worker is the victim.
5. The retry (`worker/claim.rs:2419-2427`) waits for the test transaction, finds the fence still `KILLING` under its lease, and commits.

Mutation check: with the retry arm removed, the tick returns `Err`.

## 3. WorkingDirectory (PR-1)

### 3.1 The `_wd` crash sweep

**Layout:**
- The snapshots of block run `b` live under `s3://{bucket}/{prefix}{ws}/executions/{exec}/_wd/{b}/` (`storage/mod.rs:232-242`).
- The objects are `prepare.tar` (`worker/working_dir.rs:43,280`) and one `{child run}.tar` per child (`:133`).
- The design said `{seq}.tar` (`docs/en/specs/2026-10-16-workflow-working-directory-design.md:101`); that is doc drift, fixed in PR-1 Task 7.

**Normal removal:** `working_dir::discard` (`worker/working_dir.rs:953-966`) is one best-effort `delete_prefix`. It runs after a finish claim commits (`worker/claim.rs:1829`), or before the commit for a discard-only finish (`:1786`). On failure it logs "the pruner will sweep them".

**That log line is false.** The W9a pruner deletes only `execution_prefix`, which is `{ws}/{ns}/{flow}/executions/{exec}/` (`storage/prune.rs:512-531`, `storage/mod.rs:245-260`). `_wd` sits under `{ws}/executions/`, which has no namespace or flow segment. The pruner's candidates also need index rows that `_wd` objects never create (`storage/prune.rs:411-438`). So the design's open point 2 ("removed by the pruner's retention") does not hold.

**Leaks today:**
- a crash between writing a snapshot and the discard;
- any `delete_prefix` that failed;
- a block that never reaches its finish claim.

**Design (D6):**
- Migration 0197: `ALTER TABLE workflow.task_runs ADD COLUMN working_dir_swept_at timestamptz`, with `CHECK (working_dir_swept_at IS NULL OR working_dir IS NOT NULL)`, and a partial index on `(ended_at) WHERE working_dir IS NOT NULL AND working_dir_swept_at IS NULL`.
  *Renumbered, 2026-10-19:* written as 0196; #694 took 0195 first, so C2 moved to 0196 and this migration ships as `0197_workflow_task_run_working_dir_swept.sql`.
- **Why a column:** without a "done" mark, the sweep must re-list every block of the last 30 days on every pass. With one, it lists each crashed block once.
- `discard` stamps `working_dir_swept_at = now()` after a `delete_prefix` that succeeded. A failed one leaves it NULL.
- **The sweeper** (`storage::wd_sweep`, modelled on `storage/nsfiles/staging.rs:118-206`):
  - every hour, it selects block runs with `working_dir IS NOT NULL AND working_dir_swept_at IS NULL`, whose execution is terminal (`workflow.state_is_terminal`) and whose `COALESCE(ended_at, updated_at)` is more than 24 h old (`StoragePruneConfig::new`'s `orphan_age`, `storage/prune.rs:136-144`), 100 per batch;
  - for each, it calls `delete_prefix(working_dir_prefix(ws, exec, run))`, then stamps the column;
  - it takes a `pg_try_advisory_lock` of its own, so one replica sweeps at a time.
- It is spawned in `crates/egret-bin/src/main.rs:1928-1946` beside the staging and KV sweepers, on every replica with a store. It runs whatever `storage_retention_days` says: these are scratch objects, not the author's files.
- It is runner-agnostic, so it covers Kubernetes when WorkingDirectory reaches it.

*Note, 2026-10-19 (as built, PR-1 Task 5):* the selection does not read the run's state, so it also sweeps the snapshots of a block that a kill's force-close closed as `KILLED` (§2.2): such a block keeps its `_wd` objects, and nothing claims a `KILLED` block again. The test `the_wd_sweep_removes_a_force_closed_blocks_snapshots` pins this, and adding `t.state <> 'KILLED'` to the selection turns it red. A discard that deletes successfully stamps `working_dir_swept_at` itself (`worker::working_dir::discard_snapshots`); a failed delete leaves it NULL for the sweep. A restart in place of an execution is not built (a replay is a new execution), so the swept mark has no reader that could resurrect a swept block; roadmap row 7 (execution operations) must clear or respect `working_dir_swept_at` when it adds one.

### 3.2 "Flake 335"

**The assertion:** `oracle-exec/driver.rs:341-348` asserts that `workflow.flows` is empty before `publish_corpus` files the corpus. `publish_corpus` is the only writer of `workflow.flows` in the `executor` binary.

**The caller:** `egret_side()` (`tests/executor.rs:1452-1477`) runs it inside a `tokio::sync::OnceCell::get_or_init`, after `TRUNCATE workflow.executions CASCADE`. That truncate does not empty `workflow.flows`, because `flows` references nothing in `executions`. Fourteen tests call `egret_side()`.

**The arithmetic:**
- One pass over a corpus of N files `N - PARSE_REFUSED (6) - CREATE_REFUSED (4)` rows (`driver.rs:799,803`).
- At 351 that is 341. At 345, the corpus before WorkingDirectory Task 8 added its six flows, it is **335**.
- So "left 335" is exactly one completed pass over the 345-flow corpus.

**The cause, statically:**
- If the first init files the corpus and then panics on one of its census asserts (`driver.rs:1135-1166`; `a7270435` sits inside WorkingDirectory Task 8, where the corpus and the census constants changed together), the `OnceCell` stays empty.
- tokio's `get_or_init` does not store a panicked init (to confirm in PR-1 Task 6).
- So every later caller runs the init again on the same per-process clone (`crates/egret-test-db/src/lib.rs:352`), finds 335 rows and fails with the misleading message.
- That gives 1 real failure plus up to 13 copies.
- Cross-process sharing is ruled out: each process clones its own `egret_t_{pid}` database (`egret-test-db/src/lib.rs:105,209-235`).

**Fix (D7):** inside the existing `is_process_clone` guard, `TRUNCATE workflow.flows CASCADE` before `publish_corpus`. A re-run init then fails on the same census assert as the first one, so all fourteen failures carry the real cause.

**Reproduction (PR-1 Task 6):** a temporary, uncommitted `LIVE - 1` in `driver.rs`.
- Before the fix: one census failure, and "left 341" everywhere else.
- After the fix: the census failure everywhere.

*Note, 2026-10-19 (as built, PR-1 Task 6):* the guard is `TRUNCATE workflow.flows, workflow.executions CASCADE` in `egret_side()`, in one statement. Measured with a temporary `LIVE = 292`: before the fix 17 tests failed, 1 on the census and 16 on "workflow.flows must be empty" with `left: 341`; after it 17 failed, all on the census. The ratchet then read 301 golden entries, 73 mismatching, 73 baselined, 0 broken. The comment in the code keeps the historical "335/345" figures (at `a7270435`); today the figures are 341 and 351.

#### D14: the oracle driver and a short deadline (PR-1 Task 1.6b)

**The cause.** The flake investigation (2026-10-19) found that the intermittent extra mismatch was not Task 1.1 to 1.3 or C2. It was a race in the oracle driver, from W3b (#576): when a run's deadline is short, the driver's `pass_time` can see it after the deadline has passed under load, so the driver stopped on a run that was due and left it `PAUSED`.

**The fix.** `driver::run` now delegates to `run_with(.., before_pass_time: Duration)`, a seam with a delay (zero in the ratchet path; no env var). When `pass_time` returns `Ok(false)`, the driver calls `due_in_tree`, which walks the root's tree over `parent_execution_id` and asks `worker::due_retries(pool, i64::MAX)`, the selection `wake` runs. A due run in the tree makes the driver `continue`, so the next tick's `wake` resumes it; otherwise it stops. The existing step bound ends a spin as `Broken("did not settle within ..")`, so a never-resumed due run is a broken entry, which cannot be baselined. The pin test is `the_driver_resumes_a_run_whose_short_deadline_passed_before_pass_time` (a 300 ms gap on `valids/waitfor-multiple-tasks`); with `Ok(true) => continue` mutated to `break` it goes red (`left: Some("PAUSED")`). With a 300 ms delay on every entry the whole ratchet reads 73 mismatching with no regression (before the fix: 83 and 10 regressions); under 12 CPU burners, 4 runs of 4 were green.

**What is still open ("mode 2").** Under heavy load `valids/waitfor-child-task-warning` can still match by accident, which makes the ratchet fail with "PROGRESS NOT BANKED". Wall time leaks into the Tick clock, so `reached_maximums` (`executor/flowable/loop_until.rs`) fires `maxDuration` one iteration early. The proper fix is a virtual clock in the driver. The entry stays baselined, with a comment in `oracle-exec/baseline.txt`, and must not be banked from such a run. The tree walk follows `parent_execution_id` only (as `pass_time` does), so a due run in a child linked only by W6's own column is outside both.

**Ratchet counts for PR-1:** 301 golden entries, 73 mismatching, 73 baselined, 0 broken, with the census LIVE 293 (351 corpus flows: 293 live, 48 draft-only, 6 unparseable, 4 name-contested). Source: `cargo test -p egret-workflow --test executor`. Its test `executor_matches_golden` prints `oracle-exec ratchet: 301 golden entries, 73 mismatching, 73 baselined, 0 broken`, and its census asserts `LIVE = 293` (`oracle-exec/driver.rs`). The figures are identical to the baseline before PR-1. The D14 pin test is `the_driver_resumes_a_run_whose_short_deadline_passed_before_pass_time`.

### 3.3 Plan checkboxes

`docs/en/plans/2026-10-16-workflow-working-directory.md` has 35 unticked boxes:
- Task 0: 3; Task 1 Step 5: 1; Task 3: 4; Task 4: 3; Task 5: 4; Task 6: 4; Task 7: 3; Task 8: 5; Task 9: 6; Task 10: 2.

The work behind 32 of them is in the code:
- Task 3: `executor/plan.rs:393`.
- Task 4: `worker/snapshot.rs`.
- Task 5: `executor/flowable/working_directory.rs`.
- Tasks 6-7: `worker/working_dir.rs:1113-1353`.
- Task 8: six `oracle-exec/corpus/valids/working-directory-real-*` flows, census 351 at `oracle-exec/driver.rs:1135`.
- Task 9: the live run recorded in the WorkingDirectory spec §7.
- Task 10: EN programme `:535`, and the VI files exist.

Three stay unticked with a dated "deferred, Kubernetes" note: Task 0's Kubernetes question and Task 1 Step 5. The VI plan (27 unticked) mirrors the EN plan.

## 4. C2 follow-ups (PR-2)

### 4.1 Size caps (D8, D9)

**Measured bounds:**
- `MAX_OUTPUT_BYTES` 1 MiB per render (`runners/env.rs:204`).
- `MAX_VARS_BYTES` 1 MiB on a container's output markers (`worker/commands.rs:204`).
- The webhook body 256 KiB (`webhook_routes.rs:254`).
- A JSON start body 2 MiB (`routes.rs:2360`).
- Backfill labels: 50 labels, 128-character keys, 1,024-character values (`labels.rs:6-8`), checked only by `backfill_labels_refusal` (`labels.rs:37-75`).

Nothing bounds the merged `executions.variables` or the labels a `Labels` task adds (C2 final review M3).

**Change:** in `executor/update.rs::compute`:
- **`Set`:** the serialised result must be at most `MAX_VARIABLES_BYTES = runners::env::MAX_OUTPUT_BYTES`. Otherwise the run fails with `` `variables`: the execution's variables would be <n> bytes, over egret's limit of 1048576 ``. `Unset` only shrinks, so it is not checked.
- **`Labels`:**
  - each label the task sets must pass the key and value lengths, with the backfill's messages;
  - the merged list must hold at most `MAX_EXECUTION_LABELS = 100` labels, with `` `labels`: the execution would carry <n> labels, over egret's limit of 100 ``;
  - Kestra's two checks keep running first, so the golden messages are unchanged.

These are egret's own bounds; Kestra has none. No golden flow comes near them; the plan confirms this with the ratchet.

### 4.2 M5: render against the pass's start (D10)

**Kestra, measured:**
- `handleWorkerTasks` builds a `RunContext` for every `CREATED` task run from the execution as the cycle began (`executor/src/main/java/io/kestra/executor/ExecutorService.java:1098-1107`). `RunContextFactory` freezes the variables at that point (`core/.../runners/RunContextFactory.java:152-168`).
- `handleExecutionUpdatingTasks` then calls each `update(executor.getExecution(), runContext)` with the live, already updated execution (`ExecutorService.java:1390-1406`).
- `SetVariables.update` renders with the frozen context, checks `overwrite` against the live variables, and merges onto them (`core/.../plugin/core/execution/SetVariables.java:69-88`).
- So in Kestra, the second update renders what the first one saw and merges onto what the first one wrote.

**Egret today:** `render_updates` builds each context after the previous update changed the surface (`worker/render.rs:1505-1528`, doc `:1459-1460`). `tests/worker.rs:11538` pins `y = "1"`.

**Change:** build every pending run's context first, then compute in run order with the live `surface.vars()` and `current_labels()`.

**The test:**
- The flow gains `variables: {x: 0}`, because Kestra fails `two` without it: `vars` is undefined there (to measure: egret's renderer's behaviour on an undefined `vars`, PR-2 Task 2).
- It expects `{"x": 2, "y": "0"}`.
- No corpus flow puts updates in a `Parallel`, so the ratchet cannot move.

The C2 spec §3.1 bullet "Each one sees the result of the one before it" is corrected.

**(2026-10-19, built.)**
- The test is `two_updates_in_one_parallel_render_the_start_and_merge_in_run_order` (`cargo test -p egret-workflow --test worker`). It was red against the old code (`y` was `"1"`) and is green now.
- **Measured (the "to measure" above):** `{{ vars.x }}` with no `vars` fails in egret, as it does in Kestra. The renderer answers `undefined value` for a missing `vars` key, for `vars: null` and for `vars: {}`. It never renders empty, because the renderer is strict about undefined values.
- So there is **no M5 divergence** to list. The only change is D10's run-order rule.

### 4.3 M6: a failed update (D11, record)

**Kestra's catch:**
- It fails the update's task run and stores the exception (`ExecutorService.java:1425-1432`).
- `DefaultExecutor` then calls `handleFailedExecutionFromExecutor` (`executor/src/main/java/io/kestra/executor/ExecutorService.java:247-252`), which runs `failedExecutionFromExecutor`.
- That forces the **last unfinished task run** to `FAILED`, or the execution when there is none (`Execution.java:860-902`).

**Consequences in Kestra:**
- A root-level failing update fails the execution directly, so the flow's `errors:` does not run.
- A nested one can force-fail an innocent sibling, or the enclosing `AllowFailure`.

The C2 final review's M6 and this measurement agree that this is a fallback path for an exception, not a designed behaviour.

**Egret:** `FailUnstarted` fails only the update run (`worker/render.rs:1531-1541`). `errors:` and `AllowFailure` then apply as to any failure.

**Golden:** the only covered shapes are `valids/set-variables-duplicate` and `valids/labels-update-task-empty`, both top-level with no `errors:`, and both systems agree there. No corpus flow combines an update with `errors:` or `AllowFailure`.

**Decision:** recorded in the C2 spec's divergence list; no change.

### 4.4 M7: a new task-run id per `LoopUntil` iteration (D12)

**Kestra:**
- `ExecutionService.retryWaitFor` removes every descendant task run of the loop (`core/src/main/java/io/kestra/core/services/ExecutionService.java:180-208`).
- `LoopUntil.resolveNexts` recreates them through `TaskRun.of`, each with `IdUtils.create()` (`core/.../models/executions/TaskRun.java:210-213`).

**Egret:**
- `Command::RestartLoop` empties the same rows in place (`executor/state.rs:986-1002`).
- `Surface::reserve_id` returns the loaded id (`worker/render.rs:678-685`).
- So `{{ taskrun.id }}` repeats across iterations (`render.rs:845-847`).
- So does every key derived from it:
  - the log lines' `task_run_id` (migration 0159, `routes.rs:1979-2109` filters on it);
  - the execution files' URI `tasks/{task}/{run}/{file}` (`storage/uri.rs:296,345`), whose index row is upserted, so iteration 2 overwrites iteration 1's (`storage/mod.rs:469-472`);
  - and §2.1's container name.

**Change:**
- When a settle's `RestartLoop` empties a run, `persist` writes it a fresh `id`. No foreign key references `task_runs.id`: migrations 0159 and 0166 say so, `:25` and `:24`.
- The surface forgets the old id, so the next `reserve_id` is fresh.
- The point inside `claim::settle`/`persist` where before and after are both in hand is to find in PR-2 Task 3.

**The test:** a `WaitFor` body `SetVariables {ids: ["{{ taskrun.id }}"]}` over three iterations. `deep_merge` appends only new list items (`executor/update.rs:169-199`), so the result holds ~~3~~ **4** ids after the change, and 1 before it.

**(2026-10-19, correction.)** The text above said 3 ids; that was wrong. `maxIterations: 3` runs the body four times: the first pass plus three restarts. Kestra's golden for `valids/waitfor-max-iterations` (`maxIterations: 3`) has the `WaitFor` run go `RUNNING` four times with three `PAUSED` between them, and `LoopUntil.java:142` renews the ids on each restart. The built test `each_loop_until_iteration_has_its_own_task_run_id` (`cargo test -p egret-workflow --test worker`) reads the iteration count from the loop's own history, asserts it is 4, and asserts one distinct id per iteration and that the row holds the last one.

**(2026-10-19, built.)** The renewal is in `claim::drive`, not in `persist`: `drive` applies a `RestartLoop` and then `renew_restarted_ids` gives every run that had a history and now has none a fresh id (the surface forgets the old one, `Surface::forget_id`). `write_render_side` writes the new ids to the rows. It cannot wait for `persist`, because a `SetVariables` body is rebuilt and rendered in the same settle that restarts it, and its render must already see the new id.

### 4.5 M8: Flow-trigger labels (D13, record)

**Kestra:** the trigger is evaluated against the end-of-cycle `Execution` object, with only the state swapped per transition (`executor/src/main/java/io/kestra/executor/DefaultExecutor.java:719-726`). It is not a fresh read.

**Egret:** `flow_trigger/process.rs:779-805` reads the current row when it processes the event. A `Labels` task in a later settle can therefore change what an earlier event's trigger sees.

**A fix** needs a labels snapshot on `workflow.flow_trigger_events` (migration 0157 `:38-50`), written in settle (`worker/claim.rs:3398-3412`). But `vars` and `outputs` have the same staleness, and a labels-only snapshot would make the rule half true.

**Decision:** recorded as a divergence in the C2 spec, and named in the parity roadmap's renderer row. No golden flow covers it.

### 4.6 Code minors

- `executor/update.rs:94`: compute `current` only in the `Set` and `Unset` arms.
- `executor/plan.rs:1365-1367`: one `let Some(op) = UpdateOp::of(..)` arm, with no `expect`.
- `spec/unrunnable.rs:144`: rename the test to `the_executor_level_types_pass_the_serve_gate_on_a_deployment_with_no_handlers`.
- New `update.rs` tests, each pinning one message or rule:
  - D8's null whole expression;
  - `deep_merge` with an absent key and a null value;
  - `Unset` on null variables;
  - the `labels[i]` and form messages;
  - `set_variables_without_variables_fails` asserting the whole message.
- `tests/migration_0196_variables.rs` keeps its own seed. Sharing `worker.rs`'s `seed()` would make a migration test depend on a worker test file; this is recorded and not changed.

### 4.7 A superseded `LoopUntil` iteration's kept work (D15)

D12 gives each iteration's body runs new ids, so the previous iteration's run id has no row. Two things hang on such an id. Both were found while building D12 (PR-2 Task 3b).

**Kept containers and pods.**
- Every container and pod already carries the label `egret.io/execution-id` (Docker `labels()` in `worker/commands.rs`, pods in `pod_spec.rs`), so no new label or migration was needed. The Docker sweep reads it as one more `docker ps` column, and the pod sweep reads the pod label. A label that is absent or not a UUID counts as none, and the container is left alone as before.
- `run_liveness` takes `(run, execution)` pairs. For the runs with no row it runs one more read-only query, bounded by the listed ids: which of those executions are terminal (`workflow.state_is_terminal`), and when they ended (`COALESCE(ended_at, updated_at)`, on the database's clock).
- **The rule (D15):** a kept (`delete: false`) container whose run has no row, and whose execution is terminal, is treated as the container of a terminal run (D3). It is removed once the execution ended more than `workflow.kept_container_retention_days` ago, and never with `0`. The Docker sweep also stops such a container if it is still running, as it does for a terminal run. That case does not arise normally, because an iteration's body has ended before the restart.
- **Left alone, as before:**
  - a `delete: true` container of a run with no row. In practice none exists, because release or cancel removes it;
  - a container whose run has no row and whose execution is unknown to this database or still live. That includes another egret's containers on a shared daemon.
- **Tests:**
  - unit tests `a_superseded_runs_kept_work_gets_the_retention_deadline` (it also pins the `delete: true` case) and `the_docker_sweep_parses_the_execution_label` (`cargo test -p egret-workflow --lib -- worker::commands`);
  - the live Docker case `a_loop_until_body_with_delete_false_runs_a_new_container_each_iteration` (`--test container_docker_live`): after a `LoopUntil` with two iterations, a one-hour sweep removes 0 containers and leaves 2, and a zero-retention sweep removes 2.
- **Kubernetes is covered by unit tests only.** The pod sweep goes through the same `sweep_decision`, but no cluster was available, and the pod path's own guard needs a `kube::Client`, so it has no unit seam.

**A `WorkingDirectory` block inside a `LoopUntil`.**
- Before this change the block kept its phase across a restart, so it was not prepared again for the next iteration. Now `Command::RestartLoop` clears the phase and the block is prepared again each iteration. `persist` also clears `working_dir_snapshot` when the phase is cleared, and `working_dir_swept_at` when the phase becomes `PREPARING`, so a restarted block does not carry the previous iteration's snapshot pointer or sweep stamp (the CHECKs of migrations 0194 and 0197 apply).
- **The old prefixes.** The block's run has a new id each iteration, so the `_wd/{old id}/` prefix of an earlier iteration would be orphaned. `wd_sweep::delete_superseded` lists `executions/{ex}/_wd/` and deletes every prefix whose block id no `task_runs` row of that execution holds. It runs after a discard's own delete succeeds, and from the `_wd` sweep for each block it removes. If it fails, the block is not stamped, so the sweep comes back to it. So an execution's last discard or sweep removes every earlier iteration's prefix.
- **Why that is safe:** a prefix whose id a row holds is that row's, live or not. A prefix with no row is over, because `RestartLoop` renames a block's row only after the block ended.
- **Rejected:** re-keying the stamp on `(execution_id, run_ix)`, and a shared per-`run_ix` prefix. A late discard of iteration 1 would then act on iteration 2's row or objects while iteration 2 prepares.
- **Tests** (`cargo test -p egret-workflow --test working_dir_claims`): `a_block_inside_a_loop_until_is_prepared_again_each_iteration`, `a_block_inside_a_waiting_loop_is_unstamped_when_it_restarts`, `a_block_after_another_task_in_a_loop_restarts_with_no_snapshot`, `a_later_discard_removes_a_superseded_iterations_snapshots`, `the_wd_sweep_removes_a_superseded_iterations_snapshots`. The live case `a_block_inside_a_wait_for_runs_its_docker_child_each_iteration` (`--test working_dir_docker_live`) runs a child that fails if iteration 2 reuses iteration 1's directory.
- **A residual hole, recorded and not closed.** If an earlier iteration's discard delete failed, and afterwards no later discard or `_wd` sweep runs in that execution, `_wd/{old id}/` stays. Two cases lead there: the block is never reached again and the execution has no other block; or the restart cleared the phase and the snapshot, so the row is no longer selected by the sweep. Closing it needs a durable record of the superseded prefix, which means a migration. This package adds none.

## 5. C1 follow-ups and docs (PR-2)

### 5.1 The two publish over-refusals

**Measured:**
- Kestra's `FlowService.warnings` turns a missing `Subflow` target into a **warning** (`core/src/main/java/io/kestra/core/services/FlowService.java:546-551,619-648`): "The subflow '<id>' not found in namespace '<ns>'.".
- It says nothing about a Flow trigger's `dependsOn` (`:556-568`).
- Both corpus flows were published with 200 (`oracle-exec/README.md:13453-13464`).

**Egret refuses both as `ref_not_found()`** (`publish.rs:94-105,433-450,532-558`). The refusal exists so that publish cannot probe what other workspaces hold: a foreign object and an absent one answer the same.

The lookup behind it, `FlowRepo::find_by_name` (`repo.rs:892-946`), is not scoped to a workspace. Its own doc records that a name two workspaces share can resolve to the wrong one and be refused as foreign (`repo.rs:901-922`).

**Runtime does not use the frozen id:** `subflow::resolve` looks the target up by name inside the execution's workspace (`worker/subflow.rs:1100-1117`).

**Proposed:**
- Publish resolves `Subflow` and `dependsOn` pairs **in the caller's workspace only**, through a scoped `FlowRepo` lookup, not the unscoped port.
- Found: the id is frozen, as today.
- Absent: the flow publishes, `flow_ref` stays `None`, and a `Subflow` gets Kestra's warning in the snapshot's `warnings`. A `dependsOn` gets none, as in Kestra.
- The answer no longer depends on any other workspace, so there is still no probe. It also closes `find_by_name`'s cross-workspace over-refusal.
- A run of a `Subflow` whose target is still absent fails as today, at `subflow::resolve`.
- This rewrites four security tests in `tests/publish.rs` (`:62`, `:95`, `:279`, `:308`): "foreign" becomes "absent from your workspace, published with the warning, and the run fails not found". The user agreed on 2026-10-19.
- The oracle census's `VALIDATE_REFUSED` and `LIVE` move by the two entries. To measure in PR-2 Task 5; `baseline.txt` is unchanged, because the executor oracle runs `build_plan`, not publish (`oracle-exec/README.md:13459`).

**(2026-10-19, built.)** As proposed, with these details:
- `FlowRepo::find_in_workspace(ws, namespace, flow_id)` is the scoped lookup. Publish uses it for a `Subflow` and for a Flow trigger's `dependsOn`, and only in the caller's workspace. A flow that only another workspace holds is answered exactly as an absent one.
- An absent `Subflow` target publishes with the warning `subflow_not_found`, message `The subflow '<flowId>' not found in namespace '<namespace>'.`, at the task's path. An absent `dependsOn` target publishes with no warning, as in Kestra.
- Publish always overwrites `flow_ref` with the lookup's answer, so an absent target clears a `flowRef` the author wrote in the draft.
- Publish also walks the nested `errors:` lists. A `Subflow` there gets the same lookup, warning and cleared `flowRef`, and a `database:` there is checked like a top-level one. (Unknown or unserved task types there were already refused by validate.)
- The unscoped lookup is gone: `RefKind::Subflow` and `FlowRepo::find_by_name` were removed, so the cross-workspace over-refusal cannot return.
- **Census, measured** (`cargo test -p egret-workflow --test executor`, over the 351-flow corpus): `LIVE` 293 to **295** and `VALIDATE_REFUSED` 27 to **25**. The two flows are `valids/task-runif` and `valids/trigger-flow-listener-no-inputs`. The census's counts of unscoped subflow lookups made by publish fell to 0, and they now guard against publish returning to the unscoped port.
- **The ratchet is unchanged:** 301 golden entries, 73 mismatching, 73 baselined, 0 broken. `baseline.txt` and `golden.json` are untouched.
- **Tests:** in `tests/publish.rs`, `a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it`, `a_trigger_dependency_absent_from_the_callers_workspace_publishes_whoever_has_it`, `an_authored_flow_ref_does_not_survive_an_absent_target`, and for nested `errors:` `a_subflow_inside_a_nested_errors_list_is_resolved_warned_and_its_authored_flow_ref_cleared` and `a_foreign_database_inside_a_nested_errors_list_cannot_be_published` (`cargo test -p egret-workflow --test publish`); in `tests/worker.rs`, `a_published_flow_whose_subflow_target_is_absent_runs_and_the_subflow_fails_not_found` (`--test worker -- subflow`), which runs the published flow and sees the child fail with `unable to find flow`.

### 5.2 Small doc fixes

- **C1 spec build note** (EN + VI):
  - `render_map` renders a string as one whole expression when, trimmed, it starts with `{{` or `{%` and ends with `}}` or `%}` (`models/property.rs:162-163`; Kestra's `PebbleUtil` delimiter checks). So `"{% if … %}{…}{% endif %}"` is read as a JSON object after rendering.
  - Any other string is parsed as JSON first, then walked.
- **`src/spec/schema/README.md` §1:**
  - Split the 90-word sentence at `:29-31` into three.
  - Rename the recipe's container and user from `w7a-kestra-t1`/`admin@w7a.local` to `kestra-schema-export`/`admin@egret.local`. Change nothing else, because the image ID and the commands are what was measured.
- **Programme spec §9:**
  - VI `:511` gains EN `:523`'s strike-through and its "(retired 2026-10-17 …)" note.
  - EN and VI gain a dated C2 note (the counts are in the C2 spec's Status: ratchet 84 → 73, LIVE 281 → 293) and, once each PR merges, a note for this package.
- **Kill spec §4.2** (EN + VI): the "open product question" note is closed by the user's ruling, and D1 and D2 are written there.
- **WorkingDirectory spec** (EN + VI): fix open point 2 and `:101`/`:111` (the pruner does not reach `_wd`; §3.1's sweep does); `{seq}.tar` becomes `prepare.tar` and `{child run}.tar`.
- **C2 spec** (EN + VI): D10, M6, M7 and M8 in the divergence list; §3.1's sentence corrected.

## 6. Packaging

Two PRs. Each is reviewable alone, and they touch different files except the docs:
- **PR-1, kill and WorkingDirectory:** §2 and §3, with migration 0197 and the new config key.
  - Live Docker cases: the flipped `a_killed_container_is_stopped_by_the_cancel_even_when_delete_is_false`, and a new kept-then-swept case.
- **PR-2, C2, C1 and docs:** §4 and §5. There is no migration.
  - §5.1 is built (the user answered yes on 2026-10-19).

They may run in parallel under the autonomy rules: one worktree, Postgres and target directory each. PR-2 rebases on PR-1's docs.

**Tests and acceptance, both PRs:**
- TDD per task; unit tests where the logic is pure (the sweep decision, the caps, `compute`).
- DB tests on a private Postgres.
- Docker live cases on a private daemon, never the shared dev stack.
- The oracle ratchet (`cargo test -p egret-workflow --test executor`, 301 golden, 73 mismatching / 73 baselined, 0 broken, LIVE 293) stays green after every task. Only §5.1 is expected to move census counts (LIVE 293 to 295 once §5.1 is built; measured in §5.1).
- The final gate is the C2 plan's, plus the Docker live suite for PR-1.

## 7. Open questions

None. Both questions of the first draft were answered by the user on 2026-10-19 (see "Decided with the user").
