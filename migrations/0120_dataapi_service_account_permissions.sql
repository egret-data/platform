-- Grant the Data API service-account permissions to the built-in roles.
--
-- Both go to `ws_admin` ONLY, not `ws_editor`. Every other `:read`/`:manage`
-- pair in this tree (0012 datasets, 0077 saved queries, 0110 schedules) reaches
-- down to `ws_editor` because those objects are editorial. A service account is
-- not: it is a machine PRINCIPAL that an admin then grants data access to in
-- Ranger, and its client is a bearer credential for that access. Creating one is
-- an administrative act, so it stops at `ws_admin`.
--
-- `:read` is seeded alongside `:manage` rather than being implied by it. egret's
-- permissions are flat — `require` checks one action and nothing implies
-- anything — so a role that may issue a client must be granted the ability to
-- see the list explicitly, exactly as `saved_query:read` is seeded next to
-- `saved_query:manage` in 0077.
--
-- WHAT THIS GRANTS ON UPGRADE, stated plainly: `ws_admin` is a GLOBAL role row,
-- so these two INSERTs reach every existing `ws_admin` assignment in every
-- existing workspace at once. The `data_api` feature module also defaults ON for
-- any workspace with no entitlement row, which is every workspace that predates
-- it. So the moment this migration runs, every current workspace admin can mint
-- machine credentials for their workspace with no operator action and no
-- per-workspace opt-in. That is the intended reach — a ws_admin already
-- administers that workspace's access — but it is a capability that appears at
-- upgrade rather than being turned on, so it is written down here and in spec
-- §6.1 rather than left to be discovered. An operator who does not want it must
-- either write a `data_api` entitlement row disabling the module for those
-- workspaces, or remove these two permissions from `ws_admin`.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'service_account:read'
  FROM roles
 WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'service_account:manage'
  FROM roles
 WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
