-- Grant the Data API endpoint permissions to the built-in roles.
--
-- Both go to `ws_admin` ONLY, not `ws_editor`, for the same reason 0120 stopped
-- the service-account pair there. An endpoint is not editorial content: it is a
-- PUBLISHED HTTP contract that machine clients call with a bearer token, running
-- SQL against a workspace connection. Authoring one is closer to standing up a
-- service than to writing a chart, so it stops at `ws_admin`. When the UI grows
-- an author role, it will be an explicit grant, not a widening of `ws_editor`.
--
-- `:read` is seeded alongside `:manage` rather than being implied by it. egret's
-- permissions are flat — `require` checks one action and nothing implies
-- anything — so a role that may edit an endpoint must be granted the ability to
-- see the list explicitly, exactly as 0120 does for `service_account:*` and 0077
-- does for `saved_query:*`. A role holding only `:manage` could create endpoints
-- it could never list.
--
-- WHAT THIS GRANTS ON UPGRADE, stated plainly: `ws_admin` is a GLOBAL role row,
-- so these two INSERTs reach every existing `ws_admin` assignment in every
-- existing workspace at once. The `data_api` feature module also defaults ON for
-- any workspace with no entitlement row, which is every workspace that predates
-- it. So the moment this migration runs, every current workspace admin can
-- author Data API endpoints in their workspace with no operator action and no
-- per-workspace opt-in. Authoring alone publishes nothing to the outside world —
-- serving is P4 and still needs a client credential minted under
-- `service_account:manage` — but a draft is one route away from a live endpoint,
-- so this is a capability that appears at upgrade rather than being turned on,
-- and it is written down here rather than left to be discovered. An operator who
-- does not want it must either write a `data_api` entitlement row disabling the
-- module for those workspaces, or remove these two permissions from `ws_admin`.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'data_api_endpoint:read'
  FROM roles
 WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'data_api_endpoint:manage'
  FROM roles
 WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;
