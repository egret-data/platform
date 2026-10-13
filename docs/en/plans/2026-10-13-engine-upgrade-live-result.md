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

> **Status of this section while it holds only 2.0:** these are the predictions, committed BEFORE any cell was
> observed. Observations replace the "prediction" column's neighbours in a second commit. A wrong prediction is a
> finding; no prediction is edited afterwards.

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
