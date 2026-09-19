-- 0148_catalog_permissions.sql
-- Registry catalogs (SP3 §4.7) get their own platform permissions,
-- catalog_registry:*. Every platform role receives the catalog_registry:*
-- counterpart of each warehouse:* it holds, so data_steward gains
-- catalog_registry:read. The workspace-scope catalog:read from 0024 (egret-query's
-- workspace engine view) is a different key and is not touched: NOT because
-- Cedar would let a platform grant pass that check in every workspace (it
-- can't — a platform-scope role's policy is `resource == Platform::"root"`,
-- which never matches a Workspace entity, same action name or not; B1b-2's
-- anti-leak), but because permission keys are scope-blind strings everywhere
-- else they're read (rbac::is_known, the effective-permission lists the admin
-- permission UI renders, and the single Cedar action namespace a policy names
-- by string) — one key meaning two different things there is ambiguous.
-- warehouse:* stays until SP3c removes the Warehouses pages.
INSERT INTO role_permissions (role_id, permission)
SELECT rp.role_id, 'catalog_registry:' || split_part(rp.permission, ':', 2)
  FROM role_permissions rp
  JOIN roles r ON r.id = rp.role_id
 WHERE r.scope = 'platform'
   AND rp.permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete')
ON CONFLICT (role_id, permission) DO NOTHING;
