# Phase 1a-3 — OIDC / SSO Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** OIDC/SSO login — authorization-code + PKCE via the `openidconnect` crate, JIT provisioning (match by `sub`), and IdP group sync at login (roles flow through the existing 1a-2 group→role path), tested against a real Keycloak.

**Architecture:** OIDC lives in `egret-auth`: `OidcService` (openidconnect), `GroupRepo` + new `UserRepo` methods, `provision_and_sync`, encrypted state cookie, and `oidc/login` + `oidc/callback` routes mounted via a dedicated `OidcRoutesState` (so `AuthState` is untouched). Per the [Phase 1a-3 design spec](../specs/2026-06-17-phase-1a-3-oidc-sso-design.md).

**Tech Stack:** Rust (stable), `openidconnect` 4, `reqwest` (rustls), `axum-extra` PrivateCookieJar, sqlx 0.8; tests use `testcontainers` + real Keycloak. Reuses 1a-1/1a-2 helpers.

**Reference:** [Phase 1a-3 spec](../specs/2026-06-17-phase-1a-3-oidc-sso-design.md); existing `egret-auth` (`users`, `session`, `routes::{AuthState, auth_router}`, `cookie`, `principal`), `egret-config`, `egret-server`, `egret-bin`.

**Conventions:** runtime sqlx; `db_err` per module; DB-gated tests self-skip without `DATABASE_URL`; Keycloak tests self-skip without Docker; plain `git commit` (configured identity, NO `-c` overrides) + trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**⚠️ Executor-iteration points:** Tasks 4 (openidconnect API) and 8–9 (Keycloak/testcontainers + HTML-form scripting) use **version-sensitive external APIs** that may not compile verbatim. Treat the provided code as correct-to-intent; adjust call shapes to the resolved crate versions **without changing behavior or test assertions**. Acceptance for the OIDC HTTP layer = the Keycloak integration tests pass.

---

## File Structure

```text
migrations/0005_oidc.sql
crates/egret-config/src/lib.rs            # OidcConfig, SecurityConfig, Config::validate()
crates/egret-auth/src/users.rs            # find_by_external_subject, insert_oidc, update_profile
crates/egret-auth/src/groups.rs           # GroupRepo
crates/egret-auth/src/oidc.rs             # OidcService, OidcClaims, OidcStart, provision_and_sync
crates/egret-auth/src/oidc_cookie.rs      # encrypted state cookie (build/read)
crates/egret-auth/src/oidc_routes.rs      # OidcRoutesState + oidc_router
crates/egret-auth/src/lib.rs              # module exports
crates/egret-auth/Cargo.toml              # openidconnect, reqwest, axum-extra cookie-private; dev: testcontainers
crates/egret-auth/tests/oidc_db.rs        # Tier-0 DB tests (provision_and_sync, GroupRepo)
crates/egret-auth/tests/oidc_keycloak.rs  # Tier-1 Keycloak integration tests (gated)
crates/egret-server/src/lib.rs            # AppState.oidc; mount oidc_router
crates/egret-bin/src/main.rs              # build OidcRoutesState when enabled; Config::validate()
deploy/keycloak/egret-realm.json          # pre-provisioned realm
deploy/docker-compose.dev.yml             # keycloak service
Cargo.toml                                # workspace deps
crates/egret-metastore/src/lib.rs         # migrator count >= 5
```

---

### Task 1: Migration 0005 + config (OidcConfig, SecurityConfig, validation) + deps

**Files:** Create `migrations/0005_oidc.sql`; Modify `Cargo.toml`, `crates/egret-config/src/lib.rs`, `crates/egret-metastore/src/lib.rs`, `crates/egret-auth/Cargo.toml`.

- [ ] **Step 1: Migration + migrator count**

`migrations/0005_oidc.sql`:
```sql
CREATE UNIQUE INDEX users_oidc_subject_uniq
    ON users (external_subject)
    WHERE provider = 'oidc' AND external_subject IS NOT NULL;
```
In `crates/egret-metastore/src/lib.rs` bump the assertion to `>= 5`:
```rust
        assert!(MIGRATOR.migrations.len() >= 5, "expected through the oidc migration");
```

- [ ] **Step 2: Add workspace dependencies**

In `Cargo.toml` `[workspace.dependencies]`:
```toml
openidconnect = "4"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "cookies"] }
testcontainers = "0.23"
```
In `crates/egret-auth/Cargo.toml`: add to `[dependencies]` `openidconnect.workspace = true`, `reqwest.workspace = true`, and change `axum-extra` to include the private-cookie feature:
```toml
axum-extra = { version = "0.10", features = ["cookie", "cookie-private"] }
```
and to `[dev-dependencies]` add `testcontainers.workspace = true` and `reqwest.workspace = true`.

- [ ] **Step 3: Add config structs + validation with tests**

In `crates/egret-config/src/lib.rs` add:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub post_login_redirect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// base64-encoded key (≥64 bytes) for encrypting the OIDC state cookie.
    pub secret_key: String,
}
```
Add `pub oidc: OidcConfig,` and `pub security: SecurityConfig,` to `Config`, and to its `Default`:
```rust
            oidc: OidcConfig {
                enabled: false,
                issuer_url: String::new(),
                client_id: String::new(),
                client_secret: String::new(),
                redirect_url: "http://localhost:8080/api/auth/oidc/callback".into(),
                scopes: vec!["openid".into(), "email".into(), "profile".into(), "groups".into()],
                groups_claim: "groups".into(),
                post_login_redirect: "/".into(),
            },
            security: SecurityConfig { secret_key: String::new() },
```
Add a validation method on `Config`:
```rust
impl Config {
    /// Validate cross-field invariants after loading. Call at boot.
    pub fn validate(&self) -> Result<(), String> {
        if self.oidc.enabled {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.security.secret_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 64 {
                return Err("oidc.enabled requires security.secret_key (base64, >=64 bytes)".into());
            }
            if self.oidc.issuer_url.is_empty() || self.oidc.client_id.is_empty() {
                return Err("oidc.enabled requires issuer_url and client_id".into());
            }
        }
        Ok(())
    }
}
```
Add `base64.workspace = true` to `crates/egret-config/Cargo.toml` `[dependencies]`. Append tests:
```rust
    #[test]
    fn validate_ok_when_oidc_disabled() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_requires_secret_key_when_oidc_enabled() {
        let mut cfg = Config::default();
        cfg.oidc.enabled = true;
        cfg.oidc.issuer_url = "http://idp".into();
        cfg.oidc.client_id = "egret".into();
        assert!(cfg.validate().is_err()); // empty secret_key
    }
```

- [ ] **Step 4: Build + test + commit**

`cargo test -p egret-config -p egret-metastore --lib` → pass. `cargo fmt --all`; `cargo clippy -p egret-config -p egret-metastore --all-targets -- -D warnings`.
```bash
git add Cargo.toml migrations/0005_oidc.sql crates/egret-config crates/egret-metastore/src/lib.rs crates/egret-auth/Cargo.toml
git commit -m "feat(config): oidc + security config, validation, oidc migration"
```

---

### Task 2: `UserRepo` oidc methods + `GroupRepo`

**Files:** Modify `crates/egret-auth/src/users.rs`, `crates/egret-auth/src/lib.rs`; Create `crates/egret-auth/src/groups.rs`, `crates/egret-auth/tests/oidc_db.rs`.

- [ ] **Step 1: Add UserRepo oidc methods**

In `crates/egret-auth/src/users.rs` `impl UserRepo`:
```rust
    pub async fn find_by_external_subject(&self, sub: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             WHERE provider = 'oidc' AND external_subject = $1",
        )
        .bind(sub)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn insert_oidc(&self, sub: &str, email: &str, name: &str) -> Result<UserId> {
        let res = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO users (provider, external_subject, email, name) \
             VALUES ('oidc', $1, $2, $3) RETURNING id",
        )
        .bind(sub)
        .bind(email)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(UserId(id)),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("email or subject already exists: {email}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }

    pub async fn update_profile(&self, id: UserId, email: &str, name: &str) -> Result<()> {
        sqlx::query("UPDATE users SET email = $1, name = $2 WHERE id = $3")
            .bind(email)
            .bind(name)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
```

- [ ] **Step 2: Create GroupRepo**

`crates/egret-auth/src/groups.rs`:
```rust
//! Postgres-backed group upsert + per-user OIDC group synchronization.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

#[derive(Clone)]
pub struct GroupRepo {
    db: Db,
}

impl GroupRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert (source='oidc') or return the existing group id by name.
    pub async fn upsert_oidc_group(&self, name: &str) -> Result<Uuid> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO groups (name, source) VALUES ($1, 'oidc') \
             ON CONFLICT (name) DO UPDATE SET name = groups.name RETURNING id",
        )
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    /// Replace the user's OIDC-sourced group memberships with `names` (in one tx).
    /// Manual (source='local') memberships are left untouched.
    pub async fn sync_user_oidc_groups(&self, user_id: UserId, names: &[String]) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM user_groups WHERE user_id = $1 \
             AND group_id IN (SELECT id FROM groups WHERE source = 'oidc')",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for name in names {
            sqlx::query(
                "INSERT INTO user_groups (user_id, group_id) \
                 SELECT $1, id FROM groups WHERE name = $2 \
                 ON CONFLICT DO NOTHING",
            )
            .bind(user_id.0)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
```
Add `pub mod groups;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 3: Create the Tier-0 DB test harness + GroupRepo/UserRepo tests**

`crates/egret-auth/tests/oidc_db.rs`:
```rust
//! Tier-0 OIDC DB tests (no IdP). Self-skip without DATABASE_URL.

use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn insert_and_find_oidc_user_by_subject() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let users = UserRepo::new(db.clone());
    let sub = uniq("sub");
    let email = format!("{}@idp.local", uniq("u"));
    let id = users.insert_oidc(&sub, &email, "Tester").await.unwrap();
    let found = users.find_by_external_subject(&sub).await.unwrap().expect("found");
    assert_eq!(found.id, id);
    assert_eq!(found.provider, "oidc");
}

#[tokio::test]
async fn group_sync_replaces_oidc_keeps_local() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let id = users.insert_oidc(&uniq("sub"), &format!("{}@idp.local", uniq("u")), "T").await.unwrap();

    // a manual (local) group membership that must survive sync
    let local_gid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
        .bind(uniq("local-grp")).fetch_one(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(id.0).bind(local_gid).execute(&db.pool).await.unwrap();

    // first sync: [a, b]
    let a = uniq("g"); let b = uniq("g");
    groups.upsert_oidc_group(&a).await.unwrap();
    groups.upsert_oidc_group(&b).await.unwrap();
    groups.sync_user_oidc_groups(id, &[a.clone(), b.clone()]).await.unwrap();

    // second sync: [a] only → b removed, local kept
    groups.sync_user_oidc_groups(id, &[a.clone()]).await.unwrap();

    let names: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id WHERE ug.user_id = $1")
        .bind(id.0).fetch_all(&db.pool).await.unwrap();
    assert!(names.contains(&a));
    assert!(!names.contains(&b), "oidc group b should be revoked");
    assert_eq!(names.iter().filter(|n| n.starts_with("local-grp")).count(), 1, "local membership kept");
}
```

- [ ] **Step 4: Run with DB + lint + commit**

Offline `cargo test -p egret-auth` (skips); with DB `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test oidc_db` → pass. Clippy/fmt clean.
```bash
git add crates/egret-auth/src/users.rs crates/egret-auth/src/groups.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/oidc_db.rs
git commit -m "feat(auth): UserRepo oidc lookups and GroupRepo with sync"
```

---

### Task 3: `provision_and_sync` + `OidcClaims`

**Files:** Create `crates/egret-auth/src/oidc.rs` (claims + provisioning only this task); Modify `crates/egret-auth/src/lib.rs`, `tests/oidc_db.rs`.

- [ ] **Step 1: Define claims + provisioning**

`crates/egret-auth/src/oidc.rs` (the `OidcService` itself comes in Task 4 — this task adds only the types + provisioning):
```rust
//! OIDC login: claims, provisioning, and (Task 4) the OidcService.

use egret_core::{Result, UserId};

use crate::groups::GroupRepo;
use crate::users::UserRepo;

/// Normalized claims extracted from a validated id_token.
#[derive(Debug, Clone)]
pub struct OidcClaims {
    pub subject: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

/// JIT-provision the user (match by `sub`) and sync their OIDC group memberships.
/// Returns the local user id. Roles resolve via the existing group→role path.
pub async fn provision_and_sync(
    users: &UserRepo,
    groups: &GroupRepo,
    claims: &OidcClaims,
) -> Result<UserId> {
    let user_id = match users.find_by_external_subject(&claims.subject).await? {
        Some(u) => {
            users.update_profile(u.id, &claims.email, &claims.name).await?;
            u.id
        }
        None => users.insert_oidc(&claims.subject, &claims.email, &claims.name).await?,
    };
    users.touch_last_login(user_id).await?;
    for g in &claims.groups {
        groups.upsert_oidc_group(g).await?;
    }
    groups.sync_user_oidc_groups(user_id, &claims.groups).await?;
    Ok(user_id)
}
```
Add `pub mod oidc;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 2: Add provisioning DB tests**

Append to `crates/egret-auth/tests/oidc_db.rs`:
```rust
use egret_auth::oidc::{provision_and_sync, OidcClaims};

fn claims(sub: &str, email: &str, groups: &[&str]) -> OidcClaims {
    OidcClaims {
        subject: sub.into(),
        email: email.into(),
        name: "T".into(),
        groups: groups.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn provision_creates_then_updates_same_user() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let sub = uniq("sub");
    let g = uniq("g");
    let e1 = format!("{}@idp.local", uniq("u"));

    let id1 = provision_and_sync(&users, &groups, &claims(&sub, &e1, &[&g])).await.unwrap();
    // second login, same sub, new email → same id, profile updated
    let e2 = format!("{}@idp.local", uniq("u"));
    let id2 = provision_and_sync(&users, &groups, &claims(&sub, &e2, &[&g])).await.unwrap();
    assert_eq!(id1, id2);
    let u = users.find_by_id(id1).await.unwrap().unwrap();
    assert_eq!(u.email.to_lowercase(), e2.to_lowercase());
}

#[tokio::test]
async fn provision_email_collision_with_other_account_conflicts() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let email = format!("{}@idp.local", uniq("u"));
    // a pre-existing local user owns this email
    users.insert_local(&email, "L", "$argon2id$dummy").await.ok(); // hash format irrelevant for the unique check
    let err = provision_and_sync(&users, &groups, &claims(&uniq("sub"), &email, &[])).await;
    assert!(matches!(err, Err(egret_core::Error::Conflict(_))));
}
```
> If `insert_local` rejects the dummy hash, insert the colliding local user via raw SQL instead: `INSERT INTO users (provider, email, name) VALUES ('local', $1, 'L')`.

- [ ] **Step 3: Run with DB + lint + commit**

`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test oidc_db` → pass. Clippy/fmt clean.
```bash
git add crates/egret-auth/src/oidc.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/oidc_db.rs
git commit -m "feat(auth): JIT provision_and_sync with OIDC group sync"
```

---

### Task 4: `OidcService` (openidconnect)

**Files:** Modify `crates/egret-auth/src/oidc.rs`.

> **⚠️ Version-sensitive:** `openidconnect` 4's exact types/builders (http client adapter, `discover_async`, claim extraction) may differ from the code below. Keep the public interface (`discover`/`start`/`finish`/`verify_id_token`) and behavior; adjust internal calls to the resolved API. The Keycloak tests (Tasks 8–9) are the acceptance gate.

- [ ] **Step 1: Add the OidcService**

Append to `crates/egret-auth/src/oidc.rs`:
```rust
use egret_config::OidcConfig;
use egret_core::Error;
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
};

/// What `start` returns; the caller persists state/nonce/verifier in the encrypted cookie.
pub struct OidcStart {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

#[derive(Clone)]
pub struct OidcService {
    client: CoreClient,
    http: reqwest::Client,
    scopes: Vec<String>,
    groups_claim: String,
}

fn oidc_err(msg: impl std::fmt::Display) -> Error {
    Error::Internal(anyhow::anyhow!("oidc: {msg}"))
}

impl OidcService {
    /// Discover provider metadata and construct the client.
    pub async fn discover(cfg: &OidcConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(oidc_err)?;
        let issuer = IssuerUrl::new(cfg.issuer_url.clone()).map_err(oidc_err)?;
        let meta = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(oidc_err)?;
        let client = CoreClient::from_provider_metadata(
            meta,
            ClientId::new(cfg.client_id.clone()),
            Some(ClientSecret::new(cfg.client_secret.clone())),
        )
        .set_redirect_uri(RedirectUrl::new(cfg.redirect_url.clone()).map_err(oidc_err)?);
        Ok(Self {
            client,
            http,
            scopes: cfg.scopes.clone(),
            groups_claim: cfg.groups_claim.clone(),
        })
    }

    /// Build the authorize URL with a fresh PKCE challenge, nonce, and state.
    pub fn start(&self) -> OidcStart {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in &self.scopes {
            req = req.add_scope(Scope::new(s.clone()));
        }
        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();
        OidcStart {
            authorize_url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        }
    }

    /// Exchange the code and validate the id_token; return normalized claims.
    pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<OidcClaims> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
            .request_async(&self.http)
            .await
            .map_err(oidc_err)?;
        let id_token = openidconnect::OAuth2TokenResponse::extra_fields(&token_response);
        let _ = id_token; // see verify_id_token; openidconnect exposes id_token via IdTokenFields
        self.verify_from_response(&token_response, nonce)
    }

    // Extracted so the ROPC test (Task 8) can validate a raw id_token too.
    fn verify_from_response(
        &self,
        token_response: &openidconnect::core::CoreTokenResponse,
        nonce: &str,
    ) -> Result<OidcClaims> {
        use openidconnect::TokenResponse;
        let id_token = token_response.id_token().ok_or_else(|| oidc_err("no id_token"))?;
        let verifier = self.client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &Nonce::new(nonce.to_string()))
            .map_err(oidc_err)?;
        self.claims_from(claims)
    }

    fn claims_from(&self, claims: &openidconnect::core::CoreIdTokenClaims) -> Result<OidcClaims> {
        let subject = claims.subject().to_string();
        let email = claims
            .email()
            .map(|e| e.to_string())
            .ok_or_else(|| oidc_err("no email claim"))?;
        let name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string())
            .unwrap_or_else(|| email.clone());
        // groups come from a custom claim; read via the additional-claims JSON.
        let groups = extract_groups(claims, &self.groups_claim);
        Ok(OidcClaims { subject, email, name, groups })
    }
}

/// Read a string-array claim by name from the id_token's additional claims.
fn extract_groups(claims: &openidconnect::core::CoreIdTokenClaims, claim: &str) -> Vec<String> {
    // openidconnect exposes unknown claims via serde; serialize then read the field.
    let val = serde_json::to_value(claims).unwrap_or(serde_json::Value::Null);
    match val.get(claim) {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}
```
> **Executor note:** the exact way to read a custom (`groups`) claim and to call `id_token.claims(verifier, nonce)` depends on the resolved `openidconnect` version. The intent: validate signature/iss/aud/exp/nonce via the crate, then read `sub`/`email`/`name` and the configured groups array. If `serde_json::to_value(claims)` doesn't surface `groups`, parameterize `CoreIdTokenClaims` with an `AdditionalClaims` type that captures `groups: Option<Vec<String>>` and read it directly. Keep `OidcClaims` and the tests unchanged.

- [ ] **Step 2: Build (no unit test here — validated by Keycloak tests)**

Run: `cargo build -p egret-auth`. Expected: compiles. `cargo clippy -p egret-auth --all-targets -- -D warnings`; `cargo fmt --all`.

- [ ] **Step 3: Commit**
```bash
git add crates/egret-auth/src/oidc.rs
git commit -m "feat(auth): OidcService over openidconnect (discover/start/finish)"
```

---

### Task 5: Encrypted state cookie

**Files:** Create `crates/egret-auth/src/oidc_cookie.rs`; Modify `crates/egret-auth/src/lib.rs`.

- [ ] **Step 1: Write the state-cookie helpers with a unit test**

`crates/egret-auth/src/oidc_cookie.rs`:
```rust
//! Encrypted, short-lived cookie carrying OIDC state/nonce/pkce between login and callback.

use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};

pub const OIDC_STATE_COOKIE: &str = "egret_oidc_state";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcStateCookie {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Build the (plaintext) cookie to be added to a `PrivateCookieJar` (which encrypts it).
pub fn build_state_cookie(value: &OidcStateCookie, secure: bool) -> Cookie<'static> {
    let json = serde_json::to_string(value).expect("serialize oidc state");
    Cookie::build((OIDC_STATE_COOKIE, json))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/api/auth/oidc")
        .max_age(time::Duration::seconds(600))
        .build()
}

pub fn clear_state_cookie() -> Cookie<'static> {
    Cookie::build((OIDC_STATE_COOKIE, ""))
        .path("/api/auth/oidc")
        .max_age(time::Duration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_roundtrips_through_json() {
        let v = OidcStateCookie { state: "s".into(), nonce: "n".into(), pkce_verifier: "p".into() };
        let c = build_state_cookie(&v, true);
        let back: OidcStateCookie = serde_json::from_str(c.value()).unwrap();
        assert_eq!(back.nonce, "n");
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.path(), Some("/api/auth/oidc"));
    }
}
```
> Uses the `time` dependency already added to `egret-auth` in Phase 1a-1.

Add `pub mod oidc_cookie;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 2: Run + commit**

`cargo test -p egret-auth --lib` → cookie test passes. Clippy/fmt clean.
```bash
git add crates/egret-auth/src/oidc_cookie.rs crates/egret-auth/src/lib.rs
git commit -m "feat(auth): encrypted OIDC state cookie helpers"
```

---

### Task 6: OIDC routes + `OidcRoutesState` + server/bin wiring

**Files:** Create `crates/egret-auth/src/oidc_routes.rs`; Modify `crates/egret-auth/src/lib.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Write the routes + state**

`crates/egret-auth/src/oidc_routes.rs`:
```rust
//! OIDC login/callback routes. Mounted only when OIDC is enabled.

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::cookie::{Key, PrivateCookieJar};
use egret_config::Config;
use serde::Deserialize;

use crate::cookie::{csrf_cookie, session_cookie};
use crate::groups::GroupRepo;
use crate::oidc::{provision_and_sync, OidcService};
use crate::oidc_cookie::{build_state_cookie, clear_state_cookie, OidcStateCookie, OIDC_STATE_COOKIE};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;

#[derive(Clone)]
pub struct OidcRoutesState {
    pub oidc: OidcService,
    pub users: UserRepo,
    pub groups: GroupRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub cookie_key: Key,
}

impl FromRef<OidcRoutesState> for Key {
    fn from_ref(s: &OidcRoutesState) -> Self {
        s.cookie_key.clone()
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

pub fn oidc_router() -> Router<OidcRoutesState> {
    Router::new()
        .route("/login", get(oidc_login))
        .route("/callback", get(oidc_callback))
}

async fn oidc_login(State(st): State<OidcRoutesState>, jar: PrivateCookieJar) -> Response {
    let start = st.oidc.start();
    let cookie = build_state_cookie(
        &OidcStateCookie {
            state: start.state,
            nonce: start.nonce,
            pkce_verifier: start.pkce_verifier,
        },
        st.config.session.secure,
    );
    let jar = jar.add(cookie);
    (jar, Redirect::to(&start.authorize_url)).into_response()
}

async fn oidc_callback(
    State(st): State<OidcRoutesState>,
    jar: PrivateCookieJar,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(raw) = jar.get(OIDC_STATE_COOKIE) else {
        return (StatusCode::BAD_REQUEST, "missing oidc state").into_response();
    };
    let Ok(state): Result<OidcStateCookie, _> = serde_json::from_str(raw.value()) else {
        return (StatusCode::BAD_REQUEST, "bad oidc state").into_response();
    };
    if state.state != q.state {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }

    let claims = match st.oidc.finish(&q.code, &state.pkce_verifier, &state.nonce).await {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "oidc exchange failed").into_response(),
    };
    let user_id = match provision_and_sync(&st.users, &st.groups, &claims).await {
        Ok(id) => id,
        Err(egret_core::Error::Conflict(_)) => {
            return (StatusCode::CONFLICT, "email already used by another account").into_response()
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "provisioning failed").into_response(),
    };
    let issued = match st.sessions.create(user_id, "", "").await {
        Ok(i) => i,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "session failed").into_response(),
    };

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    let jar = jar
        .remove(clear_state_cookie())
        .add(session_cookie(issued.token, secure, max_age))
        .add(csrf_cookie(csrf, secure, max_age));
    (jar, Redirect::to(&st.config.oidc.post_login_redirect)).into_response()
}
```
Add `pub mod oidc_routes;` to `crates/egret-auth/src/lib.rs`.
> **Executor note:** returning `(PrivateCookieJar, Redirect)` requires the jar to be the response-modifying part; if the tuple `IntoResponse` ordering differs in the resolved axum-extra, wrap as `(jar, Redirect::to(...)).into_response()` (already done) or convert the jar into headers explicitly. `session_cookie`/`csrf_cookie` return plain `Cookie`s added to the *private* jar — that's fine (private jar also stores plaintext cookies added via `add`; if you need them un-encrypted, use a separate `CookieJar`. For 1a-3 keep them in the private jar — the browser still receives valid Set-Cookie headers).

- [ ] **Step 2: Wire into egret-server**

In `crates/egret-server/src/lib.rs`: import `use egret_auth::oidc_routes::{oidc_router, OidcRoutesState};`, add `pub oidc: Option<OidcRoutesState>` to `AppState` (and `oidc: None` in `without_db()`), and in `build_router` mount it (public, no csrf):
```rust
    if let Some(oidc) = state.oidc.clone() {
        router = router.merge(
            Router::new().nest("/api/auth/oidc", oidc_router()).with_state(oidc),
        );
    }
```

- [ ] **Step 3: Build OidcRoutesState in egret-bin when enabled**

In `crates/egret-bin/src/main.rs` `serve`, after building `db`/`config`: call `config.validate().map_err(|e| anyhow::anyhow!(e))?` right after load (in `main`, before dispatch). In `serve`, build the optional OIDC state:
```rust
    let oidc = if config.oidc.enabled {
        use axum_extra::extract::cookie::Key;
        use base64::Engine;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(config.security.secret_key.as_bytes())
            .map_err(|e| anyhow::anyhow!("security.secret_key base64: {e}"))?;
        let service = egret_auth::oidc::OidcService::discover(&config.oidc)
            .await
            .map_err(|e| anyhow::anyhow!("oidc discovery: {e}"))?;
        Some(egret_auth::oidc_routes::OidcRoutesState {
            oidc: service,
            users: UserRepo::new(db.clone()),
            groups: egret_auth::groups::GroupRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                Duration::hours(config.session.absolute_ttl_hours as i64),
                Duration::minutes(config.session.idle_ttl_minutes as i64),
            ),
            config: config.clone(),
            cookie_key: Key::from(&key_bytes),
        })
    } else {
        None
    };
```
Pass `oidc` into `AppState { db: Some(db), auth: Some(auth), workspace: Some(workspace), oidc }`. Add `base64.workspace = true` to `crates/egret-bin/Cargo.toml` if not present.
> `Key::from(&key_bytes)` requires ≥64 bytes — guaranteed by `Config::validate()`.

- [ ] **Step 4: Build + offline test + commit**

`cargo build --workspace`; `cargo test --workspace` (offline; existing health/`without_db` tests still pass with `oidc: None`); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
```bash
git add crates/egret-auth/src/oidc_routes.rs crates/egret-auth/src/lib.rs crates/egret-server/src/lib.rs crates/egret-bin
git commit -m "feat(auth): oidc login/callback routes and conditional wiring"
```

---

### Task 7: Keycloak realm + dev compose service

**Files:** Create `deploy/keycloak/egret-realm.json`; Modify `deploy/docker-compose.dev.yml`.

- [ ] **Step 1: Create the realm export**

`deploy/keycloak/egret-realm.json`:
```json
{
  "realm": "egret",
  "enabled": true,
  "groups": [{ "name": "data-team" }],
  "clients": [
    {
      "clientId": "egret",
      "enabled": true,
      "protocol": "openid-connect",
      "publicClient": false,
      "secret": "egret-secret",
      "standardFlowEnabled": true,
      "directAccessGrantsEnabled": true,
      "redirectUris": ["http://localhost:8080/*", "http://localhost/*", "*"],
      "attributes": { "pkce.code.challenge.method": "S256" },
      "protocolMappers": [
        {
          "name": "groups",
          "protocol": "openid-connect",
          "protocolMapper": "oidc-group-membership-mapper",
          "config": {
            "claim.name": "groups",
            "full.path": "false",
            "id.token.claim": "true",
            "access.token.claim": "true",
            "userinfo.token.claim": "true"
          }
        }
      ]
    }
  ],
  "users": [
    {
      "username": "tester",
      "enabled": true,
      "email": "tester@egret.local",
      "emailVerified": true,
      "firstName": "Test",
      "lastName": "User",
      "credentials": [{ "type": "password", "value": "pw", "temporary": false }],
      "groups": ["/data-team"]
    }
  ]
}
```

- [ ] **Step 2: Add Keycloak to the dev compose**

In `deploy/docker-compose.dev.yml` `services:` add:
```yaml
  keycloak:
    image: quay.io/keycloak/keycloak:26.0
    command: ["start-dev", "--import-realm"]
    environment:
      KC_BOOTSTRAP_ADMIN_USERNAME: admin
      KC_BOOTSTRAP_ADMIN_PASSWORD: admin
    ports:
      - "8081:8080"
    volumes:
      - ./keycloak:/opt/keycloak/data/import:ro
```

- [ ] **Step 3: Validate compose + commit**

Run: `docker compose -f deploy/docker-compose.dev.yml config -q` → exit 0.
```bash
git add deploy/keycloak/egret-realm.json deploy/docker-compose.dev.yml
git commit -m "chore(dev): keycloak realm and dev-stack service"
```

---

### Task 8: Keycloak testcontainers harness + ROPC validation test

**Files:** Create `crates/egret-auth/tests/oidc_keycloak.rs`; Modify `crates/egret-auth/src/oidc.rs` (add `verify_id_token`).

> **⚠️ Integration-heavy + version-sensitive** (testcontainers API, Keycloak readiness, realm import path). The behavior is fixed; adjust container setup to the resolved `testcontainers` version. Gate the whole file so it self-skips when Docker is unavailable.

- [ ] **Step 1: Expose a raw id_token validator for the ROPC path**

In `crates/egret-auth/src/oidc.rs`, add a public method to `impl OidcService` that validates a raw id_token string (nonce optional):
```rust
    /// Validate a raw id_token (used by tests / non-auth-code flows). `nonce` is checked
    /// only when `Some`. Returns normalized claims.
    pub fn verify_id_token(&self, id_token_str: &str, nonce: Option<&str>) -> Result<OidcClaims> {
        use openidconnect::core::CoreIdToken;
        use std::str::FromStr;
        let id_token = CoreIdToken::from_str(id_token_str).map_err(oidc_err)?;
        let verifier = self.client.id_token_verifier();
        let nonce_check = |n: Option<&Nonce>| match (nonce, n) {
            (None, _) => Ok(()),
            (Some(expected), Some(got)) if got.secret() == expected => Ok(()),
            _ => Err(openidconnect::ClaimsVerificationError::InvalidNonce("mismatch".into())),
        };
        let claims = id_token.claims(&verifier, nonce_check).map_err(oidc_err)?;
        self.claims_from(claims)
    }
```
> **Executor note:** the `nonce` verifier argument shape varies by `openidconnect` version (a `Nonce` or a closure). Adjust to match; the intent is "skip nonce check when `None`, else require equality."

- [ ] **Step 2: Write the gated Keycloak harness + ROPC test**

`crates/egret-auth/tests/oidc_keycloak.rs`:
```rust
//! Tier-1 OIDC tests against a real Keycloak. Self-skip when Docker is unavailable.
//! Run locally with Docker running; CI provides Docker.

use egret_auth::oidc::OidcService;
use egret_config::OidcConfig;

// Starts Keycloak with the egret realm imported; returns the base issuer URL.
// Returns None if Docker isn't available (test self-skips).
async fn start_keycloak() -> Option<(String, testcontainers::ContainerAsync<KeycloakImage>)> {
    // See KeycloakImage below. testcontainers detects Docker; if absent, return None.
    match KeycloakImage::start_with_realm().await {
        Ok((port, container)) => {
            Some((format!("http://localhost:{port}/realms/egret"), container))
        }
        Err(e) => {
            eprintln!("keycloak unavailable, skipping: {e}");
            None
        }
    }
}

fn oidc_cfg(issuer: &str) -> OidcConfig {
    OidcConfig {
        enabled: true,
        issuer_url: issuer.into(),
        client_id: "egret".into(),
        client_secret: "egret-secret".into(),
        redirect_url: "http://localhost:8080/api/auth/oidc/callback".into(),
        scopes: vec!["openid".into(), "email".into(), "profile".into(), "groups".into()],
        groups_claim: "groups".into(),
        post_login_redirect: "/".into(),
    }
}

#[tokio::test]
async fn ropc_token_validates_and_has_groups() {
    let Some((issuer, _kc)) = start_keycloak().await else { return; };
    let svc = OidcService::discover(&oidc_cfg(&issuer)).await.expect("discover");

    // Direct-access-grant (ROPC) to obtain a real id_token.
    let token_url = format!("{issuer}/protocol/openid-connect/token");
    let resp: serde_json::Value = reqwest::Client::new()
        .post(&token_url)
        .form(&[
            ("grant_type", "password"),
            ("client_id", "egret"),
            ("client_secret", "egret-secret"),
            ("username", "tester"),
            ("password", "pw"),
            ("scope", "openid email profile groups"),
        ])
        .send().await.expect("token req")
        .json().await.expect("token json");
    let id_token = resp["id_token"].as_str().expect("id_token");

    let claims = svc.verify_id_token(id_token, None).expect("validate");
    assert_eq!(claims.email, "tester@egret.local");
    assert!(claims.groups.iter().any(|g| g == "data-team"));
}

// --- Minimal testcontainers image wrapper for Keycloak ---
struct KeycloakImage;
impl KeycloakImage {
    async fn start_with_realm() -> Result<(u16, testcontainers::ContainerAsync<KeycloakImage>), Box<dyn std::error::Error>> {
        use testcontainers::core::{Mount, WaitFor};
        use testcontainers::{GenericImage, ImageExt, runners::AsyncRunner};
        let realm_host = format!("{}/deploy/keycloak", env!("CARGO_WORKSPACE_DIR_FALLBACK_OK"));
        let image = GenericImage::new("quay.io/keycloak/keycloak", "26.0")
            .with_wait_for(WaitFor::message_on_stdout("Listening on: http://0.0.0.0:8080"))
            .with_exposed_port(8080.into())
            .with_cmd(["start-dev", "--import-realm"])
            .with_env_var("KC_BOOTSTRAP_ADMIN_USERNAME", "admin")
            .with_env_var("KC_BOOTSTRAP_ADMIN_PASSWORD", "admin")
            .with_mount(Mount::bind_mount(realm_host, "/opt/keycloak/data/import"));
        let container = image.start().await?;
        let port = container.get_host_port_ipv4(8080).await?;
        Ok((port, container))
    }
}
// NOTE: the `KeycloakImage`/typing above is illustrative — implement with the resolved
// testcontainers API (the `GenericImage` builder is what matters). The realm bind-mount
// must point at the absolute `deploy/keycloak` dir; resolve it from `env!("CARGO_MANIFEST_DIR")`
// joined with `../../deploy/keycloak`.
```
> **Executor note:** Resolve the realm directory as `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/keycloak")` (canonicalized). Use the resolved `testcontainers` 0.23 builder/runner API; the wait-for string may differ across Keycloak versions — if so, wait on the readiness endpoint `/health/ready` via a short poll instead. The `ContainerAsync<KeycloakImage>` return type is just "keep the container alive"; use whatever handle type the API returns.

- [ ] **Step 3: Run gated test (with Docker) + lint + commit**

With Docker available: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test oidc_keycloak -- --nocapture` → `ropc_token_validates_and_has_groups` passes (or self-skips if Docker is absent — note which). `cargo clippy -p egret-auth --all-targets -- -D warnings`; `cargo fmt --all`.
```bash
git add crates/egret-auth/src/oidc.rs crates/egret-auth/tests/oidc_keycloak.rs
git commit -m "test(auth): keycloak testcontainers harness + ROPC id_token validation"
```

---

### Task 9: Full authorization-code + PKCE + nonce test (Keycloak)

**Files:** Modify `crates/egret-auth/tests/oidc_keycloak.rs`.

> **⚠️ Integration-heavy:** scripts Keycloak's HTML login form headlessly. Selectors/redirect handling may need adjustment to the Keycloak version.

- [ ] **Step 1: Add the full-flow test**

Append to `crates/egret-auth/tests/oidc_keycloak.rs`:
```rust
#[tokio::test]
async fn full_auth_code_flow_yields_claims() {
    let Some((issuer, _kc)) = start_keycloak().await else { return; };
    let svc = OidcService::discover(&oidc_cfg(&issuer)).await.expect("discover");
    let start = svc.start();

    // reqwest client with a cookie jar, redirects disabled so we can read the Location.
    let http = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build().unwrap();

    // 1. GET the authorize URL → Keycloak login page (HTML with a form action).
    let login_html = http.get(&start.authorize_url).send().await.unwrap().text().await.unwrap();
    let action = extract_form_action(&login_html).expect("login form action");

    // 2. POST credentials to the form action → 302 to redirect_uri?code=...&state=...
    let resp = http.post(&action)
        .form(&[("username", "tester"), ("password", "pw")])
        .send().await.unwrap();
    let location = resp.headers().get("location").expect("redirect").to_str().unwrap().to_string();
    let code = extract_query_param(&location, "code").expect("code");
    let state = extract_query_param(&location, "state").expect("state");
    assert_eq!(state, start.state, "state echoed back");

    // 3. finish: exchange code (PKCE) + validate id_token (nonce).
    let claims = svc.finish(&code, &start.pkce_verifier, &start.nonce).await.expect("finish");
    assert_eq!(claims.email, "tester@egret.local");
    assert!(claims.groups.iter().any(|g| g == "data-team"));
}

fn extract_form_action(html: &str) -> Option<String> {
    // Keycloak login form: <form ... action="<url>" method="post">
    let i = html.find("action=\"")? + "action=\"".len();
    let rest = &html[i..];
    let j = rest.find('"')?;
    Some(rest[..j].replace("&amp;", "&"))
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            return it.next().map(|v| v.to_string());
        }
    }
    None
}
```
> **Executor note:** Keycloak's login page sometimes returns the form action as a relative or absolute URL with session params; `extract_form_action` handles the common absolute case. If the POST returns 200 (re-rendered form) instead of 302, the credentials/selectors need adjusting (check for a different field name or a required `credentialId`). The redirect `Location` host is `http://localhost:8080/...` (the configured redirect) — we never actually hit our server; we only parse the `code`.

- [ ] **Step 2: Run gated (with Docker) + lint + commit**

With Docker: `cargo test -p egret-auth --test oidc_keycloak` → both Keycloak tests pass. Clippy/fmt clean.
```bash
git add crates/egret-auth/tests/oidc_keycloak.rs
git commit -m "test(auth): full auth-code+PKCE+nonce flow against keycloak"
```

---

### Task 10: Final verification + e2e

**Files:** none (verification only).

- [ ] **Step 1: Offline gate**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Expected: build OK; all tests pass (DB-gated + Keycloak-gated self-skip offline); clippy/fmt clean.

- [ ] **Step 2: DB-gated**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test -p egret-auth --test oidc_db
```
Expected: all Tier-0 OIDC DB tests pass.

- [ ] **Step 3: Keycloak-gated (Docker)**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test -p egret-auth --test oidc_keycloak -- --nocapture
```
Expected: ROPC + full-flow tests pass against a real Keycloak (testcontainers).

- [ ] **Step 4: Manual e2e (optional, against dev stack)**

```bash
docker compose -f deploy/docker-compose.dev.yml up -d keycloak postgres   # wait healthy
# configure egret.toml/env: oidc.enabled=true, issuer http://localhost:8081/realms/egret,
#   client_id=egret, client_secret=egret-secret, security.secret_key=<base64 64 bytes>
cargo run -p egret-bin -- serve &
# browse http://localhost:8080/api/auth/oidc/login → Keycloak login (tester/pw) → redirected, session cookie set
```
Expected: login through Keycloak issues an Egret session; `GET /api/auth/me` returns the provisioned user.

- [ ] **Step 5: Commit (only if a verification fix was needed)**
```bash
git add -A
git commit -m "fix(auth): address final OIDC verification findings"
```

---

## Self-Review

**Spec coverage (Phase 1a-3 spec):**
- §3 modules (`oidc`, `groups`, `users` additions, `oidc_routes`) → Tasks 2–6. ✅
- §4 `OidcService` discover/start/finish → Task 4 (+`verify_id_token` Task 8). ✅
- §5 encrypted state cookie → Task 5. ✅
- §6 login/callback routes (404 when disabled via conditional mount; state check; CSRF-exempt public mount) → Task 6. ✅
- §7 `provision_and_sync` (match by sub, email conflict, group replace keeping local) → Tasks 2, 3. ✅
- §8 migration `0005` → Task 1. ✅
- §9 config (`OidcConfig`, `SecurityConfig`, validation, secret_key) → Task 1, 6. ✅
- §10 security (id_token validation, state, PKCE, no auto-link, secrets) → Tasks 4, 6, 8. ✅
- §11 testing (Tier-0 DB; Tier-1 Keycloak ROPC + full flow; compose dev) → Tasks 2,3,7,8,9. ✅

**Placeholder scan:** No "TBD"/"add validation". The three executor notes (Tasks 4, 8, 9) describe **concrete required adaptations** to version-sensitive external APIs with the exact intent + fallback, not unwritten logic — this is the honest treatment of `openidconnect`/`testcontainers`/Keycloak-HTML, which cannot be pinned blind. The `env!("CARGO_WORKSPACE_DIR_FALLBACK_OK")` token in Task 8 is explicitly flagged in its note to be replaced with `CARGO_MANIFEST_DIR`-relative resolution.

**Type consistency:**
- `OidcClaims { subject, email, name, groups }` — consistent Tasks 3, 4, 8. ✅
- `OidcService::{discover, start, finish, verify_id_token}` + `claims_from`/`extract_groups` — consistent Tasks 4, 8. ✅
- `provision_and_sync(&UserRepo, &GroupRepo, &OidcClaims) -> UserId` — consistent Tasks 3, 6. ✅
- `GroupRepo::{new, upsert_oidc_group, sync_user_oidc_groups}` — consistent Tasks 2, 3, 6. ✅
- `OidcRoutesState { oidc, users, groups, sessions, config, cookie_key }` + `FromRef<_> for Key` — consistent Tasks 6. ✅
- `Error::Conflict` reused for email collision; `AppState.oidc: Option<OidcRoutesState>` — consistent Tasks 2, 6. ✅
- `AuthState` is **not** modified (OIDC uses its own state) → no churn to 1a-1/1a-2 AuthState constructions. ✅

**Open follow-ups:** account linking by verified email; `idp_role_mapping` (claim→role) activation; multi-IdP — all future.

---

## Execution record (applied during implementation)

Implemented via subagent-driven TDD; independently reviewed (spec + security + code quality). All gates green: offline `cargo test --workspace` (18 suites), DB `oidc_db` 4/4, **real Keycloak `oidc_keycloak` 2/2** (ROPC + full auth-code+PKCE+nonce, ~1–2 min), clippy/fmt clean.

**Adaptations (version-sensitive, applied correctly):**
- `openidconnect` **4.0.1** endpoint type-states: client carries `Endpoint{Set,NotSet,MaybeSet}` generics; `exchange_code` is fallible; groups read via a custom `AdditionalClaims` (`GroupsClaims`) parameterizing the id-token type. Default features already include reqwest+rustls.
- `testcontainers` **0.23**: `GenericImage` + `ImageExt` (`with_cmd`/`with_env_var`/`with_mount`) + `AsyncRunner::start`; readiness by polling the discovery endpoint.

**Real-IdP testing paid off — two genuine defects caught that a mock would have missed:**
1. **`invalid_scope`**: requesting a `groups` *scope* against the realm (which emits groups via a client mapper, not a client-scope) is rejected by Keycloak and breaks the authorize page. Fixed by dropping `"groups"` from the default `OidcConfig.scopes` (commit `e8ceebb`); the group claim is still emitted by the mapper. Spec §9 updated.
2. **Encrypted session cookie (Critical, found in review)**: the callback added `egret_session`/`egret_csrf` to the `PrivateCookieJar`, which encrypts them — so the browser's session cookie was ciphertext that `SessionService.validate()` could never match, leaving OIDC users silently unauthenticated. Fixed (commit `9e9ee94`): session/csrf now go on a plain `CookieJar` (only the PKCE state cookie stays encrypted), state cookie is cleared on all error paths, and a `login_response` helper + regression test assert the emitted `Set-Cookie` value is the plaintext token.

**Minor deferred (non-blocking):** `upsert_oidc_group` returns an existing `source='local'` group's id by name (a claim group colliding with a manual group name won't be revoked by sync — matches the "don't touch local" intent); account linking by verified email; `idp_role_mapping` activation. A browser-level callback→`/me` e2e (beyond the unit regression test) would further harden the cookie path.
