-- Epic B: named, reusable per-workspace secrets.
-- `secrets` stays the AES-GCM vault; `workspace_secrets` is the named layer above it.
-- Backfill existing inline connection secrets into named entries and repoint the FKs.

CREATE TABLE workspace_secrets (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text,
    secret_ref   uuid NOT NULL REFERENCES secrets(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_secrets_workspace_id_idx ON workspace_secrets(workspace_id);

-- Backfill workspace_storages inline secrets, then repoint secret_ref to the named row.
WITH ins AS (
    INSERT INTO workspace_secrets (workspace_id, name, secret_ref)
    SELECT workspace_id, name || ' secret', secret_ref
    FROM workspace_storages WHERE secret_ref IS NOT NULL
    RETURNING id, secret_ref AS old_ref
)
UPDATE workspace_storages s SET secret_ref = ins.id FROM ins WHERE s.secret_ref = ins.old_ref;

-- Backfill databases inline secrets (only workspace-scoped rows), then repoint secret_id.
WITH ins AS (
    INSERT INTO workspace_secrets (workspace_id, name, secret_ref)
    SELECT workspace_id, name || ' secret', secret_id
    FROM databases WHERE secret_id IS NOT NULL AND workspace_id IS NOT NULL
    RETURNING id, secret_ref AS old_ref
)
UPDATE databases d SET secret_id = ins.id FROM ins WHERE d.secret_id = ins.old_ref;

-- Orphan databases rows (secret_id set but no workspace) cannot map to a named
-- secret; clear their dangling secret_id so the new FK accepts them.
UPDATE databases SET secret_id = NULL WHERE workspace_id IS NULL;

-- Repoint workspace_storages.secret_ref FK to workspace_secrets.
-- (No pre-existing FK on this column to drop.)
ALTER TABLE workspace_storages ADD CONSTRAINT workspace_storages_secret_ref_fkey
    FOREIGN KEY (secret_ref) REFERENCES workspace_secrets(id);

-- Repoint databases.secret_id FK to workspace_secrets.
ALTER TABLE databases DROP CONSTRAINT databases_secret_id_fkey;
ALTER TABLE databases ADD CONSTRAINT databases_secret_id_fkey
    FOREIGN KEY (secret_id) REFERENCES workspace_secrets(id) ON DELETE SET NULL;
