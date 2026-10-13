# W7c — Workflows UI part 3 (topology): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read-only graph of a flow, in the editor (from the unsaved YAML) and on every execution (with live state colours), built by the server and drawn by the browser.

**Architecture:**
- **Backend** (`crates/egret-workflow`, no migration):
  - a pure `graph::flow_graph(&Flow, &GraphOptions)` over the parsed model;
  - an execution-state overlay that folds `Loop` iterations;
  - `Subflow` expansion;
  - three routes.
- **Frontend** (inside W7a's lazy Workflows area, in a Topology chunk of its own):
  - a shared `TopologyView` on `@xyflow/react` with a pure dagre `layout`;
  - an editor tab registered through `registerTab`;
  - an execution tab in W7b's URL tabs.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), React 19 + TypeScript, TanStack Query 5, orval, `@xyflow/react`, `@dagrejs/dagre`, vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-13-workflow-w7c-topology-design.md`. **§n refers to the spec.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes and test cases to verify against the code, not text to paste. Names were read at `8a42b800`. Re-read every fact this plan states about the code before relying on it.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every frontend task review answers its reviewer checklist.
- **User decisions (spec header, 2026-10-13)**, in effect verbatim:
  - The whole read-only topology, as Kestra has it:
    - an editor tab drawn from the unsaved YAML;
    - an execution tab with live state colours;
    - collapsible clusters, coloured `errors`/`finally` branches, a trigger cluster, both orientations, zoom and fit;
    - `Subflow` expansion in place;
    - no editing from the graph.
  - A `Loop` body is one node per task, its state folded over every iteration by Kestra's priority (FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS), with an iteration count and, on selection, a list of iterations linking to their child executions.
  - The server builds the graph; the browser draws it with `@xyflow/react` and lays it out with `@dagrejs/dagre`.
- **Earlier rulings still hold:**
  - every execution runs the saved draft (W7a);
  - W7b's state vocabulary, `StateChip` and polling backoff;
  - no-silent-stall's refusal codes.
- **Exact values:**
  - node uid = the task's validation path (`tasks.seq.child`), prefixed by the `Subflow` node's uid inside an expansion;
  - the relation names `sequential`, `choice`, `parallel`, `dynamic`, `error`, `finally`, `after_execution`;
  - expansion reasons `not_found`, `dynamic`, `depth`;
  - expansion depth 2;
  - node limit 5,000 (422 above it);
  - editor debounce 1 s;
  - the orientation's `localStorage` key `egret.workflows.topology.orientation`.
- **Routes** under `/api/ws/{workspace}/workflows/`:
  - `GET flows/{id}/graph?version=&subflows=` (`workflow:read`);
  - `POST flows/graph?subflows=` with body `{source}` (`workflow:manage`);
  - `GET executions/{id}/graph?subflows=` (`workflow:read`).
- **Dependencies:** `@xyflow/react` and `@dagrejs/dagre`, pinned exactly; `package-lock.json` changes only for them. Regenerate the client only with `npm run gen:api`. No prettier.
- **Environment:**
  - Never touch the shared dev DB (:5432), shared buckets, `docker-compose.dev.yml` services, or a kind cluster named `egret`.
  - Database tests run on this branch's Postgres container (`egret-w7c-postgres`, port 5444), created by the controller.
  - Never run two cargo or vitest suites at once; wait while `pgrep -x cargo` is non-empty.
  - Commit before a long suite.
  - Never `git add -A` or `commit -a`; add files by name; never commit `.superpowers/`.
  - Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Gates

- **Backend:**
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --no-fail-fast`, with `DATABASE_URL` set to the branch DB and `EGRET_ALLOW_DB_SKIP` unset
  - `cargo test -p egret-server`
- **Frontend** (from `frontend/`):
  - `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`
  - every `test:bundle*`

Count tests from the full output, not from a `tail`.

---

### Task 1: The graph model (§3.1–§3.3, §3.5)

**Files:**
- Create: `crates/egret-workflow/src/graph/mod.rs` (model types and `flow_graph`), plus submodules as the code argues.
- Modify: `crates/egret-workflow/src/lib.rs` (the module).
- Test: unit tests in the module, and a corpus sweep beside the existing corpus tests in `src/executor/mod.rs`.

**Interfaces — Produces:**

```rust
pub struct GraphOptions { pub subflows: BTreeSet<String>, pub depth: u8 }
pub struct FlowGraph { pub nodes: Vec<Node>, pub edges: Vec<Edge>, pub clusters: Vec<Cluster> }
pub enum NodeKind { Task { task_id, r#type, canonical_type, kind: TaskNodeKind, branch: Branch, disabled: bool, refusal: Option<String> },
                    Trigger { trigger_id, r#type, disabled: bool }, Start, End }
pub struct Node { pub uid: String, #[serde(flatten)] pub kind: NodeKind }
pub struct Edge { pub source: String, pub target: String, pub relation: Relation, pub label: Option<String> }
pub struct Cluster { pub uid: String, pub task_id: Option<String>, pub kind: ClusterKind, pub nodes: Vec<String>,
                     pub parent: Option<String>, pub start: String, pub end: String, pub reason: Option<ExpansionReason> }
pub fn flow_graph(flow: &Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError>; // GraphError::TooLarge { nodes }
```

All are `Serialize` and `utoipa::ToSchema`, with `snake_case` enums. `flow_graph` performs no expansion in this task: `opts.subflows` is honoured in Task 2 through a loader seam.

- [ ] **Step 1: Read the model.** Read `models::{Flow, Task, TaskKind}` and each flowable struct, how `spec::validate_for_run` builds finding paths (`walk_task`, including `error_children()`), and `models::canonical_flow_type`. In the report, list the path form for each position: `tasks`, nested lists, `errors`, `finally`, `afterExecution`, `Dag` nodes, `Switch` cases, `If` then/else.

- [ ] **Step 2: Write the tests (red).**
  1. **One test per construct in §3.2's table.** Each asserts:
     - the exact node uids;
     - the exact edge set as `(source, target, relation, label)`;
     - the cluster's `kind`, `nodes`, `parent`, `start` and `end`.
  2. **The specific shapes:**
     - `If` with an empty `else`;
     - `Switch` with `defaults`;
     - a `Dag` with a diamond `dependsOn`;
     - a `Parallel` nested in a `Sequential`'s `errors:`;
     - the flow's `finally:` and `afterExecution:`;
     - triggers of each kind, including a disabled one.
  3. **Disabled tasks** appear with `disabled: true` and keep their edges.
  4. **Refusals.** `refusal` carries the code of the `validate_for_run` error at the task's own path: `unsupported_allow_failure` on an `AllowFailure` cluster, `unknown_task_type` on a typo'd leaf, and null on a valid task.
  5. **The node limit.** A generated flow over 5,000 nodes gives `GraphError::TooLarge`.
  6. **The corpus sweep.** For every corpus flow that parses: `flow_graph` returns `Ok`, and every task id the executor's plan holds (`executor::plan_stored` where it plans) appears as exactly one node. Give it a population assertion (the number of flows judged) so it cannot pass by judging nothing.

- [ ] **Step 3: Implement.** Write the graph module, deriving uids from the same path logic validation uses. Do not keep a second spelling of it: extract a shared helper if needed, and say where it lives.

- [ ] **Step 4: Gates, then commit.** `feat(workflow): the flow graph model`

### Task 2: Flow graph routes and Subflow expansion (§3.4, §5)

**Files:**
- Modify:
  - `crates/egret-workflow/src/graph/` (expansion through a loader trait);
  - `crates/egret-workflow/src/routes.rs` (two routes);
  - `openapi.json`;
  - the generated client (`npm run gen:api`).
- Test: a new `crates/egret-workflow/tests/graph_routes.rs`.

**Interfaces:**
- Consumes: Task 1's types.
- Produces:
  - `trait SubflowSource { async fn current_draft(&self, ws, namespace, flow_id) -> Result<Option<String>> }`, plus a Postgres implementation over the flow repo;
  - the routes `GET flows/{id}/graph` and `POST flows/graph`;
  - generated client functions for both.

- [ ] **Step 1: Write the tests (red).**
  1. **`GET` the draft graph,** and `?version=n`: equal to `flow_graph` over the stored source.
  2. **`POST`** with unsaved YAML returns its graph. YAML that does not parse answers 400 whose message carries the line and column. YAML that parses but fails validation answers 200, with the refusals set.
  3. **Permissions:** `GET` needs `workflow:read`; `POST` needs `workflow:manage`.
  4. **404** for a flow or version of another workspace.
  5. **422** over the node limit.
  6. **Subflow expansion:**
     - success: the child's nodes are prefixed and sit in a `subflow` cluster under the `Subflow` node;
     - `not_found` (including a flow of the same name in another workspace);
     - `dynamic`;
     - `depth` at the third level;
     - several uids at once;
     - an unknown uid in `subflows` is ignored.
  7. **Mutations:** remove the workspace scope from the loader, and remove the depth check. Record which tests fail.

- [ ] **Step 2: Implement,** then regenerate `openapi.json` the way the repo does and run `npm run gen:api` in `frontend/`.

- [ ] **Step 3: Gates (backend, plus `npx tsc -b` in `frontend/`), then commit.** `feat(workflow): flow graph routes with Subflow expansion`

### Task 3: The execution graph (§4, §5)

**Files:**
- Modify:
  - `crates/egret-workflow/src/graph/` (a `states` overlay);
  - `crates/egret-workflow/src/routes.rs` (`GET executions/{id}/graph`);
  - `openapi.json`;
  - the generated client.
- Test: `tests/graph_routes.rs`.

**Interfaces — Produces:**
- `ExecutionGraph { graph: FlowGraph, states: BTreeMap<String, NodeState> }`
- `NodeState { state, attempts, run_id, started_at, ended_at, iterations: Option<Iterations { total, by_state }> }`
- `fn fold_state(states: impl Iterator<Item = RunState>) -> RunState`, following Kestra's priority.

- [ ] **Step 1: Read** how W7b's routes read runs, attempts, `started_at`/`ended_at` and loop children (`parent_execution_id`, `loop_index`). Read how a `Subflow` run names its child execution.

- [ ] **Step 2: Write the tests (red).**
  1. A finished execution: each run's state, attempts and timestamps sit on its node.
  2. A retried task: `attempts` is 2.
  3. A `Pause` waiting: `PAUSED`.
  4. A task that never ran has no entry.
  5. **A `Loop` of 3 values with one failing iteration:**
     - each body node folds to `FAILED`, with `iterations: {total: 3, by_state: {SUCCESS: 2, FAILED: 1}}`;
     - `run_id` is null.
  6. **`fold_state`:** a table over Kestra's priority, including RUNNING with SUCCESS giving RUNNING.
  7. **Expanding a `Subflow`** on an execution uses the child execution's frozen source and carries its states.
  8. **The source used** is the execution's frozen `source`, not the flow's current draft: change the draft after the run and the graph does not move.
  9. 404 for another workspace's execution.
  10. **Mutation:** fold by "last run wins". Test 5 fails.

- [ ] **Step 3: Implement, regenerate the client, run the gates, then commit.** `feat(workflow): execution graph with folded run states`

### Task 4: `TopologyView` (§6.1)

**Files:**
- Create: `frontend/src/components/workflows/topology/`:
  - `layout.ts` (pure);
  - `TopologyView.tsx`;
  - node components;
  - `TopologyListView.tsx`;
  - tests.
- Modify:
  - `frontend/package.json` and `package-lock.json` (the two dependencies, pinned);
  - `frontend/scripts/assert-workflows-lazy.mjs` (a marker for the topology chunk).
- Test: `frontend/src/components/workflows/topology/__tests__/`.

**Interfaces:**
- Consumes: the generated `FlowGraph` and `ExecutionGraph` types.
- Produces:
  - `layout(graph: FlowGraph, opts: { orientation: "LR" | "TB"; collapsed: ReadonlySet<string> }): { nodes: Positioned[]; edges: RoutedEdge[] }`;
  - the component:

    ```tsx
    <TopologyView
      graph
      states?
      selectedUid?
      onSelect(uid)
      onExpandSubflow(uid)
      busy?
      error?
      stale?
    />
    ```

- [ ] **Step 1: Write the tests (red).**
  1. **`layout`:**
     - both orientations put the flow's `start` first along the main axis;
     - a collapsed cluster hides its nodes and reroutes their edges to the cluster;
     - an empty graph lays out without throwing.
  2. **Nodes:**
     - a task node shows its icon, `task_id` and short type;
     - with `states`, it shows `StateChip`, the duration and the iteration count;
     - a task with no state is dimmed;
     - `refusal` shows the note and the code;
     - `disabled` is dimmed and labelled.
  3. **Branch colours** come from tokens (no literal colours). `choice` edges show their label; trigger edges are dashed.
  4. **Controls:** zoom in and out, fit, orientation (persisted under the exact key), expand all, collapsing a cluster, and Expand on a `Subflow` calling `onExpandSubflow`.
  5. **Keyboard:** nodes are focusable buttons; arrows move along layout order; Enter calls `onSelect`.
  6. **List view:**
     - the toggle shows a treegrid nested by cluster, carrying the same states;
     - `role="treegrid"` with the ARIA W7b's Gantt uses.
  7. **States:** skeleton while `busy`; the empty-flow message; an `error` keeps the last graph and shows the message; `stale` shows the banner text of §6.2.

- [ ] **Step 2: Implement.**
  - Add the two dependencies pinned exactly. Run `npm ci` to confirm the lock is consistent.
  - xyflow's stylesheet is imported inside the chunk only.
  - Add the marker to `assert-workflows-lazy.mjs`, proven by a scratch eager import that fails the guard (record it).

- [ ] **Step 3: Frontend gates, then commit.** `feat(workflows): TopologyView with dagre layout, keyboard and list view`

### Task 5: The editor's Topology tab (§6.2)

**Files:**
- Create: `frontend/src/components/workflows/topology/TopologyTab.tsx`, and a registration in the Topology chunk.
- Modify: the editor's lazy registration point (read `registerEditorTabs.tsx` and the W7a spec §5.1 on late registration) so the tab registers when its chunk loads.
- Test: `TopologyTab.test.tsx`.

**Interfaces — Consumes:**
- `registerTab` (`components/workflows/panels/registry.ts`);
- `EditorContext` (`source`, `focusRange`);
- `rangeOfPath(source, path)` (`editor/yamlPath.ts`);
- the generated `POST flows/graph` client.

- [ ] **Step 1: Write the tests (red).**
  1. **Debounce:**
     - an edit sends no request within 1 s;
     - one request goes 1 s after the last of several edits;
     - the request carries the current source.
  2. **A 400** keeps the previous graph and shows "The graph is out of date: the YAML has an error".
  3. **Selecting a node** calls `ctx.focusRange(rangeOfPath(source, uid))`.
  4. **Expanding a `Subflow`** refetches with `subflows`, and the expanded set survives an edit.
  5. **Registration:**
     - the tab appears in the editor's tab menu;
     - a stored layout naming `topology` restores it (W7a's late-registration rule).

- [ ] **Step 2: Implement, run the frontend gates (the workflows bundle test included), then commit.** `feat(workflows): Topology tab in the flow editor`

### Task 6: The execution's Topology tab (§6.3)

**Files:**
- Create: `frontend/src/components/workflows/executions/topology/ExecutionTopologyTab.tsx`, and an iteration list component.
- Modify:
  - `frontend/src/routes/workflows/ExecutionDetail.tsx` (`EXECUTION_TABS` gains `topology` after `overview`);
  - the Gantt's run panel only if it must be shared; prefer reusing `GanttRunPanel` as it is.
- Test: `ExecutionTopologyTab.test.tsx` and the `ExecutionDetail` tab tests.

**Interfaces — Consumes:**
- `nextExecutionPollMs` / `executionFinished` (`executions/polling.ts`);
- `GanttRunPanel`;
- the generated `GET executions/{id}/graph` client.

- [ ] **Step 1: Write the tests (red).**
  1. **The tab:**
     - the order is Overview, Topology, Gantt, Logs, Outputs;
     - `?tab=topology` opens it;
     - an unknown tab still redirects as in W7b.
  2. **Polling:**
     - polling follows W7b's backoff while the execution runs;
     - no request is sent after it is terminal.
  3. **Selecting a task node** opens the run panel with that run's logs and outputs, the same panel the Gantt opens.
  4. **Selecting a `Loop` body node** opens the iteration list: one row per iteration with its state, each linking to its child execution's page.
  5. **Expanding a `Subflow`** on the execution shows the child execution's states.

- [ ] **Step 2: Implement, run the frontend gates, then commit.** `feat(workflows): Topology tab on the execution detail`

### Task 7: End to end, the Kestra comparison, and docs

**Files:**
- Create: `frontend/tests/e2e/workflows-topology.spec.ts`.
- Modify: the spec (EN and VI): status line, §2 with measured differences, and §11 if needed.

- [ ] **Step 1: Private stack** per `frontend/tests/e2e/README.md`, never the shared dev stack or a kind cluster named `egret`. The run:
  1. Create a flow with a `Sequential`, an `If`, a `Parallel`, a `Loop` over three values (one failing), an `errors:` branch and a `Subflow` to a second flow.
  2. Open Topology and edit the YAML: the graph follows after the debounce; break the YAML: the banner shows and the last graph stays.
  3. Select a node: the Code cursor moves.
  4. Run the flow: the execution's Topology shows states changing while it runs, and the final states once it finishes.
  5. Select the `Loop` body node: three iterations, one failed. Expand the `Subflow`.
  6. Measure the DOM in light and dark mode and in a panel 600 px wide: nodes stay inside the canvas, labels are not clipped, and focus rings are visible.
- [ ] **Step 2: Kestra comparison.** Run the same flow on the local Kestra image (`kestra/kestra`, as W7b §11 did) and compare both topologies. Record every difference in spec §2, in EN and VI.
- [ ] **Step 3: Gates, then commit.** `test(workflows): end-to-end topology; docs: W7c divergences`
