# Workflow package #2 "End hooks": `finally:` everywhere, `afterExecution:`, and the `errors:` the loops drop

**Status:** designed 2026-10-26. Not built. Roadmap row #2 of `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, with the additions of package #1's spec §9 item 3. Plan: `docs/en/plans/2026-10-26-workflow-end-hooks.md`. Branch `feat/workflow-end-hooks`, stacked on `feat/workflow-no-silent-drops` (draft PR #756). This spec proposes two PRs, #2a and #2b (D1, Q1).

**Where the claims come from.** Every claim about egret was checked in the worktree `egret-endhooks` at `68c9f4de`. egret paths are relative to `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`, pinned at `96893b589`. Kestra paths use these short forms:
- `ES` is `executor/src/main/java/io/kestra/executor/ExecutorService.java`.
- `DE` is `…/executor/DefaultExecutor.java`.
- `k/` is `core/src/main/java/io/kestra/`. Under it, `Execution.java` is `k/core/models/executions/Execution.java`, `FU` is `k/core/runners/FlowableUtils.java` and `ExS` is `k/core/services/ExecutionService.java`.

What the oracle recorded comes from `oracle-exec/golden.json`, normalised as `tests/executor.rs` compares it (`EX` below). Nothing was built or run for this spec. A claim that could not be checked statically says "to measure in Task N".

**Goal (user, roadmap §2).** egret is Kestra OSS's engine, except where egret diverges for security. Package #2 makes egret run every task list Kestra runs at the end of a block or of an execution:
- the flow's `finally:` and `afterExecution:`;
- `finally:` on every flowable;
- `errors:` on `Loop`, `LoopUntil` and `WorkingDirectory`.

It also re-checks the premise "a terminal execution has no live work" (roadmap Rule 2).

## 0. What Kestra does (the semantics this spec copies)

### 0.1 One rule chooses the list: `Execution.findTaskDependingFlowState`

`Execution.java:579-624` is the only place that chooses which list runs next. It serves every flowable and the flow itself (`ES:504-512` for the next task, `ES:1025-1050` for the end). It checks these cases in order:
1. **A `finally` run already exists**: keep running `finally` (`:589-593`).
2. **The `errors` list has not started and the caller's `terminalState` is `FAILED`**: start `errors`, or `finally` when there is no `errors` list (`:595-599`). Only `Pause` passes a `terminalState`.
3. **`errors` has started, or a task failed**:
   - If every failed task will be retried, return nothing. Neither `errors` nor `finally` starts (`:602-606`).
   - If `finally` exists and `errors` exists but is not terminated, return `errors` (`:608-609`).
   - If there is no `finally`, return `errors` (`:610-612`).
4. **`finally` exists and the tasks are all terminated, or one has failed with no retry left**: return `finally` (`:615-620`).
5. **Otherwise**: return `tasks`.

What follows from this rule:
- The order is `tasks`, then `errors` (on failure only), then `finally`. `finally` runs on success and on failure.
- `errors` and `finally` always run **one at a time**, even under a `Parallel` or a `Dag` (`FU:350-358`: "errors & finally must be run as sequential tasks").
- A `finally` child that fails does not stop the next `finally` child. `innerResolveSequentialNexts` moves to `lastIndex + 1` whatever the last child's state (`FU:79-109`). egret's `sequential::walk` already follows this reading (doc `executor/flowable/sequential.rs:68-84`, fn `:85`).

### 0.2 The state of a block, or of the flow, comes from `tasks` alone

- **A flowable.** `FU.resolveState` waits until the current list has terminated (`finally`, when one exists). It then returns `guessFinalState(tasks, …)` over the main `tasks` alone (`FU:225-246`; `Execution.java:769-801`). A `finally` (or an `errors`) child that fails does not change the block's state. A block whose tasks failed stays `FAILED` even when its `finally` succeeds.
- **The flow.** `onEnd` uses `guessFinalState(flow)` over `flow.getTasks()` (`ES:455`; `Execution.java:760-761`).
- **The block's own end.** The block terminates only after its `finally`. The golden agrees: in `valids/finally-sequential`, every `finally` run is `overlapping` with its `Sequential` (§7.1).
- **Two cases skip `finally`:**
  - an `If` whose branch is empty ends at once, from its own state (`k/plugin/core/flow/If.java:188-198`);
  - a `Sequential`-style block whose children are all disabled is `SUCCESS` at once (`FU:163-170`).

### 0.3 The flow's `afterExecution:` runs after the execution is terminal

- **When it starts.** `handleAfterExecution` (`ES:1005-1023`) does nothing until `execution.getState().isTerminated()`. It then creates the list's runs one at a time, each `withForceExecution(true)`: "so it would be executed even if the execution is killed" (`ES:1015`). It is called after `handleNext` and also while the execution is `KILLING` ("but keep listeners on killing", `ES:192-193`).
- **The execution's state does not change.** The execution keeps its terminal state (`SUCCESS`, `FAILED`, `KILLED`, …), and `endDate` is already set (`k/core/models/flows/State.java:141-150`). `handleEnd` returns at once on a terminal execution (`ES:1026`). So a failed `afterExecution` task changes nothing. Its runs are ordinary entries in the same `taskRunList` (`ES:1018`).
- **"Terminated" is a separate predicate.** `ExS.isTerminated(flow, execution)` (`ExS:1325-1338`) means: terminal, and every `afterExecution` run terminated. A `LOOP` iteration is excluded from the second half (`:1331`). `process` stops at that predicate (`ES:171`).
- **What fires before `afterExecution`.** Flow triggers fire on every state change of a cycle (`DE:722-726`). The terminal state is entered before `afterExecution` runs, so the flow triggers have already fired.
- **What waits until `afterExecution` has terminated.** All of these wait for "terminated" (`DE:714,729-817`):
  - the concurrency slot is released ("the cycle completing them is the one that really terminates the execution", `DE:737-742`);
  - the subflow's end is reported to its parent (`DE:752-759`);
  - the loop event is sent (`DE:762-791`);
  - the SLA monitor is purged (`DE:793-796`);
  - the trigger is reset (`DE:798-801`);
  - the `TERMINATED` event is sent (`DE:803-811`).
- **Flow outputs** are computed in `onEnd` (`ES:461-479`), before `afterExecution`.
- **Kill.** Kestra answers 409 to a kill on a terminated execution (`webserver/…/ExecutionController.java:1757`). A running `afterExecution` therefore cannot be killed. Its worker skips the kill check for a force-executed run (`worker/…/WorkerTaskProcessor.java:201-204`).

### 0.4 Kill, `Exit`, retry, restart

- **Kill.** Nothing new is created while the execution is `KILLING` (`FU:83`, `FU:337-339`; `ES:185-190`). Flowable `finally:` and flow `finally:` therefore do not run on a kill. `afterExecution:` runs once the execution is `KILLED` (§0.3).
- **`Exit`.** An `Exit` with a state other than `KILLED` sets the execution terminal directly (`k/plugin/core/execution/Exit.java:81-109`). The flow's `errors`/`finally` do not start after it. `afterExecution` still runs (`ES:1005-1018`).
- **Retry.** A failed task that will be retried starts neither `errors` nor `finally` (`Execution.java:603-605`). A flowable's own retry waits for its `errors`/`finally` (`ES:587-590`, `isErrorOrFinallyHandlingPending` `ES:860-905`). egret retries no flowable today (`executor/retry.rs:48-56`).
- **Restart** (`ExS:339-351`, `:535-552`) removes the `errors`/`finally`/`afterExecution` runs before it re-runs. egret has no restart: a replay is a new execution (§9).

### 0.5 The loops

- **`Loop`.** Each iteration is a `LOOP` sub-execution. Its `handleNext`/`handleEnd` run the `Loop`'s `tasks`, `errors` and `finally` as that sub-execution's three lists (`ES:513-526`, `ES:1037-1043`). The iteration's state comes from the `Loop`'s `tasks` alone (`ES:446-450`). The `Loop` in the parent does nothing with `errors`/`finally` (`k/plugin/core/flow/Loop.java:282-286,303-309`).
- **`LoopUntil`.** `resolveWaitForNext` runs `tasks`, then `errors`, then `finally`, all in each iteration, through the same rule (`k/plugin/core/flow/LoopUntil.java:107-116`; `FU:113-151`). `resolveState` asks the condition only after that (`LoopUntil.java:177-201`).
- **`WorkingDirectory`** extends `Sequential` (`WorkingDirectory.java:207`). The worker runs `tasks`. The executor resolves next tasks only when a child failed (`WorkingDirectory.java:234-243`), and then runs `errors`, then `finally`, as ordinary tasks outside the worker's directory. On success it resolves nothing, but `resolveState` still waits for `finally` (`Execution.java:615-620`). **Read from the code: a `WorkingDirectory` that succeeds with a `finally:` never starts it, and the block never ends.** This was not run, and no Kestra test covers it (Q4).

### 0.6 What the golden pins and what it does not

The golden pins: the order `tasks` → `errors` → `finally` → `afterExecution`, at both levels; that each list is sequential; that a block ends after its `finally`; that `afterExecution` runs are ordinary entries with parent `None`, created after the execution's last state; and that the execution's history never re-enters `RUNNING` for them. This spans 23 entries (§7.1).

The golden does not pin:
- a `finally` or `afterExecution` task that fails (none in the corpus fails);
- `afterExecution` after a kill (`flow-concurrency-queue-killed-after-execution` ran to `SUCCESS`, nothing was killed);
- `WorkingDirectory` `errors`/`finally` (`valids/working-directory` was refused by Kestra with a 422);
- `Pause` `finally` without `errors`;
- `retry:` inside these lists.

Each of these is decided from Kestra's source above, or named as egret's own choice.

## Decisions

All decisions are this spec's defaults, dated 2026-10-26, under the rule "follow Kestra unless egret has a reason not to, and keep safety tight". D1, D7, D9 and D15 carry a question for the controller (§12).

- **D1. Two PRs: #2a then #2b, from one spec and one plan.**
  - **#2a, "finally and errors inside a live execution":**
    - `finally:` on `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil` and `WorkingDirectory`, plus the flow's `finally:`;
    - `errors:` on `Loop`, `LoopUntil` and `WorkingDirectory`;
    - the index-stable plan (D3);
    - the walker (§8);
    - the graph (§10).

    Everything in #2a happens before the execution is terminal, so no worker premise moves.
  - **#2b, "afterExecution: work after the end":**
    - the post-terminal phase (D8–D13);
    - Rule 2's audit and its fix (§6);
    - the frontend's "finished" (§10).

  The roadmap sizes #2 as M. Measured, it is two Ms: #2a touches the model, plan, executor and walker; #2b touches the worker's settle, three sweeps, a migration and the frontend. #2b depends on #2a for the flow `finally:` (`after-execution-finally`) and for the plan layout (D3). The PR boundary falls after plan Task A8 (Q1).
- **D2. One list-choosing function, Kestra's.** `flowable::current_list` (`executor/flowable/mod.rs:199-233`) is egret's half of `findTaskDependingFlowState`. It already chooses `Tasks`, `Errors`, `RetryPending` or `Ended`. It gains a `finally: &[TaskIx]` argument and a `Current::Finally(list)` answer, and its cases follow §0.1's order exactly. Every caller passes its block's `finally`: `sequential`, `parallel`, `dag`, `branch`, `allow_failure`, `working_directory`, `loop_until`, and the flow's own root call (`executor/mod.rs:225`). No flowable gets a `finally` rule of its own. A `Current::Finally` list is walked by `sequential::walk` whatever the block's scheduling (§0.1). `Done` still carries `aggregate(state, tasks, parent)` over the main `tasks` (`flowable/mod.rs:244-282`), which is Kestra's `guessFinalState` (§0.2).
- **D3. The plan stays index-stable for executions that already exist.** This replaces package #1's §10 instruction (see "False premise 1" below).
  - **Why.** egret stores a run's task as a positional index. `task_runs.task_ix` is written at `worker/claim.rs:538` and read back as `TaskRun::task` at `:4393`. The plan is rebuilt from `executions.source` at every claim (`plan_stored`, `claim.rs:4349`; `executor/plan.rs:1149`). `flatten_one` is pre-order (`plan.rs:1255-1258`). If #2 flattened a `finally:` in place, every task after it would move down, and an execution that is running when #2 is deployed would read its stored runs against the wrong tasks: another task's `type`, spec and secrets.
  - **The rule.**
    - `flatten_flow` first produces exactly today's layout. That includes `AllowFailure`'s `finally`, which is already inline.
    - The lists #2 adds are deferred to a queue. They are flattened after the flow's `errors`, in model pre-order: flowable `finally`, then `LoopUntil`/`WorkingDirectory` `errors`. Then come the flow's `finally` and the flow's `afterExecution` (#2b), each followed by a drain of the queue.
    - A child's index stays greater than its parent's (the invariant `plan.rs:1255-1256` states).
  - **The test.** A committed fixture of today's layout, taken before any code changes (plan Task A1), is asserted to be a prefix of the new plan for every corpus flow.
- **D4. Executions that are running when #2 is deployed start running their end hooks at their next round.** They are re-planned from the same source, and the source has always held these lists. No column records "the rules it started under" (that would need a migration and would keep skipping the author's cleanup). A block that is already terminal does not go back: its `finally` never runs. Rolling deploys must not mix binaries. An old binary that loads a run created by a new one for an appended index reads `plan.tasks.get(ix) == None` and refuses it as corrupt. The release note says to stop the old workers first. (Q2)
- **D5. `Loop`'s `errors:` and `finally:` run inside each iteration**, as Kestra runs them (§0.5).
  - `spawn::sub_flow_source` (`worker/spawn.rs:823-883`, the clears at `:836-839`) today clears the parent's flow-level `errors`, `finally` and `afterExecution`. It now sets the iteration's `errors` to the `Loop`'s `errors` and its `finally` to the `Loop`'s `finally`. `afterExecution` stays cleared, as Kestra's `isTerminated` excludes `LOOP` (`ExS:1331`).
  - The parent's plan does not flatten a `Loop`'s `errors`/`finally`: they create no run in the parent (`Loop.java:282-286`).
  - `spawn::find_task` (`spawn.rs:891-902`) also walks `after_execution`, so a `Loop` inside `afterExecution:` is found (#2b).
- **D6. `LoopUntil`'s `errors:` and `finally:` run in each iteration, before the condition is asked.** "The body is done" changes from "`tasks` exhausted" to "`current_list` has walked `tasks`, `errors` (on failure) and `finally`". The condition, the bound and the failure rules of `loop_until::drive` (`executor/flowable/loop_until.rs:224-279`) are unchanged. `Command::RestartLoop` already discards every descendant run, and `Plan::children_of` gains `finally`, so an iteration's `errors`/`finally` runs are discarded with its body's.
- **D7. `WorkingDirectory`'s `errors:` and `finally:` run after the block's `FinishWorkingDirectory`, outside the directory, as ordinary runs.** `errors` run on failure. `finally` runs on success too. Kestra's code never starts a `finally` after a successful `WorkingDirectory` (§0.5). This spec reads that as a Kestra bug (an execution that never ends). The roadmap's #20 rule allows egret to differ ("unless the spec shows Kestra's behaviour is a bug"). (Q4)
- **D8. `afterExecution:` runs in a post-terminal phase, and the execution's state, history and `ended_at` do not change (#2b).** This is Kestra's model, and the golden pins it: `after-execution-error` reads `CREATED RUNNING FAILED` with its `afterExecution` run created after `FAILED`. `advance` gains the phase. A terminal execution, which today gets `Vec::new()` (`executor/mod.rs:179-189`), now gets `after_execution::drive(plan, state, now)`. That function walks `plan.after_execution` with `sequential::walk` (parent `None`). It never emits `SetExecutionState`, `ScheduleRetry` or `PauseRun`. `KILLING` keeps `advance_killing`. So `afterExecution` starts once the execution is `KILLED`, never while it is `KILLING`, as in Kestra.
- **D9. Kill during `afterExecution` stays a 409, as in Kestra.** Today egret's kill route returns `Terminated` (409) on a terminal execution (`worker/kill.rs:186`). Kestra does the same (§0.3). An `afterExecution` task is therefore bounded only by its own `timeout:`. A safety-minded divergence (letting the kill route stop live after-execution runs and leave the execution's state alone) is offered as Q3. Default: follow Kestra.
- **D10. "Terminated" is a pure predicate, and the parent hears about the child then.** The new function is `executor::terminated(plan, state) -> bool`: the execution is terminal, and `after_execution::drive` has nothing left to do (Kestra's `ExS:1325-1338`).
  - `worker::claim`'s tail reports a terminal to a `Subflow` parent or a `Loop` parent only when `terminated` first becomes true. Today it reports when the history first becomes terminal (`claim.rs:3487-3497`, `notifies_parent` `:2930`).
  - Flow-trigger outbox events stay per state change (`claim.rs:3608-3672`), so they fire before `afterExecution`, as Kestra's do (§0.3).
  - A `Loop` iteration has no `afterExecution` (D5), so for it "terminated" is "terminal".
- **D11. In the post-terminal phase a task is not retried, and the author is told at save.** Kestra's retry of an `afterExecution` task is not measured, and egret's retry machinery reopens the execution (`retry::scan` emits `SetExecutionState { Retrying }`, `executor/retry.rs:348-361`). The due-retry query also skips runs of a terminal execution (`claim.rs:799-807`). `after_execution::drive` never calls `retry::scan`: a failed run stays `FAILED`, and the walk moves to the next. At save, a `retry:` on a task in `afterExecution:`, or a flow-level `retry:` when `afterExecution:` exists, gets the warning `not_honoured` naming #13 (roadmap note §11).
- **D12. Two kinds are refused inside `afterExecution:`, at save and at start, naming their package.**
  - **`Pause`**, code `unsupported_after_execution_task`, #9. A pause would mark an execution that has ended as `PAUSED` (`pause_mark`, `executor/mod.rs:269`).
  - **`Exit`**, same code, #20. `exit::drive` sets the execution's state (`executor/exit.rs:212-219`).

  Both are refused by `validate_for_run`, so `build_plan` refuses to start such a flow. Corpus: 0 flows (§7.2).

  Every other kind runs. That includes the `If` that `after-execution-flowable` pins and the executor-level updates. The update pass's gate `drives_forward` (`worker/render.rs:1504`) widens to "drives forward, or is in the post-terminal phase". For `Subflow`, `Loop`, `LoopUntil` and `WorkingDirectory` inside `afterExecution:`, plan Task B4 builds one DB test each. If one stalls, the task adds that kind to this refusal and reports it; it does not patch around it.
- **D13. Rule 2 (§6): every SQL rule that read "terminal execution" as "no live work" reads one new predicate, `workflow.execution_is_idle(id)`.** It is true when the execution is terminal and no task run of it is live. It is defined once, in migration **0210** (the first free number above `0209_username_ledger.sql`; re-check main and open PRs before merge). Its readers are the orphan sweep's superseded rule, the `_wd` crash sweep and the storage pruner.
- **D14. Pause `finally:` moves to #9, with `Pause` `errors:`.** Both need the same new mechanism: a resumed `Pause` must re-enter `RUNNING` to run a sub-list. Kestra sets it `RUNNING` on resume when it has `errors` or `finally` (`ExS:630-640`), and the golden shows `CREATED RUNNING PAUSED RUNNING FAILED` (`pause-errors-finally-after-execution`). egret's resume emits the terminal directly (`flowable/mod.rs` `resume_paused`, `Resume::Terminal`), and migration 0142's CHECK ties `PAUSED` to `resume_with`.
  - The model gains `Pause.finally` so that `executor::pause_gap` (`plan.rs:862-919`) can refuse it: code `unsupported_pause_property`, message ending "Roadmap package #9 lifts it". The code is kept.
  - Today a Pause `finally:` is dropped silently. From #2a it is refused at publish and at start. Corpus: 1 flow, already refused for its `errors:` first, so the census does not move.
  - This narrows the user-approved row #2 (Q5).
- **D15. The frontend's "finished" becomes "terminal, and no task run is live" (#2b).** Today the execution pages stop polling at a terminal state (`frontend/src/components/workflows/executions/polling.ts:43-45`, and the topology and Gantt tabs). An `afterExecution` run would then appear only after a manual reload. The detail DTO already carries `runs` (`routes.rs:1793-1817`), so no API change is needed. This is egret's version of Kestra's follow, which ends on the `TERMINATED` event (§0.3).
- **D16. Security** (§11). No new surface. No new endpoint, no new render site, no new permission.

**False premises found while verifying** (each corrected by a dated note in its document, plan Task A8):
1. **Spec #1 D8 and §10, and roadmap §2 note 3**, say "a version published before #2 holds flowable `finally:` only in `snapshot.source`… #2 must re-derive the plan from `snapshot.source`". No run reads the snapshot:
   - a start runs the **draft row's** `flows.source` (`routes.rs:2621-2625`, `&flow.source`; the DTO doc `routes.rs:352`: "executions run the saved draft");
   - it copies that source into `executions.source`;
   - every claim re-plans from that copy (`claim.rs:4344-4349`);
   - the snapshot's `"plan"` (`publish.rs:347-351`) is the re-serialised `models::Flow`, and only tests read it (`tests/publish.rs:288,958,1084`).

   So nothing needs re-deriving: once the model has the field, every execution sees it. The real hazard is the positional `task_ix`, and D3 answers it.
2. **Roadmap Rule 2** attributes the premise to "debt spec D15". D15 is the `LoopUntil` superseded-container rule. The premise is written in `sweep_decision`'s doc (`worker/commands.rs:2669-2671`). And the premise was false before #2: an `Exit` with `SUCCESS`, `WARNING`, `FAILED` or `CANCELLED` leaves its siblings running in a terminal execution (`executor/exit.rs:66-73`). §6 shows why that did no harm, and why `afterExecution` changes the answer for the `_wd` sweep and the pruner.
3. **Roadmap row #2 sizes it "M", "~20 upper, 14–15 strict".** Measured, it is two Ms (D1), with 23 upper and 21 expected (§7.1).
4. **Roadmap Appendix A** groups `restart-with-finally` and `restart-with-after-execution` as restart flows. The harness cannot restart (`oracle-exec/driver.rs:1160-1241` starts once and ticks). Kestra's golden for both is the first execution only, with no restart recorded. Both are ordinary `finally`/`afterExecution` entries (§7.1).

## 1. What #2 covers

| construct | today | #2 | PR |
|---|---|---|---|
| flow `finally:` | parsed (`models/flow.rs:81-82`), never planned (`plan.rs:1173-1183`) | planned (appended), run after `tasks`/`errors` while the execution is `RUNNING` | 2a |
| `finally:` on `Sequential`, `Parallel`, `If`, `Switch`, `Dag` | dropped by serde (`models/task.rs:309-409`), silent (`spec/keys.rs:561-569`) | modelled, planned (appended), run by `current_list` | 2a |
| `Loop` `errors:`/`finally:` | dropped | run in each iteration (D5) | 2a |
| `LoopUntil` `errors:`/`finally:` | dropped | run in each iteration (D6) | 2a |
| `WorkingDirectory` `errors:`/`finally:` | dropped | run after the block's finish (D7) | 2a |
| `AllowFailure` `finally:` | runs (`flowable/allow_failure.rs:23-48`) | unchanged, moved onto D2's rule | 2a |
| `Pause` `finally:` | dropped | refused, names #9 (D14) | 2a |
| flow `afterExecution:` | parsed (`models/flow.rs:91-92`), never planned | post-terminal phase (D8) | 2b |
| `Pause`/`Exit` inside `afterExecution:` | never run | refused (D12) | 2b |
| `retry:` reaching an `afterExecution:` task | never run | not applied; `not_honoured` #13 (D11) | 2b |
| "terminal means no live work" | premise of three SQL rules | `workflow.execution_is_idle` (D13) | 2b |

## 2. Model (`models/task.rs`, `models/flow.rs`)

- `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil`, `WorkingDirectory` and `Pause` each gain `#[serde(rename = "finally", default, skip_serializing_if = "Vec::is_empty")] pub finally: Vec<Task>`. This is the attribute `AllowFailure` uses (`task.rs:621-632`), plus the skip, so that a source re-serialised for a `Loop` iteration (`spawn.rs:878`) stays byte-close.
- `Loop`, `LoopUntil` and `WorkingDirectory` gain `errors: Vec<Task>`, written as `Sequential`'s is.
- `Task::children()` (`task.rs:122-160`) lists every kind's `finally` after its control-flow children, as `AllowFailure` already does (`:144`). `error_children()` (`:182-204`) gains `Loop`, `LoopUntil` and `WorkingDirectory`. The `_mut` twins (`:212`, `:235`) change the same way.
  - Every walk built on these functions then sees the new lists: `spec::validate` (`duplicate_task_id`, `validate.rs:725`), `spec::path::every_task`, `publish::resolve_all`, `graph/` and `spawn::find_task`. That is the point.
  - One consequence: a duplicate id between a flowable's `finally` and another task, accepted today because the list was dropped, becomes `duplicate_task_id`. Corpus: measured in Task A2; 0 is expected.

## 3. Plan (`executor/plan.rs`)

- `Plan` gains `pub finally: Vec<TaskIx>` (2a) and `pub after_execution: Vec<TaskIx>` (2b).
- `PlanTask` gains `pub finally: Vec<TaskIx>`. `TaskKind::AllowFailure { tasks, finally }` (`plan.rs:384-387`) becomes `AllowFailure { tasks }`, and its `finally` moves to `PlanTask::finally`, flattened inline where it is today (D3: the layout does not move).
- `Plan::children_of` / `direct_task_children` (`plan.rs:108-149`) return `errors`, the control-flow children, then `finally`, for every kind.
- `flatten_flow` (`plan.rs:1173-1183`) builds the D3 layout. The flow's `roots` and `errors` come first, unchanged. The deferred `(owner, list)` queue comes next: each entry is flattened with `flatten_list`, and nested new lists are deferred again. Then `flow.finally_`, a drain, `flow.after_execution` (2b), and a final drain. `Loop` defers nothing (D5). `Pause` plans nothing new (D14).
- `build_plan` and `plan_stored` keep their signatures. `validate_for_run` gains D12's refusal in #2b.

## 4. Executor (`advance` stays pure)

### 4.1 #2a: one rule, every caller

- `current_list(plan, state, tasks, errors, finally, parent, now) -> Current` follows §0.1 case by case:
  - `finally_started` → `Finally`;
  - (no `terminalState` argument: egret's `Pause` does not use it until #9);
  - (`errors_started` or `failed`):
    - all failures will retry → `RetryPending`;
    - `finally` and `errors` both exist and `errors` is not exhausted → `Errors`;
    - there is no `finally` → `Errors`, or `Ended` when `errors` is empty;
  - `finally` exists and (`tasks` exhausted or failed with no retry) → `Finally`;
  - otherwise → `Tasks`.

  "Exhausted" means every planned child has a terminal run (Kestra's `isTerminated(list)`). The existing `RetryPending` arm for the flow's own list (`sequential.rs:55`) is kept.
- `sequential::drive` takes `finally`, walks `Current::Finally(list)` with `walk`, and returns `Done(aggregate(tasks))` when it is exhausted.
- `parallel::drive` / `dag::drive` reach `Current::Finally` and walk it sequentially (§0.1). On the way into `Finally` from a failure, they apply `fail_the_rest` as they do on the way into `Errors` (`parallel.rs:33-66`). The `Errors` half is measured (`finally-parallel`: `p1..p3` stay `SUCCESS`, README:3036-3043). Where no `errors` list exists and siblings are still live, egret's existing `fail_the_rest` rule governs. Kestra lets them run until the parent ends and then forces them (`ES:793-806`); the corpus does not reach this case.
- `branch::drive_if` / `drive_switch` pass the chosen branch's `tasks` and the block's `errors`/`finally`. An `If` whose chosen branch is empty is `Done` at once, without `finally` (`If.java:188-198`). A list whose children are all disabled (planned empty) is `Done(Success)` without `finally` (`FU:163-170`).
- `allow_failure::drive` loses its private `finally` loop and calls `sequential::drive` with `finally`. Its `block_state` is unchanged. The three tests `allow_failure_drive_tests` (`executor/mod.rs:10033-10170`) pin that nothing moved.
- `working_directory::drive` (`working_directory.rs:27-49`): Begin → Prepared drives `tasks` (with `&[]` errors, as today) → Finish. Then, after the block's own claim is finished, `current_list` over (`tasks`, `errors`, `finally`) runs `errors`/`finally` as ordinary child runs (D7). The block's run stays `RUNNING` until then. **To measure in Task A5:** whether the block's run can stay `RUNNING` after `FinishWorkingDirectory` without the worker seeing a finished block as live (`worker/working_dir.rs`). If it cannot, the task stops and reports, and D7 moves to a follow-up with a refusal naming #17.
- `loop_until::drive`: the body step uses `sequential::drive(tasks, errors, finally)` (D6).
- The flow's root call (`executor/mod.rs:225`) passes `&plan.finally`.

### 4.2 #2b: the post-terminal phase

```rust
pub fn advance(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    if !drives_forward(state) {
        return match state.history.last() {
            Some(RunState::Killing) => kill::advance_killing(plan, state),
            Some(s) if s.is_terminal() => after_execution::drive(plan, state, now),
            _ => Vec::new(),
        };
    }
    // … unchanged …
}
```

- `executor/after_execution.rs` (new): `drive` returns the walk's commands for `plan.after_execution` (parent `None`). It filters out nothing, because `walk` emits only `CreateRun`, `StartRun`, `SkipRun`, `RecreateRun` and descent. A `debug_assert!` and a unit test pin that no `SetExecutionState`, `ScheduleRetry` or `PauseRun` comes out (D8, D11, D12).
- `pub fn terminated(plan, state) -> bool` (D10) is `last.is_terminal() && after_execution::drive(…)` at its fixed point: every after-run terminal and none left to create.
- `pub fn drives_after(plan, state) -> bool` is true when the execution is terminal and not `terminated`. It is the widened gate of `render_updates` (D12).
- A `RETRYING` round cannot start the phase. The round that writes `FAILED` then `RETRYING` (`retry.rs:329-365`) leaves `RETRYING` last, so the history is not terminal, and the phase begins one round after a terminal state is last.

## 5. Worker (#2b)

| site | today | #2b |
|---|---|---|
| claim of a leaf (`claim.rs:519-560`) | no execution-state filter | unchanged: an after-run is claimed like any run |
| `apply_outcome_on` (`claim.rs:2822-2838`) | fences on the run only | unchanged |
| settle tail, parent report (`claim.rs:3487-3497`) | when the history first becomes terminal | when `terminated` first becomes true (D10) |
| `render_updates` gate (`render.rs:1504`) | `drives_forward` | `drives_forward \|\| drives_after` |
| other render passes (`render_run_if`, the `If`/`Switch` render) | to measure in Task B3 whether any gates on a terminal execution | any gate found is widened the same way, with a test |
| due retries (`claim.rs:799-807`) | skip a terminal execution's runs | unchanged (D11: no retry after the end) |
| `persist` / `stamp_set!` (`stamps.rs:27-36`) | `ended_at` set on entering terminal | unchanged, and nothing re-enters terminal |
| outbox (`claim.rs:3608-3672`) | one event per appended execution state | unchanged: no state is appended in the phase |
| `force_close` (`worker/force_close.rs`, rule 6) | no plan, no `errors`/`finally` | unchanged: a force-closed execution runs no `afterExecution` either (it has no plan to run it from) |
| kill route (`worker/kill.rs:186`) | 409 on terminal | unchanged (D9) |
| kill cascade (`worker/kill.rs:358`) | skips terminal children | unchanged: as Kestra, a terminal child's `afterExecution` is not killed (`ExS:998-1000`) |
| oracle driver (`oracle-exec/driver.rs:1160-1241`) | ticks until no work | to measure in Task B6 that it keeps ticking through the phase |

## 6. Rule 2: what reads "terminal execution" as "no live work"

`grep -rn "state_is_terminal(e.state)\|state_is_terminal(state)" src` lists every SQL reader. Each one, judged:

| reader | what it does on a terminal execution | live work possible before #2? | after #2b | decision |
|---|---|---|---|---|
| `run_liveness`'s superseded query (`worker/commands.rs:2591-2597`), used by `sweep_decision` (`:2672`) and `pod_sweep_deletes` (`k8s/runner.rs:1592`) | treats a container whose run has no row as superseded: removes it at once (not kept) or after the retention (kept), measured from the execution's end | yes, a `LoopUntil` beside an `Exit`. Harmless: a superseded iteration's body ended before its restart (debt §4.7) | a `LoopUntil` in `afterExecution:` is the same case. But the retention is measured from `ended_at`, which is now before the after-work | read `execution_is_idle(e.id)` instead of `state_is_terminal(state)`; measure the end as `GREATEST(e.ended_at, max(task_runs.ended_at))`; rewrite the doc at `:2669-2671` |
| `_wd` crash sweep (`storage/wd_sweep.rs:163-169`) | deletes a block's `_wd/` snapshots 24 h after `COALESCE(t.ended_at, t.updated_at)` when the execution is terminal | yes, an `Exit` sibling block that has been live for 24 h | a `WorkingDirectory` in `afterExecution:` is live with a terminal execution | add `AND workflow.state_is_terminal(t.state)`: the block itself has ended. `KILLED` is terminal, so the force-closed case of debt §3.1 (`the_wd_sweep_removes_a_force_closed_blocks_snapshots`) is kept |
| storage pruner (`storage/prune.rs:10-30`, `:375-381`) | prunes a family once its root is terminal and `updated_at` is older than the retention; "an execution that has not ended is never pruned" | only if a live sibling outlives the retention | the same, for after-work | "has not ended" becomes "not idle" (`execution_is_idle`) |
| kill cascade (`worker/kill.rs:358`) | skips terminal children | – | – | unchanged (Kestra) |
| `due_retries` (`claim.rs:799-807`) | skips a terminal execution's retries | – | – | unchanged (D11) |

Migration 0210 (`migrations/0210_workflow_execution_is_idle.sql`):

```sql
-- A terminal execution can still have live work: an `Exit` sibling (since
-- kill-and-exit) and `afterExecution:` (since package #2b). "Idle" is what the
-- sweeps mean when they ask whether an execution is over.
CREATE OR REPLACE FUNCTION workflow.execution_is_idle(execution uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT workflow.state_is_terminal(e.state)
     AND NOT EXISTS (
       SELECT 1 FROM workflow.task_runs r
        WHERE r.execution_id = e.id
          AND NOT workflow.state_is_terminal(r.state))
    FROM workflow.executions e
   WHERE e.id = execution
$$;
```

"Idle" (no live run at all) is wider than "terminated" (D10: the after-list is done). The sweeps take the conservative one, so an `Exit` sibling also keeps its work.

## 7. Oracle: census and ratchet

### 7.1 Expected banks (to measure; each line leaves `baseline.txt` only when the ratchet says "banked")

The ratchet compares the whole trace: the execution history, the runs in creation order with parent, value, history and attempts, the golden's ordered pairs, and `spawned` (`EX:199-215`, `:2048-2063`, `:305-315`).

| entry | baseline.txt | golden (short) | needs | PR |
|---|---|---|---|---|
| `valids/finally-flow` | 583 | `FAILED`: if, ko, a1, a2 | flow `finally` | 2a |
| `valids/finally-flow-error` | 584 | if, ko, e1, e2, a1, a2 | flow `finally` | 2a |
| `valids/finally-flow-error-first` | 585 | ko, a1 (`ok` never created) | flow `finally` | 2a |
| `valids/finally-sequential` | 589 | seq `FAILED`; a1, a2 under seq, `overlapping` with it | `Sequential` `finally` | 2a |
| `valids/finally-sequential-error` | 590 | seq: if, ko, e1, e2, a1, a2 | `Sequential` `errors`+`finally` | 2a |
| `valids/finally-sequential-error-first` | 591 | seq, ko, a1 (flow level) | flow `finally` | 2a |
| `valids/finally-dag` | 582 | dag, t1, t2, t3, ko, e1, e2, a1, a2 | `Dag` `finally` | 2a |
| `valids/finally-parallel` | 588 | p1..p3 `SUCCESS`, if, ko, e1, e2, a1, a2 | `Parallel` `finally`; one dropped pair (`EX:2263`) | 2a |
| `valids/restart-with-finally` | 625 | hello, fail_randomly `FAILED`, end (the `finally`) | flow `finally`; `runIf` on `taskrun.attemptsCount` (README:4140-4150 notes a render cost that may be stale) | 2a |
| `valids/task-runif-taskswithstate` | 648 | `SetVariables` in flow `finally` `CREATED RUNNING SUCCESS`; Log `CREATED SKIPPED` | flow `finally`; `runIf` with `tasksWithState` (`runners/functions.rs:177`) | 2a |
| `valids/finally-loop` | 586 | each `FAILED`; every sub-execution: if, ko, e1, e2, a1, a2 at its top level | `Loop` `errors`+`finally` (D5) | 2a |
| `valids/finally-loop-parallel` | 587 | the same | the same | 2a |
| `invalids/loop-switch-failed` | 570 | sub-execution 2: switch, fail, errorforeach | `Loop` `errors` | 2a |
| `valids/after-execution-error` | 574 | `CREATED RUNNING FAILED`; fail, hello, end | `afterExecution` | 2b |
| `valids/after-execution-finally` | 576 | hello, finally, end | flow `finally` + `afterExecution` | 2b |
| `valids/after-execution-flowable` | 577 | mytask, after_if (`CREATED RUNNING SUCCESS`), after_if_log, after_end | `afterExecution` with an `If` | 2b |
| `valids/restart-with-after-execution` | 624 | as `restart-with-finally` | `afterExecution` | 2b |
| `valids/flow-concurrency-queue-after-execution` | 593 | sleep, afterExecution | `afterExecution` (QUEUE never fires with one execution) | 2b |
| `valids/flow-concurrency-queue-killed-after-execution` | 595 | `SUCCESS` (nothing was killed) | `afterExecution`; `EX:7527`'s "the only baselined id with `kill`" becomes an empty list | 2b |
| `valids/sleep-long` | 642 | sleep-long, output | `afterExecution` (`Sleep` capped at 0, `driver.rs:219`) | 2b |
| `valids/sleep-long-after-execution-flowable` | 643 | sleep-long, after-if, after-if-log, after-end | `afterExecution` with an `If` | 2b |

Expected: #2a banks 13, from 72 baselined to **59**, and `the_sixteen_banked_entries…`'s `(301, 229, 72)` (`EX:813`) becomes `(301, 242, 59)`. #2b banks 8, to **51**, `(301, 250, 51)`. Risky entries: `finally-parallel` (pairs), `restart-with-*` (the `runIf` render) and `task-runif-taskswithstate` (`SetVariables` inside `finally`). An entry that does not bank stays baselined with its cause rewritten. The plan does not force it.

Not banked by #2, with their package:
- `valids/after-execution`: its `afterExecution` renders `{{ execution.outputs.final }}`, and egret never evaluates flow `outputs:` (`worker/render.rs:279-284`). No row of the roadmap owns this today; plan Task B7 adds it to #6's scope as a dated note ("flow-level `outputs:`").
- `valids/pause-errors-finally-after-execution`: #9 (D14).
- `valids/all-flowable`: #5 (renderer, `EX:5086-5089`).

The upper bound is 23 (21 + `after-execution` + `pause-errors-…`), against the roadmap's "~20 upper, 14–15 strict".

### 7.2 Census and pinned constants

- `LIVE` 295, `VALIDATE_REFUSED` 25 and `UNKNOWN_TYPE_REFUSED` 21 are expected unchanged. No corpus flow has a `Pause`/`Exit` in `afterExecution:` (scan of all 351 files, 2026-10-26). The one corpus `Pause` with `finally:` is already refused for `errors:`. Measured in Tasks A2 and B5. Any move is a finding to report.
- Constants that move with the banks, each changed only in the task that moves it, with a dated comment naming the entries:
  - `the_residual_baseline_is_attributed…` rows `finally:` 14 (`EX:6999`) and `afterExecution:` 10 (`EX:7044`);
  - `MISMATCH_ON_RUNS` 55 (`EX:4402`), `MISMATCH_ON_ORDER` 27 (`EX:4426`), `MISMATCH_ON_EXECUTION` 32 (`EX:4377`);
  - `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` 4 (`EX:4826`);
  - the `RESTRICTION_REMOVES*` lists (`EX:3098-3150`);
  - the kill-name list (`EX:7527`);
  - the `handler_surface.py` guard and `handler-surface.txt` ceiling;
  - `oracle-exec/README.md`'s asserted counts.
- `spec/schema/property-treatment.json`, regenerated with `EGRET_WRITE_PROPERTY_TREATMENT=1`:
  - `finally` on `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil` and `WorkingDirectory` goes from `ignored` to `honoured`;
  - `errors` on `Loop`, `LoopUntil` and `WorkingDirectory` goes from `ignored` to `honoured`;
  - `Pause` `finally` goes from `ignored` to `refused`.

  No other row moves. `honoured` rows are served in the editor schema, which is intended: the editor starts offering them.

## 8. The walker (`spec/keys.rs`)

- `silent()` (`keys.rs:561-569`) loses `"finally" | "errors"` for flowables. They become model keys (`flowable_keys` reads `wire_fields_of`), so no finding is raised. `Pause` `finally` is reported by `pause_gap` alone (D14); the walker does not repeat it.
- `child_lists` (`keys.rs:507-522`) adds `finally` to every flowable kind and `errors` to `Loop`, `LoopUntil` and `WorkingDirectory`, in `children()` then `error_children()` order. A task inside these lists is now walked for unknown keys.
- No `lift_of` row exists for these keys (`keys.rs:1280-1286` asserts `None`), so none is deleted. The cross-check's `TWO` exemption (`keys.rs:1351`) is deleted. `finally_and_the_errors_two_takes_draw_nothing` (`keys.rs:1636-1650`) is replaced by `a_typo_inside_a_flowable_finally_is_refused_at_its_path` and `a_typo_inside_a_loop_errors_is_refused_at_its_path`.
- #2b adds the D11 warning (`not_honoured`, #13) and D12's `unsupported_after_execution_task` (in `validate_for_run`, so `build_plan` sees it).
- Spec #1 §10 is closed by a dated note: the `snapshot.source` instruction was a false premise (above), and the walker change is this section.

## 9. Replay, restart, `Exit`, kill, SLA

- **Replay** (`worker/replay.rs`) creates a new execution that copies the source. The new execution runs its end hooks like any other. A `CREATE_NEW_EXECUTION` retry ends the old execution `RETRIED`, which is terminal, so its `afterExecution` runs too. Kestra's `handleAfterExecution` checks only `isTerminated`.
- **Restart in place** is #7's. When #7 builds it, it must remove `errors`/`finally`/`afterExecution` runs, as `ExS:339-351` does. A dated note goes on #7's row.
- **`Exit`.** The flow `finally` does not run after an `Exit` that ends the execution. `drives_forward` is false, as in Kestra (`ES:171`). `afterExecution` runs.
- **Kill.** No `finally` runs at either level. `afterExecution` runs after `KILLED` (D8). Kill on a terminal execution stays 409 (D9).
- **SLA.** `claim::sla_terminal` skips a terminal execution (`claim.rs:4652-4691`). An SLA does not fire during `afterExecution`. Kestra's expiry loop counts the execution live until "terminated" (`DE:622-626`), but `handleExecutionChangedSLA` stops at a terminal state (`ES:1662`). What a Kestra SLA does to an execution whose state is already terminal is not measured. It is left to #16, with a dated note.

## 10. Graph and frontend

- `graph/mod.rs:831-841` draws `finally:` for `AllowFailure` only. It draws it for every kind whose model now has it (`Branch::Finally` and `Relation::Finally` exist, `graph/mod.rs:229-251`). The flow-level lists are already drawn (`:489-553`). `graph/tests.rs` gains one test per kind family. The frontend's topology already renders `finally` and `after_execution` (`frontend/src/components/workflows/topology/topologyModel.ts:111-121`). No frontend change for the graph.
- #2b, D15: `executionFinished` (`polling.ts:43-45`) becomes `isTerminalState(state) && runs.every(r => isTerminalState(r.state))`. `ExecutionTopologyTab.tsx:67-79`, `ExecutionGanttTab.tsx:91` and `timing.ts:50` read the same helper (`executionLive`, new in `executionState.ts`). `actions.ts:53,74` (the kill and resume buttons) keep reading the execution state: the kill is 409 on a terminal execution (D9). Tests go in `polling.test.ts` and the tab tests, under `docs/en/frontend-rules.md`'s rules for data hooks and polling. No new component, no copy beyond the existing labels, no API regeneration.

## 11. Security (D16)

- **No new surface.** No endpoint, permission, render site or secret path is added. `afterExecution` and `finally` tasks render through the same `Surface` as every task, scoped to the execution's workspace.
- **Work after a kill.** As in Kestra, `afterExecution` runs after a kill and cannot itself be killed (D9). An author's after-list keeps running for up to its tasks' `timeout:`. This is Q3.
- **Storage and containers are not taken from live work** (D13). The sweeps and the pruner read `execution_is_idle`, so an after-run's files, `_wd` snapshots and containers stay until it ends. This closes the same hole for `Exit` siblings, which was already open before #2 (§6).
- **The plan cannot point a stored run at another task** (D3). Without D3, the deploy would have handed in-flight runs another task's spec, which may hold another task's secrets.
- **Refusals read the draft only.** D12's and D14's refusals read the flow being saved or started: no repository, workspace or other flow. Their messages carry task ids and type names, never a value.

## 12. Questions for the controller

Each question is the user's to answer: a change of scope, a behaviour change for running work, or a conflict between Kestra's behaviour and egret's safety. Everything else above is decided by default and dated.

- **Q1. Split #2 into #2a and #2b, two PRs from one spec and one plan (D1)?** **Default: yes.** #2a ships the flow and flowable `finally:` and the loops' `errors:` (13 banks). #2b ships `afterExecution:` and the Rule 2 fix (8 banks). The alternative is one PR of about 16 tasks, touching the model, plan, executor, worker, three sweeps, a migration and the frontend.
- **Q2. Executions running at deploy time start running their end hooks (D4)?** **Default: yes.** They are re-planned from their own source at the next round, with an index-stable layout (D3), so no stored run is mis-read. A block that already ended does not go back. The alternative is a per-execution rules column (migration) so that pre-#2 executions keep skipping their `finally:`/`afterExecution:`. That keeps "runs by the rules it started under", but drops the author's cleanup for those executions. Either way, rolling deploys must not mix binaries.
- **Q3. A kill cannot stop `afterExecution:` (D9).** Kestra answers 409 and force-executes the list after a kill. **Default: follow Kestra.** A runaway after-task ends only at its `timeout:`. The alternative is a safety divergence (roadmap §3.1 style): a kill on a terminal execution with live after-runs kills those runs and leaves the execution's state alone. It costs about one task in #2b and diverges from roadmap #20's goal of aligning the 409.
- **Q4. `WorkingDirectory` `finally:` after success (D7).** Kestra's code reads as never starting it, and the block never ending (not run). **Default: egret runs it, treating Kestra's behaviour as a bug.** The alternative is to refuse `WorkingDirectory` `finally:` with a named refusal until a live Kestra probe settles it.
- **Q5. `Pause` `finally:` moves from #2 to #9 (D14)**, where `Pause` `errors:` and `onPause:` already are, because all three need the same "resumed `Pause` re-enters `RUNNING`" mechanism. **Default: yes.** Until then it is refused at publish and at start, naming #9 (today it is silently dropped). Corpus: the one flow that has it is already refused.

### Answers (user, 2026-10-27)

- **Q1: yes.** Two PRs: #2a (Part A of the plan), then #2b (Part B).
- **Q2: yes** (default taken).
- **Q3: the user chose the safety divergence, NOT Kestra's behaviour.** A kill stops `afterExecution:` runs. This diverges from Kestra (which answers 409 and force-executes the list after a kill) so that a tenant cannot create a task that no one can kill; it goes into the roadmap §3.1 list as a security divergence in #2b. Plan Task B3 builds it: a kill of a terminal execution that still has live after-tasks is accepted (not 409), moves those runs to KILLING/KILLED through the existing kill path, and never changes the execution's own final state. Task B3's brief must carry this; the plan text below that says "follow Kestra" for D9 is superseded.
- **Q4: default** (egret runs `WorkingDirectory` `finally:`; Kestra's behaviour is treated as a bug).
- **Q5: yes.** `Pause` `finally:` moves to #9 and is refused naming #9 until then.

