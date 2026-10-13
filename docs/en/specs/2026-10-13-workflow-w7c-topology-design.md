# W7c — Workflows UI, part 3: topology (the flow graph and the execution graph)

**Status:** design, 2026-10-13.

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
| One node per iteration in the execution graph (backend), folded to one state by the task node (frontend) | One node per task, the state folded over iterations, a count, and an iteration list on selection | egret runs each `Loop` iteration as a child execution; the user's ruling |
| PNG/JPEG export of the graph | Not built | Not asked for |
| `GET /flows/{ns}/{id}/dependencies` (flow-to-flow graph) | Not in W7c | A different feature; it belongs with namespaces or triggers |

The comparison of §9 adds measured differences found against a running Kestra.

## 3. The graph model (backend)

A new module `crates/egret-workflow/src/graph/` holds a pure function:

```rust
pub fn flow_graph(flow: &models::Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError>;
```

It walks the parsed model (`models::Flow`, `models::TaskKind`). It does not walk the executor's plan, because the plan drops what does not run, such as disabled tasks and refused constructs, and the graph must show them.

### 3.1 Nodes

Every node has a `uid`: the task's path in the same form as validation findings use (`tasks.seq.child`, `errors.alert`), so a node and a finding about it name the same place. Inside an expanded `Subflow` (§3.4) the child's uids are prefixed with the `Subflow` node's uid.

| Node kind | Fields | When |
|---|---|---|
| `task` | `uid`, `task_id`, `type` (as written), `canonical_type`, `kind` (`leaf`, `executable`, `flowable`), `branch` (`tasks`, `errors`, `finally`, `after_execution`), `disabled`, `refusal` (a validation error code, or null) | every task, including flowables, which are also clusters |
| `trigger` | `uid` (`triggers.{id}`), `trigger_id`, `type`, `disabled` | every trigger |
| `start` / `end` | `uid` | one pair for the flow, one pair per cluster |

`refusal` is the code of the first `validate_for_run` error whose path is the task's path. It covers the constructs no-silent-stall refuses, so the graph can mark a task egret will not run.

### 3.2 Edges

`{source, target, relation, label}`, where `relation` is one of Kestra's seven: `sequential`, `choice`, `parallel`, `dynamic`, `error`, `finally`, `after_execution`.

| Construct | Edges |
|---|---|
| A task list (`tasks`, `Sequential`, `AllowFailure`, `WorkingDirectory`, `LoopUntil`, a `Pause` with `tasks:`) | `sequential`, in order, from the cluster's `start` to its `end` |
| `Parallel` | `parallel` from `start` to each child and from each child to `end` |
| `If` | `choice`, labelled `then` and `else` |
| `Switch` | `choice`, labelled with the case key, and `defaults` for the default list |
| `Dag` | `dynamic` from `start` to each node without `dependsOn`, one edge per `dependsOn` entry, and from each node nothing depends on to `end` |
| `Loop` | `dynamic` from `start` through the body to `end` |
| `errors:` of the flow or of a flowable | `error`, from the owner's `start` to the error list |
| `finally:` and `afterExecution:` of the flow | `finally` and `after_execution`, after the main list |
| Triggers | `sequential` from each trigger to the flow's `start`, drawn dashed by the client |

### 3.3 Clusters

Every flowable is a cluster: `{uid, task_id, kind, nodes, parent, start, end}`. `kind` is the canonical flowable name: `sequential`, `parallel`, `if`, `switch`, `dag`, `loop`, `loop_until`, `allow_failure`, `working_directory`, or `pause` when a `Pause` has `tasks:`. Triggers form one cluster, `triggers`, as in Kestra.

`AllowFailure` and `WorkingDirectory` are drawn as clusters, as Kestra draws them, and carry their `refusal`.

### 3.4 Subflow expansion

`GraphOptions.subflows` is a set of `Subflow` node uids to expand.

- For each one, the server loads the child flow's current draft by `namespace` and `flowId` in the same workspace, builds its graph with the same options, one level deeper, and splices it in as a cluster of kind `subflow` under the `Subflow` node.
- An expansion that cannot happen becomes a cluster with no nodes and a `reason`:
  - `not_found`: no such flow in this workspace;
  - `dynamic`: `namespace` or `flowId` is an expression;
  - `depth`: expansion is limited to two levels.
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
  "iterations": { "total": 3, "by_state": { "SUCCESS": 2, "FAILED": 1 } }
}
```

- **A task with one run** takes that run's state, attempt count and timestamps.
- **A task in a `Loop` body** has runs in several child executions, the ones whose `parent_execution_id` is this execution with a `loop_index`. Its `state` is the fold of those runs by Kestra's priority, and `iterations` counts them by state. `run_id` is null; the iteration list comes from §6.3.
- **A task with no run** has no entry. The client draws it dimmed.
- **A flowable cluster** takes its own run's state when it has one.

States follow W7b's vocabulary and `StateChip`.

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
- Nodes are not draggable and not connectable.

**A task node** shows the type's icon, `task_id` and the short type. On the execution graph it also shows its `StateChip`, its duration and, for a `Loop` body, the iteration count.

**Marking:**
- Branch colours use OKLCH tokens: `errors` in the danger token, `finally` and `afterExecution` in the warning token.
- `choice` edges carry their label. Trigger edges are dashed.
- A task with `refusal` gets a hatched border and the note "egret does not run this task" with its code. A disabled task is dimmed and says so.

**Controls:**
- Zoom in, zoom out and fit.
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

- **Registration.** `registerTab({ id: "topology", … })` registers the tab in the Topology chunk. The editor frame does not change (W7a §5.1).
- **Updates.** The tab sends `POST flows/graph` with the editor's current YAML, 1 s after the last edit.
- **Errors.** While the YAML does not parse, it keeps the last graph and shows a banner, "The graph is out of date: the YAML has an error".
- **Selection.** Selecting a node moves the Code tab's cursor to the task, through the same path-to-range mapping the Problems tab uses.

### 6.3 The execution tab

- **The tab.** It is added to W7b's URL tabs in Kestra's order: Overview, Topology, Gantt, Logs, Outputs.
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

## 10. Rules this package must meet

- The frontend rulebook, every rule; in particular FE-ROUTE-2 (a lazy area with a bundle test), FE-STATE-2 (skeletons), FE-A11Y (focusable nodes, the list view) and FE-DATA-2 (generated client).
- The two new dependencies are pinned exactly, and `package-lock.json` changes only for them.

## 11. Known limits

- A `Subflow` expands to the child flow's current draft in the editor, not to the version that would run. Every execution runs the saved draft, so the two are the same once the child is saved.
- The execution graph re-reads every run on each poll. A flow with thousands of loop iterations makes that read heavier; the 5,000-node limit does not bound iterations, because iterations fold into one node.
