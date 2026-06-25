-- Per-type, flat folders for Explore resources. Polymorphic by entity_type,
-- mirroring the bookmarks pattern (0020). A resource's folder_id is nullable;
-- NULL = Uncategorized. Deleting a folder sends its items back to Uncategorized.
CREATE TABLE folders (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entity_type  text NOT NULL,           -- 'chart' | 'dashboard' | 'dataset'
    name         text NOT NULL,
    created_by   uuid REFERENCES users(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT folder_name_uniq UNIQUE (workspace_id, entity_type, name)
);
CREATE INDEX folders_ws_type_idx ON folders (workspace_id, entity_type);

ALTER TABLE charts     ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE dashboards ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE datasets   ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
