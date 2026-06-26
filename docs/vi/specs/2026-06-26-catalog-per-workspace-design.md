# Spec A — Catalog theo workspace (+ chuẩn hóa metadata) — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-26-catalog-per-workspace-design.md`.
> File này là bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-26 · **Phạm vi:** sub-spec đầu của epic Notebook; ship độc lập.

## Bối cảnh
egret-catalog hiện là **một Iceberg REST catalog global**: namespaces/tables không scope theo workspace, `CatalogService` có một `warehouse` global, auth là một bearer token tĩnh, và `catalog_tables.metadata` là **một cục JSONB** (món nợ kỹ thuật). Spec A biến nó thành **per-workspace** và, nhân lúc rebuild schema, **trả luôn nợ chuẩn hóa JSONB** (mô phỏng `split_table_metadata` của lakekeeper). **Migration: drop & recreate** (egret pre-release, chưa có data thật).

## Quyết định kiến trúc
1. **Scope theo prefix (Iceberg REST chuẩn):** thêm `/v1/{prefix}/...`, `prefix` = slug workspace. `/v1/config?warehouse=<ws>` trả `overrides.prefix`. Daft/Trino tự dùng.
2. **Bảng `catalogs` tường minh** (1 workspace = 1 catalog, có `warehouse_location` + properties).
3. **Chuẩn hóa metadata kiểu lakekeeper:** bỏ blob; tách collection/quan hệ/con trỏ thành row, leaf vẫn jsonb. Load = ráp `TableMetadata` từ row qua `TableMetadataBuilder`. Nhắm **Iceberg V2**.
4. **AuthN/Z 2 đường, 1 bất biến** (*caller chỉ chạm prefix của workspace mình*): UI → session + `Authorizer.require_workspace(catalog:read|write)`; engine → token scoped theo workspace (`catalog_tokens` hashed), thay token tĩnh.
5. **Frontend** Catalog vào khu workspace/explore (không phải platform admin).

## Schema (migration mới; drop `catalog_*` + `workspace_catalog_grant`)
`catalogs` (workspace_id UNIQUE, name, warehouse_location, properties); `catalog_namespaces` thêm `catalog_id`, UNIQUE `(catalog_id, levels)`; `catalog_tables` (bỏ metadata jsonb, thêm con trỏ + counters) + các bảng con `catalog_table_{schemas,partition_specs,sort_orders,properties,snapshots,snapshot_log,metadata_log,refs,statistics,partition_statistics}`; tương tự cho **views** (`catalog_view_{schemas,versions,representations,version_log,properties}`); `catalog_tokens` (hashed, scoped theo workspace).
**Lưu ý đúng đắn:** `catalog_table_snapshots.schema_id` **nullable** (snapshot không schema); `table_uuid`/`view_uuid` UNIQUE.

## Service & wiring
`CatalogService` bỏ `warehouse` global → resolve theo request (extractor từ prefix). `CatalogStore` filter `catalog_id`. `load_table` ráp metadata bằng một query `LEFT JOIN`/`ARRAY_AGG` rồi `TableMetadataBuilder` (kiểu `load_tables` của lakekeeper). create/commit shred metadata vào row trong tx + CAS; collection con append-only theo id, con trỏ/refs/properties cập nhật, log append. `metadata.json` vẫn ghi ra object storage; DB là nguồn authoritative.

## Phân rã (mỗi cái một plan→build)
- **A-1** Data layer (migration + store chuẩn hóa table/view).
- **A-2** HTTP `{prefix}` routing + resolve catalog/warehouse theo request + provisioning hook.
- **A-3** Auth `catalog_tokens` trong require_bearer + RBAC `catalog:*` + internal UI API + frontend Catalog view.

## Ngoài phạm vi
File browser/storage, notebook/kernel, MinIO IAM per-user, multi-catalog/external catalog, refactor nav admin↔workspace rộng, Iceberg V3 (row lineage/encryption).
