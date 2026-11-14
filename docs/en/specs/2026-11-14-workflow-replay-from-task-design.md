# Workflow package 7b: replay from a task, on a chosen source, with new inputs

**Status:** designed 2026-11-14. Branch `feat/workflow-replay-from-task`, one PR. Roadmap row #7 (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), second of the six sub-packages 7a–7f. 7a, restart in place, merged as #860 (`docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md`, cited below as "7a Dn"). Kestra is the checkout at `data-portal/kestra`, `96893b58` (2.0.0-SNAPSHOT). `ES` is `core/src/main/java/io/kestra/core/services/ExecutionService.java`, and `EC` is `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`.

## 1. Problem

egret's replay (`POST /api/ws/{ws}/workflows/executions/{id}/replay`, `routes.rs` `replay_execution`) starts a new execution of the whole flow. It uses the old execution's frozen source and the same inputs. Kestra's replay (`EC:1437-1500`, `ES:393-512`) can do three more things:

1. **Start from a chosen task run** (`taskRunId`). The new execution copies the old one's task runs under new ids, together with their outputs. It resets the chosen run and its ancestors and drops everything after the chosen run in the graph, so the flow carries on from that task.
2. **Run on another revision** of the flow (`revision`).
3. **Run with new inputs** (`/actions/replay-with-inputs`, a multipart body that is validated the way a start is).

Every Kestra replay also adds the label `system.replay: true` and `attempt + 1` (`ES:497-501`). egret's replay adds neither.

Kestra's restart also takes a `revision` (`ES:281-373`). With a revision, the restart is not done in place: it creates a new execution on that revision that copies the task runs, resets the failed ones and is labelled `system.restarted`. 7a refused this parameter with `restart_revision_unsupported`, naming 7b (7a Q2).

## 2. Decisions

### 2.1 User decisions, 2026-11-14 (asked, answered)

- **Q1. A replay can run on three sources.**
  - The old execution's **frozen source** stays the default, as today.
  - **The flow's current source** is the draft in `workflow.flows.source`, which is what a start runs. This choice exists for "fix the flow, then replay from the failed task".
  - **A published version N** is a `workflow.flow_versions` row.
- **Q2. Restart with a revision is in 7b.**
  - It follows Kestra: a new execution that copies the task runs and resets the failed ones. It shares 7b's copy machinery and differs only in which runs it resets.
  - A restart on the frozen source stays in place, exactly as 7a built it.
- **Q3. Breakpoints are not in 7b.**
  - Kestra's `breakpoints` parameter stops a replay before chosen tasks and comes with "resume from breakpoint". It is a debugging feature with its own pause and resume state, so it gets a package of its own, next to 7c (pause and resume).
  - 7b refuses the parameter with 400, code `replay_breakpoints_unsupported`, and the message names the later package.

### 2.2 Design decisions (default decisions, 2026-11-14)

- **D1. Copy then reset, as Kestra does.** A replay from a task, or a restart on another source, creates the new execution row and copies the old execution's task runs into it. Each copy gets a new id (Kestra `mapTaskRunId`) with parent links remapped and outputs carried over (`taskOutputService.copyOutputs`).
  - The reset uses 7a's pure commands in `executor::restart` on the copied runs, and the executor drives on from there.
  - Rejected alternative: start fresh and insert synthetic `SUCCESS` runs for the tasks before the chosen one. It loses real outputs and leaves flowables (`Parallel`, `Loop`, `Switch`) with states that do not match their children.
- **D2. Replay from a task resets the run and drops what follows it** (`ES:410-492`):
  - The chosen run and its flowable ancestors are reset (Kestra `taskRunToRestart`, `ES:375-390`).
  - Every run that follows the chosen run in the graph is dropped (`GraphUtils.successors`, `ES:437-454`). This drops successors only, not the chosen run or its ancestors.
  - A run that had not ended in the old execution (for example a `Parallel` sibling) is reset too (`ES:456-484`).
  - The worker-task children of a reset run are removed (`removeWorkerTask`, `ES:491-492`).
  - The new execution starts `RESTARTED` (`ES:494-496`).
  - Whether Kestra's graph successors include the `errors`/`finally`/`afterExecution` runs is checked against Kestra while planning, and egret follows whatever Kestra does. Kestra's restart removes them explicitly (`ES:338-351`); its replay does not name them.
- **D3. Restart on another source resets every restartable run** (`ES:311-351`). The reset set is the runs whose state `canBeRestarted` (failed, or paused, as in 7a) and their ancestors. Worker-task children are removed, and so are the runs of `errors`, `finally` (with their children) and `afterExecution`. The label is `system.restarted: true` and the new execution gets `attempt + 1` (`ES:363-367`).
- **D4. The new execution's link and files.** It carries `replayed_from_execution_id` = the old id, the column today's replay writes (migration 0166). The copied outputs keep their `kestra://` URIs, which point at the old execution's files.
  - This needs today's read rule, under which a replay reads its source's files, to reach far enough. The plan checks the rule's depth: a replay of a replay must read outputs that live two executions back.
  - If the rule covers only one hop, the copy rewrites nothing. The read rule follows the `replayed_from` chain instead, bounded by the chain's length.
- **D5. Mapping onto another source.**
  - The copied runs are matched to the chosen source's tasks by `taskId`, as Kestra's are.
  - If the chosen task run's task, or the task of any run that is kept and not reset, is missing from the chosen source, the request is refused with 400, code `replay_task_not_in_source`, and the message names the task id.
  - Kestra does not check this and fails later in the executor, so this is an egret divergence (§5).
  - Runs that are reset or dropped need no match: a reset run is planned again from the new source.
- **D6. The source a replay runs.**
  - `frozen` uses `executions.source`, as today.
  - `current` uses the flow's draft, read inside the replay's transaction.
  - `version:N` uses that `flow_versions` row's source. A missing N answers 404, code `flow_version_not_found`.
  - In every case the new execution freezes the source it ran, as every execution does.
  - The flow's variables follow the chosen source, as Kestra's `applyNewRevision` takes `flow.getVariables()` (`ES:514-517`). The execution's run `variables` (7a D3 of W9) travel unchanged.
- **D7. Inputs.**
  - With no `inputs`, the old execution's inputs are reused, revealed as today (KI-112).
  - With `inputs`, the body (JSON, or multipart as `start_flow_execution` takes it) replaces them. It is checked against the **chosen source's** declared inputs by the same start check, as Kestra's `readExecutionInputs(flow, current, inputs)` checks it (`EC:1497-1498`).
  - `FILE` inputs follow the start route's rules.
  - Replacing the inputs does not reset any run, as in Kestra. A kept run keeps the output it computed from the old inputs.
- **D8. Gates, all of them those of today's replay:**
  - `workflow:manage`;
  - `startable_flow` (a trashed or purged flow answers 404, a disabled one 409);
  - the old execution must be terminal (409);
  - a loop iteration is refused (400, naming its loop root);
  - `sql_principal::check_start` with the **caller** against the source that will run (7a D24). With `current` or `version:N`, the check reads that source, so a non-creator cannot run a version that adds an `egret.sql` task.
- **D9. Refusals specific to a task run:**
  - A `taskRunId` that is not a run of this execution answers 404, code `task_run_not_found`, whatever workspace or execution it belongs to. Kestra answers `IllegalArgumentException` (`EC:1503-1507`), which shows as 400 or 500; egret uses the not-found answer its routes already give.
  - A task run inside a `Loop` iteration answers 400, naming the `Loop` task and the root execution to replay from instead. Kestra replays the iteration's own sub-execution (`ES:393-401`), but egret does not replay iterations on their own (W9a ruling); the root's replay from the `Loop` run gives the same result.
  - A task run inside a `WorkingDirectory` block answers 409 with code `inside_working_directory`, as 7a's mark-as does (7a D15).
- **D10. Labels and attempt on every replay.** Every replay, including the whole-execution replay that exists today, adds `system.replay: true` (unless the label is already there) and sets `attempt_number` = old + 1 (`ES:497-501`). Migration 0225 (7a) already allows `attempt_number > 1` on any row, so no migration is expected for this.
- **D11. API shape.**
  - `POST …/executions/{id}/replay` takes the query parameters `taskRunId`, `source` (`frozen` | `current` | `version:N`) and `breakpoints` (refused), and an optional body with `inputs` (JSON or multipart). This is one route where Kestra has two; egret's start route already takes both body types.
  - `POST …/executions/{id}/restart` takes `source` with the same values. `frozen`, or no value, keeps 7a's in-place restart.
  - On both routes, Kestra's `revision` parameter is accepted as an integer N meaning `version:N`, so a Kestra-shaped call works. Passing both `revision` and `source` answers 400, `invalid_source`. 7a's `restart_revision_unsupported` code is retired.
  - Any other value of either parameter answers 400, code `invalid_source`.
- **D12. Response.** A replay answers `201` with the new execution, as today. A restart that creates a new execution answers `201` with the new one; an in-place restart keeps 7a's `200`. Each route documents both answers in OpenAPI.
- **D13. Audit.**
  - `workflow_execution:replay` gains `task_run_id`, `source` and `inputs_replaced` (a boolean; the inputs themselves are never logged).
  - A restart on another source is audited as `workflow_execution:restart`, with `replacement` set to the new id and with `source`.
- **D14. Frontend.**
  - The **Replay** dialog gets a source choice: *This run's version* (default), *The flow's current version*, or a published version from a list.
  - It also gets an optional **Change inputs** section, a form built from the chosen source's inputs and prefilled with the old values.
  - Each task run's menu gets **Replay from this task**, which opens the same dialog with the task named in its title.
  - The **Restart** dialog gets the same source choice. When a source other than the default is chosen, its text says the restart creates a new execution.
  - Every refusal shows its message in the dialog. The user-visible strings go through the existing i18n of the execution page.

## 3. Shape of the change

| Area | Change |
|---|---|
| `executor::restart` (pure) | A `replay_from` command (D2) and the reset set for restart on another source (D3), next to 7a's commands, with unit tests. |
| `worker` | A copy of the task-run tree into a new execution, with remapped ids and parents and outputs carried over (D1, D4), called by the replay and restart routes in one transaction with the new row. |
| `routes.rs` | Replay and restart gain `taskRunId`, `source` and inputs. The refusals of D5, D8, D9 and D11, the label and attempt (D10), audit (D13) and OpenAPI. |
| Storage read rule | The files of a replay's source chain (D4), if the depth check calls for it. |
| Frontend | The Replay and Restart dialogs and the task-run menu item (D14), regenerated with `npm run gen:api`. |
| Docs | `egret-data/docs` Run actions page and screenshots; this spec's Vietnamese copy. |

Migrations: none expected (D10). If one becomes necessary, its number is checked against main and the open PRs before push.

## 4. Tests

- **Pure** (`executor::restart::tests`), replay-from cases:
  - in a `Sequential`, the runs after the chosen task are dropped;
  - in a `Parallel`, siblings that had not ended are reset;
  - in a `Switch`;
  - the reset set includes the flowable ancestors;
  - worker-task children are removed.
- **Pure**, restart on another source:
  - the reset set;
  - the `errors`/`finally`/`afterExecution` runs are removed.
- **Integration** (`tests/replay.rs`, new):
  - Replay from a task in a `Sequential`: the earlier tasks are not run again and keep their outputs, the later tasks run.
  - Replay from a task inside a `Parallel` branch.
  - Replay from a task inside a `Loop`: the run is in the root, and the request names the `Loop` run.
  - Replay from a `Subflow` run: the replayed run starts a new child.
  - `source=current` after the draft was edited runs the edited task.
  - `source=version:N` runs version N.
  - A task missing from the source is refused.
  - New inputs reach the replayed tasks and are refused by the start check when invalid.
  - The label and `attempt_number` are set on a whole-execution replay too.
  - A replay of a replay reads outputs two executions back (D4).
  - Restart on another source: a new execution where the failed task is reset and the successful ones are kept.
- **Routes** (`tests/replay_routes.rs`, new):
  - every refusal code of D5, D8, D9 and D11, each with a control call that is accepted;
  - `workflow:read` is refused;
  - the audit fields;
  - the `revision=N` alias.
- **Frontend:** Vitest for the dialog's source choice and inputs form, and for the task-run menu item.
- **Gates:** the executor ratchet and census stay unchanged.
- **Live check** in a browser on a native `egret serve`: replay from a task, replay on the current source after an edit, restart on another source.

## 5. Recorded divergences from Kestra

1. Replay and restart on another source refuse a task that is missing from that source (D5). Kestra lets the executor fail.
2. A task run inside a `Loop` iteration is replayed from the root's `Loop` run (D9). Kestra replays the iteration's own sub-execution.
3. The "revision" can also be the flow's current draft (Q1). Kestra's revisions are every save; egret's are published versions plus the draft.
4. Breakpoints are refused until their own package (Q3).
5. Replay and replay-with-inputs are one route (D11).

Security-driven difference, carried from 7a D24: the `egret.sql` creator gate checks the caller against the source that will run (D8).

## 6. Out of scope

- Breakpoints (Q3).
- Bulk replay and restart by ids or by query: row #7's bulk operations, later in 7a–7f.
- Subflow `NEW_EXECUTION` restart: roadmap row #12.
- Pause by hand and resume with `onResume` inputs: 7c.
