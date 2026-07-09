# E5 — Analytics + Kernel Seam(s) (decomposition)

> **Status:** Decomposition · **Date:** 2026-07-09 · **Epic:** E5 of the [service-ready redesign](../architecture/2026-07-08-service-ready-architecture-design.md) · **Precedents:** E1–E4. Executed **subagent-driven** on branch `feat/e5-analytics-kernel-seam`.

## 1. What E5 is
The design's largest product cluster, but the crates are **already cleanly factored** — so E5 is **schema ownership + a contract crate + one FE fix**, NOT a restructure:
- **Analytics (#5)** = `egret-bi` (dataset/chart/dashboard/SQL-lab, ~5.4k) + `egret-notebook` (notebook-DOCS, ~1.9k) — 13 tables.
- **Kernel-runtime (#6, ⚠ stateful)** = `egret-kernel` (~3.2k) — the Daft Jupyter gateway (live child procs + in-mem `ActiveExecutions` + WS proxy sessions); 2 tables (`kernels`, `notebook_kernels`).
- Coupling is **one-directional `egret-kernel → egret-notebook`** (kernel embeds `NotebookState`, calls `build_notebook_runtime` + reads the notebook store); egret-notebook has no kernel dep — the exact direction the design wants. **No crate merge/split needed.**
- egret-bi already consumes the E4 query seam (pure SQL-compiler + metadata store; `charts/data` → `{sql, database_id}`, executed via the query seam). No connector work.

## 2. Locked decisions
1. **Contract → full `egret-analytics-api` DTO crate** (leaf: analytics wire DTOs + the notebook↔kernel Port). The DTOs have no external consumer today, but per the design the crate is built; the meaningful part is the **`NotebookDocsPort`** so `egret-kernel` depends on the interface (not concrete `egret-notebook`), making the stateful kernel seam genuinely splittable (the E1/E2 reader-Port pattern).
2. **Two schemas → `analytics` + `kernel`** (distinct seams #5/#6). `notebook_kernels.notebook_id → analytics.notebooks` becomes a `kernel→analytics` cross-schema FK (annotated SEAM-CROSSING).
3. Fix the two OKLCH `ChartTable.tsx` bugs (FE — the only FE change in the whole initiative).

## 3. Parts
| Part | Content | Risk |
|---|---|---|
| **E5a — FE OKLCH fix** | `frontend/src/components/chart/ChartTable.tsx`: line 92 mini-bar `hsl(var(--destructive/primary))` → `var(--…)`; line 335 pinned-column base `hsl(var(--background))` → `var(--background)` (OKLCH tokens make `hsl(var(...))` invalid → silent transparent). Verify in the browser. | Low |
| **E5b — schema analytics + kernel** | Migration **0067**: `CREATE SCHEMA analytics; CREATE SCHEMA kernel`; `SET SCHEMA analytics` for datasets/dataset_columns/dataset_metrics/charts/dashboards/dashboard_cards/dashboard_tabs/folders/bookmarks/saved_queries/notebooks/notebook_cells/notebook_engine_access; `SET SCHEMA kernel` for kernels/notebook_kernels. Schema-qualify ALL SQL in egret-bi (dataset/chart/dashboard/saved/folder) + egret-notebook (store — incl. the shared `folders`/`bookmarks` writes) + egret-kernel (registry/notebook_kernels). Whole-workspace SQL sweep (bin's `DELETE FROM kernels`, any test seeders). Annotate cross-seam FKs (inbound `query.databases` already in 0066; `notebook_kernels.notebook_id → analytics.notebooks`; iam FKs). Force metastore re-embed. | Medium-High (15 tables, 3 crates) |
| **E5c — egret-analytics-api + NotebookDocsPort** | Leaf `egret-analytics-api` (pure-serde analytics DTOs + `NotebookDocsPort` covering the reads kernel needs: `get_summary`/`record_output`/`list_engine_options` + the `build_notebook_runtime` manifest). egret-notebook impls the Port; **egret-kernel rewired onto `Arc<dyn NotebookDocsPort>`** instead of concrete `NotebookState` (egret-notebook → dev-dep of kernel, wired in egret-bin). | Medium |

## 4. Execution
Subagent-driven (implementer → spec review → code-quality review per part) — except E5a (2-line FE fix, done directly + browser-verified). Each part: fmt + clippy(ws, all-targets) + affected tests green; FE lint/typecheck/test/build for the FE fix. FE URLs preserved (all `/api/ws/{ws}/…` route strings unchanged). Stacked on one branch → one squash PR (`--admin`).

## 5. Deferred / out of scope
FE `can()` permission codegen (Effort B); OpenAPI emit + FE client codegen (E0·P4); no egret-kernel-api (nothing consumes the kernel seam cross-crate). This completes the strangler product cluster; E6 (DataAPI/Workflow) is a scaffold-only future.
