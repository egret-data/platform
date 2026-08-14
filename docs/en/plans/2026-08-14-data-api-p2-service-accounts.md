# Data API P2 — Service accounts and API keys Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-14-data-api-design.md` §5, §6.1
> Part: P2 of 5 — see `docs/en/plans/2026-08-14-data-api-decomposition.md`
> Stacked on: P1 (`fix/dialect-escaping`, PR #504). Branch off P1, do not re-include its code.

**Goal:** an admin can create a workspace-scoped service account, mint an API key for it, and that key verifies — with no endpoint to call yet.

**Architecture:** a service account is a real row in `users` with `provider = 'service'`, so `access_grants`, Cedar, `ensure_username`, machine-idp and audit all treat it exactly like a user with no parallel plumbing. Its workspace binding and metadata live in a new `dataapi.service_accounts` table; keys live in `dataapi.api_keys`. The cost of reusing `users` is that service accounts must be kept out of human-facing enumeration surfaces — Task 2 does that explicitly and tests it, rather than leaving it as cleanup.

**Tech Stack:** Rust (axum, sqlx) · Postgres · SHA-256 (`sha2`) · Cedar PDP.

## Global Constraints

- A service account **must never be able to log in interactively**. The login handler already requires `password_hash.is_some()` (`crates/egret-auth/src/routes.rs:184`), but that is safety by accident — this plan adds a DB-level CHECK so it holds even if a future insert path forgets.
- **Filter enumeration, never id-keyed joins.** A service account appearing in the admin user list is a defect; a service account's name appearing next to its own grant or audit row is correct. Task 2 lists exactly which sites are which. Do not "helpfully" filter a join.
- API keys are hashed with **SHA-256, not Argon2**. `crates/egret-auth/src/password.rs` is for user-chosen passwords; a 32-byte random secret needs no brute-force-resistant KDF, and ~50–100 ms of KDF per API request is unacceptable. Compare in constant time.
- The key secret is returned **exactly once**, at creation. It is never recoverable afterwards.
- Follow `crates/egret-controlplane/src/access/mod.rs`'s authz idiom: reads and lifecycle gate on a delegatable `require(&*st.pdp, &p, "x:y")`, but **anything that mutates the authorization graph gates on platform admin**. Minting a key for a service account that already holds grants is such a mutation — see Task 4.
- Run scoped checks, not `just check` (it runs `cargo test --workspace`). `egret-config::tests::toml_file_is_merged` is a known pre-existing flake.
- Never run two workspace test suites concurrently.

## File Structure

| File | Responsibility |
|---|---|
| `migrations/0119_dataapi_service_accounts.sql` (create) | `users.provider` CHECK extension + the service/no-password CHECK; `dataapi` schema; `service_accounts` and `api_keys` tables. |
| `crates/egret-auth/src/users.rs` (modify) | `insert_service`; `list` excludes service accounts. |
| `crates/egret-dataapi-api/src/lib.rs` (create) | Pure-serde DTOs for service accounts and keys. |
| `crates/egret-dataapi/src/lib.rs` (create) | Crate root; re-exports the store and router. |
| `crates/egret-dataapi/src/service_accounts.rs` (create) | Store: create / list / disable a service account. |
| `crates/egret-dataapi/src/keys.rs` (create) | Key mint, hash, verify, revoke. The only place key material is handled. |
| `crates/egret-dataapi/src/routes.rs` (create) | `management_router()` — the workspace-scoped admin surface. |
| `crates/egret-core/src/rbac.rs` (modify) | Two new permissions in the catalog. |
| `crates/egret-server/src/lib.rs` (modify) | Mount the router; merge the OpenAPI. |
| `docs/en/specs/2026-08-14-data-api-design.md` (modify) | §5.1 gains `dataapi.service_accounts`; `api_keys` FK retargeted. |

---

### Task 1: Schema and the service-account row

**Files:**
- Create: `migrations/0119_dataapi_service_accounts.sql`
- Modify: `crates/egret-auth/src/users.rs`
- Test: `crates/egret-auth/tests/service_accounts.rs` (create)

**Interfaces:**
- Consumes: the existing `users` table (`migrations/0002_identity.sql:3`), `workspaces` (`migrations/0003_workspace_rbac.sql:1`), `UsersStore` (`crates/egret-auth/src/users.rs:15`).
- Produces: `pub async fn UsersStore::insert_service(&self, email: &str, name: &str) -> Result<UserId>`.

- [ ] **Step 1: Write the migration**

Create `migrations/0119_dataapi_service_accounts.sql`. Match the house style of `migrations/0118_automation_slack.sql`: explain *why*, not just what.

```sql
-- Data API P2: service accounts and their API keys.
--
-- A service account is a REAL row in `users` with provider = 'service'. That is
-- deliberate: `access_grants`, Cedar, `ensure_username` (Ranger/Trino
-- delegation) and audit all key off a user id, so reusing `users` means a
-- machine principal needs no parallel identity plumbing anywhere. The price is
-- that human-facing user LISTS must exclude it — see `UsersStore::list`.
ALTER TABLE users DROP CONSTRAINT users_provider_check;
ALTER TABLE users ADD CONSTRAINT users_provider_check
  CHECK (provider IN ('local','oidc','service'));

-- A service account must never be able to log in interactively. The login
-- handler already requires password_hash IS NOT NULL, but that is one code path
-- away from being wrong forever. Enforce it where it cannot be forgotten.
ALTER TABLE users ADD CONSTRAINT users_service_has_no_password
  CHECK (provider <> 'service' OR password_hash IS NULL);

CREATE SCHEMA IF NOT EXISTS dataapi;

-- The workspace binding lives HERE, not on `users`: `users` is platform-level
-- and every other consumer of it would have to learn to ignore a workspace
-- column that means nothing for humans.
CREATE TABLE dataapi.service_accounts (
  user_id      uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, name)
);
CREATE INDEX service_accounts_ws_idx ON dataapi.service_accounts (workspace_id);

CREATE TABLE dataapi.api_keys (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  service_account_id uuid NOT NULL REFERENCES dataapi.service_accounts(user_id) ON DELETE CASCADE,
  name               text NOT NULL,
  -- Looked up by prefix, so it must be unique and indexed. The secret itself is
  -- never stored — only its SHA-256.
  prefix             text NOT NULL UNIQUE,
  secret_hash        text NOT NULL,
  expires_at         timestamptz,
  last_used_at       timestamptz,
  revoked_at         timestamptz,
  created_by         uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX api_keys_sa_idx ON dataapi.api_keys (service_account_id);
```

Before writing this, confirm the existing constraint's name — `users_provider_check` is Postgres's default for an inline `CHECK` on `provider`, but verify rather than assume:

```bash
psql "$DATABASE_URL" -c "\d users" | grep -i check
```

If the name differs, use the real one. A `DROP CONSTRAINT` naming a constraint that does not exist fails the whole migration.

- [ ] **Step 2: Write the failing test**

Create `crates/egret-auth/tests/service_accounts.rs`. Follow the fixture pattern of the existing tests in `crates/egret-auth/tests/` — open one and copy how it builds a `Db` and a `UsersStore`.

```rust
//! A service account is a user row that can never log in.

#[tokio::test]
async fn service_account_cannot_have_a_password() {
    let db = test_db().await;
    let users = egret_auth::users::UsersStore::new(db.clone());
    let id = users
        .insert_service("svc.reporting@acme.service.egret.local", "reporting")
        .await
        .unwrap();

    // The DB, not the insert path, is what guarantees this.
    let err = sqlx::query("UPDATE users SET password_hash = 'x' WHERE id = $1")
        .bind(id.0)
        .execute(&db.pool)
        .await
        .expect_err("the CHECK must reject a password on a service account");
    assert!(
        err.to_string().contains("users_service_has_no_password"),
        "expected the named CHECK to fire, got: {err}"
    );
}

#[tokio::test]
async fn service_accounts_are_absent_from_the_user_list() {
    let db = test_db().await;
    let users = egret_auth::users::UsersStore::new(db.clone());
    let human = users
        .insert_local("human@acme.test", "human", "hash")
        .await
        .unwrap();
    let svc = users
        .insert_service("svc.x@acme.service.egret.local", "x")
        .await
        .unwrap();

    let listed: Vec<_> = users.list(100, 0).await.unwrap().into_iter().map(|u| u.id).collect();
    assert!(listed.contains(&human), "the human must still be listed");
    assert!(
        !listed.contains(&svc),
        "a service account must not appear in the admin user list"
    );
}

#[tokio::test]
async fn service_account_is_still_findable_by_id() {
    // The filter belongs to ENUMERATION only. Resolving a service account by id
    // must keep working, or its name cannot be rendered next to its own grants
    // and audit rows.
    let db = test_db().await;
    let users = egret_auth::users::UsersStore::new(db.clone());
    let svc = users
        .insert_service("svc.y@acme.service.egret.local", "y")
        .await
        .unwrap();
    assert!(users.find_by_id(svc).await.unwrap().is_some());
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-auth --test service_accounts`

Expected: compile error — `insert_service` does not exist. That is the red state.

- [ ] **Step 4: Add `insert_service`**

In `crates/egret-auth/src/users.rs`, directly after `insert_local` (which ends at line 66), add:

```rust
    /// Insert a service account: a user row that can never log in.
    ///
    /// `password_hash` is deliberately not a parameter — a service account
    /// authenticates by API key, and `users_service_has_no_password` (migration
    /// 0119) rejects a row that has one.
    pub async fn insert_service(&self, email: &str, name: &str) -> Result<UserId> {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (provider, email, name) \
             VALUES ('service', $1, $2) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(UserId(id)),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("user already exists: {email}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }
```

- [ ] **Step 5: Exclude service accounts from `list`**

Still in `crates/egret-auth/src/users.rs`, change `list` (line 67). Only the SQL changes:

```rust
    /// Human users, newest first. Service accounts (`provider = 'service'`) are
    /// EXCLUDED: this feeds the admin Users page and the Access console's
    /// principal picker, where a machine identity is noise at best and gets
    /// mistaken for a person at worst. Resolution by id (`find_by_id`) is
    /// deliberately NOT filtered — a service account's name must still render
    /// next to its own grants and audit rows.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             WHERE provider <> 'service' \
             ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
```

- [ ] **Step 6: Run the tests and verify they pass**

Run: `cargo test -p egret-auth --test service_accounts`

Expected: PASS, 3 tests.

- [ ] **Step 7: Verify the tests are real by mutating the implementation**

Two mutations, run separately:

1. Remove `WHERE provider <> 'service'` from `list`. Expected: `service_accounts_are_absent_from_the_user_list` FAILS.
2. Drop the `users_service_has_no_password` constraint from the migration and re-run against a fresh DB (`just reset-testdata`). Expected: `service_account_cannot_have_a_password` FAILS.

Revert both and confirm green. A test that passes either way is not evidence.

- [ ] **Step 8: Commit**

```bash
git add migrations/0119_dataapi_service_accounts.sql \
        crates/egret-auth/src/users.rs \
        crates/egret-auth/tests/service_accounts.rs
git commit -m "feat(auth): service accounts as non-loginable user rows

A service account is a real users row (provider = 'service') so
access_grants, Cedar, ensure_username and audit need no parallel plumbing
for machine principals. A DB CHECK — not just the login handler — is what
guarantees it can never hold a password.

UsersStore::list now excludes them: it feeds the admin Users page and the
Access console picker. Resolution by id is deliberately unfiltered."
```

---

### Task 2: Keep service accounts out of the human surfaces

**Files:**
- Modify: `crates/egret-controlplane/src/access/mod.rs`
- Test: `crates/egret-controlplane/tests/service_account_surfaces.rs` (create)

**Interfaces:**
- Consumes: `UsersStore::insert_service` and the filtered `list` from Task 1.
- Produces: no new public API — behavioural guarantees only.

Task 1 filtered the one enumeration query. This task closes the routes that would still let an admin operate on a service account *as if it were a person*, and pins the behaviour with tests. The sites were enumerated by hand; the table below is the whole set, and the split matters more than the filtering.

| Site | Kind | Action |
|---|---|---|
| `egret-auth/src/users.rs:67` `list()` | enumeration | filtered in Task 1 |
| `access/mod.rs` `POST /users/{id}/password` | mutation | must 400 on a service account |
| `access/mod.rs` `PATCH /users/{id}` | mutation | must 400 on a service account |
| `egret-workspace/src/access_views.rs:167` | id-keyed `LEFT JOIN` | **leave alone** — renders the grant's principal name |
| `egret-notebook`, `egret-bi`, `egret-kernel`, `egret-automation` joins on `users` | id-keyed | **leave alone** — all render an owner/actor name |

- [ ] **Step 1: Write the failing tests**

Create `crates/egret-controlplane/tests/service_account_surfaces.rs`. Follow the harness in `crates/egret-controlplane/tests/routes_api.rs` — open it and copy how it boots a router and authenticates as a platform admin.

```rust
//! A service account is a users row, so the human-facing user routes must
//! refuse to treat it as a person.

#[tokio::test]
async fn password_reset_rejects_a_service_account() {
    let f = fixture().await;
    let svc = f.create_service_account("reporting").await;

    let res = f
        .admin_post(&format!("/api/control-plane/users/{svc}/password"),
                    serde_json::json!({ "password": "hunter22!" }))
        .await;

    assert_eq!(res.status(), 400, "a service account has no password to reset");
}

#[tokio::test]
async fn profile_update_rejects_a_service_account() {
    let f = fixture().await;
    let svc = f.create_service_account("reporting").await;

    let res = f
        .admin_patch(&format!("/api/control-plane/users/{svc}"),
                     serde_json::json!({ "email": "person@acme.test", "name": "Person" }))
        .await;

    assert_eq!(res.status(), 400, "a service account is not editable as a user");
}

#[tokio::test]
async fn service_account_is_absent_from_the_users_route() {
    let f = fixture().await;
    let svc = f.create_service_account("reporting").await;

    let body: serde_json::Value = f.admin_get("/api/control-plane/users").await.json().await;
    let ids: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&svc.to_string().as_str()),
        "GET /users leaked a service account"
    );
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p egret-controlplane --test service_account_surfaces`

Expected: the two rejection tests FAIL with 200 (the routes currently accept). The list test should already pass thanks to Task 1 — confirm that it does, and that you understand why, before continuing.

- [ ] **Step 3: Reject service accounts in the two mutating handlers**

In `crates/egret-controlplane/src/access/mod.rs`, add this helper next to the other module-level helpers:

```rust
/// The human-facing user routes operate on people. A service account is a
/// `users` row (Data API P2) but has no password and no editable profile, so
/// these routes refuse it rather than half-working.
async fn refuse_service_account(st: &CpState, id: UserId) -> Result<(), CpError> {
    let u = st
        .users
        .find_by_id(id)
        .await?
        .ok_or_else(|| CpError::NotFound("user not found".into()))?;
    if u.provider == "service" {
        return Err(CpError::BadRequest(
            "this is a service account; manage it through the Data API service-account routes"
                .into(),
        ));
    }
    Ok(())
}
```

Call it as the first line after the authz gate in both `reset_password` and `update_user`:

```rust
    refuse_service_account(&st, UserId(id)).await?;
```

Check `CpError`'s actual variant names in `crates/egret-controlplane/src/error.rs` and use the real ones — `NotFound`/`BadRequest` above are the expected shape, not a guess you should keep if the enum differs.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p egret-controlplane --test service_account_surfaces`

Expected: PASS, 3 tests.

- [ ] **Step 5: Verify the tests are real by mutating the implementation**

Comment out the `refuse_service_account` call in `reset_password`. Expected: `password_reset_rejects_a_service_account` FAILS. Revert, confirm green.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/src/access/mod.rs \
        crates/egret-controlplane/tests/service_account_surfaces.rs
git commit -m "feat(controlplane): refuse service accounts on the human user routes

A service account is a users row, so PATCH /users/{id} and POST
/users/{id}/password would half-work on one. They now 400 and point at the
Data API routes. Id-keyed joins that render a principal's name are
deliberately left unfiltered."
```

---

### Task 3: API key mint, verify, revoke

**Files:**
- Create: `crates/egret-dataapi-api/Cargo.toml`, `crates/egret-dataapi-api/src/lib.rs`
- Create: `crates/egret-dataapi/Cargo.toml`, `crates/egret-dataapi/src/lib.rs`, `crates/egret-dataapi/src/service_accounts.rs`, `crates/egret-dataapi/src/keys.rs`
- Modify: `Cargo.toml` (workspace members)
- Test: `crates/egret-dataapi/src/keys.rs` (inline `#[cfg(test)] mod tests`) and `crates/egret-dataapi/tests/keys_store.rs` (create)

**Interfaces:**
- Consumes: `dataapi.service_accounts` and `dataapi.api_keys` from Task 1; `egret_metastore::Db`; `egret_core::Error`.
- Produces:
  - `pub struct MintedKey { pub id: Uuid, pub secret: String }` — `secret` is the full `egt_<prefix>_<secret>` token, returned once.
  - `pub fn keys::format_key(prefix: &str, secret: &str) -> String`
  - `pub fn keys::split_key(key: &str) -> Option<(&str, &str)>`
  - `pub fn keys::hash_secret(secret: &str) -> String`
  - `pub fn keys::hash_matches(computed: &str, stored: &str) -> bool`
  - `pub const keys::PREFIX_LEN: usize = 12`
  - `pub async fn KeyStore::mint(&self, service_account_id: Uuid, name: &str, expires_at: Option<OffsetDateTime>, by: UserId) -> Result<MintedKey>`
  - `pub async fn KeyStore::verify(&self, presented: &str) -> Result<Option<VerifiedKey>>` where `VerifiedKey { key_id: Uuid, service_account_id: Uuid, workspace_id: Uuid }`
  - `pub async fn KeyStore::revoke(&self, id: Uuid) -> Result<()>`

- [ ] **Step 1: Create the two crates**

`crates/egret-dataapi-api/Cargo.toml` — a pure leaf, mirroring `crates/egret-query-api/Cargo.toml`'s dependency list (`egret-core`, `serde`, `uuid`, `utoipa`). `crates/egret-dataapi/Cargo.toml` — mirror `crates/egret-query/Cargo.toml`'s shape and add `sha2` and `subtle` (for constant-time comparison) plus `egret-dataapi-api`. Add both to the workspace `members` list in the root `Cargo.toml`, keeping it in the file's existing order.

Check whether `sha2` and `subtle` are already in the workspace dependency table; if so use `.workspace = true`, matching how sibling crates declare deps.

- [ ] **Step 2: Write the failing tests**

In `crates/egret-dataapi/src/keys.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips_through_format_and_split() {
        let k = format_key("abc123def456", "s3cr3t");
        assert_eq!(k, "egt_abc123def456_s3cr3t");
        assert_eq!(split_key(&k), Some(("abc123def456", "s3cr3t")));
    }

    #[test]
    fn split_rejects_malformed_keys() {
        assert_eq!(split_key("abc123def456_s3cr3t"), None, "missing egt_ prefix");
        assert_eq!(split_key("egt_onlyprefix"), None, "no secret segment");
        assert_eq!(split_key(""), None);
        assert_eq!(split_key("egt__s3cr3t"), None, "empty prefix");
        assert_eq!(split_key("egt_abc123def456_"), None, "empty secret");
    }

    #[test]
    fn a_secret_containing_underscores_survives_the_split() {
        // The secret is base64url, which never contains '_'... except that
        // base64URL's alphabet DOES include '_'. Splitting on the LAST
        // underscore would corrupt it; splitting on the first two is what makes
        // this safe.
        let k = format_key("abc123def456", "aa_bb_cc");
        assert_eq!(split_key(&k), Some(("abc123def456", "aa_bb_cc")));
    }

    #[test]
    fn hash_is_stable_and_differs_per_secret() {
        assert_eq!(hash_secret("a"), hash_secret("a"));
        assert_ne!(hash_secret("a"), hash_secret("b"));
        // SHA-256 hex is 64 chars — a length change means the algorithm changed
        // and every stored hash just became unverifiable.
        assert_eq!(hash_secret("a").len(), 64);
    }
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-dataapi --lib keys`

Expected: compile error — the functions do not exist.

- [ ] **Step 4: Implement the key primitives**

In `crates/egret-dataapi/src/keys.rs`:

```rust
//! API key material: format, hash, verify. The ONLY place a key secret is
//! handled.
//!
//! Keys are hashed with SHA-256, deliberately NOT with
//! `egret_auth::password`'s Argon2. Argon2 is right for a password a human
//! chose, because it must survive brute force. A key secret is 32 bytes of
//! CSPRNG output, so brute force is already infeasible — and paying Argon2's
//! ~50-100 ms on EVERY API request would make the serving surface unusable.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// The prefix length. Long enough to make a lookup collision implausible,
/// short enough to display in a key list.
pub const PREFIX_LEN: usize = 12;

/// Assemble the token a caller presents: `egt_<prefix>_<secret>`.
pub fn format_key(prefix: &str, secret: &str) -> String {
    format!("egt_{prefix}_{secret}")
}

/// Split a presented token into `(prefix, secret)`. Returns `None` for anything
/// malformed — the caller must treat that as an authentication failure, not as
/// a reason to look anything up.
///
/// Splits on the first two separators only: the secret is base64url, whose
/// alphabet includes `_`, so splitting on the last separator would truncate it.
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("egt_")?;
    let (prefix, secret) = rest.split_once('_')?;
    if prefix.is_empty() || secret.is_empty() {
        return None;
    }
    Some((prefix, secret))
}

/// SHA-256 of the secret, lowercase hex.
pub fn hash_secret(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    format!("{:x}", h.finalize())
}

/// Constant-time comparison of a computed hash against a stored one.
pub fn hash_matches(computed: &str, stored: &str) -> bool {
    computed.as_bytes().ct_eq(stored.as_bytes()).into()
}
```

- [ ] **Step 5: Run the tests and verify they pass**

Run: `cargo test -p egret-dataapi --lib keys`

Expected: PASS, 4 tests.

- [ ] **Step 6: Implement mint / verify / revoke**

Add to the same file. Follow `crates/egret-query/src/registry/repo.rs` for sqlx error-mapping idiom.

```rust
use egret_core::{Error, Result, UserId};
use egret_metastore::Db;
use time::OffsetDateTime;
use uuid::Uuid;

/// A freshly minted key. `secret` is the assembled `egt_…` token and is the ONLY
/// time it exists outside the caller's hands — it is not recoverable.
pub struct MintedKey {
    pub id: Uuid,
    pub secret: String,
}

/// What a valid key resolves to. Carries the workspace so the serving surface
/// (P4) can do its path-workspace check without a second query.
pub struct VerifiedKey {
    pub key_id: Uuid,
    pub service_account_id: Uuid,
    pub workspace_id: Uuid,
}

#[derive(Clone)]
pub struct KeyStore {
    db: Db,
}

impl KeyStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn mint(
        &self,
        service_account_id: Uuid,
        name: &str,
        expires_at: Option<OffsetDateTime>,
        by: UserId,
    ) -> Result<MintedKey> {
        use base64::Engine as _;
        use rand::RngCore;

        let mut sb = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut sb);
        let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sb);

        let mut pb = [0u8; 9]; // 9 bytes -> 12 base64url chars, = PREFIX_LEN
        rand::thread_rng().fill_bytes(&mut pb);
        let prefix = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pb);
        debug_assert_eq!(prefix.len(), PREFIX_LEN);

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.api_keys \
               (service_account_id, name, prefix, secret_hash, expires_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(service_account_id)
        .bind(name)
        .bind(&prefix)
        .bind(hash_secret(&secret))
        .bind(expires_at)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("mint api key: {e}")))?;

        Ok(MintedKey {
            id,
            secret: format_key(&prefix, &secret),
        })
    }

    /// Resolve a presented token. Returns `Ok(None)` for every failure mode —
    /// malformed, unknown, revoked, expired, or wrong secret — so a caller
    /// cannot tell them apart. Never return a distinguishing error here.
    pub async fn verify(&self, presented: &str) -> Result<Option<VerifiedKey>> {
        let Some((prefix, secret)) = split_key(presented) else {
            return Ok(None);
        };
        let row: Option<(Uuid, Uuid, Uuid, String, Option<OffsetDateTime>, Option<OffsetDateTime>)> =
            sqlx::query_as(
                "SELECT k.id, k.service_account_id, sa.workspace_id, k.secret_hash, \
                        k.expires_at, k.revoked_at \
                 FROM dataapi.api_keys k \
                 JOIN dataapi.service_accounts sa ON sa.user_id = k.service_account_id \
                 WHERE k.prefix = $1",
            )
            .bind(prefix)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("verify api key: {e}")))?;

        let Some((key_id, sa, ws, stored, expires_at, revoked_at)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(exp) = expires_at {
            if exp <= OffsetDateTime::now_utc() {
                return Ok(None);
            }
        }
        if !hash_matches(&hash_secret(secret), &stored) {
            return Ok(None);
        }
        Ok(Some(VerifiedKey {
            key_id,
            service_account_id: sa,
            workspace_id: ws,
        }))
    }

    pub async fn revoke(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE dataapi.api_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("revoke api key: {e}")))?;
        Ok(())
    }
}
```

Note `verify` returns `Ok(None)` for **every** failure. Returning a distinguishing error would let a caller separate "unknown prefix" from "bad secret", which is a free oracle for anyone probing.

- [ ] **Step 7: Write and run the store integration test**

Create `crates/egret-dataapi/tests/keys_store.rs`. Build the `Db` the way the other integration tests in this repo do — copy the fixture shape from `crates/egret-query/tests/registry_repo.rs`.

```rust
//! Every failure mode of `verify` must be indistinguishable from outside.

#[tokio::test]
async fn a_minted_key_verifies() {
    let f = fixture().await;
    let minted = f.keys.mint(f.sa, "k", None, f.admin).await.unwrap();
    let v = f.keys.verify(&minted.secret).await.unwrap().expect("must verify");
    assert_eq!(v.service_account_id, f.sa);
    assert_eq!(v.workspace_id, f.ws);
    assert_eq!(v.key_id, minted.id);
}

#[tokio::test]
async fn a_revoked_key_does_not_verify() {
    let f = fixture().await;
    let minted = f.keys.mint(f.sa, "k", None, f.admin).await.unwrap();
    f.keys.revoke(minted.id).await.unwrap();
    assert!(f.keys.verify(&minted.secret).await.unwrap().is_none());
}

#[tokio::test]
async fn an_expired_key_does_not_verify() {
    let f = fixture().await;
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let minted = f.keys.mint(f.sa, "k", Some(past), f.admin).await.unwrap();
    assert!(f.keys.verify(&minted.secret).await.unwrap().is_none());
}

#[tokio::test]
async fn a_tampered_secret_does_not_verify() {
    let f = fixture().await;
    let minted = f.keys.mint(f.sa, "k", None, f.admin).await.unwrap();
    let (prefix, secret) = egret_dataapi::keys::split_key(&minted.secret).unwrap();
    // Same valid prefix, one character changed in the secret.
    let mut bad = secret.to_string();
    bad.replace_range(0..1, if secret.starts_with('A') { "B" } else { "A" });
    let tampered = egret_dataapi::keys::format_key(prefix, &bad);
    assert!(f.keys.verify(&tampered).await.unwrap().is_none());
}

#[tokio::test]
async fn malformed_tokens_do_not_verify() {
    let f = fixture().await;
    for bad in ["", "egt_", "egt_abc", "not-a-key", "egt__x"] {
        assert!(
            f.keys.verify(bad).await.unwrap().is_none(),
            "malformed token accepted: {bad:?}"
        );
    }
}

#[tokio::test]
async fn deleting_the_service_account_removes_its_keys() {
    let f = fixture().await;
    let minted = f.keys.mint(f.sa, "k", None, f.admin).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.sa)
        .execute(&f.db.pool)
        .await
        .unwrap();
    // ON DELETE CASCADE reaches api_keys through service_accounts.
    assert!(f.keys.verify(&minted.secret).await.unwrap().is_none());
}
```

Run: `cargo test -p egret-dataapi`

Expected: PASS, 6 tests.

- [ ] **Step 8: Verify the tests are real by mutating the implementation**

Make `verify` skip the `revoked_at` check. Expected: the revoked-key test FAILS. Then make it compare with `==` instead of `hash_matches` — the tests should still pass (constant-time comparison is not observable in a unit test), which is worth noticing: **that property is not test-enforceable here**, so it is protected by the code comment and review, not by the suite. Note this in your report rather than inventing a timing test that would be flaky.

Revert both.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-dataapi-api crates/egret-dataapi Cargo.toml
git commit -m "feat(dataapi): service-account store and API key mint/verify/revoke

Keys are egt_<prefix>_<secret>: looked up by an indexed prefix, verified
by constant-time comparison against a stored SHA-256. Not Argon2 — the
secret is 32 CSPRNG bytes, so a brute-force-resistant KDF buys nothing
and would cost ~50-100ms on every API request.

split_key splits on the first two separators because base64url includes
'_' in its alphabet."
```

---

### Task 4: Management routes and permissions

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`
- Create: `crates/egret-dataapi/src/routes.rs`
- Modify: `crates/egret-dataapi/src/lib.rs`, `crates/egret-server/src/lib.rs`
- Test: `crates/egret-dataapi/tests/routes_authz.rs` (create)

**Interfaces:**
- Consumes: `KeyStore` and the service-account store from Task 3.
- Produces: `pub fn egret_dataapi::management_router() -> Router<DataApiState>` and `pub fn egret_dataapi::openapi() -> utoipa::openapi::OpenApi`.

Routes, all nested under `/api` and workspace-scoped:

```text
GET    /api/ws/{ws}/data-api/service-accounts
POST   /api/ws/{ws}/data-api/service-accounts
DELETE /api/ws/{ws}/data-api/service-accounts/{id}
GET    /api/ws/{ws}/data-api/service-accounts/{id}/keys
POST   /api/ws/{ws}/data-api/service-accounts/{id}/keys      -> returns the secret ONCE
DELETE /api/ws/{ws}/data-api/keys/{id}
```

- [ ] **Step 1: Add the permissions**

In `crates/egret-core/src/rbac.rs`, next to the existing `secret:*` block (around line 130), add two entries in the same `perm!` form:

```rust
    perm!(
        "service_account:read",
        "Data API",
        "View service accounts and their keys (never the secret)",
        Scope::Workspace
    ),
    perm!(
        "service_account:manage",
        "Data API",
        "Create and delete service accounts and mint their API keys",
        Scope::Workspace
    ),
```

Confirm `Scope::Workspace` is the real variant name by reading the enum; the `secret:*` entries use `Scope::Platform`, so the workspace variant's exact spelling must be checked, not assumed.

- [ ] **Step 2: Write the failing authz tests**

Create `crates/egret-dataapi/tests/routes_authz.rs`. These are the tests that matter most in this task — mint-a-key is a privilege-granting operation.

```rust
//! Minting a key for a service account hands out that account's access. These
//! tests are written from the caller's side: a real non-admin principal, not a
//! mocked decision.

#[tokio::test]
async fn read_permission_cannot_mint_a_key() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;
    // A principal with service_account:read and nothing else.
    let reader = f.principal_with(&["service_account:read"]).await;

    let res = f.post_as(reader, &format!("/api/ws/{}/data-api/service-accounts/{sa}/keys", f.ws),
                        serde_json::json!({ "name": "k" })).await;

    assert_eq!(res.status(), 403, "read must not imply mint");
}

#[tokio::test]
async fn a_key_cannot_be_minted_across_workspaces() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;   // in f.ws
    let other_ws = f.create_workspace("other").await;
    let manager = f.principal_with_in(&["service_account:manage"], other_ws).await;

    let res = f.post_as(manager, &format!("/api/ws/{other_ws}/data-api/service-accounts/{sa}/keys"),
                        serde_json::json!({ "name": "k" })).await;

    assert_eq!(res.status(), 404, "the account does not belong to that workspace");
}

#[tokio::test]
async fn the_secret_is_returned_once_and_never_again() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;
    let manager = f.principal_with(&["service_account:manage"]).await;

    let created: serde_json::Value = f
        .post_as(manager, &format!("/api/ws/{}/data-api/service-accounts/{sa}/keys", f.ws),
                 serde_json::json!({ "name": "k" }))
        .await.json().await;
    assert!(created["secret"].as_str().unwrap().starts_with("egt_"));

    let listed: serde_json::Value = f
        .get_as(manager, &format!("/api/ws/{}/data-api/service-accounts/{sa}/keys", f.ws))
        .await.json().await;
    assert!(
        listed[0].get("secret").is_none(),
        "the key list must never carry secret material"
    );
}
```

The cross-workspace test asserts **404, not 403** — consistent with spec §6.2 step 3: a caller must not be able to distinguish "no such account" from "exists elsewhere".

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-dataapi --test routes_authz`

Expected: compile error — the router does not exist yet.

- [ ] **Step 4: Implement the router**

Write `crates/egret-dataapi/src/routes.rs` following the handler idiom of `crates/egret-controlplane/src/access/mod.rs`: `State(st)`, `Auth(p)`, a `require(...)` gate as the first statement, `utoipa::path` annotations on every handler.

Two DTOs, and the distinction between them is the whole point:

```rust
/// What a key list returns. There is deliberately NO `secret` field — not a
/// skipped one, not an Option. A field that cannot exist cannot leak through a
/// future serde attribute change.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ApiKeyDto {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// The mint response, and the only type in the crate that carries key material.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MintedKeyDto {
    pub id: Uuid,
    /// The full `egt_…` token. Shown once; not recoverable afterwards.
    pub secret: String,
}
```

The scoping helper every handler calls first, which is what stops the IDOR:

```rust
/// Resolve a service account and confirm it belongs to the workspace in the
/// path. Returns 404 — never 403 — on mismatch, so a caller in another
/// workspace cannot distinguish "no such account" from "not yours".
async fn scoped_account(st: &DataApiState, ws: Uuid, sa: Uuid) -> Result<Uuid, DataApiError> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM dataapi.service_accounts WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(sa)
    .bind(ws)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(|e| DataApiError::Internal(anyhow::anyhow!("{e}")))?;
    found.ok_or(DataApiError::NotFound)
}
```

The mint handler, which is the privileged one:

```rust
async fn mint_key(
    State(st): State<DataApiState>,
    Auth(p): Auth,
    Path((ws, sa)): Path<(Uuid, Uuid)>,
    Json(body): Json<MintKeyInput>,
) -> Result<(StatusCode, Json<MintedKeyDto>), DataApiError> {
    // `:manage`, never `:read` — minting a key hands out this account's access.
    egret_core::authz::require_ws(&*st.pdp, &p, ws, "service_account:manage").await?;
    let sa = scoped_account(&st, ws, sa).await?;
    let minted = st.keys.mint(sa, &body.name, body.expires_at, p.user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(MintedKeyDto { id: minted.id, secret: minted.secret }),
    ))
}
```

Check the real name and signature of the workspace-scoped authz helper before using `require_ws` — read how an existing workspace-scoped route in `crates/egret-bi/src/routes.rs` gates, and use that, rather than keeping the name above if it differs.

- [ ] **Step 5: Mount it**

In `crates/egret-server/src/lib.rs`, add one `doc.merge(egret_dataapi::openapi());` line to the explicit merge list (the file's note at line 90 says do NOT loop) and nest the router alongside the other workspace-scoped groups.

- [ ] **Step 6: Run the tests and verify they pass**

Run: `cargo test -p egret-dataapi`

Expected: PASS.

- [ ] **Step 7: Verify each authz gate by mutation**

For each of the three tests, remove the guard it targets and confirm that test — and ideally only that test — goes red:

1. Drop the `require(..., "service_account:manage")` gate on the mint handler → `read_permission_cannot_mint_a_key` FAILS.
2. Drop the workspace-match check → `a_key_cannot_be_minted_across_workspaces` FAILS.
3. Add `secret` to the list DTO → `the_secret_is_returned_once_and_never_again` FAILS.

Revert each. A gate whose removal leaves the suite green is not a gate.

- [ ] **Step 8: Full scoped check**

```bash
cargo fmt --all --check
cargo clippy -p egret-dataapi -p egret-dataapi-api -p egret-auth -p egret-controlplane -p egret-core --all-targets
cargo test -p egret-dataapi -p egret-dataapi-api -p egret-auth -p egret-controlplane -p egret-core
```

Run sequentially. Do not use `just check`.

- [ ] **Step 9: Update the spec and commit**

`docs/en/specs/2026-08-14-data-api-design.md` §5.1 currently shows `dataapi.api_keys.service_account_id` referencing `users.id` directly and has no `service_accounts` table. Update it to match what was built, and mirror the change into `docs/vi/specs/2026-08-14-data-api-design.md`. Explain the reason in the spec text: `users` is platform-level, so the workspace binding belongs to the `dataapi` schema rather than to a column on `users` that means nothing for humans.

```bash
git add crates/ docs/ Cargo.toml
git commit -m "feat(dataapi): workspace-scoped service-account and key routes

Mint gates on service_account:manage, never on :read — minting a key
hands out that account's access. Every handler checks the account belongs
to the path workspace and 404s otherwise, so a cross-workspace caller
cannot distinguish 'absent' from 'not yours'.

The key-list DTO has no secret field at all, rather than a skipped one."
```

---

## Done when

- `cargo test -p egret-dataapi -p egret-dataapi-api -p egret-auth -p egret-controlplane` green, `cargo fmt --all --check` and clippy clean.
- Every test named in Tasks 1, 2, 3 and 4 has been observed FAILING against its stated mutation. This is the acceptance criterion — green alone does not close this out.
- An admin can create a service account, mint a key, and `KeyStore::verify` accepts it; revoking makes verification fail.
- A service account cannot log in, does not appear in `GET /api/control-plane/users`, and still renders by id wherever a principal name is shown.
- The spec's §5.1 matches the schema that was actually built.

## Not in this plan

- **Any serving surface.** No endpoint accepts a key yet; P2 ends at `KeyStore::verify` returning a `VerifiedKey`. The `/data-api/v1` router is P4.
- **Endpoint definitions, versioning, publish** — P3.
- **Granting a service account access to data.** P2 creates the principal; granting it a role through `access_grants`, and granting it in Ranger, are existing flows. Spec §5.3's warning that publishing does not grant data access is surfaced in the UI in P5.
- **Key rotation as a first-class operation.** Mint-new-then-revoke-old covers it; a dedicated rotate endpoint can wait for a real request.
- **Frontend.** P5.
