# View clearance measurement: can SQL views carry or lose a column clearance?

> **Status:** predictions (section 1) were committed FIRST as `176eac116`; observations (sections 2 to 5)
> are added in a second commit. A wrong prediction is a finding; no test was retuned to match a
> prediction. Canonical (English). Measurement only; no product code changed.
> Measured 2026-10-13 (UTC).

## 0. Method and scope

Branch `docs/view-clearance-measure` off `origin/main` `f5bbbd879`. Engines: Trino **470**
(`ranger-plugins-common-2.5.0`, plugin `trino-ranger-470`) with the `memory` connector, and StarRocks
**4.1.4** (`ranger-plugins-common-2.8.0`) with native internal tables. egret serves the Ranger
policies and tag download to both engines (egret IS the Ranger Admin). Protocol: predictions
committed, then observations committed separately.

Fixed vocabulary used below:

- **cleared** user: holds the clearance (deny exception on the tagged base column).
- **uncleared** user: does not.
- **owner**: the user who ran `CREATE VIEW`.
- **base deny**: tag deny on `secret` of the base table, `denyExceptions` = cleared users.

Source facts read before predicting (the predictions rest on them; observation tests them):

- Trino 470 `RangerSystemAccessControl`: `checkCanCreateView` -> `hasPermission(createTableResource(view), CREATE)`,
  the SAME resource shape and access type as `checkCanCreateTable`; `checkCanCreateViewWithSelectFromColumns`
  -> `SELECT` on each base column for the CREATOR; `checkCanRenameView`, `SetViewAuthorization`, `SetViewComment`
  -> `ALTER`; `checkCanDropView` -> `DROP`. The SPI call `checkCanCreateView(context, view)` carries no
  DEFINER/INVOKER flag.
- StarRocks (source tree `../starrocks`): `visitCreateViewStatement` -> `checkDbAction(CREATE_VIEW)` on the
  DATABASE, then the query check. The grammar has `CREATE VIEW ... SECURITY (NONE | INVOKER)` and
  `ALTER VIEW ... SET SECURITY`; `ColumnPrivilege.checkViewPrivilege` re-checks the view's query when
  `view.isSecurity()`. Whether the 4.1.4 image contains this is unproven; Q4 measures it.
- egret `PUT /api/control-plane/warehouses/{id}/resource-tags` takes free-text `schema`, `table`, `column`.

## 1. Predictions

### Q1. Can CREATE VIEW be denied without denying what the user legitimately does?

| id | engine | prediction |
|---|---|---|
| 1.T1 | Trino | `CREATE VIEW v` makes a `create` request on `{catalog,schema,table=v}`, indistinguishable from `CREATE TABLE v` / `CTAS v`. Also a `select` request per base column (CreateViewWithSelectFromColumns). |
| 1.T2 | Trino | A deny of `create` on table `*` blocks CREATE VIEW **and** CREATE TABLE and CTAS together. No Ranger policy separates VIEW from TABLE creation by access type. |
| 1.T3 | Trino | A deny of `create` on table `v_*` blocks only names matching it, so it blocks `CREATE VIEW v_x` and also `CREATE TABLE v_x`, and a view named `x` is created freely. Naming convention is the only lever and is trivially bypassed. |
| 1.T4 | Trino | `CREATE OR REPLACE VIEW` on an existing view hits the same `create` check (no `drop` check). `ALTER VIEW RENAME` hits `alter` on source and target. `ALTER VIEW ... SET AUTHORIZATION` hits `alter`. Denying `create` does not stop the rename of an existing view. |
| 1.T5 | Trino | A user who is denied `select` on the secret column cannot create a view that selects it (`Cannot create view ... with select from`), because of 1.T1's per-column request. This is a partial defence against UNCLEARED creators only. |
| 1.S1 | StarRocks | `CREATE VIEW` makes a `create view` request on the `database` resource. `CREATE TABLE` makes `create table`. They are separable. |
| 1.S2 | StarRocks | Denying `create view` for a user while allowing `create table`, `select`, `insert` works. `CREATE OR REPLACE VIEW` takes the same check. `ALTER VIEW ... AS` takes an `alter` request on the view object and is not blocked by a `create view` deny, but it needs an existing view. |
| 1.S3 | StarRocks | A user can still create a MATERIALIZED VIEW (own access type `create materialized view`) unless that is denied too. |

### Q2. Does a policy on a VIEW column apply when the view is queried?

| id | engine | prediction |
|---|---|---|
| 2.T1 | Trino | A path-based column deny on `mem.s.v.secret` applies to a query of `v` (view columns are checked with `checkCanSelectFromColumns` on the view name, as the invoker). |
| 2.T2 | Trino | A path-based mask on `mem.s.v.secret` applies to a query of `v`. |
| 2.T3 | Trino | The base-table check (as the view OWNER, DEFINER view) still runs in addition. A base deny with the owner uncleared fails the query even when the view-level policy allows. |
| 2.T4 | Trino | egret's resource-tag API accepts `table = <view name>` and `column = <view column>`: the coordinates are free text with no existence check, and the tag download then carries the view coordinate. The UI browse cannot offer a view (it lists tables), so this is an API-only path. |
| 2.T5 | Trino | A tag assigned to the view column and served through the tag download behaves exactly like 2.T1/2.T2 (deny/mask applies to the view). |
| 2.S1 | StarRocks | A deny on the VIEW column `db.v.secret`: the check for a view is `SELECT` on the VIEW object only (`checkViewAction`), never per column, so a column-level deny on the view column does NOT apply. A deny on the view object does. |
| 2.S2 | StarRocks | A mask on the view column `db.v.secret` does not apply either (masks are looked up per scanned base table column). |
| 2.S3 | StarRocks | A tag on the view column is not honoured (same reason). |

### Q3. Trino DEFINER vs INVOKER, owner x reader. Base deny on `secret`, exception = cleared.

| view | owner | reader cleared | reader uncleared |
|---|---|---|---|
| DEFINER | uncleared | DENIED ("View owner ... cannot create view that selects from") | DENIED |
| DEFINER | cleared | ALLOWED | **ALLOWED (leak)** |
| INVOKER | uncleared | ALLOWED | DENIED ("Cannot select from columns") |
| INVOKER | cleared | ALLOWED | DENIED |

Other Q3 predictions:

- 3.R: a base-table row filter and column mask apply under INVOKER with the READER's identity, and under DEFINER with the OWNER's identity.
- 3.C: Trino has no config or session property that forces INVOKER or refuses DEFINER. `CreateViewTask` hard-codes DEFINER; the only refusal is `Catalog does not support run-as DEFINER views`, for catalogs with connector-managed security, and the system Ranger access control does not make a catalog that kind. An Iceberg-on-memory confirmation cell gives the same answer as the memory connector.
- 3.V: an uncleared user cannot CREATE even an INVOKER view over the denied column while the deny stands (see 1.T5).

### Q4. StarRocks

| id | prediction |
|---|---|
| 4.1 | Re-measure: a base-column deny AND a base-column mask do not apply through a plain view. The SP4c claim holds on 4.1.4 / 2.8.0. |
| 4.2 | The 4.1.4 image DOES accept `CREATE VIEW ... SECURITY INVOKER` (source tree has it), and `SHOW CREATE VIEW` prints it. With it, the base-column deny applies through the view for an uncleared reader and a cleared reader passes. Confidence: moderate (the image may predate the feature). |
| 4.3 | A materialized view does not carry the base deny either (its own `select` on the MV object only). Report briefly. |
| 4.4 | A view over an external Iceberg table behaves like 4.1 (only if cheap). |

### Q5. Precedence

- View-level and base-level policy BOTH run; there is no precedence. The query succeeds only if every check allows. Within a single check Ranger's deny beats allow.
- Masks stack: a view-column mask and a base-column mask both apply (double masking), so a view-level mask cannot UNDO a base-level mask, and a base-level mask cannot be skipped by DEFINER; it is evaluated as the owner.
- Surprise expected: on Trino the base check uses the OWNER identity, so the `denyExceptions` list is consulted for the owner and not the reader.

---

# OBSERVATIONS (second commit)

## 2. Setup, with versions proven by output

| piece | what ran |
|---|---|
| egret | worktree `egret-sp4a`, `cargo build -p egret-bin` (11 m 49 s), `target/debug/egret serve` on **:8095**; DB **`egret_viewclr_live`** in `egret-dev-postgres-1`, URL passed explicitly; `EGRET_RANGER__ENABLED=true` (without it no service-defs are seeded and `POST /policies/services` answers 500); `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8095`; admin from `egret init` (`admin@egret.local`) |
| Trino | `docker run viewclr-trino` from `trinodb/trino:470` (pulled; the local `latest` is 483/ranger 2.8.0 and was NOT used). `select version()` = `470`. `plugin/apache-ranger/` holds `io.trino_trino-ranger-470.jar` and `org.apache.ranger_ranger-plugins-common-2.5.0.jar`. Port 18080. Connectors `memory` (`mem`) and `iceberg` (`ice`, `TESTING_FILE_METASTORE`, `file:///tmp/ice`). HTTP, no authentication, user chosen per request (`X-Trino-User`) |
| StarRocks | `docker run viewclr-sr` from `starrocks/allin1-ubuntu:latest`; `select current_version()` = **`4.1.4-4a9848e`**; `fe/lib/ranger-plugins-common-2.8.0.jar`. FE query port published as 19130 |
| registration | both engines registered through `POST /api/control-plane/engines/external`; plugin files taken from `GET /engines/{id}/ranger-plugin-config` and installed by hand (Trino: `access-control.properties` + `ranger-trino-security.xml`; StarRocks: the three files + `access_control = ranger`) |
| debug evidence | Trino `log.properties`: `org.apache.ranger.plugin.policyengine=DEBUG` (the engine's `evaluatePolicies` line carries resource, accessType, user, `action=`, policyType, isAllowed, policyId). StarRocks `fe.conf`: `sys_log_verbose_modules = org.apache.ranger.plugin.policyengine,com.starrocks.authorization.ranger` |
| disk | `df -h /` free: 97 GiB before, 61 GiB at the end of measurement (cargo build and the 2.8 GB Trino 470 image); after cleanup see section 6 |

Users (engine users, not egret users): `vc_admin`, `vc_cleared`, `vc_plain` (uncleared reader), `vc_owner_u` (uncleared view owner), `vc_owner_c` (cleared view owner), plus StarRocks-only `vc_viewonly` and `vc_nobody` (no grant at all).

### Every write, and the three labelled deviations

All policy writes went through `POST /api/control-plane/policies` (editor shapes `access`, `masking`, `rowfilter`, `tag-access`, `tag-masking`) on two **manual** profiles: `vctrino` (id 6, `config.tagService = vctrino_tag`) and `vcsr` (id 7). Tags through `POST /tags` and `PUT/DELETE /warehouses/{id}/resource-tags`. Clearance was expressed as the SAME policy shape the SP4c compiler emits (tag policy, `deny` for group `public`, `denyExceptions` = the cleared users); **the `PUT /tags/{id}/access-policy` clearance route was NOT used** (it needs egret users and roles, compiles into a *generated* profile that then refuses hand-written allow policies, and its StarRocks half targets registry catalogs, which cannot be `default_catalog`). That is a scope choice, not a finding about the route.

- **DEV-1 (SQL).** `INSERT INTO policy.authz_services (name,type,config_jsonb) VALUES ('vctrino_tag','tag','{}')`. There is no supported route for an EXTERNAL Trino to obtain a tag service: `POST /policies/services` refuses `type: "tag"` ("type must be trino or starrocks"), external registration binds only an existing profile whose `tagService` resolves, and the only code that creates `<name>_tag` is the managed-engine create path (`bind_trino_profile`, needs Kubernetes). The Trino profile itself was then created through the API with `config.tagService`.
- **DEV-2 (SQL).** A stub `storage.storage_connections` row, a stub `catalog.warehouses` row, and `UPDATE registry.catalogs SET kind='iceberg', warehouse_id=id, config=...metastore{type:egret}` for the registry catalog `mem`. Reason: `tag_assignments` join `registry.catalogs ON c.warehouse_id = a.warehouse_id`, and the CHECK `catalogs_warehouse_mirrors_config` allows a `warehouse_id` only on an `iceberg` catalog with `metastore.type = egret`. A `generic` catalog (memory connector) therefore can never receive a tag download. The tag *assignment* PUT accepted the catalog id anyway (no FK), so the first attempt silently produced an empty tag download (`serviceResources: []`).
- **DEV-3 (config).** Trino `ranger.plugin.trino.policy.pollIntervalMs` set to 5000 instead of the generated 30000, to shorten waits. The tag refresher is a separate timer (see finding F-6).
- **Harness incident.** I ran `pkill -x egret` once to restart my own server. Nothing was listening on :8080 afterwards; I could not prove no other egret process existed before. Later kills used PIDs only.

## 3. Results

Legend: PRED = prediction id in section 1, OK = prediction held, WRONG = prediction wrong (a finding).

### Q1. Can a Ranger policy stop CREATE VIEW without stopping what the user legitimately does?

| id | engine | observed | verdict |
|---|---|---|---|
| 1.T1 | Trino | `CREATE VIEW mem.s.v1 AS ...` as `vc_plain`: `action=CreateView resource=[catalog=mem schema=s table=v1] access=create` (policy 2). `CREATE TABLE`: `action=CreateTable ... table=t1 access=create`; CTAS: the same `CreateTable ... access=create`. The creator's per-column checks are `action=SelectFromColumns ... access=select` on the base columns. **`CreateViewWithSelectFromColumns` is NOT asked at create time**; it is asked at READ time, as the owner, for DEFINER views (section Q3). | create OK; CreateViewWithSelect timing WRONG |
| 1.T2 | Trino | deny `create` on table `*` (policy 5): `CREATE VIEW` -> `Access Denied: Cannot create view v2`; `CREATE TABLE` -> `Cannot create table t3`; CTAS -> `Cannot create table t4`. Not separable. | OK |
| 1.T3 | Trino | deny `create` on table `v_*` (policy 6): `create view v_a` and `create table v_t` both denied; `create view innocent as select id, secret from mem.s.base` **succeeded** and `select id from innocent` returned rows. | OK |
| 1.T4 | Trino | `CREATE OR REPLACE VIEW` on an existing view under the `create *` deny: `Cannot create view v1`, a single `CreateView/create` request, no `drop`. `ALTER VIEW v1 RENAME TO v1r` under the same deny **succeeded**: `RenameView:source` + `RenameView:target`, both `alter`. `ALTER VIEW ... SET AUTHORIZATION` asked `alter` and was then refused by the connector (`Catalog does not support permission management: mem`). `COMMENT ON VIEW` asks `alter` (`SetViewComment`). `DROP VIEW` asks `drop`. | OK |
| 1.T5 | Trino | with the tag deny on `base.secret` and the tag cache refreshed: `vc_plain` `create view ... as select id, secret from mem.s.base` -> `Access Denied: Cannot select from columns [id, secret] in table or view base` (`SelectFromColumns secret allowed=false policy=7`); `vc_cleared` created the same view. | OK, but see F-6 (it was NOT denied during the first ~60 s) |
| 1.S1 | StarRocks | `CREATE VIEW`: `access=create view` on `[catalog=default_catalog database=vcdb]` (policy 17). `CREATE TABLE`: `access=create table` on the same resource. CTAS: `create table`. | OK |
| 1.S2 | StarRocks | deny `create view` on `vcdb` for `vc_plain` (policy 21): `CREATE VIEW` -> `ERROR 5204 ... CREATE VIEW privilege(s) on DATABASE vcdb`; `CREATE TABLE`, CTAS, `INSERT`, `SELECT` on the new table all worked. `CREATE OR REPLACE VIEW` of an existing view -> the same refusal (same `create view` request). `ALTER VIEW v AS select ...` **succeeded** (`access=alter` on `view=v_existing`), as did `ALTER VIEW ... SET SECURITY INVOKER`. | OK (the `ALTER VIEW AS` hole is the extra) |
| 1.S3 | StarRocks | `CREATE MATERIALIZED VIEW` under the `create view` deny **succeeded** (`access=create materialized view`, policy 17) and the MV returned `id, secret` rows to `vc_plain`. | OK |

### Q2. Does a policy whose resource is a VIEW column apply when users query the view?

| id | engine | observed | verdict |
|---|---|---|---|
| 2.T1 | Trino | path deny `select` on `mem.s.vd_c.secret` for `vc_cleared` (policy 10): `select id, secret from vd_c` -> `Access Denied: Cannot select from columns [id, secret] in table or view vd_c`, request `SelectFromColumns [table=vd_c column=secret] allowed=false policy=10`; `select id from vd_c` returned rows. | OK |
| 2.T2 | Trino | path MASK on `mem.s.vd_c.plain` for `vc_cleared` (policy 11): `[[1,'xxxx-0'],[2,'xxxx-0']]` via the view; the same user reading `base.plain` directly got `open-1`/`open-2`. The mask lookup is `getColumnMask [table=vd_c column=plain] allowed=true policy=11` as the READER. | OK |
| 2.T3 | Trino | the base check DOES still run, as the OWNER: reading DEFINER view `vd_u` (owner uncleared) as the cleared `vc_cleared`: view-level `SelectFromColumns` allowed (policy 2) and then `Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base`, request `as=vc_owner_u action=CreateViewWithSelectFromColumns [table=base column=secret] allowed=false policy=7`. | OK |
| 2.T4 | Trino/egret | `PUT /warehouses/{id}/resource-tags {schema:'s', table:'vd_c', column:'secret', tag_id}` -> 200, and `GET ...?table=vd_c` lists it. It also answered 200 for `table: 'no_such_view_xyz', column: 'nope'` (no existence check anywhere). **The UI CAN address a view column**: `frontend/src/routes/control-plane/warehouse-tabs/BrowseTab.tsx` (lines 438 to 478) renders a Tags button per column of a *view* and opens `CatalogTagDialog` with the view's name as `table`. That was read in source, not clicked, because a real Iceberg-egret warehouse was not stood up. A tag only reaches an engine for a catalog that has a `warehouse_id` (DEV-2). | **WRONG** ("the UI cannot offer a view") |
| 2.T5 | Trino | tag `vcview` on `vd_c.secret` + `tag-access` deny `select` for `vc_cleared` (policy 12): the tag download carried `table=vd_c column=secret`; after the tag cache refreshed `select id, secret from vd_c` as `vc_cleared` -> `Access Denied ... [id, secret] in table or view vd_c`, `allowed=false policy=12`. Tag `vcviewmask` on `vd_c.plain` + `tag-masking` (policy 13): `[[1,'xxxx-0'],[2,'xxxx-0']]`. | OK |
| 2.S1 | StarRocks | (a1) deny `select` on resource `view=v_c` for `vc_cleared` (policy 28): the query **still returned rows**. (a2) deny `select` on `table=v_c column=secret` (view named as a table, policy 29): **still returned** `classified-1/2`. **No `select` access request is made for a view at all**: for `select id from vcdb.v_u` the engine asked only mask / row-filter lookups (policyType 1 and 2) and no policyType 0 request. A user with NO grant anywhere (`vc_nobody`) read `v_u` and `vi_u` and got `classified-1/2`, while the same user was denied on `base` and on the MV (`ERROR 5204 ... SELECT privilege(s) on COLUMN id`). | **WRONG** (predicted the view-OBJECT deny would bite) |
| 2.S2 | StarRocks | a mask addressed as `table=v_c column=plain` for `vc_cleared` (policy 30): `select id, plain from vcdb.v_c` -> `xxxx-0`. SR looks up masks for view columns as TABLE resources (mask lookup, policyType 1, `[table=v_c column=plain] allowed=true policy=30`). Separately, a mask on the BASE column (policy 23, `vc_plain`) applied through views `v_u` and `vi_u` (`xxxx-0`), evaluated for the READER. | **WRONG** (predicted a view-column mask would not apply) |
| 2.S3 | StarRocks | NOT measured through egret. egret's tag clearance on StarRocks compiles to one path policy per tagged column in a registry catalog; `default_catalog` is a reserved name and cannot be a registry catalog (`400 StarRocks reserves the catalog name default_catalog`), so a tag cannot even address a native StarRocks table. By equivalence a compiled deny would be the (a2) path policy, which does not apply to a view. Inference, labelled. | inferred |

### Q3. Trino DEFINER vs INVOKER (memory connector; tag deny on `base.secret`, exceptions `vc_admin`, `vc_cleared`, `vc_owner_c`; views created BEFORE the deny)

`SHOW CREATE VIEW` proved the mode (`SECURITY DEFINER` for `vd_*`, `SECURITY INVOKER` for `vi_*`). Query: `select id, secret from mem.s.<view>`.

| view | owner | reader `vc_cleared` | reader `vc_plain` (uncleared) | prediction |
|---|---|---|---|---|
| DEFINER `vd_u` | `vc_owner_u` uncleared | **DENIED** `View owner 'vc_owner_u' cannot create view that selects from base` | **DENIED** (same text) | OK |
| DEFINER `vd_c` | `vc_owner_c` cleared | ALLOWED | **ALLOWED: the leak** `[[1,'classified-1'],[2,'classified-2']]` | OK |
| INVOKER `vi_u` | `vc_owner_u` uncleared | ALLOWED | **DENIED** `Cannot select from columns [plain, id, secret] in table or view base` | OK |
| INVOKER `vi_c` | `vc_owner_c` cleared | ALLOWED | **DENIED** (same) | OK |
| control: `base` directly | n/a | ALLOWED | DENIED `Cannot select from columns [id, secret] in table or view base` | OK |

Same 8 cells on an **Iceberg** catalog (`ice`, path-based column deny instead of a tag): `vd_u` denied/denied, `vd_c` allowed/**allowed (leak)**, `vi_u` allowed/denied, `vi_c` allowed/denied. Identical to memory.

Extra observations: an INVOKER read needs the reader to hold access to **every base column the view body references** (`plain` was in the error list although the query selected only `id, secret`). For a DEFINER read the engine asks, as the owner, `CreateViewWithSelectFromColumns` for every referenced base column (`plain, id, secret`) and not the plain `SelectFromColumns`.

| id | observed | verdict |
|---|---|---|
| 3.R | mask `MASK` on `base.plain` and row filter `id > 1` listed for `vc_plain` and `vc_owner_u` only. DEFINER `wd_u` (owner IS listed): both `vc_admin` and `vc_plain` got `[[2,'xxxx-0']]` (filtered AND masked as the owner; request `as=vc_owner_u getRowFilters [table=base] allowed=true policy=9`). DEFINER `wd_c` (owner NOT listed): `vc_plain` got `[[1,'open-1'],[2,'open-2']]`, **unmasked and unfiltered, a bypass of a base mask and row filter** (`as=vc_owner_c ... allowed=false`). INVOKER `wi_u`/`wi_c`: `vc_admin` unmasked, `vc_plain` masked and filtered (`as=vc_plain ... policy 9`). | OK |
| 3.C | `SHOW SESSION` has no row matching `view`, `invoker`, `definer` or `security`. Source (Trino tag 470): `CreateViewTask` comment "use DEFINER security by default", INVOKER only on the explicit clause; `SystemAccessControl.checkCanCreateView(context, view)` carries no mode flag. The only refusal is `Catalog does not support run-as DEFINER views` for connector-managed-security catalogs (`MetadataManager.getViewRunAsIdentity`), not reachable through the Ranger system access control. | OK |
| 3.V | covered by 1.T5 | OK |

### Q4. StarRocks 4.1.4 / ranger-plugins-common 2.8.0

Setup: base deny on `base.secret` (deny group `public`, exceptions `vc_admin`, `vc_cleared`, `vc_owner_c`; policy 22) and path MASK on `base.plain` for `vc_plain`, `vc_owner_u` (policy 23); views and MV created before them. Control: `vc_plain` direct `select id, secret from vcdb.base` -> `ERROR 5204 ... SELECT privilege(s) on COLUMN secret`; `vc_cleared` direct read OK.

| object | reader `vc_cleared` | reader `vc_plain` | notes |
|---|---|---|---|
| `v_u` (SECURITY NONE, owner `vc_owner_u`) | `classified-1/2` | **`classified-1/2` (deny not applied)** | `SHOW CREATE VIEW`: `SECURITY NONE` |
| `v_c` (NONE, owner `vc_owner_c`) | rows | **rows** | owner has no effect (no definer mode) |
| `vi_u` (**SECURITY INVOKER**) | rows | **rows (deny not applied)** | `SHOW CREATE VIEW`: `SECURITY INVOKER` |
| `vi_c` (INVOKER) | rows | **rows** | |
| `mv_c` (materialized view) | rows | **rows** | MV is a copy; also read UNMASKED (`open-1`) |
| `select id, plain` via `v_u`/`vi_u` as `vc_plain` | | `xxxx-0` | **base mask DID apply through the views**, for the reader |
| `select id, secret` on `v_u`/`vi_u`/`base` as `vc_nobody` (no grant at all) | | views: rows; base: denied | see 2.S1 |

| id | observed | verdict |
|---|---|---|
| 4.1 | column deny: SP4c claim holds (the deny is not applied through a plain view). **Mask: the SP4c-style assumption that a base-column policy is not consulted is wrong for masks**: the base mask applied through `v_u` and `vi_u` for the reader. | deny OK, mask **WRONG** |
| 4.2 | the image accepts `CREATE VIEW ... SECURITY INVOKER`, `ALTER VIEW ... SET SECURITY INVOKER`, and prints `SECURITY INVOKER` in `SHOW CREATE VIEW`. **It does not enforce anything under Ranger.** `vc_plain` read `secret` through `vi_u`/`vi_c`; a table-level deny with `column: *` on `base` blocked the direct read and NOT the INVOKER view; `vc_viewonly`/`vc_nobody` (no table grant) read `base.secret` through `vi_u`. Bytecode, `javap -c` on `fe-core-4.1.4.jar`, `ColumnPrivilege.checkViewPrivilege`: when `View.isSecurity()` it walks `View.getTableRefs()` and calls `Authorizer.checkTableAction(SELECT)` / `checkMaterializedViewAction` / (for a nested view) `Authorizer.check(query)`: a TABLE-level request, not column-level (the source tree in `../starrocks` re-checks the whole query and is newer than the image). The FE debug log shows no policyType 0 request for the base table during an INVOKER read. | **WRONG** |
| 4.3 | MV: copies data; base deny not carried, base mask not carried (`vc_plain` read `plain` as `open-1`); policies written on the MV's own columns apply (it is a table: `policyType 0 select [table=mv_c column=id] allowed=true policy=18`). | OK |
| 4.4 | external Iceberg view in StarRocks: **not run** (not cheap); not measured. | n/a |

### Q5. Precedence

| case | observed |
|---|---|
| Trino, view allowed + base denied (DEFINER, uncleared owner, cleared reader) | DENIED: both checks run, the base check as the OWNER; a deny anywhere is final |
| Trino, view-column deny + base allowed | DENIED (2.T1) |
| Trino, resource allow (policy 2) vs tag deny (policy 7) on `base.secret` | deny wins: `allowed=false policy=7` while policy 2 would allow |
| Trino, base mask + view-level mask (INVOKER view `wi_c`, reader `vc_plain`) | they STACK: base `MASK` gives `xxxx-0`, adding a view-level `MASK_SHOW_FIRST_4` gave `[[2,'xxxxXX']]` (the view mask applied to the output of the base mask) |
| Trino, two masks on one view coordinate | egret refused the second at write time: `409 rule_overlap` ("Ranger arbitrates masks with `>=`, so the FIRST evaluator in its band is final") |
| StarRocks, base mask (`MASK`) + view-level mask addressed as `table=v_u` (`MASK_NULL`) | `NULL` (view-level mask visible on top); `MASK_SHOW_FIRST_4` is not an SR mask type and returned `ERROR 1064 ... "s" is null` (my input error, retried with `MASK_NULL`) |
| StarRocks, view allow/deny vs base | no precedence question arises: the view is never asked; the base deny is never asked for a view read |
| Surprise | on Trino the `denyExceptions` list is consulted for the **owner**, not the reader: that single fact produces both Q3 failure directions and the 3.R mask/filter bypass |

## 4. Findings that were not predicted

- **F-1 (StarRocks, new hole, High).** A plain view is readable by a user who holds **no** grant at all (`vc_nobody`): the plugin is never asked a `select` question about the view. The earlier "the view is the only object checked" is stronger than that: the view is not checked. This is independent of clearance.
- **F-2 (StarRocks).** `SECURITY INVOKER` is accepted, stored and printed but not enforced under Ranger in 4.1.4 (4.2 above).
- **F-3 (StarRocks).** Masks DO apply through views (reader identity), denies do not. A MV does neither.
- **F-4 (Trino).** DEFINER views evaluate base masks and row filters as the OWNER (3.R): a bypass for every owner outside the mask list, not only for tag denies.
- **F-5 (egret).** An external Trino cannot be given a tag service through any route (DEV-1); tags reach only catalogs with `warehouse_id` (Iceberg on the embedded catalog, DEV-2); the resource-tags PUT accepts any string coordinate including nonexistent ones; a `generic` catalog's assignment succeeds and then does nothing.
- **F-6 (Trino).** The tag cache refreshes on its own timer: with a 5 s policy poll the deny on a newly assigned tag took about 60 s to bite (my first 1.T5 attempt read `secret` as `vc_plain` and created a view over it; that attempt is kept in the raw output and is a timing artefact, retried after the log line `Updated tags-cache to new version` for the same tag version). One sample, not a distribution.
- **F-7 (StarRocks).** `ALTER VIEW ... AS` and `ALTER VIEW ... SET SECURITY` need `alter` on the view and are not blocked by a `create view` deny; `CREATE MATERIALIZED VIEW` is its own access type.

## 5. What this means for the three fixes

| fix | Trino 470 | StarRocks 4.1.4 |
|---|---|---|
| **(1) deny view creation** | **Not viable.** `create` on the table resource is the same request for `CREATE VIEW`, `CREATE TABLE` and CTAS; the only lever is a name pattern, which `innocent` bypassed. It also leaves every existing DEFINER view, and the cleared users are exactly the ones who must be allowed to create. A schema-wide `create` deny is possible but blunt. | **Viable as a preventive control, incomplete.** `create view` on the database is a separate access type and can be denied while `create table`, CTAS and DML work (1.S2). Gaps: existing views stay readable by anyone (F-1); `ALTER VIEW AS` rewrites a view with only `alter`; `create materialized view` must be denied too and an MV copies data out. The deny must cover everyone who can read the classified column. |
| **(2) propagate tags to view columns** | **Viable, measured end to end for the engine half.** A tag on a view column denies/masks that column for the view (2.T5), and the DEFINER view owned by a cleared principal plus a view-column tag deny with the cleared as exceptions gave cleared ALLOWED and uncleared DENIED. egret already accepts the coordinate (API, and a UI button on view columns in source). Missing: the lineage that decides which view columns inherit a tag (an expression like `upper(secret) AS x` needs it). It does NOT remove the over-block half (an uncleared owner still blocks everybody, 2.T3) nor the mask/row-filter bypass (3.R) unless masks are propagated as well. Tag changes take up to about a minute to bite (F-6). | **Not viable for deny.** A deny on a view column or on the view object is never consulted (2.S1). A mask addressed as the view column does apply (2.S2), so only a MASK-shaped clearance could be propagated. egret cannot tag native StarRocks tables at all (reserved `default_catalog`). |
| **(3) `SECURITY INVOKER` / cleared owner** | **Viable and the only one that makes the base policy decide as the reader**: all 4 INVOKER cells correct, on memory and on Iceberg, and base masks/row filters follow the reader (3.R). Cost: nothing forces it. There is no setting, the default is DEFINER, and Ranger's `create` request carries no mode, so a policy cannot reject a DEFINER view; it needs a convention (tooling that always writes `SECURITY INVOKER`) plus an audit that reads `SHOW CREATE VIEW`. A cleared service owner is NOT a fix: a DEFINER view owned by a cleared principal is the leak (`vd_c`). The reader needs access to all base columns in the view body, not only the selected ones. | **Not viable.** The clause exists and is silently ineffective under Ranger (F-2). |

Candidate designs the data point to (not specified here): on Trino, (3) for enforcement plus (2) for defence in depth; on StarRocks, (1) for prevention plus a MASK-shaped clearance, because masks are the only base-column policy that survives a view.

## 6. Cleanup record

See the end of the working log `.superpowers/view-clearance-measure-log.md` (kept untracked).

---

# Appendix. Raw outputs, trimmed

Trimmed from the harness log: engine lookups that returned `policy=-1` and mask/row-filter lookups that did not match are removed; every query, every result and every matching request is kept. `REQ` lines are the engine plugin's `evaluatePolicies` entries from the debug log (`action=` is Trino's SPI call; StarRocks has no action name). `as=` is the identity the request was made for.

```text
## Q1 Trino (470), vc_plain may do everything (baseline)
[trino vc_plain] create view mem.s.v1 as select id, secret, plain from mem.s.base
  -> []
     REQ action=CreateView resource=[schema=s catalog=mem table=v1] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] create table mem.s.t1 as select id from mem.s.base
  -> [[2]]
     REQ action=CreateTable resource=[schema=s catalog=mem table=t1] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2

## Q1 Trino, DENY create on table * for vc_plain
(200, {'id': 5})
[trino vc_plain] create view mem.s.v2 as select id from mem.s.base
  -> FAILED: Access Denied: Cannot create view v2
     REQ action=CreateView resource=[schema=s catalog=mem table=v2] access=create policyType=pt0 allowed=false policy=pol5
[trino vc_plain] create table mem.s.t3 (a integer)
  -> FAILED: Access Denied: Cannot create table t3
     REQ action=CreateTable resource=[schema=s catalog=mem table=t3] access=create policyType=pt0 allowed=false policy=pol5
[trino vc_plain] create table mem.s.t4 as select id from mem.s.base
  -> FAILED: Access Denied: Cannot create table t4
     REQ action=CreateTable resource=[schema=s catalog=mem table=t4] access=create policyType=pt0 allowed=false policy=pol5
[trino vc_plain] create or replace view mem.s.v1 as select id from mem.s.base
  -> FAILED: Access Denied: Cannot create view v1
     REQ action=CreateView resource=[schema=s catalog=mem table=v1] access=create policyType=pt0 allowed=false policy=pol5
[trino vc_plain] alter view mem.s.v1 rename to mem.s.v1r
  -> []
     REQ action=RenameView:source resource=[schema=s catalog=mem table=v1] access=alter policyType=pt0 allowed=true policy=pol2
     REQ action=RenameView:target resource=[schema=s catalog=mem table=v1r] access=alter policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id from mem.s.v1r
  -> [[1], [2]]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=v1r] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] insert into mem.s.t1 values (9)
  -> [[1]]
     REQ action=InsertIntoTable resource=[schema=s catalog=mem table=t1] access=insert policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id from mem.s.t1
  -> [[2], [9], [1]]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=t1] access=select policyType=pt0 allowed=true policy=pol2

## Q1 Trino, DENY create on table v_* only
(204, '')
(200, {'id': 6})
[trino vc_plain] create view mem.s.v_a as select id from mem.s.base
  -> FAILED: Access Denied: Cannot create view v_a
     REQ action=CreateView resource=[schema=s catalog=mem table=v_a] access=create policyType=pt0 allowed=false policy=pol6
[trino vc_plain] create table mem.s.v_t as select id from mem.s.base
  -> FAILED: Access Denied: Cannot create table v_t
     REQ action=CreateTable resource=[schema=s catalog=mem table=v_t] access=create policyType=pt0 allowed=false policy=pol6
[trino vc_plain] create view mem.s.innocent as select id, secret from mem.s.base
  -> []
     REQ action=CreateView resource=[schema=s catalog=mem table=innocent] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id from mem.s.innocent
  -> [[1], [2]]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=innocent] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
(204, '')

## Q3/Q2 setup: tag vcsecret on mem.s.base.secret; tag-access deny select for public, denyExceptions vc_admin, vc_cleared, vc_owner_c
### 1.T5 with the deny active: uncleared vc_plain creates a view over the tagged column
[trino vc_plain] select id, secret from mem.s.base
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] create view mem.s.x_plain_view as select id, secret from mem.s.base
  -> []
     REQ action=CreateView resource=[schema=s catalog=mem table=x_plain_view] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] create view mem.s.x_plain_view2 as select id, plain from mem.s.base
  -> []
     REQ action=CreateView resource=[schema=s catalog=mem table=x_plain_view2] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_admin] drop view if exists mem.s.x_plain_view2
  -> []
### unassign tag, create the four views, reassign
(200, {'ok': True})
[trino vc_owner_u] create view mem.s.vd_u as select id, secret, plain from mem.s.base
  -> []
[trino vc_owner_c] create view mem.s.vd_c as select id, secret, plain from mem.s.base
  -> []
[trino vc_owner_u] create view mem.s.vi_u security invoker as select id, secret, plain from mem.s.base
  -> []
[trino vc_owner_c] create view mem.s.vi_c security invoker as select id, secret, plain from mem.s.base
  -> []
(200, {'ok': True})
[trino vc_admin] show create view mem.s.vd_u
  -> [['CREATE VIEW mem.s.vd_u SECURITY DEFINER AS\nSELECT\n  id\n, secret\n, plain\nFROM\n  mem.s.base']]
[trino vc_admin] show create view mem.s.vd_c
  -> [['CREATE VIEW mem.s.vd_c SECURITY DEFINER AS\nSELECT\n  id\n, secret\n, plain\nFROM\n  mem.s.base']]
[trino vc_admin] show create view mem.s.vi_u
  -> [['CREATE VIEW mem.s.vi_u SECURITY INVOKER AS\nSELECT\n  id\n, secret\n, plain\nFROM\n  mem.s.base']]
[trino vc_admin] show create view mem.s.vi_c
  -> [['CREATE VIEW mem.s.vi_c SECURITY INVOKER AS\nSELECT\n  id\n, secret\n, plain\nFROM\n  mem.s.base']]

### 1.T5 retried after the tag cache refreshed (tag version 3)
[trino vc_plain] select id, secret from mem.s.base
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7
[trino vc_plain] create view mem.s.x_plain_view3 as select id, secret from mem.s.base
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view base
     REQ action=CreateView resource=[schema=s catalog=mem table=x_plain_view3] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7
[trino vc_cleared] create view mem.s.x_cleared_view as select id, secret from mem.s.base
  -> []
     REQ action=CreateView resource=[schema=s catalog=mem table=x_cleared_view] access=create policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_admin] drop view if exists mem.s.x_plain_view
  -> []
[trino vc_admin] drop view if exists mem.s.x_cleared_view
  -> []

## Q3 Trino 8 cells (tag deny on base.secret, exceptions vc_admin, vc_cleared, vc_owner_c; views created BEFORE the deny)
[trino vc_cleared] select id, secret from mem.s.base
  -> [[2, 'classified-2'], [1, 'classified-1']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.base
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7
[trino vc_cleared] select id, secret from mem.s.vd_u
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.vd_u
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_cleared] select id, secret from mem.s.vd_c
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.vd_c
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_cleared] select id, secret from mem.s.vi_u
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vi_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vi_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.vi_u
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vi_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vi_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7
[trino vc_cleared] select id, secret from mem.s.vi_c
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vi_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vi_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.vi_c
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vi_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vi_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7

### Q3/Q2.T3: DEFINER views, requests of ALL identities (owner vs reader)
[trino vc_plain] select id, secret from mem.s.vd_c
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ as=vc_plain action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_plain action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_c action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_c action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_c action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_cleared] select id, secret from mem.s.vd_u
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_u] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_u action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_u action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=id table=base] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_owner_u action=CreateViewWithSelectFromColumns resource=[schema=s catalog=mem column=secret table=base] access=select policyType=pt0 allowed=false policy=pol7

## 3.R / Q5 Trino: base-table mask on plain and row filter id>1, applying to users vc_plain and vc_owner_u
(200, {'id': 8})
(200, {'id': 9})
control: direct base reads
[trino vc_admin] select id, plain from mem.s.base order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.base order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_plain action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_plain action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_owner_u] select id, plain from mem.s.base order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_owner_u action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_owner_u action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
views over base (columns id, plain): who is evaluated?

### 3.R: row filter + mask through views (mask/filter listed users: vc_plain, vc_owner_u). Reader x view
[trino vc_admin] select id, plain from mem.s.vd_u order by id
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
     REQ as=vc_owner_u action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_owner_u action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_plain] select id, plain from mem.s.vd_u order by id
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
     REQ as=vc_owner_u action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_owner_u action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_admin] select id, plain from mem.s.vd_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.vd_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_admin] select id, plain from mem.s.vi_u order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.vi_u order by id
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
     REQ as=vc_plain action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_plain action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_admin] select id, plain from mem.s.vi_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.vi_c order by id
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
     REQ as=vc_plain action=getColumnMask resource=[schema=s catalog=mem column=plain table=base] access=select policyType=pt1 allowed=true policy=pol8
     REQ as=vc_plain action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9

### 3.R clean: views WITHOUT the tagged column (id, plain). Row filter/mask listed users: vc_plain, vc_owner_u
(owners vc_owner_u = in the mask list, vc_owner_c = not in it)
[trino vc_owner_u] create view mem.s.wd_u as select id, plain from mem.s.base
  -> []
[trino vc_owner_c] create view mem.s.wd_c as select id, plain from mem.s.base
  -> []
[trino vc_owner_u] create view mem.s.wi_u security invoker as select id, plain from mem.s.base
  -> []
[trino vc_owner_c] create view mem.s.wi_c security invoker as select id, plain from mem.s.base
  -> []
[trino vc_admin] select id, plain from mem.s.wd_u order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_owner_u action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_plain] select id, plain from mem.s.wd_u order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_owner_u action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_admin] select id, plain from mem.s.wd_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.wd_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_admin] select id, plain from mem.s.wi_u order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.wi_u order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_plain action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9
[trino vc_admin] select id, plain from mem.s.wi_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]
[trino vc_plain] select id, plain from mem.s.wi_c order by id
  -> [[2, 'xxxx-0']]
     REQ as=vc_plain action=getRowFilters resource=[schema=s catalog=mem table=base] access=select policyType=pt2 allowed=true policy=pol9

## Q2 Trino (a): path policies whose resource is the VIEW column (view vd_c, DEFINER, owner vc_owner_c cleared)
baseline (no view-level policy): vc_cleared and vc_plain both read vd_c.secret
[trino vc_cleared] select id, secret, plain from mem.s.vd_c order by id
  -> [[1, 'classified-1', 'open-1'], [2, 'classified-2', 'open-2']]
(200, {'id': 10})
(200, {'id': 11})
2a deny on view column:
[trino vc_cleared] select id, secret from mem.s.vd_c order by id
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view vd_c
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=false policy=pol10
[trino vc_cleared] select id from mem.s.vd_c order by id
  -> [[1], [2]]
2a mask on view column (vc_cleared has NO base mask):
[trino vc_cleared] select id, plain from mem.s.vd_c order by id
  -> [[1, 'xxxx-0'], [2, 'xxxx-0']]
     REQ as=vc_cleared action=getColumnMask resource=[schema=s catalog=mem column=plain table=vd_c] access=select policyType=pt1 allowed=true policy=pol11
[trino vc_cleared] select id, plain from mem.s.base order by id
  -> [[1, 'open-1'], [2, 'open-2']]
unaffected user vc_plain via the same view:
[trino vc_plain] select id, secret, plain from mem.s.vd_c order by id
  -> [[1, 'classified-1', 'open-1'], [2, 'classified-2', 'open-2']]
(204, '')
(204, '')

## Q2 Trino (b): TAG on the VIEW column (egret API PUT resource-tags, table=<view>)
API accepts a view coordinate: (200, {'ok': True})
API accepts a coordinate that exists nowhere: (200, {'ok': True})
API list for vd_c: [{'schema': 's', 'table': 'vd_c', 'column': 'secret', 'tag': {'id': '431d447e-3a39-4cdd-a3f3-af57795f6eea', 'key': 'vcview', 'name': 'vcview', 'description': '', 'color': '', 'assignment_count': 0, 'created_at': '2026-10-13T01:35:34.515920Z'}}]
(200, {'ok': True})
(200, {'ok': True})
(200, {'id': 12})
(200, {'id': 13})
tag download resources: [{"id": 1, "isEnabled": true, "serviceName": "vctrino", "resourceElements": {"catalog": {"values": ["mem"]}, "schema": {"values": ["s"]}, "table": {"values": ["base"]}, "column": {"values": ["secret"]}}}, {"id": 2, "isEnabled": true, "serviceName": "vctrino", "resourceElements": {"catalog": {"values": ["mem"]}, "schema": {"values": ["s"]}, "table": {"values": ["vd_c"]}, "column": {"values": ["plain"]}}}, {"id": 3, "isEnabled": true, "serviceName": "vctrino", "resourceElements": {"catalog": {"values": ["mem"]}, "schema": {"values": ["s"]}, "table": {"values": ["vd_c"]}, "column": {"values": ["secret"]}}}]
[trino vc_cleared] select id, secret from mem.s.vd_c order by id
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view vd_c
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=false policy=pol12
[trino vc_cleared] select id from mem.s.vd_c order by id
  -> [[1], [2]]
[trino vc_cleared] select id, plain from mem.s.vd_c order by id
  -> [[1, 'xxxx-0'], [2, 'xxxx-0']]
     REQ as=vc_cleared action=getColumnMask resource=[schema=s catalog=mem column=plain table=vd_c] access=select policyType=pt1 allowed=true policy=pol13
[trino vc_plain] select id, secret from mem.s.vd_c order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]

## Fix (2) end-to-end on Trino: DEFINER view vd_c owned by CLEARED vc_owner_c; tag vcview on vd_c.secret with deny public, exceptions cleared
(204, '')
(200, {'id': 14})
[trino vc_cleared] select id, secret from mem.s.vd_c order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=plain table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_cleared action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
[trino vc_plain] select id, secret from mem.s.vd_c order by id
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view vd_c
     REQ as=vc_plain action=SelectFromColumns resource=[schema=s catalog=mem column=id table=vd_c] access=select policyType=pt0 allowed=true policy=pol2
     REQ as=vc_plain action=SelectFromColumns resource=[schema=s catalog=mem column=secret table=vd_c] access=select policyType=pt0 allowed=false policy=pol14

## Q5 Trino: stacking of base-level and view-level MASK for vc_plain on wi_c.plain (INVOKER view)
[trino vc_plain] select id, plain from mem.s.wi_c order by id
  -> [[2, 'xxxx-0']]
(200, {'id': 15})
with view-level MASK_SHOW_FIRST_4 added:
[trino vc_plain] select id, plain from mem.s.wi_c order by id
  -> [[2, 'xxxxXX']]
same view-level mask on a view whose base mask does not apply to the reader (vc_cleared has none):
(409, {'error': 'two mask policies would be served at policyPriority 0 that may both decide one request (catalog=[mem] column=[plain] schema=[s] table=[wi_c]) with different dataMaskPolicyItems: policy "vc-view-mask-wi_c" and policy "vc-view-mask-wi_c2". Ranger arbitrates masks with `>=`, so the FIRST evaluator in its band is final, and within a band the order is `getEvalOrder()` (a wildcard-count score: with the same users, groups and accesses the broader policy runs first and wins wherever both match) and then the policy name, i.e. a rule\'s uuid — neither of which says which of the two should win. Narrow one of the two so they no longer select a common resource, or delete one, or make them agree, or set Override on the one that should win (an override is evaluated first).', 'code': 'rule_overlap', 'rules': [{'id': None, 'name': 'vc-view-mask-wi_c'}, {'id': None, 'name': 'vc-view-mask-wi_c2'}], 'coordinate': 'catalog=[mem] column=[plain] schema=[s] table=[wi_c]', 'kind': 'mask'})
[trino vc_cleared] select id, plain from mem.s.wi_c order by id
  -> [[1, 'open-1'], [2, 'open-2']]

## Q3 Trino: any setting that forces INVOKER / refuses DEFINER?
SHOW SESSION rows matching view/invoker/definer/security: []
[['system'], ['mem']]
[sr vc_admin] create table vcdb.t0 (id int)
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create table policyType= allowed=true policy=17
     REQ resource=[database=vcdb catalog=default_catalog] access=create table policyType=0 allowed=true policy=17

# StarRocks 4.1.4 / ranger-plugins-common 2.8.0
current_version()
4.1.4-4a9848e
[sr vc_admin] drop table vcdb.t0
  -> 
[sr vc_admin] create table vcdb.base (id int, secret varchar(20), plain varchar(20)) distributed by hash(id) buckets 1 properties('replication_num'='1')
  -> 
[sr vc_admin] insert into vcdb.base values (1,'classified-1','open-1'),(2,'classified-2','open-2')
  -> 
[sr vc_admin] select * from vcdb.base order by id
  -> id	secret	plain | 1	classified-1	open-1 | 2	classified-2	open-2

## Q1 StarRocks baseline (vc_plain may do everything)
[sr vc_plain] create view vcdb.v1 as select id, secret, plain from vcdb.base
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create view policyType=0 allowed=true policy=17
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] create table vcdb.t1 (a int) distributed by hash(a) buckets 1 properties('replication_num'='1')
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create table policyType=0 allowed=true policy=17
[sr vc_plain] create table vcdb.t2 distributed by hash(id) buckets 1 properties('replication_num'='1') as select id from vcdb.base
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create table policyType=0 allowed=true policy=17
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] create or replace view vcdb.v1 as select id, secret from vcdb.base
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create view policyType=0 allowed=true policy=17
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] alter view vcdb.v1 as select id, secret, plain from vcdb.base
  -> 
     REQ resource=[database=vcdb view=v1 catalog=default_catalog] access=alter policyType=0 allowed=true policy=19
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] alter view vcdb.v1 set security invoker
  -> 
     REQ resource=[database=vcdb view=v1 catalog=default_catalog] access=alter policyType=0 allowed=true policy=19
[sr vc_plain] show create view vcdb.v1
  -> View	Create View	character_set_client	collation_connection | v1	CREATE VIEW `v1` (`id`, `secret`, `plain`) SECURITY INVOKER AS select id, secret, plain from vcdb.base;	utf8	utf8_general_ci
[sr vc_admin] drop view vcdb.v1
  -> 
[sr vc_admin] drop table vcdb.t1
  -> 
[sr vc_admin] drop table vcdb.t2
  -> 
[sr vc_admin] create view vcdb.v_existing as select id, secret, plain from vcdb.base
  -> 

## Q1 StarRocks: DENY 'create view' on database vcdb for vc_plain (create table etc. still allowed)
(200, {'id': 21})
[sr vc_plain] create view vcdb.v2 as select id from vcdb.base
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the CREATE VIEW privilege(s) on DATABASE vcdb for this operation.
     REQ resource=[database=vcdb catalog=default_catalog] access=create view policyType=0 allowed=false policy=21
[sr vc_plain] create table vcdb.t3 (a int) distributed by hash(a) buckets 1 properties('replication_num'='1')
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create table policyType=0 allowed=true policy=17
[sr vc_plain] create table vcdb.t4 distributed by hash(id) buckets 1 properties('replication_num'='1') as select id from vcdb.base
  -> 
[sr vc_plain] insert into vcdb.t3 values (1)
  -> 
[sr vc_plain] select * from vcdb.t3
  -> a | 1
[sr vc_plain] create or replace view vcdb.v_existing as select id from vcdb.base
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the CREATE VIEW privilege(s) on DATABASE vcdb for this operation.
     REQ resource=[database=vcdb catalog=default_catalog] access=create view policyType=0 allowed=false policy=21
[sr vc_plain] alter view vcdb.v_existing as select id from vcdb.base
  -> 
     REQ resource=[database=vcdb view=v_existing catalog=default_catalog] access=alter policyType=0 allowed=true policy=19
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] alter view vcdb.v_existing set security invoker
  -> 
     REQ resource=[database=vcdb view=v_existing catalog=default_catalog] access=alter policyType=0 allowed=true policy=19
[sr vc_plain] create materialized view vcdb.mv1 distributed by hash(id) refresh manual properties('replication_num'='1') as select id, secret from vcdb.base
  -> 
     REQ resource=[database=vcdb catalog=default_catalog] access=create materialized view policyType=0 allowed=true policy=17
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=true policy=18
[sr vc_plain] select * from vcdb.mv1
  -> id	secret | 2	classified-2 | 1	classified-1
[sr vc_plain] drop view vcdb.v_existing
  -> 
     REQ resource=[database=vcdb view=v_existing catalog=default_catalog] access=drop policyType=0 allowed=true policy=19
[sr vc_admin] drop materialized view vcdb.mv1
  -> 
[sr vc_admin] drop table vcdb.t3
  -> 
[sr vc_admin] drop table vcdb.t4
  -> 
[sr vc_admin] drop view if exists vcdb.v_existing
  -> 
[sr vc_admin] create view vcdb.vx as select 1
  -> 

## Q4 StarRocks setup: views created BEFORE any column policy
[sr vc_owner_u] create view vcdb.v_u as select id, secret, plain from vcdb.base
  -> 
[sr vc_owner_c] create view vcdb.v_c as select id, secret, plain from vcdb.base
  -> 
[sr vc_owner_u] create view vcdb.vi_u security invoker as select id, secret, plain from vcdb.base
  -> 
[sr vc_owner_c] create view vcdb.vi_c security invoker as select id, secret, plain from vcdb.base
  -> 
[sr vc_owner_c] create materialized view vcdb.mv_c distributed by hash(id) refresh manual properties('replication_num'='1') as select id, secret, plain from vcdb.base
  -> 
[sr vc_owner_c] refresh materialized view vcdb.mv_c with sync mode
  -> QUERY_ID | 01a132ec-6703-76b2-bc2c-b42dea433e3a
[sr vc_admin] show create view vcdb.v_u
  -> View	Create View	character_set_client	collation_connection | v_u	CREATE VIEW `v_u` (`id`, `secret`, `plain`) SECURITY NONE AS select id, secret, plain from vcdb.base;	utf8	utf8_general_ci
[sr vc_admin] show create view vcdb.vi_u
  -> View	Create View	character_set_client	collation_connection | vi_u	CREATE VIEW `vi_u` (`id`, `secret`, `plain`) SECURITY INVOKER AS select id, secret, plain from vcdb.base;	utf8	utf8_general_ci

## Q4: clearance-shaped COLUMN deny on base.secret: deny public select, exceptions vc_admin, vc_cleared, vc_owner_c; + path MASK on base.plain for vc_plain,vc_owner_u
(200, {'id': 22})
(200, {'id': 23})
direct base reads (control)
[sr vc_cleared] select id, secret from vcdb.base order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=true policy=18
[sr vc_cleared] select id, plain from vcdb.base order by id
  -> id	plain | 1	open-1 | 2	open-2
[sr vc_plain] select id, secret from vcdb.base order by id
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN secret for this operation.
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
     REQ resource=[database=vcdb catalog=default_catalog column=id table=base] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=base] access=select policyType=0 allowed=false policy=22
[sr vc_plain] select id, plain from vcdb.base order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0

## Q4 StarRocks: through views (deny on base.secret; mask on base.plain for vc_plain, vc_owner_u)
[sr vc_cleared] select id, secret from vcdb.v_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_plain] select id, secret from vcdb.v_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_cleared] select id, secret from vcdb.v_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_plain] select id, secret from vcdb.v_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_cleared] select id, secret from vcdb.vi_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_plain] select id, secret from vcdb.vi_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_cleared] select id, secret from vcdb.vi_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_plain] select id, secret from vcdb.vi_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_cleared] select id, secret from vcdb.mv_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=id table=mv_c] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=mv_c] access=select policyType=0 allowed=true policy=18
[sr vc_plain] select id, secret from vcdb.mv_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
     REQ resource=[database=vcdb catalog=default_catalog column=id table=mv_c] access=select policyType=0 allowed=true policy=18
     REQ resource=[database=vcdb catalog=default_catalog column=secret table=mv_c] access=select policyType=0 allowed=true policy=18
[sr vc_plain] select id, plain from vcdb.v_u order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0
[sr vc_plain] select id, plain from vcdb.vi_u order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0
[sr vc_plain] select id, plain from vcdb.mv_c order by id
  -> id	plain | 1	open-1 | 2	open-2

## Q4.2 StarRocks 4.1.4 bytecode: ColumnPrivilege.checkViewPrivilege, isSecurity branch (javap -c on fe-core-4.1.4.jar):
   for each View.getTableRefs(): getBasicTable -> isOlapView? Analyzer.analyze + Authorizer.check(view query) : isMaterializedView? checkMaterializedViewAction : checkTableAction(SELECT)  -- i.e. TABLE-level, not column-level, for base tables

## Q4.2b live: TABLE-level deny on base for vc_plain (vs the earlier column-level deny)
(200, {'id': 24})
[sr vc_plain] select id from vcdb.base order by id
  -> id | 1 | 2
[sr vc_plain] select id from vcdb.v_u order by id
  -> id | 1 | 2
[sr vc_plain] select id from vcdb.vi_u order by id
  -> id | 1 | 2
[sr vc_plain] select id from vcdb.mv_c order by id
  -> id | 1 | 2

## Q4.2b redo: deny select on base, ALL columns (column '*'), for vc_plain
(200, {'id': 25})
[sr vc_plain] select id from vcdb.base order by id
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.
[sr vc_plain] select id from vcdb.v_u order by id
  -> id | 1 | 2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_plain] select id from vcdb.vi_u order by id
  -> id | 1 | 2
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=base] access=select policyType=1 allowed=true policy=23
[sr vc_plain] select id from vcdb.mv_c order by id
  -> id | 1 | 2
     REQ resource=[database=vcdb catalog=default_catalog column=id table=mv_c] access=select policyType=0 allowed=true policy=18

## Q4.2c: does the INVOKER branch run at all? vc_viewonly holds ONLY catalog usage + select on views v_u, vi_u (no table grant anywhere)
(200, {'id': 26})
(200, {'id': 27})
[sr vc_viewonly] select id, secret from vcdb.base
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.
[sr vc_viewonly] select id, secret from vcdb.v_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_viewonly] select id, secret from vcdb.vi_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2

## Q2 StarRocks: policies addressed at the VIEW (view v_c). vc_cleared reads base.secret freely; no base mask applies to vc_cleared
[sr vc_cleared] select id, secret, plain from vcdb.v_c order by id
  -> id	secret	plain | 1	classified-1	open-1 | 2	classified-2	open-2
(a1) deny select on resource view=v_c (object level) for vc_cleared
(200, {'id': 28})
[sr vc_cleared] select id from vcdb.v_c order by id
  -> id | 1 | 2
(a2) COLUMN-level deny addressed as table=v_c column=secret (the view named as a table) for vc_cleared
(200, {'id': 29})
[sr vc_cleared] select id, secret from vcdb.v_c order by id
  -> id	secret | 1	classified-1 | 2	classified-2
(a3) MASK addressed as table=v_c column=plain for vc_cleared
(200, {'id': 30})
[sr vc_cleared] select id, plain from vcdb.v_c order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0
     REQ resource=[database=vcdb catalog=default_catalog column=plain table=v_c] access=select policyType=1 allowed=true policy=30
[sr vc_viewonly] select id from vcdb.v_u
  -> id | 1 | 2
[sr vc_viewonly] select id from vcdb.vi_u
  -> id | 1 | 2
[sr vc_nobody] select id from vcdb.v_u
  -> id | 2 | 1
[sr vc_nobody] select id from vcdb.base
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.
[sr vc_nobody] select id from vcdb.mv_c
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.
[sr vc_nobody] show databases
  -> 

## Q5 StarRocks: base mask (policy 23, MASK for vc_plain on base.plain) + view-level mask addressed as table=v_u column=plain
[sr vc_plain] select id, plain from vcdb.v_u order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0
(200, {'id': 31})
[sr vc_plain] select id, plain from vcdb.v_u order by id
  -> ERROR 1064 (HY000) at line 1: Cannot invoke "String.length()" because "s" is null
[sr vc_plain] select id, plain from vcdb.base order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0

## Q4 SR: a view's own owner has no effect (StarRocks has no definer): vc_owner_u (uncleared) created v_u; vc_cleared reads secret through it fine (above). Control: plain no-grant user reading a view
[sr vc_nobody] select id, secret from vcdb.v_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_nobody] select id, secret from vcdb.vi_u order by id
  -> id	secret | 1	classified-1 | 2	classified-2
[sr vc_nobody] select id, secret from vcdb.base order by id
  -> ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.
(retry: MASK_SHOW_FIRST_4 is not in StarRocks' mask types, so that error is my input, not a finding; use MASK_NULL)
(200, {'id': 32})
[sr vc_plain] select id, plain from vcdb.v_u order by id
  -> id	plain | 1	NULL | 2	NULL
[sr vc_plain] select id, plain from vcdb.vi_u order by id
  -> id	plain | 1	xxxx-0 | 2	xxxx-0

## Q3 confirmation on an ICEBERG catalog (Trino 470 iceberg, TESTING_FILE_METASTORE, file:///tmp/ice); path-based column deny (not tag), exceptions vc_admin, vc_cleared, vc_owner_c. (memory tables were lost by the container restart; the memory results above predate it)
[trino vc_admin] create schema ice.s
  -> []
[trino vc_admin] create table ice.s.base as select 1 id, 'classified-1' secret, 'open-1' plain union all select 2,'classified-2','open-2'
  -> [[2]]
[trino vc_owner_u] create view ice.s.vd_u as select id, secret, plain from ice.s.base
  -> []
[trino vc_owner_c] create view ice.s.vd_c as select id, secret, plain from ice.s.base
  -> []
[trino vc_owner_u] create view ice.s.vi_u security invoker as select id, secret, plain from ice.s.base
  -> []
[trino vc_owner_c] create view ice.s.vi_c security invoker as select id, secret, plain from ice.s.base
  -> []
(200, {'id': 33})
[trino vc_cleared] select id, secret from ice.s.base
  -> [[2, 'classified-2'], [1, 'classified-1']]
[trino vc_plain] select id, secret from ice.s.base
  -> FAILED: Access Denied: Cannot select from columns [id, secret] in table or view base
[trino vc_cleared] select id, secret from ice.s.vd_u order by id
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
[trino vc_plain] select id, secret from ice.s.vd_u order by id
  -> FAILED: Access Denied: View owner does not have sufficient privileges: View owner 'vc_owner_u' cannot create view that selects from base
[trino vc_cleared] select id, secret from ice.s.vd_c order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]
[trino vc_plain] select id, secret from ice.s.vd_c order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]
[trino vc_cleared] select id, secret from ice.s.vi_u order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]
[trino vc_plain] select id, secret from ice.s.vi_u order by id
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
[trino vc_cleared] select id, secret from ice.s.vi_c order by id
  -> [[1, 'classified-1'], [2, 'classified-2']]
[trino vc_plain] select id, secret from ice.s.vi_c order by id
  -> FAILED: Access Denied: Cannot select from columns [plain, id, secret] in table or view base
```
