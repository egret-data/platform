# W7d — Workflows UI, part 4: namespaces (list, detail, files, KV, dependencies)

**Status:** design, 2026-10-14.

**Depends on:**
- W7a (#659): the Workflows area, the multi-panel editor and its tab registry, the stale-save dialog pattern (`if_updated_at`, 409 `draft_changed`).
- W7b (#662): the executions list with `namespace_prefix`.
- W7c (#667): `TopologyView`'s layout and drawing pieces (xyflow + dagre), lazily loaded.
- W9b: namespace files and their routes.
- W9c: the KV store and its routes.

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (Kestra's UI as reference) and §9 row W7. The W7a spec, §5.1, reserved the editor's Files tab for this package.

**Decided with the user, 2026-10-14:**
- **The namespace detail has Kestra OSS's tabs, Dependencies included:** Overview, Flows, Executions, Dependencies, Files, KV. Kestra's EE tabs (Secrets, Variables, Policies, History, Audit logs, Plugin defaults) are left out.
- **Namespaces come from flows, namespace files and KV keys.** A namespace is listed when it has a flow, a live namespace file or a live KV key, and every parent namespace by dots is listed too. There is no namespace table and no migration for one. Kestra OSS lists only namespaces with flows; egret also lists a namespace that holds only files or keys, which a task such as `UploadFiles` can create.
- **One package,** not split.
- **In the flow editor, each namespace file opens as its own tab** in W7a's multi-panel frame, next to Code, as in Kestra. Flow Save and file save are separate. There is a "Save all files" action, and file tabs survive a stored layout. A file save sends the revision it was read at and is refused with 409 if the file changed meanwhile. Kestra's save is last-write-wins.

## 1. Why

W9b and W9c gave egret namespace files and a KV store, but they can be reached only through the API and through tasks. A user cannot see which namespaces exist or browse their files. Nobody can edit a script that a flow's `namespaceFiles` loads, or manage a key that `{{ kv('…') }}` reads, without `curl`. Kestra users expect a Namespaces page and a Files panel next to the flow's YAML.

## 2. Divergences from Kestra

| Kestra (OSS) | egret | Why |
|---|---|---|
| Namespaces are derived from flows only (`GET /namespaces/search`) | From flows, live namespace files and live KV keys, plus parents | A namespace can hold files or keys without a flow (user's ruling) |
| Overview is the executions dashboard, or a "no executions" screen | Counts (flows, files, keys), recent executions, child namespaces | egret has no dashboard charts |
| Saving a namespace file is last-write-wins | Compare-and-write on the file's revision; 409 opens Reload or Overwrite | Two editors of one file must not silently overwrite each other (W7a's rule for flows) |
| Dependencies graph via `GET /namespaces/{ns}/dependencies` | The same idea, built from drafts (§3.4) | No stored dependency index in egret |
| Namespace creation (EE) | None: a namespace exists when something lives in it | OSS parity |

The comparison of §9 adds measured differences.

## 3. Backend

### 3.1 Listing namespaces

`GET /api/ws/{workspace}/workflows/namespaces?prefix=` requires `workflow:read`.

- **What it returns.** The union of three sources, each scoped to the workspace:
  - namespaces of flows that are not deleted (`deleted_at IS NULL`);
  - namespaces with at least one live namespace file (the highest revision of a path is not `deleted`);
  - namespaces with at least one live KV key.

  Every parent by dots is added to that set.
- **Each row:** `{namespace, flows, files, kv_keys, own}`. `own` is false for a namespace present only as a parent.
- **Order and filter:** rows are sorted by namespace. `prefix` filters with the dot-anchored rule W7b uses for `namespace_prefix`.
- **Query:** one SQL statement over the existing tables; there are no migrations.

`GET /api/ws/{workspace}/workflows/namespaces/{ns}` returns the same row for one namespace, or 404 when no source knows it and it is not a parent of one. Overview uses it.

### 3.2 File writes with a revision check

`POST …/namespaces/{ns}/files` (W9b) gains an optional `if_revision` query parameter.

- **With it:** the write commits only if the path's current revision equals it. Otherwise it answers 409 with `code: "file_changed"`, as W7a's `draft_changed` does. `if_revision=0` means "the path must not exist".
- **Without it:** the behaviour is unchanged, so existing callers and tasks are not affected.
- **Atomicity:** the check and the write are one statement under W9b's existing per-path lock.

### 3.3 Unchanged

Every W9b file route and W9c KV route keeps its permission: read needs `workflow:read`, and write, delete and purge need `workflow:manage`.

### 3.4 Dependencies

`GET /api/ws/{workspace}/workflows/namespaces/{ns}/dependencies` requires `workflow:read`.

- **Nodes:** the flows of the namespace and its children (dot-anchored), plus every flow elsewhere in the workspace with a direct edge to or from one of them. Each node is `{id, namespace, flow_id, disabled, in_namespace}`.
- **Edges:** `{source, target, kind}`, where `kind` is:
  - `subflow`: a `Subflow` task anywhere in the flow's task tree, `errors:` included, whose `namespace` and `flowId` are literals naming an existing flow;
  - `flow_trigger`: a `Flow` trigger whose conditions name the source flow, from `workflow.flow_listeners` and the trigger's conditions.
- **Dynamic targets:** a `Subflow` whose target is an expression draws no edge, and its node carries `dynamic_subflows: n`.
- **Source of truth:** built from the flows' saved drafts, which is what every execution runs (W7a ruling). There is no stored index.
- **Limit:** reading more than 2,000 flows answers 422 with a message.

## 4. Frontend

Everything lives inside W7a's lazy Workflows area. A Workflows nav entry "Namespaces" is added.

### 4.1 The namespace list

The page is `/explore/:workspace/workflows/namespaces`.

- **Tree:** namespaces are drawn as a tree by dots, with each node's flows, files and keys counts. A parent-only node is muted.
- **Filter:** a prefix filter, kept in the URL (FE-PAGE-2).
- **States:** a skeleton while loading (FE-STATE-2) and an empty state.
- **Navigation:** selecting a node opens its detail.

### 4.2 The namespace detail

The page is `/explore/:workspace/workflows/namespaces/:namespace/:tab`, with vertical tabs in Kestra's order: Overview, Flows, Executions, Dependencies, Files, KV. The tab is part of the URL, and an unknown tab redirects to Overview. A breadcrumb shows each segment of the namespace.

- **Overview:** counts, the 10 most recent executions (W7b's list with `namespace_prefix`), and the direct child namespaces.
- **Flows** and **Executions:** W7a's flow list and W7b's executions list, with the namespace filter fixed to this namespace and its children.
- **Dependencies:** a graph of flows drawn with W7c's xyflow and dagre pieces.
  - `subflow` edges are solid and `flow_trigger` edges are dashed.
  - Flows outside the namespace are muted.
  - Selecting a node opens that flow.
  - It has keyboard navigation and a list view, as W7c's `TopologyView` does.
- **Files:** `NamespaceFiles`, §4.3.
- **KV:** §4.4.

### 4.3 Files

`NamespaceFiles` is one component used in two places: the namespace's Files tab and the flow editor's Files tab.

**The file tree**
- Create a file or a folder.
- Import a file, a folder or a `.zip` (extracted by the server, as W9b does). Export the namespace as a zip.
- Rename, and move by drag and drop. Every drag has a keyboard equivalent ("Move to…").
- Delete, with a confirmation that names how many files go.
- Search, through W9b's search route.
- See a file's revisions and restore one. Restoring writes that revision's content as a new revision.

**Opening a file**
- A text file opens in Monaco, with the language taken from its extension.
- An image shows a preview.
- Any other binary file offers download only.

**Saving**
- A save sends `if_revision`.
- A 409 `file_changed` opens W7a's stale-save dialog: Reload or Overwrite. Overwrite resends without `if_revision`.
- An unsaved file shows a dot on its tab, and closing it asks first.

**On the namespace page**
- The tree is on the left and the open files are tabs on the right.
- The open tabs are remembered per namespace in `localStorage`.

**In the flow editor**
- A Files tab is registered through `registerTab`, the same way W7c registered Topology: a stub whose body loads lazily. It shows the tree of the flow's namespace.
- Opening a file adds a tab `file:<path>` to the multi-panel frame, in the panel that holds Code.
- A file tab saves with Ctrl/Cmd+S. "Save all files" saves every unsaved file tab.
- File tabs are restored from the stored layout.
- The flow's own Save does not save files, and saving a file does not save the flow.

### 4.4 KV

**The table**
- Columns: key, type, description, last modified, expiration. All are sortable.
- A key filter, kept in the URL.

**Row actions**
- View the value.
- Edit.
- Copy `{{ kv('key') }}`.
- Delete. Selected rows can be deleted together with W9c's bulk delete.

**The drawer for creating and editing**
- Fields: key (locked on edit), type, value, description, and expiration as a TTL.
- Types: STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION, JSON. Each has its own control; JSON uses Monaco.
- The value is checked in the form with the same rules the server applies.
- Creating a key that already exists asks before overwriting.

**Inherited keys**
- An "Inherited KV" drawer lists the keys inherited from parent namespaces, through W9c's inheritance route. The nearest parent wins.

### 4.5 Permissions

A viewer with `workflow:read` but not `workflow:manage` sees everything. Every write action (create, import, rename, move, delete, save, KV writes) is disabled with its reason shown (FE-ACTION). The flows and executions lists keep their own rules from W7a and W7b.

### 4.6 Chunks

- Monaco loads only in the file editor's chunk.
- xyflow and dagre load only in the Dependencies chunk, reusing W7c's Topology chunk where possible.
- `test:bundle:workflows` gains markers for the new modules.

## 5. Errors

**Namespaces**
- An unknown namespace shows "not found" with a link back to the list.

**Files**
- A `_files` segment is refused with the server's message (W9b's rule).
- A file over the size cap shows the 413's message.
- A revision conflict opens the dialog of §4.3.
- Network errors show what happened and what to do (FE-COPY-2).

**KV**
- A value of the wrong type is refused in the form before the request is sent.

**Dependencies**
- Over 2,000 flows gives 422 with its message.

## 6. Tests

**Rust**
- Listing:
  - each of the three sources, and parents added by dots;
  - another workspace's namespaces absent;
  - deleted flows, deleted files and deleted or expired keys not counted;
  - exact counts;
  - the prefix filter is dot-anchored;
  - the detail route answers 404.
- `if_revision`:
  - a match commits;
  - a mismatch answers 409 `file_changed` and writes nothing;
  - `0` on an existing path is refused;
  - without it, the behaviour is unchanged;
  - two concurrent writes with the same revision: exactly one commits.
- Dependencies:
  - a literal `Subflow`, including one under `errors:`;
  - a dynamic `Subflow` counted, not drawn;
  - a `Flow` trigger;
  - edges into and out of the namespace;
  - children of the namespace included;
  - the flow limit;
  - workspace scoping.
- Mutations on the scoping and on the revision check.

**Vitest**
- The namespace tree from a flat list.
- Tabs and the URL.
- Files:
  - tree operations, import and export;
  - the 409 dialog;
  - a file tab in the editor: open, dirty dot, Ctrl/Cmd+S, save all, restore from the stored layout;
  - binary and image handling.
- The KV form for every type, bulk delete and inherited keys.
- The read-only viewer.

**Playwright,** on a private stack, never the shared dev stack:
1. Create a flow in `company.team`, and a flow in `company.other` that calls it through a `Subflow`.
2. On the namespace page, create `scripts/a.py`, edit and save it, rename it, export the zip.
3. Create a JSON key and a DATE key, and see inherited keys from `company`.
4. In the flow editor, open `a.py` in the Files tab next to Code, edit it, Ctrl/Cmd+S.
5. Make a second save from another session, and see the 409 dialog.
6. Open Dependencies and see the `subflow` edge.

DOM measurements cover light and dark mode.

## 7. What W7d does not do

- Namespace creation and deletion, and namespace descriptions (EE).
- Secrets, variables, plugin defaults, policies, history and audit logs (EE).
- An executions dashboard with charts.
- Editing the flow from the Files tab: `_flows/` imports are not supported, and W9b refuses them.

## 8. Rules this package must meet

- The frontend rulebook, every rule.
- No new dependencies unless the plan names them; the existing Monaco, xyflow and dagre are reused.
- Regenerate the client only with `npm run gen:api`.

## 9. Comparing with Kestra

The scenario of §6 runs on the local Kestra image. Every deliberate difference is recorded in §2.

## 10. Known limits

- Dependencies reads every flow's draft on each request, bounded at 2,000 flows. A workspace with more flows gets the 422.
- A `Subflow` whose target is an expression has no edge.
