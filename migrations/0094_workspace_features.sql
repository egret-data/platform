-- Per-workspace feature entitlements (module on/off), set by platform admins.
--
-- An ABSENT ROW MEANS ENABLED. That is why this migration has no backfill:
-- every existing workspace keeps every module until an admin turns one off.
-- `feature` is TEXT validated against the code catalog in
-- `egret-workspace/src/features.rs`, not a Postgres enum, so adding a module
-- later needs no migration.
CREATE TABLE workspace_features (
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    feature      TEXT        NOT NULL,
    enabled      BOOLEAN     NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by   UUID        REFERENCES users(id) ON DELETE SET NULL,
    PRIMARY KEY (workspace_id, feature)
);

-- The hot path is "which features are OFF for this workspace", run on every
-- workspace-scoped request that touches a gated module.
CREATE INDEX workspace_features_disabled_idx
    ON workspace_features (workspace_id)
    WHERE enabled = false;
