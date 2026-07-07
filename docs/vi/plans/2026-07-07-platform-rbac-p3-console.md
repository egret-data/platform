# Kế hoạch — Platform RBAC P3 (Admin Console)

> Bản chuẩn: `docs/en/plans/2026-07-07-platform-rbac-p3-console.md`. File này là tổng quan.

**Mục tiêu:** Frontend **Access control**: 4 trang Users/Groups/Roles/Permissions trên API P2, gom 1 nhóm sidebar Control Plane.

**Tech/convention:** React 18 + TanStack Query + shadcn; theo `Policies.tsx`/`ProfileDetailPage.tsx` (AppShell padding, `COLUMNS`, ConfirmDeleteDialog, Alert, Switch, Badge, token). API P2 dưới `/api/control-plane`.

## Task (subagent-driven)
1. **api client + sidebar + routes + trang Permissions** — `src/api/access.ts` (types + hàm cho mọi endpoint); thêm nhóm sidebar "Access control" (Users/Groups/Roles/Permissions), bỏ Users khỏi nhóm Control Plane; App.tsx routes (3 trang kia stub tạm); **PermissionsPage** read-only (catalog theo nhóm + role nào có quyền). Vitest.
2. **UsersPage + GroupsPage** — Users: bảng Email/Name/Provider/Status/Roles + tạo user, sửa/disable, gán role (diff), guard last-admin 409 inline. Groups: Name/Source/Members/Roles + tạo local, rename/delete (local; oidc→409+disable), manage members, gán role. Vitest.
3. **RolesPage** — Scope/Name/Desc/#perms/#assignees; tạo role; sửa (description + **checklist permission** theo nhóm từ catalog, prefill) → setRolePermissions; seeded → badge built-in + Delete disabled. Vitest.
4. **Gate + browser smoke** — lint/typecheck/test/build; rebuild serve; smoke admin@egret.local: 4 trang, tạo group/role, set permission, gán role, last-admin bị chặn. 0 console error.

## Ghi chú
- Xác nhận casing JSON của DTO P2 (snake/camel) rồi type cho khớp. Query key `["control-plane","access",…]`.
