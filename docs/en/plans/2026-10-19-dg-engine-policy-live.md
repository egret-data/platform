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

### 1.1 Result (read this first)

**Not blocked.** Neither stop rule fired: every compiler row parses as premise 5 says and its uuid exists (M1), and
the grant dialog's catalog list equals the server's (M3). Impersonation was exercised live on both engines (M6), so
Task 17 can prove it on each. Three findings change what the frontend tasks must do:

1. **A compiled profile refuses NEW hand-written policies (new premise 13).** The first compile flips a profile's
   `origin` to `generated` (tag services too), and from then on `POST /policies` and `POST
   /policies/services/{s}/seed-defaults` answer 409 "profile N is compiled by the SP4a permission model — edit its
   grants and rules instead of writing a policy directly …" (`egret-policy/src/store.rs:543-561`, `:701`). Existing
   hand-written rows stay editable: `PUT /policies/{id}/enabled` on an `operator` row of a generated profile answered
   204 (`refuse_if_compiler_owned` refuses by name only, `store.rs:619-640`). So the Advanced tab's **Create policy**
   and **Seed default policies** can succeed only on a `manual` profile (an unbound or never-compiled one); every bound
   engine reaching a catalog with a grant, rule or engine grant is `generated`. No user decision changes (EP2/EP9 keep
   today's permissions and today's server behaviour), but Task 14 must gate both controls on
   `ProfileDto.origin === "manual"` and say why otherwise; recorded in the spec as premise 13.
2. **`ProfileDto` is snake_case.** `GET /policies/services` returns `policy_version`, `policy_count`, `tag_service`,
   `origin` (no `rename_all`, `admin/mod.rs:437-461`); the plan's "camelCase as the DTO already is" (Task 4) is wrong.
   Q1's additive fields must follow the DTO's own casing (`policy_counts {compiler, infra, operator}`, and
   `origin`/`type` on `ProfileEngine`) or the generated type mixes two casings in one object.
3. **A Denied explain names no policy with an empty string**, not null: `policyName: ""`, `decision.policy_name: ""`,
   `reason: "no_policy_matched"`. The Check dialog must treat `""` as "no deciding policy".

Smaller: Trino's refusal reads "Access Denied: Principal ua cannot become user ub" (predicted "User ua cannot
impersonate user ub"); a catalog-level Trino capability is answered `undecidable` / `tags_unresolved` with
`tagResolution: declined` once the tag service holds a policy, as the spec says.

### 1.2 Stack and deviations

- **Image** `egret-backend:dgep-1`, built from a `git archive` of `7875cb95` (the predictions commit; no code change
  since `3731f0ce`), not from the worktree, which other agents were editing. Warm release build, 25 min (parallel
  builds). Free disk at its lowest: 153 GiB.
- **DB and containers.** DB `egret_dgep_live` in `dgep-pg` (attached to `dgep-net` for the run): `migrate`, `init`,
  `serve`; log `kubernetes unavailable; engine provisioning disabled`. On `dgep-net`: `dgep-egret` (host
  127.0.0.1:18497), `dgep-rustfs` (`rustfs/rustfs:1.0.1`), `dgep-trino` (Trino `db58cc93…` = 483, host
  `dgtr-coordinator-6d5f7c9b8-ab1cd`, JWT + TLS with a fresh self-signed leaf, 127.0.0.1:18943), `dgep-sr` (allin1
  `5cbf09c4…` = 4.1.6-6862092, `access_control = ranger`, 127.0.0.1:19497). Ranger poll 5000 ms on both.
- **Profiles through the API:** `dgtr-tag` (1), `dgtr` (2, `tagService = dgtr-tag`), `dgsr` (3).
- **DEV-1 (SQL): engine rows** `dgtr` (spec pins Trino 483 by digest, `delegationMode = jwt`, `auth.jwt = {}`,
  `coordinator_cert` = the leaf) → 2 and `dgsr` (spec pins `starrocks/fe-ubuntu:4.1.6@sha256:d07a…`) → 3, both
  `provisioned`, `running`. A restart then seeded the per-user baseline (`seeded=1 failed=0`).
- **DEV-2 (SQL): attachments** `dgtr` and `dgsr` → `ice` (`applied`); `srcat` (tpch, API) → `dgsr` only, for M3.
  Catalog `ice` itself came from `POST /warehouses`; `lonely` (tpch, API) is attached to nothing.
- **DEV-3: the warehouse role's grant.** `POST /roles/{id}/grants` with privilege `CATALOG_MANAGE_CONTENT` was
  accepted (201) but the catalog RBAC knows only `write`/`manage` for writes; the row was set to `manage` by SQL.
  (`"privilege": "manage"` through the API is the clean way; the stack script says so.)
- **DEV-4 (SQL): `origin` flipped to `manual` around the hand-written creates** (finding 1): the access, masking and
  row-filter policies on `dgtr`, the by-tag policy in `dgtr-tag` and an access policy on `dgsr` were written through
  `POST /policies` with the profiles briefly `manual`, then set back to `generated`. Two hand-written access policies
  giving `admin` DDL on `ice` were created before the first compile, to create `ice.s.t` through Trino.
- **DEV-5 (seeding).** The by-tag mask on tag `PII` was refused 409 `rule_overlap` with the compiled tag rule
  `dgep-pii-mask`; it was written on a second tag `INTERNAL` instead. `seed-defaults` on `dgtr` answered
  `{"created":0}` (the profile already had policies), so no `all - …` set exists; the `infra` rows are the delegator
  baseline.
- **Identity.** Trino was queried directly with JWTs signed by egret's machine-IdP key (from the throwaway DB),
  claims as `engine_claims`; StarRocks as same-named users created by `root` without a password.
- **Seeds:** data grant `bce3aa23` (allow read `ice.s.t` to `ua`; the warehouse also produced workspace-assignment
  grant `aa74e181`), path mask rule `1c1e192b` (`ice.s.t.country`), tag mask rule `c0eddc87` (`pii`), tag `pii`
  (`f7acd352`) on `ice.s.t.email` with clearance role `dgep_cleared` (user `ur`), eight engine grants (every
  capability; grantee `ua`, impersonation target `ub`). Every compile reported `skipped: []`.

### 1.3 Every cell

| id | prediction | observed | verdict |
|---|---|---|---|
| M1a | every compiler row parses | 20 rows: `perm-grant-<uuid>` ×4, `perm-rule-<uuid>` ×3 (dgtr, dgsr, dgtr-tag) and `perm-rule-<uuid>-ice.s.t.email` (dgsr), `perm-clearance-<tag>` (dgtr-tag), `perm-clearance-<tag>-ice.s.t.email` and `perm-clearance-<tag>-mask-ice.s.t.email` (dgsr), `perm-clearance-mv` (dgsr), `perm-engine-<uuid>` ×8 | as predicted |
| M1b | uuids exist | grant 4/4, rule 4/4, clearance (tags) 3/3, engine 8/8 | as predicted |
| M1c | clearance location | Trino's in `dgtr-tag`; StarRocks' in `dgsr`, one access deny and one clearance mask per tagged column | as predicted |
| M1d | no mismatch | 0 prefixed rows not `compiler`; 0 `compiler` rows without a prefix | as predicted |
| M2 | valueKinds | Trino {catalog, identifier, username}; StarRocks {fixed, function_signature, username}; `fixed` only `starrocks.manage_blacklist` (`system`) | as predicted |
| M3 | lists equal | governance catalogs: `ice` [dgsr, dgtr], `srcat` [dgsr], `lonely` []; reached by `dgtr` = {ice}. 400s: "engine dgtr does not reach catalog \"lonely\"; it reaches: ice" and the same for `srcat` | as predicted (equal) |
| M4a | managed_by on dgtr | `compiler`, `infra`, `operator` | as predicted |
| M4b | policy_type values | only `access`, `masking`, `rowfilter` in all three lists; tag-service rows typed by kind | as predicted |
| M4c | compiled clearances | `perm-clearance-<tag>` in `?service=dgtr-tag`; `…-ice.s.t.email` (access) and `…-mask-ice.s.t.email` (masking) in `?service=dgsr` | as predicted |
| M5a | engines[] fields | `{id, name, status}`; keys snake_case | as predicted (finding 2) |
| M5b | tag service row | `type: "tag"`, `engines: []`, `tag_service: null`, `origin: "generated"` | as predicted |
| M5c | engines as policy:read-only | `uc` (role `policy:read` only): `GET /control-plane/engines` 403; `GET /policies/services` 200 | as predicted |
| M6a | Trino refused | `PERMISSION_DENIED` "Access Denied: Principal ua cannot become user ub" (and for `uc`) | wording wrong, outcome right |
| M6b | Trino after grant | 15 s after the grant: `ua` as `ub` → `OK [["ub"]]`; `uc` as `ub` still refused | as predicted |
| M6c | StarRocks refused | `ERROR 5204 (42000): Access denied; you need (at least one of) the IMPERSONATE privilege(s) on USER ub for this operation.` | as predicted |
| M6d | StarRocks after grant | `ua`: `'ub'@'%'`; `uc`: still ERROR 5204 | as predicted |
| M7a | Trino Undecidable | `undecidable`, why `policy_construct` of `egret-user-impersonate-self`, `resource_value_expression_marker` `trinouser` = `{USER}`; the same for `uc` | as predicted |
| M7b | StarRocks | `ua`: `allowed`, `policy_name perm-engine-bbf084b6…`, attribution `{kind: engine_grant, engine_grant_id}`; `uc`: `denied`, `reason no_policy_matched`, `policyName ""` | as predicted (finding 3) |
| extra | generated profile writes | new policy and seed-defaults 409 on all three profiles; enable toggle of an operator row 204 | finding 1 |

**Recipes for Task 17** (the stack script below rebuilds the stack):
- Trino: `python3 tq.py ua ub "SELECT current_user"` — a JWT (RS256, egret's machine-IdP key, `preferred_username =
  ua`, `aud = engine`) as `Authorization: Bearer`, plus `X-Trino-User: ub`, to `https://localhost:18943/v1/statement`.
- StarRocks: `docker exec dgep-sr mysql -h127.0.0.1 -P9030 -uua -N -e "EXECUTE AS ub WITH NO REVERT; SELECT
  current_user();"` (users created by `root` with no password).
- Stack: scratchpad `dgep1/dgep-stack.sh` (`up <tag>`, `sr`, `trino`, `down`, with the manual seeding order as
  comments), `dgep1/dgep-build.sh <commit> <tag>` (builds from a `git archive`, disk watchdog at 20 GiB), `dgep1/api.sh`,
  `dgep1/tq.py`.

### 1.4 Left behind

Nothing of the run: containers `dgep-egret`, `dgep-trino`, `dgep-sr`, `dgep-rustfs`; network `dgep-net` (`dgep-pg`
disconnected first); DB `egret_dgep_live`; image `egret-backend:dgep-1`; the exported source tree, signing key,
catalog credential, secrets, env file, TLS key/keystore and cookie jars are gone. Kept in the scratchpad: the scripts,
Trino/StarRocks configs and the self-signed certificate (public). `dgep-pg` and `dgep-valkey` stay for the package.
