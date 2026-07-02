-- Control-plane pivot: Storage becomes global; Secrets gain a global scope.
-- Greenfield: existing workspace_storages rows are dev-only.

-- 1. Storage: drop workspace scoping; rename for clarity; name globally unique.
ALTER TABLE workspace_storages DROP COLUMN workspace_id;
ALTER TABLE workspace_storages RENAME TO storage_connections;
DROP INDEX IF EXISTS workspace_storages_workspace_id_idx;
ALTER TABLE storage_connections ADD CONSTRAINT storage_connections_name_key UNIQUE (name);

-- 2. Secrets: allow global (control-plane) secrets = workspace_id NULL.
ALTER TABLE workspace_secrets ALTER COLUMN workspace_id DROP NOT NULL;
CREATE UNIQUE INDEX workspace_secrets_global_name_key
    ON workspace_secrets (name) WHERE workspace_id IS NULL;
