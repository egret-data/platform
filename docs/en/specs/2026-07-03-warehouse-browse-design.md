# Warehouse Browse + grant picker — design (2026-07-03)

VI reference: `docs/vi/specs/2026-07-03-warehouse-browse-design.md`.

**Goal:** Let a platform admin browse the catalog structure of an embedded-catalog
warehouse — namespaces → tables → table schema (columns + types) — from the
control-plane Warehouse detail page, and pick a namespace/table from that browse
when adding a grant (instead of typing it).

This restores the browse + namespace/table grant picker that PR #90 shipped
(`egret-catalog-engine/src/browse.rs` + `CatalogAccess.tsx`) and EB-1 stripped,
re-done on the embedded-catalog + control-plane architecture (Architecture B).

**Non-goals:** per-client RBAC-filtered browse (admin sees everything in the
warehouse); write operations from the browse (view-only); workspace-side browse.

---

## Architecture

egret already embeds lakekeeper in-process. The browse calls lakekeeper's Iceberg
v1 services **directly on the `ApiContext`** with a system `RequestMetadata`
(admin sees all) — the same approach the old `browse.rs` used, and the same
in-process pattern as warehouse provisioning (`WarehouseProvisioner`). No HTTP
round-trip, no token, no client-scoped filtering.

Three layers, mirroring provisioning:

1. **`egret-catalog-engine::browse`** — thin functions over lakekeeper services.
2. **`egret-controlplane`** — a `WarehouseBrowser` trait (in `CpState`) + three
   platform-admin routes that resolve the egret warehouse → `lakekeeper_warehouse_id`
   → call the browser.
3. **`egret-bin`** — implements `WarehouseBrowser` over the lakekeeper `ApiContext`
   (like `LakekeeperProvisioner`).

Warehouse with no `lakekeeper_warehouse_id` (not provisioned) → the routes return
an empty list (namespaces/tables) rather than an error, so the UI shows an empty
browse gracefully.

---

## Backend

### `egret-catalog-engine::browse` (new module)

```rust
/// A namespace as its multipart identifier (Iceberg namespaces can be nested).
pub struct NamespaceEntry { pub parts: Vec<String> }

pub enum TabularKind { Table, View }
pub struct TabularEntry { pub name: String, pub kind: TabularKind }

pub struct ColumnInfo { pub name: String, pub r#type: String, pub required: bool }
pub struct TableSchema {
    pub columns: Vec<ColumnInfo>,
    pub partition_by: Vec<String>,   // partition-spec source column names
    pub sort_by: Vec<String>,        // sort-order column names
}

pub async fn list_namespaces(
    engine_state: ApiContext<EngineState>, system_user: &str, warehouse_id: &str,
) -> anyhow::Result<Vec<NamespaceEntry>>;

pub async fn list_tabulars(
    engine_state: ApiContext<EngineState>, system_user: &str, warehouse_id: &str, namespace: &[String],
) -> anyhow::Result<Vec<TabularEntry>>;   // tables + views

pub async fn load_table_schema(
    engine_state: ApiContext<EngineState>, system_user: &str, warehouse_id: &str, namespace: &[String], table: &str,
) -> anyhow::Result<TableSchema>;
```

- `list_namespaces` uses `NamespaceService::list_namespaces` (iceberg v1), recursing
  into child namespaces (as the old browse did) so nested namespaces appear.
- `list_tabulars` uses `TablesService::list_tables` + `ViewService::list_views`.
- `load_table_schema` uses `TablesService::load_table` and maps the current schema's
  fields → `ColumnInfo`, the default partition spec → `partition_by`, the default
  sort order → `sort_by` (only the fields the UI needs, not the whole `TableMetadata`).
- `warehouse_id` is the lakekeeper warehouse id (the prefix). A system
  `RequestMetadata` (reuse `bootstrap::system_request_metadata`) authorizes the calls.

### `egret-controlplane` — `WarehouseBrowser` trait + routes

```rust
pub struct BrowseNamespace { pub parts: Vec<String> }
pub struct BrowseTabular { pub name: String, pub kind: String }   // "table" | "view"
pub struct BrowseColumn { pub name: String, pub type_: String, pub required: bool }
pub struct BrowseSchema { pub columns: Vec<BrowseColumn>, pub partition_by: Vec<String>, pub sort_by: Vec<String> }

#[async_trait]
pub trait WarehouseBrowser: Send + Sync {
    async fn namespaces(&self, lakekeeper_warehouse_id: &str) -> Result<Vec<BrowseNamespace>, String>;
    async fn tabulars(&self, lakekeeper_warehouse_id: &str, namespace: &[String]) -> Result<Vec<BrowseTabular>, String>;
    async fn schema(&self, lakekeeper_warehouse_id: &str, namespace: &[String], table: &str) -> Result<BrowseSchema, String>;
}
```

`CpState` gains `browser: Option<Arc<dyn WarehouseBrowser>>` (like `provisioner`).

Routes (added to `warehouses::router()`, all `require_platform_admin`):

| Method + path | Returns |
|---|---|
| `GET /warehouses/{id}/namespaces` | `[{ parts: ["analytics"] }]` (camelCase DTO) |
| `GET /warehouses/{id}/namespaces/{ns}/tables` | `[{ name, kind }]` (tables + views) |
| `GET /warehouses/{id}/namespaces/{ns}/tables/{table}` | `{ columns:[{name,type,required}], partitionBy, sortBy }` |

- `{ns}` is the namespace parts joined by `.` (dot). Multipart namespaces whose
  parts contain a dot are a documented limitation (rare); the parts are split on `.`.
- Each handler: `require_platform_admin` → `get_warehouse(id)` → if
  `lakekeeper_warehouse_id` is `None`, return `[]` (namespaces/tables) or `404`
  (schema) → else call `self.browser` (skip / empty if `browser` is `None`, e.g. tests).

### `egret-bin`

`LakekeeperBrowser { engine_state, system_user }` implements `WarehouseBrowser` by
calling the `egret-catalog-engine::browse` functions. Constructed alongside
`LakekeeperProvisioner` (both hold `engine_state.clone()`), added to `CpState.browser`.

---

## Frontend

### API client (`controlPlane.ts`)

```ts
export interface Namespace { parts: string[] }
export interface Tabular { name: string; kind: "table" | "view" }
export interface Column { name: string; type: string; required: boolean }
export interface Schema { columns: Column[]; partitionBy: string[]; sortBy: string[] }

listNamespaces(warehouseId): Promise<Namespace[]>
listTabulars(warehouseId, namespace: string /* dot-joined */): Promise<Tabular[]>
loadSchema(warehouseId, namespace: string, table: string): Promise<Schema>
```

### Browse tab (`WarehouseDetail.tsx`)

A third tab **"Browse"** (after Roles & grants, Catalog Clients):
- Left: namespace list (`listNamespaces`) — click a namespace to select it.
- Middle: tables + views of the selected namespace (`listTabulars`), a `table`/`view`
  badge — click a table to select it.
- Right: schema panel of the selected table (`loadSchema`) — a table of
  Name / Type / Required, plus partition-by / sort-by chips if present.
- Empty states: "This warehouse has no namespaces yet." / "No tables in this namespace." /
  "Select a table to view its schema." A not-yet-provisioned warehouse shows the
  namespaces empty state.

### Grant picker (`AddGrantDialog` in `WarehouseDetail.tsx`)

Replace the two free-text `Namespace` / `Table` inputs with two browse-fed
`Select`s (mirroring the old `CatalogAccess.tsx`):
- **Namespace** `Select` from `listNamespaces` + an "All namespaces" sentinel.
- **Table** `Select` from `listTabulars` of the chosen namespace + an "All tables"
  sentinel; disabled until a concrete namespace is chosen.
- `securableType` is derived, not chosen: a concrete table → `table`; a concrete
  namespace with all-tables → `namespace`; all-namespaces → `catalog`. The
  `namespace`/`tableName` fields on the grant are set from the selections.
- `privilege` Select (read / write / manage) stays as-is.

The `Securable` select currently in `AddGrantDialog` is removed (securable is now
implied by how deep you pick).

---

## Data flow

Browse tab: UI → `GET /api/control-plane/warehouses/{id}/namespaces` →
controlplane route → `WarehouseBrowser` (egret-bin) → `egret-catalog-engine::browse`
→ lakekeeper `NamespaceService` (in-process, system metadata) → JSON back. Tables
and schema follow the same path.

Add grant: same browse endpoints feed the two Selects; on submit, the existing
`addGrant(roleId, {securableType, namespace, tableName, privilege})` is called with
values derived from the picks.

## Error handling

- Non-admin → `403` (existing `require_platform_admin`).
- Warehouse not provisioned (`lakekeeper_warehouse_id` null) → namespaces/tables
  `[]`; schema `404`.
- lakekeeper service error (e.g. table dropped mid-browse) → `500` with the error
  message; the UI shows an inline error and lets the user retry.
- `browser` is `None` (test/unconfigured) → routes behave as not-provisioned (`[]`).

## Testing

- **egret-controlplane** route tests: namespaces/tables/schema happy path against a
  fake `WarehouseBrowser`; `403` for non-admin; not-provisioned warehouse → `[]`.
- **Frontend** (Vitest): Browse tab renders namespaces → tables → schema from mocked
  API; grant picker renders namespace/table Selects and submits the derived
  `securableType`.
- **Manual**: browse `test-wh` (has namespace `az_eng` + a pyiceberg-created table)
  in the UI; add a table-level grant via the picker; confirm the grant row shows the
  picked namespace.table.

## Out of scope / future

- Per-client RBAC-filtered browse ("what can THIS client see").
- Browsing views' schema (list views, but schema panel is table-only for now).
- Search/filter within large namespace/table lists.
