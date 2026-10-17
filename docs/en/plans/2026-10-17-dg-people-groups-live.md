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
