-- Data Apps permissions (spec §8). Viewers open published apps; editors author.
-- `app:manage` (share management) arrives with shares in P2.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:read' FROM roles WHERE scope = 'workspace' AND name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:write' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
