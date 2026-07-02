# Control-Plane Polaris — Plan 4: catalog + RBAC + assignment routes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P4 of the epic.** Depends on **P2** (`egret-controlplane` crate + `platform_admin` guard + global storage/secrets stores) and **P3** (`egret-polaris` client). VI reference: `docs/vi/plans/2026-07-02-polaris-p4-catalog-rbac-assignment-routes.md`.

**Goal:** Control-plane (`platform_admin`-gated) HTTP routes that register the single Polaris connection, manage catalogs + full RBAC (principals / principal-roles / catalog-roles / grants) by proxying to `egret-polaris`, and **assign catalogs to workspaces** (provisioning a Polaris-native service credential per assignment) — plus a consume-only read route for workspaces.

**Architecture:** Extend the `egret-controlplane` crate. A `polaris_connection` singleton (base URL + Keycloak admin-token config) builds a `PolarisClient`. Catalog/RBAC routes are thin proxies to `egret-polaris`. Assignment provisions, per (workspace, catalog): a Polaris principal (→ `clientId`/`clientSecret`, stored as a global Secret), a principal-role + catalog-role, and a grant; recorded in `workspace_catalog_assignments`. **v1 engine credential = Polaris-native principal** (engine uses Polaris's `/v1/oauth/tokens`; deprecated-but-functional; migrate to Keycloak-mapped later).

**Tech Stack:** Rust (axum, sqlx), `egret-polaris`, Postgres.

---

## Background the implementer needs

- **From P2:** crate `egret-controlplane` with `require_platform_admin(&principal)`, `CpState`, and the global `StorageConn` (`egret-catalog::service::storage_conn`) + secrets (`egret-secrets` with `Option<Uuid>` scope, `None` = global). Routes nested at `/api/control-plane` behind CSRF.
- **From P3:** `egret_polaris::{PolarisClient, PolarisConfig, PolarisError, NewCatalog, CatalogType, StorageConfig, PrincipalWithCredentials, Grant}`. `PolarisClient::new(cfg, transport)` where the token source is Keycloak client-credentials.
- **Auth:** control-plane handlers use the `egret_auth` `Auth(principal)` extractor + `require_platform_admin`. Workspace read route uses the existing `WsContext` (member access).
- **Engine credential (v1 decision):** on assign, `PolarisClient::create_principal` returns `clientId`/`clientSecret` — store both as a global Secret; that IS the engine's credential (used via Polaris `/v1/oauth/tokens`). No Keycloak-admin integration in v1.
- Migrations: next after P2's `0049` is **`0050`**; `touch crates/egret-metastore/src/lib.rs`.

---

## Task 1: Migration — connection + assignments tables

**Files:** Create `migrations/0050_polaris_connection_assignments.sql`; touch `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write the migration**

```sql
-- Single registered Polaris connection + admin-token (Keycloak) config.
CREATE TABLE polaris_connection (
    id              boolean PRIMARY KEY DEFAULT true CHECK (id),   -- singleton row
    base_url        text NOT NULL,
    kc_token_url    text NOT NULL,
    kc_client_id    text NOT NULL,
    kc_secret_ref   uuid NOT NULL REFERENCES secrets(id),          -- admin client_secret
    kc_scope        text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Which Polaris catalogs a workspace may use + its provisioned service credential.
CREATE TABLE workspace_catalog_assignments (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id      uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    catalog_name      text NOT NULL,
    principal_name    text NOT NULL,
    credential_ref    uuid NOT NULL REFERENCES workspace_secrets(id),  -- global secret holding clientId/secret
    created_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, catalog_name)
);
CREATE INDEX wca_workspace_id_idx ON workspace_catalog_assignments(workspace_id);
```

- [ ] **Step 2: Re-embed + apply + verify**

Run: `touch crates/egret-metastore/src/lib.rs && cargo run -q -p egret-bin -- migrate`
Then `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "\d polaris_connection; \d workspace_catalog_assignments"` — both present.

- [ ] **Step 3: Commit**

```bash
git add migrations/0050_polaris_connection_assignments.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): polaris_connection singleton + workspace_catalog_assignments"
```

---

## Task 2: Connection store + PolarisClient provider

**Files:** Create `crates/egret-controlplane/src/polaris_conn.rs`; modify `src/lib.rs`, `CpState`

- [ ] **Step 1: Failing test — upsert + read the connection**

```rust
#[tokio::test]
async fn polaris_connection_upserts_singleton() {
    let store = PolarisConnStore::new(db().await);
    store.upsert(&PolarisConnInput{ base_url:"http://polaris".into(), kc_token_url:"http://kc/token".into(),
        kc_client_id:"egret-admin".into(), kc_secret_ref: seed_secret().await, kc_scope: None }).await.unwrap();
    let got = store.get().await.unwrap().unwrap();
    assert_eq!(got.base_url, "http://polaris");
}
```

- [ ] **Step 2: Implement the store + a client builder**

```rust
pub struct PolarisConnStore { db: Db }
impl PolarisConnStore {
    pub async fn upsert(&self, i: &PolarisConnInput) -> Result<(), CpError> { /* INSERT ... ON CONFLICT (id) DO UPDATE */ }
    pub async fn get(&self) -> Result<Option<PolarisConn>, CpError> { /* SELECT ... WHERE id = true */ }
}
/// Build a PolarisClient from the registered connection (resolves the admin secret).
pub async fn polaris_client(store: &PolarisConnStore, secrets: &egret_secrets::SecretStore)
    -> Result<egret_polaris::PolarisClient, CpError> {
    let c = store.get().await?.ok_or(CpError::NotConfigured)?;
    let client_secret = secrets.open(c.kc_secret_ref).await?;   // plaintext admin client_secret
    Ok(egret_polaris::PolarisClient::new(
        egret_polaris::PolarisConfig { base_url: c.base_url, kc_token_url: c.kc_token_url,
            kc_client_id: c.kc_client_id, kc_client_secret: client_secret, kc_scope: c.kc_scope },
        std::sync::Arc::new(egret_polaris::ReqwestTransport{ http: reqwest::Client::new() })))
}
```

- [ ] **Step 3: Connection routes**

`GET/PUT /api/control-plane/polaris/connection` (register/inspect; `require_platform_admin`); `GET /api/control-plane/polaris/connection/health` → calls `client.health()`.

- [ ] **Step 4: Run + commit** — test passes.
```bash
git add crates/egret-controlplane
git commit -m "feat(control-plane): polaris connection store + client provider + connection routes"
```

---

## Task 3: Catalog routes (proxy egret-polaris)

**Files:** Create `crates/egret-controlplane/src/catalogs.rs`

- [ ] **Step 1: Failing test — create catalog builds StorageConfig from a storage connection**

```rust
#[tokio::test]
async fn create_catalog_maps_storage_connection() {
    // storage connection "minio" (bucket=warehouse, endpoint=minio:9000) exists (global).
    // POST /api/control-plane/polaris/catalogs {name:"lakehouse", storage_connection:"minio"}
    // asserts the outbound egret-polaris NewCatalog has StorageConfig::S3 { allowed_locations:["s3://warehouse/lakehouse/"], .. }
}
```
(Use a fake `PolarisClient` transport capturing the create-catalog body — reuse P3's `FakeTransport`.)

- [ ] **Step 2: Implement**

```rust
// POST /api/control-plane/polaris/catalogs {name, storage_connection}
async fn create(State(st): State<CpState>, Auth(p): Auth, Json(b): Json<CreateCatalogBody>) -> Result<(StatusCode, Json<CatalogDto>), CpError> {
    require_platform_admin(&p)?;
    let sc = st.storage.get_by_name(&b.storage_connection).await?.ok_or(CpError::BadRequest("unknown storage connection".into()))?;
    let base = format!("s3://{}/{}/", sc.bucket, b.name);          // allowed location for this catalog
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    let cat = client.create_catalog(&egret_polaris::NewCatalog{
        name: b.name.clone(), catalog_type: egret_polaris::CatalogType::Internal,
        storage: egret_polaris::StorageConfig::S3{ allowed_locations: vec![base], role_arn: None, region: sc.region.clone() },
        properties: Default::default(),
    }).await.map_err(CpError::from)?;
    Ok((StatusCode::CREATED, Json(cat.into())))
}
// GET list, GET/{name}, DELETE {name} — all require_platform_admin, proxy to client.{list_catalogs,get_catalog,delete_catalog}.
```

- [ ] **Step 3: Run + commit**
```bash
git add crates/egret-controlplane
git commit -m "feat(control-plane): catalog routes (proxy Polaris; storage connection → allowed location)"
```

---

## Task 4: RBAC routes (principals / roles / grants — proxy)

**Files:** Create `crates/egret-controlplane/src/rbac.rs`

- [ ] **Step 1: Failing test — add a table grant proxies to Polaris**

```rust
#[tokio::test]
async fn add_grant_route_proxies() {
    // PUT /api/control-plane/polaris/catalogs/lakehouse/catalog-roles/analysts/grants
    //   {type:"table", namespace:["marts"], table:"sales", privilege:"TABLE_READ_DATA"}
    // asserts a PUT to the Polaris grants path with that body (FakeTransport).
}
```

- [ ] **Step 2: Implement the proxy routes** (all `require_platform_admin`)

- `GET/POST /polaris/principals`, `DELETE /polaris/principals/{name}` → `list`/`create_principal`/`delete_principal`. **On create, the returned `clientSecret` is shown ONCE** in the response (never stored by these routes).
- `GET/POST /polaris/principal-roles`, `DELETE /{name}` → roles.
- `GET/POST /polaris/catalogs/{c}/catalog-roles`, `DELETE /{cr}` → catalog roles.
- `PUT /polaris/principals/{p}/principal-roles`, `PUT /polaris/principal-roles/{pr}/catalog-roles/{c}` → assignments.
- `PUT /polaris/catalogs/{c}/catalog-roles/{cr}/grants` (+ revoke) → `add_grant`/`revoke_grant`; map the JSON body to `egret_polaris::Grant`.

Each handler: `require_platform_admin`, build `polaris_client(...)`, call the method, map `PolarisError` → `CpError` (403/404/409/500).

- [ ] **Step 3: Run + commit**
```bash
git add crates/egret-controlplane
git commit -m "feat(control-plane): RBAC routes (principals/roles/grants) proxying Polaris"
```

---

## Task 5: Workspace catalog assignment

**Files:** Create `crates/egret-controlplane/src/assignment.rs`; modify `src/lib.rs`

- [ ] **Step 1: Failing test — assign provisions a principal + credential + grant + row**

```rust
#[tokio::test]
async fn assign_catalog_provisions_principal_and_records_row() {
    // POST /api/control-plane/workspaces/{ws}/catalog-assignments {catalog:"lakehouse"}
    // With a FakeTransport scripting: get_catalog 200, create_principal→{clientId,clientSecret},
    // create_principal_role 200, assign_principal_role 200, create_catalog_role 200,
    // assign_catalog_role 200, add_grant 200.
    // Asserts: a global Secret was written (holds clientId+clientSecret), and a
    // workspace_catalog_assignments row exists (ws, "lakehouse", principal_name, credential_ref).
}
```

- [ ] **Step 2: Implement assign**

```rust
// POST /api/control-plane/workspaces/{ws}/catalog-assignments {catalog}
async fn assign(State(st): State<CpState>, Auth(p): Auth, Path(ws): Path<Uuid>, Json(b): Json<AssignBody>) -> Result<Json<AssignmentDto>, CpError> {
    require_platform_admin(&p)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.get_catalog(&b.catalog).await.map_err(|_| CpError::BadRequest("unknown catalog".into()))?;
    let pname = format!("egret-ws-{ws}");
    let prole = format!("egret-ws-{ws}-role");
    let crole = format!("egret-ws-{ws}");
    // idempotent-ish: ignore Conflict on already-exists
    let creds = client.create_principal(&pname).await?;              // clientId/clientSecret (once)
    ignore_conflict(client.create_principal_role(&prole).await)?;
    ignore_conflict(client.assign_principal_role(&pname, &prole).await)?;
    ignore_conflict(client.create_catalog_role(&b.catalog, &crole).await)?;
    ignore_conflict(client.assign_catalog_role(&prole, &b.catalog, &crole).await)?;
    // rw on the whole catalog for the workspace (governance stays at the engine).
    client.add_grant(&b.catalog, &crole, &egret_polaris::Grant::Catalog{ privilege: "CATALOG_MANAGE_CONTENT".into() }).await?;
    // store clientId+clientSecret as a global secret; record the assignment.
    let secret_val = serde_json::json!({ "clientId": creds.credentials.client_id, "clientSecret": creds.credentials.client_secret }).to_string();
    let cred_ref = st.secrets.create(None, &format!("polaris/{}/{ws}", b.catalog), Some("engine service credential"), &secret_val).await?;
    let row = st.assignments.insert(ws, &b.catalog, &pname, cred_ref).await?;
    Ok(Json(row.into()))
}
```
Add `AssignmentStore::{insert, list_for_workspace, get, delete}` (over `workspace_catalog_assignments`), `ignore_conflict` (maps `PolarisError::Conflict` → `Ok`).

- [ ] **Step 3: Implement unassign**

`DELETE /api/control-plane/workspaces/{ws}/catalog-assignments/{catalog}`: delete the assignment row + the credential secret; best-effort `client.delete_principal(&pname)` (log on failure). `require_platform_admin`.

- [ ] **Step 4: Workspace read route (consume-only)**

`GET /api/ws/{ws}/catalogs` (member access via `WsContext`): list `workspace_catalog_assignments` for the workspace → `[{catalog_name}]` (no credentials exposed).

- [ ] **Step 5: Run + commit** — assign/unassign tests pass.
```bash
git add crates/egret-controlplane
git commit -m "feat(control-plane): workspace catalog assignment (provision principal + credential + grant)"
```

---

## Task 6: Mount + verify

**Files:** Modify `crates/egret-bin/src/main.rs` (state) + `crates/egret-server/src/lib.rs` (nest)

- [ ] **Step 1: Wire state + nest**

Build `CpState { storage, secrets, polaris_conn: PolarisConnStore::new(db), assignments: AssignmentStore::new(db) }` in `main.rs`; nest the full control-plane router (connection + catalogs + rbac + assignment + storage + secrets from P2) at `/api/control-plane` and the workspace read route under the workspace router.

- [ ] **Step 2: fmt + clippy + tests**

Run:
```bash
cargo fmt --all --check
cargo clippy -p egret-controlplane -p egret-bin --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-controlplane
```
Expected: pass. Authz tests: every `/api/control-plane/polaris/*` + assignment endpoint returns 403 for a non-`platform_admin`.

- [ ] **Step 3: Commit**
```bash
git add crates/egret-bin crates/egret-server
git commit -m "feat(control-plane): mount polaris catalog/RBAC/assignment routes"
```

---

## Self-review notes (spec coverage for P4)

- Spec **C. control-plane routes**: connection (Task 2), catalogs (Task 3), RBAC principals/roles/grants (Task 4), workspace assignment + service credential (Task 5), workspace read (Task 5), all `platform_admin`-gated (Task 6 authz tests).
- **Engine credential (v1 decision):** Polaris-native principal per assignment; `clientId`/`clientSecret` stored as a global Secret (the engine uses it via Polaris `/v1/oauth/tokens`). Migrate to Keycloak-mapped later (noted in P3/spec).
- **Assignment grant:** `CATALOG_MANAGE_CONTENT` (catalog-wide rw) — coarse on purpose; fine-grained governance is the engine's (Ranger) job.
- **Out of P4:** control-plane UI (P5), Polaris docker-compose + e2e + the Trino/StarRocks connector wiring that consumes the stored credential (P6).
