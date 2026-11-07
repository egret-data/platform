# Survey for roadmap package #6, "`Loop` own `outputs:`"

Worktree: `/Users/1pro-l4r8/Documents/github/data-portal/egret-loopout` (branch `feat/workflow-loop-outputs`, from main `6dc1dcf9`, which holds #5a; ratchet 301 / 40 / 40 / 0).
#5b (PR #780, branch `feat/workflow-renderer-b`, worktree `egret-render`) is not in this branch. Its ratchet is 301 / 35 / 35 / 0, and this survey counts from that, because #6 will be rebased onto it.
Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` (checkout `96893b589`, 2026-08-17). The oracle's engine is the image pinned by digest in `oracle-exec/docker-compose.yml:31` (`v2.0.0-rc13`, a different build). `javap` on `io.kestra:core:2.0.0-rc13` shows the same `Loop` API: `outputs`, `fetchType`, `computeIterationOutput`, `guessOutputMode`, `LoopOutput`, `LoopItem`.
Abbreviations: `EW` = `crates/egret-workflow/`, `OE` = `EW oracle-exec/`, `KX` = `kestra/executor/src/main/java/io/kestra/executor/`, `KL` = `kestra/core/src/main/java/io/kestra/plugin/core/flow/Loop.java`.
This is a read-only survey. Items marked **(unverified)** were inferred and not measured.

Roadmap anchors (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`): row 6 at `:79`, note 6 (flow-level `outputs:`) at `:157`, the cause list at `:277-279` and `:300-301`, and the cause table at `:334` and `:338`.

## 0. How the premises were measured

- **Golden side:** `OE golden.json` (states only), and `OE traces.json` (the image's own executions, with timestamps and `parentTaskRunId`). Both were read with scripts. Neither records task or execution **outputs**, so no oracle can see what an output contains. The oracle sees only the states that a failed or missing output causes.
- **egret side:** the oracle's own printouts from earlier runs in the scratchpad. These are `a7-exec5.txt` (#5a, 40 baselined) and `OE baseline.txt` on main and on `egret-render` (#5b). The code was also read.
- **Not run:** `cargo test -p egret-workflow --test executor`. The disk is tight, and the printouts above already contain the per-entry facts this survey needs. Bank counts are attributions, and task T0 below measures them.

## 1. The four "unattributed `Loop`" entries, named

The roadmap (`:300`, `:338`) lists `loop-object`, `loop-object-in-list`, `if-in-flowable` and `retry-flowable-parallel` with "no cause text anywhere". Three of the four are renderer causes, and package #5 already banks them. The fourth is a `Loop` `retry:` that never reaches the iteration. None of the four is a `Loop` `outputs:` cause.

| entry | golden vs egret | cause | owner, status |
|---|---|---|---|
| `valids/if-in-flowable` | the `If` on `{{ item.value equals 'value 2' }}` FAILED in egret; it is SUCCESS in Kestra | `equals` did not parse | #5a Task A6, **banked** (`OE baseline.txt:590`, on main) |
| `valids/loop-object` | egret spawns **1** sub-execution, Kestra **3** (`a7-exec5.txt:117`, "top-level spawned keys … (1, 3)"). It is the vacuity control because its top-level run count differs (`a7-exec5.txt:130`, `tests/executor.rs:4236-4257`). The first iteration's `Switch` on `{{ item.value is json }}` FAILED, and `transmitFailed` (default `true`) then ended the `Loop` FAILED, so `2_end` never ran | `json` is an unknown test on main (`EW src/runners/env.rs:3077`, "`empty`, `json`, `map`, `null` … still unknown") | #5b Task B2, **banked on `egret-render`** (`egret-render/…/oracle-exec/baseline.txt:598`). Lands when #780 merges |
| `valids/loop-object-in-list` | the same, (1, 3) | the same | the same |
| `valids/retry-flowable-parallel` | It differs **only inside `spawned`** (`a7-exec5.txt:166`). In Kestra each of the two sub-executions runs `child` **3** attempts, with execution history `CREATED RUNNING FAILED RETRYING RUNNING FAILED RETRYING RUNNING FAILED`. In egret each runs it **once** (from the code, see below) | The `Loop`'s `retry:` is not inherited by the iteration's tasks | **Unowned.** Proposed for #6a (§5, Q2) |

**The `retry-flowable-parallel` mechanism, with evidence:**

- **Kestra.** `KX ExecutorService.java:576-599` gives a failed `RunnableTask` its own `retry:`. If it has none, it uses the nearest ancestor's (`searchForParentTaskWithRetry`, `:841-856`). That search runs only when `taskRun.getParentTaskRunId() != null`, and it walks the **flow's task tree** (`Flow.findParentTasksByTaskId`, `Flow.java:227-234`). Inside a LOOP sub-execution the body's root tasks keep a non-null `parentTaskRunId`: in `traces.json` both iterations' `child` runs carry `parentTaskRunId` `29lJTb3g…`. The walk therefore reaches the `Loop` in the parent flow and finds `retry: constant, maxAttempts: 3`.
- **egret.** `spawn::sub_flow_source` (`EW src/worker/spawn.rs:830-899`) makes the iteration's flow from the parent flow. It swaps the `Loop`'s `tasks` in as the root `tasks` and copies the `Loop`'s `errors`/`finally`. The `Loop`'s `retry:` is dropped. `retry::policy_for` (`EW src/executor/retry.rs:156-190`) walks only the **run** parent chain, which stops at the sub-execution's root, and then falls back to `plan.retry`, the parent flow's retry (none here). So `child` fails once and is not retried.
- **Timing (from `traces.json`).** The parent's `flowable` run ends FAILED at `19:16:08.124`, after both sub-executions' last FAILED at `07.810`. The sub-execution's first `FAILED` at `04.44` therefore did **not** end the `Loop`. Kestra stamped `RETRYING` before any terminal report reached the parent. egret reports only the state a settle ends in (`EW src/worker/claim.rs:3509-3516`), so it should behave the same if `FAILED` and `RETRYING` are written in one settle. That is **(unverified)**, and a test must pin it (§4, R3).
- The README already says it "is the `Loop`" that holds this entry (`OE baseline.txt:343-350`). It never said which property.

**Two neighbouring silent drops.** Package #1 left no `not_honoured` warning for either key:

- A `Loop`'s `retry:` has no effect at all. Kestra retries only `RunnableTask`/`Subflow` runs, and the body's runs are in another execution.
- The flow-level `outputs:` is never evaluated (`EW src/worker/render.rs:279-284`, `src/worker/subflow.rs:774-779`).

`EW src/spec/schema/property-treatment.json` marks both "honoured" (`Flow.outputs` at `:17`, `Loop.retry` in the `Loop` row at `:557-575`). That file measures the **save** path only, so "honoured" there does not mean "run". `EW src/spec/keys.rs:167` warns only on `Loop` `outputs`/`fetchType`.

## 2. What Kestra's `Loop` `outputs:` does

Sources: `KL:242-261`, `:323-346`, `:349-389`; `KX DefaultExecutor.java:762-791`; `KX handler/LoopExecutionEventMessageHandler.java:94-160`, `:214-240`; `KX ExecutorService.java:441-477`; `kestra/core/.../runners/InputAndOutputImpl.java:39-61`; `FlowInputOutput.java:491-518`.

1. **When the outputs are evaluated.** They are evaluated **once per iteration**, when that iteration's LOOP sub-execution terminates, not once at the end of the `Loop`. `DefaultExecutor`'s terminal block calls `loop.computeIterationOutput(runContext, execution)` with the sub-execution's own `RunContext`. This happens after the iteration's `errors:`/`finally:`, because those run inside the sub-execution. It happens only if `outputs` is non-empty. The result travels on the `LoopExecutionEvent`.
2. **The render context** is the sub-execution's: its own task outputs (`outputs.createOutput.value`), `item`, and the parent outputs the sub-execution inherits.
3. **How the outputs are rendered** (`renderOutputs`). Required outputs (`required` absent or `true`) are rendered together, and one failure fails them all. Each optional output is rendered alone, and a failure is logged and becomes `null`. Then `typedOutputs` coerces each value by its `type` (`parseData`, the same coercion as flow outputs and inputs). A coercion error throws.
4. **When a render or coercion fails,** the sub-execution's state becomes `FAILED` **after** the terminal it reached, and that state is persisted. The golden shows it: `loop-outputs-failed-render`'s only sub-execution reads `CREATED RUNNING SUCCESS FAILED`. The `Loop` then sees a failed iteration. With `transmitFailed: true` (the default) the `Loop` ends FAILED and no further iteration starts (golden: 1 sub-execution, `loop` FAILED).
5. **How the results accumulate on the `Loop` run** (`handleTerminated`). When an iteration fails and `transmitFailed` is set, the `Loop` terminates at once and **does not append** that iteration's outputs. Otherwise, if the iteration's outputs map is non-empty, it appends `{item: {value, iteration, key?}, outputs: {…}}` to the `Loop` run's `outputs` list. The list is in **termination order**, not index order (with `concurrencyLimit: 1` the two are the same). `item.value` is the iteration's string value, `iteration` the 0-based index, and `key` is set only for map values.
6. **What the `Loop` run's outputs always hold:** `iterationCount`, `runningIterations` and `terminatedIterations` (a map from state to count), plus `nextOffset` in ION-file mode. `outputs` is present only once something was appended. Today egret writes **none** of these for a `Loop` (only `LoopUntil` gets `iterationCount`, `EW src/worker/render.rs:745-779`), so `{{ outputs.loop.outputs }}` is undefined in egret and FAILS under the strict renderer.
7. **`fetchType`** (default `AUTO`):
   - `FETCH` keeps the map.
   - `STORE` writes the typed map as **one ION file per iteration**, in the sub-execution's internal storage (`runContext.storage().putFile`). It replaces the map by `{uri: "kestra://…"}`, so the list becomes `[{item, outputs: {uri}}]`.
   - `AUTO` is `STORE` when `values` is a **String** that renders to a URI `URIFetcher` supports (`guessOutputMode`), and `FETCH` otherwise.
   - So `loop-outputs-auto` (`values: "{{ outputs.write.uri }}"`) is STORE, and `loop-outputs` is FETCH.
8. **Flow-level `outputs:`** (`onEnd`, `ExecutorService.java:441-477`) use the same `renderOutputs` + `typedOutputs` on the execution, evaluated when the execution ends. A failure appends `FAILED` after the computed final state. The golden shows `flow-with-outputs-failed` as `CREATED RUNNING SUCCESS FAILED`. **Inside a LOOP sub-execution, `flow` is the PARENT flow** (`onEnd` uses `executor.getFlow()`), so as far as the source shows, the parent flow's `outputs:` are evaluated in every iteration too **(unverified on the artifact; no corpus flow has both)**.

## 3. What egret does today

- **Model.** `models::Loop` (`EW src/models/task.rs:534-559`) carries `tasks`, `values`, `concurrencyLimit`, `transmitFailed`, `errors` and `finally`. `outputs`/`fetchType` are not modelled, and the comment at `:518-521` ("deliberately out … unused before W3") is stale.
- **Save.** `outputs` and `fetchType` on a `Loop` produce a `not_honoured` warning naming package #6 (`EW src/spec/keys.rs:167`, test `:1769`). There is no warning for `Loop` `retry:` or for the flow-level `outputs:`.
- **Run.**
  - An iteration's flow is `sub_flow_source` (`spawn.rs:830`), and nothing evaluates outputs at its end.
  - `on_execution_terminal` (`spawn.rs:519-657`) receives only the child's terminal state.
  - The `Loop` run's `task_runs.outputs` stays NULL, so `render::Surface` gives `outputs.<loop>` as `{}` (`render.rs:784-790`).
  - Flow-level `outputs:` are validated (type required, `EW src/spec/validate.rs:1088-1096`) and never evaluated. `execution.outputs` is a constant `{}` (`render.rs:279-284`), and so is a `Subflow`'s `outputs.<sub>.outputs` (`subflow.rs:774-779`). `workflow.executions` has no `outputs` column.
- **Machinery #6 can reuse.**
  - Internal storage and ION files (W9a; `loop-values-from-uri` matches, so `values:` from a URI works).
  - The ION writer `runners::ionfmt::write_ion_element`.
  - The input coercer from #3 (`inputs_check`).
  - The sub-execution's render surface, which is already loaded in `claim::settle`'s outbox phase.
  - The parent row lock that `on_execution_terminal` already takes (`claim::load(… Stack::Nested)`).
- **Stale text.** `render.rs:8-11` still says the surface gap is "the reason `loop-outputs-auto` cannot resolve its `values:`". W9a Task 11 closed that.

## 4. Corpus, banks, size and risks

**Golden entries this package can move** (counted from #5b's 35):

| entry | what holds it today | needs | confidence |
|---|---|---|---|
| `valids/loop-outputs` | `end` renders `{{outputs.loop.outputs}}`, which is undefined, so `end` FAILED (golden SUCCESS) **(unverified field)** | FETCH | high |
| `valids/loop-outputs-failed-render` | measured: egret runs 3 sub-executions and the loop SUCCEEDS; Kestra runs 1 (`SUCCESS FAILED`) and the loop FAILS (`a7-exec5.txt:108`) | a failed render fails the iteration | high |
| `valids/if-in-parallel` | measured: `assert` FAILS on `outputs.for_each.outputs[i]…` (`OE baseline.txt:591`) | FETCH and `???` (`???` exists, `env.rs:967`) | high |
| `valids/loop-outputs-store` | as `loop-outputs` | STORE | high |
| `valids/loop-outputs-auto` | as `loop-outputs`; its `values:` comes from a `Write` with no `extension:` (egret names that file `content.tmp`, `handlers/storage.rs:28`) | AUTO, which resolves to STORE | medium: T0 must show nothing else holds it |
| `valids/retry-flowable-parallel` | §1 | `Loop` `retry:` inherited by the iteration | medium |
| `valids/after-execution` | flow `outputs:` (note 6, `OE baseline.txt:580`) | flow outputs (#6b) | high |
| `valids/flow-with-outputs-failed` | the execution ends SUCCESS; the golden has `SUCCESS FAILED` | flow outputs (#6b) | high |
| `valids/subflow-secret-output-child`, `-parent` | Kestra FAILED because the harness has no encryption key (roadmap `:296`) | not bankable unless egret refuses SECRET outputs (Q3) | stays |

`working-directory-loop` nests three `Loop` `outputs:` but has no golden (it uses a test-only task), so it is a unit-test fixture, not a bank.

**Banks.** #6a (Loop outputs and retry): **6** upper bound, 4 strict (35 → 29). #6b (flow outputs): **2** (29 → 27). That is **8** in all, against the row's "3 (+ up to 4)". Only the two SECRET entries stay.

**Size.**

- #6a is M:
  - model the two keys, plus the save-time `type` check that flow outputs already have;
  - an outputs evaluator shared with #6b (render required/optional, then type);
  - evaluation at the iteration's terminal, with the `SUCCESS → FAILED` rewrite;
  - append on the parent `Loop` run under its lock;
  - the three counters;
  - STORE/AUTO through storage;
  - the retry inheritance (S).
- #6b is M:
  - a migration for `executions.outputs` (a number above main **and** above the open PRs);
  - evaluation at `terminated`, before `afterExecution:` sees `execution.outputs`;
  - `execution.outputs` in render;
  - a `Subflow`'s `outputs.<sub>.outputs`;
  - a Flow trigger's `trigger.outputs`;
  - the execution API;
  - SECRET typing.

**Risks.**

- R1. **Outputs content is not measured by any oracle.** Every bank is a state consequence. Item shape, order, typing and STORE URIs need unit tests written against §2. A probe flow on the pinned image is optional; it needs the harness containers, not the network.
- R2. **Unbounded growth.** The `Loop` run's list grows with the number of iterations times the output size, all in one JSONB row (Q5).
- R3. **The retry fix and the terminal report.** A sub-execution's intermediate `FAILED` must not end the `Loop` (§1, timing). A test must pin "FAILED then RETRYING in one settle does not report".
- R4. **Flow outputs inside an iteration.** Once #6b exists, `sub_flow_source` keeps `flow.outputs`, so iterations would evaluate the parent's outputs as Kestra's source does (§2 item 8). This must be a decision, not an accident (D6).
- R5. **Concurrent iterations append in egret's termination order.** For `concurrencyLimit > 1` Kestra's order is also non-deterministic, so no oracle can pin it.

## 5. Open questions for the user

- **Q1 (scope).** Should flow-level `outputs:` (roadmap note 6) be part of #6, as a second PR #6b? *Proposed: yes.* It shares the evaluator and banks 2. It also lifts a silent drop that #1 missed.
- **Q2 (scope).** Should `retry-flowable-parallel` (a `Loop` `retry:` inherited by the iteration's tasks) be part of #6a? *Proposed: yes.* It is the one unattributed entry #5 does not take, and it is S.
- **Q3 (security).** How should SECRET-typed outputs (flow and `Loop`) be handled? *Proposed:* encrypt them with egret's secret key, mask them in the API, logs and render, and keep `subflow-secret-output-child` and `-parent` baselined as a harness difference. Kestra fails them only because its harness has no key. The alternative is to refuse SECRET outputs until #10.
- **Q4 (security).** A FILE-typed output, or a STORE URI read back, could name storage outside the execution's own namespace and flow. *Proposed:* refuse such a URI with egret's storage-scope rule, and list it as a §3.1 security divergence. Kestra OSS checks nothing.
- **Q5 (resource).** Should the accumulated `Loop` `outputs` list be capped? *Proposed:* cap each iteration's map at `MAX_OUTPUT_BYTES` (1 MiB, `env.rs:204`) and the list at 16 MiB. Beyond that the iteration FAILS with a message pointing to `fetchType: STORE`. Kestra has no cap.

## 6. Proposed defaults (no decision needed)

- **D1. Evaluation site.** Evaluate at each iteration's terminal, after its `errors:`/`finally:`, on the sub-execution's render surface, inside the same settle. A failure appends `FAILED` to the sub-execution's history and reports `FAILED` to the `Loop`.
- **D2. The list.**
  - Entries are appended in termination order.
  - An entry is appended only if the map is non-empty.
  - A failed iteration under `transmitFailed` is not appended.
  - The entry shape is `{item: {value, iteration, key?}, outputs}`.
- **D3. Rendering.** `required`/optional are rendered as `renderOutputs` does, and typing reuses #3's coercer.
- **D4. Counters.** Write `iterationCount`, `runningIterations` and `terminatedIterations` on the `Loop` run. Do not expose `nextOffset`, because egret reads file values by index.
- **D5. STORE and AUTO.** STORE writes one ION file per iteration under the **sub-execution's** storage prefix (as Kestra's `runContext.storage()` does). AUTO means STORE when `values` is a string that renders to a `kestra://` URI.
- **D6. Flow outputs in an iteration.** Clear `flow.outputs` in `sub_flow_source`, and treat Kestra's evaluation of the parent's outputs in every iteration as a bug (roadmap #20 rule; WD `finally` precedent).
- **D7. Retry inheritance.** The nearest retry on the `Loop` or one of its ancestors in the parent flow becomes the iteration's inherited policy. If there is none, the flow's retry applies, as Kestra's precedence does. A `CREATE_NEW_EXECUTION` reached this way stays refused by `worker::replay`, because Kestra throws there too (`OE baseline.txt:352-356`).
- **D8. Warnings.**
  - Drop the `not_honoured` warning for `Loop` `outputs`/`fetchType` when #6a lands.
  - Add one for the flow-level `outputs:` (naming #6) until #6b lands.
  - Add one for `Loop` `retry:` if Q2 is "no".
- **D9. Bookkeeping.** Update `OE baseline.txt` with comments for each bank, `sixteen_banked`'s triple, the construct table, the roadmap row and the cause list (`:277-301`, `:334-340`), and the docs (EN + VI).

## 7. Roadmap premises that are wrong or stale

1. **"Evaluated at its end."** This is wrong. The outputs are evaluated **per iteration**, at each sub-execution's end, in that sub-execution's context, and collected into a list on the `Loop` run (§2).
2. **"4 unattributed `Loop` entries."** This is stale. Three are renderer causes owned by #5: `if-in-flowable` (#5a, banked) and `loop-object` ×2 (#5b, banked on PR #780). The fourth is `Loop` `retry:` inheritance, not outputs. The cause table's guesses ("item/jq rendering or Loop-level retry", `:338`) were half right.
3. **"Banks: 3 (+ up to 4)".** The row now expects 5 `Loop`-output entries (STORE/AUTO were reopened 2026-10-19), plus 1 retry, plus 2 flow outputs if Q1 is "yes". The "+ up to 4" leave through #5.
4. **"STORE needs W9a store"** (`:334`). This is stale. The W9a storage exists and Loop `values:` from a URI already matches.
5. **Package #1's "every parsed-but-ignored key".** Two keys were missed: flow-level `outputs:` and `Loop` `retry:`. `property-treatment.json`'s "honoured" measures saving, not running.
6. **The cause list's "flow `outputs:` … may be partly stale (inferred)"** (`:296`). This is not stale. Nothing evaluates flow outputs anywhere (§3).
7. **Code comments.** `render.rs:8-11` (`loop-outputs-auto`'s `values:`) and `models/task.rs:518-521` are stale.

## 8. Recommended scope and ordered task list

Scope: #6a (Loop `outputs:` FETCH/STORE/AUTO, the counters, and the retry inheritance), then #6b (flow-level `outputs:`). Both are built after #780 (#5b) merges.

- **T0.** Rebase onto main with #5b. Run the executor ratchet and confirm 301 / 35 / 35 / 0. Record the exact differing field for `loop-outputs`, `-store`, `-auto` and `retry-flowable-parallel`.
- **#6a**
  - **A1.** Model `Loop.outputs` (reuse `FlowOutput`) and `fetchType`. Add the save-time `type` check, drop the `keys.rs:167` warning, and update `property-treatment.json`.
  - **A2.** A shared outputs evaluator: render required, render each optional, then type. Unit tests against §2 items 3 and 4.
  - **A3.** Evaluation at an iteration's terminal (D1): the `SUCCESS → FAILED` rewrite, with the result carried to `on_execution_terminal`. Banks `loop-outputs-failed-render`.
  - **A4.** Append on the `Loop` run, plus the counters (D2, D4) and the Q5 cap. Banks `loop-outputs` and `if-in-parallel`.
  - **A5.** STORE/AUTO (D5). Banks `loop-outputs-store` and `-auto`.
  - **A6.** Retry inheritance (D7), plus the R3 test. Banks `retry-flowable-parallel`.
  - **A7.** Oracle bookkeeping, roadmap and docs (D9). Target 35 → 29.
- **#6b**
  - **B1.** Migration for `executions.outputs`, and evaluation at `terminated` with the `FAILED` rewrite. Banks `flow-with-outputs-failed`.
  - **B2.** `execution.outputs` in render. Banks `after-execution`.
  - **B3.** A `Subflow`'s `outputs.<sub>.outputs`, a Flow trigger's `trigger.outputs`, and the execution API.
  - **B4.** SECRET (Q3), FILE scope (Q4) and D6.
  - **B5.** Bookkeeping. Target 29 → 27.
