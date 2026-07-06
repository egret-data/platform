# Data Governance — Policies P1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship phase 1 of the Control-Plane **Policies** page — profile↔engine binding, a read-only Ranger policy list (Access/Masking/Row-filter tabs) driven by the Trino service-def, plus the sidebar menu move.

**Architecture:** A new session-auth, platform-admin `policies` module in `egret-controlplane` reads the shared `egret_authz::AuthzStore` (same store the machine-facing `/service` router writes). A *profile* is a Ranger service (`authz_services` row) with an independent lifecycle; engines link to one via a new nullable `authz_service_id` column, created-or-linked at provision time. The React list page mirrors the existing `ControlPlaneSecrets` page.

**Tech Stack:** Rust (axum 0.8, sqlx/Postgres), React 18 + TanStack Query + shadcn/ui, Vitest.

**Spec:** `docs/en/specs/2026-07-06-data-governance-policies-design.md` (§1 backend, §2 binding, §3 frontend — P1 subset).

**Conventions to respect (from project memory):**
- After adding a migration, **touch `crates/egret-metastore/src/lib.rs`** (add/adjust a comment) so the embedded migrator re-embeds — otherwise `serve` runs a stale migrator.
- Backend sweeps must pass `cargo fmt --all --check` + clippy; frontend must pass `npm run lint && npm run typecheck && npm run test && npm run build`.
- Backend DB route tests **self-skip** when `DATABASE_URL` is unset (return `None` from the fixture) — never hard-fail.

---

### Task 1: Migration + `authz_service_id` on engines

**Files:**
- Create: `migrations/0058_engine_authz_service.sql`
- Modify: `crates/egret-catalog/src/service/embedded.rs:35-44` (struct `Engine`), `:201-245` (`create_engine`, `list_engines`, `get_engine`), add `set_engine_authz_service`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to force re-embed)

- [ ] **Step 1: Write the migration**

Create `migrations/0058_engine_authz_service.sql`:

```sql
-- Link a provisioned engine to its Ranger policy profile (authz_services row).
-- Many engines MAY share one profile (no UNIQUE); deleting the service detaches
-- the engine (SET NULL) rather than deleting it. Nullable: pre-existing engines
-- and the standalone seeded `trino` service stay valid.
ALTER TABLE engines
    ADD COLUMN authz_service_id BIGINT NULL
        REFERENCES authz_services(id) ON DELETE SET NULL;
```

- [ ] **Step 2: Add the field to the `Engine` struct**

In `embedded.rs`, add to `pub struct Engine` (after `status`):

```rust
    pub status: String,
    pub authz_service_id: Option<i64>,
    pub created_at: DateTime<Utc>,
```

- [ ] **Step 3: Add the column to every engine query**

Update the three `RETURNING`/`SELECT` column lists in `create_engine`, `list_engines`, `get_engine` to include `authz_service_id`:

```rust
// create_engine RETURNING:
"INSERT INTO engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
 RETURNING id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at",
// list_engines SELECT:
"SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
 FROM engines ORDER BY created_at",
// get_engine SELECT:
"SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
 FROM engines WHERE id = $1",
```

- [ ] **Step 4: Add `set_engine_authz_service`**

After `set_engine_status` in `embedded.rs`:

```rust
    /// Bind (or detach with `None`) an engine to a Ranger policy profile.
    pub async fn set_engine_authz_service(
        &self,
        id: Uuid,
        authz_service_id: Option<i64>,
    ) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET authz_service_id = $2 WHERE id = $1")
            .bind(id)
            .bind(authz_service_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }
```

- [ ] **Step 5: Touch the metastore so the migration re-embeds**

In `crates/egret-metastore/src/lib.rs`, bump the migration-count comment (or add one) near the top, e.g. `// migrations: through 0058 (engine authz_service_id)`.

- [ ] **Step 6: Build to verify it compiles**

Run: `cargo build -p egret-catalog`
Expected: compiles (the new column is now selected everywhere `Engine` is `FromRow`-decoded).

- [ ] **Step 7: Commit**

```bash
git add migrations/0058_engine_authz_service.sql crates/egret-catalog/src/service/embedded.rs crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): link engines to a Ranger policy profile (authz_service_id)"
```

---

### Task 2: `AuthzStore::list_policies` (all policies for a service)

`enabled_policies_for_service` returns only enabled policies as raw `Value`. The admin list needs **all** policies (incl. disabled) as typed rows.

**Files:**
- Modify: `crates/egret-authz/src/store.rs` (add method near `enabled_policies_for_service`)
- Test: `crates/egret-authz/src/store.rs` (inline `#[cfg(test)]`) — DB-gated, self-skipping

- [ ] **Step 1: Write the failing test**

Add to the tests module in `store.rs` (mirror any existing DB-gated test's skip guard):

```rust
    #[tokio::test]
    async fn list_policies_returns_all_for_service() {
        let Some(store) = test_store().await else { return }; // skip without DATABASE_URL
        let svc = store.create_service("trino_lp_test", "trino", &json!({})).await.unwrap();
        store.create_policy(svc.id, "p1", &json!({"name":"p1","service":"trino_lp_test"})).await.unwrap();
        let rows = store.list_policies(svc.id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "p1");
        store.delete_service(svc.id).await.unwrap();
    }
```

If no `test_store()` helper exists, add one that reads `DATABASE_URL`, connects `Db`, runs `migrate()`, returns `Some(AuthzStore::new(db))` or `None`.

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=$DATABASE_URL cargo test -p egret-authz list_policies_returns_all_for_service -- --nocapture`
Expected: FAIL — `no method named list_policies`.

- [ ] **Step 3: Implement `list_policies`**

Add to the `impl AuthzStore` block that holds `enabled_policies_for_service`:

```rust
    /// All policies for a service (enabled and disabled), newest first — the
    /// admin list view. The DB `id`/`version`/`enabled` are injected into each
    /// stored body so the UI can round-trip them.
    pub async fn list_policies(&self, service_id: i64) -> Result<Vec<PolicyRow>, AuthzError> {
        Ok(sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version
               FROM authz_policies WHERE service_id = $1 ORDER BY id DESC"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=$DATABASE_URL cargo test -p egret-authz list_policies_returns_all_for_service`
Expected: PASS (or SKIP with no DB — then run once against a live `egret_test` before committing).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-authz/src/store.rs
git commit -m "feat(authz): list_policies(service_id) returns all policies incl disabled"
```

---

### Task 3: Wire `AuthzStore` into `CpState`

**Files:**
- Modify: `crates/egret-controlplane/Cargo.toml` (add `egret-authz`)
- Modify: `crates/egret-controlplane/src/lib.rs` (add `authz` field)
- Modify: `crates/egret-bin/src/main.rs:1581` (construct `authz`)
- Modify: `crates/egret-controlplane/tests/routes_api.rs:67` (construct `authz` in `setup_full`)

- [ ] **Step 1: Add the dependency**

In `crates/egret-controlplane/Cargo.toml` under `[dependencies]`:

```toml
egret-authz.workspace = true
```

- [ ] **Step 2: Add the `authz` field to `CpState`**

In `lib.rs`, add to `pub struct CpState` (after `storage`):

```rust
    /// Global storage-connection registry (`storage_connections`).
    pub storage: CatalogStore,
    /// Shared Ranger policy store (`authz_*`), same store the `/service` router writes.
    pub authz: egret_authz::AuthzStore,
```

- [ ] **Step 3: Construct it in egret-bin**

In `main.rs` at the `CpState { ... }` literal (line ~1581), add after `storage:`:

```rust
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
```

- [ ] **Step 4: Construct it in the test fixture**

In `routes_api.rs` `setup_full` at the `CpState { ... }` literal (line ~67), add after `storage:`:

```rust
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
```

- [ ] **Step 5: Build to verify**

Run: `cargo build -p egret-controlplane -p egret-bin`
Expected: compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/Cargo.toml crates/egret-controlplane/src/lib.rs crates/egret-bin/src/main.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): add AuthzStore to CpState"
```

---

### Task 4: `policies.rs` — summary DTO + parser (unit-tested, no DB)

**Files:**
- Create: `crates/egret-controlplane/src/policies.rs`
- Modify: `crates/egret-controlplane/src/lib.rs` (declare `mod policies;`)

- [ ] **Step 1: Write the failing test (Ranger JSON → summary)**

Create `crates/egret-controlplane/src/policies.rs` with just the test + type stubs' signatures:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarize_access_policy() {
        let body = json!({
            "name": "allow-analysts-select",
            "policyType": 0,
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]}
            },
            "policyItems": [
                {"users": ["alice"], "groups": ["analysts"], "accesses": [{"type": "select"}]}
            ]
        });
        let s = summarize_policy(42, "trino", true, &body);
        assert_eq!(s.id, 42);
        assert_eq!(s.name, "allow-analysts-select");
        assert_eq!(s.policy_type, "access");
        assert_eq!(s.resource_path, "iceberg.sales.orders.*");
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.principals.contains(&"analysts".to_string()));
        assert_eq!(s.permissions, vec!["select".to_string()]);
        assert!(s.is_enabled);
    }

    #[test]
    fn summarize_masking_and_rowfilter_types() {
        let mask = json!({"name":"m","policyType":1,"dataMaskPolicyItems":[
            {"groups":["pii"],"accesses":[{"type":"select"}]}]});
        assert_eq!(summarize_policy(1, "trino", true, &mask).policy_type, "masking");
        let rf = json!({"name":"r","policyType":2,"rowFilterPolicyItems":[
            {"users":["bob"],"accesses":[{"type":"select"}]}]});
        assert_eq!(summarize_policy(2, "trino", true, &rf).policy_type, "rowfilter");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-controlplane summarize_access_policy`
Expected: FAIL — `summarize_policy` / `PolicySummary` not found.

- [ ] **Step 3: Implement the DTO + parser**

At the top of `policies.rs` (above the tests):

```rust
//! Control-plane Policies router (`/policies`), nested under `/api/control-plane`.
//!
//! Read/parse Ranger policies from the shared `AuthzStore` into typed summaries
//! for the admin UI. Ranger stores three policy kinds by `policyType`:
//! 0 = access (`policyItems`), 1 = masking (`dataMaskPolicyItems`),
//! 2 = row-filter (`rowFilterPolicyItems`). P1 is read-only.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Human-readable name for a Ranger `policyType` integer.
fn policy_type_name(t: i64) -> &'static str {
    match t {
        1 => "masking",
        2 => "rowfilter",
        _ => "access",
    }
}

/// Flattened, UI-facing view of one stored Ranger policy.
#[derive(Debug, Serialize)]
pub struct PolicySummary {
    pub id: i64,
    pub name: String,
    pub service: String,
    pub policy_type: String,
    pub resource_path: String,
    pub principals: Vec<String>,
    pub permissions: Vec<String>,
    pub is_enabled: bool,
}

/// The item array key for each policy kind.
fn item_key(policy_type: i64) -> &'static str {
    match policy_type {
        1 => "dataMaskPolicyItems",
        2 => "rowFilterPolicyItems",
        _ => "policyItems",
    }
}

/// Parse a stored Ranger policy body into a `PolicySummary`.
pub fn summarize_policy(id: i64, service: &str, enabled: bool, body: &Value) -> PolicySummary {
    let policy_type = body.get("policyType").and_then(Value::as_i64).unwrap_or(0);

    // resource_path = catalog.schema.table.column, joining the first value of each level present.
    let res = body.get("resources").and_then(Value::as_object);
    let level = |k: &str| -> Option<String> {
        res.and_then(|r| r.get(k))
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let resource_path = ["catalog", "schema", "table", "column"]
        .iter()
        .filter_map(|k| level(k))
        .collect::<Vec<_>>()
        .join(".");

    // principals + permissions collected across this kind's items.
    let mut principals: Vec<String> = Vec::new();
    let mut permissions: Vec<String> = Vec::new();
    if let Some(items) = body.get(item_key(policy_type)).and_then(Value::as_array) {
        for it in items {
            for key in ["users", "groups", "roles"] {
                if let Some(arr) = it.get(key).and_then(Value::as_array) {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            if !principals.contains(&s.to_string()) {
                                principals.push(s.to_string());
                            }
                        }
                    }
                }
            }
            if let Some(acc) = it.get("accesses").and_then(Value::as_array) {
                for a in acc {
                    if let Some(t) = a.get("type").and_then(Value::as_str) {
                        if !permissions.contains(&t.to_string()) {
                            permissions.push(t.to_string());
                        }
                    }
                }
            }
        }
    }

    PolicySummary {
        id,
        name: body.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        service: service.to_string(),
        policy_type: policy_type_name(policy_type).to_string(),
        resource_path,
        principals,
        permissions,
        is_enabled: enabled,
    }
}
```

Declare the module in `lib.rs` (with the other `mod` lines):

```rust
mod policies;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p egret-controlplane summarize_access_policy summarize_masking_and_rowfilter_types`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/policies.rs crates/egret-controlplane/src/lib.rs
git commit -m "feat(controlplane): Ranger policy summary parser (all 3 kinds)"
```

---

### Task 5: `policies` read endpoints + router

**Files:**
- Modify: `crates/egret-controlplane/src/policies.rs` (add router + handlers)
- Modify: `crates/egret-controlplane/src/lib.rs` (`.merge(policies::router())`, export)
- Test: `crates/egret-controlplane/tests/routes_api.rs` (DB-gated route test)

- [ ] **Step 1: Add the handlers + router to `policies.rs`**

```rust
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub service: String,
    #[serde(default)]
    pub r#type: Option<String>, // access | masking | rowfilter
    #[serde(default)]
    pub q: Option<String>,
}

/// Engine(s) that link to a profile, plus the profile itself.
#[derive(Debug, Serialize)]
pub struct ProfileDto {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub policy_version: i64,
    pub engines: Vec<ProfileEngine>,
}

#[derive(Debug, Serialize)]
pub struct ProfileEngine {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/policies", get(list_policies))
        .route("/policies/{id}", get(get_policy))
        .route("/policies/services", get(list_profiles))
        .route("/policies/service-def/{name}", get(get_service_def))
}

/// GET /policies?service=&type=&q= — summaries, filtered.
async fn list_policies(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<ListQuery>,
) -> Result<Json<Vec<PolicySummary>>, CpError> {
    require_platform_admin(&p)?;
    let svc = st.authz.get_service(&qp.service).await?;
    let mut out: Vec<PolicySummary> = st
        .authz
        .list_policies(svc.id)
        .await?
        .into_iter()
        .map(|r| summarize_policy(r.id, &svc.name, r.enabled, &r.policy_jsonb))
        .collect();
    if let Some(t) = qp.r#type.as_deref() {
        out.retain(|s| s.policy_type == t);
    }
    if let Some(q) = qp.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty()) {
        out.retain(|s| {
            s.name.to_lowercase().contains(&q) || s.resource_path.to_lowercase().contains(&q)
        });
    }
    Ok(Json(out))
}

/// GET /policies/{id} — the raw stored Ranger body (with id), for the detail view.
async fn get_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    let row = st.authz.get_policy(id).await?;
    let mut body = row.policy_jsonb;
    if let Value::Object(m) = &mut body {
        m.insert("id".into(), Value::from(row.id));
        m.insert("isEnabled".into(), Value::from(row.enabled));
    }
    Ok(Json(body))
}

/// GET /policies/services — profiles (services) + the engine(s) that link to each.
async fn list_profiles(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<ProfileDto>>, CpError> {
    require_platform_admin(&p)?;
    let engines = st.storage.list_engines().await?;
    let profiles = st
        .authz
        .list_services()
        .await?
        .into_iter()
        .map(|s| ProfileDto {
            engines: engines
                .iter()
                .filter(|e| e.authz_service_id == Some(s.id))
                .map(|e| ProfileEngine {
                    id: e.id,
                    name: e.name.clone(),
                    status: e.status.clone(),
                })
                .collect(),
            id: s.id,
            name: s.name,
            r#type: s.r#type,
            policy_version: s.policy_version,
        })
        .collect();
    Ok(Json(profiles))
}

/// GET /policies/service-def/{name} — the def slices the editor renders from.
async fn get_service_def(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    Ok(Json(st.authz.get_service_def(&name).await?.def_jsonb))
}
```

- [ ] **Step 2: Map `AuthzError` → `CpError`**

`st.authz.*` returns `AuthzError`; the handlers use `?` into `CpError`. Add to `crates/egret-controlplane/src/error.rs` (mirror the `From<SecretsError>` impl):

```rust
impl From<egret_authz::AuthzError> for CpError {
    fn from(e: egret_authz::AuthzError) -> Self {
        use egret_authz::AuthzError as A;
        match e {
            A::NotFound(_) => CpError::NotFound,
            A::BadRequest(s) => CpError::BadRequest(s),
            _ => CpError::Internal,
        }
    }
}
```

(Confirm the `AuthzError` variant names by reading `crates/egret-authz/src/error.rs`; adjust arms to the actual variants — `NotFound`, `BadRequest` are used by the ranger CRUD so they exist.)

- [ ] **Step 3: Merge the router + export DTOs in `lib.rs`**

```rust
pub fn control_plane_router() -> Router<CpState> {
    Router::new()
        .merge(storage::router())
        .merge(secrets::router())
        .merge(warehouses::router())
        .merge(engines::router())
        .merge(policies::router())
}
```

- [ ] **Step 4: Write the failing route test**

In `routes_api.rs`, add (mirroring an existing `setup_full`-based test + the `platform_admin` session helper):

```rust
#[tokio::test]
async fn policies_list_requires_admin_and_filters() {
    let Some(fx) = setup_full(None, None).await else { return };
    // seed the trino service-def + a service + one access policy directly via AuthzStore
    let authz = egret_authz::AuthzStore::new(fx.db.clone());
    egret_authz::seed_trino(&authz).await.unwrap();
    let svc = authz.create_service("trino_route_test", "trino", &serde_json::json!({})).await.unwrap();
    authz.create_policy(svc.id, "p-access", &serde_json::json!({
        "name":"p-access","policyType":0,
        "resources":{"catalog":{"values":["iceberg"]}},
        "policyItems":[{"users":["alice"],"accesses":[{"type":"select"}]}]
    })).await.unwrap();

    let token = fx.admin_session().await; // existing helper: creates platform_admin + session
    let res = fx.app.clone().oneshot(
        Request::builder()
            .uri("/policies?service=trino_route_test&type=access")
            .header("cookie", format!("egret_session={token}"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = json_body(res).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["policyType"], "access");

    authz.delete_service(svc.id).await.unwrap();
}
```

Adapt `admin_session()` / `json_body()` / cookie name to the helpers already in `routes_api.rs` (read the top of the file — reuse its exact session-minting helper and request-builder style).

- [ ] **Step 5: Run test to verify it fails, then passes**

Run: `DATABASE_URL=$DATABASE_URL cargo test -p egret-controlplane policies_list_requires_admin_and_filters`
Expected: FAILS before Steps 1-3 land / on a non-admin path; PASSES after (or SKIPS without DB — run once against live `egret_test`).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/src/policies.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/src/error.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): read-only policies endpoints (list/get/services/service-def)"
```

---

### Task 6: Auto-bind — create-or-link a profile on engine provision

**Files:**
- Modify: `crates/egret-controlplane/src/engines.rs` (`CreateEngineBody` + `create_engine` handler ~1707-1786)

- [ ] **Step 1: Add the optional link field to `CreateEngineBody`**

Find `struct CreateEngineBody` in `engines.rs` and add:

```rust
    /// Link the new engine to an EXISTING Ranger profile (service id). When
    /// omitted, provisioning creates a fresh profile named after the engine.
    #[serde(default)]
    pub authz_service_id: Option<i64>,
```

- [ ] **Step 2: Bind after the engine row is created**

In `create_engine`, right after the engine row is created successfully (after the `let engine = match ... { Ok(e) => e, ... }` block, before returning), insert:

```rust
    // Auto-bind the engine to a Ranger policy profile: link an existing service,
    // or create a fresh one named after the engine. Best-effort — a binding
    // failure must not fail provisioning (the engine still works; the admin can
    // relink later), so log and continue.
    let profile_id = match b.authz_service_id {
        Some(sid) => {
            // Validate: the service exists (get_service is by name; use list to check id).
            match st.authz.list_services().await {
                Ok(svcs) if svcs.iter().any(|s| s.id == sid) => Some(sid),
                Ok(_) => { tracing::warn!(sid, "authz_service_id not found; leaving engine unlinked"); None }
                Err(e) => { tracing::warn!(?e, "list_services failed during auto-bind"); None }
            }
        }
        None => {
            egret_authz::seed_trino(&st.authz).await.ok();
            match st.authz.create_service(&b.name, "trino", &serde_json::json!({})).await {
                Ok(svc) => Some(svc.id),
                Err(e) => { tracing::warn!(?e, "create_service failed during auto-bind"); None }
            }
        }
    };
    if let Some(pid) = profile_id {
        if let Err(e) = st.storage.set_engine_authz_service(engine.id, Some(pid)).await {
            tracing::warn!(?e, "set_engine_authz_service failed");
        }
    }
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p egret-controlplane`
Expected: compiles.

- [ ] **Step 4: Write/extend a route test for binding**

In `routes_api.rs`, extend the existing engine-create test (or add one) to assert that after `POST /engines` (no `authz_service_id`), `GET /policies/services` lists a profile whose `engines` contains the new engine. If the engine-create test path needs an `EngineProvisioner`, pass `None` and assert on the DB row via `st.authz.list_services()` + `list_engines()` instead of the full HTTP provision path. Keep it DB-gated/self-skipping.

- [ ] **Step 5: Run the test**

Run: `DATABASE_URL=$DATABASE_URL cargo test -p egret-controlplane engine_autobind`
Expected: PASS (or SKIP without DB — verify once live).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/src/engines.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): auto-bind engine to a Ranger profile on provision (create or link)"
```

---

### Task 7: Frontend — `api/policies.ts` + menu move + route

**Files:**
- Create: `frontend/src/api/policies.ts`
- Modify: `frontend/src/components/layout/Sidebar.tsx` (move "Policies" to Control Plane)
- Modify: `frontend/src/App.tsx` (add `policies` route under `ControlPlaneRoutes`)
- Create: `frontend/src/routes/control-plane/Policies.tsx`

- [ ] **Step 1: Add the API module**

Create `frontend/src/api/policies.ts`:

```ts
import { apiJson } from "./client";

export interface PolicySummary {
  id: number;
  name: string;
  service: string;
  policy_type: "access" | "masking" | "rowfilter";
  resource_path: string;
  principals: string[];
  permissions: string[];
  is_enabled: boolean;
}

export interface ProfileEngine { id: string; name: string; status: string; }
export interface Profile {
  id: number;
  name: string;
  type: string;
  policy_version: number;
  engines: ProfileEngine[];
}

export function listProfiles(): Promise<Profile[]> {
  return apiJson<Profile[]>("/control-plane/policies/services");
}

export function listPolicies(service: string, type?: string): Promise<PolicySummary[]> {
  const p = new URLSearchParams({ service });
  if (type) p.set("type", type);
  return apiJson<PolicySummary[]>(`/control-plane/policies?${p.toString()}`);
}
```

- [ ] **Step 2: Move the menu item in `Sidebar.tsx`**

Remove the **Data Governance** group from `EXPLORE_GROUPS` (delete the `{ heading: "Data Governance", items: [ Policies, Data Lineage ] }` block) and re-add **Data Lineage** under a leaner Explore governance group, then add Policies to `CONTROL_PLANE_GROUPS`. Concretely:

In `EXPLORE_GROUPS`, replace the Data Governance block with (keep Data Lineage only):

```tsx
  {
    heading: "Data Governance",
    items: [
      { label: "Data Lineage", icon: Share2 },
    ],
  },
```

In `CONTROL_PLANE_GROUPS`, add a group before "Control Plane" or after it:

```tsx
  {
    heading: "Data Governance",
    items: [
      { label: "Policies", icon: ShieldCheck },
    ],
  },
```

(`ShieldCheck` and `Share2` are already imported.)

- [ ] **Step 3: Add the route in `App.tsx`**

Import the page and add the route inside `ControlPlaneRoutes`'s `<Routes>`:

```tsx
import Policies from "@/routes/control-plane/Policies";
// ...inside ControlPlaneRoutes <Routes>, after the secrets route:
          <Route path="policies" element={<Policies />} />
```

- [ ] **Step 4: Build to verify wiring (page stub next task) — skip build until Task 8 adds the page.**

Proceed to Task 8; commit both together.

---

### Task 8: Frontend — `Policies.tsx` list page + Vitest

**Files:**
- Create: `frontend/src/routes/control-plane/Policies.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/Policies.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/control-plane/__tests__/Policies.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import Policies from "../Policies";

vi.mock("@/api/policies", () => ({
  listProfiles: () => Promise.resolve([
    { id: 1, name: "trino", type: "trino", policy_version: 3, engines: [{ id: "e1", name: "prod", status: "running" }] },
  ]),
  listPolicies: () => Promise.resolve([
    { id: 42, name: "allow-analysts", service: "trino", policy_type: "access",
      resource_path: "iceberg.sales.orders.*", principals: ["analysts"], permissions: ["select"], is_enabled: true },
  ]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("Policies page", () => {
  it("renders the profile and a policy row", async () => {
    wrap(<Policies />);
    expect(await screen.findByText("allow-analysts")).toBeInTheDocument();
    expect(await screen.findByText("iceberg.sales.orders.*")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/Policies.test.tsx`
Expected: FAIL — cannot find `../Policies`.

- [ ] **Step 3: Implement the page**

Create `frontend/src/routes/control-plane/Policies.tsx` (mirror `ControlPlaneSecrets` structure: `useQuery`, `Card`, `Table`, `Badge`):

```tsx
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import { type Profile, type PolicySummary, listProfiles, listPolicies } from "@/api/policies";
import { ApiError } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@/components/ui/table";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

export default function Policies() {
  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const [service, setService] = useState<string | null>(null);
  const [type, setType] = useState<(typeof TYPES)[number]["key"]>("access");

  // Default to the first profile once loaded.
  const activeService = service ?? profiles.data?.[0]?.name ?? null;

  const policies = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", activeService, type],
    queryFn: () => listPolicies(activeService!, type),
    enabled: !!activeService,
    retry: false,
  });

  const rows = policies.data ?? [];

  return (
    <div className="space-y-4 p-6">
      <div className="flex items-center gap-2">
        <ShieldCheck className="h-5 w-5" />
        <h1 className="text-xl font-semibold">Policies</h1>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Profiles</CardTitle>
          <CardDescription>Ranger policy sets. Each engine enforces one profile.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-2">
          {(profiles.data ?? []).map((p) => (
            <button
              key={p.id}
              onClick={() => setService(p.name)}
              className={`rounded-md border px-3 py-1.5 text-sm ${
                p.name === activeService ? "bg-accent text-accent-foreground" : "text-muted-foreground"
              }`}
            >
              {p.name}
              {p.engines.length > 0 && (
                <span className="ml-2 text-xs opacity-70">{p.engines.map((e) => e.name).join(", ")}</span>
              )}
            </button>
          ))}
        </CardContent>
      </Card>

      <div className="flex gap-1 border-b">
        {TYPES.map((t) => (
          <button
            key={t.key}
            onClick={() => setType(t.key)}
            className={`px-3 py-2 text-sm ${type === t.key ? "border-b-2 border-primary font-medium" : "text-muted-foreground"}`}
          >
            {t.label}
          </button>
        ))}
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Resource</TableHead>
            <TableHead>Principals</TableHead>
            <TableHead>Permissions</TableHead>
            <TableHead>Enabled</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.id}>
              <TableCell className="font-medium">{r.name}</TableCell>
              <TableCell className="font-mono text-xs">{r.resource_path}</TableCell>
              <TableCell>{r.principals.join(", ")}</TableCell>
              <TableCell>{r.permissions.join(", ")}</TableCell>
              <TableCell>
                <Badge variant={r.is_enabled ? "secondary" : "outline"}>
                  {r.is_enabled ? "enabled" : "disabled"}
                </Badge>
              </TableCell>
            </TableRow>
          ))}
          {rows.length === 0 && (
            <TableRow>
              <TableCell colSpan={5} className="text-center text-muted-foreground">
                No {type} policies for this profile.
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/Policies.test.tsx`
Expected: PASS.

- [ ] **Step 5: Full frontend gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/policies.ts frontend/src/routes/control-plane/Policies.tsx frontend/src/routes/control-plane/__tests__/Policies.test.tsx frontend/src/components/layout/Sidebar.tsx frontend/src/App.tsx
git commit -m "feat(frontend): Control-Plane Policies read-only list + profile selector + type tabs"
```

---

### Task 9: Full local CI + manual smoke

- [ ] **Step 1: Backend gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && DATABASE_URL=$DATABASE_URL cargo test --workspace`
Expected: all pass (DB tests run against a live `egret_test`; never run two `cargo test --workspace` concurrently).

- [ ] **Step 2: Frontend gate** (already run in Task 8, re-run if backend changed shapes)

Run: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`

- [ ] **Step 3: Manual smoke** (per `egret-frontend-viz-verify`)

Start the stack (docker + `cargo run -p egret-bin -- serve` + `npm run dev`), log in as `admin@egret.local` / `Admin12345!`, open **Control Plane → Data Governance → Policies**. Verify: the profile selector lists the seeded `trino` profile (and any provisioned engine's profile); the Access/Masking/Row-filter tabs switch; seeding a policy via the API makes it appear in the Access tab. State exactly what was exercised.

- [ ] **Step 4: Commit any fixups, then stop for review.**

---

## Self-Review notes
- **Spec coverage (P1 subset):** menu move (Task 7) ✓; migration + auto-bind create-or-link + keep-on-delete via `ON DELETE SET NULL` (Tasks 1, 6) ✓; backend `services` + `service-def` + `list`/`get` (Task 5) ✓; `PoliciesList` read-only + profile selector + type tabs (Task 8) ✓; def-driven service-def endpoint present for the P2 editor (Task 5) ✓. **Deferred to P2/P3 (not in this plan):** create/update/delete, principals endpoint + pickers, masking/row-filter editors, Layer-B download bundle, Java-harness enforcement tests.
- **Type consistency:** `authz_service_id: Option<i64>` used identically in the migration, `Engine` struct, `set_engine_authz_service`, `list_profiles` filter, and `CreateEngineBody`; `PolicySummary` field names match between `summarize_policy`, the endpoint, and the TS interface (`policy_type`, `resource_path`, `is_enabled` — snake_case on both sides since no serde rename is applied).
- **Open confirmations for the implementer:** the exact `AuthzError` variant names (Task 5 Step 2) and the `routes_api.rs` session/`json_body` helper names (Task 5 Step 4, Task 6 Step 4) — read those files and match; both are noted inline.
