-- Notebook C1.x: folders + soft-delete (trash) for notebooks, mirroring the
-- dashboard subsystem (0019 soft-delete, 0021 folders). Folders are polymorphic
-- by entity_type; notebooks use entity_type='notebook'. folder_id is nullable
-- (NULL = Uncategorized); deleting a folder sends its notebooks back to NULL.
-- Soft-delete via deleted_at (NULL = live, NOT NULL = trashed); the
-- (workspace_id, deleted_at) index serves both the live list and the trash bin.
ALTER TABLE notebooks ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE notebooks ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX notebooks_deleted_idx ON notebooks (workspace_id, deleted_at);
