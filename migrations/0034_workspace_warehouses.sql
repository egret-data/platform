-- Catalog sub-epic 1: turn the single auto-provisioned warehouse per workspace
-- into N user-managed warehouses, each optionally on a registered storage
-- connection. Catalog tokens become per-warehouse.
--
-- The legacy `workspace_warehouse` (1:1) and the unused singular
-- `workspace_storage` tables are dropped at the end of this migration.

CREATE TABLE workspace_warehouses (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    warehouse_id    uuid NOT NULL,                       -- lakekeeper warehouse uuid
    name            text NOT NULL,
    storage_conn_id uuid REFERENCES workspace_storages(id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_warehouses_workspace_id_idx ON workspace_warehouses(workspace_id);

ALTER TABLE catalog_tokens
    ADD COLUMN warehouse_id uuid REFERENCES workspace_warehouses(id) ON DELETE CASCADE;

-- Backfill: every legacy 1:1 mapping becomes a 'default' warehouse with no
-- storage connection (the legacy warehouse came from config default_storage,
-- not from a workspace_storage DB row, so storage_conn_id is NULL).
INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name, storage_conn_id)
SELECT workspace_id, warehouse_id, 'default', NULL FROM workspace_warehouse;

-- Re-point existing tokens at the backfilled default warehouse.
UPDATE catalog_tokens t SET warehouse_id = wh.id
FROM workspace_warehouses wh
WHERE wh.workspace_id = t.workspace_id AND wh.name = 'default';

DROP TABLE workspace_warehouse;
DROP TABLE workspace_storage;
