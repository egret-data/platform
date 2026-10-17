# Governance Data: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-16-dg-frontend-data.md`. Spec:
> `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` §2.3 (decision F8). Branch `feat/dg-frontend-redesign`,
> worktree `egret-dgfe`.

## Measurements (Task 1)

The question: can egret's **existing** internal identities list a catalog that egret does not manage (schemas,
tables, columns) on a live engine, with no new privilege? The answer decides Task 3's statements and whether F8
holds.

### 1.0 Predictions (committed before anything was observed)

Written from the code at `2c8bfcfc` and the earlier live results, before any engine of this run was started. A
wrong prediction is a finding. Nothing in this subsection is edited after observing.

**Setup the predictions are about.**
- egret image built from `2c8bfcfc` (`deploy/docker/backend.Dockerfile`), throwaway DB `egret_dgfe_live`, own
  containers `vg-*` on network `vg-net`, recipe of the view-governance live result §2.2 and §4.9.
- **egret Iceberg catalog `ice`**: warehouse `ice` on `vg-rustfs`, namespace `s`, one table `s.t`.
- **External catalog `ext`** (the label used throughout): a registry catalog of kind `jdbc`, dialect `postgresql`,
  pointing at a throwaway `postgres:16` container `vg-pg`, database `extdb`. It is "external" in the spec's
  sense: egret does not manage it, so the browse route must list it through an engine. Chosen because it is the
  cheapest catalog both engines attach (Trino's `postgresql` connector, StarRocks' JDBC catalog) with no second
  REST catalog service. Contents:
  - schema `sales`: table `orders(id integer, amount numeric(10,2), note text)`, view `v_orders` (over `orders`),
    materialized view `mv_orders` (a Postgres one);
  - schema `empty_s`: no objects.
- **Trino JWT engine `vgtr`** (Trino 483, Ranger plugin bound to profile `vgtr`): the provisioner's JWT rendering
  (TLS 8443, JWT only, `principal-field=preferred_username`, `required-audience=engine`), pod-shaped host name
  `vgtr-coordinator-<hash>-<id>`, so egret serves it the lineage transform. Catalog files `ice` (the `render_trino`
  Egret shape), `ext` (`connector.name=postgresql`, the `jdbc_url` rendering) and a control `tpch` that exists on
  the engine but is **not** attached in egret's registry. Engine row and attachments by SQL as in the earlier runs
  (`POST /engines` and the Trino attach need Kubernetes).
- **Trino impersonate engine `vgti`**: a second profile and engine row, `delegationMode: impersonate`, PASSWORD
  authentication with the `egret_delegator` user, the same three catalog files. Its stored policies are what egret
  writes on creation for that mode: the delegator's `trinouser = *` impersonate, the per-user baseline, and the
  platform admins' all-access set (seeded by SQL with the exact bodies egret builds, because creation needs
  Kubernetes).
- **StarRocks `vgsr`** (4.1.6, plugin 2.9.0, `access_control = ranger`), catalogs `ice` and `ext` created by
  egret's own attach path. The MV lister's account is `root` (`fe_target`: `root` with the engine's root secret,
  none here).
- The lineage token is minted with egret's signing key and the claims of `lineage_claims`
  (`sub = preferred_username = egret-lineage`, `groups: []`, `aud: engine`), and sent with
  `X-Trino-User: egret-lineage`, as the lineage runner does.

**Why the Trino predictions are what they are (code, not measurement).** In `io.trino_trino-ranger-483.jar`
(`javap`), `checkCanShowSchemas`, `checkCanShowTables`, `checkCanShowColumns`, `filterSchemas`, `filterTables`,
`filterColumns` and `canAccessCatalog` all ask Ranger for the pseudo access `_any`; `checkCanSelectFromColumns`
asks `select`. The lineage transform gives `egret-lineage` `use`+`select` on `<catalog>.*.*.*` for every catalog
attached to the profile's engines, and an override-priority deny of every other access. The lineage EXPLAIN already
passes `canAccessCatalog` (`_any`) live (view-governance result §2.7 M1, §4.16), so `_any` is allowed despite that
deny.

**Trino JWT engine, as `egret-lineage`.**

| id | statement | prediction |
|---|---|---|
| J1 | `SHOW SCHEMAS FROM ext` | rows `empty_s`, `information_schema`, `public`, `sales` (`pg_catalog` hidden by the connector; not confident) |
| J2 | `SHOW TABLES FROM ext.sales` | `mv_orders`, `orders`, `v_orders` |
| J3 | `SHOW TABLES FROM ext.empty_s` | an empty result, not an error |
| J4 | `DESCRIBE ext.sales.orders` | `id integer`, `amount decimal(10,2)`, `note varchar` |
| J5 | `SELECT schema_name FROM ext.information_schema.schemata` | the same rows as J1 |
| J6 | `SELECT table_name, table_type FROM ext.information_schema.tables WHERE table_schema = 'sales'` | the three names, **all `BASE TABLE`**: a JDBC connector exposes a remote view or MV as a table, so the kind is not recoverable from Trino for `ext` |
| J7 | `SELECT column_name, data_type FROM ext.information_schema.columns WHERE table_schema = 'sales' AND table_name = 'orders'` | as J4 |
| J8 | J1, J2, J4 on `ice` (`ice.s`, `ice.s.t`) | rows; `information_schema.tables` says `BASE TABLE` for `t` |
| J9 | control: `SHOW SCHEMAS FROM tpch` (on the engine, not attached in the registry) | `Access Denied: Cannot access catalog tpch` |
| J10 | control: the same J1 as `vcplain` (a non-admin egret user with no grant) | `Access Denied: Cannot access catalog ext` |
| J11 | `SELECT * FROM system.metadata.materialized_views` | refused (`Cannot access catalog system`): `system` is not attached |
| J12 | duration of each of J1–J7 | under 2 s, far inside the 15 s deadline |

**Trino impersonate engine** (basic auth as `egret_delegator`, `X-Trino-User` = the identity below, the way
`egret-query` runs an impersonate connection).

| id | identity | `SHOW SCHEMAS FROM ext` prediction |
|---|---|---|
| I1 | `egret_delegator` as itself | `Access Denied: Cannot access catalog ext` (it holds only `impersonate`) |
| I2 | delegator → `egret-lineage` | `Access Denied: Cannot access catalog ext` (the transform is served only to JWT engines, and the stored policies say nothing about `egret-lineage`) |
| I3 | delegator → `vcplain` | `Access Denied: Cannot access catalog ext` |
| I4 | delegator → the platform admin (`admin`) | rows as J1: the `egret-admin` all-access set |

So no **internal** identity can list on an impersonate engine without a new grant. The only one that can is a
platform admin's own name, which is a person, not egret. The predicted decision for impersonate engines is
`notListed` (`engine_error`), unless the user rules otherwise.

**StarRocks, as `root`.**

| id | statement | prediction |
|---|---|---|
| S1 | `SHOW DATABASES FROM ext` | `empty_s`, `information_schema`, `public`, `sales` at least (Postgres system schemas may appear) |
| S2 | `SHOW TABLES FROM ext.sales` | `orders`, `v_orders`; `mv_orders` not confident |
| S3 | `SHOW TABLES FROM ext.empty_s` | an empty set |
| S4 | `DESCRIBE ext.sales.orders` and `SHOW FULL COLUMNS FROM ext.sales.orders` | `id INT`, `amount DECIMAL(10,2)`, `note VARCHAR(…)` |
| S5 | `SHOW FULL TABLES FROM ext.sales` | answers, with every row `BASE TABLE` (not confident) |
| S6 | `SELECT TABLE_NAME, TABLE_TYPE FROM ext.information_schema.tables WHERE TABLE_SCHEMA = 'sales'` | answers, as S5 |
| S7 | S1, S2, S4 on `ice` | rows |
| S8 | control: S1 as a StarRocks user with no grant (`vcnobody`) | refused, naming the `USAGE` privilege on `CATALOG ext` |
| S9 | Ranger and `root` | `root` lists everything with no Ranger policy for it (view-governance result §2.3 S7: root is a superuser under Ranger on 4.1.6) |

**F8 verdict predicted:** holds for Trino JWT engines (`egret-lineage`, J1–J8) and StarRocks (`root`, S1–S7). For
Trino impersonate engines no existing internal identity can list without a new grant; that is not a STOP (the spec
allows `notListed` there), but it is reported for a ruling.

### 1.1 Result (read this first)

**F8 holds where the spec needs it to. Nothing is BLOCKED.** Measured 2026-10-16 between 19:13 and 19:20 UTC.

**Scope.** Measured on one external catalog kind (JDBC/Postgres, `ext`) and one egret Iceberg catalog (`ice`).
Other connector kinds (Hive, Delta, an external Iceberg REST catalog, …) were not tried. For Trino they are
reasoned, not measured: the lineage transform grants `use`+`select` on every attached catalog whatever its kind, and
the listing checks are connector-independent `_any` checks. Every query went directly to the engines with egret's
identity shapes (§1.2), not through egret's query or lineage code.

- **Trino JWT engine:** `egret-lineage` lists `ext` and `ice` with every statement tried: `SHOW SCHEMAS`,
  `SHOW TABLES`, `DESCRIBE`, and `information_schema.schemata` / `tables` / `columns`. Each one took 0.2–2.3 s.
  No new grant is needed: the download-time lineage transform already serves `use`+`select` on every attached
  catalog, and Trino 483 checks `_any` for listings.
- **StarRocks:** `root`, the MV lister's account, lists `ext` and `ice` with `SHOW DATABASES`, `SHOW [FULL]
  TABLES`, `DESCRIBE` / `SHOW FULL COLUMNS`, and `information_schema`, with no Ranger policy naming it
  (0.2–3.5 s).
- **Trino impersonate engine:** no internal identity can list. `egret_delegator` as itself, impersonating
  `egret-lineage`, or impersonating a non-admin user, each gets `Access Denied: Cannot access catalog ext`. Only
  impersonating a platform admin (`admin`, which holds the `egret-admin` all-access set) lists. That is a
  person's identity, not egret's, so it is reported for a ruling, not used. The predicted decision stands:
  impersonate engines answer `notListed` (`engine_error`), unless a grant is approved.

25 predicted cells (J1–J12, I1–I4, S1–S9): 19 matched, 6 did not: J1, J5 and I4 (`pg_catalog` is listed), J8
(`system` schema), J12 (J1 took 2.33 s, over the predicted 2 s) and S8. None changes the verdict. S8 matters for
Task 3, below.

**Facts Task 3 must build on:**
1. **The kind was lost for the one JDBC/Postgres catalog measured.** In `ext` (one Postgres view `v_orders`, one
   Postgres MV `mv_orders`), Trino `information_schema.tables` and StarRocks `SHOW FULL TABLES` /
   `information_schema.tables` both report the view and the MV as `BASE TABLE` (J6, S5, S6a). On StarRocks the same
   is true for the StarRocks-written Iceberg view `v_t` (S7b, S7d). Trino reports that Iceberg view as `VIEW` (J8d).
   So for these cases the listers can map `BASE TABLE` → `table` and `VIEW` → `view`, and must accept that a remote
   view or MV may show as `table`. **Not tried:** a Trino Iceberg MV, a Hive catalog, an external Iceberg REST
   catalog, other JDBC dialects. Whether those report `VIEW` / an MV kind is unmeasured.
2. **Empty answers come in two kinds.**
   - *Trustworthy empty:* `SHOW TABLES FROM ext.empty_s` answers an empty set on both engines (J3, S3), which is
     a real empty schema.
   - *Silent empty:* a StarRocks user without privilege also gets an empty set, not an error, for `SHOW
     DATABASES FROM ext` and `SHOW TABLES FROM ext.sales` (S8a, S8b). On Trino, `SHOW CATALOGS` for
     `egret_delegator` was empty too (I1c). Trino's `SHOW SCHEMAS` and `information_schema.schemata` on a
     catalog the identity cannot access give an error (J9, J10, J10b), not an empty set. (`SHOW TABLES` on a
     denied catalog was not run.)
   - So on StarRocks an empty listing is trustworthy **only because the identity is `root`**. Task 3 must not
     list StarRocks as any other account.
3. **Missing objects are errors, not empty sets.** Trino: `Schema 'nosuch' does not exist` (`SCHEMA_NOT_FOUND`)
   and `Table 'ext.sales.nosuch' does not exist` (`TABLE_NOT_FOUND`). StarRocks: `ERROR 5501 … Unknown database
   'nosuch'` and `ERROR 1064 … table : nosuch not exists`.
4. **System schemas are listed.** `pg_catalog` and `information_schema` appear in `ext` on both engines, and
   Trino's Iceberg connector adds `system` to `ice` (J8a). Filtering them is a product choice for Task 3. The
   data says only that they are there.
5. An Iceberg view written by StarRocks (no Trino dialect) DESCRIBEs on Trino as **zero columns, no error** (J13,
   J14). This does not affect browse: egret catalogs are read from the warehouse, not the engine. It is recorded
   because it is another "empty is not empty" case.

### 1.2 Stack and deviations

- **Image.** `egret-backend:vg-dgfe1`, built from `2c8bfcfc` (the predictions commit `134fa1e2` changes only this
  file). The build was cold and took 37 min. Free disk was 45.9 GiB at its lowest, against a floor of 30 GiB.
- **DB and containers.** DB `egret_dgfe_live`: `migrate`, `init`, `serve`. Log `kubernetes unavailable; engine
  provisioning disabled`. Containers on `vg-net`:
  - `vg-egret` (`serve`, `catalog.public_url = http://vg-egret:8096`);
  - `vg-rustfs` (`rustfs/rustfs:1.0.1`);
  - `vg-pg` (`postgres:16`, the external database);
  - `vg-trino-coord` (Trino 483, `nodeVersion 483`, digest `db58cc93…`, host
    `vgtr-coordinator-6d5f7c9b8-ab1cd`);
  - `vg-trino-imp` (same image, host `vgti-coordinator-7c8d9e6f5-cd2ef`);
  - `vg-sr` (`allin1-ubuntu@sha256:5cbf09c4…`, `4.1.6-6862092`, `access_control = ranger`, Ranger poll 5 s).
- **Catalogs.** Registry `ice` (egret Iceberg, warehouse `ice`) and `ext` (kind `jdbc`, `postgresql`, host
  `vg-pg`, database `extdb`, password in an egret secret). Both were created through the API.
  - StarRocks attached both through egret's own `POST /engines/{id}/catalogs/{cid}` (`applied`; `SHOW CATALOGS`
    → `ext Jdbc`, `ice Iceberg`).
  - The Iceberg data (`ice.s.t`, view `ice.s.v_t`) was created as StarRocks `root` before Ranger was switched on.
- **DEV-1 (SQL): engine rows.** `vgtr` (JWT spec, profile `vgtr`, tag profile `vgtr-tag`), `vgti` (`delegationMode:
  impersonate`, profile `vgti`) and `vgsr` were inserted as `provisioned`, `running`. `POST /engines` needs
  Kubernetes.
- **DEV-2 (SQL): Trino attachments.** `vgtr`/`vgti` → `ice`/`ext` were inserted as `applied`. The Trino attach
  needs Kubernetes.
  - Trino catalog files were hand-written:
    - `ext`: `connector.name=postgresql`, `connection-url` = `jdbc_url`'s rendering;
    - `ice`: `render_trino`'s Egret shape, with a catalog client minted through `POST /warehouses/{id}/clients`
      (role `catalog` `read`);
    - `tpch`: on the engine only, the control for J9.
- **DEV-3 (SQL): the impersonate profile's seeds.**
  - The per-user baseline came from egret's own startup pass, after a restart (`seeded=2 failed=0`, `vgtr` and
    `vgti`).
  - On `vgti`, these were inserted as `infra`, then `policy_version` was bumped:
    - the delegator's `egret-delegator-impersonate` (`trinouser = *`, `impersonate`, user `egret_delegator`);
    - the 13 `egret-admin - …` all-access policies for user `admin`, built from the embedded Trino service-def
      the way `seed_admin_all_access` builds them.

    Engine creation writes all of these, but it needs Kubernetes.
- **DEV-4 (identity).** The JWTs for `egret-lineage` and `vcplain` were signed with egret's active key from
  `catalog.oidc_signing_keys`, with `lineage_claims`'s claim set. Each request carried `X-Trino-User` equal to
  the token's user and `X-Trino-Session: query_max_run_time=15s`. `vcplain` is not an egret user. It only stands
  for "a name with no grant".
- **DEV-5 (config).** The Ranger poll is 5000 ms on both Trino engines (rendered: 30000).
- **DEV-6 (impersonate rendering).** `vgti` ran `http-server.authentication.type=PASSWORD` over TLS 8443, with a
  `password.db` holding only `egret_delegator`. Requests were made directly with Basic auth plus `X-Trino-User`,
  which is the shape of `egret-query`'s impersonate connection. They did not go through egret's query API.
- **Path of the queries.** Every listing query in §1.3 went directly to the engines (Trino `/v1/statement`,
  StarRocks `mysql` inside `vg-sr`) with egret's identity shapes: the lineage JWT claims, Basic `egret_delegator` +
  `X-Trino-User`, StarRocks `root` without a password. None went through egret's query API, lineage runner or MV
  lister code.
- **What egret's own code did.** It served the lineage transform to the `vgtr` pod-shaped `pluginId`. Its cache
  file holds `egret-lineage-read-ext`, `egret-lineage-read-ice`, the `query_max_run_time` allow and 12
  override-priority denies, at `policyVersion 5`. It served `vgti` the stored policies only (17, no lineage
  policy) and `vgsr` its compiled set. It also ran the StarRocks attach.

### 1.3 Every cell

**Trino JWT engine `vgtr`, JWT `egret-lineage`.**

| id | statement | observed | verdict |
|---|---|---|---|
| J1 | `SHOW SCHEMAS FROM ext` | `empty_s`, `information_schema`, `pg_catalog`, `public`, `sales` (2.33 s, first query) | mismatch: `pg_catalog` is listed |
| J2 | `SHOW TABLES FROM ext.sales` | `mv_orders`, `orders`, `v_orders` (0.80 s) | match |
| J3 | `SHOW TABLES FROM ext.empty_s` | empty set, no error (0.38 s) | match |
| J4 | `DESCRIBE ext.sales.orders` | `id integer`, `amount decimal(10,2)`, `note varchar`; columns `Column, Type, Extra, Comment` (0.56 s). `SHOW COLUMNS FROM ext.sales.v_orders` → `id`, `amount` | match |
| J5 | `SELECT schema_name FROM ext.information_schema.schemata` | as J1 (0.30 s) | mismatch (as J1) |
| J6 | `SELECT table_name, table_type FROM ext.information_schema.tables WHERE table_schema = 'sales'` | `mv_orders BASE TABLE`, `orders BASE TABLE`, `v_orders BASE TABLE`; `information_schema.views` for `sales` is empty | match |
| J7 | `SELECT column_name, data_type FROM ext.information_schema.columns WHERE …` | as J4; one statement for the whole schema (J15) also works | match |
| J8 | `ice` | `SHOW SCHEMAS` → `information_schema`, `s`, `system`; `SHOW TABLES FROM ice.s` → `t`, `v_t`; `DESCRIBE ice.s.t` → `id integer`, `v varchar`; `information_schema.tables` → `t BASE TABLE`, `v_t VIEW` | mismatch: `system` is listed (rest as predicted) |
| J9 | `SHOW SCHEMAS FROM tpch` (not attached in the registry) | `Access Denied: Cannot access catalog tpch` (`PERMISSION_DENIED`) | match |
| J10 | J1 as `vcplain` | `Access Denied: Cannot access catalog ext`; the `information_schema.schemata` form too | match |
| J11 | `SELECT * FROM system.metadata.materialized_views` | `Access Denied: Cannot access catalog system`. `SHOW CATALOGS` → `ext`, `ice` only | match |
| J12 | durations | 0.07–2.33 s, all well inside 15 s; J1 (the first query on a fresh engine) took 2.33 s, over the predicted 2 s | mismatch (J1 only) |

**Trino impersonate engine `vgti`, Basic auth `egret_delegator`.**

| id | `X-Trino-User` | `SHOW SCHEMAS FROM ext` | verdict |
|---|---|---|---|
| I1 | `egret_delegator` | `Access Denied: Cannot access catalog ext`; `information_schema.schemata` the same; `SHOW CATALOGS` → **empty set** | match |
| I2 | `egret-lineage` | `Access Denied: Cannot access catalog ext` (also the `information_schema` form) | match |
| I3 | `vcplain` | `Access Denied: Cannot access catalog ext` | match |
| I4 | `admin` | the J1 rows (so `pg_catalog` too); `SHOW TABLES FROM ext.sales` and `DESCRIBE ext.sales.orders` as J2/J4; `SHOW CATALOGS` → `ext`, `ice`, `system`, `tpch` | mismatch: lists, as predicted, but the predicted rows (J1's) omitted `pg_catalog` |

**StarRocks `vgsr`, `mysql -uroot` (no password, as `fe_target` with no root secret).**

| id | statement | observed | verdict |
|---|---|---|---|
| S1 | `SHOW DATABASES FROM ext` | `empty_s`, `information_schema`, `pg_catalog`, `public`, `sales` (0.25 s) | match |
| S2 | `SHOW TABLES FROM ext.sales` | `mv_orders`, `orders`, `v_orders` | match (`mv_orders` listed) |
| S3 | `SHOW TABLES FROM ext.empty_s` | empty set | match |
| S4 | `DESCRIBE ext.sales.orders` / `SHOW FULL COLUMNS FROM ext.sales.orders` | `id INT`, `amount DECIMAL(10,2)`, `note VARCHAR(1048576)` (columns `Field, Type, Null, Key, Default, Extra, Comment`) / lowercase `int`, `decimal(10,2)`, `varchar(1048576)` | match |
| S5 | `SHOW FULL TABLES FROM ext.sales` | three rows, all `BASE TABLE` (column `Table_type`) | match |
| S6 | `ext.information_schema.tables` / `schemata` / `columns` | `tables` as S5; `schemata` as S1; `columns`: `DATA_TYPE` `int`/`decimal`/`varchar`, `COLUMN_TYPE` `int(11)`/`decimal(10, 2)`/`varchar(1048576)` | match |
| S7 | `ice` | `SHOW DATABASES FROM ice` → `information_schema`, `s`; `SHOW FULL TABLES FROM ice.s` → `t BASE TABLE`, `v_t BASE TABLE`; `DESCRIBE ice.s.t` → `id INT`, `v VARCHAR(1073741824)` (3.48 s) | match |
| S8 | as `vcnobody` (no grant) | **empty set** for `SHOW DATABASES FROM ext` and `SHOW TABLES FROM ext.sales`. `DESCRIBE ext.sales.orders` → `ERROR 5204 … the ANY privilege(s) on TABLE orders`. `information_schema.schemata` → `ERROR 5204 … SELECT privilege(s) on COLUMN SCHEMA_NAME` | **mismatch**: the listing statements filter to nothing instead of refusing |
| S9 | Ranger and `root` | the only `vgsr` policy is the compiled `select` on `ice.*.*.*` for `admin`; nothing names `root`, and root listed both catalogs | match |

### 1.4 The statements that work (for Task 3)

**Shape only.** The battery ran these with unquoted identifiers and string literals on simple lowercase names
(`ext`, `sales`, `orders`). Quoting/escaping (Trino `"…"`, StarRocks backticks, mixed-case, hyphens, quotes or
backslashes in names) and parameter binding were **not measured**; Trino's statement protocol has no `?` binding.
Task 3 uses the engines' existing quoting helpers and literal escaping. Below, `c`, `s`, `t` stand for the
catalog, schema and table, and `'s'`, `'t'` for escaped string literals.

| engine, identity | schemas | tables (+ kind) | columns |
|---|---|---|---|
| Trino JWT, `egret-lineage` (lineage token) | `SHOW SCHEMAS FROM c` (column `Schema`) | `SELECT table_name, table_type FROM c.information_schema.tables WHERE table_schema = 's'` (`BASE TABLE` / `VIEW`). `SHOW TABLES FROM c.s` (column `Table`) also works but carries no kind | `DESCRIBE c.s.t` → columns `Column, Type, Extra, Comment`, types lowercase Trino types (`integer`, `decimal(10,2)`, `varchar`); or `SELECT column_name, data_type FROM c.information_schema.columns WHERE table_schema = 's' AND table_name = 't' ORDER BY ordinal_position` |
| StarRocks, `root` (the MV lister's `fe_target`) | `SHOW DATABASES FROM c` (column `Database`) | `SHOW FULL TABLES FROM c.s` → columns `Tables_in_<s>, Table_type` (`BASE TABLE` for everything measured) | `SHOW FULL COLUMNS FROM c.s.t` → `Field, Type, Collation, Null, Key, Default, Extra, Privileges, Comment`, types lowercase (`int`, `decimal(10,2)`, `varchar(1048576)`); `DESCRIBE c.s.t` → `Field, Type, Null, Key, Default, Extra, Comment`, types UPPERCASE (`INT`, `DECIMAL(10,2)`, `VARCHAR(1048576)`) |
| Trino impersonate | none without a new grant → `notListed` (`engine_error`) | — | — |

Trino's errors carry `errorName` (`PERMISSION_DENIED`, `SCHEMA_NOT_FOUND`, `TABLE_NOT_FOUND`). StarRocks's carry
codes `5204`, `5501` and `1064`.

**Also observed, not in the battery files** (`dgfe1/{jwt,imp,sr}-battery.txt`): read from logs, caches and the DB
during the run.
- `vgtr`'s plugin cache `trino_vgtr.json` at `policyVersion 5`: the three baseline policies, `egret-lineage-read-ext`,
  `egret-lineage-read-ice`, `egret-lineage-alter-query_max_run_time` and 12 override-priority `egret-lineage-deny-*`
  policies. Trino log `Switched policy engine to [5]`.
- `vgti`'s plugin cache at `policyVersion 6`: 17 policies, none named `egret-lineage-*`.
- egret log `per-user Ranger baseline pass seeded=2 failed=0` after the restart.
- StarRocks `SHOW CATALOGS` after egret's attach: `default_catalog`, `ext Jdbc`, `ice Iceberg`. The only `vgsr`
  policy was `perm-grant-…` (`select` on `ice.*.*.*` for `admin`, from the warehouse's workspace assignment).
- The Trino plugin's `Error getting Roles; service not found … response=404` at startup (known, T10-O2).

### 1.5 Left behind

- **Removed:**
  - containers `vg-egret`, `vg-rustfs`, `vg-pg`, `vg-trino-coord`, `vg-trino-imp`, `vg-sr`;
  - network `vg-net`;
  - DB `egret_dgfe_live` (dropped);
  - image `egret-backend:vg-dgfe1`.

  `docker ps -a`, `docker network ls` and `docker images` show no `vg-` entry.
- **Kept:**
  - BuildKit cache, 8.1 GB, all reclaimable and almost all from this build;
  - the scratchpad `dgfe1/`: battery outputs, configs and the public cert.

  Secrets were deleted from the scratchpad: env files, passwords, S3 key, catalog client, signing key, TLS key
  and keystore, cookie jar.
- **Untouched:** DB `egret`, every `egret-dev-*` container, `egret-w7*-postgres`, ports 5173, 5000 and 8080.
  Free disk at the end: 46 GiB. No product code changed.

## 2. The live browser check of the Data area (Task 12)

Measurement only: defects are recorded here and fixed in a later fix round.

### 2.0 Predictions (committed before anything was observed)

Written from the code at `8d9e4fa8` and §1, before any container of this run was started. A wrong prediction is a
finding. Nothing in this subsection is edited after observing.

**Setup the predictions are about.** §1.2's stack, rebuilt from `8d9e4fa8` (image `egret-backend:vg-dgfe12`, DB
`egret_dgfe_live`, containers `vg-*` on `vg-net`). The frontend is this branch's vite dev server on a free port,
proxying `/api`, `/catalog` and `/oauth` to the `vg-egret` container. The browser is Playwright, logged in once as
`admin@egret.local`.
- Engines: `vgtr` (Trino 483, JWT, one coordinator), `vgsr` (StarRocks 4.1.6, Ranger), `vgti` (a Trino row in
  impersonate mode; no container is needed, because it is never dialled).
- Catalogs:
  - `ice` (egret Iceberg): `s.pii(id, ssn, name)`, `s.pub(id, v)`, `s.t(id, v)` and a view `s.v_t`, written as
    StarRocks `root` before Trino was attached. Attached to `vgtr` and `vgsr`.
  - `ext` (jdbc/postgresql, database `extdb` on `vg-pg`): attached to `vgtr` and `vgsr`, so browse lists it **on
    Trino** (Trino is chosen first).
  - `exsr` (the same database, a second registry catalog): attached to `vgsr` only, so browse lists it **on
    StarRocks as `root`**.
  - `exti` (the same database): attached to `vgti` only.
- `extdb` holds §1's `sales` (`orders`, `v_orders`, `mv_orders`) and `empty_s`. It also holds a schema `system`
  with a table `t_sys`, and six odd-name schemas, each with one table:
  `q"s`/`t"q`, ``b`s``/``t`b``, `d.s`/`t.d`, `bs\s`/`t\b`, `MixS`/`MixT`, `sp s`/`t s`.
- Users: `vcplain` and `vccleared` (member of platform role `vg_clear`, the clearance of tag `vgsecret`), and
  `vcsteward` (platform role `vg_steward`: `policy:read`, `policy:update`, `tag:read`; no `catalog_registry:*`, not
  a platform admin). StarRocks users of the same names exist.

**P1. Odd names (review item 1).**

| id | where | prediction |
|---|---|---|
| P1a | `ext` on Trino, catalog level | `b`s`, `bs\s`, `d.s`, `empty_s`, `pg_catalog`, `public`, `q"s`, `sales`, `sp s`, and `mixs` (**lowercased** by Trino's JDBC connector, not `MixS`). `system` and `information_schema` are absent |
| P1b | `ext` on Trino, each odd schema expanded | its one table listed; DESCRIBE answers the columns. Not confident about `bs\s` |
| P1c | `ext.mixs` on Trino | `notListed` `engine_error` (Trino cannot map the lowercased name back), never an empty branch. Not confident: it may instead list `mixt` |
| P1d | `exsr` on StarRocks (`root`) | the names exactly as Postgres stores them, `MixS` in mixed case, plus `system`. Each table and its columns list. Not confident about the backtick and the backslash names, which StarRocks passes through its JDBC metadata calls |
| P1e | URLs | each odd name opens its object page from the tree and survives a reload: the page shows the same name, kind and columns. A name containing `.` cannot be typed in Go to path, because the box splits on dots. `d.s.t.d` opens schema `d`, object `t.d`, or no page, never the right object |
| P1f | header path | the dotted path `ext.d.s.t.d` is printed as is, with no quoting, so it is ambiguous (display only) |

**P2. `system` in a Postgres catalog (item 2).** Trino `ext`: `system` is **hidden**, a known limit. Go to path
`ext.system.t_sys` opens the table, and its columns list. StarRocks `exsr`: `system` is listed.

**P3. Impersonate alone (item 3).** Expanding `exti` shows the reason as text, `vgti runs in impersonate mode; egret
has no identity there that can list catalogs`, with a Go to path link. It is not an empty branch and not an error
row. The object page's Overview says the same.

**P4. 390 px and dark mode (item 4).**
- At 390 px the tree is folded behind "Show catalogs" and closes when an object is opened.
- The header wraps: name, then kind and path, then actions. The tab list scrolls sideways inside itself.
- The page itself never scrolls sideways, on any tab, in the column panel (a right sheet at most `max-w-md`), in
  Who can read? or in Add tag.
- Not confident: a long explain result in Who can read? may push its own table wider than the dialog. The dialog
  should scroll, not the page.
- Dark mode: every surface uses tokens, so text stays readable. No white panel appears.
- Screenshots: 390 px wide (× DPR), never a 16 or 32 px icon crop.

**P5. Keyboard only (item 5).**
- Tab reaches Go to path, then the filter, then exactly one tree row.
- Down and Up move between rows. Right expands, or enters an expanded row. Left collapses, or goes to the parent.
  Enter opens the object. Enter on a `notListed` row puts `catalog.` in Go to path and focuses it.
- Tabs follow Radix: Left and Right move and select, and the URL's `?tab=` follows.
- Escape closes the column panel and removes `?column=`. Focus goes back to the column link that opened it. When
  the panel was opened by URL there is no trigger, so focus goes to the body (not confident).

**P6. `readersIncomplete` (item 6).** `ice.s.v_t` was written before Trino was attached, so egret recorded no lineage
for it. So the global unknown-lineage count is ≥ 1, and **every** table's Lineage tab shows "This list may be
incomplete" in every catalog, including tables no view reads.

**P7. Writes from the page against real engines (item 7, spec §2.7).**
- **Grant.** Grant access on `ice.s.pub`, `read` for `vcplain`. A toast appears, and the Permissions tab shows the
  grant as "here". Within one poll (5 s), `SELECT * FROM ice.s.pub` as `vcplain` goes from refused to rows on
  Trino (`Access Denied` → rows) and on StarRocks (`5204` → rows).
- **Tag.** Add tag `vgsecret` on column `ice.s.pii.ssn` (from the column panel). It appears on the column's row in
  Overview and in the panel. It does not appear in the header, which shows table tags only. Add tag on the table
  `ice.s.pub` puts the tag in the header.
- **Mask.** From Masks & filters on `ice.s.pii`, add a `MASK` rule on column `name` for everyone. A real
  `SELECT name` as `vcplain` returns masked values (`xxxx`-shaped) on both engines within one poll.
- **Who can read? on `ice.s.pii` column `ssn`** (`vcplain` and `vccleared` hold `read` on `pii`):

  | user | explain, Trino profile | Trino query | explain, StarRocks profile | StarRocks query |
  |---|---|---|---|---|
  | `vccleared` | allowed | real values | allowed | real values |
  | `vcplain` | denied (the clearance deny) | `Access Denied … [ssn]` | `MASK_NULL`, "returns NULL" (C2) | `NULL` ×N, no error |

**P8. Steward (item 8)**, `vcsteward` with no `catalog_registry:read`.
- The tree lists all four catalogs from `GET /governance/catalogs`, and the Permissions tab works.
- Grant access is shown, and GrantDialog's catalog picker lists the four catalogs.
- Add tag is hidden: it needs `catalog_registry:update`. Who can read? is shown.
- Data permissions and Mask & filter rules render with Open in Data links.

**P9. Open in Data (item 9).**
- The `pub` grant's link opens `/governance/data/ice/s/pub?tab=permissions`.
- The `name` mask rule's link opens `/governance/data/ice/s/pii?column=name&tab=masks`: the Masks tab with the
  panel open on `name`.
- A grant on `ice.s.*` stops at the schema.

**P10. Brief steps.** Browsing `ice` lists `s` and its four objects from the warehouse. `v_t` is a View, and its
Overview shows a security mode and an unknown lineage state. `ext.sales.v_orders` and `mv_orders` show as
**Table** (§1.1 fact 1). Opening a column opens the panel with its tags, rules, grants and Who can read.

### 2.1 Result (read this first)

Measured 2026-10-17 between 07:21 and 07:52 UTC, on an image built from `8d9e4fa8` (predictions commit `928123c4`
changes only this file).

**The Data area works end to end against real engines.**
- Browsing `ice` (from the warehouse), `ext` (on Trino as `egret-lineage`) and `exsr` (on StarRocks as `root`)
  lists real schemas, tables, views and columns.
- Grant, tag and mask written from the page reach both engines within one poll.
- Who can read? agrees with a real query on both engines for a cleared user and an uncleared one.

**12 defects**, none Critical. They are listed in §2.4.
- The two that break the spec's "never an empty branch" are silent empty listings: a mixed-case Postgres schema on
  Trino (D1), and a schema with a backslash on StarRocks (D2).
- The worst UX defect: a grant created from a schema page never appears on that page, nor on the page its "Open in
  Data" link opens (D4).

24 predicted cells in §2.3 (the row "P5 390" is a finding, not a prediction): 18 matched and 6 did not (P1c,
P1d, P4 clipping, P5 panel focus, P9 pattern, P10's lineage wording).

### 2.2 Stack and deviations

- **Image and frontend.**
  - Image `egret-backend:vg-dgfe12` was built from `8d9e4fa8`: a warm release build, 11.5 min. Free disk stayed
    ≥ 90 GiB.
  - Vite 8.2.1 ran from this worktree on port 5291, through a scratch config that pointed the `/api`, `/catalog`
    and `/oauth` proxies at `localhost:18096`.
  - The browser was the Playwright MCP Chromium, at 1280×900 and 390×844, in light and dark mode.
- **DB and containers.** DB `egret_dgfe_live` (`migrate`, `init`, `serve`), with the log line `kubernetes
  unavailable; engine provisioning disabled`. Containers on `vg-net`:
  - `vg-egret`;
  - `vg-rustfs` (`rustfs/rustfs:1.0.1`);
  - `vg-pg` (`postgres:16`, database `extdb`);
  - `vg-trino-coord` (Trino 483 `db58cc93…`, host `vgtr-coordinator-6d5f7c9b8-ab1cd`, JWT/TLS rendering of §1.2);
  - `vg-sr` (allin1 4.1.6, `access_control = ranger`).
- **Catalogs** were created through the API:
  - `ice` (warehouse `ice`);
  - `ext`, `exsr` and `exti` (`jdbc`/`postgresql` → `vg-pg:5432/extdb`, password in an egret secret).

  StarRocks attached `ice`, `ext` and `exsr` through egret's own `POST /engines/{id}/catalogs/{cid}` (`applied`).
  The Iceberg data (`ice.s.pii`, `pub`, `t`, view `v_t`) was written as StarRocks `root` before Ranger was
  switched on and before Trino was attached.
- **DEV-1 (SQL): engine rows.** As in §1.2:
  - `vgtr` (JWT, profile `vgtr` with tag profile `vgtr-tag`);
  - `vgti` (`delegationMode: impersonate`, profile `vgti`, **no container**: browse never dials it);
  - `vgsr` (profile `vgsr`).
- **DEV-2 (SQL): Trino attachments.** `vgtr`→`ice`/`ext` and `vgti`→`exti` were inserted as `applied`. Then `PUT
  /engines/{vgtr}/workspaces` materialised the delegated `query.databases` row the Trino lister needs. The per-user
  baseline came from a restart (`seeded=2 failed=0`).
- **DEV-3 (SQL): usernames.** Users created through `POST /users` have no `users.username`. The principal picker
  shows them as "no allocated username yet" and cannot pick them. `users.username` was set to the email local part
  for `vcplain`, `vccleared` and `vcsteward` (the slug `ensure_username` would allocate).
- **DEV-4 (identity): the real queries.**
  - Trino queries went straight to Trino, with JWTs signed by egret's active key and `engine_claims`'s claim set.
  - StarRocks queries ran as the same-named StarRocks users through `mysql` inside `vg-sr`.
  - None went through egret's query API.
- **DEV-5 (config).** The Ranger poll was 5000 ms on both engines.
- **DEV-6 (setup).** The first `vgtr-tag` profile was created with type `trino`; the API requires type `tag`. It
  was deleted and recreated, before any engine existed.
- **DEV-7 (harness).** The shared Playwright browser still carried route intercepts from an earlier session, which
  answered `/src/api/*.ts` with `[]`. `unrouteAll` cleared them before any measurement.
- No privilege of `egret-lineage` or of StarRocks `root` was changed. Logins: admin twice (curl, browser) and
  `vcsteward` once.

### 2.3 Every cell

| id | observed | verdict |
|---|---|---|
| P1a | `ext` on Trino lists `b`s`, `bs\s`, `d.s`, `empty_s`, `mixs`, `pg_catalog`, `public`, `q"s`, `sales`, `sp s`. `system` and `information_schema` are absent | match |
| P1b | every odd schema on Trino lists its table. DESCRIBE answers each one, including `c"1`, `` c`1 ``, `c.1`, `c\1`, `c 1` | match |
| P1c | `ext.mixs` shows **"No tables or views"**. Trino itself answers an empty set to `information_schema.tables` and to `SHOW TABLES FROM ext.mixs` / `ext."MixS"`. `DESCRIBE ext.mixs.mixt` → `TABLE_NOT_FOUND` | **mismatch → D1** |
| P1d | `exsr` on StarRocks lists every schema as stored (`MixS`, `system`). `MixS.MixT`, `q"s`, `d.s`, `sp s` and `system` list their tables and columns. ``b`s`` → `engine_error` (StarRocks itself: ``SHOW FULL TABLES FROM `exsr`.`b``s` `` → `Unknown database 'bs'`). `bs\s` → **"No tables or views"** (StarRocks itself answers `Empty set`). `MixT`'s column is listed as `"MixC"`, with literal quotes | **mismatch → D2, D3** |
| P1e | each odd-name object opened from the tree and showed the same header, kind and columns after a reload. The URLs were `ext/q%22s/t%22q`, `ext/b%60s/t%60b`, `ext/d.s/t.d`, `ext/bs%5Cs/t%5Cb` and `ext/sp%20s/t%20s`. Go to path `ext.d.s.t.d` opened `ext/d.s.t/d`: "Kind unknown", `engine_error` (`Schema 'd.s.t' does not exist`) | match (D8) |
| P1f | the header prints `ext.d.s.t.d` | match |
| P2 | Trino hides `system`. Go to path `ext.system.t_sys` opens the table with columns `id integer`, `s varchar`. StarRocks lists `system` | match |
| P3 | `exti` → the text "vgti runs in impersonate mode; egret has no identity there that can list catalogs", with a Go to path link. `exti.sales.orders` → the same text in Overview, and "Kind unknown" | match |
| P4 page | at 390 px, light and dark, on the index, the open tree, all 5 tabs, the column panel, Add tag and Who can read?: `scrollWidth == clientWidth == 390` every time. The tree is folded behind "Show catalogs" and closes on open (`aria-expanded=false`) | match |
| P4 dialog | Who can read? with the long `vcplain` result: the dialog is 778 px high with an inner `overflow-y-auto` region (2385/730 px) and no sideways scroll | match |
| P4 clipping | the app's icon rail stays at 390 px, leaving about 300 px. The tab list cuts "Masks & filter…", with Lineage and Details off-screen. The Masks table's Actions column (right edge 456 px) and the Overview's "Masking rules" column are clipped inside their scroll containers | **partly → D10** |
| P4 dark | screenshots readable, with no white surface | match |
| P5 tree | Tab order: Go to path, Go, the filter, then one row. Down/Up/Home/End move. Right expands, then enters. Left goes to the parent, then collapses. Enter opens the object. Enter on the `exti` notListed row puts `exti.` in Go to path and focuses it; typing `sales.orders` and Enter opens it | match |
| P5 tabs | Radix: Right/Left/Home/End move and select. Left on Overview wraps to Details. `?tab=` follows | match |
| P5 panel | Escape closes the panel and removes `?column=`, but focus goes to `<body>`, not to the `ssn` link that opened it. When the panel was opened by URL, focus also goes to `<body>`, as predicted | **mismatch → D6** |
| P5 390 | Enter on a tree row at 390 px opens the object and folds the tree; focus drops to `<body>` | finding D7 |
| P6 | "This list may be incomplete. 1 view in egret's catalogs has lineage egret does not know…" appears on `ice.s.pii`, `ice.s.pub` and `ext.sales.orders` (the last also adds "views defined in this catalog are not tracked"): on every table checked | match (D12) |
| P7 grant | Grant access on `ice.s.pub` for `vcplain` (preview: "Would recompile vgsr, vgtr"). Toast "Grant created."; the Permissions tab shows it under Here. Before: Trino `Access Denied: Cannot access catalog ice`, StarRocks `5204`. 10 s after: rows on both. On `ext.sales.orders` (as `vcsteward`): Trino denied → rows, StarRocks (`ext`) `5204` → rows; `exsr` (not granted) still `5204` | match |
| P7 tag | Add tag `vgsecret` on `ice.s.pii.ssn` from the panel: toast, the panel's Tags and the Overview row show it, the header shows "None on this object". Add tag `vgpub` on `ice.s.pub`: the header shows `vgpub` | match |
| P7 mask | rule `vg-mask-name` (`MASK` on `ice.s.pii.name`) from Masks & filters. The tab, the Overview's Masking rules column and the `name` panel ("by path") show it. 15 s later `vcplain` and `vccleared` read `xxxxx`, `xxx`, `xxxxx` on both engines | match |
| P7 who | `vccleared`: vgsr allowed, `MASK_NONE`; vgtr allowed. Real queries: real `ssn` on both engines. `vcplain`: vgsr "not access-checked … returns NULL (MASK_NULL)"; vgtr "Refused by a deny item" (clearance). Real queries: Trino `Access Denied: Cannot select from columns [name, id, ssn]`, StarRocks `NULL` ×3. On `ext.sales.orders`, `vcplain` is allowed on both (rows on both) and `vccleared` gets "No policy matched" (refused on both) | match |
| P8 | as `vcsteward`: the tree lists 4 catalogs and browses `ext`/`exti`/`ice`. Header: Grant access and Who can read?, with Add tag hidden. Permissions, Masks, Lineage and Overview tags all load. The GrantDialog catalog picker offers `exsr`, `ext`, `exti`, `ice`. Data permissions shows 4 Open in Data links and Mask & filter rules 1. Who can read? answers. No `/api` response ≥ 400 | match |
| P9 links | `pub` → `/governance/data/ice/s/pub?tab=permissions`. `vg-mask-name` → `/governance/data/ice/s/pii?column=name&tab=masks`, which lands with Masks & filters selected and the panel open on `name`. `ice.s.*` → `/governance/data/ice/s?tab=permissions`, labelled "opens the nearest level above its pattern" | match |
| P9 pattern | the schema page that link opens does not list the `ice.s.*` grant | **mismatch → D4** |
| P10 | `ice.s` lists `pii`, `pub`, `t`, `v_t` (View). `v_t`: security mode INVOKER, "Owner cleared: Not known", lineage **"Failed: the view version carries no Trino SQL representation"** (predicted "unknown"). `ext.sales.v_orders` and `mv_orders` show as Table. Column panel as predicted | match except lineage wording |

### 2.4 Defects

Severity is a guess. Screenshots are in the scratchpad, `dgfe12/shots/` (390×844 or 1280×900, checked with `sips`).

- **D1 (Important): silent empty on Trino for a mixed-case Postgres schema.** `ext.mixs` (Postgres `MixS` holding
  `MixT`) shows "No tables or views". Trino's JDBC connector lowercases the name and then answers an empty set to
  both `information_schema.tables` and the lister's `SHOW TABLES` probe, so the probe cannot tell it from a real
  empty schema. Repro: Postgres `CREATE SCHEMA "MixS"; CREATE TABLE "MixS"."MixT"(id int)`, attach to a JWT Trino,
  expand `ext` → `mixs`. Candidate fix: render JDBC catalogs with `case-insensitive-name-matching=true`, or flag
  any lowercased name.
- **D2 (Important): silent empty on StarRocks for a schema whose name holds a backslash.** `exsr.bs\s` shows "No
  tables or views". StarRocks `` SHOW FULL TABLES FROM `exsr`.`bs\s` `` answers `Empty set`: the JDBC metadata
  pattern reads `\` as an escape. Related: `pg_catalog` lists "No tables or views" on both engines (the connectors
  drop system tables).
- **D3 (Minor): StarRocks cannot list a schema whose name holds a backtick.** ``exsr.b`s`` → `engine_error`.
  StarRocks reads `` `b``s` `` as `bs` (`Unknown database 'bs'`). This is an engine limit, and the message is
  honest. Also, StarRocks lists a mixed-case Postgres column as `"MixC"`, with literal quotes, shown verbatim.
- **D4 (Important): a grant on a pattern below the page is invisible where it was made, and where its link lands.**
  Repro: `/governance/data/ice/s?tab=permissions`, Grant access, Table `*`, include `vcsteward`, Create. The toast
  says "Grant created.", but the tab still shows only the catalog's workspace grant. The grants on `s.pub` and
  `s.pii` are not listed either. Data permissions' "Open in Data" for this grant opens the same tab. The table
  pages list it under "Matching a pattern".
- **D5 (Minor): a workspace grant is shown with a raw id.** `workspace_members 220f80e1-17f9-4653-be41-26c7af644963
  can read ice.*` appears in the Permissions tab and in Data permissions, although the same row says "Set by
  workspace assignment · Default".
- **D6 (Minor, a11y): the column panel loses focus on close.** Focus the `ssn` link in Overview, press Enter, then
  Escape: `?column=` is removed, but `document.activeElement` is `<body>`, not the link. (WCAG 2.4.3.)
- **D7 (Minor, a11y): at 390 px, opening from the tree loses focus.** Show catalogs, arrow to a row, Enter: the tree
  folds and focus is `<body>`.
- **D8 (Minor): Go to path cannot name an object whose name holds a dot.** `ext.d.s.t.d` opens `ext/d.s.t/d`, which
  shows Kind unknown and an engine error. The tree link to `ext.d.s.t.d` works.
- **D9 (Minor, pre-existing `PrincipalPicker`, reached from the object page's Grant access).**
  - After one principal is picked, the suggestion list does not reopen while the input keeps focus. A second user
    cannot be added without a blur and a refocus more than 150 ms later.
  - A user who never queried has no username and cannot be picked (DEV-3).
- **D10 (Minor, 390 px): content is clipped inside scroll containers with no affordance.** The icon rail leaves
  about 300 px. The tab list hides Lineage and Details, and the Masks table's edit and delete actions are off-screen
  (`390-light-tab-masks.png`). The page itself never scrolls sideways.
- **D11 (Cosmetic):** the Lineage tab reads "Failed: egret could not work out what this view reads. the view version
  carries…" (a lowercase "the" after the full stop).
- **D12 (Minor, design): `readersIncomplete` is on every table.** One egret view with unknown lineage, here a
  StarRocks-written view with no Trino SQL, puts "This list may be incomplete" on every table of every catalog,
  external ones included. On an install that has any such view, the warning is permanent.

**Not measured:**
- a view with known lineage (so the Lineage tab's reader list with entries);
- a Trino Iceberg MV;
- a timeout or `engine_not_running` live;
- the egret query API path for the real queries (DEV-4);
- browsers other than Chromium.

### 2.5 Left behind

- **Removed:**
  - containers `vg-egret`, `vg-rustfs`, `vg-pg`, `vg-sr`, `vg-trino-coord`;
  - network `vg-net`;
  - DB `egret_dgfe_live`;
  - image `egret-backend:vg-dgfe12`;
  - the vite process.

  `docker ps -a`, `docker network ls` and `docker images` show no `vg-` entry.
- **Secrets deleted from the scratchpad:** env files, the S3 and Postgres passwords, the signing key, the TLS key and
  keystore, the catalog client and the cookie jar.
- **Kept:** the scratchpad `dgfe12/` (screenshots, `extdb.sql`, `odd-cols.txt`, configs, the public cert). BuildKit
  cache grew from 8.1 to 9.6 GB (reclaimable).
- **Untouched:** DB `egret`, every `egret-dev-*` container, `egret-w7*-postgres`, `egret-grpa-starrocks`, ports 5173,
  5000 and 8080. Free disk at the end: 82 GiB. No product code changed.
