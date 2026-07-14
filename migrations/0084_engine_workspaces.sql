-- P1: assign a provisioned engine to a workspace. The join table records the
-- assignment; `query.databases.engine_id` marks a connection as provisioned
-- (read-only, managed by an assignment) and links it back to the source engine.
CREATE TABLE engine_mgmt.engine_workspaces (
    engine_id    uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    assigned_by  uuid REFERENCES users(id),
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, workspace_id)
);

ALTER TABLE query.databases
    ADD COLUMN engine_id uuid NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE;

-- A workspace can hold at most one materialized connection per engine.
CREATE UNIQUE INDEX databases_engine_per_workspace
    ON query.databases (workspace_id, engine_id) WHERE engine_id IS NOT NULL;
