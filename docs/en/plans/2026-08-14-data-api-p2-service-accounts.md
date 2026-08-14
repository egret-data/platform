# Data API P2 — Service accounts and OAuth2 clients Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-14-data-api-design.md` §5, §6.1
> Part: P2 of 5 — see `docs/en/plans/2026-08-14-data-api-decomposition.md`
> Stacked on: P1 (`fix/dialect-escaping`, PR #504). Branch off P1, do not re-include its code.

**Goal:** an admin can create a workspace-scoped service account and an OAuth2 client for it; that client can exchange `client_id`/`client_secret` for a short-lived JWT at the existing `/oauth/token` endpoint.

**Architecture:** a service account is a real row in `users` with `provider = 'service'`, so `access_grants`, Cedar, `ensure_username` (Ranger/Trino delegation) and audit need no parallel plumbing for machine principals. Credentials are **not** a bespoke API-key scheme: egret already runs an OAuth2 `client_credentials` machine-IdP (`crates/egret-machine-idp`) serving Catalog Clients, and this plan generalises it to serve a second audience rather than standing up a second credential system beside it.

**Tech Stack:** Rust (axum, sqlx) · Postgres · JWT (existing `SigningKeyStore` + JWKS) · Argon2 · Cedar PDP.

## Why OAuth2 and not API keys

An earlier draft of this plan invented `dataapi.api_keys`: a bearer key looked up in Postgres on every request. Three facts killed it.

1. **It duplicates working infrastructure.** `crates/egret-machine-idp/src/lib.rs` already serves `POST /oauth/token` (`grant_type=client_credentials`), OIDC discovery, and JWKS. `catalog_clients` (`migrations/0053_embedded_catalog_rbac.sql:30`) already stores `workspace_id`, `client_id`, `client_secret_hash`, `last_used_at` — nearly the table the draft proposed. Rejecting sqlrest's `AppClient` for creating a second identity system, then creating a second *credential* system, is the same mistake one layer down.
2. **Per-request DB lookup on the hot path.** Key verification meant a `SELECT` per API call. A JWT is verified from JWKS in-process, with no DB round-trip — which matters for a data API under load.
3. **The hashing argument dissolves.** The draft argued for SHA-256 over Argon2 because ~50–100 ms of KDF per API request is unacceptable. True — but under OAuth2 the secret is verified once, at token issuance, not per request. So this plan uses **Argon2, matching `CatalogStore::verify_client`** (`crates/egret-catalog-engine/src/store/embedded.rs:384`), and the inconsistency disappears.

**The trade-off, stated plainly:** revocation is no longer instant. A revoked client's already-issued token stays valid until `exp`. That is the price of stateless verification — the same property that removes the DB hit is what defers revocation. This plan mitigates it with a short TTL (**600s** for Data API) and does **not** add a per-request revocation check, because doing so would give back the throughput the design was chosen for.

## Global Constraints

- **The catalog's tokens must not change.** `machine-idp` is generalised, not rewritten. A Catalog Client's token must keep `aud: "catalog"`, its `warehouse` and `roles` claims, and its **3600s** TTL, byte-for-byte. Task 3 pins this with a regression test before touching anything.
- A service account **must never be able to log in interactively**. The login handler already requires `password_hash.is_some()` (`crates/egret-auth/src/routes.rs:184`), but that is safety by accident — this plan adds a DB-level CHECK so it holds even if a future insert path forgets.
- **Filter enumeration, never id-keyed joins.** A service account in the admin user list is a defect; a service account's name beside its own grant or audit row is correct. Task 2 lists which is which. Do not "helpfully" filter a join.
- Client secrets are hashed with **Argon2**, matching the catalog. The secret is returned **exactly once**, at creation.
- Follow `crates/egret-controlplane/src/access/mod.rs`'s authz idiom: reads and lifecycle gate on a delegatable `require(...)`, but anything that mutates the authorization graph gates on platform admin. Creating a client for a service account that already holds grants is such a mutation — see Task 5.
- Run scoped checks, not `just check` (it runs `cargo test --workspace`). `egret-config::tests::toml_file_is_merged` is a known pre-existing flake.
- Never run two workspace test suites concurrently.

## File Structure

| File | Responsibility |
|---|---|
| `migrations/0119_dataapi_service_accounts.sql` (create) | `users.provider` CHECK extension + the service/no-password CHECK; `dataapi` schema; `service_accounts` and `clients` tables. |
| `crates/egret-auth/src/users.rs` (modify) | `insert_service`; `list` excludes service accounts. |
| `crates/egret-controlplane/src/access/mod.rs` (modify) | Refuse service accounts on the human user routes. |
| `crates/egret-machine-idp/src/lib.rs` (modify) | `VerifiedClient` carries audience + claims + TTL; `IdpState` holds several verifiers. |
| `crates/egret-catalog-engine/src/store/embedded.rs` (modify) | Its `ClientVerifier` impl fills in the new fields, producing an unchanged catalog token. |
| `crates/egret-bin/src/main.rs` (modify) | Register both verifiers on `IdpState`. |
| `crates/egret-dataapi-api/src/lib.rs` (create) | Pure-serde DTOs. |
| `crates/egret-dataapi/src/clients.rs` (create) | Client store + the Data API `ClientVerifier` impl. |
| `crates/egret-dataapi/src/routes.rs` (create) | `management_router()` — workspace-scoped admin surface. |
| `crates/egret-core/src/rbac.rs` (modify) | Two new permissions. |
| `crates/egret-server/src/lib.rs` (modify) | Mount the router; merge the OpenAPI. |
| `docs/en/specs/...`, `docs/vi/specs/...` (modify) | §5.1 and §6.1 rewritten for OAuth2. |

---

### Task 1: Schema and the service-account row

**Files:**
- Create: `migrations/0119_dataapi_service_accounts.sql`
- Modify: `crates/egret-auth/src/users.rs`
- Test: `crates/egret-auth/tests/service_accounts.rs` (create)

**Interfaces:**
- Consumes: `users` (`migrations/0002_identity.sql:3`), `workspaces` (`migrations/0003_workspace_rbac.sql:1`), `UserRepo` (`crates/egret-auth/src/users.rs:15`).
- Produces: `pub async fn UserRepo::insert_service(&self, email: &str, name: &str) -> Result<UserId>`.

- [ ] **Step 1: Write the migration**

Create `migrations/0119_dataapi_service_accounts.sql`, matching the house style of `migrations/0118_automation_slack.sql` — explain *why*, not just what.

```sql
-- Data API P2: service accounts and their OAuth2 clients.
--
-- A service account is a REAL row in `users` with provider = 'service'. That is
-- deliberate: `access_grants`, Cedar, `ensure_username` (Ranger/Trino
-- delegation) and audit all key off a user id, so reusing `users` means a
-- machine principal needs no parallel identity plumbing anywhere. The price is
-- that human-facing user LISTS must exclude it — see `UserRepo::list`.
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

-- OAuth2 client_credentials clients, deliberately shaped like
-- `catalog.catalog_clients` (0053) — same idea, same machine-IdP, second
-- audience. N clients per service account, so rotation is mint-new-then-revoke-old.
CREATE TABLE dataapi.clients (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  service_account_id uuid NOT NULL REFERENCES dataapi.service_accounts(user_id) ON DELETE CASCADE,
  name               text NOT NULL,
  client_id          text NOT NULL UNIQUE,
  -- Argon2, matching CatalogStore::verify_client. Affordable because it is
  -- checked once per TOKEN, not once per API request.
  client_secret_hash text NOT NULL,
  expires_at         timestamptz,
  revoked_at         timestamptz,
  last_used_at       timestamptz,
  created_by         uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX dataapi_clients_sa_idx ON dataapi.clients (service_account_id);
CREATE INDEX dataapi_clients_client_id_idx ON dataapi.clients (client_id);
```

Before writing this, confirm the existing constraint's name — `users_provider_check` is Postgres's default for an inline `CHECK`, but verify rather than assume:

```bash
psql "$DATABASE_URL" -c "\d users" | grep -i check
```

A `DROP CONSTRAINT` naming a constraint that does not exist fails the whole migration.

- [ ] **Step 2: Write the failing tests**

Create `crates/egret-auth/tests/service_accounts.rs`. Follow the fixture pattern of the existing tests in `crates/egret-auth/tests/` — open one and copy how it builds a `Db` and a `UserRepo`.

```rust
//! A service account is a user row that can never log in.

#[tokio::test]
async fn service_account_cannot_have_a_password() {
    let db = test_db().await;
    let users = egret_auth::users::UserRepo::new(db.clone());
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
    let users = egret_auth::users::UserRepo::new(db.clone());
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
    let users = egret_auth::users::UserRepo::new(db.clone());
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

In `crates/egret-auth/src/users.rs`, directly after `insert_local` (which ends at line 66):

```rust
    /// Insert a service account: a user row that can never log in.
    ///
    /// `password_hash` is deliberately not a parameter — a service account
    /// authenticates via an OAuth2 client, and `users_service_has_no_password`
    /// (migration 0119) rejects a row that has one.
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

Change `list` (line 67). Only the SQL and the doc comment change:

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
2. Drop the `users_service_has_no_password` constraint from the migration and re-run against a **genuinely fresh database**. Expected: `service_account_cannot_have_a_password` FAILS.

   `just reset-testdata` is NOT sufficient — it wipes data but does not replay migrations, so an already-applied constraint survives it and the mutation silently does nothing. Create a throwaway database and point `DATABASE_URL` at it.

   Also note: `sqlx::migrate!` embeds the migration directory at compile time, so editing a `.sql` file does not necessarily rebuild it. Force a re-embed with `touch crates/egret-metastore/src/lib.rs` before re-running, or you will be testing the previous migration set.

Revert both and confirm green.

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

UserRepo::list now excludes them: it feeds the admin Users page and the
Access console picker. Resolution by id is deliberately unfiltered."
```

---

### Task 2: Keep service accounts out of the human surfaces

**Files:**
- Modify: `crates/egret-controlplane/src/access/mod.rs`
- Test: `crates/egret-controlplane/tests/service_account_surfaces.rs` (create)

**Interfaces:**
- Consumes: `UserRepo::insert_service` and the filtered `list` from Task 1.
- Produces: no new public API — behavioural guarantees only.

The sites were enumerated by hand. This table is the whole set, and the split matters more than the filtering.

| Site | Kind | Action |
|---|---|---|
| `egret-auth/src/users.rs:67` `list()` | enumeration | filtered in Task 1 |
| `access/mod.rs` `POST /users/{id}/password` | mutation | must 400 |
| `access/mod.rs` `PATCH /users/{id}` | mutation | must 400 |
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

Expected: the two rejection tests FAIL with 200. The list test should already pass thanks to Task 1 — confirm it does, and that you understand why, before continuing.

- [ ] **Step 3: Reject service accounts in the two mutating handlers**

In `crates/egret-controlplane/src/access/mod.rs`, add next to the other module-level helpers:

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

Check `CpError`'s actual variant names in `crates/egret-controlplane/src/error.rs` and use the real ones — `NotFound`/`BadRequest` above are the expected shape, not a guess to keep if the enum differs.

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

### Task 3: Generalise the machine-IdP to a second audience

**Files:**
- Modify: `crates/egret-machine-idp/src/lib.rs`
- Modify: `crates/egret-catalog-engine/src/store/embedded.rs` (its `ClientVerifier` impl, line 403)
- Modify: `crates/egret-bin/src/main.rs` (`IdpState` construction, line 1759)
- Test: `crates/egret-machine-idp/src/lib.rs` (inline tests — a `FakeVerifier` already exists at line 300)

**Interfaces:**
- Consumes: the existing `ClientVerifier` trait, `IdpState`, `SigningKeyStore`.
- Produces:
  - `pub struct VerifiedClient { pub client_id: String, pub audience: String, pub extra_claims: serde_json::Map<String, serde_json::Value>, pub ttl_secs: u64 }` — **breaking change** to the struct.
  - `IdpState.verifiers: Vec<Arc<dyn ClientVerifier>>` — replaces the single `clients` field.

This is the riskiest task in the plan: it touches a crate the catalog depends on. Write the regression test first, and do not proceed until it is green against unmodified code.

- [ ] **Step 1: Pin the catalog's current token shape**

Before changing anything, add a test that captures exactly what a Catalog Client's token contains today. `crates/egret-machine-idp/src/lib.rs:300` already has a `FakeVerifier` — extend the existing inline test module:

```rust
    /// The catalog's token shape is a published contract with every Iceberg
    /// client already deployed. This test exists to fail loudly if the
    /// multi-audience refactor changes it.
    #[tokio::test]
    async fn catalog_token_claims_are_unchanged() {
        let st = test_idp_state_with_catalog_verifier().await;
        let resp = post_token(&st, "grant_type=client_credentials&client_id=cat1&client_secret=s").await;

        let claims = decode_claims_without_verifying(&resp.access_token);
        assert_eq!(claims["aud"], "catalog");
        assert_eq!(claims["sub"], "cat1");
        assert_eq!(claims["warehouse"], "wh1");
        assert_eq!(claims["roles"], serde_json::json!(["role1"]));
        assert_eq!(resp.expires_in, 3600, "catalog TTL must stay 3600");
    }
```

Run it against unmodified code. Expected: PASS. If it does not pass, your fixture is wrong — fix it before touching production code, or the regression test is worthless.

- [ ] **Step 2: Write the failing multi-audience test**

```rust
    #[tokio::test]
    async fn a_second_verifier_serves_its_own_audience_and_ttl() {
        // Two verifiers registered; each client resolves through exactly one.
        let st = test_idp_state_with_two_verifiers().await;

        let cat = post_token(&st, "grant_type=client_credentials&client_id=cat1&client_secret=s").await;
        assert_eq!(decode_claims_without_verifying(&cat.access_token)["aud"], "catalog");
        assert_eq!(cat.expires_in, 3600);

        let api = post_token(&st, "grant_type=client_credentials&client_id=api1&client_secret=s").await;
        let c = decode_claims_without_verifying(&api.access_token);
        assert_eq!(c["aud"], "data-api");
        assert_eq!(c["ws"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(api.expires_in, 600, "Data API TTL is short by design");
        assert!(c.get("warehouse").is_none(), "catalog claims must not leak in");
    }

    #[tokio::test]
    async fn an_unknown_client_is_rejected_by_every_verifier() {
        let st = test_idp_state_with_two_verifiers().await;
        let err = post_token_expecting_error(&st, "grant_type=client_credentials&client_id=nope&client_secret=s").await;
        assert_eq!(err.status, 401);
        assert_eq!(err.body["error"], "invalid_client");
    }

    /// A verifier supplies `extra_claims`, so it must not be able to overwrite
    /// the claims that establish WHO the token is for. This is why the standard
    /// claims are inserted after the verifier's, and it is a security property,
    /// not a style choice — so it gets a test rather than a comment.
    #[tokio::test]
    async fn a_verifier_cannot_forge_the_standard_claims() {
        let hostile = FakeVerifier::with_extra_claims(serde_json::json!({
            "sub": "attacker",
            "aud": "catalog",
            "iss": "https://evil.example",
            "exp": 9_999_999_999u64,
        }));
        let st = test_idp_state_with(vec![Arc::new(hostile)]).await;

        let resp = post_token(&st, "grant_type=client_credentials&client_id=api1&client_secret=s").await;
        let c = decode_claims_without_verifying(&resp.access_token);

        assert_eq!(c["sub"], "api1", "sub must come from the verified client id");
        assert_eq!(c["aud"], "data-api", "aud must come from the verifier's audience field");
        assert_eq!(c["iss"], st.issuer.trim_end_matches('/'));
        assert_ne!(c["exp"], 9_999_999_999u64, "exp must be derived from ttl_secs");
    }
```

`FakeVerifier` (line 300) currently returns fixed values; extend it with a `with_extra_claims` constructor so this test can drive it. Keep its existing constructor working so the tests already using it are untouched.

Run: `cargo test -p egret-machine-idp`

Expected: compile error — `IdpState` has no `verifiers` field.

- [ ] **Step 3: Generalise `VerifiedClient` and `IdpState`**

In `crates/egret-machine-idp/src/lib.rs`, replace the `VerifiedClient` struct (line 32) and the `clients` field on `IdpState` (line 65):

```rust
/// A verified machine client. The verifier that matched owns the token's shape:
/// which `aud` it carries, what audience-specific claims it adds, and how long
/// it lives. That is what lets one IdP serve the Catalog and the Data API
/// without either knowing about the other.
pub struct VerifiedClient {
    pub client_id: String,
    /// The JWT `aud`.
    pub audience: String,
    /// Merged into the JWT alongside the standard claims. Must not contain
    /// `iss`, `sub`, `aud` or `exp` — those are the IdP's to set.
    pub extra_claims: serde_json::Map<String, serde_json::Value>,
    /// Token lifetime. Catalog: 3600. Data API: 600 — short, because a stateless
    /// token cannot be revoked before it expires.
    pub ttl_secs: u64,
}
```

```rust
#[derive(Clone)]
pub struct IdpState {
    pub keys: Arc<dyn SigningKeyStore>,
    /// The egret public root URL used as the JWT `iss` + OIDC issuer.
    pub issuer: String,
    /// Tried in order until one recognises the `client_id`. Each owns its own
    /// audience; a client belongs to exactly one registry.
    pub verifiers: Vec<Arc<dyn ClientVerifier>>,
}
```

- [ ] **Step 4: Build claims from the verifier**

Replace the hardcoded claim block in `token_endpoint` (lines 185-197):

```rust
    let mut vc = None;
    for v in &st.verifiers {
        if let Some(found) = v.verify(&f.client_id, &f.client_secret).await? {
            vc = Some(found);
            break;
        }
    }
    let vc = vc.ok_or(IdpError::InvalidClient)?;

    let km = st.keys.active().await?;
    let exp = chrono::Utc::now().timestamp() as u64 + vc.ttl_secs;
    let mut claims = serde_json::Map::new();
    // Audience-specific claims first, so the standard ones below always win —
    // a verifier must never be able to forge `iss`, `sub`, `aud` or `exp`.
    claims.extend(vc.extra_claims.clone());
    claims.insert("iss".into(), st.issuer.trim_end_matches('/').into());
    claims.insert("sub".into(), vc.client_id.clone().into());
    claims.insert("aud".into(), vc.audience.clone().into());
    claims.insert("exp".into(), exp.into());

    let jwt = sign_claims(&km, &serde_json::Value::Object(claims))?;
    Ok(Json(TokenResp {
        access_token: jwt,
        token_type: "Bearer".into(),
        expires_in: vc.ttl_secs,
    }))
```

The insert order is load-bearing: standard claims are written **after** the verifier's, so a buggy or hostile verifier cannot override `sub` or `aud`. Do not "tidy" this by extending last.

- [ ] **Step 5: Update the catalog's verifier**

In `crates/egret-catalog-engine/src/store/embedded.rs:403`, fill in the new fields so the emitted token is unchanged:

```rust
            Some((warehouse, catalog_role)) => Ok(Some(egret_machine_idp::VerifiedClient {
                client_id: client_id.to_string(),
                audience: "catalog".into(),
                extra_claims: serde_json::json!({
                    "warehouse": warehouse,
                    "roles": [catalog_role],
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
                ttl_secs: 3600,
            })),
```

- [ ] **Step 6: Update `IdpState` construction**

In `crates/egret-bin/src/main.rs:1759`, change `clients: <catalog store>` to `verifiers: vec![<catalog store>]`. The Data API verifier joins this list in Task 4.

- [ ] **Step 7: Run the tests and verify they pass**

Run: `cargo test -p egret-machine-idp -p egret-catalog-engine`

Expected: PASS, including `catalog_token_claims_are_unchanged` from Step 1. That test passing after the refactor is the whole point of this task.

- [ ] **Step 8: Verify the tests are real by mutating the implementation**

1. Change the catalog verifier's `ttl_secs` to 600. Expected: `catalog_token_claims_are_unchanged` FAILS on `expires_in`.
2. Move the `claims.extend(vc.extra_claims)` line to **after** the standard-claim inserts. Expected: `a_verifier_cannot_forge_the_standard_claims` (Step 2) FAILS on `sub`, `aud` and `exp` together.

Revert both.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-machine-idp/src/lib.rs \
        crates/egret-catalog-engine/src/store/embedded.rs \
        crates/egret-bin/src/main.rs
git commit -m "refactor(idp): let the machine-IdP serve several audiences

VerifiedClient now carries its audience, its extra claims and its TTL, so
a second consumer (Data API) can register a verifier without the catalog
knowing about it. Standard claims are written after the verifier's, so a
verifier cannot forge sub or aud.

The catalog's token is unchanged — aud, warehouse, roles and the 3600s
TTL are pinned by a regression test written before the refactor."
```

---

### Task 4: Data API clients and their verifier

**Files:**
- Create: `crates/egret-dataapi-api/{Cargo.toml,src/lib.rs}`, `crates/egret-dataapi/{Cargo.toml,src/lib.rs,src/clients.rs}`
- Modify: root `Cargo.toml` (workspace members), `crates/egret-bin/src/main.rs`
- Test: `crates/egret-dataapi/tests/clients_store.rs` (create)

**Interfaces:**
- Consumes: `dataapi.service_accounts` and `dataapi.clients` (Task 1); `VerifiedClient` (Task 3).
- Produces:
  - `pub struct NewClient { pub id: Uuid, pub client_id: String, pub client_secret: String }`
  - `pub async fn ClientStore::create(&self, sa: Uuid, name: &str, expires_at: Option<OffsetDateTime>, by: UserId) -> Result<NewClient>`
  - `pub async fn ClientStore::revoke(&self, id: Uuid) -> Result<()>`
  - `impl egret_machine_idp::ClientVerifier for ClientStore`

- [ ] **Step 1: Create the two crates**

`crates/egret-dataapi-api/Cargo.toml` — a pure leaf mirroring `crates/egret-query-api/Cargo.toml` (`egret-core`, `serde`, `uuid`, `utoipa`). `crates/egret-dataapi/Cargo.toml` — mirror `crates/egret-query/Cargo.toml` and add `egret-machine-idp`, `egret-dataapi-api`, `argon2`, `rand`, `base64`. Add both to the workspace `members` list in the root `Cargo.toml`, keeping its existing order.

Use `.workspace = true` for any dep already in the workspace dependency table, matching how sibling crates declare deps.

- [ ] **Step 2: Write the failing tests**

Create `crates/egret-dataapi/tests/clients_store.rs`. Build the `Db` the way other integration tests do — copy the fixture shape from `crates/egret-query/tests/registry_repo.rs`.

```rust
//! Every failure mode of `verify` must be indistinguishable from outside.
use egret_machine_idp::ClientVerifier;

#[tokio::test]
async fn a_new_client_verifies_and_carries_its_workspace() {
    let f = fixture().await;
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();

    let vc = f.clients
        .verify(&c.client_id, &c.client_secret)
        .await
        .unwrap()
        .expect("must verify");

    assert_eq!(vc.audience, "data-api");
    assert_eq!(vc.ttl_secs, 600, "short TTL is the revocation mitigation");
    assert_eq!(vc.extra_claims["ws"], f.ws.to_string());
    assert_eq!(vc.extra_claims["egret_user_id"], f.sa.to_string());
    assert!(
        vc.extra_claims.get("warehouse").is_none(),
        "no catalog claims on a data-api token"
    );
}

#[tokio::test]
async fn a_revoked_client_does_not_verify() {
    let f = fixture().await;
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    f.clients.revoke(c.id).await.unwrap();
    assert!(f.clients.verify(&c.client_id, &c.client_secret).await.unwrap().is_none());
}

#[tokio::test]
async fn an_expired_client_does_not_verify() {
    let f = fixture().await;
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let c = f.clients.create(f.sa, "prod", Some(past), f.admin).await.unwrap();
    assert!(f.clients.verify(&c.client_id, &c.client_secret).await.unwrap().is_none());
}

#[tokio::test]
async fn a_wrong_secret_does_not_verify() {
    let f = fixture().await;
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    assert!(f.clients.verify(&c.client_id, "not-the-secret").await.unwrap().is_none());
}

#[tokio::test]
async fn an_unknown_client_id_does_not_verify() {
    let f = fixture().await;
    assert!(f.clients.verify("nope", "s").await.unwrap().is_none());
}

#[tokio::test]
async fn deleting_the_service_account_removes_its_clients() {
    let f = fixture().await;
    let c = f.clients.create(f.sa, "prod", None, f.admin).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.sa)
        .execute(&f.db.pool)
        .await
        .unwrap();
    // ON DELETE CASCADE reaches dataapi.clients through service_accounts.
    assert!(f.clients.verify(&c.client_id, &c.client_secret).await.unwrap().is_none());
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-dataapi`

Expected: compile error — `ClientStore` does not exist.

- [ ] **Step 4: Implement the store and the verifier**

`crates/egret-dataapi/src/clients.rs`:

```rust
//! Data API OAuth2 clients: create, revoke, and the `ClientVerifier` the
//! machine-IdP calls at `POST /oauth/token`.
//!
//! Argon2 is affordable here because it runs once per TOKEN, not once per API
//! request — that is the whole reason this is an OAuth2 client and not a bearer
//! key checked on every call.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use egret_core::{Error, Result, UserId};
use egret_metastore::Db;
use time::OffsetDateTime;
use uuid::Uuid;

/// Data API tokens are short-lived: a stateless token cannot be revoked before
/// it expires, so the exposure window after revoking a client is this long.
pub const DATA_API_TTL_SECS: u64 = 600;

pub struct NewClient {
    pub id: Uuid,
    pub client_id: String,
    /// Shown once. Not recoverable.
    pub client_secret: String,
}

#[derive(Clone)]
pub struct ClientStore {
    db: Db,
}

impl ClientStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        service_account_id: Uuid,
        name: &str,
        expires_at: Option<OffsetDateTime>,
        by: UserId,
    ) -> Result<NewClient> {
        use base64::Engine as _;
        use rand::RngCore;

        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        let client_id = format!("egret-dataapi-{}", Uuid::new_v4());

        let salt = SaltString::generate(&mut rand::thread_rng());
        let hash = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|e| Error::Internal(anyhow::anyhow!("hash client secret: {e}")))?
            .to_string();

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.clients \
               (service_account_id, name, client_id, client_secret_hash, expires_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(service_account_id)
        .bind(name)
        .bind(&client_id)
        .bind(hash)
        .bind(expires_at)
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("create dataapi client: {e}")))?;

        Ok(NewClient { id, client_id, client_secret: secret })
    }

    pub async fn revoke(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE dataapi.clients SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("revoke dataapi client: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl egret_machine_idp::ClientVerifier for ClientStore {
    /// Returns `Ok(None)` for EVERY failure — unknown, revoked, expired, wrong
    /// secret. A distinguishing error here would be a free oracle for anyone
    /// probing client ids.
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> std::result::Result<Option<egret_machine_idp::VerifiedClient>, egret_machine_idp::IdpError>
    {
        let row: Option<(Uuid, Uuid, Uuid, String, Option<OffsetDateTime>, Option<OffsetDateTime>)> =
            sqlx::query_as(
                "SELECT c.id, c.service_account_id, sa.workspace_id, c.client_secret_hash, \
                        c.expires_at, c.revoked_at \
                 FROM dataapi.clients c \
                 JOIN dataapi.service_accounts sa ON sa.user_id = c.service_account_id \
                 WHERE c.client_id = $1",
            )
            .bind(client_id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?;

        let Some((id, sa, ws, hash, expires_at, revoked_at)) = row else {
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
        let parsed = PasswordHash::new(&hash)
            .map_err(|e| egret_machine_idp::IdpError::Db(format!("hash parse: {e}")))?;
        if Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        let _ = sqlx::query("UPDATE dataapi.clients SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await;

        let extra = serde_json::json!({ "ws": ws.to_string(), "egret_user_id": sa.to_string() })
            .as_object()
            .cloned()
            .unwrap_or_default();
        Ok(Some(egret_machine_idp::VerifiedClient {
            client_id: client_id.to_string(),
            audience: "data-api".into(),
            extra_claims: extra,
            ttl_secs: DATA_API_TTL_SECS,
        }))
    }
}
```

- [ ] **Step 5: Register the verifier**

In `crates/egret-bin/src/main.rs:1759`, append the Data API store to the `verifiers` vec built in Task 3, after the catalog store.

- [ ] **Step 6: Run the tests and verify they pass**

Run: `cargo test -p egret-dataapi -p egret-machine-idp`

Expected: PASS. The machine-IdP suite must stay green — a Data API verifier in the list must not disturb catalog clients.

- [ ] **Step 7: Verify the tests are real by mutating the implementation**

1. Skip the `revoked_at` check. Expected: `a_revoked_client_does_not_verify` FAILS.
2. Skip the `expires_at` check. Expected: `an_expired_client_does_not_verify` FAILS.
3. Change `ttl_secs` to 3600. Expected: `a_new_client_verifies_and_carries_its_workspace` FAILS.

Revert each.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-dataapi crates/egret-dataapi-api crates/egret-bin/src/main.rs Cargo.toml
git commit -m "feat(dataapi): OAuth2 client_credentials clients for service accounts

Registers a second ClientVerifier on the existing machine-IdP rather than
standing up a parallel credential system. Argon2 matches the catalog and
is affordable because it runs once per token, not once per API request.

TTL is 600s: a stateless token cannot be revoked before it expires, so the
exposure window after revoking a client is bounded by the TTL rather than
by a per-request DB check."
```

---

### Task 5: Management routes and permissions

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-server/src/lib.rs`
- Create: `crates/egret-dataapi/src/routes.rs`
- Modify: `crates/egret-dataapi/src/lib.rs`
- Modify: `docs/en/specs/2026-08-14-data-api-design.md`, `docs/vi/specs/2026-08-14-data-api-design.md`
- Test: `crates/egret-dataapi/tests/routes_authz.rs` (create)

**Interfaces:**
- Consumes: `ClientStore` (Task 4), the service-account store.
- Produces: `pub fn egret_dataapi::management_router() -> Router<DataApiState>`, `pub fn egret_dataapi::openapi() -> utoipa::openapi::OpenApi`.

```text
GET    /api/ws/{ws}/data-api/service-accounts
POST   /api/ws/{ws}/data-api/service-accounts
DELETE /api/ws/{ws}/data-api/service-accounts/{id}
GET    /api/ws/{ws}/data-api/service-accounts/{id}/clients
POST   /api/ws/{ws}/data-api/service-accounts/{id}/clients   -> returns the secret ONCE
DELETE /api/ws/{ws}/data-api/clients/{id}
```

- [ ] **Step 1: Add the permissions**

In `crates/egret-core/src/rbac.rs`, next to the `secret:*` block (around line 130), in the same `perm!` form:

```rust
    perm!(
        "service_account:read",
        "Data API",
        "View service accounts and their OAuth2 clients (never the secret)",
        Scope::Workspace
    ),
    perm!(
        "service_account:manage",
        "Data API",
        "Create and delete service accounts and issue their OAuth2 clients",
        Scope::Workspace
    ),
```

Confirm `Scope::Workspace` is the real variant name by reading the enum — the `secret:*` entries use `Scope::Platform`, so the workspace variant's spelling must be checked, not assumed.

- [ ] **Step 2: Write the failing authz tests**

Create `crates/egret-dataapi/tests/routes_authz.rs`. These matter most in this task: issuing a client hands out that account's access.

```rust
//! Issuing a client for a service account hands out that account's access.
//! Written from the caller's side: a real non-admin principal, not a mocked
//! decision.

#[tokio::test]
async fn read_permission_cannot_issue_a_client() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;
    let reader = f.principal_with(&["service_account:read"]).await;

    let res = f.post_as(reader,
        &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
        serde_json::json!({ "name": "prod" })).await;

    assert_eq!(res.status(), 403, "read must not imply issue");
}

#[tokio::test]
async fn a_client_cannot_be_issued_across_workspaces() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;   // in f.ws
    let other_ws = f.create_workspace("other").await;
    let manager = f.principal_with_in(&["service_account:manage"], other_ws).await;

    let res = f.post_as(manager,
        &format!("/api/ws/{other_ws}/data-api/service-accounts/{sa}/clients"),
        serde_json::json!({ "name": "prod" })).await;

    assert_eq!(res.status(), 404, "the account does not belong to that workspace");
}

#[tokio::test]
async fn the_secret_is_returned_once_and_never_again() {
    let f = fixture().await;
    let sa = f.create_service_account("reporting").await;
    let manager = f.principal_with(&["service_account:manage"]).await;

    let created: serde_json::Value = f
        .post_as(manager, &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws),
                 serde_json::json!({ "name": "prod" }))
        .await.json().await;
    assert!(created["client_id"].as_str().unwrap().starts_with("egret-dataapi-"));
    assert!(!created["client_secret"].as_str().unwrap().is_empty());

    let listed: serde_json::Value = f
        .get_as(manager, &format!("/api/ws/{}/data-api/service-accounts/{sa}/clients", f.ws))
        .await.json().await;
    assert!(
        listed[0].get("client_secret").is_none(),
        "the client list must never carry secret material"
    );
}
```

The cross-workspace test asserts **404, not 403** — consistent with spec §6.2 step 3: a caller must not distinguish "no such account" from "exists elsewhere".

- [ ] **Step 3: Run the tests and verify they fail**

Run: `cargo test -p egret-dataapi --test routes_authz`

Expected: compile error — the router does not exist yet.

- [ ] **Step 4: Implement the router**

Write `crates/egret-dataapi/src/routes.rs` following the handler idiom of `crates/egret-controlplane/src/access/mod.rs`: `State(st)`, `Auth(p)`, a `require(...)` gate as the first statement, `utoipa::path` on every handler.

Two DTOs, and the distinction between them is the point:

```rust
/// What a client list returns. There is deliberately NO secret field — not a
/// skipped one, not an Option. A field that cannot exist cannot leak through a
/// future serde attribute change.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ClientDto {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// The issue response, and the only type in the crate carrying secret material.
#[derive(Serialize, utoipa::ToSchema)]
pub struct NewClientDto {
    pub id: Uuid,
    pub client_id: String,
    /// Shown once; not recoverable afterwards.
    pub client_secret: String,
    /// Where to exchange these for a token.
    pub token_endpoint: String,
}
```

The scoping helper every handler calls first — this is what stops the IDOR:

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

The issue handler, which is the privileged one:

```rust
async fn issue_client(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path(sa): Path<Uuid>,
    Json(body): Json<IssueClientInput>,
) -> Result<(StatusCode, Json<NewClientDto>), DataApiError> {
    // `:manage`, never `:read` — issuing a client hands out this account's access.
    egret_core::authz::require(&*st.pdp, &ctx, "service_account:manage").await?;
    // ctx.workspace_id, NEVER a path parameter — see the rule below.
    let sa = scoped_account(&st, ctx.workspace_id.0, sa).await?;
    let c = st
        .clients
        .create(sa, &body.name, body.expires_at, ctx.principal.user_id)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(NewClientDto {
            id: c.id,
            client_id: c.client_id,
            client_secret: c.client_secret,
            token_endpoint: format!("{}/oauth/token", st.issuer.trim_end_matches('/')),
        }),
    ))
}
```

**The workspace comes from `ctx.workspace_id`, never from a `{ws}` path parameter.** There is no `require_ws` helper in this codebase: `egret_core::authz::require(pdp, target, action)` takes the workspace *from the target*, which is the `WsContext` extractor (see `crates/egret-bi/src/routes.rs:158-163` for the canonical shape). If a handler gates on `ctx` but scopes its query with a separately-extracted path `ws`, the two can disagree — and that divergence is exactly the cross-workspace hole `scoped_account` exists to close. Extract `sa` alone; let `WsContext` own the workspace.

If `Path(sa): Path<Uuid>` mis-binds because the route has two path segments, extract the tuple and **discard** the workspace element rather than using it:

```rust
    Path((_ws_ignored, sa)): Path<(Uuid, Uuid)>,
```

- [ ] **Step 5: Mount it**

In `crates/egret-server/src/lib.rs`, add one `doc.merge(egret_dataapi::openapi());` line to the explicit merge list (the note at line 90 says do NOT loop) and nest the router alongside the other workspace-scoped groups.

- [ ] **Step 6: Run the tests and verify they pass**

Run: `cargo test -p egret-dataapi`

Expected: PASS.

- [ ] **Step 7: Verify each authz gate by mutation**

For each test, remove the guard it targets and confirm that test — and ideally only that test — goes red:

1. Drop the `require_ws(..., "service_account:manage")` gate → `read_permission_cannot_issue_a_client` FAILS.
2. Drop the `scoped_account` call → `a_client_cannot_be_issued_across_workspaces` FAILS.
3. Add `client_secret` to `ClientDto` → `the_secret_is_returned_once_and_never_again` FAILS.

Revert each. A gate whose removal leaves the suite green is not a gate.

- [ ] **Step 8: Full scoped check**

```bash
cargo fmt --all --check
cargo clippy -p egret-dataapi -p egret-dataapi-api -p egret-machine-idp -p egret-catalog-engine -p egret-auth -p egret-controlplane -p egret-core --all-targets
cargo test -p egret-dataapi -p egret-dataapi-api -p egret-machine-idp -p egret-catalog-engine -p egret-auth -p egret-controlplane -p egret-core
```

Run sequentially. Do not use `just check`.

- [ ] **Step 9: Update the spec and commit**

`docs/en/specs/2026-08-14-data-api-design.md` §6.1 still describes a bespoke `egt_<prefix>_<secret>` API key hashed with SHA-256, and §5.1 still lists `dataapi.api_keys`. Rewrite both for what was actually built:

- §5.1: `dataapi.service_accounts` + `dataapi.clients`, no `api_keys`.
- §6.1: OAuth2 `client_credentials` against the existing machine-IdP; Argon2 (affordable because it is per-token, not per-request); 600s TTL; and the revocation trade-off stated plainly.
- §6.2 step 1: "key → service account" becomes "verify JWT via JWKS → principal + workspace from claims", with the note that this is stateless and costs no DB round-trip.

Mirror every change into `docs/vi/specs/2026-08-14-data-api-design.md`.

```bash
git add crates/ docs/ Cargo.toml
git commit -m "feat(dataapi): workspace-scoped service-account and client routes

Issuing a client gates on service_account:manage, never on :read — it
hands out that account's access. Every handler checks the account belongs
to the path workspace and 404s otherwise, so a cross-workspace caller
cannot distinguish 'absent' from 'not yours'.

The client-list DTO has no secret field at all, rather than a skipped one.

Spec §5.1 and §6.1 rewritten: the bespoke API-key scheme is replaced by
OAuth2 client_credentials on the existing machine-IdP."
```

---

## Done when

- `cargo test` green for all seven crates listed in Task 5 Step 8; `cargo fmt --all --check` and clippy clean.
- Every test named in Tasks 1–5 has been observed FAILING against its stated mutation. This is the acceptance criterion — green alone does not close this out.
- **The catalog's token is byte-identical to before**: `catalog_token_claims_are_unchanged` passes, and an Iceberg client authenticating against the embedded catalog still works.
- An admin can create a service account, issue a client, and exchange `client_id`/`client_secret` at `POST /oauth/token` for a JWT with `aud: "data-api"`, `ws`, and `egret_user_id`. Revoking the client makes the next token request fail.
- A service account cannot log in, does not appear in `GET /api/control-plane/users`, and still renders by id wherever a principal name is shown.
- Spec §5.1, §6.1 and §6.2 step 1 match what was built, in both languages.

## Not in this plan

- **Any serving surface.** No endpoint accepts the token yet; P2 ends at `/oauth/token` returning a Data API JWT. Validating that JWT and mapping it to a request is P4.
- **Endpoint definitions, versioning, publish** — P3.
- **Granting a service account access to data.** P2 creates the principal and its credential; granting it a role through `access_grants`, and granting it in Ranger, are existing flows. Spec §5.3's warning that publishing does not grant data access is surfaced in the UI in P5.
- **Instant revocation.** Explicitly rejected: it would require a per-request check and give back the throughput this design was chosen for. The 600s TTL is the bound.
- **Refresh tokens.** `client_credentials` does not use them — the client re-requests with its credentials. Every OAuth2 library handles this.
- **Frontend.** P5.
