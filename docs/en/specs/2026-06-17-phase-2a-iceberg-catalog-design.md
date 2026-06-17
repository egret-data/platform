# Phase 2a — Iceberg REST Catalog Design Spec

> **Status:** Approved design · **Date:** 2026-06-17 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-17-phase-2a-iceberg-catalog-design.md`](../../vi/specs/2026-06-17-phase-2a-iceberg-catalog-design.md).
> Implements the **Iceberg REST Catalog** half of [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.3 / Phase 2. The Apache Ranger half (`egret-authz` + `egret-ranger`) is a separate spec (Phase 2b).
> Validated against the `lakekeeper` reference implementation (`data-portal/lakekeeper`).

---

## 1. Goal & scope

Stand up a spec-compliant **Iceberg REST Catalog** so external engines (Trino, StarRocks, Spark, pyiceberg) use Egret as their catalog: create namespaces and tables, commit snapshots, load and query — with data files in S3/MinIO accessed via catalog-issued **remote signing**. This is the data-plane half of Phase 2 (governance core); the Ranger policy-plane is Phase 2b.

**In scope**
- Two new crates: `egret-storage` (object-store abstraction + SigV4 signing) and `egret-catalog` (the Iceberg REST HTTP layer + Postgres `CatalogStore`). A small `egret-iceberg-ext` crate holds the REST request/response types (mirrors lakekeeper's `iceberg-ext`).
- **Full read-write** catalog: namespace CRUD, table create/load/commit/drop/rename/register, view CRUD/rename. Commit uses optimistic concurrency (Iceberg `requirements`).
- **Postgres is the source of truth** for table metadata, stored as a single `jsonb` `TableMetadata`; `metadata.json` is also materialized to storage with a `metadata_location` pointer.
- **Remote signing** (`POST /v1/aws/s3/sign`): the catalog signs S3 requests with its own credentials; engines never see raw credentials.
- **Global catalog**, authenticated by a static bearer token. Multi-workspace tenancy/isolation is deferred.
- Reuse the apache `iceberg` (iceberg-rust) crate for the spec model and metadata-update application.
- Tested by a **real pyiceberg client** (create → table → append → commit → load) over MinIO via testcontainers, plus Rust integration (DB + MinIO gated) and unit tests.

**Out of scope (deferred)**
- Multi-workspace tenancy via the Iceberg `prefix` mechanism; `workspace_catalog_grant` enforcement (the table is created but unused).
- OAuth token endpoint (`/v1/oauth/tokens`) — engines use a static bearer token.
- Multi-table transactions (`POST /v1/transactions/commit`).
- Vended STS credentials (`aws-sdk-sts` is wired as a dependency for later; remote signing is the Phase 2a mechanism).
- Normalized metadata tables (lakekeeper-style ~11 tables) — a later optimization over the JSONB blob.
- Admin UI catalog browser — backend only this phase; UI arrives with the governance/BI frontend phases.

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Phase split | Catalog first (this spec); Ranger is Phase 2b | Policies reference catalog resources; storage is reused infra |
| 2 | Capability | Full read-write (namespace + table create/load/commit/drop/rename + views) | Matches architecture flow §11.4 |
| 3 | Storage creds | Credential vending via **remote signing** (`/v1/aws/s3/sign`) | lakekeeper `io` supports it; engine never sees creds |
| 4 | Metadata store | **Postgres-as-truth, single `jsonb` `TableMetadata`** + materialized `metadata.json` artifact | lakekeeper normalizes (~11 tables); JSONB is the pragmatic MVP of the same principle |
| 5 | Tenancy | **Global catalog**, static bearer-token auth; workspace isolation deferred | Smaller surface for 2a |
| 6 | Metadata engine | Reuse apache `iceberg` (iceberg-rust) + an `egret-iceberg-ext` crate for REST types | lakekeeper depends on `iceberg` + `iceberg-ext` |
| 7 | Storage SDK | `aws-sdk-s3` + `aws-sigv4` (+ `aws-sdk-sts` wired for future vended creds) | lakekeeper crate `io` uses exactly these |

## 3. Architecture / crate layout

```text
crates/
├── egret-storage/
│   └── src/
│       ├── lib.rs              # ObjectStore trait: put/get/delete/list/head
│       ├── s3.rs              # S3Store (aws-sdk-s3, MinIO endpoint override, path-style)
│       ├── signer.rs          # sign_s3_request() via aws-sigv4 (remote signing)
│       ├── location.rs        # warehouse layout: <warehouse>/<ns>/<table>-<uuid>/...
│       └── config.rs          # StorageConfig (endpoint, region, bucket, creds via secrets)
│
├── egret-iceberg-ext/
│   └── src/
│       ├── lib.rs              # re-export iceberg::{TableUpdate, TableRequirement, ...}
│       ├── catalog.rs         # REST DTOs: CreateNamespaceRequest, CreateTableRequest,
│       │                      #   CommitTableRequest, LoadTableResult, ConfigResponse,
│       │                      #   RenameTableRequest, ListTablesResponse, View DTOs, ErrorModel
│       ├── sign.rs            # S3SignRequest / S3SignResponse DTOs
│       └── validation.rs      # namespace/identifier validation helpers
│
└── egret-catalog/
    └── src/
        ├── lib.rs
        ├── api/iceberg/        # axum handlers, one module per resource group
        │   ├── config.rs      # GET /v1/config
        │   ├── namespaces.rs  # namespace CRUD + properties
        │   ├── tables.rs      # create/load/commit/drop/rename/register/metrics
        │   ├── views.rs       # view CRUD + rename
        │   └── sign.rs        # POST /v1/aws/s3/sign
        ├── service/
        │   ├── store.rs       # CatalogStore (Postgres): namespaces/tables/views
        │   ├── commit.rs      # load → check requirements → apply updates → CAS write
        │   └── metadata.rs    # (de)serialize TableMetadata, write metadata.json artifact
        ├── auth.rs            # bearer-token middleware
        ├── error.rs          # CatalogError → Iceberg ErrorModel JSON
        └── routes.rs         # iceberg_router() mounted at /catalog
```

**Layering** follows lakekeeper: `api/iceberg` (HTTP) → `service::CatalogStore` (Postgres) → `egret-storage` (data/metadata files). `egret-catalog` depends on `egret-iceberg-ext`, `egret-storage`, `egret-metastore`, `egret-config`, `egret-core`.

## 4. REST API surface (mounted at `/catalog`)

Iceberg REST Catalog OpenAPI subset. All routes require a valid bearer token (§7) except none — config included.

| Group | Method + path | Notes |
|---|---|---|
| Config | `GET /v1/config` | Returns `{ defaults, overrides }`. `overrides` carries `warehouse` + `s3.signer.uri` (+ `s3.remote-signing-enabled=true`) so clients enable remote signing. |
| Namespace | `GET /v1/namespaces` | List; optional `parent` query for nested. |
| | `POST /v1/namespaces` | Create with properties. |
| | `GET /v1/namespaces/{ns}` | Load properties. |
| | `HEAD /v1/namespaces/{ns}` | Exists (200/404). |
| | `DELETE /v1/namespaces/{ns}` | Drop (404 if missing, 409 if non-empty). |
| | `POST /v1/namespaces/{ns}/properties` | Update/remove properties. |
| Table | `GET /v1/namespaces/{ns}/tables` | List. |
| | `POST /v1/namespaces/{ns}/tables` | Create (supports `stage-create`). Writes initial metadata.json + row. |
| | `GET /v1/namespaces/{ns}/tables/{t}` | Load → `LoadTableResult` (metadata + `config` with signer uri). |
| | `HEAD /v1/namespaces/{ns}/tables/{t}` | Exists. |
| | `POST /v1/namespaces/{ns}/tables/{t}` | **Commit** — `{requirements, updates}`; optimistic CAS; 409 on conflict. |
| | `DELETE /v1/namespaces/{ns}/tables/{t}` | Drop (`purgeRequested` honored best-effort). |
| | `POST /v1/tables/rename` | Rename/move across namespaces. |
| | `POST /v1/namespaces/{ns}/register` | Register an existing table by `metadata_location`. |
| | `POST /v1/namespaces/{ns}/tables/{t}/metrics` | Accept & ignore (200). |
| View | `GET/POST /v1/namespaces/{ns}/views`, `GET/HEAD/POST/DELETE /v1/namespaces/{ns}/views/{v}`, `POST /v1/views/rename` | Same shape as tables; view metadata in `catalog_views.metadata` jsonb. |
| Sign | `POST /v1/aws/s3/sign` | Body `S3SignRequest{ region, uri, method, headers, body? }` → `S3SignResponse{ uri, headers }`. |

Errors use the Iceberg `ErrorModel` JSON (`{ error: { message, type, code } }`) with correct HTTP status (400/401/403/404/409/419/500).

## 5. Commit flow & optimistic concurrency

`POST /v1/namespaces/{ns}/tables/{t}` with `CommitTableRequest{ requirements, updates }`:

```text
1. BEGIN tx
2. SELECT metadata, current_version FROM catalog_tables
   WHERE namespace_id=? AND name=? FOR UPDATE          -- row lock
3. Deserialize TableMetadata from the jsonb column (iceberg crate).
4. For each requirement: assert against current metadata
   (assert-create / assert-ref-snapshot-id / assert-current-schema-id /
    assert-default-spec-id / assert-default-sort-order-id / assert-table-uuid /
    assert-last-assigned-field-id / assert-last-assigned-partition-id).
   Any failure → ROLLBACK → 409 CommitFailedException.
5. Apply updates in order via TableMetadataBuilder
   (add-schema, set-current-schema, add-snapshot, set-snapshot-ref,
    set-properties, remove-properties, add-spec, add-sort-order, set-location, ...).
6. new_version = current_version + 1
7. metadata_location = <location>/metadata/<new_version>-<uuid>.metadata.json
8. egret-storage.put(metadata_location, serialized new TableMetadata)  -- artifact
9. UPDATE catalog_tables SET metadata=?, metadata_location=?,
          current_snapshot_id=?, current_version=new_version, updated_at=now()
   WHERE id=? AND current_version=current_version     -- CAS guard
10. INSERT catalog_table_metadata_log(table_id, metadata_location)
11. COMMIT  → return LoadTableResult(new metadata)
```

The `FOR UPDATE` row lock plus the `current_version` CAS guards against concurrent commits. Data and manifest files are written by the **client** (engine/pyiceberg) using remote-signed S3 requests before it sends the commit; the catalog only writes the table **metadata.json** and owns the pointer.

## 6. Remote signing (`egret-storage`)

The catalog advertises `s3.signer.uri = <egret>/catalog/v1/aws/s3/sign` and `s3.remote-signing-enabled=true` in `/v1/config` and per-table `LoadTableResult.config`. The client, when it needs to read/write a data file, sends the unsigned request shape to the sign endpoint:

```text
POST /catalog/v1/aws/s3/sign
{ "region": "us-east-1", "uri": "https://minio/warehouse/ns/t-uuid/data/f.parquet",
  "method": "PUT", "headers": { "x-amz-content-sha256": [...], ... } }
→ 200 { "uri": "...", "headers": { "Authorization": "AWS4-HMAC-SHA256 ...",
                                    "x-amz-date": [...], ... } }
```

`egret-storage::signer::sign_s3_request` uses `aws-sigv4` to compute the SigV4 signature over the presented request using the catalog's stored S3 credentials, returning the headers to add. The client then sends the signed request **directly** to MinIO/S3 — Egret is not in the data path, only the signing path.

## 7. Auth & config

**Auth.** A bearer-token middleware on `iceberg_router` validates `Authorization: Bearer <token>` against `config.catalog.auth_token` (constant-time compare). `GET /v1/config` is included so a misconfigured token fails fast on the first call. pyiceberg/engines configure `token=<...>`. Follow-up: migrate to `api_keys` + OAuth `/v1/oauth/tokens` when tenancy lands.

**Config additions (`egret-config`).**
```toml
[catalog]
enabled    = true
warehouse  = "s3://warehouse"      # base location handed to clients
auth_token = "..."                 # static bearer (from env/secret in prod)

[storage]
endpoint   = "http://localhost:9000"   # MinIO; omit for real AWS
region     = "us-east-1"
bucket     = "warehouse"
access_key = "..."                 # from env/secret
secret_key = "..."                 # from env/secret
path_style = true                  # required for MinIO
```
`validate()` rejects `catalog.enabled && (warehouse empty || auth_token empty || storage incomplete)`.

**Server wiring (`egret-server`).** `AppState` gains `catalog: CatalogState`. `iceberg_router()` is mounted at `/catalog` (sibling of `/api`, since engines call it directly, not under the browser CSRF/session stack). When `catalog.enabled = false`, the router is not mounted.

## 8. Data model (`migrations/0006_catalog.sql`)

```sql
create table catalog_namespaces (
  id          uuid primary key default gen_random_uuid(),
  levels      text[] not null,                 -- multi-level Iceberg namespace
  properties  jsonb  not null default '{}',
  created_at  timestamptz not null default now(),
  unique (levels)
);

create table catalog_tables (
  id                        uuid primary key default gen_random_uuid(),
  namespace_id              uuid not null references catalog_namespaces(id) on delete restrict,
  name                      text not null,
  table_uuid                uuid not null,
  metadata                  jsonb not null,     -- full TableMetadata (source of truth)
  metadata_location         text  not null,
  previous_metadata_location text,
  location                  text  not null,
  current_snapshot_id       bigint,
  current_version           int   not null default 0,   -- optimistic CAS counter
  created_at                timestamptz not null default now(),
  updated_at                timestamptz not null default now(),
  unique (namespace_id, name)
);

create table catalog_table_metadata_log (
  id                uuid primary key default gen_random_uuid(),
  table_id          uuid not null references catalog_tables(id) on delete cascade,
  metadata_location text not null,
  created_at        timestamptz not null default now()
);

create table catalog_views (
  id                uuid primary key default gen_random_uuid(),
  namespace_id      uuid not null references catalog_namespaces(id) on delete restrict,
  name              text not null,
  metadata          jsonb not null,
  metadata_location text not null,
  location          text not null,
  current_version   int  not null default 0,
  created_at        timestamptz not null default now(),
  updated_at        timestamptz not null default now(),
  unique (namespace_id, name)
);

-- created for forward-compat; NOT enforced this phase (tenancy deferred)
create table workspace_catalog_grant (
  workspace_id uuid not null references workspaces(id) on delete cascade,
  namespace_id uuid not null references catalog_namespaces(id) on delete cascade,
  granted_by   uuid references users(id),
  granted_at   timestamptz not null default now(),
  primary key (workspace_id, namespace_id)
);
```

## 9. Testing

- **pyiceberg real-client gate** (`tests/` or `lakekeeper-integration-tests`-style crate): spin up MinIO + Egret via testcontainers; run a pyiceberg script that does `create_namespace → create_table(schema) → append(pyarrow table) → load_table → scan().to_arrow()` and asserts the round-tripped rows. Proves the REST protocol + remote signing end-to-end. Self-skips when Docker is unavailable (mirrors the Keycloak gate).
- **Rust integration tests (DB + MinIO gated):** per-endpoint round-trips; namespace nesting; create + load; commit happy path; commit conflict → 409; rename across namespaces; drop (with/without purge); view CRUD; register existing table.
- **Unit tests:** `signer::sign_s3_request` produces a valid SigV4 Authorization header for a known request/credential pair; `commit` applies an `add-snapshot` update and bumps the snapshot ref; `CatalogStore` CAS rejects a stale `current_version`.
- Tests follow the existing DB-gated pattern (self-skip without `DATABASE_URL`); the MinIO-dependent tests gate on a reachable `[storage]` endpoint.

## 10. Decomposition for the plan

1. `egret-storage`: `ObjectStore` trait + `S3Store` (aws-sdk-s3, MinIO) + `location` helpers; unit test put/get/list against MinIO (gated).
2. `egret-storage::signer`: `sign_s3_request` (aws-sigv4); unit test signature against a known vector.
3. `egret-iceberg-ext`: REST DTOs + re-exports + validation; serde round-trip unit tests.
4. `migrations/0006_catalog.sql` + `CatalogStore` skeleton (namespaces CRUD); DB-gated tests.
5. `CatalogStore` tables: create/load/list/drop/rename + `current_version` CAS; DB-gated tests.
6. `service::commit` + `service::metadata`: requirements check + `TableMetadataBuilder` apply + artifact write; unit + DB-gated tests.
7. `egret-catalog` views in the store; DB-gated tests.
8. `api/iceberg` handlers (config/namespaces/tables/views/sign) + `error` mapping + `auth` middleware; `iceberg_router()`.
9. `egret-config` `[catalog]` + `[storage]` + `validate()`; `egret-server` mount at `/catalog`.
10. pyiceberg real-client gate + final verification.

## 11. References

- **lakekeeper** (`data-portal/lakekeeper`): crate `io` (aws-sdk-s3 + aws-sigv4 + aws-sdk-sts; s3 remote signing), `iceberg-ext` (REST DTOs + spec + validation), `lakekeeper-storage-postgres` (normalized metadata — the model we simplify to JSONB), dependency on `iceberg` (iceberg-rust fork).
- **apache iceberg-rust** (`iceberg` crate): `TableMetadata`, `TableMetadataBuilder`, `TableUpdate`, `TableRequirement`, spec types.
- **Iceberg REST Catalog OpenAPI**: the endpoint contract in §4.
- Architecture: [Master design](../architecture/2026-06-17-egret-architecture-design.md) §8.x (governance), §9.4 (catalog data model), §10.3 (catalog), §11.4 (commit flow).
