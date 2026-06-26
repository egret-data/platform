-- Maps each egret workspace to its lakekeeper warehouse UUID.
-- warehouse_id is stored here (not in lakekeeper itself) so egret can build
-- the REST {prefix} and enforce token ownership without cross-DB queries.
CREATE TABLE workspace_warehouse (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  warehouse_id uuid NOT NULL,
  created_at   timestamptz NOT NULL DEFAULT now()
);
