# B2-1b — Sharing UI + object-scoped data path

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2) · **Predecessor:** [B2-1a object-scope backend](./2026-07-10-b2-1a-object-scope-design.md).
>
> Second increment of B2. Makes object-sharing **functionally complete + usable**: (1) closes the B2-1a data-access gap so a shared user actually sees a shared dashboard/chart's **data**, and (2) adds the Metabase-style **sharing UI**.

---

## 1. Goal & scope

B2-1a made object-shared users able to GET a dashboard/chart **definition** but not its **data** — `ch_data` (build SQL) and query `submit` (run SQL) are workspace-scoped, so an object-guest gets 403 fetching data. B2-1b closes that gap with **object-scoped, server-run data endpoints** for saved objects (Option B), and ships the **sharing modal** that drives the B2-1a share API.

**In scope:**
- Backend: `POST /ws/{ws}/charts/{id}/data` and `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` — object-scoped endpoints that run the **saved** object's own query server-side and return rows.
- Frontend: switch the view/render pipeline (dashboard cards + viewing a saved chart) onto those endpoints; the editor keeps `ch_data`. The Metabase-style **sharing modal** (people/group picker, Viewer/Editor, inherited-vs-direct) on dashboards/charts/datasets/folders.

**Out of scope:** dataset data-preview object-scoping and object-scope for notebooks/saved-queries/warehouses (B2-2); **per-user data-plane delegation** — data still runs on the workspace's DB connection, not the caller's identity (B3); role hierarchy / explicit deny (B2-3); the legacy-table flip (B2-flip); nested collections.

## 2. Backend — object-scoped saved-object data endpoints (Option B)

### 2.1 Why not the arbitrary-spec path
`ch_data` accepts a client-supplied `ChartDataBody { dataset_id, query_spec }` and returns SQL for the client to run. Authorizing that at object scope is **unsafe** — a user shared one chart could POST a *different* spec/dataset and run arbitrary queries. So shared rendering must run the object's **own saved query**, server-side, never a client-supplied spec.

### 2.2 The enabling fact: `params.spec` IS the `QuerySpec`
A saved chart's `params` JSONB already stores the query under a `spec` key (the FE's `paramsForSave()` returns `{ spec: <QuerySpec>, …viz-config }` for every viz type). So the backend can **deserialize `params.spec` directly into the Rust `QuerySpec`** (`chartquery::QuerySpec`) — no need to re-implement the FE's params→spec projection. This keeps Option B a moderate change and avoids the C2 "params opacity" debt (we only read the already-structured `spec` sub-object).

### 2.3 `POST /ws/{ws}/charts/{id}/data`
1. Resolve the chart `ObjectRef` (via `shares::resolve`); `require_object(&*st.pdp, &ctx, "chart:read", chart_obj)`.
2. Load the saved chart → `dataset_id`, `params`; extract `params.spec` → `QuerySpec` (error if absent/invalid → the chart can't be rendered).
3. Load the dataset's columns + metrics; `chartquery::build(&spec, …)` → SQL + `database_id` (same builder `ch_data` uses).
4. **Execute server-side** on the workspace's DB connection (build the connector as `schema.rs` does, or delegate to a query-execution helper) with the chart row-limit; return `{ columns, rows }` (a `TabularResult`-shaped body).
5. The caller never supplies the spec → they can only run the chart's own query. Safe.

### 2.4 `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data`
A dashboard is composed of cards that reference `chart_id`. A user granted the **dashboard** (not each chart) must see its cards' data (Metabase semantics: dashboard access ⇒ card-data access, transitively).
1. `require_object(&*st.pdp, &ctx, "dashboard:read", dashboard_obj)`.
2. Verify the card belongs to the dashboard (`dashboard_cards.dashboard_id = {id} AND id = {card_id}`) → else `NotFound`.
3. Load the card's chart (`chart_id`) + its `params.spec` + dataset; build SQL; execute server-side; return rows. **No per-chart grant is required** — the dashboard grant is the authority (transitive). Apply the card's dashboard-parameter/filter values if present (the current dashboard-render path already interpolates params — reuse that logic server-side, or accept the resolved filter values in the request body and merge them into the spec's filters, validated against the dataset).

### 2.5 Execution note
Data runs on the **workspace** database connection (as today — queries are not yet per-user; that's B3). So an object-shared user's render uses the workspace's connection, gated only by the object grant. This is the current tenancy model; B3 later downscopes to per-user tokens.

## 3. Frontend — render pipeline + sharing modal

### 3.1 Render pipeline switch
- **Dashboard cards** (`DashboardCardView` → `useChartData`): call `POST /dashboards/{id}/cards/{card_id}/data` (returns rows directly) instead of `ch_data`+`submitQuery`. This is the path a shared viewer hits.
- **Viewing a saved chart** (read-only, not the editor): call `POST /charts/{id}/data`.
- **The chart editor** (`ChartExplore`, unsaved/editing preview): keep `ch_data`+`submitQuery` (workspace-scoped) — the editor operates on an unsaved client spec and its user has workspace `query:run`.
- `useChartData` gains a mode/endpoint selector (saved-object vs editor). Keep `ChartRenderer` the single viz dispatch (BI-review M2/Md3 direction — don't duplicate).

### 3.2 Sharing modal (Metabase-style)
A "Share" action on dashboards / charts / datasets / folders opens a permissions modal driven by the B2-1a share API (`/ws/{ws}/objects/{kind}/{id}/grants`, now in the generated client):
- **List** current direct grants (`GET …/grants`, manage-gated) — grantee (user/group) + role (Viewer/Editor).
- **Add**: a people/group picker + a Viewer/Editor dropdown → `PUT …/grants`.
- **Remove**: `DELETE …/grants/{grant_id}`.
- **Inherited vs direct**: show the item's folder grants as inherited (read-only) alongside its direct grants, so users understand cascade. (Folder grants are listed by calling the folder's `GET …/grants`.)
- Only shown to managers (the list endpoint is manage-gated → non-managers don't see the Share action, or see it disabled).
Follow the FE conventions (orval hooks, shadcn Dialog, OKLCH tokens); reuse the existing people/group pickers if present.

## 4. Security

- Data endpoints run **only the saved object's own `params.spec`** — never a client spec — so an object-shared user cannot exfiltrate beyond the chart's query. The dashboard-card endpoint authorizes on the **dashboard** and verifies the card belongs to it (no cross-dashboard card access). Any dashboard-parameter filter values accepted from the client must be validated against the dataset columns (reuse the existing filter validation) so they can't inject predicates.
- The share modal's write actions are manage-gated at the API (B2-1a); the UI merely reflects that. The list endpoint is manage-gated (B2-1a F2), so ACLs aren't exposed to viewers.
- Regenerate the FE api client for the two new data endpoints.

## 5. Testing

- **Backend:** an object-guest with a `ws_viewer` grant on chart C → `POST /charts/{C}/data` returns rows (200); on a non-granted chart → 403; the endpoint ignores any client-supplied spec (only the saved spec runs — assert the SQL/rows match the saved chart, not a spoofed body). Dashboard-card: a `ws_viewer` grant on dashboard D → `POST /dashboards/{D}/cards/{card}/data` returns rows even with no grant on the card's chart; a card_id not belonging to D → 404/403.
- **Frontend:** dashboard card + shared-chart view fetch via the new endpoints (mock returns rows); the editor still uses `ch_data`; the sharing modal lists/adds/removes grants (mocked client), shows inherited vs direct, and is hidden/disabled for non-managers. Vitest.
- Full local gate: BE `cargo test --workspace` (pdp parity unchanged), FE `lint/typecheck/test/build`, OpenAPI + client current.

## 6. Open questions (for the plan)

- Where the server-side execution lives: a small helper in `egret-bi` that builds the connector + runs SQL (mirroring `schema.rs`), or a reusable `egret-query` execution entry that egret-bi calls. Prefer reusing an `egret-query` execution function so BI doesn't re-implement connector/limit/type-coercion handling.
- Dashboard-parameter/cross-filter values: does the card-data endpoint accept resolved filter values in the body (validated), or does B2-1b keep cards unfiltered (defer dashboard-param interaction for shared viewers)? Default: accept validated filter values so shared dashboards are interactive; validate against dataset columns.
- The people/group picker: reuse an existing component (the workspace-members UI has one) vs a new one.
- `datasets` in the share modal: sharing a dataset grants read on the dataset entity, but there's no dataset "data view" endpoint in this increment — clarify what a dataset share conveys (dataset definition/columns) or drop datasets from the modal for B2-1b. Default: allow dataset shares (definition read) but note data-preview is later.

---

*Design v1.0 (2026-07-11). Closes the object-share data-access gap with server-run, object-scoped data endpoints (running the saved object's own `params.spec`), and ships the Metabase-style sharing modal. Data still runs on the workspace connection (per-user delegation is B3). One increment (backend + UI).*
