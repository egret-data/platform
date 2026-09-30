-- Data Apps permissions (spec §8). Viewers open published apps; editors author;
-- admins also manage shares.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:read' FROM roles WHERE scope = 'workspace' AND name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:write' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:manage' FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
