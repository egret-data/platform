-- Reset egret LOCAL DEV test data to a clean baseline.
--
-- KEEPS:  the break-glass admin + seeded SSO users, any user still referenced by
--         surviving data, and the seeded `demo` and `eng` workspaces (plus their
--         viz demo data).
-- WIPES:  every embedded-catalog object (warehouses, catalog roles, catalog
--         clients, grants), all storage connections + orphan secrets, the whole
--         lakekeeper catalog schema (namespaces, tables, snapshots, metadata),
--         every OTHER (test) workspace — which cascades to its notebooks,
--         dashboards, connections, etc. — every RBAC role outside the five the
--         schema seeds, and every orphan/test user left over.
--
-- The lakekeeper schema is DROPPED rather than surgically truncated (it has
-- ~45 tables + triggers); `egret serve` re-migrates + re-bootstraps an empty
-- catalog schema on its next start. RustFS objects are cleared separately by
-- deploy/reset-testdata.sh.
--
-- Tables are SCHEMA-QUALIFIED for the service-ready owned-schema layout (E-series
-- split: catalog / engine_mgmt / analytics / query / storage / audit / public).
-- Keep this file in sync if a table moves schemas or is renamed.
--
-- Run via `just reset-testdata` (which also stops serve + clears RustFS), or
-- directly: psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f deploy/reset-testdata.sql

BEGIN;

-- Embedded-catalog control-plane rows (all test-created).
DELETE FROM catalog.catalog_grants;
DELETE FROM catalog.catalog_clients;
DELETE FROM catalog.catalog_roles;
-- Engines must go first (SP3c-2 Task 6: not because of `engines.warehouse_id`
-- any more — that column and FK are gone — but because
-- `registry.engine_catalog_attachments.engine_id` cascades from engines,
-- while its `catalog_id` FK is NO ACTION; deleting warehouses first would
-- cascade into deleting `registry.catalogs` while attachment rows still
-- pointed at them).
DELETE FROM engine_mgmt.engines;
DELETE FROM catalog.warehouses;

-- Every workspace except the two seeded ones; FKs cascade the rest
-- (notebooks, dashboards, database/storage connections, secrets, …).
DELETE FROM public.workspaces WHERE slug NOT IN ('demo', 'eng');

-- Every RBAC role except the five the schema seeds (migration 0003). Integration
-- tests create roles by the hundred — `appwiring-*`, `bi_authz_*`, `hotreload-*`,
-- `forbid_test_*`, `attacker_role-*` — and nothing ever removed them, so they
-- accumulated across every run and every reset. Two consequences, both real:
--
--   * the Add-member Role picker becomes a list of ~1800 entries with the five
--     usable ones buried in it;
--   * some of those roles carry permissions that are not in
--     `egret_core::rbac::PERMISSIONS` (`a:read`, `x:read`, `role:create`, …), and
--     the startup check in egret-bin PANICS on them under `debug_assertions` — so
--     a `just serve` eventually refuses to start on a developer's own database.
--
-- `role_permissions` and `access_grants` both cascade from `roles`, and
-- `roles.parent_role_id` is ON DELETE SET NULL, so this one statement is enough.
DELETE FROM public.roles
 WHERE name NOT IN ('platform_admin', 'data_steward', 'ws_admin', 'ws_editor', 'ws_viewer');

-- Prune orphan/test users. Keep set = the break-glass admin + seeded SSO users,
-- plus everyone still referenced by surviving rows (they own kept content, and
-- the NO-ACTION FKs below would block their deletion anyway). This runs AFTER
-- the workspace wipe so test users' content is already gone.
CREATE TEMP TABLE _keep_users ON COMMIT DROP AS
      SELECT id FROM public.users
        WHERE email IN ('admin@egret.local', 'admin-sso@egret.local', 'member-sso@egret.local')
  UNION SELECT owner_user_id FROM analytics.notebooks     WHERE owner_user_id IS NOT NULL
  UNION SELECT created_by    FROM public.workspaces        WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM analytics.charts         WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM analytics.dashboards     WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM query.databases          WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM analytics.datasets       WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM analytics.folders        WHERE created_by IS NOT NULL
  UNION SELECT created_by    FROM analytics.saved_queries  WHERE created_by IS NOT NULL
  UNION SELECT user_id       FROM query.queries            WHERE user_id IS NOT NULL;

-- audit_log.actor_user_id is NO-ACTION and would block; drop audit rows for pruned users.
DELETE FROM audit.audit_log WHERE actor_user_id IS NOT NULL AND actor_user_id NOT IN (SELECT id FROM _keep_users);
DELETE FROM public.users WHERE id NOT IN (SELECT id FROM _keep_users);

-- Storage connections are global test config; the just-deleted warehouses were
-- their only references. Drop them all, then drop every secret no longer
-- referenced by surviving rows (viz database connections, workspace secrets,
-- active refresh tokens) — this clears the pile of test/OIDC secrets.
DELETE FROM storage.storage_connections;
-- Global (control-plane) named secrets — storage-connection creds + test
-- secrets — carry workspace_id NULL. Drop those; keep workspace-scoped ones
-- (demo/eng). Then drop every secret blob no longer referenced by anything.
DELETE FROM public.workspace_secrets WHERE workspace_id IS NULL;
DELETE FROM public.secrets WHERE id NOT IN (
        SELECT secret_id  FROM query.databases            WHERE secret_id IS NOT NULL
  UNION SELECT secret_ref FROM public.workspace_secrets   WHERE secret_ref IS NOT NULL
);

-- Drop lakekeeper's catalog metadata wholesale; serve re-creates it on boot.
DROP SCHEMA IF EXISTS lakekeeper CASCADE;

COMMIT;

\echo 'Postgres reset complete. Remaining workspaces:'
SELECT slug, name FROM public.workspaces ORDER BY slug;
