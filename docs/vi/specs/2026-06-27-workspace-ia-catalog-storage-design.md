# Tái cấu trúc IA workspace — Catalog area + Storage sửa được (Phase A+B) — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-27-workspace-ia-catalog-storage-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-27 · **Branch:** `feat/workspace-admin-catalog` (BE token routes đã có sẵn) · **Parent:** ADR-19 + ADR-19a.

## Mục tiêu
Theo ADR-19a:
- **Sidebar chia vùng:** EXPLORE (SQL Lab, Datasets, Charts, Dashboards, Notebooks, Catalog) + MANAGE (Storage; sau: Members/Connections).
- **Catalog = AREA giàu** (feature trong EXPLORE) có tab **Browse** + **Access (tokens)** — tokens chuyển từ "Settings" (bỏ) vào đây. Mầm cho full lakekeeper-console (sau: table-detail, tasks, permissions).
- **Storage = page riêng** vùng MANAGE, **sửa được**, re-point warehouse của workspace.

**Thay thế** "Workspace Settings" vừa build (bỏ `WorkspaceSettings`/`settings/*` + mục "Settings"). **BE token routes giữ nguyên**, chỉ đổi chỗ FE.

## Quyết định
- Làm Phase A (restructure FE) + Phase B (Storage sửa được) đợt này. Full catalog parity = epic sau.
- Sửa storage re-point warehouse qua lakekeeper `update_storage_profile` + `update_storage_credential` (in-process `WarehouseService`, cùng seam `create_warehouse`). **Cảnh báo UI:** đổi bucket/endpoint của warehouse đã có data → data cũ bị orphan; egret không migrate.
- Mọi endpoint quản lý gate `workspace:manage_members`.

## Phase A (FE)
1. **Sidebar** (`Sidebar.tsx`): `NavGroup = {heading, items}`; EXPLORE_GROUPS = [Explore: SQL Lab…Catalog] + [Manage: Storage]; bỏ "Settings". Mỗi group có heading + items (collapsed: ẩn heading, giữ icon+tooltip). Admin nav giữ nguyên.
2. **Catalog area** (`App.tsx` + `routes/explore/catalog/`): route `catalog` lồng → `CatalogArea` (sub-nav Browse/Access + Outlet); index=`CatalogBrowse` (chuyển từ `explore/Catalog.tsx`), `access`=`CatalogAccess` (chuyển từ settings/). Xoá `settings/`.
3. **Storage page** (`routes/explore/StoragePage.tsx`): route `storage` (vùng Manage). Phase A = view read (từ StorageSettings cũ: provider/bucket/endpoint/region/key_prefix/access_key/has_secret; 404→default; 403→notice; không lộ secret).

## Phase B (Storage sửa được)
1. **BE** (`browse.rs`): `PUT /ws/{ws}/catalog/storage` (gate admin). Body `UpdateStorageRequest{provider, endpoint?, region, bucket, key_prefix?, access_key, secret_key?, path_style}` (secret_key optional = giữ secret cũ). Resolve warehouse_id (không có → 409); dựng StorageProfile+Credential (dùng `build_storage_profile/credential` của provision_ws, đặt `pub(crate)`); gọi `update_storage_profile` + `update_storage_credential` (lakekeeper **trước**, nó validate access); thành công → put secret mới (nếu có) + upsert workspace_storage; trả `WorkspaceStorageInfo`. Lakekeeper fail (creds sai) → 400, KHÔNG để egret row lệch lakekeeper.
2. **FE** (`StoragePage.tsx` + `useUpdateStorage`): form sửa (endpoint/region/bucket/key_prefix/access_key/secret_key; secret_key trống = giữ) prefilled từ `useWorkspaceStorage`; cảnh báo re-point; Save → PUT → invalidate; 404→form set mới; 403→notice; không prefill/render secret.

## Test
BE (DB+MinIO): PUT update row + re-point warehouse (đọc lại get_warehouse/storage GET khớp bucket mới); bỏ secret_key giữ secret cũ; creds sai → 400 + row không hỏng; non-admin → 403; ws không warehouse → 4xx; token-route tests cũ vẫn xanh. FE (mock): Sidebar 2 vùng (không có Settings; Storage ở Manage); CatalogArea Browse/Access + token CRUD; StoragePage read (không secret) + form submit useUpdateStorage + cảnh báo + 403.

## Ngoài phạm vi
Full catalog parity (table detail/tasks/permissions); Members/connections vùng Manage; GCS/Azure; migrate data khi re-point.
