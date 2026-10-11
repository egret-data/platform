# W7b — Workflows UI, part 2: the executions list, the execution detail (overview, gantt, logs, outputs), resume and replay

**Status:** design, 2026-10-11; not yet implemented.

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
| The executions list has bulk actions | No bulk actions | Per-execution routes only; the user's ruling |
| The list shows every execution | Root executions by default, with a "Show child executions" switch | The user's ruling: loop iterations and subflow children would flood the list |
| Outputs has an expression debugger | No expression debugger | No expression-evaluation endpoint |
| CSV export of the list | Not in W7b | Not asked for |

Differences found while comparing against a running Kestra (§11) are added here, each with its reason.

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
- If a later transition leaves a terminal state, which nothing does today, `ended_at` is cleared. A test pins that no current path does this.

**Task runs**
- `started_at` is set once, the first time the run enters `RUNNING`. For a worker task, that is when the worker claims and starts it. For a flowable, it is when the executor first runs it.
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

`GET …/executions` gains two filters:
- `root_only=true` excludes rows with a non-null `parent_execution_id` or `trigger_execution_id`, that is, loop iterations, subflow children and flow-trigger children.
- `parent=<uuid>` returns the rows whose `parent_execution_id` or `trigger_execution_id` equals that id. The id must belong to the caller's workspace; an id from another workspace answers an empty page, never an error that reveals it exists.

It is a 400 to send both, or to send either with a malformed value.

Indexes on `(workspace_id, parent_execution_id)` and `(workspace_id, trigger_execution_id)`, partial where the column is non-null, keep both filters cheap.

### 3.4 DTOs

- `ExecutionListItemDto` and `ExecutionDto` gain:
  - `started_at: Option<DateTime>`
  - `ended_at: Option<DateTime>`
  - `replayed_from_execution_id: Option<Uuid>`, from migration 0166's column
- `TaskRunDto` gains `started_at` and `ended_at`.
- Each element of `attempts` carries `ended_at` when it is known.

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
- namespace, with "include children" implemented client-side over the known namespaces, because the API matches namespaces exactly;
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

Both actions need `workflow:manage`. A user without it does not see them.

**Resume**
- It is shown only while the state is `PAUSED`, and goes through a confirm dialog.
- A `Pause` task with neither `pauseDuration` nor `timeout` stays paused after a resume call. This is a backend limit, `routes.rs` resume. For that case Resume is disabled with the reason "This pause has no duration or timeout, so it cannot be resumed from here". The pause's properties are read from the execution's frozen `source`.
- On success the execution query is refreshed.

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
- **Task runs:** a compact list of task id, value, state and duration. Clicking a row opens the Gantt with that run selected.

## 6. The Gantt

### 6.1 Rows and nesting

- There is one row per task run, in `run_ix` order.
- Rows nest under their `parent_ix` parent and collapse.
- A loop or `ForEach` run's children are labelled with their `value`.
- Each row shows the task id, the value if there is one, the state chip and the duration.

### 6.2 Bars

**The axis**
- The axis runs from the execution's start to its end, or to now while it runs.
- It has ticks with readable labels.

**Bars**
- A run's bar runs from `started_at` to `ended_at`, coloured by the run's state token.
- Each attempt is a separate segment within the row, from its `started_at` to its `ended_at`. A failed attempt that was retried is coloured by its own end state, so retries are visible.
- A run that has not ended extends to now and is redrawn on each poll. The redraw is throttled to animation frames while visible, and paused while the tab is hidden.
- A run that never started (`CREATED`, or `SKIPPED` by `runIf`) shows a marker, not a bar.

### 6.3 Estimated times

For an execution recorded before migration 0180, `started_at` and `ended_at` are null. The gantt then uses the old recipe (`tests/executions_routes.rs` `gantt_rows`):
- start is the execution's `created_at` plus `attempts[0].started_at`;
- end is the run's `updated_at` once `ended_seq` is set.

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
- Each path can be copied as an expression, for example `{{ outputs.<taskId>.<key> }}`. Inside a loop, the expression form follows Kestra's `outputs.<taskId>[value]` and is noted as such.

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
- `executionLogs(id, params)`;
- `executionFile(id, path)`.

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

## 12. Testing

**Rust**
- Timestamps on every state path: success, failure, retry (`RETRY_FAILED_TASK` and `CREATE_NEW_EXECUTION`), pause and resume, subflow (`wait` true and false), loop iterations, SLA cancel, and replay.
- The invariants of §3.2.
- The attempt end on retry.
- `root_only` and `parent`, including another workspace's parent id giving an empty page, both filters together giving 400, and the indexes being used (`EXPLAIN`).
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
  - the resume no-timeout case;
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
- Resume cannot release a `Pause` that has no duration or timeout (§5.2).
- "Include children" for namespaces only covers namespaces the client already knows (§4.1).
