# Phase 1a-1 — AuthN & Session (local) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Local password authentication with server-side sessions, CSRF protection, an `egret init` bootstrap admin, and a protected `/api/auth/me` — the identity foundation for Workspace/RBAC (1a-2) and OIDC (1a-3).

**Architecture:** A new `egret-auth` crate provides argon2 password hashing, opaque server-side sessions (Postgres, two-tier expiry), a `Principal` extractor, and a CSRF middleware. Routes mount into `egret-server`; the `egret` binary gains an `init` command. Per the [Phase 1a design spec](../specs/2026-06-17-phase-1a-identity-design.md).

**Tech Stack:** Rust (stable), axum 0.8, axum-extra (cookie), sqlx 0.8 (Postgres, runtime queries), argon2, rand, sha2, base64, hex, chrono.

**Reference:** [Phase 1a spec](../specs/2026-06-17-phase-1a-identity-design.md) §4, §6, §7, §8, §9, §13, §14.

---

## File Structure

```text
Cargo.toml                                  # add argon2/rand/sha2/base64/hex/axum-extra to [workspace.dependencies]
migrations/0002_identity.sql                # users, groups, user_groups, sessions
migrations/0003_workspace_rbac.sql          # workspaces, members, roles, role_permissions, platform_role_assignments + seed
crates/egret-auth/Cargo.toml
crates/egret-auth/src/lib.rs                # re-exports
crates/egret-auth/src/password.rs           # argon2 hash/verify
crates/egret-auth/src/token.rs              # opaque token generate + sha-256 hash
crates/egret-auth/src/model.rs              # Provider, UserStatus, User, Session
crates/egret-auth/src/users.rs              # UserRepo (Postgres)
crates/egret-auth/src/session.rs            # SessionService (Postgres)
crates/egret-auth/src/cookie.rs             # session/csrf cookie build + read helpers
crates/egret-auth/src/principal.rs          # Principal extractor (FromRequestParts)
crates/egret-auth/src/csrf.rs               # csrf_layer middleware
crates/egret-auth/src/routes.rs             # /api/auth/login|logout|me
crates/egret-auth/tests/session_db.rs       # DB-gated integration tests
crates/egret-server/src/lib.rs              # extend AppState; mount auth router
crates/egret-bin/src/main.rs                # implement `init`
crates/egret-config/src/lib.rs              # add SessionConfig + CsrfConfig
```

**Note:** Internal working plan — English only under `docs/en/plans/` (the bilingual convention applies to design/architecture/ADR docs, not plans).

**Executor note on version-sensitive APIs:** the cookie builder (`axum_extra::extract::cookie`) and a few sqlx call shapes may need minor signature tweaks for the exact crate versions resolved. Treat the provided code as correct-to-intent; if it doesn't compile, adjust the API call (not the behavior) and keep the test assertions unchanged.

---

### Task 1: Dependencies + migrations (identity + RBAC schema, seeded)

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]`)
- Create: `migrations/0002_identity.sql`, `migrations/0003_workspace_rbac.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (migrator count test)

- [ ] **Step 1: Add workspace dependencies**

In `Cargo.toml` under `[workspace.dependencies]`, add:

```toml
argon2 = "0.5"
rand = "0.8"
sha2 = "0.10"
base64 = "0.22"
hex = "0.4"
axum-extra = { version = "0.10", features = ["cookie"] }
async-trait = "0.1"
```

- [ ] **Step 2: Create the identity migration**

Create `migrations/0002_identity.sql`:

```sql
CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider        text NOT NULL CHECK (provider IN ('local','oidc')),
    external_subject text,
    email           citext NOT NULL UNIQUE,
    name            text NOT NULL DEFAULT '',
    password_hash   text,
    status          text NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_login_at   timestamptz
);

CREATE TABLE groups (
    id     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name   text NOT NULL UNIQUE,
    source text NOT NULL DEFAULT 'local' CHECK (source IN ('local','oidc'))
);

CREATE TABLE user_groups (
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id uuid NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
);

CREATE TABLE sessions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash          text NOT NULL UNIQUE,
    user_id             uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at          timestamptz NOT NULL DEFAULT now(),
    absolute_expires_at timestamptz NOT NULL,
    idle_expires_at     timestamptz NOT NULL,
    last_seen_at        timestamptz NOT NULL DEFAULT now(),
    user_agent          text NOT NULL DEFAULT '',
    ip                  text NOT NULL DEFAULT ''
);

CREATE INDEX sessions_user_id_idx ON sessions(user_id);
```

- [ ] **Step 3: Create the workspace/RBAC migration with seed**

Create `migrations/0003_workspace_rbac.sql`:

```sql
CREATE TABLE workspaces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       text NOT NULL UNIQUE,
    name       text NOT NULL,
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    settings   jsonb NOT NULL DEFAULT '{}',
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspace_members (
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    principal_type text NOT NULL CHECK (principal_type IN ('user','group')),
    principal_id   uuid NOT NULL,
    role           text NOT NULL,
    added_by       uuid REFERENCES users(id),
    added_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, principal_type, principal_id)
);

CREATE TABLE roles (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    scope       text NOT NULL CHECK (scope IN ('platform','workspace')),
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    UNIQUE (scope, name)
);

CREATE TABLE role_permissions (
    role_id    uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission text NOT NULL,
    PRIMARY KEY (role_id, permission)
);

CREATE TABLE platform_role_assignments (
    principal_type text NOT NULL CHECK (principal_type IN ('user','group')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (principal_type, principal_id, role_id)
);

-- Seed roles
INSERT INTO roles (scope, name, description) VALUES
    ('platform','platform_admin','Full platform control'),
    ('platform','data_steward','Manage catalog, policy, databases'),
    ('workspace','ws_admin','Manage workspace members and resources'),
    ('workspace','ws_editor','Create and edit workspace content'),
    ('workspace','ws_viewer','View workspace content');

-- Seed permissions (catalog grows in later phases)
INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY[
    'workspace:create','workspace:read','workspace:update','workspace:archive',
    'user:read','user:assign_role'
]) AS perm
WHERE scope='platform' AND name='platform_admin';

INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY['workspace:read','user:read']) AS perm
WHERE scope='platform' AND name='data_steward';

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workspace:manage_members' FROM roles
WHERE scope='workspace' AND name='ws_admin';
```

- [ ] **Step 4: Update the migrator count test**

In `crates/egret-metastore/src/lib.rs`, the existing test asserts the migrator is non-empty. Replace its body to assert the count is at least 3:

```rust
    #[test]
    fn migrator_embeds_baseline_migration() {
        assert!(
            MIGRATOR.migrations.len() >= 3,
            "expected baseline + identity + rbac migrations to be embedded"
        );
    }
```

- [ ] **Step 5: Run the metastore test**

Run: `cargo test -p egret-metastore --lib`
Expected: PASS (migration files are embedded at compile time; no DB needed).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml migrations crates/egret-metastore/src/lib.rs
git commit -m "feat(metastore): identity + workspace/rbac migrations with seed"
```

---

### Task 2: `egret-auth` scaffold + argon2 password hashing

**Files:**
- Create: `crates/egret-auth/Cargo.toml`, `crates/egret-auth/src/lib.rs`, `crates/egret-auth/src/password.rs`
- Modify: `Cargo.toml` (`[workspace.dependencies]` add `egret-auth`)

- [ ] **Step 1: Register the crate in the workspace dependency table**

In `Cargo.toml` `[workspace.dependencies]`, add:

```toml
egret-auth = { path = "crates/egret-auth" }
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/egret-auth/Cargo.toml`:

```toml
[package]
name = "egret-auth"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
egret-config.workspace = true
axum.workspace = true
axum-extra.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
argon2.workspace = true
rand.workspace = true
sha2.workspace = true
base64.workspace = true
hex.workspace = true
async-trait.workspace = true
anyhow.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
tower.workspace = true
http-body-util = "0.1"
```

- [ ] **Step 3: Write the failing password test**

Create `crates/egret-auth/src/password.rs`:

```rust
//! argon2id password hashing.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use egret_core::{Error, Result};

/// Hash a plaintext password with argon2id (PHC string output).
pub fn hash_password(plain: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| Error::Internal(anyhow::anyhow!("argon2 hash failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash. Returns Ok(false) on mismatch.
pub fn verify_password(plain: &str, phc_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(phc_hash)
        .map_err(|e| Error::Internal(anyhow::anyhow!("invalid password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_argon2id_and_verifies() {
        let h = hash_password("s3cret!").unwrap();
        assert!(h.starts_with("$argon2id$"), "expected argon2id PHC, got {h}");
        assert!(verify_password("s3cret!", &h).unwrap());
    }

    #[test]
    fn wrong_password_does_not_verify() {
        let h = hash_password("s3cret!").unwrap();
        assert!(!verify_password("wrong", &h).unwrap());
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        assert_ne!(hash_password("x").unwrap(), hash_password("x").unwrap());
    }
}
```

Create `crates/egret-auth/src/lib.rs`:

```rust
//! `egret-auth` — authentication (local), sessions, and the request `Principal`.

pub mod password;
```

- [ ] **Step 4: Run the password tests**

Run: `cargo test -p egret-auth`
Expected: PASS — `hash_is_argon2id_and_verifies`, `wrong_password_does_not_verify`, `two_hashes_of_same_password_differ`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/egret-auth
git commit -m "feat(auth): scaffold egret-auth with argon2id password hashing"
```

---

### Task 3: Opaque session tokens + domain types

**Files:**
- Create: `crates/egret-auth/src/token.rs`, `crates/egret-auth/src/model.rs`
- Modify: `crates/egret-auth/src/lib.rs`

- [ ] **Step 1: Write the token module with tests**

Create `crates/egret-auth/src/token.rs`:

```rust
//! Opaque session tokens. The raw token lives only in the cookie; the DB stores its hash.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a fresh 256-bit url-safe token.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Hash a token for storage/lookup (sha-256, hex).
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn hash_is_deterministic_and_hex() {
        let t = generate_token();
        assert_eq!(hash_token(&t), hash_token(&t));
        assert_eq!(hash_token(&t).len(), 64); // sha-256 hex
    }
}
```

- [ ] **Step 2: Write the domain model**

Create `crates/egret-auth/src/model.rs`:

```rust
//! Auth domain types.

use chrono::{DateTime, Utc};
use egret_core::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Local,
    Oidc,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Oidc => "oidc",
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: UserId,
    pub provider: String, // 'local' | 'oidc'
    pub email: String,
    pub name: String,
    pub password_hash: Option<String>,
    pub status: String, // 'active' | 'disabled'
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: uuid::Uuid,
    pub user_id: UserId,
    pub absolute_expires_at: DateTime<Utc>,
    pub idle_expires_at: DateTime<Utc>,
}
```

- [ ] **Step 3: Export the modules**

Replace `crates/egret-auth/src/lib.rs` with:

```rust
//! `egret-auth` — authentication (local), sessions, and the request `Principal`.

pub mod model;
pub mod password;
pub mod token;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p egret-auth`
Expected: PASS — token + password tests.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth
git commit -m "feat(auth): session token utilities and domain types"
```

---

### Task 4: `UserRepo` (Postgres)

**Files:**
- Create: `crates/egret-auth/src/users.rs`
- Modify: `crates/egret-auth/src/lib.rs`
- Create: `crates/egret-auth/tests/session_db.rs` (shared DB-gated harness; user tests first)

- [ ] **Step 1: Write the user repository**

Create `crates/egret-auth/src/users.rs`:

```rust
//! Postgres-backed user lookup and creation.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::User;

#[derive(Clone)]
pub struct UserRepo {
    db: Db,
}

impl UserRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn find_by_id(&self, id: UserId) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    /// Insert a local user with a password hash; returns the new id.
    pub async fn insert_local(&self, email: &str, name: &str, password_hash: &str) -> Result<UserId> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, password_hash) \
             VALUES ('local', $1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(UserId(id))
    }

    pub async fn touch_last_login(&self, id: UserId) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    provider: String,
    email: String,
    name: String,
    password_hash: Option<String>,
    status: String,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: UserId(r.id),
            provider: r.provider,
            email: r.email,
            name: r.name,
            password_hash: r.password_hash,
            status: r.status,
        }
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
```

- [ ] **Step 2: Export the module**

Add to `crates/egret-auth/src/lib.rs`:

```rust
pub mod users;
```

(so the file lists `model`, `password`, `token`, `users`).

- [ ] **Step 3: Write the DB-gated integration test**

Create `crates/egret-auth/tests/session_db.rs`:

```rust
//! DB-gated integration tests. Skip (pass) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@test.local", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn insert_and_find_user() {
    let Some(db) = connect().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let repo = UserRepo::new(db);
    let email = unique_email("user");
    let hash = hash_password("pw").unwrap();

    let id = repo.insert_local(&email, "Tester", &hash).await.unwrap();
    let found = repo.find_by_email(&email).await.unwrap().expect("user exists");
    assert_eq!(found.id, id);
    assert_eq!(found.provider, "local");
    assert!(found.password_hash.is_some());

    let by_id = repo.find_by_id(id).await.unwrap().expect("by id");
    assert_eq!(by_id.email.to_lowercase(), email.to_lowercase());
}
```

- [ ] **Step 4: Run the tests (offline self-skip, then with DB)**

Run offline: `cargo test -p egret-auth`
Expected: PASS — unit tests pass; `insert_and_find_user` prints the skip message and returns.

Run with DB (start Postgres via `docker compose -f deploy/docker-compose.dev.yml up -d postgres`, wait healthy):
`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test session_db`
Expected: PASS — connects, migrates, inserts, finds.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/users.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/session_db.rs
git commit -m "feat(auth): UserRepo with DB-gated integration test"
```

---

### Task 5: `SessionService` (create / validate+slide / revoke)

**Files:**
- Create: `crates/egret-auth/src/session.rs`
- Modify: `crates/egret-auth/src/lib.rs`, `crates/egret-auth/tests/session_db.rs`

- [ ] **Step 1: Write the session service**

Create `crates/egret-auth/src/session.rs`:

```rust
//! Server-side sessions with two-tier expiry (absolute cap + sliding idle window).

use chrono::{DateTime, Duration, Utc};
use egret_core::{Principal, Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::token::{generate_token, hash_token};

#[derive(Clone)]
pub struct SessionService {
    db: Db,
    absolute_ttl: Duration,
    idle_ttl: Duration,
}

/// What `create` returns: the raw token (for the cookie) plus expiry timestamps.
pub struct IssuedSession {
    pub token: String,
    pub absolute_expires_at: DateTime<Utc>,
    pub idle_expires_at: DateTime<Utc>,
}

impl SessionService {
    pub fn new(db: Db, absolute_ttl: Duration, idle_ttl: Duration) -> Self {
        Self { db, absolute_ttl, idle_ttl }
    }

    /// Create a fresh session for a user. Caller sets the cookie from the returned token.
    pub async fn create(&self, user_id: UserId, user_agent: &str, ip: &str) -> Result<IssuedSession> {
        let now = Utc::now();
        let absolute = now + self.absolute_ttl;
        let idle = now + self.idle_ttl;
        let token = generate_token();
        let token_hash = hash_token(&token);

        sqlx::query(
            "INSERT INTO sessions \
             (token_hash, user_id, absolute_expires_at, idle_expires_at, user_agent, ip) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&token_hash)
        .bind(user_id.0)
        .bind(absolute)
        .bind(idle)
        .bind(user_agent)
        .bind(ip)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;

        Ok(IssuedSession { token, absolute_expires_at: absolute, idle_expires_at: idle })
    }

    /// Validate a raw token. Returns the `Principal` and slides the idle window, or
    /// `Ok(None)` if missing/expired. Platform roles are loaded for the principal.
    pub async fn validate(&self, token: &str) -> Result<Option<Principal>> {
        let token_hash = hash_token(token);
        let now = Utc::now();

        let row = sqlx::query_as::<_, ValidateRow>(
            "SELECT s.user_id, u.email, u.status \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.absolute_expires_at > $2 AND s.idle_expires_at > $2",
        )
        .bind(&token_hash)
        .bind(now)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;

        let Some(row) = row else { return Ok(None) };
        if row.status != "active" {
            return Ok(None);
        }

        // Slide the idle window.
        let new_idle = now + self.idle_ttl;
        sqlx::query("UPDATE sessions SET idle_expires_at = $1, last_seen_at = $2 WHERE token_hash = $3")
            .bind(new_idle)
            .bind(now)
            .bind(&token_hash)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;

        let platform_roles = self.platform_roles(UserId(row.user_id)).await?;
        Ok(Some(Principal {
            user_id: UserId(row.user_id),
            email: row.email,
            platform_roles,
        }))
    }

    pub async fn revoke(&self, token: &str) -> Result<()> {
        let token_hash = hash_token(token);
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn revoke_all_for_user(&self, user_id: UserId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Platform roles assigned directly to the user (group roles arrive in 1a-2).
    async fn platform_roles(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT r.name FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE pra.principal_type = 'user' AND pra.principal_id = $1",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }
}

#[derive(sqlx::FromRow)]
struct ValidateRow {
    user_id: Uuid,
    email: String,
    status: String,
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
```

> `Principal` (in `egret-core`) already has fields `user_id: UserId`, `email: String`, `platform_roles: Vec<String>` (defined in Phase 0a). No change needed.

- [ ] **Step 2: Export the module**

Add `pub mod session;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 3: Add session integration tests**

Append to `crates/egret-auth/tests/session_db.rs`:

```rust
use chrono::Duration;
use egret_auth::session::SessionService;
use egret_core::UserId;

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&unique_email("sess"), "S", &hash).await.unwrap()
}

#[tokio::test]
async fn create_then_validate_slides_idle() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));

    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().expect("valid");
    assert_eq!(principal.user_id, uid);
}

#[tokio::test]
async fn expired_session_does_not_validate() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let uid = seed_user(&db).await;
    // idle_ttl in the past → immediately expired.
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(-1));
    let issued = svc.create(uid, "agent", "127.0.0.1").await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}

#[tokio::test]
async fn revoke_invalidates() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let uid = seed_user(&db).await;
    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "127.0.0.1").await.unwrap();
    svc.revoke(&issued.token).await.unwrap();
    assert!(svc.validate(&issued.token).await.unwrap().is_none());
}
```

- [ ] **Step 4: Run offline then with DB**

Offline: `cargo test -p egret-auth` → PASS (DB tests skip).
With DB: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test session_db`
Expected: PASS — create/validate, expiry, revoke.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/session.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/session_db.rs
git commit -m "feat(auth): SessionService with two-tier expiry and revocation"
```

---

### Task 6: Cookie + CSRF helpers

**Files:**
- Create: `crates/egret-auth/src/cookie.rs`
- Modify: `crates/egret-auth/src/lib.rs`

- [ ] **Step 1: Write the cookie helpers with tests**

Create `crates/egret-auth/src/cookie.rs`:

```rust
//! Session + CSRF cookie construction and the CSRF token generator.

use axum_extra::extract::cookie::{Cookie, SameSite};

pub const SESSION_COOKIE: &str = "egret_session";
pub const CSRF_COOKIE: &str = "egret_csrf";
pub const CSRF_HEADER: &str = "x-egret-csrf";

/// Build the session cookie (HttpOnly).
pub fn session_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(axum_extra::extract::cookie::time::Duration::seconds(max_age_secs))
        .build()
}

/// Build the CSRF cookie (NOT HttpOnly — the SPA reads it to echo in a header).
pub fn csrf_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, value))
        .http_only(false)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(axum_extra::extract::cookie::time::Duration::seconds(max_age_secs))
        .build()
}

/// Build an expired cookie to clear one on logout.
pub fn clear_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .max_age(axum_extra::extract::cookie::time::Duration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_is_http_only_lax() {
        let c = session_cookie("tok".into(), true, 3600);
        assert_eq!(c.name(), SESSION_COOKIE);
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
        assert_eq!(c.secure(), Some(true));
    }

    #[test]
    fn csrf_cookie_is_readable_by_js() {
        let c = csrf_cookie("csrf".into(), false, 3600);
        assert_eq!(c.http_only(), Some(false));
    }
}
```

> **Executor note:** `axum_extra::extract::cookie::time` re-exports the `time` crate used by `cookie`. If that path doesn't resolve in the resolved version, add `time = "0.3"` to dev/deps and use `time::Duration::seconds(...)`. Keep the assertions unchanged.

- [ ] **Step 2: Export the module**

Add `pub mod cookie;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p egret-auth --lib`
Expected: PASS — cookie attribute tests.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/cookie.rs crates/egret-auth/src/lib.rs
git commit -m "feat(auth): session and CSRF cookie helpers"
```

---

### Task 7: `Principal` extractor + `csrf_layer` middleware

**Files:**
- Create: `crates/egret-auth/src/principal.rs`, `crates/egret-auth/src/csrf.rs`
- Modify: `crates/egret-auth/src/lib.rs`

- [ ] **Step 1: Write the Principal extractor**

Create `crates/egret-auth/src/principal.rs`:

```rust
//! Axum extractor that authenticates a request from the session cookie.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::CookieJar;
use egret_core::{Error, Principal};

use crate::cookie::SESSION_COOKIE;
use crate::session::SessionService;

/// Wrapper so we can implement `FromRequestParts` for a foreign type (`Principal`).
pub struct Auth(pub Principal);

impl<S> FromRequestParts<S> for Auth
where
    SessionService: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
        let token = token.ok_or(Error::Unauthorized)?;

        let svc = SessionService::from_ref(state);
        match svc.validate(&token).await? {
            Some(principal) => Ok(Auth(principal)),
            None => Err(Error::Unauthorized),
        }
    }
}
```

> Requires `Error` to convert into an HTTP response. Add an `IntoResponse` impl for `egret_core::Error` (Step 2) — this is the canonical mapping the spec §13 calls for, placed in `egret-auth` to avoid a web dependency in `egret-core`. Define a newtype if orphan rules block a direct impl.

- [ ] **Step 2: Add the error→response mapping**

Add to `crates/egret-auth/src/principal.rs` (same file, since the extractor's rejection needs it):

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Map `egret_core::Error` to an HTTP response. Orphan rules: wrap in a local newtype
/// used by handlers, plus a direct impl is allowed here because `Error` is from a
/// workspace crate we own.
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, msg) = match self.0 {
            Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found"),
            Error::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid input"),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Error::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// Newtype to satisfy orphan rules for `IntoResponse`.
pub struct AuthError(pub Error);
impl From<Error> for AuthError {
    fn from(e: Error) -> Self {
        AuthError(e)
    }
}
```

Then change the extractor's `type Rejection = Error;` to `type Rejection = AuthError;` and map the error returns:

```rust
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_string())
            .ok_or(AuthError(Error::Unauthorized))?;
        let svc = SessionService::from_ref(state);
        match svc.validate(&token).await.map_err(AuthError)? {
            Some(principal) => Ok(Auth(principal)),
            None => Err(AuthError(Error::Unauthorized)),
        }
    }
```

- [ ] **Step 3: Write the CSRF middleware**

Create `crates/egret-auth/src/csrf.rs`:

```rust
//! Double-submit CSRF: mutating requests must echo the CSRF cookie in a header.

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::cookie::{CSRF_COOKIE, CSRF_HEADER};

/// Reject mutating requests whose `X-Egret-CSRF` header is missing or ≠ the CSRF cookie.
/// Safe methods pass through. Pre-auth POSTs (login/oidc) are mounted on a router without
/// this layer, so they are not affected.
pub async fn csrf_layer(jar: CookieJar, request: Request, next: Next) -> Response {
    let method = request.method();
    let is_mutating = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if !is_mutating {
        return next.run(request).await;
    }

    let cookie_val = jar.get(CSRF_COOKIE).map(|c| c.value().to_string());
    let header_val = request
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (cookie_val, header_val) {
        (Some(c), Some(h)) if c == h && !c.is_empty() => next.run(request).await,
        _ => (StatusCode::FORBIDDEN, "csrf check failed").into_response(),
    }
}
```

- [ ] **Step 4: Export modules + write middleware test**

Add to `crates/egret-auth/src/lib.rs`:

```rust
pub mod csrf;
pub mod principal;
```

Add tests to `crates/egret-auth/src/csrf.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/m", post(|| async { "ok" }))
            .layer(axum::middleware::from_fn(csrf_layer))
    }

    #[tokio::test]
    async fn rejects_post_without_matching_header() {
        let resp = app()
            .oneshot(HttpRequest::builder().method("POST").uri("/m").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_post_with_matching_cookie_and_header() {
        let resp = app()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/m")
                    .header("cookie", "egret_csrf=abc")
                    .header("x-egret-csrf", "abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn allows_get_without_csrf() {
        let app = Router::new()
            .route("/g", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(csrf_layer));
        let resp = app
            .oneshot(HttpRequest::builder().uri("/g").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p egret-auth`
Expected: PASS — CSRF middleware tests + earlier unit tests (DB tests skip offline).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/principal.rs crates/egret-auth/src/csrf.rs crates/egret-auth/src/lib.rs
git commit -m "feat(auth): Principal extractor, error mapping, and CSRF middleware"
```

---

### Task 8: Local auth routes + wire into `egret-server`

**Files:**
- Create: `crates/egret-auth/src/routes.rs`
- Modify: `crates/egret-auth/src/lib.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-server/Cargo.toml`

- [ ] **Step 1: Write the auth routes**

Create `crates/egret-auth/src/routes.rs`:

```rust
//! /api/auth routes: login (local), logout, me.

use axum::extract::{FromRef, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use egret_config::Config;
use egret_core::Error;
use serde::{Deserialize, Serialize};

use crate::cookie::{clear_cookie, csrf_cookie, session_cookie, CSRF_COOKIE, SESSION_COOKIE};
use crate::password::verify_password;
use crate::principal::{Auth, AuthError};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub email: String,
    pub platform_roles: Vec<String>,
}

pub fn auth_router() -> Router<AuthState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
}

async fn login(
    State(st): State<AuthState>,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AuthError> {
    // Uniform error for unknown user vs bad password (no enumeration).
    let user = st.users.find_by_email(&body.email).await.map_err(AuthError)?;
    let user = user.ok_or(AuthError(Error::Unauthorized))?;
    if user.status != "active" {
        return Err(AuthError(Error::Unauthorized));
    }
    let hash = user.password_hash.as_deref().ok_or(AuthError(Error::Unauthorized))?;
    if !verify_password(&body.password, hash).map_err(AuthError)? {
        return Err(AuthError(Error::Unauthorized));
    }

    let issued = st
        .sessions
        .create(user.id, "", "")
        .await
        .map_err(AuthError)?;
    st.users.touch_last_login(user.id).await.map_err(AuthError)?;

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    let jar = jar
        .add(session_cookie(issued.token, secure, max_age))
        .add(csrf_cookie(csrf, secure, max_age));

    Ok((jar, Json(MeResponse {
        user_id: user.id.to_string(),
        email: user.email,
        platform_roles: vec![],
    })))
}

async fn logout(
    State(st): State<AuthState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AuthError> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        st.sessions.revoke(c.value()).await.map_err(AuthError)?;
    }
    let jar = jar.add(clear_cookie(SESSION_COOKIE)).add(clear_cookie(CSRF_COOKIE));
    Ok((jar, Json(serde_json::json!({ "ok": true }))))
}

async fn me(Auth(principal): Auth) -> impl IntoResponse {
    Json(MeResponse {
        user_id: principal.user_id.to_string(),
        email: principal.email,
        platform_roles: principal.platform_roles,
    })
}

// Let the Principal extractor pull SessionService from AuthState.
impl FromRef<AuthState> for SessionService {
    fn from_ref(st: &AuthState) -> SessionService {
        st.sessions.clone()
    }
}
```

- [ ] **Step 2: Export + add server dep**

Add `pub mod routes;` to `crates/egret-auth/src/lib.rs`.

In `crates/egret-server/Cargo.toml` `[dependencies]`, add:

```toml
egret-auth.workspace = true
egret-config.workspace = true
```

- [ ] **Step 3: Extend `AppState` and mount the auth router**

In `crates/egret-server/src/lib.rs`, extend `AppState` to carry the auth state and mount the router. Replace the `AppState` struct and `build_router` with:

```rust
use egret_auth::routes::{auth_router, AuthState};

#[derive(Clone)]
pub struct AppState {
    pub db: Option<Db>,
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub auth: Option<AuthState>,
}

impl AppState {
    pub fn without_db() -> Self {
        Self { db: None, auth: None }
    }
}

pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz));

    if let Some(auth) = state.auth.clone() {
        // CSRF applies to the authed API surface; login is mounted under it but the
        // login route is exempt because the SPA has no session/CSRF cookie yet —
        // it is reached via GET of the CSRF token first OR is allowed pre-auth.
        let api = Router::new()
            .nest("/api/auth", auth_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(auth);
        router = router.merge(api);
    }

    router.with_state(state)
}
```

> **Executor note:** `login` is a POST and would be blocked by `csrf_layer` (no cookie yet). Resolve by mounting `/api/auth/login` and `/api/auth/oidc/*` on a **separate sub-router without** the csrf layer, and apply `csrf_layer` only to the authenticated routes (`logout`, and 1a-2 admin routes). Concretely: split `auth_router` into `public_auth_router()` (login) and `protected_auth_router()` (logout, me); layer CSRF only on the protected one. Update the test below accordingly.

- [ ] **Step 4: Fix the existing health tests**

The health tests use `AppState::without_db()`, which still compiles (now sets `auth: None`). Confirm they pass unchanged:

Run: `cargo test -p egret-server`
Expected: PASS — `healthz_returns_200_ok`, `readyz_ok_without_db`.

- [ ] **Step 5: Add a DB-gated login/me integration test**

Append to `crates/egret-auth/tests/session_db.rs`:

```rust
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::routes::{auth_router, AuthState};
use egret_config::Config;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn login_sets_cookies_and_me_returns_user() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let repo = UserRepo::new(db.clone());
    let email = unique_email("login");
    repo.insert_local(&email, "L", &hash_password("pw").unwrap()).await.unwrap();

    let st = AuthState {
        users: repo,
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
    };
    let app = axum::Router::new().nest("/api/auth", auth_router()).with_state(st);

    // Login.
    let body = serde_json::to_vec(&serde_json::json!({"email": email, "password": "pw"})).unwrap();
    let resp = app.clone().oneshot(
        HttpRequest::builder().method("POST").uri("/api/auth/login")
            .header("content-type","application/json").body(Body::from(body)).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp.headers().get_all("set-cookie").iter().count();
    assert!(set_cookie >= 2, "expects session + csrf cookies");
    let cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    let session_kv = cookie.split(';').next().unwrap().to_string(); // egret_session=...

    // /me with the session cookie.
    let resp = app.oneshot(
        HttpRequest::builder().uri("/api/auth/me").header("cookie", session_kv)
            .body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["email"].as_str().unwrap().to_lowercase(), email.to_lowercase());
}

#[tokio::test]
async fn me_without_cookie_is_401() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
    };
    let app = axum::Router::new().nest("/api/auth", auth_router()).with_state(st);
    let resp = app.oneshot(
        HttpRequest::builder().uri("/api/auth/me").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 6: Run offline then with DB**

Offline: `cargo test --workspace` → PASS (DB tests skip; server/auth unit tests pass).
With DB: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test session_db`
Expected: PASS — login sets 2 cookies, `/me` returns the user, no-cookie `/me` is 401.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-auth crates/egret-server
git commit -m "feat(auth): local login/logout/me routes wired into egret-server"
```

---

### Task 9: `egret init` bootstrap + config + final verification

**Files:**
- Modify: `crates/egret-config/src/lib.rs` (SessionConfig, CsrfConfig)
- Modify: `crates/egret-bin/src/main.rs` (implement `init`; build AuthState in `serve`)
- Modify: `crates/egret-bin/Cargo.toml`

- [ ] **Step 1: Add session + csrf config**

In `crates/egret-config/src/lib.rs`, add two config structs and fields to `Config`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub absolute_ttl_hours: u32,
    pub idle_ttl_minutes: u32,
    pub secure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfConfig {
    pub cookie_name: String,
    pub header_name: String,
}
```

Add `pub session: SessionConfig,` and `pub csrf: CsrfConfig,` to `struct Config`, and to its `Default`:

```rust
            session: SessionConfig {
                cookie_name: "egret_session".into(),
                absolute_ttl_hours: 24,
                idle_ttl_minutes: 60,
                secure: false,
            },
            csrf: CsrfConfig {
                cookie_name: "egret_csrf".into(),
                header_name: "X-Egret-CSRF".into(),
            },
```

Update the existing config tests if they construct `Config` literally (they use `Config::figment().extract()` / defaults, so they keep working). Run `cargo test -p egret-config` → PASS.

- [ ] **Step 2: Add a config test for the new defaults**

Append to the `tests` module in `crates/egret-config/src/lib.rs`:

```rust
    #[test]
    fn session_and_csrf_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.session.absolute_ttl_hours, 24);
            assert_eq!(cfg.session.idle_ttl_minutes, 60);
            assert_eq!(cfg.csrf.header_name, "X-Egret-CSRF");
            Ok(())
        });
    }
```

Run: `cargo test -p egret-config` → PASS.

- [ ] **Step 3: Implement `egret init` and wire `serve`**

In `crates/egret-bin/Cargo.toml` `[dependencies]`, add `egret-auth.workspace = true`.

In `crates/egret-bin/src/main.rs`, replace the `Init` arm and `serve` wiring:

```rust
use egret_auth::password::hash_password;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use chrono::Duration;
```

Replace the `Command::Init` arm in `main`:

```rust
        Command::Init => init(config).await,
```

Add the `init` function:

```rust
/// Bootstrap the first platform admin (local provider). Idempotent: refuses if a
/// platform_admin already exists. Prints a generated password when none is supplied.
async fn init(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    // Refuse if a platform_admin assignment already exists.
    let existing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM platform_role_assignments pra \
         JOIN roles r ON r.id = pra.role_id WHERE r.scope='platform' AND r.name='platform_admin'",
    )
    .fetch_one(&db.pool)
    .await?;
    if existing > 0 {
        tracing::warn!("a platform_admin already exists; nothing to do");
        return Ok(());
    }

    let email = std::env::var("EGRET_ADMIN_EMAIL").unwrap_or_else(|_| "admin@egret.local".into());
    let password =
        std::env::var("EGRET_ADMIN_PASSWORD").unwrap_or_else(|_| egret_auth::token::generate_token());
    let hash = hash_password(&password)?;

    let users = UserRepo::new(db.clone());
    let user_id = users.insert_local(&email, "Administrator", &hash).await?;

    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         SELECT 'user', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(user_id.0)
    .execute(&db.pool)
    .await?;

    tracing::info!(%email, "bootstrap admin created");
    println!("Bootstrap admin: {email}");
    println!("Password: {password}");
    Ok(())
}
```

Update `serve` to build `AuthState` and pass it in `AppState`:

```rust
async fn serve(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations on boot")?;

    let auth = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            Duration::hours(config.session.absolute_ttl_hours as i64),
            Duration::minutes(config.session.idle_ttl_minutes as i64),
        ),
        config: config.clone(),
    };

    let app = build_router(AppState { db: Some(db), auth: Some(auth) });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}
```

- [ ] **Step 4: Verify the whole workspace builds and tests pass offline**

Run:
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Expected: build OK; all tests pass (DB-gated ones skip); clippy clean; fmt clean.

- [ ] **Step 5: End-to-end smoke test with a database**

Run:
```bash
docker compose -f deploy/docker-compose.dev.yml up -d postgres   # wait healthy
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
EGRET_ADMIN_EMAIL=admin@egret.local EGRET_ADMIN_PASSWORD=devpass \
  cargo run -p egret-bin -- init
# Expect: "Bootstrap admin: admin@egret.local"
cargo run -p egret-bin -- serve &   # in background
sleep 2
# Login, capture cookie:
curl -s -i -X POST localhost:8080/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"admin@egret.local","password":"devpass"}' | tee /tmp/login.txt | grep -i set-cookie
# Expect: Set-Cookie: egret_session=...  and  egret_csrf=...
kill %1
docker compose -f deploy/docker-compose.dev.yml down
```
Expected: `init` prints the admin; login returns 200 with `egret_session` + `egret_csrf` cookies.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-config crates/egret-bin
git commit -m "feat(bin): egret init bootstrap admin and serve wiring; session/csrf config"
```

---

## Self-Review

**Spec coverage (Phase 1a spec, local-auth portion):**
- §4 `egret-auth` (local authN, sessions, cookies, `Principal`) → Tasks 2–8. ✅
- §6 migrations (users/groups/user_groups/sessions; workspaces/roles/... schema + seed) → Task 1. ✅ (workspace/RBAC *logic* is 1a-2; schema seeded here so `init` can assign the admin role.)
- §7 local login / logout / me / `egret init` bootstrap → Tasks 8, 9. ✅ (OIDC flow → 1a-3.)
- §8 sessions: server-side, `token_hash`, two-tier expiry, rotation (fresh token on login), revoke → Task 5. ✅
- §9 CSRF double-submit → Tasks 6, 7. ✅
- §12 config additions (session, csrf) → Task 9. ✅ (oidc/idp_role_mapping config → 1a-3.)
- §13 error→HTTP mapping, no-enumeration uniform login error → Tasks 7, 8. ✅
- §14 unit + DB-gated integration, offline-green → every task. ✅
- §10/§11 RBAC + workspace-context middleware → **deferred to 1a-2** (stated in plan intro). ⚠️ intentional.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N". Two explicit executor notes (cookie `time` path; splitting login onto a CSRF-exempt sub-router) describe *concrete* required adjustments with the exact fix, not vague gaps. Rate-limiting (§13) is **not** implemented in 1a-1 — noted here as a deliberate deferral to 1a-2's hardening (avoid scope creep; uniform error + argon2 cost already blunt online guessing).

**Type consistency:**
- `SessionService::new(db, absolute_ttl, idle_ttl)` / `.create(user_id, ua, ip)` / `.validate(token)` — consistent across Tasks 5, 8, 9. ✅
- `UserRepo::new(db)` / `find_by_email` / `find_by_id` / `insert_local(email,name,hash)` / `touch_last_login` — consistent Tasks 4, 8, 9. ✅
- `AuthState { users, sessions, config }` + `FromRef<AuthState> for SessionService` — consistent Tasks 8, 9. ✅
- `Auth(Principal)` extractor + `AuthError(Error)` — consistent Tasks 7, 8. ✅
- Cookie names `egret_session`/`egret_csrf`/header `x-egret-csrf` — consistent Tasks 6, 7, 8. ✅
- `Principal { user_id, email, platform_roles }` matches the Phase 0a `egret-core` definition. ✅

**Open follow-ups (next plans):** 1a-2 Workspace & RBAC (Authorizer, admin routes, `workspace_context_layer`, group-role resolution, login rate-limiting); 1a-3 OIDC/SSO.

---

## Execution record (applied during implementation)

Implemented via subagent-driven TDD; independently reviewed (spec + security + code quality) — **APPROVED**, 35 offline tests + 6 DB-integration tests green, clippy/fmt clean.

**Deviations from the plan text (all accepted, applied correctly):**
- `time = "0.3"` added to `egret-auth` deps; cookie builders use `time::Duration` (axum-extra doesn't re-export `time`).
- `use axum::extract::FromRef` imported in `principal.rs`.
- `SessionConfig`/`CsrfConfig` + their config test landed in Task 8 (not 9) because `routes.rs` consumes `config.session.*`.
- CSRF scoped via an inner protected sub-router (`login` exempt; `logout`+`me` behind `csrf_layer`); no outer csrf layer on `/api/auth` — resolves the Task 8 executor note.
- `egret-bin` gained `chrono` + `sqlx` deps for `init`.
- `sessions.ip` is `text` (not `inet`) to avoid the sqlx `ipnetwork` feature — matches this plan's migration; spec §6 updated to `text`.

**Minor items deferred to Phase 1a-2 (hardening):**
- Login user-enumeration timing side-channel → add a dummy argon2 verify + login rate-limiting.
- `egret init --force` flag (currently idempotent refuse-only).
- Dead-code domain types (`model::Provider`, `model::Session`) — wire in or drop when 1a-2/1a-3 need them.
