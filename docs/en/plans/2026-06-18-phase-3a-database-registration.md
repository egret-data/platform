# Phase 3a — Database Registration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register query engines (Trino + StarRocks) Egret runs against — store connection config, encrypt credentials (AES-256-GCM), grant databases to workspaces, and verify connectivity on demand with a real `SELECT 1`.

**Architecture:** Two new crates. `egret-secrets` (a `Crypto` that seals/opens with AES-256-GCM + a Postgres `SecretStore`). `egret-database` (an `Engine` enum, `DatabaseRepo` CRUD that seals creds through the `SecretStore`, workspace grants, a per-engine connection `tester`, and admin/me axum routers). Both follow the existing `egret-catalog` conventions: runtime-sqlx (no `query!` macros), `Option<_>` sub-state mounted conditionally in `AppState`, `thiserror` → `IntoResponse`, and DB-gated / engine-gated self-skipping tests.

**Tech Stack:** Rust 2021 (MSRV 1.85), axum 0.8, sqlx 0.8 (postgres, runtime queries), `aes-gcm` 0.10, `rand` 0.8, `reqwest` 0.12 (rustls — Trino statement protocol), `mysql_async` 0.34 (rustls — StarRocks), `tokio`.

---

## Conventions every task MUST follow

These are load-bearing — violating them fails review:

1. **Runtime sqlx only.** Use `sqlx::query_as::<_, Row>(...)`, `sqlx::query_scalar::<_, T>(...)`, `sqlx::query(...).bind(...)`. **NEVER** the compile-checked `query!`/`query_as!` macros (this repo has no `DATABASE_URL` at build time and no `.sqlx` cache).
2. **DB-gated tests** self-skip when `DATABASE_URL` is unset, using exactly this shape (copied from `crates/egret-catalog/tests/store_ns.rs`):
   ```rust
   async fn store() -> Option<SecretStore> {
       let url = std::env::var("DATABASE_URL").ok()?;
       let db = egret_metastore::Db::connect(&url, 5).await.expect("connect");
       db.migrate().await.expect("migrate");
       Some(/* build the thing under test */)
   }
   // in each test:  let Some(s) = store().await else { eprintln!("skip"); return; };
   ```
3. **Engine-gated tests** self-skip unless `EGRET_TEST_TRINO_URL` / `EGRET_TEST_STARROCKS_URL` is set (same `let Some(..) else { return }` shape).
4. **Error → HTTP** via `impl IntoResponse` (pattern: `crates/egret-catalog/src/http_error.rs`). Internal DB/crypto errors map to 500 and never leak the underlying message verbatim beyond `to_string()` of a typed variant.
5. **Credential hygiene:** plaintext password/token is accepted ONLY on register/update; it is NEVER serialized into any read response. Detail responses expose `has_secret: bool` + non-secret `options` only.
6. **Commits:** plain `git commit` (uses the configured identity — never pass `--author`). Conventional Commits format.
7. **Format before every commit:** `cargo fmt --all`, and keep `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Reference facts (already verified against the codebase)

- `egret_metastore::Db { pub pool: PgPool }`, ctor `Db::connect(url, max) -> Result<Db, sqlx::Error>`, `db.migrate()`, `db.ping()`. Migrator embeds `../../migrations` (so the new `0008_database.sql` is picked up automatically).
- `egret_core::Error` variants: `NotFound(String)`, `InvalidInput(String)`, `Unauthorized`, `Forbidden`, `Conflict(String)`, `TooManyRequests`, `Internal(#[from] anyhow::Error)`; alias `egret_core::Result<T>`.
- Auth extractor: `egret_auth::principal::{Auth, AuthError}`. `Auth(p): Auth` requires `SessionService: FromRef<S>` on the router state. `p` is `egret_core::Principal` with `p.user_id: UserId` and `p.platform_roles: Vec<String>`.
- RBAC: `egret_workspace::rbac::Authorizer::new(db)`, method `require_platform(&p, "perm") -> egret_core::Result<()>` (Err = `Forbidden`). `workspace_roles_for(user_id, ws_id) -> Result<Vec<String>>` (empty ⇒ not a member).
- Workspace resolution: `egret_workspace::workspaces::WorkspaceRepo::new(db)`, `get_by_slug(slug) -> Result<Option<Workspace>>`; `Workspace { id: WorkspaceId, slug, name, status }` (filter `status == "active"`).
- Session for the Auth extractor: `egret_auth::session::SessionService` (already built in `serve()`); CSRF layer `egret_auth::csrf::csrf_layer`.
- Seeded platform roles: `platform_admin`, `data_steward` (migration `0003`). They currently have NO `database:*` permission — this plan seeds it in `0008`.
- Dev engines (already in `deploy/docker-compose.dev.yml`): Trino at host port **8090** (→ container 8080), StarRocks MySQL at **9030** (user `root`, no password).
- `AppState` lives in `crates/egret-server/src/lib.rs`; binary wiring in `crates/egret-bin/src/main.rs::serve`.

## File structure (created/modified across the plan)

```text
Cargo.toml                                    # MODIFY: workspace deps (egret-secrets, egret-database, aes-gcm, mysql_async)
migrations/0008_database.sql                  # CREATE: secrets/databases/workspace_database_grant + RBAC seed
crates/egret-config/src/lib.rs                # MODIFY: SecretsConfig + validate + tests
crates/egret-secrets/
  Cargo.toml                                  # CREATE
  src/lib.rs                                  # CREATE: re-exports
  src/error.rs                                # CREATE: SecretsError
  src/crypto.rs                               # CREATE: Crypto seal/open (+ unit tests)
  src/store.rs                                # CREATE: SecretStore put/get/delete
  tests/store.rs                              # CREATE: DB-gated SecretStore tests
crates/egret-database/
  Cargo.toml                                  # CREATE
  src/lib.rs                                  # CREATE: re-exports + DatabaseState
  src/error.rs                                # CREATE: DatabaseError + DbApiError(IntoResponse)
  src/model.rs                                # CREATE: Engine, Options, DTOs, DatabaseRow
  src/repo.rs                                 # CREATE: DatabaseRepo CRUD (seals creds)
  src/grants.rs                               # CREATE: grant/revoke + list_for_workspace
  src/tester.rs                               # CREATE: test_connection (Trino + StarRocks)
  src/routes.rs                               # CREATE: admin_database_router + me_database_router
  tests/repo.rs                               # CREATE: DB-gated repo + grants tests
  tests/http.rs                               # CREATE: router oneshot tests
  tests/tester_trino.rs                       # CREATE: engine-gated
  tests/tester_starrocks.rs                   # CREATE: engine-gated
crates/egret-server/src/lib.rs                # MODIFY: AppState.database + mount routers
crates/egret-bin/src/main.rs                  # MODIFY: build DatabaseState in serve()
```

---

## Task 1: Scaffold `egret-secrets` + `egret-database` crates and workspace deps

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Create: `crates/egret-secrets/Cargo.toml`, `crates/egret-secrets/src/lib.rs`
- Create: `crates/egret-database/Cargo.toml`, `crates/egret-database/src/lib.rs`

The workspace already globs `members = ["crates/*"]`, so new crates are members automatically; we only add the path entries to `[workspace.dependencies]` plus the two new third-party deps.

- [ ] **Step 1: Add workspace dependency entries**

In `Cargo.toml`, under `[workspace.dependencies]`, add the two path crates next to the existing `egret-*` entries:

```toml
egret-secrets = { path = "crates/egret-secrets" }
egret-database = { path = "crates/egret-database" }
```

And add the two new third-party deps (place near the other crypto/http deps):

```toml
aes-gcm = "0.10"
mysql_async = { version = "0.34", default-features = false, features = ["default-rustls"] }
```

- [ ] **Step 2: Create `crates/egret-secrets/Cargo.toml`**

```toml
[package]
name = "egret-secrets"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
aes-gcm.workspace = true
rand.workspace = true
base64.workspace = true
uuid.workspace = true
chrono.workspace = true
sqlx.workspace = true
thiserror.workspace = true

[dev-dependencies]
tokio.workspace = true
```

- [ ] **Step 3: Create `crates/egret-secrets/src/lib.rs`**

```rust
//! Credential encryption (AES-256-GCM) and a Postgres-backed secret store.

mod crypto;
mod error;
mod store;

pub use crypto::Crypto;
pub use error::SecretsError;
pub use store::SecretStore;
```

(`crypto`/`error`/`store` modules are created in Tasks 2–3. To keep this task compiling on its own, create the three files as empty-but-valid stubs now and fill them in later tasks. Add to `error.rs`: the full enum from Task 3 Step 1. Add to `crypto.rs` and `store.rs` a `// filled in Task 2/3` comment plus the minimal item so `pub use` resolves — i.e. create `error.rs` fully here, and stub `pub struct Crypto;` / `pub struct SecretStore;` to be replaced. Simpler: implement Task 2 and 3 contents now is NOT required — instead, in this task, only declare modules that exist. To avoid a broken build, create `crypto.rs`/`store.rs` with the real struct definitions from Tasks 2/3.)

> **Implementer note:** To avoid a half-broken crate, it is fine to create `error.rs` fully (Task 3 Step 1 content) in this task, and create `crypto.rs` + `store.rs` containing only `pub struct Crypto;` and `pub struct SecretStore;` placeholders. Tasks 2 and 3 replace those placeholders with the real implementations + tests. The crate must `cargo build -p egret-secrets` cleanly at the end of every task.

- [ ] **Step 4: Create `crates/egret-database/Cargo.toml`**

```toml
[package]
name = "egret-database"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-config.workspace = true
egret-metastore.workspace = true
egret-auth.workspace = true
egret-workspace.workspace = true
egret-secrets.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
reqwest.workspace = true
mysql_async.workspace = true
tokio.workspace = true

[dev-dependencies]
tower.workspace = true
http-body-util = "0.1"
```

- [ ] **Step 5: Create `crates/egret-database/src/lib.rs` (stub)**

```rust
//! Database registry: engines, encrypted credentials, workspace grants, connection test.

mod error;
mod grants;
mod model;
mod repo;
mod routes;
mod tester;

pub use error::{DatabaseError, DbApiError};
pub use model::Engine;
pub use repo::DatabaseRepo;
pub use routes::{admin_database_router, me_database_router, DatabaseState};
```

> **Implementer note:** Same approach — create each module file with the minimal real definitions so the crate builds. Subsequent tasks flesh them out. After every task, `cargo build -p egret-database` must pass.

- [ ] **Step 6: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: PASS (new crates compile as stubs).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/egret-secrets crates/egret-database
git commit -m "feat(database): scaffold egret-secrets and egret-database crates"
```

---

## Task 2: `egret-secrets::crypto` — AES-256-GCM seal/open

**Files:**
- Create/replace: `crates/egret-secrets/src/crypto.rs`
- (uses) `crates/egret-secrets/src/error.rs`

- [ ] **Step 1: Write the failing unit tests** (append into `crypto.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let c = Crypto::new(&key()).unwrap();
        let (nonce, ct) = c.seal("hunter2").unwrap();
        assert_eq!(nonce.len(), 12);
        assert_ne!(ct, b"hunter2"); // actually encrypted
        assert_eq!(c.open(&nonce, &ct).unwrap(), "hunter2");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let c = Crypto::new(&key()).unwrap();
        let (nonce, mut ct) = c.seal("secret").unwrap();
        ct[0] ^= 0xFF;
        assert!(c.open(&nonce, &ct).is_err());
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let c1 = Crypto::new(&key()).unwrap();
        let (nonce, ct) = c1.seal("secret").unwrap();
        let mut k2 = key();
        k2[0] ^= 0xFF;
        let c2 = Crypto::new(&k2).unwrap();
        assert!(c2.open(&nonce, &ct).is_err());
    }

    #[test]
    fn short_key_is_rejected() {
        assert!(Crypto::new(&[0u8; 16]).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p egret-secrets crypto`
Expected: FAIL to compile (`Crypto::new`/`seal`/`open` not implemented).

- [ ] **Step 3: Implement `Crypto`** (replace the stub at the top of `crypto.rs`)

```rust
//! AES-256-GCM authenticated encryption for credentials.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use rand::RngCore;

use crate::error::SecretsError;

/// Wraps an AES-256-GCM cipher built from a 32-byte master key.
#[derive(Clone)]
pub struct Crypto {
    cipher: Aes256Gcm,
}

impl Crypto {
    /// Build from a key of at least 32 bytes (only the first 32 are used).
    pub fn new(key: &[u8]) -> Result<Self, SecretsError> {
        if key.len() < 32 {
            return Err(SecretsError::Crypto("master key must be >= 32 bytes".into()));
        }
        let key = Key::<Aes256Gcm>::from_slice(&key[..32]);
        Ok(Self {
            cipher: Aes256Gcm::new(key),
        })
    }

    /// Encrypt `plaintext` with a fresh random 96-bit nonce.
    /// Returns `(nonce[12], ciphertext)` (ciphertext includes the GCM auth tag).
    pub fn seal(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), SecretsError> {
        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| SecretsError::Crypto("encryption failed".into()))?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    /// Decrypt; fails on a wrong key or tampered ciphertext (GCM auth-tag check).
    pub fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<String, SecretsError> {
        if nonce.len() != 12 {
            return Err(SecretsError::Crypto("nonce must be 12 bytes".into()));
        }
        let nonce = Nonce::from_slice(nonce);
        let pt = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretsError::Crypto("decryption failed".into()))?;
        String::from_utf8(pt).map_err(|_| SecretsError::Crypto("non-utf8 plaintext".into()))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p egret-secrets crypto`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-secrets/src/crypto.rs
git commit -m "feat(secrets): AES-256-GCM seal/open"
```

---

## Task 3: Migration `0008` + `SecretStore` (DB-gated) + `SecretsError`

**Files:**
- Create: `migrations/0008_database.sql`
- Create/replace: `crates/egret-secrets/src/error.rs`, `crates/egret-secrets/src/store.rs`
- Create: `crates/egret-secrets/tests/store.rs`

- [ ] **Step 1: Write `crates/egret-secrets/src/error.rs`**

```rust
//! Errors for credential encryption and storage.

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// Encryption/decryption/key failure (message is non-sensitive).
    #[error("crypto error: {0}")]
    Crypto(String),
    /// No master key configured but an encrypted operation was attempted.
    #[error("secrets master_key is not configured")]
    NotConfigured,
    /// Secret row not found.
    #[error("secret not found")]
    NotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
```

- [ ] **Step 2: Write `migrations/0008_database.sql`**

```sql
-- Encrypted credentials (AES-256-GCM): one row per stored password/token.
CREATE TABLE secrets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,                       -- 'db_password' | 'db_token'
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text NOT NULL DEFAULT 'cfg',         -- which master key sealed it (rotation reserve)
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Registered query engines (databases).
CREATE TABLE databases (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL UNIQUE,
    engine     text NOT NULL CHECK (engine IN ('trino','starrocks')),
    host       text NOT NULL,
    port       int  NOT NULL,
    options    jsonb NOT NULL DEFAULT '{}',         -- user, http_scheme, catalog/database, ssl…
    secret_id  uuid REFERENCES secrets(id) ON DELETE SET NULL,
    status     text NOT NULL DEFAULT 'unknown',     -- unknown | ok | error (last test)
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Grant a database to a workspace (global register → grant model).
CREATE TABLE workspace_database_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, database_id)
);

-- RBAC: platform_admin + data_steward may manage/read databases.
INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY['database:manage','database:read']) AS perm
WHERE scope = 'platform' AND name IN ('platform_admin','data_steward');
```

- [ ] **Step 3: Write the failing DB-gated tests** `crates/egret-secrets/tests/store.rs`

```rust
//! DB-gated SecretStore tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};

fn crypto() -> Crypto {
    Crypto::new(&[7u8; 32]).unwrap()
}

async fn store() -> Option<SecretStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(SecretStore::new(db, Some(crypto())))
}

#[tokio::test]
async fn put_get_delete_roundtrip() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let id = s.put("db_password", "p@ss").await.unwrap();
    assert_eq!(s.get(id).await.unwrap(), "p@ss");
    s.delete(id).await.unwrap();
    assert!(s.get(id).await.is_err());
}

#[tokio::test]
async fn put_without_key_errors() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let no_key = SecretStore::new(s.db_handle(), None);
    let err = no_key.put("db_password", "x").await.unwrap_err();
    assert!(matches!(err, egret_secrets::SecretsError::NotConfigured));
}
```

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test -p egret-secrets --test store`
Expected: FAIL to compile (`SecretStore::new`/`put`/`get`/`delete`/`db_handle` missing).

- [ ] **Step 5: Implement `crates/egret-secrets/src/store.rs`**

```rust
//! Postgres-backed store for AES-256-GCM-sealed secrets.

use egret_metastore::Db;
use uuid::Uuid;

use crate::crypto::Crypto;
use crate::error::SecretsError;

/// Stores encrypted secrets. `crypto` is `None` when no master key is configured —
/// any encrypted operation then fails with `NotConfigured`.
#[derive(Clone)]
pub struct SecretStore {
    db: Db,
    crypto: Option<Crypto>,
    key_id: String,
}

impl SecretStore {
    pub fn new(db: Db, crypto: Option<Crypto>) -> Self {
        Self {
            db,
            crypto,
            key_id: "cfg".into(),
        }
    }

    /// Whether a master key is configured (registration with a secret needs this).
    pub fn is_configured(&self) -> bool {
        self.crypto.is_some()
    }

    /// Clone the underlying DB handle (used by callers that need their own store).
    pub fn db_handle(&self) -> Db {
        self.db.clone()
    }

    fn crypto(&self) -> Result<&Crypto, SecretsError> {
        self.crypto.as_ref().ok_or(SecretsError::NotConfigured)
    }

    /// Seal `plaintext` and insert a new secret row; returns its id.
    pub async fn put(&self, kind: &str, plaintext: &str) -> Result<Uuid, SecretsError> {
        let (nonce, ct) = self.crypto()?.seal(plaintext)?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO secrets (kind, nonce, ciphertext, key_id) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(kind)
        .bind(nonce)
        .bind(ct)
        .bind(&self.key_id)
        .fetch_one(&self.db.pool)
        .await?;
        Ok(id)
    }

    /// Fetch and decrypt a secret by id.
    pub async fn get(&self, id: Uuid) -> Result<String, SecretsError> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, ciphertext FROM secrets WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?;
        let (nonce, ct) = row.ok_or(SecretsError::NotFound)?;
        self.crypto()?.open(&nonce, &ct)
    }

    /// Delete a secret by id (idempotent).
    pub async fn delete(&self, id: Uuid) -> Result<(), SecretsError> {
        sqlx::query("DELETE FROM secrets WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}
```

Replace the `crypto`-module declaration visibility in `lib.rs` so `store.rs` can `use crate::crypto::Crypto` — keep `mod crypto;` (private module) but `pub use crypto::Crypto;` already re-exports the type; `crate::crypto::Crypto` resolves regardless. No change needed.

- [ ] **Step 6: Run to verify it passes (with a database)**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-secrets --test store`
Expected: PASS (2 tests). Without `DATABASE_URL`: both print `skip` and pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add migrations/0008_database.sql crates/egret-secrets/src/error.rs crates/egret-secrets/src/store.rs crates/egret-secrets/tests/store.rs
git commit -m "feat(secrets): SecretStore over Postgres + 0008 migration"
```

---

## Task 4: `egret-config` `[secrets] master_key` + validate

**Files:**
- Modify: `crates/egret-config/src/lib.rs`

- [ ] **Step 1: Write the failing config tests** (add into the existing `mod tests` in `crates/egret-config/src/lib.rs`)

```rust
    #[test]
    fn secrets_master_key_defaults_empty_and_validates() {
        let cfg = Config::default();
        assert!(cfg.secrets.master_key.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_must_be_base64_32_bytes_when_set() {
        let mut cfg = Config::default();
        cfg.secrets.master_key = "tooshort".into(); // not valid base64 of >=32 bytes
        assert!(cfg.validate().is_err());
        use base64::Engine;
        cfg.secrets.master_key =
            base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert!(cfg.validate().is_ok());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-config secrets_master_key`
Expected: FAIL to compile (`cfg.secrets` field missing).

- [ ] **Step 3: Add the `SecretsConfig` struct** (next to `RangerSettings`, around line 36)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// base64-encoded master key (≥32 bytes) for AES-256-GCM credential encryption.
    /// Empty disables credential sealing — registering a database WITH a secret then fails.
    pub master_key: String,
}
```

- [ ] **Step 4: Add the field to `Config`** (after `pub ranger: RangerSettings,`)

```rust
    pub secrets: SecretsConfig,
```

- [ ] **Step 5: Add the default** (in `impl Default for Config`, after the `ranger: RangerSettings { .. }` block)

```rust
            secrets: SecretsConfig {
                master_key: String::new(),
            },
```

- [ ] **Step 6: Add validation** (in `validate()`, before the final `Ok(())`)

```rust
        if !self.secrets.master_key.is_empty() {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.secrets.master_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 32 {
                return Err("secrets.master_key must be base64 of >=32 bytes".into());
            }
        }
```

- [ ] **Step 7: Run to verify it passes**

Run: `cargo test -p egret-config`
Expected: PASS (all existing + 2 new tests).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/egret-config/src/lib.rs
git commit -m "feat(config): add [secrets] master_key with validation"
```

---

## Task 5: `egret-database` model + `DatabaseRepo` CRUD (DB-gated)

**Files:**
- Create/replace: `crates/egret-database/src/model.rs`, `crates/egret-database/src/error.rs`, `crates/egret-database/src/repo.rs`
- Create: `crates/egret-database/tests/repo.rs`

- [ ] **Step 1: Write `crates/egret-database/src/model.rs`**

```rust
//! Domain types: engine enum, connection options, DB row, request/response DTOs.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Supported query engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Trino,
    StarRocks,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Trino => "trino",
            Engine::StarRocks => "starrocks",
        }
    }

    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "trino" => Some(Engine::Trino),
            "starrocks" => Some(Engine::StarRocks),
            _ => None,
        }
    }
}

/// Non-secret connection options stored in `databases.options` (jsonb).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Trino only: "http" | "https".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_scheme: Option<String>,
    /// Trino default catalog (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// StarRocks default database (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// A full database registration row (no secret material).
#[derive(Debug, Clone)]
pub struct Database {
    pub id: Uuid,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub has_secret: bool,
    pub status: String,
}

// ── Request DTOs ──────────────────────────────────────────────────────────────

/// Register/update body. `secret` is plaintext, accepted only on write, never returned.
#[derive(Debug, Deserialize)]
pub struct DatabaseInput {
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub options: ConnOptions,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Body for testing an unsaved connection (no `name`).
#[derive(Debug, Deserialize)]
pub struct TestInput {
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub options: ConnOptions,
    #[serde(default)]
    pub secret: Option<String>,
}

// ── Response DTOs ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DatabaseDetailDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub has_secret: bool,
    pub status: String,
}

impl From<Database> for DatabaseDetailDto {
    fn from(d: Database) -> Self {
        DatabaseDetailDto {
            id: d.id.to_string(),
            name: d.name,
            engine: d.engine,
            host: d.host,
            port: d.port,
            options: d.options,
            has_secret: d.has_secret,
            status: d.status,
        }
    }
}

/// Minimal listing for the Explorer DB picker (no connection internals).
#[derive(Debug, Serialize)]
pub struct DatabaseListDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
}

/// Result of a connection test.
#[derive(Debug, Clone, Serialize)]
pub struct TestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub detail: String,
}
```

- [ ] **Step 2: Write `crates/egret-database/src/error.rs`**

```rust
//! Domain + HTTP error types for the database registry.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("secrets master_key is not configured")]
    SecretsNotConfigured,
    #[error(transparent)]
    Secrets(SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<SecretsError> for DatabaseError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotConfigured => DatabaseError::SecretsNotConfigured,
            SecretsError::Db(db) => DatabaseError::Db(db),
            other => DatabaseError::Secrets(other),
        }
    }
}

/// HTTP-facing error: either an authz failure (`egret_core::Error`) or a domain error.
pub enum DbApiError {
    Auth(egret_core::Error),
    Domain(DatabaseError),
}

impl From<egret_core::Error> for DbApiError {
    fn from(e: egret_core::Error) -> Self {
        DbApiError::Auth(e)
    }
}
impl From<DatabaseError> for DbApiError {
    fn from(e: DatabaseError) -> Self {
        DbApiError::Domain(e)
    }
}

impl IntoResponse for DbApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            DbApiError::Auth(e) => match e {
                egret_core::Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found".to_string()),
                egret_core::Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
                egret_core::Error::Unauthorized => {
                    (StatusCode::UNAUTHORIZED, "unauthorized".to_string())
                }
                egret_core::Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                egret_core::Error::Conflict(m) => (StatusCode::CONFLICT, m),
                egret_core::Error::TooManyRequests => {
                    (StatusCode::TOO_MANY_REQUESTS, "too many requests".to_string())
                }
                egret_core::Error::Internal(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
                }
            },
            DbApiError::Domain(e) => match &e {
                DatabaseError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                DatabaseError::AlreadyExists(_) => (StatusCode::CONFLICT, e.to_string()),
                DatabaseError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                DatabaseError::SecretsNotConfigured => (StatusCode::BAD_REQUEST, e.to_string()),
                DatabaseError::Secrets(_) | DatabaseError::Db(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
                }
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
```

- [ ] **Step 3: Write the failing DB-gated repo tests** `crates/egret-database/tests/repo.rs`

```rust
//! DB-gated DatabaseRepo tests. Self-skip without DATABASE_URL.
use egret_database::{DatabaseRepo, Engine};
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};

fn input(name: &str, secret: Option<&str>) -> egret_database::model::DatabaseInput {
    egret_database::model::DatabaseInput {
        name: name.into(),
        engine: Engine::Trino,
        host: "localhost".into(),
        port: 8080,
        options: egret_database::model::ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some("http".into()),
            ..Default::default()
        },
        secret: secret.map(|s| s.into()),
    }
}

async fn repo(with_key: bool) -> Option<DatabaseRepo> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let crypto = with_key.then(|| Crypto::new(&[3u8; 32]).unwrap());
    Some(DatabaseRepo::new(db.clone(), SecretStore::new(db, crypto)))
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn register_list_get_delete() {
    let Some(r) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let db = r.create(&input(&name, Some("pw")), None).await.unwrap();
    assert!(db.has_secret);
    let got = r.get(db.id).await.unwrap();
    assert_eq!(got.name, name);
    assert!(got.has_secret);
    assert!(r.list().await.unwrap().iter().any(|d| d.id == db.id));
    r.delete(db.id).await.unwrap();
    assert!(matches!(
        r.get(db.id).await.unwrap_err(),
        egret_database::DatabaseError::NotFound(_)
    ));
}

#[tokio::test]
async fn duplicate_name_conflicts() {
    let Some(r) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let a = r.create(&input(&name, None), None).await.unwrap();
    let err = r.create(&input(&name, None), None).await.unwrap_err();
    assert!(matches!(err, egret_database::DatabaseError::AlreadyExists(_)));
    r.delete(a.id).await.unwrap();
}

#[tokio::test]
async fn update_rotates_secret() {
    let Some(r) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let db = r.create(&input(&name, Some("old")), None).await.unwrap();
    let updated = r.update(db.id, &input(&name, Some("new")), None).await.unwrap();
    assert!(updated.has_secret);
    // plaintext re-seal verified indirectly: open_secret returns the new value
    assert_eq!(r.open_secret(db.id).await.unwrap(), Some("new".to_string()));
    r.delete(db.id).await.unwrap();
}

#[tokio::test]
async fn register_with_secret_without_key_errors() {
    let Some(r) = repo(false).await else {
        eprintln!("skip");
        return;
    };
    let err = r
        .create(&input(&uniq("db"), Some("pw")), None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        egret_database::DatabaseError::SecretsNotConfigured
    ));
}
```

> **Implementer note:** the test references `egret_database::model::*` — make the `model` module `pub` in `lib.rs` (`pub mod model;`) so DTOs are reachable from integration tests, and re-export `Engine` as already specified.

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test -p egret-database --test repo`
Expected: FAIL to compile (`DatabaseRepo` methods missing).

- [ ] **Step 5: Implement `crates/egret-database/src/repo.rs`**

```rust
//! Postgres-backed database registry CRUD; seals/open credentials via SecretStore.

use egret_core::UserId;
use egret_metastore::Db;
use egret_secrets::SecretStore;
use uuid::Uuid;

use crate::error::DatabaseError;
use crate::model::{ConnOptions, Database, DatabaseInput, Engine};

#[derive(Clone)]
pub struct DatabaseRepo {
    db: Db,
    secrets: SecretStore,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    name: String,
    engine: String,
    host: String,
    port: i32,
    options: serde_json::Value,
    secret_id: Option<Uuid>,
    status: String,
}

impl Row {
    fn into_database(self) -> Result<Database, DatabaseError> {
        let engine = Engine::parse(&self.engine)
            .ok_or_else(|| DatabaseError::BadRequest(format!("unknown engine: {}", self.engine)))?;
        let options: ConnOptions = serde_json::from_value(self.options).unwrap_or_default();
        Ok(Database {
            id: self.id,
            name: self.name,
            engine,
            host: self.host,
            port: self.port,
            options,
            has_secret: self.secret_id.is_some(),
            status: self.status,
        })
    }
}

impl DatabaseRepo {
    pub fn new(db: Db, secrets: SecretStore) -> Self {
        Self { db, secrets }
    }

    /// Expose the secret store (used by routes to open a stored secret for testing).
    pub fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    /// Register a new database, sealing `secret` if provided.
    pub async fn create(
        &self,
        input: &DatabaseInput,
        created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        let secret_id = match &input.secret {
            Some(s) if !s.is_empty() => Some(self.secrets.put("db_password", s).await?),
            _ => None,
        };
        let opts = serde_json::to_value(&input.options).unwrap_or_default();
        let row = sqlx::query_as::<_, Row>(
            "INSERT INTO databases (name, engine, host, port, options, secret_id, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING id, name, engine, host, port, options, secret_id, status",
        )
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(secret_id)
        .bind(created_by.map(|u| u.0))
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        });
        // If insert failed after sealing a secret, clean it up to avoid orphans.
        match row {
            Ok(r) => r.into_database(),
            Err(e) => {
                if let Some(sid) = secret_id {
                    let _ = self.secrets.delete(sid).await;
                }
                Err(e)
            }
        }
    }

    pub async fn get(&self, id: Uuid) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status \
             FROM databases WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
        .into_database()
    }

    pub async fn list(&self) -> Result<Vec<Database>, DatabaseError> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status \
             FROM databases ORDER BY name",
        )
        .fetch_all(&self.db.pool)
        .await?;
        rows.into_iter().map(Row::into_database).collect()
    }

    /// Update fields; if `secret` is present, re-seal and replace (delete the old secret).
    pub async fn update(
        &self,
        id: Uuid,
        input: &DatabaseInput,
        _actor: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        let existing = self.get(id).await?;
        let old_secret_id: Option<Uuid> =
            sqlx::query_scalar("SELECT secret_id FROM databases WHERE id = $1")
                .bind(id)
                .fetch_one(&self.db.pool)
                .await?;

        let new_secret_id = match &input.secret {
            Some(s) if !s.is_empty() => Some(self.secrets.put("db_password", s).await?),
            _ => old_secret_id, // keep the existing secret when none supplied
        };
        let opts = serde_json::to_value(&input.options).unwrap_or_default();
        sqlx::query(
            "UPDATE databases SET name=$2, engine=$3, host=$4, port=$5, options=$6, \
                 secret_id=$7, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(new_secret_id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        })?;

        // Delete the rotated-out secret if we replaced it.
        if input.secret.as_ref().is_some_and(|s| !s.is_empty()) {
            if let Some(old) = old_secret_id {
                let _ = self.secrets.delete(old).await;
            }
        }
        let _ = existing;
        self.get(id).await
    }

    /// Delete the database and its secret.
    pub async fn delete(&self, id: Uuid) -> Result<(), DatabaseError> {
        let secret_id: Option<Uuid> =
            sqlx::query_scalar("SELECT secret_id FROM databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten();
        sqlx::query("DELETE FROM databases WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        if let Some(sid) = secret_id {
            let _ = self.secrets.delete(sid).await;
        }
        Ok(())
    }

    /// Open the stored plaintext secret for a database (used by the connection tester).
    pub async fn open_secret(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        let secret_id: Option<Uuid> =
            sqlx::query_scalar("SELECT secret_id FROM databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten();
        match secret_id {
            None => Ok(None),
            Some(sid) => Ok(Some(self.secrets.get(sid).await?)),
        }
    }

    /// Record the last connection-test result.
    pub async fn set_status(&self, id: Uuid, status: &str) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE databases SET status=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(status)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}
```

> **Implementer note on `sqlx::query_scalar` + nullable column:** `secret_id` is `uuid NULL`; `query_scalar::<_, Option<Uuid>>` over `fetch_optional` yields `Option<Option<Uuid>>` — `.flatten()` collapses it. For the `update` path it uses `fetch_one` (the row exists), giving `Option<Uuid>` directly.

- [ ] **Step 6: Make `model` public for integration tests** — in `crates/egret-database/src/lib.rs` change `mod model;` to `pub mod model;` (keep `pub use model::Engine;`).

- [ ] **Step 7: Run to verify it passes (with a database)**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-database --test repo`
Expected: PASS (4 tests). Without `DATABASE_URL`: all `skip`.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/egret-database/src/model.rs crates/egret-database/src/error.rs crates/egret-database/src/repo.rs crates/egret-database/src/lib.rs crates/egret-database/tests/repo.rs
git commit -m "feat(database): model + DatabaseRepo CRUD with sealed credentials"
```

---

## Task 6: Grants + grant-aware listing

**Files:**
- Create/replace: `crates/egret-database/src/grants.rs`
- Modify: `crates/egret-database/tests/repo.rs` (add grant tests)

The grant methods live on `DatabaseRepo` (same struct, `impl` block in `grants.rs`) to reuse its `db` handle.

- [ ] **Step 1: Write the failing grant tests** (append to `crates/egret-database/tests/repo.rs`)

```rust
async fn ws_and_user() -> Option<(Db, uuid::Uuid, uuid::Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    // a throwaway user + workspace
    let uid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'t','local','active','x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", uuid::Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    let wid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", uuid::Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("ws");
    Some((db, wid, uid))
}

#[tokio::test]
async fn grant_revoke_and_list_for_workspace() {
    let Some(r) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let Some((_db, wid, uid)) = ws_and_user().await else {
        return;
    };
    let database = r.create(&input(&uniq("db"), None), None).await.unwrap();

    r.grant(wid, database.id, Some(egret_core::UserId(uid)))
        .await
        .unwrap();
    let listed = r.list_for_workspace(wid).await.unwrap();
    assert!(listed.iter().any(|d| d.id == database.id));

    r.revoke(wid, database.id).await.unwrap();
    let after = r.list_for_workspace(wid).await.unwrap();
    assert!(!after.iter().any(|d| d.id == database.id));

    r.delete(database.id).await.unwrap();
}
```

> **Implementer note:** confirm the actual `users` insert columns against `migrations/0002_identity.sql` before relying on the test helper — adjust the column list/values if the schema differs (e.g. a different NOT NULL set). The helper only needs to produce a valid `users` + `workspaces` row.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-database --test repo grant_revoke`
Expected: FAIL to compile (`grant`/`revoke`/`list_for_workspace` missing).

- [ ] **Step 3: Implement `crates/egret-database/src/grants.rs`**

```rust
//! Workspace grants for databases + grant-aware listing.

use egret_core::UserId;
use uuid::Uuid;

use crate::error::DatabaseError;
use crate::model::Database;
use crate::repo::DatabaseRepo;

impl DatabaseRepo {
    /// Grant a database to a workspace (idempotent).
    pub async fn grant(
        &self,
        workspace_id: Uuid,
        database_id: Uuid,
        granted_by: Option<UserId>,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO workspace_database_grant (workspace_id, database_id, granted_by) \
             VALUES ($1, $2, $3) ON CONFLICT (workspace_id, database_id) DO NOTHING",
        )
        .bind(workspace_id)
        .bind(database_id)
        .bind(granted_by.map(|u| u.0))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Revoke a database from a workspace (idempotent).
    pub async fn revoke(&self, workspace_id: Uuid, database_id: Uuid) -> Result<(), DatabaseError> {
        sqlx::query(
            "DELETE FROM workspace_database_grant WHERE workspace_id = $1 AND database_id = $2",
        )
        .bind(workspace_id)
        .bind(database_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Databases granted to a workspace (ordered by name).
    pub async fn list_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Database>, DatabaseError> {
        let rows = sqlx::query_as::<_, crate::repo::Row>(
            "SELECT d.id, d.name, d.engine, d.host, d.port, d.options, d.secret_id, d.status \
             FROM databases d \
             JOIN workspace_database_grant g ON g.database_id = d.id \
             WHERE g.workspace_id = $1 ORDER BY d.name",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(crate::repo::Row::into_database).collect()
    }
}
```

> **Implementer note:** `grants.rs` references `crate::repo::Row` and `Row::into_database`. Make `Row` and `into_database` visible to the sibling module: in `repo.rs` change `#[derive(sqlx::FromRow)] struct Row` to `pub(crate) struct Row` and `fn into_database` to `pub(crate) fn into_database`. Also add a `pub(crate) fn pool(&self) -> &sqlx::PgPool { &self.db.pool }` accessor on `DatabaseRepo` in `repo.rs` (used above).

- [ ] **Step 4: Run to verify it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-database --test repo`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-database/src/grants.rs crates/egret-database/src/repo.rs crates/egret-database/tests/repo.rs
git commit -m "feat(database): workspace grants and grant-aware listing"
```

---

## Task 7: Connection tester — Trino REST `SELECT 1` (engine-gated)

**Files:**
- Create/replace: `crates/egret-database/src/tester.rs`
- Create: `crates/egret-database/tests/tester_trino.rs`

- [ ] **Step 1: Write the failing engine-gated test** `crates/egret-database/tests/tester_trino.rs`

```rust
//! Engine-gated Trino tester test. Self-skip unless EGRET_TEST_TRINO_URL is set.
//! EGRET_TEST_TRINO_URL example: http://localhost:8090 (dev compose maps Trino there).
use egret_database::model::ConnOptions;
use egret_database::tester::test_trino;

#[tokio::test]
async fn trino_select_1_ok() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let scheme = url.scheme().to_string();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(8080) as i32;
    let opts = ConnOptions {
        user: Some("admin".into()),
        http_scheme: Some(scheme),
        ..Default::default()
    };
    let outcome = test_trino(&host, port, &opts, None).await;
    assert!(outcome.ok, "detail: {}", outcome.detail);
}
```

> **Implementer note:** add `url = { workspace = true }` to `egret-database`'s `[dev-dependencies]` for this test (workspace already pins `url`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-database --test tester_trino`
Expected: FAIL to compile (`test_trino` missing).

- [ ] **Step 3: Implement the Trino half of `crates/egret-database/src/tester.rs`**

```rust
//! On-demand connection test: a real `SELECT 1` per engine.

use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::model::{ConnOptions, Engine, TestOutcome};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Dispatch a connection test by engine. `secret` is the opened plaintext (if any).
pub async fn test_connection(
    engine: Engine,
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    match engine {
        Engine::Trino => test_trino(host, port, options, secret).await,
        Engine::StarRocks => test_starrocks(host, port, options, secret).await,
    }
}

// ── Trino (statement protocol) ────────────────────────────────────────────────

#[derive(Deserialize)]
struct TrinoResp {
    #[serde(default)]
    #[serde(rename = "nextUri")]
    next_uri: Option<String>,
    #[serde(default)]
    error: Option<TrinoError>,
}

#[derive(Deserialize)]
struct TrinoError {
    message: String,
}

/// Run `SELECT 1` via `POST /v1/statement` + `nextUri` polling.
pub async fn test_trino(
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    let started = Instant::now();
    let scheme = options.http_scheme.as_deref().unwrap_or("http");
    let user = options.user.as_deref().unwrap_or("egret");
    let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return fail(started, format!("client build: {e}")),
    };
    let url = format!("{scheme}://{host}:{port}/v1/statement");
    let mut req = client
        .post(&url)
        .header("X-Trino-User", user)
        .body("SELECT 1");
    if let Some(s) = secret {
        req = req.basic_auth(user, Some(s));
    }
    let mut resp: TrinoResp = match send_json(req).await {
        Ok(r) => r,
        Err(e) => return fail(started, e),
    };
    loop {
        if let Some(err) = resp.error {
            return fail(started, err.message);
        }
        match resp.next_uri.take() {
            Some(next) => {
                let r = client.get(&next);
                resp = match send_json(r).await {
                    Ok(r) => r,
                    Err(e) => return fail(started, e),
                };
            }
            None => break, // FINISHED with no error
        }
    }
    ok(started)
}

async fn send_json(req: reqwest::RequestBuilder) -> Result<TrinoResp, String> {
    let resp = req.send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.json::<TrinoResp>()
        .await
        .map_err(|e| format!("decode: {e}"))
}

// ── StarRocks (filled in Task 8) ──────────────────────────────────────────────

pub async fn test_starrocks(
    _host: &str,
    _port: i32,
    _options: &ConnOptions,
    _secret: Option<&str>,
) -> TestOutcome {
    // Implemented in Task 8.
    TestOutcome {
        ok: false,
        latency_ms: 0,
        detail: "starrocks tester not yet implemented".into(),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn ok(started: Instant) -> TestOutcome {
    TestOutcome {
        ok: true,
        latency_ms: started.elapsed().as_millis() as u64,
        detail: "SELECT 1 ok".into(),
    }
}

fn fail(started: Instant, detail: String) -> TestOutcome {
    TestOutcome {
        ok: false,
        latency_ms: started.elapsed().as_millis() as u64,
        detail,
    }
}
```

- [ ] **Step 4: Make `tester` module public** — in `crates/egret-database/src/lib.rs` change `mod tester;` to `pub mod tester;`.

- [ ] **Step 5: Run to verify it passes (against dev Trino)**

Run: `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-database --test tester_trino`
Expected: PASS. Without the env var: `skip`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/egret-database/src/tester.rs crates/egret-database/src/lib.rs crates/egret-database/Cargo.toml crates/egret-database/tests/tester_trino.rs
git commit -m "feat(database): Trino connection tester (SELECT 1 via statement protocol)"
```

---

## Task 8: Connection tester — StarRocks `mysql_async` `SELECT 1` (engine-gated)

**Files:**
- Modify: `crates/egret-database/src/tester.rs` (replace the `test_starrocks` stub)
- Create: `crates/egret-database/tests/tester_starrocks.rs`

- [ ] **Step 1: Write the failing engine-gated test** `crates/egret-database/tests/tester_starrocks.rs`

```rust
//! Engine-gated StarRocks tester test. Self-skip unless EGRET_TEST_STARROCKS_URL is set.
//! Example: EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030
use egret_database::model::ConnOptions;
use egret_database::tester::test_starrocks;

#[tokio::test]
async fn starrocks_select_1_ok() {
    let Some(base) = std::env::var("EGRET_TEST_STARROCKS_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(9030) as i32;
    let user = if url.username().is_empty() {
        "root".to_string()
    } else {
        url.username().to_string()
    };
    let secret = url.password().map(|p| p.to_string());
    let opts = ConnOptions {
        user: Some(user),
        ..Default::default()
    };
    let outcome = test_starrocks(&host, port, &opts, secret.as_deref()).await;
    assert!(outcome.ok, "detail: {}", outcome.detail);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 cargo test -p egret-database --test tester_starrocks`
Expected: FAIL (the stub returns `ok = false`).

- [ ] **Step 3: Replace the `test_starrocks` stub in `crates/egret-database/src/tester.rs`**

```rust
pub async fn test_starrocks(
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    use mysql_async::prelude::Queryable;

    let started = Instant::now();
    let user = options.user.clone().unwrap_or_else(|| "root".into());
    let mut builder = mysql_async::OptsBuilder::default()
        .ip_or_hostname(host.to_string())
        .tcp_port(port as u16)
        .user(Some(user));
    if let Some(s) = secret {
        builder = builder.pass(Some(s.to_string()));
    }
    if let Some(db) = &options.database {
        builder = builder.db_name(Some(db.clone()));
    }
    let opts: mysql_async::Opts = builder.into();

    let work = async {
        let mut conn = mysql_async::Conn::new(opts)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let got: Option<i64> = conn
            .query_first("SELECT 1")
            .await
            .map_err(|e| format!("query: {e}"))?;
        let _ = conn.disconnect().await;
        if got == Some(1) {
            Ok(())
        } else {
            Err(format!("unexpected result: {got:?}"))
        }
    };

    match tokio::time::timeout(TIMEOUT, work).await {
        Ok(Ok(())) => ok(started),
        Ok(Err(e)) => fail(started, e),
        Err(_) => fail(started, "timeout".into()),
    }
}
```

> **Implementer note:** verify the `mysql_async::OptsBuilder` method names against the resolved `mysql_async` 0.34 API before finishing (`ip_or_hostname`, `tcp_port`, `user`, `pass`, `db_name`, and `Opts: From<OptsBuilder>` are the expected surface). Adjust if the pinned version differs.

- [ ] **Step 4: Run to verify it passes (against dev StarRocks)**

Run: `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 cargo test -p egret-database --test tester_starrocks`
Expected: PASS. Without the env var: `skip`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-database/src/tester.rs crates/egret-database/tests/tester_starrocks.rs
git commit -m "feat(database): StarRocks connection tester (mysql_async SELECT 1)"
```

---

## Task 9: Routers (admin + me) + RBAC + HTTP tests

**Files:**
- Create/replace: `crates/egret-database/src/routes.rs`
- Create: `crates/egret-database/tests/http.rs`

- [ ] **Step 1: Write the failing HTTP tests** `crates/egret-database/tests/http.rs`

```rust
//! Router smoke tests via oneshot. Unauthenticated requests must be rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_database::{admin_database_router, me_database_router, DatabaseState, DatabaseRepo};
use egret_metastore::Db;
use egret_secrets::SecretStore;
use tower::ServiceExt;

async fn state() -> Option<DatabaseState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(DatabaseState {
        repo: DatabaseRepo::new(db.clone(), SecretStore::new(db.clone(), None)),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db),
    })
}

#[tokio::test]
async fn admin_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = admin_database_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/databases")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = me_database_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/databases?workspace=eng")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-database --test http`
Expected: FAIL to compile (`DatabaseState`, routers missing).

- [ ] **Step 3: Implement `crates/egret-database/src/routes.rs`**

```rust
//! Admin CRUD/test/grant routers + the Explorer listing router.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::session::SessionService;
use egret_core::{Error, UserId};
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::DbApiError;
use crate::model::{
    DatabaseDetailDto, DatabaseInput, DatabaseListDto, TestInput, TestOutcome,
};
use crate::repo::DatabaseRepo;

#[derive(Clone)]
pub struct DatabaseState {
    pub repo: DatabaseRepo,
    pub authz: Authorizer,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
}

// The Auth extractor needs SessionService from the router state.
impl FromRef<DatabaseState> for SessionService {
    fn from_ref(s: &DatabaseState) -> Self {
        s.sessions.clone()
    }
}

const PERM_MANAGE: &str = "database:manage";

fn is_platform_admin(p: &egret_core::Principal) -> bool {
    p.platform_roles.iter().any(|r| r == "platform_admin")
}

// ── Admin router ──────────────────────────────────────────────────────────────

pub fn admin_database_router() -> Router<DatabaseState> {
    Router::new()
        .route("/databases", post(register).get(list))
        .route("/databases/test", post(test_unsaved))
        .route(
            "/databases/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/databases/{id}/test", post(test_saved))
        .route("/databases/{id}/grants", post(grant))
        .route("/databases/{id}/grants/{workspace_id}", delete(revoke))
}

async fn register(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let db = st.repo.create(&body, Some(p.user_id)).await?;
    Ok(Json(db.into()))
}

async fn list(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
) -> Result<Json<Vec<DatabaseDetailDto>>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let dbs = st.repo.list().await?;
    Ok(Json(dbs.into_iter().map(Into::into).collect()))
}

async fn detail(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    Ok(Json(st.repo.get(id).await?.into()))
}

async fn update(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    Ok(Json(st.repo.update(id, &body, Some(p.user_id)).await?.into()))
}

async fn remove(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo.delete(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_saved(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let db = st.repo.get(id).await?;
    let secret = st.repo.open_secret(id).await?;
    let outcome = crate::tester::test_connection(
        db.engine,
        &db.host,
        db.port,
        &db.options,
        secret.as_deref(),
    )
    .await;
    let _ = st
        .repo
        .set_status(id, if outcome.ok { "ok" } else { "error" })
        .await;
    Ok(Json(outcome))
}

async fn test_unsaved(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Json(body): Json<TestInput>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    let outcome = crate::tester::test_connection(
        body.engine,
        &body.host,
        body.port,
        &body.options,
        body.secret.as_deref(),
    )
    .await;
    Ok(Json(outcome))
}

#[derive(Deserialize)]
struct GrantBody {
    workspace_id: Uuid,
}

async fn grant(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<GrantBody>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo.grant(body.workspace_id, id, Some(p.user_id)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn revoke(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Path((id, workspace_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_platform(&p, PERM_MANAGE).await?;
    st.repo.revoke(workspace_id, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Explorer (me) router ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WsQuery {
    workspace: String,
}

pub fn me_database_router() -> Router<DatabaseState> {
    Router::new().route("/databases", get(list_my_databases))
}

async fn list_my_databases(
    State(st): State<DatabaseState>,
    Auth(p): Auth,
    Query(q): Query<WsQuery>,
) -> Result<Json<Vec<DatabaseListDto>>, DbApiError> {
    let ws = st
        .workspaces
        .get_by_slug(&q.workspace)
        .await?
        .filter(|w| w.status == "active")
        .ok_or(Error::NotFound(format!("workspace: {}", q.workspace)))?;
    if !is_platform_admin(&p) {
        let roles = st.authz.workspace_roles_for(p.user_id, ws.id).await?;
        if roles.is_empty() {
            return Err(Error::Forbidden.into());
        }
    }
    let dbs = st.repo.list_for_workspace(ws.id.0).await?;
    Ok(Json(
        dbs.into_iter()
            .map(|d| DatabaseListDto {
                id: d.id.to_string(),
                name: d.name,
                engine: d.engine,
            })
            .collect(),
    ))
}
```

> **Implementer note:** `WorkspaceRepo::get_by_slug` / `workspace_roles_for` return `egret_core::Result<_>`; `?` converts via `DbApiError: From<egret_core::Error>`. `ws.id` is a `WorkspaceId(Uuid)`, so pass `ws.id.0` to `list_for_workspace`. Remove the unused `UserId` import if clippy flags it.

- [ ] **Step 4: Run to verify it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-database --test http`
Expected: PASS (2 tests assert `401`). Without `DATABASE_URL`: `skip`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/egret-database/src/routes.rs crates/egret-database/tests/http.rs
git commit -m "feat(database): admin + me routers with RBAC and connection test"
```

---

## Task 10: Wire into `egret-server` + `egret-bin`

**Files:**
- Modify: `crates/egret-server/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs`
- Modify: `crates/egret-server/Cargo.toml` (add `egret-database` dep), `crates/egret-bin/Cargo.toml` (add `egret-secrets` + `egret-database` deps)

- [ ] **Step 1: Add crate deps**

In `crates/egret-server/Cargo.toml` `[dependencies]` add:
```toml
egret-database.workspace = true
```
In `crates/egret-bin/Cargo.toml` `[dependencies]` add:
```toml
egret-secrets.workspace = true
egret-database.workspace = true
base64.workspace = true
```
(`base64` may already be present — check before adding.)

- [ ] **Step 2: Extend `AppState`** in `crates/egret-server/src/lib.rs`

Add the field after `pub ranger: ...`:
```rust
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub database: Option<egret_database::DatabaseState>,
```
And in `AppState::without_db()` add `database: None,`.

- [ ] **Step 3: Mount the routers** in `build_router`, after the `ranger` block and before `router.with_state(state)`:

```rust
    if let Some(database) = state.database.clone() {
        let group = Router::new()
            .nest("/api/admin", egret_database::admin_database_router())
            .nest("/api/me", egret_database::me_database_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(database);
        router = router.merge(group);
    }
```

> **Implementer note:** this merges a second `/api/admin` + `/api/me` group with a distinct state (`DatabaseState`). Axum merges route tables by path; `/api/admin/databases` and `/api/me/databases` don't collide with the workspace group's `/api/admin/workspaces` / `/api/me/workspaces`. Keep the CSRF layer (mutating admin routes need it, matching the workspace admin group).

- [ ] **Step 4: Build `DatabaseState` in `serve()`** in `crates/egret-bin/src/main.rs`, after the `catalog` block and before `build_router(...)`:

```rust
    let database = {
        use base64::Engine as _;
        let crypto = if config.secrets.master_key.is_empty() {
            None
        } else {
            let key = base64::engine::general_purpose::STANDARD
                .decode(config.secrets.master_key.as_bytes())
                .map_err(|e| anyhow::anyhow!("secrets.master_key base64: {e}"))?;
            Some(egret_secrets::Crypto::new(&key).map_err(|e| anyhow::anyhow!(e.to_string()))?)
        };
        let secrets = egret_secrets::SecretStore::new(db.clone(), crypto);
        Some(egret_database::DatabaseState {
            repo: egret_database::DatabaseRepo::new(db.clone(), secrets),
            authz: Authorizer::new(db.clone()),
            sessions: sessions.clone(),
            workspaces: WorkspaceRepo::new(db.clone()),
        })
    };
```

- [ ] **Step 5: Pass it into `build_router`** — extend the `AppState { .. }` literal at the end of `serve()` with `database,`.

- [ ] **Step 6: Build + run server unit tests**

Run: `cargo build --workspace && cargo test -p egret-server`
Expected: PASS (health tests still green with the new `database: None` field).

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/egret-server crates/egret-bin
git commit -m "feat(server): mount database registry routers and wire DatabaseState"
```

---

## Task 11: Final verification

**Files:** none (verification only) — except a possible touch-up to `crates/egret-metastore/src/lib.rs` if its migration-count assertion needs bumping.

- [ ] **Step 1: Format + lint the whole workspace**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Fix any warnings in the touched crates (commit the fix).

- [ ] **Step 2: Full unit-test pass (no DB/engines)**

Run: `cargo test --workspace`
Expected: PASS — DB-gated and engine-gated tests print `skip`; everything else green. Confirm `egret-metastore`'s `migrator_embeds_baseline_migration` (asserts `>= 7`) still passes with `0008` present (it does; 8 ≥ 7).

- [ ] **Step 3: DB-backed pass (Postgres up)**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace`
Expected: PASS — `egret-secrets` store tests, `egret-database` repo/grants/http tests run for real.

- [ ] **Step 4: Engine-backed pass (dev compose Trino + StarRocks up)**

Run:
```bash
EGRET_TEST_TRINO_URL=http://localhost:8090 \
EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 \
DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
cargo test -p egret-database
```
Expected: PASS — both tester tests assert `ok = true` against real engines.

- [ ] **Step 5: Manual smoke (optional but recommended)** — with `egret.toml` setting a base64 `[secrets] master_key`, boot `cargo run -p egret-bin -- serve`, log in as the bootstrap admin, then `POST /api/admin/databases` (Trino, `host=localhost port=8090 options.http_scheme=http options.user=admin`), `POST /api/admin/databases/{id}/test` → expect `{ "ok": true }`, grant to a workspace, and `GET /api/me/databases?workspace=<slug>` → see the database. Verify no read endpoint ever returns the plaintext secret.

- [ ] **Step 6: Final review + finish the branch** — dispatch the final code reviewer over the whole branch, then use **superpowers:finishing-a-development-branch** (tests must pass first) to push + open the PR.

---

## Self-review notes (author)

- **Spec coverage:** §4 secrets → Tasks 2–3; §5 data model → Task 3 Step 2 (+ RBAC seed added, a gap the spec implied via §7); §6 test → Tasks 7–8; §7 REST/RBAC → Task 9; §8 config/wiring → Tasks 4, 10; §9 testing → every task's tests + Task 11; §10 decomposition → Tasks 1–11 (1:1). 
- **Deviation from spec §3 (logged intentionally):** `SecretsConfig` lives in `egret-config` (inline, next to `RangerSettings`/`CatalogSettings`) rather than in `egret-secrets`, to avoid `egret-config → egret-secrets` coupling and match the existing settings pattern. `egret-secrets` stays config-agnostic (`Crypto::new(&[u8])`, `SecretStore::new(db, Option<Crypto>)`). The binary decodes the base64 key and injects it.
- **Deviation from spec §4:** `SecretStore` holds `Option<Crypto>` so the registry works for no-credential databases even when `master_key` is empty; registering *with* a secret then fails as `SecretsNotConfigured` (spec's "fails clearly when master_key empty"). 
- **Type consistency:** `Engine` (`as_str`/`parse`), `ConnOptions`, `DatabaseInput`, `Database`, `TestOutcome`, `DatabaseRepo::{create,get,list,update,delete,open_secret,set_status,grant,revoke,list_for_workspace}`, `DatabaseState{repo,authz,sessions,workspaces}` are used identically across Tasks 5–10.
- **No placeholders:** every code step is complete; engine/version "implementer notes" flag the two places (`mysql_async` builder API, `users` test-insert columns) to verify against the resolved dependency/schema.
