# Assign Engine → Workspace — P1 (Backend Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a platform admin assign a provisioned engine to a workspace; the assignment materializes a read-only, per-user-delegated `query.databases` connection so the engine is immediately queryable in that workspace via the existing query path.

**Architecture:** A new `engine_workspaces` join table records the assignment. `query.databases` gains a nullable `engine_id` that both marks a row as provisioned (read-only) and links it to the source engine. An **assignment service** in `egret-engine-mgmt` orchestrates the two seams it spans — `EngineMgmtStore` (engines) and `egret_query::DatabaseRepo` (materialize/remove the connection). Auth needs no new work: the materialized row sets `delegated = true`, so the shipped B3a per-user-JWT path (`EngineTokenMinter`) authenticates each query as the real user.

**Tech Stack:** Rust (axum, sqlx, utoipa), Postgres. Schemas: engines live in `engine_mgmt.*`, databases in `query.*`.

**Scope:** P1 is backend only (no frontend). P2 (surfaces: CP Engine-detail Workspaces tab, Workspace-detail assign, workspace two-group Engines page, SQL Lab tag) is a separate plan. Target engine type for delegated query is **Trino**; StarRocks parity is out of scope.

---

## Key facts (verified against current code)

- Engines: `engine_mgmt.engines` (`Engine { id, name, type, namespace, warehouse_id, spec: jsonb, status, authz_service_id, created_at }`), store `EngineMgmtStore` (`crates/egret-engine-mgmt/src/store.rs`).
- Engine catalogs: `engine_mgmt.engine_catalogs` (`EngineCatalog { id, engine_id, name, kind, config, secret_ref, is_primary, created_at }`).
- Databases: `query.databases`, repo `egret_query::registry::repo::DatabaseRepo` with `create_in_workspace(workspace_id, input: &DatabaseInput, secret_id, created_by)`, `get`, `list_for_workspace`, `get_in_workspace`, `delete_in_workspace`. `DatabaseInput { name, engine, host, port, options, delegated }`.
- Delegation: `query.databases.delegated=true` → `execution/routes.rs` mints the caller's engine JWT via `EngineTokenMinter` → Trino authenticates as the user. No new auth work.
- Coordinator address: `host = "{engine.name}-coordinator.{engine.namespace}.svc"`, `port = 8080` (`egret-orchestrator/src/kube_client.rs`).
- Next migration number: **0084**.
- Engine router (`egret-engine-mgmt/src/engines.rs::router() -> Router<CpState>`) mounts under `/api/control-plane`. Workspace-admin router is `admin_workspace_router()` (`egret-workspace`, `WorkspaceState`) under `/api/admin`.

---

## File structure

- Create: `migrations/0084_engine_workspaces.sql`
- Modify: `crates/egret-engine-mgmt/src/store.rs` — assignment CRUD on `engine_workspaces`.
- Create: `crates/egret-engine-mgmt/src/assignment.rs` — `EngineAssignmentService` (orchestrates store + DatabaseRepo).
- Modify: `crates/egret-engine-mgmt/Cargo.toml` — add `egret-query` dep (for `DatabaseRepo`); confirm no cycle.
- Modify: `crates/egret-query/src/registry/repo.rs` + `model.rs` — `create_provisioned`, expose `engine_id` on `Database`.
- Modify: `crates/egret-query/src/registry/routes.rs` (workspace database router) — read-only guards.
- Modify: `crates/egret-engine-mgmt/src/engines.rs` + `state.rs` — CP assignment endpoints + read-only engine view.
- Modify: `crates/egret-workspace/src/routes.rs` — workspace-admin assignment endpoints (delegate to the service).
- Modify: server wiring (`crates/egret-server/src/lib.rs`) — inject the assignment service into the relevant router states.

---

## Task 1: Migration — assignment table + `databases.engine_id`

**Files:** Create `migrations/0084_engine_workspaces.sql`

- [ ] **Step 1: Write the migration**

```sql
-- P1: assign a provisioned engine to a workspace. The join table records the
-- assignment; `query.databases.engine_id` marks a connection as provisioned
-- (read-only, managed by an assignment) and links it back to the source engine.
CREATE TABLE engine_mgmt.engine_workspaces (
    engine_id    uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    assigned_by  uuid REFERENCES users(id),
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, workspace_id)
);

ALTER TABLE query.databases
    ADD COLUMN engine_id uuid NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE;

-- A workspace can hold at most one materialized connection per engine.
CREATE UNIQUE INDEX databases_engine_per_workspace
    ON query.databases (workspace_id, engine_id) WHERE engine_id IS NOT NULL;
```

> Confirm the exact schema name for the engine tables by grepping an existing engine migration/store query (`engine_mgmt.engines`). If engines live in `public.engines` in this checkout, drop the `engine_mgmt.` qualifier.

- [ ] **Step 2: Force migrator re-embed + apply**

Touch the metastore lib so the embedded migrator picks up the new file, then run the DB tests' migrate path:

Run: `touch crates/egret-metastore/src/lib.rs && cargo build -p egret-metastore`
Expected: builds. (Per the sqlx-migration-staleness gotcha, a new migration file needs a metastore rebuild.)

- [ ] **Step 3: Commit**

```bash
git add migrations/0084_engine_workspaces.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(engine): migration for engine↔workspace assignment"
```

---

## Task 2: `EngineMgmtStore` — assignment CRUD

**Files:** Modify `crates/egret-engine-mgmt/src/store.rs`; Test: `crates/egret-engine-mgmt/tests/` (mirror the existing store test file)

- [ ] **Step 1: Write the failing test**

Add a `#[sqlx::test]` (or the DB-gated `connect()` pattern this crate's tests already use) that seeds a warehouse + engine + workspace, then:

```rust
#[sqlx::test]
async fn assign_list_unassign_engine_workspace(pool: PgPool) {
    let store = EngineMgmtStore::new(Db::from_pool(pool.clone()));
    let (engine_id, ws_id, admin) = seed_engine_and_ws(&pool).await; // helper: insert engine_mgmt.engines + workspaces rows

    store.assign_workspace(engine_id, ws_id, admin).await.unwrap();
    let ws = store.list_workspaces_for_engine(engine_id).await.unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].workspace_id, ws_id);

    let engines = store.list_engines_for_workspace(ws_id).await.unwrap();
    assert_eq!(engines.len(), 1);

    store.unassign_workspace(engine_id, ws_id).await.unwrap();
    assert!(store.list_workspaces_for_engine(engine_id).await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-engine-mgmt assign_list_unassign -- --nocapture`
Expected: FAIL (methods don't exist).

- [ ] **Step 3: Add the store methods + row type**

In `store.rs`:

```rust
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineWorkspace {
    pub engine_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub assigned_at: DateTime<Utc>,
}

impl EngineMgmtStore {
    pub async fn assign_workspace(&self, engine_id: Uuid, workspace_id: Uuid, by: Uuid)
        -> Result<(), EngineMgmtError> {
        sqlx::query(
            "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id, assigned_by) \
             VALUES ($1,$2,$3) ON CONFLICT (engine_id, workspace_id) DO NOTHING",
        )
        .bind(engine_id).bind(workspace_id).bind(by)
        .execute(self.pool()).await?;
        Ok(())
    }

    pub async fn unassign_workspace(&self, engine_id: Uuid, workspace_id: Uuid)
        -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engine_workspaces WHERE engine_id=$1 AND workspace_id=$2")
            .bind(engine_id).bind(workspace_id)
            .execute(self.pool()).await?;
        Ok(())
    }

    pub async fn list_workspaces_for_engine(&self, engine_id: Uuid)
        -> Result<Vec<EngineWorkspace>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineWorkspace>(
            "SELECT ew.engine_id, ew.workspace_id, w.name, w.slug, ew.assigned_at \
             FROM engine_mgmt.engine_workspaces ew JOIN workspaces w ON w.id = ew.workspace_id \
             WHERE ew.engine_id = $1 ORDER BY w.name",
        ).bind(engine_id).fetch_all(self.pool()).await?)
    }

    pub async fn list_engines_for_workspace(&self, workspace_id: Uuid)
        -> Result<Vec<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT e.id, e.name, e.type, e.namespace, e.warehouse_id, e.spec, e.status, e.authz_service_id, e.created_at \
             FROM engine_mgmt.engines e JOIN engine_mgmt.engine_workspaces ew ON ew.engine_id = e.id \
             WHERE ew.workspace_id = $1 ORDER BY e.name",
        ).bind(workspace_id).fetch_all(self.pool()).await?)
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p egret-engine-mgmt assign_list_unassign -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/tests
git commit -m "feat(engine): engine↔workspace assignment store methods"
```

---

## Task 3: Engine helpers — coordinator address + JWT assignability

**Files:** Modify `crates/egret-engine-mgmt/src/store.rs` (impl on `Engine`); Test: same test file

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn coordinator_host_and_jwt_check() {
    let e = Engine {
        id: Uuid::nil(), name: "trino-prod".into(), r#type: "trino".into(),
        namespace: "engines".into(), warehouse_id: Uuid::nil(),
        spec: serde_json::json!({ "auth": { "jwt": { "jwks_uri": "http://kc/jwks" } } }),
        status: "running".into(), authz_service_id: None, created_at: Utc::now(),
    };
    assert_eq!(e.coordinator_host(), "trino-prod-coordinator.engines.svc");
    assert_eq!(e.coordinator_port(), 8080);
    assert!(e.supports_jwt_delegation());

    let e2 = Engine { spec: serde_json::json!({ "auth": {} }), ..e.clone() };
    assert!(!e2.supports_jwt_delegation());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-engine-mgmt coordinator_host_and_jwt -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
impl Engine {
    pub fn coordinator_host(&self) -> String {
        format!("{}-coordinator.{}.svc", self.name, self.namespace)
    }
    pub fn coordinator_port(&self) -> i32 { 8080 }
    /// Delegated query requires the coordinator to accept the minted engine JWT.
    pub fn supports_jwt_delegation(&self) -> bool {
        self.spec.get("auth").and_then(|a| a.get("jwt")).is_some()
    }
}
```

> Confirm the `spec` JSON shape for JWT auth by reading how `egret-orchestrator/src/trino.rs::auth_type` reads `auth.jwt` — match the exact key path (the test above assumes `spec.auth.jwt`).

- [ ] **Step 4: Run to verify it passes** — `cargo test -p egret-engine-mgmt coordinator_host_and_jwt` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/store.rs
git commit -m "feat(engine): coordinator address + JWT-assignability helpers"
```

---

## Task 4: `DatabaseRepo` — materialize a provisioned connection

**Files:** Modify `crates/egret-query/src/registry/repo.rs`, `model.rs`; Test: the repo's existing DB test file

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn create_provisioned_sets_engine_id_and_delegated(pool: PgPool) {
    let repo = DatabaseRepo::new(Db::from_pool(pool.clone()));
    let (ws, engine_id) = seed_ws_and_engine(&pool).await;
    let db = repo.create_provisioned(ws, engine_id, "trino-prod", "trino",
        "trino-prod-coordinator.engines.svc", 8080,
        &serde_json::json!({ "catalog": "sale" }), None).await.unwrap();
    assert!(db.delegated);
    let fetched = repo.get(db.id).await.unwrap();
    assert_eq!(fetched.engine_id, Some(engine_id));
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p egret-query create_provisioned_sets_engine_id` → FAIL.

- [ ] **Step 3: Add `engine_id` to the `Row`/`Database` model + a `create_provisioned` method**

In `model.rs` add `pub engine_id: Option<Uuid>` to `Database` (and the internal `Row` + `into_database()` mapping). Update every `SELECT` list in `repo.rs` to include `engine_id` (get/list_for_workspace/get_in_workspace).

In `repo.rs`:

```rust
    /// Materialize a read-only, per-user-delegated connection for a provisioned
    /// engine assigned to a workspace. `delegated = true` so the query path mints
    /// the caller's engine JWT; `engine_id` marks the row read-only + links the engine.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_provisioned(
        &self, workspace_id: Uuid, engine_id: Uuid, name: &str, engine: &str,
        host: &str, port: i32, options: &serde_json::Value, created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "INSERT INTO query.databases \
               (name, engine, host, port, options, secret_id, created_by, workspace_id, delegated, engine_id) \
             VALUES ($1,$2,$3,$4,$5,NULL,$6,$7,true,$8) \
             RETURNING id, name, engine, host, port, options, secret_id, status, delegated, engine_id",
        )
        .bind(name).bind(engine).bind(host).bind(port).bind(options)
        .bind(created_by.map(|u| u.0)).bind(workspace_id).bind(engine_id)
        .fetch_one(&self.db.pool).await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => DatabaseError::AlreadyExists(name.to_string()),
            _ => DatabaseError::Db(e),
        })?
        .into_database()
    }
```

> Add `engine_id` to the `RETURNING`/`SELECT` of `create_in_workspace`, `get`, `list_for_workspace`, `get_in_workspace` too, so `Row` deserializes consistently. Also add `pub async fn delete_by_engine(&self, workspace_id: Uuid, engine_id: Uuid)` = `DELETE FROM query.databases WHERE workspace_id=$1 AND engine_id=$2` for unassign.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p egret-query create_provisioned_sets_engine_id` → PASS, then `cargo test -p egret-query` to catch model/SELECT drift.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-query/src/registry
git commit -m "feat(query): materialize provisioned (delegated, read-only) databases rows"
```

---

## Task 5: Read-only guards on the workspace database routes

**Files:** Modify `crates/egret-query/src/registry/routes.rs` (the `workspace_database_router` handlers); Test: same crate's route tests

- [ ] **Step 1: Write the failing test**

Seed a provisioned row (via `create_provisioned`) in a workspace, then assert `PUT`, `DELETE`, and Test-connection on it return 409:

```rust
#[sqlx::test]
async fn provisioned_databases_are_read_only(pool: PgPool) {
    let (app, cookie, ws) = ws_db_app(pool.clone()).await; // existing test harness for the workspace database router
    let db = seed_provisioned_db(&pool, ws).await;          // engine_id set
    for (method, path) in [("PUT", format!("/api/ws/{ws}/databases/{db}")),
                           ("DELETE", format!("/api/ws/{ws}/databases/{db}"))] {
        let res = send(&app, &cookie, method, &path, method_body(method)).await;
        assert_eq!(res.status(), StatusCode::CONFLICT, "{method} must be rejected");
    }
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL (edit/delete currently succeed).

- [ ] **Step 3: Guard in each mutating handler**

In the `update`, `delete`, and `test`-connection handlers of `workspace_database_router`, after loading the target row, reject provisioned rows:

```rust
    let db = st.databases.get_in_workspace(ws, id).await?;
    if db.engine_id.is_some() {
        return Err(EngineError::Conflict(
            "this connection is provisioned by egret and is read-only".into()).into());
    }
```

> Use whatever `Conflict`/409 variant the crate's error enum already exposes; match the existing error-to-status mapping. If there is no `Conflict` variant, add one mapping to `StatusCode::CONFLICT`.

- [ ] **Step 4: Run to verify it passes** — PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-query/src/registry/routes.rs
git commit -m "feat(query): provisioned databases connections are read-only in the workspace API"
```

---

## Task 6: `EngineAssignmentService` — orchestrate the two seams

**Files:** Create `crates/egret-engine-mgmt/src/assignment.rs`; Modify `crates/egret-engine-mgmt/src/lib.rs`, `Cargo.toml`; Test: `crates/egret-engine-mgmt/tests`

- [ ] **Step 1: Add the dependency (check for a cycle first)**

Run: `cargo tree -p egret-query -i egret-engine-mgmt`
Expected: NO path (egret-query must not already depend on egret-engine-mgmt). If clean, add to `crates/egret-engine-mgmt/Cargo.toml`: `egret-query = { workspace = true }`.

- [ ] **Step 2: Write the failing test**

```rust
#[sqlx::test]
async fn assign_materializes_a_delegated_connection(pool: PgPool) {
    let svc = EngineAssignmentService::new(
        EngineMgmtStore::new(Db::from_pool(pool.clone())),
        DatabaseRepo::new(Db::from_pool(pool.clone())));
    let (engine_id, ws, admin) = seed_jwt_engine_and_ws(&pool).await; // engine spec has auth.jwt

    svc.assign(engine_id, ws, admin).await.unwrap();
    let dbs = DatabaseRepo::new(Db::from_pool(pool.clone())).list_for_workspace(ws).await.unwrap();
    let mat = dbs.iter().find(|d| d.engine_id == Some(engine_id)).expect("materialized row");
    assert!(mat.delegated);
    assert_eq!(mat.host, "trino-prod-coordinator.engines.svc");

    svc.unassign(engine_id, ws).await.unwrap();
    assert!(DatabaseRepo::new(Db::from_pool(pool)).list_for_workspace(ws).await.unwrap()
        .iter().all(|d| d.engine_id != Some(engine_id)));
}

#[sqlx::test]
async fn assign_rejects_non_jwt_engine(pool: PgPool) {
    let svc = /* as above */;
    let (engine_id, ws, admin) = seed_non_jwt_engine_and_ws(&pool).await;
    assert!(matches!(svc.assign(engine_id, ws, admin).await, Err(EngineMgmtError::NotAssignable(_))));
}
```

- [ ] **Step 3: Run to verify it fails** — FAIL.

- [ ] **Step 4: Implement the service**

```rust
use egret_query::registry::repo::DatabaseRepo;
use crate::store::EngineMgmtStore;
use crate::error::EngineMgmtError;
use uuid::Uuid;

#[derive(Clone)]
pub struct EngineAssignmentService {
    engines: EngineMgmtStore,
    databases: DatabaseRepo,
}

impl EngineAssignmentService {
    pub fn new(engines: EngineMgmtStore, databases: DatabaseRepo) -> Self {
        Self { engines, databases }
    }

    pub async fn assign(&self, engine_id: Uuid, workspace_id: Uuid, by: Uuid)
        -> Result<(), EngineMgmtError> {
        let engine = self.engines.get_engine(engine_id).await?
            .ok_or_else(|| EngineMgmtError::NotFound(engine_id.to_string()))?;
        if !engine.supports_jwt_delegation() {
            return Err(EngineMgmtError::NotAssignable(
                "engine must have JWT auth enabled before it can be assigned".into()));
        }
        // Idempotent: if already assigned, do nothing.
        self.engines.assign_workspace(engine_id, workspace_id, by).await?;
        // Materialize the read-only connection (skip if one already exists for this engine).
        let existing = self.databases.list_for_workspace(workspace_id).await
            .map_err(to_mgmt_err)?;
        if existing.iter().any(|d| d.engine_id == Some(engine_id)) { return Ok(()); }
        let name = dedup_name(&engine.name, &existing);
        let opts = serde_json::json!({}); // seed catalog defaults in a later step if needed
        self.databases.create_provisioned(
            workspace_id, engine_id, &name, &engine.r#type,
            &engine.coordinator_host(), engine.coordinator_port(), &opts,
            Some(egret_core::UserId(by)),
        ).await.map_err(to_mgmt_err)?;
        Ok(())
    }

    pub async fn unassign(&self, engine_id: Uuid, workspace_id: Uuid)
        -> Result<(), EngineMgmtError> {
        self.databases.delete_by_engine(workspace_id, engine_id).await.map_err(to_mgmt_err)?;
        self.engines.unassign_workspace(engine_id, workspace_id).await?;
        Ok(())
    }
}

fn dedup_name(base: &str, existing: &[egret_query::registry::model::Database]) -> String {
    if !existing.iter().any(|d| d.name == base) { return base.to_string(); }
    (2..).map(|n| format!("{base}-{n}"))
        .find(|c| !existing.iter().any(|d| &d.name == c))
        .unwrap()
}
```

Add `NotAssignable(String)` and (if missing) `NotFound(String)` variants to `EngineMgmtError`, and a `to_mgmt_err(DatabaseError) -> EngineMgmtError` conversion. Export the service from `lib.rs`.

> Confirm `DatabaseRepo::new` and `Db::from_pool` exist with these names; if `Db` construction differs, match the real constructor used in `egret-query` tests.

- [ ] **Step 5: Run to verify it passes** — both tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-engine-mgmt
git commit -m "feat(engine): assignment service (materialize/remove delegated connection)"
```

---

## Task 7: CP engine assignment endpoints

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`, `state.rs`; server wiring; Test: engine-mgmt route tests

- [ ] **Step 1: Write the failing test**

An HTTP test (mirror the crate's existing engine route tests) that `PUT /api/control-plane/engines/{id}/workspaces { workspace_ids: [ws] }` then `GET …/workspaces` returns `[ws]`, and materialized row exists.

- [ ] **Step 2: Run to verify it fails** — 404 (routes unregistered).

- [ ] **Step 3: Add routes + handlers + state**

Add the service to the router state (`CpState` or a dedicated engine-mgmt state — match `engines.rs::router()`'s state type; add `pub assignment: EngineAssignmentService` there and a `FromRef` if needed). Add:

```rust
        .route("/engines/{id}/workspaces", get(list_engine_workspaces).put(set_engine_workspaces))
```

Handlers: `list_engine_workspaces` → `store.list_workspaces_for_engine(id)`; `set_engine_workspaces` → diff the requested set against current, calling `assignment.assign` / `assignment.unassign` per delta. Gate `require_platform_admin` (the pattern used by the other engine handlers). Add `#[utoipa::path]` annotations and register in `EngineMgmtApi`.

- [ ] **Step 4–5: Verify PASS + commit**

```bash
git add crates/egret-engine-mgmt crates/egret-server
git commit -m "feat(engine): CP endpoints to assign engines to workspaces"
```

---

## Task 8: Workspace-admin assignment endpoints

**Files:** Modify `crates/egret-workspace/src/routes.rs` (or a thin router in `egret-engine-mgmt` mounted under `/api/admin/workspaces`); server wiring; Test

- [ ] **Step 1: Write the failing test** — `GET /api/admin/workspaces/{id}/engines` empty; `POST …/engines {engine_id}` assigns; `GET` shows it; `DELETE …/engines/{engine_id}` removes it.

- [ ] **Step 2: Run to verify it fails** — 404.

- [ ] **Step 3: Implement**

Because these endpoints also need the `EngineAssignmentService` (which lives in `egret-engine-mgmt`), mount a small router from `egret-engine-mgmt` at `/api/admin/workspaces/{id}/engines` with its own state carrying the service + `EngineMgmtStore`, rather than adding an engine dependency to `egret-workspace`. Handlers: `GET` → `store.list_engines_for_workspace(id)`; `POST {engine_id}` → `assignment.assign(engine_id, id, actor)`; `DELETE /{engine_id}` → `assignment.unassign`. Gate platform_admin. utoipa-annotate + register.

- [ ] **Step 4–5: Verify PASS + commit**

```bash
git commit -am "feat(engine): workspace-admin endpoints to assign engines"
```

---

## Task 9: Read-only engine view for the workspace

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`; Test

- [ ] **Step 1: Write the failing test** — `GET /api/ws/{ws}/engines/{engine_id}` returns `{name,type,status,catalogs:[{name,kind,storage_name,secret_name}]}` for an assigned engine; 404 for an unassigned one.

- [ ] **Step 2: Run to verify it fails** — 404.

- [ ] **Step 3: Implement**

Route `GET /api/ws/{workspace}/engines/{engine_id}` (mounted under `/api/ws`), gated `catalog:read` on the ws (reuse `WsContext`). Verify the engine is assigned to `{ws}` (else 404). Build the DTO: `store.get_engine` + `store.list_engine_catalogs(engine_id)`; for each catalog resolve `storage_name` (engine.warehouse_id → warehouse → storage connection name) and `secret_name` (`secret_ref` → workspace_secrets/secret name, **name only**). 

> Resolving `storage_name` crosses into the warehouse/storage seam — confirm the lookup path (warehouse → storage connection) and reuse an existing read method rather than a raw join if one exists. If that resolution is heavy, return `storage_name: null` best-effort in P1 and complete it in P2.

- [ ] **Step 4–5: Verify PASS + commit**

```bash
git commit -am "feat(engine): read-only per-workspace engine view (catalogs, storage, secret)"
```

---

## Task 10: Regenerate OpenAPI + full verification

- [ ] **Step 1: Regenerate the spec + client**

Run: `cargo run -p egret-bin --bin egret -- openapi > openapi.json && (cd frontend && npm run gen:api && npm run typecheck)`
Expected: new engine-workspace paths appear; client regenerates; typecheck clean.

- [ ] **Step 2: Backend CI locally**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-engine-mgmt -p egret-query -p egret-workspace`
Expected: green. (Do not run two `cargo test --workspace` suites concurrently.)

- [ ] **Step 3: Live smoke (manual, not automated)**

On the dev stack, provision (or pick) a **JWT-configured** Trino engine, assign it to a workspace via the CP API, then as a workspace user run a SQL Lab query against the materialized connection → returns rows and reaches Trino as that user (delegation). Confirm the connection is read-only (edit/delete rejected) in the workspace database API.

- [ ] **Step 4: Commit + open PR**

```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate client for engine assignment endpoints"
```

Then use `superpowers:finishing-a-development-branch`. PR title: `feat(engine): assign provisioned engines to workspaces (P1 backend)`.

---

## Self-review notes

- **Spec coverage (P1):** assignment table (T1), store CRUD (T2), coordinator+JWT helpers (T3), materialize delegated read-only connection (T4), read-only guards (T5), orchestration service (T6), CP endpoints (T7), workspace-admin endpoints (T8), read-only engine view (T9). P2 surfaces (FE, SQL Lab tag) are a separate plan.
- **Auth:** no dedicated task — `create_provisioned` sets `delegated=true`, so the shipped B3a path handles per-user JWT. T10 live smoke proves it.
- **Cross-seam wiring risk:** T6 adds `egret-query` as a dep of `egret-engine-mgmt`; T6 Step 1 checks for a cycle first. Endpoints that span both seams (T8) are mounted from `egret-engine-mgmt` to avoid making `egret-workspace` depend on the query seam.
- **Verify-before-invent flags:** engine schema name (`engine_mgmt.` vs `public.`), `spec.auth.jwt` key path, `Db`/`DatabaseRepo` constructors, the crate error `Conflict` variant, and the warehouse→storage resolution — each is called out inline to confirm against real code before writing.
