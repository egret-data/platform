# No silent stall: refuse what egret cannot run, and fail what slips through

**Status:** design, 2026-10-12.

**Depends on:** W7b (#662), which refused the stalling `Pause` (`unsupported_pause_property`) and `ForEach` (`unsupported_foreach`) and recorded the rest of the class in its §15.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`.

**Decided with the user, 2026-10-12 (W7b final review):**
- The silent-stall class is a separate engine package, right after W7b.
- The general rule: a task planned as opaque that no handler can run is refused at validate and at start.

**Decided with the user, 2026-10-12 (this package's brainstorm):**
- **Refusal is per deployment.** The six publishable types no production deployment wires, and the script and plugin types when no runner is configured, are refused like the static cases. Whether a deployment serves a type is decided from its configuration by the same logic that builds the worker's handlers (§4).
- **The worker fails what slips through.** A claimed run with no handler (`NoHandler`) or with no leaf (`NotALeaf`) ends `FAILED`, with a system log line saying egret cannot run it. It no longer keeps its lease and loops for ever. Retry and `errors:` apply as for any `FAILED` (§5). This reverses the earlier design choice, recorded on `WorkerError::NoHandler`, that a failure "would look like the author's flow was wrong". The log line is what says it is not.
- **The served set is installed once at start-up** (`Served`, like `storage::current()`), and a pure function judges a flow against it. `build_plan` and `validate_for_run` stay pure and deployment-free (§4.2).

**Decided with the user, 2026-10-12 (Task 4 review):**
- **Stored executions are planned without validation; validation gates starts only.** An execution that started runs by the rules it started under. Every re-plan of an existing execution (`claim::load_with`, a `Loop` iteration's sub-source in `spawn::insert_iteration`, a `CREATE_NEW_EXECUTION` retry in `replay`) uses `executor::plan_stored`: parse and flatten, no `validate_for_run`. New starts (`start_execution_in_on`, a `Subflow` target's current source) keep `build_plan`. Without this, every construct §3 refuses turned the executions already running into `Unplannable` aborts that cycled through the reaper for ever (§5).

## 1. Why

An execution can stall in `RUNNING` for ever, with nothing in its logs. The mechanism is the same for every case:
1. the worker claims the run;
2. it fails before recording an outcome (`NotALeaf` or `NoHandler`);
3. the error leaves `run_one` and the tick, so the run keeps its lease;
4. the reaper clears the lease a `working_timeout` later;
5. the worker claims the run again, and the loop repeats.

The only trace is a `warn` per cycle in the server log. W7b's UI shows such an execution as running, with no end.

Measured on main `ee8194af`, these cases reach the loop:

| Case | Refusal at run | At publish today |
|---|---|---|
| `AllowFailure`, `WorkingDirectory` (planned opaque) | `NotALeaf` | clean, no warning |
| `LoopUntil`/`WaitFor` whose bounds are expressions (`loop_until_gap`) | `NotALeaf` | `not_yet_enforced` warning |
| A type spelled like Kestra control flow that egret does not implement | `NoHandler` | `not_yet_enforced` warning; refused as unknown at publish, but a draft starts |
| An unknown type | `NoHandler` | `unknown_task_type` warning; refused at publish, but a draft starts |
| `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack` | `NoHandler` | **clean**: they have `TASK_TYPES` rows and no deployment registers a handler (`egret-bin/src/main.rs`, "Six of twenty-three publishable types hang in production") |
| Script tasks with `workflow.container_runner = none` | `NoHandler` | refused at publish (`check_script_tasks`), but a draft starts |

## 2. What this package does not do

- It does not run `AllowFailure`, `WorkingDirectory`, expression-bounded `LoopUntil`, or the six unwired types. Each stays a separate decision: `AllowFailure` needs a `WARNING` state that `RunState` cannot hold, and `WorkingDirectory` hands a subtree to one worker.
- It adds no kill route. §5 ends the existing stalled executions; a kill route stays a follow-up.
- It adds no frontend code. The new codes reach the editor through the existing validation report and Problems tab, and the schema loses the unserved types through the existing filter.

## 3. Static refusals (deployment-free, in `validate_for_run`)

Every construct `build_plan` plans as an opaque `Runnable` because egret has no executor for it becomes a validation **error**. It is no longer a warning, nor silent. `validate_for_run` is `build_plan`'s third gate, so every start path refuses these flows, and publish refuses them too.

| Construct | Code | Message says |
|---|---|---|
| `io.kestra.plugin.core.flow.AllowFailure` (and `core.allowfailure`) | `unsupported_allow_failure` | egret does not run it; set `allowFailure: true` on each child task instead |
| `io.kestra.plugin.core.flow.WorkingDirectory` (and `core.workingdirectory`) | `unsupported_working_directory` | egret does not run it; pass files between script tasks with `outputFiles` and `inputFiles` |
| `LoopUntil`/`WaitFor` with a `loop_until_gap` field | `unsupported_loop_until_property`, which replaces the `not_yet_enforced` warning | names the field (`checkFrequency.interval`, …) and says it must be a literal |
| A type spelled like Kestra control flow that egret does not implement (`canonical_flow_type(..).is_some()`, not `foreach`) | `unsupported_flow_task`, which replaces the `not_yet_enforced` warning | names the type |
| An unknown type | `unknown_task_type`, from a warning to an error | unchanged: check for a typo |

**Rules that hold the table together:**
- **One predicate each.** `pause_gap` and `loop_until_gap` stay the single predicates that both `build_plan` and `validate` ask. The errors are emitted where the warnings were.
- **Drafts still save.** `update_flow` and `create_flow` do not refuse on validation errors, so a draft with any of these is stored. The editor shows the finding as an error.
- **The paths that keep their errors.** `working_directory_tasks_required`, `working_directory_child_not_runnable`, `working_directory_namespace_files_invalid` and `allow_failure_tasks_required` stay. A `WorkingDirectory` now carries `unsupported_working_directory` too.
- **Nested `errors:` too.** `walk_task` descends through `children()` only. `validate_script_tasks` documents that difference, and `build_plan` does flatten a flowable's own `errors:` (`Sequential`, `Parallel`, `If`, `Switch`, `Dag`). So the checks of this table must also reach tasks under `error_children()`; otherwise an `AllowFailure` inside a `Sequential`'s `errors:` slips through. The plan chooses whether `walk_task` itself descends or a second walk does it, and states what that changes for the other checks.
- **A guard over the corpus: opaque implies refused.** For every flow in `oracle-exec/corpus`, every task that `build_plan` plans as `TaskKind::Runnable` while `models::TaskKind` is not `Leaf` must carry a `validate_for_run` error at its path. A future flowable planned opaque without a refusal then fails a test by name.
- **The oracle.**
  - The corpus `valids` that use `AllowFailure` or `WorkingDirectory`, or a `LoopUntil` with a gap, now fail to plan. Each joins `PUBLISHED_BUT_REFUSED` with its own `cause`, written as W7b's `Pause` rows were.
  - A grep of `valids` finds 6 files that mention `AllowFailure` and 13 that mention `WorkingDirectory`; the plan re-derives the exact set.
  - The ratchet (`baseline.txt`) is re-measured. No entry may move from match to mismatch for any other reason.

## 4. Per-deployment refusals

### 4.1 `Served`

`Served` is the set of task type ids this process's worker would run: exact strings, matched as `Handlers::get` matches them, with every alias and canonical spelling it registers.

- **One source, two consumers.**
  - `runtime::served_for(&WorkflowConfig) -> Served` is pure, with no I/O and no runner construction.
  - `runtime::handlers_for` keeps building the handlers.
  - A test asserts `served_for(cfg) == handlers_for(cfg).type_ids()` for each `container_runner`: `none`, `docker`, and `kubernetes` with the client build stubbed or skipped as the plan finds possible. A handler added without `served_for`, or the reverse, fails it.
- **What it holds today:**
  - with `none`: the eleven core tasks, the storage, namespace and KV handlers;
  - with a runner: also the four script types with their aliases, and every `kestra.` catalog type.

  The plan measures the exact list from the code and does not copy it from this sentence.
- **Installation.**
  - `egret-bin`'s `main.rs` installs `Served` once at start-up, before the router and the worker start, as `storage::install` does.
  - `runtime::served()` reads it.
  - A process that never installs one is a test process, and it gets a value chosen by the test. **Never "serve everything" by default:** a missing installation in production must not reopen the hole. The plan decides how tests install it, and every start path must have one.

### 4.2 `unrunnable`

`spec::unrunnable(flow: &Flow, served: &Served) -> Vec<Finding>` is pure.

- It walks every task in `tasks`, `errors`, `finally` and `afterExecution`, through `children()` and `error_children()`.
- For each task the plan would make a `Runnable`, meaning a `models::TaskKind::Leaf` whose type is known, it emits an error `task_type_not_served` at the task's path when `served` lacks the type.
- Unknown types are left to `unknown_task_type` (§3), so a typo is not reported twice. The same holds for the flowables of §3.
- **The message names the type and the reason, by class:**
  - script and `kestra.` types: "this egret deployment has no container runner (`workflow.container_runner`), so it cannot run `X`";
  - every other type: "this egret deployment has no handler for `X`, so it cannot run it".
  - Both add that the flow cannot be started here, and neither blames the flow.

### 4.3 Where it is called

Every place that builds a validation report, plus the two places that start work:

1. `routes::draft_dto`, the report on `GET` and `PUT` of a flow;
2. `routes::validate_flow_source`, which both validate routes use;
3. `publish`, which refuses with the existing 400 shape for validation errors;
4. `worker::claim::start_execution_in_on`, the one chokepoint for the API start, the webhook, a schedule fire, a Flow trigger and replay, which answers `Error::InvalidInput("the flow cannot run on this deployment: …")`. Schedule and Flow-trigger fires keep their existing handling of a start that fails; the plan records what each does and adds nothing new;
5. `worker::subflow`'s target, where the run fails with the existing message "cannot execute an invalid flow '{ns}'.'{id}': …", as when the target does not plan.

Loop iterations re-plan the parent's own stored source, with `executor::plan_stored` (no validation; Task 4 review ruling). That source passed point 4 when the execution started, so they need no check of their own; an execution started before this package's refusals is not re-judged by them, and its run fails in the worker instead (§5).

### 4.4 The flow schema

`spec::schema`'s `Accepted` already keeps a script or plugin type only when its gates pass. It also requires `served` membership now, so `egret.sql`, `core.http` and the other unwired types are no longer offered by the editor. The schema's ETag changes with the served set.

## 5. The worker fails what slips through

**The new behaviour.** In `worker::run_one`, `WorkerError::NoHandler` and `WorkerError::NotALeaf` no longer leave the tick:
- The run's attempt is recorded `FAILED` through the same commit path a handler's failed outcome takes: the same lease fence, the same `ended_at` stamp, the same retry evaluation.
- One `ERROR` line is written to the run's log:
  - `NoHandler`: "egret cannot run task type `X` on this deployment: no handler is registered for it. This is not an error in the flow."
  - `NotALeaf`: "egret cannot run task `id`: it is a control-flow task (`X`) that egret does not execute. This is not an error in the flow."
- The `warn` in the server log stays.
- **Retry and `errors:` apply** as for any `FAILED`. A retry ends after `maxAttempts`, because each attempt fails the same way.

**Existing stalled executions end on their own.** When the reaper next releases the lease, the next claim fails the run and the execution settles. No migration and no clean-up script are needed.

**Unchanged:**
- `Unplannable` and `Corrupt` stay aborts, because those are damaged data, not a missing handler. `Unplannable` means only that the stored source does not parse or flatten: a stored execution is planned with `executor::plan_stored`, never re-validated (Task 4 review ruling), so a source that validation refuses since §3 reaches `NoHandler` or `NotALeaf` and fails like any other.
- `Registry::cancel` and `Registry::release` keep refusing on a miss.

**Docs that must change in the same commit as the behaviour:**
- `WorkerError::NoHandler` and `NotALeaf`;
- `spec::registry`'s "Not guarding it is still the right call" paragraph;
- `egret-bin/src/main.rs`'s "A flow whose task type is unregistered HANGS";
- `oracle-exec/README.md` item 19.

**The oracle driver.** Its comment says it "carries on" past a stall. It now sees a `FAILED` run. The traces of those entries change from a stall to `FAILED`; they were mismatches before and remain so, and the ratchet records it.

## 6. Testing

**Static refusals:**
- A unit test per new code, in `validate` and in `build_plan`, covering every spelling: FQCN and `core.` alias, any case.
- The corpus guard of §3, plus a mutation that removes one error and shows the guard fail by name.
- `PUBLISHED_BUT_REFUSED` updated and the ratchet re-measured.

**`Served` and `unrunnable`:**
- Unit tests for `unrunnable`:
  - a task nested in `errors:`, `finally:` and inside a flowable;
  - an unknown type reported once;
  - a served type passing.
- The drift test `served_for` against `handlers_for`, for each runner setting.
- Database tests on the branch's own Postgres container, with `container_runner = none`, a flow using `egret.sql`:
  - the validate routes and the flow `GET` carry `task_type_not_served`;
  - publish answers 400;
  - the API start answers 400 and writes no execution;
  - a schedule fire behaves as the existing failed-start path does;
  - a `Subflow` to it fails its run with the message;
  - the schema has no `egret.sql`.
- One mutation per call site of §4.3.

**The worker:**
- Database tests with a `Handlers` that lacks a type:
  - the execution ends `FAILED` and the run's log has the `ERROR` line;
  - with `retry`, the attempts stop at `maxAttempts`;
  - with `errors:`, the handler branch runs;
  - a run whose lease was reaped while stalled is failed by the next tick.
- The same for `NotALeaf`, using a flow inserted directly, since validation now refuses it.

**Live**, on a private stack, never the shared dev stack or a kind cluster named `egret`:
1. In the browser, a flow using `egret.sql` shows the red error, and Run is refused with the message.
2. An execution stalled under the old binary (started before the upgrade, on the same database) ends `FAILED` after the upgrade, and W7b's detail page shows the log line.

## 7. Known limits

- A deployment whose API process and worker process run with different `workflow` configurations can still accept a start that its worker cannot serve. §5 turns that into a `FAILED` run rather than a stall.
- A flow already published with a construct of §3 or §4 stays published and cannot start. The editor shows why.
- A replay of an execution whose source is now refused is itself refused, as for `ForEach` (W7b §15).
