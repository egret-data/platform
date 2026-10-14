# W7d — Workflows UI part 4 (namespaces): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the browser, a user can:
- list the workspace's namespaces;
- open a namespace's Overview, Flows, Executions, Dependencies, Files and KV;
- edit its files with a revision check;
- manage its KV keys;
- open namespace files as tabs next to a flow's YAML in the flow editor.

**Architecture:**
- **Backend** (`crates/egret-workflow`, no migration):
  - a namespace listing over flows, files and KV;
  - `if_revision` on W9b's file upload;
  - a dependencies route built from drafts.
- **Frontend** (inside W7a's lazy Workflows area):
  - a namespaces list and a detail page with vertical tabs;
  - a shared `NamespaceFiles` component (tree, Monaco editor, revisions);
  - a KV table with a drawer;
  - a tab family in W7a's registry for per-file editor tabs;
  - a Dependencies graph on W7c's drawing pieces.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), React 19 + TypeScript, TanStack Query 5, orval, Monaco (existing), `@xyflow/react` + `@dagrejs/dagre` (existing), vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-14-workflow-w7d-namespaces-design.md`. **§n refers to the spec.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes and test cases to verify against the code, not text to paste. Names were read at `26258c91`. Re-read every fact this plan states about the code before relying on it.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every frontend task review answers its reviewer checklist.
- **User decisions (spec header, 2026-10-14)**, in effect verbatim:
  - The detail has Kestra OSS's tabs, Dependencies included: Overview, Flows, Executions, Dependencies, Files, KV. No EE tabs.
  - Namespaces come from flows, live namespace files and live KV keys, plus parents by dots. There is no namespace table and no migration.
  - One package.
  - In the flow editor each namespace file opens as its own tab next to Code. Flow Save and file save are separate. There is "Save all files", and file tabs survive the stored layout.
  - A file save sends `if_revision` and gets 409 `file_changed` on a mismatch.
- **Exact values:**
  - routes `GET namespaces?prefix=`, `GET namespaces/{ns}`, `GET namespaces/{ns}/dependencies` (all `workflow:read`);
  - `POST namespaces/{ns}/files` gains `if_revision` (`0` means "must not exist");
  - the code `file_changed`;
  - the dependencies limit 2,000 flows (422);
  - the edge kinds `subflow` and `flow_trigger`;
  - the editor tab id prefix `file:`;
  - the tab order Overview, Flows, Executions, Dependencies, Files, KV;
  - the URL `/explore/:workspace/workflows/namespaces/:namespace/:tab`.
- **Earlier rulings hold:**
  - every execution runs the saved draft (W7a);
  - W9b's file rules (`_files` refused, caps) and W9c's KV rules;
  - W7a's stale-save dialog;
  - W7c's lazy-chunk discipline and the strengthened bundle guard (`loadedWith`).
- **Dependencies:** none new. Reuse Monaco, `@xyflow/react` and `@dagrejs/dagre`. Regenerate the client only with `npm run gen:api`. No prettier.
- **Environment:**
  - Never touch the shared dev DB (:5432), shared buckets, `docker-compose.dev.yml` services, or a kind cluster named `egret`.
  - DB tests use the branch container `egret-w7d-postgres` (:5445), created by the controller. W9b and W9c file and KV tests need the object store their tests already arrange; follow their existing test setup.
  - Never run two cargo or vitest suites at once; wait while `pgrep -x cargo` is non-empty.
  - Commit before a long suite.
  - Never `git add -A` or `commit -a`; never commit `.superpowers/`.
  - Use `command cp -f` in scripts (a `cp -i` alias exists).
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

### Task 1: The namespace listing (§3.1)

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (two routes), `repo.rs` (or a new `namespaces.rs` module), `openapi.json`, the generated client.
- Test: a new `crates/egret-workflow/tests/namespaces_routes.rs`.

**Interfaces — Produces:**
- `NamespaceDto { namespace, flows: i64, files: i64, kv_keys: i64, own: bool }`
- `GET namespaces?prefix=` → `Vec<NamespaceDto>` sorted by namespace
- `GET namespaces/{ns}` → `NamespaceDto`, or 404

- [ ] **Step 1: Read the sources.**
  - How W9b decides a live file: the highest revision of a path is not `deleted`.
  - How W9c decides a live key: not deleted and not expired.
  - How W7b's `namespace_prefix` anchors on dots.

  Record each predicate, with `file:line`, in the report.
- [ ] **Step 2: Write the tests (red).**
  1. Each source alone makes a namespace appear: a flow only, a file only, a key only.
  2. Parents by dots appear with `own: false` and their own counts.
  3. A deleted flow, a soft-deleted file (latest revision deleted), and a deleted or expired key are not counted, and do not make their namespace appear.
  4. Exact counts. Several revisions of one path count as 1 file.
  5. Another workspace's namespaces are absent, including a namespace of the same name.
  6. `prefix=company` returns `company` and `company.*`, not `companyx`.
  7. `GET namespaces/{ns}`: 200 for a source or a parent, 404 otherwise.
  8. Permissions: `workflow:read`, with 403 and 401 controls.
  9. Mutations: remove the workspace scope, remove the deleted filter. Record which tests fail.
- [ ] **Step 3: Implement as one SQL statement.** Regenerate `openapi.json` the way the repo does, run `npm run gen:api` in `frontend/`, then `npx tsc -b`.
- [ ] **Step 4: Gates, then commit.** `feat(workflow): list namespaces from flows, files and KV keys`

### Task 2: File writes with `if_revision` (§3.2)

**Files:**
- Modify: `crates/egret-workflow/src/storage/nsfiles/routes.rs` (`upload_namespace_file`, its query), `store.rs` (the write under the per-path lock), `openapi.json`, the client.
- Test: W9b's file route tests (`tests/nsfiles_api.rs` or its neighbours).

**Interfaces — Produces:** `POST namespaces/{ns}/files?path=&if_revision=`, answering 409 `{code: "file_changed"}` on a mismatch.

- [ ] **Step 1: Read** how the upload writes (staging, `Conflict::Overwrite`, the per-path lock) and how W7a's `if_updated_at` returns its coded 409. Mirror that.
- [ ] **Step 2: Write the tests (red).**
  1. A matching revision commits a new revision.
  2. A mismatch answers 409 `file_changed`, and no revision, object or staging row is left.
  3. `if_revision=0` on an existing path answers 409; on a new path it commits.
  4. Without `if_revision`, the behaviour is unchanged (the existing tests pass unmodified).
  5. Two concurrent writes carrying the same revision: exactly one commits.
  6. A `.zip` upload with `if_revision` is refused with 400, since a zip writes many paths. Say so in the openapi description.
  7. Mutation: drop the check. Tests 2 and 5 fail.
- [ ] **Step 3: Implement, regenerate openapi and the client, run the gates, then commit.** `feat(workflow): namespace file writes can require the revision they were read at`

### Task 3: Dependencies (§3.4)

**Files:**
- Create: `crates/egret-workflow/src/namespaces/dependencies.rs` (or beside Task 1's module).
- Modify: `routes.rs`, `openapi.json`, the client.
- Test: `tests/namespaces_routes.rs`.

**Interfaces — Produces:** `NamespaceDependenciesDto { nodes: [{id, namespace, flow_id, disabled, in_namespace, dynamic_subflows}], edges: [{source, target, kind}] }`, where `kind` is `subflow` or `flow_trigger`.

- [ ] **Step 1: Read**:
  - how a `Subflow`'s `namespace`/`flowId` are read, and when they are literals (W6, `models::Subflow`);
  - `workflow.flow_listeners` and the `Flow` trigger conditions (W13), to see how a listener names its upstream flows.
- [ ] **Step 2: Write the tests (red).**
  1. A literal `Subflow`, including one under a flowable's `errors:`.
  2. A dynamic `Subflow`: no edge, and `dynamic_subflows` counts it.
  3. A `Flow` trigger listening to a flow.
  4. An edge into the namespace from outside, and one out of it. The outside flows are present with `in_namespace: false`.
  5. Child namespaces are included.
  6. A deleted flow is absent.
  7. Another workspace is invisible.
  8. 2,001 flows answer 422.
  9. A draft that does not parse is skipped and counted, not a 500. State how it is counted.
- [ ] **Step 3: Implement, regenerate openapi and the client, run the gates, then commit.** `feat(workflow): namespace dependencies from flow drafts`

### Task 4: The namespaces list and the detail frame (§4.1, §4.2: Overview, Flows, Executions)

**Files:**
- Create: `frontend/src/routes/workflows/NamespacesList.tsx`, `NamespaceDetail.tsx`, `components/workflows/namespaces/…` (Overview).
- Modify:
  - `WorkflowsRoutes.tsx` (routes);
  - the Workflows nav (a "Namespaces" entry);
  - `ExecutionsList.tsx` (a `pinnedNamespace` prop, like `pinnedFlowId`);
  - `FlowsList.tsx` (a `pinnedNamespace` prop);
  - `scripts/assert-workflows-lazy.mjs` (markers).
- Test: vitest beside each.

**Interfaces:**
- Consumes: Task 1's client.
- Produces:
  - `NamespaceDetail` with vertical URL tabs and a slot per tab;
  - `pinnedNamespace` props that fix the filter to the namespace and its children (`namespace_prefix`) and hide the namespace picker.

- [ ] **Step 1: Write the tests (red).**
  1. **The list's tree** built from a flat list: parent-only nodes muted, counts shown. The prefix filter is in the URL. Skeleton, then empty state.
  2. **The detail:**
     - the tab order is exact;
     - an unknown tab redirects to Overview;
     - the breadcrumb segments link to their namespaces;
     - a 404 shows "not found" with a link back.
  3. **Overview:** counts, the 10 most recent executions (`namespace_prefix`), and the child namespaces.
  4. **Flows and Executions** with `pinnedNamespace`: the request carries `namespace_prefix`, no namespace picker shows, and W7a's and W7b's existing tests are unchanged.
- [ ] **Step 2: Implement.** The frame is lazy, and its bundle markers are proven by a scratch static import.
- [ ] **Step 3: Frontend gates, then commit.** `feat(workflows): namespaces list and detail with Overview, Flows and Executions`

### Task 5: KV (§4.4)

**Files:**
- Create: `frontend/src/components/workflows/namespaces/kv/` (table, drawer, value controls, inherited drawer).
- Test: vitest.

**Interfaces — Consumes:** W9c's generated client: list, get value, put (body, `description`/`ttl` headers), delete, bulk delete, inheritance. Read the routes to confirm the exact headers and body encoding per type.

- [ ] **Step 1: Write the tests (red).**
  1. Columns, sorting, and the key filter in the URL.
  2. A row's view, edit, copy (`{{ kv('key') }}`) and delete.
  3. Bulk delete of selected rows.
  4. The drawer for each of the 7 types: valid and invalid values, and the exact body and headers sent.
  5. A TTL. A key that already exists asks before overwriting.
  6. The inherited drawer: nearest parent wins.
  7. A read-only viewer: writes are disabled, with their reason.
  8. Errors use FE-COPY-2 wording.
- [ ] **Step 2: Implement, run the frontend gates, then commit.** `feat(workflows): namespace KV table and editor`

### Task 6: `NamespaceFiles` on the namespace page (§4.3)

**Files:**
- Create: `frontend/src/components/workflows/namespaces/files/` (tree, toolbar, row actions, move dialog, revisions drawer, file editor pane, stale dialog reuse).
- Test: vitest.

**Interfaces:**
- Consumes:
  - W9b's client: directory, stats, search, download, upload multipart, move (`PUT`), delete, export, revisions, revision read (`?revision=`);
  - Task 2's `if_revision`;
  - W7a's stale-save dialog and `MonacoEditor`.
- Produces:
  - `NamespaceFiles({ namespace, onOpenFile?, readOnly })` for Task 7;
  - `useNamespaceFile(namespace, path)` for loading and saving with a revision (read once, save with `if_revision`, 409 → dialog).

- [ ] **Step 1: Write the tests (red).**
  1. **The tree:** directory listing, expanding folders, and the search route.
  2. **Create and import:** create a file or a folder; import a file, a folder (`webkitdirectory`) or a `.zip`.
  3. **Export, rename and delete:** export the zip; rename; delete, with a confirmation that names the count.
  4. **Move:** drag and drop, plus the keyboard "Move to…" dialog.
  5. **Revisions:** list them, view one, restore one (writes the content as a new revision).
  6. **Opening a file:** text opens in Monaco with the language from the extension; an image shows a preview; other binaries offer download only.
  7. **Saving:** it sends `if_revision`. A 409 opens the dialog: Reload refetches, Overwrite resends without it.
  8. **Open tabs:** a dirty dot, closing asks first, and tabs are remembered per namespace.
  9. **Errors:** a `_files` path and a 413 show the server's message.
  10. **A read-only viewer.**
- [ ] **Step 2: Implement.** Monaco stays in its lazy chunk.
- [ ] **Step 3: Frontend gates, then commit.** `feat(workflows): namespace files explorer and editor`

### Task 7: The flow editor's Files tab and per-file tabs (§4.3, "In the flow editor")

**Files:**
- Modify:
  - `frontend/src/components/workflows/panels/registry.ts`: a tab family, `registerTabFamily({ prefix: "file:", title(id), icon, closable, render(id, ctx), badge? })`, resolved by prefix. Every existing tab and the stored-layout restore keep working.
  - `editor/registerEditorTabs.tsx`: the Files stub, lazy body.
  - `MultiPanel` / `layout.ts`, only as needed to open a family tab in the panel holding Code.
- Test: registry, layout, and `MultiPanel` tests; a new Files tab test.

**Interfaces — Consumes:** Task 6's `NamespaceFiles` and `useNamespaceFile`; `EditorContext`; W7a's late-registration rule.

- [ ] **Step 1: Write the tests (red).**
  1. A family tab `file:scripts/a.py` opens in the panel holding Code, and opening it again focuses it rather than duplicating it.
  2. Its title is the file name, and its accessible name includes the path.
  3. A dirty dot; Ctrl/Cmd+S saves only that file; "Save all files" saves every dirty file tab.
  4. The flow's Save does not save files, and a file save does not save the flow.
  5. A stored layout naming `file:…` restores the tab and reloads its content.
  6. A file deleted meanwhile shows a clear state, not a crash.
  7. The Files tab shows the flow's namespace tree. A read-only viewer cannot save.
  8. The existing registry, layout and editor tests pass unchanged.
  9. Bundle: the Files tab body and Monaco stay out of the flow page's chunk, proven by a scratch static import.
- [ ] **Step 2: Implement, run the frontend gates, then commit.** `feat(workflows): namespace files as tabs in the flow editor`

### Task 8: Dependencies tab (§4.2)

**Files:**
- Create: `frontend/src/components/workflows/namespaces/dependencies/`.
- Modify: W7c's `components/workflows/topology/` only to export reusable pieces (the xyflow canvas wrapper, the dagre layout over a generic node and edge list, the keyboard and list-view helpers). No behaviour change to Topology: its tests stay unchanged.
- Test: vitest.

**Interfaces — Consumes:** Task 3's client, and W7c's pieces.

- [ ] **Step 1: Write the tests (red).**
  1. Nodes are flows; `subflow` edges are solid and `flow_trigger` edges dashed.
  2. Outside flows are muted. A node shows its dynamic-subflow count. Selecting a node opens its flow.
  3. Keyboard navigation and the list view.
  4. A 422 shows its message. An empty namespace has an empty state.
  5. Bundle: xyflow and dagre stay in a lazy chunk.
- [ ] **Step 2: Implement, run the frontend gates, then commit.** `feat(workflows): namespace dependencies graph`

### Task 9: End to end, the Kestra comparison, and docs

**Files:**
- Create: `frontend/tests/e2e/workflows-namespaces.spec.ts`.
- Modify: the spec (EN and VI): status line, §2, and §10 if needed.

- [ ] **Step 1: Private stack** per `frontend/tests/e2e/README.md`, never the shared dev stack or a kind cluster named `egret`. Use a fresh database in `egret-w7d-postgres`, and object storage as the README describes, in throwaway `w7d-` containers that are removed afterwards. Run §6's Playwright scenario, with real assertions.
- [ ] **Step 2: Measure the DOM** in light and dark mode, including the editor with a file tab beside Code at its default width.
- [ ] **Step 3: Kestra comparison** on the local image (`w7d-kestra`, removed afterwards). Record the differences in spec §2 (EN and VI). Label what was observed versus what was read in source.
- [ ] **Step 4: Gates, then commit.** `test(workflows): end-to-end namespaces; docs: W7d divergences`
