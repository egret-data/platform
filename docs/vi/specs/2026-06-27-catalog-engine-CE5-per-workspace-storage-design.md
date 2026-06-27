# Catalog Engine CE-5 — Cấu hình storage per-workspace — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-27-catalog-engine-CE5-per-workspace-storage-design.md`.
> Bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-27 · **Branch:** `feat/catalog-engine-ce5-workspace-storage` (off main; CE-1..CE-4 đã merge).

## Mục tiêu
Mỗi workspace cấu hình **storage S3 riêng** (bucket/endpoint/region/creds) cho warehouse của nó, thay vì dùng chung 1 `StorageConfig` global. Secret key lưu **qua egret-secrets** (envelope-encrypt; secrets bridge mức tối thiểu), không plaintext trong bảng. Workspace không khai storage → fallback global default (back-compat CE-2).

CE-5 **re-scoped**: chỉ per-workspace storage config. GCS/Azure + thay hẳn SecretStore của lakekeeper = để sau (cột `provider` chừa cho GCS/Azure).

## Quyết định (chốt)
- **Chỉ S3** (gồm MinIO). Cột `provider` default `'s3'`, chừa cho GCS/Azure.
- **Set storage tại lúc tạo workspace.** Request create mang storage spec optional; warehouse provisioned theo đó. **Đổi storage của warehouse đã provisioned = ngoài phạm vi** (cần migrate data).
- **Secret qua egret-secrets.** `put` secret key vào egret-secrets, bảng `workspace_storage` chỉ lưu `secret_ref` (uuid). Provision thì `get` lại để dựng credential. lakekeeper vẫn giữ bản copy operational của credential (chưa thay SecretStore của nó).
- **egret-catalog-engine thêm dep `egret-secrets` trực tiếp** (foundation crate), `SecretStore` dựng ở egret-bin (sqlx 0.8) truyền vào provisioner — như pattern `CatalogStore`, không kẹt biên 0.8/0.9.
- `provision_workspace_warehouse` giữ nguyên chữ ký (vẫn nhận `StorageConfig`); resolve per-workspace ở tầng trên (`EngineWarehouseProvisioner::provision`).

## Thành phần
1. **Migration 0029** `workspace_storage(workspace_id PK→workspaces ON DELETE CASCADE, provider default 's3', endpoint, region NOT NULL, bucket NOT NULL, key_prefix, access_key NOT NULL, secret_ref uuid, path_style bool default true, created_at)`. Additive. Touch `egret-metastore/src/lib.rs` (re-embed).
2. **Store (egret-catalog, sqlx 0.8)** file mới `service/storage.rs`: `WorkspaceStorageRow` (KHÔNG mang secret value, có `secret_ref`); `upsert_workspace_storage` (ON CONFLICT DO UPDATE); `workspace_storage_for(ws) -> Option<Row>`.
3. **`StorageSpec`** (struct thuần, egret-workspace/provision.rs — không dep secrets/catalog): provider/endpoint/region/bucket/key_prefix/access_key/secret_key/path_style. Trait `WarehouseProvisioner::provision(ws, slug, storage: Option<StorageSpec>)` thêm tham số.
4. **`EngineWarehouseProvisioner`** thêm `secret_store: egret_secrets::SecretStore`. `provision`:
   - storage=Some → `secret_ref = secret_store.put("workspace_s3_secret", secret_key)` + `upsert_workspace_storage(row+secret_ref)`.
   - resolve: `workspace_storage_for(ws)` → có → `secret = secret_store.get(secret_ref)` → dựng `StorageConfig`; không → global default.
   - gọi `provision_workspace_warehouse(effective_cfg, …)`.
5. **API create** (egret-workspace/routes.rs): `CreateWorkspace` thêm `storage: Option<StorageSpecDto>` (secret_key write-only, không trả về). `create_ws` map DTO→StorageSpec truyền vào provision. Thêm `GET /workspaces/{slug}/storage` trả config (ẩn secret, chỉ `has_secret`/secret_ref), 404 nếu dùng global default.
6. **Wiring egret-bin:** dựng `egret_secrets::SecretStore` (đã có sẵn ~dòng 386) và truyền vào `EngineWarehouseProvisioner::new`.

## Test
- Unit resolve: ws có row → StorageConfig từ row + secret lấy qua egret-secrets; ws không row → global default; secret không đọc thẳng từ row (chỉ qua secret_ref).
- Unit store (sqlx 0.8): upsert rồi `workspace_storage_for` round-trip đủ field + secret_ref; upsert 2 lần update; row không có secret value.
- Integration (DB+MinIO): tạo workspace storage custom (cùng MinIO, bucket/key riêng) → warehouse provisioned OK + có row + secret nằm trong egret-secrets (cột là uuid secret_ref, không plaintext); tạo workspace không storage → global default (regression CE-2), không có row; `GET .../storage` trả config ẩn secret cho ws custom, 404 cho ws default.

## Rủi ro
S3 round-trip thật lúc create → storage spec sai thì tạo workspace fail rõ ràng (chấp nhận). Secret khi chưa cấu hình crypto → egret-secrets xử theo default sẵn có (khuyến nghị cấu hình master key). Đổi arity trait → mọi impl/call-site `WarehouseProvisioner` phải sửa (compile bắt). Giữ 1 iceberg.

## Ngoài phạm vi
GCS/Azure (chừa cột provider); đổi storage của warehouse đã provisioned; full secrets bridge thay SecretStore lakekeeper; CE-6 (GC workers).
