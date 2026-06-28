# Catalog — table detail (enrich) — Design

**Date:** 2026-06-28 · **Status:** Design — approved · **Branch:** `feat/catalog-table-detail` (off main)
**Program:** Sub-epic 3 of the Catalog-console-parity program (warehouse ✅ → namespace ✅ → **table detail** → tasks/statistics → permissions). Reference: the lakekeeper console table "details" + "versioning" tabs.

## Goal
Enrich the existing table-detail view (currently location + schema columns + snapshot count + properties) to mirror the lakekeeper table **details** + **versioning** content — partition spec, sort order, snapshot history, format version, last-updated — using **only the iceberg metadata egret already loads** (no query execution).

## Background (current state)
- Browse is warehouse-scoped: clicking a table calls `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}` → the `table_detail` handler in `browse.rs`, which `load_table`s the full iceberg metadata but projects only a thin DTO.
- Current `TableDetail` (FE `api/catalog.ts`): `{ name, location, current_schema_id, columns: CatalogColumn[], current_snapshot_id, snapshot_count, properties }`. `CatalogColumn { name, type, required }`. The FE `TableDetailView` (in `CatalogBrowse.tsx`) renders the location, a Column·Type·Nullable schema table, the snapshot count, and a properties table — a single scrollable panel.
- The lakekeeper `load_table` response carries the full `TableMetadata`: schema(s) + `current_schema_id`, `partition_specs` + `default_spec_id`, `sort_orders` + `default_sort_order_id`, `snapshots` (history) + `current_snapshot_id`, `properties`, `format_version`, `location`, `last_updated_ms`.

## 1. Backend (`browse.rs` `table_detail` + the `TableDetail` DTO)
Project these additional fields from the already-loaded `TableMetadata` (read the exact lakekeeper `LoadTableResult`/`TableMetadata` accessors when implementing — the plan pins them):
- **Overview:** `format_version: i32`, `location: String` (kept), `last_updated_ms: i64`, `current_schema_id` (kept), `current_snapshot_id` (kept), `snapshot_count` (kept).
- **Schema:** keep `columns`; render each column's `type` as a string, including nested/complex types (struct/list/map) when iceberg returns them (use iceberg's type display or a small recursive formatter).
- **Partition spec** (the default/current spec): `partition_fields: [{ name, source_column, transform }]` — `source_column` resolved from the schema field id; `transform` as its string (e.g. `identity`, `day`, `bucket[16]`). Empty when unpartitioned.
- **Sort order** (the default/current order): `sort_fields: [{ column, transform, direction, null_order }]`. Empty when unsorted.
- **Snapshots (history):** `snapshots: [{ snapshot_id: i64, parent_snapshot_id: Option<i64>, timestamp_ms: i64, operation: String, summary: Map<String,String> }]`, sorted newest→oldest (the `operation`/`summary` come from the snapshot's `summary` map; `operation` is the `operation` key).
The handler stays warehouse-scoped + `catalog:read`-gated (unchanged auth). DTO field names use snake_case to match the existing ones.

## 2. Frontend (`api/catalog.ts` `TableDetail` + `CatalogBrowse.tsx` `TableDetailView`)
Extend `TableDetail` with the new fields + types (`PartitionField`, `SortField`, `Snapshot`). `TableDetailView` becomes a single scrollable panel with clear sections (NO nested tabs — the warehouse detail already owns the Browse/Access/Details tabs):
- **Header:** `{ns} / {table}` (kept back-link) + a meta line: `location` · `format v{format_version}` · `last updated {relative}`.
- **Schema:** keep the Column · Type · Nullable table (type now shows nested types).
- **Partition spec:** a small table (Field · Source column · Transform); render "Unpartitioned" when empty.
- **Sort order:** a small table (Column · Transform · Direction · Nulls); hidden when empty.
- **Snapshots:** a table (Snapshot id · Time · Operation · key summary fields like `added-records`/`added-data-files`/`total-records`); "No snapshots yet." when empty. (This is the lakekeeper "versioning" content.)
- **Properties:** keep the existing table.

## 3. Testing
- **Backend (DB + MinIO + lakekeeper):** create a table with a partition spec + at least one snapshot (via the lakekeeper table-create/commit API or a metadata fixture — the plan picks the mechanism that the test harness already supports; if creating a real snapshot is heavy, assert the partition spec + schema + format-version + an empty-snapshots case, and cover the snapshot-mapping with a unit test over a constructed `TableMetadata`). `table_detail` returns the partition fields, sort fields, snapshots, format_version, last_updated_ms; an unpartitioned table → empty partition/sort.
- **Frontend (Vitest, mock `@/api/catalog`):** `TableDetailView` renders the header meta, schema, partition table (and "Unpartitioned" when empty), sort table (hidden when empty), snapshots table (and "No snapshots yet." when empty), properties; a column with a nested type renders the formatted type string.
- **Manual + full local CI** before merge: live smoke — open a table (create one via SQL Lab or a token-driven write if needed to get a real partitioned/snapshotted table; otherwise verify the overview/schema/empty-states render against an existing table).

## 4. Risks
- **iceberg metadata shape** — the exact `TableMetadata`/`LoadTableResult` accessors (partition spec fields, snapshot summary, sort order) must be read carefully from the lakekeeper/iceberg crate; a wrong field map yields empty/garbage sections. The plan pins the accessors.
- **Nested type rendering** — iceberg nested types need a recursive string formatter; keep it simple (struct<...>, list<...>, map<...,...>).
- **Snapshot summary keys vary** by operation — show a known subset (added-records, added-data-files, total-records, total-data-files) plus the operation; don't assume all keys exist.
- **Getting a real snapshot in tests** is heavy (needs a write); fall back to a unit test over a constructed metadata if the integration harness can't cheaply create one.

## 5. Out of scope (later)
Data **preview** (sample rows — needs query execution via the engine); **files/manifests** view (manifest-list reading); **health** metrics; **view** detail (iceberg view metadata / SQL representations — a different load path); permissions/tasks tabs (sub-epics 4/5); editing table properties.
