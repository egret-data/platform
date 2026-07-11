-- B2-flip: access_grants is now the single grant store. Defensively re-backfill
-- (self-heal any historical drift), then drop the sync triggers, their functions,
-- and the two legacy grant tables.
--
-- IMPORTANT — NULL scope_id gap: the 0068/0072 UNIQUE includes scope_id, which is
-- NULL for PLATFORM grants. Postgres treats NULLs as DISTINCT, so `ON CONFLICT DO
-- NOTHING` never fires for platform rows and would DUPLICATE them (over-counting
-- count_platform_admins → weakening the last-admin guard). Use NOT EXISTS guards
-- for the re-backfill, and add a partial unique index that makes platform grants
-- properly unique at the DB level (integrity guarantee for the last-admin count).

-- Self-heal: collapse any pre-existing duplicate platform grants before adding the
-- unique index. A platform grant is keyed by (principal, role) at platform scope;
-- extra rows are exact duplicates (the grant is unchanged by removing them). None
-- exist in production (the flip writers use NOT EXISTS, and the legacy PK gated the
-- trigger), but a shared/long-lived dev DB may hold test-seed residue from before
-- this index existed — this DELETE makes the migration robust on any database.
DELETE FROM access_grants a USING access_grants b
WHERE a.scope_type = 'platform' AND b.scope_type = 'platform'
  AND a.principal_type = b.principal_type
  AND a.principal_id = b.principal_id
  AND a.role_id = b.role_id
  AND a.ctid > b.ctid;

-- Platform-grant integrity: one grant per (principal, role) at platform scope.
CREATE UNIQUE INDEX access_grants_platform_uniq
  ON access_grants (principal_type, principal_id, role_id)
  WHERE scope_type = 'platform';

-- Defensive re-backfill (dedup-safe via NOT EXISTS; the tables still exist here).
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT pra.principal_type, pra.principal_id, pra.role_id, 'platform', NULL
  FROM platform_role_assignments pra
  WHERE NOT EXISTS (
    SELECT 1 FROM access_grants ag
    WHERE ag.principal_type = pra.principal_type AND ag.principal_id = pra.principal_id
      AND ag.role_id = pra.role_id AND ag.scope_type = 'platform');
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
  FROM workspace_members wm JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role
  WHERE NOT EXISTS (
    SELECT 1 FROM access_grants ag
    WHERE ag.principal_type = wm.principal_type AND ag.principal_id = wm.principal_id
      AND ag.role_id = r.id AND ag.scope_type = 'workspace' AND ag.scope_id = wm.workspace_id);

DROP TRIGGER IF EXISTS sync_platform_grant_trg ON platform_role_assignments;
DROP TRIGGER IF EXISTS sync_workspace_grant_trg ON workspace_members;
DROP FUNCTION IF EXISTS sync_platform_grant();
DROP FUNCTION IF EXISTS sync_workspace_grant();
DROP TABLE IF EXISTS platform_role_assignments;
DROP TABLE IF EXISTS workspace_members;
