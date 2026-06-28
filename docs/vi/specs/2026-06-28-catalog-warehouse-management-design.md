# Catalog — quản lý warehouse (per-workspace, N warehouse) — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-28-catalog-warehouse-management-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-28 · **Branch:** `feat/catalog-warehouses` (off main). **Sub-epic 1** của chương trình Catalog-console-parity (warehouse → namespace → table/view detail → tasks/stats → permissions). Tham chiếu: console lakekeeper (`data-portal/console`).

## Mục tiêu
Thay 1-warehouse-auto-provision/ws bằng **N warehouse/ws do user quản lý**, mỗi cái tạo trên một **storage connection đã đăng ký** (`workspace_storages`). Catalog area thành **list → detail** (mirror lakekeeper). Bỏ auto-provision + retire legacy single-storage. Token scope **per-warehouse**.

## Hiện trạng
`workspace_warehouse` (0025) map ws **1:1** → 1 lakekeeper warehouse, auto-provision (`ensure_warehouse` + lúc tạo workspace) từ legacy `workspace_storage`/`default_storage`. Browse routes trên 1 warehouse đó; tokens scope **per-workspace** + namespace_scope. lakekeeper mgmt API đã wire (`create/list/delete_warehouse`). Storage registry + named secret đã có.

## Quyết định
1. **`workspace_warehouses`** (1:N): `id PK, workspace_id FK, warehouse_id(lakekeeper), name, storage_conn_id→workspace_storages, created_at, UNIQUE(workspace_id,name)`. `catalog_token` thêm `warehouse_id` (token gắn 1 warehouse).
2. **`WarehouseStore`**: create(ws,name,storage_conn_id) → resolve conn (2-hop secret) → build StorageProfile → lakekeeper create_warehouse (validate S3) → lưu mapping; list/get/rename/delete (delete gọi lakekeeper delete_warehouse). Scope-ws, IDOR-safe, gate `workspace:manage_members`.
3. **Routes**: `/ws/{ws}/catalog/warehouses` (list/create) + `/{wid}` (get/rename/delete). Browse + tokens rescope dưới `/{wid}/namespaces[...]/tables[...]` và `/{wid}/tokens`. Resolve {wid}→lakekeeper-id verify ownership.
4. **Bỏ** auto-provision (`ensure_warehouse` + provisioner workspace-create) + routes/bảng legacy single-storage (`/catalog/storage`, `workspace_storage` singular).
5. **Migration** (`0034`): mỗi warehouse cũ → tạo registry connection từ legacy storage (name `catalog-default`, secret qua named-secret), insert managed row (name `default`, giữ nguyên warehouse_id), backfill `catalog_token.warehouse_id`, drop bảng cũ (`workspace_warehouse` + `workspace_storage`). Phần wrap secret phức tạp → có thể làm bằng one-shot Rust step lúc startup (idempotent) thay vì SQL thuần. Touch metastore lib.
6. **FE**: Catalog area = **Warehouses list** (Name·Storage·Created·Actions + Add warehouse dialog chọn storage conn) → **Warehouse detail** (route `/explore/:ws/catalog/:wid`, tabs Browse/Access/Details scope {wid}). `api/catalog.ts`: hooks warehouse CRUD + rescope browse/token theo {wid}.

## Test
BE (DB+MinIO): warehouse CRUD scope-ws (create → lakekeeper warehouse thật; cross-ws 404 IDOR; non-admin 403; delete gỡ lakekeeper; storage sai → 400); browse/tokens scope {wid}; migration backfill. FE (mock): list/create/rename/delete + detail tabs gọi hooks với {wid}. Manual Trino + full local CI trước merge.

## Rủi ro
create_warehouse gọi S3 thật (conn sai → 400); migration atomic (warehouse cũ không mất mapping); bỏ auto-provision = workspace mới chưa có warehouse (UI empty-state rõ; code giả định warehouse_for_workspace luôn Some phải sửa); {wid}→lakekeeper-id verify ownership (IDOR).

## Ngoài phạm vi (sub-epic sau)
namespace create/drop; table/view detail (schema/snapshots/properties); tasks + statistics; permissions; chia sẻ cross-workspace; Ranger.
