# Workflow package 7c: pause by hand, resume with onResume inputs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a person with `workflow:manage` can pause a running execution by hand (runs already running finish, nothing new starts), resume it, and resume a `Pause` task with typed `onResume` inputs that the flow reads as `outputs.<pause>.onResume.<id>`. A `resume/validate` route checks those inputs without resuming. The semantics are Kestra's (`ES.pause` `ES:907-916`, `ES.resume` `ES:880-897`, `EC:1854-1906`, `Pause.java:193`, `:305-330`), except where spec §5 and §"Spec corrections" below record a divergence.

**Architecture:**
- A pure module, `executor::manual_pause`, owns D1's rule (`paused_by_hand`), D3's accepted states, the resume-by-hand command, the choice of the first waiting `Pause`, and the `{onResume, resumed}` payload. `executor::drives_forward` is false for a paused-by-hand execution, so `advance` and the worker's update pass stop together (D2).
- `models::Pause` gains `on_resume`. Publish checks each entry with the flow `inputs:` checks, under the path `tasks.<pause>.onResume.<id>`. The `not_honoured` warning goes, and the property-treatment table is re-measured.
- A new worker module, `worker/pause.rs`, holds `pause_by_hand` and `resume_with`. Both run inside the execution lock (`claim::load`) and end in `claim::settle`. `resume_with` resolves the posted values with the start's input check against the chosen `Pause`'s `onResume`. It writes `{onResume, resumed}` into that run's `outputs` (SECRET values sealed into `secret_outputs`), indexes uploaded files under the `Pause` run, releases the pauses as today, and settles.
- `claim::drive` stops rendering while the execution is paused by hand, and the waker stops waking it.
- Routes: `POST …/pause`; `POST …/resume` with an optional JSON or multipart body; `POST …/resume/validate`. Also coded refusals, the D7 creator gate, the D9 trash gate, audit, OpenAPI, and `ExecutionDto.paused_by_hand`.
- Frontend: a Pause button and dialog, a `ResumeDialog` with the Run-flow inputs form, and a "Paused by hand" note in the header.

**Tech Stack:** Rust (axum, sqlx 0.9, Postgres, utoipa) in `crates/egret-workflow`; React 18 + TypeScript + orval client + vitest in `frontend/`.

Spec: `docs/en/specs/2026-11-15-workflow-pause-resume-design.md`. Its Q1, Q2 and D1–D13 are binding. Where this plan finds a spec premise wrong, it says so under "Spec corrections" and does **not** change the decision silently. Items S1, S2, S3 and S7 must be put to the user before T3 merges. Branch `feat/workflow-pause-resume`, one PR, base `origin/main` `7b19481b1` (spec commit `940b0f5fb`).

Tasks: T0 (baseline), T1–T5 (engine and API), T6 (client), T7 (frontend), T8 (docs in this repo). The user docs repo and the live check are the controller's (§"After the tasks").

**Review is not a task of this plan.** The controller reviews each task between tasks and runs one whole-branch review after T8. Fixes are recorded in spec §7 "As built".

---

## Spec corrections

Each item says where the spec's premise is wrong or incomplete, gives the evidence, and names the narrowest fix. The plan implements the line marked **Plan**. Items marked **USER** need the user's confirmation; the others only inform.

- **S1 (D3: "Pausing the loop root stops it starting new iterations") — USER.**
  - In egret a `Loop` starts its next iteration from the worker, not from `advance`. When an iteration ends, `spawn::on_execution_terminal_inner` loads the loop root under its lock and, on `Next::Start`, calls `spawn_one` directly (`worker/spawn.rs:735-746`). It never asks `drives_forward`. Only the first batch goes through `advance` (`executor/flowable/loop_.rs:21-36`, "only the FIRST batch is spawned").
  - Kestra does the same. `LoopExecutionEventMessageHandler.handleTerminated` (`executor/.../handler/LoopExecutionEventMessageHandler.java:94-182`) emits the next `loopExecution` under the parent's lock, whatever the parent's state. Only `ExecutorService.process` checks `canBeProcessed()` (`ExecutorService.java:171`, `ExecutorContext.java:67-70`), and this handler does not go through it.
  - **Plan:** follow Kestra. While the loop root is paused by hand, each iteration that ends starts the next one, up to the loop's values. The `Loop` run completes when the last iteration ends. Nothing after the `Loop` starts until resume. T3 test 6 pins this.
  - If the user wants the spec's sentence instead: gate `Next::Start` in `on_execution_terminal_inner` on `executor::paused_by_hand(&state)`, and make `resume_with`'s by-hand branch start `min(concurrencyLimit − running, remaining)` iterations for each `RUNNING` `Loop` run. That is one more task with its own tests, and it is a recorded divergence from Kestra.
- **S2 (D6: where FILE `onResume` inputs are stored) — USER.**
  - `resume/{task_run_id}/{input}/{file}` is not an execution file URI. `storage::uri::execution_file` accepts only `inputs/…`, `tasks/…`, `trigger/…` or a single file name (`storage/uri.rs:291-317`); anything else is "`'resume' is not an execution file tail`". An unparseable URI is `Unreadable::Malformed` in `storage::index::resolve_readable_many` (`storage/index.rs:228-250`). So no later task and no UI download could read the file.
  - A readable file must also have a row in `workflow.execution_files` (`index.rs:258-283`), which the spec does not plan.
  - **Plan:**
    - Store each file at `tasks/{pause-task-slug}/{pause_run_id}/resume/{request_id}/{input}/{file}`. That is a `Tail::Task` URI under the `Pause` run's own task prefix (`uri::task_prefix`, `uri.rs:393-418`). It cannot overwrite a start input, which is what D6 wants.
    - Index the files with `storage::index_task_files(conn, ws, pause_run_id, files)` inside the resume transaction (`storage/mod.rs:390-435`).
    - The `{request_id}` segment (a fresh UUID per request) means a refused or raced resume deletes only its own files, never a file a concurrent winning resume stored.
    - Spec §5 divergence 6 is reworded at T8.
- **S3 (D10: "a `Pause` run's timer cannot fire on a paused-by-hand execution, because … no run is then `PAUSED`") — USER.**
  - This is false for two `Pause`s that end at different times. In `two_pauses_in_one_parallel_resume_independently` (`executor/mod.rs:8365-8445`), after `quick` ends the execution reads `… PAUSED RESTARTED RUNNING` while `slow` is still `PAUSED` with a deadline. A manual pause is then accepted (the state is `RUNNING`), and D1 classifies the execution as **not** paused by hand, because a run is `PAUSED`.
  - What follows: `slow`'s timer fires, `pause_mark` moves the execution to `RESTARTED` and then `RUNNING` (`executor/mod.rs:387-424`), and the manual pause ends. A resume before that releases `slow`, with `slow`'s `onResume`.
  - Kestra is the same: `ES.resume` finds the `PAUSED` task run and resumes it.
  - **Plan:** no code change. T3 test 9 pins the behaviour, and the user docs say it. The alternative, if the user prefers: refuse a manual pause with 409 `not_running` while any run is `PAUSED`.
- **S4 (D2: what else moves a paused execution) — inform.**
  - `claim::drive` (`worker/claim.rs:5234-5310`) runs `render::render_run_if` (`render.rs:1718-1751`) and `render::render_expressions` (`render.rs:1973-…`) between `advance` rounds, and neither is gated. Only `render_updates` is (`render.rs:1853`).
  - Under a manual pause, a `LoopUntil` whose body ended would have its condition rendered at pause time. A render failure would fail it (`render.rs` `fail`), which moves a run.
  - **Plan:** `drive` returns before the three render passes when `executor::paused_by_hand(state)` is true (T3). This stays within D2: the executor and the render surface stop together.
- **S5 (D2: the waker) — inform.**
  - `claim::due_retries` (`claim.rs:790-840`) wakes an execution that holds a due `RETRYING` run unless it is terminal or `KILLING`. A paused-by-hand execution can hold one: a run that failed during the pause, or a parallel sibling's retry. `advance` then answers nothing (D2), and the waker would wake the execution every tick for nothing.
  - **Plan:** `due_retries` excludes a `PAUSED` execution that has no `PAUSED` run (T3, with a test).
- **S6 (D5: "Undeclared keys give 400, as on start") — inform.**
  - The start does **not** refuse an undeclared JSON key; it drops it (`inputs_check.rs:175-195`, test `undeclared_keys_are_dropped_and_order_decides_the_first_error`, `inputs_check.rs:1541`). Only a multipart part that names no declared input is refused (`routes.rs:3099-3103`).
  - **Plan:** keep D5's 400 for resume, for JSON and multipart alike, with the message naming the key. The words "as on start" are wrong. This is the no-silent-drops rule.
- **S7 (SECRET `onResume` inputs; the spec is silent) — USER (confirm).**
  - The corpus declares one (`oracle-exec/corpus/valids/pause_on_resume.yaml`, `secret_pause: SECRET`). Writing it into `task_runs.outputs` would store a secret in clear text and show it in every `ExecutionDto`.
  - **Plan:**
    - Mask it to `******` in `outputs`.
    - Seal the clear values into `task_runs.secret_outputs` with `secret_inputs::split_outputs` (`secret_inputs.rs:337-363`), the seal SECRET outputs already use (migration 0214).
    - Reveal them in the render surface with a new `onResume` arm in `render::reveal_run_outputs` (`render.rs:2149-2167`), so a later task reads the clear value and its log masks it.
    - With no `secrets.master_key`, the resume is refused with 400 naming the input, as a SECRET start input is.

    The alternative is to refuse SECRET in `onResume` at publish. That would move the corpus census (`pause_on_resume` stops publishing) and diverge from Kestra.
- **S8 (D4: the path `tasks[..].onResume[i]`) — inform.** egret's finding paths name inputs by id: `inputs.<id>.<field>` (`spec/validate.rs:1418-1531`). `onResume` findings therefore read `tasks.<pause>.onResume.<id>.<field>` (`<pause>` is `spec::path::task_path`'s dotted path). Same intent, egret's spelling.
- **S9 (§3: "`release_now` gains the outputs payload") — inform.** `ExecutionState` holds no outputs, and the writer never touches `outputs` (`claim.rs` `persist`). `release_now` (`executor/flowable/pause.rs:94-109`) stays as it is. The payload is the pure `executor::resumed_outputs`, written by the worker with one `UPDATE` in the resume's transaction, before the settle.

---

## Investigation record (facts the implementers must not re-derive)

### Kestra (`/Users/1pro-l4r8/Documents/github/data-portal/kestra`, read-only)

- `ExecutorContext.canBeProcessed()` is false for `isPaused()` (`executor/src/main/java/io/kestra/executor/ExecutorContext.java:67-70`), checked in `ExecutorService.process` (`ExecutorService.java:171`).
- Loop iterations are started by `LoopExecutionEventMessageHandler.handleTerminated` (`:94-182`), which never asks the parent's state (S1).
- The rest is cited in spec §1 and §2.

### egret

- **The executor's gate.**
  - `advance` (`executor/mod.rs:191-213`): when `!drives_forward(state)` it answers `advance_killing` for `KILLING`, `after_execution::drive` for a terminal state, and `Vec::new()` for anything else. A manual `PAUSED` falls into the last arm. Nothing below it runs: no `reopen_due`, no walk, no `retry::scan`, no `pause_mark`.
  - `drives_forward` is `executor/mod.rs:366-371`. Its callers are `advance` and `render::render_updates` (`render.rs:1853`, together with `drives_after`).
- **What a worker still does under a manual pause (D2).**
  - A leaf is claimed only when it is `RUNNING` (`claim::claim`, `claim.rs:515-555`), so a run already `RUNNING` is claimed and run.
  - Its outcome is written by `apply_outcome_on` (`claim.rs:2918-3035`): `UPDATE … SET outputs, message`, then `SetRunState`/`CompleteRun`, then `settle`, whose `advance` creates nothing.
  - A `Subflow` child that ends completes its parent's run through `subflow::on_child_terminal` → `Job::Complete` → `subflow::open_completion`, which records the run's terminal and settles. Nothing else moves.
  - A `Loop` iteration that ends starts the next one (S1).
  - Kill: `worker/kill.rs:191-245` loads any non-terminal execution and applies `executor::kill::kill_leaves` (`executor/kill.rs:226-247`); then `advance_killing` (`:79-87`) closes the flowables. This works on `PAUSED` as on `RUNNING`.
- **D1 holds.**
  - `Command::PauseRun` is the only producer of a `PAUSED` run, and `pause_mark` puts the execution in `PAUSED` only in a batch that holds a `PauseRun` (`executor/mod.rs:387-424`).
  - The execution leaves `PAUSED` in the batch that ends the last waiting run: `SetRunState` on a `PAUSED` run gives `RESTARTED`, and `RestartLoop` gives `RUNNING`.
  - 7a's restart of a `PAUSED` execution resets the `PAUSED` runs and writes `RESTARTED` (`executor/restart.rs:68-116`).
  - So a `PAUSED` execution with no `PAUSED` run is reachable only by the new manual pause. The converse, a `RUNNING` execution with a `PAUSED` run, is S3.
- **Today's resume.**
  - `claim::resume_by_hand` (`claim.rs:4535-4552`) loads under the lock and answers `NotPaused` unless the last state is `PAUSED`. It calls `executor::release_paused_pauses` (= `flowable::pause::release_now`, re-exported `executor/mod.rs:100`), answers `NoPauseWaiting` when that released nothing, and otherwise settles.
  - `release_now` marks every `PAUSED` run whose `resume_with` is `Never` or `Terminal` (a `Pause`, never a `LoopUntil`'s `Iterate`) to end `SUCCESS` now.
  - The route is `routes.rs:3198-3276`. Its router line is `routes.rs:4887-4890`, its OpenAPI path `routes.rs:4943`, and its audit `workflow_execution:resume` `{from_state}` (`routes.rs:3261-3268`).
- **Run outputs.**
  - `workflow.task_runs.outputs` (jsonb), `secret_outputs` (bytea, migration 0214), `outputs_bytes` (bigint, `Loop` only).
  - The render surface reads every run's `outputs` and `secret_outputs` in run order (`render.rs:594-620`), whatever the run's state, through `reveal_run_outputs` (`render.rs:2149-2167`). That function knows the `Subflow` shape (`outputs.outputs`) and the `Loop` shape.
- **The input check.**
  - `claim::checked_inputs` (`claim.rs:4243-4249`) is `inputs_check::resolve(&flow.inputs, posted, &start_renderer(flow))`.
  - `claim::input_outcomes` (`:4255-4260`) is `resolve_all`.
  - `start_renderer` (`:4263-4295`) binds `flow`, `vars` and `inputs`, with `null` for each defaulted input of `flow.inputs`.
  - Re-exported `pub(crate)` at `worker/mod.rs:118`.
- **FILE inputs.**
  - `storage::inputs::upload_input` (`storage/inputs.rs:97-167`) checks the type, the name and the extension, builds `uri::input_uri`, streams, and heads the object.
  - `check_file_inputs` (`:240-…`) reads only `flow.inputs`.
  - `FileStore::execution_prefix` is `storage/mod.rs:255-270`.
  - Routes: `receive_parts` (`routes.rs:3006-3051`), `discard_uploads` (`:3054-3066`, deletes the whole execution prefix), `read_parts` (`:3069-3137`, "the flow declares no input" at `:3099-3103`).
- **JSON bodies.** `read_json_body` (`routes.rs:2593-2640`: 2 MiB, a headerless body read as JSON) and `start_inputs` (`:2507-2524`).
- **Validate.**
  - `validate_execution_inputs` (`routes.rs:2905-2930`) answers `ValidateInputsDto` (`:2792-2803`) built by `validate_inputs_answer` (`:2828-2880`).
  - `declared_inputs(source)` (`:2807-2824`) echoes the YAML's own input objects.
- **Gates.**
  - `startable_flow` (`routes.rs:2431-2458`): 404 trash, 400 unparseable draft, 409 disabled.
  - `rerun_gate` (`:2476-2504`) runs it plus `sql_principal::check_start` (`worker/sql_principal.rs:90-113`, door 2: with `started_by = Some(caller)`, only the creator passes on a flow that runs `egret.sql`; `runs_sql` decides that).
  - A loop iteration: `ExecOpError::loop_iteration(id, root, what)` (`routes.rs:3873-3878`), used by 7a's restart (`routes.rs:4196-4201`).
  - `ExecOpError` is `routes.rs:3854-4033`.
- **`Pause` model and its gap.**
  - `models::Pause` is `models/task.rs:739-761`.
  - The warning comes from `spec/keys.rs:171` (`lift_of` → `Package(9)`). Its tests: `keys.rs:1344-1347`, `:1539-1556`, `:1895-1914`.
  - The key walker's `Loop.outputs` block (`keys.rs:984-997`) is the model for walking `onResume` entries with `input(...)` (`keys.rs:897-935`, whose paths are hard-coded `inputs.{id}`).
  - `executor::pause_gap`/`pause_plan_gap` (`executor/plan.rs:920-990`) name `tasks`, `errors`, `finally`, `behavior` and `onPause` only. `onResume` was never a gap, only a warning.
  - `spec::validate`'s `Pause` arm is `validate.rs:2302-2338`. The flow-input checks are `validate_inputs` (`validate.rs:1381-1470`, run gate, called at `:791`), `input_save_findings` (`:1499-1541`, save-only, called at `:398`) and `input_dependency_findings` (`:1543-1583`). All three write `inputs.{id}` paths.
  - `spec::path::every_task` (`spec/path.rs:69`) and `find_task` (`:49`) walk all four root lists at every depth.
- **Property treatment.**
  - `spec/schema/property-treatment.json:648` reads `"onResume": "ignored"`. The file is measured by `spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today` and rewritten with `EGRET_WRITE_PROPERTY_TREATMENT=1` (`measure.rs:1-7`).
  - A property of a flowable's typed model struct is in the read set, so adding the field makes it `honoured` (`measure.rs:19-27`).
  - `spec/schema/README.md:214` (counts) and `:230` (the `Pause` row) restate it.
- **Census and ratchet.**
  - `oracle-exec/driver.rs:854` `const LIVE: usize = 295`.
  - Ratchet `(301, 276, 25)` at `tests/executor.rs:889`.
  - The three `onResume` corpus flows (`valids/pause_on_resume.yaml`, `pause_on_resume_optional.yaml`, `resume-validate.yaml`) publish today, with a warning. Their entries are unbounded pauses that never reach the golden. So no count is expected to move, but T0 and T2 **measure**; they do not assume.
- **Migrations.**
  - No migration is needed. `PAUSED` is a valid execution state. 0142's CHECK (`state <> 'PAUSED' OR resume_with IS NOT NULL`) binds `PAUSED` **runs** only, and a manual pause writes no run. Outputs use existing columns.
  - The top migration is `0228_compile_run_migration_complete.sql`. If one becomes necessary, flag it and number it above main and every open PR (memory: migration numbers are a shared namespace).
- **OpenAPI.**
  - `crates/egret-server/src/lib.rs:855-866` `committed_openapi_json_is_current`, whose message is "run `egret openapi > openapi.json`": `cargo run -p egret-bin -- openapi > openapi.json`.
  - `generated_client_was_emitted_from_the_committed_spec` (`lib.rs:~940-990`) reads `frontend/src/api/generated.digest.json`, which `npm run gen:api` writes.
  - Operation ids are the handler names (`openapi.json:33819` `"resume_execution"`). orval prefixes the tag: `useWorkflowResumeExecution`, `getWorkflowResumeExecutionUrl`.
- **Frontend.**
  - `components/workflows/executions/actions.ts`: `showsResume`/`canResume` `:19-44`, `isLoopIteration` `:193-195`.
  - `routes/workflows/ExecutionDetail.tsx`:
    - `ExecutionActions` `:366-542`;
    - the Resume `ConfirmAction` `:489-497`;
    - `runResume` `:391-400`;
    - the header `status` slot `:284`.
  - `ReplayDialog.tsx` posts JSON through the hook and multipart through `customInstance(getWorkflowReplayExecutionUrl(...), { method: "POST", body: FormData })` (`:369-386`).
  - The Run-flow form is `components/workflows/run/inputModel.ts`:
    - `inputsFromSource` `:120-164`;
    - `formFromInputs`, `validateForm`, `requestFromForm`;
    - `validationRequestFromForm` `:500-502`;
    - `validationView` `:518-534`;
    - `serverErrorsByInput` `:550`.

    The controls are `inputControls.tsx` (`InputField`).
  - Test harness: `components/workflows/executions/__tests__/ReplayDialog.test.tsx:1-80` (a `Server` class stubbing `fetch`) and `routes/workflows/__tests__/ExecutionDetail.test.tsx:92-122` (the `resume` reply), with the resume tests at `:384-482`.

---

## Global Constraints

- Spec Q1, Q2 and D1–D13 are binding, as amended only by the Spec corrections above once the user has confirmed S1, S2, S3 and S7. When the spec and egret's code disagree in a way not listed above, **stop and report**; do not improvise. Kestra's source is read-only.
- **Code search:** CodeGraph first: `codegraph_explore` with `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Use grep only for literal strings. `rg` is not installed.
- **Worktree:** `/Users/1pro-l4r8/Documents/github/data-portal/egret-execops`, branch `feat/workflow-pause-resume`.
  - Never switch branches. Never use `git stash`.
  - No network: no `git push`/`fetch`/`pull`, no `gh`, no `npm ci`/`npm install`.
- **Environment (every cargo command):**
  ```bash
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target
  export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465
  ```
  - **Run one cargo command at a time**; never two test suites at once.
  - Run `df -h` before a full build. Clean `target/debug/incremental` rather than starting a second target dir.
  - Never touch port 5432 or the `egret-dev-*` / `egret-demo-*` containers.
- **Frontend `node_modules`:** if `frontend/node_modules` is absent, symlink it, never install:
  ```bash
  ln -s ../../egret-sp2b/frontend/node_modules /Users/1pro-l4r8/Documents/github/data-portal/egret-execops/frontend/node_modules
  ```
  Remove the symlink at the end of T7 (it is gitignored; never commit it). `npm run gen:api`, never bare `orval`.
- **Gates** (each task runs the ones it touches; T8 runs them all, one at a time):
  - `cargo fmt --check`
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`
  - `cargo check -p egret-bin`
  - `cargo test -p egret-workflow --no-fail-fast` (7 `sql_starrocks_live` failures are expected on this host; anything else is a failure)
  - `cargo test -p egret-config -p egret-server -p egret-metastore`
  - `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows` (in `frontend/`)
- **Commits:** Conventional Commits, each message ending with
  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
  No `Closes #` in commits. Never commit scratch files or `.superpowers/`. Scratch output goes to the session scratchpad (`$SCRATCH` below), never into the repo.
- **Test binaries:** every new `#[sqlx::test]` binary calls `egret_test_db::assert_database_url_is_exported()` in its setup **and** has
  ```rust
  #[test]
  fn database_url_is_exported_rather_than_read_from_dotenv() {
      egret_test_db::assert_database_url_is_exported();
  }
  ```
  This plan's new binaries are `tests/pause_resume.rs` and `tests/pause_resume_routes.rs`.
- **Red first:** each task writes its failing tests before the code, and runs them to see them fail.
- **Ratchet and census:** **301 / 25 / 25 / 0** and census **295 / 25 / 21**, re-measured at T0. T2 re-measures them. A change is measured and written down with its reason, never guessed. Any change outside T2 is a regression to report.
- No new `std::env::var`. If one becomes necessary, add it to `egret-config`'s `ENV_OUTSIDE_CONFIG` and run `cargo test -p egret-config`.
- No migration (D12). If one becomes necessary, stop and report.
- **Words in code and messages:**
  - New refusal codes, exactly: `not_running` (409), `not_paused` (409), `resume_inputs_unexpected` (400).
  - Audit actions: `workflow_execution:pause` `{from_state}`; `workflow_execution:resume` `{from_state, by_hand, task_run_id, inputs_given}`. Never the input values.
  - Output keys: `onResume`, `resumed` (`by`, `on`, `to`).
- Code, comments and docs in English.

---

## File structure

| File | Change | Task |
|---|---|---|
| `crates/egret-workflow/src/executor/manual_pause.rs` | **new**: `paused_by_hand`, `paused_by_hand_of`, `NotRunning`, `pause_by_hand_commands`, `resume_by_hand_commands`, `first_waiting_pause`, `resumed_outputs`; tests | T1 |
| `crates/egret-workflow/src/executor/mod.rs` | `pub mod manual_pause;`, re-exports; `drives_forward` learns D1 | T1 |
| `crates/egret-workflow/src/models/task.rs` | `Pause.on_resume` | T2 |
| `crates/egret-workflow/src/spec/validate.rs` | input checks take a path prefix; `onResume` checked in run and save passes | T2 |
| `crates/egret-workflow/src/spec/keys.rs` | `lift_of` row removed; `input` takes its list path; `Pause.onResume` entries walked; tests | T2 |
| `crates/egret-workflow/src/spec/schema/property-treatment.json`, `spec/schema/README.md` | re-measured | T2 |
| `crates/egret-workflow/src/worker/pause.rs` | **new**: `Paused`, `pause_by_hand`; then `ResumeRequest`, `ResumeRefused`, `resume_with` | T3, T4 |
| `crates/egret-workflow/src/worker/claim.rs` | `drive` render gate; `due_retries` exclusion; `Resumed` gains `ByHand`, `Refused`; `resume_by_hand` delegates; `checked_inputs_for`, `input_outcomes_for`, `input_renderer` | T3, T4 |
| `crates/egret-workflow/src/worker/mod.rs` | `mod pause;` and re-exports | T3, T4 |
| `crates/egret-workflow/src/worker/render.rs` | `reveal_run_outputs` `onResume` arm | T4 |
| `crates/egret-workflow/src/worker/sql_principal.rs` | `check_resume_inputs` | T5 |
| `crates/egret-workflow/src/secret_inputs.rs` | `reveal_keyed_outputs` (generalises `reveal_subflow_outputs`) | T4 |
| `crates/egret-workflow/src/storage/uri.rs`, `storage/mod.rs`, `storage/inputs.rs` | `resume_input_uri`; `FileStore::resume_root`/`resume_prefix`; `upload_resume_input`; `check_file_values` | T4 |
| `crates/egret-workflow/tests/pause_resume.rs` | **new** worker-level tests | T3, T4 |
| `crates/egret-workflow/src/routes.rs` | `pause_execution`, `resume_execution` (body), `validate_resume_inputs`, `Waiting`, `live_flow_of`, `resume_gate`, `PartSink`, `receive_parts`/`read_parts`/`discard_*` generalised, codes, `ExecutionDto.paused_by_hand`, OpenAPI, router | T5 |
| `crates/egret-workflow/tests/pause_resume_routes.rs` | **new** route tests | T5 |
| `openapi.json` | regenerated | T5 |
| `frontend/src/api/generated/**`, `frontend/src/api/generated.digest.json` | `npm run gen:api` | T6 |
| `frontend/src/components/workflows/executions/actions.ts` (+ tests) | `showsPause`, `canPause`, `canResume`, `pauseRefusal`, `resumeRefusal` | T7 |
| `frontend/src/components/workflows/run/inputModel.ts` | `runInputFrom` exported from `inputsFromSource` | T7 |
| `frontend/src/components/workflows/executions/ResumeDialog.tsx` (+ test) | **new** | T7 |
| `frontend/src/routes/workflows/ExecutionDetail.tsx` (+ test), `routes/workflows/queryKeys.ts` | Pause button and dialog, ResumeDialog, header note, `resumeInputs` key | T7 |
| `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, `docs/en/specs/2026-10-28-workflow-inputs-design.md`, spec §5, `docs/vi/...` | dated corrections | T8 |

---

## T0. Environment and baseline

- [ ] Check the containers: `docker ps --format '{{.Names}} {{.Ports}}' | grep egret-execops` lists `egret-execops-postgres` on 5465 and `egret-execops-valkey` on 6465. If either is missing, start it as 7b's T0 did (image `postgres:16`, `egret`/`egret`/`egret_test`, host port 5465; Valkey on 6465), and report it.
- [ ] Export the three variables (Global Constraints).
- [ ] Ratchet and census: `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t0-executor.txt"`. Read the tuple and the census line from the file, not from a `tail` (memory: `tail` truncates the report).
- [ ] Property treatment: `cargo test -p egret-workflow --lib spec::schema::measure 2>&1 | tee "$SCRATCH/t0-measure.txt"` (green expected). Count the treatments with `python3 -c "import json,collections;d=json.load(open('crates/egret-workflow/src/spec/schema/property-treatment.json'));c=collections.Counter(v for r in d.values() for v in (r.values() if isinstance(r,dict) else []) if isinstance(v,str));print(c)"`. Record the counts.
- [ ] Migration: `ls migrations | tail -1` (expect `0228_compile_run_migration_complete.sql`).
- **Accept:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; treatment counts recorded; top migration 0228. Report all four as measured. No commit.

---

## T1. The pure rules: `executor::manual_pause` (spec D1, D2, D3, D5)

**Files:**
- Create: `crates/egret-workflow/src/executor/manual_pause.rs`.
- Modify: `crates/egret-workflow/src/executor/mod.rs:46-56` (module list), `:59-140` (re-exports), `:359-371` (`drives_forward` and its doc).

**Interfaces.**

Consumes: `executor::{Command, ExecutionState, RunIx, RunState, Resume, apply, advance, build_plan}`; `executor::test_support::{flow, leaf, driven, drive, last, run, T0}` (tests); `executor::release_paused_pauses` (tests).

Produces (`executor/manual_pause.rs`, re-exported from `executor`):
```rust
pub fn paused_by_hand(state: &ExecutionState) -> bool;
pub fn paused_by_hand_of(
    execution: Option<RunState>,
    runs: impl Iterator<Item = Option<RunState>>,
) -> bool;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotRunning(pub Option<RunState>);
pub fn pause_by_hand_commands(state: &ExecutionState) -> Result<Vec<Command>, NotRunning>;
pub fn resume_by_hand_commands(state: &ExecutionState) -> Vec<Command>;
pub fn first_waiting_pause(state: &ExecutionState) -> Option<RunIx>;
pub fn resumed_outputs(
    on_resume: serde_json::Map<String, serde_json::Value>,
    on: &str,
) -> serde_json::Value;
```
`drives_forward(state)` keeps its signature and also answers false when `paused_by_hand(state)`.

**Steps.**

- [ ] **Write the failing tests** (the `#[cfg(test)] mod tests` below), with the module holding only its `use` lines and the doc. Add `pub mod manual_pause;` after `pub mod exit;` in `executor/mod.rs`. Run `cargo test -p egret-workflow --lib executor::manual_pause`: it does not compile. That is the red.
- [ ] **Implement** (code below). Add to `executor/mod.rs`, beside the other `pub use` lines:
  ```rust
  pub use manual_pause::{
      first_waiting_pause, pause_by_hand_commands, paused_by_hand, paused_by_hand_of,
      resume_by_hand_commands, resumed_outputs, NotRunning,
  };
  ```
- [ ] **`drives_forward`** (`executor/mod.rs:359-371`) becomes:
  ```rust
  /// **Whether [`advance`] still drives this execution's runs forward**: it
  /// is neither terminal nor `KILLING`, nor paused by hand (package 7c D2,
  /// [`paused_by_hand`]). Every other state -- `RUNNING`, a `PAUSED` that a
  /// `Pause` or a waiting `LoopUntil` holds, `RETRYING`, `CREATED`,
  /// `RESTARTED` -- reaches the walk, which creates, starts and completes
  /// runs. `advance` branches on exactly this, and the worker's update pass
  /// (`worker::render::render_updates`, group C2 ruling D9) gates on it, so
  /// the two cannot disagree about when a sibling may still move. A manual
  /// pause reaches `advance`'s `_ => Vec::new()` arm: Kestra's
  /// `ExecutorContext.canBeProcessed()` is false for `PAUSED`.
  pub fn drives_forward(state: &ExecutionState) -> bool {
      !state
          .history
          .last()
          .is_some_and(|s| s.is_terminal() || *s == RunState::Killing)
          && !paused_by_hand(state)
  }
  ```
  In `advance`'s guard comment (`executor/mod.rs:193-200`) add one line: "A manual pause (package 7c) returns `Vec::new()` here, and `kill` and 7a's restart still reach it."
- [ ] Run `cargo test -p egret-workflow --lib executor::manual_pause` (all pass), `cargo test -p egret-workflow --lib executor::` (no regression), and the ratchet `cargo test -p egret-workflow --test executor` (301/25/25/0 unchanged).
- [ ] Commit: `feat(workflow): pure rules for pause by hand and resume (package 7c)`.

**Implementation** (`crates/egret-workflow/src/executor/manual_pause.rs`):

```rust
//! Pause by hand and resume by hand -- workflow package 7c (spec
//! `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D1-D3, D5;
//! Kestra `ExecutionService.pause`, `ES:907-916`, and `.resume`,
//! `ES:880-897`).
//!
//! Pure, like `executor::restart`: these functions say what a manual pause
//! and a manual resume write. The worker (`worker::pause`) takes the
//! execution's lock, applies them and settles.

use super::command::Command;
use super::state::{ExecutionState, Resume, RunIx, RunState};

/// **D1: paused by hand** -- the execution's last state is `PAUSED` and no
/// run's last state is (Kestra `ES.resume`'s `findFirstByState(PAUSED)`
/// empty). A `Pause` task or a waiting `LoopUntil` always leaves a `PAUSED`
/// run (`Command::PauseRun` is the only producer of one, and `pause_mark`
/// pauses the execution only in a batch holding one), so the two cannot be
/// confused.
pub fn paused_by_hand(state: &ExecutionState) -> bool {
    paused_by_hand_of(
        state.history.last().copied(),
        state.runs.iter().map(|r| r.history.last().copied()),
    )
}

/// [`paused_by_hand`]'s rule over the last states alone, for a reader that
/// holds rows rather than an `ExecutionState` (`routes::ExecutionDto`). One
/// rule, two shapes of input.
pub fn paused_by_hand_of(
    execution: Option<RunState>,
    mut runs: impl Iterator<Item = Option<RunState>>,
) -> bool {
    execution == Some(RunState::Paused) && !runs.any(|r| r == Some(RunState::Paused))
}

/// Why an execution cannot be paused by hand (D3): its last state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotRunning(pub Option<RunState>);

/// **D3**: a `RUNNING` or `RESTARTED` execution takes `PAUSED`; no task run
/// changes. Every other state is refused with itself -- `KILLING` too,
/// which Kestra's `isRunning()` accepts (spec §5 divergence 1): `PAUSED`
/// after `KILLING` would stop `advance_killing`.
pub fn pause_by_hand_commands(state: &ExecutionState) -> Result<Vec<Command>, NotRunning> {
    match state.history.last().copied() {
        Some(RunState::Running | RunState::Restarted) => Ok(vec![Command::SetExecutionState {
            state: RunState::Paused,
        }]),
        other => Err(NotRunning(other)),
    }
}

/// **D5, paused by hand**: the execution goes back to `RUNNING` (Kestra
/// `ES:880-897`), and the next `advance` drives it. Empty when the
/// execution is not paused by hand.
pub fn resume_by_hand_commands(state: &ExecutionState) -> Vec<Command> {
    if paused_by_hand(state) {
        vec![Command::SetExecutionState {
            state: RunState::Running,
        }]
    } else {
        Vec::new()
    }
}

/// **D5**: the first `PAUSED` `Pause` run in run order -- the one whose
/// `onResume` a resume reads (Kestra `findFirstByState(PAUSED)`). A
/// `LoopUntil`'s wait (`Resume::Iterate`) is skipped: a resume does not end
/// it. The same predicate as `flowable::pause::release_now`.
pub fn first_waiting_pause(state: &ExecutionState) -> Option<RunIx> {
    state.runs.iter().position(|r| {
        r.history.last() == Some(&RunState::Paused)
            && matches!(r.resume_with, Some(Resume::Never | Resume::Terminal { .. }))
    })
}

/// **D5**: the outputs of a `Pause` run released by hand (`Pause.java:305-330`):
/// `{"onResume": {...}, "resumed": {"by": null, "on": <ISO-8601>, "to":
/// "SUCCESS"}}`. `by` stays null, as in Kestra OSS (`Pause.Resumed.now()`);
/// who resumed is in the audit row. A run with no `onResume` gets `{}` so a
/// template reading it does not fail.
pub fn resumed_outputs(
    on_resume: serde_json::Map<String, serde_json::Value>,
    on: &str,
) -> serde_json::Value {
    serde_json::json!({
        "onResume": on_resume,
        "resumed": { "by": null, "on": on, "to": "SUCCESS" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_support::{driven, flow, last, leaf, run, T0};
    use crate::executor::{advance, apply, build_plan, release_paused_pauses, Plan, Tick};

    /// `advance` alone to a fixed point: no leaf ends (a worker would).
    fn rest(plan: &Plan, state: &mut ExecutionState, now: Tick) {
        for _ in 0..100 {
            let cmds = advance(plan, state, now);
            if cmds.is_empty() {
                return;
            }
            for c in &cmds {
                apply(state, c);
            }
        }
        panic!("advance did not come to rest");
    }

    fn started(src: &str) -> (Plan, ExecutionState) {
        let plan = build_plan(src).expect("the fixture flow plans");
        let mut state = ExecutionState::new(&plan);
        rest(&plan, &mut state, T0);
        (plan, state)
    }

    fn paused(state: &mut ExecutionState) {
        for c in pause_by_hand_commands(state).expect("running") {
            apply(state, &c);
        }
    }

    const PAUSE: &str = "io.kestra.plugin.core.flow.Pause";

    // 1. D1 against each kind of PAUSED.
    #[test]
    fn paused_by_hand_tells_a_manual_pause_from_every_pause_that_leaves_a_paused_run() {
        // Manual.
        let (plan, mut state) = started(&flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2))));
        paused(&mut state);
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(paused_by_hand(&state));

        // A `Pause` task.
        let (_, state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n{}", leaf("after", 2))),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // A waiting `LoopUntil` (the drive answers its condition `false`).
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    tasks:\n{}",
                leaf("body", 6)
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // Two `Pause`s in a `Parallel`.
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n"
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // The row form agrees.
        assert!(paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Running), Some(RunState::Success)].into_iter()
        ));
        assert!(!paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Paused)].into_iter()
        ));
        assert!(!paused_by_hand_of(Some(RunState::Running), std::iter::empty()));
    }

    // 2. D2.
    #[test]
    fn drives_forward_is_false_for_a_manual_pause_and_true_for_a_pause_task() {
        let (plan, mut state) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        assert!(crate::executor::drives_forward(&state));
        paused(&mut state);
        assert!(!crate::executor::drives_forward(&state));

        let (_, state) = driven(&flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")), &[]);
        assert!(crate::executor::drives_forward(&state), "a Pause task's PAUSED still drives");
    }

    // 3. D2: a run ending during the pause creates nothing and completes nothing.
    #[test]
    fn a_run_ending_during_a_manual_pause_creates_and_completes_nothing() {
        let src = flow(&format!(
            "tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}{}{}",
            leaf("a", 6),
            leaf("b", 6),
            leaf("c", 2)
        ));
        let (plan, mut state) = started(&src);
        let a = run(&plan, &state, "a");
        assert_eq!(last(&state, a), Some(RunState::Running));
        paused(&mut state);

        apply(&mut state, &Command::SetRunState { run: a, state: RunState::Success });
        let before = state.runs.len();
        assert!(advance(&plan, &state, Tick(10)).is_empty(), "nothing is created or started");
        assert_eq!(state.runs.len(), before, "b has no run");
        let seq = run(&plan, &state, "seq");
        assert_eq!(last(&state, seq), Some(RunState::Running), "the parent is not completed");

        // A run that FAILS during the pause is not retried, failed upward or handled either.
        let src = flow(&format!("tasks:\n{}{}", leaf("x", 2), leaf("y", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let x = run(&plan, &state, "x");
        apply(&mut state, &Command::SetRunState { run: x, state: RunState::Failed });
        assert!(advance(&plan, &state, Tick(10)).is_empty());
        assert_eq!(state.history.last(), Some(&RunState::Paused));
    }

    // 4. D5: resume by hand gives RUNNING, and the next advance creates the next run.
    #[test]
    fn resume_by_hand_gives_running_and_the_next_advance_creates_the_next_run() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let a = run(&plan, &state, "a");
        apply(&mut state, &Command::SetRunState { run: a, state: RunState::Success });

        let cmds = resume_by_hand_commands(&state);
        assert_eq!(cmds, vec![Command::SetExecutionState { state: RunState::Running }]);
        for c in &cmds {
            apply(&mut state, c);
        }
        rest(&plan, &mut state, Tick(20));
        let b = run(&plan, &state, "b");
        assert_eq!(last(&state, b), Some(RunState::Running));
        assert_eq!(
            &state.history[state.history.len() - 3..],
            &[RunState::Running, RunState::Paused, RunState::Running],
            "Kestra's ES.resume: PAUSED back to RUNNING, no RESTARTED"
        );

        // Not paused by hand: nothing.
        let (_, state) = driven(&flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")), &[]);
        assert!(resume_by_hand_commands(&state).is_empty());
    }

    // 5. D3: only RUNNING and RESTARTED are paused.
    #[test]
    fn pause_by_hand_refuses_every_state_but_running_and_restarted() {
        let (_, base) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        for s in RunState::ALL {
            let mut state = base.clone();
            state.history.push(*s);
            let got = pause_by_hand_commands(&state);
            match s {
                RunState::Running | RunState::Restarted => assert!(got.is_ok(), "{s:?}"),
                other => assert_eq!(got, Err(NotRunning(Some(*other))), "{s:?}"),
            }
        }
    }

    // 6. D5: the first PAUSED Pause in run order, never a LoopUntil; and the payload.
    #[test]
    fn the_first_waiting_pause_is_a_pause_in_run_order_and_its_payload_is_kestras() {
        let (plan, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: lu\n        type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{{{ false }}}}\"\n        checkFrequency:\n          interval: PT1H\n          maxIterations: 3\n        tasks:\n{}      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n",
                leaf("body", 10)
            )),
            &[],
        );
        let p1 = run(&plan, &state, "p1");
        assert_eq!(first_waiting_pause(&state), Some(p1));

        let mut map = serde_json::Map::new();
        map.insert("ok".into(), serde_json::json!(true));
        assert_eq!(
            resumed_outputs(map, "2026-11-15T10:00:00.000Z"),
            serde_json::json!({
                "onResume": {"ok": true},
                "resumed": {"by": null, "on": "2026-11-15T10:00:00.000Z", "to": "SUCCESS"}
            })
        );
    }

    // 7. D5: a released Pause still ends SUCCESS and the execution goes RESTARTED.
    #[test]
    fn releasing_a_pause_still_restarts_the_execution() {
        let (plan, mut state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n{}", leaf("after", 2))),
            &[],
        );
        let released = release_paused_pauses(&mut state, Tick(10));
        assert_eq!(released, vec![run(&plan, &state, "p")]);
        let cmds = advance(&plan, &state, Tick(10));
        assert!(
            cmds.contains(&Command::SetExecutionState { state: RunState::Restarted }),
            "{cmds:?}"
        );
    }
}
```

`RunState::ALL` is a slice of all variants (`executor/state.rs`, used by `due_retries`). If it is an array, iterate `RunState::ALL.iter()`. Check `last`/`run` in `test_support` before use; they are `pub(crate)`.

---

## T2. `Pause.onResume`: model, publish validation, warning removed, re-measured (spec D4; S8)

**Files:**
- Modify:
  - `crates/egret-workflow/src/models/task.rs:739-761`;
  - `src/spec/validate.rs` (`:384-400`, `:721-800`, `:1381-1583`);
  - `src/spec/keys.rs` (`:171`, `:897-935`, `:984-997`, tests `:1344-1347`, `:1539-1556`, `:1895-1914`);
  - `src/spec/schema/property-treatment.json` (re-measured);
  - `src/spec/schema/README.md` (`:214`, `:230`).
- If T2's measurement moves them: `oracle-exec/driver.rs:854`, `tests/executor.rs:889`.

**Interfaces.**

Consumes: `models::Input`, `spec::path::every_task`, `spec::path::task_path`.

Produces:
```rust
// models/task.rs, in `pub struct Pause`
#[serde(default, skip_serializing_if = "Option::is_none")]
pub on_resume: Option<Vec<crate::models::Input>>,

// spec/validate.rs (private)
fn validate_inputs_at(inputs: &[Input], list: &str, report: &mut ValidationReport);
fn input_save_findings_at(inputs: &[Input], list: &str, report: &mut ValidationReport);
fn input_dependency_findings_at(inputs: &[Input], list: &str, report: &mut ValidationReport);

// spec/keys.rs (private, on the walker)
fn input(&mut self, list: &str, id: &str, input: &serde_json::Map<String, Value>);
```

**Steps.**

- [ ] **Failing tests first.** In `spec/validate.rs`'s test module add the four tests below. In `spec/keys.rs`:
  - replace `pause_on_resume_warns_and_pause_tasks_is_refused_once` (`:1895-1914`) with `pause_on_resume_is_honoured_and_pause_tasks_is_refused_once`;
  - in `a_key_kestra_accepts_and_egret_ignores_is_not_honoured_and_names_its_package` (`:1539-1556`), drop the `tasks.gate.onResume` assertion and expect `got.len() == 2`;
  - in `lift_of_names_the_package_the_roadmap_gives` (`:1344-1347`), expect `None`;
  - add `an_unknown_key_in_an_on_resume_input_is_refused_at_its_path`.

  Run `cargo test -p egret-workflow --lib spec::`: red (no field, the warning is still emitted).
- [ ] **Model.** Add the field above to `models::Pause`, after `on_pause`, with the doc: "`onResume` (`Pause.java:193`): inputs a resume collects, written to this run's outputs as `onResume` (package 7c D4, D5). `None` and `Some([])` are the same: no inputs."
- [ ] **`spec/validate.rs`.**
  - Rename `validate_inputs` to `validate_inputs_at(inputs, list, report)`. Replace each `format!("inputs.{}…", input.id)` path in it with `format!("{list}.{}…", input.id)` (`:1418`, `:1449`, `:1464`). Keep `fn validate_inputs(inputs: &[Input], report: &mut ValidationReport) { validate_inputs_at(inputs, "inputs", report) }`.
  - Do the same for `input_save_findings` → `input_save_findings_at` (`:1508`, `:1515`, `:1522`, `:1534`) and `input_dependency_findings` → `input_dependency_findings_at` (`:1561`, `:1577`), with `"inputs"` wrappers.
  - In `validate_for_run`, right after `validate_inputs(&flow.inputs, &mut report);` (`:791`):
    ```rust
    // Package 7c D4: a `Pause`'s `onResume` is an input list, checked as the
    // flow's `inputs:` are, under the task's path (`tasks.<id>.onResume`).
    for (path, task) in crate::spec::path::every_task(flow) {
        if let TaskKind::Pause(p) = &task.kind {
            if let Some(list) = &p.on_resume {
                validate_inputs_at(list, &format!("{path}.onResume"), &mut report);
            }
        }
    }
    ```
  - In `save_only_findings`, after `input_save_findings(&flow.inputs, report);` (`:398`):
    ```rust
    for (path, task) in crate::spec::path::every_task(flow) {
        if let TaskKind::Pause(p) = &task.kind {
            if let Some(list) = &p.on_resume {
                input_save_findings_at(list, &format!("{path}.onResume"), report);
            }
        }
    }
    ```
  - `input_save_findings_at` calls `input_dependency_findings_at(inputs, list, report)`.
  - `every_task` returns `Vec<(String, &Task)>` with the task's dotted path (`tasks.p`), as the `Pause` arm's `path` reads. Confirm by reading `spec/path.rs:69`. If the first element is not that path, build it with `spec::path::task_path`.
- [ ] **`spec/keys.rs`.**
  - Delete `("io.kestra.plugin.core.flow.Pause", "onResume") => Some(Package(9)),` (`:171`).
  - Change `fn input(&mut self, id, input)` to `fn input(&mut self, list: &str, id: &str, input: &serde_json::Map<String, Value>)`. Its two paths become `format!("{list}.{id}.dependsOn")` and `format!("{list}.{id}.{key}")`. The flow walker (`:694-697`) calls `self.input("inputs", id, o)`.
  - After the `Loop` block (`:984-997`) add:
    ```rust
    // Package 7c D4: `Pause.onResume` is a list of the flow's `Input` bean,
    // checked key by key as `inputs:` is.
    if row == "io.kestra.plugin.core.flow.Pause" {
        if let Some(Value::Array(items)) = t.get("onResume") {
            let list = format!("{path}.onResume");
            for item in items {
                let Some(o) = item.as_object() else { continue };
                let Some(iid) = o.get("id").and_then(Value::as_str) else {
                    continue;
                };
                self.seen.inputs += 1;
                self.input(&list, iid, o);
            }
        }
    }
    ```
    If `self.seen.inputs` feeds a count that a test pins, leave it out and say so in the report.
- [ ] **Re-measure the property treatment:**
  ```bash
  EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today
  cargo test -p egret-workflow --lib spec::schema::measure
  git diff --stat crates/egret-workflow/src/spec/schema/property-treatment.json
  ```
  Expect exactly one changed line, `io.kestra.plugin.core.flow.Pause.onResume: "ignored"` → `"honoured"`. Any other line that moves: stop and report.

  Update `spec/schema/README.md`:
  - `:214` gains a dated lead sentence with the counts measured by the T0 Python one-liner, re-run now ("…, 2026-11-15, after package 7c modelled `Pause.onResume` and moved it from `ignored` to `honoured`; before that, …");
  - `:230`, the `Pause` row: `honoured` 10 → 11, and `onResume` leaves the `ignored` cell.
- [ ] **Tests** (`spec/validate.rs` test module; `report(src)` is the module's existing helper over `validate_source`, else use `crate::spec::validate_source(src, &crate::spec::parse(src).unwrap())`):
  ```rust
  const ON_RESUME_ALL_TYPES: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: s\n        type: STRING\n      - id: n\n        type: INT\n        min: 1\n      - id: x\n        type: FLOAT\n      - id: b\n        type: BOOL\n        defaults: true\n      - id: d\n        type: DATE\n      - id: dt\n        type: DATETIME\n      - id: t\n        type: TIME\n      - id: du\n        type: DURATION\n      - id: f\n        type: FILE\n        required: false\n      - id: sec\n        type: SECRET\n        required: false\n      - id: sel\n        type: SELECT\n        values: [a, b]\n      - id: ms\n        type: MULTISELECT\n        values: [a, b]\n      - id: arr\n        type: ARRAY\n        itemType: INT\n      - id: j\n        type: JSON\n      - id: u\n        type: URI\n      - id: e\n        type: EMAIL\n      - id: y\n        type: YAML\n        required: false\n  - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{ outputs.p.onResume.s }}\"\n";

  #[test]
  fn on_resume_with_every_input_type_publishes_without_a_finding() {
      let flow = crate::spec::parse(ON_RESUME_ALL_TYPES).expect("parses");
      let r = crate::spec::validate_source(ON_RESUME_ALL_TYPES, &flow);
      assert!(r.errors.is_empty(), "{r:?}");
      assert!(r.warnings.iter().all(|w| !w.path.contains("onResume")), "{r:?}");
      assert!(crate::executor::build_plan(ON_RESUME_ALL_TYPES).is_ok());
  }

  #[test]
  fn a_bad_on_resume_input_is_refused_at_its_path() {
      let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: a\n        type: ARRAY\n        itemType: SECRET\n      - id: f\n        type: FILE\n        allowedFileExtensions: [csv]\n      - id: dup\n        type: STRING\n      - id: dup\n        type: STRING\n";
      let flow = crate::spec::parse(src).expect("parses");
      let r = crate::spec::validate_source(src, &flow);
      let at = |code: &str| r.errors.iter().find(|e| e.code == code).map(|e| e.path.clone());
      assert_eq!(at("input_item_type_invalid").as_deref(), Some("tasks.p.onResume.a.itemType"));
      assert_eq!(at("input_file_extension_invalid").as_deref(), Some("tasks.p.onResume.f.allowedFileExtensions"));
      assert_eq!(at("duplicate_input_id").as_deref(), Some("tasks.p.onResume.dup"));
  }

  #[test]
  fn a_bad_on_resume_default_or_dependency_is_refused_at_save_at_its_path() {
      let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: n\n        type: INT\n        min: abc\n      - id: m\n        type: STRING\n        dependsOn:\n          inputs: [nope]\n";
      let flow = crate::spec::parse(src).expect("parses");
      let r = crate::spec::validate_source(src, &flow);
      let paths: Vec<_> = r.errors.iter().map(|e| (e.code, e.path.as_str())).collect();
      assert!(paths.contains(&("input_bound_invalid", "tasks.p.onResume.n.min")), "{paths:?}");
      assert!(paths.contains(&("input_depends_on_unknown", "tasks.p.onResume.m.dependsOn.inputs")), "{paths:?}");
      // A save-only finding: the run gate does not raise it.
      assert!(crate::spec::validate_for_run(&flow).errors.iter().all(|e| e.code != "input_depends_on_unknown"));
  }

  #[test]
  fn on_resume_ids_may_repeat_a_flow_input_id_and_the_other_pause_gaps_stay() {
      let src = "id: f\nnamespace: a.b\ninputs:\n  - id: ok\n    type: STRING\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n";
      let flow = crate::spec::parse(src).expect("parses");
      assert!(crate::spec::validate_source(src, &flow).errors.is_empty());
      let gap = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n    behavior: RESUME\n";
      let flow = crate::spec::parse(gap).expect("parses");
      assert!(crate::spec::validate_source(gap, &flow)
          .errors
          .iter()
          .any(|e| e.code == "unsupported_pause_property" && e.path == "tasks.p.behavior"));
  }
  ```
  In `spec/keys.rs`:
  ```rust
  #[test]
  fn pause_on_resume_is_honoured_and_pause_tasks_is_refused_once() {
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n");
      assert!(r.errors.is_empty(), "{r:?}");
      assert!(r.warnings.iter().all(|w| !w.path.starts_with("tasks.p.onResume")), "{r:?}");
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
      assert_eq!(r.errors.iter().filter(|e| e.path == "tasks.p.tasks").count(), 1, "{r:?}");
  }

  #[test]
  fn an_unknown_key_in_an_on_resume_input_is_refused_at_its_path() {
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n        colour: red\n");
      assert!(
          r.errors.iter().any(|e| e.code == "unknown_property" && e.path == "tasks.p.onResume.ok.colour"),
          "{r:?}"
      );
  }
  ```
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --lib spec::`
  - `cargo test -p egret-workflow --lib executor::`
  - `cargo test -p egret-workflow --test flow_schema --test publish`
- [ ] **Ratchet and census:** `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t2-executor.txt"`. Read the tuple and the `LIVE` census line from the file.
  - Expected unchanged: 301/25/25/0 and 295/25/21. The three `onResume` corpus flows published before, and onResume validation refuses none of them.
  - If either moves, the new numbers are the **measured** ones. Update `oracle-exec/driver.rs:854` (`LIVE`) and its doc block, or `tests/executor.rs:889` and the sentence above it, in this same commit, with a dated line naming the corpus entry that moved and why. Report it as a finding.
- [ ] Commit: `feat(workflow): model Pause onResume and check it at publish (package 7c)`.

---

## T3. Worker: pause by hand under the lock, and what still moves (spec D2, D3, D10; S1, S3, S4, S5)

**Files:**
- Create: `crates/egret-workflow/src/worker/pause.rs`, `crates/egret-workflow/tests/pause_resume.rs`.
- Modify: `src/worker/mod.rs` (`mod pause;` after `mod kill;`, re-exports), `src/worker/claim.rs:5234-5310` (`drive`), `src/worker/claim.rs:790-840` (`due_retries`).

**Interfaces.**

Consumes: `claim::{load, settle, Stack}`, `txn::transaction`, `executor::{pause_by_hand_commands, NotRunning, paused_by_hand, apply}`.

Produces:
```rust
// worker/pause.rs, re-exported from `worker`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paused {
    Done,
    /// D3: not `RUNNING` or `RESTARTED`; the state's wire name.
    NotRunning(String),
}
/// D3. In the execution's lock; scoped by `workspace` (`NotFound` otherwise).
pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused>;
```

**Steps.**

- [ ] **Write the failing tests** in `tests/pause_resume.rs` (list below). Run `cargo test -p egret-workflow --test pause_resume`: it does not compile.
- [ ] **Implement `worker/pause.rs`:**
  ```rust
  //! Pause by hand and resume with `onResume` inputs -- workflow package 7c
  //! (spec `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D3, D5,
  //! D6). Both run inside the execution's lock (`claim::load`) and end in
  //! `claim::settle`, as every writer does.

  use egret_core::{Error, Result};
  use sqlx::PgPool;
  use uuid::Uuid;

  use super::claim::{self, Stack};
  use super::txn::transaction;
  use crate::executor::apply;

  /// What [`pause_by_hand`] did.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Paused {
      /// The execution is `PAUSED`; no task run changed.
      Done,
      /// Nothing written: the execution is not `RUNNING` or `RESTARTED`; its
      /// state's wire name (D3).
      NotRunning(String),
  }

  /// **Pause by hand** (D3; Kestra `ES.pause`, `ES:907-916`): one
  /// `SetExecutionState { PAUSED }` under the execution's lock, then the
  /// settle, whose `advance` answers nothing (D2). Runs already `RUNNING`
  /// are left to finish. The state is re-checked under the lock, so a pause
  /// that loses the race to a kill or to the execution's end writes nothing.
  pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused> {
      transaction!(pool, |at| {
          let (conn, gate) = at.parts();
          let (ctx, mut state, now) = claim::load(conn, id, Stack::Fresh(gate.clone())).await?;
          if ctx.workspace_id != workspace {
              return Err(Error::NotFound("execution".into()));
          }
          let cmds = match crate::executor::pause_by_hand_commands(&state) {
              Ok(cmds) => cmds,
              Err(crate::executor::NotRunning(s)) => {
                  return Ok(Paused::NotRunning(
                      s.map_or("UNKNOWN", |s| s.wire()).to_string(),
                  ));
              }
          };
          let before = state.clone();
          for c in &cmds {
              apply(&mut state, c);
          }
          claim::settle(conn, &ctx, &before, state, now, None).await?;
          Ok(Paused::Done)
      })
  }
  ```
  In `worker/mod.rs` add `mod pause;` and `pub use pause::{pause_by_hand, Paused};`.

  `claim::load` and `claim::settle` are `pub(super)`. `ExecutionCtx::workspace_id` is `pub`. If `load` returns a different tuple shape, follow `claim::resume_by_hand` (`claim.rs:4535-4552`), which is the model.
- [ ] **S4, `claim::drive`** (`claim.rs:5234-5310`). After the `if !cmds.is_empty() { … continue; }` block and before the first render pass, insert:
  ```rust
  // Package 7c D2 (S4): a manual pause stops the render passes too. A
  // `LoopUntil` whose body ended during the pause is asked its condition
  // at resume, not now, and no `runIf:` or expression render can fail a
  // run while nothing drives the execution (Kestra: `canBeProcessed()`).
  if crate::executor::paused_by_hand(state) {
      return Ok((spawns, subflows, exit_killed));
  }
  ```
- [ ] **S5, `due_retries`** (`claim.rs:790-840`). In the `WHERE`, after `AND e.state <> $3`, add:
  ```sql
  AND NOT (e.state = 'PAUSED' \
           AND NOT EXISTS (SELECT 1 FROM workflow.task_runs p \
                            WHERE p.execution_id = e.id AND p.state = 'PAUSED')) \
  ```
  This keeps the spacing of the surrounding string literal. Add a doc paragraph above the function: "**Package 7c (S5):** an execution paused by hand (`executor::paused_by_hand`: `PAUSED` with no `PAUSED` run) is not woken. `advance` answers nothing for it until resume, so a due `RETRYING` run in it would wake it every tick for nothing. `a_manual_pause_with_a_due_retry_is_not_woken` is the test."
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test pause_resume`
  - `cargo test -p egret-workflow --test worker --test restart --test kill_route --test end_hooks`
  - the ratchet `--test executor` (unchanged)
- [ ] Commit: `feat(workflow): pause an execution by hand; nothing moves while it waits (package 7c)`.

**Tests** (`tests/pause_resume.rs`). Module doc: the package, spec path and "Run with: export DATABASE_URL=… ; cargo test -p egret-workflow --test pause_resume".

Fixtures:
- copy `one_task`, `two_tasks`, `file_flow`, `settle_everything` and `subflow_parent` verbatim from `tests/worker.rs:69-81`, `:4330-4344`, `:1910-1920`, `:4408-4415`;
- the registry is `egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current())` (`tests/worker.rs:852`);
- add the helpers below.

```rust
async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    (db, ws, user)
}

async fn start(pool: &sqlx::PgPool, db: &Db, ws: Uuid, user: egret_core::UserId, src: &str) -> Uuid {
    let flow = file_flow(db, ws, user, src).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), src, serde_json::json!({}))
        .await
        .expect("start")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id).fetch_one(pool).await.expect("state")
}

/// `task_id -> state` of every run, in run order.
async fn runs(pool: &sqlx::PgPool, id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as("SELECT task_id, state FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix")
        .bind(id).fetch_all(pool).await.expect("runs")
}

fn handlers() -> egret_workflow::worker::Handlers {
    egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current())
}
```

Each test is `#[sqlx::test(migrations = "../../migrations")]`. Arrange / Act / Assert:

1. `a_running_execution_pauses_and_no_run_changes`:
   - Arrange: `two_tasks("p")` started, not ticked; `a` is `RUNNING`.
   - Act: `pause_by_hand(&pool, ws, ex)` gives `Paused::Done`.
   - Assert: the execution is `PAUSED`; its history ends `["RUNNING", "PAUSED"]`; `runs` is still `[("a","RUNNING")]`.
2. `a_run_completing_during_the_pause_is_recorded_and_nothing_new_is_created`:
   - Arrange: as 1; pause.
   - Act: `settle_everything(&pool, &handlers())`.
   - Assert: `runs` is `[("a","SUCCESS")]` (no `b`); the execution is `PAUSED`; `a`'s `outputs`/`ended_at` are written (`ended_at IS NOT NULL`).
3. `a_run_ending_inside_a_flowable_does_not_complete_its_parent`:
   - Arrange: a `Sequential` `seq` holding `a`, `b`, then a top-level `c`; start; pause.
   - Act: settle.
   - Assert: `seq` is `RUNNING`, `a` is `SUCCESS`, no `b` and no `c` run.
4. `pause_against_kill_and_against_the_end`:
   - (a) kill first (`egret_workflow::worker::kill_execution(&pool, ws, ex, true)`), then pause: `NotRunning("KILLING")` or `NotRunning("KILLED")`, and the state is unchanged.
   - (b) pause first, then kill: the execution goes `KILLING`; after `settle_everything` it is `KILLED`, and no `b` run exists.
   - (c) a finished execution (`one_task`, settled to `SUCCESS`): `NotRunning("SUCCESS")`.
   - (d) concurrent: `tokio::join!(pause_by_hand(..), kill_execution(..))` on a fresh `two_tasks`. Both return `Ok`. The final state after `settle_everything` is `KILLED`, and `a` is never followed by `b`.
5. `a_foreign_workspace_cannot_pause`: `pause_by_hand(&pool, other_ws, ex)` gives `Err(NotFound)`; the state is unchanged.
6. `s1_a_loop_iteration_ending_during_the_pause_starts_the_next_and_the_flow_waits`:
   - Arrange: the flow
     ```yaml
     id: loopy
     namespace: t
     tasks:
       - id: each
         type: io.kestra.plugin.core.flow.Loop
         values: ["1", "2"]
         concurrencyLimit: 1
         tasks:
           - id: a
             type: io.kestra.plugin.core.log.Log
             message: "{{ taskrun.value }}"
       - id: after
         type: io.kestra.plugin.core.log.Log
         message: done
     ```
     Start it and pause the root.
   - Act: `settle_everything`.
   - Assert (S1, Kestra):
     - two iterations exist (`SELECT count(*) FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index IS NOT NULL` = 2), both `SUCCESS`;
     - the `each` run is `SUCCESS`;
     - no `after` run; the root is `PAUSED`.
   - Then `resume_with` (T4) or, in T3, the by-hand resume through `egret_workflow::worker::resume_by_hand(&pool, ex)`, which T4 teaches the by-hand case. **In T3 this half is `#[ignore = "T4"]`; T4 removes the ignore.** After it and `settle_everything`, the root is `SUCCESS` and `after` ran.
   - If the user rejects S1, this test flips: one iteration, and the second starts only after resume.
7. `a_subflow_child_ending_during_the_pause_completes_its_run_and_nothing_after`:
   - Arrange: `file_flow(one_task("child"))`; parent `subflow_parent("parent", "child", "")`; start the parent and pause it before any tick.
   - Act: `settle_everything`.
   - Assert: the child is `SUCCESS`; the parent's `sub` run is `SUCCESS`; no `after` run; the parent is `PAUSED`.
8. `s4_a_loop_until_condition_is_not_rendered_during_the_pause`:
   - Arrange: a `LoopUntil` `lu` with `condition: "{{ false }}"`, `checkFrequency: {interval: PT1H, maxIterations: 2}`, body `a` (Log). Start; pause.
   - Act: `settle_everything`.
   - Assert: `a` is `SUCCESS`; `SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'lu'` is `NULL`; the execution is `PAUSED`.
   - Red before the `drive` gate: `rendered` is `"false"`.
9. `s3_a_manual_pause_beside_a_waiting_pause_is_not_by_hand_and_its_timer_ends_it`:
   - Arrange: a `Parallel` of `quick` (`Pause`, `pauseDuration: PT0S`) and `slow` (`Pause`, `pauseDuration: PT1H`), then `after` (Log). Start, `settle_everything` and `egret_workflow::worker::wake(&pool, 16)` until `quick` is `SUCCESS` and the execution is `RUNNING`.
   - Act: pause (accepted, `Done`).
   - Assert: the execution is `PAUSED`, and `executor::paused_by_hand_of(...)` over the rows is false (`slow` is `PAUSED`).
   - Then `UPDATE workflow.task_runs SET not_before_ms = 0 WHERE task_id = 'slow'`, `wake`, `settle_everything`: the execution leaves `PAUSED` (`RESTARTED` then `RUNNING`, then `SUCCESS` after `after`). This pins S3.
10. `s5_a_manual_pause_with_a_due_retry_is_not_woken`:
    - Arrange: `one_task_with_retry("r", "PT0S")` (copy from `tests/worker.rs:2768-2774`) beside a second top-level task `b` in a `Parallel`, so that the execution stays `RUNNING` while `a` is `RETRYING`. Or simpler: start `two_tasks`, pause, then write a due `RETRYING` run by hand:
      ```sql
      UPDATE workflow.task_runs
         SET state = 'RETRYING', history = history || '["RETRYING"]', not_before_ms = 0
       WHERE execution_id = $1 AND task_id = 'a'
      ```
    - Act/Assert: `egret_workflow::worker::due_retries(&pool, 10)` does not contain `ex`. Control: `UPDATE workflow.executions SET state = 'RUNNING'` → it does.
11. `kill_on_a_manual_pause_with_a_finished_child_ends_killed`:
    - Arrange: test 3's flow; pause; settle (`a` `SUCCESS`, `seq` `RUNNING`).
    - Act: `kill_execution(&pool, ws, ex, true)`.
    - Assert: `KILLING` then, after settle, `KILLED`; `seq` is `KILLED`; no `b`.
12. `restart_of_a_manual_pause_follows_7a`:
    - (a) `two_tasks`; pause; settle (`a` `SUCCESS`). `egret_workflow::worker::restart_execution(&pool, ex)` gives `Restarted::Refused(RestartRefused::NothingToRestart)`, and the state stays `PAUSED`.
    - (b) a `two_tasks` whose `a` fails (a registry that fails `a`; copy `Flaky` from `tests/restart_routes.rs:33-67`); pause before the tick; tick (`a` `FAILED`, the execution still `PAUSED`); restart gives `Done`, the execution goes `RESTARTED`, and after a settle with `a` succeeding, `b` runs and the execution is `SUCCESS`.
13. `database_url_is_exported_rather_than_read_from_dotenv` (the Global Constraints test).

---

## T4. Worker: resume, with `onResume` inputs (spec D5, D6; S2, S6, S7, S9)

**Files:**
- Modify:
  - `src/worker/pause.rs`;
  - `src/worker/claim.rs` (`Resumed` `:4499-4513`, `resume_by_hand` `:4535-4552`, `checked_inputs`/`input_outcomes`/`start_renderer` `:4243-4295`);
  - `src/worker/mod.rs` (re-exports, `:118-122`);
  - `src/worker/render.rs:2149-2167`;
  - `src/secret_inputs.rs:468-481`;
  - `src/storage/uri.rs` (after `task_prefix`, `:393-420`);
  - `src/storage/mod.rs` (after `execution_prefix`, `:255-270`);
  - `src/storage/inputs.rs` (`:97-167`, `:240-…`);
  - `tests/pause_resume.rs`.

**Interfaces.**

Consumes: T1 (`paused_by_hand`, `resume_by_hand_commands`, `first_waiting_pause`, `resumed_outputs`, `release_paused_pauses`); `spec::path::find_task`; `inputs_check::resolve`; `secret_inputs::{split_outputs, can_seal}`; `storage::index_task_files`.

Produces:
```rust
// worker/pause.rs, re-exported from `worker`
pub struct ResumeRequest<'a> {
    pub id: Uuid,
    /// The posted values; `None` = no body or `"inputs": null`.
    pub inputs: Option<&'a serde_json::Map<String, serde_json::Value>>,
    /// The files the route stored for this resume (S2), all under `expected`.
    pub files: &'a [crate::storage::StoredFile],
    /// The `Pause` run the route read as waiting and stored files under.
    pub expected: Option<Uuid>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeRefused {
    /// D5: inputs where no `onResume` applies (paused by hand, or a `Pause`
    /// with none).
    InputsUnexpected,
    /// A value refused, a key not declared (S6), a FILE not readable, or a
    /// SECRET with no master key (S7): the message names the input.
    Invalid(String),
    /// The waiting `Pause` run is not the one the route read (a lost race).
    Moved,
}
pub async fn resume_with(pool: &PgPool, req: &ResumeRequest<'_>) -> Result<Resumed>;

// worker/claim.rs
pub enum Resumed {
    Released(usize),
    /// D5: the execution was paused by hand and is `RUNNING` again.
    ByHand,
    NoPauseWaiting,
    NotPaused,
    /// Nothing written; why.
    Refused(ResumeRefused),
}
/// Unchanged signature: a resume with no body (tests, W7b callers).
pub async fn resume_by_hand(pool: &PgPool, execution_id: Uuid) -> Result<Resumed>;
pub(crate) fn checked_inputs_for(flow: &Flow, declared: &[Input], posted: &Value)
    -> std::result::Result<Value, crate::inputs_check::InputError>;
pub(crate) fn input_outcomes_for(flow: &Flow, declared: &[Input], posted: &Value)
    -> Vec<crate::inputs_check::InputOutcome>;

// storage/uri.rs
pub fn resume_input_uri(namespace: &str, flow_id: &str, execution: Uuid, task_id: &str,
    task_run: Uuid, request: Uuid, input: &str, file: &str) -> Result<String, String>;
// storage/mod.rs, on FileStore
pub fn resume_root(&self, workspace: Uuid, namespace: &str, flow_id: &str, execution: Uuid,
    task_id: &str, task_run: Uuid) -> String;
pub fn resume_prefix(&self, workspace: Uuid, namespace: &str, flow_id: &str, execution: Uuid,
    task_id: &str, task_run: Uuid, request: Uuid) -> String;
// storage/inputs.rs
#[allow(clippy::too_many_arguments)]
pub async fn upload_resume_input(fs: &FileStore, workspace: Uuid, namespace: &str, flow_id: &str,
    execution: Uuid, task_id: &str, task_run: Uuid, request: Uuid, input: &Input,
    file_name: &str, body: &mut (dyn tokio::io::AsyncRead + Send + Unpin))
    -> std::result::Result<StoredFile, UploadError>;
pub async fn check_file_values(conn: &mut PgConnection, workspace: Uuid, execution: Uuid,
    declared: &[Input], values: &serde_json::Value, uploaded: &[StoredFile])
    -> Result<std::result::Result<(), String>>;
// secret_inputs.rs
pub(crate) fn reveal_keyed_outputs(outputs: Value, key: &str, sealed: Option<&[u8]>)
    -> Result<(Value, Vec<String>)>;
```

**Steps.**

- [ ] **Failing tests** (list below) in `tests/pause_resume.rs`. Remove T3 test 6's `#[ignore]`. Run `cargo test -p egret-workflow --test pause_resume`: red.
- [ ] **Generalise the input check** (`claim.rs:4243-4295`):
  ```rust
  pub(crate) fn checked_inputs(flow: &crate::models::Flow, posted: &serde_json::Value)
      -> std::result::Result<serde_json::Value, crate::inputs_check::InputError> {
      checked_inputs_for(flow, &flow.inputs, posted)
  }
  /// [`checked_inputs`] against another declared list -- a `Pause`'s
  /// `onResume` (package 7c D5), rendered as a start's are.
  pub(crate) fn checked_inputs_for(
      flow: &crate::models::Flow,
      declared: &[crate::models::Input],
      posted: &serde_json::Value,
  ) -> std::result::Result<serde_json::Value, crate::inputs_check::InputError> {
      crate::inputs_check::resolve(declared, posted, &input_renderer(flow, declared))
          .map(serde_json::Value::Object)
  }
  pub(crate) fn input_outcomes(flow: &crate::models::Flow, posted: &serde_json::Value)
      -> Vec<crate::inputs_check::InputOutcome> {
      input_outcomes_for(flow, &flow.inputs, posted)
  }
  pub(crate) fn input_outcomes_for(
      flow: &crate::models::Flow,
      declared: &[crate::models::Input],
      posted: &serde_json::Value,
  ) -> Vec<crate::inputs_check::InputOutcome> {
      crate::inputs_check::resolve_all(declared, posted, &input_renderer(flow, declared))
  }
  ```
  Rename `start_renderer(flow)` to `input_renderer(flow, declared: &[crate::models::Input])`. Its `defaulted` map iterates `declared` instead of `flow.inputs`. Add `checked_inputs_for, input_outcomes_for` to the `pub(crate) use claim::{…}` at `worker/mod.rs:118`.
- [ ] **Storage.**
  - `uri.rs`, after `task_prefix`:
    ```rust
    /// A FILE `onResume` input's URI (package 7c S2): under the `Pause` run's
    /// own task prefix, `tasks/{task-slug}/{task_run}/resume/{request}/{input}/{file}`,
    /// so it parses as [`Tail::Task`], is indexed under that run, and can
    /// never overwrite a start input (`inputs/{input}/{file}`).
    #[allow(clippy::too_many_arguments)]
    pub fn resume_input_uri(
        namespace: &str,
        flow_id: &str,
        execution: Uuid,
        task_id: &str,
        task_run: Uuid,
        request: Uuid,
        input: &str,
        file: &str,
    ) -> Result<String, String> {
        let prefix = task_prefix(namespace, flow_id, execution, task_id, task_run)?;
        let tail = format!("resume/{}/{input}/{file}", request.hyphenated());
        let uri = format!("{prefix}{tail}");
        checked(
            &uri,
            execution,
            &Tail::Task {
                task_slug: slugify_str(task_id),
                task_run: task_run.hyphenated().to_string(),
                file: tail,
            },
            &format!("namespace {namespace:?}, flow {flow_id:?}, task {task_id:?}, input {input:?} and file {file:?}"),
        )?;
        Ok(uri)
    }
    ```
    `checked` is the private helper `input_uri` uses (`uri.rs:438-446`). Read its signature before writing this.
  - `storage/mod.rs`, on `FileStore`, after `execution_prefix`:
    ```rust
    /// Every resume's files of `task_run` (package 7c S2), ending with `/`.
    pub fn resume_root(&self, workspace: Uuid, namespace: &str, flow_id: &str,
                       execution: Uuid, task_id: &str, task_run: Uuid) -> String {
        format!(
            "{}tasks/{}/{}/resume/",
            self.execution_prefix(workspace, namespace, flow_id, execution),
            uri::slugify_str(task_id),
            task_run.hyphenated()
        )
    }
    /// One resume request's files: what a refused resume deletes.
    #[allow(clippy::too_many_arguments)]
    pub fn resume_prefix(&self, workspace: Uuid, namespace: &str, flow_id: &str,
                         execution: Uuid, task_id: &str, task_run: Uuid, request: Uuid) -> String {
        format!(
            "{}{}/",
            self.resume_root(workspace, namespace, flow_id, execution, task_id, task_run),
            request.hyphenated()
        )
    }
    ```
  - `storage/inputs.rs`. Split `upload_input` into:
    - `fn admit(input: &Input, file_name: &str) -> std::result::Result<(), UploadError>`: the type, empty-name and extension checks, `:105-118`;
    - `async fn store(fs, workspace, uri: String, input: &Input, file_name: &str, body) -> std::result::Result<StoredFile, UploadError>`: everything from `let object = fs.object_uri(…)` to the end.

    `upload_input` becomes `admit` + `uri::input_uri` + `store`. Add `upload_resume_input`, which is `admit` + `uri::resume_input_uri(…)` (mapping `Err(e)` to `UploadError::Author(format!("Invalid value for input `{}`: {e}", input.id))`) + `store`.

    Rename the body of `check_file_inputs` to `check_file_values(conn, workspace, execution, declared, values, uploaded)`, which reads `declared` where it read `flow.inputs` (its only use of `flow`). `check_file_inputs` becomes a one-line wrapper passing `&flow.inputs`.
- [ ] **Secrets.**
  - `secret_inputs.rs`: replace the body of `reveal_subflow_outputs` with a call to a new `reveal_keyed_outputs(outputs, "outputs", sealed)`. That function is the old body with `"outputs"` replaced by `key`.
  - `render.rs:2154-2165`, the `match` in `reveal_run_outputs`:
    ```rust
    (Some(o), Some(sealed)) => {
        let (o, values) = if matches!(o.get("outputs"), Some(Value::Object(_))) {
            crate::secret_inputs::reveal_subflow_outputs(o, Some(sealed))?
        } else if matches!(o.get("onResume"), Some(Value::Object(_))) {
            // Package 7c (S7): a `Pause` run's SECRET `onResume` values.
            crate::secret_inputs::reveal_keyed_outputs(o, "onResume", Some(sealed))?
        } else {
            crate::secret_inputs::reveal_loop_outputs(o, Some(sealed))?
        };
        secrets.extend(values);
        Ok(Some(o))
    }
    ```
- [ ] **`worker/pause.rs`: `resume_with`.**
  ```rust
  // `use super::claim::Resumed;` and `use crate::executor::RunState;` join the imports.

  /// **Resume** (D5): in the execution's lock. Paused by hand: back to
  /// `RUNNING` (no inputs allowed). Otherwise the first `PAUSED` `Pause`
  /// run's `onResume` is checked and written into its outputs, every
  /// waiting `Pause` is released as today, and the execution settles on
  /// (`RESTARTED`, `pause_mark`). Every refusal comes before the first
  /// write: `transaction!` commits an `Ok`.
  pub async fn resume_with(pool: &PgPool, req: &ResumeRequest<'_>) -> Result<Resumed> {
      transaction!(pool, |at| {
          let (conn, gate) = at.parts();
          let (ctx, mut state, now) =
              claim::load(conn, req.id, Stack::Fresh(gate.clone())).await?;
          if state.history.last() != Some(&RunState::Paused) {
              return Ok(Resumed::NotPaused);
          }
          let posted = req.inputs.cloned().unwrap_or_default();
          let before = state.clone();
          if crate::executor::paused_by_hand(&state) {
              if !posted.is_empty() {
                  return Ok(Resumed::Refused(ResumeRefused::InputsUnexpected));
              }
              for c in crate::executor::resume_by_hand_commands(&state) {
                  apply(&mut state, &c);
              }
              claim::settle(conn, &ctx, &before, state, now, None).await?;
              return Ok(Resumed::ByHand);
          }
          let Some(chosen) = crate::executor::first_waiting_pause(&state) else {
              return Ok(Resumed::NoPauseWaiting);
          };
          let chosen_ix = i32::try_from(chosen).expect("a run index fits the column");
          let chosen_id: Uuid = sqlx::query_scalar(
              "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
          )
          .bind(req.id)
          .bind(chosen_ix)
          .fetch_one(&mut *conn)
          .await
          .map_err(db_err)?;
          if req.expected.is_some_and(|e| e != chosen_id) {
              return Ok(Resumed::Refused(ResumeRefused::Moved));
          }
          let flow = crate::spec::parse(&ctx.source).map_err(|e| {
              Error::Internal(anyhow::anyhow!(
                  "execution {}: its frozen source does not parse: {}",
                  req.id,
                  e.message
              ))
          })?;
          let task_id = ctx.plan.tasks[state.runs[chosen].task].id.clone();
          let declared: Vec<crate::models::Input> = crate::spec::path::find_task(&flow, &task_id)
              .and_then(|t| match &t.kind {
                  crate::models::TaskKind::Pause(p) => p.on_resume.clone(),
                  _ => None,
              })
              .unwrap_or_default();
          let (values, sealed) = match checked_on_resume(conn, &ctx, &flow, &task_id, &declared, &posted, req.files).await? {
              Ok(v) => v,
              Err(r) => return Ok(Resumed::Refused(r)),
          };

          let released = crate::executor::release_paused_pauses(&mut state, now);
          let on = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
          for &ix in &released {
              let (outputs, secret) = if ix == chosen {
                  (crate::executor::resumed_outputs(values.clone(), &on), sealed.clone())
              } else {
                  (crate::executor::resumed_outputs(serde_json::Map::new(), &on), None)
              };
              sqlx::query(
                  "UPDATE workflow.task_runs SET outputs = $3, secret_outputs = $4 \
                    WHERE execution_id = $1 AND run_ix = $2 AND state = 'PAUSED'",
              )
              .bind(req.id)
              .bind(i32::try_from(ix).expect("a run index fits the column"))
              .bind(&outputs)
              .bind(secret.as_deref())
              .execute(&mut *conn)
              .await
              .map_err(db_err)?;
          }
          crate::storage::index_task_files(conn, ctx.workspace_id, chosen_id, req.files).await?;
          claim::settle(conn, &ctx, &before, state, now, None).await?;
          Ok(Resumed::Released(released.len()))
      })
  }

  /// D5, D6, S6, S7: the posted values against `declared`. `Ok(Err(_))` is
  /// the caller's refusal; `Err` is the database's. Answers the masked
  /// values and, when a SECRET was given, their seal.
  #[allow(clippy::too_many_arguments)]
  async fn checked_on_resume(
      conn: &mut sqlx::PgConnection,
      ctx: &claim::ExecutionCtx,
      flow: &crate::models::Flow,
      task_id: &str,
      declared: &[crate::models::Input],
      posted: &serde_json::Map<String, serde_json::Value>,
      files: &[crate::storage::StoredFile],
  ) -> Result<std::result::Result<(serde_json::Map<String, serde_json::Value>, Option<Vec<u8>>), ResumeRefused>> {
      if declared.is_empty() {
          return Ok(if posted.is_empty() {
              Ok((serde_json::Map::new(), None))
          } else {
              Err(ResumeRefused::InputsUnexpected)
          });
      }
      // S6: refused, not dropped (the no-silent-drops rule).
      if let Some(k) = posted.keys().find(|k| !declared.iter().any(|i| &i.id == *k)) {
          return Ok(Err(ResumeRefused::Invalid(format!(
              "Invalid value for input `{k}`: the Pause `{task_id}` declares no such onResume input"
          ))));
      }
      let posted = serde_json::Value::Object(posted.clone());
      let values = match claim::checked_inputs_for(flow, declared, &posted) {
          Ok(serde_json::Value::Object(m)) => m,
          Ok(_) => unreachable!("checked_inputs_for answers an object"),
          Err(e) => return Ok(Err(ResumeRefused::Invalid(e.message))),
      };
      if let Err(why) = crate::storage::inputs::check_file_values(
          conn,
          ctx.workspace_id,
          ctx.id,
          declared,
          &serde_json::Value::Object(values.clone()),
          files,
      )
      .await?
      {
          return Ok(Err(ResumeRefused::Invalid(why)));
      }
      let secret_ids: Vec<String> = declared
          .iter()
          .filter(|i| i.r#type == crate::models::InputType::Secret)
          .filter(|i| values.get(&i.id).is_some_and(|v| !v.is_null()))
          .map(|i| i.id.clone())
          .collect();
      if secret_ids.is_empty() {
          return Ok(Ok((values, None)));
      }
      if !crate::secret_inputs::can_seal() {
          return Ok(Err(ResumeRefused::Invalid(format!(
              "egret refuses input {}: a SECRET value cannot be stored on this deployment: set secrets.master_key",
              secret_ids.join(", ")
          ))));
      }
      let (masked, sealed) = crate::secret_inputs::split_outputs(&values, &secret_ids)?;
      Ok(Ok((masked, sealed)))
  }

  fn db_err(e: sqlx::Error) -> Error {
      Error::Internal(anyhow::Error::new(e))
  }
  ```
  - Make `ExecutionCtx` reachable as `claim::ExecutionCtx` (it is `pub(super) struct`, so it already is).
  - Add `use crate::executor::RunState;` and `use super::claim::Resumed;` to `pause.rs`'s imports.
  - Move nothing else out of `claim.rs`.
- [ ] **`claim.rs`.**
  - `Resumed` gains `ByHand` and `Refused(super::pause::ResumeRefused)` with docs, and stops deriving `Copy` (it now carries a `String`).
  - `resume_by_hand(pool, id)` becomes:
    ```rust
    pub async fn resume_by_hand(pool: &PgPool, execution_id: Uuid) -> Result<Resumed> {
        super::pause::resume_with(pool, &super::pause::ResumeRequest {
            id: execution_id,
            inputs: None,
            files: &[],
            expected: None,
        })
        .await
    }
    ```
    Keep its doc and add one line: "Since package 7c it is [`super::pause::resume_with`] with no body: a manual pause goes back to `RUNNING`, a `Pause` run ends `SUCCESS` with `{onResume: {}, resumed}`."
  - Re-export `pub use pause::{pause_by_hand, resume_with, Paused, ResumeRefused, ResumeRequest};` in `worker/mod.rs`.
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test pause_resume`
  - `cargo test -p egret-workflow --test executions_routes --test file_inputs --test secret_inputs --test inputs_start --test worker --test storage_index`
  - `cargo test -p egret-workflow --lib storage:: secret_inputs:: worker::render`
  - the ratchet.

  Existing resume tests (`executions_routes.rs:2445-2760`) must stay green, except any that asserts a `Pause` run's `outputs` is `NULL` after a resume. Such a test now sees `{onResume: {}, resumed}` by design; update it and say so in the report.
- [ ] Commit: `feat(workflow): resume a Pause with onResume inputs written to its outputs (package 7c)`.

**Tests** (`tests/pause_resume.rs`, added). Run `egret_workflow::storage::install_mem_for_tests()` and `egret_workflow::secret_inputs::install_for_tests()` at the start of each test that needs them.

14. `resume_by_hand_runs_on_and_the_flow_finishes`:
    - Arrange: `two_tasks`; pause; settle (`a` `SUCCESS`).
    - Act: `resume_with` with no inputs gives `Resumed::ByHand`; then settle.
    - Assert: `b` ran; the execution is `SUCCESS`; its history holds `PAUSED` then `RUNNING`, with no `RESTARTED` between them.
15. `resume_by_hand_with_inputs_is_refused_and_writes_nothing`: inputs `{"x": 1}` give `Refused(InputsUnexpected)`; the state is `PAUSED`.
16. `resume_with_inputs_writes_on_resume_and_resumed_and_the_next_task_reads_them`. The end-to-end test of spec §4:
    - Arrange: the flow
      ```yaml
      id: approve
      namespace: t
      tasks:
        - id: p
          type: io.kestra.plugin.core.flow.Pause
          onResume:
            - id: who
              type: STRING
            - id: n
              type: INT
            - id: doc
              type: FILE
              required: false
        - id: log
          type: io.kestra.plugin.core.log.Log
          message: "who={{ outputs.p.onResume.who }} n={{ outputs.p.onResume.n }} by={{ outputs.p.resumed.to }}"
      ```
      Start; `settle_everything` (the execution is `PAUSED`).
    - Act: `resume_with(inputs {"who":"ada","n":"3"})` gives `Released(1)`; then settle.
    - Assert:
      - `p`'s `outputs` = `{"onResume":{"who":"ada","n":3},"resumed":{"by":null,"on":<string>,"to":"SUCCESS"}}`; the `on` value parses as RFC 3339;
      - `SELECT message FROM workflow.task_logs WHERE execution_id = $1` contains `who=ada n=3 by=SUCCESS`;
      - the execution is `SUCCESS`, and its history holds `RESTARTED`.
17. `a_refused_value_names_the_input_and_writes_nothing`:
    - `{"who":"ada","n":"x"}` gives `Refused(Invalid(m))`, with `m` containing `` `n` ``; the state is `PAUSED`; `p`'s outputs are `NULL`.
    - Missing required `who` gives `Invalid` containing `Missing required input:who`.
18. `s6_an_undeclared_key_is_refused`: `{"who":"ada","n":"3","zzz":1}` gives `Invalid` containing `` `zzz` ``.
19. `a_pause_without_on_resume_accepts_no_body_and_refuses_inputs`: an unbounded `Pause` and `after`. `{"a":1}` gives `InputsUnexpected`; `{}` gives `Released(1)`, and `p`'s outputs are `{"onResume":{},"resumed":{…}}`.
20. `parallel_pauses_the_first_gets_the_inputs_and_the_others_on_resume_empty`: a `Parallel` of `p1` (`onResume: [{id: a, type: STRING}]`) and `p2` (`onResume: [{id: b, type: STRING}]`). `{"a":"x"}` gives `Released(2)`; `p1.onResume == {"a":"x"}`; `p2.onResume == {}`.
21. `only_a_loop_until_waiting_is_no_pause_waiting`: the `LoopUntil` fixture of T1 test 1, settled to `PAUSED`. `resume_with` gives `NoPauseWaiting`.
22. `a_lost_race_is_moved`: `expected: Some(Uuid::new_v4())` gives `Refused(Moved)`; nothing is written.
23. `s2_a_file_stored_under_the_pause_run_is_indexed_and_readable`:
    - Arrange: test 16's flow; `let fs = install_mem_for_tests();`; read `p`'s run id.
    - Act: store `b"hello"` with `upload_resume_input(&fs, ws, "t", "approve", ex, "p", run, req_id, &doc_input, "a.txt", &mut &b"hello"[..])`. Then `resume_with` with `{"who":"ada","n":"1","doc": <uri>}` and `files: &[stored]`.
    - Assert:
      - `Released(1)`;
      - the URI matches `…/executions/{ex}/tasks/p/{run}/resume/{req_id}/doc/a.txt`;
      - `egret_workflow::storage::resolve_readable(&mut conn, ws, ex, &uri)` gives `Ok(Ok(_))` (readable by the execution's own later tasks and by the file API, which uses the same rule);
      - `workflow.execution_files` holds the URI with `task_run_id = run`.
24. `s7_a_secret_on_resume_value_is_sealed_and_a_later_task_reads_it_masked_in_logs`:
    - Arrange: `install_for_tests()`; `onResume: [{id: tok, type: SECRET}]`; `log` message `"tok={{ outputs.p.onResume.tok }}"`.
    - Act: resume with `{"tok":"s3cr3t"}`; settle.
    - Assert: `p.outputs.onResume.tok == "******"`; `secret_outputs IS NOT NULL`; the log line does not contain `s3cr3t` (it is masked); the execution is `SUCCESS`.
    - With no cipher: this needs a separate binary because the cipher is process-wide. **Assert the no-key refusal in `tests/secret_inputs_no_key.rs`** (an existing binary with no key): a resume with a SECRET gives `Invalid` containing `secrets.master_key`.
25. Un-ignore T3 test 6.

---

## T5. Routes, gates, audit, OpenAPI, `ExecutionDto.paused_by_hand` (spec D3, D5–D9, D11's DTO field)

**Files:**
- Modify:
  - `crates/egret-workflow/src/routes.rs` (sections named below);
  - `src/worker/sql_principal.rs` (after `check_start`, `:90-113`);
  - `src/repo.rs` (nothing expected);
  - `openapi.json` (regenerated).
- Create: `crates/egret-workflow/tests/pause_resume_routes.rs`.

**Interfaces.**

Consumes: T3/T4 worker API; `ExecOpError`; `read_json_body`, `start_inputs`, `startable_flow`; `crate::spec::path::find_task`.

Produces:
```rust
// routes.rs
pub const NOT_RUNNING_CODE: &str = "not_running";
pub const NOT_PAUSED_CODE: &str = "not_paused";
pub const RESUME_INPUTS_UNEXPECTED_CODE: &str = "resume_inputs_unexpected";

async fn pause_execution(State<WorkflowState>, WsContext, Path<(String, Uuid)>)
    -> Result<Json<ExecutionDto>, ExecOpError>;
async fn resume_execution(State<WorkflowState>, WsContext, Path<(String, Uuid)>, Request)
    -> Result<Json<ExecutionDto>, Response>;
async fn validate_resume_inputs(State<WorkflowState>, WsContext, Path<(String, Uuid)>, Request)
    -> Result<Json<ValidateInputsDto>, Response>;

/// D9: 404 for a flow in the trash or deleted for good; disabled does not matter.
async fn live_flow_of(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail) -> EgretResult<()>;
/// What a resume finds waiting, from the scoped read (before any body is read).
enum Waiting { ByHand, Pause { run: Uuid, task_id: String, on_resume: Vec<crate::models::Input> }, LoopOnly }
fn waiting_of(detail: &crate::repo::ExecutionDetail) -> EgretResult<Waiting>;
/// Where a multipart part's file goes.
enum PartSink<'a> { Start { execution: Uuid }, Resume { execution: Uuid, task_id: &'a str, task_run: Uuid, request: Uuid }, Discard }
async fn receive_parts(workspace: Uuid, parsed: &crate::models::Flow, declared: &[crate::models::Input],
    owner: &str, sink: &PartSink<'_>, req: Request) -> Result<Uploaded, Response>;
async fn discard_prefix(prefix: String);
fn sink_prefix(fs: &crate::storage::FileStore, workspace: Uuid, parsed: &crate::models::Flow, sink: &PartSink<'_>) -> Option<String>;

// ExecutionDto
/// Package 7c D1/D11: `PAUSED` and no task run waits -- paused by hand.
pub paused_by_hand: bool,

// worker/sql_principal.rs
pub(crate) async fn check_resume_inputs(conn: &mut PgConnection, flow: &Flow,
    flow_id: Option<Uuid>, caller: Uuid) -> Result<()>;
```

**Handler shapes.**

- **`live_flow_of`** (D9). Factor the first half of `startable_flow` out as `live_flow(st, ws, flow_id) -> EgretResult<FlowRow>` (the `repo.get` and the `deleted_at` check, `routes.rs:2439-2447`). `startable_flow` calls it, then parses and checks disabled. Then:
  ```rust
  async fn live_flow_of(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail) -> EgretResult<()> {
      let flow_id = detail.execution.flow_id.ok_or_else(|| Error::NotFound("flow".into()))?;
      live_flow(st, ws, flow_id).await.map(|_| ())
  }
  ```
- **`check_resume_inputs`** (D7):
  ```rust
  const NOT_THE_CREATOR_RESUME: &str = "the flow contains an `egret.sql` task, which runs as the \
       flow's creator, and the waiting Pause takes onResume inputs that task can read, so only \
       the flow's creator can resume it";

  /// Package 7c D7 (7a/7b's D24, door 2): a resume of a `Pause` with a non-empty
  /// `onResume` on a flow that runs `egret.sql` is its creator's alone.
  pub(crate) async fn check_resume_inputs(conn: &mut PgConnection, flow: &Flow,
      flow_id: Option<Uuid>, caller: Uuid) -> Result<()> {
      let Some(flow_id) = flow_id else { return Ok(()) };
      if !runs_sql(flow) {
          return Ok(());
      }
      let creator: Option<Uuid> =
          sqlx::query_scalar("SELECT created_by FROM workflow.flows WHERE id = $1")
              .bind(flow_id)
              .fetch_optional(&mut *conn)
              .await
              .map_err(|e| Error::Internal(anyhow::Error::new(e)))?
              .flatten();
      match creator {
          Some(c) if c != caller => Err(Error::InvalidInput(NOT_THE_CREATOR_RESUME.into())),
          _ => Ok(()),
      }
  }
  ```
- **`waiting_of`**:
  ```rust
  fn waiting_of(detail: &crate::repo::ExecutionDetail) -> EgretResult<Waiting> {
      let by_hand = crate::executor::paused_by_hand_of(
          crate::worker::from_wire(&detail.execution.state),
          detail.runs.iter().map(|r| crate::worker::from_wire(&r.state)),
      );
      if by_hand {
          return Ok(Waiting::ByHand);
      }
      // The engine's own spelling (`executor::Resume::wire`): a `Pause` waits
      // with `NEVER` or `TERMINAL:*`; a `LoopUntil` with `ITERATE`.
      let Some(run) = detail.runs.iter().find(|r| {
          r.state == "PAUSED"
              && r.resume_with.as_deref().is_some_and(|w| w == "NEVER" || w.starts_with("TERMINAL:"))
      }) else {
          return Ok(Waiting::LoopOnly);
      };
      let flow = parse(&detail.source).map_err(|e| {
          Error::Internal(anyhow::anyhow!("its frozen source does not parse: {}", e.message))
      })?;
      let on_resume = crate::spec::path::find_task(&flow, &run.task_id)
          .and_then(|t| match &t.kind {
              crate::models::TaskKind::Pause(p) => p.on_resume.clone(),
              _ => None,
          })
          .unwrap_or_default();
      Ok(Waiting::Pause { run: run.id, task_id: run.task_id.clone(), on_resume })
  }
  ```
  `detail.runs` is in `run_ix` order (`ExecutionDto` doc), which matches `first_waiting_pause`. The worker re-decides under the lock and answers `Moved` if they disagree.
- **Multipart, generalised.**
  - `receive_parts` gains `declared`, `owner` and `sink`, and keeps `parsed` (for the namespace and the flow id). `read_parts` gains the same.
  - A part is looked up in `declared`. "No input" reads ``"{owner} declares no input `{name}`: each multipart part is named by an input id"``, and the start passes `owner = "the flow"`, so its text is unchanged.
  - A file part is stored by sink:
    - `Start { execution }`: `upload_input(fs, ws, &parsed.namespace, &parsed.id, execution, input, &file_name, &mut body)`, as today;
    - `Resume { execution, task_id, task_run, request }`: `upload_resume_input(...)`;
    - `Discard`: drain the field with `while field.chunk().await…?.is_some() {}` and give the **file name** as the value (D8: "the would-be value, not a URI"). Nothing is stored and `uploading` stays false.
  - On its own failure, `receive_parts` deletes `sink_prefix(...)` when `uploading`.
  - `sink_prefix`:
    - `Start` gives `fs.execution_prefix(ws, ns, flow, execution)`;
    - `Resume` gives `fs.resume_prefix(ws, ns, flow, execution, task_id, task_run, request)`;
    - `Discard` gives `None`.
  - `discard_uploads(workspace, parsed, execution)` becomes `discard_prefix(prefix)` (the same `delete_prefix` and warning). The start and the replay compute the prefix with `sink_prefix(.., &PartSink::Start { execution })`.
  - **A resume must never delete the execution's own prefix.** That is why the sink exists. Test 9 checks it.
  - `multipart_first_part(req) -> Result<Option<String>, Response>`: builds `axum::extract::Multipart::from_request(req, &())` and answers the name of the first field, or `None`. It is used where `onResume` is empty, to answer `resume_inputs_unexpected` without storing anything.
- **`pause_execution`** (D3):
  ```rust
  async fn pause_execution(
      State(st): State<WorkflowState>,
      WsContext(ctx): WsContext,
      Path((_ws, id)): Path<(String, Uuid)>,
  ) -> std::result::Result<Json<ExecutionDto>, ExecOpError> {
      egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
      let ws = ctx.workspace_id.0;
      let before = st.executions.get(ws, id).await?;
      if let (Some(root), Some(_)) = (before.execution.parent_execution_id, before.execution.loop_index) {
          return Err(ExecOpError::loop_iteration(id, root, "cannot be paused"));
      }
      live_flow_of(&st, ws, &before).await?;
      match crate::worker::pause_by_hand(&st.db.pool, ws, id).await? {
          crate::worker::Paused::Done => {}
          crate::worker::Paused::NotRunning(state) => {
              return Err(ExecOpError::conflict(
                  NOT_RUNNING_CODE,
                  format!("execution {id} is {state}; only a RUNNING or RESTARTED execution can be paused"),
              ));
          }
      }
      st.audit.emit(egret_audit::mutation_for(
          &ctx.principal,
          Some(ws),
          "workflow_execution:pause",
          "workflow_execution",
          Some(id.to_string()),
          serde_json::json!({ "from_state": before.execution.state }),
      ));
      let detail = st.executions.get(ws, id).await?;
      Ok(Json(ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)))
  }
  ```
- **`resume_execution`** (D5–D7, D9). Its steps:
  1. `require(WF_MANAGE)`.
  2. `before = executions.get` (404).
  3. `live_flow_of` (404).
  4. Not `PAUSED`: 409 `not_paused`, "execution {id} is {state}, not PAUSED; only a paused execution can be resumed".
  5. `waiting = waiting_of(&before)`.
  6. D7, before any body is read: `if let Waiting::Pause { on_resume, .. } = &waiting && !on_resume.is_empty()`, then `check_resume_inputs(&mut conn, &parse(&before.source)?, before.execution.flow_id, caller)` (400, no code).
  7. Body. `let request = Uuid::new_v4();`
     - multipart, with `Waiting::Pause { on_resume }` non-empty: `receive_parts(ws, &parsed, on_resume, &format!("the Pause `{task_id}`"), &PartSink::Resume { execution: id, task_id, task_run: run, request }, req)` → `Some(inputs)`, `files`, and `uploaded_prefix = sink_prefix(..)` when files is not empty;
     - multipart, otherwise: `multipart_first_part(req)`. `Some(_)` gives 400 `resume_inputs_unexpected`; `None` gives no inputs;
     - JSON (`read_json_body`): `inputs` absent or `null` gives `None`; an object gives `Some(map)`; anything else gives 400 with `start_inputs`'s message.
  8. `crate::worker::resume_with(&st.db.pool, &ResumeRequest { id, inputs: inputs.as_ref(), files: &files, expected })`, with `expected = Some(run)` for `Waiting::Pause` and `None` otherwise.
  9. Map the answer:
     - `Released(_)` and `ByHand`: OK;
     - `NotPaused`: 409 `not_paused` ("… its pause ended before the resume reached it");
     - `NoPauseWaiting`: today's uncoded 409 text;
     - `Refused(InputsUnexpected)`: 400 `resume_inputs_unexpected`, "execution {id} takes no inputs to resume: it is paused by hand, or the waiting Pause declares no onResume";
     - `Refused(Invalid(m))`: 400 uncoded `m`;
     - `Refused(Moved)`: 409 uncoded, "the Pause execution {id} waited on was resumed by another request; refresh and resume again".

     On any `Err` or refusal with `uploaded_prefix` set: `discard_prefix(prefix)`.
  10. Audit `workflow_execution:resume` with
      ```json
      {"from_state": "...", "by_hand": true|false, "task_run_id": "<uuid>|null", "inputs_given": true|false}
      ```
      `by_hand` = `matches!(outcome, Resumed::ByHand)`; `task_run_id` = the `Waiting::Pause` run; `inputs_given` = non-empty inputs.
  11. Read back, 200.
- **`validate_resume_inputs`** (D8). Its steps:
  1. `require(WF_MANAGE)`.
  2. `get` (404).
  3. `live_flow_of` (404).
  4. Not `PAUSED`: 409 `not_paused`.
  5. `waiting_of`.
  6. `declared` = the `onResume` list (empty for `ByHand`/`LoopOnly`).
  7. The posted values:
     - multipart: `receive_parts(.., declared, owner, &PartSink::Discard, req)`;
     - else: `read_json_body` + `start_inputs`.
  8. `Ok(Json(inputs_answer(declared_resume_inputs(&before.source, task_id), &flow, &declared, &posted)))`. No audit.

  Refactor `validate_inputs_answer(row, flow, posted)` into `inputs_answer(declared_yaml: HashMap<String, Value>, flow: &Flow, inputs: &[Input], posted: &Value) -> ValidateInputsDto`, with the same body, `crate::worker::input_outcomes_for(flow, inputs, posted)`, and `types` built from `inputs`. `validate_execution_inputs` calls `inputs_answer(declared_inputs(&row.source), &flow, &flow.inputs, &posted)`.

  `declared_resume_inputs(source, task_id: Option<&str>)` walks the YAML as JSON and returns, by id, the entries of the `onResume` array of the first object whose `id` equals `task_id` and that holds an `onResume` array. Write it as a recursive `fn find<'a>(v: &'a Value, id: &str) -> Option<&'a Vec<Value>>` over objects and arrays. `None` gives an empty map.
- **`ExecutionDto::new`** computes, before `d.execution` moves:
  ```rust
  let paused_by_hand = crate::executor::paused_by_hand_of(
      crate::worker::from_wire(&d.execution.state),
      d.runs.iter().map(|r| crate::worker::from_wire(&r.state)),
  );
  ```
  The field's doc: "`true` when the execution is `PAUSED` and no task run waits: it was paused by hand (`POST …/pause`). A resume sends it on; it takes no inputs. `false` for a `Pause` task or a `LoopUntil` waiting, and for every other state."
- **`ExecOpError`** gains `fn not_paused(id: Uuid, state: &str) -> Self` = `Self::conflict(NOT_PAUSED_CODE, format!("execution {id} is {state}, not PAUSED; only a paused execution can be resumed"))`.
- **Router** (`routes.rs:4887-4890`):
  ```rust
  .route(
      "/ws/{workspace}/workflows/executions/{id}/pause",
      post(pause_execution),
  )
  .route(
      "/ws/{workspace}/workflows/executions/{id}/resume",
      // As the start route: the handler limits each content type itself.
      post(resume_execution).layer(axum::extract::DefaultBodyLimit::disable()),
  )
  .route(
      "/ws/{workspace}/workflows/executions/{id}/resume/validate",
      post(validate_resume_inputs).layer(axum::extract::DefaultBodyLimit::disable()),
  )
  ```
- **OpenAPI.** Add `pause_execution` and `validate_resume_inputs` to `paths(...)` beside `resume_execution` (`routes.rs:4943`).
  - **Pause:** 200 `ExecutionDto` ("The execution is `PAUSED`. Runs already running finish and record their results; no run is created or started until it is resumed."); 400 (loop iteration, names its root); 401; 403; 404 (no such execution; its flow is in the trash or deleted); 409 `not_running` (not `RUNNING` or `RESTARTED`, `KILLING` included; the message names the state); 500.
  - **Resume:**
    - `request_body(description = "Optional. No body, `{}`, or `\"inputs\": null` resumes with no inputs. With `inputs` (JSON, `StartExecutionRequest`, at most 2 MiB; no `Content-Type` is read as JSON) or `multipart/form-data` (`StartExecutionMultipart`; one part per input id, a file part stored under the Pause run, at most `workflow.storage.max_file_bytes` + 1 MiB), the values are checked against the first waiting Pause's `onResume` as a start checks the flow's `inputs:`, and written to its outputs as `onResume`.", content((StartExecutionRequest = "application/json"), (StartExecutionMultipart = "multipart/form-data")))`;
    - 200 (paused by hand: back to `RUNNING`; a `Pause`: it ends `SUCCESS` with `outputs.onResume` and `outputs.resumed`, every other waiting `Pause` ends `SUCCESS` with `onResume: {}`; the body is the detail afterwards);
    - 400 (`resume_inputs_unexpected`; no code: a refused or undeclared input, named; the flow runs `egret.sql`, the Pause declares `onResume`, and the caller is not the flow's creator; no master key for a SECRET input);
    - 401; 403; 404 (execution; trashed or deleted flow);
    - 409 (`not_paused`; no code: only a `LoopUntil` waits, or another resume won);
    - 413; 415; 500; 503.
  - **Validate:** 200 `ValidateInputsDto` ("Every `onResume` input of the first waiting Pause, with the start check's errors; an empty body answers each with its default; an empty list when the execution is paused by hand or the Pause has no `onResume`. Nothing is stored: a file part is read and dropped, and its value is its file name. No audit."); 400; 401; 403; 404; 409 `not_paused`; 413; 415; 500.
- [ ] **Failing route tests** (list below) in `tests/pause_resume_routes.rs`. Copy `setup`, `flow_of`, `start`, `exec_state`, `expect_error` from `tests/restart_routes.rs:77-183`, and `multipart_body`/`Part`/`BOUNDARY` from `tests/file_inputs.rs:135-166`. Run `cargo test -p egret-workflow --test pause_resume_routes`: red.
- [ ] Implement.
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test pause_resume_routes --test pause_resume --test executions_routes --test restart_routes --test replay_routes --test routes --test inputs_start --test file_inputs --test secret_inputs --test validate_inputs_route`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] **OpenAPI:**
  ```bash
  cargo run -p egret-bin -- openapi > openapi.json
  cargo test -p egret-server committed_openapi_json_is_current
  ```
  `generated_client_was_emitted_from_the_committed_spec` now fails until T6; that is expected, and the controller runs T6 next.
- [ ] Commit: `feat(workflow): pause, resume with inputs and validate resume inputs routes (package 7c)`.

**Route tests** (`tests/pause_resume_routes.rs`). Each refusal is paired with an accepted control.

1. `pause_answers_200_paused_and_is_audited`:
   - `one_task` started (`a` `RUNNING`). `POST …/pause` gives 200, `state == "PAUSED"`, `paused_by_hand == true`.
   - The audit is `("workflow_execution:pause", Some(ex), {"from_state":"RUNNING"})`.
   - `GET` the execution: `paused_by_hand == true`. A `Pause`-task execution: `paused_by_hand == false`.
2. `pause_resume_and_validate_need_workflow_manage`: a `workflow:read`-only user gets 403 on all three; the state is unchanged.
3. `pause_refuses_what_is_not_running`:
   - `PAUSED` (paused twice): 409 `not_running`;
   - `KILLING`: kill a `one_task` with its run `RUNNING` (a kill of a claimed run stays `KILLING`; claim it first with `egret_workflow::worker::claim(&pool, 10)`), then 409 `not_running`, with the message containing `KILLING`;
   - terminal: 409 `not_running`.
4. `resume_of_a_running_execution_is_409_not_paused` (and validate the same).
5. `resume_by_hand_runs_on_and_inputs_are_refused`:
   - pause; `POST …/resume` with `{"inputs":{"x":1}}` gives 400 `resume_inputs_unexpected`, and the state stays `PAUSED`;
   - multipart with one text part gives 400 `resume_inputs_unexpected`;
   - no body gives 200, and the state is not `PAUSED`;
   - audit `{"from_state":"PAUSED","by_hand":true,"task_run_id":null,"inputs_given":false}`.
6. `resume_with_inputs_writes_them_and_a_refused_one_is_named`:
   - T4 test 16's flow (no `egret.sql`), driven to `PAUSED` with `settle_everything`;
   - `{"inputs":{"who":"ada","n":"x"}}` gives 400, and the error contains `` `n` ``;
   - `{"inputs":{"who":"ada","n":"3","zz":1}}` gives 400 containing `` `zz` ``;
   - `{"inputs":{"who":"ada","n":"3"}}` gives 200; the run `p`'s `outputs.onResume == {"who":"ada","n":3}`;
   - audit `{"by_hand":false,"task_run_id":<p's id>,"inputs_given":true}`.
7. `a_pause_without_on_resume_keeps_todays_resume`: an unbounded `Pause`. No body gives 200; `{"inputs":{"a":1}}` gives 400 `resume_inputs_unexpected` (on a fresh execution).
8. `only_a_loop_until_waiting_is_still_409`: today's text "Nothing to resume", no code.
9. `a_multipart_file_resume_stores_under_the_pause_run_and_a_refused_one_deletes_only_its_own`:
   - Arrange: `install_mem_for_tests()`; test 6's flow with `doc: FILE`, `PAUSED`; also start a **second** execution of a flow with a FILE start input uploaded by multipart, to have an `inputs/` object in the store.
   - Refused: multipart with `who=ada`, `n=x` and `doc` = `a.txt` gives 400. Then `fs.store.list(&fs.resume_root(ws, "t", "approve", ex, "p", run), None)` gives no objects, and the other execution's `inputs/` object still exists (`head` is `Some`).
   - Accepted: multipart with `who=ada`, `n=3` and `doc` gives 200. `outputs.onResume.doc` is a URI containing `/tasks/p/{run}/resume/`. `GET /api/ws/{ws}/workflows/executions/{ex}/files?path=<uri>` (the file route used by `tests/storage_files_api.rs`; copy its call) answers 200 with `hello`.
10. `the_egret_sql_gate_applies_only_with_on_resume`:
    - a flow with an `egret.sql` task after a `Pause` with `onResume`, filed by user A (the workspace owner, `flow_of`). User B with `workflow:manage` resumes: 400, and the message contains "only the flow's creator can resume it"; the state stays `PAUSED`. User A resumes: 200.
    - The same flow with a `Pause` without `onResume`: user B gets 200.
    - A manual pause of an `egret.sql` flow: user B gets 200.

    `egret.sql` needs a served registry entry. Copy the flow-filing approach of `tests/restart_routes.rs`'s `the_egret_sql_gate…` test (grep `egret.sql` there). It never runs the SQL: the execution is driven only to the `Pause`.
11. `a_trashed_flow_is_404_and_a_disabled_one_is_not`:
    - `UPDATE workflow.flows SET deleted_at = now()`: pause, resume and validate give 404;
    - restore it, then set the flow's YAML `disabled: true` in `workflow.flows.source`: pause gives 200 and resume gives 200.
12. `a_loop_iteration_is_400_naming_its_root`: T3 test 6's `Loop` flow. Pause the first iteration's execution id: 400, and the error contains the root id; nothing changes.
13. `validate_with_an_empty_body_answers_the_defaults` (`resume-validate.yaml`'s shape: `approved: BOOL, defaults: true`): 200; `inputs[0].value == true`, `isDefault == true`, `errors == []`. A paused-by-hand execution: `inputs == []`.
14. `validate_with_values_answers_per_input_errors_and_stores_nothing`:
    - JSON `{"inputs":{"n":"x"}}`: 200, and the `n` entry has one error containing `` `n` ``;
    - multipart with a `doc` file part: 200; `fs.store.list(resume_root…)` is empty; the `doc` value is `"a.txt"`; no audit row.
15. `the_workflows_entitlement_gates_the_three_routes` (as `restart_routes.rs:222-232`).
16. `database_url_is_exported_rather_than_read_from_dotenv`.

---

## T6. Generated client (controller)

- [ ] In `frontend/`, after the `node_modules` check of the Global Constraints, run `npm run gen:api` (never bare `orval`).
- [ ] Check `frontend/src/api/generated/workflow/workflow.ts` for:
  - `useWorkflowPauseExecution`;
  - `useWorkflowValidateResumeInputs` and its raw function `workflowValidateResumeInputs`;
  - `useWorkflowResumeExecution`, whose variables now carry `data`;
  - `getWorkflowResumeExecutionUrl`;
  - `ExecutionDto.paused_by_hand: boolean`.

  Record the exact names for T7.
- [ ] Run `cargo test -p egret-server generated_client_was_emitted_from_the_committed_spec` (green).
- [ ] Commit: `chore(frontend): regenerate the API client (package 7c)`.

---

## T7. Frontend: Pause, ResumeDialog with inputs, "Paused by hand" (spec D11)

Read `docs/en/frontend-rules.md` first; every MUST rule applies. The ones this task meets:
- FE-ACTION-1 (confirm);
- FE-ACTION-2 (toast only when staying);
- FE-ACTION-3 (failure shows the server's message and clears nothing);
- FE-ACTION-4 (a disabled control says why);
- FE-STATE-2 (skeleton);
- FE-FORM-2 and FE-FORM-4 (labels, errors beside fields);
- FE-COPY-1 to FE-COPY-3;
- FE-TEST-1.

**Files:**
- Modify:
  - `frontend/src/components/workflows/executions/actions.ts` (+ `__tests__/actions.ops.test.ts`);
  - `frontend/src/components/workflows/run/inputModel.ts:120-164`;
  - `frontend/src/routes/workflows/ExecutionDetail.tsx` (`:4`, `:42-57`, `:269-353`, `:355-542`);
  - `frontend/src/routes/workflows/queryKeys.ts`;
  - `routes/workflows/__tests__/ExecutionDetail.test.tsx`;
  - every test fixture that `npm run typecheck` reports missing `paused_by_hand` (expected: `ReplayDialog.test.tsx:25-46`, `ExecutionOutputsTab.test.tsx`, `ExecutionDetail.test.tsx`; add `paused_by_hand: false`).
- Create: `frontend/src/components/workflows/executions/ResumeDialog.tsx` and `__tests__/ResumeDialog.test.tsx`.

**Interfaces.**

Consumes: T6's hooks; `inputModel` (`formFromInputs`, `validateForm`, `requestFromForm`, `validationRequestFromForm`, `validationView`, `serverErrorsByInput`, `unsupportedInputs`); `InputField`; `useRefreshExecution`; `runExecutionAction`.

Produces:
```ts
// actions.ts
export function showsPause(execution: Pick<ExecutionDto, "state">): boolean; // RUNNING | RESTARTED | KILLING
export const PAUSE_KILLING_REASON: string;
export function canPause(execution: Pick<ExecutionDto, "state" | "parent_execution_id" | "loop_index">): Enablement;
export function canResume(execution: Pick<ExecutionDto, "runs" | "paused_by_hand">): Enablement; // changed
export function pauseRefusal(code: string | undefined): string | null;
export function resumeRefusal(code: string | undefined): string | null;
// inputModel.ts
export function runInputFrom(item: unknown): RunInput | null;
// ResumeDialog.tsx
export function ResumeDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (open: boolean) => void }): JSX.Element;
// queryKeys.ts
resumeInputs: (id: string) => ["workflows", "execution", id, "resume-inputs"] as const,
```

**Code.**

`actions.ts` additions (after `canResume`):
```ts
/**
 * Pause by hand (package 7c, D3, D11): shown while the execution runs, and while it is being
 * killed, where it is disabled with the reason (the server refuses `KILLING`).
 */
export function showsPause(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "RUNNING" || execution.state === "RESTARTED" || execution.state === "KILLING";
}

export const PAUSE_KILLING_REASON = "The execution is being killed, so it cannot be paused";

/** Pause, for an execution that `showsPause`: not while it is killed, and not a loop iteration. */
export function canPause(execution: Pick<ExecutionDto, "state"> & LoopFields): Enablement {
  if (execution.state === "KILLING") return { ok: false, reason: PAUSE_KILLING_REASON };
  return isLoopIteration(execution) ? { ok: false, reason: iterationReason(execution, "Pause") } : { ok: true };
}

/** One sentence for the pause route's coded refusal; `null` otherwise. */
export function pauseRefusal(code: string | undefined): string | null {
  return code === "not_running" ? "The execution is no longer running, so it cannot be paused. Refresh to see its latest state." : null;
}

/** One sentence for each coded refusal of the resume and resume-validate routes; `null` otherwise. */
export function resumeRefusal(code: string | undefined): string | null {
  switch (code) {
    case "not_paused":
      return "The execution is no longer paused. Refresh to see its latest state.";
    case "resume_inputs_unexpected":
      return "This execution takes no inputs to resume. Resume it without inputs.";
    default:
      return null;
  }
}
```
`LoopFields` and `iterationReason` are declared further down the file (`:190-199`). Move both above `showsPause`. `canResume` becomes:
```ts
/** Resume, for an execution that `showsResume`: enabled when it was paused by hand or a `Pause` task waits (spec §5.2; 7c D1). */
export function canResume(execution: Pick<ExecutionDto, "runs" | "paused_by_hand">): Enablement {
  return execution.paused_by_hand || aPauseIsWaiting(execution) ? { ok: true } : { ok: false, reason: LOOP_WAIT_REASON };
}
```

`inputModel.ts`: move the loop body of `inputsFromSource` (`:133-162`) into
```ts
/** One `inputs:` (or `onResume:`) entry of a flow's YAML as the form's model; `null` for one with no string `id` or `type`. */
export function runInputFrom(item: unknown): RunInput | null {
  if (!isRecord(item) || typeof item.id !== "string" || typeof item.type !== "string") return null;
  // ... the existing object literal, unchanged ...
}
```
`inputsFromSource` then does `const inputs = raw.map(runInputFrom).filter((i): i is RunInput => i !== null);`.

`ResumeDialog.tsx`:
```tsx
import { useId, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { customInstance } from "@/api/mutator";
import {
  getWorkflowResumeExecutionUrl,
  useWorkflowResumeExecution,
  useWorkflowValidateResumeInputs,
  workflowValidateResumeInputs,
  type workflowResumeExecutionResponse,
} from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { resumeFeedback, resumeRefusal } from "@/components/workflows/executions/actions";
import { useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  formFromInputs,
  requestFromForm,
  runInputFrom,
  serverErrorsByInput,
  unsupportedInputs,
  validateForm,
  validationRequestFromForm,
  validationView,
  type FieldValue,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const BY_HAND = "The execution continues: the tasks that were waiting start now.";
const NO_INPUTS =
  "Every paused task ends now as a success, whatever its duration or timeout, and the execution continues with the tasks after it.";
const WITH_INPUTS = "Fill in what the paused task asks for. It then ends as a success and the execution continues.";

/**
 * Resume (package 7c, D11): asks `resume/validate` with an empty body when it opens, which answers
 * the waiting Pause's `onResume` inputs with their defaults. With none it is today's confirm; with
 * some it shows the Run-flow inputs form, checks the values on the server first, then resumes with
 * JSON (multipart when a file is chosen). A refusal stays in the dialog (FE-ACTION-3).
 */
export function ResumeDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <AlertDialog open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto">{props.open ? <ResumeForm {...props} /> : null}</AlertDialogContent>
    </AlertDialog>
  );
}

function ResumeForm({ ws, execution, onOpenChange }: { ws: string; execution: ExecutionDto; onOpenChange: (open: boolean) => void }) {
  const idPrefix = useId();
  const refresh = useRefreshExecution(ws, execution.id);
  const resume = useWorkflowResumeExecution();
  const validate = useWorkflowValidateResumeInputs();
  const asked = useQuery({
    queryKey: workflowsKeys.resumeInputs(execution.id),
    queryFn: () => workflowValidateResumeInputs(ws, execution.id, {}),
    staleTime: 0,
    gcTime: 0,
    retry: false,
  });
  const inputs: RunInput[] = useMemo(
    () =>
      asked.data?.status === 200
        ? asked.data.data.inputs.map((o) => runInputFrom(o.input)).filter((i): i is RunInput => i !== null)
        : [],
    [asked.data],
  );
  const [values, setValues] = useState<Record<string, FieldValue>>({});
  const form: FormState = useMemo(() => {
    const base = formFromInputs(inputs);
    return { ...base, values: { ...base.values, ...values } };
  }, [inputs, values]);
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const loading = asked.isPending;
  const loadError =
    asked.error instanceof ApiError
      ? (resumeRefusal(asked.error.code) ?? asked.error.message)
      : asked.isError
        ? "The inputs could not be loaded. Close the dialog and try again."
        : null;
  const unsupported = unsupportedInputs(inputs);
  const description = execution.paused_by_hand ? BY_HAND : inputs.length > 0 ? WITH_INPUTS : NO_INPUTS;

  const setValue = (id: string, v: FieldValue) => {
    setValues((s) => ({ ...s, [id]: v }));
    setClientErrors((e) => without(e, id));
    setServerErrors((e) => without(e, id));
  };

  const confirm = async () => {
    setPending(true);
    setError(null);
    setServerErrors({});
    try {
      let res: workflowResumeExecutionResponse;
      if (inputs.length === 0) {
        res = await resume.mutateAsync({ workspace: ws, id: execution.id, data: {} });
      } else {
        const errors = validateForm(form);
        setClientErrors(errors);
        if (Object.keys(errors).length > 0) return;
        // The server's own check first (D11): its per-input errors sit beside their fields.
        const checked = await validate.mutateAsync({ workspace: ws, id: execution.id, data: validationRequestFromForm(form) });
        if (checked.status === 200) {
          const view = validationView(checked.data, inputs);
          if (Object.keys(view.errors).length > 0) {
            setServerErrors(view.errors);
            return;
          }
        }
        const req = requestFromForm(form);
        res =
          "json" in req
            ? await resume.mutateAsync({ workspace: ws, id: execution.id, data: req.json })
            : // The generated function JSON.stringifies its body: a multipart resume goes through the mutator with the FormData (as RunFlow does).
              await customInstance<workflowResumeExecutionResponse>(getWorkflowResumeExecutionUrl(ws, execution.id), {
                method: "POST",
                body: req.multipart,
              });
      }
      if (res.status !== 200) throw new Error("The execution could not be resumed. Try again.");
      refresh();
      // The page stays, so a toast confirms it (FE-ACTION-2), in terms of the answered state.
      const feedback = resumeFeedback(res.data.state);
      if (feedback.tone === "success") toast.success(feedback.message);
      else toast.warning(feedback.message);
      onOpenChange(false);
    } catch (e) {
      const sentence = e instanceof ApiError ? resumeRefusal(e.code) : null;
      if (sentence) {
        setError(sentence);
        refresh();
      } else {
        const message = e instanceof Error && e.message ? e.message : "The execution could not be resumed. Try again.";
        const { fields, rest } = serverErrorsByInput(message, inputs);
        if (Object.keys(fields).length > 0) {
          setServerErrors(fields);
          if (rest.length > 0) setError(rest.join(" "));
        } else setError(message);
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>Resume this execution?</AlertDialogTitle>
        <AlertDialogDescription>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      {loading ? (
        <div aria-busy="true" className="grid gap-2">
          <Skeleton className="h-4 w-48" />
          <span className="sr-only">Reading the inputs</span>
        </div>
      ) : loadError ? (
        <Alert variant="destructive">
          <AlertDescription>{loadError}</AlertDescription>
        </Alert>
      ) : inputs.length > 0 ? (
        <div className="grid gap-3">
          {unsupported.length > 0 ? (
            <p id={`${idPrefix}-unsupported`} className="text-sm text-muted-foreground">
              {`${unsupported.map((u) => `${u.id} (${u.type})`).join(", ")} cannot be entered in this form`}
            </p>
          ) : null}
          {inputs.map((input) => (
            <InputField
              key={input.id}
              input={input}
              value={form.values[input.id] ?? null}
              onChange={(v) => setValue(input.id, v)}
              error={serverErrors[input.id] ?? clientErrors[input.id]}
              idPrefix={idPrefix}
            />
          ))}
        </div>
      ) : null}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
        <Button
          type="button"
          disabled={pending || loading || loadError !== null || unsupported.length > 0}
          aria-describedby={unsupported.length > 0 ? `${idPrefix}-unsupported` : undefined}
          onClick={() => void confirm()}
        >
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Resume
        </Button>
      </AlertDialogFooter>
    </>
  );
}

function without(errors: Record<string, string>, id: string): Record<string, string> {
  if (!(id in errors)) return errors;
  const next = { ...errors };
  delete next[id];
  return next;
}
```
Use the exact names T6 recorded. If orval names the raw function differently (for example with a `Response` suffix), import that name. If `useQuery`'s `queryFn` result shape differs from the hooks' (`{ status, data }`), follow `useWorkflowGetExecution`'s use at `ExecutionDetail.tsx:107-114`.

`ExecutionDetail.tsx`:
- **Imports:** `Pause as PauseIcon` from lucide; `useWorkflowPauseExecution`; `canPause`, `showsPause` and `pauseRefusal` from actions; `ResumeDialog`. Delete `resumeFeedback` and `useWorkflowResumeExecution` from this file's imports once `runResume` is gone.
- **`type Pending`** gains `"pause"`.
- **In `ExecutionActions`:**
  ```tsx
  const pause = useWorkflowPauseExecution();
  const pauseShown = showsPause(execution);
  const pauseState = canPause(execution);
  const runPause = () =>
    runExecutionAction({
      call: () => pause.mutateAsync({ workspace: ws, id: execution.id }),
      expected: 200,
      failure: "The execution could not be paused. Try again.",
      refresh,
      refusal: pauseRefusal,
      onSuccess: () => toast.success("Paused the execution"),
    });
  ```
  Delete `runResume` and the `resume` hook.
- **Buttons:** before the Kill button, add
  ```tsx
  {pauseShown ? (
    <ReasonButton enablement={pauseState} reasonId={`${reasonsId}-pause`} variant="outline" onClick={() => setOpen("pause")}>
      <PauseIcon aria-hidden />
      Pause
    </ReasonButton>
  ) : null}
  ```
  and the matching `DisabledReasons` entry `{ id: `${reasonsId}-pause`, label: "Pause", enablement: pauseState }`.
- **Replace the Resume `ConfirmAction`** (`:489-497`) with
  ```tsx
  <ResumeDialog ws={ws} execution={execution} open={open === "resume"} onOpenChange={(o) => setOpen(o ? "resume" : null)} />
  <ConfirmAction
    open={open === "pause"}
    onOpenChange={(o) => setOpen(o ? "pause" : null)}
    title="Pause this execution?"
    description="Runs that are already running finish; nothing new starts until you resume."
    confirmLabel="Pause"
    onConfirm={runPause}
    fallbackError="The execution could not be paused. Try again."
  />
  ```
  `runExecutionAction` must throw on failure so `ConfirmAction` keeps the dialog open. If it returns instead, follow how `runSetStatus` is wired to `ChangeStateDialog` and make `onConfirm` throw the refusal sentence.
- **Header** (`:284`): `status={<span className="flex items-center gap-2"><StateChip state={execution.state} kind="execution" />{execution.paused_by_hand ? <span className="text-sm text-muted-foreground">Paused by hand</span> : null}</span>}`.

`queryKeys.ts`: add `resumeInputs: (id: string) => ["workflows", "execution", id, "resume-inputs"] as const,` beside `execution`.

**Tests** (vitest; Arrange / Act / Assert; every test can fail):
- `actions.ops.test.ts`:
  - `showsPause` is true for `RUNNING`, `RESTARTED` and `KILLING`, false for `PAUSED`, `SUCCESS` and `CREATED`;
  - `canPause` is disabled with `PAUSE_KILLING_REASON` on `KILLING`, disabled with a reason naming the parent on a loop iteration, and ok on `RUNNING`;
  - `canResume` is ok for `{paused_by_hand: true, runs: []}`, ok for a `NEVER` run, and gives `LOOP_WAIT_REASON` for only an `ITERATE` run;
  - `pauseRefusal("not_running")` and both `resumeRefusal` codes are distinct and non-null; an unknown code gives `null`.
- `ResumeDialog.test.tsx`: copy the `Server` class pattern of `ReplayDialog.test.tsx:61-80`, with routes `POST …/resume/validate` → `this.validate` and `POST …/resume` → `this.resume`. Tests:
  1. no inputs (`validate` answers `{id, namespace, inputs: [], checks: []}`): the description is `NO_INPUTS`; Resume posts `{}` to `/resume` once; toast `"Resumed the execution"`;
  2. paused by hand (`paused_by_hand: true`): the description is `BY_HAND`;
  3. with inputs: `validate` answers an INT `n` (`input: {id:"n", type:"INT"}`, `value: null`). The `n` field is labelled. Typing `x` and clicking Resume shows the client error beside `n`, and `/resume` is not called. Typing `3`: `validate` is called with `{inputs:{n:"3"}}`, then `/resume` with `{inputs:{n:"3"}}`;
  4. a server field error: the second `validate` call answers `errors: [{message: "Invalid value for input `n`. Cause: it must be more than `5`", renderError: false}]`. The message shows beside `n`, the dialog stays open, and `/resume` is not called;
  5. a coded refusal: `/resume` answers 400 `{code: "resume_inputs_unexpected"}`. The sentence shows and the dialog stays open;
  6. loading: while `validate` is pending a skeleton with "Reading the inputs" shows and Resume is disabled;
  7. a FILE input: choosing a file sends a `FormData` body (the `body` is an instance of `FormData`).
- `ExecutionDetail.test.tsx`:
  - extend the fake server (`:92-122`) with `pause: Reply` (`POST ${EXEC_PATH}/pause`) and `resumeValidate: Reply` (`POST ${EXEC_PATH}/resume/validate`, default `{status: 200, body: {id: "f", namespace: "n", inputs: [], checks: []}}`);
  - update the Resume tests (`:384-482`): the dialog title is still "Resume this execution?" and the refusal test still finds the server's message;
  - add: a `RUNNING` execution shows **Pause**; confirming posts `/pause` and toasts "Paused the execution"; a 409 `not_running` keeps the dialog open with `pauseRefusal`'s sentence; `KILLING` shows Pause disabled with `PAUSE_KILLING_REASON`; `paused_by_hand: true` shows "Paused by hand" in the header and an enabled Resume.
- [ ] Run, in `frontend/`:
  - `npm run typecheck`;
  - `npm run lint`;
  - `npx vitest run src/components/workflows/executions src/routes/workflows/__tests__/ExecutionDetail.test.tsx src/components/workflows/run`;
  - `npm run build`;
  - `npm run test:bundle:workflows`.
- [ ] Remove the `node_modules` symlink if T7 created it.
- [ ] Commit: `feat(workflows): pause by hand, resume with inputs, paused-by-hand note (package 7c)`.

---

## T8. Docs in this repo, and the gates (spec D13)

- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. The rows are not edited; add a dated note block in the style of `:162-183`:
  ```markdown
  **Notes, 2026-11-15 (package 7c, `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` Q1; the rows above are not edited).**
  13. **#7:** 7c builds pause by hand (`POST …/pause`), resume with `onResume` inputs and resume-validate (`POST …/resume/validate`). Row #7's "pause" and "Resume with `onResume` inputs, and resume-validate" are 7c's.
  14. **#9:** `Pause` `onResume` moved to 7c (user decision Q1, 2026-11-15). #9 keeps `Pause` `behavior:`, `onPause:`, `errors:` and `finally:` (still refused, `unsupported_pause_property`) and `AllowFailure` block `retry:`.
  ```
  Appendix B `:425` (`flow.Pause`) and `:556` (`POST /{id}/actions/resume`) each get a trailing *Corrected 2026-11-15 (7c)* sentence: `onResume` and resume with inputs are supported; resume-validate exists; resume-from-breakpoint is still not. Number the notes after the highest existing note number (grep `^[0-9]*\. \*\*#` in that section).
- [ ] **Inputs spec** `docs/en/specs/2026-10-28-workflow-inputs-design.md:43`: append *Corrected 2026-11-15: resume with `onResume` inputs and resume-validate moved to package 7c (`docs/en/specs/2026-11-15-workflow-pause-resume-design.md` Q1).*
- [ ] **This package's spec:**
  - §5 gains the divergences the user confirmed from S1, S2, S3 and S7 (each as a numbered item);
  - divergence 6 is reworded for S2's path;
  - §7 "As built" stays `(Filled in at the end of the implementation.)`; the controller fills it after the review.
- [ ] **Vietnamese copies:** mirror every paragraph changed above in `docs/vi/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, `docs/vi/specs/2026-10-28-workflow-inputs-design.md` and `docs/vi/specs/2026-11-15-workflow-pause-resume-design.md` (code, identifiers and commands untranslated).
- [ ] **Gates**, one at a time, each read from a file in `$SCRATCH`:
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow --no-fail-fast 2>&1 | tee "$SCRATCH/t8-workflow.txt"`: report passed, failed and ignored. Only the 7 `sql_starrocks_live` may fail;
  - `cargo test -p egret-config -p egret-server -p egret-metastore` (includes the OpenAPI and client-digest tests);
  - the ratchet and census, which must equal T2's measured numbers;
  - in `frontend/` (symlink as in Global Constraints, removed after): `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`.
- [ ] Commit: `docs(workflow): record package 7c in the roadmap and the inputs spec`.

---

## After the tasks (controller, not implementers)

- **User docs** (`egret-data/docs`, Platform → Workflows):
  - Executions → Run actions: Pause, Resume with inputs, the codes `not_running`, `not_paused`, `resume_inputs_unexpected`, S1's loop behaviour and S3's note;
  - the `Pause` task page: `onResume`, `outputs.<id>.onResume` and `outputs.<id>.resumed`, and SECRET masking;
  - new screenshots.

  A separate PR; docs PRs merge at once.
- **Live check** on a native `egret serve` against the worktree's own containers and throwaway stores (never the shared demo stack). Before it, grep the binary for `resume_inputs_unexpected` (memory: the build cache ships a stale binary). In a browser as `admin@egret.local`, light and dark:
  1. pause by hand while a slow task runs: it finishes, nothing new starts; resume: the flow carries on;
  2. resume a `Pause` with STRING, INT and FILE `onResume` inputs; a later Log shows them; the file downloads from the outputs tab;
  3. a refused input shows beside its field;
  4. the "Paused by hand" note.

  Take the docs screenshots.
- **Whole-branch review**, then spec §7 "As built" (commits per task, measured counts, deviations, the ratchet at T0 and at the end), its Vietnamese copy, and the PR (body ends with the attribution line).

---

## Self-review against the spec

| Spec item | Where |
|---|---|
| Q1 `onResume` in 7c; the other `Pause` gaps stay #9 | T2 (model, validate, keys; `pause_gap` untouched, test `on_resume_ids_may_repeat_a_flow_input_id_and_the_other_pause_gaps_stay`), T8 roadmap/inputs-spec notes |
| Q2 pause flips state only; live runs finish | T1 `pause_by_hand_commands`, T3 tests 1–3 |
| D1 derived, one helper, used by executor, route, DTO | T1 `paused_by_hand`/`_of`; T4 `resume_with`; T5 `waiting_of`, `ExecutionDto::new` |
| D2 no drive forward; run ending recorded; parent not completed; sibling not created | T1 `drives_forward` + test 3; T3 tests 2, 3, 7; S4 render gate test 8; S5 waker test 10; S1 test 6 |
| D3 route, permission, states, `KILLING` 409, loop iteration 400 naming root, lock, audit | T1 test 5; T3 `pause_by_hand` + test 4; T5 `pause_execution`, route tests 1–3, 12 |
| D4 model, same validator, path, unique ids, warning removed | T2 (S8 path spelling) |
| D5 body shapes; by hand; first `PAUSED` `Pause`; outputs; others `onResume: {}`; `RESTARTED`; `by` null; no `onResume`; `LoopUntil` 409 | T4 `resume_with` + tests 14–22; T5 `resume_execution` + route tests 5–8 |
| D6 FILE storage, URI value, size limit, delete on refusal, read rule + test | T4 S2 storage + test 23; T5 `PartSink` + route test 9 |
| D7 creator gate only with non-empty `onResume`, frozen source | T5 `check_resume_inputs` + route test 10 |
| D8 validate route, permission, `not_paused`, DTO, empty list, no file stored, no audit | T5 `validate_resume_inputs` + route tests 13, 14 |
| D9 trash/deleted 404, disabled allowed | T5 `live_flow_of` + route test 11 |
| D10 kill; 7a restart both cases; timer premise (S3); SLA as today | T3 tests 11, 12, 9; SLA untouched (`claim::sla_terminal` already acts on any non-terminal, non-`KILLING` execution) |
| D11 Pause button, confirm text, `KILLING` reason; Resume dialog validate-on-open, form, JSON/multipart, per-input errors; `paused_by_hand`; `gen:api` | T5 DTO field; T6; T7 |
| D12 no migration | Investigation record; Global Constraints |
| D13 docs | T8; "After the tasks" |
| §4 test list | executor T1; spec T2; worker T3/T4; routes T5; e2e T4 test 16; frontend T7; live check after |
| §5 divergences 1–7 | 1: T1 test 5, T5 route test 3; 2: T5 route test 10; 3: route test 11; 4: route test 2; 5: route test 5; 6: S2; 7: T7 ResumeDialog |

**Open points the controller must settle before T3 merges:** S1 (loop iterations under a paused root), S2 (the FILE path), S3 (a manual pause beside a waiting `Pause`), S7 (SECRET `onResume` sealed).
