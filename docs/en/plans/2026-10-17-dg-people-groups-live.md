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
- **A member with no username gets one at compile** (C2). `ub` had no username before G1 and has `ub` after it;
  no SQL touched `ub`. A `minted a username` line was logged at 11:34:32.81, the time G1 was created. `ub`
  then read on both engines.
- **A role grant reaches its direct holders only** (C3, C4).
  - G2 (`role r1`) compiled to `[ud]`.
  - `ue`, who holds `r1` only through group `g2`, is not named and is refused on both engines.
  - `uf`, who holds `r1` directly but has no username, is not named either. The warning was logged 10 times,
    inferred to be once per profile for each of the 5 compiles that referenced `r1`. `uf`'s `users.username` stayed NULL to the end.
- **Removing a member reaches the engines within one poll** (C5). `DELETE /groups/g1/members/ua` answered 204 in
  0.12 s. `/perm/explain` said denied 0.07 s later. Trino refused `ua` 2.3 s later, and StarRocks 3.8 s later.
  `ub` kept reading.
- **A gate that names only groups is not enforced on engines** (C6, one tag on one column, one grant).
  `PUT /tags/{id}/access-policy` answered 200 and returned the clearance in `skipped`, with the D7 reason. `uc` read
  the tagged column `secret` on both engines.
- **`/perm/explain` agreed with the engine in every cell** (C7): 10 recorded explain calls, two profiles each.

**Scope.** Group expansion was measured for `read` (`select`) only, on exactly one group (two members) and one role
(one direct holder with a username, one without, one holder through a group).

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

**Queries** (`SELECT * FROM ice.s.t ORDER BY id`) and `/perm/explain` (`read`, target `ice.s.t`). The battery
recorded each explain verdict as `select=<kind>` and, for an allow, the matched policy and grant id. "denied" is
`select=denied`; the reason was not recorded. "G1"/"G2"/"G3" in the explain column means the attribution's grant id
equals that grant's id.

| id | user | Trino | StarRocks | explain vgtr / vgsr | verdict |
|---|---|---|---|---|---|
| C1a | `ua` | rows `[1,x],[2,y]` (11:35:13) | rows | allowed / allowed, grant id G1 | match |
| C1b | `uc` | `Access Denied: Cannot access catalog ice` (`PERMISSION_DENIED`) | `ERROR 5204 … SELECT privilege(s) on COLUMN v` | denied / denied | match |
| C2a | `ub` username | `users.username` NULL before G1 (observed at 11:34, output not saved), `ub` at the end (`users-final.txt`); no SQL touched `ub` (§1.2 DEV-3). A `minted a username for a workspace or group member` line was logged at 11:34:32.81, when G1 was created; the saved excerpt is cut before `user_id`/`username`, so the line itself does not show `ub`. The two earlier mint lines (11:32:30.26 and 11:32:31.16) fall in the StarRocks attach (11:32:30–31), whose recompile named the workspace-assignment grant's only member, `admin`; `admin` had a username by 11:34. That attribution is by timing, the saved lines are cut too | — | — | match |
| C2b | `ub` | rows | rows | allowed / allowed, G1 | match |
| C3a | `ud` | rows | rows | allowed / allowed, G2 | match |
| C3b | `ue` (`r1` only through `g2`) | `Access Denied: Cannot access catalog ice` | `ERROR 5204 … COLUMN v` | denied / denied | match |
| C4a | `uf` in the policy | not named (G2 = `[ud]`). Log: `a holder of this role has no users.username yet … user_id=8deceb61… role=r1`, 10 times (each compile, each profile). `users.username` NULL at the end | same | — | match |
| C4b | `uf` | `Access Denied: Cannot access catalog ice` | `ERROR 5204 … COLUMN v` | denied / denied | match |
| control | `admin` | rows | rows | allowed / allowed, workspace-assignment grant | — |
| C5 | remove `ua` from `g1` | `DELETE` → 204 at 11:35:46 (0.12 s). `ua`: OK at +0.40 s and +1.44 s, **refused at +2.26 s** (`Cannot access catalog ice`). Trino log `Switched policy engine to [9]` at 11:35:47.77 | OK at +0.60, +1.65, +2.46 and +3.16 s, **refused at +3.83 s** (`5204`) | `ua` denied / denied at +0.07 s; `ub` allowed / allowed (G1) at +0.12 s. `ub` still read on both engines afterwards | match |
| C6a | group-only gate | `PUT /tags/{vggroup}/access-policy {"required_roles":[],"required_groups":["<g1 id>"]}` → 200, `skipped: [{source_kind: clearance, reason: "the tag's clearance names only required_groups, and D7 measured … the gate stays enforced on object access (Cedar) only"}]`. No policy in `vgtr`, `vgtr-tag` or `vgsr` names the tag. Trino's tag cache was seen carrying `vggroup` on `ice.s.c6.secret` (observed, output not saved) | — | — | match |
| C6b | `uc` reads `ice.s.c6.secret` (G3 grants `uc` read on `c6`) | rows `[1,s1],[2,s2]` | rows | allowed / allowed, grant id G3, `tags=[]` | match |
| C7 | explain and engine | 10 recorded explain calls, every profile verdict equal to the engine's answer | | | match |

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

## 2. The live browser check of People & groups (Task 14)

Measurement only: defects are recorded here and fixed in a later fix round.

### 2.0 Predictions (committed before anything was observed)

Written from the code at `cfba7b88` and §1, before any container of this run was started (only the image build had
begun). A wrong prediction is a finding. Nothing in this subsection is edited after observing.

**Setup the predictions are about.** §1.2's stack, rebuilt from `cfba7b88`: image `egret-backend:vg-dgpg14`, DB
`egret_dgpg_live2`, containers `vg-*` on `vg-net` (`vg-egret`, `vg-rustfs`, `vg-trino-coord` Trino 483 JWT/TLS
profile `vgtr` + tag profile `vgtr-tag`, `vg-sr` StarRocks 4.1.6 Ranger profile `vgsr`, both polling every 5 s).
The frontend is this branch's vite dev server on a free port, proxying `/api`, `/catalog`, `/oauth` to `vg-egret`;
the browser is Playwright, logged in once as `admin@egret.local` and once per steward.
- Catalog `ice` (egret Iceberg) attached to both engines: `s.t(id, v)`, `s.c6(id, secret)`, `s.c7(id, secret)`,
  written as StarRocks `root` before Ranger is switched on.
- Users (`POST /users`, `<name>@vg.local`): `ua`, `uc`, `ud`, `ue` get their username through the new
  `POST /perm/people/users/{id}/username` (no SQL, unlike §1's DEV-3); `ub`, `uf`, `un`, `uz` have none.
  Stewards: `vsr` (platform role `vg_reader`: `policy:read` only), `vs` (`vg_steward`: `policy:read`,
  `policy:update`), `vt` (`vg_tagger`: `tag:read` only). A Data API service account `vgsa`
  (`POST /ws/default/data-api/service-accounts`). StarRocks users of the same names exist.
- Groups `g1` = {`ua`, `ub`}, `g2` = {`ue`}. Role `r1` (platform), held directly by `ud`, `uf` and by group `g2`.
- Grants (`POST /perm/grants`, after the memberships):
  - G1 allow read `ice.s.t` → group `g1`; G2 allow read `ice.s.t` → role `r1`;
  - G3 allow read `ice.s.c6` → user `uc`;
  - G4 allow read `ice.s.t` → user `ue`, **exclude role `r1`** (the exclude via a group-held role);
  - G5 allow read `ice.s.c7` → group `g1`, role `r1`;
  - G6 allow read `ice.s.t` → user `ua`, then by SQL (DEV) an include `user uz` (the stale ref for the 409 guard)
    and an exclude `user ghost` (a pre-J-F3 row that fails open).
- Gates: tag `vggroup` on `ice.s.c6.secret` with `required_groups = [g1]`; tag `vgrole` on `ice.s.c7.secret` with
  `required_roles = [r1]`.
- Rules: M1 `MASK` on `ice.s.t.v`, exempt group `g1`; M2 `MASK_NULL` on tag `vgtagrule` (assigned nowhere), exempt
  group `g2`; M3 `MASK_NULL` on tag `vgorphan`, whose tag is then deleted.
- Real queries as in §1.2 DEV-4: Trino directly with a JWT signed by egret's key; StarRocks as the same-named user
  through `mysql` in `vg-sr`.

**Q1. Index (`/governance/people`).**

| id | check | prediction |
|---|---|---|
| Q1a | Users tab | `admin`, `ua`…`uf`, `un`, `uz`, `vs`, `vsr`, `vt` (no `vgsa`). Username column: `admin`, `ua`, `ub` (minted by G1's compile), `uc`, `ud`, `ue`; the rest "No username yet". |
| Q1b | Show service accounts | `vgsa` appears with a "Service account" badge and an empty Email cell; `?serviceAccounts=1` in the URL |
| Q1c | Groups tab | `g1` "2 members", `g2` "1 member", source Local; the note "A member with no username gets one when a grant naming the group is compiled." Not confident whether `public` is listed |
| Q1d | Roles tab | `r1` "2 holders (1 not on engines: no username)" (`ud`, `uf`; `g2`'s `ue` not counted), scope platform; the note naming that a group holding a role is not counted |
| Q1e | Unmatched names | `user ghost` first, "Widens access", Allow, exclude; then `user uz`, "Narrows access", Allow, include. Each links to `/governance/grants/<G6>` |
| Q1f | search | `?q=u` filters users; a miss shows "No user matches “…”." with Clear search |

**Q2. Principal pages: Overview** (marks exactly as §1.1 measured).

| id | page | prediction |
|---|---|---|
| Q2a | `ua` | Groups: `g1`, Applied on engines "Yes". No roles, no workspaces |
| Q2b | `ub` | Groups `g1` "Yes"; username `ub` in the header |
| Q2c | `ud` | Roles: `r1`, Platform, Direct, Never, "Yes" |
| Q2d | `ue` | Groups `g2` "Yes"; Roles: `r1`, Platform, Through "Group g2", "egret only — not applied on engines: held through a group" |
| Q2e | `uf` | header: the no-username sentence; Grant access disabled with "A grant names a user by username, and this user has none yet."; Roles `r1` Direct "No: no username yet" |
| Q2f | `g1` | Members `ua` (`ua`), `ub` (`ub`); no roles held, no workspaces held |
| Q2g | `g2` | Members `ue`; Roles held `r1` Platform "egret only — not applied on engines: held through a group" |
| Q2h | `r1` | Holders `ud` (`ud`), `uf` ("Not named on engines until they have a username"); Groups holding this role: `g2`, Local, "egret only — not applied on engines"; header "2 holders" |
| Q2i | `public` | the "Everyone. Engines resolve public themselves…" sentence; tabs Permissions and Masks & filters only |

**Q3. Permissions, Masks & filters, Clearances.**

| id | page | prediction |
|---|---|---|
| Q3a | `ua` Permissions | Directly: G6; Through a group: G1 ("Through group g1 · applied on engines"); G5 under Through a group |
| Q3b | `ue` Permissions | Directly: G4 with two lines, "Directly · applied on engines" and "Excluded in egret only — not on engines: through role r1, held by group g2"; egret only: G2 |
| Q3c | `ue` real query | `ue` reads `ice.s.t` on both engines (G4's exclude compiles to an `allowExceptions` naming only `ud`); `v` masked by M1 |
| Q3d | `ud` Permissions | Through a role: G2, G5; Excluded: G4 ("Excluded: through role r1 · applied on engines"). `ud` still reads `ice.s.t` on both engines (G2) |
| Q3e | `uf` Permissions | the no-username note; G2, G5 under "Not applied on engines" |
| Q3f | `g1` / `r1` Permissions | `g1`: Directly G1, G5. `r1`: Directly G2, G5; Excluded G4 |
| Q3g | `ua` Masks & filters | Through a group: M1; "2 other rules apply to everyone they do not exempt…" |
| Q3h | `ue` Masks & filters | Through a group: M2 |
| Q3i | M1 real query | `ua`, `ub` read real `v`; `ud`, `ue` read masked `v`, on both engines |
| Q3j | Clearances `uc` | `vggroup`: Engines "Engines do not enforce this gate (it names only groups)", egret objects "Not cleared". Real query: `uc` reads real `ice.s.c6.secret` on both engines |
| Q3k | Clearances `ua` | `vggroup`: Engines the groups-only sentence, egret "Cleared through group g1". `vgrole`: Engines "Not cleared", egret "Not cleared". Real query: `ua` on `c7.secret` refused by Trino; NULL on StarRocks (the clearance mask, #1 live doc C2) |
| Q3l | Clearances `ud`, `ue`, `uf` | `vgrole`: `ud` Engines "Cleared through role r1"; `ue` "Not cleared: the required role is held through a group…", egret "Cleared through role r1, held by group g2"; `uf` "Not cleared: no username yet", egret "Cleared through role r1". `ud` reads real `c7.secret` on both engines |
| Q3m | Clearances `g1` | `vggroup`: Engines the groups-only sentence; egret "Cleared: the gate names this group" |

**Q4. Check access** (target `ice.s.t`, action read).

| id | subject | prediction |
|---|---|---|
| Q4a | `ua` (user page tab) | `ExplainResultView`, allowed on `vgtr` and `vgsr`; Trino rows, StarRocks rows |
| Q4b | `uf` | "No username yet, so no engine knows this user."; nothing asked |
| Q4c | `g1` | table of 2 members (`ua`, `ub`), each allowed on both profiles; counts 2 / 0 / 0; the "no single verdict" sentence; no "Not asked" |
| Q4d | `r1` | 1 evaluated (`ud`, allowed on both); "Not asked": `uf` "No username yet…"; counts 1 / 0 / 0 |
| Q4e | each member's verdict | equals a real query on each engine |
| Q4f | index dialog | the same answers for a user and a group, chosen through the subject kind |

**Q5. Writes from People.**
- Q5a: Grant access on `uc`'s page opens GrantDialog with `user uc` in Include; read on `ice.s.t`, Create; toast;
  the Permissions tab shows it under Directly. Within one poll (≤ 10 s) `uc` goes from refused to rows (`v` masked)
  on both engines.
- Q5b: in a GrantDialog (Data page `ice.s.t`), the include picker lists `un` under "Users with no username yet"
  with "Allocate username and add"; confirm; the chip `user un` appears with "un was allocated a username and
  added, stored as un."; Create; `un` reads `ice.s.t` on both engines within one poll.
- Q5c: two picks in a row (`ua` then `ub`) without leaving the field: the list stays open after the first pick.
- Q5d: Allocate for `uz` is refused: an alert "1 stored grant principal(s) or rule exemption(s) already name "uz"
  as a user; …" with a link "grant <G6 id> · include names uz"; no chip; `uz` keeps no username.

**Q6. Old URLs and single items.**
- `/governance/data-permissions`, `/governance/mask-filter-rules`, `/governance/explain-access` →
  `/governance/people` (replace). `?grant=<G1>` → `/governance/grants/<G1>`; `?rule=<M1>` →
  `/governance/rules/<M1>`. `/governance/explain` (the brief's name) is not a route: the catch-all opens
  `/governance/data`.
- `/governance/grants/<G1>`: sentence, scope, principals linked to People, Open in Data, Edit/Delete for admin. An
  unknown id: "No grant has this id; it may have been deleted."
- `/governance/rules/<M1>` likewise; `/governance/rules/<M3>` says "No tag has the key vgorphan now."
- An explain attribution link (Check access answer for `ua`) opens `/governance/grants/<id>`.
- TagDetail of `vgtagrule`: section "Mask and filter rules" listing M2, "This tag", exempt `g2`.
- Classifications: "1 rule selects a tag that no longer exists" listing M3 · tag key `vgorphan`.

**Q7. Data area Who can read? with a group.** On `ice.s.t`: subject Group `g1` → 2 members allowed on both.
On `ice.s.c7` column `secret`: `ua`, `ub` refused on `vgtr` (clearance deny), `MASK_NULL` on `vgsr`; real queries:
Trino `Access Denied`, StarRocks `NULL`.

**Q8. 390 px, dark, keyboard.**
- At 390 px, light and dark: index, each principal page and tab, GrantPage, RulePage, the GrantDialog with the
  picker open and the Check access dialog: the page never scrolls sideways (`scrollWidth == clientWidth`). Tables
  may clip inside their own scroll containers (as #1's D10). Dark mode: no white surface.
- Index tabs and principal tabs: Radix arrows move and select; `?kind=` / `?tab=` follow.
- PrincipalPicker: ArrowDown/Up move the active option, Enter adds it and the list stays open, Escape closes the
  list and **not** the GrantDialog.
- SubjectNameField (group/role subject, Check access dialog): arrows and Enter pick. **Escape with the list open
  closes the whole dialog** (Radix listens on the document in the capture phase, before the field's React
  handler; the field has no window capture listener like the picker's). Predicted defect.
- SubjectUserField (user subject): not a combobox; no arrow keys; Tab to a suggestion closes the list (the
  150 ms blur timer) so a suggestion cannot be picked by keyboard; typing the username works. Escape closes the
  dialog.

**Q9. Stewards.**
- `vsr` (`policy:read`): sidebar shows People & groups; index and pages load; no Grant access on principal pages;
  no Manage in Control Plane; GrantPage says "Editing or deleting a grant needs the policy:update permission.";
  Clearances tag names are not links (no `tag:read`). Allocate is unreachable (no GrantDialog opens).
- `vs` (`policy:read` + `policy:update`): Grant access shown; the picker offers "Allocate username and add".
- `vt` (`tag:read` only): no People & groups in the sidebar; `/governance/people` renders the Forbidden page.
