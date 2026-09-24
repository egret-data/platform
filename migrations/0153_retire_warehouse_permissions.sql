-- 0153_retire_warehouse_permissions.sql
-- SP3c-2 §4 step 4. 0148 granted catalog_registry:* to every platform role
-- holding warehouse:* — once, in the past. Every role created since holds
-- warehouse:* alone, so the grant is RE-RUN here, in the same transaction as
-- the delete and before it. Nobody gains access, nobody loses it.
INSERT INTO role_permissions (role_id, permission)
SELECT rp.role_id, 'catalog_registry:' || split_part(rp.permission, ':', 2)
  FROM role_permissions rp
  JOIN roles r ON r.id = rp.role_id
 WHERE r.scope = 'platform'
   AND rp.permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete')
ON CONFLICT (role_id, permission) DO NOTHING;

DELETE FROM role_permissions
 WHERE permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete');
