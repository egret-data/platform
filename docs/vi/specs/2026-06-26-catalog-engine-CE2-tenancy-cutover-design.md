# Catalog Engine CE-2 — Tenancy + provisioning + cutover — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-26-catalog-engine-CE2-tenancy-cutover-design.md`.
> File này là bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-26 · **Branch:** `feat/catalog-engine-lakekeeper` (nối tiếp spike CE-1).

## Mục tiêu
Biến spike CE-1 thành catalog lakekeeper per-workspace **chạy thật**: dọn shortcut, provision warehouse per-workspace, **mount router lakekeeper vào egret-server thay router builtin A-1/A-2**, và rewire browse API/UI đọc dữ liệu lakekeeper. Giữ bảo mật token per-workspace (A-3) ở tầng middleware egret; per-token scope thật để CE-3/CE-4.

## Quyết định (đã chốt)
- **Deps:** git-pin lakekeeper rev upstream (thay path-dep sibling).
- **Mapping tenancy:** bảng mới **`workspace_warehouse`** (không dùng bảng `catalogs` của A-1 — CE-8 sẽ bỏ).
- **Cutover + xoá A-1/A-2 thừa:** thay router `iceberg_router` builtin **VÀ xoá luôn** schema metadata catalog + store/handlers/`resolve`/`iceberg_router` của egret (gộp CE-8 lên, không để dormant). **Giữ** lớp identity A-3: `catalog_tokens` (`tokens.rs` — engine auth CE-3), RBAC `catalog:*`, `require_bearer`, browse API (rewire), Catalog UI. Xoá **sau khi** lakekeeper serve + browse rewired (trong CE-2) → lúc xoá chắc chắn không còn ai dùng.
- **UI:** CE-2 **kéo phần read của CE-7 lên** — browse API đọc lakekeeper để UI không vỡ.
- 1 default project; 1 warehouse/workspace; `{prefix}` = **UUID warehouse**.

## Thành phần
1. **Deps + dọn spike:** git-pin (verify vẫn 1 iceberg); xử lý **SQLX_OFFLINE** (vendor `.sqlx` của lakekeeper vào egret — đường dẫn `~/.cargo/git` không ổn định); `test-utils` → dev-deps, bỏ `test_user`; giữ rustflags `tokio_unstable`/`tracing_unstable`.
2. **Bootstrap admin thật:** lúc khởi động, migrate (schema `lakekeeper`) + `bootstrap` default project bằng **principal có UserId thật** (từ system/superadmin egret qua config `catalog.system_user`), không phải `test_user`. Idempotent.
3. **Provisioning + `workspace_warehouse`:** migration `workspace_warehouse(workspace_id PK→workspaces, warehouse_id uuid, created_at)`. Tạo workspace → `create_warehouse` (S3 profile từ storage config, prefix `workspaces/<slug>/warehouse`) → lưu warehouse_id. **Idempotent tra mapping trước** (không đoán theo lỗi create). Rewire hook A-2.
4. **Mount + cutover:** nest `new_v1_full_router` dưới `/catalog/v1`; **bỏ mount `iceberg_router` builtin**. Trước router: `require_bearer` egret validate token→workspace + bắt `{prefix}(=warehouse UUID) == warehouse của workspace` (giữ bất biến A-3, 403/401), rồi `from_fn` chèn `RequestMetadata`. Bên trong lakekeeper: AllowAll (scope thật = CE-4). Không cần cờ `catalog.engine` (sau khi xoá builtin, lakekeeper là engine duy nhất).
5. **Browse API rewire (read CE-7):** `/api/ws/{ws}/catalog/...` đọc lakekeeper thay `catalog_*` (resolve warehouse_id rồi đọc qua read API lakekeeper hoặc query schema `lakekeeper`); vẫn gate `require_workspace` + scope workspace. UI giữ nguyên.
6. **Xoá A-1/A-2 thừa (CE-8 gộp vào, làm CUỐI CÙNG trong CE-2):** migration `DROP TABLE` các bảng metadata catalog (`catalogs`, `catalog_namespaces`, `catalog_tables`+con, `catalog_views`+con) — **KHÔNG** drop `catalog_tokens` / `role_permissions`. Xoá code: `table_repo.rs`/`view_repo.rs`/`catalogs.rs`, các method table/view/ns trong `store.rs`, `api/*`, `resolve.rs`, `iceberg_router`+`CatalogState`. **Giữ** `tokens.rs`, `require_bearer`, `browse.rs` (đã rewire). Build/clippy phải xanh.

## Test
Create workspace → có row `workspace_warehouse` + warehouse resolvable (idempotent). Round-trip per-workspace qua router: token A trên prefix A → 200; token A trên prefix B → **403** (middleware egret); token sai → 401. Browse API list từ lakekeeper; non-member/cross-workspace → 403. Cutover: bỏ `iceberg_router` builtin; suite xanh; clippy/fmt sạch; vẫn 1 iceberg.

## Rủi ro
Bootstrap principal phải ổn định qua restart (đừng tạo user mới mỗi boot). Ưu tiên read API typed của lakekeeper hơn query thô schema (schema đổi theo rev). SQLX_OFFLINE với git dep phải có lời giải reproducible (vendor cache). Đảm bảo không còn ai ghi/đọc `catalog_*` sau cutover trừ browse API (đã rewire).

## Ngoài phạm vi
CE-3 (token→principal + per-token scope), CE-4 (`Authorizer` thật = RBAC+scope, chặn trước sign), CE-5 (storage/secrets, GCS/Azure), CE-6 (GC workers). **CE-8 đã gộp vào spec này (mục 6)** — không còn CE-8 riêng; catalog cũ bị xoá ngay tại đây, không để dormant.
