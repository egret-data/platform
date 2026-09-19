# SP3 — Catalog registry

**Status:** design, awaiting review
**Programme:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Followed by:** SP4 (one egret data-permission model over registry catalogs), SP5 (egret-enforced engines)
**Delivery:** three PRs — SP3a backend, SP3b StarRocks, SP3c frontend

## 1. Problem

A catalog today is an `engine_mgmt.engine_catalogs` row owned by one engine. The same Postgres
connection used by two engines is configured twice, each with its own secrets; the row stores raw
Trino `.properties`, which egret itself cannot interpret; StarRocks ignores every row except the
primary warehouse; and warehouses belong to exactly one workspace (`catalog.warehouses.workspace_id
NOT NULL`) while engines are platform-wide. The user asked for a **Catalogs** section under
Infrastructure that holds connection definitions for every catalog type, absorbs Warehouses, and
is what engine provisioning attaches.

## 2. Decisions (taken with the user, 2026-09-19)

| # | Decision |
|---|---|
| D1 | A catalog is a **platform resource** (Control Plane, like storage and engines). Platform admins create it, attach it to engines, and **assign it to workspaces**. |
| D2 | Configuration is **typed per kind**, plus free-form **advanced properties per engine**. A `generic` kind carries raw properties for connectors egret does not model. |
| D3 | v1 types four families (§4.2): Iceberg (incl. the egret warehouse), JDBC, Hive/Delta/Hudi, Elasticsearch + TPC-H/TPC-DS. Everything else goes through `generic`, for **both** engines. |
| D4 | Editing a catalog that engines use is **applied immediately, after a confirmation** that lists the affected engines. |
| D5 | The **registry name is the catalog's name in SQL**, identical on every engine. SP4 generates one Ranger policy per catalog, so a catalog must not be called different things on different engines. |

## 3. What both engines can read

Verified in the vendored sources (`trino/plugin`, `trino/docs/src/main/sphinx/connector`;
StarRocks `fe/fe-core/.../connector/ConnectorType.java`, `docs/en/data_source/catalog`).

| Family | Kinds | Trino | StarRocks |
|---|---|---|---|
| Lakehouse | Iceberg (REST, Hive metastore, Glue) | `iceberg` | `iceberg` (`iceberg.catalog.type` = rest/hive/glue) |
| | Hive, Delta Lake, Hudi | `hive`, `delta_lake`, `hudi` | `hive`, `deltalake`, `hudi` |
| JDBC | PostgreSQL, MySQL, MariaDB | `postgresql`, `mysql`, `mariadb` | `jdbc` (MySQL/PostgreSQL since 3.0) |
| | Oracle, SQL Server | `oracle`, `sqlserver` | `jdbc` (since 3.2.9 / 3.3.1) |
| | ClickHouse | `clickhouse` | `jdbc` (experimental since 3.3.0) |
| Search | Elasticsearch | `elasticsearch` | `es` |
| Sample | TPC-H, TPC-DS | `tpch`, `tpcds` | `benchmark` (also SSB) |

Trino only: BigQuery, Snowflake, Redshift, SingleStore, Cassandra, MongoDB, Redis, Kafka, Druid,
Pinot, Prometheus, Loki, Google Sheets, Exasol, DuckDB, Ignite, OpenSearch, Lakehouse, JMX,
Memory, Blackhole, Faker, Thrift. StarRocks only: Paimon, Kudu, MaxCompute (ODPS), Fluss, Lance,
Unified. These are reachable through `generic`.

## 4. Design

### 4.1 Data model (new schema `registry`)

**`registry.catalogs`**

| Column | Type | Notes |
|---|---|---|
| `id` | uuid PK | |
| `name` | text UNIQUE NOT NULL | SQL name on every engine. CHECK `name ~ '^[a-z][a-z0-9_]{0,62}$'`. Immutable after creation. |
| `kind` | text NOT NULL | CHECK over the kinds in §4.2. |
| `config` | jsonb NOT NULL | Typed per kind, validated by a Rust enum on every write. Never holds a plaintext secret — only secret ids and storage-connection ids. |
| `advanced` | jsonb NOT NULL DEFAULT `{}` | `{"trino": {k: v}, "starrocks": {k: v}}`, rendered verbatim after the typed keys. |
| `description` | text | |
| `created_at`, `updated_at` | timestamptz | |

**`registry.engine_catalog_attachments`** — `(engine_id → engine_mgmt.engines ON DELETE CASCADE,
catalog_id → registry.catalogs ON DELETE RESTRICT, PRIMARY KEY both)`, plus
`catalog_client_id uuid NULL` (only for iceberg-egret: the per-engine catalog client egret mints,
as today), `apply_status text NOT NULL DEFAULT 'applied'` (`applied` | `failed`),
`apply_error text NULL`, `applied_at timestamptz NULL`.

**`registry.catalog_workspaces`** — `(catalog_id → registry.catalogs ON DELETE RESTRICT,
workspace_id → workspaces ON DELETE CASCADE, PRIMARY KEY both)`.

**The warehouse stays the backing store of an iceberg-egret catalog.** The catalog's `config`
names `warehouse_id`; lakekeeper, `catalog.catalog_roles`, `catalog_grants`, `catalog_clients`,
`catalog_tag_assignments` and `analytics.notebook_warehouse_settings` keep keying on the
warehouse and are untouched. `catalog.warehouses.workspace_id` is dropped; the workspace link
becomes a `catalog_workspaces` row. The lakekeeper warehouse name is independent of the registry
name and never changes.

**`engine_mgmt.engines.warehouse_id` becomes nullable** — an engine no longer needs a warehouse —
and is dropped once nothing reads it (SP3c).

### 4.2 Kinds and their typed configuration

| `kind` | `config` fields |
|---|---|
| `iceberg` | `metastore`: `egret` {`warehouse_id`} \| `rest` {`uri`, `warehouse`, `auth`: none \| oauth2 {`credential_secret_id`, `scope?`, `token_uri?`}} \| `hive` {`uri`} \| `glue` {`region`, `access_key?`, `secret_key_secret_id?`}; `storage_connection_id` (required unless `metastore = egret`, which uses the warehouse's) |
| `hive`, `delta_lake`, `hudi` | `metastore`: `hive` {`uri`} \| `glue` {…as above}; `storage_connection_id` |
| `jdbc` | `dialect`: postgresql \| mysql \| mariadb \| oracle \| sqlserver \| clickhouse; `host`, `port`, `database?`, `user`, `password_secret_id`, `ssl`: disable \| require \| verify_full {`ca_secret_id?`} |
| `elasticsearch` | `hosts[]`, `port`, `tls`, `user?`, `password_secret_id?` |
| `tpch`, `tpcds` | `scale?` (Trino `tpch.splits-per-node` etc. go in `advanced`) |
| `generic` | `engines`: subset of {trino, starrocks}; the connector name and every property live in `advanced.<engine>` |

A kind that an engine cannot read is refused when attaching (e.g. `generic` with only `trino` on
a StarRocks engine, `tpch` on StarRocks is rendered as `benchmark`).

### 4.3 Rendering

One pure function per target, each unit-tested per kind against hand-written expected output:

- `render_trino(catalog, attachment) -> TrinoCatalogFile` — `connector.name` + properties +
  env entries for secrets (`${ENV:CAT_<n>_<i>}`, resolved from egret secrets at apply time, as
  today). iceberg-egret renders the REST catalog with the attachment's minted client credential.
- `render_starrocks(catalog, attachment) -> StarRocksCatalogDdl` — `CREATE EXTERNAL CATALOG`,
  `ALTER CATALOG <name> SET (...)` (StarRocks rebuilds the connector in place:
  `CatalogMgr.alterCatalog`) and `DROP CATALOG`. JDBC renders `jdbc_uri`, `driver_class`,
  `driver_url` (egret ships a pinned driver URL per dialect in config), `schema_resolver`.
- SP5 adds a third target, egret's own connection.

`advanced.<engine>` keys are appended after the typed keys; a key that collides with a typed key
is refused at save time (so advanced cannot silently override what the form shows).

### 4.4 Attaching and detaching

`POST/DELETE /api/control-plane/engines/{id}/catalogs/{catalog_id}`:

- **Trino**: add/remove the attachment, rebuild the provision request through the existing
  `build_provision_req_with` → `apply_built` path (apply first, then commit; 502 on a rejected
  apply, nothing persisted). Trino rolls its pods, as for any catalog change today.
- **StarRocks**: run `CREATE EXTERNAL CATALOG` / `DROP CATALOG` through the same DDL channel the
  bootstrap uses; the attachment row is written only after the DDL succeeds.
- The StarRocks bootstrap (`spawn_starrocks_catalog_wire`) creates **every** attached catalog
  once the FE is ready, not only the primary warehouse, keeping its checkpoints (SP1 Part B).
- An iceberg-egret attachment mints a catalog client for that engine; detaching revokes it if
  egret minted it.

### 4.5 Editing a catalog that engines use

1. `GET /api/control-plane/catalogs/{id}/impact` returns the attached engines with what applying
   will do to each (`restart` for Trino, `in_place` for StarRocks).
2. The UI shows that list and asks for confirmation.
3. `PUT /api/control-plane/catalogs/{id}` validates and stores the new configuration — the
   registry is the source of truth — then applies it to each attached engine in turn (Trino:
   `apply_built` Update; StarRocks: `ALTER CATALOG SET`).
4. The response lists the outcome per engine. A failed engine gets `apply_status = failed` and
   `apply_error` on its attachment, is flagged in the UI, and has a **Retry** action
   (`POST …/engines/{id}/catalogs/{catalog_id}/apply`). A failure does not revert the catalog,
   because the other engines already hold the new configuration.

Also:

- **No rename.** `name` is the SQL identity.
- **Delete** is refused with 409 and the list of engines and workspaces while any attachment or
  assignment exists (same shape as engine delete in SP1 Part A).
- **Secrets are not pushed on rotation.** Rotating a secret does not change any catalog; the
  Secrets page lists the catalogs that reference a secret, and a catalog has **Re-apply to
  engines**, which runs step 3 with the unchanged configuration.
- **Standing rule from SP1 Part B holds:** egret never deletes a secret an engine may still hold.

### 4.6 Workspace assignment

`GET/PUT /api/control-plane/catalogs/{id}/workspaces`. In SP3 an assignment decides which
catalogs a workspace sees in data browsing and notebook warehouse selection (iceberg-egret only).
It does **not** yet restrict what an engine returns — that is enforcement, which is SP4 (Ranger
profiles) and SP5 (egret-enforced engines).

### 4.7 Permissions

New platform permissions `catalog_registry:read`, `catalog_registry:create`,
`catalog_registry:update`, `catalog_registry:delete` (assign-to-workspace, re-apply and Retry use
`catalog_registry:update`). Migration grants each platform role the `catalog_registry:*`
counterpart of every `warehouse:*` it holds (so `data_steward` gains `catalog_registry:read`).

The keys are deliberately not `catalog:*`: `catalog:read` already exists as a workspace-scope
permission (migration 0024, held by every workspace role, checked by the workspace engine view),
and permission keys are scope-blind strings in `rbac::is_known`, in the effective-permission lists
the frontend reads, in the admin permission UI and as Cedar actions — one key with two meanings
would be ambiguous in all of them. (It would **not** widen access: a non-admin platform grant is
bound to `resource == Platform::"root"` and can never satisfy a workspace-scope check — an earlier
draft of this section claimed otherwise, and that was wrong.) The workspace `catalog:read` is left
untouched.
`warehouse:*` stays until SP3c removes the Warehouses pages, then is dropped.

### 4.8 Migration of existing data

One migration plus a Rust backfill run at boot (the backfill needs the typed parser), writing a
report to the log and to a `registry.migration_report` table the Catalogs page shows once.

1. Every warehouse → an `iceberg` catalog with `metastore = egret`. Name: the warehouse name
   normalised to `^[a-z][a-z0-9_]*$` (lowercase, other characters → `_`, a leading digit gets a
   `w_` prefix), suffixed `_2`, `_3` … on collision. Its old `workspace_id` becomes a
   `catalog_workspaces` row. On the kind cluster today: 24 warehouses in one workspace, all
   distinct, none a valid SQL identifier — all 24 are renamed (e.g. `e2e-wh-1784519491585` →
   `e2e_wh_1784519491585`).
2. Every engine's primary `egret-warehouse` row → an attachment to that warehouse's catalog, with
   its catalog client carried over. **Exception, to keep saved SQL working:** if exactly one
   warehouse is attached as an engine primary across the platform, its catalog is named `egret`
   instead of its normalised name. If several are, they keep normalised names and the report
   lists every saved SQL object (datasets, charts, Data API endpoints, alerts, saved queries)
   whose text references `egret.`.
3. Every other `engine_catalogs` row → a catalog: a recognised kind whose properties parse becomes
   typed; anything else becomes `generic` with its properties preserved. Two engines with a
   same-named catalog and identical configuration share one catalog; a same name with different
   configuration gets an `_<engine>` suffix, reported.
4. Ranger tag export (`egret-policy/src/store.rs:221-264`) resolves the catalog name through the
   attachment instead of assuming the primary catalog is called `egret`.
5. `engine_mgmt.engine_catalogs` is dropped in SP3c, after the old engine-catalog endpoints are
   removed.

## 5. User interface (SP3c)

- **Control Plane → Infrastructure → Catalogs** replaces Warehouses; `/control-plane/warehouses/*`
  redirects to the matching catalog.
- **List**: name, kind (icon), attached engines, assigned workspaces, a warning when any
  attachment failed to apply.
- **Create**: pick a kind → typed form → **Advanced** tab per engine. Built from the existing
  config primitives (`ConfigSection`, `FieldLabel`, secret picker, storage-connection picker).
- **Detail tabs**: Configuration; Engines (attach/detach, apply status, Retry); Workspaces
  (assign); for iceberg-egret, the existing warehouse tabs move here (namespace browser, catalog
  roles, grants, clients, tags).
- **Engine detail → Catalogs** picks from the registry instead of the inline form.
- **Create-engine wizard**: choose catalogs to attach instead of choosing a warehouse.
- **Workspace detail (Control Plane)**: assigned catalogs.
- **Test** on a catalog: `SHOW SCHEMAS FROM <name>` through an attached engine when there is one;
  otherwise a TCP reachability check of the configured host/port. Full driver-level checks arrive
  with SP5.

## 6. Delivery

| PR | Scope | Keeps working |
|---|---|---|
| SP3a | Schema, typed config enum + validation, Trino rendering, migration + backfill + report, registry and attachment API, Trino attach/detach and edit propagation, permissions, regenerated client | The existing engine-catalog endpoints are reimplemented over the registry, so today's UI keeps working |
| SP3b | StarRocks rendering (create/alter/drop), attach/detach, propagation, generalised bootstrap | — |
| SP3c | Catalogs section and forms for the four families, engine attach tab, workspace assignment, wizard, redirects; removal of the old engine-catalog endpoints, `engine_catalogs`, `engines.warehouse_id` and `warehouse:*` | — |

## 7. Testing

- **Rendering**: per kind × per engine against hand-written expected output, including every JDBC
  dialect, each iceberg metastore, `advanced` merge and the collision refusal.
- **Validation**: each kind rejects missing required fields, unknown fields, plaintext secrets,
  and names outside the identifier rule.
- **Migration**: seeded data covering warehouse name normalisation and suffixing, the single-
  primary `egret` exception, the several-primaries report, typed vs generic classification, and
  same-name dedupe vs suffix; the report rows are asserted, not only the catalogs.
- **Routes** (DB-backed): attach/detach per engine type; edit propagation with one engine failing
  (the other applied, the failed one flagged, Retry succeeds); delete refused while attached or
  assigned; permission gates with the real PDP.
- **Live on kind** (manual, reported as such): attach a PostgreSQL catalog to a Trino engine and
  query it; attach it to a StarRocks engine and query it; edit its password and watch both
  engines pick it up; confirm the 24 migrated warehouse catalogs and their report.

## 8. Out of scope

- Enforcing workspace assignment inside engines (SP4, SP5).
- egret querying a catalog directly (SP5).
- Per-workspace catalogs.
- Catalog renames and versioned configuration history.
