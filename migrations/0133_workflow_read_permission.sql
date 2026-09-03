-- Workflow W1 Task 7: `workflow:read` was owed by migration 0132, which
-- seeded only `workflow:manage` and left listing/viewing routes with no
-- permission any role actually holds -- see that migration's own comment
-- ("managing flows is an editor act") and 0122's ("`:read` is seeded
-- alongside `:manage` rather than being implied by it. egret's permissions
-- are flat -- `require` checks one action and nothing implies anything -- so
-- a role that may edit ... must be granted the ability to see the list
-- explicitly").
--
-- Same two roles 0132 granted `:manage` to, and no wider: `ws_viewer` gets
-- neither, matching that migration's choice to stop workflow authorship (and
-- now, workflow visibility) at editor and above rather than widening it to
-- every workspace member the way `data_api_endpoint:*` deliberately does not.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workflow:read' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin')
ON CONFLICT DO NOTHING;
