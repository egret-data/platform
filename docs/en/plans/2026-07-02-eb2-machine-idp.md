# EB-2: egret mini machine-IdP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 2). **EB-2.** VI: `docs/vi/plans/2026-07-02-eb2-machine-idp.md`.

**Goal:** An embedded, egret-owned OAuth2 provider that mints machine tokens: `POST /oauth/token` (client_credentials) → egret-signed JWT, plus `/.well-known/{openid-configuration,jwks.json}` so the embedded lakekeeper (EB-3) can validate. The client registry (`catalog_clients`) + `ClientVerifier` impl land in EB-3 (they reference EB-3's `warehouses`/`catalog_roles`); EB-2 defines the IdP + the `ClientVerifier` seam.

**Architecture:** New crate `egret-machine-idp`: a rotatable RSA signing key (persisted in `oidc_signing_keys`), JWKS + discovery, and the token endpoint that verifies a client via an injected `ClientVerifier` trait and signs a JWT with `sub`/`warehouse`/`roles`/`aud`/`exp`.

**Tech Stack:** Rust (axum, jsonwebtoken, rsa/rand, sqlx), Postgres.

---

## Background the implementer needs

- Migrations at repo-root; latest merged `0051`; EB-1 added none. This plan adds `0052` (just `oidc_signing_keys`). `touch crates/egret-metastore/src/lib.rs` to re-embed.
- `egret-machine-idp` is a pure IdP crate: `/oauth/token` + `/.well-known/*` are **public** machine endpoints (no platform-admin gate). It depends on a `ClientVerifier` trait (impl in EB-3), never on the client store crate directly.
- JWT: `jsonwebtoken` `encode` (RS256, `kid` header). JWKS: RSA JWK (`n`,`e`,`kid`,`kty:RSA`,`use:sig`,`alg:RS256`). Key: RSA-2048 via `rsa`, stored PEM (pkcs8).

---

## Task 1: Migration — `oidc_signing_keys`

**Files:** Create `migrations/0052_oidc_signing_keys.sql`; touch `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write + apply**

```sql
CREATE TABLE oidc_signing_keys (
    kid          text PRIMARY KEY,
    algorithm    text NOT NULL DEFAULT 'RS256',
    private_pem  text NOT NULL,
    public_jwk   jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    retired_at   timestamptz
);
```
`touch crates/egret-metastore/src/lib.rs && cargo run -q -p egret-bin -- migrate`; verify `to_regclass('oidc_signing_keys')` non-null.

- [ ] **Step 2: Commit**
```bash
git add migrations/0052_oidc_signing_keys.sql crates/egret-metastore/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(db): oidc_signing_keys (mini-IdP)"
```
(Co-Authored-By trailer on all commits.)

---

## Task 2: Crate scaffold + signing keys + JWKS/discovery

**Files:** Create `crates/egret-machine-idp/src/{lib.rs,keys.rs,jwks.rs,token.rs,error.rs}`

- [ ] **Step 1: Scaffold** — deps `jsonwebtoken`, `rsa`, `rand`, `serde`, `serde_json`, `base64`, `axum`, `thiserror`, `async-trait`, `sqlx`, `uuid`, `chrono`. `IdpError` (thiserror).

- [ ] **Step 2: Failing test — generate key → JWK + round-trip sign/verify**

```rust
#[test]
fn generate_key_signs_and_verifies() {
    let km = keys::generate_rsa_key("kid-1").unwrap();
    assert_eq!(km.public_jwk["kty"], "RSA");
    let tok = token::sign_claims(&km, &serde_json::json!({"sub":"c","aud":"catalog","exp":9999999999u64})).unwrap();
    assert_eq!(token::verify_with_jwk(&tok, &km.public_jwk).unwrap()["sub"], "c");
}
```

- [ ] **Step 3: Implement** `keys.rs` (`generate_rsa_key(kid) -> KeyMaterial{kid,private_pem,public_jwk}`), `token.rs` (`sign_claims`, `verify_with_jwk`), and the `SigningKeyStore` trait + `PgSigningKeyStore` over `oidc_signing_keys` (`active()` — create+insert if none; `all_public_jwks()`).

- [ ] **Step 4: Routes** — `IdpState { keys: Arc<dyn SigningKeyStore>, issuer: String, clients: Arc<dyn ClientVerifier> }`; `idp_router()` with `/.well-known/openid-configuration` (issuer/jwks_uri/token_endpoint), `/.well-known/jwks.json` (`{keys: all_public_jwks}`), `/oauth/token` (Task 3).

- [ ] **Step 5: Run + commit** — `cargo test -p egret-machine-idp` PASS.
```bash
git add crates/egret-machine-idp && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): egret-machine-idp — RSA keys + JWKS + discovery"
```

---

## Task 3: `/oauth/token` (client_credentials) + `ClientVerifier` seam

**Files:** Modify `crates/egret-machine-idp/src/{token.rs,lib.rs}`

- [ ] **Step 1: Trait + failing test**

```rust
pub struct VerifiedClient { pub client_id: String, pub warehouse: String, pub catalog_role: String }
#[async_trait::async_trait]
pub trait ClientVerifier: Send + Sync {
    async fn verify(&self, client_id: &str, client_secret: &str) -> Result<Option<VerifiedClient>, IdpError>;
}
```
```rust
#[tokio::test]
async fn token_endpoint_mints_jwt() {
    // FakeVerifier("cid","sec") -> VerifiedClient{warehouse:"eng-wh", catalog_role:"engine"}
    // POST /oauth/token grant_type=client_credentials&client_id=cid&client_secret=sec
    // -> 200 {access_token, token_type:"Bearer", expires_in}; decode: sub=cid, aud="catalog",
    //    warehouse="eng-wh", roles=["engine"]. wrong secret -> 401 invalid_client.
}
```

- [ ] **Step 2: Implement**

```rust
async fn token(State(st): State<IdpState>, Form(f): Form<TokenForm>) -> Result<Json<TokenResp>, IdpError> {
    if f.grant_type != "client_credentials" { return Err(IdpError::UnsupportedGrant); }
    let vc = st.clients.verify(&f.client_id, &f.client_secret).await?.ok_or(IdpError::InvalidClient)?;
    let km = st.keys.active().await?;
    let claims = serde_json::json!({ "iss": st.issuer, "sub": vc.client_id, "aud": "catalog",
        "warehouse": vc.warehouse, "roles": [vc.catalog_role], "exp": /* now + 3600 */ });
    Ok(Json(TokenResp{ access_token: token::sign_claims(&km,&claims)?, token_type:"Bearer".into(), expires_in:3600 }))
}
```
`IdpError::{InvalidClient,UnsupportedGrant}` → 401/400 OAuth2 JSON (`{"error":"invalid_client"}`); no secret in logs.

- [ ] **Step 3: Run + commit** — token test PASS.
```bash
git add crates/egret-machine-idp && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): /oauth/token client_credentials -> egret-signed JWT"
```

---

## Task 4: Mount the IdP in egret-server

**Files:** `crates/egret-bin/src/main.rs`, `crates/egret-server/src/lib.rs`

- [ ] **Step 1: Wire (public, no CSRF/auth)** — build `IdpState { keys: Arc::new(PgSigningKeyStore::new(db)), issuer: config.catalog.public_url_root, clients: <a temporary AllowNone/Fake until EB-3 provides CatalogClientStore> }`. For EB-2, inject a placeholder `ClientVerifier` that always returns `None` (no clients exist yet); EB-3 swaps in `CatalogClientStore`. Nest `idp_router()` at root in egret-server (no CSRF/auth). Add crate deps.
- Note: `issuer` MUST equal what lakekeeper is configured to trust (EB-3) and the JWT `iss` — the egret public root URL.

- [ ] **Step 2: Smoke** — `curl localhost:8080/.well-known/openid-configuration` + `/.well-known/jwks.json` (≥1 key). `/oauth/token` returns `invalid_client` (no clients until EB-3) — expected.

- [ ] **Step 3: CI + commit** — fmt + clippy + `cargo test -p egret-machine-idp`.
```bash
git add crates/egret-bin crates/egret-server && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): mount egret mini machine-IdP (/oauth/token + JWKS)"
```

---

## Self-review notes (spec coverage for EB-2)

- Spec **Component 2**: `/oauth/token` → egret-signed JWT (Task 3), JWKS + discovery (Task 2), rotatable signing key persisted (Task 1/2), mounted public (Task 4). `ClientVerifier` seam defined; real impl + `catalog_clients` land in EB-3 (they reference EB-3's warehouses/roles).
- **Security:** JWT `aud`/`exp`/`kid`; no token/secret logging.
- **Out of EB-2:** the client store + warehouses/roles/grants + Authorizer + lakekeeper trust (EB-3), mgmt (EB-4), UI (EB-5), e2e (EB-6).
