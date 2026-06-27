# Catalog Engine CE-5 — Per-workspace storage config — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Each workspace can configure its own S3 storage (bucket/endpoint/region/creds) for its lakekeeper warehouse, set at workspace-create time; the secret key is stored through `egret-secrets`. No per-workspace config → global default (CE-2 behaviour).

**Architecture:** A `workspace_storage` table (egret-catalog, sqlx 0.8) holds each workspace's S3 config + a `secret_ref` into `egret-secrets`. The create-workspace API accepts an optional storage spec; `EngineWarehouseProvisioner` persists it (secret → egret-secrets) and, at provision time, resolves the effective `StorageConfig` (per-workspace row or global default) and feeds it to the unchanged `provision_workspace_warehouse`.

**Tech Stack:** Rust, sqlx 0.8 (egret-catalog), sqlx 0.9 + lakekeeper (egret-catalog-engine), axum, egret-secrets (envelope crypto).

**Branch:** `feat/catalog-engine-ce5-workspace-storage` (off main). DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO up.

**Spec:** `docs/en/specs/2026-06-27-catalog-engine-CE5-per-workspace-storage-design.md`.

**Verified seams:**
- `StorageConfig { endpoint: Option<String>, region: String, bucket: String, access_key: String, secret_key: String, path_style: bool }` (`crates/egret-storage/src/config.rs`).
- `CatalogStore::new(db: Db)` + `pool()` (`egret-catalog/src/service/store.rs`); warehouse methods in `service/warehouse.rs`; `CatalogError` has `BadRequest`/etc.
- `egret_secrets::SecretStore`: `put(&self, kind: &str, plaintext: &str) -> Result<Uuid, SecretsError>`, `get(&self, id: Uuid) -> Result<String, SecretsError>`, `is_configured()`. Built in `egret-bin/src/main.rs` (~line 386: `SecretStore::new(db.clone(), crypto)`).
- `WarehouseProvisioner::provision(&self, workspace_id: WorkspaceId, slug: &str) -> Result<(), String>` (`egret-workspace/src/provision.rs`); `SharedWarehouseProvisioner = Option<Arc<dyn WarehouseProvisioner>>`.
- `EngineWarehouseProvisioner::new(engine_state, store, storage_cfg, system_user)` + its `provision` delegating to `provision_workspace_warehouse(engine_state, &store, ws.0, slug, &storage_cfg, &system_user)` (`egret-catalog-engine/src/provisioner.rs`).
- Create route `create_ws` calls `provisioner.provision(ws.id, &ws.slug)` at `egret-workspace/src/routes.rs:120`; request `CreateWorkspace { slug, name }` (~line 68); router `/workspaces` POST (~line 84).

---

## Task 1: `workspace_storage` table + store

**Files:** Create `migrations/0029_workspace_storage.sql`; `crates/egret-catalog/src/service/storage.rs`; touch `crates/egret-metastore/src/lib.rs`; export in `crates/egret-catalog/src/service/mod.rs` + `lib.rs`; test `crates/egret-catalog/tests/store_workspace_storage.rs`.

- [ ] **Step 1 (migration):**
```sql
CREATE TABLE workspace_storage (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  provider     text NOT NULL DEFAULT 's3',
  endpoint     text,
  region       text NOT NULL,
  bucket       text NOT NULL,
  key_prefix   text,
  access_key   text NOT NULL,
  secret_ref   uuid,
  path_style   boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now()
);
```
Touch `egret-metastore/src/lib.rs` (trivial comment). Reset test DB: `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "DROP DATABASE IF EXISTS egret_test; CREATE DATABASE egret_test;"`.

- [ ] **Step 2 (failing test)** in `tests/store_workspace_storage.rs`: insert a workspace row (reuse the existing test helper that other egret-catalog tests use to make a workspace, or insert directly into `workspaces`), then:
  - `upsert_then_get_round_trips`: `upsert_workspace_storage(&row)` with `secret_ref = Some(<uuid>)`, endpoint `Some("http://localhost:9000")`, region `us-east-1`, bucket `wh`, access_key `ak`, path_style true → `workspace_storage_for(ws)` returns an equal `WorkspaceStorageRow` (no secret value field present).
  - `upsert_is_idempotent_update`: upsert twice (second changes bucket) → `workspace_storage_for` reflects the second.
  - `get_absent_returns_none`: `workspace_storage_for(<random ws>)` → `Ok(None)`.
  Run → FAIL (compile).

- [ ] **Step 3 (implement `service/storage.rs`):**
```rust
use uuid::Uuid;
use crate::service::error::CatalogError;
use crate::service::store::CatalogStore;

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceStorageRow {
    pub workspace_id: Uuid,
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_ref: Option<Uuid>,
    pub path_style: bool,
}

impl CatalogStore {
    pub async fn upsert_workspace_storage(&self, row: &WorkspaceStorageRow) -> Result<(), CatalogError> {
        sqlx::query(
            "INSERT INTO workspace_storage
               (workspace_id, provider, endpoint, region, bucket, key_prefix, access_key, secret_ref, path_style)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
             ON CONFLICT (workspace_id) DO UPDATE SET
               provider=$2, endpoint=$3, region=$4, bucket=$5, key_prefix=$6,
               access_key=$7, secret_ref=$8, path_style=$9",
        )
        .bind(row.workspace_id).bind(&row.provider).bind(&row.endpoint).bind(&row.region)
        .bind(&row.bucket).bind(&row.key_prefix).bind(&row.access_key).bind(row.secret_ref).bind(row.path_style)
        .execute(self.pool()).await.map_err(CatalogError::from)?;
        Ok(())
    }

    pub async fn workspace_storage_for(&self, workspace_id: Uuid) -> Result<Option<WorkspaceStorageRow>, CatalogError> {
        let r = sqlx::query_as::<_, (Uuid,String,Option<String>,String,String,Option<String>,String,Option<Uuid>,bool)>(
            "SELECT workspace_id, provider, endpoint, region, bucket, key_prefix, access_key, secret_ref, path_style
               FROM workspace_storage WHERE workspace_id = $1",
        ).bind(workspace_id).fetch_optional(self.pool()).await.map_err(CatalogError::from)?;
        Ok(r.map(|t| WorkspaceStorageRow {
            workspace_id: t.0, provider: t.1, endpoint: t.2, region: t.3, bucket: t.4,
            key_prefix: t.5, access_key: t.6, secret_ref: t.7, path_style: t.8,
        }))
    }
}
```
Match the codebase's actual sqlx style (if other store methods use `query!`/`query_as!` macros with the offline cache, follow that instead of the runtime `query`/`query_as` shown here — check `warehouse.rs`). Export `storage` module + `WorkspaceStorageRow` from `service/mod.rs` and `lib.rs`.

- [ ] **Step 4:** `cargo test -p egret-catalog --test store_workspace_storage` → PASS. `cargo fmt --all && cargo clippy -p egret-catalog --all-targets -- -D warnings`. Commit `feat(catalog): workspace_storage table + store`.

---

## Task 2: `StorageSpec` + `provision` arity change

**Files:** `crates/egret-workspace/src/provision.rs`; `crates/egret-catalog-engine/src/provisioner.rs`; `crates/egret-workspace/src/routes.rs`; plus any other `WarehouseProvisioner` impl / test stub (grep).

- [ ] **Step 1:** In `provision.rs` add the `StorageSpec` struct (exact fields from the spec §2.3) and change the trait method to `async fn provision(&self, workspace_id: WorkspaceId, slug: &str, storage: Option<StorageSpec>) -> Result<(), String>;`. Export `StorageSpec` from the crate.

- [ ] **Step 2:** Update `EngineWarehouseProvisioner`'s `impl WarehouseProvisioner` to the new signature. For NOW, accept `storage: Option<StorageSpec>` and ignore it with a `let _ = storage;` + `// TODO(CE-5 Task 3): persist + resolve per-workspace storage` (Task 3 implements it). Keep delegating to `provision_workspace_warehouse(... &self.storage_cfg ...)` unchanged.

- [ ] **Step 3:** Update the create route `create_ws` call `provisioner.provision(ws.id, &ws.slug, None).await` (the request DTO gains a storage field in Task 4; pass `None` for now). Grep `\.provision(` and `impl WarehouseProvisioner` across the workspace for any other call site / stub (tests) and update each to the new arity (pass `None`).

- [ ] **Step 4:** `cargo build --workspace`; `cargo test -p egret-workspace -p egret-catalog-engine`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`. All green (pure arity refactor). Commit `refactor(workspace): WarehouseProvisioner takes optional StorageSpec`.

---

## Task 3: provisioner persists + resolves per-workspace storage

**Files:** `crates/egret-catalog-engine/src/provisioner.rs` (+ `Cargo.toml` add `egret-secrets`); `crates/egret-bin/src/main.rs`; test `crates/egret-catalog-engine/tests/provision_storage.rs` (or extend an existing provisioner test).

- [ ] **Step 1 (dep + field):** Add `egret-secrets` to `egret-catalog-engine/Cargo.toml` (workspace dep). Add `secret_store: egret_secrets::SecretStore` to `EngineWarehouseProvisioner` and its `new(engine_state, store, storage_cfg, system_user, secret_store)`.

- [ ] **Step 2 (failing unit test)** in `tests/provision_storage.rs`: build a `CatalogStore` (test DB) + a `SecretStore` (test DB; crypto optional). Write a `workspace_storage` row for a workspace (via `store.upsert_workspace_storage`, with `secret_ref` = a real `secret_store.put("workspace_s3_secret","sk").await?`). Then call a NEW pure-ish resolver fn (see Step 3) and assert: the resolved `StorageConfig` has the row's bucket/endpoint/region/access_key + `secret_key == "sk"`; for a workspace with NO row, the resolver returns the global default config. Run → FAIL.

- [ ] **Step 3 (implement resolve + persist):** Add to `provisioner.rs`:
```rust
impl EngineWarehouseProvisioner {
    async fn persist_storage(&self, workspace_id: Uuid, spec: &StorageSpec) -> Result<(), String> {
        let secret_ref = self.secret_store.put("workspace_s3_secret", &spec.secret_key)
            .await.map_err(|e| format!("storing workspace storage secret: {e}"))?;
        let row = WorkspaceStorageRow {
            workspace_id, provider: spec.provider.clone(), endpoint: spec.endpoint.clone(),
            region: spec.region.clone(), bucket: spec.bucket.clone(), key_prefix: spec.key_prefix.clone(),
            access_key: spec.access_key.clone(), secret_ref: Some(secret_ref), path_style: spec.path_style,
        };
        self.store.upsert_workspace_storage(&row).await.map_err(|e| format!("persisting workspace storage: {e}"))
    }

    async fn resolve_storage(&self, workspace_id: Uuid) -> Result<StorageConfig, String> {
        match self.store.workspace_storage_for(workspace_id).await.map_err(|e| e.to_string())? {
            Some(row) => {
                let secret_key = match row.secret_ref {
                    Some(id) => self.secret_store.get(id).await.map_err(|e| format!("reading workspace storage secret: {e}"))?,
                    None => String::new(),
                };
                Ok(StorageConfig {
                    endpoint: row.endpoint, region: row.region, bucket: row.bucket,
                    access_key: row.access_key, secret_key, path_style: row.path_style,
                })
            }
            None => Ok(self.storage_cfg.clone()),
        }
    }
}
```
Then in `provision`: `if let Some(spec) = storage { self.persist_storage(workspace_id.0, &spec).await?; }` then `let cfg = self.resolve_storage(workspace_id.0).await?;` then `provision_workspace_warehouse(self.engine_state.clone(), &self.store, workspace_id.0, slug, &cfg, &self.system_user).await...`. Remove the Task-2 `let _ = storage;` TODO.

- [ ] **Step 4 (wire egret-bin):** In `main.rs`, hoist the `egret_secrets::SecretStore` (currently built in the database block ~line 386) so the engine block can pass it into `EngineWarehouseProvisioner::new(..., secret_store)`. (If a single `SecretStore` instance can be cloned/shared, share it; else construct one for the provisioner from the same `db` + `crypto`.)

- [ ] **Step 5:** `cargo test -p egret-catalog-engine --test provision_storage`; `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`. Green. Commit `feat(catalog-engine): resolve per-workspace storage (secret via egret-secrets)`.

---

## Task 4: create-API storage + GET storage + integration

**Files:** `crates/egret-workspace/src/routes.rs`; test `crates/egret-catalog-engine/tests/front_auth.rs` or a new `tests/workspace_storage_e2e.rs` (whichever already has the DB+MinIO+engine harness — reuse it).

- [ ] **Step 1 (create DTO):** Add `pub storage: Option<StorageSpecDto>` to `CreateWorkspace`, where `StorageSpecDto` is a serde struct mirroring `StorageSpec` (provider default `"s3"`, endpoint Option, region, bucket, key_prefix Option, access_key, **secret_key** (write-only — `#[serde(skip_serializing)]`), path_style default true). In `create_ws`, map `body.storage` → `Option<StorageSpec>` and pass it to `provisioner.provision(ws.id, &ws.slug, storage)`.

- [ ] **Step 2 (GET storage):** Add `GET /workspaces/{workspace}/storage` → resolve the workspace by slug, read `workspace_storage_for` via the catalog store available to the route (thread it onto `WorkspaceState` if not already; the route already has access to the provisioner — if the store isn't reachable from the route, expose a read method through a small seam or carry the `CatalogStore` on `WorkspaceState`). Return JSON `{ provider, endpoint, region, bucket, key_prefix, access_key, has_secret: bool }` (NEVER the secret). 404 when no row.

- [ ] **Step 3 (integration test, DB+MinIO):**
  - Create a workspace WITH a custom storage spec (endpoint = the dev MinIO `http://localhost:9000`, bucket = the existing dev bucket, region `us-east-1`, a valid MinIO access_key/secret_key, path_style true) → expect 201; assert the warehouse provisioned (the workspace→warehouse mapping exists / a catalog op works), assert `workspace_storage` has a row whose `secret_ref` is a UUID and that the `workspace_storage` table does NOT contain the plaintext secret, and `secret_store.get(secret_ref)` returns the secret.
  - Create a workspace with NO storage → 201, no `workspace_storage` row, warehouse provisioned against the global default (CE-2 regression).
  - `GET /workspaces/{slug}/storage` → 200 with the config and `has_secret: true` (no secret value) for the custom workspace; 404 for the default one.

- [ ] **Step 4:** `cargo test -p egret-workspace -p egret-catalog-engine -p egret-server`; `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; confirm single `iceberg` (`cargo tree -i iceberg -e features | head`). Commit `feat(workspace): per-workspace storage on create + GET storage endpoint`.

---

## Self-review checklist
- [ ] `workspace_storage` table additive (no row = global default, CE-2 regression intact).
- [ ] Secret stored via `egret-secrets` (`secret_ref` UUID in the table; plaintext secret never persisted in `workspace_storage`).
- [ ] `provision` takes `Option<StorageSpec>`; every impl/caller updated; create route threads the DTO.
- [ ] Provisioner persists (on Some) + resolves (row → effective `StorageConfig`; none → global default); `provision_workspace_warehouse` unchanged.
- [ ] GET storage hides the secret; 404 when default.
- [ ] suite green; clippy/fmt; single iceberg.

## Out of scope
GCS/Azure (provider column reserved); re-config of a provisioned warehouse; full lakekeeper `SecretStore` bridge; CE-6 (GC workers).
