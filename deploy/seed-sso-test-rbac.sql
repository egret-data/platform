-- Seed the RBAC binding for the SSO test users (see deploy/keycloak/egret-realm.json).
--
-- The realm ships two SSO test users:
--   admin-sso@egret.local  / Admin12345!   in group `platform-admins`
--   member-sso@egret.local / Member12345!  in group `data-team`
--
-- OIDC users JIT-provision as plain members; platform-admin is granted here by
-- assigning the `platform_admin` role to the `platform-admins` GROUP (RBAC
-- supports group principals). Any SSO user in that group is then a platform
-- admin — this is the one-time "admin grants access" step, kept out of the IdP.
--
-- Idempotent. Run against the egret Postgres, e.g.:
--   docker exec -i egret-dev-postgres-1 psql -U egret -d egret < deploy/seed-sso-test-rbac.sql
--
-- (member-sso reaches a workspace via the `data-team` group binding — bind
--  data-team to your workspace once with:
--    cargo run -p egret-bin -- workspace add-group --workspace <slug> --group data-team --role ws_editor)

-- Ensure the `platform-admins` group row exists (matched by the OIDC groups claim name).
INSERT INTO groups (name, source) VALUES ('platform-admins', 'oidc')
  ON CONFLICT (name) DO NOTHING;

-- Grant the group the platform_admin role (guarded so re-runs are no-ops).
INSERT INTO platform_role_assignments (principal_type, principal_id, role_id)
SELECT 'group', g.id, r.id
FROM groups g
JOIN roles r ON r.scope = 'platform' AND r.name = 'platform_admin'
WHERE g.name = 'platform-admins'
  AND NOT EXISTS (
    SELECT 1 FROM platform_role_assignments p
    WHERE p.principal_type = 'group' AND p.principal_id = g.id AND p.role_id = r.id
  );
