# Gateway P-B: egret mini machine-IdP + Catalog Clients Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-polaris-gateway-machine-idp-design.md` (Pillar 2). **P-B of the gateway epic.** The foundation for machine auth. VI reference: `docs/vi/plans/2026-07-02-gw-pb-machine-idp-catalog-clients.md`.

**Goal:** An embedded, egret-owned OAuth2/OIDC provider that mints per-consumer machine tokens: `POST /oauth/token` (client_credentials) → egret-signed JWT, plus `/.well-known/{openid-configuration,jwks.json}` so Polaris can verify. Backed by a first-class `catalog_clients` registry (create → clientId/secret once, revoke independently).

**Architecture:** New crate `egret-machine-idp` holds the signing key (persisted, rotatable), JWKS/discovery, and the token endpoint. `catalog_clients` (in egret-controlplane) is the client store; creating a client generates a clientId/secret (secret hashed at rest, shown once) and records the `(consumer_type, workspace, catalog, catalog_role)` it maps to. The JWT carries `iss`, `sub=client_id`, `aud=polaris`, `roles:[catalog_role]`, short `exp`. Polaris trust is configured in P-D.

**Tech Stack:** Rust (axum, jsonwebtoken, rsa/rand, argon2, sqlx), Postgres.

---

## Background the implementer needs

- Migrations at repo-root `migrations/`; latest merged is `0051`. This plan adds `0052`. `touch crates/egret-metastore/src/lib.rs` to re-embed.
- `egret-controlplane` (P2/P4) holds control-plane stores + `CpState` + `require_platform_admin` + `CpError` + `polaris_client(...)`. Add the client store + management routes here.
- `egret-machine-idp` is a NEW crate: pure IdP (no platform-admin gate — `/oauth/token` + `/.well-known/*` are public machine endpoints). It needs the client store to verify credentials — inject a trait so it does not depend on egret-controlplane directly.
- JWT: `jsonwebtoken` `encode` with `Header{ alg: RS256, kid: Some(kid) }`. JWKS: build an RSA JWK (`n`,`e`,`kid`,`kty:RSA`,`use:sig`,`alg:RS256`) from the public key. Generate RSA-2048 with `rsa` crate; store PEM.

---

## Task 1: Migration — `catalog_clients` + `oidc_signing_keys`; drop assignments

**Files:** Create `migrations/0052_catalog_clients_idp.sql`; touch `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write the migration**

```sql
-- Machine identities: one Catalog Client per consumer, egret-owned.
CREATE TABLE catalog_clients (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name               text NOT NULL,
    consumer_type      text NOT NULL CHECK (consumer_type IN ('trino','starrocks','notebook','external')),
    workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    catalog_name       text NOT NULL,
    catalog_role       text NOT NULL,
    client_id          text NOT NULL UNIQUE,
    client_secret_hash text NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    last_used_at       timestamptz
);
CREATE INDEX catalog_clients_ws_idx ON catalog_clients(workspace_id);

-- egret mini-IdP signing keys (rotatable; one active at a time).
CREATE TABLE oidc_signing_keys (
    kid          text PRIMARY KEY,
    algorithm    text NOT NULL DEFAULT 'RS256',
    private_pem  text NOT NULL,
    public_jwk   jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    retired_at   timestamptz
);

-- Superseded by catalog_clients (creating a client IS granting access).
DROP TABLE IF EXISTS workspace_catalog_assignments CASCADE;
```

- [ ] **Step 2: Re-embed + apply + verify**

`touch crates/egret-metastore/src/lib.rs && cargo run -q -p egret-bin -- migrate`; verify `to_regclass('catalog_clients')`, `to_regclass('oidc_signing_keys')` non-null and `workspace_catalog_assignments` null.

- [ ] **Step 3: Commit**
```bash
git add migrations/0052_catalog_clients_idp.sql crates/egret-metastore/src/lib.rs && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(db): catalog_clients + oidc_signing_keys; drop workspace_catalog_assignments"
```
(Co-Authored-By trailer on all commits.)

---

## Task 2: `egret-machine-idp` — signing keys + JWKS + discovery

**Files:** Create `crates/egret-machine-idp/` (`src/{lib.rs,keys.rs,jwks.rs,token.rs}`)

- [ ] **Step 1: Scaffold + `SigningKeyStore` trait + `KeyMaterial`**

`Cargo.toml`: `jsonwebtoken`, `rsa`, `rand`, `serde`, `serde_json`, `base64`, `axum`, `thiserror`, `async-trait`, `sqlx` (for the pg-backed store), `uuid`, `chrono`.
```rust
pub struct KeyMaterial { pub kid: String, pub private_pem: String, pub public_jwk: serde_json::Value }
#[async_trait::async_trait]
pub trait SigningKeyStore: Send + Sync {
    async fn active(&self) -> Result<KeyMaterial, IdpError>;      // create one if none active
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError>; // active + not-yet-expired retired
}
```

- [ ] **Step 2: Failing test — generate an RSA key → valid JWK + round-trip sign/verify**

```rust
#[test]
fn generate_key_produces_jwk_and_signs() {
    let km = crate::keys::generate_rsa_key("kid-1").unwrap();
    assert_eq!(km.public_jwk["kty"], "RSA");
    assert_eq!(km.public_jwk["kid"], "kid-1");
    // sign a token with the private pem, verify with a DecodingKey built from the JWK.
    let tok = crate::token::sign_claims(&km, &serde_json::json!({"sub":"c","aud":"polaris","exp": 9999999999u64})).unwrap();
    let claims = crate::token::verify_with_jwk(&tok, &km.public_jwk).unwrap();
    assert_eq!(claims["sub"], "c");
}
```

- [ ] **Step 3: Implement `keys.rs` (generate + JWK) + `token.rs` (sign/verify)**

`generate_rsa_key(kid)` → RSA-2048, PEM (pkcs8), and JWK (`n`/`e` base64url from the public components). `sign_claims(&KeyMaterial, claims)` → `jsonwebtoken::encode(Header{alg:RS256,kid}, claims, EncodingKey::from_rsa_pem(pem))`. `verify_with_jwk(token, jwk)` → build `DecodingKey::from_rsa_components(n,e)` + validate (used in tests + by Polaris in real life).

- [ ] **Step 4: JWKS + discovery routes (`jwks.rs`, `lib.rs`)**

```rust
pub struct IdpState { pub keys: Arc<dyn SigningKeyStore>, pub issuer: String, pub clients: Arc<dyn ClientVerifier> /* Task 3 */ }
pub fn idp_router() -> Router<IdpState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/oauth/token", post(token))   // Task 3
}
// discovery → {"issuer": st.issuer, "jwks_uri": "{issuer}/.well-known/jwks.json",
//              "token_endpoint": "{issuer}/oauth/token",
//              "grant_types_supported":["client_credentials"], "id_token_signing_alg_values_supported":["RS256"]}
// jwks → {"keys": st.keys.all_public_jwks().await? }
```

- [ ] **Step 5: `PgSigningKeyStore`** — implements `SigningKeyStore` over `oidc_signing_keys` (SELECT active; if none, generate + INSERT; `all_public_jwks` returns active + retired-but-within-grace).

- [ ] **Step 6: Run + commit** — `cargo test -p egret-machine-idp` (PASS).
```bash
git add crates/egret-machine-idp && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): egret-machine-idp — RSA signing keys + JWKS + OIDC discovery"
```

---

## Task 3: `/oauth/token` (client_credentials)

**Files:** Modify `crates/egret-machine-idp/src/{token.rs,lib.rs}`

- [ ] **Step 1: `ClientVerifier` trait + failing test**

```rust
pub struct VerifiedClient { pub client_id: String, pub catalog: String, pub catalog_role: String }
#[async_trait::async_trait]
pub trait ClientVerifier: Send + Sync {
    /// Verify client_id + secret; return its catalog/role, or None if invalid.
    async fn verify(&self, client_id: &str, client_secret: &str) -> Result<Option<VerifiedClient>, IdpError>;
}
```
```rust
#[tokio::test]
async fn token_endpoint_mints_jwt_for_valid_client() {
    // FakeVerifier accepts ("cid","sec") -> VerifiedClient{catalog:"lakehouse", catalog_role:"writer"}
    // FakeKeyStore returns a generated key
    // POST /oauth/token grant_type=client_credentials&client_id=cid&client_secret=sec
    // -> 200, {access_token, token_type:"Bearer", expires_in}; decode access_token: aud=polaris, sub=cid,
    //    roles=["writer"], and a "catalog" claim = "lakehouse".
    // wrong secret -> 401.
}
```

- [ ] **Step 2: Implement the handler**

```rust
async fn token(State(st): State<IdpState>, Form(f): Form<TokenForm>) -> Result<Json<TokenResp>, IdpError> {
    if f.grant_type != "client_credentials" { return Err(IdpError::UnsupportedGrant); }
    let vc = st.clients.verify(&f.client_id, &f.client_secret).await?.ok_or(IdpError::InvalidClient)?;
    let km = st.keys.active().await?;
    let exp = /* now + 3600s — inject a clock or use jsonwebtoken's numeric exp; keep now-injection out of the pure signer */;
    let claims = serde_json::json!({
        "iss": st.issuer, "sub": vc.client_id, "aud": "polaris",
        "roles": [vc.catalog_role], "catalog": vc.catalog, "exp": exp,
    });
    let jwt = crate::token::sign_claims(&km, &claims)?;
    Ok(Json(TokenResp { access_token: jwt, token_type: "Bearer".into(), expires_in: 3600 }))
}
```
`IdpError`: `InvalidClient`/`UnsupportedGrant` → `401`/`400` (OAuth2 error JSON `{"error":"invalid_client"}`); no secret in logs; constant-time compare happens in the verifier (Task 4). Rate-limit note: add a simple per-client_id throttle or leave a TODO for a tower layer (document).

- [ ] **Step 3: Run + commit** — token test PASS.
```bash
git add crates/egret-machine-idp && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): /oauth/token client_credentials -> egret-signed JWT"
```

---

## Task 4: `catalog_clients` store + management routes + `ClientVerifier` impl

**Files:** Create `crates/egret-controlplane/src/catalog_clients.rs`; modify `src/lib.rs`, `CpState`

- [ ] **Step 1: Failing test — create client returns secret once + verify works**

```rust
#[tokio::test]
async fn create_client_then_verify() {
    let store = CatalogClientStore::new(db);
    let created = store.create(NewClient{ name:"trino-eng".into(), consumer_type:"trino".into(),
        workspace_id: ws, catalog_name:"lakehouse".into(), catalog_role:"writer".into() }).await.unwrap();
    assert!(!created.client_secret.is_empty());          // shown once
    // verify with the returned secret succeeds; wrong secret fails.
    let vc = store.verify(&created.client_id, &created.client_secret).await.unwrap().unwrap();
    assert_eq!((vc.catalog.as_str(), vc.catalog_role.as_str()), ("lakehouse","writer"));
    assert!(store.verify(&created.client_id, "wrong").await.unwrap().is_none());
}
```

- [ ] **Step 2: Implement `CatalogClientStore`**

- `create(NewClient) -> CreatedClient{ id, client_id, client_secret /* plaintext, once */ }`: generate `client_id` (e.g. `egret_` + 16 hex) + `client_secret` (32 random bytes base64url); hash the secret with **argon2**; INSERT. Before insert, **ensure the Polaris catalog-role exists** for `(catalog_name, catalog_role)` via `polaris_client(...).create_catalog_role` wrapped in `ignore_conflict` (so a token's `roles` claim maps to a real role; grants on it are managed in P-C). 
- `verify(client_id, secret) -> Option<VerifiedClient>`: SELECT by `client_id`; argon2-verify (constant-time); on success update `last_used_at` + return `{client_id, catalog, catalog_role}`.
- `list_for_catalog(catalog) -> Vec<ClientDto>` (no secret); `list_for_workspace(ws)`; `revoke(id)` (DELETE).
- Implement `egret_machine_idp::ClientVerifier for CatalogClientStore` (delegates to `verify`).

- [ ] **Step 3: Management routes (control-plane, `require_platform_admin`)**

`crates/egret-controlplane/src/catalog_clients.rs` router (merged into `control_plane_router()`):
- `POST /polaris/catalogs/{catalog}/clients` — body `{name, consumerType, workspaceId, catalogRole}` → `create` → 201 `{...client, clientId, clientSecret}` (**secret once**).
- `GET /polaris/catalogs/{catalog}/clients` → `ClientDto[]` (no secret).
- `DELETE /polaris/clients/{id}` → 204 (revoke).

- [ ] **Step 4: Run + commit** — store test + a 403 authz test pass.
```bash
git add crates/egret-controlplane && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(control-plane): catalog_clients store + management routes (+ ClientVerifier)"
```

---

## Task 5: Wire the IdP into egret-server + verify

**Files:** Modify `crates/egret-bin/src/main.rs`, `crates/egret-server/src/lib.rs`

- [ ] **Step 1: Build `IdpState` + nest (public, no CSRF/auth)**

In `main.rs`: `IdpState { keys: Arc::new(PgSigningKeyStore::new(db.clone())), issuer: config.catalog.public_url_root /* e.g. http://localhost:8080 */, clients: Arc::new(CatalogClientStore::new(db.clone())) }`. Thread as `Option`/always-on into `AppState`. In `egret-server`: nest `egret_machine_idp::idp_router().with_state(idp_state)` at root (`/oauth/token`, `/.well-known/*`) with **no CSRF, no session auth** (machine endpoints). Add crate deps.
- Note: `issuer` MUST equal what Polaris is configured to trust (P-D) and what the JWT `iss` uses — the egret public root URL.

- [ ] **Step 2: Live smoke**

serve up + a registered Polaris connection + create a client via the API; then:
```bash
curl -s localhost:8080/.well-known/openid-configuration | python3 -m json.tool   # issuer, jwks_uri, token_endpoint
curl -s localhost:8080/.well-known/jwks.json | python3 -c "import sys,json;print(len(json.load(sys.stdin)['keys']),'key(s)')"
curl -s -X POST localhost:8080/oauth/token -d grant_type=client_credentials -d client_id=<cid> -d client_secret=<sec> | python3 -m json.tool  # access_token
```
Decode the access_token → `aud=polaris`, `roles`, `catalog`, `iss` = egret public root.

- [ ] **Step 3: CI**

`cargo fmt --all --check` + `cargo clippy -p egret-machine-idp -p egret-controlplane -p egret-server -p egret-bin --all-targets -- -D warnings` + `DATABASE_URL=… cargo test -p egret-machine-idp -p egret-controlplane`.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-bin crates/egret-server Cargo.lock && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(idp): mount egret mini machine-IdP (/oauth/token + JWKS) in egret-server"
```

---

## Self-review notes (spec coverage for P-B)

- Spec **Pillar 2**: `/oauth/token` client_credentials → egret-signed JWT (Task 3), JWKS + discovery (Task 2), signing-key lifecycle persisted + rotatable (Task 1/2), `catalog_clients` first-class + create-once-secret + revoke (Task 4), mounted public (Task 5). `workspace_catalog_assignments` dropped/folded (Task 1).
- **Security:** secret argon2-hashed + shown once + constant-time verify (Task 4); JWT `aud=polaris` + short exp + `kid` (Task 3); no token/secret logging.
- **Cross-plan:** the JWT `roles` claim maps to a Polaris catalog-role — the role's *existence* is ensured here (Task 4), its *grants* are managed in P-C, and Polaris's *trust + claim-mapping* is P-D.
- **Out of P-B:** the proxy (P-A), RBAC grants UI (P-C), Polaris OIDC trust config (P-D).
