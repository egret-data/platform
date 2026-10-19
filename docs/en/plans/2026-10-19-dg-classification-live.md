# Classification: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-19-dg-classification.md`. Spec:
> `docs/en/specs/2026-10-19-dg-classification-design.md` (premises 1–18, C1, C2, CL1–CL17, Q1–Q5). Branch
> `feat/dg-classification`, worktree `egret-dgcl`.

## Measurements (Task 1)

The question: do the premises that reading alone could not settle hold on a real stack? Eight cells: the catalog name
an engine's access request carries (M1), how each policy shape matches each object and what the engine does with it
(M2), whether a hand-written item naming a group or a role reaches anyone (M3), what a BI read check costs (M4),
whether explain names a hand-written decider (M5), the app URL of each BI kind (M6), whether a tag policy matches a
tag name of another case (M7), and what a tag delete removes or refuses (M8).

### 1.0 Predictions (committed before anything was observed)

Written from the code at `07a84415` (main `309405b1` plus the spec and plan commits, which change no code), before any
container of this run was started. A wrong prediction is a finding. Nothing in this subsection is edited after
observing.

**Setup the predictions are about.** egret from this branch with its own DB `egret_dgcl_live` in the private `dgcl-pg`;
Trino 483 (`trinodb/trino@sha256:db58cc93…`) with the Ranger plugin on profile `cltr` (tag service `cltr-tag`) and
StarRocks 4.1.6 allin1 (`5cbf09c4…`, `access_control = ranger`) on profile `clsr`, both polling egret every 5000 ms;
every container and network named `dgcl-`. A registry catalog `ice` attached to both engines, whose engine-side catalog
is named `ice` and holds `s.t (id, email, country)` and `s.u (id)`. Users `ua`, `ub`; egret group `g1` (`ua` a member);
egret role `r1` (held by `ua`). Hand-written policies written while a profile is briefly `manual` (#4a DEV-4). Each M2
policy grants `ua` `select`, `use` and `show` and is the only non-baseline policy present while it is measured.

| id | check | prediction |
|---|---|---|
| M1a | catalog name in each engine's access request for `ice` | the registry name `ice` on both (Trino names a catalog file `<registry name>.properties`, StarRocks runs `CREATE EXTERNAL CATALOG \`<registry name>\``) |
| M1b | a hand-written allow on `catalog = ICE` | works on both: the `catalog` level is `ignoreCase: true` in both service-defs; `catalog = ic3` (another spelling) refused on both |
| M2a | `{catalog: ice}` only | matcher: `ice` Self, `ice.s` Ancestor, `ice.s.t` Ancestor, `ice.s.t.email` Ancestor; engine refuses `SELECT email FROM ice.s.t` on both |
| M2b | `{ice, schema: s}` only | `ice` Descendant, `ice.s` Self, table and column Ancestor; refused on both |
| M2c | `{ice, s, table: *, column: *}` | `ice` Descendant, `ice.s` SelfAndAllDescendants, `ice.s.t` SelfAndAllDescendants, column Self; allowed on both |
| M2d | `{ice, s, t}` no column | table Self, column Ancestor; refused on both |
| M2e | `{ice, s, t, column: email}` | `ice.s.t` Descendant, column Self; the query naming only `email` allowed on both |
| M2f | `{ice, s, table: u (isExcludes), column: *}` | `ice.s.t` SelfAndAllDescendants (t is not u), any `ice.s.u…` None; `email` allowed on both |
| M2g | `{ice, schema: s (isRecursive), table: *, column: *}` | the default matcher ignores `isRecursive`: same as M2c on the matcher and the engines |
| M3a | allow naming group `g1` only (ua a member in egret) | refused on both engines (no group provider, D7) |
| M3b | allow naming role `r1` only (held by ua in egret) | refused on both engines (no roles download, D0) |
| M4 | 500 sequential `object_ref` + `Pdp::authorize` over 500 dashboards in 5 workspaces | admin: p50 ≈ 1.5 ms, p95 ≈ 4 ms, total ≈ 0.8 s; one-workspace user: p50 ≈ 2 ms, p95 ≈ 5 ms, total ≈ 1.2 s; both under 2 s |
| M5 | `POST /perm/explain` for `ua` on each M2 policy the engine applied | `allowed` with `policyName` = that hand-written policy's name (and its id in the attribution as a hand-written row); for M2a/b/d `denied`, `policyName ""` |
| M6 | app URL per BI kind | dataset `/explore/<slug>/datasets/<id>`, chart `/explore/<slug>/charts/<id>`, dashboard `/explore/<slug>/dashboards/<id>`, app `/explore/<slug>/apps/<id>`; saved query and folder: no URL opens one by id |
| M7 | hand-written Trino tag deny on `PII` against a tag named `pii` on `ice.s.t.email` | not applied: `ua` reads `email`; the same policy on `pii` refuses it |
| M8a | delete a gated tag with a table and a column assignment, no rule | 204; no `tag_assignments`/`catalog_tag_assignments` row for it; the clearance deny (`perm-clearance-<tag id>`) absent from the next `cltr-tag` and `clsr` downloads |
| M8b | delete a tag a `perm.rules` row selects | 409 with `this tag is still selected by a permission rule (perm.rules) — delete or retarget that rule first`; nothing removed |

**Stop rules (plan Task 1).** M1 shows a spelling other than `registry.catalogs.name`; M2 contradicts Q2's mapping
(Self/SelfAndAllDescendants applied, Descendant partial, Ancestor not applied); M3 shows a group or role item applied;
M4 exceeds 5 s. Any of these is recorded and reported before Tasks 3–6.

### 1.1 Result (read this first)

**Not blocked.** No stop rule fired. Q2's mapping holds on both engines (M2: every `Self_`/`SelfAndAllDescendants`
shape was allowed, every `Ancestor`-only shape refused, a column policy allowed exactly its column, an excluded table
refused), a group- or role-named item reached nobody (M3), both engines carry the registry name and fold its case (M1),
explain names hand-written deciders by name and id (M5), and a tag policy does not fold the tag name's case (M7). Two
predictions were wrong, neither changing a user decision:

1. **M4 is slower than predicted but under the 5 s line.** 500 sequential `object_ref` + `Pdp::authorize` took
   3.0–3.3 s in a release build on a quiet machine (p50 ≈ 5.5 ms, p95 ≈ 9–10 ms; admin and one-workspace user alike),
   against a prediction of < 2 s. A debug build while other builds ran took 5.4–7.1 s. 500 bare `SELECT 1` took
   0.93–1.12 s, so the cost is round trips to Postgres in Docker Desktop (about three per check), not Cedar. CL12's
   500 cap stands; the constant's doc cites this. Spec premise 17 records the numbers.
2. **M5: a hand-written decider carries no attribution** (`attribution: null`); the id and name are there. Task 4's
   differential compares by policy name.

Also seen, outside #4b: a **table-level** tag with a clearance compiled, on StarRocks, only for the tagged **column**
of the same tag (`perm-clearance-<tag>-ice.s.t.country` and its mask); the table-level assignment on `ice.s.u`
produced no StarRocks policy (Trino's single tag-service deny covers both through the tag download). Recorded for
whoever owns clearance; #4b only reads.

### 1.2 Stack and deviations

- **egret ran as a host process, not an image** (DEV-1). The binary was built from `ac939cf8` (the predictions
  commit; no code change since `07a84415`) with `cargo build -p egret-bin`, copied to the scratchpad and run as
  `egret serve` on 127.0.0.1:18498 against DB `egret_dgcl_live` in `dgcl-pg` (`migrate`, `init`), with
  `EGRET_RANGER__ENABLED=true` and `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:18498`; engines reached it
  through `host.docker.internal`. A release image build was not needed for anything the cells measure.
- **The engine side of `ice` is PostgreSQL, not Iceberg** (DEV-2). Registry catalog `ice` is an egret Iceberg catalog
  (its warehouse `icewh` inserted by SQL with a storage connection; `storage_connection_id` then removed from its config
  because the external-engine registration refused it). On the engines, a catalog named `ice` reads database
  `dgcl_src` in `dgcl-pg` (Trino `postgresql` connector; StarRocks JDBC catalog) holding `s.t (id, email, country)`,
  `s.u (id)` and `s2.v (id)`. Ranger requests name catalog, schema, table and column only, so the connector does not
  change what the plugin evaluates; the tag download for `ice` comes from egret's `catalog_tag_assignments` as always.
- **Engines registered as external through the API** (`POST /control-plane/engines/external`, Trino plugin 2.8.0 on
  profile `cltr` with tag service `cltr-tag`, StarRocks 2.9.0 on `clsr`, both declaring `ice`), and their plugin files
  taken from `GET /engines/{id}/ranger-plugin-config` with the poll set to 5000 ms. `dgcl-trino` (Trino
  `db58cc93…` = 483) authenticates nobody: the user is the `X-Trino-User` header, which names the principal exactly as
  a JWT would; no signing key was needed. `dgcl-sr` (allin1 `5cbf09c4…` = 4.1.6-6862092): users `ua`, `ub` and catalog
  `ice` created by `root` before `access_control = ranger` was appended and the FE restarted.
- **Hand-written policies inserted by SQL** as `operator` rows (DEV-3), all disabled; each cell enabled exactly the
  rows it measured (plus a Trino baseline `trinouser:*` impersonate and `queryid:*` execute for `ua`, `ub`), bumped every
  profile's `policy_version` and waited 14 s (StarRocks runs two plugin instances). The M7 tag denies were written in
  the editor's tag shape (`serviceType: tag`, access `select`).
- **M3 did not create egret group `g1` or role `r1`**: the engine evaluates the item against its own (empty) group and
  role resolution, so egret-side membership cannot change the engine's answer; the cell measures the engine.
- **M4 ran as a scratch test** (never committed) against the test database in `dgcl-pg`: 5 workspaces, 500 tagged
  dashboards, a platform admin and a `ws_viewer` of one workspace, the real `CedarPdp`; release build from a
  `git archive` of `889b8370` in its own target dir, removed afterwards.
- **M6 was read from the route table**, not driven in a browser: `frontend/src/App.tsx:239`, `:242`, `:244`, `:247`.
- **M2's matcher column** came from a scratch test calling `match_policy_resources` on the exact bodies the engines
  received (never committed).

### 1.3 Every cell

| id | prediction | observed | verdict |
|---|---|---|---|
| M1a | registry name `ice` on both | a policy on `ice` decides both engines' requests (M2c, M2e…); Trino renders `<registry name>.properties`, StarRocks `CREATE EXTERNAL CATALOG \`<registry name>\`` | as predicted |
| M1b | `ICE` works, `ic3` refused | `ICE`: Trino `OK`, StarRocks rows; `ic3`: Trino "Cannot access catalog ice", StarRocks ERROR 5204 | as predicted |
| M2a | `{ice}`: Self, Ancestor below; refused | matcher `ice` Self_, rest Ancestor; Trino "Cannot select from columns [email] in table or view t" (catalog access passed), StarRocks ERROR 5204 SELECT on COLUMN email | as predicted |
| M2b | `{ice, s}`: Descendant/Self/Ancestor; refused | `ice` Descendant, `ice.s` Self_, table/column Ancestor; both refused as M2a | as predicted |
| M2c | `{ice, s, *, *}`: SelfAndAll…; allowed | `ice` Descendant, `ice.s`/`ice.s.t`/`ice.s.u` SelfAndAllDescendants, column Self_; both return 2 rows | as predicted |
| M2d | `{ice, s, t}`: table Self, column Ancestor; refused | `ice.s` Descendant, table Self_, column Ancestor, `ice.s.u` None; both refused | as predicted |
| M2e | `{ice, s, t, email}`: table Descendant, column Self; allowed | as stated; both return 2 rows | as predicted |
| M2f | exclude `u`: t SelfAndAll…, u None; t allowed | as stated; `SELECT email FROM ice.s.t` allowed on both; `SELECT id FROM ice.s.u` refused on both | as predicted |
| M2g | `isRecursive` on schema = M2c | identical to M2c on the matcher and both engines | as predicted |
| M3a | group `g1` item: refused on both | Trino "Cannot access catalog ice"; StarRocks ERROR 5204 | as predicted |
| M3b | role `r1` item: refused on both | the same | as predicted |
| M4 | < 2 s for 500 | release, quiet: 3.19 / 3.04 s and 3.27 / 2.93 s (admin / one-workspace user), p50 5.4–5.9 ms, p95 8.4–10.1 ms, `object_ref` p50 1.6–1.8 ms; 500 × `SELECT 1` 0.93–0.98 s. Debug under load: 5.4–7.1 s | **wrong** (finding 1), under the 5 s stop line |
| M5 | explain names the hand-written policy | Trino and StarRocks, `ua`, `ice.s.t.email`: M2c/e/f/g `allowed` with `policy_name` `m2c-cltr` (id 7) / `m2c-clsr` (id 19) etc.; M2a/b/d `denied`, `no_policy_matched`, `policy_name ""`; `attribution: null` | as predicted (finding 2) |
| M6 | dataset/chart/dashboard/app URLs; none for saved query, folder | `/explore/:workspace/datasets/:id`, `charts/:id`, `dashboards/:id`, `apps/:id`; no route opens a saved query or a folder by id | as predicted (read from the route table) |
| M7 | `PII` deny does not apply to tag `pii` | with `PII` deny: `ua` reads `email` (`OK`); with `pii` deny: "Cannot select from columns [email]", `id` still readable | as predicted |
| M8a | 204, assignments and clearance gone | before: 0 BI and 2 catalog assignments, `perm-clearance-<tag>` in `cltr-tag`, `…-ice.s.t.country` and `…-mask-ice.s.t.country` in `clsr` (both downloads); delete 204; after: 0 / 0 / no row, neither download carries it | as predicted |
| M8b | 409 with the FK sentence | 409 `{"error":"conflict: this tag is still selected by a permission rule (perm.rules) — delete or retarget that rule first"}`; tag kept | as predicted (the body adds the `conflict: ` prefix) |

### 1.4 Left behind

Removed: the egret process, containers `dgcl-trino` and `dgcl-sr`, network `dgcl-net` (`dgcl-pg` disconnected first),
DBs `egret_dgcl_live` and `dgcl_src`, the egret binary copy, the admin password file, the cookie jar, the release
target dir and the exported source tree of M4. Kept in the scratchpad: the scripts (`api.sh`, `tq.py`, `sq.sh`,
`only.sh`, `m2.sh`, `policies.py`), the Trino and StarRocks plugin files and the cell outputs. `dgcl-pg` and
`dgcl-valkey` stay for the package.
