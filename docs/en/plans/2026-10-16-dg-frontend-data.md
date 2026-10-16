# Governance Data (sub-project #1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Unity-Catalog-style "Data" area in Governance: a catalog tree, and an object page per catalog / schema /
table / view with Overview, Permissions, Masks & filters, Lineage and Details tabs, a column panel, and grant, tag and
"who can read" actions in place.

**Architecture:** Two new read-only control-plane routes (`governance/browse`, `governance/lineage`) in
`egret-controlplane`, behind a `CatalogLister` seam with Trino and StarRocks implementations next to the code that
already dials those engines. The frontend adds `/governance/data/*` routes and a `routes/governance/data/` module that
composes existing dialogs (`GrantDialog`, `RuleDialog`) and the existing list/explain/tag APIs.

**Tech Stack:** Rust (axum, sqlx, utoipa), React 18 + TypeScript, TanStack Query via the orval client, Radix UI,
vitest + Testing Library, Playwright MCP for the live check.

**Spec:** `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (decisions F1–F10; Part 2 is this plan).

## Global Constraints

- **Starts after the DG backend cleanup package (`feat/dg-backend-cleanup`) merges.** Rebase this branch on main
  first; regenerate the client after the rebase.
- DB tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only. Never DB `egret`. Never
  `EGRET_ALLOW_DB_SKIP=1`.
- Live runs: own `vg-` containers on `vg-net`, throwaway DB `egret_dgfe_live` in `egret-dev-postgres-1`. Never start,
  stop or modify `egret-dev-*` or `egret-w7*-postgres`. Ports 5173, 5000, 8080 are not ours.
- An engine with `origin = external` is never dialled (commit `19d7ac2db`).
- Widening any privilege of `egret-lineage` or the StarRocks service account: STOP and ask the user (F8).
- Frontend: `docs/en/frontend-rules.md` in full; egret + jsx-a11y lint; never add a file to `eslint-rules/legacy.js`;
  no prettier; OKLCH tokens as `var(--token)`; regenerate with `cd frontend && npm run gen:api` only, after
  `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`.
- Frontend gate per task: `npm run lint`, `npx tsc -b`, `npx vitest run <touched tests>`. Full gate at the end.
- Rust per task: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`.
- Never `git stash`, `git checkout -- <file>`, `git clean`. Kill only your own PIDs. ≥ 30 GiB free disk. Never commit
  `.superpowers/`. Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- UI copy is English (the app's language); keep labels short and factual. Empty states state the fact.

## File structure

Backend (`crates/egret-controlplane/src/`):
- `governance_browse.rs` — route, DTOs, engine selection, `CatalogLister` trait, the egret-catalog path.
- `governance_lineage.rs` — route and DTOs.
- `lib.rs` — `CpState.catalog_lister: Arc<dyn CatalogLister>`, router + OpenAPI merge.
- Trino lister: `crates/egret-query/src/execution/catalog_listing.rs` (beside `lineage.rs`, reusing
  `LineageTokenMinter` and `lineage_engines_via`).
- StarRocks lister: beside the real `MaterializedViewLister` (find it with `grep -rn "impl.*MaterializedViewLister"
  crates`).
- Wiring: `crates/egret-bin/src/main.rs` where `mv_lister` is built.

Frontend (`frontend/src/routes/governance/data/`):
- `DataPage.tsx` (layout: tree + outlet), `CatalogTree.tsx`, `ObjectHeader.tsx`, `ObjectTabs.tsx`,
  `tabs/OverviewTab.tsx`, `tabs/PermissionsTab.tsx`, `tabs/MasksFiltersTab.tsx`, `tabs/LineageTab.tsx`,
  `tabs/DetailsTab.tsx`, `ColumnPanel.tsx`, `WhoCanReadDialog.tsx`, `AddTagDialog.tsx`,
  `objectPath.ts` (URL ↔ object path), `coverage.ts` (which grants/rules reach an object), `__tests__/`.
- Modify: `App.tsx` (routes, index), `components/layout/Sidebar.tsx` (`GOVERNANCE_GROUPS`),
  `GrantDialog.tsx` (`initialScope`), `RuleDialog.tsx` (`initialSelector`), `DataPermissions.tsx` and
  `MaskFilterRules.tsx` ("Open in Data").

---

### Task 1: live measurement — can egret's identities list catalogs?

No product code. Decides Task 3's queries and whether F8 holds.

- [ ] Write predictions first (append `## Measurements (Task 1)` to a new
  `docs/en/plans/2026-10-16-dg-frontend-data-live.md`, commit).
- [ ] Stack as `docs/en/plans/2026-10-13-view-governance-live-result.md` §2.2 (own `vg-` containers, image from this
  branch, DB `egret_dgfe_live`), with one egret Iceberg catalog and one **external** catalog attached to both
  engines (e.g. a second Iceberg REST catalog or a JDBC/Postgres catalog — pick the cheapest that Trino and
  StarRocks both attach; label it).
- [ ] Trino JWT engine, as `egret-lineage` (mint the token the way `MachineIdpEngineMinter::mint_lineage_token`
  does): `SHOW SCHEMAS FROM <ext>`, `SHOW TABLES FROM <ext>.<s>`, `DESCRIBE <ext>.<s>.<t>`, and the
  `information_schema` equivalents. Record exact output or error.
- [ ] Trino impersonate-mode engine: which identity can list (delegator impersonating whom?) without new grants;
  record.
- [ ] StarRocks, as the account the MV lister uses: `SHOW DATABASES FROM <ext>`, `SHOW TABLES FROM <ext>.<db>`,
  `DESCRIBE <ext>.<db>.<t>` (or `SHOW FULL COLUMNS`). Record.
- [ ] If any listing needs a privilege these identities lack: **STOP, report BLOCKED with the exact privilege** (F8).
- [ ] Tear down, commit the results. Report `task-1-report.md` with the exact statements that work per engine.

---

### Task 2: the browse route, engine selection and the egret path

**Files:**
- Create: `crates/egret-controlplane/src/governance_browse.rs`
- Modify: `crates/egret-controlplane/src/lib.rs` (field, router, OpenAPI)
- Test: `crates/egret-controlplane/tests/governance_browse.rs`

**Interfaces (produces):**

```rust
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct BrowseResponse {
    pub source: BrowseSource,                 // {"kind":"egret"} | {"kind":"engine","name":..}
    pub engines: Vec<BrowseEngine>,           // every engine the catalog is attached to (name, type, origin, status)
    pub items: Option<Vec<BrowseItem>>,       // schemas, or tables/views/MVs
    pub columns: Option<Vec<BrowseColumn>>,   // when `table` is given
    pub not_listed: Option<NotListed>,
}
pub struct BrowseItem { pub name: String, pub kind: BrowseKind } // schema|table|view|materialized_view
pub struct BrowseColumn { pub name: String, pub r#type: String }
pub struct NotListed { pub reason: NotListedReason, pub detail: String }
pub enum NotListedReason { NoEngineAttached, OnlyExternalEngines, EngineNotRunning, Timeout, EngineError }

#[async_trait]
pub trait CatalogLister: Send + Sync {
    /// List one level of `catalog` (its name as the ENGINE knows it) on `engine`.
    async fn list(&self, engine: &ListingEngine, catalog: &str, schema: Option<&str>, table: Option<&str>)
        -> Result<Listing, String>;
}
pub enum Listing { Items(Vec<BrowseItem>), Columns(Vec<BrowseColumn>) }
```

Route: `GET /api/control-plane/governance/browse/{catalog_id}` with query `schema`, `table`; permission
`policy:read` OR `tag:read` (use the same check helper the governance routes use; find it with
`grep -rn "tag:read" crates/egret-controlplane/src`).

- [ ] **Step 1: failing DB tests** (fake `CatalogLister` recording calls):
  - egret Iceberg catalog → `source.kind == "egret"`, items from `CpState.browser` (`WarehouseBrowser`), lister not
    called.
  - external catalog attached to a provisioned running Trino → `source.kind == "engine"`, lister called with the
    engine-side catalog name.
  - attached only to an `origin = external` engine → `notListed.reason == only_external_engines`, lister **never**
    called.
  - attached to nothing → `no_engine_attached`; provisioned but `status != running` → `engine_not_running`.
  - lister returns `Err` → `engine_error` with a generic detail (no raw engine text); lister sleeps past the deadline
    (set `CpState` timeout to 50 ms in the test) → `timeout`.
  - no `policy:read`/`tag:read` → 403.
  - `engines` lists every attached engine with origin and status.
- [ ] **Step 2: run** `cargo test -p egret-controlplane --test governance_browse` → fails.
- [ ] **Step 3: implement.** Engine choice: provisioned + running + attached, Trino before StarRocks, then by name;
  deadline = `view_audit_timeout` unless a dedicated field is cleaner (if added, wire it in egret-bin with 15 s).
- [ ] **Step 4: run** the crate tests + clippy → pass.
- [ ] **Step 5: commit** `feat(control-plane): browse a catalog for Governance`.

---

### Task 3: the real Trino and StarRocks listers

**Files:** `crates/egret-query/src/execution/catalog_listing.rs` (Trino), the StarRocks MV-lister file (StarRocks),
`crates/egret-bin/src/main.rs` (wiring), tests beside each.

**Interfaces:** consumes `CatalogLister`, `ListingEngine` (Task 2); Task 1's statements.

- [ ] **Step 1: failing unit tests** for the statement builders and row parsers: identifier quoting (a catalog,
  schema and table name containing `"`, `` ` ``, `.` and a backslash — use the quoting helpers the engines' code
  already uses; StarRocks backslash handling per the #504 fix), the kind mapping (table / view / MV), column
  name+type parsing.
- [ ] **Step 2: run** → fail.
- [ ] **Step 3: implement** with Task 1's statements; Trino via the lineage token and `lineage_engines_via`
  connection data; StarRocks via the MV lister's connection. Never log a token or password.
- [ ] **Step 4: run** `cargo test -p egret-query -p egret-engine-mgmt` (whichever crates hold them) + clippy.
- [ ] **Step 5: commit** `feat(query): list a catalog through Trino or StarRocks for Governance`.

---

### Task 4: the lineage route

**Files:** create `crates/egret-controlplane/src/governance_lineage.rs`, modify `lib.rs`, test
`crates/egret-controlplane/tests/governance_lineage.rs`.

**Interfaces (produces):**

```rust
pub struct ObjectLineage {
    pub readers: Vec<LineageReader>,      // views/MVs that read this table
    pub view: Option<ViewLineageInfo>,    // set when the object is a view
}
pub struct LineageReader { pub catalog: String, pub schema: String, pub name: String, pub kind: BrowseKind, pub engine: Option<String> }
pub struct ViewLineageInfo {
    pub security_mode: SecurityMode, pub owner: Option<String>, pub owner_cleared: Option<bool>,
    pub lineage_state: LineageState, pub bases: Vec<LineageTableRef>,
}
```

Route `GET /api/control-plane/governance/lineage/{catalog_id}?schema=&table=`, same permission as browse. Reuse
`view_exposure`'s `SecurityMode`, `LineageState` and the recorded-lineage readers
(`egret_perm_compile::lineage::recorded_lineages`), and the MV listing for StarRocks readers (under the same
deadline; a failed MV listing sets a `readersIncomplete: { detail }` field instead of returning a partial list as
complete).

- [ ] **Step 1: failing DB tests:** a table read by two views (seed `view_lineage` rows) → both readers; a view →
  bases, mode, owner, state (`computed`, `failed`, `missing`); an MV lister error → `readersIncomplete` set; 403
  without permission.
- [ ] **Steps 2–5:** run → implement → run + clippy → commit `feat(control-plane): object lineage for Governance`.
- [ ] Regenerate `openapi.json` and the client (Global Constraints) in this task's commit; run
  `cargo test -p egret-server --lib`.

---

### Task 5: the Data shell, routes and the tree

**Files:** create `routes/governance/data/{DataPage,CatalogTree}.tsx`, `objectPath.ts`, tests; modify `App.tsx`,
`components/layout/Sidebar.tsx`.

**Interfaces (produces):**

```ts
export type ObjectPath = { catalog: string; schema?: string; object?: string; column?: string };
export function pathToUrl(p: ObjectPath, tab?: string): string;   // /governance/data/c/s/o?column=..&tab=..
export function urlToPath(params: Record<string,string|undefined>, search: URLSearchParams): ObjectPath;
```

Catalogs resolve by **name** in the URL (registry names are unique; map to `catalog_id` from the registry list).

- [ ] **Step 1: failing tests:**
  - `objectPath`: round-trip for each level, names with spaces / `%` / `/` (encoded).
  - Sidebar: Governance shows "Data" first in its group order per spec Part 1 (Data, then the existing items; Overview
    comes in #3).
  - `/governance` redirects to `/governance/data`.
  - Tree: lists catalogs with source labels; expanding a catalog calls browse and shows schemas; expanding a schema
    shows tables/views/MVs with kind icons; a `notListed` node shows its reason text and a "Go to path" control; the
    search box filters loaded nodes; "Go to path" with `ice.sales.customers` navigates there; keyboard: ArrowDown /
    ArrowUp / ArrowRight (expand) / ArrowLeft (collapse) / Enter (open), roving tabindex, `role="treegrid"`, rows
    `aria-level` / `aria-expanded` (mirror `routes/workflows/NamespacesList.tsx`).
- [ ] **Step 2: run** `npx vitest run src/routes/governance/data` → fail.
- [ ] **Step 3: implement.** Layout: tree column (resizable not required; 18rem, collapses under a toggle below
  768 px) + `<Outlet/>`. Use the generated hooks for browse and the registry catalogs.
- [ ] **Step 4: run** tests + lint + tsc.
- [ ] **Step 5: commit** `feat(governance): the Data area and catalog tree`.

---

### Task 6: object header, tabs, Overview and Details

**Files:** `ObjectHeader.tsx`, `ObjectTabs.tsx`, `tabs/OverviewTab.tsx`, `tabs/DetailsTab.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - Header: name, kind, attached engines (from browse `engines`), tags as chips with direct vs inherited
    distinguished (inherited chip says "from <level>"); action buttons shown only with permission (`policy:create` for
    Grant access, the tag-assign permission for Add tag, `policy:read` for Who can read?).
  - Tabs per level exactly as spec §2.2's table; `?tab=` selects; unknown tab falls back to Overview; Radix Tabs
    keyboard.
  - Overview: catalog → schemas + attached engines + workspaces; schema → tables/views; table → columns with type and
    tags; view/MV → plus security mode, owner cleared, lineage state (from the lineage route).
  - Details: full path, warehouse, workspaces, engines.
  - Each tab: loading, empty ("No columns returned"), and error states; an error in one tab leaves the header and
    other tabs working.
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): object header, tabs, overview and details`.

---

### Task 7: Permissions tab and `GrantDialog.initialScope`

**Files:** `tabs/PermissionsTab.tsx`, `coverage.ts`, modify `GrantDialog.tsx`, tests.

**Interfaces (produces):**

```ts
/** A grant reaches `p` when its scope's catalog matches and each set level equals p's (unset = all). */
export function grantCoverage(g: Grant, p: ObjectPath, catalogId: string):
  "here" | { inheritedFrom: "catalog" | "schema" | "table" } | "child" | null; // "child" = on a column below p
export function ruleSelects(r: Rule, p: ObjectPath, catalogId: string, tagsOnPath: string[]):
  "path" | "tag" | null;
// GrantDialog gains: initialScope?: Scope   (used only when `existing` is not set)
```

- [ ] **Step 1: failing tests:** `grantCoverage` truth table (each level × here/inherited/child/unrelated, other
  catalog → null); tab lists grants grouped Here / Inherited / On columns, deny marked; "Grant access" opens
  `GrantDialog` with the scope filled; editing an `explicit` grant opens it with `existing`; a
  `workspace_assignment` grant is shown read-only (as the list page does); after save the tab refetches.
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): permissions on the object page`.

---

### Task 8: Masks & filters tab, masking rules on columns, `RuleDialog.initialSelector`

**Files:** `tabs/MasksFiltersTab.tsx`, extend `coverage.ts` tests, modify `RuleDialog.tsx`, `tabs/OverviewTab.tsx`.

**Tag reach:** decide whether a tag on a table (or schema) reaches its columns for a tag selector by reading how
egret serves tags and how explain resolves them (`ExplainResolvedTag` in the explain response,
`egret-ranger-eval`'s tag matching, `ServiceTags::from_assignments`). Mirror exactly that; cite file:line in a
comment and in the report. Do not guess.

- [ ] **Step 1: failing tests:** `ruleSelects` truth table (path at each level; tag on the column; tag on an
  ancestor per the rule above; other catalog → null); the tab lists masks and row filters that select the object;
  "Add rule" opens `RuleDialog` with a `path` selector for this object; the Overview column row lists its masking
  rules by name and kind (no "who sees what" text, F10).
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): masks and filters on the object page`.

---

### Task 9: Lineage tab

**Files:** `tabs/LineageTab.tsx`, tests.

- [ ] **Step 1: failing tests:** a table shows its readers, each linking to its object page; a view shows its bases
  (linked), mode, owner cleared, lineage state with the state's meaning in plain words; `readersIncomplete` shows a
  warning that the list may be incomplete and why; empty → "No view or materialized view reads this table".
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): lineage on the object page`.

---

### Task 10: column panel, Who can read?, Add tag

**Files:** `ColumnPanel.tsx`, `WhoCanReadDialog.tsx`, `AddTagDialog.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - `?column=` opens the panel (Radix Dialog/Sheet, focus trapped, Escape closes and removes the param); it shows the
    column's tags, masking rules, grants on that column.
  - Who can read?: principal picker (reuse `components/perm/PrincipalPicker.tsx` in single-subject mode if it has one;
    otherwise the explain page's subject picker), calls `/perm/explain` with this object (and column), renders the
    verdict per engine using the explain page's existing result component if it can be extracted without changing its
    behaviour (otherwise a compact rendering of the same fields); the explain page's tests stay green.
  - Add tag: egret catalog → picks a tag and writes via `resource-tags`; external catalog → the control is disabled
    with "Tags can be added to egret-managed catalogs only" (F9).
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): column panel, who can read, add tag`.

---

### Task 11: "Open in Data" from the old lists

**Files:** `DataPermissions.tsx`, `MaskFilterRules.tsx`, tests.

- [ ] Each grant / path-rule row gets an "Open in Data" link to its scope's object page; tag-selector rules link to
  the classification page instead. Tests for both; existing tests stay green.
- [ ] Commit `feat(governance): open a grant or rule in Data`.

---

### Task 12: live browser check

- [ ] Stack as Task 1; vite dev server on a free port (not 5173/5000/8080), logged in as the admin.
- [ ] With Playwright MCP: browse the egret catalog and the external catalog; open a table, a view and a column;
  grant access and add a tag from the page and see them appear; add a mask rule; "Who can read?" for a cleared and
  an uncleared user, and compare with a real query on each engine; check 390 px width and keyboard-only navigation
  of the tree and tabs. Screenshots to the scratchpad.
- [ ] Record findings in `2026-10-16-dg-frontend-data-live.md` §2 (predictions first). Defects found are fixed in a
  fix round, not here.
- [ ] Tear down; commit the doc.

---

## Final gate (controller)

Whole-branch review on the most capable model; one fix dispatch + scoped re-review; rebase on fresh main; Rust gate
(`cargo fmt --check`, workspace clippy `-D warnings`, `just test-shipping-resolution`,
`cargo test --workspace --no-fail-fast` with the shared store, `just test-ranger-gates`); frontend gate
(`npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, all bundle tests).
