# Catalog Engine CE-4 — Authorizer lakekeeper tối thiểu + namespace scope — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-27-catalog-engine-CE4-authorizer-design.md`.
> Bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-27 · **Branch:** `feat/catalog-engine-ce4-authorizer` (off main; CE-1+CE-2+CE-3 đã merge).

## Mục tiêu
Thêm **`Authorizer` thật, tối thiểu** cho lakekeeper (thay `AllowAllAuthorizer` trơ): authorize **data action** (namespace/table/view) ngay BÊN TRONG lakekeeper — defense-in-depth sau `front_auth` — và thêm **scope namespace per-token** (`namespace_scope[]`): token bị giới hạn vào một/nhiều nhánh namespace (vd `marts.*`), ngoài đó bị chặn.

## Quyết định (chốt)
- **Posture:** `EgretAuthorizer` chỉ enforce `namespace`/`table`/`view` (read/write + `namespace_scope`). Control-plane (`server`/`project`/`warehouse`/`user`/`role`) mà flow bình thường chạm → **allow**. Hook post-action (`delete_*`, role assignment) → no-op `Ok`. (User chọn: "allow control-plane, enforce data actions").
- **System principal bypass:** principal hệ thống (bootstrap + browse in-process) nhận diện theo `UserId` → **allow tất cả**. KHÔNG dùng `LAKEKEEPER__INSTANCE_ADMINS` (instance-admin sẽ bypass Authorizer — ngược ý đồ với token principal).
- **Cầu scope:** `front_auth` thôi inject `new_lakekeeper_internal` (bypass authz). Thay bằng **token principal** `Actor::Principal(EGRET_TOKEN_PRINCIPAL)` (UserId **non-admin** ổn định, khác system principal) + `limes::Authentication` mang scope egret trong **claim tùy ý**: `claims["egret"] = {perms:[...], ns:[...]}`. Authorizer đọc qua `RequestMetadata::authentication()?.claims("egret")`. Dùng **claim**, KHÔNG dùng `roles()` (lakekeeper parse roles thành RoleIdent).
- **`front_auth` vẫn là tuyến đầu:** ownership (`{prefix}==warehouse`, 401/403) + read/write `required_permission` (CE-3) **giữ nguyên**. Authorizer là tuyến hai + là nơi chứa `namespace_scope` (front_auth khó enforce vì phải parse namespace từ mọi route).
- **Back-compat:** `namespace_scope` mặc định rỗng (NULL) = mọi namespace. Token cũ không ảnh hưởng.
- **Không thêm user lakekeeper:** request path không auto-tạo user; `create_table` không FK `created_by→users`. Token principal không cần tồn tại trong bảng users. `created_by` của write = id token principal (đủ dùng; audit per-token tinh hơn để sau).

## Vì sao cầu sạch
`RequestMetadata::authentication()` public (462); `limes::Authentication` có `claims: serde_json::Value` + accessor `claims(key)`, builder y hệt `bootstrap::system_request_metadata`. Method `are_allowed_{namespace,table,view}_actions_impl` nhận target namespace có cấu trúc + `&RequestMetadata` → có cả scope (claim) lẫn target để match, không parse path.

## Thành phần
1. **Schema** `0028_catalog_token_namespace_scope.sql`: `ALTER TABLE catalog_tokens ADD COLUMN namespace_scope text[] NOT NULL DEFAULT '{}'`. Touch `egret-metastore/src/lib.rs` (re-embed migrator — [[egret-sqlx-migration-staleness]]).
2. **tokens.rs:** `create_token(ws, label, permissions, namespace_scope)` thêm tham số (validate identifier hợp lệ, sai → `BadRequest`, không check tồn tại). `TokenAuth` + `CatalogToken` thêm `namespace_scope: Vec<String>`; `authenticate_token` select cùng lookup.
3. **CLI:** `mint-token --namespaces marts,staging` (mặc định rỗng = all); `list-tokens` hiện cột `namespaces` (`*` khi rỗng).
4. **Cầu (front_auth):** `EGRET_TOKEN_PRINCIPAL` (UserId cố định, subject khác system). Thay chỗ inject `new_lakekeeper_internal` (front_auth.rs:172) bằng `token_request_metadata(auth)` dựng `Authentication` với `claims["egret"]={perms,ns}` + `set_authentication(Actor::Principal(...), auth)`. Ownership + `required_permission` phía trên giữ nguyên.
5. **`EgretAuthorizer`** (`authorizer.rs` mới): impl trait `Authorizer`; giữ `server_id` + system principal `UserId`. `decide(metadata, target_ns_levels, is_write)`: system principal → allow; đọc claim `egret` (không có → deny); thiếu perm read/write → deny; `ns` không rỗng mà không entry nào là prefix của target → deny (subtree match); còn lại allow. Ba method data (`namespace/table/view`) dùng `decide`. **Mọi method khác → allow** (copy skeleton từ `AllowAllAuthorizer`, chỉ override 3 method data). Hook post-action → no-op. `new_router` → Router rỗng; `api_doc` tối thiểu; `implementation_name`="egret". Associated `*Action` = enum `Catalog*Action`.
6. **Wiring:** `EngineState = State<EgretAuthorizer, PostgresBackend, SecretsState>`; lan type vào `bootstrap.rs`/`browse.rs`/`provisioner.rs`. `build_state` dựng `EgretAuthorizer{server_id, system_principal}`. Bootstrap+browse vẫn dùng `system_request_metadata` → đi nhánh bypass.

## Test
- Unit authorizer: phân loại read/write từng `Catalog*Action`; `decide` (system allow; read-token write deny; `ns=[marts]` → `marts.x` allow, `marketing` deny; ns rỗng → allow; thiếu claim → deny); subtree match.
- Unit tokens/front_auth: `create_token` từ chối namespace sai (`BadRequest`); `required_permission` regression.
- Integration (DB+MinIO): token scope `marts` rw → tạo/commit trong `marts.*` 200, dưới `marketing` → **denied**; token read-only full → GET 200/write 403; regression token full-scope rw chạy mọi namespace; A→B 403; token sai 401; browse+bootstrap (system) vẫn chạy.
- Defense-in-depth: gọi Service in-process với token principal thiếu scope → bị Authorizer chặn dù không qua front_auth.

## Rủi ro
Authz chạy thật → phải allow mọi action flow bình thường cần (control-plane=allow, chỉ 3 method data deny được); test create/list/load/commit/drop full-scope trước khi test deny. System principal phải bypass (bootstrap/browse). Claim shape phải khớp giữa front_auth (ghi) và Authorizer (đọc) — lệch serde → fail-closed (deny), test full-scope bắt ngay. Giữ 1 `iceberg`.

## Ngoài phạm vi
S3-verb scoping tinh trên `/sign` (vẫn read như CE-3); audit identity per-token; authz parity đầy đủ (roles/projects/server/OpenFGA + authz-management API); CE-5 (storage/secrets), CE-6 (GC workers).
