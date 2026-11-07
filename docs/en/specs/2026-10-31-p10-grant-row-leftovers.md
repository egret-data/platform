# P10 — small grant-row leftovers (KI-324, two P5b findings)

DG debt Wave 3, package P10. Three rows, one rule: `egret_auth::live_allow_grant!`
(#810/#811) decides whether an `access_grants` row confers its role.

## Row 1 — KI-324: a group delete leaves its role grants

`access_grants.principal_id` names a group by id with no foreign key.
`GroupRepo::delete_local` (the only group delete; OIDC groups are never
deleted, only their memberships) now deletes every `access_grants` row with
`principal_type = 'group'` and the group's id, at every scope, in its own
transaction: after the recompile's affected set is read (it reads the grants)
and before the group row goes.

Existing orphans: migration **0212** deletes the `group` rows whose group no
longer exists. Decision: delete, not report. Such a row grants nobody anything
(the memberships went with the group by cascade, and a group id is a random
uuid no later group gets), so deleting it changes nobody's access; a report
would ask an admin to act on rows that cannot matter.

## Row 2 — the username-reuse report's admin list

`username_reuse::POLICIES` built its `admins` set from every `platform_admin`
grant, so a `deny` or lapsed grant counted as an admin, unlike the engine seed
set (`engine_admin_user_ids_via`). Both of its `EXISTS` now use
`live_allow_grant!("g")`, so "active admin" is exactly the seed set and "holds
admin" is the same rule without the status filter.

## Row 3 — `grant_endpoint_invoke` and an expired or deny row

Re-granting over an expired `allow` row was a no-op (`ON CONFLICT DO NOTHING`
on the unique key), and a `deny` row kept overriding the new `allow`. The
handler now, in one transaction, deletes this (account, endpoint, invoke role)
row set's rows that are not live allows (`NOT live_allow_grant!`), then
inserts. Decision: delete-then-insert rather than updating the old row, so
the existing invariant holds (one row per account and endpoint) and the row's
`created_at` / `granted_by` describe the grant that is live. Removing a deny
row takes nothing away a manager could not already take: revoke deletes the
same rows.
