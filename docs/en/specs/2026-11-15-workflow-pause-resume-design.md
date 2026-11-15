# Workflow package 7c: pause by hand, resume with `onResume` inputs, validate resume inputs

**Status:** designed 2026-11-15. Branch `feat/workflow-pause-resume`, one PR. Roadmap row #7 (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), third of the six sub-packages 7a–7f. 7a (restart in place, #860) and 7b (replay from a task, #866) are merged; their specs are `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md` ("7a Dn") and `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md` ("7b Dn"). Kestra is the checkout at `data-portal/kestra`. `ES` is `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `EC` is `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`, and `Pause.java` is `core/src/main/java/io/kestra/plugin/core/flow/Pause.java`.

## 1. Problem

egret already runs the `Pause` task (`pauseDuration`, `timeout`) and has `POST /api/ws/{ws}/workflows/executions/{id}/resume` (`routes.rs` `resume_execution`). That route takes no body: every paused `Pause` run ends `SUCCESS`. Kestra has three things egret lacks:

1. **Pause by hand** (`POST /{id}/actions/pause`, `EC:2017-2035`; `ES.pause`, `ES:907-916`). It sets a running execution to `PAUSED` and touches no task run. Runs already running finish, but the executor schedules nothing new (`ExecutorContext.canBeProcessed()` is false for `PAUSED`). Resume finds no `PAUSED` run and sets the execution back to `RUNNING` (`ES:880-897`).
2. **`onResume` inputs.** A `Pause` task declares `onResume: List<Input>` (`Pause.java:193`). Resume (`EC:1872-1906`, multipart) types and validates the posted values against the first `PAUSED` run's `onResume` (`ES.readInputs`, `FlowInputOutput.readExecutionInputs`). It writes them to that run's outputs as `{onResume: {...}, resumed: {by, on, to}}` (`Pause.java:305-330`; `ES.markAs`, `ES:603-661`), so the flow reads `{{ outputs.<pause>.onResume.<input> }}`.
3. **Validate resume inputs** (`POST /{id}/actions/resume/validate`, `EC:1854-1870`; `ES.validateForResume`, `ES:791-822`). It checks the posted values without resuming and answers the same shape as the start-inputs validation.

egret today warns on `onResume` and names roadmap row #9 (`spec/keys.rs`). `docs/en/specs/2026-10-28-workflow-inputs-design.md` also assigns it to #9, while 7a's spec assigns it to 7c. §2.1 Q1 settles this.

## 2. Decisions

### 2.1 User decisions, 2026-11-15 (asked, answered)

- **Q1. `onResume` is in 7c.** 7c models `Pause.onResume`, lifts its warning, validates resume inputs against it and writes the `onResume` and `resumed` outputs. The rest of `Pause` stays in row #9: `behavior`, `onPause`, `errors` and `finally` are still refused at publish (`unsupported_pause_property`). The inputs spec and the roadmap get a dated correction.
- **Q2. Pause by hand follows Kestra.** It flips the execution to `PAUSED` and changes no task run. Runs already `RUNNING` or `SUBMITTED` finish and record their results. No run is created or started until resume. Resume sets `RUNNING`. Rejected: stopping running tasks the way kill does and re-running them on resume, because it re-runs work that is not idempotent and Kestra does not do it.
- **Design approval, 2026-11-15:** the user approved part 1 (engine and API, D1–D10 below) and part 2 (UI, tests, docs, D11–D13).

### 2.2 Design decisions (default decisions, 2026-11-15)

- **D1. "Paused by hand" is a derived fact, not a new column.** An execution is paused by hand when its last state is `PAUSED` and no run's last state is `PAUSED`. This is the test Kestra's `ES.resume` uses (`findFirstByState(PAUSED)` empty, `ES:880-897`). A `Pause` task or a waiting `LoopUntil` always leaves a `PAUSED` run, so the two cases cannot be confused. One pure helper, `executor::paused_by_hand(&ExecutionState) -> bool`, owns the rule. Every caller (the executor, the resume route, `ExecutionDto`) uses it.
- **D2. A paused-by-hand execution does not drive forward.** `executor::drives_forward` returns false for it, as it does for terminal and `KILLING` executions. The worker's update pass (`worker::render::render_updates`) gates on the same function, so the two stay in step. Runs that are live keep running. A worker still writes their terminal state and outputs. The executor does not use them to create, start or complete any other run until resume. The plan must show by test that a run ending during the pause is recorded, and that its parent is not completed and its next sibling is not created.
- **D3. Pause by hand.**
  - Route: `POST /api/ws/{ws}/workflows/executions/{id}/pause`, no body.
  - Permission: `workflow:manage`.
  - Accepted states: `RUNNING` and `RESTARTED`. Any other state gives 409 code `not_running`.
  - `KILLING` gives 409 too. Kestra's `isRunning()` is true for `KILLING` (`State.java:304`), so Kestra pauses a killing execution. In egret, `PAUSED` after `KILLING` would stop `advance_killing`. §5 divergence 1.
  - A loop iteration is refused with 400 and no code; the message names its loop root, which is paused instead, as 7a's restart does. Pausing the loop root stops it starting new iterations. Iterations already running carry on.
  - Done inside the execution lock (`claim::load`), with a re-check of the state. It writes one `SetExecutionState { PAUSED }` and calls `settle`.
  - Answers 200 with the `ExecutionDto`.
  - Audit: `workflow_execution:pause`, entity `workflow_execution`, payload `{from_state}`.
- **D4. `Pause.onResume`.**
  - `models::task::Pause` gains `on_resume: Option<Vec<Input>>`.
  - Each entry is checked at publish by the same code that checks a flow's `inputs:` (types, ids, defaults, `dependsOn`), and the errors carry the path `tasks[..].onResume[i]`.
  - `onResume` inside `afterExecution:` cannot occur: a `Pause` there is already refused.
  - Ids must be unique within one `onResume`. They do not have to differ from the flow's `inputs:` ids, because they live under `outputs.<pause>.onResume`.
  - The `keys.rs` warning and its test are removed. A test proves `onResume` publishes without a warning.
- **D5. Resume takes an optional body.**
  - `POST …/resume` accepts no body, a JSON body `{"inputs": {...}}`, or `multipart/form-data` with one part per input (part name = input id). The shape is the same as 7b's replay (`StartExecutionRequest`, `StartExecutionMultipart`).
  - **Paused by hand (D1):** any non-empty `inputs` gives 400 code `resume_inputs_unexpected`. Otherwise the execution gets `RUNNING`, the next `settle` drives it forward, and the audit payload says `by_hand: true`.
  - **A `Pause` run waiting:** the first `PAUSED` `Pause` run in run order (not a `LoopUntil`) is the one whose `onResume` applies, as Kestra's `findFirstByState(PAUSED)`.
    - The posted values are resolved with `inputs_check::resolve` against its `onResume`, rendered against the execution as for a start. Undeclared keys give 400, as on start. A refused value gives 400 with the input named.
    - That run's outputs get `{"onResume": {...}, "resumed": {"by": null, "on": <now ISO-8601>, "to": "SUCCESS"}}`.
    - Every other `PAUSED` `Pause` run is released as today, with `resumed` only. They get `onResume: {}` so templates do not fail.
    - The execution goes to `RESTARTED` (the existing `pause_mark` rule).
    - `by` stays null, as in Kestra OSS (`Pause.Resumed.now()`). Who resumed is in the audit row.
  - **A `Pause` without `onResume`** keeps today's behaviour. An empty or absent body is accepted. Non-empty `inputs` gives 400 `resume_inputs_unexpected`.
  - **Only a `LoopUntil` waiting:** today's 409 "Nothing to resume" is unchanged.
- **D6. FILE `onResume` inputs.**
  - Each file part streams to storage under the execution prefix at `resume/{task_run_id}/{input}/{file}`. This is not `inputs/{input}/{file}`, so it cannot overwrite a start input with the same id.
  - The value stored in outputs is the storage URI, as for a start input.
  - The size limit and the header budget are the multipart gate's (`workflow.storage.max_file_bytes` + 1 MiB).
  - If the resume is then refused (a validation error, the D7 gate, a lost race on the lock), the uploaded files are deleted (`discard_uploads`, generalised to take a prefix).
  - The storage read rule already allows reads under the execution's own prefix. The plan confirms `resume/` is covered and adds a test.
- **D7. The `egret.sql` creator gate on inputs (security).** The flow may run `egret.sql`, and the waiting `Pause` may declare a non-empty `onResume`. In that case only the flow's creator may resume (400, no code; the message says the tasks run as the creator). The posted values become template values the creator's SQL can read, so a `workflow:manage` holder could otherwise shape SQL run under someone else's identity. This is the D24 rule of 7a and 7b (`sql_principal::check_start`, door 2). A resume of a `Pause` with no `onResume`, or of an execution paused by hand, does not change what runs, and stays open to any `workflow:manage` holder, as today. The check uses the execution's frozen source, which is what runs. §5 divergence 2.
- **D8. Validate resume inputs.**
  - Route: `POST …/resume/validate`, same body shapes as D5.
  - Permission: `workflow:manage` (it is half of resume).
  - The execution must be `PAUSED` (409 `not_paused` otherwise).
  - **A `Pause` run waiting:** it answers the start-validation DTO (`validate_execution_inputs`'s response, `inputs_check::resolve_all`) for that run's `onResume`. An empty body answers each input with its default, which is how the UI learns the form (D11).
  - **Paused by hand, or a `Pause` with no `onResume`:** it answers an empty input list.
  - Multipart file parts are read and discarded, never stored, as Kestra's `uploadFiles=false`. The answer carries the would-be value, not a URI.
  - No audit row: it changes nothing.
- **D9. Flow gates.** Pause, resume and validate refuse a flow in the trash or deleted permanently with 404, as 7a does. A disabled flow does not block pause or resume: Kestra checks neither, and stopping or finishing a run of a disabled flow is safe. §5 divergence 3.
- **D10. Interplay.**
  - Kill on a paused-by-hand execution works as on a running one (`kill_leaves`). Live leaves go `KILLING`, and the execution goes `KILLING`. `drives_forward` already stops at `KILLING`.
  - 7a's restart of a paused-by-hand execution follows 7a's rules unchanged. With no `FAILED` or `PAUSED` run it answers 409 `nothing_to_restart`. With a run that failed during the pause, it resets that run and the execution goes `RESTARTED`, which drives forward again. This is Kestra's `ES.restart` on a `PAUSED` execution.
  - A `Pause` run's `pauseDuration` or `timeout` timer cannot fire on a paused-by-hand execution, because a manual pause needs `RUNNING` or `RESTARTED`, and no run is then `PAUSED`.
  - SLA timers still fire. An SLA breach on a paused execution behaves as today on a `PAUSED` execution.

### 2.3 UI, tests, docs (approved part 2)

- **D11. UI** (execution page, `docs/en/frontend-rules.md`):
  - A **Pause** button, a `ReasonButton` beside Kill. It shows on `RUNNING` and `RESTARTED`, and is disabled with a reason on `KILLING`. It opens a confirm dialog: "Runs that are already running finish; nothing new starts until you resume."
  - The **Resume** dialog calls `resume/validate` with an empty body when it opens. With no inputs it stays today's confirm. With inputs it shows the inputs form the "Run flow" dialog uses, posts JSON (multipart when a FILE input is set), and shows per-input errors from a 400 or from validate.
  - A paused-by-hand execution shows "Paused by hand" in the overview, from a new `ExecutionDto.paused_by_hand: bool` that D1's helper fills.
  - The client is regenerated with `npm run gen:api`.
- **D12. No migration expected.** `PAUSED` is already a valid execution state. 0142's CHECK binds `PAUSED` runs only, and a manual pause writes none. Outputs use the existing run outputs. The plan verifies this. If it turns out wrong, the number goes above 0228 and above every open PR (memory: migration numbers are shared).
- **D13. Docs.**
  - `egret-data/docs`:
    - Executions → Run actions: add Pause, Resume with inputs and the new refusal codes.
    - The `Pause` task page: add `onResume` and `outputs.<id>.onResume` / `resumed`.
    - New screenshots.
  - This repo:
    - this spec and the plan, each with a Vietnamese copy under `docs/vi/`;
    - a dated correction in the roadmap (row #7 and row #9) and in the inputs spec (`onResume` moved to 7c).

## 3. Shape of the change

- **`executor`:** `paused_by_hand`; `drives_forward` learns it; a `resume_by_hand_commands` that returns `SetExecutionState { RUNNING }` for the D1 case; and `release_now` gains the outputs payload for the chosen run.
- **`models::task::Pause`, `spec::validate`, `spec::keys`:** `onResume` (D4).
- **`worker::claim`:**
  - `pause_by_hand(pool, ws, id)`, a new function (D3);
  - `resume_by_hand(pool, id, inputs)`, which gains the inputs and the outputs write and returns which case applied (D5).
- **`routes.rs`:**
  - the `pause` route;
  - the `resume` body (JSON or multipart);
  - the `resume/validate` route;
  - the `egret.sql` creator gate (D7);
  - error codes `not_running`, `not_paused`, `resume_inputs_unexpected`;
  - audit events;
  - OpenAPI.
- **`ExecutionDto.paused_by_hand`.**
- **Frontend:** `actions.ts` (`showsPause`, `canPause`), `ExecutionDetail.tsx` (the Pause button and dialog, a `ResumeDialog` with the inputs form) and the generated client.

## 4. Tests

- **Executor unit tests:**
  - `paused_by_hand` against each kind of `PAUSED` (manual, a `Pause` run, a `LoopUntil` wait, parallel pauses);
  - `drives_forward` is false for a manual pause;
  - a run ending during the pause creates no next run and completes no parent;
  - resume by hand gives `RUNNING`, and the next `advance` creates the next run;
  - releasing a `Pause` with inputs writes `{onResume, resumed}` and the execution goes `RESTARTED`.
- **Spec validation:**
  - `onResume` with each input type publishes without a warning;
  - a bad type, a duplicate id or a bad default is refused with the `onResume[i]` path;
  - the removed warning's test is replaced.
- **Worker (DB) tests:**
  - pause under the lock, and races: pause against kill, and pause against a run completing;
  - a run completing during the pause is recorded, and nothing new is created;
  - resume by hand;
  - resume with inputs writes outputs;
  - kill on a manual pause;
  - 7a restart on a manual pause (both D10 cases).
- **Route tests:**
  - 403 with `workflow:read` only, for pause, resume and validate;
  - 409 `not_running` (`PAUSED`, `KILLING`, terminal) and 409 `not_paused`;
  - 400 `resume_inputs_unexpected`;
  - 400 for a refused input, naming it;
  - multipart FILE resume stores under `resume/{run}/…`, and a refused one deletes it;
  - the D7 gate: a non-creator is refused with `onResume` and allowed without it;
  - 404 for a flow in the trash;
  - a loop iteration gets 400 naming its root;
  - audit payloads;
  - validate with an empty body answers defaults;
  - validate with values answers per-input errors;
  - validate stores no file.
- **An end-to-end executor test:** a flow `Pause` (`onResume`: STRING, INT, FILE) → `Log` that reads `outputs.p.onResume.*`. The resume posts values, and the log line carries them.
- **Frontend (vitest):**
  - `showsPause` and `canPause`;
  - the Resume dialog with no inputs (confirm) and with inputs (form, validate on open, errors shown);
  - the Pause dialog.
- **Live check**, against a native `egret serve` on throwaway stores: pause by hand while a slow task runs (it finishes, nothing new starts), then resume (the flow carries on); resume a `Pause` with inputs, including a FILE; a refused input; light and dark mode.

## 5. Recorded divergences from Kestra

1. **A `KILLING` execution cannot be paused** (D3). Kestra allows it because `isRunning()` includes `KILLING`. In egret it would stop the kill.
2. **Security: the `egret.sql` creator gate on resume inputs** (D7). Kestra OSS has no per-flow identity. egret runs `egret.sql` as the flow's creator, so inputs that feed it are limited to the creator.
3. **Trash and deleted flows are refused with 404** (D9). This is the gate 7a and 7b use. A disabled flow is not refused, as in Kestra.
4. **Pause and resume need `workflow:manage`.** Kestra OSS checks no permission on the server; its UI checks `EXECUTION:UPDATE`.
5. **A paused-by-hand execution answers 400 to resume inputs** (`resume_inputs_unexpected`). Kestra silently ignores them, because there is no `Pause` to read them against. egret refuses rather than drop data (the no-silent-drops rule, `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md`).
6. **FILE `onResume` inputs are stored under `resume/{task_run_id}/`.** Kestra stores them where start inputs go (`storageInterface.from(execution, inputId, …)`), so a resume input can overwrite a start input with the same id.
7. **The UI reads the inputs from `resume/validate`.** Kestra's UI reads `task.inputs` of the paused task (`Resume.vue:72-81`), not `onResume`, and never calls `validateResume`. egret uses the server's answer, so the form matches what the server checks.

## 6. Out of scope

- Pause and resume by ids or by query: sub-package 7f.
- Breakpoints and resume from breakpoint: their own package (7b Q3).
- `Pause` `behavior`, `onPause`, `errors` and `finally`: row #9.
- Resume of a `Pause` other than the first `PAUSED` one (Kestra has no such parameter either).

## 7. As built

(Filled in at the end of the implementation.)
