# Catalog Engine CE-5 — Per-workspace storage config — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/catalog-engine-ce5-workspace-storage` (off main; CE-1..CE-4 merged)
**Parent:** `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`; predecessor `docs/en/specs/2026-06-27-catalog-engine-CE4-authorizer-design.md`

## 0. Goal

Let **each workspace configure its own S3 storage** (bucket / endpoint / region / credentials) for its lakekeeper warehouse, instead of every workspace sharing the single global `StorageConfig`. The secret access key is stored **through `egret-secrets`** (envelope-encrypted; the minimal "secrets bridge"), never as plaintext in the workspace table. When a workspace provides no storage, provisioning falls back to the global default (CE-2 behaviour — back-compat).

This is the **re-scoped CE-5**: per-workspace storage config only. GCS/Azure multi-cloud and a full replacement of lakekeeper's `SecretStore` are deferred (the `provider` column is reserved for the former).

## 1. Decisions (locked)

- **Per-workspace S3 only.** A `provider` column defaults to `'s3'` and is reserved for GCS/Azure later; CE-5 implements S3 (incl. MinIO-compatible) exactly as the global path does today.
- **Storage is set at workspace-create time.** The create request carries an optional storage spec; the warehouse is provisioned against it. **Changing the storage of an already-provisioned warehouse is out of scope** (it would require data migration).
- **Secret via `egret-secrets`.** The secret key is `put` into `egret-secrets` (`SecretStore`), and `workspace_storage` stores only the returned `secret_ref` UUID. At provision time the provisioner `get`s it back to build the lakekeeper credential. lakekeeper still keeps its own operational copy of the credential in its secret store (a full bridge replacing lakekeeper's `SecretStore` is out of scope).
- **`egret-catalog-engine` gains a direct `egret-secrets` dependency** (a foundation crate) rather than introducing another resolver trait. The `SecretStore` is constructed in `egret-bin` (sqlx 0.8) and passed into the provisioner — same pattern as the `CatalogStore`, so the 0.8/0.9 pool boundary is never crossed.
- **Provisioner signature to `provision_workspace_warehouse` is unchanged** — it still takes a resolved `StorageConfig`. Per-workspace resolution happens one level up, inside `EngineWarehouseProvisioner::provision`.

## 2. Components

### 2.1 Schema (migration `0029_workspace_storage.sql`)
```sql
CREATE TABLE workspace_storage (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  provider     text NOT NULL DEFAULT 's3',
  endpoint     text,                       -- NULL/empty = real AWS; set = S3-compatible (MinIO)
  region       text NOT NULL,
  bucket       text NOT NULL,
  key_prefix   text,                       -- NULL = default workspaces/<slug>/warehouse
  access_key   text NOT NULL,
  secret_ref   uuid,                        -- → egret-secrets secret id (NULL if none)
  path_style   boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now()
);
```
Additive; no existing workspace has a row → all keep using the global default. Touch `crates/egret-metastore/src/lib.rs` to re-embed (see [[egret-sqlx-migration-staleness]]).

### 2.2 `egret-catalog` store (sqlx 0.8, alongside `workspace_warehouse`)
New file `crates/egret-catalog/src/service/storage.rs` (methods on `CatalogStore`):
- `pub struct WorkspaceStorageRow { workspace_id, provider, endpoint: Option<String>, region, bucket, key_prefix: Option<String>, access_key, secret_ref: Option<Uuid>, path_style }` (NO secret value).
- `upsert_workspace_storage(&self, row: &WorkspaceStorageRow) -> Result<(), CatalogError>` — `INSERT … ON CONFLICT (workspace_id) DO UPDATE`.
- `workspace_storage_for(&self, workspace_id: Uuid) -> Result<Option<WorkspaceStorageRow>, CatalogError>`.

### 2.3 `StorageSpec` + trait change (`egret-workspace`)
In `crates/egret-workspace/src/provision.rs` (plain struct, no secrets/catalog deps):
```rust
#[derive(Clone, Debug)]
pub struct StorageSpec {
    pub provider: String,          // "s3"
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_key: String,        // plaintext in-flight; persisted via egret-secrets, never stored raw
    pub path_style: bool,
}
```
`WarehouseProvisioner::provision` gains a parameter:
```rust
async fn provision(&self, workspace_id: WorkspaceId, slug: &str, storage: Option<StorageSpec>) -> Result<(), String>;
```

### 2.4 `EngineWarehouseProvisioner` (`egret-catalog-engine`)
- `new(engine_state, store, storage_cfg, system_user, secret_store: egret_secrets::SecretStore)` — gains the `SecretStore`.
- `provision(workspace_id, slug, storage)`:
  1. **Persist (if `storage` is Some):** `secret_ref = secret_store.put("workspace_s3_secret", &spec.secret_key)?`; build a `WorkspaceStorageRow` from `spec` + `secret_ref`; `store.upsert_workspace_storage(&row)?`.
  2. **Resolve effective config:** `row = store.workspace_storage_for(workspace_id)?`. If `Some`: `secret = row.secret_ref.map(|id| secret_store.get(id)).transpose()?.unwrap_or_default()`; build `StorageConfig { endpoint: row.endpoint, region: row.region, bucket: row.bucket, access_key: row.access_key, secret_key: secret, path_style: row.path_style }`. If `None`: clone the global `self.storage_cfg`.
  3. Call `provision_workspace_warehouse(self.engine_state, &self.store, workspace_id.0, slug, &effective_cfg, &self.system_user)`.
- `provision_workspace_warehouse` itself is unchanged; if `row.key_prefix` is set, pass it through (a small optional addition: let the caller override the default `workspaces/<slug>/warehouse` prefix; otherwise keep the default). Keep this minimal — default prefix is fine for CE-5, so `key_prefix` may be stored but the default is used unless trivially threaded.

### 2.5 Create-workspace API (`egret-workspace/src/routes.rs`)
- `CreateWorkspace` gains `pub storage: Option<StorageSpecDto>` (a serde DTO mirroring `StorageSpec`; the `secret_key` is write-only — accepted on create, never returned).
- `create_ws` maps the DTO → `StorageSpec` and passes it to `provisioner.provision(ws.id, &ws.slug, storage)`.
- A read endpoint `GET /workspaces/{slug}/storage` returns the `workspace_storage` row **without** the secret (just `secret_ref` presence / a `has_secret: bool`), or 404 when the workspace uses the global default.

### 2.6 Wiring (`egret-bin/src/main.rs`)
- Construct `egret_secrets::SecretStore` (already built around line 386 for the database block) and pass it into `EngineWarehouseProvisioner::new(...)`. Hoist it so both the database and engine blocks share the one instance.

## 3. Testing

- **Unit (provisioner resolution):** with a fake/in-mem `CatalogStore` + `SecretStore`: a workspace WITH a `workspace_storage` row → resolved `StorageConfig` carries the row's bucket/endpoint/region + the secret fetched from `egret-secrets`; a workspace WITHOUT a row → the global default config. Secret is never read from the row directly (only via `secret_ref`).
- **Unit (store, sqlx 0.8 against test DB):** `upsert_workspace_storage` then `workspace_storage_for` round-trips all fields incl. `secret_ref`; upsert twice updates; the row carries no secret value.
- **Integration (DB + MinIO):**
  - Create a workspace with a custom storage spec (same MinIO endpoint, explicit bucket + a distinct `access_key`/`secret_key` valid for MinIO) → warehouse provisions successfully (real S3 round-trip) AND a `workspace_storage` row exists AND `egret-secrets` holds the secret (the table column is a UUID `secret_ref`, the plaintext secret is NOT in `workspace_storage`).
  - Create a workspace with no storage → provisions against the global default (CE-2 regression), no `workspace_storage` row.
  - `GET /workspaces/{slug}/storage` returns the config without the secret for the custom workspace; 404 for the default one.

## 4. File map
- Create: `migrations/0029_workspace_storage.sql`; `crates/egret-catalog/src/service/storage.rs` (+ test).
- Modify: `crates/egret-metastore/src/lib.rs` (touch); `crates/egret-catalog/src/service/mod.rs` + `lib.rs` (export); `crates/egret-workspace/src/provision.rs` (`StorageSpec` + trait param); `crates/egret-workspace/src/routes.rs` (create DTO + storage GET); `crates/egret-catalog-engine/src/provisioner.rs` (+ `provision_ws.rs` if threading `key_prefix`) + `Cargo.toml` (egret-secrets dep); `crates/egret-bin/src/main.rs` (pass `SecretStore`); any other `WarehouseProvisioner` impl/caller updated to the new arity (e.g. test doubles, the `None` path).

## 5. Risks
- **Real S3 round-trip on create:** `create_warehouse` validates access against the configured bucket/creds. A bad per-workspace storage spec → workspace creation fails loudly (acceptable; the warehouse would be unusable anyway). Surface the error clearly.
- **Secret without crypto:** if `egret-secrets` crypto is not configured, `SecretStore` stores per its existing default — CE-5 inherits that behaviour (recommend configuring the master key). Not a CE-5-introduced gap.
- **Trait arity change:** every `WarehouseProvisioner` impl + call site (incl. the create route and any test stub) must take the new `storage` arg — a compile-enforced sweep.
- Single `iceberg` version must hold.

## 6. Out of scope (later)
- GCS / Azure storage (the `provider` column is reserved; lakekeeper supports them natively).
- Re-configuring storage for an already-provisioned warehouse (needs data migration / warehouse re-creation).
- Full secrets bridge: implementing lakekeeper's `SecretStore` trait against `egret-secrets` so the catalog engine's secrets all live in egret-secrets (CE-5 only routes the workspace storage secret through egret-secrets; lakekeeper keeps its own operational copy).
- CE-6 (GC / task-queue workers).
