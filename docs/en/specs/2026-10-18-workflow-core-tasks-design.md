# Workflow follow-ups, group C: the remaining Kestra core tasks

**Status:** designed 2026-10-18. **C1 built and verified 2026-10-18** on `feat/workflow-kestra-core-tasks`; **C2 built and verified 2026-10-18** on `feat/workflow-kestra-executor-tasks`. The oracle ratchet (`cargo test -p egret-workflow --test executor`, the `executor_matches_golden` printout) went from 100 mismatching / 100 baselined to **84 / 84**, 0 broken: 16 entries banked (`OutputValues` 11, `Assert` 1, `PurgeLogs` 4) with the entries still baselined named, by cause, in `oracle-exec/README.md`. The census LIVE count went 254 to **281**, and the handler-surface ceiling (`python3 handler_surface.py`) 232 to **250** (population 228). Per task: 100 to 89, 89 to 88, 88 to 84.

**C2 results** (Task 5 report, the `executor_matches_golden` printout and `python3 handler_surface.py`, over the 351-flow corpus): the ratchet went from 84 mismatching / 84 baselined to **73 / 73**, 0 broken. 11 of the 12 C2 entries are banked; `valids/task-runif-taskswithstate` is baselined because the flow-level `finally:` does not run (egret has no `write_execution_context` and no `skipped_when_failures_exist` run; `finally-flow*` is baselined for the same reason). The census LIVE count went 281 to **293**, `UNKNOWN_TYPE_REFUSED` 33 to 21, `VALIDATE_REFUSED` stays 27, the `PUBLISHED_BUT_REFUSED` table 18 rows to 6, and the handler-surface ceiling 250 to **261** (population 228).

**Where the claims come from:** every claim about egret's code was checked in the worktree `egret-coretasks` at `15406ad9` (main, after #689). Code is cited `path:line` relative to `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`, and its paths are relative to that checkout.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9. Since no-silent-stall Task 1, these six types are `unknown_task_type` errors. The oracle (`executor/mod.rs`, the `PublishedButRefused` list) refuses 40 published corpus entries because of them.

**Decided with the user, 2026-10-18:**
- **`PurgeLogs` reaches only the flow's own namespace and its child namespaces.** This is the W9b write rule for `PurgeFiles` and KV (`storage/nsfiles/access.rs:201`, `Access::Purge`). Kestra OSS checks nothing here.
- **Two PRs: C1 for the worker tasks, C2 for the executor tasks.** C1 ships first, because every C2 entry reads its result back through an `OutputValues`.
- **The design below (§2-§4) was approved as presented.**

**Default decisions (2026-10-18).** The rule: follow Kestra unless egret has a reason not to, and keep correctness tight.
- **D1.** `Assert` records no `success`/`failed` metrics. Egret has no metric store. The counts go into the log and the failure message instead.
- **D2.** `PurgeLogs.purgeNonExecutionLogs` deletes nothing and counts 0. Every line in `workflow.task_logs` belongs to an execution (migration 0159), so egret has no non-execution logs.
- **D3.** A replay copies the execution's `variables`, just as it already copies `labels` (`worker/replay.rs:275`). Kestra's `childExecution` copies the whole execution.
- **D4.** `SetVariables`, `UnsetVariables` and `Labels` are refused inside a `WorkingDirectory` at validate, as `Exit` is (`spec/validate.rs:301`). Kestra admits only `RunnableTask` children there, and these three are `ExecutionUpdatableTask`s.
- **D5.** `SetVariables` merges exactly as Kestra's `deepMerge` (`MapUtils.java:62-133`), including list concatenation without duplicates and "a null new value keeps the old one". Spec §3.2 said "any other value replaces", which is not Kestra's rule for lists or nulls; §3.2 is corrected.
- **D6.** A failed update records one attempt `[FAILED]`, as the golden does, through a new `Command::FailUnstarted`. A `Loop`'s failure at `CREATED` keeps no attempt.
- **D7.** A `Labels` map whose two keys render to the same text fails the run (`labels: two keys render to "<k>"`). Kestra throws an `IllegalStateException` there; a run failure with a message is the same outcome, stated.
- **D8.** A whole-expression `variables:` that renders `null` fails the run (the existing `render_map` error). Kestra's `asMap` returns null and `SetVariables` then throws a `NullPointerException` in `deepMerge`'s caller; a named failure is the same outcome without the crash.
- **D9 (2026-10-18, controller default decision during Task 4).** Updates are computed whenever `advance()` still drives the execution, that is `executor::drives_forward`: not terminal and not `KILLING`. So `PAUSED` and `RETRYING` executions compute updates, and a `SetVariables` beside a waiting `Pause` runs before the resume. This supersedes the wording "only while RUNNING" in the plan.
- **D10 (2026-10-18, controller default decision after the final review).** A `Loop` iteration inherits its parent's `variables`, as it already inherits `inputs` and `labels`: the iteration INSERT (`worker/spawn.rs`) copies the parent row's column. Kestra's `Execution.loopExecution` builds the iteration with `this.variables` (`kestra/core/src/main/java/io/kestra/core/models/executions/Execution.java:438-460`). It is a copy at spawn time, as Kestra's is: a `SetVariables` inside an iteration changes only that iteration, and nothing flows back to the parent. A `Subflow` child still inherits nothing (§3.2).

## 1. Which mechanism each task needs, measured

Kestra runs a `RunnableTask` on a worker and an `ExecutionUpdatableTask` in the executor (`executor/…/ExecutorService.java:1371`). The golden shows the difference in every entry:

| Task | Kestra interface | Golden run states |
|---|---|---|
| `io.kestra.plugin.core.output.OutputValues` | `RunnableTask` | `CREATED SUBMITTED RUNNING SUCCESS` (all 28 entries) |
| `io.kestra.plugin.core.execution.Assert` | `RunnableTask` | `… SUBMITTED RUNNING SUCCESS` / `… FAILED` |
| `io.kestra.plugin.core.log.PurgeLogs` | `RunnableTask` | `CREATED SUBMITTED RUNNING SUCCESS` (all 5) |
| `io.kestra.plugin.core.execution.SetVariables` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS`; on error `CREATED FAILED` (`set-variables-duplicate`) |
| `io.kestra.plugin.core.execution.UnsetVariables` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS` |
| `io.kestra.plugin.core.execution.Labels` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS`; on error `CREATED FAILED` (`labels-update-task-empty`) |

Executor tasks never pass through `SUBMITTED`. They also fail from `CREATED`, because Kestra runs `update()` before it records the `RUNNING` attempt (`ExecutorService.java:1389-1408`). A falsy `runIf` gives `CREATED SKIPPED` (`task-runif-executionupdating`), which is the existing `runIf` path.

Many of the 40 entries are also blocked by something outside this group: `WaitFor`, `afterExecution`, webhooks, flow concurrency, `Pause`. The plan measures which entries bank. This spec promises none.

## 2. C1: three worker handlers

Each task is a `TaskHandler` in `worker/handlers/` and a `spec::registry` row, following the pattern of `Return` (`worker/handlers/ret.rs`). C1 adds no migration.

### 2.1 `OutputValues`

- `values: Property<Map<String,Object>>`. It is rendered recursively, so values inside nested maps and lists are rendered too, as Kestra's `render(values).asMap` does.
- On success the outputs are `{"values": <rendered map>}`. With no `values`, the outputs are `{"values": {}}`.
- A render error fails the run, and the message is the render error.

### 2.2 `Assert`

- `conditions: List<String>` (required) and `errorMessage: Property<String>`.
- Each condition is rendered separately. It fails when the rendered text is falsy under `executor::flowable::branch::is_truthy`, which is the same truthiness the `If` task uses (`executor/flowable/branch.rs:93`). A condition that fails to render also counts as failed. A failed condition is logged at `ERROR`, using Kestra's messages ``Assertion `<c>` failed!`` and ``Assertion `<c>` failed, failed to render `<err>` ``.
- If any condition fails, the run is `FAILED` with the message `"<n> assertion failed!"` or `"<n> assertions failed!"`. When `errorMessage` is set, a newline and the rendered `errorMessage` follow. Otherwise the run is `SUCCESS` with no outputs.
- There are no metrics (D1). The INFO log line `"<s> succeeded, <f> failed"` carries the counts instead.

### 2.3 `PurgeLogs`

- **Properties:**
  - `namespace`, `flowId` and `executionId` (each a `Property<String>`);
  - `logLevels: Property<List<Level>>`;
  - `startDate` and `endDate`, each a `Property<String>` read as an ISO-8601 date-time, with `endDate` required;
  - `purgeExecutionLogs` (default `true`) and `purgeNonExecutionLogs` (default `true`);
  - `batchSize: Property<Integer>`.
- **Scope, decided:**
  - Every delete is limited to the running execution's workspace.
  - With no `namespace`, the scope is the flow's own namespace and its child namespaces.
  - With a `namespace`, the namespace must pass `storage::nsfiles::access::authorize` with `Access::Purge`, through a resource that names logs, so the refusal reads "may not purge logs of namespace …". Otherwise the run is `FAILED` with that message, and nothing is deleted.
  - A child namespace means a match on a dot boundary: `a.b` reaches `a.b.c` but not `a.bc`.
- **The delete:**
  - It removes rows from `workflow.task_logs` joined to `workflow.executions` on `execution_id`, filtered by workspace and namespace scope, plus `flow_id`, `execution_id`, `level IN (…)` and `ts` in `[startDate, endDate]`, each filter only when the property is set.
  - **(2026-10-18, C1 Task 4 review)** Three corrections, each Kestra's rule (`AbstractJdbcLogDataStore.buildDeleteCondition`):
    - The window is `[startDate, endDate]`: both ends are inclusive (`ts <= endDate`, Kestra's `lessOrEqual`). It used to read `[startDate, endDate)`.
    - With `flowId`, the namespace matches exactly. The child namespaces are reached only without `flowId`.
    - The flow's own namespace is read from the execution row (`workflow.executions.namespace`, by execution id and workspace), not from the render scope, so the task runs on a deployment with no `[storage]` too.
  - With `batchSize`, the work is batches of at most that many rows, each its own statement, until a batch deletes fewer. Without it, the delete is one statement. A future `endDate` over a busy namespace can keep the loop going while new lines arrive; Kestra's loop has the same property.
  - **(2026-10-18, final review)** The usual future `endDate` also deletes the purging execution's own earlier lines, and the lines of other in-scope executions that are still running. Log lines are stored asynchronously (the log writer batches them), so `count` depends on timing. Kestra behaves the same.
  - **(2026-10-18, final review) A deliberate difference:** an `executionId` that does not render to a UUID fails the run, naming `executionId`. Kestra matches it against no row and succeeds with a count of 0. Egret's execution ids are UUIDs, so a non-UUID can only be an author's mistake.
  - It writes no audit record of its own. The run and its log lines are the record.
- **Non-execution logs:** `purgeNonExecutionLogs` deletes nothing (D2). With `purgeExecutionLogs: false`, nothing is deleted.
- **Outputs:** `{count, executionLogsCount, nonExecutionLogsCount}`, with `nonExecutionLogsCount = 0`.
- **The handler needs a pool.** It is registered the way `Delete` is (`handlers::storage::register`, `worker/handlers/storage.rs:336`), not in `with_core_tasks`.
- **One known divergence:** `valids/purge_logs_full_arguments` runs in `io.kestra.tests` with `namespace: purge.namespace`. Kestra answers SUCCESS. Egret answers FAILED, by the decided rule. The entry moves from the refused list to `baseline.txt`, with a comment that gives the decided reason. It is a permanent line there, not a gap that waits for work.

**(2026-10-18, C1 built)** Where the build differs from §2.1-§2.3:
- **Registration.** Each of the three also has a key in `runtime::served_for`, the second copy of the `with_core_tasks` keys that publish's "this deployment has no handler" check reads. The sections above do not name it. `PurgeLogs` is registered by its own `worker::handlers::purge_logs::register(&mut handlers, pool)` in `runtime::handlers_for`, beside `kv::register`, not by `handlers::storage::register` as §2.3 says.
- **`Assert`.** A missing `conditions` fails the run with `conditions: must not be null` (Kestra declares it `@NotNull`). **(final review)** A boolean or number entry is read as its text (`true`, `1`, `1.5`), as Jackson coerces a scalar to `String`; a null, list or map entry fails the run naming `conditions[i]`. `is_truthy` is reached as `crate::executor::is_truthy`, because `executor::flowable` is private to `worker`.
- **`OutputValues`.** `models::property::render_map` is the shared recursive map render. When two keys render to the same text the first one wins, and a top-level array, number, boolean or null is an error. A collision replaces a first value that is null, as Java's `putIfAbsent` does (fixed in the final review). One edge is known and deferred: a whole-expression `values` that renders `null` errors where Kestra's `asMap` returns null.
- **One oracle entry is held by an unported construct, not by C1.** `valids/if-in-parallel` runs and its `assert` FAILS where Kestra's SUCCEEDED, because its conditions read the `Loop`'s own `outputs:`, which egret does not produce. The same cause holds `loop-outputs*`.

## 3. C2: three executor tasks

### 3.1 Plan and states

- **Plan:** a new `TaskKind::ExecutionUpdate { op, spec }` in `executor/plan.rs`, where `op` is one of `Set`, `Unset` or `Labels`. Each type is a `spec::registry` row with no handler and is listed in `EXECUTOR_LEVEL` (`spec/registry.rs:333`), so every deployment serves it, as with `Exit`.
- **The update is computed at `CREATED`**, in a render pass of its own (`worker/render.rs::render_updates`, after `render_expressions`), like a `Loop`'s `values:` (`renders_at`, `worker/render.rs:1345`). The pass renders the task's properties against the execution's current variables and labels and computes the new value.
  - **On error,** it fails the run with the message: `Command::FailUnstarted`, `CREATED FAILED` with one attempt `[FAILED]` (D6).
  - **On success,** it records the new value in `RenderSide` and completes the run in the same pass: `CREATED RUNNING SUCCESS`, with one attempt `CREATED RUNNING SUCCESS`. Settle writes the new `variables` or `labels` to the execution row, in the same transaction that holds the execution lock.
- **Runs that update in the same round** (two branches of a `Parallel`) are applied in run order. Each one sees the result of the one before it, so the outcome does not depend on timing.
- **`runIf` falsy** gives `CREATED SKIPPED`, through the existing path, with nothing written.

**(2026-10-18, final review)** The text above is corrected to the build. The pass is `render_updates`, separate from `render_expressions` because an update CHANGES the surface every later render of the settle reads; the plan kind is `ExecutionUpdate { op, spec }`; the attempt is `CREATED RUNNING SUCCESS`, the golden's. And the completion moved from `advance()` into the pass (final review I1): when it was left to `advance()`, a computed update beside a failing one in the same `Parallel` or `Dag` was failed by `fail_the_rest` (`CREATED FAILED`, no attempt) while settle still wrote its change. Now the change and the `SUCCESS` land together, and only the failing update fails, as in Kestra. `flowable::open`'s `ExecutionUpdate` arm is kept as the same completion, for a caller that sets `rendered` itself.

### 3.2 Execution variables (migration 0196)

> **Renumbered (2026-10-19):** written as migration 0195; #694 merged `0195_perm_engine_grants.sql` first, so this package ships as `0196_workflow_execution_variables.sql`.

- **Column:** `ALTER TABLE workflow.executions ADD COLUMN variables jsonb`, with a CHECK that it is NULL or a JSON object. NULL means "the flow's `variables:`".
- **Render:** `vars` is the execution's `variables` when set, and the flow's `variables:` otherwise (`worker/render.rs:183`). This is Kestra's `RunVariables` rule: execution variables first, the flow's as fallback (`core/…/runners/RunVariables.java:541-550`).
- **`SetVariables`:**
  - `variables: Property<Map>` is required and rendered; `overwrite` defaults to `true`.
  - The result is a deep merge into the current variables, by D5's rule, Kestra's `deepMerge`: a null new value keeps the old one, two maps merge recursively, two lists concatenate (the new items not already in the old list are appended in order), and anything else is replaced.
  - With `overwrite: false`, when a top-level key already exists, the run fails with Kestra's message ``"`overwrite` is set to false and the following variables already exist: <k1>,<k2>"``.
- **`UnsetVariables`:**
  - `variables: Property<List<String>>` is required; `ignoreMissing` defaults to `false`.
  - A dotted key walks nested maps (`a.b.c`).
  - A missing last segment is a no-op, as in Kestra: `vars.remove(key)` on an absent key changes nothing (`UnsetVariables.java`, `removeVar`). So in Kestra `ignoreMissing` makes no difference to the leaf.
  - A missing or non-map parent segment fails the run with a message naming the key, unless `ignoreMissing` is set. Then it is a no-op. **This diverges from Kestra on purpose:** Kestra's recursion hands `null` down and throws a `NullPointerException` whatever `ignoreMissing` says. That is a crash, not a behaviour to copy.
- **Copying `variables`:**
  - A replay copies `variables` (D3), through both doors: the retry replay (`worker/replay.rs`) and the user's `POST .../executions/{id}/replay` (`routes.rs::replay_execution`, through `NewExecution::variables`).
  - A `Loop` iteration inherits its parent's `variables` at spawn time (D10), as in Kestra's `Execution.loopExecution`.
  - A `Subflow` child does not inherit it; its `vars` are its own flow's, as in Kestra.

**(2026-10-18, final review)** This list said "Subflow and loop children do not inherit it … as in Kestra", which was false for a loop: Kestra's `loopExecution` passes `this.variables`. Corrected by D10. The user-facing replay route did not copy `variables` either (final review I3); it does now.

### 3.3 `Labels`

- **`labels` has three forms:**
  - a string, rendered and parsed as a JSON object of strings;
  - a list of `{key, value}`, where a later duplicate key wins;
  - a map.
  
  Keys are rendered. A value that is a map is rendered and written as its JSON text. Any other value is rendered as a string.
- **Refusals:**
  - A key starting with `system.` fails the run: ``"System labels can only be set by Kestra itself, offending label: <k>=<v>"``.
  - An empty value fails the run: ``"Label values cannot be empty, offending label: <k>"``.
- **The result** goes to `executions.labels`, the execution's own labels (`crate::labels`). Starting from the execution's current labels (the flow's, then its own, the later one winning), the new keys override, and the column stores the merged list. Later `{{ labels.x }}` reads see the new value, through the existing `labels::execution_labels`.

**(2026-10-18, C2 built)** Where the build differs from §3:
- **`Command::FailUnstarted` and the `[FAILED]` attempt (D6).** An update that fails at `CREATED` is written `CREATED FAILED` with one attempt `[FAILED]`, as the golden shows (`valids/set-variables-duplicate`, `labels-update-task-empty`). The existing `render::fail` pushes no attempt, so a new command does this.
- **`taskrun.id`.** It is reserved for an update run created in the same settle, so `{{ taskrun.id }}` renders inside a `Labels` at `CREATED` (`valids/labels-update-task`, `npe-labels-update-task`). Every other kind still sees `taskrun.id` null in the settle that creates it. That is a known gap, unchanged.
- **Updates under a kill (Task 3).** A `CREATED` update under a kill closes `CREATED` to `KILLING` to `KILLED` over two rounds, because `kill::is_leaf` does not count it, as `Exit` does.
- **Key order.** `executions.variables` is `jsonb`, so the key order after a later settle is Postgres's (shortest key first), not insertion order. Kestra keeps insertion order.
- **Registration.** `runtime::served_for` needs no key for `EXECUTOR_LEVEL` types, so the three publish on a deployment with no handlers (test `the_executor_level_types_publish_on_a_deployment_with_no_handlers`).
- **Known divergences from Kestra's `Labels`** (no golden flow covers them):
  - A list or float label value renders as JSON text (`["a","b"]`, `1e20`), where Java's `toString` gives `[a, b]` and `1.0E20`.
  - When several labels break a rule, egret names the first in source order; Kestra names the first in `HashMap` order.

## 4. Tests and acceptance

- **Unit tests:** each handler, and each op's pure computation (deep merge, dotted unset, the three label forms, every refusal message), in the style of `ret.rs`'s tests.
- **`advance()` tests:** the three state shapes from §1, with failure from `CREATED`, `runIf` skipped, and two updates in one `Parallel` round.
- **DB tests:**
  - `PurgeLogs` scope: its own namespace, a child, a sibling `a.bc` that must not be reached, another namespace refused, another workspace never reached, and the batch loop.
  - Migration 0196's CHECK.
  - Settle writing `variables` and `labels`.
  - Replay copying `variables`.
- **Oracle ratchet:** `cargo test -p egret-workflow --test executor`.
  - Each banked entry leaves `baseline.txt` and the refused list.
  - No entry newly breaks.
  - `golden.json` is not edited.
  - `purge_logs_full_arguments` stays in `baseline.txt` with §2.3's reason.
- **Full gate before each merge:** fmt, clippy, the egret-workflow, egret-query, egret-dataapi and egret-server suites, then vitest, lint and typecheck when the frontend changes. Run it on a private Postgres and Valkey, never the shared dev stack.

## 5. Scope

**In scope:** the six tasks, migration 0196, the render rule for `vars`, and the oracle bookkeeping.

**Out of scope:**
- a metric store (D1);
- non-execution logs (D2);
- a namespace permission model beyond the W9b purge rule;
- frontend changes. The execution page already shows labels and outputs. Showing `variables` is a later frontend item.
