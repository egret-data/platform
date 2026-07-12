-- Saved-query object-scope (Effort B leftover).
-- 1. folder_id lets a saved query live in an analytics.folders row (entity_type='saved_query').
-- 2. Grant the new saved_query:{read,manage} perms to the seeded workspace roles
--    (mirrors migration 0070's grants for the BI object perms).

ALTER TABLE analytics.saved_queries
  ADD COLUMN folder_id uuid REFERENCES analytics.folders(id) ON DELETE SET NULL;

CREATE INDEX saved_queries_folder_idx ON analytics.saved_queries (folder_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:read'
  FROM roles
 WHERE scope = 'workspace' AND name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:manage'
  FROM roles
 WHERE scope = 'workspace' AND name IN ('ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
