# Workflow package #7b "Replay from a task, on a chosen source, with new inputs": implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to carry out this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a person with `workflow:manage` can replay a finished execution from a chosen task run, on its frozen source, the flow's current draft or a published version, with its old inputs or new ones; and can restart a failed execution on another source as a new execution. The semantics are Kestra's (`ES:281-517`, `EC:1437-1507`) except where §"Spec corrections" and spec §5 record a divergence.

**Architecture:** the old execution's task runs are copied into a new execution, Kestra's "copy then reset" (spec D1).
- Two pure functions in a new `executor::replay` module choose the runs to keep and the runs to reset: `replay_from` (D2) and `restart_copy` (D3, which reuses 7a's `executor::restart`).
- A third pure function, `copy_onto`, renumbers the kept runs densely and maps each one to the target source's plan by task id (D5).
- A fourth, `reset_copied`, resets the copied runs with 7a's per-kind rules.
- The worker (`worker::copy`) inserts the new execution row through the start's own row writer, factored out of `start_execution_in_on`. It then inserts the copied rows, with outputs carried and leases cleared, and runs the result as 7a's `Job::Restart`, so the side writes and the settle are 7a's own.
- A whole-execution replay keeps today's start path and gains the label and `attempt_number` (D10).
- The routes gain `taskRunId`, `source` / `revision`, `breakpoints` (refused) and an optional JSON or multipart body.
- The storage read rule gains the replay chain's loop iterations (§S5).
- The frontend gets one dialog for Replay, Replay from this task and Restart.

**Tech stack:** Rust (axum, sqlx 0.9, Postgres, utoipa), `crates/egret-workflow`; React + orval client in `frontend/`.

Spec: `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`. Its Q1–Q3 and D1–D14 are binding. Where this plan finds a spec assumption wrong, it says so under "Spec corrections" and does **not** change the decision silently; items S1, S2, S4 and S7 are divergences or additions the controller must put to the user before B1 merges. Branch `feat/workflow-replay-from-task`, one PR. Base `origin/main` `844ec47db`.

Tasks: T0, A1–A5 (engine), B1 (routes), C1 (frontend), Z (docs, gates, live check).

**Review is not a task of this plan.** The controller reviews each task between tasks and runs one whole-branch review after Z. Fixes are recorded in spec §7 "As built" (to be created at Z, as 7a's) as fix waves.

---

## Spec corrections

Each item is a place where the spec's assumption is wrong or incomplete, with the evidence. The plan implements the line marked **Plan**; the user confirmed S1, S2, S4 and S7 on 2026-11-14, each as the line marked **Plan**.

- **S1 (D2: what Kestra's successors include).**
  - Kestra's `GraphUtils.successors` (`GraphUtils.java:144-181`) walks the flow graph from the chosen run's node. It **always** reaches the `finally:` and `afterExecution:` runs of every scope it climbs through: a list's last node links to its cluster's finally node (`GraphUtils.java:488-489`), the finally chain links to the afterExecution node, then to the end (`:413-421`, `:365-399`).
  - It reaches `errors:` runs **only through a cluster root**, because an error branch hangs off its cluster's root (`root → e1`, `:468-472`). Since commit `7a48d1dca` (Kestra #6959, `:157-169`), the walk also starts from the root that has an edge straight to the chosen node. That happens in three cases:
    - the chosen run is the flow's **first** top-level task (the top graph has no task node, so `root → t1`, `:410`);
    - the chosen run is a **flowable**, whose own root links to its task node (`GraphCluster.java:74`), so its own errors are dropped;
    - the chosen run is the **first task of an `errors:` list**.
  - In every other case Kestra **keeps** the errors runs of the scopes the chosen run sits in. In Kestra's executor a kept errors run puts the list in error mode (`Execution.java:595-606`: `errorsFlow` not empty, no failed task, so it returns nothing), and the chosen task is never run again. egret's walk does the same (`executor/flowable/mod.rs:215-260`, `errors_started`). This is the "roadmap-note-7 regression" that 7a A1 test 2 guards against.
  - **Plan:** port Kestra's successor walk faithfully (A1 `successors`). Add one divergence: when the chosen run, or an ancestor the walk climbs through, sits in scope P's **`tasks`** (any branch), P's `errors:` runs, with their subtrees, are dropped too. Kestra's own #6959 comment (`GraphUtils.java:161-163`) states this intent. The result differs from Kestra's only where Kestra's result cannot run. It is recorded as spec §5 divergence 6. If the user rejects it, delete `scope_errors` in A1 and flip test A1.6.
- **S2 (D1, D3: outputs on a restart on another source).**
  - Kestra's restart with a revision does **not** copy outputs. `ES:316-331` has no `taskOutputService.copyOutputs` call (only replay has one, `ES:425`). `TaskRun.forChildExecution` (`TaskRun.java:193-208`) carries no outputs, which since Kestra 2.0 live in the output repository (`TaskOutputService.java:288-294`, `TaskRun.java:80-86` deprecated).
  - So in Kestra a kept task's outputs are gone in the restarted copy, and a later task that reads them fails.
  - **Plan:** carry outputs for both, as spec D1 ("shares 7b's copy machinery") says. Recorded as spec §5 divergence 7.
- **S3 (D5: "runs that are reset or dropped need no match").** This holds for dropped runs only.
  - In egret every copied row stores a `task_ix` of the plan it runs under (`workflow.task_runs.task_ix`; `claim.rs:4826` loads it as `TaskRun::task`). A reset run (the chosen run, its ancestors, an unfinished sibling) is a copied row too.
  - The run's parent link must also agree with the target plan: the walk finds a run by `(parent run, task)` (`flowable::current_list`, `run_of`).
  - **Plan:** every copied run must match a task of the chosen source with the same id, the same parent task, the same list (`tasks` / `errors` / `finally` / `afterExecution`) and the same kind. Otherwise the request is refused with 400 `replay_task_not_in_source`, and the message names the task id and says which of the three failed. Same code, wider check; no decision changes.
- **S4 (D9: a `Loop` run in a copy).**
  - 7a resets a `Loop` run by reopening it and restarting its last failing iteration **in place** (`worker/restart.rs:376-443`, 7a D8). In a copy, the iterations belong to the old execution (`parent_execution_id` = old id), so there is nothing to restart in the new one.
  - Kestra fails the copied `Loop` at once. It looks up sub-executions of the new execution (`ES:985-991`), finds none, and marks the loop `FAILED` (`ExecutorService.java:735-757`). So in Kestra a replay from a `Loop` run fails immediately, and D9's "the root's replay from the `Loop` run gives the same result" does not hold there.
  - **Plan:** in a copy, a reset `Loop` run goes `CREATED` and spawns every iteration under the new execution (A2 `reset_copied`). This is what makes D9's sentence true. Recorded as spec §5 divergence 8. The same applies to a restart on another source (D3).
- **S5 (D4: the read rule's depth).**
  - The rule is in `storage/index.rs:114-157` (`readable_sql!`). Its `chain` CTE is `WITH RECURSIVE` over `replayed_from_execution_id` (and `parent_execution_id` of a retry replay). It is transitive, bounded by the chain's length (the `UNION` dedups).
  - So a replay of a replay **already** reads outputs two or more executions back. No fix for depth is needed; A4 test 9 pins it.
  - There is a gap: the chain's own loop iterations are excluded (`index.rs:170`, "The chain's own loop iterations are NOT in the set"). A kept `Loop` run's copied outputs can name files under the old execution's iterations, and those would be unreadable from the copy.
  - **Plan:** A5 adds the loop family of every chain member, red test first. If the red test is green before the change, A5 stops and reports.
- **S6 (D6: "current is read inside the replay's transaction").**
  - A multipart replay uploads its files before any transaction opens (`routes.rs:2970-3073`, the start's order). It must know the chosen source's declared inputs, namespace and id to do so. So the route reads the source text once, before the transaction.
  - **Plan:** everything that judges that source happens inside the transaction, on that same text: `sql_principal::check_start` (in the start's row writer, `claim.rs:4341`), the input check, the plan build and D5's match. The gate is not weakened.
- **S7 (D8: gates for a copy).**
  - Today's replay checks no files and no `afterExecution:`. A copy carries the old runs' `kestra://` URIs and must not copy live runs.
  - **Plan:** a replay from a task and a restart on another source also pass 7a's D10 refusals:
    - `files_pruned` (409), for the source execution or any execution in its replay chain;
    - `after_execution_running` (409).

    A whole-execution replay is unchanged.
- **S8 (D11: the field being retired).** `RestartQuery.revision` is `Option<String>` and any value answers `restart_revision_unsupported` (`routes.rs:3587`, `:3705-3713`, `:3788-3794`). Its test is `tests/restart_routes.rs:253`. Its generated client comment is in `frontend/src/api/generated/model/workflowRestartExecutionParams.ts:13`. B1 removes all three.
- **S9 (D14: the frontend's shape).**
  - The execution page has **no i18n**: every string is inline English (`routes/workflows/ExecutionDetail.tsx`, `ExecutionOverview.tsx`); `package.json` has no i18n library. **Plan:** inline English, as 7a's actions.
  - There is **no task-run menu**: 7a added a per-row ghost button "Change state" (`ExecutionOverview.tsx:415-421`). **Plan:** the row's action cell becomes a `DropdownMenu` (`components/ui/dropdown-menu.tsx`) labelled "Actions for {task}", with "Change state" and "Replay from this task".
- **S10 (D2: the new execution's history; Kestra fact).**
  - Kestra's copy carries the old execution's state history and appends `RESTARTED` (`Execution.childExecution`, `Execution.java:405-432`, with `execution.withState(RESTARTED).getState()`, `ES:494-496`; `ExecutionServiceTest.replaySimple` asserts 4 entries).
  - egret's new row is born `CREATED` (`claim.rs:4374-4378`), then `RESTARTED`, then `RUNNING`.
  - **Plan:** keep egret's history (the row's timestamps and outbox are a fresh row's). Recorded as spec §5 divergence 9.
- **S11 (D2/D3: which plan picks the runs; Kestra fact).**
  - Kestra's restart picks the runs to reset against the old execution's flow (`ExecutionCommandMessageHandler.java:90-94`: `findByExecutionForRuntime`).
  - Its replay builds the graph from the **new** revision's flow (`ExecutionCommandMessageHandler.java:176-187`).
  - **Plan:** egret picks both on the plan the old runs ran under. Their `RunIx`/`TaskIx` are indexes into it, and S3's match then refuses any kept run that moved. A reordered list can therefore drop a run Kestra would keep; recorded with divergence 6.

---

## Investigation record (facts the implementers must not re-derive)

### Kestra
- `ES` = `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `EC` = `webserver/.../ExecutionController.java`, Kestra `96893b58`.
- **Restart** `ES:281-373`.
  - `canBeRestarted`, else `IllegalStateException`.
  - `newExecutionId` only with a revision (`:289`).
  - An execution with no task runs gets a fresh child (`:293-309`).
  - `taskRunToRestart(canBeRestarted)` (`:311-314`), `mapTaskRunId(keep = revision == null)` (`:316`), `mapTaskRun` (`:318-331`).
  - It removes: worker-task children (`:335-337`), `allErrorsWithChildren` (`:339-342`), `allFinallyWithChildren` (`:344-347`), `afterExecution` (`:349-351`).
  - Then `childExecution(.., RESTARTED)`, the label `system.restarted`, `nextAttempt()`, `withFlowRevision`.
- **`taskRunToRestart`** `ES:375-390`: the predicate's runs plus their ancestors (`taskRunWithAncestors`, `ES:1258-1263`, `getAncestors` `ES:1202-1212`). It throws "No task found to restart execution from!" when the set is empty.
- **Replay** `ES:393-512`.
  - `findExecutionWithTaskRun` descends into loop sub-executions (`:393-401`, `ES:569-583`).
  - `GraphUtils.of(flow, execution)` (`:410`), the reset set (`:412-415`), new ids for all runs (`:417`).
  - `mapTaskRun` plus `copyOutputs` per run (`:419-433`).
  - Successors minus the chosen run minus the reset set are removed (`:435-454`).
  - Runs that had not ended (not in the reset set, not removed) are re-mapped with `onRunningResend` and reset (`:456-484`).
  - `removeWorkerTask` (`:486-488`).
  - State: `CREATED` for a whole replay, else old history plus `RESTARTED` (`:491-496`).
  - Label `system.replay` (`Label.java:29`) and `nextAttempt()` (`:497-501`).
  - `applyNewRevision` takes the new flow's `variables` (`ES:514-517`).
- **`mapTaskRun`** `ES:1227-1256`.
  - A leaf, `WorkingDirectory`, `LoopUntil` or `Loop` takes the new state (`RESTARTED`). Any other flowable takes `RUNNING`.
  - Only runs in the reset set change state; the others keep theirs (`forChildExecution(.., null)`).
- **`removeWorkerTask`** `ES:1179-1200`: for each `WorkingDirectory` in the reset set, its successors except itself, that is, its children.
- **Revision flows.** Restart: `executorContext.getFlow()`, the execution's own flow (`ExecutionCommandMessageHandler.java:90-94`). Replay: the revision's flow when given (`:176-187`).
- **Inputs on replay.** `readExecutionInputs(flow, current, inputs)` against the revision's flow (`EC:1497`). The run must be in the (target) execution, else `IllegalArgumentException` (`EC:1502-1507`).
- **Subflow restart lookup** happens only under the label `system.restarted` (`ExecutableUtils.java:90-150`), so a replayed `Subflow` run spawns a new child.

### egret

- **Task runs** (`\d workflow.task_runs`, migrations 0134, 0136, 0137, 0141, 0183, 0193, 0194, 0197, 0214).
  - Identity and position:
    - `id` (uuid, default `gen_random_uuid()`);
    - `execution_id`;
    - `run_ix` (`UNIQUE (execution_id, run_ix)`; must equal the run's position, `claim.rs:4812-4824`, else `Corrupt`);
    - `task_ix`, `task_id`, `type_id` (NULL unless `TaskKind::Runnable`, `claim.rs:5546-5549`);
    - `parent_ix`, `value`, `rendered`, `rendered_run_if`.
  - State: `state`, `history` (jsonb), `attempts` (jsonb `[{history, started_at, ended_at}]`; ticks are milliseconds after the **execution's** `created_at`, `routes.rs:1665-1674`, parsed at `claim.rs:5707-5746`).
  - Timing: `not_before_ms` (tick), `created_seq`, `ended_seq` (the execution's logical clock `executions.seq`, `claim.rs:4910-4985`), `started_at`, `ended_at` (timestamptz; CHECK `ended_only_when_terminal`, `ends_after_it_starts`).
  - Lease: `claimed_at` (NULL means unclaimed; claim takes only NULL).
  - Results: `outputs`, `message`, `secret_outputs` (sealed with no associated data, `secret_inputs.rs:315-326`, so a copy decrypts), `outputs_bytes`.
  - Pause: `resume_with` (CHECK pairs it with `not_before_ms`).
  - Working directory: `working_dir`, `working_dir_snapshot`, `working_dir_swept_at`. The `_wd/{block run id}/` snapshots are swept by `storage/wd_sweep.rs:168-174` for unstamped ended blocks.
- **The writer** (`persist`, `claim.rs:5446-5600`) INSERTs a new `run_ix` and UPDATEs a changed one. `states_json` (`claim.rs:5621`) and `attempts_json` (`claim.rs:5630`) serialise. It never touches `outputs` or `message`.
- **Loading.** `load`/`load_with` (`claim.rs:4746-4878`) lock the execution row (`FOR NO KEY UPDATE`). `ExecutionState` is `{runs, history, metadata{attempt, origin}}` (`executor/state.rs:93-111`); `TaskRun` is `executor/state.rs:114-160`.
- **Emptied runs.** A run with an empty history is "removed from `taskRunList`" (`command.rs:224-240`). `current_list` reads it as not started (`flowable/mod.rs:225-229`).
- **The start.**
  - `NewExecution` is `claim.rs:4126-4191`. It is built at 27 sites: 9 in `src` (routes ×2, `scheduler/fire.rs`, `flow_trigger/process.rs`, `claim.rs` ×5) and 18 in tests. **This plan adds no field.**
  - `start_execution_in_with` (`claim.rs:4222-4228`) leads to `start_execution_in_on` (`claim.rs:4297-4452`), which does:
    - parse, `build_plan`, the served check;
    - `sql_principal::check_start(conn, &flow, flow_id, source, started_by)` (`:4341`);
    - the input check, **skipped when `replayed_from_execution_id` is set** (`:4357-4363`);
    - `secret_inputs::split`, the INSERT (`:4367-4400`; row born `CREATED`);
    - `check_file_inputs`, `index_input_files`;
    - `settle` on an empty state at `Tick(0)`.
  - `checked_inputs` (`claim.rs:4243-4249`) is `pub(crate)` and re-exported (`worker/mod.rs:117`).
- **Today's replay route** is `routes.rs:3240-3411`:
  - loop-iteration 400 (`:3312-3320`);
  - flow NULL means 404 (`:3339-3342`);
  - `startable_flow` (`:3343`; defined `:2431-2458`: 404 trash, 400 unparseable draft, 409 disabled);
  - terminal or 409 (`:3348-3353`);
  - `secret_inputs::reveal` (`:3356-3359`);
  - `start_execution_in_with` with `replayed_from_execution_id: Some(id)`, labels copied, variables copied, `started_by: Some(caller)` (`:3362-3392`);
  - audit `workflow_execution:replay` `{replayed_from, replacement}` (`:3393-3401`);
  - answers 201 (`:3402-3410`).

  Its router line is `routes.rs:4474-4477`. A test asserts today's labels exactly: `tests/executions_routes.rs:2100-2146` (`a_replay_copies_the_labels_and_not_the_backfill`); B1 updates it for D10.
- **The restart route** is `routes.rs:3755-3816`.
  - `RestartQuery` is at `:3705-3713`.
  - `rerun_gate` (`routes.rs:2474-2504`) runs `startable_flow` and `check_start` on `detail.source`.
  - `ExecOpError` (`:3589-3703`) maps a coded refusal to 409 or else 400. B1 adds 404.
  - Router line `:4482-4485`; OpenAPI registration `:4524-4526`.
- **The start route's bodies.**
  - JSON: `routes.rs:2642-2684` (2 MiB limit, a headerless body read as JSON, `start_inputs` at `:2510-2524`).
  - Multipart: `start_multipart` `:2935-2997`, `read_parts_and_start` `:3000-3099`, `read_text_part` `:3103+`.
  - The route disables the default body limit (`routes.rs:4289-4295`).
  - Types: `StartExecutionRequest` `:2353`, `StartExecutionMultipart` `:2539-2563`.
- **7a's restart.**
  - Pure side: `executor/restart.rs`.
    - `restart` (`:68-116`) and `mark_as` (`:141-217`);
    - private `kind` (`:223`), `ancestors` (`:241-254`), `end_hook_runs` (`:261-275`), `empty` (`:278`), `reset` (`:288-322`), `to_created` (`:324`), `reopen` (`:333-340`);
    - test helpers `drive`, `drive_from`, `driven`, `last`, `is_terminal`, `run`, `leaf`, `flow` (`:349-470`).
  - Worker side: `worker/restart.rs`.
    - `restart_execution` (`:84-99`, deadlock retry), `Nested` (`:107-119`), `Prepared`/`Op` (`:131-148`);
    - `prepare` (`:205-246`), `lock` (`:259-290`, run rows `FOR UPDATE`), `d10_refusal` (`:323-338`);
    - `open` (`:362-479`): loops, apply, `side_writes`, `Frame::loaded(..).without_sla()`;
    - `side_writes` (`:485-582`): `attempt_number + 1`, label `system.restarted`, clear outputs and lease on `reset ∪ emptied` minus `loops`, renew emptied ids.
  - The job: `claim::Job::Restart` (`claim.rs:3265-3285`), run by `claim::run_job` (`claim.rs:3135`).
- **Sources.**
  - Draft: `workflow.flows.source` (`FlowRepo::get`, `repo.rs:518`).
  - Published version: `workflow.flow_versions.snapshot->>'source'`. `publish.rs:352-356` writes `{"source", "plan", "warnings"}`; `routes.rs:1376-1391` reads it; `sql_principal.rs:68` compares it.
  - The repo read is `FlowRepo::get_version(ws, flow, version)` (`repo.rs:893-911`), scoped, `NotFound("flow version")`.
- **The `egret.sql` gate.** `sql_principal::check_start` (`sql_principal.rs:90-113`): with `started_by = Some(caller)`, only the flow's creator passes on a flow that runs `egret.sql`. Since the copy goes through the start's row writer with `started_by: Some(caller)` and the **chosen** source text, D8's gate holds for `current` and `version:N`.
- **Storage read rule** (S5). `readable_executions` is `storage/index.rs:172-183`; the SQL is `readable_sql!` `:114-157`. The pruner protects the inverse for every non-idle execution (`prune.rs:25-38`, late check `:615-627`).
- **Labels.** The column is `[{"key","value"}]`, read by `labels::own_labels`. Only the engine writes `system.*`.
- **Executor ratchet and census**, measured by 7a and unchanged at its end: 301/25/25/0 and 295/25/21 (7a spec §7).
- **Migration.** None is needed:
  - D10's `attempt_number > 1` on any row is allowed since 0225;
  - the copy writes only existing columns;
  - A5 changes a query, not a schema.

  The highest migration on `origin/main` is `0225` (`git ls-tree --name-only origin/main migrations/ | tail -1`). If a migration becomes necessary, flag it and choose its number against main and the open PRs before push (memory: migration numbers are a shared namespace).
- **Frontend.**
  - Actions: `components/workflows/executions/actions.ts` (`showsReplay`/`canReplay` `:52-73`, restart `:155-162`, `restartRefusal` `:185-199`).
  - `ChangeStateDialog.tsx`, `useExecutionAction.ts`.
  - `ExecutionDetail.tsx`: `ExecutionActions` around `:358-570`; the Replay `ConfirmAction` `:544-552`; `runReplay` `:430-436`.
  - `ExecutionOverview.tsx` `TaskRuns` `:361-435`.
  - Input form: `components/workflows/run/RunFlow.tsx` and `inputModel.ts` (`inputsFromSource`, `formFromInputs`, `requestFromForm`, `validateForm`, `serverErrorsByInput`) and `inputControls.tsx` (`InputField`).
  - Generated hooks: `useWorkflowListFlowVersions`, `useWorkflowGetFlowVersion`, `useWorkflowGetFlow`.
  - `npm run gen:api` = `orval && node scripts/encode-path-params.mjs && node scripts/write-api-digest.mjs`; never bare `orval`.

---

## Global constraints

- Spec Q1–Q3 and D1–D14 are binding, as amended only by the Spec corrections above (S1, S2, S4 and S7 confirmed by the user 2026-11-14). When the spec and egret's code disagree in a way not listed above, **stop and report**; do not improvise. Kestra source: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` at `96893b58` (read-only).
- **Code search:** CodeGraph first, `codegraph_explore` with `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Use grep only for literal strings. `rg` is not installed.
- **Environment:**
  - `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target`
  - `export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test` (container `egret-execops-postgres`)
  - `export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465` (container `egret-execops-valkey`)
  - **Never run two cargo commands at once.** Run `df -h` before a full build; clean `target/debug/incremental` rather than starting a second target dir.
- **Off-limits:** never touch port 5432 or the `egret-dev-*` / `egret-demo-*` containers. Never commit `.superpowers/` or scratch files.
- **Commits:** Conventional Commits, each message ending with
  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
- **No network for subagents:** no `git push`/`fetch`/`pull`, no `gh`, no `npm ci`. The controller runs `npm ci` / `npm run gen:api` when C1 needs them.
- **Test files:** every new `#[sqlx::test]` test binary calls `egret_test_db::assert_database_url_is_exported()` in its setup, **and** has
  ```rust
  #[test]
  fn database_url_is_exported_rather_than_read_from_dotenv() {
      egret_test_db::assert_database_url_is_exported();
  }
  ```
  CI enforces this; 7a missed it once. This plan's new binaries are `tests/replay.rs` and `tests/replay_routes.rs`.
- **Red first:** the test that shows the gap exists before the fix.
- **Tests per task:** each task runs only the tests it touches. The full suites run once, at Z.
- **The executor ratchet and census must not move:** **301 / 25 / 25 / 0** and census **295 / 25 / 21**, re-measured at T0. Any change is a regression to report, not a bank.
- No new `std::env::var`. If one becomes necessary, add it to `egret-config`'s `ENV_OUTSIDE_CONFIG` and run `cargo test -p egret-config`.
- **Words in code and messages.**
  - New refusal codes, exactly:
    - `invalid_source` (400);
    - `replay_breakpoints_unsupported` (400);
    - `replay_task_not_in_source` (400);
    - `flow_version_not_found` (404);
    - `task_run_not_found` (404).
  - Reused: `inside_working_directory`, `not_restartable`, `nothing_to_restart`, `after_execution_running`, `files_pruned`.
  - Retired: `restart_revision_unsupported`.
  - Labels: `system.replay` / `"true"` (replay) and `system.restarted` / `"true"` (restart on another source).
  - Audit actions: `workflow_execution:replay` and `workflow_execution:restart`.
  - Source spellings: `frozen`, `current`, `version:N` (N ≥ 1).

---

## File structure

| File | Change | Task |
|---|---|---|
| `crates/egret-workflow/src/executor/test_support.rs` | **new** (`#[cfg(test)]`): 7a's drive helpers moved out of `restart::tests` | A1 |
| `crates/egret-workflow/src/executor/replay.rs` | **new**: `CopySelection`, `CopyRefusal`, `MatchFault`, `replay_from`, `restart_copy`, `successors`, `scope_errors`; then `CopiedRuns`, `copy_onto`, `shift_ticks`, `reset_copied`; tests | A1, A2 |
| `crates/egret-workflow/src/executor/restart.rs` | `kind`, `ancestors`, `reset`, `to_created`, `is_seed` → `pub(super)`; tests import `test_support` | A1 |
| `crates/egret-workflow/src/executor/mod.rs` | `pub mod replay;`, `#[cfg(test)] pub(crate) mod test_support;`, re-exports | A1, A2 |
| `crates/egret-workflow/src/worker/claim.rs` | `insert_execution` factored out of `start_execution_in_on`; `attempts_json` → `pub(super)`; `settle_new` | A3 |
| `crates/egret-workflow/src/worker/restart.rs` | `Op::Replay`, `copy_job`, `with_label`, `REPLAY_LABEL_KEY`/`VALUE`, `side_writes` keyed by op | A3, A4 |
| `crates/egret-workflow/src/worker/copy.rs` | **new**: `ReplayRequest`, `ReplayKind`, `ReplayInputs`, `ReplayRefused`, `replay_execution`, `insert_runs` | A3, A4 |
| `crates/egret-workflow/src/worker/mod.rs` | `mod copy;` and re-exports | A3 |
| `crates/egret-workflow/tests/replay.rs` | **new** worker-level integration tests | A3, A4, A5 |
| `crates/egret-workflow/src/storage/index.rs` | `readable_sql!`: the chain's loop families | A5 |
| `crates/egret-workflow/src/repo.rs` | `ExecutionRepo::loop_run_holding` | B1 |
| `crates/egret-workflow/src/routes.rs` | `SourceChoice`, `ReplayQuery`, `RestartQuery` (new fields), `ExecOpError` 404 arm and `from_replay_refusal`, `read_json_body`, `receive_parts`/`discard_uploads`, `replay_execution` and `restart_execution` rewritten, `rerun_gate` takes the source text, OpenAPI, router layer | B1 |
| `crates/egret-workflow/tests/replay_routes.rs` | **new** route tests | B1 |
| `crates/egret-workflow/tests/restart_routes.rs`, `tests/executions_routes.rs` | the `revision` test and the replay-labels test updated | B1 |
| `frontend/src/api/generated/**` | `npm run gen:api` (controller) | C1 |
| `frontend/src/components/workflows/executions/actions.ts` (+ test) | `SourceChoice`, `sourceParam`, `replayRefusal`, `restartRefusal` additions, `showsReplayFromTask` | C1 |
| `frontend/src/components/workflows/executions/ReplayDialog.tsx` (+ test) | **new**: source choice, inputs section, both modes | C1 |
| `frontend/src/routes/workflows/ExecutionDetail.tsx`, `ExecutionOverview.tsx` (+ tests) | dialogs wired; row menu | C1 |
| `docs/en/specs/2026-11-14-…-design.md` §5, §7; roadmap; `docs/vi/...` | docs | Z |

---

## T0. Environment and baseline

- [ ] Check the containers: `docker ps --format '{{.Names}} {{.Ports}}' | grep egret-execops` must list `egret-execops-postgres` on 5465 and `egret-execops-valkey` on 6465. Both were up on 2026-11-14. If either is missing, start it as 7a's T0 did (image `postgres:16`, user/password/db `egret`/`egret`/`egret_test`, host port 5465; Valkey on 6465).
- [ ] Export the three variables (Global constraints).
- [ ] Run the ratchet and census: `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t0-executor.txt"`. Read the tuple from the file, not from a `tail` (memory: `tail` truncates the report).
- [ ] Re-read migration numbers: `git ls-tree --name-only origin/main migrations/ | tail -1` (expect `0225_workflow_restart_attempt_number.sql`).
- **Accept:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; highest migration 0225. Report all three as measured. No commit.

---

## A. Engine

### A1. The pure selection: `replay_from` and `restart_copy` (spec D2, D3; S1, S11)

**Files:**
- Create: `src/executor/test_support.rs`, `src/executor/replay.rs`.
- Modify: `src/executor/restart.rs`, `src/executor/mod.rs`.

**Interfaces.**

Consumes:
- `executor::restart::{restart, RestartCommands, RestartRefusal}`;
- the private helpers made `pub(super)`:
  ```rust
  pub(super) fn kind<'a>(plan: &'a Plan, state: &ExecutionState, run: RunIx) -> Option<&'a TaskKind>;
  pub(super) fn ancestors(state: &ExecutionState, run: RunIx) -> Vec<RunIx>;
  pub(super) fn reset(plan: &Plan, state: &ExecutionState, run: RunIx, now: Tick, out: &mut RestartCommands);
  pub(super) fn to_created(run: RunIx, out: &mut RestartCommands);
  ```
- `state::is_descendant_of` (already `pub(super)`) and `Plan::children_of` (already `pub(super)`).

Produces (`executor/replay.rs`, re-exported from `executor`):
```rust
/// Why a copy cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    /// The chosen run is not a run of this execution (out of range, or emptied).
    NoSuchRun,
    /// `restart_copy`: the execution is not `FAILED` or `PAUSED` (7a's rule).
    NotRestartable(RunState),
    /// `restart_copy`: no run is `FAILED` or `PAUSED`.
    NothingToRestart,
    /// `replay_from`: the chosen run has a `WorkingDirectory` ancestor (D9).
    InsideWorkingDirectory,
    /// `copy_onto` (A2): a copied run's task does not fit the target source (D5, S3).
    TaskNotInSource { task_id: String, why: MatchFault },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchFault { Missing, Moved, OtherKind }

/// The runs a copy keeps, and the kept runs it resets, as indexes of the OLD state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopySelection {
    /// Ascending. Never an empty run, never a run whose parent is not kept.
    pub keep: Vec<RunIx>,
    /// A subset of `keep`.
    pub reset: std::collections::BTreeSet<RunIx>,
}

/// Spec D2 (Kestra `ES:393-512`, with S1's divergence).
pub fn replay_from(plan: &Plan, state: &ExecutionState, chosen: RunIx)
    -> Result<CopySelection, CopyRefusal>;

/// Spec D3 (Kestra `ES:311-351`): 7a's `restart` sets, as a selection.
pub fn restart_copy(plan: &Plan, state: &ExecutionState)
    -> Result<CopySelection, CopyRefusal>;
```

**Steps.**

- [ ] **Move the test helpers.** Create `src/executor/test_support.rs` holding, verbatim from `restart.rs:349-470`: `T0`, `NOW`, `LOG`, `leaf`, `flow`, `drive`, `drive_from`, `driven`, `last`, `is_terminal`, `run`. Make each `pub(crate)`. In `executor/mod.rs` add `#[cfg(test)] pub(crate) mod test_support;`. In `restart::tests` replace the moved items with `use crate::executor::test_support::*;`. Run `cargo test -p egret-workflow --lib executor::restart`: same count as before (20, 7a §7), all green. Commit: `refactor(workflow): share the executor's restart test drive`.
- [ ] **Write the failing tests** in `executor/replay.rs` `#[cfg(test)] mod tests` (code below). Run `cargo test -p egret-workflow --lib executor::replay`. They fail to compile; that is the red.
- [ ] **Implement** (code below).
- [ ] Run `cargo test -p egret-workflow --lib executor::replay` (all pass), `cargo test -p egret-workflow --lib executor::restart` (20 pass), and the ratchet `cargo test -p egret-workflow --test executor` (unchanged).
- [ ] Commit: `feat(workflow): pure replay-from and restart-copy selections (package 7b)`.

**Implementation** (`src/executor/replay.rs`):

```rust
//! Replay from a task run, and restart on another source, as copies --
//! workflow package 7b (spec `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`
//! D1-D5; Kestra `ExecutionService.replay`, `ES:393-512`, and `.restart` with
//! a revision, `ES:281-373`).
//!
//! Pure, like `executor::restart`: these functions say which of the old
//! execution's runs the new execution keeps and which of those it resets.
//! The worker copies the kept runs (`worker::copy`) and runs the reset as
//! 7a's restart job.

use std::collections::BTreeSet;

use super::plan::{Plan, TaskIx, TaskKind};
use super::restart::{self, RestartRefusal};
use super::state::{is_descendant_of, ExecutionState, RunIx, RunState};

// (CopyRefusal, MatchFault, CopySelection as in Interfaces)

pub fn replay_from(
    plan: &Plan,
    state: &ExecutionState,
    chosen: RunIx,
) -> Result<CopySelection, CopyRefusal> {
    let n = state.runs.len();
    if chosen >= n || state.runs[chosen].history.is_empty() {
        return Err(CopyRefusal::NoSuchRun);
    }
    let above = restart::ancestors(state, chosen);
    if above.iter().any(|&a| {
        matches!(restart::kind(plan, state, a), Some(TaskKind::WorkingDirectory { .. }))
    }) {
        return Err(CopyRefusal::InsideWorkingDirectory);
    }
    // Kestra `taskRunToRestart` (`ES:375-390`): the run and its ancestors.
    let mut reset: BTreeSet<RunIx> = above.into_iter().collect();
    reset.insert(chosen);
    // `ES:435-454`, plus S1's divergence.
    let mut dropped = successors(plan, state, chosen);
    dropped.extend(scope_errors(plan, state, chosen));
    dropped.retain(|r| !reset.contains(r));
    // `removeWorkerTask` (`ES:1179-1200`): a reset `WorkingDirectory` re-runs whole.
    for &w in &reset {
        if matches!(restart::kind(plan, state, w), Some(TaskKind::WorkingDirectory { .. })) {
            dropped.extend((0..n).filter(|&ix| is_descendant_of(state, ix, w)));
        }
    }
    let keep = kept(state, &dropped);
    // `ES:456-484`: a kept run that had not ended is reset too.
    for &ix in &keep {
        if !state.runs[ix].history.last().is_some_and(|s| s.is_terminal()) {
            reset.insert(ix);
        }
    }
    reset.retain(|r| keep.contains(r));
    Ok(CopySelection { keep, reset })
}

pub fn restart_copy(plan: &Plan, state: &ExecutionState) -> Result<CopySelection, CopyRefusal> {
    // 7a's sets, its tick irrelevant: only the sets are read.
    let cmds = restart::restart(plan, state, super::state::Tick(0)).map_err(|r| match r {
        RestartRefusal::NothingToRestart => CopyRefusal::NothingToRestart,
        RestartRefusal::NotRestartable(s) => CopyRefusal::NotRestartable(s),
        RestartRefusal::InsideWorkingDirectory => unreachable!("only mark_as returns it"),
    })?;
    let dropped: BTreeSet<RunIx> = cmds.emptied.iter().copied().collect();
    let keep = kept(state, &dropped);
    let reset = cmds
        .reset
        .iter()
        .chain(&cmds.loops)
        .copied()
        .filter(|r| keep.contains(r))
        .collect();
    Ok(CopySelection { keep, reset })
}

/// Every run neither dropped nor empty, whose parent is kept too (parents
/// precede children, so one ascending pass decides it).
fn kept(state: &ExecutionState, dropped: &BTreeSet<RunIx>) -> Vec<RunIx> {
    let mut keep: Vec<RunIx> = Vec::new();
    for ix in 0..state.runs.len() {
        let run = &state.runs[ix];
        let parent_kept = run.parent.is_none_or(|p| keep.binary_search(&p).is_ok());
        if !dropped.contains(&ix) && !run.history.is_empty() && parent_kept {
            keep.push(ix);
        }
    }
    keep
}

/// The list a run's task sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role { Tasks, Errors, Finally, AfterExecution }

/// How a list's members follow each other in Kestra's graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// A chain (`GraphUtils.fillGraph`, SEQUENTIAL): every list but these two.
    Chain,
    /// `Parallel`'s tasks: each from the task node, none after another.
    Parallel,
    /// A `Dag`'s nodes: by `dependsOn` (`GraphUtils.fillGraphDag`, `:523-633`).
    Dag,
}

struct Slot {
    parent: Option<RunIx>,
    role: Role,
    shape: Shape,
    list: Vec<TaskIx>,
}

/// Where `run`'s task sits under its parent run (or the flow).
fn slot(plan: &Plan, state: &ExecutionState, run: RunIx) -> Option<Slot> {
    let task = state.runs[run].task;
    let parent = state.runs[run].parent;
    let lists: Vec<(Role, Shape, Vec<TaskIx>)> = match parent {
        None => vec![
            (Role::Tasks, Shape::Chain, plan.roots.clone()),
            (Role::Errors, Shape::Chain, plan.errors.clone()),
            (Role::Finally, Shape::Chain, plan.finally.clone()),
            (Role::AfterExecution, Shape::Chain, plan.after_execution.clone()),
        ],
        Some(p) => {
            let pt = &plan.tasks[state.runs[p].task];
            let mut v = vec![
                (Role::Errors, Shape::Chain, pt.errors.clone()),
                (Role::Finally, Shape::Chain, pt.finally.clone()),
            ];
            match &pt.kind {
                TaskKind::Sequential { tasks }
                | TaskKind::AllowFailure { tasks }
                | TaskKind::WorkingDirectory { tasks }
                | TaskKind::LoopUntil { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Chain, tasks.clone()))
                }
                TaskKind::Parallel { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Parallel, tasks.clone()))
                }
                TaskKind::If { then_, else_, .. } => {
                    v.push((Role::Tasks, Shape::Chain, then_.clone()));
                    v.push((Role::Tasks, Shape::Chain, else_.clone()));
                }
                TaskKind::Switch { cases, default, .. } => {
                    for (_, c) in cases {
                        v.push((Role::Tasks, Shape::Chain, c.clone()));
                    }
                    v.push((Role::Tasks, Shape::Chain, default.clone()));
                }
                TaskKind::Dag { nodes } => {
                    v.push((Role::Tasks, Shape::Dag, nodes.iter().map(|d| d.task).collect()))
                }
                _ => {}
            }
            v
        }
    };
    lists
        .into_iter()
        .find(|(_, _, l)| l.contains(&task))
        .map(|(role, shape, list)| Slot { parent, role, shape, list })
}

/// Kestra `GraphUtils.successors(graph, {chosen})` (`GraphUtils.java:144-239`)
/// over egret's plan and runs (S1).
fn successors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let n = state.runs.len();
    let mut out = BTreeSet::new();
    let subtree = |out: &mut BTreeSet<RunIx>, r: RunIx| {
        out.insert(r);
        out.extend((0..n).filter(|&ix| is_descendant_of(state, ix, r)));
    };
    let under = |p: Option<RunIx>| (0..n).filter(move |&ix| state.runs[ix].parent == p);
    subtree(&mut out, chosen);

    // `:157-169`: a node whose incoming edge comes from a cluster ROOT is
    // also walked from that root -- the first top-level task (`root → t1`)
    // and the first task of an `errors:` list (`root → e1`, `:468-472`). A
    // flowable's own root reaches its own errors, already in its subtree.
    if let Some(s) = slot(plan, state, chosen) {
        let first = s.list.first() == Some(&state.runs[chosen].task);
        let top_tasks = s.parent.is_none() && s.role == Role::Tasks;
        if first && (top_tasks || s.role == Role::Errors) {
            for r in under(s.parent).collect::<Vec<_>>() {
                subtree(&mut out, r);
            }
        }
    }

    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        let task = state.runs[cur].task;
        let later: BTreeSet<TaskIx> = match s.shape {
            Shape::Chain => s.list.iter().skip_while(|&&t| t != task).skip(1).copied().collect(),
            Shape::Parallel => BTreeSet::new(),
            Shape::Dag => dag_dependents(plan, state, s.parent, task),
        };
        for r in under(s.parent).collect::<Vec<_>>() {
            let t = state.runs[r].task;
            // `fillGraph` chains a task's several runs in order, then the next task's.
            let same_task_later = s.shape == Shape::Chain && t == task && r > cur;
            if later.contains(&t) || same_task_later {
                subtree(&mut out, r);
            }
        }
        // The scope's later phases (`:488-489`, `:413-421`).
        let finally: Vec<TaskIx> = match s.parent {
            None => plan.finally.clone(),
            Some(p) => plan.tasks[state.runs[p].task].finally.clone(),
        };
        let after: &[TaskIx] = if s.parent.is_none() { &plan.after_execution } else { &[] };
        let phases: Vec<TaskIx> = match s.role {
            Role::Tasks | Role::Errors => finally.iter().chain(after).copied().collect(),
            Role::Finally => after.to_vec(),
            Role::AfterExecution => Vec::new(),
        };
        for r in under(s.parent).collect::<Vec<_>>() {
            if phases.contains(&state.runs[r].task) {
                subtree(&mut out, r);
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}

/// The `Dag` nodes that depend on `task`, transitively.
fn dag_dependents(plan: &Plan, state: &ExecutionState, parent: Option<RunIx>, task: TaskIx)
    -> BTreeSet<TaskIx>
{
    let Some(p) = parent else { return BTreeSet::new() };
    let TaskKind::Dag { nodes } = &plan.tasks[state.runs[p].task].kind else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    let mut frontier = vec![task];
    while let Some(t) = frontier.pop() {
        for d in nodes.iter().filter(|d| d.depends_on.contains(&t)) {
            if out.insert(d.task) {
                frontier.push(d.task);
            }
        }
    }
    out
}

/// **S1's divergence (spec §5 item 6):** every scope that the chosen run, or
/// an ancestor the walk climbs through, sits in through its `tasks` (any
/// branch) has its `errors:` runs dropped with their subtrees. Kestra keeps
/// them, and a kept errors run holds the list in error mode
/// (`Execution.java:595-606`; egret `flowable::current_list`), so the
/// chosen task would never run.
fn scope_errors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let n = state.runs.len();
    let mut out = BTreeSet::new();
    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        if s.role == Role::Tasks {
            let errors: Vec<TaskIx> = match s.parent {
                None => plan.errors.clone(),
                Some(p) => plan.tasks[state.runs[p].task].errors.clone(),
            };
            for r in (0..n).filter(|&ix| state.runs[ix].parent == s.parent) {
                if errors.contains(&state.runs[r].task) {
                    out.insert(r);
                    out.extend((0..n).filter(|&ix| is_descendant_of(state, ix, r)));
                }
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}
```

- `keep.contains` on a `Vec` is O(n): fine for these sizes. A reviewer may ask for a `BTreeSet`; either is acceptable.
- `Option::is_none_or` is stable since 1.82; the MSRV is 1.85.
- Use the real field names of `TaskKind::LoopUntil`, `If` and `Switch` from `plan.rs:376-640`. `If { then_, else_, .. }` and `Switch { cases, default, .. }` are as listed in the record above.

**Tests** (`executor/replay.rs`, `mod tests`):

```rust
use super::*;
use crate::executor::test_support::{drive, driven, flow, leaf, run};
use crate::executor::{build_plan, ExecutionState, RunState};

const PAR: &str = "io.kestra.plugin.core.flow.Parallel";
const SEQ: &str = "io.kestra.plugin.core.flow.Sequential";
const WD: &str = "io.kestra.plugin.core.flow.WorkingDirectory";

fn ids(plan: &Plan, state: &ExecutionState, ixs: impl IntoIterator<Item = RunIx>) -> Vec<String> {
    let mut v: Vec<String> = ixs.into_iter().map(|ix| plan.tasks[state.runs[ix].task].id.clone()).collect();
    v.sort();
    v
}

fn block(id: &str, ty: &str, indent: usize, body: &str) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}- id: {id}\n{pad}  type: {ty}\n{pad}  tasks:\n{body}")
}
```

1. `replay_simple_keeps_what_came_before` (Kestra `replaySimple`, `ExecutionServiceTest.java:197-215`).
   - Flow: tasks `t1..t5` (`leaf(.., 2)`), all `SUCCESS`; replay from `t2`.
   - Assert `keep` is `t1, t2` and `reset` is `t2`.
2. `replay_parallel_nested_keeps_eight_of_eleven` (Kestra `replayParallel`, `:239-258`).
   - Flow: `parallel-nested.yaml` built with `block`:
     ```text
     1_par (Parallel)
     ├── 1-1
     ├── 1-2
     ├── 1-3_par (Parallel)
     │   ├── 1-3-1
     │   ├── 1-3-2_par (Sequential)
     │   │   ├── 1-3-2-1
     │   │   └── 1-3-2-2_end
     │   └── 1-3-3_end
     └── 1-4_end
     2_end
     ```
   - Replay from `1-3-2_par`.
   - Assert 11 runs before, 8 kept, and `reset` = `{1-3-2_par, 1-3_par, 1_par}`.
3. `replay_from_a_flowable_drops_its_errors_runs` (Kestra `replaySequentialWithErrorHandler`, `:563-596`).
   - Flow: `before`, then `sequential` with `failing-task` (outcome `Failed`) and `errors: error-handler`. 4 runs.
   - Replay from `sequential`. Assert `keep` = `before, sequential`.
4. `the_first_top_level_task_drops_everything_else`: tasks `a, b, c`; replay from `a`. Assert `keep` = `a`.
5. `finally_and_after_execution_runs_are_dropped`: tasks `a, b`; `finally: f`; `afterExecution: x`; replay from `b`. Assert `keep` = `a, b`.
6. `a_kept_scope_errors_run_is_dropped_s1`.
   - Flow: tasks `a, b`; `errors: e`; outcomes `b = Failed` (runs `a, b, e`). Replay from `b`. Assert `keep` = `a, b`.
   - A comment says Kestra keeps `e`, and names S1.
7. `the_second_error_task_keeps_the_first`: as 6 with `errors: e, e2`; replay from `e2`. Assert `keep` = `a, b, e, e2` and `reset` = `e2`.
8. `the_first_error_task_reaches_the_whole_scope`: as 7, replay from `e`. Assert `keep` = `e` (Kestra's root rule).
9. `in_a_parallel_siblings_are_not_successors`: `p (Parallel) [x, y]`, then `z`; replay from `x`. Assert `keep` = `p, x, y` and `reset` = `p, x`.
10. `in_a_switch_later_case_tasks_are_dropped`.
    - Flow: `s` (`io.kestra.plugin.core.flow.Switch`, `value: "A"`, `cases: {A: [a1, a2]}`), then `after`.
    - Drive: `drive`, set `state.runs[run(.., "s")].rendered = Some("A".into())`, `drive` again.
    - Replay from `a1`. Assert `keep` = `s, a1` and `reset` = `s, a1`.
11. `a_loop_until_iteration_drops_later_iterations` (`same_task_later`).
    - Flow: `LoopUntil lu [c1, c2]`. `drive_from` with `pass_time` and a condition answered `false` once, then `true`. If the shared drive answers only `false`, write a two-iteration state by hand as `state.rs` tests do.
    - Replay from the first `c1`. Assert the second iteration's `c1` and `c2` are dropped.
12. `a_dag_drops_only_dependents`: `Dag` nodes `d1`, `d2 dependsOn d1`, `d3`; replay from `d1`. Assert `keep` contains `d3` and not `d2`.
13. `a_working_directory_is_replayed_whole`: `w (WorkingDirectory) [w1, w2]`, then `c`; replay from `w`. Assert `keep` = `w`.
14. `a_run_inside_a_working_directory_is_refused`: replay from `w1`. Assert `Err(CopyRefusal::InsideWorkingDirectory)`.
15. `an_unfinished_kept_run_is_reset` (Kestra `replayParallelRestartsRunningSibling`, `:260-284`).
    - Flow: `p (Parallel) [x, y]`, all `SUCCESS`. Then `state.runs[y].history.pop()` and push `Running`.
    - Replay from `x`. Assert `reset` = `p, x, y`.
16. `an_empty_or_unknown_run_is_refused`: `replay_from(.., state.runs.len())` and a run whose history is cleared each give `NoSuchRun`.
17. `restart_copy_resets_the_failed_and_drops_the_hooks`.
    - Flow: tasks `a, b, c`; `errors: e`; `finally: f`; `afterExecution: x`; `b = Failed`.
    - Assert `keep` = `a, b` and `reset` = `b`.
18. `restart_copy_refusals`: a `SUCCESS` execution gives `NotRestartable(Success)`.

### A2. The pure copy: `copy_onto`, `shift_ticks`, `reset_copied` (spec D1, D5; S3, S4)

**Files:** modify `src/executor/replay.rs` and `src/executor/mod.rs` (re-exports).

**Interfaces.**

Consumes: A1's `CopySelection`, `CopyRefusal`, `MatchFault`; `restart::{reset, to_created, kind}`.

Produces:
```rust
/// The new execution's runs: the kept runs renumbered densely in order and
/// mapped onto the target plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedRuns {
    pub runs: Vec<TaskRun>,
    /// `old_of[new] = old` run index.
    pub old_of: Vec<RunIx>,
    /// `selection.reset`, renumbered; ascending.
    pub reset: Vec<RunIx>,
}

pub fn copy_onto(from: &Plan, to: &Plan, state: &ExecutionState, sel: &CopySelection)
    -> Result<CopiedRuns, CopyRefusal>;

/// Move every tick of `runs` by `by` ms: the attempts' `started_at`/`ended_at`
/// and `not_before`. A copy's ticks are measured from the NEW execution's
/// `created_at`.
pub fn shift_ticks(runs: &mut [TaskRun], by: i64);

/// Reset the copied runs (new indexes). 7a's per-kind state (`restart::reset`),
/// except a `Loop`, which goes `CREATED` and spawns its iterations anew (S4).
/// Ends with `SetExecutionState { Restarted }`. `loops` is always empty.
pub fn reset_copied(plan: &Plan, state: &ExecutionState, reset: &[RunIx], now: Tick)
    -> RestartCommands;
```

**Implementation:**

```rust
pub fn copy_onto(
    from: &Plan,
    to: &Plan,
    state: &ExecutionState,
    sel: &CopySelection,
) -> Result<CopiedRuns, CopyRefusal> {
    let parent_task = parents(to);
    let mut new_of = std::collections::HashMap::new();
    let mut runs = Vec::with_capacity(sel.keep.len());
    let mut old_of = Vec::with_capacity(sel.keep.len());
    for &old in &sel.keep {
        let r = &state.runs[old];
        let id = &from.tasks[r.task].id;
        let fault = |why| CopyRefusal::TaskNotInSource { task_id: id.clone(), why };
        let t = to.tasks.iter().position(|t| &t.id == id).ok_or_else(|| fault(MatchFault::Missing))?;
        let parent = r.parent.map(|p| new_of[&p]);
        let want_parent = parent.map(|p: RunIx| runs[p].task as TaskIx);
        if parent_task.get(&t).copied() != Some(want_parent)
            || role_of(from, r.parent.map(|p| state.runs[p].task), r.task)
                != role_of(to, want_parent, t)
        {
            return Err(fault(MatchFault::Moved));
        }
        if std::mem::discriminant(&from.tasks[r.task].kind) != std::mem::discriminant(&to.tasks[t].kind) {
            return Err(fault(MatchFault::OtherKind));
        }
        new_of.insert(old, runs.len());
        let mut copy = r.clone();
        copy.task = t;
        copy.parent = parent;
        runs.push(copy);
        old_of.push(old);
    }
    let reset = sel.reset.iter().map(|o| new_of[o]).collect();
    Ok(CopiedRuns { runs, old_of, reset })
}
```

- `parents(plan) -> HashMap<TaskIx, Option<TaskIx>>`: `None` for each of `roots`, `errors`, `finally` and `after_execution`; `Some(p)` for each `c` in `plan.children_of(p)` over every `p`.
- `role_of(plan, parent_task: Option<TaskIx>, task) -> Role`: from the same lists `slot` reads. `Errors` / `Finally` come from the parent's (or the flow's) lists, `AfterExecution` from the flow's, `Tasks` otherwise.
- `new_of[&p]` cannot miss: `kept` guarantees the parent is kept and parents precede children.

```rust
pub fn shift_ticks(runs: &mut [TaskRun], by: i64) {
    for r in runs {
        for a in &mut r.attempts {
            a.started_at = Tick(a.started_at.0 + by);
            a.ended_at = a.ended_at.map(|t| Tick(t.0 + by));
        }
        r.not_before = r.not_before.map(|t| Tick(t.0 + by));
    }
}

pub fn reset_copied(plan: &Plan, state: &ExecutionState, reset: &[RunIx], now: Tick) -> RestartCommands {
    let mut out = RestartCommands::default();
    for &run in reset {
        match restart::kind(plan, state, run) {
            // S4: the iterations are the old execution's; this one spawns its own.
            Some(TaskKind::Loop { .. }) => restart::to_created(run, &mut out),
            _ => restart::reset(plan, state, run, now, &mut out),
        }
    }
    out.commands.push(Command::SetExecutionState { state: RunState::Restarted });
    out
}
```

`Tick`'s inner field is `i64` (`claim.rs:4874`). If it is private, use the existing constructor and accessor; do not widen visibility without reason.

**Tests** (same module):

1. `the_identity_copy_renumbers_densely`.
   - Flow: `p (Parallel) [s (Sequential) [s1, s2], y (Sequential) [y1, y2]]`.
   - Replay from `s1`; `copy_onto(&plan, &plan, ..)`.
   - Assert:
     - `runs.len() == keep.len()`;
     - `old_of` is strictly increasing;
     - the copied `y2`'s `parent` is the copied `y`'s new index;
     - no copied `parent` is ≥ its own index.
2. `a_task_missing_from_the_target_is_refused`: target source without `t2`; replay from `t2`. Assert `TaskNotInSource { task_id: "t2", why: Missing }`.
3. `a_kept_task_moved_under_another_parent_is_refused`: target where `t1` sits inside a new `Sequential`. Assert `Moved`.
4. `a_kept_task_of_another_kind_is_refused`: target where `t1` is a `Sequential`. Assert `OtherKind`.
5. `an_added_task_shifts_the_task_index`: target `t0, t1, t2, t3, t4, t5`. Assert the copied `t1`'s `task` is the target's index of `t1`.
6. `shift_moves_every_tick`.
7. `reset_copied_runs_on_from_the_chosen_task`.
   - Replay from `t2` of `t1..t3`; copy; `reset_copied`; `apply` its commands; `drive`.
   - Assert every run ends `SUCCESS`.
   - Assert the execution's history contains `Restarted`.
   - Assert `t3` was created after the reset (3 runs).
8. `a_copied_loop_is_created_again`.
   - Flow: `l (Loop)`, values rendered by `drive` as `["a"]`, then `after`; replay from `l`.
   - Assert the command for `l` is `SetRunState { Created }` and `reset_copied(..).loops` is empty.
9. `a_copied_sequential_ancestor_is_reopened`: replay from `s1` in `s (Sequential) [s1, s2]`. Assert `ReopenFlowable` for `s` and `SetRunState { Created }` for `s1`.

- [ ] Write the tests, run `cargo test -p egret-workflow --lib executor::replay` (red), implement, run again (green), run the ratchet (`--test executor`, unchanged).
- [ ] Commit: `feat(workflow): pure copy of task runs onto a chosen source (package 7b)`.

### A3. The worker: insert the row, replay a whole execution, and the copy (spec D1, D6, D7, D10; S6, S7)

**Files:**
- Modify: `src/worker/claim.rs`, `src/worker/restart.rs`, `src/worker/mod.rs`.
- Create: `src/worker/copy.rs`, `tests/replay.rs`.

**Interfaces.**

Consumes: A1/A2 (`replay_from`, `restart_copy`, `copy_onto`, `shift_ticks`, `reset_copied`); `claim::{load, settle, run_job, Job, Stack, checked_inputs, attempts_json}`; `restart::{lock, Prepared, Opening, Op, side_writes}`; `secret_inputs::reveal`.

Produces:
```rust
// worker/claim.rs
/// The row half of a start (everything `start_execution_in_on` did before
/// its settle). No settle: a start settles the empty state; a copy inserts
/// runs first.
pub(super) async fn insert_execution(
    conn: &mut PgConnection,
    gate: &Gate,
    new: NewExecution<'_>,
) -> Result<ExecutionCtx>;
/// A fresh row's first settle, `Tick(0)`, from the empty state.
pub(super) async fn settle_new(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<()>;
pub(super) fn attempts_json(attempts: &[Attempt]) -> serde_json::Value; // was private
pub(super) fn states_json(states: &[RunState]) -> serde_json::Value;     // was private

// worker/restart.rs
pub const REPLAY_LABEL_KEY: &str = "system.replay";
pub const REPLAY_LABEL_VALUE: &str = "true";
/// `labels` with `key: "true"` appended unless a label of that key is there.
pub(super) fn with_label(labels: Option<&serde_json::Value>, key: &str, value: &str)
    -> serde_json::Value;
/// A copied execution's reset, as 7a's restart job: locked, its commands
/// computed by `executor::reset_copied`, opened by the walk.
pub(super) async fn copy_job(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    reset: &[RunIx],
    op: CopyOp,
) -> Result<claim::Job>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyOp { Replay, Restart }

// worker/copy.rs (re-exported from worker)
pub struct ReplayRequest<'a> {
    pub workspace_id: Uuid,
    /// The execution replayed or restarted; the route scoped it.
    pub from: Uuid,
    /// The source the new execution runs and freezes, chosen by the route (D6, S6).
    pub source: &'a str,
    pub kind: ReplayKind,
    pub inputs: ReplayInputs,
    /// The person (D8: `check_start`'s door 2 judges the caller).
    pub started_by: Uuid,
    /// Chosen first: a multipart body's files are stored under it.
    pub id: Uuid,
    pub input_files: Vec<crate::storage::StoredFile>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayKind { Whole, FromTaskRun(Uuid), RestartOnSource }
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayInputs { Reuse, Replace(serde_json::Value) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayRefused {
    LoopIteration { root: Uuid },
    /// Replay: the execution has not ended; its state.
    NotTerminal(String),
    /// Restart on a source: 7a's refusals.
    NotRestartable(String),
    NothingToRestart,
    FilesPruned,
    AfterExecutionRunning,
    TaskRunNotFound,
    InsideWorkingDirectory,
    TaskNotInSource { task_id: String, why: crate::executor::MatchFault },
}
/// Spec D1-D10. Unscoped by workspace, like every worker entry point. A
/// refusal writes nothing. A deadlock's victim is run again (7a's retry).
pub async fn replay_execution(pool: &PgPool, req: &ReplayRequest<'_>)
    -> Result<std::result::Result<Uuid, ReplayRefused>>;
```

**Steps.**

- [ ] **Refactor `start_execution_in_on`** (`claim.rs:4297-4452`). Move the body from `let NewExecution { .. } = new;` through `index_input_files(..)` and the `ExecutionCtx { .. }` construction into `insert_execution`. It returns the ctx. `start_execution_in_on` becomes:
  ```rust
  let born_failed = new.born_failed;
  let ctx = insert_execution(conn, gate, new).await?;
  let empty = ExecutionState::new(&ctx.plan);
  let mut state = empty.clone();
  if born_failed {
      apply(&mut state, &Command::SetExecutionState { state: RunState::Failed });
  }
  settle(conn, &ctx, &empty, state, Tick(0), None).await?;
  Ok(ctx.id)
  ```
  `settle_new` is the three lines without `born_failed`. Run `cargo test -p egret-workflow --test worker --test inputs_start --test file_inputs`: all green, the same counts as before. Commit: `refactor(workflow): split the start's row insert from its first settle`.
- [ ] **Write the failing tests** in `tests/replay.rs` (list below). Run `cargo test -p egret-workflow --test replay`; red (does not compile).
- [ ] **Implement `worker/restart.rs` additions.**
  - `Op` gains `Replay`; `CopyOp` maps to `Op::Replay` or `Op::Restart`.
  - `side_writes` takes `label_key: &str` and calls `with_label(labels, label_key, "true")`. It writes only when the result differs from the stored labels, keeping today's behaviour.
  - `open` passes `REPLAY_LABEL_KEY` for `Op::Replay` and `RESTARTED_LABEL_KEY` otherwise. Its `debug_assert!` reads `matches!(op, Op::Restart | Op::Replay)`.
  - `copy_job`:
    ```rust
    let Locked { ctx, state, now, row } = lock(conn, id, stack).await?;
    let cmds = crate::executor::reset_copied(&ctx.plan, &state, reset, now);
    let op = match op { CopyOp::Replay => Op::Replay, CopyOp::Restart => Op::Restart };
    Ok(claim::Job::Restart(Box::new(Opening {
        id,
        prepared: Prepared { ctx, state, now, cmds, labels: row.labels, op },
    })))
    ```
- [ ] **Implement `worker/copy.rs::replay_execution`**, in `transaction!` with 7a's deadlock retry (copy `restart_execution`, `worker/restart.rs:84-99`). Inside, `replay_locked(conn, gate, req)` runs these steps. **Every refusal comes before the first write**, because `transaction!` commits an `Ok`.
  1. Load the old execution under its lock: `claim::load(conn, req.from, Stack::Fresh(gate.clone()))`. Lock its runs `FOR UPDATE` in `run_ix` order, as `restart::lock`. Then read one row:
     ```sql
     WITH RECURSIVE chain(id) AS (
       SELECT $1::uuid
       UNION
       SELECT e.replayed_from_execution_id FROM workflow.executions e JOIN chain c ON e.id = c.id
        WHERE e.replayed_from_execution_id IS NOT NULL)
     SELECT CASE WHEN x.loop_index IS NOT NULL THEN x.parent_execution_id END AS loop_root,
            EXISTS (SELECT 1 FROM workflow.executions p JOIN chain c ON p.id = c.id
                     WHERE p.files_pruned_at IS NOT NULL) AS files_pruned,
            COALESCE(workflow.execution_is_idle(x.id), false) AS idle,
            x.labels, x.attempt_number, x.seq, x.inputs, x.secret_inputs, x.variables, x.flow_id
       FROM workflow.executions x WHERE x.id = $1
     ```
  2. A loop root gives `LoopIteration { root }`.
  3. `Whole` and `FromTaskRun`: the last history entry must be terminal (`RunState::is_terminal`), else `NotTerminal(wire)`.
  4. `FromTaskRun` and `RestartOnSource` (S7): `files_pruned` gives `FilesPruned`; `is_terminal && !idle` gives `AfterExecutionRunning`.
  5. Selection:
     - `FromTaskRun(t)`: `SELECT run_ix FROM workflow.task_runs WHERE execution_id = $1 AND id = $2`; none gives `TaskRunNotFound`. Then `replay_from`: `NoSuchRun` gives `TaskRunNotFound` (an emptied run), `InsideWorkingDirectory` passes through.
     - `RestartOnSource`: `restart_copy`, refusals mapped as 7a's.
  6. `let target = crate::executor::build_plan(req.source)`, mapping an error to `Error::InvalidInput(format!("the flow does not plan: {e}"))`. For copies, `copy_onto(&old_ctx.plan, &target, &state, &sel)`, mapping `TaskNotInSource`.
  7. Inputs:
     - `Reuse`: `secret_inputs::reveal(old.inputs, old.secret_inputs.as_deref())` (KI-112, as today's route).
     - `Replace(v)`: `v` must be an object, else `InvalidInput("inputs must be a JSON object")`. Then `claim::checked_inputs(&crate::spec::parse(req.source)?, &v)` mapped to `Error::InvalidInput(e.message)` (D7: the start's check against the **chosen** source). The insert skips its own check because `replayed_from_execution_id` is set (`claim.rs:4357-4358`).
  8. Labels: `Whole` uses `Some(with_label(old.labels, REPLAY_LABEL_KEY, REPLAY_LABEL_VALUE))`. Copies use `old.labels` (`side_writes` adds the label).
  9. `let ctx = claim::insert_execution(conn, gate, NewExecution { .. })`, with:
     - `workspace_id: req.workspace_id`, `flow_id: old.flow_id`, `source: req.source`, `inputs`;
     - `trigger: None`, `scheduled_date: None`, `trigger_execution_id: None`, `execution_depth: 0`, `flow_trigger_event_id: None`, `born_failed: false`;
     - `replayed_from_execution_id: Some(req.from)`, `id: Some(req.id)`, `input_files: req.input_files.clone()`;
     - `labels`, `backfill_id: None`, `variables: old.variables`, `started_by: Some(req.started_by)`.

     This is where `check_start` judges the chosen source with the caller (D8), and where FILE inputs are checked.
  10. `Whole`: `UPDATE workflow.executions SET attempt_number = $2 WHERE id = $1` with `old.attempt_number + 1` (D10), then `claim::settle_new(conn, &ctx)`. Return `Ok(Ok(req.id))`.
  11. Copies:
      - `UPDATE workflow.executions SET attempt_number = $2, seq = $3 WHERE id = $1` with the old values: `side_writes` adds 1, and the copied `created_seq`/`ended_seq` stay below the clock.
      - Compute the shift:
        ```sql
        SELECT (EXTRACT(EPOCH FROM (o.created_at - n.created_at)) * 1000)::bigint
          FROM workflow.executions o, workflow.executions n WHERE o.id = $1 AND n.id = $2
        ```
        then `shift_ticks(&mut copied.runs, by)`.
      - `insert_runs(conn, req.id, req.from, &target, &copied)` (below).
      - `let job = restart::copy_job(conn, req.id, Stack::Fresh(gate.clone()), &copied.reset, op)`, with `CopyOp::Replay` for `FromTaskRun` and `CopyOp::Restart` for `RestartOnSource`.
      - `claim::run_job(conn, job)`; return `Ok(Ok(req.id))`.
- [ ] **`insert_runs`** carries every result column from the old row and clears the lease. `claimed_at` is not listed, so it is NULL. A copied `WorkingDirectory` block owns no `_wd/{block}/` snapshot under its new id, so it is stamped swept.
  ```rust
  const INSERT_COPIED_RUNS: &str = "\
  INSERT INTO workflow.task_runs \
    (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, rendered, rendered_run_if, \
     state, history, attempts, not_before_ms, created_seq, ended_seq, resume_with, working_dir, \
     started_at, ended_at, outputs, secret_outputs, outputs_bytes, message, \
     working_dir_snapshot, working_dir_swept_at) \
  SELECT $1, m.new_ix, m.task_ix, m.task_id, m.type_id, m.parent_ix, o.value, o.rendered, \
         o.rendered_run_if, o.state, o.history, m.attempts, m.not_before_ms, o.created_seq, \
         o.ended_seq, o.resume_with, o.working_dir, o.started_at, o.ended_at, o.outputs, \
         o.secret_outputs, o.outputs_bytes, o.message, o.working_dir_snapshot, \
         CASE WHEN o.working_dir IS NOT NULL THEN COALESCE(o.working_dir_swept_at, now()) END \
    FROM unnest($3::int[], $4::int[], $5::int[], $6::text[], $7::text[], $8::int[], \
                $9::jsonb[], $10::bigint[]) \
         AS m(old_ix, new_ix, task_ix, task_id, type_id, parent_ix, attempts, not_before_ms) \
    JOIN workflow.task_runs o ON o.execution_id = $2 AND o.run_ix = m.old_ix";
  ```
  - Bind `Vec<i32>`, `Vec<i32>`, `Vec<i32>`, `Vec<String>`, `Vec<Option<String>>`, `Vec<Option<i32>>`, `Vec<serde_json::Value>` (from `claim::attempts_json`) and `Vec<Option<i64>>`.
  - `type_id` is the **target** task's (`TaskKind::Runnable { type_id }` or NULL, `claim.rs:5546-5549`).
  - `id` takes its default (`gen_random_uuid()`), so every copy has a new task-run id (Kestra `mapTaskRunId`).
  - A static `&str`, as sqlx 0.9 requires (memory: sqlx 0.9 audit).
  - The task's state and history are the old row's. The reset is 7a's `side_writes` (outputs, message and lease cleared on the reset runs) plus `settle`'s `persist`.
- [ ] **Re-export** in `worker/mod.rs`: `mod copy; pub use copy::{replay_execution, ReplayInputs, ReplayKind, ReplayRefused, ReplayRequest};` and `pub use restart::{REPLAY_LABEL_KEY, REPLAY_LABEL_VALUE};`. In `executor/mod.rs`: `pub use replay::{copy_onto, replay_from, reset_copied, restart_copy, shift_ticks, CopiedRuns, CopyRefusal, CopySelection, MatchFault};`.
- [ ] Run `cargo test -p egret-workflow --test replay`, then `--test restart --test worker --test inputs_start`, then the ratchet.
- [ ] Commit: `feat(workflow): replay into a new execution, whole or from a task run (package 7b)`.

**Tests** (`tests/replay.rs`).

The module doc names the spec and the run command:
```text
export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
cargo test -p egret-workflow --test replay
```
Copy `Flaky` (`tests/restart.rs:37-112`, with `attempts_of`) and the helpers `setup`, `start`, `exec_state`, `exec_history`, `exec_meta`, `runs_of`, `run`, `drive`, `history_tail`, `tasks`, `three` (`tests/restart.rs:116-232`). Add the `database_url_is_exported_rather_than_read_from_dotenv` test. Add:

```rust
async fn replay(pool: &sqlx::PgPool, ws: Uuid, from: Uuid, source: &str, kind: ReplayKind, inputs: ReplayInputs)
    -> Result<Uuid, ReplayRefused>
{
    let user: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws).fetch_one(pool).await.expect("owner");
    egret_workflow::worker::replay_execution(pool, &ReplayRequest {
        workspace_id: ws, from, source, kind, inputs, started_by: user,
        id: Uuid::new_v4(), input_files: Vec::new(),
    }).await.expect("replay")
}
```

1. `replay_from_a_task_keeps_earlier_runs_and_their_outputs`.
   - Arrange: `three("seq")`; `Flaky::new(&[("b", 1)])`; drive to `FAILED`.
   - Act: replay `FromTaskRun(b.id)`, frozen source, `Reuse`; drive.
   - Assert:
     - the new execution is `SUCCESS`;
     - `reg.attempts_of("a") == [1]` (not run again);
     - the new `a` has the old `a`'s `outputs` and a different `id`;
     - the new `b` and `c` ran;
     - `exec_history` of the new one starts `["CREATED", "RESTARTED", "RUNNING"]`;
     - its `replayed_from_execution_id` is the old id;
     - the old execution's rows are unchanged (history and run count).
2. `replay_from_a_parallel_branch`: `p (Parallel) [x, y]`, then `z`; `x` fails once. Replay from `x`. Assert `y` is not run again, `x` and `z` run, and the new execution is `SUCCESS`.
3. `a_copied_run_holds_no_lease`: after Act in test 1 (before driving), every new run has `claimed_at IS NULL`, and `run_ix` is `0..n` with no gap.
4. `replay_from_a_subflow_run_spawns_a_new_child`.
   - Arrange: a child flow and a parent with `Subflow` `call` then `after`; drive.
   - Act: replay from `call`.
   - Assert: exactly one new child row with `trigger_execution_id` = the new execution; the old child is unchanged.
5. `replay_from_a_loop_run_runs_its_iterations_anew` (S4): flow `l (Loop, values ["a","b"]) [item]`, then `after`. Replay from `l`. Assert two new iteration rows with `parent_execution_id` = new id, and the new execution `SUCCESS`.
6. `replay_on_an_edited_source_runs_the_edited_task`: replay from `b` with a source in which `c`'s message changed. Assert `executions.source` of the new row equals the edited text and `c` ran (its `TaskInput` was seen).
7. `a_task_missing_from_the_source_is_refused_and_writes_nothing`: source without `b`. Assert `Err(TaskNotInSource { task_id: "b", why: Missing })` and the execution count unchanged.
8. `new_inputs_reach_the_replayed_task_and_bad_ones_are_refused`.
   - Flow with `inputs: [{id: n, type: INT, min: 1}]` and a task whose `TaskInput` is recorded.
   - Replace with `{"n": 5}`: the new row's `inputs` are `{"n": 5}`.
   - Replace with `{"n": 0}`: `Err` (`Error::InvalidInput` with ``Invalid value for input `n``), nothing written.
9. `the_whole_replay_gets_the_label_and_the_attempt` (D10): a `SUCCESS` execution with `attempt_number` 1. `Whole`. Assert labels contain `{"key":"system.replay","value":"true"}` and `attempt_number == 2`. Replay that replay with `Whole`: `attempt_number == 3` and the label appears once.
10. `a_copy_gets_the_label_once_and_the_attempt` (D10): test 1's new execution has `system.replay` once and `attempt_number == 2`.
11. `restart_on_a_source_resets_the_failed_and_keeps_the_rest` (D3).
    - `three` with `errors: e`; `b` fails once; `RestartOnSource` with the frozen text plus a changed message on `c`.
    - Assert:
      - a new execution with `system.restarted` and `attempt_number == 2`;
      - `a` kept (attempts of `a` == `[1]`), `b` re-run, `e` not copied (no run of `e` before it fails again), `c` ran;
      - the old execution is still `FAILED`.
12. `refusals_write_nothing`: a running execution (`NotTerminal`), a loop iteration (`LoopIteration`), a foreign task-run id (`TaskRunNotFound`), a run inside a `WorkingDirectory` (`InsideWorkingDirectory`), and `files_pruned_at` set on the execution's replay source (`FilesPruned`). Each is followed by an unchanged execution count.
13. `the_egret_sql_gate_judges_the_chosen_source`.
    - Use a flow owned by another user (seed it with `common::seed_flow` under a second user).
    - Frozen source without `egret.sql` (allowed).
    - A replay with a chosen source containing an `egret.sql` task, by the non-creator: `Err` (`InvalidInput`, the `NOT_THE_CREATOR` text), nothing written.
14. `a_kept_attempt_keeps_its_wall_clock`: the new `a`'s first attempt `started_at` plus the new execution's `created_at` equals the old one's plus the old `created_at`, within 1 ms.

### A4. Restart on another source: route-free coverage of the remaining shapes (spec D3, D9)

Covered by A3 tests 11–12. This task adds the shapes 7a's in-place restart handles specially, now as copies.

**Files:** extend `tests/replay.rs`; fix in `src/worker/copy.rs` or `src/executor/replay.rs` only if a test is red.

1. `a_restart_copy_of_a_failed_loop_creates_new_iterations` (S4): a `Loop` whose second iteration failed. `RestartOnSource` gives new iterations under the new id, none restarted in the old one.
2. `a_restart_copy_of_a_failed_subflow_spawns_a_new_child`: the old child is not restarted (no lookup hit under the new id, `ExecutableUtils.java:90-150`'s equivalent `subflow::spawn_one`).
3. `a_restart_copy_of_a_paused_execution_leaves_the_old_one_paused`: Kestra-faithful. Assert the old one is still `PAUSED`.
4. `a_replay_of_a_replay_reads_two_back` (D4, S5): `X`; `Y` = copy of `X` from `b`; `Z` = copy of `Y` from `c`. Then `storage::index::readable_executions(&mut conn, ws, Z)` contains `X`. This pins S5's "already transitive"; it is green before A5.
5. `an_after_execution_task_can_be_replayed`: flow with `afterExecution: x`. Replay from `x` re-runs `x` after the new execution ends; the tasks before it are not run again. If this shape does not come to rest, **stop and report**: do not add a refusal code.

- [ ] Write, run (`--test replay`), fix only what is red, run the ratchet.
- [ ] Commit: `test(workflow): restart on another source and replay chains (package 7b)`.

### A5. The read rule reaches the chain's loop iterations (S5)

**Files:** modify `src/storage/index.rs` (`readable_sql!` and the doc at `:159-171`); extend `tests/replay.rs`.

- [ ] **Red first.** Add `a_copy_reads_its_source_loop_iterations_files`.
  - `X` runs a `Loop` over `["a"]`, then `after`.
  - `Y` = replay of `X` from `after` (the `Loop` run is kept with its outputs).
  - Assert `readable_executions(.., Y)` contains `X`'s iteration id.
  - Run it. **If it is green, stop and report** (the premise is false).
- [ ] **Implement.** In `readable_sql!` add, after `chain`:
  ```sql
  chain_family(id) AS (
    SELECT id FROM chain
    UNION
    SELECT e.id FROM workflow.executions e JOIN chain_family f ON e.parent_execution_id = f.id
     WHERE e.loop_index IS NOT NULL AND e.workspace_id = $1
  ),
  ```
  and change the final select's `UNION SELECT id FROM chain` to `UNION SELECT id FROM chain_family`. The `triggers` CTE keeps `e.id IN (SELECT id FROM chain)`: one trigger level, as before. Rewrite the doc paragraph "The chain's own loop iterations are NOT in the set" to say they are, why (a copy keeps a `Loop` run's outputs, package 7b), and that the pruner's protected set grows with it.
- [ ] Run `cargo test -p egret-workflow --test replay --test storage_index --test storage_prune --test file_inputs`.
- [ ] Commit: `fix(workflow): a replay reads its source's loop iteration files (package 7b)`.

---

## B1. Routes, OpenAPI and audit (spec D5, D8, D9, D11, D12, D13; S6, S8)

**Files:**
- Modify: `src/routes.rs`, `src/repo.rs`, `tests/restart_routes.rs` (the `revision` test), `tests/executions_routes.rs` (the replay-labels test).
- Create: `tests/replay_routes.rs`.

**Interfaces.**

Consumes: `worker::{replay_execution, ReplayRequest, ReplayKind, ReplayInputs, ReplayRefused, restart_execution}`; `FlowRepo::get_version`; `storage::index`.

Produces:
```rust
// routes.rs
pub const INVALID_SOURCE_CODE: &str = "invalid_source";
pub const REPLAY_BREAKPOINTS_UNSUPPORTED_CODE: &str = "replay_breakpoints_unsupported";
pub const REPLAY_TASK_NOT_IN_SOURCE_CODE: &str = "replay_task_not_in_source";
pub const FLOW_VERSION_NOT_FOUND_CODE: &str = "flow_version_not_found";
pub const TASK_RUN_NOT_FOUND_CODE: &str = "task_run_not_found";
// RESTART_REVISION_UNSUPPORTED_CODE is deleted.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChoice { Frozen, Current, Version(i32) }
impl SourceChoice { pub fn wire(self) -> String } // "frozen" | "current" | "version:N"
/// `source` and Kestra's `revision` (an integer N = `version:N`); both → 400.
fn source_choice(source: Option<&str>, revision: Option<&str>) -> Result<SourceChoice, ExecOpError>;
async fn source_text(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail,
                     flow: &crate::repo::FlowRow, choice: SourceChoice) -> Result<String, ExecOpError>;

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct ReplayQuery {
    /// Replay from this task run of the execution (Kestra `taskRunId`).
    #[serde(rename = "taskRunId")]
    pub task_run_id: Option<String>,
    /// `frozen` (default), `current`, or `version:N`.
    pub source: Option<String>,
    /// Kestra's spelling of `version:N`. Not with `source`.
    pub revision: Option<String>,
    /// **Not supported** (spec Q3): any value answers 400 `replay_breakpoints_unsupported`.
    pub breakpoints: Option<String>,
}
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct RestartQuery { pub source: Option<String>, pub revision: Option<String> }

// repo.rs
/// The `Loop` run of `root` whose iterations (at any depth) hold `task_run`:
/// `(loop run id, loop task id)`, or `None`. Scoped by `ws`.
pub async fn loop_run_holding(&self, ws: Uuid, root: Uuid, task_run: Uuid)
    -> Result<Option<(Uuid, String)>>;
```

**`loop_run_holding` SQL:**
```sql
WITH RECURSIVE up(id, parent, parent_run_ix, loop_index) AS (
  SELECT e.id, e.parent_execution_id, e.parent_run_ix, e.loop_index
    FROM workflow.task_runs t JOIN workflow.executions e ON e.id = t.execution_id
   WHERE t.id = $3 AND e.workspace_id = $1
  UNION
  SELECT e.id, e.parent_execution_id, e.parent_run_ix, e.loop_index
    FROM workflow.executions e JOIN up ON e.id = up.parent
   WHERE up.loop_index IS NOT NULL AND e.workspace_id = $1)
SELECT t.id, t.task_id FROM up
  JOIN workflow.task_runs t ON t.execution_id = up.parent AND t.run_ix = up.parent_run_ix
 WHERE up.parent = $2 AND up.loop_index IS NOT NULL
```

**Handler shapes.**

- **`ExecOpError` gains a 404 arm.** In `into_response`, `StatusCode::NOT_FOUND` uses `Error::NotFound(message).to_string()`. It also gains `fn from_replay_refusal(id: Uuid, r: ReplayRefused) -> Self`:
  - `NotTerminal(s)`: `Core(Error::Conflict(..))`, with today's text (`routes.rs:3349-3352`);
  - `LoopIteration`: `loop_iteration(id, root, "cannot be replayed")`;
  - `NotRestartable` / `NothingToRestart` / `FilesPruned` / `AfterExecutionRunning`: 7a's texts;
  - `TaskRunNotFound`: `Coded(NOT_FOUND, TASK_RUN_NOT_FOUND_CODE, "no task run {t} in execution {id}")`;
  - `InsideWorkingDirectory`: 7a's 409;
  - `TaskNotInSource`: `Coded(BAD_REQUEST, REPLAY_TASK_NOT_IN_SOURCE_CODE, ..)`. The message names the task id and the fault: "is not in the chosen source", "sits under another parent or list in the chosen source", "is of another kind in the chosen source".
- **`source_choice`:**
  - both given: 400 `invalid_source`, "pass `source` or `revision`, not both";
  - `revision`: parse `i32` ≥ 1 into `Version(n)`, else `invalid_source`;
  - `source`: `frozen`, `current`, or `version:N` with N ≥ 1, else `invalid_source` naming the value;
  - neither: `Frozen`.
- **`source_text`:**
  - `Frozen`: `detail.source.clone()`;
  - `Current`: `flow.source.clone()`;
  - `Version(n)`: `st.repo.get_version(ws, flow.id, n)`. `Error::NotFound` becomes `Coded(NOT_FOUND, FLOW_VERSION_NOT_FOUND_CODE, "flow version {n} not found")`. Then `snapshot.get("source").and_then(as_str)`, else `Internal` (as `routes.rs:1380-1391`).
- **`rerun_gate`** takes the source text to judge: `rerun_gate(st, ws, caller, detail, source: &str)`. `restart_execution` (frozen) and `set_task_run_state` pass `&before.source`.
- **Body helpers, factored out of the start.**
  - `async fn read_json_body(st: &WorkflowState, req: Request) -> Result<Option<StartExecutionRequest>, Response>`: the 2 MiB limit and the headerless-is-JSON rule of `routes.rs:2642-2683`, verbatim. `start_flow_execution` then calls `start_inputs(body.map(Json))`.
  - `struct Uploaded { inputs: serde_json::Map<String, Value>, files: Vec<StoredFile> }`.
  - `async fn receive_parts(st, workspace, parsed: &Flow, execution: Uuid, req: Request) -> Result<Uploaded, Response>`: the storage lookup, limit and gate setup of `start_multipart` (`:2943-2969`) and the part loop of `read_parts_and_start` (`:3013-3068`). It deletes its own uploads on its own failure.
  - `async fn discard_uploads(workspace: Uuid, parsed: &Flow, execution: Uuid)`: the best-effort `delete_prefix` of `:2986-2995`.
  - `start_multipart` becomes `receive_parts`, then the start transaction (`:3073-3098`), then `discard_uploads` on `Err`.
- **`replay_execution`** (`State`, `WsContext`, `Path((_ws, id))`, `Query(q): Query<ReplayQuery>`, `req: axum::extract::Request`), returning `Result<(StatusCode, Json<ExecutionDto>), Response>`:
  1. `require(WF_MANAGE)`.
  2. `q.breakpoints.is_some()`: 400 `replay_breakpoints_unsupported`, "breakpoints are not supported yet; they arrive with their own package, after 7c (pause and resume)".
  3. `let choice = source_choice(..)?`.
  4. `detail = executions.get` (404). Loop iteration: today's 400. Flow NULL: 404. `flow = startable_flow` (404/400/409). Not terminal: today's 409.
  5. `let source = source_text(..)?`.
  6. Kind:
     - no `taskRunId`: `Whole`;
     - a value that does not parse as a UUID: 404 `task_run_not_found`;
     - a UUID among `detail.runs` whose history is not empty: `FromTaskRun(t)`;
     - otherwise `executions.loop_run_holding(ws, id, t)`: `Some((loop_run, loop_task))` gives 400 (uncoded): "task run {t} is inside an iteration of the Loop `{loop_task}` (task run {loop_run}) of execution {id}; replay execution {id} from that Loop task run instead". `None` gives 404 `task_run_not_found`.
  7. `let new_id = Uuid::new_v4()`. Body:
     - multipart (`is_multipart`): `receive_parts` against `parse(&source)`, then `Replace(Value::Object(inputs))` and its files;
     - else `read_json_body`: a body with `inputs` that is an object gives `Replace`; one with `inputs` null or absent, or no body, gives `Reuse`; a non-object `inputs` gives 400 (`start_inputs`' message).
  8. `worker::replay_execution(&st.db.pool, &ReplayRequest { .. })`. On `Err` or a refusal after a multipart upload, call `discard_uploads`. Map refusals with `from_replay_refusal`.
  9. Audit `workflow_execution:replay` with `{"replayed_from": id, "replacement": new, "task_run_id": <uuid|null>, "source": choice.wire(), "inputs_replaced": bool}`. Never the inputs.
  10. Read back, 201.
- **`restart_execution`** gains `Query(q): Query<RestartQuery>`, returning `Result<(StatusCode, Json<ExecutionDto>), ExecOpError>`.
  - `choice == Frozen`: 7a's path unchanged, 200.
  - Otherwise:
    1. `detail` (404); `flow = startable_flow`; `source = source_text(..)`;
    2. `rerun_gate(.., &source)`;
    3. `replay_execution` with `RestartOnSource`, `Reuse`, `new_id`, no files;
    4. audit `workflow_execution:restart` with `{"from_state", "replacement": new, "source": choice.wire()}`;
    5. 201 with the new execution.
- **Router:** the replay route gets `.layer(axum::extract::DefaultBodyLimit::disable())`, with the start route's comment.
- **OpenAPI.**
  - Replay: `params(WorkspacePath, ("id" ..), ReplayQuery)`; `request_body(description = "Optional. ..", content((StartExecutionRequest = "application/json"), (StartExecutionMultipart = "multipart/form-data")))`; responses 201, 400 (every code with its meaning), 401, 403, 404 (`task_run_not_found`, `flow_version_not_found`, uncoded), 409 (not terminal, disabled, `inside_working_directory`, `files_pruned`, `after_execution_running`), 413, 415, 500, 503.
  - Restart: params `RestartQuery`; 200 (in place) and 201 (new execution, label `system.restarted`, `attempt_number + 1`); 400 now lists `invalid_source` and `replay_task_not_in_source`, without `restart_revision_unsupported`; 404 adds `flow_version_not_found`.
  - Rewrite the handler docs ("It is a start, deliberately, and from the FROZEN source") to say what the three sources are and that the frozen source stays the default.

**Tests** (`tests/replay_routes.rs`). The module doc lists the two routes and the run command. Copy `Flaky`, `flow`, `setup`, `start`, `flow_of` from `tests/restart_routes.rs:28-120`. Add the `database_url_is_exported_rather_than_read_from_dotenv` test. Each refusal is paired with a control call that is accepted:

1. `replay_from_a_task_answers_201_with_the_new_execution`: body `{}`, `?taskRunId=<b>`. 201, `replayed_from_execution_id` = old, new id.
2. `revision_is_version` (the Kestra alias):
   - publish version 1 with `INSERT INTO workflow.flow_versions (flow_id, version, snapshot, created_by) VALUES ($1, 1, jsonb_build_object('source', $2::text), $3)`;
   - `?revision=1`: 201, `source` = the version's text;
   - `?revision=1&source=current`: 400 `invalid_source`;
   - `?source=version:x`: 400 `invalid_source`;
   - `?revision=9`: 404 `flow_version_not_found`.
3. `source_current_runs_the_draft`: update `workflow.flows.source`, then `?source=current`. The new execution's source is the draft.
4. `breakpoints_are_refused`: `?breakpoints=a` gives 400 `replay_breakpoints_unsupported`; nothing written (execution count); control without it gives 201.
5. `a_foreign_or_unknown_task_run_is_404`:
   - a run of another execution of the same workspace;
   - a run in another workspace;
   - `?taskRunId=nope`.

   Each gives 404 `task_run_not_found`.
6. `a_task_run_inside_a_loop_iteration_names_the_loop`: 400, and the message contains the `Loop` task id and the root id.
7. `a_task_missing_from_the_source_is_400`: `?source=current` after removing `b` from the draft. 400 `replay_task_not_in_source`, and the message contains `` `b` ``.
8. `inside_working_directory_is_409`.
9. `new_inputs_by_json_and_by_multipart`:
   - JSON `{"inputs":{"n":5}}`: 201;
   - `{"inputs":{"n":0}}`: 400 naming `n`;
   - multipart with a text part `n=7` (`common::send_raw`, boundary written by hand as in `tests/inputs_start.rs`): 201, inputs `{"n":7}`.
10. `workflow_read_is_refused`: 403 for a `workflow:read`-only user, on both routes.
11. `the_audit_names_the_task_run_the_source_and_whether_inputs_changed`: `common::audit_mutations(&app)` has `workflow_execution:replay` with the four fields and no `inputs` key.
12. `restart_on_a_source_answers_201_and_is_audited`: `?source=current`. 201, a new id, `system.restarted` label; audit `workflow_execution:restart` with `replacement` and `source`. `?source=frozen` and no query keep 200 in place.
13. `the_egret_sql_gate_applies_to_the_chosen_source`: a non-creator replaying `?source=current` where the draft adds `egret.sql` gets 400, nothing written; the frozen control gives 201.
14. `the_entitlement_gate_covers_replay` (as `the_workflows_entitlement_gates_these_routes`).

**Updates to existing tests:**
- `tests/restart_routes.rs:253`: the `revision` case becomes `?revision=abc` → 400 `invalid_source`, plus `?revision=1` with no published version → 404 `flow_version_not_found`.
- `tests/executions_routes.rs:2100-2146`: the expected labels gain `{"key":"system.replay","value":"true"}` (spec D10).

**Steps.**
- [ ] Write the tests and run `cargo test -p egret-workflow --test replay_routes` (red).
- [ ] Implement.
- [ ] Run `cargo test -p egret-workflow --test replay_routes --test restart_routes --test executions_routes --test routes --test inputs_start --test file_inputs --test secret_inputs`.
- [ ] Run `cargo clippy -p egret-workflow --all-targets -- -D warnings`.
- [ ] Run `cargo test -p egret-server committed_openapi_json_is_current`. If it is red, regenerate the committed OpenAPI file the way the test's message says (7a did this at B1) and commit it.
- [ ] Commit: `feat(workflow): replay from a task run, choose the source, change inputs; restart on a source (package 7b)`.

---

## C1. Frontend (spec D14; S9)

**Files:**
- Regenerated: `frontend/src/api/generated/**` (controller: `npm run gen:api`, never bare `orval`; commit separately as `chore(frontend): regenerate the API client (package 7b)`).
- Modify: `components/workflows/executions/actions.ts` (+ `__tests__/actions.ops.test.ts`), `routes/workflows/ExecutionDetail.tsx`, `routes/workflows/ExecutionOverview.tsx` (+ `routes/workflows/__tests__/ExecutionDetail.test.tsx`).
- Create: `components/workflows/executions/ReplayDialog.tsx` and `__tests__/ReplayDialog.test.tsx`.

Read `docs/en/frontend-rules.md` (the frontend RULEBOOK) first; every rule applies.

**`actions.ts` additions:**
```ts
export type SourceChoice = { kind: "frozen" } | { kind: "current" } | { kind: "version"; version: number };
export function sourceParam(choice: SourceChoice): string; // "frozen" | "current" | `version:${n}`
/** Replay is offered from a task run where the whole replay is (D14). */
export function showsReplayFromTask(execution: Pick<ExecutionDto, "state">): boolean; // = showsReplay
/** One sentence for each coded refusal of the replay route; `null` otherwise. */
export function replayRefusal(code: string | undefined): string | null;
```
- `replayRefusal` covers `invalid_source`, `replay_breakpoints_unsupported`, `replay_task_not_in_source`, `flow_version_not_found`, `task_run_not_found`, `inside_working_directory`, `files_pruned` and `after_execution_running`.
- `restartRefusal` adds `invalid_source`, `replay_task_not_in_source` and `flow_version_not_found`.
- A sentence for `replay_task_not_in_source` appends the server message, which names the task.

**`ReplayDialog.tsx`** (one component, two modes):
```ts
export function ReplayDialog(props: {
  ws: string;
  execution: ExecutionDto;            // its source, inputs, flow_id
  mode: "replay" | "restart";
  taskRun?: TaskRunDto | null;        // replay from this task when set
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element;
```
- **Title:**
  - "Replay this execution?";
  - "Replay from {task_id}?";
  - "Restart this execution?".
- **Source choice:** a `RadioGroup` (`components/ui/radio-group.tsx`):
  - *This run's version* (default);
  - *The flow's current version*;
  - *A published version*, with a `Select` of `useWorkflowListFlowVersions(ws, flowId)` (newest first, "Version N, {date}, {note}"). Disabled with a reason when the list is empty.
- **Restart text:** with the default source, 7a's sentence. Otherwise "Restarting on another version creates a new execution: the failed tasks run again on that version and the tasks that succeeded keep their results."
- **Change inputs** (replay only): a `Collapsible`, closed by default.
  - The chosen source's text is `execution.source` (frozen), `useWorkflowGetFlow(ws, flowId).data.source` (current), or `useWorkflowGetFlowVersion(ws, flowId, n).data.snapshot.source` (version).
  - `inputsFromSource(text)` gives the inputs. Prefill with `formFromInputs(inputs.map(i => i.id in old ? { ...i, initial: old[i.id] } : i))`, where `old` is `execution.inputs`, except:
    - a SECRET input is never prefilled (the row holds `******`) and is marked "Enter it again";
    - a FILE input left without a new file is sent as its old `kestra://` value: the server reads it from the replay chain.
  - Validate with `validateForm` before sending. A server message naming an input goes beside it (`serverErrorsByInput`).
  - An unsupported input type disables "Change inputs" with the reason, as `RunFlow` does.
- **Submit:**
  - Replay, inputs unchanged: `replay.mutateAsync({ workspace, id, params: { taskRunId, source }, data: {} })`.
  - Replay, inputs changed with JSON: `data: requestFromForm(form).json`.
  - Replay, inputs changed with multipart: `customInstance(getWorkflowReplayExecutionUrl(ws, id, params), { method: "POST", body: req.multipart })`, as `RunFlow.tsx:159-169`.
  - On 201, invalidate `workflowsKeys.executions(ws)` and navigate to the new execution's overview (no toast, FE-ACTION-2).
  - Restart: `restart.mutateAsync({ workspace, id, params: { source } })`. On 200, 7a's refresh and toast. On 201, navigate to the new execution.
  - Every refusal stays in the dialog with its sentence (FE-ACTION-3).
  - Check the generated hook's exact variable shape after `gen:api`; the names above are orval's usual (`params`, `data`).

**Wiring:**
- `ExecutionDetail.tsx`: the Replay `ConfirmAction` (`:544-552`) and the Restart `ConfirmAction` (`:526-534`) are replaced by `ReplayDialog` in `mode="replay"` and `mode="restart"`. `runReplay` and `runRestart` move into the dialog. A `flow_id` of `null` keeps `canReplay`'s reason.
- `ExecutionOverview.tsx` `TaskRuns`:
  - the action cell becomes a `DropdownMenu` with the trigger `aria-label={`Actions for ${run.task_id}`}`;
  - item "Change state" when `changeable` (7a's predicate);
  - item "Replay from this task" when `canManage && showsReplayFromTask(execution) && canReplay(execution).ok && !isLoopIteration(execution)`;
  - the cell shows when either item does;
  - the second item opens `ReplayDialog` with `taskRun={run}`.

**Tests** (vitest):
- `actions.ops.test.ts`: `sourceParam` for the three kinds; every code of `replayRefusal` gives a distinct non-null sentence; an unknown code gives `null`.
- `ReplayDialog.test.tsx`:
  - the default source sends `source: "frozen"` and `data: {}`;
  - choosing a version sends `source: "version:2"`;
  - the versions select is disabled with its reason when there are none;
  - Change inputs is prefilled from `execution.inputs` and a SECRET is not;
  - a changed input sends `data.inputs`;
  - a `replay_task_not_in_source` 400 shows its sentence and keeps the dialog open;
  - restart mode with `current` shows the "creates a new execution" text and navigates on 201.
- `ExecutionDetail.test.tsx`: the row menu shows "Replay from this task" for a `SUCCESS` execution and not for a running one, nor for a loop iteration. Update the snapshot at `:849` (the row text no longer ends "Change state").
- [ ] Run `npm run typecheck`, `npm run lint`, and the touched vitest files. The controller provides `node_modules`: borrow it, never install.
- [ ] Commit: `feat(workflows): replay from a task, choose the version, change inputs; restart on a version (package 7b)`.

---

## Z. Docs, bookkeeping, gates and live check

- [ ] **Spec** `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`:
  - §5 gains the divergences the user confirmed: 6 (S1), 7 (S2), 8 (S4), 9 (S10), and S11's note on reordered lists;
  - a new §7 "As built", 7a's shape: commits per task, measured counts, deviations and their reasons, the ratchet at T0 and at the end.
- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`: a dated note under row #7 that 7b is done. Appendix §5's rows for replay from a task, replay with revision and inputs, and restart with revision become supported, with route evidence.
- [ ] **Vietnamese copies:** `docs/vi/specs/2026-11-14-workflow-replay-from-task-design.md` and `docs/vi/plans/2026-11-14-workflow-replay-from-task.md` (bilingual convention). A Sonnet subagent may translate.
- [ ] **`egret-data/docs`** (Platform, Workflows, executions, Run actions): replay from a task, the three sources, change inputs, restart on another version, every refusal, and fresh screenshots. The controller opens that PR.
- [ ] **Gates**, one at a time:
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow --no-fail-fast`: report passed, failed and ignored; only `sql_starrocks_live` may fail, for want of an engine;
  - `cargo test -p egret-config -p egret-server -p egret-metastore`, including `committed_openapi_json_is_current`;
  - the ratchet and census: 301/25/25/0 and 295/25/21;
  - frontend `npm run typecheck`, `npm run lint`, vitest.
- [ ] **Live check** on a native `egret serve` against the worktree's own containers (never the shared demo stack's DB). Before it, grep the binary for the new code string `replay_task_not_in_source` (memory: the build cache ships a stale binary). In a browser, as `admin@egret.local`, light and dark (7a's owed FE-TEST-2):
  1. Run `a, b (fails once), c`. Use **Replay from this task** on `b`; watch the new execution run `b` and `c` and keep `a`'s outputs.
  2. Edit `c` in the draft, then replay from `b` with **The flow's current version**; watch the edited `c` run.
  3. Restart a failed execution on a published version; watch a new execution open, labelled `system.restarted`.

  Take the screenshots for the docs.
- **Accept:** every gate passes; the ratchet is unchanged; the live check passes with screenshots.
