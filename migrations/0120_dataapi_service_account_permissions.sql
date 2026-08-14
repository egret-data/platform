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
