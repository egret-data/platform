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

- **Q4–Q7 (2026-11-14, after the plan's investigation; see the plan's "Spec corrections").**
  - **Q4 (S1):** a replay from a task drops the `errors:` runs of every scope it climbs through, so the chosen task really runs again. Kestra keeps them, and its executor then stays in error mode.
  - **Q5 (S2):** a restart on another source carries the kept runs' outputs, as a replay from a task does. Kestra's restart with a revision carries none.
  - **Q6 (S4):** a `Loop` run reset in a copy goes `CREATED`, and its iterations run again under the new execution. Kestra fails such a `Loop` at once.
  - **Q7 (S7):** a replay from a task and a restart on another source also refuse `files_pruned` and `after_execution_running`, as 7a's restart does. A whole replay is unchanged.

- **Q8 (2026-11-14, during A3). Variables follow the chosen source, as in Kestra.** When the chosen source text differs from the frozen source, the new execution stores NULL `variables`, so that source's own `variables:` apply (Kestra `applyNewRevision`, `ES:514-517`). On the frozen source the old execution's `variables` are carried unchanged. The plan had said the old column always travels; the user chose the Kestra-faithful rule after the A3 review (finding I1). This is the dated form of D6's last sentence. Recorded as §5 divergence 12.

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

Confirmed by the user on 2026-11-14 (Q4 to Q7 and the plan's "Spec corrections"):

6. **Replay from a task drops the `errors:` runs of every scope it climbs through (S1, Q4).** Kestra's `GraphUtils.successors` (`GraphUtils.java:144-181`) keeps them, except in three cases (since Kestra `7a48d1dca`, #6959, `:157-169`): the chosen run is the flow's first top-level task, a flowable, or the first task of an `errors:` list. Where Kestra keeps them, its executor stays in error mode (`Execution.java:595-606`) and the chosen task never runs again. egret drops the errors runs of the scope, with their subtrees, when the chosen run or a climbed ancestor sits in that scope's `tasks`. The result differs from Kestra's only where Kestra's result cannot run. The `finally:` and `afterExecution:` runs are reached exactly as in Kestra (`GraphUtils.java:365-399`, `:413-421`, `:488-489`).
7. **A restart on another source carries the kept runs' outputs (S2, Q5).** Kestra's restart with a revision copies none (`ES:316-331` has no `copyOutputs` call; only replay does, `ES:425`; `TaskRun.forChildExecution`, `TaskRun.java:193-208`). In Kestra a later task that reads a kept output fails. egret shares the replay's copy machinery.
8. **A reset `Loop` run in a copy goes `CREATED` and spawns every iteration under the new execution (S4, Q6).** Kestra finds no sub-executions of the new execution (`ES:985-991`) and fails the copied `Loop` at once (`ExecutorService.java:735-757`). This is what makes D9's "the root's replay from the `Loop` run gives the same result" true.
9. **The new execution's state history starts `CREATED`, then `RESTARTED`, then `RUNNING` (S10).** Kestra's copy carries the old execution's history and appends `RESTARTED` (`Execution.childExecution`, `Execution.java:405-432`; `ES:494-496`). egret keeps a fresh row's history, timestamps and outbox.
10. **Runs are picked on the plan the old runs ran under (S11).** Kestra's restart picks against the old execution's flow (`ExecutionCommandMessageHandler.java:90-94`); its replay builds the graph from the new revision's flow (`:176-187`). egret picks both on the old plan, because `RunIx` and `TaskIx` index into it, and D5's match then refuses a kept run that moved. A reordered list can therefore drop a run that Kestra would keep.

Found during the build:

11. **A copied `Switch` keeps its old case.** Kestra re-renders the `Switch` value on the new source. egret reopens the flowable but keeps the case the old run chose (A3 review, minor). A moved task in an `If` or `Switch` branch is refused by the branch check (`a0d0eb8be`).
12. **Variables follow the chosen source (Q8).** When the chosen source text differs from the frozen source, the new execution stores NULL `variables`, as Kestra's `applyNewRevision` takes the new flow's `variables` (`ES:514-517`). On the frozen source the old `variables` are carried.
13. **Replay with changed inputs replaces the inputs wholesale, as Kestra does.** An optional `SECRET` input left empty is dropped, not kept. The dialog says "Enter it again: the stored value is hidden." on a secret field. Without `inputs`, the old inputs are reused, revealed as today.
14. **The `files_pruned` refusal follows the `replayed_from` chain only.** The query in `worker/copy.rs` (`OLD_ROW`, recursive `chain`) walks `replayed_from_execution_id`, not a retry replay's `parent_execution_id`. A source whose files the pruner deleted behind a retry-parent link is not refused. Kestra has no such refusal; this is a gap in egret's own gate, to confirm at the final review.

Security-driven differences, carried from 7a D24 and extended:

- The route needs `workflow:manage`.
- The `egret.sql` creator gate judges the caller against the source that will run (D8). With `current` or `version:N`, a non-creator cannot run a version that adds an `egret.sql` task. It is judged inside the transaction, on the text that runs.
- A trashed or purged flow answers 404 and a disabled one 409 (`startable_flow`). Kestra has no trash.
- A chosen source (`current` or `version:N`) whose own YAML says `disabled: true` is refused 409, as Kestra's `FlowService.getFlowIfExecutableOrThrow(.., revision)` refuses a disabled revision (`FlowService.java:861`). The 409 has no code.
- `files_pruned` and `after_execution_running` apply to a replay from a task and a restart on another source (Q7), not to a whole replay.

## 6. Out of scope

- Breakpoints (Q3).
- Bulk replay and restart by ids or by query: row #7's bulk operations, later in 7a–7f.
- Subflow `NEW_EXECUTION` restart: roadmap row #12.
- Pause by hand and resume with `onResume` inputs: 7c.

## 7. As built

Built 2026-11-14 on `feat/workflow-replay-from-task`, base `origin/main` `844ec47db`. No migration (D10 held: 0225 already allows `attempt_number > 1`; the top migration stays 0225). The executor ratchet at T0 was **301 / 25 / 25 / 0** and the census **295 / 25 / 21**. The ratchet and census at the end are unchanged: **301 / 25 / 25 / 0** and **295 / 25 / 21** (`--test executor`, 62 passed, on `4b6bf572d`).

| Task | Commits | What landed | Fix rounds |
|---|---|---|---|
| Spec, plan | `d1cc6c09c`, `d466cba59`, `e2d20e35d` (Q4 to Q7) | This spec, its plan, and the user's decisions after the plan's investigation. | – |
| T0 | – | Containers, baseline ratchet and census, measured on `e2d20e35d`: 301/25/25/0, executor tests 62/62. | – |
| A1 | `8e1f7244b`, `604976df0`, fix `8ea71269e` | Shared test drive (`executor/test_support.rs`); pure `replay_from` and `restart_copy` in `executor/replay.rs`, with S1's `scope_errors`. | 1: the root rule fired for a flowable first in `errors:`; Kestra adds only the flowable's own root |
| A2 | `c44e50904` | Pure `copy_onto` (dense renumbering, match by id, parent, list and kind), `shift_ticks`, `reset_copied` (a `Loop` goes `CREATED`, S4). | 0 |
| A3 | `4193a282a`, `7ae1ec7f5`, fixes `a0d0eb8be`, `3045e0f75` | Worker layer: `insert_execution` split from the start's first settle; `worker/copy.rs` `replay_execution` (whole, from a task, restart on a source); `Op::Replay`, the `system.replay` label, `attempt_number` + 1. | 1: variables follow the chosen source (Q8); a kept task moved to another `If` or `Switch` branch is refused |
| A4 | `40703a80c` | Integration tests for restart copies of a failed loop, a failed subflow and a paused execution, replay of a replay, and an `afterExecution` replay. No code change. | 0 |
| A5 | `94066375d` | The storage read rule adds `chain_family`: a copy reads its source chain's loop iteration files (S5). One `storage_index` test that pinned the old exclusion was flipped. | 0 |
| B1 | `563e1a44c`, fix `edea15b2b` | Routes: `taskRunId`, `source`, `revision` alias, `breakpoints` refusal, JSON and multipart inputs; coded refusals; audit; OpenAPI; the retired `restart_revision_unsupported` (S8). | 1: the restart-on-source audit names the replaced execution; a disabled chosen source is 409 |
| C1 | `ff67795e5` (client), `249e4f74b`, fix `4ed0eab19` | `gen:api` client; `ReplayDialog.tsx` (source choice, change inputs, restart text); task-run row menu "Actions for {task}"; refusal sentences. | 1: replay hidden for a flow with no id gives a reason; input errors show in the collapsed section |

Measured counts (Z gates, 2026-11-14):

- `cargo fmt --check`, `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings` and `cargo check -p egret-bin`: clean.
- `cargo test -p egret-workflow --no-fail-fast` on `25fb40103`: 3782 passed, 55 ignored, 8 failed. Seven are `sql_starrocks_live` (no StarRocks engine on the test host, as on main). The eighth was `execution_timestamps::every_state_write_stamps_the_timestamps`: the guard did not know a verbatim copy. `4b6bf572d` taught it one (all of `state`, `history`, `started_at` and `ended_at` from one row alias of the same table), with self-check cases that a copy taking `state` from anywhere else is still refused; after it `execution_timestamps` 21/21, `replay` 29/29, `replay_routes` 20/20 and `restart` 34/34 passed.
- `cargo test -p egret-config -p egret-server -p egret-metastore`: 179 passed, including `committed_openapi_json_is_current` and the client digest.
- Frontend: `npm run typecheck` clean; `npm run lint` 0 errors (154 warnings, none in files this package touches); `npx vitest run` 8722 passed and 2 failed on 5 s timeouts under machine load, in `OutputsTree.test.tsx` and `ScheduleDetail.test.tsx` (not touched here); both files pass alone (79/79).
- Live check on a native `egret serve` (its own database and storage, port 18466), as `admin@egret.local`, light and dark: replay from task `b` on this run's version kept `a`'s outputs and reran `b` and `c` (`system.replay: true`, attempt 2); replay from `b` on the flow's current version ran the edited `c`; restart of a FAILED execution on published version 2 opened a new SUCCESS execution labelled `system.restarted: true`. The three audit rows carry `task_run_id`, `source`, `inputs_replaced` and `replayed_from`. The check found the row-menu icons unsized; `a4d6afaae` fixed them.

### Deviations and their reasons

- **Plan prose that the build corrected.** The plan said the old `variables` always travel; Q8 replaced it (A3 review I1). The plan's `role_of` folded `If` and `Switch` branches into one list, so a moved task became an orphan; a branch check refuses it (A3 review I2).
- **A1: a `LoopUntil` run keeps its count.** The count comes from the run's history, which the copy carries. Kestra keeps `iteration` too (`RestartCaseTest.restartOrReplayLoopUntil`).
- **A5 touched a test outside its file list** (`storage_index.rs`), because that test pinned the exclusion the task removes. The W9a spec carries a dated correction note.
- **C1 sends `inputs` only when a field was edited.** Otherwise the body is `{}` and the old inputs are reused. A `FILE` input with an old `kestra://` value and no new file keeps the old value.

### Review rounds

Every task was reviewed by the controller between tasks. A1, A3, B1 and C1 each needed one fix round, followed by a clean scoped re-review for B1, C1 and A3. A2, A4 and A5 were approved at once. A5's review was an authorisation review: the old exclusion was a W9a scope choice, not a boundary, and the schema confines the recursion to the same workspace's loop iterations.

### Deferred findings

- A2: `copy_onto`'s precondition (the kept set is closed under parents) is undocumented; test A2.8 stops at the command list; no test for a kept run moved inside a flowable's own `errors:` or `finally:` list.
- A3: `side_writes` takes the label value through a `debug_assert`; red tests for `AfterExecutionRunning`, `NotTerminal` with a task run, and the restart `NotRestartable` and `NothingToRestart` cases are missing; `metadata.attempt` is stale in the first settle; `origin_offset_ms` is not carried (Kestra keeps `originalCreatedDate`); the deadlock retry in `copy.rs` repeats 7a's.
- A4: the paused test does not drive the copy, so its "not run again" assertion is vacuous; `failing_nth_run_of` is not atomic; the `afterExecution` test checks no ordering.
- A5: the pruner wording at `storage/index.rs:175` suggests longer retention than it gives; a redundant assertion in `storage_index.rs`; no nested-iteration or subflow-negative test.
- B1: a non-frozen restart runs `startable_flow` twice; `refuse_disabled_source` parses the source twice; no 413 test on the replay route; `startable_flow` judges `disabled:` on the draft YAML only.
- C1: the dialog title flips while closing; a server-error path is untested; the `!hasFlow` reason text renders in replay mode.
- Owed at Z: the live browser check (light and dark, 7a's FE-TEST-2) with screenshots, and the bundle test.

## 8. Rest of row #7

See §2.1 of 7a's spec. Each sub-package gets its own spec, plan and PR.
