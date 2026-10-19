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

## 2. Live browser check (Task 19)

### 2.0 Predictions

Written at `79176846` (Tasks 1–18) before any container of this run was started; nothing in this subsection is edited
after observing. A wrong prediction is a finding.

**Setup the predictions are about.** Task 1's recipe (§1.2), rebuilt from `79176846`: egret as a host binary on
127.0.0.1:18498 against DB `egret_dgcl_live` in `dgcl-pg`; `dgcl-rustfs` as the warehouse's object store, so the egret
catalog `ice` (warehouse `ice` in workspace `w1`) holds real Iceberg tables `s.t (id, email, country)`, `s.u (id)` and
`s2.v (id)` and the Data pages can browse them; `dgcl-trino` (483, user from `X-Trino-User`) and `dgcl-sr` (4.1.6,
`access_control = ranger`) registered as external engines on profiles `cltr` (tag service `cltr-tag`) and `clsr`,
their catalog `ice` reading PostgreSQL DB `dgcl_src` with the same tables; Ranger poll 5000 ms. Users through the API:
`admin`; `vs` (`tag:read`, `tag:manage`, `policy:read`); `vt` (`tag:read`); `ua` (member of `w1` only, plus `tag:read`
so it can open a classification, L5); `ub`; egret group `g1` (`ua` in it); egret role `cleared` (held by `ub`). Tag
`PII` (key `pii`). Hand-written policies inserted by SQL as `operator` rows (DEV-n), on both profiles unless said:
`hw-l1` allow `ua`, `ub` `select` on `{ice, s, *, *}`; `hw-l2` deny `ua` `select` on `{ice, s, t, email}`; `hw-l3`
(cltr-tag only) mask `ua` on tag `PII`; `hw-l4-g1` allow group `g1` on `{ice, s, t, *}`; `hw-l4-pub` allow group
`public` on `{ice, s, u, *}`; Trino baselines `hw-base-imp`/`hw-base-qid` naming `ua`, `ub`. Vite from this worktree on
127.0.0.1:5398 proxying `/api`, `/catalog`, `/oauth` to 18498. The browser is driven by scripted Playwright
(`@playwright/test` from `frontend/node_modules`, Chromium), screenshots inspected; logins ≥ 60 s apart.

| id | check | prediction |
|---|---|---|
| L1a | `hw-l1` on Data `ice.s.t` Permissions | listed under "Hand-written policies (N)" with the "Hand-written" badge, "Applies to this object · through a pattern", items "Allow: ua, ub — select" (one row per profile: `cltr` and `clsr`) |
| L1b | `hw-l1` on schema `ice.s` | "Applies to this object" **without** "through a pattern" (every level the request names is literal: `ice`, `s`) |
| L1c | `hw-l1` in the column panel of `email` | "Applies to this object · through a pattern" |
| L1d | schema `ice.s2` | `hw-l1` not listed |
| L1e | engines, only `hw-l1` (+ baselines) enabled | `ua` `SELECT email FROM ice.s.t` returns rows on Trino and StarRocks; `SELECT id FROM ice.s2.v` refused on both |
| L2a | `hw-l2` on `ice.s.t` | "Applies to part of it", a "Deny" item with the deny icon |
| L2b | `hw-l2` in `email`'s panel | "Applies to this object" (no pattern) |
| L2c | engines, `hw-l1` + `hw-l2` | `ua` `SELECT email FROM ice.s.t` refused on both; `SELECT id FROM ice.s.t` allowed on both |
| L2d | Who can read? on `email` for `ua` | denied, deciding policy `hw-l2-<profile>` (agrees with the engines) |
| L3a | `hw-l3` in `email`'s panel | one row, service `cltr-tag`, "Applies to this object", "Through tag PII on this object", "Mask: ua — mask MASK" |
| L3b | `hw-l3` on `ice.s.t` Masks & filters | "Applies to part of it", "Through tag PII on column email"; absent from Permissions (masking row) |
| L3c | `PII` page Hand-written tab | `hw-l3` listed, engines "cltr", editor link `/governance/engines/<cltr id>/advanced/policies/masking/<id>?scope=tag` |
| L3d | engines, `hw-l1` + `hw-l3` (`hw-l2` off) | Trino returns `email` masked for `ua`; StarRocks returns it unmasked (no tag service; the UI lists the tag row only under `cltr-tag`, so it claims nothing for StarRocks) |
| L4a | People `ua` Permissions | "Direct" lists `hw-l1`, `hw-l2` (and the Trino baselines `hw-base-imp`, `hw-base-qid`); "Through a group or role" lists `hw-l4-g1` with "Names group g1, which this user is in. Engines resolve no groups, so on the engine it reaches nobody."; "Applies to everyone": "2 hand-written policies name everyone." (`hw-l4-pub` on both profiles), expandable |
| L4b | engines, `hw-l4-g1` only | `ua` refused `ice.s.t` on both (Task 1 M3) — the sentence's claim holds |
| L4c | People `ua` Masks & filters | `hw-l3` under Direct |
| L5a | tag `ice.s.u` (table), `ice.s.t.email` (column), a dashboard in `w1`, a dashboard in `w2` | index row PII: Data "1 table · 1 column", BI objects 2 |
| L5b | Where used as admin | Data: `ice.s` group, `t` "Columns: email", `u` "Table"; BI: both dashboards with links `/explore/w1/dashboards/<id>`, `/explore/w2/dashboards/<id>` |
| L5c | Where used as `ua` | BI lists `w1`'s dashboard only and "1 object you cannot read."; no workspace name of `w2` anywhere |
| L5d | as `vt` | page renders; Where used shows both dashboards as unreadable ("2 objects you cannot read."); Mask & filter "Rules are read with the policy:read permission, which this account does not have."; Hand-written "Hand-written tag policies are read with the policy:read permission, …"; no `/perm/rules` and no `/handwritten` request |
| L6a | Clearance: add role `cleared` as `vs` | row added; the compile line "Compiles to one deny tag policy in each Trino profile's tag service, and on StarRocks to a select deny per tagged column." shown |
| L6b | engines after one poll (`hw-l2` off) | `ua` `SELECT email FROM ice.s.t` refused on both; `ub` reads it on both |
| L6c | remove `cleared` | after one poll `ua` reads `email` again on both (masked on Trino by `hw-l3`) |
| L7a | delete dialog with a rule selecting `pii` | lines "Removes it from 1 table and 1 column.", "Removes it from 2 BI objects.", "1 rule selects it: the delete will be refused until they are deleted or retargeted."; Delete disabled after typing `PII`, enabled after `pii` |
| L7b | confirm with the rule present | 409 toast (the rules sentence), dialog closed, focus on the page's Delete button |
| L7c | delete the rule, delete again | navigates to the index; `PII` not listed; `tag_assignments` and `catalog_tag_assignments` have no row for its id |
| L8 | old links | Overview `tags_without_clearance` row → `/governance/classifications/<id>`; Engine & policy compiled clearance row "Open source" → `…?tab=clearance` (Clearance tab selected); `ub`'s People Clearances → `?tab=clearance`; RulePage "Open classification" → `?tab=rules`; `/control-plane/classifications/<id>` → `/governance/classifications/<id>` |
| L9 | keyboard, Escape, widths, themes | the index, the tabs (arrow keys move `?tab=`), Edit form, delete dialog and clearance picker reachable by keyboard with a visible focus ring; Escape closes each and focus returns to its opener; 390 px: `scrollWidth = clientWidth` on the index, every tab, the Data placements and People (stacked tables); 1280 px light and dark readable |

### 2.1 Result (read this first)

**Every cell ran; five defects found and fixed red-first; no user decision, security behaviour or scope changed.** The
cells were driven by scripted Playwright (`@playwright/test`'s Chromium from `frontend/node_modules`, node scripts in
the scratchpad), with every visual screenshot inspected; each engine claim was checked by running the query on Trino
483 and StarRocks 4.1.6 as that user. Route A's match words agree with what both engines did (L1, L2: an `ice.s.*.*`
allow applied, a column deny refused exactly its column, the explain dialog named the same hand-written policy); the
Trino tag mask is listed under the tag service only and the engine masked on Trino and not on StarRocks (L3); a
group-named item is marked as reaching nobody and the engines refused it (L4); Where used counts unreadable objects
only (L5); the delete dialog states every consequence and needs the exact key, the 409 is toasted with focus back on
Delete, and the delete removes every assignment (L7); every old link lands on its tab (L8); no page scrolls sideways at
390 px (L9). Defects:

| # | defect (cell) | fix |
|---|---|---|
| D1 | the Clearance tab asked a `tag:read` holder for the role and group lists it may not read and showed "The roles and groups could not be loaded" with a Retry; a `tag:manage` holder without `role:read` saw "No roles found." (L5d, L6a) | `aa01ce14` — the role list is asked only by an editor, the group list by an editor or to name required groups; a 403 says which permission lists them |
| D2 | the compile line said StarRocks gets "a select deny per tagged column"; on StarRocks the clearance's `MASK_NULL` decides and an uncleared reader reads NULL, not a refusal (L6b) | `aa01ce14` — the line names the NULL mask and its effect |
| D3 | on group `g1`'s page the policies naming `g1` sat under "Through a group or role" (route B reports the group's own name as `how: group`) (L4) | `1bfb3a03` — on a group or role page its own naming is Direct, Q1's sentence kept |
| D4 | Escape on New classification dropped focus to `<body>` (dialog opened from state) (L9) | `2ee0e418` — focus returns to the opener |
| D5 | Cancel or Save in the header's Edit form dropped focus to `<body>` (L9) | `77da0460` — Edit takes focus back |

Wrong predictions that are not defects: L1a (one row per profile, sorted `clsr` before `cltr`, as designed); L3a (the
column panel's tag row reads "Through tag PII on column email", not "on this object": the server names the column);
L6b on StarRocks (D2 fixes the wording; the engine behaviour is SP4c/view-governance's, unchanged by #4b, and explain
already states it); L5a's route (the UI refused a table-level `PII` on `s.u` while hand-written tag mask `hw-l3`
exists — the existing unreadable-table guard, a correct 400 with the form kept — so that assignment was inserted by
SQL, DEV-6).

### 2.2 Stack and deviations

- **DEV-1: egret as a host binary, not image `egret-backend:dgcl-2`.** `cargo build -p egret-bin` at `79176846`
  (100 s, 152 GiB free), run on 127.0.0.1:18498 against DB `egret_dgcl_live` in `dgcl-pg` (`migrate`, `init`) with
  Ranger, the embedded catalog (`EGRET_CATALOG__ENABLED`) and a fresh secrets master key enabled. Vite from this worktree
  on 127.0.0.1:5398 through a scratch config that imports `frontend/vite.config.ts` and points `/api`, `/catalog`,
  `/oauth` at 18498. Fixes D1–D5 were re-checked live through Vite (frontend only; no binary rebuild needed).
- **DEV-2: two copies of `ice`.** The egret catalog `ice` (warehouse `ice` in `w1`, objects in `dgcl-rustfs`,
  `rustfs/rustfs:1.0.1`) holds Iceberg tables `s.t (id, email, country)`, `s.u (id)`, `s2.v (id)` created through the
  Iceberg REST API (metadata only), so the Data pages browse them; the engines' catalog `ice` reads the same tables
  from PostgreSQL DB `dgcl_src` (Trino `postgresql` connector, StarRocks JDBC catalog), as Task 1 DEV-2.
- Engines: `dgcl-trino` (`trinodb/trino@sha256:db58cc93…` = 483, user from `X-Trino-User`, no signing key) and
  `dgcl-sr` (allin1 `5cbf09c4…` = 4.1.6-6862092, users and catalog created by `root`, then `access_control = ranger`),
  on `dgcl-net`, registered through `POST /control-plane/engines/external` on profiles `cltr` (tag service `cltr-tag`)
  and `clsr`; plugin files from `GET /engines/{id}/ranger-plugin-config` with the poll set to 5000 ms.
- **DEV-3:** catalog `ice` unassigned from `w1` (`PUT /catalogs/{id}/workspaces []`), because the workspace-assignment
  grant gave every `w1` member, `ua` included, `select` on `ice.*.*.*` and would have decided L1e/L2c/L6.
- **DEV-4 (SQL):** `origin` flipped to `manual` on `cltr`/`clsr` around the hand-written creates through
  `POST /policies` (binding an engine made both `generated`), as #4a DEV-4. `hw-l3` was created on the manual
  `cltr-tag` without a flip.
- **DEV-5 (SQL):** per engine phase, hand-written rows enabled/disabled with `version` and `policy_version` bumped,
  then a 14 s wait (`only.sh`); the enabled set of each phase is in 2.3. From L5 on, `hw-l3` stayed disabled.
- **DEV-6 (SQL):** the table-level `PII` assignment on `ice.s.u` (see 2.1).
- **DEV-7:** users through the API with engine usernames allocated (`POST /perm/people/users/{id}/username`); `vs`
  holds a custom role `tag:read`, `tag:manage`, `policy:read`, to which `role:read` and `group:read` (both in the
  seeded `data_steward`) were added for L6 after D1 was observed; `ua` and `vt` hold a role with `tag:read` only; `ub`
  holds role `cleared`; `ua` is `ws_viewer` of `w1` and in group `g1`.
- **Extra cell LX** (not in the plan; the implementer named it a risk): after the main stack was torn down, a reduced
  stack (egret, `dgcl-rustfs`, the same catalog, external engines registered with no container behind them) showed
  route A's `not_determined` before the engines were attached, and `parent` and `unknown` rows after.

### 2.3 Every cell

| id | prediction | observed | verdict |
|---|---|---|---|
| L1a | `hw-l1` on `ice.s.t` Permissions: applies · through a pattern | "Hand-written policies (6)": `hw-l1-clsr`, `hw-l1-cltr` "Applies to this object · through a pattern", "Allow: ua, ub — select", badge, links to `/governance/engines/<id>/advanced/policies/access/<id>`; `hw-l4-g1-*` "Applies to this object"; `hw-l2-*` part | as predicted |
| L1b | on `ice.s`, no pattern | "Applies to this object" without the pattern; `hw-l2`, `hw-l4-*` "Applies to part of it" | as predicted |
| L1c | column panel `email`: pattern | `hw-l1-*` "· through a pattern", `hw-l2-*` "Applies to this object", plus `hw-l3` (7 rows, all types) | as predicted |
| L1d | `ice.s2`: not listed | "Hand-written policies (0)", "No hand-written policy reaches this object." | as predicted |
| L1e | engines, `hw-l1` only | `ua` `SELECT email FROM ice.s.t`: Trino and StarRocks return both rows; `SELECT id FROM ice.s2.v`: Trino "Cannot select from columns [id]", StarRocks ERROR 5204 | as predicted |
| L2a–b | part on the table, applies on the column, deny icon | as stated; Deny with the ban icon | as predicted |
| L2c | engines, `hw-l1` + `hw-l2` | `email` refused on both ("Cannot select from columns [email]", ERROR 5204 on COLUMN email); `id` returned on both | as predicted |
| L2d | Who can read? `ua` on `email` | `cltr` "Refused by a deny item … Ranger policy hw-l2-cltr, id 5"; `clsr` the same with `hw-l2-clsr`, id 9 | as predicted |
| L3a | column panel: `hw-l3`, `cltr-tag`, applies, via tag on this object | applies, "Through tag PII on column email", "Mask: ua — mask MASK" | wording differs, not a defect |
| L3b | table Masks & filters: part, absent from Permissions | "Applies to part of it", "Through tag PII on column email"; not on Permissions | as predicted |
| L3c | `PII` Hand-written tab | `hw-l3`, engines "cltr", link `…/94a8b3eb…/advanced/policies/masking/3?scope=tag` opens the editor on By tag, tag PII | as predicted |
| L3d | Trino masked, StarRocks plain | Trino `ua` `["xxxxx@xxxxxxx.xxx"]…`; StarRocks `alice@example.com …`; `ub` plain on both | as predicted |
| L4a | `ua` Permissions groups | Direct: `hw-l1-*`, `hw-l2-*`, `hw-base-imp`, `hw-base-qid`; Through a group or role: `hw-l4-g1-*` with "Names group g1, which this user is in. …"; "2 hand-written policies name everyone." → Show them lists `hw-l4-pub-clsr`, `hw-l4-pub-cltr`. Group `g1`'s page: its own policies under "Through a group or role" (D3); role `cleared`: none | as predicted; D3 |
| L4b | engines, `hw-l4-g1` only | Trino "Cannot access catalog ice"; StarRocks ERROR 5204 | as predicted |
| L4c | `ua` Masks & filters | `hw-l3` under Direct | as predicted |
| L5a | index "1 table · 1 column", 2 BI | as stated (before the table tag: "0 tables · 1 column"); adding `PII` to `ice.s2.v.id` in the Data page and returning by the sidebar showed "1 table · 2 columns" without a reload (CL16), removed after | as predicted |
| L5b | Where used as admin | `ice.s`: `t` "Columns: email", `u` "Table"; BI "W2 secret board" (w2) and "W1 customers" (w1) linking to `/explore/<slug>/dashboards/<id>` (server order, w2's workspace id first) | as predicted |
| L5c | as `ua` | BI "W1 customers" only and "1 object you cannot read."; no `w2` anywhere | as predicted |
| L5d | as `vt` | "2 objects you cannot read."; Mask & filter and Hand-written show the policy:read sentences and send no `/perm/rules` or `/handwritten` request; **Clearance tab: 403s on `/access/roles` and `/groups` shown as "could not be loaded" + Retry** (D1, also for `ua`) | D1 |
| L6a | `vs` adds `cleared`; compile line | without `role:read` the picker said "No roles found." (D1); after adding it: PUT 200 `skipped: []`, row "role cleared", the compile line shown; focus returned to the picker trigger | D1, then as predicted |
| L6b | `ua` refused on both, `ub` reads | Trino `ua` refused, `ub` reads; **StarRocks `ua` reads `NULL NULL`** (the clearance's `MASK_NULL`; Who can read? says StarRocks does not access-check a masked column), `ub` reads | prediction wrong on StarRocks; D2 |
| L6c | remove: `ua` reads after one poll | DELETE 200; 12 s later `ua` reads plain on both | as predicted |
| L7a | consequences, key only | "Removes it from 1 table and 1 column." / "2 BI objects." / "Removes its clearance (1 role, 0 groups)." / "1 rule selects it: the delete will be refused …"; disabled after `PII`, enabled after `pii` | as predicted |
| L7b | 409 toast, focus on Delete | DELETE 409, dialog closed, toast with the server's sentence and "The "Mask and filter rules" section on this page lists the rules …", focus on the page's Delete; the row-menu dialog returns focus to "Actions for PII" | as predicted |
| L7c | delete again | "No rule selects it."; DELETE 204; index lists only `Finance`; `tags`, `tag_assignments`, `catalog_tag_assignments`, `classification_policies` and the `perm-clearance-<id>` policies have no row for it | as predicted |
| L8 | old links | Overview `tags_without_clearance` row (`Finance`) → its page; Engine & policy "Open source" of the compiled clearance → `?tab=clearance`, Clearance selected; `ub`'s Clearances → `?tab=clearance`; RulePage "Open classification" → `?tab=rules`, Mask & filter selected; `/control-plane/classifications/<id>` → the tag page; `/control-plane/classifications` → the index | as predicted |
| L9 | keyboard, Escape, widths, themes | Tab reaches every index control with a visible ring; arrow keys move the tabs and `?tab=` (the first tab drops it); delete dialog and clearance picker return focus on Escape; **New classification and the Edit form dropped focus to `<body>`** (D4, D5); 390 × 844: `scrollWidth = clientWidth = 390` on the index, all five tabs, Data Permissions / Masks / column panel and People (hand-written tables stack; Where used's tables scroll inside their border); 1280 × 900 dark: all readable. Screenshots checked with `sips`: 390×844 (36–73 kB) and 1280×900 (65–195 kB) | D4, D5 |
| LX | (extra) | before attaching engines: "No engine is attached to this catalog, so no hand-written policy reaches it." on the table and column; after: `{ice}` and `{ice, s}` allows "Set on a parent" with "Names catalog=ice; schema=s only; engines do not apply it to this object's data." (Task 1 M2a/b: both engines refuse these shapes); an unparsable body "Could not be matched: invalid type: string "ice", expected struct ResourceLevel", "No items"; on `ice.s` the schema policy applies and the catalog one is a parent; on `ice` the catalog policy applies and the schema one is part | no defect |

### 2.4 Defects

D1–D5 above, each with a failing test first, run red for the stated reason, then fixed and re-checked live:
`ClearanceTab.test.tsx` (a reader asks for neither list and sees no load error; a steward refused the role list reads
the permission sentence; the compile line), `PrincipalHandWrittenSection.test.tsx` (a group page's own naming is
Direct), `ClassificationsIndex.test.tsx` (focus back on New classification), `TagPage.test.tsx` (focus back on Edit
after Cancel and after Save). D1's guards were mutation-checked (asking for the role list regardless, dropping the
403 filter, dropping the picker's refusal branch: each turned a test red; the restore verified with `cmp`).

Noted, not fixed: Where used lists BI objects in the server's order (workspace id), not by name; the delete 409 toast
says the rules section is "on this page" while it is on the Mask & filter tab; the row-menu delete dialog opens with
focus on Cancel and the page's on the key field. Outside #4b: on StarRocks a clearance on a column the reader may
otherwise select returns NULL rather than a refusal (`MASK_NULL` decides; explain already says so); a table-level tag
with a clearance compiles nothing on StarRocks (Task 1).

### 2.5 Left behind

Removed after the run: the egret processes, Vite, containers `dgcl-trino`, `dgcl-sr`, `dgcl-rustfs`, network
`dgcl-net` (`dgcl-pg` disconnected first), DBs `egret_dgcl_live` and `dgcl_src`, the binary copy, the admin and user
password files, the master key, S3 and catalog secrets, cookie jars and browser storage states. Kept in the scratchpad
`dgcl3/`: the scripts, plugin files, logs, `engine.out` and the screenshots. `dgcl-pg` and `dgcl-valkey` stay for the
package.

Gate after the fixes (frontend only; no Rust changed): `npx tsc -b` clean; `npm run lint` 0 errors (153 warnings,
none in a touched file); `npx vitest run` 574 files, 7781 tests passed; `openapi.json` regenerated and
`npm run gen:api` leave no diff.
