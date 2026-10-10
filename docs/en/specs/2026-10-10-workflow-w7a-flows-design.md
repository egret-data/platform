# W7a — Workflows UI, part 1: the flow list, the multi-panel flow editor, revisions and Run

**Status:** design, 2026-10-10; not yet implemented.
**Depends on:**
- W1: flows, drafts, versions and validation;
- W10: start execution;
- the frontend rulebook `docs/en/frontend-rules.md` (#655), which binds every rule of this package.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`. §8 covers Kestra's UI as the reference; §9 row W7 covers its scope.

**Decided with the user, 2026-10-10:**
- **W7 is split into five packages,** in this order:
  1. **W7a, flows:** the nav group, the flow list, the flow editor, Revisions and Run.
  2. **W7b, executions:** list, detail, gantt, logs, outputs, resume and replay.
  3. **W7c, topology:** the flow graph, and the execution graph with live state colours.
  4. **W7d, namespaces:** the namespace list, files and KV.
  5. **W7e, triggers.**
  
  Each package may add the small backend endpoints its UI needs.
  - **Out of W7:** no-code editing, blueprints, plugin documentation pages, AI, dashboards, kill and restart (no backend), and SSE (egret's APIs are poll-only).
- **The flow list groups by a namespace tree** (`company` → `company.team`). `folder_id` is not shown anywhere in the workflow UI.
- **The flow editor is the full multi-panel layout of Kestra's `MultiPanelTabs`.** Panels hold tabs; tabs move between panels and split them; the layout persists per flow.
- **Save and Publish are two buttons.** Save writes the draft; Publish creates an immutable version, which is what executions run. A content-derived indicator shows when the draft differs from the live version.
- **Editor intelligence:**
  - `monaco-yaml` uses a JSON schema served by a new read-only endpoint, built from the same registry the backend validates against and filtered by the workspace's features. This gives completion, hover and structural errors as you type.
  - A debounced server validation feeds semantic findings into the editor and a Problems tab.

- **(2026-10-10, the user's ruling while planning) The schema's property data comes from Kestra's own JSON schema, filtered to egret.** egret's registry holds only type names and aliases, and `catalog.json` holds properties for nine JDBC classes, so there was no property data to serve. Kestra's flow schema for the pinned version is exported once from the local Kestra image and committed as an artifact, like the oracle fixtures. It is filtered to the types egret accepts, minus the properties egret ignores. egret's own types (`egret.*`, `core.http`, `core.slack`, the script types) get hand-written schemas (§7.1).

- **(2026-10-10, the user's ruling on Task 2's review) The backend returns the creator's and publisher's display names.** `FlowListItemDto`, `FlowDto` and `FlowVersionDto` gain `created_by_name: Option<String>`, joined server-side. The UI needs no member listing, which requires `workspace:manage_members`, so every viewer sees names (§4, §7.4).

## 1. Why

The engine runs flows in production (W10 onwards), but a user can only author and run them with `curl`. W7a is the first package where a user writes, publishes and runs a flow in the browser. It also sets up the frame the other four W7 packages plug into: the nav group, lazy routes, the tab registry, and the state vocabulary.

## 2. Divergences from Kestra

| Kestra | egret | Why |
|---|---|---|
| Save creates a revision that takes effect at once | Save writes a draft; Publish creates the version executions run | egret's flows have a draft/version model (W1); the user's ruling |
| The flows list is flat with a namespace filter | The list is grouped by a collapsible namespace tree | The user's ruling; namespace decides write scope, files and KV, so the UI makes it the organising axis |
| Tabs: overview, executions, edit, revisions, triggers, logs, metrics, dependencies, concurrency | W7a ships Edit and Revisions. Executions arrives with W7b, Triggers with W7e; the others are not planned | A tab appears only with the package that fills it |
| Editor panels: Code, No-code, Topology, Docs, Files, Blueprints | Code, Docs, Problems in W7a; Topology with W7c; Files with W7d; no No-code and no Blueprints | Out of W7 by the user's ruling. Problems is egret's: Kestra shows findings in the editor and a banner |
| `/flows/validate` takes YAML with `revision: N+1` prefixed, and detects an `outdated` save | `POST …/flows/validate` takes the YAML as is. A stale save is refused by `PUT` with `if_updated_at` (§5.3) | egret has no revision counter on drafts |
| The run form follows the latest revision | The run form follows the **live version**, and says so when the draft differs | Executions run the live version, never the draft (W1) |
| The editor's panels: each panel has a dots menu (move panel, close all tabs, close all panels); a tab dropped on the frame's outer left or right edge makes a new panel there; `KeepAlive` keeps a panel's hidden tabs mounted, and panels are keyed by index | One "Tab actions: <title>" menu per panel, acting on its selected tab (Move to panel…, Split left, Split right, Close); a tab dropped on **any panel's** left or right edge splits beside that panel; only each panel's selected tab is mounted, and a change to the set of panels remounts the panels (§5.1) | A `tablist` may own only tabs, so a menu button inside each tab would break its semantics; one menu per panel keeps them and still gives every tab its menu in two keystrokes. Splitting beside any panel is the spec's model. React has no `KeepAlive`: tab state lives outside the tab, and Task 4 keeps Monaco's model and view state across a remount (`path`, `keepCurrentModel`, `saveViewState`). A reparenting portal is the fallback if another tab needs to stay mounted |
| `monaco-yaml` 5.5.1 and `monaco-worker-manager` 2.0.1 are patched in `node_modules` (`kestra/ui/patches/`): the worker is started with monaco 0.56's `createWebWorker({ worker })`, and the validator picks a `oneOf` member by its `type` so `type:` completion lists every type | No `node_modules` patch. `monacoYaml.ts` hands `monaco-yaml` a Monaco whose `createWebWorker` primes the worker the way monaco 0.56 does; the served schema requires only `id` and `type` outright and the rest behind `if: type is a string` (§7.1), which gives every `type:` spelling | Measured with egret's schema in Chromium and with `monaco-yaml`'s own worker in Node (W7a Task 4): unchanged, the worker fell back to an in-page stub with no YAML methods, and `type:` completion offered 10 of 54 task spellings |

Any further difference found while comparing against a running Kestra (§9) is added to this table with its reason.

## 3. Routes and navigation

**Nav.** The Explore sidebar gains a **Workflows** group, gated by the `workflows` feature in the same way the other modules are (`RequireFeature`, `EXPLORE_MODULES`). Its entries are **Flows** and **Executions**. Executions is a placeholder page until W7b.

`components/layout/__tests__/Sidebar.test.tsx` currently asserts that there is no Workflows link ("the real Workflows group lands in W7"). That assertion is replaced by one asserting that the group exists and is gated.

**Routes,** under `/explore/:workspace/workflows/`. Every page is code-split with `React.lazy` (FE-ROUTE-2) behind a skeleton fallback, and the guard is `RequireFeature feature="workflows"`.

| Route | Page |
|---|---|
| `flows` | The flow list (§4) |
| `flows/new` | Create a flow (§5.4); declared before `:id` |
| `flows/:id` | Redirects to `flows/:id/edit` |
| `flows/:id/:tab` | Flow detail; `tab` is `edit` or `revisions` in W7a. An unknown tab redirects to `edit` |
| `executions` | Placeholder until W7b |
| `executions/:id` | The minimal execution page (§6.3), replaced by W7b |

## 4. The flow list

`ListPageShell` gives the title "Flows" and a primary action "New flow" (a link to `flows/new`). Below it come `SummaryStrip`, with the total and the counts by state, and then the table (FE-PAGE-1).

- **Grouping.** Rows are grouped under a **namespace tree** built client-side from the flows' namespaces, split on `.`.
  - A tree node shows its segment and the number of flows beneath it. It collapses and expands, and its expanded state is a per-viewer preference.
  - A namespace that holds flows and also has child namespaces shows its own flows above its children.
  - The tree is a hand-written grouped `<table>`, the same shape as the grouped explore lists (`ui-conventions.md` §1.5), with a comment saying so (FE-PAGE-4).
- **Columns:**
  - flow id, a link to its editor;
  - state, as a chip (draft, live, disabled);
  - updated, as a relative time;
  - created by.
- **Filters,** kept in the URL (FE-PAGE-2):
  - text search on flow id and namespace;
  - namespace, as a picker of known namespaces that includes its children;
  - state.
- **States** (FE-STATE):
  - loading uses `ListStates`;
  - an empty collection uses `EmptyState`, whose primary action is "New flow";
  - a filter that matches nothing says so and offers to clear the filter;
  - an error uses `ListStates`'s error row.

`GET …/flows` has no pagination; W7a renders whatever it returns. If a workspace ever holds enough flows for this to matter, pagination is a follow-up.

## 5. The flow editor

### 5.1 The multi-panel layout

**Model.**
- The Edit tab is a horizontal row of **panels** with resizable gutters (`react-resizable-panels`). Each panel has a tab strip and shows one active tab.
- The layout is plain data: `{ panels: [{ id, size, tabs: TabId[], active: TabId }] }`. Every operation is a pure function over it.
- **Operations:**
  - move a tab to another panel at an index;
  - split, which moves a tab into a new panel left or right of a panel;
  - close a tab;
  - reopen a tab, through the panel's "+" menu, which lists the closed tabs;
  - resize.
  - A panel with no tabs is removed. The Code tab cannot be closed.
- **Pointer:** tabs drag with `@dnd-kit`. Dropping on a tab inserts before it; dropping on a strip past its tabs appends; dropping on the left or right edge zone of **any** panel's body splits beside that panel. Releasing over none of these changes nothing.
- **Keyboard (FE-A11Y-3):**
  - every tab has a menu with "Move to panel…", "Split left", "Split right" and "Close". It is one "Tab actions: <title>" menu per panel, acting on the panel's selected tab: a `tablist` may own only tabs, so a menu button inside each tab would break the tab semantics. Select a tab with the arrows, then Tab to its menu. Delete also closes a closable tab;
  - dnd-kit's keyboard sensor is enabled, and its arrows jump from one drop target to the next;
  - the tab strip is a `tablist` with arrow-key navigation.
- **Persistence.**
  - The layout is stored per flow in `localStorage`, under the key `egret.wf.layout.<flow uuid>`. It is a per-viewer preference (FE-DATA-3).
  - Every read and write is wrapped. A missing, unparsable or invalid layout falls back to the default. An unknown tab id, for example from a later package that was rolled back, is dropped.
  - New flows use the default layout.
- **Default:** two panels, `[Code] | [Docs, Problems]`, sized 60/40.
- **Mounting.** Only each panel's selected tab is mounted, keyed by its tab id. A change to the set of panels (a split, or a close or move that empties a panel) remounts the panels, so that their sizes always match the layout. A tab therefore keeps its state outside itself: the Code tab keeps Monaco's model and view state across a remount (`path`, `keepCurrentModel`, `saveViewState`; Task 4), and the page disposes that model when it leaves the flow. §2 records the difference from Kestra's `KeepAlive`.

**The tab registry.** Each tab kind is registered once with `registerTab({ id, title, icon, closable, render })`; `render(context)` receives the `context` the page hands the frame. W7c registers Topology and W7d registers Files without changing the frame. A tab registered after the editor has mounted (a lazy chunk) is picked up: the frame re-renders, and a stored layout that names it keeps it, unless the viewer changed the layout before it registered.

### 5.2 The Code, Docs and Problems tabs

**Code.**
- Monaco plus `monaco-yaml`. The schema is `GET …/workflows/schema` (§7.1), cached by ETag. `monaco-yaml` and its worker load lazily with Monaco, and a bundle test asserts both stay out of the entry chunk.
- This gives completion of `type:`, properties and enum values, hover documentation, and structural errors as you type.
- **Server validation.**
  - It runs 600 ms after the last edit. A newer request cancels the older one.
  - It calls `POST …/flows/{id}/validate` for an existing flow, or `POST …/flows/validate` (§7.2) on the create page.
  - Findings become Monaco markers. Each finding's `path` (for example `tasks[2].retry.maxAttempt`) is mapped to a range by walking the YAML document with the `yaml` package. An unmappable path marks line 1 and is listed in Problems with its path.
- **Saving.**
  - `Ctrl/Cmd+S` saves.
  - Unsaved edits arm `useUnsavedChangesWarning`.
  - Read-only users (no `workflow:manage`) get a read-only editor with a banner explaining why.

**Docs.** This tab follows the cursor.
- It finds the innermost task or trigger enclosing the cursor, and shows that task's or trigger's documentation from the same schema: description, a properties table (name, type, required, default, description) and outputs.
  - A task or trigger is a mapping with a string `type:` that has an `id:` or is a list item. A retry policy's own `type: constant` (a mapping under a key, without an `id`) is therefore skipped, as Kestra's `isTaskLike` does, and a task whose `type:` was typed before its `id:` is still found.
  - While the text does not parse (a key without its `:`, an open quote), the error-tolerant tree is read at the edit, and the last source that parsed everywhere else.
- Outside any typed mapping it shows the flow-level documentation (`id`, `namespace`, `inputs`, `tasks`, `triggers`, `errors`, …).
- It updates on cursor moves, debounced by 150 ms.

**Problems.**
- It lists errors and warnings from both sources: schema markers and server findings. Each item shows its severity, message and path.
- Selecting an item moves the editor's cursor to the item's range and focuses the editor.
- The tab title carries the error count.
- States (FE-STATE):
  - "Validating…" uses a skeleton row;
  - "No problems" uses `EmptyState`;
  - a failed validation request shows the error with a retry action. It is never shown as "no problems".

### 5.3 Save, Publish and the draft indicator

The flow detail header is the flow id and namespace with the state chip. Its actions are:

**Save** (`workflow:manage`).
- It sends `PUT …/flows/{id}` with `{ source, if_updated_at }` (§7.3).
- On 409 a dialog explains that the draft changed since it was opened. It offers "Reload", which discards local edits after a confirm, or "Overwrite", which saves again without `if_updated_at`.
- Save stays enabled when validation has errors; a draft may be invalid.

**Publish** (`workflow:manage`).
- It opens a dialog with an optional note and sends `POST …/flows/{id}/publish`.
- It is disabled while there are unsaved edits ("Save first") or validation errors ("Fix N errors to publish"). The reason is shown on the control (FE-ACTION-4).

**Run** (`workflow:manage`).
- It opens the run form (§6). It is disabled while no version is live ("Publish the flow before running it").

**Delete** (`workflow:manage`).
- It goes through `ConfirmDeleteDialog` (FE-ACTION-1), then navigates to the list.

**The draft indicator** is a pill, "Draft differs from live v{n}".
- It is computed by comparing the saved draft's `source` with the live version's snapshot `source`, byte for byte (FE-STATE-4). It goes out again if the draft is edited back to the live text and saved.
- Unsaved local edits are a separate indicator ("Unsaved changes").

After a successful Save or Publish, the flow query is invalidated, and Publish also invalidates the versions query. Save and Publish confirm with a toast, because the user stays on the page (FE-ACTION-2).

### 5.4 Creating a flow

`flows/new` is a page (FE-PAGE-3). It opens the editor with this template:

```yaml
id: hello
namespace: company.team
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: Hello from egret
```

Behaviour:
- Validation uses `POST …/flows/validate`.
- The first Save sends `POST …/flows`, then replaces the URL with `flows/{id}/edit`, keeping the editor's text and layout.
- A 409, meaning the namespace and id already exist, is shown beside the editor with the server's message, and the text is kept.

## 6. Run

### 6.1 The form

Run reads the `inputs:` of the **live version's** snapshot. When the draft differs from live, the form says "Runs live v{n}; your draft is not published".

- **Container.** A flow with no inputs runs at once after a confirm. Up to three inputs, all of them scalar, open a dialog. Otherwise a right-hand drawer opens.
- **Controls by type** (`models/input.rs`):

| Input type | Control |
|---|---|
| STRING, EMAIL, URI | text input |
| INT, FLOAT | number input |
| BOOL | switch |
| DATETIME, DATE, TIME | date/time pickers (ISO value) |
| DURATION | text input, ISO-8601 placeholder (`PT1H`) |
| SELECT | select; MULTISELECT is a multi-select (both from `values`) |
| ARRAY | a list of rows, each typed by `itemType` |
| JSON, YAML | a small Monaco editor |
| SECRET | password input |
| FILE | file picker; the request becomes multipart |
| FORM, REUSABLE_INPUTS, ION | not supported: the form names the input and the type, and Run is disabled |

- **Labels and values.**
  - Each control has a `Label` with `htmlFor` (FE-FORM-2), the input's `displayName` or `id`, its `description`, and a required marker.
  - Defaults pre-fill. Required and empty values block submit with an inline message.
- **Submit.**
  - It sends `POST …/flows/{id}/executions`.
  - Server-side input errors show beside their field (FE-FORM-4). Nothing entered is cleared (FE-ACTION-3).
  - On success it navigates to `executions/{id}`.

### 6.2 What the run form does not do

Kestra's `dependsOn` (inputs that re-render from other inputs) is not supported in W7a. An input with `dependsOn` renders its static `values` and carries a note. Whether the backend evaluates `dependsOn` is recorded in §15 once checked.

### 6.3 The minimal execution page

`executions/:id` shows:
- the execution id, flow, namespace, state chip, start time and duration;
- a link back to the flow;
- the task runs as a plain list of id and state.

While the execution is not terminal, it polls `GET …/executions/{id}` every 2 s. W7b replaces this page.

## 7. Backend additions

### 7.1 `GET /api/ws/{ws}/workflows/schema`

- **Permission:** `workflow:read`.
- **Response:** a JSON Schema (draft 2020-12) for one flow document:
  - the flow shape (`id`, `namespace`, `description`, `labels`, `inputs`, `variables`, `tasks`, `errors`, `finally`, `triggers`, `pluginDefaults`, `concurrency`, `disabled`, …);
  - a `oneOf` over task types, keyed by `type`, for `tasks`/`errors`/`finally` (recursively for flowable children), and the same over trigger types for `triggers`.
- **Each type's entry** carries the description, `properties` with types, required, defaults and descriptions, and `outputs` (as an `x-egret-outputs` extension, for the Docs tab).
  - `required` holds only `id` and `type`; the type's other required properties are in `then.required`, behind `if: { required: [type], properties: { type: { type: string } } }`. It accepts exactly the same documents, and it lets `monaco-yaml` complete every `type:` spelling (`spec/schema/README.md` §5, W7a Task 4).
- **Source** *(the user's ruling, 2026-10-10)*. The schema is assembled at build time from three committed inputs:
  1. **`kestra-flow-schema.json`.** Kestra's flow JSON schema, exported once from the local `kestra/kestra` image (`GET /api/v1/plugins/schemas/flow`). Its Kestra version is recorded in the file, and the export command is in its header comment or README.
  2. **`egret-task-schemas.json`.** Hand-written schemas for the types Kestra does not have: `egret.*`, `core.http`, `core.slack`, and the script types if Kestra's definitions do not match egret's.
  3. **The registry** (`spec/registry.rs`), together with the flowable set in `models/task.rs`.
- **Filtering.**
  - Only types egret accepts are kept, by canonical FQCN, with their aliases added.
  - Properties egret parses but ignores are removed, so completion never suggests something egret silently drops. An example is the root task properties that egret drops on non-script tasks (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`).
  - The plan's first task measures, per type, how egret treats each property Kestra documents (honoured, ignored or refused) before fixing the filter.
- **Tests.**
  - A Rust test asserts that the set of `type` values in the served schema equals the set the registry accepts for the same features.
  - A second test asserts that no served property is one egret is known to ignore.
- **Precedence.** `catalog.json`'s nine JDBC classes keep their catalog properties where they differ from Kestra's export.
- **Features.**
  - With `workflow_kestra_plugins` off, the Kestra plugin types are absent.
  - With `workflow_containers` off, the container types are absent.
  - The response varies by workspace, so it carries a strong `ETag` and `Cache-Control: private, no-cache`.
- **Aliases.** Short aliases that the registry accepts are included as `type` enum members, with the canonical FQCN named in their description.

### 7.2 `POST /api/ws/{ws}/workflows/flows/validate`

- **Permission:** `workflow:manage`, the same as the existing validate-by-id route.
- **Body:** `{ "source": string }`, under the same body limit as create.
- **Response:** the same `ValidationReportDto` as `POST …/flows/{id}/validate`, computed by the same function. Nothing is written.
- **Errors:** as in the existing route, YAML that does not parse answers 400, with the parser's message carrying the line and column. The editor turns that message into a marker at that position and a Problems entry. A document that parses but does not validate answers 200 with findings.

### 7.3 `if_updated_at` on `PUT /flows/{id}`

`UpdateFlowRequest` gains an optional `if_updated_at` (RFC 3339).
- When it is present and differs from the row's `updated_at`, the update is refused with 409, "the draft was changed since you opened it", and nothing is written.
- The comparison and the write happen in one statement (`UPDATE … WHERE id = $1 AND updated_at = $2`), so two concurrent saves cannot both pass.
- Absent, the route behaves exactly as today.
- Every writer of `flows.updated_at` is listed in the route's doc comment, because the precondition's meaning rests on them (FE-STATE-4's rule, applied to a precondition).

### 7.4 Creator names

`FlowListItemDto`, `FlowDto` and `FlowVersionDto` gain `created_by_name: Option<String>`. It is the creating or publishing user's display name, joined in the same query that reads the row. It is null when `created_by` is null or the user no longer exists. The UI shows the name, or "—" when it is null, and never fetches workspace members for it.

`openapi.json` and the generated client are regenerated with `npm run gen:api` (FE-DATA-1).

## 8. Frontend structure

- `frontend/src/routes/workflows/` holds the pages and `queryKeys.ts`: the key factory, prefixed `["workflows", …]` (FE-DATA-2).
- `frontend/src/components/workflows/` holds:
  - the state chip, with the 17 Kestra states mapped onto egret tokens (§8.1);
  - the multi-panel frame and its tab registry;
  - the editor tabs;
  - the run form.
- Data access goes through the orval-generated hooks for every route in §3–§7 (FE-DATA-2).

### 8.1 State colours

Kestra colours states by hue. egret maps them onto its tokens (FE-VISUAL-1). Every chip also carries the state's name and an icon, so state is never colour alone (FE-A11Y-4).

| Group | States | Token |
|---|---|---|
| pending | CREATED, SUBMITTED, QUEUED, RESTARTED, RESUBMITTED | `muted` |
| running | RUNNING, RETRYING, KILLING | `primary` |
| success | SUCCESS | `success` |
| warning | WARNING, PAUSED, BREAKPOINT | `highlight` |
| failed | FAILED, KILLED | `destructive` |
| neutral | CANCELLED, SKIPPED, RETRIED | `muted-foreground` |

Flow states use the same chip: draft is `muted`, live is `success`, disabled is `muted-foreground`.

## 9. Comparing against Kestra

During implementation, a temporary Kestra runs from the local `kestra/kestra` image (`server local`) on a free port. It is removed afterwards and never touches egret's dev stack. The editor (panels, tab moves and splits, the docs panel, validation timing), Revisions and the run form are compared for structure and interaction (FE-REF-2), with screenshots or DOM notes in the task report. Differences go into §2.

## 10. Error handling

- Every data view has loading, empty and error states (FE-STATE-1).
- A mutation failure shows the server's message (FE-ACTION-3).
- A 403 for the feature or a permission renders the existing forbidden page.
- A 404 for a flow renders "Flow not found" with a link to the list.
- Schema fetch failure: the editor still works without completion, and shows "Autocomplete unavailable" with retry. Server validation still runs.
- Validation request failure: Problems shows the error (§5.2). Publish stays disabled with the reason "Validation unavailable".

## 11. Testing

- **Rust:**
  - the schema's type set equals the registry's, per feature combination;
  - the feature filtering;
  - aliases;
  - the ETag;
  - validate-without-id answers the same report as validate-by-id for the same source;
  - `if_updated_at`: match, mismatch with nothing written, absent, and two concurrent saves of which exactly one succeeds;
  - permissions on the three routes.
- **Vitest,** rendering real components (FE-TEST-1):
  - the layout functions (move, split, close, reopen, invalid stored layouts);
  - the namespace tree (grouping, a namespace that is also a parent, counts, filter by subtree);
  - the list's three states and its empty-versus-filtered distinction;
  - finding-path → YAML range mapping;
  - the Docs lookup of the enclosing `type:`;
  - Save, Publish and Run enablement with reasons;
  - the 409 dialog;
  - the draft indicator going out when the content matches again;
  - each input type's control and its submitted value;
  - server field errors and kept values;
  - Revisions restore with its confirm.
- **Playwright,** on a local stack and never the shared dev stack (FE-TEST-2):
  - create → completion appears → an error is marked and listed → Save → Publish → Run → the execution page;
  - drag a tab to split a panel, and reload to see the layout restored;
  - the keyboard alternative;
  - a revisions diff.
- **Bundle tests (FE-ROUTE-2):** the Workflows routes and `monaco-yaml` stay out of the entry chunk.
- **Gate (FE-TEST-4):** `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, the bundle tests; and `cargo test -p egret-workflow`, clippy and fmt for the backend.

## 12. Plan shape

1. Backend: §7.1, §7.2, §7.3, openapi and `gen:api`.
2. Frame: nav group, lazy routes, `queryKeys`, the state chip, the flow list with the namespace tree, and the placeholder pages.
3. The multi-panel frame and the tab registry.
4. The Code tab (monaco-yaml, schema, server validation, path mapping), Docs and Problems.
5. Flow detail header: Save with `if_updated_at`, Publish, Delete, the draft indicator, and the create page.
6. The run form and the minimal execution page.
7. Revisions and the diff.
8. End to end, the Kestra comparison, and the docs (this spec's §2 and §15 brought up to date).

## 13. Out of scope

- Everything in W7b–W7e. No-code editing, blueprints, plugin docs pages and AI.
- Flow import and export, bulk actions on flows, and pagination of the flow list.
- `dependsOn` inputs (§6.2).
- Kill and restart.
- Real-time updates beyond polling.

## 14. Rules this package must meet

All of `docs/en/frontend-rules.md`. The ones most at stake here:
- FE-PAGE-1/2/3/4;
- FE-STATE-1–4;
- FE-ACTION-1–5;
- FE-VISUAL-1/3/4;
- FE-FORM-2/3/4;
- FE-DATA-1/2/3;
- FE-ROUTE-1/2/3;
- FE-A11Y-3/6;
- FE-REF-1/2;
- FE-TEST-1/2/4.

Every task review answers the rulebook's reviewer checklist.

## 15. Known limits

- The flow list is unpaginated (§4).
- `dependsOn` inputs are static (§6.2).
- A finding whose `path` cannot be mapped is shown on line 1 (§5.2).
  - Server paths name list items by id (`tasks.<id>.…`, `inputs.<id>`). A repeated id maps to the first item the rest of the path fits; when every one fits (`tasks.dup.type`), that is the first. A child task whose id is also a property of its parent, and an input id containing dots, are resolved by trying each reading.
  - A parse error the server could not locate is shown on line 1, with its message and no path.
  - A path into text whose surroundings have never parsed (no earlier source parsed either) maps to line 1 until the document parses.
