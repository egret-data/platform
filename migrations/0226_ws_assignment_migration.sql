-- 0226_ws_assignment_migration.sql
-- Reversible record of the implicit workspace-grant -> explicit `ws:<slug> members`
-- group grant rewrite (SP3c-3 Task 3). One row per run; `original` holds, per
-- migrated grant, the pre-migration (grant_id, origin, origin_workspace_id,
-- principal kind, principal ref) so the run can be restored exactly.
CREATE TABLE perm.ws_assignment_migration (
    id bigserial PRIMARY KEY,
    catalogs uuid[] NOT NULL,
    grant_ids uuid[] NOT NULL,
    principal_hashes text[] NOT NULL,
    original jsonb NOT NULL,
    migrated_by text NOT NULL,
    migrated_at timestamptz NOT NULL DEFAULT now()
);
