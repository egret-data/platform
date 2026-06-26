# Spec A — Catalog-per-workspace (+ metadata normalization)

**Date:** 2026-06-26
**Status:** Design — pending review
**Scope:** First sub-spec of the Notebook epic. Stands alone; ships independently.

---

## 0. Context: the Notebook epic

The notebook feature for egret was decomposed into five independent sub-specs,
built roughly in dependency order:

| # | Sub-spec | Depends on |
|---|----------|-----------|
| **A** | **Catalog-per-workspace** (this doc) | — (foundation) |
| B | Storage / File browser (personal + workspace-shared) | — |
| C | Notebook core (CRUD, persistence, Monaco editor) | A |
| D | Daft kernel runtime (launcher trait: subprocess → docker → k8s; lifecycle) | A, B |
| E | Execution wiring (4 cell types, output rendering) | C, D |

Decisions already fixed for the epic (for context; only A is specified here):

- **Engine:** Daft (not Spark). Notebook = per-(user × workspace) Python/Jupyter
  kernel with Daft preloaded, wired to the workspace's Iceberg catalog.
- **Kernel scope:** one kernel per (user × workspace).
- **Catalog topology:** **1 workspace = 1 managed Iceberg catalog** (dedicated
  warehouse prefix).
- **Deploy isolation:** pluggable launcher — subprocess (MVP/local), then
  `docker_per_user`, then `k8s_per_user`.
- **Cell types:** Python (Daft DF API), SQL via Daft SQL, SQL via egret-engine
  (Trino/StarRocks), Markdown.
- **RBAC:** reuse egret's existing `Authorizer` (platform + workspace scopes).
- **Notebook persistence:** Postgres source-of-truth; large cell outputs spill to S3.

---

## 1. Goal & motivation

Today egret-catalog is a **single, global** Iceberg REST catalog:

- Namespaces/tables are not scoped to a workspace (`workspace_catalog_grant`
  exists but is explicitly **not enforced** — "tenancy deferred").
- `CatalogService` holds one global `warehouse: Arc<str>`.
- Auth is a single static bearer token shared by all engines — not tied to RBAC.
- `catalog_tables.metadata` is one monolithic JSONB blob (the known tech debt
  from the `catalog-metadata-jsonb-then-normalize` note).

Spec A turns this into a **per-workspace** catalog and, since we are already
doing a destructive schema rebuild, **pays down the JSONB normalization debt**
in the same pass (modeled on lakekeeper's `split_table_metadata`).

This is the foundation the Daft kernel (spec D) connects to: each workspace's
notebook talks to that workspace's catalog, isolated at the prefix + RBAC layer.

**Migration policy:** drop & recreate the `catalog_*` schema. egret is
pre-release and catalog tenancy was always "deferred", so no backfill.

---

## 2. Architecture decisions

### 2.1 Per-workspace scoping — prefix-based (Iceberg REST native)

Add the standard Iceberg REST `{prefix}` path segment:

```
/v1/{prefix}/namespaces/...
/v1/{prefix}/namespaces/{ns}/tables/{table}
/v1/{prefix}/aws/s3/sign
```

`prefix` = workspace **slug**. `/v1/config?warehouse=<ws>` returns
`{ overrides: { prefix: "<ws>" }, defaults, endpoints }`, so Iceberg clients
(Daft, Trino) auto-target the right prefix with zero custom client code.

A request extractor resolves `prefix → catalog row` (404 if the workspace has
no catalog) and feeds the resolved catalog (id + warehouse_location) to the
service layer.

### 2.2 Catalog data model — explicit `catalogs` table

A dedicated `catalogs` row per workspace (1:1) holds warehouse location and
catalog properties — these need a home, and a row cleanly expresses
"1 workspace = 1 catalog" while leaving room to relax 1:1 later.

### 2.3 Metadata normalization — mirror lakekeeper

Drop the `metadata jsonb` blob. Normalize **collections, relationships and
pointers** into rows; keep each leaf object (one schema, one partition spec,
one snapshot summary, one ref retention) as JSONB. This is lakekeeper's
proven level of normalization — you do not shred nested Iceberg types into
columns. The DB becomes the authoritative source; `metadata.json` is still
written to object storage for direct engine reads.

**Assemble pattern (validated against lakekeeper's `load_tables`):** loading a
table is ONE query that `LEFT JOIN`s + `ARRAY_AGG`s the child collections
(schemas, specs, sort orders, snapshots, refs, props, logs, stats) returning
each leaf as `Json<Schema>` / `Json<Snapshot>` / etc., then rebuilds
`iceberg::spec::TableMetadata` in Rust via `TableMetadataBuilder`. No N+1.

**Target Iceberg V2.** Daft and Trino target format-version 2. V3-only fields
lakekeeper carries — row lineage (`next_row_id`, `first_row_id`,
`assigned_rows`) and table encryption keys — are **out of scope** for spec A.

**Reference files consulted in lakekeeper** (`data-portal/lakekeeper/`):
`migrations/20241106201139_split_table_metadata.sql`,
`migrations/20240620151544_views.sql`,
`migrations/20241216150545_fix_schemaless_snapshots.sql`,
`migrations/20241216114355_table_statistics.sql`,
`src/tabular/table/mod.rs::load_tables`.

### 2.4 AuthN/Z — two callers, one invariant

**Invariant:** a caller may only touch the prefix of its own workspace
(resolved-workspace == prefix-workspace, else 403).

- **UI (egret session):** JWT → `WorkspaceContext` →
  `Authorizer.require_workspace(ctx, "catalog:read" | "catalog:write")`.
  Two new permission strings, seeded into workspace roles.
- **Engine/kernel (Trino/Daft):** workspace-scoped token. A `catalog_tokens`
  table (hashed) replaces the static global token; `require_bearer` resolves
  token → workspace and enforces it equals `prefix`.

### 2.5 Frontend placement

Catalog UI lives in the **explore / workspace** area as its own sidebar group
(beside SQL Lab, Datasets, Charts, Dashboards, Notebooks) — **not** in platform
admin. This aligns with the broader product shift: most features are managed
**per-workspace**; `RequirePlatform`/`AdminRoutes` narrows to platform-level
concerns (users, workspaces, platform roles, allowed domains, platform DBs).

A full admin↔workspace navigation refactor is **cross-cutting and out of scope
here** — spec A only places the new Catalog view correctly. The wider nav
re-org is its own UI spec.

---

## 3. Schema (new migration; drops `catalog_*` + `workspace_catalog_grant`)

```sql
-- One catalog per workspace
CREATE TABLE catalogs (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id       uuid NOT NULL UNIQUE REFERENCES workspaces(id) ON DELETE CASCADE,
  name               text NOT NULL,            -- default = workspace slug
  warehouse_location text NOT NULL,            -- s3://<bucket>/workspaces/<slug>/warehouse
  properties         jsonb NOT NULL DEFAULT '{}',
  created_at         timestamptz NOT NULL DEFAULT now()
);

-- Namespaces now scoped to a catalog
CREATE TABLE catalog_namespaces (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  catalog_id uuid NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  levels     text[] NOT NULL,
  properties jsonb  NOT NULL DEFAULT '{}',
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (catalog_id, levels)
);

-- Table root: pointers + counters, NO metadata blob
CREATE TABLE catalog_tables (
  id                         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  namespace_id               uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
  name                       text NOT NULL,
  table_uuid                 uuid NOT NULL,
  location                   text NOT NULL,
  format_version             smallint NOT NULL,      -- 1 | 2
  last_column_id             int NOT NULL,
  last_sequence_number       bigint NOT NULL,
  last_updated_ms            bigint NOT NULL,
  last_partition_id          int NOT NULL,
  current_schema_id          int NOT NULL,
  default_spec_id            int NOT NULL,
  default_sort_order_id      int NOT NULL,
  current_snapshot_id        bigint,
  metadata_location          text NOT NULL,
  previous_metadata_location text,
  current_version            int NOT NULL DEFAULT 0,  -- CAS guard
  created_at                 timestamptz NOT NULL DEFAULT now(),
  updated_at                 timestamptz NOT NULL DEFAULT now(),
  UNIQUE (namespace_id, name)
);

-- Table child collections (leaf = jsonb, relationships = rows)
CREATE TABLE catalog_table_schemas (
  table_id  uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  schema_id int  NOT NULL,
  schema    jsonb NOT NULL,
  PRIMARY KEY (table_id, schema_id)
);
CREATE TABLE catalog_table_partition_specs (
  table_id uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  spec_id  int  NOT NULL,
  spec     jsonb NOT NULL,
  PRIMARY KEY (table_id, spec_id)
);
CREATE TABLE catalog_table_sort_orders (
  table_id      uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  sort_order_id int  NOT NULL,
  sort_order    jsonb NOT NULL,
  PRIMARY KEY (table_id, sort_order_id)
);
CREATE TABLE catalog_table_properties (
  table_id uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  key      text NOT NULL,
  value    text NOT NULL,
  PRIMARY KEY (table_id, key)
);
CREATE TABLE catalog_table_snapshots (
  table_id           uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  snapshot_id        bigint NOT NULL,
  parent_snapshot_id bigint,
  sequence_number    bigint NOT NULL,
  manifest_list      text NOT NULL,
  summary            jsonb NOT NULL,
  schema_id          int,                 -- NULLABLE: schemaless snapshots exist
  timestamp_ms       bigint NOT NULL,     -- (lakekeeper fix_schemaless_snapshots)
  PRIMARY KEY (table_id, snapshot_id)
);
CREATE TABLE catalog_table_snapshot_log (
  table_id     uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  seq          bigserial,
  snapshot_id  bigint NOT NULL,
  timestamp_ms bigint NOT NULL,
  PRIMARY KEY (table_id, seq)
);
CREATE TABLE catalog_table_metadata_log (
  table_id      uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  seq           bigserial,
  timestamp_ms  bigint NOT NULL,
  metadata_file text NOT NULL,
  PRIMARY KEY (table_id, seq)
);
CREATE TABLE catalog_table_refs (
  table_id    uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  ref_name    text NOT NULL,
  snapshot_id bigint NOT NULL,
  type        text NOT NULL,            -- branch | tag
  retention   jsonb NOT NULL,
  PRIMARY KEY (table_id, ref_name),
  FOREIGN KEY (table_id, snapshot_id) REFERENCES catalog_table_snapshots(table_id, snapshot_id)
);

-- Puffin statistics files (mirror lakekeeper table_statistics / partition_statistics)
CREATE TABLE catalog_table_statistics (
  table_id                  uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  snapshot_id               bigint NOT NULL,
  statistics_path           text NOT NULL,
  file_size_in_bytes        bigint NOT NULL,
  file_footer_size_in_bytes bigint NOT NULL,
  key_metadata              text,
  blob_metadata             jsonb NOT NULL,
  PRIMARY KEY (table_id, snapshot_id)
);
CREATE TABLE catalog_table_partition_statistics (
  table_id           uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
  snapshot_id        bigint NOT NULL,
  statistics_path    text NOT NULL,
  file_size_in_bytes bigint NOT NULL,
  PRIMARY KEY (table_id, snapshot_id)
);

-- Views: same normalization (mirrors lakekeeper view_* tables)
CREATE TABLE catalog_views (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  namespace_id       uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
  name               text NOT NULL,
  view_uuid          uuid NOT NULL,
  location           text NOT NULL,
  format_version     smallint NOT NULL,
  current_version_id int NOT NULL,
  metadata_location  text NOT NULL,
  current_version    int NOT NULL DEFAULT 0,   -- CAS guard
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  UNIQUE (namespace_id, name)
);
CREATE TABLE catalog_view_schemas (
  view_id   uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
  schema_id int  NOT NULL,
  schema    jsonb NOT NULL,
  PRIMARY KEY (view_id, schema_id)
);
CREATE TABLE catalog_view_versions (
  view_id      uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
  version_id   int  NOT NULL,
  schema_id    int  NOT NULL,
  timestamp_ms bigint NOT NULL,
  summary      jsonb NOT NULL,
  default_catalog   text,
  default_namespace jsonb,               -- namespace levels of the default ns
  PRIMARY KEY (view_id, version_id),
  FOREIGN KEY (view_id, schema_id) REFERENCES catalog_view_schemas(view_id, schema_id)
);
-- Each SQL representation is its own row (mirrors lakekeeper view_representation)
CREATE TABLE catalog_view_representations (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  view_id    uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
  version_id int  NOT NULL,
  type       text NOT NULL DEFAULT 'sql',
  sql        text NOT NULL,
  dialect    text NOT NULL,
  FOREIGN KEY (view_id, version_id) REFERENCES catalog_view_versions(view_id, version_id) ON DELETE CASCADE
);
CREATE TABLE catalog_view_version_log (
  view_id      uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
  seq          bigserial,
  version_id   int NOT NULL,
  timestamp_ms bigint NOT NULL,
  PRIMARY KEY (view_id, seq)
);
CREATE TABLE catalog_view_properties (
  view_id uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
  key     text NOT NULL,
  value   text NOT NULL,
  PRIMARY KEY (view_id, key)
);

-- Engine/kernel access tokens, scoped per workspace catalog
CREATE TABLE catalog_tokens (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  token_hash   text NOT NULL,           -- SHA-256 of the bearer token
  label        text NOT NULL DEFAULT '',
  created_at   timestamptz NOT NULL DEFAULT now(),
  revoked_at   timestamptz,
  UNIQUE (token_hash)
);
```

---

## 4. Service & wiring changes

- **`CatalogService`**: drop the global `warehouse: Arc<str>` field. Each
  operation receives the resolved `catalog` (id + warehouse_location) from the
  request extractor.
- **`CatalogStore`**: namespace ops filter by `catalog_id`. New
  `TableMetadataRepo` responsibilities:
  - `load(table_id) -> iceberg::spec::TableMetadata` — **assemble** from rows
    via a single `LEFT JOIN` + `ARRAY_AGG` query (leaves as `Json<...>`), then
    `TableMetadataBuilder` — the lakekeeper `load_tables` pattern. No N+1.
  - `create(catalog, ns, req)` — shred v0 metadata into rows in one tx.
  - `commit(table_id, expected_version, new_metadata)` — in one tx, CAS on
    `current_version`; child collections are **append-only by id**
    (insert-if-not-exists, since schemas/specs/sort-orders/snapshots are
    immutable); update pointers (`current_*`), upsert `refs` + `properties`,
    append `snapshot_log` + `metadata_log`.
- **`egret-bin/main.rs`**: stop passing a global warehouse into the catalog
  service; warehouse is resolved per request.
- **`require_bearer`**: validate against `catalog_tokens` (hash match → active
  row → workspace) instead of the static token; enforce `workspace == prefix`.
- **Storage**: spec A keeps a single `ObjectStore` (same bucket/creds), only
  the warehouse **prefix** differs per workspace — enough for logical
  separation. Per-workspace **credential** isolation (MinIO IAM) is deferred to
  spec D (kernel).
- **Provisioning**: creating a workspace creates its `catalogs` row (hook in
  `egret-workspace::create`); warehouse_location derived from the workspace slug.

---

## 5. Internal egret API (for the UI)

The UI does **not** call the raw Iceberg REST API. A thin internal,
session-authenticated API backs the Catalog view, gated by
`Authorizer.require_workspace`:

- `GET  /api/workspaces/:ws/catalog` — catalog summary (name, warehouse).
- `GET  /api/workspaces/:ws/catalog/namespaces` — list namespaces.
- `GET  /api/workspaces/:ws/catalog/namespaces/:ns/tables` — list tables.
- `GET  /api/workspaces/:ws/catalog/namespaces/:ns/tables/:t` — table detail
  (schema, snapshots, properties — read from the normalized rows directly).

Create/drop namespace and token management may be minimal (or CLI-only) for
spec A; full management UI can follow.

---

## 6. Frontend

- Replace the `Catalog.tsx` placeholder with a real, **workspace-scoped** view:
  catalog → namespaces → tables (read-only for A), backed by the internal API.
- Add a **"Catalog"** group to the workspace/explore sidebar.

---

## 7. Testing

- **Unit:** warehouse-location derivation per workspace; metadata
  shred → assemble round-trips to an identical `TableMetadata`; store ops are
  catalog-scoped (a table in workspace A is invisible under workspace B's
  prefix — no cross-catalog leakage).
- **RBAC:** a `catalog_tokens` token for workspace A hitting workspace B's
  prefix → 403; UI without `catalog:read` → 403.
- **Integration (manual, real Trino):** update the existing manual
  Trino catalog+ranger test to use the prefixed catalog URL + a workspace
  token. This is the proof that Iceberg clients (Trino, and later Daft)
  connect per-workspace. (egret has no automated Trino test infra; this is the
  documented manual integration test.)

---

## 8. Out of scope (later sub-specs)

- Storage / file browser, personal vs workspace-shared (spec B).
- Notebook core, Daft kernel, execution (specs C–E).
- Per-user MinIO IAM credential isolation for kernels (spec D).
- Multiple / external catalogs per workspace (Glue, Nessie, read-only attach).
- Broad admin↔workspace navigation refactor (separate UI spec).
- Iceberg V3 metadata: row lineage (`next_row_id`/`first_row_id`/`assigned_rows`)
  and table encryption keys. Target is V2 (Daft/Trino).
