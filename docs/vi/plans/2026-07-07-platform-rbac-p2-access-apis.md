# Kế hoạch — Platform RBAC P2 (Access APIs)

> Bản chuẩn (interface + endpoint + guard đầy đủ): `docs/en/plans/2026-07-07-platform-rbac-p2-access-apis.md`. File này là tổng quan.

**Mục tiêu:** API CRUD + assignment cho Users/Groups/Roles + catalog Permissions trên schema sẵn có, mỗi endpoint guard bằng permission P1, kèm guard chống lock-out + role seeded bất biến.

## Task (subagent-driven, commit từng task)
1. **`RoleRepo`** (egret-auth/roles.rs) — CRUD roles, role_permissions (get/set-all trong tx), platform_role_assignments (assign/unassign/list), `roles_of`, `count_platform_admins`, `is_seeded`. Thêm `roles` vào CpState. DB test.
2. **UserRepo/GroupRepo bổ sung** — `UserRepo::set_status`; `GroupRepo` create_local/rename/delete_local(chỉ source='local')/add_member/remove_member/members_of/list_with_meta. DB test.
3. **Module `access` — Users & Groups** — `GET/POST /users`, `PATCH /users/{id}` (disable cần `user:disable`), `POST/DELETE /users/{id}/roles/*` (guard last-admin 409), `GET/POST/PATCH/DELETE /groups`, `/groups/{id}/members/*`, `/groups/{id}/roles/*`. Guard `user:*`/`group:*`. Thêm `CpError::Conflict` (409).
4. **Module `access` — Roles & Permissions** — `/access/roles` (list/create/update-desc/delete[seeded→409]), `PUT /access/roles/{id}/permissions` (validate qua `rbac::is_known`, 400 nếu lạ), `GET /permissions` (catalog + role nào có quyền). Guard `role:*`.
5. **Route test + gate** — assign/unassign, last-admin 409, group local CRUD, oidc-delete 409, permission lạ 400, seeded-delete 409, 403 thiếu `role:read`. fmt/clippy/test egret-auth+egret-controlplane trên egret_test.

## Ghi chú
- Password hash: dùng đúng hasher của login local. Gán role cho group cũng guard `user:assign_role`. SQL nằm trong repo, handler mỏng.
