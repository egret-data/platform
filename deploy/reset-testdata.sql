-- Reset egret LOCAL DEV test data to a clean baseline.
--
-- KEEPS:  the break-glass admin + seeded SSO users, any user still referenced by
--         surviving data, and the seeded `demo` and `eng` workspaces (plus their
--         viz demo data).
-- WIPES:  every embedded-catalog object (warehouses, catalog roles, catalog
--         clients, grants), all storage connections + orphan secrets, the whole
--         lakekeeper catalog schema (namespaces, tables, snapshots, metadata),
--         every OTHER (test) workspace — which cascades to its notebooks,
--         dashboards, connections, etc. — and every orphan/test user left over.
--
-- The lakekeeper schema is DROPPED rather than surgically truncated (it has
-- ~45 tables + triggers); `egret serve` re-migrates + re-bootstraps an empty
-- catalog schema on its next start. MinIO objects are cleared separately by
-- deploy/reset-testdata.sh.
--
-- Run via `just reset-testdata` (which also stops serve + clears MinIO), or
-- directly: psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f deploy/reset-testdata.sql

BEGIN;

-- Embedded-catalog control-plane rows (all test-created).
DELETE FROM catalog_grants;
DELETE FROM catalog_clients;
DELETE FROM catalog_roles;
DELETE FROM warehouses;

-- Every workspace except the two seeded ones; FKs cascade the rest
-- (notebooks, dashboards, database/storage connections, secrets, …).
DELETE FROM workspaces WHERE slug NOT IN ('demo', 'eng');

-- Prune orphan/test users. Keep set = the break-glass admin + seeded SSO users,
-- plus everyone still referenced by surviving rows (they own kept content, and
-- the NO-ACTION FKs below would block their deletion anyway). This runs AFTER
-- the workspace wipe so test users' content is already gone.
CREATE TEMP TABLE _keep_users ON COMMIT DROP AS
      SELECT id FROM users
        WHERE email IN ('admin@egret.local', 'admin-sso@egret.local', 'member-sso@egret.local')
  UNION SELECT owner_user_id FROM notebooks        WHERE owner_user_id IS NOT NULL
  UNION SELECT created_by    FROM workspaces        WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM charts            WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM dashboards        WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM databases         WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM datasets          WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM folders           WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM saved_queries     WHERE created_by IS NOT NULL
  UNION SELECT user_id       FROM queries           WHERE user_id IS NOT NULL
  UNION SELECT added_by      FROM workspace_members WHERE added_by IS NOT NULL;

-- policy_audit.actor is NO-ACTION and would block; drop audit rows for pruned users.
DELETE FROM policy_audit WHERE actor IS NOT NULL AND actor NOT IN (SELECT id FROM _keep_users);
DELETE FROM users WHERE id NOT IN (SELECT id FROM _keep_users);

-- Storage connections are global test config; the just-deleted warehouses were
-- their only references. Drop them all, then drop every secret no longer
-- referenced by surviving rows (viz database connections, workspace secrets,
-- active refresh tokens) — this clears the pile of test/OIDC secrets.
DELETE FROM storage_connections;
-- Global (control-plane) named secrets — storage-connection creds + test
-- secrets — carry workspace_id NULL. Drop those; keep workspace-scoped ones
-- (demo/eng). Then drop every secret blob no longer referenced by anything.
DELETE FROM workspace_secrets WHERE workspace_id IS NULL;
DELETE FROM secrets WHERE id NOT IN (
        SELECT secret_id  FROM databases           WHERE secret_id IS NOT NULL
  UNION SELECT secret_ref FROM workspace_secrets    WHERE secret_ref IS NOT NULL
  UNION SELECT secret_id  FROM oidc_refresh_tokens  WHERE secret_id IS NOT NULL
);

-- Drop lakekeeper's catalog metadata wholesale; serve re-creates it on boot.
DROP SCHEMA IF EXISTS lakekeeper CASCADE;

COMMIT;

\echo 'Postgres reset complete. Remaining workspaces:'
SELECT slug, name FROM workspaces ORDER BY slug;
