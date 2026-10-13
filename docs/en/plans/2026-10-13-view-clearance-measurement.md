# View clearance measurement: can SQL views carry or lose a column clearance?

> **Status:** predictions written FIRST (this commit). Observations are added in a second commit.
> A wrong prediction is a finding; no test is retuned to match a prediction.
> Canonical (English). Measurement only; no product code changed.

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
