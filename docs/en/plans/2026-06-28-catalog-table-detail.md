# Catalog — table detail (enrich) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich the catalog table-detail view with partition spec, sort order, snapshot history, format version, and last-updated — all from the iceberg metadata already loaded by `table_detail` (no query execution).

**Architecture:** Extend the `table_detail` handler in `browse.rs` to project more fields off the loaded `TableMetadata`, widen the `TableDetail` DTO, and expand the FE `TableDetailView` into a sectioned scrollable panel (Header · Schema · Partition · Sort · Snapshots · Properties).

**Tech Stack:** Rust (embedded lakekeeper + the iceberg-rust fork), egret-catalog-engine; React 18 + TanStack Query + shadcn; Vitest.

**Spec:** `docs/en/specs/2026-06-28-catalog-table-detail-design.md`. **Branch:** `feat/catalog-table-detail` (off main).

**Key facts (verified):**
- `crates/egret-catalog-engine/src/browse.rs` `table_detail` (lines ~980-1045): loads the table via `TablesService::load_table` → `result.metadata` (an iceberg `TableMetadata`, alias `md`). Currently builds `TableDetail { name, location, current_schema_id, columns, current_snapshot_id, snapshot_count, properties }`. `ColumnDto { name, type_: f.field_type.to_string() (already renders nested types), required }`. `md.current_schema()`, `.as_struct().fields()`, `md.properties()`, `md.location()`, `md.current_snapshot_id()`, `md.snapshots()` (iterator) are in use.
- iceberg `TableMetadata` accessors EXIST (verified in the fork's `spec/table_metadata.rs`): `format_version() -> FormatVersion`, `last_updated_ms() -> i64`, `default_partition_spec() -> &PartitionSpecRef`, `default_sort_order() -> &SortOrderRef`. The implementer must read the exact `PartitionSpec`/`SortOrder`/`Snapshot`/`Summary` field accessors in the same crate (`spec/partition.rs`, `spec/sort.rs`, `spec/snapshot.rs`) — expected: `PartitionSpec::fields() -> &[PartitionField]`, `PartitionField { source_id, field_id, name, transform }`; `SortOrder.fields: Vec<SortField>`, `SortField { source_id, transform, direction, null_order }`; `Snapshot::{ snapshot_id(), parent_snapshot_id(), timestamp_ms() (or .timestamp().timestamp_millis()), summary() -> &Summary { operation, additional_properties } }`. `FormatVersion` is an enum (`as u8`). Resolve a partition/sort `source_id` to a column name via the current schema (`schema.field_by_id(id)`).
- The `TableDetail`/`ColumnDto` Rust structs are defined near `table_detail` in `browse.rs` (find their `#[derive(Serialize)]`). FE `TableDetail`/`CatalogColumn` in `frontend/src/api/catalog.ts`; `TableDetailView` in `frontend/src/routes/explore/catalog/CatalogBrowse.tsx`.
- **Test pattern for creating a table:** `crates/egret-catalog-engine/tests/front_auth.rs` has `create_table_body(name)` (a JSON iceberg create-table request incl. schema) POSTed to the iceberg REST endpoint with a write-scoped catalog token; `gc_e2e.rs` creates a table that GETS A SNAPSHOT (data commit). Mirror these to create a table with a **partition spec** (add `"partition-spec"` to the create body) and (optionally) a snapshot. The browse `table_detail` is reachable at `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}` with a ws-admin/`catalog:read` session.
- Test DB `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO + lakekeeper; `--test-threads=1`.

---

## Task 1: Backend — enrich `table_detail` + DTO

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs` (`TableDetail` DTO + `table_detail` handler + new DTO structs)
- Test: `crates/egret-catalog-engine/tests/table_detail_api.rs` (new; DB+MinIO+lakekeeper-gated)

- [ ] **Step 1: Widen the DTO structs** (next to `TableDetail`/`ColumnDto` in `browse.rs`):
```rust
#[derive(serde::Serialize)]
struct PartitionFieldDto { name: String, source_column: String, transform: String }
#[derive(serde::Serialize)]
struct SortFieldDto { column: String, transform: String, direction: String, null_order: String }
#[derive(serde::Serialize)]
struct SnapshotDto {
    snapshot_id: i64,
    parent_snapshot_id: Option<i64>,
    timestamp_ms: i64,
    operation: String,
    summary: std::collections::BTreeMap<String, String>,
}
```
Add to `TableDetail`:
```rust
    format_version: u8,
    last_updated_ms: i64,
    partition_fields: Vec<PartitionFieldDto>,
    sort_fields: Vec<SortFieldDto>,
    snapshots: Vec<SnapshotDto>,
```
(keep the existing fields).

- [ ] **Step 2: Project the new fields in `table_detail`** (after `let md = &result.metadata;`). Read the exact iceberg accessors and build:
```rust
let schema = md.current_schema();
// helper to resolve a source field id → column name (fallback to the id as string)
let col_name = |id: i32| schema.field_by_id(id).map(|f| f.name.clone()).unwrap_or_else(|| id.to_string());

let partition_fields = md.default_partition_spec().fields().iter().map(|pf| PartitionFieldDto {
    name: pf.name.clone(),
    source_column: col_name(pf.source_id),
    transform: pf.transform.to_string(),
}).collect();

let sort_fields = md.default_sort_order().fields.iter().map(|sf| SortFieldDto {
    column: col_name(sf.source_id),
    transform: sf.transform.to_string(),
    direction: sf.direction.to_string(),
    null_order: sf.null_order.to_string(),
}).collect();

let mut snapshots: Vec<SnapshotDto> = md.snapshots().map(|s| SnapshotDto {
    snapshot_id: s.snapshot_id(),
    parent_snapshot_id: s.parent_snapshot_id(),
    timestamp_ms: s.timestamp_ms(),
    operation: s.summary().operation.to_string(),
    summary: s.summary().additional_properties.iter().map(|(k,v)| (k.clone(), v.clone())).collect(),
}).collect();
snapshots.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));   // newest first
```
> Confirm each accessor name/shape against the iceberg crate (`PartitionField.source_id`/`.name`/`.transform`; `SortField.source_id`/`.transform`/`.direction`/`.null_order`; `Snapshot::timestamp_ms` may be `.timestamp().timestamp_millis()`; `Summary.operation`/`.additional_properties`). `default_partition_spec()`/`default_sort_order()` return refs (deref as needed). `format_version()` → `as u8`; `last_updated_ms()` → i64.

- [ ] **Step 3: Return the widened `TableDetail`** including `format_version: md.format_version() as u8`, `last_updated_ms: md.last_updated_ms()`, `partition_fields`, `sort_fields`, `snapshots` (+ the existing fields).

- [ ] **Step 4: Test `crates/egret-catalog-engine/tests/table_detail_api.rs`** (mirror `front_auth.rs`/`gc_e2e.rs` fixture: a warehouse `{wid}`, a write-scoped catalog token to create a table via the iceberg REST endpoint, + a ws session for the browse GET). Create a table with a schema + a `"partition-spec"` (a single `identity`/`day` field on a column). Then `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}` and assert: `format_version` is 1 or 2; `partition_fields` has the one field with the right `source_column` + `transform`; `sort_fields` is `[]` (unless a sort order was set); `columns` match the schema; `snapshots` is `[]` for a freshly-created table with no commits (OR, if you reuse the gc_e2e data-commit path to get a snapshot, assert one snapshot with an `operation`). Also create an UNPARTITIONED table → `partition_fields == []`.

- [ ] **Step 5: Run + checks**
```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog-engine --test table_detail_api -- --test-threads=1
cargo build --workspace
cargo fmt --all -- --check
cargo clippy -p egret-catalog-engine --all-targets
```
(retry once on a MinIO flake; do NOT weaken). All green.

- [ ] **Step 6: Commit** `feat(catalog): enrich table_detail with partition spec, sort order, snapshots, format version`.

---

## Task 2: Frontend — sectioned `TableDetailView`

**Files:**
- Modify: `frontend/src/api/catalog.ts` (extend `TableDetail` + types)
- Modify: `frontend/src/routes/explore/catalog/CatalogBrowse.tsx` (`TableDetailView`)
- Test: `frontend/src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx` (extend)

- [ ] **Step 1: Extend `api/catalog.ts` types:**
```ts
export interface PartitionField { name: string; source_column: string; transform: string }
export interface SortField { column: string; transform: string; direction: string; null_order: string }
export interface Snapshot { snapshot_id: number; parent_snapshot_id: number | null; timestamp_ms: number; operation: string; summary: Record<string, string> }
```
Extend `TableDetail` with `format_version: number; last_updated_ms: number; partition_fields: PartitionField[]; sort_fields: SortField[]; snapshots: Snapshot[]` (keep the existing fields).

- [ ] **Step 2: `TableDetailView` sections** (in `CatalogBrowse.tsx` — keep the `ns / table` back-link header). Replace the current meta line with:
  - **Header meta:** `location` (mono) · `format v{format_version}` · `updated {new Date(last_updated_ms).toLocaleString()}` (+ keep `current_schema_id`/`current_snapshot_id`/snapshot count if desired).
  - **Schema:** keep the Column · Type · Nullable table.
  - **Partition spec:** an `<h2>Partition spec</h2>` + a table (Field · Source column · Transform) when `partition_fields.length`, else a muted "Unpartitioned".
  - **Sort order:** an `<h2>Sort order</h2>` + a table (Column · Transform · Direction · Nulls) ONLY when `sort_fields.length` (hide otherwise).
  - **Snapshots:** an `<h2>Snapshots</h2>` + a table (Snapshot id · Time `new Date(timestamp_ms).toLocaleString()` · Operation · a compact summary showing `added-records`/`added-data-files`/`total-records` when present) when `snapshots.length`, else a muted "No snapshots yet.".
  - **Properties:** keep the existing table.

- [ ] **Step 3: Tests** (extend `CatalogBrowse.test.tsx`, mock `@/api/catalog` `useTableDetail` to return the widened shape): the detail view renders the header meta (format version), the partition table (and "Unpartitioned" when `partition_fields: []`), the sort table only when present, the snapshots table (and "No snapshots yet." when empty), a nested type string in the schema, and the properties. Keep existing tests green.

- [ ] **Step 4: Gate** — `cd frontend && npm run lint && npm run typecheck && npm test && npm run build` — all green.

- [ ] **Step 5: Commit** `feat(web): sectioned table detail — partition, sort, snapshots, format version`.

---

## Self-review (controller, before final review)
- [ ] Backend projects format_version, last_updated_ms, partition fields (source column resolved), sort fields, snapshots (newest-first, operation + summary) off the loaded metadata; auth unchanged.
- [ ] FE renders all sections + the empty-states (Unpartitioned, no sort, no snapshots); nested types display.
- [ ] **Before merge:** full local CI (fmt/clippy/`cargo test -p egret-catalog-engine` + FE lint/typecheck/test/build) + live smoke (open a table — create a partitioned one via SQL Lab or the test path if the warehouse has none — and confirm the sections render). NOTE: 2 pre-existing `egret-engine` job tests fail on a fresh `egret_test` (unrelated; see the memory note).

## Out of scope (later)
Data preview (sample rows); files/manifests; health; view detail; permissions/tasks tabs; editing properties.
