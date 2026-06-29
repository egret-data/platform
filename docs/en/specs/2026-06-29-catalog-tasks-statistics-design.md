# Catalog — warehouse Tasks + Statistics tabs — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `feat/catalog-tasks-stats` (off main)
**Program:** Sub-epic 4 of the Catalog-console-parity program (warehouse ✅ → namespace ✅ → table detail ✅ → **tasks + statistics** → permissions). Reference: the lakekeeper console warehouse "tasks" + "statistics" tabs.

## Goal
Add two **read-only** tabs to the warehouse detail — **Statistics** (table/view counts over time) and **Tasks** (the GC/maintenance task list) — backed by the lakekeeper management API.

## Background (current state)
- Warehouse detail (`WarehouseDetail.tsx`) has tabs **Browse · Access · Details**, all scoped to `{wid}` (resolved → the lakekeeper warehouse uuid via `CatalogStore::lakekeeper_id`, ownership-checked). The browse routes are gated `catalog:read` via `authorized_warehouse`.
- egret runs the lakekeeper built-in GC task queues (CE-6: `tabular_expiration`, `tabular_purge`, `task_log_cleanup`) — workers spawned at startup.
- lakekeeper management API (callable the same way egret calls `WarehouseService::create_warehouse`):
  - `get_warehouse_statistics(warehouse_id, GetWarehouseStatisticsQuery, ctx, rmd) -> WarehouseStatisticsResponse { warehouse_ident, stats: Vec<WarehouseStatistics>, next_page_token }` where `WarehouseStatistics { timestamp, number_of_tables: i64, number_of_views: i64, updated_at }`. Stats are created lazily (hourly) — a fresh warehouse may have an empty `stats`. **No size-in-bytes is provided** — counts only.
  - `list_tasks(warehouse_id, ListTasksRequest, ctx, rmd) -> ListTasksResponse` — the GC/maintenance tasks with status. (Exact `Task` field shape read at implementation time.)

## 1. Backend (`browse.rs`, under the warehouse `{wid}` scope)
Two read-only routes, gated `catalog:read` (via the existing `authorized_warehouse`, NOT the admin variant), resolving `{wid}` → lakekeeper id (cross-ws → 404):
- `GET /ws/{ws}/catalog/warehouses/{wid}/statistics` → `get_warehouse_statistics(lk_id, GetWarehouseStatisticsQuery::default(), …)` → JSON `{ entries: [{ timestamp, number_of_tables, number_of_views, updated_at }] }` (the first page of `stats`, newest first — confirm the API's ordering and reverse if needed). Empty `stats` → `entries: []`.
- `GET /ws/{ws}/catalog/warehouses/{wid}/tasks` → `list_tasks(lk_id, ListTasksRequest::default(), …)` → JSON `[{ task_id, task_type, status, entity_name, created_at, scheduled_for, attempt, last_error }]` — map from the lakekeeper `Task` fields (read the `ListTasksResponse`/`Task` struct at implementation; include the fields that exist, omit those that don't). Read-only — no `control_tasks`.

## 2. Frontend (`api/catalog.ts` + `WarehouseDetail.tsx`)
- Add hooks `useWarehouseStatistics(ws, wid)` and `useWarehouseTasks(ws, wid)` (GET the two routes; paths without `/api`). Types `WarehouseStatEntry { timestamp, number_of_tables, number_of_views, updated_at }` and `WarehouseTask { task_id, task_type, status, entity_name?, created_at?, scheduled_for?, attempt?, last_error? }`.
- Add two tabs to `WarehouseDetail.tsx` after **Details**: **Statistics** and **Tasks**.
  - **Statistics:** show the latest entry as a small stat row — **{number_of_tables} tables · {number_of_views} views · as of {timestamp}**; below it, an optional compact history table (the remaining entries: Time · Tables · Views). Empty state: "No statistics yet — lakekeeper computes these hourly."
  - **Tasks:** a table (Type · Status · Entity · Created · Attempt · Error) with a status `Badge` (running→default, success→secondary/green, failed→destructive, scheduled/pending→outline). Empty state: "No tasks." (GC tasks only appear after table expiry/purge, so empty is normal).

## 3. Testing
- **Backend (DB + MinIO + lakekeeper):** the statistics endpoint returns a 200 with `entries` (possibly `[]` for a fresh warehouse, or ≥1 with `number_of_tables` after a table exists — assert the 200 + shape, tolerant of empty); the tasks endpoint returns a 200 with a list (often `[]`); cross-workspace `{wid}` → 404; a non-member is gated. Mirror the `warehouse_api.rs` fixture.
- **Frontend (Vitest, mock `@/api/catalog`):** the Statistics tab renders the latest counts + the empty state; the Tasks tab renders rows with status badges + the empty state; both call the hooks with the `wid`.
- **Manual + full local CI** before merge: live smoke — open a warehouse, view the Statistics tab (counts reflect the seeded `sale.orders` table once stats compute, or the empty state) and the Tasks tab.

## 4. Risks
- **Statistics are lazy/hourly** — a fresh or just-seeded warehouse may show empty or stale counts; the empty state must be clear (not an error). Don't force-trigger stats computation.
- **lakekeeper `Task` shape** — read `ListTasksResponse`/`Task` carefully and map only existing fields (same discipline as the table-detail metadata mapping).
- **`{wid}` ownership** must be re-checked on both routes (cross-tenant IDOR → 404).
- Both endpoints are read-only management reads — no mutation, low blast radius.

## 5. Out of scope (later)
Task **control** (run-now / cancel via `control_tasks`); endpoint statistics; size-in-bytes metrics (lakekeeper doesn't expose them here); permissions tab (sub-epic 5); a stats time-series chart (a simple table/number suffices for now).
