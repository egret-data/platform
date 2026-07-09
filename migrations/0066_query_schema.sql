-- E4c: the Query seam owns its tables. Move databases + queries from `public`
-- into their own `query` schema (mirrors E1b's 0062 for engine_mgmt, E2c's 0063
-- for policy, E3a's 0064 for storage, E3c's 0065 for catalog). SET SCHEMA
-- preserves data, indexes, constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS query;

ALTER TABLE databases SET SCHEMA query;
ALTER TABLE queries   SET SCHEMA query;

-- Databases are strictly per-workspace (ADR-19); legacy register-then-grant
-- orphans with a NULL workspace_id are unreachable and crash the non-Option
-- decode in repo.rs::open_secret. Remove them, then enforce NOT NULL.
-- The four tables referencing query.databases all ON DELETE CASCADE or SET NULL
-- (queries CASCADE, saved_queries SET NULL, datasets CASCADE,
-- notebook_engine_access CASCADE), so the delete cannot be blocked by a child FK;
-- and an ungranted (NULL-workspace) database is unreachable by any per-workspace
-- row anyway. No cascading deletes are added to those analytics tables.
DELETE FROM query.databases WHERE workspace_id IS NULL;
ALTER TABLE query.databases ALTER COLUMN workspace_id SET NOT NULL;

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
-- OUTBOUND (query seam references other seams):
--   query.databases.secret_id    -> public.workspace_secrets  -- SEAM-CROSSING (secrets/foundation) (repointed in 0032)
--   query.databases.created_by   -> public.users              -- SEAM-CROSSING (iam)
--   query.databases.workspace_id -> public.workspaces         -- SEAM-CROSSING (iam/workspace)
--   query.queries.workspace_id   -> public.workspaces         -- SEAM-CROSSING (iam/workspace)
--   query.queries.user_id        -> public.users              -- SEAM-CROSSING (iam)
-- INBOUND (other seams reference query.databases across a schema boundary):
--   saved_queries.database_id          -> query.databases  -- SEAM-CROSSING (analytics)       (0010)
--   datasets.database_id               -> query.databases  -- SEAM-CROSSING (analytics)       (0012)
--   notebook_engine_access.database_id -> query.databases  -- SEAM-CROSSING (analytics/kernel) (0039)
-- In-seam FK unchanged (moved with its table):
--   query.queries.database_id -> query.databases
