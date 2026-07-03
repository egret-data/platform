-- Seed the RBAC bindings for the SSO test users (see deploy/keycloak/egret-realm.json).
--
-- The realm ships two SSO test users:
--   admin-sso@egret.local  / Admin12345!   in group `platform-admins`
--   member-sso@egret.local / Member12345!  in group `data-team`
--
-- OIDC users JIT-provision as plain members; permissions come from bindings on
-- their GROUP (RBAC supports group principals). Those bindings are admin-owned,
-- NOT part of the IdP — so a DB reset drops them and SSO users lose access until
-- this seed re-applies them. This is the one-time "admin grants access" step.
--
-- Grants (idempotent — safe to re-run):
--   • group `platform-admins` -> platform role `platform_admin`  (Control Plane access)
--   • group `data-team`       -> `ws_editor` in workspaces `demo` and `eng` (if present)
--
-- Run against the egret Postgres:
--   docker exec -i egret-dev-postgres-1 psql -U egret -d egret < deploy/seed-sso-test-rbac.sql
--
-- To bind `data-team` to another workspace, either add its slug to the IN (...)
-- list below, or run:
--   cargo run -p egret-bin -- workspace add-group --workspace <slug> --group data-team --role ws_editor

-- Ensure both OIDC groups exist (matched by the OIDC `groups` claim name).
INSERT INTO groups (name, source) VALUES
  ('platform-admins', 'oidc'),
  ('data-team', 'oidc')
ON CONFLICT (name) DO NOTHING;

-- 1) platform-admins -> platform_admin (group principal).
INSERT INTO platform_role_assignments (principal_type, principal_id, role_id)
SELECT 'group', g.id, r.id
FROM groups g
JOIN roles r ON r.scope = 'platform' AND r.name = 'platform_admin'
WHERE g.name = 'platform-admins'
  AND NOT EXISTS (
    SELECT 1 FROM platform_role_assignments p
    WHERE p.principal_type = 'group' AND p.principal_id = g.id AND p.role_id = r.id
  );

-- 2) data-team -> ws_editor in each present workspace (demo, eng). `added_by` is
--    the local bootstrap admin; skipped entirely if that admin is absent.
INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role, added_by)
SELECT w.id, 'group', g.id, 'ws_editor', a.id
FROM workspaces w
JOIN groups g ON g.name = 'data-team'
JOIN users a ON a.email = 'admin@egret.local'
WHERE w.slug IN ('demo', 'eng')
  AND NOT EXISTS (
    SELECT 1 FROM workspace_members m
    WHERE m.workspace_id = w.id AND m.principal_type = 'group' AND m.principal_id = g.id
  );
