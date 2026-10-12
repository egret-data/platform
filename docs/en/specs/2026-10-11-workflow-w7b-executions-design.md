# W7b — Workflows UI, part 2: the executions list, the execution detail (overview, gantt, logs, outputs), resume and replay

**Status:** implemented, 2026-10-12 (Tasks 1–7; the end to end and the Kestra comparison are §11 and §12).

**Depends on:**
- W7a (#659): the Workflows area, `StateChip`, `executionState.ts`, the minimal execution page, and `nextPollMs`;
- W10 (executions and the execution routes);
- W11 (logs);
- W9a (execution files).

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (Kestra's UI as reference) and §9 row W7.

**W7a spec:** `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`. Its rulings still hold. In particular, every execution runs the saved draft.

**Decided with the user, 2026-10-11:**
- **Real timestamps.** Executions and task runs get real `started_at` and `ended_at`, and attempts get an end, each written when the state changes (migration 0180). The executions list gains a root-only filter and a children-of filter, and `replayed_from_execution_id` is exposed. Kill, restart, labels, metrics and execution-level outputs stay out, because the backend has none of them.
- **Kestra's tabs.** The execution detail has the tabs Overview, Gantt, Logs and Outputs, with the tab in the URL. Clicking a run in the Gantt opens that run's logs and outputs. Metrics and Dependencies are left out because there is no data for them. W7c adds Topology.
- **The list.** It shows root executions by default, with a "Show child executions" switch. There are no bulk actions, because the backend replays and resumes one execution at a time.

**Decided with the user, 2026-10-11 (Task 2 review):**
- **A namespace prefix filter on the server.** `GET …/executions` gains `namespace_prefix`, a namespace and every namespace beneath it, dot-anchored. "Include child namespaces" sends it. It replaces the client-side expansion over known namespaces, whose merged paging was exact only for the first 200 rows of each namespace.
- **`root_only` keeps retries.** It hides loop iterations and subflow and flow-trigger children only. The execution a `CREATE_NEW_EXECUTION` retry starts continues a root run, so it stays in the default list, labelled Retry.

**Decided with the user, 2026-10-11 (Task 3 review):**
- **A real resume.** `POST …/executions/{id}/resume` releases every paused `Pause` run of the execution at once, whatever its `pauseDuration:` or `timeout:`, and each ends `SUCCESS`. That is Kestra's `ExecutionService.resume`, and `Pause.java` says a pause resumed before its duration "will always succeed". Until this ruling, the route only re-settled the execution: it ended a pause whose deadline had passed, which the worker's waker does by itself.

**Decided with the user, 2026-10-11 (Task 4b, a defect that predates W7b):**
- **A `Pause` egret cannot run is refused at validation.** A `Pause` that declares `behavior`, `onPause`, `tasks` or `errors` is a validation error, `unsupported_pause_property`, naming the property and saying to remove it or use a plain `Pause`. Until this ruling it was a `not_yet_enforced` warning: the flow published and started, the task was planned an opaque runnable, the worker failed it with `NotALeaf` before recording an outcome, and the reaper released the claim for ever, so the execution stalled in `RUNNING` with no message. The editor now shows the error, publish refuses it, and no start path can run it: the API start, the webhook, a schedule fire, a Flow trigger and a `Subflow` call all plan through `build_plan`, whose `validate_for_run` gate already refuses a saved draft with errors. `executor::pause_gap` remains the single list of the four properties, and the flow schema classifies `behavior`, `onPause` and `errors` as `refused`, so the editor no longer offers them.

**Decided with the user, 2026-10-12 (Task 7 review, a defect that predates W7b):**
- **`ForEach` is refused at validation.** A task of type `io.kestra.plugin.core.flow.ForEach` is a validation error whose message points to `io.kestra.plugin.core.flow.Loop`, egret's loop, and the flow schema stops offering `ForEach`. Until this ruling it drew only a `not_yet_enforced` warning, so the flow published and started, but its body never ran: every start path planned it as an opaque task that no handler serves. *(Corrected 2026-10-12, Task 7b: measured with the core handler set, the worker fails it with `NoHandler` and the execution stalls, as Task 4b's `Pause` did; it does not end `SUCCESS`. A silent success in the production registry was not measured.)* The error code is `unsupported_foreach`, under every spelling that folds to `foreach` (`core.foreach`, any case), and the message names `io.kestra.plugin.core.flow.Loop` (alias `core.loop`). The API start, the webhook, a schedule fire, a Flow trigger and a `Subflow` call all refuse it through `build_plan`'s `validate_for_run` gate. The schema never offered it. Kestra 2.0, the image of §11, also refuses `ForEach`: creating a flow with it answers 422 "Invalid type: io.kestra.plugin.core.flow.ForEach", and the same flow with `Loop` is created (measured 2026-10-12). Task 7b implements the refusal.

## 1. Why

Since W7a, a user can run a flow from the browser, but can follow it only on a minimal page that lists task-run states. W7b gives runs the surface Kestra users expect:
- a filterable list;
- a timeline of what ran when, retries included;
- live logs;
- outputs and their files;
- the two actions the engine supports.

The gantt needs real start and end times, and the engine does not record them. The only end time it has is a run's `updated_at`, which changes on every write. W7b adds the timestamps.

## 2. Divergences from Kestra

| Kestra | egret | Why |
|---|---|---|
| Live updates through SSE (`/executions/{id}/follow`, `/logs/{id}/follow`) | Polling: the execution with W7a's backoff, logs every 2 s by cursor | egret's APIs are poll-only (W11) |
| Tabs: overview, gantt, logs, outputs, metrics, dependencies, audit-logs, assets | Overview, Gantt, Logs and Outputs. Topology arrives with W7c | No metrics or dependency data. Audit and assets are EE-only |
| Actions: restart, replay (with inputs, from a task run, at a revision), kill, pause, resume, change state, set labels, unqueue, force run, delete | Replay (whole execution, frozen source and inputs) and resume | The only engine commands that exist (§5) |
| Resume releases the paused run at once and it succeeds; a `Pause` may declare `onResume` inputs, which the resume dialog collects | Resume releases every paused `Pause` at once and each succeeds; no inputs on resume | egret ignores a `Pause`'s `onResume` (`spec/schema/property-treatment.json`: `ignored`), and the route takes no body, so there is nothing to collect |
| The executions list has bulk actions | No bulk actions | Per-execution routes only; the user's ruling |
| The list shows every execution | Root executions by default, with a "Show child executions" switch | The user's ruling: loop iterations and subflow children would flood the list |
| Outputs has an expression debugger | No expression debugger | No expression-evaluation endpoint |
| CSV export of the list | Not in W7b | Not asked for |
| The log level filter defaults to INFO (a stored per-user display preference) | No level filter by default: every level is shown, and `?level=` is absent | FE-STATE-3: "empty" means the unfiltered log, so a log of DEBUG lines must not read "No logs"; FE-PAGE-2: no `?level=` means no filter. Kestra's quieter default would be a preference, with the empty state saying a level filter hides lines. Decided 2026-10-11 (Task 5 review) |

**Found against a running Kestra, 2026-10-12 (Task 7, §11).** The local `kestra/kestra` image (2.0 nightly) ran the same flow as the end to end: a retried `Fail`, a `Write`, a `Sequential` with two children, 400 log lines, a three-value `Loop`, a WARN line and a `Pause`. These differences are deliberate:

| Kestra | egret | Why |
|---|---|---|
| List columns: Id, State, Start date, End date, Duration, Namespace, Flow, Labels, Triggers; a column chooser and an executions chart above the table | ID, Flow, Namespace, State, Start, Duration, Trigger; a `SummaryStrip` of counts by state, each a quick filter | No labels on the backend (§10). The end date is the start plus the duration, both shown. A chart was not asked for, and the strip answers "what state are these in" |
| List filters: free-text search, State, Started (default **Last 24 hours**), Namespace, Flow, Kind, Scope, Hierarchy, Labels, advanced filters, saved filters | State, Namespace (with child namespaces), Flow, Time (no default), and the child switch | `GET …/executions` has no text search, kind, scope or labels. Without a default window the list's empty state means "nothing ran" (FE-STATE-3), and a link with no `?time=` shows everything (FE-PAGE-2). Hierarchy is the child switch, root-only by default (the user's ruling) |
| Gantt: one bar per task run; a retried run has a warning icon, and its attempts are a picker in the row's details | Each attempt is its own segment of the run's bar, coloured by the attempt's end state, and the row carries a "↻ 2" count | Retries visible on the timeline itself (§6.2) |
| Gantt: selecting a row expands its details under the row, with an attempt picker and that attempt's logs, filtered by a log level that defaults to INFO | One panel under the chart with the run's times, attempts, message, logs (every level, every attempt, each line labelled) and outputs; `?run=` in the URL | The bars keep their places while a run is read (§6.4) |
| Logs: a card per run with an attempt picker that shows one attempt's lines; level counters that jump to the next line of a level; log search; a "Temporal view" that lists every line by time; repeated similar lines folded ("×400 similar lines"); Copy all | Groups per run with every attempt's lines (an "Attempt N" label when a run has more than one); filters by level, task and run; virtualised, so 10 000 lines stay unfolded and smooth; Download | The logs route reads by level, task and run only (W11), so there is no search. A flat view, level counters and folding were not asked for in W7b; §15 lists them |
| Outputs ("Input/Output" in this build): three panes, the task outputs with a key or value search, the value, and the expression debugger. A file output is previewed in place, a CSV as a table with columns, with Row count, Encoding and an editor view | Outputs: the runs with outputs, then the run's tree; a `kestra:///` value is a file row whose Preview opens a drawer on the right, text line by line, with the row cap and Download | No expression endpoint (§10); the route reads text as UTF-8 only, so there is no encoding to choose; a line is the file's own text, which a guessed CSV dialect could misread. §15 lists the table view |
| Resume: a confirmation that asks "Are you sure you want to resume execution …?" | A confirmation that says what happens: every paused task ends now as a success, and the execution continues | FE-ACTION-1 asks the dialog to name the consequence. Both release a pause before its deadline: Kestra ended a 10-minute pause at once with `SUCCESS`, as egret's end to end does with a 5-minute one |
| Replay: in the execution's actions menu, with Original, Latest or a specific flow revision | A Replay button beside Resume, from the frozen source and inputs | The first row of this table: the engine replays only the frozen source |

**An engine divergence, found in the same run.** It is not a UI difference and is not deliberate; it is recorded here for the engine:

| Kestra | egret | Note |
|---|---|---|
| A three-value `Loop` listed one execution and showed "Iterations 3 Success" inside it | The `Loop` spawns three child executions, listed under "Show child executions" and in Overview's Child executions | The engine's loop (§6.1, W3b), measured against Kestra's golden corpus when it was built. This Kestra 2.0 build behaved differently; reconciling the two is engine work, outside W7b |

## 3. Backend: real timestamps

### 3.1 Migration 0180

```sql
ALTER TABLE workflow.executions
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;
ALTER TABLE workflow.task_runs
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;
```

Existing rows keep `NULL`; nothing is guessed. The UI marks their times as estimated (§6.3).

### 3.2 When each timestamp is written

**Executions**
- `started_at` is set once, the first time the execution enters `RUNNING`.
- `ended_at` is set once, when the execution enters a terminal state (`models/state.rs` `is_terminated`).
- A resumed pause does not move `started_at`.
- When a transition leaves a terminal state, `ended_at` is cleared, and it is set afresh when the row next becomes terminal. *(Corrected 2026-10-11, Task 1: `RETRY_FAILED_TASK` does this today — RUNNING, FAILED, RETRYING, RUNNING, SUCCESS — and a test pins it.)*

**Task runs**
- `started_at` is set once, the first time the run enters `RUNNING`. For a worker task, that is when the run enters `RUNNING`, which is the first attempt's start; a worker's claim sets only a lease, so time queued before the claim is inside the bar. For a flowable, it is when the executor first runs it.
- `ended_at` is set once, when the run enters a terminal state.

**Attempts**
- Each element of `attempts` gains `ended_at`. It is in milliseconds after the execution's `created_at`, the same base as the existing `started_at`. It is set when that attempt reaches a terminal state, including the state it ends in before a retry starts the next attempt.

**Where the writes happen**
- Every write is part of the same `UPDATE` that changes the state: `SET state = $s, ended_at = CASE WHEN … THEN COALESCE(ended_at, now()) END …`. A state change is never read and then written.
- The implementation lists every state-writing site of executions and task runs (executor, worker, subflow, loop, retry, resume, SLA cancel, replay) and changes each one.
- A test fails if a site that changes state does not set the timestamps.

**Invariants, tested**
- `ended_at` is null or at least `started_at`.
- Once a row is terminal, `ended_at` does not change on later writes, such as an outputs or `updated_at` bump.

### 3.3 The list route

`GET …/executions` gains three filters:
- `root_only=true` excludes loop iterations (`loop_index IS NOT NULL`) and subflow and flow-trigger children (`trigger_execution_id IS NOT NULL`). *(Ruled 2026-10-11, Task 2 review: a `CREATE_NEW_EXECUTION` retry, which has `parent_execution_id` and no `loop_index`, is kept.)* `loop_index` is the exact discriminator: migration 0138's `workflow_executions_loop_run_whole_or_absent` makes the loop triple whole or absent, so a parent with an index is an iteration and a parent without one is a retry (`worker/replay.rs`).
- `parent=<uuid>` returns the rows whose `parent_execution_id` or `trigger_execution_id` equals that id. The id must belong to the caller's workspace; an id from another workspace answers an empty page, never an error that reveals it exists.

- `namespace_prefix=<ns>` returns the rows whose namespace is `<ns>` or starts with `<ns>.`: `namespace = $n OR starts_with(namespace, $n || '.')`, static SQL and no `LIKE`, so `a.b` never matches `a.bc` and `_` or `%` is a literal character. *(Ruled 2026-10-11, Task 2 review.)*

It is a 400 to send `root_only=true` and `parent` together, to send `namespace` and `namespace_prefix` together, or to send any of them with a malformed value.

Both filters are served by existing indexes that already lead with those columns (`workflow_executions_loop_run_idx`, migration 0135, and `workflow_executions_trigger_idx`, migration 0138), as `EXPLAIN` showed. No new index is added. *(Corrected 2026-10-11, Task 1: new indexes were built and measured, and the planner used neither.)* A test pins the plan.

### 3.4 DTOs

- `ExecutionListItemDto` and `ExecutionDto` gain:
  - `started_at: Option<DateTime>`
  - `ended_at: Option<DateTime>`
  - `replayed_from_execution_id: Option<Uuid>`, from migration 0166's column
- `TaskRunDto` gains `started_at` and `ended_at`.
- Each element of `attempts` carries `ended_at` when it is known.
- `ExecutionDto` gains `can_manage: bool`, the PDP's `workflow:manage` answer, as `FlowDto` carries it (W7a). It is on the execution because its flow can be trashed, and then cannot be read, while the execution still can. *(Added 2026-10-11, Task 3.)*
- `execution_trigger` is typed `object | null`: the server sends `null` for an API start. *(Corrected 2026-10-11, Task 3.)*

`openapi.json` and the generated client are regenerated with `npm run gen:api` (FE-DATA-1).

## 4. Routes and the list

Routes live under `/explore/:workspace/workflows/`, in W7a's lazy area:

| Route | Page |
|---|---|
| `executions` | The executions list (§4.1) |
| `flows/:id/executions` | The flow detail's Executions tab: the same list, pinned to the flow |
| `executions/:id` | Redirects to `executions/:id/overview` |
| `executions/:id/:tab` | The execution detail. `tab` is `overview`, `gantt`, `logs` or `outputs`; an unknown tab redirects to `overview` |

W7b replaces W7a's `ExecutionsPlaceholder` and `ExecutionMinimal`. The flow detail's tab list gains Executions.

### 4.1 The list

The page is built from `ListPageShell`, then the toolbar, then `SummaryStrip` (counts by state for the current page), then the table (FE-PAGE-1).

**Columns**
- id, as a link
- flow, as a link (hidden on the flow's own tab)
- namespace
- state chip
- start, relative with the exact time on hover
- duration, ticking while running
- trigger: API, schedule, webhook, flow trigger or subflow, each with an icon and a text label

**Filters**, all in the URL (FE-PAGE-2):
- state, multi-select, sent as a comma list;
- namespace, with "Include child namespaces", which sends the namespace as `namespace_prefix` instead of `namespace` (§3.3);
- flow;
- time window: presets 1 h, 24 h, 7 d and 30 d, plus custom, sent as `since` and `until`;
- the "Show child executions" switch, off by default, which sends `root_only=true` while off.

**Paging**
- 50 rows per page.
- The page number is in the URL. `total` drives the pager.

**Refresh**
- The list polls every 5 s, but only while some row on the page is not terminal and the browser tab is visible.
- An "Updated Ns ago" label and a Refresh button sit in the toolbar.

**States** (FE-STATE-1–3)
- Loading shows a skeleton.
- With no filter set and nothing returned, the list shows "No executions yet", plus a link to the flows page.
- When filters return nothing, it says so and offers to clear the filters.
- On an error, the list keeps the last rows and shows an inline error with Retry.

## 5. The execution detail

### 5.1 Header and polling

The header, shared by every tab, shows:
- the execution id, the flow (a link), the namespace and the state chip;
- the start time and the duration;
- the trigger;
- "Replayed from <id>" when `replayed_from_execution_id` is set;
- "Child of <id>" when the execution has a parent or trigger execution.

The execution is polled with W7a's `nextPollMs` (2 s, backing off to 30 s while reads fail) until it is terminal. A failed poll keeps the last data and shows an inline error with Retry.

### 5.2 Actions

Both actions need `workflow:manage`. A user without it does not see them. The page reads it from `ExecutionDto.can_manage` (§3.4).

**Resume**
- It is shown, and enabled, exactly while the state is `PAUSED`, and goes through a confirm dialog. The dialog says that every paused task ends now as a success, whatever its duration or timeout, and that the execution continues.
- The route releases every `PAUSED` `Pause` run at once with `SUCCESS` (header ruling, Task 3 review). It moves each pause's deadline to now, and the run then leaves `PAUSED` through the same path a passed deadline takes, so its history (`… PAUSED SUCCESS`, `RESTARTED` on the execution) and its timestamps are those of a natural end. A `LoopUntil` waiting for its next iteration is not touched.
- The race with the worker's waker is decided by the execution's lock: whichever takes it first wins. A `timeout:` pause the waker ended first is `FAILED`, and the resume answers 409. A pause still `PAUSED` when the resume takes the lock ends `SUCCESS`.
- On success the execution query is refreshed. The toast follows the state the route answered: "Resumed the execution" only when it is no longer paused; "Resumed, but the execution then ended Failed." (or Killed, Cancelled) for a bad end; and a defensive note if the answer is still `PAUSED`.
- A 409 (not paused, or the waker won) shows the server's message in the dialog.
- **Only a `Pause` is resumed** *(ruled 2026-10-11, Task 3 fix round 2, after review N-1)*. An execution is also `PAUSED` while a `LoopUntil` waits for its next iteration, and a resume does not end that wait, as in Kestra. So:
  - the route answers 409 "Nothing to resume: no Pause task is waiting." when no `Pause` run is waiting, and writes nothing;
  - the page enables Resume only when some `PAUSED` run has `resume_with` `NEVER` or `TERMINAL:*` (the engine's own record of a waiting `Pause`). Otherwise Resume is disabled with the reason "Waiting on a loop condition, not a Pause task".

**Replay**
- It is enabled only when the execution is terminal. Otherwise it is disabled with the reason "Replay is available when the execution has finished".
- The dialog says that replay starts a new execution from this execution's frozen YAML and inputs.
- On 201 the page navigates to the new execution.
- A 409 or 400 shows the server's message, for example "replay the loop root".

Every disabled action shows its reason as visible text (FE-ACTION-4), using W7a's `DisabledReasons`.

### 5.3 Overview

- **Inputs:** a key/value table. A `kestra:///` value is a file row (§8).
- **Trigger:** its type and variables. Webhook body, headers and parameters are shown as a collapsible JSON view.
- **Child executions:** a compact list fetched with `parent=<id>`, giving id, state, `loop_index`/`loop_value` and start time, each linking to its execution.
- **Task runs:** a compact list of task id, value, state and duration ("so far" while running). The task id is a link that opens the Gantt with that run selected (`gantt?run=<run_ix>`). It is a link rather than a clickable row so that it works from the keyboard (FE-A11Y-3). *(Corrected 2026-10-11, Task 3.)*

## 6. The Gantt

### 6.1 Rows and nesting

- There is one row per task run, in `run_ix` order.
- Rows nest under their `parent_ix` parent and collapse.
- A `Loop` (`io.kestra.plugin.core.flow.Loop`, also Kestra 2.0's name; `ForEach` is refused, see the header) has no child rows. Its iterations are child executions, listed in Overview's "Child executions" (§5.3), because `executor/flowable/loop_.rs` creates no run in the parent for a loop body. The loop is one flowable row. No `CreateRun` sets `value` today, so the value is shown beside the task id only when present. *(Corrected 2026-10-11, Task 4.)*
- Each row shows the task id, the value if there is one, the state chip and the duration.
- The rows are a `treegrid`: each row carries `aria-level`, `aria-posinset`/`aria-setsize` and, with children, `aria-expanded`. *(Added 2026-10-11, Task 4 review.)*

### 6.2 Bars

**The axis**
- The axis runs from the execution's start to its end, or to now while it runs.
- It has ticks with readable labels.

**Bars**
- A run's bar runs from `started_at` to `ended_at`, coloured by the run's state token.
- Each attempt is a separate segment within the row, from its `started_at` to its `ended_at`. A failed attempt that was retried is coloured by its own end state, so retries are visible.
- A run that has not ended extends to now. While the execution is live, the gantt redraws on animation frames, at most every 40 ms (every 500 ms from 50 runs), which is Kestra's `Gantt.vue` interval. It pauses while the tab is hidden, and steps to once a second under `prefers-reduced-motion`. In an execution that has ended, a run that never did is capped at the execution's end and marked estimated. *(Corrected 2026-10-11, Task 4.)*
- A run that never started (`CREATED`, or `SKIPPED` by `runIf`) shows a marker, not a bar.

### 6.3 Estimated times

For an execution recorded before migration 0180, `started_at` and `ended_at` are null. The gantt then uses the old recipe (`tests/executions_routes.rs` `gantt_rows`):
- start is the execution's `created_at` plus `attempts[0].started_at`;
- end is the run's `updated_at` once `ended_seq` is set, or once the run's state is terminal. A run whose chip says it finished is never drawn open to now; its `updated_at` is its ending write, because claim and reap only touch `RUNNING` runs. *(Added 2026-10-11, Task 4.)*
- in a finished execution, a run that has not ended is capped at the execution's end and marked estimated.

These bars are drawn hatched, with an "estimated" label and a note at the top of the tab.

### 6.4 Interaction

- Selecting a row opens a panel below the chart with:
  - the run's logs (the Logs component of §7, filtered to `task_run_id`);
  - its outputs (§8);
  - its `message`, when there is one.
- The selected run is in the URL as `?run=<run_ix>`.
- Rows are a `grid` with arrow-key navigation. Enter opens the panel and Escape closes it (FE-A11Y-3).
- State is shown by chip text and icon, not by colour alone (FE-A11Y-4).
- The chart is drawn with DOM elements, not a chart library. This is the same choice as Kestra's `Gantt.vue`, and it keeps the bars accessible.

## 7. Logs

The Logs component serves both the Logs tab and the Gantt's run panel.

**Filters**
- The filters are minimum level (TRACE, DEBUG, INFO, WARN, ERROR), task, and task run. In the Logs tab they are in the URL.
- They are sent as `min_level`, `task_id` and `task_run_id`.

**Lines**
- Lines are grouped by task run, and each group collapses. Execution-level lines, which have no task, are shown first.
- Each line shows:
  - the time;
  - a level badge with text;
  - the attempt, when there is more than one;
  - the message in a monospace font with whitespace preserved.

**Following** (the W11 contract)
- Pages are read by cursor: `after` is the last `next_after`.
- While more may come, the component polls every 2 s.
- Polling stops only when `complete && items.length === 0`, never on a terminal state alone, because the reader lags the writer by 2 s.

**Scrolling**
- The view auto-scrolls when the user is at the bottom.
- When the user has scrolled up, auto-scroll stops and a "Jump to latest" button appears.

**Volume**
- Pages are 500 lines.
- The list is virtualised, so tens of thousands of lines stay responsive.
- Earlier history beyond what is loaded is reached by "Load earlier".

**Download**
- "Download" pages through the cursor with the current filters, and saves a `.log` file, one line per log line.

**States**
- Loading shows a skeleton.
- When there are no lines, the empty state reads "No logs yet" while the execution runs, and "No logs" once it is complete.
- A failed poll keeps the lines already shown, and offers Retry.

## 8. Outputs and files

**Layout**
- The left side lists the task runs that have outputs.
- The right side shows the selected run's outputs as a collapsible tree.

**Values**
- Each value can be copied.
- Each path can be copied as an expression, for example `{{ outputs.<taskId>.<key> }}`. Keys that are not identifiers use bracket notation. *(Corrected 2026-10-11, Task 6: the Kestra loop form `outputs.<taskId>[value]` is not used. egret's renderer stores outputs flat by task id, and no task run carries a `value`. A loop iteration is its own child execution: its expressions are valid inside that iteration, and the page notes that the parent execution cannot reach them.)*

**Files**
- Any string starting with `kestra:///` in outputs or inputs renders as a file row:
  - the name;
  - the size, from `file/metas`;
  - **Preview**, which opens a drawer with `file/preview` rows;
  - **Download**, a link to `file`.
- The drawer shows Ion values as JSON. Text is shown line by line. A file that cannot be previewed says "This file cannot be previewed"; a truncated preview says so, with the row cap.
- A 404 (pruned after retention, or missing) shows "This file is no longer available".

**Not included**
- There are no execution-level outputs (§10).

## 9. Frontend structure

**Pages:** `frontend/src/routes/workflows/` holds:
- `ExecutionsList.tsx`, shared by the list page and the flow tab;
- `ExecutionDetail.tsx`, the header, the tabs and the actions;
- one file per tab.

**Components:** `frontend/src/components/workflows/executions/` holds:
- `gantt/` (`ganttModel.ts`, a pure model, and its view);
- `logs/` (`useLogFollow.ts`, a hook, and the view);
- `outputs/`;
- `files/`.

**Query keys:** `workflowsKeys` gains
- `executions(ws, params)`;
- `execution(id)` (existing);
- `executionFile(id, path)`.

The logs have no query key. `useLogFollow` keeps a cursor across pages, which one query key cannot hold, so it calls the generated fetcher `workflowGetExecutionLogs` directly, under FE-DATA-2's exception for a cursor across pages (the user's ruling, 2026-10-11, Task 5 review).

**Polling:** W7a's `nextPollMs` is generalised to any query result, and W7a's tests stay green.

## 10. Out of scope

- Kill, restart, change state, labels, delete, and bulk actions.
- Metrics, Dependencies, the expression debugger, and execution-level outputs.
- Topology (W7c), namespaces (W7d), and triggers (W7e).
- SSE.

## 11. Comparing with Kestra

A temporary Kestra from the local `kestra/kestra` image runs on a free port during Task 7 and is removed afterwards. These parts are compared for structure and interaction (FE-REF-2):
- the list's filters and columns;
- the gantt (rows, nesting, attempt segments, selection);
- logs (filters, grouping, following);
- outputs and file preview.

Differences go into §2.

**Done, 2026-10-12 (Task 7).** The comparison ran against the local image `28f59bf175e1` (Kestra 2.0 nightly) in a temporary container `w7b-kestra-t7`, removed afterwards, with the end to end's flow (§12). Every difference found is in §2: the deliberate ones in its second table, the engine's `Loop` divergence in the third; the ones not built are in §15. Resume matched: Kestra's Resume ended a 10-minute pause at once with `SUCCESS`, and egret's end to end does the same with a 5-minute one.

## 12. Testing

**Rust**
- Timestamps on every state path: success, failure, retry (`RETRY_FAILED_TASK` and `CREATE_NEW_EXECUTION`), pause and resume, subflow (`wait` true and false), loop iterations, SLA cancel, and replay.
- A manual resume:
  - every bound of pause is released at once to `SUCCESS`, with its timestamps;
  - several parallel pauses;
  - the race with the waker (one transition);
  - 409 when not paused, and 409 when only a `LoopUntil` waits, with nothing written;
  - 404 for another workspace.
- The invariants of §3.2.
- The attempt end on retry.
- `root_only` and `parent`, including another workspace's parent id giving an empty page, both filters together giving 400, and the indexes being used (`EXPLAIN`); a retry execution listed under `root_only`, a loop iteration and a subflow child not.
- `namespace_prefix`: exact, child, a sibling sharing the prefix without the dot, another workspace, and 400 beside `namespace`.
- The migration.

**Vitest** (real components, fixtures that can fail):
- `ganttModel`:
  - bars, attempt segments, running bars, never-started markers, estimated fallback, nesting;
  - unsorted input.
- `useLogFollow`:
  - cursor progression;
  - stopping only on `complete && empty`;
  - a failed poll keeping lines;
  - auto-scroll versus a scrolled-up user.
- The list:
  - filters written to the URL and read back;
  - `root_only` sent when the switch is off;
  - the three states;
  - polling only while non-terminal rows exist.
- The detail:
  - actions enabled and disabled with reasons;
  - resume enabled for every pause, and its feedback following the answered state;
  - replay navigation and server errors;
  - unknown tab redirect.
- Outputs:
  - `kestra:///` detection;
  - copy-as-expression;
  - preview states.

**Playwright,** on a private local stack, never the shared dev stack. The run:
1. runs a flow with a retried task, a `Pause` with a duration, a loop and a log-heavy task;
2. follows the logs live;
3. opens the gantt and selects a run;
4. previews a file;
5. resumes;
6. replays.

DOM measurements cover light and dark mode, plus the gantt at narrow widths.

**Done, 2026-10-12 (Task 7).** `frontend/tests/e2e/workflows-executions.spec.ts`, on a stack of its own (the recipe is in its header and in `frontend/tests/e2e/README.md`). The flow: `flaky` (a `Fail` whose condition holds on the first attempt only, `retry` constant, `maxAttempts: 3`), `write` (a `Write` of three CSV lines), `chatty` (400 lines), `each` (a `Loop` over three values), `wait` (a `Pause` of `PT5M`) and `after`. The spec asserts:
- the list shows the root alone, the switch adds the three iterations, and the flow's Executions tab shows the root;
- the gantt shows two attempt segments on `flaky`, `FAILED` then `SUCCESS`, measured inside the timeline cell and in order; at 900 px every mark stays in its cell, the chart does not scroll sideways, and the task id is not truncated;
- selecting `flaky` opens its panel, whose logs are its own lines, labelled by attempt; Escape closes it and the arrows move between rows; `?run=2` on the Logs tab shows only that run's lines;
- the Logs tab fits its panel, which does not scroll around the log;
- the file preview shows the three rows, and Escape returns focus to Preview;
- the log follows live: lines written after Resume arrive in the open view, and once the log is complete no read is sent for 6 s;
- Resume releases the pause long before its deadline, and the gantt then shows it `SUCCESS` and closed;
- Replay opens a new execution whose header says "Replayed from" the first.

It was run red against three mutations (`root_only` never sent, log polling never stopping, only the last attempt segment drawn), against the code before the Logs tab fix (its panel overflowed by 132 px) and against the code before the retry-count fix (the task id `flaky` truncated at 900 px), each failing at its own assertion, and green after. **The axis-inset fix has no red run** (fix round 1): with only the inset reverted, the end to end stayed green, because its last run, `after`, took about 1 s of a 30 s axis, 8 px, wider than a bar's 2 px minimum. The 900 px check runs again once the execution is complete, but it guards the inset only when the last run is that short. The inset's defect was measured in the browser instead (§15).

**Gate:**
- frontend: FE-TEST-4 (lint, tsc, test, build, every `test:bundle*`);
- backend: `cargo test -p egret-workflow`, `egret-server`, fmt, and clippy `-D warnings`.

## 13. Plan shape

1. Backend: migration 0180, timestamp writes at every state site, attempt ends, `root_only`/`parent` plus indexes, DTO fields, and the regenerated API.
2. The executions list and the flow's Executions tab.
3. The execution detail frame: routes, header, polling, tabs, Overview, and the actions.
4. The Gantt.
5. The Logs component and tab, plus its use in the Gantt panel.
6. Outputs and files.
7. End to end, the Kestra comparison, and docs.

## 14. Rules this package must meet

All of `docs/en/frontend-rules.md`. The rules most at stake:
- FE-PAGE-1, FE-PAGE-2 and FE-PAGE-4;
- FE-STATE-1 to FE-STATE-4;
- FE-ACTION-1 to FE-ACTION-4;
- FE-VISUAL-1, FE-VISUAL-4 and FE-VISUAL-6;
- FE-DATA-1 to FE-DATA-3;
- FE-ROUTE-2;
- FE-A11Y-3 and FE-A11Y-4;
- FE-REF-1 and FE-REF-2;
- FE-TEST-1, FE-TEST-2 and FE-TEST-4.

## 15. Known limits

- Executions recorded before migration 0180 have estimated times (§6.3).
- A retry of a loop iteration or of a subflow child appears in the default list as a root. Its `CREATE_NEW_EXECUTION` replacement carries `parent_execution_id` (the child) with no `loop_index` and no `trigger_execution_id`, which is exactly the shape of a retry of a root, so `root_only` keeps it (§3.3). *(Found in the Task 2 review.)*
- A `Pause` with `behavior`, `onPause`, `tasks` or `errors` is refused, not run (Kestra runs all four). An execution already stalled on one before 2026-10-11 stays stalled; replaying it is refused because its frozen source no longer plans. *(Ruled 2026-10-11, Task 4b.)*
- **Found in Task 7, 2026-10-12**, by measuring in the browser:
  - Two layout defects were fixed. The Logs tab's log had a fixed height, `100vh − 22rem`, which overran its panel (by 132 px at the end to end's 1280×720, and at 1440×900 too), so the panel scrolled around the log and hid its last lines; the log now fills the tab, with a 15 rem minimum. At 900 px, "2 attempts" truncated the five-letter task id `flaky`; the count now reads "↻ 2", with "attempts" for a screen reader and on hover. Also at 900 px, a run shorter than its bar's 2 px minimum width that ends at the axis end gets a bar that leaves the timeline cell. This was the `after` run, 0.07 s of a 17 s execution. With the inset reverted, its bar ended 1.1 px past the cell's right edge and gave the chart a 1 px sideways scroll (595 against 594). With the inset of 0.5 rem on both sides, both were gone. Both states were measured in the browser in fix round 1. The end to end cannot reach this case reliably (§12).
  - Safari's default Tab order skips buttons, so a log group's toggle is reached with Option+Tab there; the log itself is a tab stop in Chromium and WebKit alike, measured.
  - Below a viewport about 640 px tall, the Logs tab's log keeps its 15 rem minimum and the tab's panel scrolls.
  - From the Kestra comparison (§2), not built: a flat, time-ordered log view, level counters that jump between lines, folding of repeated lines, log search (the route has no text filter), a CSV preview as a table, and an End date column.
- A flow version published before the 2026-10-12 ruling that uses `ForEach` is refused at every start, as the versions with a refused `Pause` are (Task 4b). An execution started before then that reached one stays stalled, and replaying it is refused because its frozen source no longer plans. *(Ruled 2026-10-12, Task 7 review; Task 7b.)*
