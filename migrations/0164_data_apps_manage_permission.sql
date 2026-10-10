-- app:manage (spec §8): manage an app's shares. P1 deferred it so that no declared
-- permission went unenforced; P2 adds the enforcement site (egret-bi shares).
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'app:manage' FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
