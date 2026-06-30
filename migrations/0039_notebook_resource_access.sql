-- Per-workspace notebook resource access: which engines and catalogs are
-- exposed to the notebook kernel's connector manifest.
CREATE TABLE notebook_engine_access (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id)  ON DELETE CASCADE,
    PRIMARY KEY (workspace_id, database_id)
);

-- Note: the `catalogs` table was dropped in migration 0026 (lakekeeper took over).
-- Catalog access is now per workspace_warehouse row (each is a lakekeeper warehouse).
CREATE TABLE notebook_catalog_access (
    workspace_id  uuid NOT NULL REFERENCES workspaces(id)          ON DELETE CASCADE,
    warehouse_id  uuid NOT NULL REFERENCES workspace_warehouses(id) ON DELETE CASCADE,
    PRIMARY KEY (workspace_id, warehouse_id)
);
