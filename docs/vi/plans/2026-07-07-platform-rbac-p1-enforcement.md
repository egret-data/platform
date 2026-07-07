# Kế hoạch — Platform RBAC P1 (Enforcement Core)

> Bản chuẩn (code + bước đầy đủ): `docs/en/plans/2026-07-07-platform-rbac-p1-enforcement.md`. File này là tổng quan.

**Mục tiêu:** Biến schema RBAC sẵn có thành enforce chi tiết: catalog permission code-defined, `Principal.permissions` resolve khi tạo session, guard `require_permission` (bypass `platform_admin`), và chuyển mọi route control-plane từ `require_platform_admin` sang permission cụ thể.

**Spec:** `docs/en/specs/2026-07-07-platform-rbac-admin-design.md`. **Ràng buộc:** DB test chạy trên `egret_test` (không đụng demo :5432); touch `egret-metastore/src/lib.rs` sau khi thêm migration; `cargo fmt --all` trước commit; không `cargo build -p egret-controlplane` đơn lẻ.

## Danh sách task (TDD, commit từng task)
1. **Catalog** `egret-core::rbac` — `PERMISSIONS` (10 nhóm, ~44 key) + `is_known()` + `groups()`. Test: không trùng key.
2. **`Principal.permissions`** — thêm field `permissions: Vec<String>` vào `egret-core/context.rs`; sửa mọi constructor compile (`vec![]`).
3. **Resolve session** (`egret-auth/session.rs`) — query `permissions(user_id)` = union `role_permissions` qua role gán trực tiếp cho user + qua group; gắn vào `Principal`. DB-gated test (user trực tiếp + qua group).
4. **Guard** (`egret-controlplane/auth.rs`) — `require_permission(&Principal, "engine:create")`; `platform_admin` bypass. Unit test allow/deny/bypass.
5. **Migrate route** (6 module: engines/warehouses/storage/secrets/policies/monitor) — đổi `require_platform_admin(&p)?` → `require_permission(&p, "area:action")?` theo quy tắc: `list_*/get_*→read`, `create_*→create`, `update_*/set_*/*_enabled→update`, `delete_*→delete`, engine `provision/restart→provision`, policy `seed_defaults→seed`. Mỗi module 1 commit + route test 403-không-quyền / 200-có-quyền.
6. **Seed + validation** — migration `0059_rbac_permission_seed.sql` (data_steward nhận bộ read + policy:*; platform_admin không cần vì bypass); startup `warn!` chuỗi permission lạ. Touch metastore lib.rs.
7. **Phase gate** — fmt/clippy/test --workspace trên egret_test.

## Ghi chú
- Test-helper của `Auth` phải set `Principal.permissions` để test enforce được (thêm biến thể principal scoped, không phải platform_admin).
- Giữ `require_platform_admin` cho `role:*`/`user:assign_role` (P2). P1 không đụng frontend.
