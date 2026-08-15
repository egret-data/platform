-- Spec §6.2 step 4: `data_api_endpoint:invoke`, the permission the serving path
-- checks before it runs a published endpoint.
--
-- Until this landed, ANY valid unexpired `data-api` token for a workspace
-- reached EVERY live endpoint in that workspace: a service account created for
-- one consumer could invoke every other consumer's endpoint and receive its full
-- result set. Service-account granularity collapsed to workspace granularity and
-- an endpoint was not an access-control boundary at all. The only thing behind
-- it was the engine's own row filtering, which is absent where Ranger is not
-- enforcing and resource-only on StarRocks.
--
-- THIS PERMISSION IS NOT MEANT TO BE HELD WORKSPACE-WIDE BY A HUMAN. The role
-- seed below is the smaller half of the story: the way an operator actually
-- hands out invoke rights is ONE `access_grants` row per (service account,
-- endpoint) — `scope_type = 'object'`, `object_type = 'data_api_endpoint'`,
-- `scope_id = <endpoint id>` — written by
-- `POST /api/ws/{workspace}/data-api/endpoints/{id}/grants` (spec §5). That is
-- why `data_api_invoker` exists below: an object-scoped grant links its role's
-- WHOLE action list onto the object, so granting `ws_viewer` object-scoped would
-- hand the service account every `ws_viewer` action on that endpoint. A role
-- carrying exactly one permission is the only way the grant means exactly what
-- it says.

-- ── The seed on ws_admin ────────────────────────────────────────────────────
--
-- `ws_admin` ONLY, not `ws_editor`, for the same reason 0122 stopped the
-- endpoint pair there and 0120 stopped the service-account pair there. An
-- endpoint is a published HTTP contract running SQL against a workspace
-- connection, not editorial content.
--
-- It is seeded alongside `:read`/`:manage` rather than being implied by either.
-- egret's permissions are flat — `require` checks one action and nothing implies
-- anything — so a role that may author an endpoint must be granted the ability
-- to call one explicitly, exactly as 0122 seeds `:read` next to `:manage`.
--
-- WHAT THIS GRANTS ON UPGRADE, stated plainly: `ws_admin` is a GLOBAL role row,
-- so this INSERT reaches every existing `ws_admin` assignment in every existing
-- workspace at once, and the `data_api` feature module defaults ON for any
-- workspace with no entitlement row. So from the moment this migration runs,
-- every current workspace admin holds `data_api_endpoint:invoke` workspace-wide.
--
-- **What that is worth in practice is nothing, and the reason is worth writing
-- down rather than leaving to be inferred**: the serving surface authenticates a
-- `data-api` audience JWT minted for a SERVICE ACCOUNT and nothing else — a
-- human's browser session cannot reach `/data-api/v1/…` at all (spec §6.1). A
-- ws_admin therefore cannot use this permission directly. It is seeded so the
-- catalog is consistent (every workspace permission a built-in role could
-- plausibly need appears on the role that administers that workspace) and so a
-- future in-product "try it" console run as the human has something to check.
-- An operator who does not want it can remove it from `ws_admin`; nothing on the
-- serving path depends on the seed.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'data_api_endpoint:invoke'
  FROM roles
 WHERE scope = 'workspace' AND name = 'ws_admin'
ON CONFLICT DO NOTHING;

-- ── The role object grants link ─────────────────────────────────────────────
--
-- WORKSPACE-scoped, deliberately: `templates::template_for_role` emits
-- `resource in ?resource` for a workspace role and `resource == ?resource` for a
-- platform one, and only the cascading `in` form matches an Object resource
-- reached through an object-scoped grant. A platform-scoped role here would link
-- a policy that never fires, and the grant would be silently inert.
--
-- Exactly ONE permission, and it must stay that way. Adding a second to this
-- role widens every already-written invoke grant on every endpoint at once,
-- because a grant names the ROLE and the PDP links the role's whole effective
-- action set (including anything inherited through `parent_role_id`) onto the
-- granted object. If a future need arises for a service account to do something
-- else to one endpoint, that is a second role and a second grant.
INSERT INTO roles (scope, name, description) VALUES
    ('workspace', 'data_api_invoker',
     'Call one published Data API endpoint. Granted object-scoped to a service account; never assigned as a workspace membership.')
ON CONFLICT (scope, name) DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'data_api_endpoint:invoke'
  FROM roles
 WHERE scope = 'workspace' AND name = 'data_api_invoker'
ON CONFLICT DO NOTHING;
