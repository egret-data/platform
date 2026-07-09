# E4 — Query / Engine-access Seam (decomposition)

> **Status:** Decomposition · **Date:** 2026-07-09 · **Epic:** E4 of the [service-ready redesign](../architecture/2026-07-08-service-ready-architecture-design.md) · **Precedents:** E1/E2/E3. Executed **subagent-driven** on branch `feat/e4-query-seam`.

## 1. What E4 is (and isn't)
Unlike E1–E3, the Query seam has **no control-plane router to extract** — `egret-database` (`/api/ws/{ws}/databases`) and `egret-engine` (`/api/ws/{ws}/queries`) are already their own crates mounted in `egret-server`. E4 is **consolidation + dedup**, per design §4.3/§5:
- **(a) dedup connectors** — `egret-database/tester.rs` **re-implements** Trino (`reqwest` `/v1/statement`) + StarRocks (`mysql_async`, a *second* `prefer_socket(false)`) just for a `SELECT 1`, duplicating `egret-engine/connector/{trino,starrocks}`. Make the connection test reuse the `Connector`.
- **(b) unify `TabularResult { columns, rows, types }` in `egret-core`** — collapses `egret_engine::model::{QueryResult, Column, ResultsPage}` + `egret_bi::model::SchemaColumn`. (Kernel/Notebook's Daft path uses neither `Connector` nor `DatabaseRepo` → OUT of scope.)
- **(c) `egret-query-api` leaf contract crate.**
- **(d) `SCHEMA query`** for `databases` + `queries` (migration 0066).
- **(e) fix `databases.workspace_id`** — currently NULLABLE (0031 left legacy register-then-grant orphans NULL), but `repo.rs::open_secret` decodes it as non-`Option<Uuid>` → NULL rows are a sqlx decode error (the known "open_secret NULL ws" flake). Databases are strictly per-workspace (ADR-19), the opposite of storage's 0050 global move.

## 2. The dependency-cycle knot
`egret-engine` → `egret-database` (engine imports `Engine`/`Database`/`ConnOptions`); `egret-database` has NO engine dep but re-implements the connectors. Reusing the `Connector` from database naively → `database`→`engine`→`database` cycle.

## 3. Locked decisions
1. **Crate topology → MERGE `egret-engine` + `egret-database` into one seam crate `egret-query`** (+ a leaf `egret-query-api` for the Port + `TabularResult`/`Database`/`Engine`/`ConnOptions` DTOs) — the E3 shape (`egret-catalog-engine` + `egret-catalog-api`). `tester` calls the `Connector` internally; no cycle. The connectors' `reqwest`/`mysql_async` deps stay in `egret-query`; the *duplicate* test path is deleted.
2. **`workspace_id` fix → DELETE the NULL orphans then `SET NOT NULL`.** `DELETE FROM databases WHERE workspace_id IS NULL` (unreachable rows — no workspace sees them, and they crash `open_secret`), then `ALTER COLUMN workspace_id SET NOT NULL`. Aligns schema with the non-`Option` decode + ADR-19.
3. **`TabularResult` → `egret-core`** (design-specified).

## 4. Parts (strangler order within E4)
| Part | Content | Risk |
|---|---|---|
| **E4a — TabularResult in core** | Define `TabularResult { columns: Vec<ColumnMeta>, rows: Vec<Vec<serde_json::Value>>, row_count, bytes_scanned, truncated }` + `ColumnMeta { name, ty }` in `egret-core`. Replace `egret_engine::model::{QueryResult, Column}` (Connector returns `TabularResult`, `describe` returns `Vec<ColumnMeta>`) and the `egret-bi` `SchemaColumn`/`describe` consumers. Foundational type change; no crate move yet. | Medium |
| **E4b — merge + dedup** | Fold `egret-engine` + `egret-database` → `egret-query` (git mv, submodules); create leaf `egret-query-api` (Port + DTOs: `Database`/`Engine`/`ConnOptions`/`QueryStatus` + re-export core `TabularResult`); make `tester::test_connection` call the `Connector` (`describe`/`execute` a `SELECT 1`) — delete the duplicate reqwest/mysql_async test path (the 2nd `prefer_socket(false)`). Rewire consumers `egret_engine::`/`egret_database::` → `egret_query::` (bi/bin/server); delete the two old crates. | High |
| **E4c — SCHEMA query + workspace_id** | Migration 0066: `CREATE SCHEMA query`; `SET SCHEMA query` for `databases` + `queries`; `DELETE FROM databases WHERE workspace_id IS NULL` + `ALTER … workspace_id SET NOT NULL`; schema-qualify the store/repo SQL; annotate cross-seam FKs (`databases.secret_id`→`workspace_secrets`, `created_by`/`workspace_id`→iam; inbound `saved_queries.database_id`/`datasets.database_id`/`notebook_engine_access.database_id`→`query.databases` from analytics/kernel). Force metastore re-embed. | Medium |

## 5. Execution
Subagent-driven (implementer → spec review → code-quality review per part). Each part: fmt + clippy(ws, all-targets) + affected tests green (DB-gated vs `egret_test`; known `last_admin_guard` flake excluded). Stacked on one branch → one squash PR (`--admin`; GitHub CI out of quota). FE URLs preserved (route strings unchanged); OpenAPI/FE codegen still deferred to E0·P4.

## 6. Deferred / out of scope
Kernel/Notebook Daft result path (no `Connector`); OpenAPI emit + FE client codegen (E0·P4); the analytics-side inbound FKs are annotated `SEAM-CROSSING` now, dropped at a real split (E5 owns those tables).
