-- ws_viewer should be able to VIEW BI content (Metabase-style viewer). It was
-- seeded with only catalog:read + notebook:read. Add the read perms so a viewer
-- workspace member (and a "Viewer" object share) can read dashboards/charts/datasets.
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, p.perm
FROM roles r
CROSS JOIN (VALUES ('dashboard:read'), ('chart:read'), ('dataset:read')) AS p(perm)
WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'
ON CONFLICT DO NOTHING;
