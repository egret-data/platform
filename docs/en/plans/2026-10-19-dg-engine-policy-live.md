# Engine & policy: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-19-dg-engine-policy.md`. Spec:
> `docs/en/specs/2026-10-19-dg-engine-policy-design.md` (premises 1–12, EP1–EP21, Q1–Q4). Branch
> `feat/dg-engine-policy`, worktree `egret-dgep`.

## Measurements (Task 1)

The question: do the premises that reading alone could not settle hold on a real stack? Seven cells: how compiled
policy names map back to their sources (M1), the `valueKind`s the capability catalogue serves (M2), whether the grant
dialog's catalog list equals what the server accepts (M3), what the policy lists hold (M4), what the index can read
(M5), how impersonation is exercised on each engine (M6), and what `explain/capability` answers (M7).

### 1.0 Predictions (committed before anything was observed)

Written from the code at `fd503694` (main `3731f0ce` plus the spec and plan commits, which change no code), before any
container of this run was started. A wrong prediction is a finding. Nothing in this subsection is edited after
observing.

**Setup the predictions are about.**
- The stack of `docs/en/plans/2026-10-18-dg-overview-live.md` §1.2/§2.2, renamed: image `egret-backend:dgep-1` from
  this branch; throwaway DB `egret_dgep_live` in the private `dgep-pg`; containers `dgep-egret`, `dgep-rustfs`,
  `dgep-trino` (Trino 483, JWT/TLS, profile `dgtr` with `tagService = dgtr-tag`, delegating), `dgep-sr` (StarRocks
  4.1.6 allin1, `access_control = ranger`, profile `dgsr`) on network `dgep-net`; Ranger poll 5000 ms; engine rows
  `provisioned`/`running` by SQL (DEV-n); a catalog `ice` attached to both engines, with one real Iceberg table
  `ice.s.t (id, email)` so a column can carry a tag.
- Seeds through the API: one data grant, one mask rule (`dgtr`), one tag-addressed mask rule that StarRocks expands
  per column (a locator), a classification with a clearance on `email` (both engines read `ice`), one engine grant per
  capability the stack can satisfy, one hand-written policy per kind (access, masking, row filter) on `dgtr`, and one
  by-tag policy in `dgtr-tag`. Users `ua`, `ub`, `uc` with usernames; StarRocks users of the same names created by
  `root` without a password.

**Cells.**

| id | check | prediction |
|---|---|---|
| M1a | every `managed_by = 'compiler'` row parses as premise 5 says | yes: `perm-grant-<uuid>` (bare), `perm-rule-<uuid>` (Trino) and `perm-rule-<uuid>-<locator>` (StarRocks per column), `perm-clearance-<tag uuid>` (Trino) and `perm-clearance-<tag uuid>-<locator>` (StarRocks per column; a clearance mask, if any, as `-mask-<c>.<d>.<t>.<col>`), `perm-clearance-mv` (StarRocks), `perm-engine-<uuid>` (bare) |
| M1b | each uuid exists in its table | yes: `perm.grants`, `perm.rules`, `tags`, `perm.engine_grants` |
| M1c | where clearance policies live | Trino: in the tag service `dgtr-tag`; StarRocks: in `dgsr` itself, one per tagged column |
| M1d | a `compiler` row without a prefix, or a prefixed row that is not `compiler` | none of either |
| M2 | `resources[].valueKind` per engine type | Trino: {`username`, `identifier`, `catalog`}; StarRocks: {`username`, `function_signature`, `fixed`}; `fixed` only on `starrocks.manage_blacklist` (`system`) |
| M3 | catalogs of `GET /governance/catalogs` whose `engines` names an engine of the grant's `reach`, vs the list in the 400 for an unreached catalog | equal: both are `ice` (both read `registry.engine_catalog_attachments` with no status filter) |
| M4a | `managed_by` values in `GET /policies?service=dgtr` | `compiler`, `infra`, `operator` |
| M4b | `policy_type` values seen in either list | only `access`, `masking`, `rowfilter`; a tag service's rows are typed by kind, never `tag` |
| M4c | compiled clearance policies in the lists | Trino's in `?service=dgtr-tag` (as `compiler`), StarRocks' in `?service=dgsr` |
| M5a | `GET /policies/services` `engines[]` of a bound profile | `{id, name, status}` only (no origin, no type); keys snake_case (`tag_service`, `policy_count`, `policy_version`) |
| M5b | a tag service row | `type = "tag"`, `engines = []`, `tag_service = null`: distinguishable by its type as well as by being named |
| M5c | `GET /control-plane/engines` as a `policy:read`-only user | 403 |
| M6a | Trino before the grant: JWT principal `ua`, header `X-Trino-User: ub`, `SELECT current_user` | refused, `PERMISSION_DENIED`, "Access Denied: User ua cannot impersonate user ub" |
| M6b | Trino after granting `trino.impersonate_user` (ua → ub) and one poll | `OK`, `current_user = ub`; the same request as `uc` still refused |
| M6c | StarRocks before the grant: as `ua`, `EXECUTE AS ub WITH NO REVERT; SELECT current_user()` | refused, an access-denied message naming the `IMPERSONATE` privilege on user `ub` (exact text not confident) |
| M6d | StarRocks after granting `starrocks.impersonate_user` (ua → ub) and one poll | `current_user()` is `'ub'@'%'`; as `uc` still refused |
| M7a | `explain/capability` Trino, `ua` impersonating `ub`, delegating profile | `Undecidable`, the cause naming `{USER}` (F11 C9) |
| M7b | `explain/capability` StarRocks | `Allowed` for `ua` (deciding policy `perm-engine-<grant id>`), `Denied` for `uc` |

**Stop rules (plan Task 1).** If M1 shows compiler rows outside premise 5's shapes (other than the MV deny), or M3
shows the two catalog lists differ: record it and tell the controller before Tasks 9/10. If M6 cannot exercise
impersonation on an engine, Task 17's prediction for that engine becomes "not provable live".
