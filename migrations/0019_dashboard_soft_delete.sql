ALTER TABLE dashboards ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dashboards_deleted_idx ON dashboards (workspace_id, deleted_at);
