# Secrets module — named secret per-workspace — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-28-secrets-module-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-28 · **Branch:** `feat/secrets-module` (off main). **Epic B** của chương trình "DB-per-workspace + Secrets"; phụ thuộc Epic A (DB đã per-workspace, PR #78).

## Mục tiêu
Biến secret thành entity **quản lý được, per-workspace, có tên, tái dùng**. Vault `secrets` (AES-256-GCM) giữ làm tầng crypto; thêm tầng **named** ở trên; connection (storage + DB) **tham chiếu** named secret thay vì seal inline.

## Hiện trạng
`secrets` = vault `(id,kind,nonce,ciphertext,key_id)`; `SecretStore` có `put/get/delete`. Storage (browse.rs) seal `secret_key` → `workspace_storages.secret_ref`. DB (repo.rs) seal → `databases.secret_id`. Cả hai tạo row vault ẩn danh inline (không tên, không tái dùng). `default_storage.secret_key` (config bootstrap) ngoài phạm vi.

## Quyết định
1. Bảng mới **`workspace_secrets`** `(id PK, workspace_id FK CASCADE, name, description NULL, secret_ref→secrets(id), created_at, UNIQUE(workspace_id,name))`. 1 named secret = 1 giá trị có tên, bọc 1 row vault.
2. **Repoint FK:** `workspace_storages.secret_ref` + `databases.secret_id` → trỏ `workspace_secrets(id)`. Resolve = conn → workspace_secrets.secret_ref → secrets → decrypt (2-hop). Cột secret nullable. `access_key` storage vẫn inline.
3. **Store**: mở rộng **egret-secrets** với `WorkspaceSecretStore` (cùng Db pool, dùng chung storage & DB): `create/list(used_by)/get_meta/update_value(rotation, thêm SecretStore::update tại chỗ)/rename/delete(chặn khi in-use)/resolve_plaintext`. Value write-only.
4. **Backfill (migration)**: mỗi secret inline cũ → tạo workspace_secrets (name `"<conn> secret"`, ws của conn, secret_ref = vault cũ) → repoint conn. Tái dùng vault row 1:1, non-destructive, ATOMIC trong transaction. Thêm FK sau backfill. Touch metastore lib. Trùng tên → suffix id fragment.
5. **Rewire BE**: storage browse.rs + egret-database nhận `secret_id` (named) hoặc tạo inline `{name,value}`; resolve 2-hop khi build S3Store / open_secret. Routes `/ws/{ws}/secrets` (list/create/update/delete) gate `workspace:manage_members`, IDOR-safe, delete in-use → 409.
6. **FE**: `SecretsPage` ở MANAGE (registry Name·Description·Used by·Created·Actions + Add/Edit value write-only + Delete chặn khi dùng); sidebar "Secrets"; `SecretPicker` (dropdown named + "＋ New secret" inline) thay field secret_key/password trong form Storage & DB.

## Test
BE (DB-gated): CRUD scope-ws (cross-ws 404, non-admin 403); rotation (đổi value → resolve ra giá trị mới); delete chặn khi in-use (409); backfill (conn cũ → named + repoint + resolve đúng plaintext); storage+DB kết nối qua named secret. FE (mock): SecretsPage CRUD + SecretPicker pick/create. Manual live smoke + full local CI trước merge.

## Rủi ro
2-hop resolution đúng ở cả storage & DB (sai = mất secret); backfill atomic; delete-protection (đếm ref cả 2 bảng); value write-only.

## Ngoài phạm vi
Ranger; chia sẻ secret cross-ws; rotation tự động; versioning/audit; GCS/Azure.
