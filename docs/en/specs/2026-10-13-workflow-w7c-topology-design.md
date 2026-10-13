# W7c — Workflows UI, part 3: topology (the flow graph and the execution graph)

**Status:** implemented, 2026-10-13. The end to end (`frontend/tests/e2e/workflows-topology.spec.ts`) ran green on a private stack. The editor's graph followed an edit 1 s after it and kept the last graph under the banner while the YAML was broken; selecting a node moved the Code cursor; the execution's graph showed the `Sleep` RUNNING then SUCCESS while polling, without moving the view, then the final states, and made no read over 32 s after the end; the `Loop` body listed three iterations, one FAILED; the `Subflow` expanded with its child's states. Measured on its 15-task flow: the first view opens at zoom 0.8, the readable floor (§6.1), with the flow's start in view and the first task's id drawn at 10.4 px, horizontally and vertically, in the editor and on the execution. Fit, which shows the whole graph, goes to 0.12–0.20 there; at that zoom every card, cluster and marker stays inside the canvas and no label is cut, but the text is too small to read. The keyboard focus ring measured 3.5:1 (light) and 3.6:1 (dark) against its card. The comparison with Kestra 2.0 nightly is in §2.

**Depends on:**
- W7a (#659): the Workflows area, the multi-panel editor and its tab registry (`registerTab`), the Problems tab's path-to-YAML mapping.
- W7b (#662): the execution detail, its URL tabs, its polling backoff, `StateChip`, and the run panel the Gantt opens.
- no-silent-stall (#663): the validation codes that refuse constructs egret does not run (`unsupported_allow_failure`, `unsupported_working_directory`, …).

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (Kestra's UI as reference: `ui/src/components/executions/Topology.vue`) and §9 row W7. **W7a spec** §5.1 reserved the Topology tab for this package.

**Decided with the user, 2026-10-13:**
- **The whole read-only topology, as Kestra has it.**
  - A Topology tab in the flow editor, drawn from the unsaved YAML.
  - A Topology tab on the execution detail, with live state colours.
  - Collapsible clusters, coloured `errors`/`finally` branches, a trigger cluster, horizontal and vertical layouts, zoom and fit.
  - A `Subflow` expands in place to the child flow's graph.
  - No editing from the graph: no adding, editing or deleting tasks. Kestra's topology editing belongs to its no-code editor, which W7a ruled out.
- **A `Loop` body is one node per task, with the state folded over every iteration.** Kestra's priority applies: FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS. The node shows a count such as "2 of 3 iterations done, 1 failed", and selecting it lists the iterations with links to their child executions. egret runs each iteration as a child execution, and one node per iteration would make the graph grow with the data.
- **The server builds the graph, and the browser draws it with `@xyflow/react` and `@dagrejs/dagre`.** One source of truth for structure: aliases, canonical types, `Dag` dependencies, refused constructs. This is the React counterpart of Kestra's `vue-flow` and `dagre`.

## 1. Why

W7a lets a user write a flow and W7b lets them follow its runs, but neither shows the flow's shape. A flow with a `Dag`, a `Switch` inside a `Parallel` and an `errors:` branch reads as a YAML tree, not as the order things happen in. Kestra users expect a graph next to the code and on every execution, coloured by state.

## 2. Divergences from Kestra

| Kestra | egret | Why |
|---|---|---|
| Edit from the graph: add a task on an edge, edit, delete, add an error branch, run in playground, replay a task | Read-only | No-code editing is out of W7 (W7a ruling); replay of one task does not exist in egret |
| The execution graph follows the execution over SSE | Polling with W7b's backoff | egret's APIs are poll-only |
| A task node folds its several runs into one state on the client (`TaskNode.vue`, `SORT_STATUS`), with no count of iterations and no list of them | One node per task, the state folded over iterations on the server, a count, and an iteration list on selection | egret runs each `Loop` iteration as a child execution; the user's ruling. What Kestra 2.0 draws for a `Loop` body is measured below |
| PNG/JPEG export of the graph | Not built | Not asked for |
| `GET /flows/{ns}/{id}/dependencies` (flow-to-flow graph) | Not in W7c | A different feature; it belongs with namespaces or triggers |

The comparison of §9 adds measured differences found against a running Kestra.

**Measured against a running Kestra, 2026-10-13 (Task 7, §9).** The local image `kestra/kestra` `28f59bf175e1` (Kestra 2.0 nightly, `server local`, in a throwaway container) ran the end to end's flow (§8) with only its ids changed: a disabled `Schedule` trigger, a `Sequential` holding a `Sleep` and an `If`, a `Parallel`, a `Subflow`, a three-value `Loop` whose middle value fails, and an `errors:` branch. Both servers built the same shape: 26 nodes, 28 edges and 5 clusters, and 29, 31 and 6 with the `Subflow` expanded; every node and every edge pairs up. These differences were found. Each was observed on the running image, through its API or its UI, unless the row says it was read in Kestra's source:

| Kestra 2.0 nightly | egret | Why |
|---|---|---|
| No Topology tab on the execution: the graph sits in the Overview tab, under the summary (`executionTabs.ts`: Overview, Gantt, Logs, Input/Output, Metrics, Dependencies, …) | A Topology tab, in Kestra 1.x's order (§6.3) | The user's ruling: a tab of its own, with a URL that can be shared |
| A `Loop` iteration is a sub-execution of kind `LOOP` (its `kind` in the API), and the body node on the parent's graph shows no state: the `item` card showed only its id, with no duration and no "Show the state history" button, which every task that ran had, while the `each` card showed its duration and the API gave `each` a FAILED run | The body node folds its runs: FAILED, "2 of 3 iterations done, 1 failed", and the iteration list | The user's ruling (§4) |
| Uids: `root.prepare.check`, markers `root.prepare.root-{random}` and `root.prepare.end-{random}`, clusters `cluster_root.prepare`; the random part changes on every request | `tasks.prepare.check`, `start:tasks.prepare`, `end:tasks.prepare`, `cluster:tasks.prepare`, the same on every request | A uid is the validation path (§3.1), and a stable uid can be a URL's `?node=` |
| The edges into and out of markers, and the trigger chain, carry no relation (`relationType` null); a branch's edge to the `If`'s end has no label | Every edge has a relation: the list's own (`sequential`, `parallel`, `dynamic`, `error`), and a branch keeps its label to the end (§3.2) | The client colours and labels an edge from its own fields, without looking at its neighbours |
| Every edge is drawn dotted (`stroke-dasharray: 1.5px 3px`); `errors` edges are red | Solid edges; only the trigger chain is dashed; `errors` in the danger token, `finally` and `afterExecution` in the warning token, each with its word | §6.1: dashes mean "a trigger", and a colour is never the only signal (FE-A11Y-4) |
| Compact cards, 137 × 28 px: icon, id, duration, a menu | Cards 240 px wide: icon, id, short type, state chip, duration, iteration count, attempts | The execution's facts are read on the card; the canvas fits and zooms |
| Each node is its own tab stop (`tabindex=0`, `role=group`, no accessible name); no list view | One tab stop moved by the arrow keys, nodes are named buttons, and a List view treegrid carries the same nodes and states | FE-A11Y; the list view is the canvas's accessible equivalent |
| Clicking a node in the editor opens the no-code form of the task, and on the execution, logs and outputs come from a node's menu (read in `LowCodeEditor.vue`: `@edit`, `@show-logs`, `@show-outputs`; the UI showed a "More actions" menu on every node) | Selecting a node moves the Code cursor (§6.2); on the execution it opens the Gantt's run panel or the iteration list (§6.3) | Read-only (W7a ruling) |
| An expansion that cannot happen fails the whole graph request. Exercised on the image with a second flow: `?subflows=root.dyn` (a `flowId` expression) answered 422 "Can't expand subflow task 'dyn' because namespace and/or flowId contains dynamic values", and `?subflows=root.gone` (no such flow) answered 404 "Unable to find subflow"; the same flow's graph without `subflows` answered 200 | The graph is drawn, and the `Subflow`'s cluster carries a `reason` (`not_found`, `dynamic`, `depth`) | One bad expansion should not cost the rest of the graph |
| `?subflows=` takes Kestra's uid (`root.call`) | `?subflows=` takes egret's uid (`tasks.call`) | Follows the uid scheme |
| Orientation in `localStorage` `topology-orientation`, a boolean (read in `LowCodeEditor.vue`); vertical when nothing is stored, as the editor's panel showed, while the execution overview drew horizontally | `egret.workflows.topology.orientation`, `LR` or `TB`; horizontal when nothing is stored | egret keeps one default for both tabs |
| The canvas has zoom in, zoom out, fit, an orientation toggle and an image download | Zoom in, zoom out, fit, orientation, Expand all, Collapse all and List view; no download | Image export is not built (row above) |

## 3. The graph model (backend)

A new module `crates/egret-workflow/src/graph/` holds a pure function:

```rust
pub fn flow_graph(flow: &models::Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError>;
```

It walks the parsed model (`models::Flow`, `models::TaskKind`). It does not walk the executor's plan, because the plan drops what does not run, such as disabled tasks and refused constructs, and the graph must show them.

### 3.1 Nodes

Every node has a `uid`: the task's path in the same form as validation findings use (`tasks.seq.child`, `errors.alert`), so a node and a finding about it name the same place. Inside an expanded `Subflow` (§3.4) the child's uids are prefixed with the `Subflow` node's uid.

**Uids, as ruled 2026-10-13 (Task 1, Q6 and Q7):**
- A task is its validation path; a trigger is `triggers.{id}`.
- Markers are `start:{owner}` and `end:{owner}`, and a cluster is `cluster:{owner}`. The owner is `flow`, `triggers`, or the flowable's own uid. There is no flow cluster, only the `start:flow`/`end:flow` pair. No task or trigger uid can start with `start:`, `end:` or `cluster:`, because every one starts with `tasks.`, `errors.`, `finally.`, `afterExecution.` or `triggers.`.
- A repeated id keeps the plain path the first time; a later one takes the first free suffix of `~2`, `~3`, …, in walk order, and its descendants are built under the suffixed uid. Its `refusal` is looked up under the plain path.
- Inside an expansion a child's uid is `{subflow}/{child}`, for example `tasks.call/tasks.child_log`.

| Node kind | Fields | When |
|---|---|---|
| `task` | `uid`, `task_id`, `type` (as written), `canonical_type`, `kind` (`leaf`, `executable`, `flowable`), `branch` (`tasks`, `errors`, `finally`, `after_execution`), `disabled`, `refusal` (a validation error code, or null) | every task, including flowables, which are also clusters |
| `trigger` | `uid` (`triggers.{id}`), `trigger_id`, `type`, `disabled` | every trigger |
| `start` / `end` | `uid` | one pair for the flow, one pair per cluster |

`refusal` covers the constructs no-silent-stall refuses, so the graph can mark a task egret will not run. **Ruled 2026-10-13 (Task 1):**
- An error of `validate_for_run` belongs to the task node whose uid is the longest prefix of the error's path: the path itself, or a part of it followed by `.`. So an error at a property path (`tasks.p.tasks` for a `Pause`, `tasks.l.checkFrequency.maxIterations` for a `LoopUntil`) belongs to its task.
- A node's `refusal` is the code of the first such error, in validation order.
- Only task nodes own errors. An error no task path prefixes (`namespace`, `inputs.x`) belongs to none.
- A residual ambiguity is accepted: a child whose id is literally a property name of its parent (a child `tasks` under a flowable) takes its parent's property-path error.

### 3.2 Edges

`{source, target, relation, label}`, where `relation` is one of Kestra's seven: `sequential`, `choice`, `parallel`, `dynamic`, `error`, `finally`, `after_execution`.

**Ruled 2026-10-13 (Task 1, Q1 to Q5), as Kestra does it:** a flowable's own node `X` sits inside its cluster. `start:X → X` is `sequential`, the construct's edges leave from `X`, and the flowable's `errors:` leave from `start:X`. A flowable is entered at `start:X` and left at `end:X`.

| Construct | Edges |
|---|---|
| A task list (`Sequential`, `AllowFailure`, `WorkingDirectory`, `LoopUntil`, a `Pause` with `tasks:`) | `sequential`: `X` → first → … → last → `end:X` |
| `Parallel` | `parallel` from `X` to each child and from each child to `end:X` |
| `If` | the first edge of a branch is `choice`, labelled `then` or `else`; its later edges, and its edge to `end:X`, are `sequential` with the same label. An empty branch draws no edge |
| `Switch` | as `If`, labelled with the case key (cases in key order), then `defaults` |
| `Dag` | `dynamic` from `X` to each node without `dependsOn`, one edge per `dependsOn` entry that names a drawn node, and from each node nothing drawn depends on to `end:X` |
| `Loop` | `dynamic` on every edge from `X` through the body to `end:X` |
| `errors:` of a flowable | `error` on every edge, from `start:X`; the last error goes to `end:X` |
| The flow | `start:flow` → tasks → `finally` → `afterExecution` → `end:flow`. The flow's `errors:` leave `start:flow`, and the last one goes to the first `finally` task, else the first `afterExecution` task, else `end:flow`. The edge into the first `finally` or `afterExecution` task carries that list's relation; every other edge carries the relation of the list it leaves; an error chain is always `error` |
| Triggers | Kestra's chain: `start:triggers` → each trigger → `end:triggers` → `start:flow`, all `sequential`. The client draws an edge dashed when an endpoint is in `cluster:triggers` |

An empty list draws no edge at any level.

### 3.3 Clusters

Every flowable is a cluster: `{uid, task_id, kind, nodes, parent, start, end}`, whose `nodes` hold the flowable's own node (§3.2) and its markers. `kind` is the canonical flowable name: `sequential`, `parallel`, `if`, `switch`, `dag`, `loop`, `loop_until`, `allow_failure`, `working_directory`, or `pause` when a `Pause` has `tasks:`. Triggers form one cluster, `triggers`, as in Kestra.

`AllowFailure` and `WorkingDirectory` are drawn as clusters, as Kestra draws them, and carry their `refusal`.

### 3.4 Subflow expansion

`GraphOptions.subflows` is a set of `Subflow` node uids to expand.

- For each one, the server loads the child flow's current draft by `namespace` and `flowId` in the same workspace, builds its graph with the same options, one level deeper, and splices it in as a cluster of kind `subflow` under the `Subflow` node: `cluster:S` holds `start:S`, the `Subflow` node `S`, the child's top-level nodes (uids `{S}/{child}`) and `end:S`, and its parent is the cluster that held `S`. A child task under the parent's `errors` or `finally` takes that branch, as in Kestra.
- An expansion that cannot happen becomes a cluster with no nodes and a `reason`:
  - `not_found`: no such flow in this workspace;
  - `dynamic`: `namespace` or `flowId` is an expression;
  - `depth`: expansion is limited to two levels.

  Such a cluster draws no markers: its `start` and `end` are the `Subflow` node's own uid, and the node stays where it was.
- There is no `forbidden` reason. A `Subflow` resolves only within its own workspace, and `workflow:read` is a workspace permission, so a viewer who can read the parent can read the child.

On the execution graph (§4), a `Subflow` node whose run started a child execution expands from that child's frozen source and carries its states.

### 3.5 Limits

A graph over 5,000 nodes, counted after expansion, is refused with 422 and a message naming the count. No corpus flow is near it; the limit only stops a pathological flow from building a huge response.

## 4. Execution states

`GET …/executions/{id}/graph` builds the graph from the execution's frozen `source`, which is the source it ran, and adds `states`, a map from node `uid` to:

```json
{
  "state": "RUNNING",
  "attempts": 2,
  "run_id": "…",
  "started_at": "…",
  "ended_at": null,
  "iterations": {
    "total": 3,
    "by_state": { "SUCCESS": 2, "FAILED": 1 },
    "runs": [{ "loop_index": 0, "state": "SUCCESS", "child_execution_id": "…", "run_id": "…" }]
  }
}
```

- **A task with one run** takes that run's state, attempt count and timestamps.
- **A task in a `Loop` body** has runs in several child executions, the ones whose `parent_execution_id` is this execution with a `loop_index`. Its `state` is the fold of those runs by Kestra's priority, and `iterations` counts them by state. `run_id` is null.
  - **`iterations.runs`** (ruled 2026-10-13, Task 6) lists the node's OWN state in each iteration, one row per `loop_index` it ran in: its runs there folded, the iteration's child execution and the run. It is not the iteration's state, which another body task can decide. An iteration where the node never ran has no row, and a row's `run_id` is null when a nested `Loop` ran the node several times in it. The iteration list of §6.3 is drawn from it, with no cap.
  - **`attempts`** of a fold is the most any one run took, not a total, so the client says "up to n attempts".
  - **Nested `Loop`s** count the outer loop's iterations: an iteration runs the outermost body as its own `tasks:`.
- **A `Subflow` with one run** names the child execution it started in `child_execution_id` (the newest, when a retry started several), so the panel can link to it. The field is absent on every other node and on a fold.
- **A task with no run** has no entry. The client draws it dimmed.
- **A flowable cluster** takes its own run's state when it has one.

States follow W7b's vocabulary and `StateChip`.

**The fold's order, as ruled 2026-10-13 (Task 3).** Kestra's `SORT_STATUS` in `ui/packages/topology/src/nodes/TaskNode.vue` is FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS, RESTARTED, CREATED, but its code does not apply it: it indexes the second character of the state string (`TaskNode.vue:262-268`), so every index is -1 and the sort ends up alphabetical. egret follows the stated priority and places its own states by meaning: FAILED, KILLED, CANCELLED (a terminal that stopped the run, next to KILLED), WARNING, SKIPPED, RETRIED (a terminal left by `CREATE_NEW_EXECUTION`), PAUSED (live and waiting on a person, in KILLING's place, which egret does not have), RETRYING, RUNNING, SUBMITTED, SUCCESS, RESTARTED, CREATED.

## 5. Routes

All are under `/api/ws/{workspace}/workflows/`, gated by the workflows feature, and documented in `openapi.json`. The client is regenerated with `npm run gen:api`.

| Route | Permission | Answers |
|---|---|---|
| `GET flows/{id}/graph?version=&subflows=` | `workflow:read` | the draft's graph, or a version's |
| `POST flows/graph?subflows=`, body `{ "source": "…" }` | `workflow:manage`, as the validate route | the graph of unsaved YAML |
| `GET executions/{id}/graph?subflows=` | `workflow:read` | the execution's graph with `states` |

**Errors:**
- YAML that does not parse: 400, with its line and column.
- A flow, version or execution of another workspace: 404.
- More than 5,000 nodes: 422.

A YAML that parses but fails validation still gets its graph: the refused tasks carry `refusal`.

## 6. Frontend

### 6.1 `TopologyView`

One component under `frontend/src/components/workflows/topology/`, shared by both tabs.

**Drawing and layout:**
- It is drawn with `@xyflow/react` and laid out by `@dagrejs/dagre`. Both are pinned exactly and loaded only in the Topology chunk; `test:bundle:workflows` gains a marker so neither reaches the flow list's chunk.
- A pure `layout(graph, { orientation, collapsed })` returns node positions and sizes and is unit-tested on its own.
- **Ruled 2026-10-13 (Task 4):** one dagre layout per container, innermost first, so a cluster reserves its room in its parent's layout. Above 300 members in a container, or when dagre throws (a 2,000-task chain overflows its stack), that container falls back to a non-recursive layered layout. A layout that still fails shows an alert, and the list view still works without one.
- Nodes are not draggable and not connectable.

**A task node** shows the type's icon, `task_id` and the short type. On the execution graph it also shows its `StateChip`, its duration and, for a `Loop` body, the iteration count.

**Marking:**
- Branch colours use OKLCH tokens: `errors` in the danger token, `finally` and `afterExecution` in the warning token.
- `choice` edges carry their label. Trigger edges are dashed.
- A task with `refusal` gets a hatched border and the note "egret does not run this task" with its code. A disabled task is dimmed and says so.

**Controls:**
- Zoom in, zoom out and fit. Fit shows the whole graph, down to the canvas's 0.1 minimum zoom.
- **The first view** (amended 2026-10-13, Task 7). A fit below 0.8 is not readable: the mono task id is 13 px, drawn at 10.4 px at 0.8, and the end to end's 15-task flow fits at 0.12 to 0.20. So when the fit would go below 0.8, the first view opens at 0.8 on the flow's `start:flow` and its first task, the first card of its `tasks:` list (amended again, fix round 2). Along the layout's direction it starts at the graph's leading edge when the first task still fits in the canvas from there, so a trigger chain stays on screen; otherwise it starts just before `start:flow`, leaving the triggers off-screen to the left (or above). Across, it centres the start and the first task. Otherwise the first view is the fit. It is placed when the first graph arrives and again for another flow or execution, another orientation or another set of collapsed clusters, never for a poll's states, an edit's graph of the same flow or a `Subflow` expansion, so the user's pan and zoom stay.
- An orientation toggle, horizontal or vertical, remembered in `localStorage`.
- "Expand all".
- Clusters collapse and expand.
- A `Subflow` node has an Expand action that refetches with `subflows`.

**Keyboard and screen readers:**
- Every node is a focusable button. Arrow keys move along the graph's order, and Enter opens the node.
- A "List view" toggle replaces the canvas with a treegrid of the same nodes, nested by cluster, with their states. This is the accessible equivalent of the canvas.

**States:**
- Loading shows a skeleton (FE-STATE-2).
- An empty flow says it has no tasks.
- An error keeps the last graph and shows why.

### 6.2 The editor tab

- **Registration** (amended 2026-10-13, Task 5). `registerTab({ id: "topology", … })` registers the tab with the editor's other tabs, as a stub whose body loads lazily in the Topology chunk. A stored layout that names `topology` always finds it registered, and xyflow is not loaded on every editor open. The editor frame does not change (W7a §5.1).
- **Read-only viewers.** A viewer without `workflow:manage` cannot use `POST flows/graph` and cannot edit, so their tab draws the saved draft through `GET flows/{id}/graph`.
- **Updates.** The tab sends `POST flows/graph` with the editor's current YAML, 1 s after the last edit.
- **Errors.** While the YAML does not parse, it keeps the last graph and shows a banner, "The graph is out of date: the YAML has an error".
- **Selection.** Selecting a node moves the Code tab's cursor to the task, through the same path-to-range mapping the Problems tab uses.

### 6.3 The execution tab

- **The tab.** It is added to W7b's URL tabs in Kestra's order: Overview, Topology, Gantt, Logs, Outputs. It is a path segment, `…/executions/{id}/topology`, as W7b's tabs are (amended 2026-10-13, Task 6); the selection is `?node=` and the expanded Subflows `?expand=`.
- **Updates.** It refetches the execution graph on W7b's polling backoff and stops when the execution is terminal.
- **Selection.** Selecting a task node opens the same run panel the Gantt opens, with the run's logs and outputs. Selecting a `Loop` body node opens the panel with the list of iterations, each with its state and a link to its child execution.

## 7. What W7c does not do

- Editing from the graph (§2).
- Flow-to-flow dependency graphs.
- A minimap and image export.
- Live updates beyond W7b's polling.

## 8. Tests

**Rust:**
- `flow_graph` for every flowable: node and edge counts, relations, `choice` labels, `Dag` dependency edges, the `errors`, `finally` and `afterExecution` branches, the trigger cluster, `refusal` on refused constructs, and disabled tasks.
- A sweep over the 345 flows of `oracle-exec/corpus`: every flow that parses builds a graph without panicking, and every task of the executor's plan has a node.
- The routes:
  - permissions;
  - 404 for another workspace;
  - 400 with the line for YAML that does not parse;
  - the 5,000-node limit.
- Subflow expansion: success, `not_found` (including a flow of the same name in another workspace), `dynamic`, `depth`.
- Execution states: a retried task, a `Pause`, a `Loop` folded over child executions, a task that never ran, and the fold's priority order.

**Vitest**, on real components with fixtures that can fail:
- `layout`: both orientations, collapsed clusters, an empty graph.
- Nodes: state, `refusal` and disabled marking.
- The editor tab: the debounce, keeping the last graph on a parse error, selection moving the Code cursor.
- The execution tab: polling stops when terminal; selection opens the run panel and the iteration list.
- Keyboard navigation and the list view.

**Playwright,** on a private stack, never the shared dev stack:
1. Create a flow with a `Sequential`, an `If`, a `Parallel`, a `Loop`, an `errors:` branch and a `Subflow`.
2. Open Topology and edit the YAML: the graph follows.
3. Run the flow: the graph's states change while it runs.
4. Select a `Loop` body node and see its iterations; expand the `Subflow`.

DOM measurements cover light and dark mode and a narrow panel.

## 9. Comparing with Kestra

The same flow runs on the local Kestra image. Its topology, in the editor and on the execution, is compared with egret's, and every deliberate difference is recorded in §2.

**Done, 2026-10-13 (Task 7).** The comparison ran against the local image `28f59bf175e1` (Kestra 2.0 nightly) in a throwaway container `w7c-kestra` with its own embedded storage, removed afterwards. Kestra's side was read through its API (`GET /api/v1/main/flows/{ns}/{id}/graph`, with and without `subflows`, and the execution's graph) and its UI, driven by a Playwright script. Every difference found is in §2.

## 10. Rules this package must meet

- The frontend rulebook, every rule; in particular FE-ROUTE-2 (a lazy area with a bundle test), FE-STATE-2 (skeletons), FE-A11Y (focusable nodes, the list view) and FE-DATA-2 (generated client).
- The two new dependencies are pinned exactly, and `package-lock.json` changes only for them.

## 11. Known limits

- A `Subflow` expands to the child flow's current draft in the editor, not to the version that would run. Every execution runs the saved draft, so the two are the same once the child is saved.
- The execution graph re-reads every run on each poll, and its payload is O(body runs): `iterations.runs` lists every iteration of every body node and is sent again on each poll. A flow with thousands of loop iterations makes that read and that answer heavier; the 5,000-node limit does not bound iterations, because iterations fold into one node.
- A `Subflow` inside a `Loop` body expands, on the execution graph, from the child flow's current draft with no states: its runs are in each iteration's own child execution.
- Fit shows the whole graph and may draw it too small to read (0.12 to 0.20 for a 15-task flow in Task 7's measurements). The first view is readable instead (§6.1), and the list view reads at any size.
- An edit in the editor and a `Subflow` expansion keep the user's view and do not refit, so a new node, or an expanded child flow, can land off the canvas until the user presses Fit. Collapsing or expanding a cluster, or changing the orientation, places the first view again: those redraw the whole layout at the user's request.
- Ranks are not aligned across sibling clusters: each container is laid out on its own (§6.1), so two parallel clusters can place their members at different depths.
