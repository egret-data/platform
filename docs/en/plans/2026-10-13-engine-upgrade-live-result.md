# Engine upgrade: live result

> **Measured:** 2026-10-13 (UTC), branch `feat/engine-upgrade`, base `466cf80f9`, worktree `egret-sp4a`.
> Canonical (English). Section 1 only; later sections are added by later tasks.

## 1. The plugin files egret renders, on Trino 483 and StarRocks 4.1.6 (Task 6)

**Result: PASS. No renderer change was needed.** The files egret renders (the external-engine Plugin config tab and the
provisioner's Trino ConfigMap) load on both new engines without an unknown-key warning, the plugins poll egret, a
known allow returns rows and a known deny returns the engine's denial.

### 1.1 Stack and versions proven by output

| piece | what ran |
|---|---|
| egret | `target/debug/egret serve` from this worktree on **:8096**, DB **`egret_engup_live`** (created in `egret-dev-postgres-1`, migrated and `egret init` by the branch binary), `EGRET_RANGER__ENABLED=true`, `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8096`, dummy `EGRET_STORAGE__BUCKET/ACCESS_KEY/SECRET_KEY` to boot. `kubernetes unavailable; engine provisioning disabled` (expected). |
| Trino | `engup-trino`: `trinodb/trino:483@sha256:db58cc93...ccb0534`, memory connector, HTTP, no auth. `/v1/info` -> `"nodeVersion":{"version":"483"}`; `select version()` -> `"483"`. Plugin jars `io.trino_trino-ranger-483.jar`, `trino-ranger-483-services.jar`, `org.apache.ranger_ranger-plugins-common-2.8.0.jar`. |
| StarRocks | `engup-sr`: `starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4...333e5d`. `select current_version()` -> `4.1.6-6862092`. `fe/lib/ranger-plugins-common-2.9.0.jar` (the whole ranger set is 2.9.0). |
| disk | `df -h /` free: 63 GiB before, 62 GiB after cleanup (StarRocks image 5 GB pulled; Trino 483 was already local). |

Registration used the API, not SQL: policy profiles `engup-trino` (with `tagService` `engup-tag`, itself a trino profile; a Trino profile needs a usable one) and `engup-sr`, then `POST /api/control-plane/engines/external` with Trino plugin version **2.8.0** and StarRocks **2.9.0**. Both answered 201 with `rangerPluginVersionModelled: true` (Tasks 1 to 5 working end to end). The files were taken verbatim from `GET /api/control-plane/engines/{id}/ranger-plugin-config`.

### 1.2 Trino 483, files from the external Plugin config tab

Files: `access-control.properties` (`access-control.name=ranger`, `ranger.service.name=engup-trino`, `ranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml`) and `ranger-trino-security.xml` (7 properties, `policy.rest.url=http://host.docker.internal:8096`, poll 30000).

1. **Plugin loads.** `-- Loading plugin ranger --`, `Installing io.trino.plugin.ranger.ApacheRangerPlugin`, `Registering system access control ranger`, the property table shows `ranger.plugin.config.resource [/etc/trino/ranger-trino-security.xml]` and `ranger.service.name engup-trino`, then `-- Loaded system access control ranger --` and `SERVER STARTED`. No unknown-property or deprecation line from Trino or Ranger.
2. **Polls egret.** `PolicyRefresher(serviceName=engup-trino): found updated version. lastKnownVersion=-1; newVersion=1`, `Updated tags-cache to new version of tags`, `Switched policy engine to [1]`; after each policy write within the 30 s interval: `lastKnownVersion=1; newVersion=14`, `Switched policy engine to [14]`, then 15, 18, 21.
3. **Allow and deny.** Hand-written policies (POST `/api/control-plane/policies`): `engup-nav` (use/show on `memory.default` for alice, bob, carol), `engup-t-select` (`memory.default.t.*`: allow select for alice and carol, **deny** select for carol), and the baseline `trinouser:*` impersonate and `queryid:*` execute.
   - alice `select * from memory.default.t` -> `"1","x"`; `select id ...` -> `"1"`.
   - bob (no select grant): `Access Denied: Cannot select from columns [v, id] in table or view t`.
   - carol (allow plus explicit deny): the same denial, so the deny item wins.

### 1.3 Trino 483, provisioned shape (Step 3)

The provisioner renders through the same `ranger_plugin_files` function. A scratch `#[ignore]` test (not committed; the file was restored byte for byte) called `trino_config_map` with a `RangerBinding` for `engup-trino` and dumped the data keys. **`access-control.properties` and `ranger-trino-security.xml` are byte-identical to what the external renderer gave** (`diff` empty). Mounted into a fresh `engup-trino-prov` container: plugin loaded, `found updated version. lastKnownVersion=-1; newVersion=21`, `Switched policy engine to [21]`, `-- Loaded system access control ranger --`; alice read `"1","x"`, bob and carol got the same `Access Denied: Cannot select from columns [v, id] in table or view t`; `select version()` -> `"483"`. The container's only ranger WARN/ERROR was the roles 404 below. (The provisioned `config.properties` was not mounted; only the two plugin files are in scope for this task.)

### 1.4 StarRocks 4.1.6, files from the external Plugin config tab

Files: `fe.conf (add this line)` = `access_control = ranger` (appended to `/data/deploy/starrocks/fe/conf/fe.conf`), `ranger-starrocks-security.xml` and `ranger-starrocks-audit.xml` in the same directory; container restarted.

1. **Plugin loads.** FE log: `Interacting with Ranger Admin Server using SIMPLE authentication`, `Created PolicyRefresher Thread(PolicyRefresher(serviceName=engup-sr))`, `Start Ranger plugin (starrocks - engup-sr) success`. No unknown or deprecated key line mentions ranger (the only `unknown` hits in `fe.warn.log` are BDB master election and an internal statistics insert, unrelated).
2. **Polls egret.** `Switched policy engine to [1]`, then `found updated version. lastKnownVersion=1; newVersion=6` and `Switched policy engine to [6]` after I wrote the policies.
3. **Allow and deny.** `root` is not special under Ranger, so policies `engup-root-*` let root create the database and table (create database, create table, insert all succeeded through them); `engup-nav` gives alice, bob, carol `usage` on `default_catalog`; `engup-t-select` is the same shape as for Trino on `default_catalog.engupdb.t.*`.
   - alice `select * from engupdb.t` -> `1 x`; `select id` -> `1`.
   - bob: `ERROR 5204 (42000): Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN v for this operation.`
   - carol (allow plus deny): the same 5204 denial.

### 1.5 Deploy XML against the renderer (hand comparison)

`deploy/starrocks/ranger/ranger-starrocks-security.xml` has the same 7 `<property>` names as the renderer output, in the same order, with the same values except the example `service.name` (`sr1`), `policy.rest.url` (`:8080`) and cache dir. `ranger-starrocks-audit.xml` has the same single property (`xasecure.audit.is.enabled=false`). The only differences are the XML comments. The dev compose mounts these onto the same `allin1-ubuntu:4.1.6` pin used here. The automated drift test from `fix/dg-backend-small` is not on this branch; this is the by-hand check.

### 1.6 Observations (not defects of this task)

- **Both plugins log `ERROR ... Error getting Roles; service not found ... response=404` then `Received 404 ... Ignoring` every poll.** egret answers the roles endpoint 404. Known (SP4d-4 finding 3), cosmetic, unchanged by the upgrade.
- **Trino 483 requires `impersonate` on `trinouser` and `execute` on `queryid` for every user, including a user acting as themselves.** Before the baseline policies even `admin` got `Principal admin cannot become user admin`. This is the documented default-deny baseline (see the 2026-06-18 Trino manual test), not new in 483, but it means a Trino profile with only table policies serves nobody.
- `seed-defaults` for a Trino profile grants those baseline policies to principal `admin` only; I wrote my own for the test users.

### 1.7 Left behind

Removed: containers `engup-trino`, `engup-trino-prov`, `engup-sr`; egret (:8096, stopped by its recorded PID). Kept: database `egret_engup_live` in `egret-dev-postgres-1` (profiles `engup-trino`, `engup-tag`, `engup-sr`, two external engines, the policies above) and the pulled `starrocks/allin1-ubuntu:4.1.6` image. Never touched: DB `egret`, `egret-dev-*` containers, `egret-w7c-postgres`, ports 5173, 5000, 8080.

## 2. The view-clearance matrix on Trino 483 and StarRocks 4.1.6 (Task 8)

> **Status:** 2.0 (the predictions) was committed FIRST as `b3f7bab7b`, before any cell was observed. 2.1 onwards
> and section 3 are the observations, committed second. A wrong prediction is a finding; no prediction was edited.
> Measurement only: no product code changed.

### 2.0 Predictions

Cell ids are those of `2026-10-13-view-clearance-measurement.md` (rounds 1 and 2). Round 2 questions A and B
were StarRocks-only, so they are run on StarRocks 4.1.6 only; Trino 483 runs round 1's Q1 to Q5. "Same" means the
observed result is the one that document recorded for the old version (Trino 470 / ranger 2.5.0; StarRocks 4.1.4 /
ranger 2.8.0). The old document already contains a live StarRocks **4.1.6** run (questions A and B), so for
StarRocks the "old" column is 4.1.4 and, where measured, that earlier 4.1.6 run; a cell that differs between 4.1.4
and 4.1.6 is predicted to match the earlier 4.1.6 run.

Setup differences that are not cell results (labelled, expected): Trino 483 needs baseline `trinouser:*` impersonate
and `queryid:*` execute policies for every test user (Task 6); egret now models the engines (plugin 2.8.0 on Trino,
2.9.0 on StarRocks), so registration goes through the supported path; the Trino tag service and the stub warehouse
rows still need the round 1 SQL deviations (DEV-1, DEV-2) because nothing in egret changed for them.

**Trino 483 (ranger plugin 2.8.0), all cells predicted SAME as Trino 470:**

| cell | prediction |
|---|---|
| 1.T1 | same: `CreateView`/`create` on the view name, identical to `CreateTable`; per-column `select` on the base for the creator; `CreateViewWithSelectFromColumns` asked at read time as the owner |
| 1.T2 | same: deny `create` on `*` blocks CREATE VIEW, CREATE TABLE, CTAS together |
| 1.T3 | same: `v_*` pattern blocks by name only; `innocent` is created |
| 1.T4 | same: `CREATE OR REPLACE VIEW` denied by the `create` deny; rename allowed (`alter`); SET AUTHORIZATION asks `alter` |
| 1.T5 | same: uncleared creator denied over a tagged column once the tag cache has refreshed (F-6 delay may differ in length, I predict the same order, tens of seconds to a minute) |
| 2.T1, 2.T2 | same: a path deny / mask on the view column applies to a read of the view, as the reader |
| 2.T3 | same: the base check still runs as the owner |
| 2.T4 | same: egret accepts any string coordinate with no existence check (no egret change in this branch touches it) |
| 2.T5 | same: a tag on a view column behaves like the path policy |
| Q3 memory (8 cells) | same: `vd_u` denied/denied, `vd_c` allowed/**allowed (leak)**, `vi_u` allowed/denied, `vi_c` allowed/denied |
| Q3 Iceberg | same as memory if `TESTING_FILE_METASTORE` still exists in 483; if the property was removed this is a labelled deviation (another catalog type) or `not run` |
| 3.R | same: DEFINER evaluates base mask and row filter as the owner (bypass when the owner is not listed); INVOKER follows the reader |
| 3.C | same: no session property or setting forces INVOKER; DEFINER stays the default |
| 3.V | same as 1.T5 |
| Q5 | same: both checks run, deny final, masks stack, second mask on one coordinate refused 409 |

**StarRocks 4.1.6 (ranger plugin 2.9.0):**

| cell | prediction | vs 4.1.4 |
|---|---|---|
| 1.S1, 1.S2 | same: `create view` and `create table` are separate access types; `create view` deny leaves `create table`, CTAS, DML working; `ALTER VIEW ... AS` and `SET SECURITY` still pass with `alter` only | same |
| 1.S3 | same: `CREATE MATERIALIZED VIEW` passes a `create view` deny | same |
| 2.S1 (a1) | **CHANGED**: a deny of `select` on resource `view=v_c` now bites (4.1.6 asks `policyType=0 select [view=v_c]`) | changed |
| 2.S1 (a2) | same: a column-level deny addressed `table=v_c column=secret` is never consulted for a view | same |
| 2.S1, user with no grant | **CHANGED**: `vc_nobody` is refused on a plain view (F-1 fixed) | changed |
| 2.S2 | same: a mask addressed at the view as a table applies; a base-column mask applies through views for the reader | same |
| 2.S3 | inferred only (egret cannot tag native StarRocks tables): same inference, not measured | same |
| 4.1 | deny not applied through a plain view for a user who holds the view grant: same. Base mask applies through the view: same | same |
| 4.2 | **CHANGED**: `SECURITY INVOKER` is enforced: `vc_plain` denied on `vi_u`/`vi_c`, `vc_cleared` allowed, a no-grant user denied (F-2 fixed). `SHOW CREATE VIEW` prints it as before | changed |
| 4.3 | same: MV carries neither base deny nor base mask; policies on its own columns apply; `vc_nobody` still refused on the MV, with new error text naming MATERIALIZED VIEW | same (text changed) |
| 4.4 | not run (not cheap), as before | n/a |
| Q5 StarRocks | same: base mask + view-level mask addressed as the view stack (`NULL`) | same |
| A1 to A5 (N, H) | same as the earlier 4.1.6 run; vs 4.1.4 the only difference is `vc_nobody` on views (refused instead of reading masked values) | changed for `vc_nobody` on views only |
| A6a, A6b, A10b | same: MV and cleared copies leak real values; **6b is not fixed** (uncleared owner's MV holds real values) | same |
| A6c | same: the MV rewrite happens and the mask is re-applied above the MV scan | same |
| A7 to A9, A8h | same (N leaks nothing through predicates; H leaks equality, cardinality, dictionary attack; hash length 128) | same |
| A10 | same (N: copy holds NULL; H: INSERT/CTAS fail on the `varchar(64)` target) | same |
| B (F-1, F-2, F-3-deny, F-3-mask) | F-1 and F-2 fixed; F-3-deny unchanged (plain view is a boundary); F-3-mask unchanged. Plugin jar is 2.9.0 | as the earlier 4.1.6 run |

**SP4d-4 live essentials (Trino 483 and StarRocks 4.1.6):** the 409 overlap refusal and the broader rule winning
live are predicted to hold on both engines (broad `MASK` wins over narrow `MASK_NULL`, as before). A table-stopping
`select` is denied at column depth on StarRocks (`count(*)` denied on `___count___`, as B-1 found) and on Trino
`count(*)` is predicted to be allowed with `select id` denied (Trino checks `select` per referenced column; a
column-less query asks no column), which would make the editor callout true on Trino and false on StarRocks (a
claim/engine difference, Critical by the brief's rule, repeated from B-1). One pruned migration: the preview says
equivalent and the engine answers the same before and after migrate and after undo, on both engines.

### 2.1 Cells that changed (read this first)

All of them are StarRocks, and all are the upstream fix `e0d97ad2659` / `2a36c2d0715` (`ColumnPrivilege.checkViewPrivilege`),
which 4.1.6 carries and 4.1.4 does not. **No Trino cell changed between 470 and 483.** The earlier document had already
run StarRocks 4.1.6 live (questions A and B), so "old" below is 4.1.4 and every cell agrees with that earlier 4.1.6 run.

| cell | StarRocks 4.1.4 | StarRocks 4.1.6 |
|---|---|---|
| F-1 / 2.S1: a user with no grant reads a plain view | reads real rows | `ERROR 5204 ... SELECT privilege(s) on VIEW v_u`; request `policyType=0 select [view=v_u] allowed=false` |
| 2.S1 (a1): deny `select` on resource `view=v_c` for a user who holds the view grant | ignored, rows returned | `ERROR 5204 ... on VIEW v_c` (policy matched, `allowed=false`) |
| F-2 / 4.2: `SECURITY INVOKER` view | accepted, printed, not enforced | enforced as the reader: `vc_plain` denied, `vc_cleared` reads, `vc_nobody` denied |
| 4.2c: `vc_viewonly` (select on views `v_u`, `vi_u` only) | reads both | reads `v_u`; `vi_u` denied on COLUMN `plain` (the INVOKER re-check of the view body) |
| 4.3: zero-grant user on a materialized view | `ERROR 5204 ... COLUMN id` | `ERROR 5204 ... MATERIALIZED VIEW mv_c` (still refused; text and checked object differ) |
| A1 to A9 for `vc_nobody` on every view kind | reads the masked values | refused with `ERROR 5204 ... VIEW <name>` (INVOKER: `COLUMN id`) |

Unchanged on purpose (and the reason spec 2 cannot lean on the fix): a plain (`SECURITY NONE`) view is still an access
boundary. `vc_plain`, holding the view grant, reads `secret` through `v_plain`, `v_nest`, `v_expr` under a base deny.
The materialized-view laundering (6a, 6b, 10b) is unchanged.

**Critical (page or preview versus engine): none found.** Every claim checked in section 3 equalled the engine.

### 2.2 Stack, versions proven by output, and every deviation

| piece | what ran |
|---|---|
| egret | `target/debug/egret serve` (PID 56917) on **:8096**, DB `egret_engup_live`, `EGRET_RANGER__ENABLED=true`, `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8096`, dummy storage env. The Task 6 admin password was not recorded, so `egret init --force` added `t8admin@egret.local` / `Admin12345!`. |
| Trino | `engup-trino`: `trinodb/trino@sha256:db58cc93...ccb0534` on :18090, `select version()` = `483`; `engup-trino-mig` (same digest, :18091) for the migration only. Connectors `mem` (memory) and `ice` (iceberg, `TESTING_FILE_METASTORE`, still accepted in 483). Plugin files exactly as `GET /engines/{id}/ranger-plugin-config` renders them, except poll interval 5000 (DEV-3). |
| StarRocks | `engup-sr`: `starrocks/allin1-ubuntu@sha256:5cbf09c4...333e5d` on :19031; `current_version()` = `4.1.6-6862092`; `fe/lib/ranger-plugins-common-2.9.0.jar`; `javap` on `fe-core-4.1.jar` lists `ColumnPrivilege.checkViewPrivilege(ConnectContext, TableName, View)`. Plugin files as rendered, poll interval 5000; `sys_log_verbose_modules` set for the Ranger request log. |
| engines in egret | the two external engines from Task 6, `rangerPluginVersion` 2.8.0 (Trino) and 2.9.0 (StarRocks), both `rangerPluginVersionModelled: true`. |
| users | engine users `vc_admin`, `vc_cleared`, `vc_plain`, `vc_owner_u`, `vc_owner_c` (+ StarRocks `vc_nobody`, `vc_viewonly`; SP4d-4 `alice`, `dave`, `erin`). Same clearance shape as before: deny `public`, `denyExceptions` the cleared users. |
| policies | all through `POST /api/control-plane/policies` in the editor shape, cloned from the stored bodies in the earlier measurement's database. Trino 483 needs the `trinouser:*` impersonate and `queryid:*` execute baseline for every user (Task 6). |

Deviations, all labelled:

- **DEV-1 (SQL), not needed after all.** I created a tag service of type `tag` (`t8-tag`) by SQL and pointed `engup-trino` at it, copying the earlier recipe. A separate check (`engup-trino.tagService` back at `engup-tag`, the Task 6 trino-typed profile) enforced the same tag deny: `vc_plain` denied `Cannot select from columns [id, secret]`, `policy=pol103`, the tag policy served with `serviceDef.name=trino`. So the earlier DEV-1 ("no route gives an external Trino a tag service") is **not** a blocker on this branch: a trino profile used as the tag service works. My own first run of the F-6 timer was polluted by a wrong tag name in a cloned policy, not by the service type.
- **DEV-2 (SQL), still needed.** A stub `storage.storage_connections` row, a stub `catalog.warehouses` row and a registry catalog `mem` (kind iceberg, `metastore.type=egret`) so a tag assignment can address the memory connector. The catalog's `config` and `advanced` had to be trimmed to pass the attach validation (the earlier recipe's `engines` key and `connector.name` are now refused).
- **New egret behaviour learnt while doing DEV-2 (silent):** a tag assigned BEFORE the catalog is attached to the engine does not reach the engine, and the tag download stays at `tagVersion 1` with no resources; attaching does not bump the version. Only a re-PUT of the assignment bumped it (to 2) and made the download carry the coordinate. No error anywhere.
- **DEV-3 (config):** poll interval 5000 instead of 30000, both engines.
- **DEV-SP-1 and DEV-SP-2 (SQL), section 3:** the stored overlap pair (as in SP4d-4) and the stored table-stopping `select` on Trino.
- **DEV-SP-3 (SQL):** `UPDATE policy.authz_services SET origin='manual'` for `engup-sr` after the SP4d-4 compile, so the hand-written policies of the StarRocks rerun could be written. It was a scratch-DB convenience.
- **Created `t8-tag-mig` (type tag) and `t8-trino-mig`:** the migration needed a profile small enough for the proof cap, section 3.
- **Harness finding, StarRocks 4.1.6: the FE runs TWO Ranger plugin instances** (two `PolicyRefresher` threads, `Start Ranger plugin ... success` logged twice). A sync that waits for the first "Switched policy engine to [N]" can return while the other instance still serves version N-1. It produced one stale read (a `NULL` mask after the mask policy was deleted). All StarRocks cells below come from the rerun with a sync that waits for both threads; the rerun matched the first run cell for cell (`diff` of the non-request lines: only query ids and tablet ids differ).
- **One rerun cell is mixed:** in the rerun of 1.S2's baseline, `create view v1` answered `Table 'v1' already exists` (left by an aborted earlier attempt); the clean result for that one statement is from the first run. Nothing depends on it.

### 2.3 Trino 483 (ranger plugin 2.8.0) against Trino 470 (2.5.0)

Mem connector unless marked. `old` is the earlier document; `new` was observed with the stack above.

| cell | old (470) | new (483) | changed? |
|---|---|---|---|
| 1.T1 | `CreateView`/`create` on the view name = `CreateTable`; per-column `select` for the creator; `CreateViewWithSelectFromColumns` only at READ time | identical: `action=CreateView ... access=create policy=pol26`, then `SelectFromColumns` on `plain`, `id`, `secret` of the base; CTAS `CreateTable` | no |
| 1.T2 | deny `create` on `*` blocks CREATE VIEW, CREATE TABLE, CTAS | `Cannot create view v2`, `Cannot create table t3`, `Cannot create table t4` | no |
| 1.T3 | `v_*` pattern blocks by name; `innocent` created | `v_a`, `v_t` denied; `innocent` created and `select id from innocent` returned rows | no |
| 1.T4 | OR REPLACE denied by the `create` deny; RENAME allowed (`alter` x2); SET AUTHORIZATION `alter` then refused by the connector | identical (`Catalog does not support permission management: mem`); `COMMENT ON VIEW` `alter`; `DROP VIEW` `drop` | no |
| 1.T5 | uncleared creator denied over a tagged column after the tag cache refreshed | `vc_plain`: `Cannot select from columns [id, secret] in table or view base`; `vc_cleared` created the view | no |
| F-6 tag-cache delay | about 60 s (one sample) | **30 s** (one sample, clean unassign then assign, polled every 1 s; unassign took 21 s to lift) | not a behaviour change; one sample each |
| 2.T1 | path deny on `mem.s.vd_c.secret` applies to the view | `Cannot select from columns [id, secret] in table or view vd_c`; `select id` returned rows | no |
| 2.T2 | path mask on a view column applies as the reader | `[[1,'xxxx-0'],[2,'xxxx-0']]` via the view; the same user got `open-1/2` from `base` | no |
| 2.T3 | the base check still runs, as the OWNER | reading DEFINER `vd_u` as `vc_cleared`: view-level allowed, then `View owner 'vc_owner_u' cannot create view that selects from base`, request `as=vc_owner_u action=CreateViewWithSelectFromColumns [column=secret table=base] allowed=false` | no |
| 2.T4 | PUT resource-tags accepts a view coordinate and a coordinate that exists nowhere (200 both); UI Tags button on view columns (source) | both 200; `BrowseTab.tsx` view-column Tags button unchanged (source read, not clicked) | no |
| 2.T5 | tag on a view column denies/masks like the path policy | tag download carried `table=vd_c column=secret`; `select id, secret from vd_c` denied for `vc_cleared`; tag mask `xxxx-0` | no |
| Q3 DEFINER `vd_u` (uncleared owner) | DENIED / DENIED | `vc_cleared` DENIED, `vc_plain` DENIED (`View owner 'vc_owner_u' cannot create view that selects from base`) | no |
| Q3 DEFINER `vd_c` (cleared owner) | ALLOWED / **ALLOWED (leak)** | `vc_cleared` rows, `vc_plain` rows `[[1,'classified-1'],[2,'classified-2']]` | no |
| Q3 INVOKER `vi_u` | ALLOWED / DENIED | `vc_cleared` rows, `vc_plain` `Cannot select from columns [plain, id, secret] in table or view base` | no |
| Q3 INVOKER `vi_c` | ALLOWED / DENIED | same as `vi_u` | no |
| Q3 Iceberg (`ice`, path deny) | the same 8 cells | the same 8 cells (`TESTING_FILE_METASTORE` still works) | no |
| 3.R | DEFINER base mask/row filter evaluated as the OWNER (bypass when the owner is not listed); INVOKER follows the reader | `wd_u` (owner listed): admin and `vc_plain` both `[[2,'xxxx-0']]`; `wd_c` (owner not listed): `vc_plain` `[[1,'open-1'],[2,'open-2']]`, unmasked and unfiltered; `wi_u`/`wi_c`: admin unmasked, `vc_plain` `[[2,'xxxx-0']]` | no |
| 3.C | no setting forces INVOKER or refuses DEFINER | `SHOW SESSION` (152 rows) has no row matching view, invoker, definer or security; the iceberg catalog has none either; default stays DEFINER | no |
| 3.V | covered by 1.T5 | same | no |
| Q5 | both checks run, deny final; masks stack; second mask on one coordinate refused 409 | base `MASK` then view-level `MASK_SHOW_FIRST_4` gave `[[2,'xxxxXX']]` (stacked); second mask: `409 rule_overlap` | no |

### 2.4 StarRocks 4.1.6 (ranger plugin 2.9.0) against 4.1.4 (2.8.0): round 1

`vcdb.base(id, secret varchar(20), plain varchar(20))`, deny on `base.secret` (public, exceptions `vc_admin`, `vc_cleared`, `vc_owner_c`) and path mask `MASK` on `base.plain` for `vc_plain`, `vc_owner_u`; views created before the policies.

| cell | old (4.1.4) | new (4.1.6) | changed? |
|---|---|---|---|
| 1.S1 | `create view` and `create table` are separate access types on `[catalog database]` | `access=create view policy=30`, `access=create table policy=30` | no |
| 1.S2 | deny `create view` leaves `create table`, CTAS, DML; OR REPLACE refused; `ALTER VIEW AS` and `SET SECURITY` pass on `alter` | `CREATE VIEW` and `OR REPLACE` -> `ERROR 5204 ... CREATE VIEW privilege(s) on DATABASE vcdb`; create table, CTAS, insert, select worked; `alter view ... as` and `set security invoker` succeeded (`access=alter view=v_existing`) | no |
| 1.S3 | `CREATE MATERIALIZED VIEW` passes the deny and copies the data | succeeded (`access=create materialized view policy=30`), `select * from mv1` returned `id, secret` rows to `vc_plain` | no |
| 2.S1 (a1) | deny on `view=v_c` ignored | **denied** (`policy=110`, `on VIEW v_c`) for `vc_cleared`; `vc_plain` (no such deny) still reads | **yes** |
| 2.S1 (a2) | column deny addressed `table=v_c column=secret` ignored | still ignored: `vc_cleared` read `classified-1/2` | no |
| 2.S1 zero-grant | `vc_nobody` reads `v_u`, `vi_u` | refused (F-1 fixed) | **yes** |
| 2.S2 | mask addressed `table=v_c column=plain` applies; base mask applies through views for the reader | `vc_cleared`: `xxxx-0` via `v_c`, `open-1/2` from `base`; `vc_plain`: `xxxx-0` through `v_u` | no |
| 2.S3 | inferred, not measured | inferred again (egret cannot tag native StarRocks tables); not measured | n/a |
| 4.1 | deny not applied through plain views; mask is | `vc_plain` reads `secret` through `v_u`, `v_c`; `plain` is `xxxx-0` through `v_u` | no |
| 4.2 | `SECURITY INVOKER` accepted, printed, not enforced | **enforced**: `vc_plain` -> `vi_u`/`vi_c` denied on COLUMN secret; `vc_cleared` reads; `SHOW CREATE VIEW` prints `SECURITY INVOKER`. A table-level-style deny (column `*`) on `base` for `vc_plain` blocks `base` and `vi_u` but not `v_u` or `mv_c` | **yes** |
| 4.3 | MV carries neither deny nor mask; own-column policies apply | `vc_plain` reads `secret` and `plain` unmasked from `mv_c`; `vc_nobody` refused (`MATERIALIZED VIEW mv_c`) | no (error text only) |
| 4.4 | not run | not run | n/a |
| Q5 | view-level mask on top of base mask (`MASK_NULL`) shows `NULL` | `NULL` through `v_u`; `vi_u` now denied (INVOKER); `base` unchanged `xxxx-0` | no (INVOKER now enforced) |

### 2.5 StarRocks 4.1.6: round 2, question A (clearance as a mask) and question B

`vcdb.base(id, secret varchar(64), plain varchar(64))` rows `(1,'classified-1',..)`, `(2,'classified-2',..)`, `(3,'classified-1',..)`; table `mine`; objects `v_plain`, `v_expr`, `v_nest`, `vi_inv` (INVOKER), `mv_c` (cleared owner), `mv_u` (uncleared `vc_plain`, built while the mask was on). Mask policy: cleared users `MASK_NONE`, group `public` the mask under test. `N` = `MASK_NULL`, `H` = `MASK_HASH`. "old" is the earlier 4.1.4 observation, with the earlier 4.1.6 run in brackets where it differs.

| # | cell | old (4.1.4) | new (4.1.6) | changed? |
|---|---|---|---|---|
| 1 | direct read | N: uncleared `NULL` x3, cleared real, nobody 5204; H: 128-char hash, rows 1 and 3 equal | same | no |
| 2 | plain view | uncleared masked; **nobody reads it, masked** | uncleared masked; **nobody refused** (`VIEW v_plain`) | **yes for nobody** |
| 3 | `v_expr` | `NULL` in `u,c,s` (N); hash-derived, no plaintext (H); nobody reads | same for `vc_plain`/`vc_cleared`; nobody refused | yes for nobody |
| 4 | nested `v_nest` | as 2 | as 2 | yes for nobody |
| 5 | INVOKER `vi_inv` | as 2, nobody reads | `vc_plain` `NULL` (N); nobody refused (`COLUMN id`) | yes for nobody |
| 6a | `mv_c` (cleared owner) read by uncleared | LEAK real | LEAK `classified-1,2,1` (N and H) | no |
| 6b | `mv_u` (uncleared builder) | LEAK real | LEAK `classified-1,2,1`: **not fixed** | no |
| 6c | MV rewrite with the mask on | rewrite happens; mask re-applied above the MV scan | `explain` for `vc_plain` scans `mv_c` with `<slot 5> : NULL` projected; for `vc_cleared` scans `mv_u` without the projection (which MV is chosen differs from the earlier run; the mechanism is the same) | no |
| 7 | `count(distinct)`, `min`, `max(length)` | N `0, NULL, NULL`; H `2, hash, 128` | same | no |
| 8 | `=`, `LIKE`, `ORDER BY`, `GROUP BY` | N: nothing; H: groups of 2 and 1 | same | no |
| 8h | `where secret = hex(sha2('classified-1',256))` | N none; H rows 1, 3 | same (H returns ids 1 and 3: the dictionary attack works) | no |
| 9 | join against `mine`, plain and hashed | N none; H the hashed join matches ids 1,2,3 | same | no |
| 10 | CTAS / INSERT..SELECT as uncleared | N copy holds `NULL`; H both fail `Insert has filtered data` | same | no |
| 10b | cleared CTAS, uncleared reads the copy | LEAK real | LEAK real; nobody refused | no |
| side | nobody `select ... where secret = 'x'` on `base` under N | empty set, no 5204 | empty set, no 5204 | no |

Question B, same objects and users on 4.1.6 (the 4.1.4 column is the earlier document's):

| setting | object / user | 4.1.4 | 4.1.6 observed |
|---|---|---|---|
| deny only | `v_plain`, `v_nest` / `vc_nobody` | reads | `ERROR 5204 ... VIEW v_plain` / `VIEW v_nest` |
| deny only | `vi_inv` / `vc_nobody` | reads | `ERROR 5204 ... COLUMN id` |
| deny only | `vi_inv` / `vc_plain` | reads | `ERROR 5204 ... COLUMN secret`; `vc_cleared` reads |
| deny only | `v_plain`, `v_nest`, `v_expr` / `vc_plain` | reads | **still reads** (`classified-1,2,1`; `CLASSIFIED-1` for `v_expr`) |
| mask N only | four views / `vc_plain` | `NULL` | `NULL` (INVOKER included) |
| mask N only | `vc_nobody` on the four views | reads `NULL` | refused |
| any | `mv_u` / `vc_plain` | real rows | real rows (6b unfixed) |
| plugin jar | `ranger-plugins-common` | 2.8.0 | **2.9.0**, `Switched policy engine` served by two refresher threads |

### 2.6 What spec 2 (view governance) should assume

1. **Trino: nothing changed in 483.** CREATE VIEW cannot be told apart from CREATE TABLE (`create` on the table resource, no mode flag), so a Ranger policy cannot refuse a DEFINER view and DEFINER stays the default. The only enforcing form is `SECURITY INVOKER` (all four INVOKER cells correct, on memory and Iceberg). A cleared service owner does not fix DEFINER (`vd_c` leaks); an uncleared owner over-blocks everybody (`vd_u`). Base masks and row filters follow the owner under DEFINER (the `wd_c` bypass).
2. **A tag on a view column works end to end on Trino** (API-only unless the UI Tags button is used), with the lineage that decides which view column inherits a tag still missing. The tag cache lags the policy poll: 30 s in one clean sample.
3. **StarRocks 4.1.6 makes INVOKER views the vehicle for a base-column deny** and refuses a zero-grant reader of any view; both are new relative to 4.1.4. Do not assume it on an engine older than 4.1.6, or on an egret that still declares 2.8.0 (egret now stores the exact image, so check the pair before relying on it).
4. **A plain StarRocks view is a boundary, not a carrier:** a reader with the view grant bypasses a base deny. A mask-shaped clearance (`MASK_NULL`) is the only base-column policy that holds through every view kind, for every reader, and it leaks nothing through predicates, aggregates or joins; `MASK_HASH` leaks equality, cardinality and (with a guess) content; the 128-character hash does not fit a `varchar(64)` target.
5. **Materialized views and copies launder either clearance** (6a, 6b, 10b are unchanged). `create materialized view` is its own access type and must be denied for any non-cleared user if a mask is the clearance; an MV built by the uncleared builder still holds real values, because the refresh runs as the builder without a mask lookup.
6. **egret behaviours that bite a governance feature:** (a) the first compile (a rule or a migration) flips the profile to `generated` and every later hand-written write answers 409; (b) the migration proof groups every profile that shares a tag service, so a small profile sharing a tag service with a 30-policy profile was refused by the work cap (25,740 probes, 1,472,580 units) until it had its own tag service; (c) a tag assignment made before its catalog is attached to the engine is silently dropped from the download; (d) the registry cannot hold a `default_catalog`, so native StarRocks tables cannot carry a tag (from the earlier document; not re-tested here); (e) StarRocks FE applies a policy version on two plugin instances that do not switch together, so a decision can use the older instance for up to one poll after a write.
7. **Trino 483 default-denies the `trinouser` impersonate and `queryid` execute resources for every user**, including a user acting as themselves; a profile of table policies alone serves nobody. (Not new, restated because spec 2's fixtures need it.)

## 3. SP4d-4 live essentials on Trino 483 and StarRocks 4.1.6

Done through egret's API (the same routes the UI calls); the UI was not driven, so "what a page says" below means the API field the page renders (`GET /perm/overlaps`, `GET /perm/policies/services/{id}/warnings`, the `409` body, the migrate `dry_run` body). Profiles: `engup-trino` (perm service 1) and `engup-sr` (2); a small `t8-trino-mig` (5) for the Trino migration.

| essential | Trino 483 / plugin 2.8.0 | StarRocks 4.1.6 / plugin 2.9.0 | claim = engine? |
|---|---|---|---|
| 409 overlap refusal | rule A `mem.s.*.plain` `MASK` accepted (compile created 1 policy); rule B `mem.s.base.plain` `MASK_NULL`: `POST /perm/preview` and `POST /perm/rules` both **409** `rule_overlap`, naming both rules and "Today Ranger serves rule t8-rule-a-broad" | rule A `lake.sales.*.email` accepted; rule B `lake.sales.orders.email`: **409**, same text with `database=[sales]` | the sentence names the broad rule; see next row |
| the broader rule wins live | with a stored hand-written pair (DEV-SP-1: policy row inserted, version bumped), `GET /perm/overlaps` says `servedToday` = rule A. Engine: `vc_plain` and `vc_admin` read `[[1,'xxxx-0'],[2,'xxxx-0']]` (MASK), request `getColumnMask ... allowed=true policy=pol62` (rule A), not NULL; on `mem.s.other` (only A matches) the same | `servedToday` = rule A (policy 101). Engine: `alice` reads `x@x.xxx` x2 and the mask lookup is `column=email ... allowed=true policy=101`, not NULL. After deleting the pair, `a@x.com`, `b@y.org` | yes, both |
| table-stopping `select` denied at column depth | stored `mem.s.base` (no column key) for `erin`; warnings API: "stops at `table` and allows `select` ... grants only the checks made at the depth it stops at (... Trino's `count(*)`)". Engine: `select count(*)` returned `[[2]]` (`SelectFromColumns [table=base] allowed=true policy=pol64`); `select id` -> `Cannot select from columns [id]`; `select *` denied | stored `lake.sales.orders` (no column) written through the API (200, `resource_path lake.sales.orders`); same warning text. Engine: `select count(*)` -> `ERROR 5204 ... COLUMN ___count___`; `select id` -> `COLUMN id`; `select *` denied | yes: the sentence limits the `count(*)` allowance to Trino, and StarRocks denies it |
| one pruned migration, preview = engine, with undo | `t8-trino-mig`, hand-written `select mem.s.base.*` for `dave`. Dry run 200 `equivalent: true`, `probes 1200`, `probes_full 5880`, 0 disagreements, 0.74 s. Migrate 200 committed 0.85 s. Undo 200 (`grants_deleted 1`) 0.12 s. `dave`'s answers before, after migrate, after undo are identical: `select id, plain` rows, `count(*)` `[[2]]`, a missing table error | `t8-sp-sr-dave-orders`, `select lake.sales.orders.*` for `dave`. Dry run 200 `equivalent: true`, `probes 1620`, `probes_full 18468`, 0 disagreements, 1.57 s. Migrate 200 committed 1.37 s. Undo 200 0.32 s. `dave`'s answers identical in all three states: `a@x.com`/`b@y.org` rows, `count(*)` 2, `lake.sales.items` denied on COLUMN id | yes, both |

Two refusals worth keeping for the same cap: on `engup-trino` (30 policies) the first dry run answered **422** `29700 probe evaluations ... 1782000 units of work ... over the limit of 1300000`; the 10-policy `t8-trino-mig` still answered 422 (25,740 probes, 1,472,580 units) until it stopped sharing a tag service with `engup-trino`, then fit at 1,200 probes.

### 3.1 Defects and observations in this task

| id | severity | what |
|---|---|---|
| none | | No difference between what an egret page, preview or API sentence claims and what either engine does. No Critical. |
| T8-1 | Info (egret) | the first compile flips a profile to `generated`; later hand-written writes answer 409 `profile N is compiled by the SP4a permission model`. Documented behaviour, listed because every live recipe has to order its steps around it. |
| T8-2 | Minor (egret) | a tag assignment made before the catalog is attached to the engine is dropped silently from the tag download; the version does not move until the assignment is re-PUT. |
| T8-3 | Minor (egret) | the migration proof cap counts every profile that shares a tag service with the target; a small profile can be refused because of a large neighbour (error text says "across 2 profile(s)" and "the largest serves 30 policies"). |
| T8-4 | Info (StarRocks) | the 4.1.6 FE runs two Ranger plugin instances that switch policy versions seconds apart; one stale read observed. |
| T8-5 | Info (out of scope, not hit) | `POST /policies` silently stores a non-editor body with empty resources (known; every body here used the editor shape). |

### 3.2 Left behind

Stopped and removed by PID or name of my own containers: egret (PID 56917, :8096), containers `engup-trino`, `engup-trino-mig`, `engup-sr`. Kept: database `egret_engup_live` in `egret-dev-postgres-1` (profiles `engup-trino`, `engup-sr`, `engup-tag`, `t8-tag`, `t8-trino-mig`, `t8-tag-mig`; the policies, rules, tags, stub warehouse rows and registry catalogs `mem` and `lake` above; two external engines plus `t8-trino-mig`), the pulled `starrocks/allin1-ubuntu:4.1.6` image and `trinodb/trino:483`. The raw harness output stays in the session scratchpad (not committed). Never touched: DB `egret`, the `egret-dev-*` containers, `egret-w7c-postgres`, ports 5173, 5000, 8080.
