# People & groups: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-17-dg-people-groups.md`. Spec:
> `docs/en/specs/2026-10-17-dg-people-groups-design.md` (decisions P3 and P15). Branch `feat/dg-people-groups`,
> worktree `egret-dgpg`.

## Measurements (Task 1)

The question: do group and role grants reach Trino and StarRocks the way the People pages will say? J-F3 (#685)
shipped group expansion with no live run. The answer decides whether P3's "applied on engines" and "egret only — not
applied on engines" marks are true.

### 1.0 Predictions (committed before anything was observed)

Written from the code at `27c2d157` (main `0741a48f` plus the spec and plan commits, which change no code), before
any container of this run was started. A wrong prediction is a finding. Nothing in this subsection is edited after
observing.

**Setup the predictions are about.**
- The stack of `docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.2 and §2.2:
  - an egret image built from `27c2d157`;
  - throwaway DB `egret_dgpg_live`;
  - own containers `vg-*` on network `vg-net`;
  - Trino 483 with JWT and TLS (`vgtr`, profile `vgtr`, tag profile `vgtr-tag`, poll 5000 ms);
  - StarRocks 4.1.6 allin1 with `access_control = ranger` (`vgsr`, poll 5000 ms);
  - one egret Iceberg catalog `ice`, attached to both, holding `s.t(id, v)` and, for C6, `s.c6(id, secret)`.
- **Users**, all created through `POST /api/control-plane/users` (emails `<name>@vg.local`). None is a member of
  any workspace.
  - `ua`, `ud`, `ue`: given a `users.username` equal to their name before any grant exists (SQL, DEV; no API sets
    one without making the user a workspace member or a query runner).
  - `ub`, `uc`, `uf`: no username.
- **Groups**, through `POST /groups` and `POST /groups/{id}/members`: `g1` = {`ua`, `ub`}, `g2` = {`ue`}.
- **Role** `r1` (platform scope, `POST /access/roles`), held through `POST /users/{id}/roles` by `ud` and `uf`, and
  by group `g2` through `POST /groups/{id}/roles`.
- **Grants** (`POST /perm/grants`, both written after the memberships):
  - G1: allow `read` on `ice.s.t` to `group g1`;
  - G2: allow `read` on `ice.s.t` to `role r1`.
- **The warehouse's own workspace-assignment grant** (`workspace_members` of `default` can read `ice.*`) exists. Its
  only member is the bootstrap admin, so it names `admin` and none of the six users.
- **C6 adds**, after C5:
  - G3: allow `read` on `ice.s.c6` to `user uc` (and the `uc` username, so the grant can name `uc`, set by SQL,
    DEV);
  - tag `vggroup` on column `ice.s.c6.secret`;
  - the tag's gate `required_groups = [g1]`, `required_roles = []`.
- **Engine queries.**
  - Trino: straight to Trino with a JWT signed by egret's key, `engine_claims`' claim set (#1 live doc DEV-4).
  - StarRocks: as the same-named StarRocks user through `mysql` in `vg-sr`.
  - The statement is `SELECT * FROM ice.s.t` (for C6, `SELECT id, secret FROM ice.s.c6`).
  - A user with no stored username is queried under the name `ensure_username` would allocate, which is the email
    local part (`uc`, `uf`).

**Compiled policy (read from the download each plugin polls).**
- On both `vgtr` and `vgsr`, the policy compiled from G1 names users exactly `[ua, ub]`, and the one from G2 names
  exactly `[ud]`.
- No policy item names a group other than `public`, and none names a role.
- Neither `ue` nor `uf` appears in any `ice.s.t` policy.

**Cells.**

| id | check | Trino prediction | StarRocks prediction | `/perm/explain` prediction (both profiles) |
|---|---|---|---|---|
| C1a | `ua` reads `ice.s.t` | rows | rows | allowed, attributed to G1 |
| C1b | `uc` reads `ice.s.t` | `Access Denied` (`PERMISSION_DENIED`; catalog or table) | `ERROR 5204` | denied, no policy matched |
| C2a | `ub` has a username after G1 is created | `users.username = 'ub'` written by the compile of G1, with the log line `minted a username for a workspace or group member` | — | — |
| C2b | `ub` reads `ice.s.t` | rows | rows | allowed, attributed to G1 |
| C3a | `ud` reads `ice.s.t` | rows | rows | allowed, attributed to G2 |
| C3b | `ue` reads `ice.s.t` (holds `r1` only through `g2`) | `Access Denied` | `ERROR 5204` | denied, no policy matched |
| C4a | `uf` named in the compiled policy | no: G2's policy names `[ud]` only; egret logs `a holder of this role has no users.username yet`; `uf`'s `users.username` stays NULL | same | — |
| C4b | `uf` reads `ice.s.t` | `Access Denied` | `ERROR 5204` | denied, no policy matched |
| C5 | `DELETE /groups/{g1}/members/{ua}`, then `ua` reads every second | the G1 policy becomes `[ub]`; the first refusal comes within 10 s of the DELETE answering (one 5 s poll plus the engine's switch); `ub` still reads | same | `ua` denied right after the DELETE (the stored artifact changes in the same transaction); `ub` still allowed |
| C6a | the gate write | `PUT /tags/{id}/access-policy` answers 200 and reports the clearance as skipped (a group-only gate compiles to nothing on engines) | — | — |
| C6b | `uc` reads `ice.s.c6.secret` | real values | real values | allowed (no clearance deny in either artifact) |
| C7 | explain agrees with the engine in every cell above | yes, every cell | yes, every cell | — |

**Not confident:**
- Trino's denial text may name the catalog (`Cannot access catalog ice`) or the table/columns. Either is a denial.
- Whether `uf`'s missing username is logged at compile on each recompile, or only once.

**Stop rule (brief).** If C1, C2, C3a, C5 or C7 fails, the run stops and reports BLOCKED: that is a compiler defect
outside #2. C3b, C4 and C6 failing would be findings (fail-closed narrowings that turned out wider or narrower than
the code says), not a stop.

### 1.1 Result (read this first)

**Group and role grants reach both engines as the code says. Nothing is BLOCKED.** Measured 2026-10-17 between
11:34 and 11:37 UTC on an image built from `27c2d157` (the predictions commit `817d5b25` changes only this file).

All 14 predicted cells matched, on Trino 483 and on StarRocks 4.1.6.
- **A group grant reaches its members.** G1 (`group g1`) compiled to users `[ua, ub]` on both profiles. Both read
  `ice.s.t` on both engines; `uc`, outside the group, was refused on both.
- **A member with no username gets one at compile** (C2). Creating G1 minted `ub` (log line at 11:34:32.81). `ub`
  then read on both engines.
- **A role grant reaches its direct holders only** (C3, C4).
  - G2 (`role r1`) compiled to `[ud]`.
  - `ue`, who holds `r1` only through group `g2`, is not named and is refused on both engines.
  - `uf`, who holds `r1` directly but has no username, is not named either. Every recompile logs the warning
    (10 times, once per profile per compile). `uf`'s `users.username` stayed NULL to the end.
- **Removing a member reaches the engines within one poll** (C5). `DELETE /groups/g1/members/ua` answered 204 in
  0.12 s. `/perm/explain` said denied 0.07 s later. Trino refused `ua` 2.3 s later, and StarRocks 3.8 s later.
  `ub` kept reading.
- **A gate that names only groups is not enforced on engines** (C6). `PUT /tags/{id}/access-policy` answered 200
  and returned the clearance in `skipped`, with the D7 reason. The tag itself did reach Trino's tag download. `uc`
  read the tagged column `secret` on both engines.
- **`/perm/explain` agreed with the engine in every cell** (C7): 15 explain calls, two profiles each.

**What this decides for the People pages (P3, P15).** These statements are true as measured, and the UI may make
them:
- a group's direct members are "applied on engines";
- a member with no username is applied once a grant names the group (the username is allocated at compile);
- a role's direct holders with a username are "applied on engines";
- a holder with no username is "not named on engines";
- a role held through a group is "egret only — not applied on engines";
- a gate naming only groups is "not enforced on engines".

**Not measured:**
- a workspace held by a group (the same `principal_type = 'user'` narrowing as a group-held role; reasoned, not
  run);
- an expired membership;
- a group in a grant's `exclude` or in a rule exemption;
- a role or group with more than two members;
- egret's own object-access check (Cedar) for the group-only gate;
- the egret query API path for the real queries (DEV-4).

### 1.2 Stack and deviations

- **Image.** `egret-backend:vg-dgpg`, built from `27c2d157`. The build was a warm release build and took 21 min.
  Free disk was 99 GiB at its lowest, against a floor of 30 GiB.
- **DB and containers.** DB `egret_dgpg_live`: `migrate`, `init`, `serve`. Log line: `kubernetes unavailable;
  engine provisioning disabled`. Containers on `vg-net`:
  - `vg-egret` (`catalog.public_url = http://vg-egret:8096`);
  - `vg-rustfs` (`rustfs/rustfs:1.0.1`, bucket `warehouse`);
  - `vg-trino-coord` (Trino 483 `db58cc93…`, host `vgtr-coordinator-6d5f7c9b8-ab1cd`). It ran the JWT/TLS rendering
    of the #1 live doc §1.2, with a fresh self-made leaf cert;
  - `vg-sr` (allin1 `5cbf09c4…`, `4.1.6-6862092`, `access_control = ranger`).

  There was no `vg-pg` (no external catalog was needed).
- **Catalog.** Warehouse and registry catalog `ice` were created through the API (`POST /storage`,
  `POST /warehouses`). StarRocks attached it through egret's own `POST /engines/{id}/catalogs/{cid}` (`applied`).
  The tables `ice.s.t(id, v)` and `ice.s.c6(id, secret)` were written as StarRocks `root` before Ranger was
  switched on. Trino's `ice` catalog file has `render_trino`'s Egret shape, with a catalog client minted through
  `POST /warehouses/{id}/clients`.
- **Profiles.** `vgtr-tag` (type `tag`), `vgtr` (`tagService = vgtr-tag`) and `vgsr`, all created through the API.
- **Fixture through the API.**
  - Users: `POST /users`.
  - Groups: `POST /groups` and `POST /groups/{id}/members`.
  - Role `r1`: `POST /access/roles` (scope `platform`). Held through `POST /users/{id}/roles` (`ud`, `uf`) and
    `POST /groups/{id}/roles` (`g2`).
  - Grants: `POST /perm/grants`.
  - Tag: `POST /tags`, `PUT /warehouses/{id}/resource-tags`, `PUT /tags/{id}/access-policy`.
  - The membership removal: `DELETE /groups/{id}/members/{user_id}`.

  No user is a member of any workspace. The warehouse's workspace-assignment grant names only `admin`.
- **DEV-1 (SQL): engine rows.** `vgtr` (JWT spec, profile `vgtr`) and `vgsr` (profile `vgsr`) were inserted as
  `provisioned`, `running`. `POST /engines` needs Kubernetes.
- **DEV-2 (SQL): the Trino attachment.** `vgtr` → `ice` was inserted as `applied`, because the Trino attach needs
  Kubernetes. The per-user baseline came from egret's startup pass after a restart (`seeded=1 failed=0`).
  `POST /perm/compile/2` (an API call) then compiled the workspace-assignment grant into `vgtr`, because neither
  SQL insert recompiles. This happened before any fixture existed.
- **DEV-3 (SQL): usernames.** `users.username` was set to the email local part (the value `ensure_username`
  allocates) for `ua`, `ud` and `ue` before any grant existed, and for `uc` before G3 (C6). No API allocates one
  without making the user a workspace member or a query runner. That missing route is Q1's
  `POST /perm/people/users/{id}/username`. `ub` and `uf` were never touched.
- **DEV-4 (identity): the real queries.**
  - Trino queries went straight to Trino, with JWTs signed by egret's active key from `catalog.oidc_signing_keys`
    and `engine_claims`'s claim set.
  - StarRocks queries ran as the same-named StarRocks users (created as `root`, no password) through `mysql`
    inside `vg-sr`.
  - None went through egret's query API.
- **DEV-5 (config).** The Ranger poll was 5000 ms on both engines (rendered: 30000).
- No privilege of `egret-lineage` or of StarRocks `root` was changed.

### 1.3 Every cell

**Compiled policy** (`policy.authz_policies`, and the plugins' own caches: `trino_vgtr.json` at `policyVersion 8`,
`starrocks_vgsr.json` at 5). Every item has `groups: []` and `roles: []`.

| policy | after G1 + G2 (vgtr v8, vgsr v5) | after C5 (v9, v6) |
|---|---|---|
| G1 `perm-grant-b2ea82ca…` (`group g1`, read `ice.s.t`) | users `[ua, ub]` | `[ub]` |
| G2 `perm-grant-85d4ec19…` (`role r1`, read `ice.s.t`) | `[ud]` | `[ud]` |
| workspace assignment `perm-grant-bcb31e6f…` (`ice.*`) | `[admin]` | `[admin]` |

Trino's cache also holds the three `{USER}` baseline policies and the lineage transform. Neither `ue` nor `uf`
appears in any policy.

**Queries** (`SELECT * FROM ice.s.t ORDER BY id`) and `/perm/explain` (`read`, target `ice.s.t`). "denied
(`no_policy_matched`)" is explain's `decision.kind = denied` with `reason = no_policy_matched` on both profiles.

| id | user | Trino | StarRocks | explain vgtr / vgsr | verdict |
|---|---|---|---|---|---|
| C1a | `ua` | rows `[1,x],[2,y]` (11:35:13) | rows | allowed / allowed, policy and attribution G1 | match |
| C1b | `uc` | `Access Denied: Cannot access catalog ice` (`PERMISSION_DENIED`) | `ERROR 5204 … SELECT privilege(s) on COLUMN v` | denied (`no_policy_matched`) / same | match |
| C2a | `ub` username | `users.username` NULL before G1, `ub` after. Log at 11:34:32.81: `minted a username for a workspace or group member … user_id=9b52e7f3… username=ub` | — | — | match |
| C2b | `ub` | rows | rows | allowed / allowed, G1 | match |
| C3a | `ud` | rows | rows | allowed / allowed, G2 | match |
| C3b | `ue` (`r1` only through `g2`) | `Access Denied: Cannot access catalog ice` | `ERROR 5204 … COLUMN v` | denied (`no_policy_matched`) / same | match |
| C4a | `uf` in the policy | not named (G2 = `[ud]`). Log: `a holder of this role has no users.username yet … user_id=8deceb61… role=r1`, 10 times (each compile, each profile). `users.username` NULL at the end | same | — | match |
| C4b | `uf` | `Access Denied: Cannot access catalog ice` | `ERROR 5204 … COLUMN v` | denied (`no_policy_matched`) / same | match |
| control | `admin` | rows | rows | allowed / allowed, workspace-assignment grant | — |
| C5 | remove `ua` from `g1` | `DELETE` → 204 at 11:35:46 (0.12 s). `ua`: OK at +0.40 s and +1.44 s, **refused at +2.26 s** (`Cannot access catalog ice`). Trino log `Switched policy engine to [9]` at 11:35:47.77 | OK at +0.60, +1.65, +2.46 and +3.16 s, **refused at +3.83 s** (`5204`) | `ua` denied / denied at +0.07 s; `ub` allowed / allowed (G1) at +0.12 s. `ub` still read on both engines afterwards | match |
| C6a | group-only gate | `PUT /tags/{vggroup}/access-policy {"required_roles":[],"required_groups":["<g1 id>"]}` → 200, `skipped: [{source_kind: clearance, reason: "the tag's clearance names only required_groups, and D7 measured … the gate stays enforced on object access (Cedar) only"}]`. No policy in `vgtr`, `vgtr-tag` or `vgsr` names the tag. Trino's `trino_vgtr_tag.json` (tagVersion 2) carries `vggroup` on `ice.s.c6.secret` | — | — | match |
| C6b | `uc` reads `ice.s.c6.secret` (G3 grants `uc` read on `c6`) | rows `[1,s1],[2,s2]` | rows | allowed / allowed, policy and attribution G3. `tags: []` with `tag_policies_in_download: 0`, as the field's doc says | match |
| C7 | explain and engine | 15 explain calls, every profile verdict equal to the engine's answer | | | match |

Battery outputs are in the scratchpad `dgpg/`: `battery-main.txt`, `c5.txt`, `c6.txt`, `compiled-*.txt`,
`*-cache-1.txt` and `egret-mint-log.txt`.

### 1.4 Left behind

- **Removed:**
  - containers `vg-egret`, `vg-trino-coord`, `vg-sr`, `vg-rustfs`;
  - network `vg-net`;
  - DB `egret_dgpg_live` (dropped);
  - image `egret-backend:vg-dgpg`.

  `docker ps -a`, `docker network ls` and `docker images` show no `vg-` entry.
- **Secrets deleted from the scratchpad:** env files, the S3 password, the catalog client, the signing key and
  `kid`, the TLS key and keystore, the cookie jar.
- **Kept:**
  - BuildKit cache, 11.5 GB, all reclaimable;
  - the scratchpad `dgpg/`: battery outputs, scripts, Trino/StarRocks configs and the public cert.
- **Untouched:** DB `egret`, every `egret-dev-*` container, `egret-w7*-postgres`, ports 5173, 5000 and 8080.
  Free disk at the end: 102 GiB. No product code changed.
