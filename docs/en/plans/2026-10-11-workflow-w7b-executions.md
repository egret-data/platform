# W7b — Workflows UI part 2 (executions): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the browser, a user lists executions (root executions by default), opens one, and sees four tabs (Overview, Gantt, Logs, Outputs). They can resume or replay it. The gantt draws real start and end times, recorded by the engine on every state change.

**Architecture:**
- **Backend** (`crates/egret-workflow` plus migration 0180):
  - `started_at`/`ended_at` on executions and task runs, and `ended_at` on each attempt. All are written inside the same `UPDATE` that changes state.
  - `root_only` and `parent` filters on the list.
  - New DTO fields.
- **Frontend** (inside W7a's lazy Workflows area):
  - `ExecutionsList`, shared by the list page and the flow's Executions tab.
  - `ExecutionDetail`: the header, polling, actions and URL tabs.
  - A pure `ganttModel` with a DOM view.
  - A `useLogFollow` cursor hook with a virtualised log view.
  - An outputs tree with file rows.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), Postgres migrations, React 19 + TypeScript, TanStack Query 5, orval, vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-11-workflow-w7b-executions-design.md`. **§n refers to the spec.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes and test cases to verify against the code, not text to paste. Names were read at `7a64c3bb`.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every task review answers its reviewer checklist.
- **User decisions (spec header, 2026-10-11)**, in effect verbatim:
  - Real timestamps come from migration 0180, written when the state changes.
  - The list gets `root_only` and `parent` filters, and `replayed_from_execution_id` in the DTO.
  - There is no kill, restart, labels, metrics or execution-level outputs.
  - The detail page has the tabs Overview, Gantt, Logs and Outputs, with the tab in the URL. Clicking a run in the Gantt opens its logs and outputs.
  - The list shows root executions by default, with a "Show child executions" switch. There are no bulk actions.
- **W7a rulings still hold:**
  - Every execution runs the saved draft.
  - The state chip vocabulary.
  - `updated_at` strings round-trip verbatim.
- **Routes:** under `/explore/:workspace/workflows/`, in W7a's lazy area.
  - `executions`
  - `flows/:id/executions`
  - `executions/:id/:tab`, where the tab is one of `overview`, `gantt`, `logs` or `outputs`
- **Polling:**
  - The execution uses W7a's `nextPollMs`: 2 s, backing off to 30 s.
  - Logs poll every 2 s, and stop only when `complete && items.length === 0`.
  - The list polls every 5 s, only while a row on the page is non-terminal and the tab is visible.
- **List page size:** 50.
- **Log page size:** 500, virtualised.
- **Query keys:** `workflowsKeys`. Workspace-scoped keys carry the slug.
- **Migration number:** 0180. Check `ls migrations | sort | tail -1` first; if 0180 is taken on main, use the next free number and say so.
- **Never touch the shared dev stack.**
  - Backend tests use this branch's Postgres container `egret-w7b-postgres` on :5442, via `source .superpowers/sdd/2026-10-11-workflow-w7b-executions/w7b-env.sh`.
  - Playwright runs on a private stack.
  - The Kestra comparison uses a temporary container.
- **Rust:** never run two cargo test suites at once. **Frontend:** no prettier. Regenerate the client only with `npm run gen:api`.
- **Git:** never commit `.superpowers/`; never override the author; end each commit with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; do not push.
- **Docs:** bilingual, English canonical.

## Gates

- **Backend:**
  - `cargo fmt --check`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
  - `EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --no-fail-fast`
  - `cargo test -p egret-server`
- **Frontend** (from `frontend/`):
  - `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`
  - every `test:bundle*`

---

### Task 1: Backend — real timestamps, list filters, DTO fields

**Files:**
- Create: `migrations/0180_workflow_execution_timestamps.sql`
- Modify:
  - every state-writing site of `workflow.executions` and `workflow.task_runs` (executor, worker, subflow, loop, retry, resume, SLA cancel, replay; enumerate them with grep: `UPDATE workflow.executions`, `UPDATE workflow.task_runs`, `SET state`)
  - `repo.rs`: the list query
  - `routes.rs`: `ListExecutionsQuery` and the DTOs
  - the attempt model: the `Attempt` struct in `executor/`
  - `openapi.json`
  - the generated client
- Test: a new `crates/egret-workflow/tests/execution_timestamps.rs`, plus additions to `tests/executions_routes.rs`

**Interfaces — Produces:**
- `ExecutionListItemDto` and `ExecutionDto` gain `started_at`, `ended_at` and `replayed_from_execution_id`.
- `TaskRunDto` gains `started_at` and `ended_at`.
- Each element of `attempts` gains `ended_at` (milliseconds after the execution's `created_at`).
- The list query gains `root_only: Option<bool>` and `parent: Option<Uuid>`.

- [ ] **Step 1: Enumerate the writers.** List every SQL statement and code path that changes the `state` of an execution or a task run, with `file:line`, in the report. This list is what the timestamp writes must cover.

- [ ] **Step 2: Write the tests first (red).** Cases:
  1. **Simple success.** Execution `started_at` ≤ each run's `started_at`; each run's `ended_at` ≤ the execution's `ended_at`; all non-null.
  2. **Failure.**
  3. **`RETRY_FAILED_TASK`.** Two attempts, each with `started_at < ended_at`; attempt 1's `ended_at` ≤ attempt 2's `started_at`; the run's `started_at` equals attempt 1's start.
  4. **`CREATE_NEW_EXECUTION` retry.** The new execution has its own timestamps.
  5. **Pause and resume.** The run's `ended_at` is set once it finishes; `started_at` does not move on resume; the execution's `ended_at` stays null while paused.
  6. **Subflow.** With `wait: true`, the parent's run ends after the child ends. With `wait: false`, the parent's run ends at once.
  7. **Loop iterations.** Each has its own timestamps.
  8. **SLA cancel.** The execution's `ended_at` is set.
  9. **Replay.** The new execution's timestamps are fresh.
  10. **`SKIPPED` by `runIf`.** `started_at` is null and `ended_at` is set.
  11. **Invariant.** After a terminal row receives a later write (for example an outputs or `updated_at` bump), `ended_at` is unchanged. Every row has `ended_at` null or ≥ `started_at`.
  12. **Coverage guard.** A test that fails when a new state-writing site is added without the timestamp CASE. For example, grep the source for `SET state` in SQL strings and assert each one also mentions `ended_at`, or assert it through a single helper that all writers use. Prefer the helper.
  13. **The list filters.**
      - `root_only=true` excludes loop iterations and subflow children.
      - `parent=<id>` returns exactly the children.
      - A `parent` from another workspace gives an empty page.
      - Sending both filters gives 400.
      - A malformed uuid gives 400.
  14. **The DTO.** It carries `replayed_from_execution_id` for a replay.
  15. **Index.** `EXPLAIN` for `parent=` uses the new index.

- [ ] **Step 3: Implement.**
  - **Migration 0180:** the four columns, plus the two partial indexes `(workspace_id, parent_execution_id) WHERE parent_execution_id IS NOT NULL` and the same for `trigger_execution_id`.
  - **One SQL fragment helper** for the timestamp CASE, used by every writer.
  - **The attempt `ended_at`** is set at the same write that ends the attempt.
  - **No read-then-write anywhere.**

- [ ] **Step 4: Regenerate openapi, run `npm run gen:api` and `npx tsc -b`, then run the backend gates.**

- [ ] **Step 5: Commit.** `feat(workflow): real start and end times for executions, task runs and attempts; root_only and parent list filters`

### Task 2: The executions list and the flow's Executions tab

**Files:**
- Create:
  - `frontend/src/routes/workflows/ExecutionsList.tsx`
  - `frontend/src/routes/workflows/executionsListModel.ts`: URL to filters to query params
  - tests
- Modify:
  - `WorkflowsRoutes.tsx`: replace `ExecutionsPlaceholder`, and add `flows/:id/executions`
  - the flow detail's tab list: add Executions
  - `queryKeys.ts`: `executions(ws, params)`
  - the bundle markers in `assert-workflows-lazy.mjs` for the new modules

- [ ] **Step 1: Write the tests (red).**
  - **The URL model:**
    - state (multi), namespace with include-children, flow, time preset or custom, `child` switch, page;
    - each value round-trips through the URL;
    - `root_only=true` is sent while the switch is off;
    - `since`/`until` are computed from the presets with a fixed clock.
  - **The list:**
    - columns, including the flow link (hidden on the flow tab) and the trigger label plus icon from `execution_trigger.type`;
    - the three states, and empty versus filtered;
    - paging from `total`.
  - **Polling:** only while a row is non-terminal and the document is visible (fake timers plus `visibilitychange`). "Updated Ns ago" and Refresh work.
  - **Timing:** duration uses `started_at`/`ended_at` when present, and the old `created_at`/`updated_at` with an "estimated" title otherwise.

- [ ] **Step 2: Implement, run the gates, and commit.** `feat(workflows): executions list with filters in the URL, root executions by default, and the flow's Executions tab`

### Task 3: The execution detail frame, Overview and the actions

**Files:**
- Create:
  - `ExecutionDetail.tsx`: header, tabs, actions, polling
  - `ExecutionOverview.tsx`
  - `components/workflows/executions/actions.ts`: `canResume` and `canReplay` with reasons
  - tests
- Modify:
  - generalise `nextPollMs` (W7a, `ExecutionMinimal.tsx`) into `components/workflows/executions/polling.ts`
  - remove `ExecutionMinimal` once replaced; W7a's polling tests move with it

- [ ] **Step 1: Write the tests (red).**
  - **Routes:** `executions/:id` redirects to `overview`, and an unknown tab redirects to `overview`.
  - **Header:**
    - id, flow link, namespace, chip, start, duration, trigger;
    - "Replayed from" and "Child of" links when those fields are set.
  - **Polling:** stops at a terminal state. A failed poll keeps data and shows the error with Retry; the W7a cases still pass.
  - **Resume:**
    - shown only while `PAUSED`;
    - disabled with the no-duration-or-timeout reason when the frozen `source`'s pausing task has neither property (find the paused run's task in `source`);
    - confirm dialog, then success refetches the execution.
  - **Replay:**
    - disabled with its reason while not terminal;
    - confirm dialog naming the frozen source and inputs;
    - a 201 navigates to the new id;
    - a 409 or 400 shows the server's message.
  - **Permissions:** without `can_manage` (or the workspace permission signal W7a uses), neither action is shown.
  - **Overview:**
    - an inputs table with file rows (a stub here; the real one comes in Task 6);
    - trigger variables;
    - children fetched with `parent=` (loop index and value);
    - a task-run list linking to `gantt?run=`.

- [ ] **Step 2: Implement, run the gates, and commit.** `feat(workflows): execution detail with header, polling, overview, resume and replay`

### Task 4: The Gantt

**Files:**
- Create:
  - `components/workflows/executions/gantt/ganttModel.ts` (pure)
  - `GanttView.tsx`
  - `ExecutionGanttTab.tsx`
  - tests

**Interfaces — Produces:**
- `buildGantt(execution, now): { axis: {start, end}, rows: GanttRow[] }`
- Each `GanttRow` is `{ runIx, depth, parentIx, taskId, value, state, bar?: {start, end, open}, attempts: {start, end?, state}[], estimated: boolean, marker: boolean }`

- [ ] **Step 1: Write the tests (red), against unsorted fixtures.**
  - Bars use `started_at`/`ended_at`. A run that is not ended is `open` to `now`.
  - Attempt segments come from `attempts[]`, each start and end being execution `created_at` + ms. A retried attempt keeps its own state.
  - A run that never started gets `marker`.
  - **The estimated fallback,** when timestamps are null:
    - start is `created_at` + `attempts[0].started_at`;
    - end is `updated_at` once `ended_seq` is set;
    - `estimated` is true.
  - Nesting depth comes from `parent_ix`. Loop children are labelled by `value`. Collapsing hides the descendants.
  - The axis spans the execution, extending to `now` while it runs.
  - **The view:**
    - rows form a `grid`, with arrow keys, Enter and Escape;
    - the selected run is in `?run=`;
    - the panel shows a placeholder slot for logs and outputs, which Tasks 5 and 6 fill;
    - estimated bars are hatched and carry the note;
    - the redraw is throttled and paused while the tab is hidden.

- [ ] **Step 2: Implement, run the gates, and commit.** `feat(workflows): execution gantt with attempt segments, live bars and run selection`

### Task 5: Logs

**Files:**
- Create:
  - `components/workflows/executions/logs/useLogFollow.ts`
  - `LogView.tsx` (virtualised; use the repo's existing virtualisation library if there is one, otherwise a small windowing component, and say which)
  - `ExecutionLogsTab.tsx`
  - tests
- Modify: the Gantt panel slot, so it uses `LogView` filtered by `task_run_id`

- [ ] **Step 1: Write the tests (red).**
  - **`useLogFollow`:**
    - `after` advances to `next_after`, and a null `next_after` keeps the old cursor;
    - it polls every 2 s while not done, and stops only on `complete && empty`;
    - a terminal state alone does not stop it;
    - a failed poll keeps the lines and exposes the error and a retry;
    - a filter change resets the cursor.
  - **The view:**
    - grouping by task run, with execution-level lines first;
    - level badges with text;
    - attempts shown when there is more than one;
    - whitespace preserved.
  - **Scrolling:** auto-scroll when the user is at the bottom; "Jump to latest" when they have scrolled up.
  - **Load earlier,** for history beyond what is loaded.
  - **Download:** pages through every page with the current filters, and saves the lines.
  - **Filters in the URL** on the Logs tab.
  - **The three states:** "No logs yet" while running versus "No logs".

- [ ] **Step 2: Implement, run the gates (including bundle size: the log view stays in the Workflows chunk), and commit.** `feat(workflows): execution logs that follow by cursor, with filters, grouping and download`

### Task 6: Outputs and files

**Files:**
- Create:
  - `components/workflows/executions/outputs/OutputsTree.tsx`
  - `files/FileRow.tsx`
  - `files/FilePreviewDrawer.tsx`
  - `ExecutionOutputsTab.tsx`
  - tests
- Modify: the Overview inputs file rows and the Gantt panel outputs slot

- [ ] **Step 1: Write the tests (red).**
  - **Detection:** `kestra:///` URIs are found anywhere in outputs and inputs (nested objects and arrays).
  - **The tree:** expand and collapse; copy a value; copy a path as an expression (`{{ outputs.t1.key }}`, and the loop form `outputs.t1[value]`).
  - **File row:** size from `file/metas`, Preview and Download (an `<a href>` to the file route).
  - **Preview drawer:**
    - Ion rows as JSON, text rows as lines;
    - not previewable;
    - truncated, with the row cap;
    - 404 shows "no longer available".

- [ ] **Step 2: Implement, run the gates, and commit.** `feat(workflows): execution outputs tree and file preview`

### Task 7: End to end, the Kestra comparison, and docs

- [ ] **Step 1: Playwright on a private stack.** Use W7a's committed recipe in `frontend/tests/e2e/README.md`, with this branch's Postgres. Never use the shared dev stack. Write `frontend/tests/e2e/workflows-executions.spec.ts`. The flow it runs has:
  - a retried task;
  - a `Pause` with `pauseDuration`;
  - a loop;
  - a log-heavy task;
  - a task writing a file.

  Assert:
  - the list shows the root only, and the switch shows children;
  - the gantt shows two attempt segments for the retried task;
  - logs follow live until the execution completes;
  - selecting a run filters its logs;
  - the file preview shows rows;
  - Resume works on the pause;
  - Replay navigates to a new execution.

- [ ] **Step 2: Measure in the DOM, in light and dark.**
  - the gantt at 1440 and 900 px: bars inside the axis, labels not clipped;
  - log view scrolling and virtualisation with about 10 000 lines;
  - chip and badge contrast.

- [ ] **Step 3: Compare with a temporary Kestra.** Cover:
  - the list's filters and columns;
  - the gantt;
  - the logs;
  - outputs and file preview.

  Fold the deliberate differences into §2 (EN and VI).

- [ ] **Step 4: Update the docs, run every gate, and commit.**
  - Spec §2, §15 and the programme spec's W7 row ("W7b done").
  - Commit: `test(workflows): end-to-end executions; docs: W7b divergences and limits`
