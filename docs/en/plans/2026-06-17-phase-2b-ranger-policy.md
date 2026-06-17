# Phase 2b — Apache Ranger Policy & REST Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose a Ranger Admin REST-compatible surface (`egret-ranger`) backed by a policy store (`egret-authz`) so the native Trino Ranger plugin downloads versioned `ServicePolicies` (access + row-filter + column-mask) from Egret and enforces them.

**Architecture:** Two new crates — `egret-authz` (Postgres `AuthzStore`: seeded Trino service-def, services + policies CRUD, `policy_version` counter) and `egret-ranger` (axum HTTP at `/service`, Ranger JSON DTOs). Policies are stored **native** — a `RangerPolicy` is a `jsonb` blob stored/served verbatim; only the `ServicePolicies` envelope is a typed Rust struct. The download endpoint assembles `ServicePolicies { serviceName, serviceId, policyVersion, serviceDef, policies }` and supports `lastKnownVersion` → 304. Verified by loading Egret's JSON into the real `RangerPolicyEngineImpl`.

**Tech Stack:** Rust (workspace, edition 2021, MSRV 1.85) · axum 0.8 · sqlx 0.8 (Postgres, **runtime** queries) · serde/serde_json · the Apache Ranger reference at `data-portal/ranger` (Trino service-def + policy-engine test). JVM + Maven (`org.apache.ranger:ranger-plugins-common`) for the enforcement gate.

**Reference:** Spec `docs/en/specs/2026-06-17-phase-2b-ranger-policy-design.md`. Ranger model: `ranger/agents-common/src/main/java/org/apache/ranger/plugin/{model/RangerPolicy.java,util/ServicePolicies.java,policyengine/RangerPolicyEngineImpl.java}`; Trino def: `ranger/agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json`.

**Conventions (follow the existing codebase — same as Phase 2a):**
- Crates in `crates/<name>/`, registered in root `Cargo.toml` `[workspace.dependencies]`; crate `Cargo.toml` uses `edition.workspace = true`, `license.workspace = true`, `<dep>.workspace = true`.
- **Runtime sqlx only** — NEVER the `sqlx::query!`/`query_scalar!` compile-time macros. Use `sqlx::query_as::<_, Row>("... $1").bind(x)` with `#[derive(sqlx::FromRow)]` row structs, `sqlx::query_scalar::<_, T>`, `sqlx::query(...).bind(...)`. Bind `jsonb` as `serde_json::Value`, `bigint` as `i64`, `bigserial` PKs as `i64`. (See `crates/egret-catalog/src/service/store.rs` and `crates/egret-workspace/src/workspaces.rs`.)
- Migrations: plain SQL `migrations/000N_name.sql`; bump the `egret-metastore` `MIGRATOR.migrations.len() >= N` test.
- DB-gated tests in `crates/<name>/tests/*.rs` self-skip without `DATABASE_URL` (see `crates/egret-catalog/tests/store_ns.rs`). A live Postgres is available at `postgres://egret:egret@localhost:5432/egret`.
- Config: nested struct on `egret_config::Config` + `Default` + `validate()`; tests via `figment::Jail`.
- Server: `egret_server::AppState` carries `Option<_>` sub-states; `build_router` mounts when `Some` (the catalog mounts `/catalog` via `Router::new().nest(...).with_state(())` — mirror that for `/service`).
- Bearer auth: mirror `crates/egret-catalog/src/auth.rs` `require_bearer` (constant-time compare; 401 on missing/bad).
- Plain `git commit` (never override author).

---

## Task 0: Scaffold `egret-authz` + `egret-ranger` crates

**Files:** Modify `Cargo.toml`; create `crates/egret-authz/{Cargo.toml,src/lib.rs}`, `crates/egret-ranger/{Cargo.toml,src/lib.rs}`.

- [ ] **Step 1: Root `Cargo.toml` `[workspace.dependencies]`** — add:
```toml
egret-authz = { path = "crates/egret-authz" }
egret-ranger = { path = "crates/egret-ranger" }
```
(All other needed deps — axum, sqlx, serde, serde_json, uuid, chrono, thiserror, tracing, async-trait, tower, http-body-util — already exist in the workspace.)

- [ ] **Step 2: `crates/egret-authz/Cargo.toml`:**
```toml
[package]
name = "egret-authz"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
```

- [ ] **Step 3: `crates/egret-ranger/Cargo.toml`:**
```toml
[package]
name = "egret-ranger"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-config.workspace = true
egret-metastore.workspace = true
egret-authz.workspace = true
axum.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true
http.workspace = true

[dev-dependencies]
tokio.workspace = true
tower.workspace = true
http-body-util = "0.1"
egret-config.workspace = true
```

- [ ] **Step 4: Placeholder `lib.rs` files:**
`crates/egret-authz/src/lib.rs`:
```rust
//! Ranger policy storage + versioning: service-defs, services, policies (native RangerPolicy jsonb).
```
`crates/egret-ranger/src/lib.rs`:
```rust
//! Ranger Admin REST-compatible surface: service-def/service/policy CRUD + versioned policy download.
```

- [ ] **Step 5: Build** — `cargo build -p egret-authz -p egret-ranger`. Expected: compiles.

- [ ] **Step 6: Commit**
```bash
git add Cargo.toml Cargo.lock crates/egret-authz crates/egret-ranger
git commit -m "feat(ranger): scaffold egret-authz + egret-ranger crates"
```

---

## Task 1: `egret-ranger::dto` — ServicePolicies envelope + Trino def fixture

**Files:** Create `crates/egret-ranger/src/dto.rs`; create `crates/egret-ranger/tests/fixtures/trino-servicedef.json` (copied); modify `crates/egret-ranger/src/lib.rs`.

The Ranger model uses Jackson `@JsonInclude(NON_EMPTY)` + camelCase. Since we store policies & the service-def **verbatim as jsonb**, only the `ServicePolicies` envelope needs a typed struct; `serviceDef` and each policy are `serde_json::Value`.

- [ ] **Step 1: Copy the Trino service-def into a test fixture** (used by tests now; the seed copy is created in Task 5):
```bash
mkdir -p crates/egret-ranger/tests/fixtures
cp /Users/1pro-l4r8/Documents/github/data-portal/ranger/agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json crates/egret-ranger/tests/fixtures/trino-servicedef.json
```

- [ ] **Step 2: Define the envelope** — `crates/egret-ranger/src/dto.rs`:
```rust
//! Ranger wire DTOs. Only the ServicePolicies envelope is typed; policies and the
//! service-def pass through as `serde_json::Value` (stored verbatim as jsonb).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The download payload the Trino Ranger plugin consumes (`@JsonInclude(NON_EMPTY)`, camelCase).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicePolicies {
    pub service_name: String,
    pub service_id: i64,
    pub policy_version: i64,
    /// epoch millis
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_update_time: Option<i64>,
    pub service_def: Value,
    pub policies: Vec<Value>,
    #[serde(default = "default_audit_mode")]
    pub audit_mode: String,
}

fn default_audit_mode() -> String { "DEFAULT".to_string() }

impl ServicePolicies {
    pub fn new(service_name: String, service_id: i64, policy_version: i64,
               service_def: Value, policies: Vec<Value>, updated_ms: Option<i64>) -> Self {
        Self { service_name, service_id, policy_version, policy_update_time: updated_ms,
               service_def, policies, audit_mode: default_audit_mode() }
    }
}

/// Empty, well-formed tag payload so a polling plugin doesn't error (tag policies deferred).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceTags {
    pub op: String,
    pub service_name: String,
    pub tag_version: i64,
    pub tags: serde_json::Map<String, Value>,
    pub service_resources: Vec<Value>,
}
impl ServiceTags {
    pub fn empty(service_name: String) -> Self {
        Self { op: "add".into(), service_name, tag_version: 1,
               tags: serde_json::Map::new(), service_resources: vec![] }
    }
}
```

- [ ] **Step 3: Wire `lib.rs`** — `crates/egret-ranger/src/lib.rs`:
```rust
//! Ranger Admin REST-compatible surface: service-def/service/policy CRUD + versioned policy download.
pub mod dto;
```

- [ ] **Step 4: Unit test — envelope serializes to camelCase + the Trino def parses** — append to `dto.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_policies_serializes_camel_case() {
        let sp = ServicePolicies::new("trino_prod".into(), 1, 5,
            serde_json::json!({"name":"trino"}), vec![], Some(0));
        let j = serde_json::to_value(&sp).unwrap();
        assert_eq!(j["serviceName"], "trino_prod");
        assert_eq!(j["policyVersion"], 5);
        assert_eq!(j["serviceDef"]["name"], "trino");
        assert_eq!(j["auditMode"], "DEFAULT");
    }
    #[test]
    fn trino_servicedef_fixture_is_valid_json_with_resources() {
        let raw = include_str!("../tests/fixtures/trino-servicedef.json");
        let def: Value = serde_json::from_str(raw).expect("trino def parses");
        assert_eq!(def["name"], "trino");
        // resource hierarchy present
        let resources = def["resources"].as_array().unwrap();
        let names: Vec<&str> = resources.iter().filter_map(|r| r["name"].as_str()).collect();
        for r in ["catalog","schema","table","column"] { assert!(names.contains(&r), "missing {r}"); }
    }
}
```

- [ ] **Step 5: Run** — `cargo test -p egret-ranger dto`. Expected: 2 pass.
- [ ] **Step 6: Commit**
```bash
git add crates/egret-ranger
git commit -m "feat(ranger): ServicePolicies/ServiceTags DTOs + Trino service-def fixture"
```

---

## Task 2: Migration `0007_authz.sql` + bump MIGRATOR

**Files:** Create `migrations/0007_authz.sql`; modify `crates/egret-metastore/src/lib.rs`.

- [ ] **Step 1: `migrations/0007_authz.sql`** (exact SQL from spec §8):
```sql
CREATE TABLE authz_service_defs (
    id         bigint PRIMARY KEY,
    name       text NOT NULL UNIQUE,
    def_jsonb  jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_services (
    id             bigserial PRIMARY KEY,
    name           text NOT NULL UNIQUE,
    type           text NOT NULL REFERENCES authz_service_defs(name),
    config_jsonb   jsonb NOT NULL DEFAULT '{}',
    policy_version bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_policies (
    id           bigserial PRIMARY KEY,
    service_id   bigint NOT NULL REFERENCES authz_services(id) ON DELETE CASCADE,
    name         text NOT NULL,
    policy_jsonb jsonb NOT NULL,
    enabled      boolean NOT NULL DEFAULT true,
    version      bigint NOT NULL DEFAULT 1,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (service_id, name)
);

CREATE TABLE policy_audit (
    id        bigserial PRIMARY KEY,
    policy_id bigint,
    action    text NOT NULL,
    actor     uuid REFERENCES users(id),
    at        timestamptz NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Bump assertion** in `crates/egret-metastore/src/lib.rs`:
```rust
        assert!(
            MIGRATOR.migrations.len() >= 7,
            "expected through the authz migration"
        );
```

- [ ] **Step 3: Run** — `cargo test -p egret-metastore`. Expected: PASS (embeds 0007).
- [ ] **Step 4: Commit**
```bash
git add migrations/0007_authz.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(ranger): 0007_authz.sql schema (service_defs/services/policies/audit)"
```

---

## Task 3: `AuthzStore` — service-def seed/read + services CRUD

**Files:** Create `crates/egret-authz/src/{error.rs,store.rs}`; modify `crates/egret-authz/src/lib.rs`; test `crates/egret-authz/tests/store_services.rs`.

- [ ] **Step 1: `error.rs`:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```

- [ ] **Step 2: `lib.rs`:**
```rust
//! Ranger policy storage + versioning: service-defs, services, policies (native RangerPolicy jsonb).
pub mod error;
pub mod store;
pub use error::AuthzError;
pub use store::AuthzStore;
```

- [ ] **Step 3: `store.rs` — service-def + services** (runtime sqlx; unique → AlreadyExists):
```rust
use crate::AuthzError;
use egret_metastore::Db;
use serde_json::Value;

#[derive(Clone)]
pub struct AuthzStore { db: Db }

#[derive(sqlx::FromRow)]
pub struct ServiceDefRow { pub id: i64, pub name: String, pub def_jsonb: Value }

#[derive(sqlx::FromRow)]
pub struct ServiceRow {
    pub id: i64, pub name: String, pub r#type: String,
    pub config_jsonb: Value, pub policy_version: i64,
}

impl AuthzStore {
    pub fn new(db: Db) -> Self { Self { db } }

    /// Idempotent upsert of a service-def by name (used to seed Trino).
    pub async fn upsert_service_def(&self, id: i64, name: &str, def: &Value) -> Result<(), AuthzError> {
        sqlx::query(
            r#"INSERT INTO authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,$3)
               ON CONFLICT (name) DO UPDATE SET def_jsonb = EXCLUDED.def_jsonb"#)
            .bind(id).bind(name).bind(def)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn get_service_def(&self, name: &str) -> Result<ServiceDefRow, AuthzError> {
        sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM authz_service_defs WHERE name = $1"#)
            .bind(name).fetch_optional(&self.db.pool).await?
            .ok_or_else(|| AuthzError::NotFound(format!("service-def {name}")))
    }

    pub async fn list_service_defs(&self) -> Result<Vec<ServiceDefRow>, AuthzError> {
        Ok(sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM authz_service_defs ORDER BY name"#)
            .fetch_all(&self.db.pool).await?)
    }

    pub async fn create_service(&self, name: &str, typ: &str, config: &Value) -> Result<ServiceRow, AuthzError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"INSERT INTO authz_services (name, type, config_jsonb)
               VALUES ($1,$2,$3) RETURNING id, name, type, config_jsonb, policy_version"#)
            .bind(name).bind(typ).bind(config)
            .fetch_one(&self.db.pool).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => AuthzError::AlreadyExists(name.into()),
                _ => AuthzError::Db(e),
            })
    }

    pub async fn get_service(&self, name: &str) -> Result<ServiceRow, AuthzError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version FROM authz_services WHERE name = $1"#)
            .bind(name).fetch_optional(&self.db.pool).await?
            .ok_or_else(|| AuthzError::NotFound(format!("service {name}")))
    }

    pub async fn list_services(&self) -> Result<Vec<ServiceRow>, AuthzError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version FROM authz_services ORDER BY name"#)
            .fetch_all(&self.db.pool).await?)
    }

    pub async fn delete_service(&self, id: i64) -> Result<(), AuthzError> {
        let n = sqlx::query(r#"DELETE FROM authz_services WHERE id = $1"#)
            .bind(id).execute(&self.db.pool).await?.rows_affected();
        if n == 0 { return Err(AuthzError::NotFound(format!("service id {id}"))); }
        Ok(())
    }
}
```

- [ ] **Step 4: DB-gated test** — `crates/egret-authz/tests/store_services.rs`:
```rust
//! DB-gated AuthzStore service tests. Self-skip without DATABASE_URL.
use egret_authz::AuthzStore;
use egret_metastore::Db;
use serde_json::json;

async fn store() -> Option<AuthzStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(AuthzStore::new(db))
}
fn uniq(p: &str) -> String { format!("{p}_{}", uuid::Uuid::new_v4().simple()) }

#[tokio::test]
async fn seed_def_then_create_service() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let defname = uniq("trino");
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []})).await.unwrap();
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []})).await.unwrap(); // idempotent
    assert_eq!(s.get_service_def(&defname).await.unwrap().name, defname);

    let svcname = uniq("trino_prod");
    let svc = s.create_service(&svcname, &defname, &json!({})).await.unwrap();
    assert_eq!(svc.policy_version, 1);
    assert!(matches!(s.create_service(&svcname, &defname, &json!({})).await.unwrap_err(),
                     egret_authz::AuthzError::AlreadyExists(_)));
    s.delete_service(svc.id).await.unwrap();
    assert!(s.get_service(&svcname).await.is_err());
}
```
(Add `uuid.workspace = true` to `egret-authz` `[dev-dependencies]` if the compiler flags it; it's already a normal dep so it should be available.)

- [ ] **Step 5: Run** — `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-authz store_services`. Expected: PASS with DB; skip+PASS without.
- [ ] **Step 6: Commit**
```bash
git add crates/egret-authz
git commit -m "feat(authz): AuthzStore service-def seed/read + services CRUD (DB-gated)"
```

---

## Task 4: `AuthzStore` — policies CRUD + `policy_version` bump

**Files:** Modify `crates/egret-authz/src/store.rs`; test `crates/egret-authz/tests/store_policies.rs`.

- [ ] **Step 1: Add policy methods** (each write bumps the service's `policy_version` in the same transaction):
```rust
#[derive(sqlx::FromRow)]
pub struct PolicyRow {
    pub id: i64, pub service_id: i64, pub name: String,
    pub policy_jsonb: Value, pub enabled: bool, pub version: i64,
}

impl AuthzStore {
    /// Create a policy under a service and bump policy_version atomically. Returns the new policy id.
    pub async fn create_policy(&self, service_id: i64, name: &str, policy: &Value)
        -> Result<i64, AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let id: i64 = sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO authz_policies (service_id, name, policy_jsonb)
               VALUES ($1,$2,$3) RETURNING id"#)
            .bind(service_id).bind(name).bind(policy)
            .fetch_one(&mut *tx).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => AuthzError::AlreadyExists(name.into()),
                _ => AuthzError::Db(e),
            })?;
        sqlx::query(r#"UPDATE authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#)
            .bind(service_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn update_policy(&self, id: i64, name: &str, policy: &Value, enabled: bool)
        -> Result<(), AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"SELECT service_id FROM authz_policies WHERE id = $1"#)
            .bind(id).fetch_optional(&mut *tx).await?;
        let Some(service_id) = svc else { tx.rollback().await?; return Err(AuthzError::NotFound(format!("policy {id}"))); };
        sqlx::query(r#"UPDATE authz_policies SET name=$2, policy_jsonb=$3, enabled=$4, version=version+1, updated_at=now() WHERE id=$1"#)
            .bind(id).bind(name).bind(policy).bind(enabled).execute(&mut *tx).await?;
        sqlx::query(r#"UPDATE authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#)
            .bind(service_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_policy(&self, id: i64) -> Result<(), AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"DELETE FROM authz_policies WHERE id = $1 RETURNING service_id"#)
            .bind(id).fetch_optional(&mut *tx).await?;
        let Some(service_id) = svc else { tx.rollback().await?; return Err(AuthzError::NotFound(format!("policy {id}"))); };
        sqlx::query(r#"UPDATE authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#)
            .bind(service_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_policy(&self, id: i64) -> Result<PolicyRow, AuthzError> {
        sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version FROM authz_policies WHERE id = $1"#)
            .bind(id).fetch_optional(&self.db.pool).await?
            .ok_or_else(|| AuthzError::NotFound(format!("policy {id}")))
    }

    /// Enabled policies of a service (for download assembly).
    pub async fn enabled_policies_for_service(&self, service_id: i64) -> Result<Vec<Value>, AuthzError> {
        Ok(sqlx::query_scalar::<_, Value>(
            r#"SELECT policy_jsonb FROM authz_policies WHERE service_id = $1 AND enabled = true ORDER BY id"#)
            .bind(service_id).fetch_all(&self.db.pool).await?)
    }
}
```

- [ ] **Step 2: DB-gated test** — `crates/egret-authz/tests/store_policies.rs`:
```rust
//! DB-gated AuthzStore policy tests. Self-skip without DATABASE_URL.
use egret_authz::AuthzStore;
use egret_metastore::Db;
use serde_json::json;

async fn store() -> Option<AuthzStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(AuthzStore::new(db))
}
fn uniq(p: &str) -> String { format!("{p}_{}", uuid::Uuid::new_v4().simple()) }

#[tokio::test]
async fn policy_crud_bumps_version() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let defname = uniq("trino");
    s.upsert_service_def(901000, &defname, &json!({"name": defname})).await.unwrap();
    let svc = s.create_service(&uniq("svc"), &defname, &json!({})).await.unwrap();
    assert_eq!(svc.policy_version, 1);

    let pol = json!({"name":"p1","service":svc.name,"policyType":0,
        "resources":{"catalog":{"values":["hive"],"isExcludes":false,"isRecursive":false}},
        "policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"]}]});
    let pid = s.create_policy(svc.id, "p1", &pol).await.unwrap();
    assert_eq!(s.get_service(&svc.name).await.unwrap().policy_version, 2); // bumped

    s.update_policy(pid, "p1", &pol, true).await.unwrap();
    assert_eq!(s.get_service(&svc.name).await.unwrap().policy_version, 3);

    assert_eq!(s.enabled_policies_for_service(svc.id).await.unwrap().len(), 1);
    s.delete_policy(pid).await.unwrap();
    assert_eq!(s.get_service(&svc.name).await.unwrap().policy_version, 4);
    assert_eq!(s.enabled_policies_for_service(svc.id).await.unwrap().len(), 0);
}
```

- [ ] **Step 3: Run** — `DATABASE_URL=... cargo test -p egret-authz store_policies`. Expected: PASS with DB.
- [ ] **Step 4: Commit**
```bash
git add crates/egret-authz
git commit -m "feat(authz): policy CRUD with atomic policy_version bump (DB-gated)"
```

---

## Task 5: Embed the Trino service-def + seed-on-init

**Files:** Create `crates/egret-authz/src/service-defs/trino.json` (copied); create `crates/egret-authz/src/service_def.rs`; modify `crates/egret-authz/src/lib.rs`.

- [ ] **Step 1: Copy the official Trino service-def into the crate source:**
```bash
mkdir -p crates/egret-authz/src/service-defs
cp /Users/1pro-l4r8/Documents/github/data-portal/ranger/agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json crates/egret-authz/src/service-defs/trino.json
```

- [ ] **Step 2: `service_def.rs`** — embed + parse + seed:
```rust
use crate::{AuthzError, AuthzStore};
use serde_json::Value;

/// The official Trino Ranger service-def, embedded at compile time.
pub const TRINO_SERVICE_DEF_JSON: &str = include_str!("service-defs/trino.json");
/// Service-def name and id as declared in the embedded JSON (name "trino", id 203).
pub const TRINO_DEF_NAME: &str = "trino";

/// Parse the embedded Trino service-def.
pub fn trino_service_def() -> Result<Value, AuthzError> {
    Ok(serde_json::from_str(TRINO_SERVICE_DEF_JSON)?)
}

/// Idempotently seed the Trino service-def into the store. Call at startup when ranger is enabled.
pub async fn seed_trino(store: &AuthzStore) -> Result<(), AuthzError> {
    let def = trino_service_def()?;
    let id = def["id"].as_i64().ok_or_else(|| AuthzError::BadRequest("trino def missing numeric id".into()))?;
    let name = def["name"].as_str().unwrap_or(TRINO_DEF_NAME);
    store.upsert_service_def(id, name, &def).await
}
```

- [ ] **Step 3: Export from `lib.rs`** — add:
```rust
pub mod service_def;
pub use service_def::{seed_trino, trino_service_def, TRINO_DEF_NAME};
```

- [ ] **Step 4: Unit test** — append to `service_def.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_trino_def_parses_with_id_and_resources() {
        let def = trino_service_def().unwrap();
        assert_eq!(def["name"], "trino");
        assert!(def["id"].as_i64().is_some());
        let access: Vec<&str> = def["accessTypes"].as_array().unwrap().iter()
            .filter_map(|a| a["name"].as_str()).collect();
        assert!(access.contains(&"select"));
        assert!(def["dataMaskDef"].is_object());
        assert!(def["rowFilterDef"].is_object());
    }
}
```

- [ ] **Step 5: Run** — `cargo test -p egret-authz service_def`. Expected: PASS.
- [ ] **Step 6: Commit**
```bash
git add crates/egret-authz
git commit -m "feat(authz): embed official Trino service-def + idempotent seed_trino"
```

---

## Task 6: `egret-ranger` download endpoint + tags stub

**Files:** Create `crates/egret-ranger/src/{error.rs,routes.rs,api/mod.rs,api/download.rs}`; modify `crates/egret-ranger/src/lib.rs`.

- [ ] **Step 1: `error.rs`** — Ranger-style minimal error:
```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_authz::AuthzError;

pub struct RangerError(pub AuthzError);
impl From<AuthzError> for RangerError { fn from(e: AuthzError) -> Self { Self(e) } }

impl IntoResponse for RangerError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AuthzError::NotFound(_) => StatusCode::NOT_FOUND,
            AuthzError::AlreadyExists(_) => StatusCode::CONFLICT,
            AuthzError::BadRequest(_) | AuthzError::Json(_) => StatusCode::BAD_REQUEST,
            AuthzError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "msg": self.0.to_string(), "statusCode": status.as_u16() }))).into_response()
    }
}
```

- [ ] **Step 2: `RangerState` + `routes.rs`** (auth layer added in Task 7; download routes here):
```rust
use crate::api;
use axum::routing::get;
use axum::Router;
use egret_authz::AuthzStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct RangerState {
    pub store: AuthzStore,
    pub auth_token: Arc<str>,
}

pub fn ranger_router(state: RangerState) -> Router {
    Router::new()
        .route("/plugins/policies/download/{service}", get(api::download::download_policies))
        .route("/tags/download/{service}", get(api::download::download_tags))
        // service-def + service + policy CRUD routes are added in Task 7
        .merge(api::crud_routes())
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::require_bearer))
        .with_state(state)
}
```
NOTE: Task 7 creates `crate::auth` and `api::crud_routes()`. To keep this task self-contained and compiling, for now define a temporary `pub fn crud_routes() -> Router<RangerState> { Router::new() }` in `api/mod.rs` and a passthrough `crate::auth::require_bearer` stub — OR (cleaner) implement `auth.rs` now (it's tiny) and the empty `crud_routes()` now, filling CRUD handlers in Task 7. Implement `auth.rs` now:

`crates/egret-ranger/src/auth.rs`:
```rust
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use crate::routes::RangerState;

pub async fn require_bearer(State(state): State<RangerState>, req: Request<axum::body::Body>, next: Next)
    -> Result<Response, StatusCode> {
    let token = req.headers().get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(t) if ct_eq(t.as_bytes(), state.auth_token.as_bytes()) => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut d = 0u8; for (x, y) in a.iter().zip(b) { d |= x ^ y; } d == 0
}
```

- [ ] **Step 3: `api/mod.rs` + `api/download.rs`:**
`api/mod.rs`:
```rust
pub mod download;
pub mod crud;
pub use crud::crud_routes;
```
`api/download.rs`:
```rust
use crate::dto::{ServicePolicies, ServiceTags};
use crate::error::RangerError;
use crate::routes::RangerState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct DownloadParams { #[serde(rename = "lastKnownVersion")] pub last_known_version: Option<i64> }

pub async fn download_policies(State(s): State<RangerState>, Path(service): Path<String>,
    Query(q): Query<DownloadParams>) -> Result<Response, RangerError> {
    let svc = s.store.get_service(&service).await?;       // 404 if unknown
    if q.last_known_version == Some(svc.policy_version) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let def = s.store.get_service_def(&svc.r#type).await?.def_jsonb;
    let policies = s.store.enabled_policies_for_service(svc.id).await?;
    let sp = ServicePolicies::new(svc.name, svc.id, svc.policy_version, def, policies, Some(0));
    Ok(Json(sp).into_response())
}

pub async fn download_tags(State(s): State<RangerState>, Path(service): Path<String>,
    Query(_q): Query<DownloadParams>) -> Result<Response, RangerError> {
    let _ = s.store.get_service(&service).await?;
    Ok(Json(ServiceTags::empty(service)).into_response())
}
```

- [ ] **Step 4: `lib.rs`** — add `pub mod api; pub mod auth; pub mod error; pub mod routes; pub use routes::{ranger_router, RangerState};`

- [ ] **Step 5: Stub `api/crud.rs`** (filled in Task 7) so it compiles:
```rust
use crate::routes::RangerState;
use axum::Router;
/// CRUD routes (service-defs read, services, policies) — implemented in Task 7.
pub fn crud_routes() -> Router<RangerState> { Router::new() }
```

- [ ] **Step 6: Build + run** — `cargo build -p egret-ranger`; `cargo test -p egret-ranger`. Expected: compiles; dto tests still pass.
- [ ] **Step 7: Commit**
```bash
git add crates/egret-ranger
git commit -m "feat(ranger): policy/tag download endpoints (304/200) + bearer auth + router"
```

---

## Task 7: `egret-ranger` service-def/service/policy CRUD handlers

**Files:** Modify `crates/egret-ranger/src/api/crud.rs`.

- [ ] **Step 1: Implement CRUD** — Ranger-compatible handlers (accept/return native JSON `Value`):
```rust
use crate::dto::ServicePolicies;
use crate::error::RangerError;
use crate::routes::RangerState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_authz::AuthzError;
use serde_json::{json, Value};

pub fn crud_routes() -> Router<RangerState> {
    Router::new()
        .route("/plugins/definitions", get(list_defs))
        .route("/plugins/definitions/name/{name}", get(get_def))
        .route("/plugins/services", get(list_services).post(create_service))
        .route("/plugins/services/{id}", axum::routing::delete(delete_service))
        .route("/plugins/policies", post(create_policy))
        .route("/plugins/policies/{id}", get(get_policy)
            .put(update_policy).delete(delete_policy))
}

async fn list_defs(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
    let defs: Vec<Value> = s.store.list_service_defs().await?.into_iter().map(|d| d.def_jsonb).collect();
    Ok(Json(json!({ "serviceDefs": defs })))
}
async fn get_def(State(s): State<RangerState>, Path(name): Path<String>) -> Result<Json<Value>, RangerError> {
    Ok(Json(s.store.get_service_def(&name).await?.def_jsonb))
}
async fn list_services(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
    let svcs: Vec<Value> = s.store.list_services().await?.into_iter()
        .map(|r| json!({ "id": r.id, "name": r.name, "type": r.r#type, "configs": r.config_jsonb,
                         "policyVersion": r.policy_version, "isEnabled": true })).collect();
    Ok(Json(json!(svcs)))
}
async fn create_service(State(s): State<RangerState>, Json(body): Json<Value>) -> Result<Json<Value>, RangerError> {
    let name = body["name"].as_str().ok_or(AuthzError::BadRequest("service.name required".into()))?;
    let typ = body["type"].as_str().ok_or(AuthzError::BadRequest("service.type required".into()))?;
    let configs = body.get("configs").cloned().unwrap_or_else(|| json!({}));
    let row = s.store.create_service(name, typ, &configs).await?;
    Ok(Json(json!({ "id": row.id, "name": row.name, "type": row.r#type,
                    "configs": row.config_jsonb, "policyVersion": row.policy_version, "isEnabled": true })))
}
async fn delete_service(State(s): State<RangerState>, Path(id): Path<i64>) -> Result<axum::http::StatusCode, RangerError> {
    s.store.delete_service(id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
async fn create_policy(State(s): State<RangerState>, Json(body): Json<Value>) -> Result<Json<Value>, RangerError> {
    let service = body["service"].as_str().ok_or(AuthzError::BadRequest("policy.service required".into()))?;
    let name = body["name"].as_str().ok_or(AuthzError::BadRequest("policy.name required".into()))?;
    let svc = s.store.get_service(service).await?;
    let id = s.store.create_policy(svc.id, name, &body).await?;
    let mut out = body.clone();
    out["id"] = json!(id);
    Ok(Json(out))
}
async fn get_policy(State(s): State<RangerState>, Path(id): Path<i64>) -> Result<Json<Value>, RangerError> {
    let row = s.store.get_policy(id).await?;
    let mut out = row.policy_jsonb; out["id"] = json!(row.id);
    Ok(Json(out))
}
async fn update_policy(State(s): State<RangerState>, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>, RangerError> {
    let name = body["name"].as_str().ok_or(AuthzError::BadRequest("policy.name required".into()))?;
    let enabled = body.get("isEnabled").and_then(Value::as_bool).unwrap_or(true);
    s.store.update_policy(id, name, &body, enabled).await?;
    let mut out = body.clone(); out["id"] = json!(id);
    Ok(Json(out))
}
async fn delete_policy(State(s): State<RangerState>, Path(id): Path<i64>) -> Result<axum::http::StatusCode, RangerError> {
    s.store.delete_policy(id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
```
(Do NOT import `ServicePolicies` in `crud.rs` — it is only used by `download.rs`. Keep `crud.rs` imports to what it actually uses: `Path`, `State`, `get`, `post`, `Json`, `Router`, `AuthzError`, `json`, `Value`.)

- [ ] **Step 2: Build + clippy** — `cargo build -p egret-ranger`; `cargo clippy -p egret-ranger --all-targets`. Fix warnings.
- [ ] **Step 3: Commit**
```bash
git add crates/egret-ranger
git commit -m "feat(ranger): service-def/service/policy CRUD handlers"
```

---

## Task 8: Config `[ranger]` + server mount `/service` + seed-on-startup

**Files:** Modify `crates/egret-config/src/lib.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-server/Cargo.toml`, `crates/egret-bin/Cargo.toml`.

- [ ] **Step 1: `egret-config`** — add:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangerSettings { pub enabled: bool, pub auth_token: String }
```
Add `pub ranger: RangerSettings,` to `Config`; default `RangerSettings { enabled: false, auth_token: String::new() }`. Extend `validate()`:
```rust
if self.ranger.enabled && self.ranger.auth_token.is_empty() {
    return Err("ranger.enabled requires auth_token".into());
}
```
Add tests `ranger_disabled_by_default_validates` and `ranger_enabled_requires_token` (mirror the catalog ones: construct `Config::default()`, mutate, assert).

- [ ] **Step 2: `egret-server`** — add `pub ranger: Option<egret_ranger::RangerState>,` to `AppState`, `ranger: None` in `without_db()`, and mount:
```rust
    if let Some(ranger) = state.ranger.clone() {
        router = router.merge(Router::new().nest("/service", egret_ranger::ranger_router(ranger)).with_state(()));
    }
```
Add `egret-ranger.workspace = true` to `crates/egret-server/Cargo.toml`.

- [ ] **Step 3: `egret-bin`** — when `config.ranger.enabled`, seed Trino + build state (place near the catalog wiring block):
```rust
    let ranger = if config.ranger.enabled {
        let store = egret_authz::AuthzStore::new(db.clone());
        egret_authz::seed_trino(&store).await?;   // idempotent; AuthzError → anyhow via ?
        Some(egret_ranger::RangerState { store, auth_token: config.ranger.auth_token.clone().into() })
    } else { None };
```
Pass `ranger` into the `AppState { ... }` constructor (add the field). Add `egret-ranger.workspace = true` and `egret-authz.workspace = true` to `crates/egret-bin/Cargo.toml`.

- [ ] **Step 4: Verify** — `cargo build --workspace`; `cargo test -p egret-config`; `cargo test -p egret-server`. Expected: compiles; config + health tests pass.
- [ ] **Step 5: Commit**
```bash
git add crates/egret-config crates/egret-server crates/egret-bin
git commit -m "feat(ranger): config [ranger], mount /service, seed Trino on startup"
```

---

## Task 9: HTTP integration tests (CRUD + download 200/304/404 + version bump)

**Files:** Create `crates/egret-ranger/tests/http_ranger.rs`.

- [ ] **Step 1: DB-gated HTTP test** driving `ranger_router` via `oneshot` (mirror `crates/egret-catalog/tests/http_roundtrip.rs`):
```rust
//! DB-gated Ranger HTTP tests. Self-skip without DATABASE_URL.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_authz::{seed_trino, AuthzStore};
use egret_metastore::Db;
use egret_ranger::{ranger_router, RangerState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "devtoken";

async fn app() -> Option<(axum::Router, String)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = AuthzStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", uuid::Uuid::new_v4().simple());
    let state = RangerState { store, auth_token: Arc::from(TOKEN) };
    Some((ranger_router(state), svc))
}

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token { b = b.header("authorization", format!("Bearer {t}")); }
    let body = match body { Some(v) => Body::from(serde_json::to_vec(&v).unwrap()), None => Body::empty() };
    if matches!(method, "POST"|"PUT") { b = b.header("content-type", "application/json"); }
    b.body(body).unwrap()
}

#[tokio::test]
async fn download_requires_bearer() {
    let Some((app, svc)) = app().await else { eprintln!("skip"); return; };
    let r = app.oneshot(req("GET", &format!("/plugins/policies/download/{svc}"), None, None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_service_policy_then_download_and_304() {
    let Some((app, svc)) = app().await else { eprintln!("skip"); return; };
    // create service
    let r = app.clone().oneshot(req("POST", "/plugins/services", Some(TOKEN),
        Some(json!({"name": svc, "type": "trino", "configs": {}})))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // download (unknown lastKnownVersion) → 200 ServicePolicies with serviceDef
    let r = app.clone().oneshot(req("GET", &format!("/plugins/policies/download/{svc}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["serviceName"], svc.as_str());
    assert_eq!(sp["serviceDef"]["name"], "trino");
    let ver = sp["policyVersion"].as_i64().unwrap();

    // same version → 304
    let r = app.clone().oneshot(req("GET",
        &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);

    // add a policy → version bumps → download is 200 again at old version
    let pol = json!({"service": svc, "name":"p1","policyType":0,
        "resources":{"catalog":{"values":["hive"],"isExcludes":false,"isRecursive":false},
                     "schema":{"values":["default"]},"table":{"values":["t"]},"column":{"values":["*"]}},
        "policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"]}]});
    let r = app.clone().oneshot(req("POST", "/plugins/policies", Some(TOKEN), Some(pol))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app.clone().oneshot(req("GET",
        &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["policies"].as_array().unwrap().len(), 1);
    assert!(sp["policyVersion"].as_i64().unwrap() > ver);
}

#[tokio::test]
async fn download_unknown_service_404() {
    let Some((app, _svc)) = app().await else { eprintln!("skip"); return; };
    let r = app.oneshot(req("GET", "/plugins/policies/download/nope_xyz", Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
```
Add `uuid.workspace = true` to `egret-ranger` `[dev-dependencies]` if needed.

- [ ] **Step 2: Run** — `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-ranger`. Expected: all pass (download 200/304/404, version bump, 401).
- [ ] **Step 3: Commit**
```bash
git add crates/egret-ranger
git commit -m "test(ranger): HTTP integration (CRUD, download 200/304/404, version bump)"
```

---

## Task 10: Real Ranger policy-engine enforcement gate (JVM)

**Files:** Create `tests/ranger/pom.xml`, `tests/ranger/src/test/java/EgretRangerEnforcementTest.java`, `tests/ranger/fixtures/service-policies.json`, `tests/ranger/README.md`, `crates/egret-ranger/tests/ranger_engine_gate.rs`.

This proves the policies Egret serves actually enforce in the **real Ranger policy engine** (the code the Trino plugin runs).

- [ ] **Step 1: A fixture `ServicePolicies` JSON** capturing what Egret's download returns for an access + row-filter + mask scenario. `tests/ranger/fixtures/service-policies.json`: build it by hand (or capture from the Task 9 download) containing the Trino serviceDef (paste from `crates/egret-authz/src/service-defs/trino.json` into the `serviceDef` field) and three policies: an ACCESS policy (`policyType:0`) allowing `select` for user `alice` on `hive.default.t.*`; a DATAMASK policy (`policyType:1`) masking column `email` with `MASK_HASH` for `alice`; a ROWFILTER policy (`policyType:2`) with `filterExpr: "country = 'US'"` for `alice`. (The Java test asserts the engine's decisions on these.)

- [ ] **Step 2: Maven harness** — `tests/ranger/pom.xml`:
```xml
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>egret</groupId><artifactId>ranger-enforcement-test</artifactId><version>1.0</version>
  <properties><maven.compiler.release>11</maven.compiler.release></properties>
  <dependencies>
    <dependency><groupId>org.apache.ranger</groupId><artifactId>ranger-plugins-common</artifactId><version>2.4.0</version></dependency>
    <dependency><groupId>junit</groupId><artifactId>junit</artifactId><version>4.13.2</version><scope>test</scope></dependency>
  </dependencies>
</project>
```
(If `2.4.0` is not the version matching the repo's model, the implementer picks the closest published `ranger-plugins-common` on Maven Central; the API used is stable across 2.x.)

- [ ] **Step 3: The Java test** — `tests/ranger/src/test/java/EgretRangerEnforcementTest.java`:
```java
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.policyengine.*;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Test;
import java.io.FileReader;
import java.util.*;
import static org.junit.Assert.*;

public class EgretRangerEnforcementTest {
    private RangerPolicyEngineImpl engine() throws Exception {
        try (FileReader r = new FileReader("fixtures/service-policies.json")) {
            ServicePolicies sp = JsonUtils.jsonToObject(r, ServicePolicies.class);
            return new RangerPolicyEngineImpl(sp, null, null);
        }
    }
    private RangerAccessRequestImpl req(String user, String access) {
        Map<String,Object> m = new HashMap<>();
        m.put("catalog","hive"); m.put("schema","default"); m.put("table","t"); m.put("column","email");
        RangerAccessRequestImpl r = new RangerAccessRequestImpl();
        r.setResource(new RangerAccessResourceImpl(m));
        r.setUser(user); r.setUserGroups(Set.of()); r.setAccessType(access);
        return r;
    }
    @Test public void aliceSelectAllowed() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("alice","select"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertTrue("alice should be allowed select", res.getIsAllowed());
    }
    @Test public void bobSelectDenied() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("bob","select"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertFalse("bob has no policy → not allowed", res.getIsAllowed());
    }
    @Test public void emailMasked() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("alice","select"), RangerPolicy.POLICY_TYPE_DATAMASK, null);
        assertEquals("MASK_HASH", res.getMaskType());
    }
    @Test public void rowFilterApplied() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("alice","select"), RangerPolicy.POLICY_TYPE_ROWFILTER, null);
        assertEquals("country = 'US'", res.getFilterExpr());
    }
}
```

- [ ] **Step 4: Rust gate** — `crates/egret-ranger/tests/ranger_engine_gate.rs`:
```rust
//! Real Ranger policy-engine enforcement gate. Skips unless EGRET_TEST_RANGER_E2E=1.
//! Requires: a JVM + Maven + network (pulls ranger-plugins-common from Maven Central).
use std::process::Command;

#[test]
fn ranger_engine_enforces_egret_policies() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping Ranger engine gate");
        return;
    }
    let dir = env!("CARGO_MANIFEST_DIR").to_string() + "/../../tests/ranger";
    let status = Command::new("mvn").args(["-q", "test"]).current_dir(&dir).status().expect("run mvn");
    assert!(status.success(), "Ranger enforcement test failed");
}
```

- [ ] **Step 5: Runbook** — `tests/ranger/README.md`:
```text
# Ranger policy-engine enforcement gate
Proves Egret's ServicePolicies enforce in the real Apache Ranger policy engine.

1. Ensure JDK 11+ and Maven are installed.
2. (Optional) regenerate fixtures/service-policies.json from a running Egret:
   curl -s -H "Authorization: Bearer devtoken" \
     "http://localhost:8080/service/plugins/policies/download/trino_prod" > tests/ranger/fixtures/service-policies.json
3. Run the gate:
   EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger ranger_engine_gate
   (or directly: cd tests/ranger && mvn -q test)
Asserts: alice select allowed; bob denied; email → MASK_HASH; row filter "country = 'US'".
```

- [ ] **Step 6: Run the gate (self-skips)** — `cargo test -p egret-ranger ranger_engine_gate`. Expected: prints skip + PASS. With `EGRET_TEST_RANGER_E2E=1` + JDK/Maven, the Java test passes.
- [ ] **Step 7: Commit**
```bash
git add tests/ranger crates/egret-ranger/tests/ranger_engine_gate.rs
git commit -m "test(ranger): real Ranger policy-engine enforcement gate (access/mask/row-filter)"
```

---

## Task 11: Final verification

- [ ] **Step 1: Build + clippy + fmt** — `cargo build --workspace && cargo clippy --workspace --all-targets && cargo fmt --all`.
- [ ] **Step 2: Full test (with DB)** — `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace`. Expected: all pass (Ranger DB tests run; the JVM gate self-skips).
- [ ] **Step 3: Commit any fmt** — `git add -A && git commit -m "chore(ranger): fmt + final Phase 2b verification"` (skip if nothing changed).

---

## Self-review notes (coverage map spec → tasks)

- Spec §3 crates → Task 0 (scaffold), authz Tasks 3–5, ranger Tasks 1/6/7.
- Spec §4 REST surface → Task 6 (download + tags) + Task 7 (definitions/services/policies CRUD).
- Spec §5 native-JSON storage → Task 4 (policy_jsonb verbatim) + Task 7 (accept/return Value).
- Spec §6 download/versioning (304/200, counter bump) → Task 4 (bump) + Task 6 (download) + Task 9 (200/304 test).
- Spec §5/§2 three policy types → carried in the native RangerPolicy JSON; exercised by Task 10 (access/mask/row-filter in the real engine).
- Spec §7 auth/config → Task 6 (require_bearer) + Task 8 ([ranger] + validate + mount + seed).
- Spec §8 data model → Task 2.
- Spec §C seed Trino def → Task 5.
- Spec §9 testing → unit (Tasks 1,5), DB-gated store (Tasks 3,4), HTTP (Task 9), real Ranger engine (Task 10), final (Task 11).

**Known verification points for the implementer:** confirm the exact published `ranger-plugins-common` version on Maven Central and that `JsonUtils.jsonToObject` / `RangerPolicyEngineImpl(sp, null, null)` / `evaluatePolicies(req, POLICY_TYPE_*, null)` / `RangerAccessResult.getMaskType()/getFilterExpr()` match that version (they're stable across Ranger 2.x; the API was extracted from this repo's source). If `RangerPolicyEngineImpl(sp, null, null)` rejects nulls at runtime, pass a `new RangerPluginContext(new RangerPluginConfig("trino", null, "egret", null, null, null))` and `new RangerRoles()` instead — note it in the harness.
