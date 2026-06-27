# Catalog Engine CE-3 — Phạm vi quyền per-token — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-27-catalog-engine-CE3-token-scope-design.md`.
> Bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-27 · **Branch:** `feat/catalog-engine-ce3-token-scope` (off main; CE-1+CE-2 đã merge).

## Mục tiêu
Mỗi token `catalog_tokens` có **scope quyền riêng** (`catalog:read`/`catalog:write`), enforce trong `front_auth` của egret TRƯỚC lakekeeper. Token read-only đọc/browse được nhưng không tạo/commit/drop. Nhiều token/workspace, scope khác nhau. (`namespace_scope[]` để spec sau.) Authorizer thật của lakekeeper = CE-4.

## Quyết định (chốt)
- CE-3 chỉ làm `permissions text[]` (read/write); bỏ `namespace_scope[]`.
- Enforce trong `front_auth` egret (không phải Authorizer lakekeeper — đó là CE-4). lakekeeper vẫn AllowAll; egret là biên giới sống.
- Back-compat: token cũ mặc định full `{catalog:read,catalog:write}`.

## Thành phần
1. **Schema:** `ALTER TABLE catalog_tokens ADD COLUMN permissions text[] NOT NULL DEFAULT '{catalog:read,catalog:write}'`.
2. **tokens.rs:** `create_token(ws, label, permissions)` thêm tham số (validate chỉ `catalog:read`/`catalog:write`, sai → BadRequest). Thay `workspace_for_token` bằng `authenticate_token(raw) -> TokenAuth { workspace_id, permissions }` (một lookup trả cả hai); giữ filter `revoked_at IS NULL` + hash. `CatalogToken` thêm field `permissions` (cho `list-tokens`; không lộ hash).
3. **CLI:** `egret catalog mint-token --perms read|write|read,write` (mặc định `read,write`).
4. **front_auth enforce** (sau ownership check, dùng chung kết quả `authenticate_token` đã có perms):
   - Phân loại `(method, path)`: **read** (cần `catalog:read`) = `GET`/`HEAD` + `POST /config`, `.../metrics`, `.../aws/s3/sign`. **write** (cần `catalog:write`) = mọi `DELETE`; `POST` lên `namespaces`(create), `.../properties`, `.../tables`(create), `.../tables/{t}`(commit), `.../register`, `tables/rename`, `.../views`(create).
   - Token thiếu perm → **403** (fail-closed). Đủ → inject `RequestMetadata` (như cũ, internal actor) đi tiếp.
   - Helper thuần `required_permission(method, path) -> &str` (unit-test theo bảng route); mặc định-deny: method/path lạ mà mutating → đòi `catalog:write`.

## Test
Unit `required_permission` từng route. Integration (DB+MinIO): mint token read-only + read-write; read-only GET → 200, POST create-ns/table + DELETE → **403**; read-write → 200; regression ownership A→B 403, token sai 401. tokens.rs: create_token perms → authenticate_token trả đúng; perm lạ → BadRequest.

## Ngoài phạm vi
`namespace_scope[]` (spec sau). Authorizer thật lakekeeper + scope theo S3-verb trên `sign` = **CE-4**. CE-5 (storage/secrets), CE-6 (GC).
