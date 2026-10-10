# W7a — Workflows UI part 1 (flows): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the browser, a user lists flows grouped by a namespace tree, writes a flow in a Kestra-style multi-panel editor, saves and publishes it, reads its revisions, and runs its live version. Three small backend additions support this: the schema endpoint, id-less validation and stale-save refusal.

**Architecture:**
- **Backend** (`crates/egret-workflow`):
  - A schema module assembles a flow JSON schema from a committed export of Kestra's schema, a hand-written schema for egret-only types, and the registry, filtered by the workspace's features.
  - Two routes are added: `GET …/workflows/schema` and `POST …/workflows/flows/validate`.
  - `PUT …/flows/{id}` gains an `if_updated_at` precondition.
- **Frontend** (`frontend/src`):
  - a lazy-loaded `routes/workflows/` area and a `components/workflows/` component set;
  - the multi-panel frame, a pure layout model with a tab registry;
  - Monaco plus `monaco-yaml` for the Code tab, with findings mapped from `path` to a YAML range by the `yaml` package;
  - all data through orval hooks and a `["workflows", …]` key factory.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa, serde_json), React 19 + TypeScript, TanStack Query 5, orval, Monaco 0.56, `monaco-yaml`, `yaml`, `@dnd-kit`, `react-resizable-panels`, vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`. **§n refers to the spec.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes and test cases to verify against the code, not text to paste. Names were read at `af258923`, so check each one before relying on it.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every task review answers its reviewer checklist. The rules at stake: FE-PAGE-1/2/3/4, FE-STATE-1–4, FE-ACTION-1–5, FE-VISUAL-1/3/4, FE-FORM-2/3/4, FE-DATA-1/2/3, FE-ROUTE-1/2/3, FE-A11Y-3/6, FE-REF-1/2, FE-TEST-1/2/4.
- **User decisions (spec header), in effect verbatim:**
  - W7 is split W7a → W7e. W7a is the nav group, flow list, flow editor, Revisions and Run.
  - The flow list groups by a namespace tree, and `folder_id` is never shown.
  - The editor is the full multi-panel layout (Kestra's `MultiPanelTabs`).
  - Save and Publish are two buttons, with a content-derived draft indicator.
  - Schema plus server validation drive the editor.
  - The schema's property data comes from Kestra's export filtered to egret, plus hand-written egret-only types.
- **Routes:** under `/explore/:workspace/workflows/`, all lazy (`React.lazy`), behind `RequireFeature feature="workflows"`.
- **Query keys** come from `frontend/src/routes/workflows/queryKeys.ts`, prefixed `["workflows", …]`.
- **The layout `localStorage` key** is `egret.wf.layout.<flow uuid>`. Every access is wrapped, and anything invalid falls back to the default.
- **Default layout:** `[Code] | [Docs, Problems]` at 60/40. The Code tab is not closable.
- **Server validation** debounces at 600 ms, and a newer request cancels the older one.
- **The Docs tab** debounces cursor moves at 150 ms.
- **The minimal execution page** polls every 2 s while the execution is non-terminal.
- **New dependencies,** pinned exactly: `monaco-yaml` and `yaml`. Both load only with the editor; a bundle test asserts this.
- **API client:** regenerate with `npm run gen:api` only, never bare `orval`. `openapi.json` must be current.
- **Never touch the shared dev stack.**
  - Backend tests use the test DB on the branch's own Postgres container.
  - Playwright uses a local stack started for this branch.
  - The Kestra comparison uses a temporary container on a free port, deleted afterwards.
- **Rust:** never run two workspace suites at once (wait while `pgrep -x cargo` is non-empty).
- **Frontend:** no prettier.
- **Git:** never commit `.superpowers/`; never override the author; the trailer is `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; do not push.
- **Docs** are bilingual. English is canonical, and the Vietnamese twin uses mirrored paths.

## Gates

- **Backend tasks:**
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`;
  - `cargo test -p egret-workflow`;
  - `cargo test -p egret-server --lib` (the openapi-current check).
- **Frontend tasks,** run from `frontend/`:
  - `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`;
  - every `test:bundle*` script.
  - A ChartExplore 5 s timeout under load is a known flake. Re-run that file alone and report both runs.

---

### Task 1: Backend — the flow schema, id-less validation, and `if_updated_at`

**Files:**
- Create:
  - `crates/egret-workflow/src/spec/schema/mod.rs`
  - `crates/egret-workflow/src/spec/schema/kestra-flow-schema.json` (the export)
  - `crates/egret-workflow/src/spec/schema/egret-task-schemas.json`
  - `crates/egret-workflow/src/spec/schema/README.md`, which records the export command, the Kestra version and the property-treatment measurement
- Modify:
  - `crates/egret-workflow/src/spec/mod.rs`
  - `crates/egret-workflow/src/routes.rs`: two routes, `UpdateFlowRequest`, and `update_flow`
  - the flow repository's update query (find it from `update_flow`)
  - `openapi.json`, regenerated by the server's openapi test or command as the repo does it
- Test:
  - `crates/egret-workflow/tests/flow_schema.rs`
  - additions to the existing flow route tests (find the file holding `update_flow`/`validate_flow` tests)

**Interfaces:**
- Produces:
  - `pub fn flow_schema(features: &SchemaFeatures) -> serde_json::Value`;
  - `pub struct SchemaFeatures { pub kestra_plugins: bool, pub containers: bool }`;
  - the routes `GET /api/ws/{workspace}/workflows/schema` (operationId `workflowFlowSchema` or the repo's convention) and `POST /api/ws/{workspace}/workflows/flows/validate`;
  - `UpdateFlowRequest.if_updated_at: Option<DateTime<Utc>>`.

- [ ] **Step 1: Export Kestra's schema.**
  - Start the local image on a free port. Use `docker images kestra/kestra` to find it, and `docker run -d --name w7a-kestra -p <port>:8080 <image> server local`.
  - Wait for health, then `curl /api/v1/configs`, and record the `version`.
  - `curl -o …/kestra-flow-schema.json /api/v1/plugins/schemas/flow`.
  - Remove the container: `docker rm -f w7a-kestra`.
  - In `README.md`, record the command, the version, the image ID, the date and the file size. If the version is not `2.0.0-rc13` (the programme's pinned artifact), say so in the README and in the report; it is not a blocker.
  - If the export is over about 5 MB, keep only the definitions reachable from the types egret accepts (Step 3's filter), done by a committed script `crates/egret-workflow/src/spec/schema/trim.py`. The committed JSON must be reproducible from the raw export by that script.

- [ ] **Step 2: Measure how egret treats each documented property.** For every type egret accepts that has a Kestra definition, classify each Kestra property as `honoured`, `ignored` or `refused`, using egret's parser and validator.
  - The method: for each property, build a minimal flow carrying it, then run `parse` + `validate`, and where cheap, the handler's property read.
  - Record the method and the result table in `README.md`.
  - Known input: the seven root task properties (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`) are dropped on non-script tasks.
  - If egret has no reliable way to tell `honoured` from `ignored` for leaf tasks, stop and report NEEDS_CONTEXT, with what you found.

- [ ] **Step 3: Write the tests (red).** In `tests/flow_schema.rs`:
  1. **Type set equals the registry.** For each of the four combinations of `SchemaFeatures`, the set of canonical `type` values in `flow_schema(..)` (tasks, triggers and flowables) equals the set the registry and publish accept with those features. Read the acceptance from the same functions publish uses, not from a copied list.
  2. **Aliases are listed.** For example, `core.log` appears beside `io.kestra.plugin.core.log.Log`, and its description names the FQCN.
  3. **Feature filtering.** With `kestra_plugins: false` no `io.kestra.plugin.jdbc.*` type appears. With `containers: false` no script type appears.
  4. **No ignored property.** No served type definition contains a property classified `ignored` in Step 2.
  5. **Outputs.** Every type with a known outputs list carries `x-egret-outputs`.
  6. **egret-only types.** `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http` and `core.slack` each have a definition with a description and at least their required properties.

  Route tests:
  - **`GET …/workflows/schema`:**
    - 200 for `workflow:read`;
    - an `ETag` that is stable across two calls and differs between two workspaces with different features;
    - `If-None-Match` answers 304.
  - **`POST …/workflows/flows/validate`:**
    - for the same source, the same report as `POST …/flows/{id}/validate`;
    - an unparsable source answers 400 with "line" and "column" in the message;
    - it requires `workflow:manage`, and read-only answers 403;
    - nothing is written: the flows row count is unchanged.
  - **`if_updated_at`:**
    - a match updates;
    - a mismatch answers 409 "the draft was changed since you opened it", and the row is unchanged;
    - absent, it behaves as today;
    - two concurrent `PUT`s with the same `if_updated_at` result in exactly one 200 and one 409. Use two connections and a barrier.

  Run them and confirm they fail.

- [ ] **Step 4: Implement.**
  - The schema module reads both JSON files with `include_str!`, filters them, merges, and caches per `SchemaFeatures` (a `OnceLock` per combination).
  - The `ETag` is a hash of the served bytes.
  - `validate_flow_source` is the body of today's `validate_flow`, minus the containment read. Both routes call it.
  - The update query becomes `UPDATE … WHERE id = $1 AND workspace_id = $2 [AND updated_at = $3]`. Zero rows with `if_updated_at` present, on a row that exists, answers 409; a missing row still answers 404. Do this in one statement or one transaction; never read then write.
  - List every writer of `flows.updated_at` in `update_flow`'s doc comment. Find them with a grep for `updated_at` across the repository and migrations.

- [ ] **Step 5: Regenerate the API and run the gates.**
  - Regenerate `openapi.json`, then run `npm run gen:api` in `frontend/` and commit the generated client changes.
  - Run the backend gates and `npx tsc -b`.

- [ ] **Step 6: Commit.** `feat(workflow): flow JSON schema endpoint, id-less validation, and stale-save refusal for drafts`.

### Task 2: The Workflows frame and the flow list

**Files:**
- Create:
  - `frontend/src/routes/workflows/WorkflowsRoutes.tsx`: the lazy area entry, holding the sub-routes of §3.
  - `frontend/src/routes/workflows/queryKeys.ts`
  - `frontend/src/routes/workflows/FlowsList.tsx`
  - `frontend/src/routes/workflows/ExecutionsPlaceholder.tsx`
  - `frontend/src/components/workflows/StateChip.tsx`
  - `frontend/src/components/workflows/namespaceTree.ts`
  - `frontend/scripts/assert-workflows-lazy.mjs`, plus a `test:bundle:workflows` script in `package.json`.
- Modify:
  - `frontend/src/App.tsx`: one lazy `workflows/*` route under the explore routes, wrapped in `RequireFeature feature="workflows"`.
  - the module list that `EXPLORE_MODULES` lives in (`grep -rn EXPLORE_MODULES frontend/src`), and `hooks/useWorkspaceFeatures.ts` if the gate needs it.
  - `components/layout/Sidebar.tsx` and its test, which loses the "no Workflows" assertion.
- Test:
  - `frontend/src/components/workflows/__tests__/namespaceTree.test.ts`
  - `.../StateChip.test.tsx`
  - `frontend/src/routes/workflows/__tests__/FlowsList.test.tsx`
  - the Sidebar test

**Interfaces:**
- Produces:
  - `workflowsKeys` with `all`, `flows(params)`, `flow(id)`, `versions(id)`, `version(id, v)`, `schema()`, `execution(id)`;
  - `buildNamespaceTree(flows: FlowListItemDto[]): NamespaceNode[]`, where `NamespaceNode` is `{ segment, path, flows, children, total }`;
  - `<StateChip state={string} kind="execution"|"flow" />`, mapping states as in §8.1.

- [ ] **Step 1: Tests (red).**
  - **`namespaceTree`:**
    - `a`, `a.b` and `a.b.c` nest;
    - a namespace that is both a leaf with flows and a parent puts its own flows before its children;
    - `total` counts descendants;
    - siblings sort by segment;
    - input order does not matter: the fixture is deliberately unsorted.
  - **`StateChip`:** every one of the 17 execution states and 3 flow states renders its name, an icon (an `aria-hidden` svg) and the token class from §8.1. Assert the class through the DOM; do not recompute the mapping in the test.
  - **`FlowsList`:**
    - loading shows a skeleton;
    - an error shows the error row, never empty;
    - an empty unfiltered list shows `EmptyState` with "New flow";
    - a filter with no match says so and offers to clear it;
    - rows group under the tree, and collapsing hides the children;
    - the search, namespace and state filters are written to the URL and read back on reload.
  - **Sidebar:**
    - the Workflows group appears with Flows and Executions when `workflows` is enabled;
    - it is absent when disabled.

- [ ] **Step 2: Implement.**
  - **List shape:** `ListPageShell` → toolbar → `SummaryStrip` → the grouped table (FE-PAGE-1/4).
  - **Collapse state** is stored per viewer in `localStorage`, wrapped.
  - **Lazy loading:** `WorkflowsRoutes` is imported with `lazy()` in `App.tsx`, with a skeleton fallback.
  - **The bundle script:** written after `assert-monaco-lazy.mjs`. It asserts a marker unique to the workflows area is absent from the entry chunk and present in a non-entry chunk. A good marker is a string literal from `FlowsList.tsx` that survives minification.

- [ ] **Step 3: Gates, then commit.** `feat(workflows): Workflows nav group, lazy area, and the flow list grouped by namespace`.

### Task 3: The multi-panel frame and the tab registry

**Files:**
- Create, all under `frontend/src/components/workflows/panels/`:
  - `layout.ts` (pure)
  - `registry.ts`
  - `MultiPanel.tsx`
  - `PanelTabStrip.tsx`
  - `usePersistedLayout.ts`
- Test:
  - `panels/__tests__/layout.test.ts`
  - `MultiPanel.test.tsx`
  - `usePersistedLayout.test.ts`

**Interfaces:**
- Produces:
  - `type Layout = { panels: { id: string; size: number; tabs: TabId[]; active: TabId }[] }`;
  - the pure functions in `layout.ts`:
    - `moveTab(l, tab, toPanel, index)`
    - `splitTab(l, tab, besidePanel, side: "left"|"right")`
    - `closeTab(l, tab)`
    - `reopenTab(l, tab, panel)`
    - `resize(l, sizes)`
    - `normalise(l, known: TabId[]): Layout`
  - `registerTab({ id, title, icon, closable, render })` and `getTabs()`;
  - `usePersistedLayout(flowKey: string, defaults: Layout): [Layout, (l: Layout) => void]`;
  - `<MultiPanel layout onChange context />`, where `context` is passed to every tab's `render`.

- [ ] **Step 1: Tests (red).**
  - **`layout.test.ts`:**
    - move within and across panels;
    - split left and right creates a panel and re-balances sizes so they sum to 100;
    - moving or closing the last tab of a panel removes it;
    - closing Code is refused (returns the layout unchanged);
    - reopen adds the tab to the given panel;
    - `normalise` drops unknown tabs and empty panels, adds a missing Code tab back to the first panel, de-duplicates a tab listed twice, and fixes an `active` that is not in its panel.
  - **`usePersistedLayout`:**
    - round trip;
    - an unparsable stored value gives the defaults;
    - a value with unknown tabs is normalised;
    - a `localStorage` that throws on get or set still works in memory.
  - **`MultiPanel`:**
    - renders a `tablist` per panel with arrow-key navigation;
    - the tab menu's "Split right", "Move to panel…" and "Close" call through to the layout;
    - the "+" menu lists closed tabs;
    - the Code tab has no Close item.

  Pointer drag is verified in the browser in Task 8, not in jsdom.

- [ ] **Step 2: Implement.**
  - Use `react-resizable-panels` for sizes and `@dnd-kit` (with its keyboard sensor) for tab drag.
  - **Drop targets:** a tab strip (move), and a panel's left or right edge zone (split).
  - **Persistence:** `usePersistedLayout` writes on change, debounced at 300 ms.

- [ ] **Step 3: Gates, then commit.** `feat(workflows): multi-panel editor frame with movable, splittable tabs and a tab registry`.

### Task 4: The Code, Docs and Problems tabs

**Files:**
- Create, under `frontend/src/components/workflows/editor/`:
  - `CodeTab.tsx`
  - `DocsTab.tsx`
  - `ProblemsTab.tsx`
  - `yamlPath.ts`, which maps a finding path to a range and finds the type at a position
  - `useFlowSchema.ts`
  - `useServerValidation.ts`
  - `monacoYaml.ts`, which configures `monaco-yaml`, imported only from the lazy Monaco graph
- Modify:
  - `frontend/package.json` (`monaco-yaml`, `yaml`)
  - `scripts/assert-monaco-lazy.mjs`, or a new script, so that `monaco-yaml` is also asserted lazy
- Test: `editor/__tests__/yamlPath.test.ts`, `ProblemsTab.test.tsx`, `DocsTab.test.tsx`, `useServerValidation.test.ts`

**Interfaces:**
- Consumes: `registerTab` and `MultiPanel`'s `context` (Task 3); `workflowsKeys.schema()` (Task 2); the schema and validate routes (Task 1).
- Produces:
  - `rangeOfPath(source, path): { startLine, startCol, endLine, endCol } | null`;
  - `typeAt(source, offset): string | null`;
  - `useServerValidation(source, flowId?)`, returning `{ status, findings, error, retry }`;
  - an `EditorContext` holding `{ source, setSource, findings, schema, focusRange }` and shared by the tabs.

- [ ] **Step 1: Tests (red).**
  - **`yamlPath`:**
    - `id`, `tasks[0].type` and `tasks[2].retry.maxAttempt` map to their exact ranges;
    - `tasks[0].tasks[1].id` maps inside a flowable;
    - `errors[0]` maps to the item;
    - a path into a missing node returns null;
    - a fixture with flow-style YAML (`{a: 1}`) and with comments maps correctly.
  - **`typeAt`:**
    - inside a task's property, it returns that task's `type`;
    - inside a nested task, the inner type;
    - on the `tasks:` key itself, null (flow level);
    - in a trigger, the trigger's type.
  - **`useServerValidation`:**
    - debounces at 600 ms with fake timers;
    - aborts the older request when a newer one starts;
    - a 400 parse error becomes one finding at the line and column parsed from the message;
    - a network failure gives `status: "error"`, not empty findings;
    - with `flowId` it calls validate-by-id, without it the id-less route.
  - **`ProblemsTab`:**
    - the three states (validating, none, error with retry);
    - items sorted errors first;
    - selecting an item calls `focusRange` with the mapped range;
    - an unmappable path is listed with its path and focuses line 1.
  - **`DocsTab`:**
    - shows the type under the cursor (its description, properties table and outputs) from a fixture schema;
    - falls back to flow-level docs;
    - "Autocomplete unavailable" with retry when the schema query errors.

- [ ] **Step 2: Implement.**
  - `CodeTab` uses the existing `components/monaco/MonacoEditor` (lazy).
  - `monacoYaml.ts` calls `configureMonacoYaml` with the schema, once per schema ETag. The yaml worker is wired the way the repo wires Monaco workers.
  - Findings set markers with the owner `egret-validate`.
  - Read-only users get `readOnly` and a banner.
  - Register the Code, Docs and Problems tabs.

- [ ] **Step 3: Gates, including the bundle tests, then commit.** `feat(workflows): YAML editor with schema completion, server validation, docs and problems tabs`.

### Task 5: The flow detail header, Save/Publish/Delete, the draft indicator, and the create page

**Files:**
- Create, under `frontend/src/routes/workflows/`:
  - `FlowDetail.tsx` (header and tabs)
  - `FlowEditPage.tsx` (the `edit` tab)
  - `FlowNew.tsx`
  - `components/workflows/PublishDialog.tsx`
  - `components/workflows/StaleSaveDialog.tsx`
  - `components/workflows/draftState.ts`
- Test: `routes/workflows/__tests__/FlowDetail.test.tsx`, `FlowNew.test.tsx`, `components/workflows/__tests__/draftState.test.ts`

**Interfaces:**
- Consumes: Tasks 2–4; the routes in §5.3, including `if_updated_at`.
- Produces:
  - `draftDiffersFromLive(draftSource, liveSource): boolean`, a byte comparison;
  - `canPublish({ dirty, errors, validationStatus })`, returning `{ ok } | { ok: false, reason }`;
  - `canRun({ liveVersionId, manage })`.

- [ ] **Step 1: Tests (red).**
  - **Draft pill:**
    - it lights after saving a change;
    - it goes **out** again after the text is edited back to the live source and saved;
    - "Unsaved changes" is separate from the pill.
  - **Save:**
    - sends `if_updated_at` equal to the loaded `updated_at`;
    - on 409 the dialog offers Reload (with a confirm that discards) and Overwrite (which re-sends without `if_updated_at`);
    - `Ctrl/Cmd+S` saves;
    - a toast confirms.
  - **Publish:**
    - disabled with "Save first" while dirty;
    - disabled with "Fix N errors to publish" when there are errors;
    - disabled with "Validation unavailable" when validation errors;
    - the dialog sends the note;
    - versions are invalidated.
  - **Run:** disabled with "Publish the flow before running it" when nothing is live.
  - **Delete:** goes through `ConfirmDeleteDialog`, then navigates to the list.
  - **Read-only user:** sees no Save, Publish or Run, and sees the banner.
  - **404:** "Flow not found" with a link back.
  - **Create page:**
    - the template is in the editor and validation uses the id-less route;
    - the first Save POSTs, replaces the URL with `flows/{id}/edit`, and keeps the text;
    - a 409 shows the server's message and keeps the text.
  - **Unknown tab:** `:tab` redirects to `edit`.

- [ ] **Step 2: Implement.** Use orval hooks and the `workflowsKeys` invalidations. The disabled reasons appear on the controls (FE-ACTION-4).

- [ ] **Step 3: Gates, then commit.** `feat(workflows): flow detail with save, publish, delete, draft indicator, and create page`.

### Task 6: The run form and the minimal execution page

**Files:**
- Create:
  - `frontend/src/components/workflows/run/RunFlow.tsx`, which chooses the container and submits
  - `run/inputControls.tsx`, with one control per input type
  - `run/inputModel.ts`, which turns the snapshot's `inputs` into form state and builds the request (JSON or multipart)
  - `frontend/src/routes/workflows/ExecutionMinimal.tsx`
- Test: `run/__tests__/inputModel.test.ts`, `RunFlow.test.tsx`, `routes/workflows/__tests__/ExecutionMinimal.test.tsx`

**Interfaces:**
- Consumes:
  - the live version's snapshot, via `GET …/flows/{id}/versions/{v}`, or the flow's `live_version_id`, whichever the API offers;
  - `POST …/flows/{id}/executions`;
  - `GET …/executions/{id}`.
- Produces:
  - `formFromInputs(inputs)`;
  - `requestFromForm(form)`, which returns `{ json } | { multipart: FormData }`;
  - `unsupportedInputs(inputs)`, which returns `{ id, type }[]`.

- [ ] **Step 1: Tests (red).**
  - **`inputModel`, for each type of the §6.1 table:**
    - the default fills in;
    - the submitted value has the right shape: ISO strings for dates, arrays for MULTISELECT and ARRAY, parsed JSON, YAML sent as text the way the backend expects (check `models/input.rs`), and a FILE turning the request into multipart with the file under its input id;
    - required and empty blocks submit;
    - FORM, REUSABLE_INPUTS and ION are reported unsupported.
  - **`RunFlow`:**
    - no inputs opens a confirm, then submits;
    - three or fewer scalar inputs open a dialog;
    - otherwise a drawer;
    - every control has a label tied by `htmlFor`;
    - server field errors show beside their field, and the values stay;
    - "Runs live v{n}; your draft is not published" appears when the draft differs;
    - an unsupported input disables Run with its name and type shown;
    - success navigates to `executions/{id}`.
  - **`ExecutionMinimal`:**
    - shows id, flow, state chip, start time and duration, and the runs list;
    - polls every 2 s (fake timers) while non-terminal and stops at a terminal state;
    - has the three states.

  Check whether the backend evaluates `dependsOn`, and record the answer for the spec's §15 in the report.

- [ ] **Step 2: Implement, then the gates and commit.** `feat(workflows): run a flow's live version with typed inputs, and a minimal execution page`.

### Task 7: Revisions

**Files:**
- Create:
  - `frontend/src/routes/workflows/FlowRevisions.tsx`
  - `components/workflows/RevisionDiff.tsx`, which uses Monaco's diff editor through the lazy Monaco graph
- Test: `routes/workflows/__tests__/FlowRevisions.test.tsx`

- [ ] **Step 1: Tests (red).**
  - **The list:** version, note, author and time, with the live version marked, and the three states.
  - **Selecting:**
    - selecting a version shows its YAML;
    - "Compare with…" picks a second version or "Current draft";
    - the diff component receives the two sources.
  - **"Restore to draft":**
    - it writes the version's source into the draft with `if_updated_at`;
    - when the draft differs from live it first asks for a confirm that names what is lost;
    - it never publishes;
    - a toast and an invalidation follow.
  - **The URL** carries `?v=` and `?compare=` (FE-PAGE-2).

- [ ] **Step 2: Implement, then the gates and commit.** `feat(workflows): flow revisions with diff and restore to draft`.

### Task 8: End to end, the Kestra comparison, and the docs

**Files:**
- Create:
  - `frontend/tests/e2e/workflows-flows.spec.ts`
  - `.superpowers/…/kestra-compare.md` (notes, not committed), summarised in the report
- Modify:
  - `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`: the §2 divergences found and the §15 limits (including the `dependsOn` answer from Task 6)
  - the programme spec's W7 row (EN and VI): "W7 is split W7a–W7e; W7a done" with the date

- [ ] **Step 1: Playwright on a local stack started for this branch,** following `frontend/tests/e2e/README.md` and the repo's local-stack recipe. Never use the shared dev stack. Write the spec, then run it green:
  1. Create a flow.
  2. Type `type: ` in a task and see a completion list containing `io.kestra.plugin.core.log.Log`.
  3. Introduce an error. It is marked, and it is listed in Problems; clicking the item moves the cursor.
  4. Save, then Publish with a note.
  5. Run with one STRING input, and land on the execution page, which reaches SUCCESS.
  6. Drag the Docs tab to the right edge of the Code panel: there are three panels. Reload, and there are still three.
  7. Use the keyboard path, "Split right" from the tab menu.
  8. In Revisions, compare v1 with the draft.

- [ ] **Step 2: Compare with Kestra (FE-REF-2).**
  - Start the temporary Kestra again on a free port.
  - Compare the flow editor's panel interactions, the docs panel, validation timing, revisions, and the run form's per-type controls, for structure and interaction.
  - Record each difference, and fold the deliberate ones into the spec's §2 with a reason (EN and VI).
  - Fix any accidental one in this task, or list it as a finding.
  - Remove the container.

- [ ] **Step 3: Update the docs, run the full gates (frontend and backend), and commit.** `test(workflows): end-to-end flow authoring; docs: W7a divergences and limits`.
