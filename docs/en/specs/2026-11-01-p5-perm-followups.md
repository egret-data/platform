# P5 — perm-compile follow-ups (#787, #788, #789, #801, KI-322, KI-323)

DG debt wave 3, package P5. Owner decisions are settled (DG debt plan
2026-10-31); this page records what each row does and the details this package
decided.

## #787 — boot role repair retries

`recompile_role_referencing_profiles` ran once per boot; a failure was logged
and never retried. Now `egret-bin` spawns
`role_membership::repair_role_referencing_profiles_with_retry`, off the boot
path. It retries on any error, waiting 5 s, 30 s, then 2 min before each retry
(`BOOT_REPAIR_BACKOFF`). Each failure logs a `warn` with the attempt number.
After the last failure it logs an `error` that says the role-referencing
profiles were not recompiled.

Decision: "3 attempts, backoff 5 s / 30 s / 2 min" has three waits, so it is
read as one first attempt plus three retries (four attempts). The loop
(`retry_boot_repair`) takes the attempt as a closure, so tests can drive it
with paused tokio time.

## #788 — lazy `policy:read` in group rename/delete

`GroupRepo::rename` and `delete_local` now take
`detail: impl ReferenceDetailSource` instead of a resolved `ReferenceDetail`.
They ask for the detail only when they refuse because the model names the
group. A plain `ReferenceDetail` is its own source, so existing callers and
tests do not change. The control plane passes `PolicyReadDetail`, which runs
the PDP `policy:read` check when asked. A rename or delete that nothing names
runs no `policy:read` check. A PDP error on that check now fails only a request
that is being refused.

## #789 — old affected set from the row re-read under the lock

`write_grant` and `write_rule` still read the stored row before any lock,
because that read names the group locks to take. After the group locks, they
re-read the row and take the old affected set, the old scope or selector, the
origin and the R6 kept set from that fresh row.
`ensure_*_still_covered` stays after the profile locks, because the window
between the fresh read and the profile locks is still there.

Test seam (accepted by the owner): `write_seam::before_locks()` is called
between the first read and the lock wait. It is a tokio task-local that only a
test scopes (`write_seam::run_with`). In production it returns at once.

## #801 — grant/rule ids do not leak existence

New `routes::grant_in_reach` and `routes::rule_in_reach` read the stored row
and confine it. For a confined caller, a row outside its workspace answers
`NotFound`, the same as a missing id. These routes use them:
`PATCH`/`DELETE /perm/grants/{id}`, `PATCH`/`DELETE /perm/rules/{id}` (through
`write_grant`/`write_rule` and the delete handlers) and
`GET /perm/rules/{id}/effect`. `rule_effect` had no confinement before; it has
now. A body's catalog still answers `403`: it names no row.

Premise: partly. The handlers did order the checks this way, but no session
builds a confined principal that passes the `policy:*` gate. A ws-scoped overlay
over a platform-role holder is inert, and one over anyone else has no platform
authority. So the fix is defence in depth, like `confine_to_overlay_workspace`
itself. Tests: the helpers are tested directly with synthesized principals, and
an HTTP test checks that a user without `policy:*` gets the same answer for a
real id as for a random one. Engine grants (`/perm/engine-grants/{id}`) have no
workspace confinement at all, so they have no "not yours" answer to leak.

## KI-322 — engine-admin set and last-admin guard honour expiry

`roles::engine_admin_ids_query` now counts only grants that have not expired
(`expires_at IS NULL OR expires_at > now()`, the rule the session and PDP use).
It also counts only `effect = 'allow'`, the rule the console's access resolver
uses for "who has access". `LastAdminGuard` uses the same query. A lapsed
`platform_admin` leaves the engines at the next admin-policy recompile; the
hourly expiry sweep already schedules that recompile for a lapsed
`platform_admin` row.

## KI-323 — the session's platform roles honour `effect`

The session's `platform_roles` and its `permissions` list now read only
`effect = 'allow'` grants, as `effective::AccessResolver` does. A deny grant no
longer puts its role or that role's permissions in the session. Decision: a
deny does not also subtract an allow grant of the same role from the session
list. The console's resolver does not subtract either, and the PDP's forbid
template still enforces the deny.
