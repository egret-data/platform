ALTER TABLE charts   ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX charts_deleted_idx   ON charts   (workspace_id, deleted_at);
ALTER TABLE datasets ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX datasets_deleted_idx ON datasets (workspace_id, deleted_at);
