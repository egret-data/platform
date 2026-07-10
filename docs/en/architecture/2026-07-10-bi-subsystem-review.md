# BI ("Explore") subsystem — architecture review

> **Status:** Review / assessment · **Date:** 2026-07-10 · **Type:** internal
>
> A three-facet assessment of the BI/Explore subsystem — backend (`crates/egret-bi`), data model (migrations + stores), and frontend (`frontend/src` chart/dashboard/explore). Produced by parallel read-only audits. Purpose: is the BI feature design sound, and what should we schedule to pay down?

---

## Verdict

**Sound, with bounded debt.** All three layers are well-founded; the debt is in **code organization** and a few **correctness gaps**, not structural rot. Nothing here calls the design problematic. This document ranks the findings so they can be scheduled.

## What is genuinely well-designed (keep the direction)

- **Backend** — `crates/egret-bi/chartquery.rs` is a pure `QuerySpec → SQL` compiler, injection-hardened (enum `FilterOp`; type-validated `sql_literal` isolated at `chartquery.rs:135-187`; ~24 inline tests incl. an injection test). `typegen.rs` is a single source of truth for engine-type classification. Per-entity repos are thin and consistent (soft-delete, bookmark, conflict mapping). The **"backend compiles SQL, frontend renders"** split is deliberate: `ch_data` returns `{ sql, database_id }` (`routes.rs:1627-1666`), the client executes + shapes + renders. Real dialect thinking (portable `GROUPING SETS` because StarRocks misparses `ROLLUP(a),ROLLUP(b)` — `chartquery.rs:408-457`).
- **Data model** — the dataset semantic layer (`dataset_columns`, `dataset_metrics`, `0012`/`0013`) and dashboard layout (`dashboard_cards` grid geometry as real columns, `dashboard_tabs` a real table) are **properly normalized — better than Metabase's JSON-heavy equivalents**. Consistent `workspace_id NOT NULL … ON DELETE CASCADE` with no cross-tenant path (**tenant-safe**). Migration hygiene is disciplined (the `0062–0067` per-seam schema splits are deliberate, not churn). `access_grants` (`0068`) already reserves `scope_type='object'` + `object_type` + `scope_id`.
- **Frontend** — excellent generated-API (orval) discipline: **zero hand-rolled `fetch` in feature code**, a single mutator chokepoint, no client drift. Near-zero `any` (one in the whole chart/explore surface). Heavy logic (pivot `engine.ts`, dashboard `params.ts`, echarts `options/*`) is **pure and unit-tested** (74 tests under `components/chart`, 159 FE test files total). Faithful Metabase fidelity — palette/style constants lifted from Metabase source with attribution; 19 viz types; a full Metabase-shaped dashboard model (grid, tabs, cross-filters, click-behavior, per-card overrides). `ChartRenderer` reused by both editor preview and dashboard cards.

## Findings, ranked

### Correctness (fix soon; cheap)

**C1 — `UNIQUE(workspace_id, name)` conflicts with soft-delete.** `charts`/`dashboards`/`datasets` have `*_name_uniq UNIQUE(workspace_id, name)` (`0014`/`0015`/`0012`) **and** later `deleted_at` soft-delete (`0019`/`0022`). A trashed item still occupies its name → a user can't create a new one with that name, and restore-from-trash can conflict. **Latent bug.** Fix: make the unique index **partial** — `… UNIQUE (workspace_id, name) WHERE deleted_at IS NULL`. (Also worth reconsidering workspace-wide name uniqueness for a multi-user tool — two analysts can't both make "Revenue".)

**C2 — Chart `params`/`viz_type` are unvalidated opaque JSONB.** `analytics.charts.params jsonb` (`0014`) is persisted verbatim (`chart.rs:20-21`, `routes.rs:1481-1482`) with no enum, no schema, no validation. Critically, the *validated* `QuerySpec` builder (`chartquery.rs`) is a **separate, unlinked type** — a saved chart's config is never checked against the builder that will consume it, so nothing guarantees a persisted chart can actually produce SQL, and config isn't queryable ("which charts use metric X?"). This is the BI incarnation of the known "JSONB-then-normalize" debt. Recommend at least a **versioned, validated envelope** for `params`. (Same, more defensibly, for `dashboards.parameters` filter defs.)

### Maintainability (high-value refactors)

**M1 — `routes.rs` is a 2621-line monolith.** It holds `BiState`, all five routers, ~40 handlers, DTO helpers, and five `#[derive(OpenApi)]` structs; section comments do the job of module boundaries. Compounded by the authz+error line `egret_core::authz::require(&*st.pdp, &ctx, PERM).await.map_err(BiApiError::from)?` copy-pasted ~40×. Split into `routes/{sqllab,dataset,chart,dashboard,folder}.rs` and introduce a guard combinator/extractor.

**M2 — `ChartExplore.tsx` is a 986-line god-component** (37 `useState`, ~11 per-viz config objects). The same per-viz branching is duplicated across **six** sites (hydration effect, `specForRun`, `run` validation, `paramsForSave`, `resetControls`, the JSX panel). There is **no editor-side viz registry** — adding a viz type means editing all six plus `ChartRenderer`, `chartTypes.ts`, and the API types. Highest-value FE refactor: extract a **per-viz descriptor/registry** owning config-state shape, spec projection, validation, save-params, and render.

### Medium

**Md1 — Polymorphic `bookmarks` (no FK) leak orphan rows.** `bookmarks(entity_type, entity_id)` (`0020`) has no FK on `entity_id`. `dashboard.rs:203` purges bookmarks on hard-delete; `chart.rs`/`dataset.rs` deletes do **not** → orphan accumulation + potential stale resurfacing on UUID reuse. Centralize the cleanup (trigger or shared purge).

**Md2 — Missing indexes.** No index on `charts.folder_id` / `dashboards.folder_id` / `datasets.folder_id`, nor on `dashboard_cards.chart_id` / `tab_id`. The `chart_id → charts ON DELETE CASCADE` fires a seq scan of `dashboard_cards` per chart delete; folder-filtered lists seq-scan. List queries also compute counts via per-row correlated subqueries (`dashboard.rs:153`, `dataset.rs:159`). Tolerable at current cardinality; add before scale.

**Md3 — Editor bypasses the renderer dispatcher.** `ChartExplore` renders `ChartTable`/`PivotTable` directly (its preview) while using `ChartRenderer` for other viz → table/pivot prop-wiring exists in two places that can drift. And `ChartRenderer` (251 lines) is a 7×-repeated `<div>{empty?…:…}</div>` if-chain. Consolidate the preview onto `ChartRenderer`; collapse the if-chain with a per-viz descriptor.

**Md4 — SQL builder + dialect handling has an extensibility ceiling.** `build()` (`chartquery.rs:355-485`) is one ~130-line `format!`-based function with no intermediate AST; metric/`HAVING` filters and joins are explicitly deferred (`chartquery.rs:280`). Dialect `match engine` arms are spread across `chartquery.rs`, `schema.rs`, `expr.rs`, `typegen.rs` with no central `Dialect` trait (Rust's exhaustive `match` is the safety net). Fine today; will resist growth toward joins/HAVING/a third engine.

### Product ceilings (roadmap, not defects)

**P1 — Flat, per-type folders vs Metabase nested collections.** `folders` (`0021`) has no `parent_id` and is partitioned by `entity_type` (`UNIQUE(workspace_id, entity_type, name)`). Two limits: no hierarchy/nesting for large workspaces; a folder can't hold mixed types. **Directly relevant to the in-flight B2 object-scope work:** object grants will be per-object; folder-inheritance works but is confined to flat, single-type folders. If real Metabase-collection parity matters, nested (polymorphic) collections is the change.

**P2 — `saved_queries` is a second-class Explore citizen.** No `folder_id`, no `deleted_at`, no bookmarks (`0010`), unlike every other Explore entity — the "Explore resource" abstraction is not uniform.

**P3 — Frontend scope gaps vs Metabase.** No GUI query-builder / "notebook" join-and-aggregate editor (metrics are pre-defined on the dataset, not composed in-editor); alerts/subscriptions/pulses are a route `Placeholder`; the heavy editor isn't lazy-loaded. Scope gaps, not design flaws.

## The one architectural boundary worth revisiting deliberately

The **"backend compiles SQL; frontend does all result-shaping + rendering"** decoupling is why adding a chart type costs zero backend work — and also why the backend offers **zero guardrails on chart config** (the root of C2). That boundary is defensible, but it is the single place worth a conscious decision: e.g. validate `params` server-side (structure/refs) **without** pulling rendering back to the server.

## Suggested schedule

- **This week (cheap, correctness):** C1 partial unique index · Md2 folder/card indexes · Md1 centralize bookmark cleanup.
- **High-value refactors:** M1 split `routes.rs` + authz guard combinator · M2 `ChartExplore` viz-registry (and Md3 fold the table/pivot preview into `ChartRenderer`).
- **Deeper decision:** C2 version + validate chart `params` (+ revisit the backend-guardrail boundary above).
- **Roadmap:** P1 nested collections if Metabase-parity matters at scale · P2 bring `saved_queries` into the Explore-resource abstraction · Md4 introduce a query AST / `Dialect` trait when joins/HAVING/a 3rd engine land.

---

*Assessment 2026-07-10. The BI design is fundamentally sound across backend, data model, and frontend; the debt is organizational (M1/M2) plus a small set of correctness gaps (C1/C2) and roadmap ceilings (P1–P3). None is structural. Related to the in-flight Effort B / B2 object-scoping work: the flat-folder finding (P1) confirms object grants will be per-object with limited folder-inheritance.*
