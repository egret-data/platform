-- Workflow W5b fix round 1 (whole-branch review I3, user decision 2026-09-30):
-- `workflow:use_secrets`, required to save or publish a flow whose content
-- can call `secret()`.
--
-- `secret()` reads any secret of the workspace -- including the engine and
-- database credentials a workspace admin registers under
-- `workspace:manage_members` -- and the flow's author decides where the value
-- goes: a query's outputs, or a JDBC url pointing at a host of their own.
-- `workflow:manage` is an editor act (0132), so it cannot carry this.
--
-- `ws_admin` only. `ws_editor` keeps `workflow:manage`/`workflow:read` and
-- may still run, edit and publish every flow that does not call `secret()`.
-- The platform-scoped `platform_admin` needs no row: its Cedar template is
-- unrestricted (`egret-auth` pdp/templates.rs).
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workflow:use_secrets' FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
