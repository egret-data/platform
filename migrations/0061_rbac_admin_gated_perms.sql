-- Drop grants for permissions removed from the code catalog
-- (`egret_core::rbac::PERMISSIONS`) as part of closing a privilege-escalation
-- flaw: `role:create`/`role:update`/`role:delete`/`role:assign_permission`,
-- `user:assign_role`, and `group:manage_members` used to be delegatable
-- permissions guarding writes to the authorization graph itself (role
-- definitions, role-permission sets, role/group-role assignments, group
-- membership). A non-admin holding one of them could grant themselves any
-- permission — e.g. `platform_admin` (seeded by 0003) held `user:assign_role`,
-- which gated `PUT /api/admin/users/{id}/platform-roles`
-- (`egret-workspace/src/routes.rs::set_platform_roles`), a route that
-- replaces a user's platform role assignments outright. Any role granted
-- `user:assign_role` could call it to assign itself `platform_admin` directly,
-- bypassing `role_permissions` entirely.
--
-- Those routes (and the analogous ones in
-- `egret-controlplane/src/access/mod.rs`) now gate on `require_platform_admin`
-- / a direct `platform_admin` role check instead, so these keys are no longer
-- enforced anywhere. `platform_admin` itself is unaffected (still holds the
-- role, still bypasses every other check), so dropping these stale grants is
-- pure cleanup — same rationale as 0060 for `database:*`.
DELETE FROM role_permissions
WHERE permission IN (
    'role:create',
    'role:update',
    'role:delete',
    'role:assign_permission',
    'user:assign_role',
    'group:manage_members'
);
