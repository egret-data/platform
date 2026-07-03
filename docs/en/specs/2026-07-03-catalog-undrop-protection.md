# Catalog Undrop + Protection + View detail — design (2026-07-03)

VI reference: `docs/vi/specs/2026-07-03-catalog-undrop-protection.md`.

**Goal:** Surface three lakekeeper management capabilities the embedded catalog
already supports but egret never exposed:

1. **Undrop / recovery** — dropped tables/views are soft-deleted (the GC queues
   purge them after a TTL); let a platform admin **list soft-deleted tabulars and
   recover them** before purge.
2. **Protection** — set/clear a **protected** flag on a table, view, or namespace
   so it can't be dropped by accident.
3. **View detail** — the Browse tree lists views but selecting one errors
   (`load_table` is table-only); load the **view schema** instead.

**Non-goals:** protection for generic (non-Iceberg) tables; delete-profile TTL
config; task-queue UI; pagination of the deleted list beyond the first page (log
if truncated).

## Architecture (mirrors the existing browse layers)

lakekeeper's **management API** methods, called in-process on `EngineApiServer`
(`ApiServer<PostgresBackend, AllowAllAuthorizer, SecretsState>`) with a system
`RequestMetadata` — the same pattern as browse/provisioning:

- `warehouse::Service::{list_soft_deleted_tabulars, undrop_tabulars}`
- `table::TableManagementService::set_table_protection`
- `namespace::NamespaceManagementService::set_namespace_protection`
- `view::ViewManagementService::set_view_protection`
- iceberg `views::ViewService::load_view` for the view schema

Management calls key by **lakekeeper UUIDs**. The browse iceberg lists already
accept `return_uuids` + `return_protection_status`; flip both so each
namespace/tabular carries its `id` (Uuid) + `protected` — no extra round-trips.

## Backend

### `egret-catalog-engine`
- `browse.rs`: `NamespaceEntry`/`TabularEntry` gain `id: Option<Uuid>` +
  `protected: bool` (read from `namespace_uuids`/`table_uuids`/`protection_status`,
  index-aligned). New `load_view_detail` (columns via `ViewService::load_view`).
- `recovery.rs` (new): `list_deleted_tabulars(engine, sys, wh_lk_id) -> Vec<DeletedTabular>`
  (`{id, name, namespace, kind, deleted_at, expiration}`); `undrop_tabular(…, kind, id)`;
  `set_protection(…, ProtectTarget{Table|View|Namespace}, id, protected) -> bool`.

### `egret-controlplane` (`warehouses.rs`, platform-admin gated)
- Browse DTOs: add `id` + `protected` to namespace/tabular; a view-detail DTO.
- Routes:
  - `GET  /warehouses/{id}/deleted-tabulars`
  - `POST /warehouses/{id}/undrop`  body `{kind, id}`
  - `PUT  /warehouses/{id}/protection`  body `{target, id, protected}`
  - `GET  /warehouses/{id}/namespaces/{ns}/views/{view}` → view schema
- `WarehouseBrowser` trait gains `deleted_tabulars`, `undrop`, `set_protection`,
  `view_detail`. Unprovisioned warehouse → `[]` / `404`.

### `egret-bin`
`LakekeeperBrowser` implements the new methods over the engine fns.

## Frontend (`WarehouseDetail.tsx`, Browse tab)
- api client: `Namespace`/`Tabular` gain `id?`/`protected`; `DeletedTabular`,
  `listDeletedTabulars`, `undropTabular`, `setProtection`, `loadViewDetail`.
- **Protection**: a shield toggle on each namespace node and in the table/view
  detail header; filled shield = protected. Toggling calls `setProtection`.
- **View detail**: when a view is selected the detail pane loads the view schema.
- **Recycle bin**: a "Deleted tables" button in the Browse header opens a dialog
  listing soft-deleted tabulars (name, namespace, kind, deleted/expiry) with an
  **Undrop** action per row.

## Testing
- Rust unit test for the protect-target → trait dispatch is not meaningful (thin
  passthroughs); rely on the existing route tests (extend the fake browser) +
  live verification against MinIO: drop a seeded table → it appears in the deleted
  list → undrop restores it → browse shows it again; protect a table → drop is
  rejected; select a view → schema renders.
- Frontend: extend `WarehouseDetail.test.tsx` fake API for the new calls.

## Out of scope / future
Namespace/generic-table recovery nuances, delete-profile TTL config UI, task
visibility, protection audit log.
