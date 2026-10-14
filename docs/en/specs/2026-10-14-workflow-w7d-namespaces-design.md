# W7d — Workflows UI, part 4: namespaces (list, detail, files, KV, dependencies)

**Status:** implemented, 2026-10-14. Measured live on a private stack (a fresh database, a private RustFS, `serve` and Vite on their own ports): the namespace list and detail, the Files tab with a real drag and drop and a real `webkitdirectory` folder import, Ctrl/Cmd+S per editor with Code and two file tabs side by side (proved by the requests sent), the 409 `file_changed` dialog from a second session, the namespace-change question, KV (JSON, DATE, an exponent NUMBER read back with every digit, inherited keys) and the `subflow` edge, with the editors measured in light and dark mode (`frontend/tests/e2e/workflows-namespaces.spec.ts`). Dark mode had a defect, found by that run and fixed in the final review: Monaco's theme is global and a theme-less editor set it light, so a file tab turned the flow's Code beside it light. Every editor now takes the app's mode (`MonacoEditor` defaults it), and the run asserts a dark editor background on the namespace page, for Code beside two file tabs, and after a revision opens in the drawer. The differences from a running Kestra are in §2.

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

**Measured against a running Kestra, 2026-10-14 (Task 9, §9).** The local image `kestra/kestra` `28f59bf175e1` (Kestra 2.0 nightly, `server local`, in a throwaway container) ran §6's scenario: `company.team.child`, `company.other.caller` calling it through a `Subflow`, a `Flow` trigger on `child`, a namespace file, KV keys of each type. The same requests went to egret. Each row says whether it was **observed** on the running image (its API or its UI) or **read** in Kestra's source.

| Kestra | egret | Why |
|---|---|---|
| Observed: `GET /namespaces/search` lists namespaces with flows and their parents (`company`, `company.other`, `company.team`); a namespace holding only a file (`w7d.filesonly`) or only a KV key (`w7d.kvonly`) is not listed | Listed, with `own: true` and its counts; a namespace holding only a live folder is listed too (`own: true`, `files: 0`) | A namespace can hold files or keys without a flow (user's ruling) |
| Observed: `GET /namespaces/nope.nothing` answers 200 `{"id":"nope.nothing"}` | 404, and the page says "Namespace not found" with a link back | A link to nothing should say so |
| Observed: the namespace page's tabs are Overview, Edit (EE), Flows, Executions, Dependencies, Secrets, Assets, Variables, Policies (EE), **KV Store, Namespace Files**, Revisions, Audit logs (EE); read: `ORDER` in `ui/src/components/namespaces/utils/useHelpers.ts` puts `kv` before `files` | Overview, Flows, Executions, Dependencies, **Files, KV** | The order the user decided (2026-10-14); this nightly image puts KV first |
| Read: the Overview tab is the executions `Dashboard`, or `NoExecutions` when the namespace has none (`ui/src/components/namespaces/components/NamespaceOverview.vue`) | Counts (flows, files, keys), recent executions, child namespaces | egret has no dashboard charts |
| Observed: two writes of one file answer 200 and 200, the second wins; an `if_revision` parameter is ignored (200); `files/stats` carries no revision | Compare-and-write: `if_revision` (`0` = must not exist); a stale one answers 409 `file_changed` naming the revision ("it is at revision 2, not 1; nothing was written"); the editor opens Reload or Overwrite | Two editors of one file must not silently overwrite each other (W7a's rule for flows) |
| Observed: uploading `bundle.zip` to `?path=/bundle.zip` extracts it, at the namespace root (the path is ignored): `/inner/x.txt`, `/y.sql` | Stored as one file, `/bundle.zip`. Import is files and folders (`webkitdirectory`, one upload per file) | USER RULING 2026-10-14: no zip extraction |
| Observed: a file under `/_flows/` is imported as a flow (`viafile` created in the file's namespace) | 400: "'/_flows' and everything under it is reserved for flow sources" | W9b's rule; flows are edited in the flow editor |
| Observed: `GET /namespaces/company.team/dependencies` has no edge for `caller` → `child` (a `Subflow` into the namespace), while `company.other`'s graph has it; `company.other`'s graph has no edge for the `Flow` trigger into `listener`, while `company.team`'s has it | Every edge into or out of the namespace, in both graphs (`company.other.caller → company.team.child`, `subflow`; `company.team.child → company.other.listener`, `flow_trigger`) | One graph per namespace that is the same from either end |
| Observed: a `Flow` trigger whose `dependsOn` names only `namespace: company.team` draws an edge from every flow of `company.team` | No edge: only a `dependsOn` entry naming both `namespace` and `flowId` draws one (§3.4) | An edge says "this flow starts that one"; a namespace-wide condition is not that |
| Read: `GET /namespaces/{ns}/dependencies` answers `flowTopologyService.namespaceGraph` (`NamespaceController.java`), a graph kept in the stored flow topology (`FlowTopologyRepository`) | Built from the drafts on each request (§3.4) | No stored dependency index in egret |
| Observed: a KV `PUT` of `1.2345678901234567890123e5` reads back `123456.78901234567` (an Ion float); `…d5` reads back every digit. Read: the UI's NUMBER field is `type="number"` and sends `String(value)` (`KVTable.vue`) | The API is the same (W9c parity); the UI sends an exponent as an Ion decimal (`d`), so `1.2345678901234567890123e5` reads back `123456.78901234567890123`, measured in the browser | A key's number is not rounded by the form |
| Read: the UI sends JSON as typed text (`KVTable.vue`) | The same: JSON is sent as typed, its numbers kept as written | Parity |
| Observed: `ttl: PT0S` is accepted (200) and the key reads back 410 at once | The API is the same (200); the form refuses `PT0S` before a request is sent | A key that expires as it is written is a typo |
| Read: the namespace list's Create action is shown only outside OSS (`v-if="!isOSS && canCreate"`, `ui/src/override/components/namespaces/Namespaces.vue`) | None: a namespace exists when something lives in it | OSS parity |

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
- **Folders:** a namespace holding only a live folder (no file below it) is listed, `own: true` and `files: 0`. Empty folders are not counted as files.

`GET /api/ws/{workspace}/workflows/namespaces/{ns}` returns a `WorkflowNamespaceDetailDto` for one namespace: the same row plus `can_manage`, whether the caller holds `workflow:manage` there (the page disables writes from it). It answers 404 when no source knows the namespace and it is not a parent of one. Overview uses it.

### 3.2 File writes with a revision check

`POST …/namespaces/{ns}/files` (W9b) gains an optional `if_revision` query parameter.

- **With it:** the write commits only if the path's current revision equals it. Otherwise it answers 409 with `code: "file_changed"`, as W7a's `draft_changed` does. `if_revision=0` means "the path must not exist".
- **Without it:** the behaviour is unchanged, so existing callers and tasks are not affected.
- **Atomicity:** the check and the write are one transaction (`commit_staged` in `storage/nsfiles/store.rs`), which takes W9b's existing per-path lock (`xact_lock`) before it reads the current revision, so no write commits between the check and the insert.
- **Deleted paths:** the current revision is the latest live one. A soft-deleted path, like one that never existed, is at revision 0: `if_revision=0` writes it again, and the revision an editor read before the delete answers 409 `file_changed` ("was deleted").
- **Other 409s:** a 409 without `code: "file_changed"` (for example a folder at that path) is a plain error, not the stale dialog.
- **Zips:** a `.zip` is a file like any other here (W9b stores it as one file), so `if_revision` applies to it. The idea of refusing `if_revision` on a `.zip` is dropped: its premise, that the server extracts zips, is false.

### 3.3 Unchanged

Every W9b file route and W9c KV route keeps its permission: read needs `workflow:read`, and write, delete and purge need `workflow:manage`.

### 3.4 Dependencies

`GET /api/ws/{workspace}/workflows/namespaces/{ns}/dependencies` requires `workflow:read`.

- **Nodes:** the flows of the namespace and its children (dot-anchored), plus every flow elsewhere in the workspace with a direct edge to or from one of them. Each node is `{id, namespace, flow_id, disabled, in_namespace}`.
- **Edges:** `{source, target, kind}`, where `kind` is:
  - `subflow`: a `Subflow` task anywhere in the flow's task tree, `errors:` included, whose `namespace` and `flowId` are literals naming an existing flow;
  - `flow_trigger`: a `Flow` trigger whose `dependsOn` has an entry naming both the source flow's `namespace` and its `flowId`. It is derived from the drafts, not from `workflow.flow_listeners`, which has no upstream list. An entry naming only a namespace draws no edge (§2).
- **Dynamic targets:** a `Subflow` whose target is an expression draws no edge, and its node carries `dynamic_subflows: n`.
- **Broken drafts:** a draft that does not parse stays a node, flagged `unparsable: true`, when it is in the namespace or an edge reaches it; its incoming edges are kept and it has no outgoing ones. The top-level `unparsable` counts every broken draft in the workspace, since one outside the namespace can hide an edge into it.
- **Source of truth:** built from the flows' saved drafts, which is what every execution runs (W7a ruling). There is no stored index.
- **Where it runs:** the drafts are parsed off the async workers (`spawn_blocking`).
- **Limit:** the bound counts every flow of the workspace, not only the namespace's. More than 2,000 answers 422 with `code: "dependencies_too_large"` and a message.

## 4. Frontend

Everything lives inside W7a's lazy Workflows area. A Workflows nav entry "Namespaces" is added.

### 4.1 The namespace list

The page is `/explore/:workspace/workflows/namespaces`.

- **Tree:** namespaces are drawn as a tree by dots, with each node's flows, files and keys counts. A parent-only node is muted. The table's caption says the counts are each namespace's own, as the Overview does ("Counted in this namespace only; each child namespace counts its own.").
- **Filter:** a prefix filter, kept in the URL (FE-PAGE-2).
- **States:** a skeleton while loading (FE-STATE-2) and an empty state.
- **Navigation:** selecting a node opens its detail.

### 4.2 The namespace detail

The page is `/explore/:workspace/workflows/namespaces/:namespace/:tab`, with vertical tabs in Kestra's order: Overview, Flows, Executions, Dependencies, Files, KV. The tab is part of the URL, and an unknown tab redirects to Overview. A breadcrumb shows each segment of the namespace.

- **Overview:** counts, the 10 most recent executions (W7b's list with `namespace_prefix`), and the direct child namespaces.
- **Flows** and **Executions:** W7a's flow list and W7b's executions list, with the namespace filter fixed to this namespace and its children. The Flows tab's tree folds in memory, starting open: the workspace list's stored collapse state is not read there, so a collapsed ancestor cannot hide the tab.
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
- Import files, or a folder through the browser's folder picker (`webkitdirectory`): one upload per file, each with its path below the folder, each with `if_revision=0`, and existing paths listed for an Overwrite question. **USER RULING 2026-10-14: no zip extraction.** A `.zip` is imported as one file, as W9b stores it. Export the namespace as a zip.
- Rename, and move by drag and drop. Every drag has a keyboard equivalent ("Move to…").
- Delete, with a confirmation that names how many files go.
- Search, through W9b's search route.
- See a file's revisions and restore one. Restoring writes that revision's content as a new revision, with `if_revision`; a 409 asks whether to restore over the newer revision.

- An import of more than 100 files asks first. While an import runs, every write (new file or folder, another import, rename, move, delete, drag and drop) is disabled with "Wait for the import to finish."
- The tree's keyboard help offers F2 and Delete only when writes are on.

**Opening a file**
- A text file opens in Monaco, with the language taken from its extension, in the app's light or dark theme.
- An image shows a preview.
- Any other binary file, and any file over 5 MB, offers download only. A revision over 5 MB, viewed from Revisions, is a download only as well.

**Saving**
- A save sends `if_revision`.
- A 409 `file_changed` opens W7a's stale-save dialog: Reload or Overwrite. Overwrite resends without `if_revision`.
- Ctrl/Cmd+S belongs to the editor that has focus: each file editor registers its own Monaco action (`addAction`, disposed with the editor), so the key saves that file only, never the flow or another file.
- An unsaved file shows a dot on its tab, and closing it asks first.

**On the namespace page**
- The tree is on the left and the open files are tabs on the right.
- The open tabs are remembered per namespace in `localStorage`.
- The Files tab stays mounted, hidden, once it has been shown, so a visit to another tab of the namespace keeps unsaved edits. The tabs use manual activation: arrows move focus, Enter or Space selects.
- **Deviation (FE-PAGE-2):** the open file and the search text are not in the URL here. `NamespaceFiles` is one component for this page and the flow editor, whose open files are the layout's tabs; they are kept in component state (and the open tabs in `localStorage`).

**In the flow editor**
- A Files tab is registered through `registerTab`, the same way W7c registered Topology: a stub whose body loads lazily. It shows the tree of the flow's namespace.
- Opening a file adds a tab `file:<path>` to the multi-panel frame, in the panel that holds Code.
- A file tab saves with Ctrl/Cmd+S. "Save all files" saves every unsaved file tab.
- File tabs are restored from the stored layout.
- The flow's own Save does not save files, and saving a file does not save the flow.
- A flow save that moves the flow to another namespace while a file tab has unsaved edits asks first: **Save files first** (into the old namespace, then the flow), **Discard file changes**, or **Cancel**. The file tabs of the old namespace close when the flow moves.
- A restore from the Files tree's Revisions reloads an open file tab that has no unsaved edits; one with edits keeps them, and its save meets the stale-save question.
- When someone else's save moves the flow to another namespace, the file tabs close and their unsaved edits are lost, with a notice that says so.

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
- JSON is sent as typed, so its numbers keep every digit. A NUMBER with an exponent is sent as an Ion decimal (`6.02e23` as `6.02d23`), so every digit is kept; an Ion float would round it.
- The TTL check follows egret's own duration parser (fractions are accepted). `PT0S` is refused in the form: the key would expire as it is written.
- Editing a key keeps its remaining TTL unless the expiration is changed. The time left is read when Save is pressed; if the key expired while the drawer was open, the form refuses ("This key expired while you were editing; choose an expiration") and sends nothing, rather than a PUT without `ttl`, which would make the key permanent.
- An edit sends a DATETIME whose text is unchanged exactly as the server sent it, so a stored value's micro- or nanoseconds survive an edit of only the description (the form's `datetime-local` text holds milliseconds at most).
- Creating a key that already exists asks before overwriting.

**Inherited keys**
- An "Inherited KV" drawer lists the keys inherited from parent namespaces, through W9c's inheritance route. The nearest parent wins.
- **Deviation (FE-DATA-2):** a key named `inheritance` cannot be read at the path the generated fetcher builds, because `…/kv/inheritance` is the inheritance listing. That one key is read through the same mutator (`customInstance`) at the generated URL with its last letter percent-encoded (`…/kv/inheritanc%65`), which the server decodes back to the key (`kvApi.ts`).

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

**Playwright** (`frontend/tests/e2e/workflows-namespaces.spec.ts`), on a private stack, never the shared dev stack:
1. Create a flow in `company.team`, a flow in `company.other` that calls it through a `Subflow`, and a key in `company`; the namespace list shows them as a tree.
2. On the namespace page: create files with `if_revision=0`; edit one, leave for the KV tab and come back (the text and its unsaved dot are still there), save it with Ctrl/Cmd+S (one POST with `if_revision`); rename; move by a real drag and drop; import a folder through `webkitdirectory`; export the zip. The tree and the editor are measured in light and dark mode, the editor's background asserted dark in dark mode.
3. KV: create a JSON key and a DATE key, read back an exponent NUMBER with every digit, and see the keys inherited from `company`.
4. In the flow editor: Code beside TWO file tabs, side by side; Ctrl/Cmd+S in each file saves only that file, and in Code only the flow. In dark mode the two file editors and Code have a dark background, and still do after a revision opens, dark, in the Revisions drawer.
5. A second session saves a file: this session's save answers 409 `file_changed`, the stale dialog opens, and Overwrite resends without `if_revision`.
6. A flow saved into another namespace while a file has unsaved edits asks first; "Save files first" writes into the old namespace.
7. Dependencies draws the `subflow` edge.

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

The scenario of §6 ran on the local Kestra image on 2026-10-14 (Task 9). Every difference found is recorded in §2, each labelled observed or read in source.

## 10. Known limits

- Dependencies reads every flow's draft on each request, bounded at 2,000 flows. A workspace with more flows gets the 422.
- A `Subflow` whose target is an expression has no edge.
- Every draft of the workspace is loaded for the request, and their size is not bounded, only their count.
