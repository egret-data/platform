-- catalog:read for all workspace roles; catalog:write for admin + editor.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'catalog:read' FROM roles
WHERE scope = 'workspace' AND name IN ('ws_admin','ws_editor','ws_viewer')
ON CONFLICT (role_id, permission) DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'catalog:write' FROM roles
WHERE scope = 'workspace' AND name IN ('ws_admin','ws_editor')
ON CONFLICT (role_id, permission) DO NOTHING;
