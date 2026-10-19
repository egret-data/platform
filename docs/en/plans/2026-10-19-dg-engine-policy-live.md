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

## 2. Live browser check (Task 17)

### 2.0 Predictions

Written at `c87bb9c9` (Tasks 1–16 plus a test-only fix) before any container of this run was started; nothing in this
subsection is edited after observing. A wrong prediction is a finding.

**Setup the predictions are about.** Task 1's stack (§1.2) rebuilt from `c87bb9c9` (image `egret-backend:dgep-2`,
DB `egret_dgep_live`, `dgep-*` on `dgep-net`): Trino 483 `dgtr` (+ tag service `dgtr-tag`, delegating), StarRocks
4.1.6 `dgsr`, both `provisioned`/`running` by SQL, catalog `ice` attached to both; one external Trino engine `dgext`
registered through the API with its own profile `dgext`; one unbound manual Trino profile `dgloose` created through
the index. Users through the API: `admin` (platform admin), `ua`, `ub`, `uc` (usernames as their names), `uroot`
(username `root`), a steward `vs` (role: `policy:read`, `policy:update`, no `engine:read`) and a reader `vr` (role:
`policy:read` only). Seeds as Task 1 (a data grant and a mask rule on `ice.s.t`, hand-written access/masking/row-filter
on `dgtr` written while the profile was briefly `manual`, DEV-n). Vite dev server from this worktree on 5397,
proxying `/api`, `/catalog`, `/oauth` to `dgep-egret`. Playwright MCP; logins ≥ 60 s apart.

| id | check | prediction |
|---|---|---|
| L0a | index as admin | `h1` "Engine & policy"; Engines table lists `dgtr` (trino, provisioned, profile `dgtr`), `dgsr` (starrocks, provisioned, `dgsr`), `dgext` (trino, external, `dgext`, plugin "Not judged: external"); hand-written counts equal `ProfileDto.policy_counts.operator` of each profile plus its tag service |
| L0b | "Profiles without an engine" | lists `dgloose` only (not `dgtr-tag`, which `dgtr` names as `tag_service`); Delete asks for the typed name |
| L0c | index as `vr` (no `engine:read`) | Engines table built from `ProfileDto.engines` (bound engines only, every one of ours is bound) and no plugin column |
| L1a | grant `trino.impersonate_user` ua → ub through the dialog | toast "Saved and compiled into dgtr."; row under the capability with `ub` linked to its People page; after one poll (≤ 15 s) `tq.py ua ub "SELECT current_user"` → `OK [["ub"]]`, the same as `uc` refused `PERMISSION_DENIED` |
| L1b | the same with `starrocks.impersonate_user` on `dgsr` | toast compiled into `dgsr`; `EXECUTE AS ub WITH NO REVERT; SELECT current_user()` as `ua` → `'ub'@'%'`; as `uc` ERROR 5204 |
| L2 | target `root` (user `uroot`), then target `admin` (platform admin) | each refused, the dialog stays open with every input kept and shows the server's reason ("… is reserved …" / "… is a platform admin, whom egret seeds as an engine admin"); no row added |
| L3a | Check `ua`, `trino.impersonate_user`, target `ub` on `dgtr` | "Cannot be decided ahead of time", a cause naming `{USER}` and the F11 §5.4 impersonation sentence |
| L3b | Check on `dgsr` | `ua` → "Allowed", deciding policy `perm-engine-<id>`, source "egret engine grant" linked to the row; `uc` → "Denied", no deciding policy shown |
| L4 | plugin state | Plugin tab on `dgtr`/`dgsr` says `up_to_date` per kind while engines poll; the Overview raises no `plugin_*` for them; external `dgext` shows its `not_judged` sentence. (The 5-min `quiet` stop/start leg: predicted `quiet` on the tab and `plugin_quiet` on the Overview for `dgsr`, then `pending` → `up_to_date` and the finding gone.) |
| L5 | Policies tab on `dgtr` | count line "N egret-compiled · N egret-seeded · N hand-written" matching the rows; compiled rows locked with "Open source" landing on the grant / rule / engine-permissions row; seeded rows read-only; hand-written rows have enable/disable/delete and Edit, Edit opens `/governance/engines/<dgtr id>/advanced/policies/<kind>/<id>`, Save returns to `?tab=advanced` |
| L5b | a compiler-owned policy opened by URL in the editor | read-only as before Task 14 (the editor's existing compiler-owned check) |
| L5c | Overlaps tab | count badge equals rows; empty sentence when none |
| L5d | Migrate tab | shown on `dgtr` (has hand-written rows), absent on a profile without; dry run lists the planned change without writing; apply writes and the row becomes compiled; Undo restores the hand-written row |
| L5e | Advanced on `dgtr` (generated) | the fixed warning; the "egret compiles this profile …" sentence instead of Create/Seed (premise 13) |
| L5f | Advanced on `dgloose` (manual, unbound) | Create policy (Access/Masking/Row filter, no By tag) and Seed default policies; creating an access policy saves and returns to the tab, editing it saves; Seed reports the created count |
| L6 | every old URL of §3 typed in the address bar | `/governance/policies` → `/governance/engines`; `/governance/policies/dgtr` → `/governance/engines/<dgtr id>?tab=policies`; `?tab=masking` → `…&type=masking`; `/governance/policies/dgloose` → `/governance/engines/profiles/dgloose?tab=policies`; `…/dgtr/migrate` → `?tab=migrate`; `…/dgtr/access/new?scope=tag` → `…/advanced/policies/access/new?scope=tag`; `…/dgtr/access/<id>` → the editor; `/control-plane/policies/dgtr` → same as `/governance/policies/dgtr`; unknown → index with "No policy profile named X"; Back never returns to the old URL |
| L6b | Overview links | profile refs open the resolver (→ engine page), `plugin_*` → `?tab=plugin`, `policy_warnings` → `?tab=policies`, `rule_overlaps` → `?tab=overlaps`; engine refs stay on Control Plane |
| L7a | steward `vs` (no `engine:read`) | Plugin tab shows "Plugin state needs the engine:read permission." and no request to `/plugin`; no plugin column on the index; write controls of `policy:update` (enable/disable, Edit/Delete grant) present |
| L7b | reader `vr` (`policy:read` only) | no Grant, Edit, Delete, enable/disable, New profile, Create policy, Seed, Migrate apply anywhere; Check is present |
| L8 | keyboard, Escape, widths, themes | every tab and both dialogs reachable by Tab/arrow keys with a visible focus ring; Escape closes each dialog and returns focus to its trigger; at 390 px no horizontal page scroll (tables scroll inside their own container); 1280 px light and dark render with no unreadable text |
| L9 | People: user `ua` Permissions tab | "Engine permissions" section at the end listing both impersonation grants with engine links and capability labels; `ub`'s page lists none ("No engine permission reaches this user.") |

### 2.1 Result (read this first)

**Every cell ran; four defects found and fixed red-first; apply/undo of Migrate not exercised.** Impersonation granted
through the dialog works on both real engines (L1), the refusals read the server's reason with the dialog kept open
(L2), Check answers Undecidable / Allowed / Denied as predicted (L3), the Plugin tab and the Overview agree through a
5-minute StarRocks stop and recovery (L4), every old URL lands where spec §3 says with Back skipping it (L6), the
steward and the reader see what their permissions allow (L7), and the pages hold at 390 px (L8). Defects:

| # | defect (cell) | fix |
|---|---|---|
| D1 | the index showed every healthy StarRocks engine as "Not reported": its tags kind is always `not_reported` because StarRocks never downloads tags, and the worst-kind word counted it (L0a) | `23dd7f90` — `pluginWord` ignores a StarRocks engine's tags kind |
| D2 | a created engine grant left the grant dialog open with no toast and the list unrefreshed: create answers **201** and the success handler returned on any status but 200; the test harness answered 200 (L1a) | `aab8ae6a` — any 2xx is success; harness `created()` answers 201 |
| D3 | closing the Grant or Check dialog dropped focus to `<body>`: the dialogs open from state, so Radix had no trigger to return to (L8) | `24624438` — the panel remembers the opener and focuses it on close |
| D4 | after an editor save or delete, the Advanced/Policies tab it returned to showed the old row until a reload: the editor invalidated only the legacy `["control-plane","policies"]` key, the tabs read the generated key (L5f) | `04d46f5b` — the editor invalidates both keys |

Wrong predictions that are not defects: L5b (a compiler-owned policy does **not** open read-only; it opens editable
and Save shows the server's 409 as "This policy is owned by the permission model. … edit or delete that grant/rule
instead" — exactly main's behaviour; the plan's "read-only exactly as today" was wrong about today); L5d (both
previews were refused, "did not prove", so apply/undo could not be reached, see 2.3).

### 2.2 Stack and deviations

- Image `egret-backend:dgep-2` from a `git archive` of `c87bb9c9` (scratchpad `dgep2/dgep-build.sh`; 23 min; lowest
  free disk 148 GiB). DB `egret_dgep_live` in `dgep-pg`; `dgep-egret`, `dgep-rustfs`, `dgep-trino` (483, JWT + TLS,
  fresh self-signed leaf), `dgep-sr` (4.1.6 allin1, `access_control = ranger`) on `dgep-net`; poll 5 s. Vite from this
  worktree on 127.0.0.1:5397 with a scratch config proxying `/api`, `/catalog`, `/oauth` to 18497. Fixes D1–D4 were
  checked live through Vite HMR (frontend only; the image needs no rebuild).
- Profiles through the API: `dgtr-tag` (1), `dgtr` (2, `tagService`), `dgsr` (3), `dgext-tag` (5), `dgext` (6,
  `tagService` — registering an external Trino engine on a profile without one is refused 400), `dgloose` (via the
  index dialog). External engine `dgext` through `POST /control-plane/engines/external`.
- **DEV-1/DEV-2 (SQL)** as §1.2: engine rows `dgtr`/`dgsr` `running`, attachments to `ice`.
- **DEV-3: Trino trusts a test-only key.** Copying egret's machine-IdP private key out of the throwaway DB (Task 1's
  recipe) was refused by the session's permission classifier, so Trino's `jwt.key-file` points at the public half of a
  fresh RSA key made for this run, and `tq.py` signs with it. JWT authentication only names the principal; the
  impersonation decision is Ranger's, so L1's proof is unaffected.
- **DEV-4 (SQL): `origin` flipped to `manual`** around three hand-written creates on the generated `dgtr`
  (`hw-mask-country` attempt, `hw-read-ub`, `hw-read-ua`), as Task 1. `hw-mask-country` itself was then **inserted by
  SQL** (both the API and the compile refuse an overlapping pair), to give the Overlaps tab a row.
- Users `ua`, `ub`, `uc`, `root` (egret allocated the username `root` without objection), `vs` (role
  `policy:read`+`policy:update`), `vr` (`policy:read`); StarRocks users `ua`, `ub`, `uc` by `root`.

### 2.3 Every cell

| id | prediction | observed | verdict |
|---|---|---|---|
| L0a | index lists 3 engines, origins, counts | `dgext` Trino External "Not judged: external" 0/0; `dgsr` StarRocks Provisioned **"Not reported"**; `dgtr` Up to date 0/6; summary "3 engines · 0 profiles without an engine · 7 hand-written" | D1, then "Up to date" after `23dd7f90` |
| L0b | unbound section lists only `dgloose` | after New profile: `dgloose` Trino Manual 0, Delete offered; `dgtr-tag`/`dgext-tag` absent | as predicted |
| L0c | no plugin column without `engine:read` | `vs` and `vr`: columns Engine/Type/Origin/Profile/Engine grants/Hand-written; no plugin request | as predicted |
| L1a | grant ua→ub on `dgtr`, Trino proof | three POSTs: 409 (root), 409 (admin), **201 — dialog stayed open, no toast** (D2). Then `tq.py ua ub` → `OK [["ub"]]`; `uc` → "Principal uc cannot become user ub" | D2; engine proof as predicted |
| L1b | the same on `dgsr` | (after D2's fix) toast "Saved and compiled into dgsr. The engine applies it at its next plugin download."; row `ub` linked to its People page; `ua` → `'ub'@'%'`, `uc` → ERROR 5204 | as predicted |
| L2 | root / platform admin refused, dialog open | "Not saved: \"root\" is a principal egret runs as on the engine; it cannot be impersonated"; "Not saved: the impersonation target \"admin\" is a platform admin, whom egret seeds as an engine admin: …"; inputs kept | as predicted |
| L3a | Trino Undecidable with `{USER}` | "Cannot be decided ahead of time", the impersonation sentence, cause "egret-user-impersonate-self … {USER}" | as predicted |
| L3b | StarRocks Allowed / Denied | `ua` "Allowed", "Decided by perm-engine-b4696f81… (egret engine grant)" linked to `?grant=`; `uc` "Denied", "No policy grants it." | as predicted |
| L4 | quiet then recovery, Overview agrees | `docker stop dgep-sr` 08:08:40; at +6 min the route said `quiet` "No plugin has polled profile dgsr for 6 minutes." and `/attention` raised `plugin_quiet` with the same sentence; start + a compile: `pending` (cur 7, rep 6) for ~10 s, then `up_to_date`; `plugin_quiet` gone | as predicted |
| L5 | Policies tab | "13 policies: 4 egret-compiled · 3 egret-seeded · 6 hand-written"; Open source → grant page, rule page, `?tab=engine-permissions&grant=` (row `aria-current`, highlighted); hand-written Edit → editor, Save → `?tab=advanced`; switch re-enable toasts "Policy enabled." | as predicted |
| L5b | compiler-owned opens read-only | opens editable; Save → the 409 sentence above (unchanged from main) | prediction wrong, not a defect |
| L5c | Overlaps | badge 1; the pair with links to the editor and the rule page, sentence, "How to fix" | as predicted |
| L5d | Migrate dry run / apply / undo | `hw-read-ub` and `hw-read-ua` each "Migratable"; both previews "did not prove this migration" (275 / 271 probe disagreements across 2 profiles), "Nothing was written", Migrate stays disabled. **Apply and undo not exercised** | partly measured |
| L5e | generated `dgtr` Advanced | fixed warning + "egret compiles this profile, so new hand-written policies are refused. …"; no Create/Seed | as predicted |
| L5f | manual `dgloose` Advanced | menu Access/Masking/Row filter (no By tag); Seed → "Added 13 default policies." (all `infra`, so the hand-written list stays empty); create `loose-read` → back to the tab, listed; edit (disable) → back to the tab **still "Enabled"** until reload (D4) | D4 |
| L6 | old URLs | all 14 typed URLs landed as spec §3 (incl. `?tab=masking`→`&type=masking`, `?tab=tags`→`&type=tag`, `/control-plane/policies/*`, unknown → index with "No policy profile named …"); Back returned to the page before each time. `/governance/policies/dgtr-tag` lands on the tag service's own profile page | as predicted |
| L6b | Overview links | `policy_warnings` sample `dgtr` → `/governance/engines/profiles/dgtr?tab=policies` → replaced by the engine page `?tab=policies`; `rule_overlaps` sample → rule page; `plugin_quiet` ref is the profile (route checked by API, not clicked) | as predicted |
| L7a | steward | Plugin tab "Plugin state needs the engine:read permission." (header too) and no `/plugin` request; Grant, Edit, Delete, enable switches present; no policy Delete (no `policy:delete`); "Download Trino config" still offered (EP18, `policy:read`) | as predicted |
| L7b | reader | no Grant/Edit/Delete/switch/New profile/Create/Seed anywhere; Check present; editor "read-only for this account …", Save disabled | as predicted |
| L8 | keyboard, Escape, widths, themes | Tab order reaches the index rows, Enter opens, arrows move tabs and the URL; focus ring `:focus-visible`; Escape closes suggestions then the dialog, but **focus fell to `<body>`** (D3, fixed and re-checked: focus back on Check); 390 px: `scrollWidth = clientWidth` on the index, every tab, the profile page, the editor and People; dialog fits with margins; 1280 dark/light read cleanly (screenshots checked with `sips`: 390×844 and 1280×900, 52–173 kB) | D3 |
| L9 | People | `ua`: "Engine permissions" lists `dgsr` and `dgtr` "Impersonate a user", `ub`, "Directly · applied on engines"; `ub`: "No engine permission reaches this user." | as predicted |

### 2.4 Defects

D1–D4 above, each with a failing test first (`EnginesIndex.test.tsx`, `EngineGrantDialog.test.tsx` + harness,
`CheckCapabilityDialog.test.tsx`, `PolicyEditorPage.test.tsx`). Noted, not fixed (outside #4a or a wording choice):
a managed row with no mapped capability shows an empty "Capability" label (`queryid=*` baseline); the editor offers
"By tag" on a profile without a tag service (unchanged editor); `PUT /policies/{id}/enabled` leaves
`policy_jsonb.isEnabled` at its old value (server, unchanged); egret allocates the engine username `root` to a user
named root (the grant path refuses it as a target, as L2 shows).

### 2.5 Left behind

Containers `dgep-egret`, `dgep-trino`, `dgep-sr`, `dgep-rustfs`, network `dgep-net` and DB `egret_dgep_live` removed
after the run; image `egret-backend:dgep-2` removed. Kept: `dgep-pg`, `dgep-valkey`; scratchpad scripts, configs and
screenshots.
