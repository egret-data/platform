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
