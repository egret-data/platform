-- DB connections become per-workspace (ADR-19): each database belongs to one
-- workspace instead of being globally registered and granted.
ALTER TABLE databases
    ADD COLUMN workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE;

CREATE INDEX databases_workspace_id_idx ON databases(workspace_id);

-- Backfill: each granted database adopts its workspace (data shows no fan-out).
UPDATE databases d
SET workspace_id = g.workspace_id
FROM workspace_database_grant g
WHERE g.database_id = d.id;

-- Ungranted rows keep workspace_id = NULL (legacy/unreachable), filtered out of
-- every workspace-scoped query. No destructive cleanup is run.

DROP TABLE workspace_database_grant;
