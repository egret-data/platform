# Thiết kế — Platform RBAC & Access-Control Admin

> Bản chuẩn (đầy đủ, có bảng permission + API + code): `docs/en/specs/2026-07-07-platform-rbac-admin-design.md`. File này là tổng quan tiếng Việt.

**Trạng thái:** Thiết kế (đã duyệt để sang plan) · **Ngày:** 2026-07-07 · **Nhánh:** `feat/platform-rbac-admin`

## Mục tiêu
Biến schema RBAC đã có sẵn nhưng **chưa enforce** của egret thành phân quyền chi tiết thực sự, kèm **console quản trị Access Control**. Hiện control-plane chỉ có 1 cổng `require_platform_admin` và menu **Users** là placeholder. Đợt này giao:
1. **Permission catalog code-defined** (`resource:action`) do backend enforce.
2. **Enforce chi tiết**: mỗi mutation control-plane check 1 permission cụ thể thay vì cờ `platform_admin` thô.
3. **Console admin**: quản lý Users, Groups, Roles, Permissions.

Phạm vi **platform**. Quản lý thành viên theo workspace = follow-up.

## Tái dùng schema (không dựng lại)
`0002_identity.sql` + `0003_workspace_rbac.sql` đã có: `users`, `groups`/`user_groups` (OIDC sync + local), `roles` (scope platform/workspace, seeded platform_admin/data_steward/ws_*), `role_permissions` (chuỗi `resource:action`), `platform_role_assignments` (gán role cho user/group), `workspace_members`. `Principal` đã có `platform_roles`; đợt này **thêm** `permissions` đã resolve + CRUD/enforce quanh các bảng trên.

## Quyết định đã chốt
- **Permission code-defined** — backend sở hữu catalog + enforce; admin chỉ gán vào role, không tự đẻ permission.
- **Enforce chi tiết ngay** — thay `require_platform_admin` bằng `require_permission(p, "…")` từng route.
- **Group local + OIDC** — admin tạo/sửa group local + thành viên; group OIDC read-only nhưng gán role được.
- **Scope platform**; workspace-member để sau.

## Permission catalog (tóm tắt — chi tiết ở bản en)
Nguồn sự thật: module mới `egret-core::rbac`. Nhóm quyền: `engine:*`, `warehouse:*`, `storage:*`, `secret:*`, `database:*`, `policy:*` (read/create/update/delete/…), `workspace:*`, `user:*` (…/assign_role), `group:*` (…/manage_members), `role:*` (…/assign_permission). Startup cảnh báo chuỗi permission lạ trong DB. `GET /control-plane/permissions` trả catalog + role nào có quyền gì.

## Enforce
`session.rs` resolve `permissions: Set<String>` = union role_permissions của mọi role gán trực tiếp cho user **hoặc** qua group của user → gắn vào `Principal`. `platform_admin` = **superuser** (bypass trong `require_permission`, không cần migration khi catalog lớn thêm). Guard: `require_permission(&Principal, "engine:create")` — mọi route control-plane đổi từ `require_platform_admin`. Bảng route→permission nằm trong plan.

## API mới (module `access` trong egret-controlplane)
CRUD + assignment cho **Users** (list/tạo local/sửa/disable/gán role), **Groups** (list/tạo local/sửa/xóa/thành viên/gán role — OIDC read-only), **Roles** (mount `/access/roles` tránh đụng `/policies/roles` của Ranger; tạo/sửa/xóa custom, set permissions), **Permissions** (`GET /permissions` catalog). Thêm `RoleRepo` trong egret-auth; tái dùng `UserRepo`/`GroupRepo`.

## Console (frontend)
Nhóm sidebar **Access control** (collapsible, thay placeholder Users) theo convention control-plane (ConfigSection, COLUMNS table, ConfirmDeleteDialog, token shadcn): **Users**, **Groups**, **Roles**, **Permissions** (trang cuối read-only tham chiếu).

## An toàn (chống lock-out)
Bypass `platform_admin`; **chặn xóa platform_admin cuối cùng** (409); admin bootstrap giữ platform_admin; role seeded không xóa được, name/scope bất biến.

## Chia phase
- **P1** — Enforcement core: catalog `egret-core::rbac` + startup validation; resolve `Principal.permissions`; `require_permission` + bypass; migrate mọi route sang permission cụ thể; migration seed role→permission mặc định. Test backend.
- **P2** — Access APIs: `RoleRepo` + module `access` (CRUD/assignment) + guard last-admin/immutability. Route test.
- **P3** — Console: 4 trang frontend + nhóm sidebar + api client + Vitest. Full CI + browser smoke.

## Ngoài phạm vi (follow-up)
Quản lý thành viên workspace (`ws_*` trong 1 workspace); SCIM; audit log chi tiết; enforce permission scope-workspace.
