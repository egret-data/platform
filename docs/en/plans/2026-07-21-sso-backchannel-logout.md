# SSO Back-Channel Logout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Keycloak-called `POST /api/auth/oidc/backchannel-logout` endpoint that validates an OIDC back-channel logout token and revokes the user's egret sessions (by `sub`).

**Architecture:** A pure `verify_logout_token(jwks, iss, aud, token)` validator (RS256 via `jsonwebtoken`, checking `events` + no-`nonce`) returns the `sub`; the handler maps `sub`→egret user (`find_by_external_subject`) and calls the existing `SessionService::revoke_all_for_user`. Plus the Keycloak realm-client config that makes Keycloak call it.

**Tech Stack:** Rust (axum, `jsonwebtoken`, sqlx), `egret-auth`; Keycloak realm JSON; Playwright e2e.

**Spec:** `docs/en/specs/2026-07-21-sso-backchannel-logout-design.md`

**DB for tests:** `.env` `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` — export for DB tests.

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/egret-auth/src/oidc.rs` | OIDC service | Add `verify_logout_token` (pure) + `OidcService::provider_jwks()` |
| `crates/egret-auth/src/oidc_routes.rs` | OIDC routes | Add `backchannel_logout` handler + route (T2) |
| `deploy/helm/egret/files/egret-realm.json` + `deploy/keycloak/egret-realm.json` | Keycloak realm | Add backchannel-logout client attributes (T3) |
| `frontend/tests/e2e/backchannel-logout-kind.spec.ts` + `justfile` | e2e | Best-effort live proof (T3) |

---

## Task 1: `verify_logout_token` validator

**Files:**
- Modify: `crates/egret-auth/src/oidc.rs` (add the pure validator + an error enum)
- Test: inline `#[cfg(test)]` in `oidc.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/egret-auth/src/oidc.rs` `#[cfg(test)]`. Helper: generate an RSA keypair, build a `jsonwebtoken::jwk::JwkSet` from the public key (with a `kid`), and sign logout tokens with the private key.

```rust
#[cfg(test)]
mod logout_token_tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    const ISS: &str = "https://kc/realms/egret";
    const AUD: &str = "egret";
    const EVT: &str = "http://schemas.openid.net/event/backchannel-logout";

    // test helper: returns (jwks, encoding_key, kid) — generate a 2048-bit RSA key,
    // expose its public n/e as a JWK, wrap in a JwkSet. (Use the `rsa` + `jsonwebtoken`
    // crates; mirror how egret-machine-idp builds a KeyMaterial/public_jwk in tests.)
    fn test_keys() -> (jsonwebtoken::jwk::JwkSet, EncodingKey, String) { /* … */ }

    fn claims(now: i64, with_events: bool, with_nonce: bool) -> serde_json::Value {
        let mut c = serde_json::json!({
            "iss": ISS, "aud": AUD, "sub": "user-123",
            "iat": now, "exp": now + 120, "jti": "j1",
        });
        if with_events { c["events"] = serde_json::json!({ EVT: {} }); }
        if with_nonce { c["nonce"] = serde_json::json!("n1"); }
        c
    }

    fn sign(key: &EncodingKey, kid: &str, c: &serde_json::Value) -> String {
        let mut h = Header::new(jsonwebtoken::Algorithm::RS256);
        h.kid = Some(kid.into());
        encode(&h, c, key).unwrap()
    }

    #[test]
    fn accepts_valid_logout_token() {
        let (jwks, key, kid) = test_keys();
        let t = sign(&key, &kid, &claims(now_ts(), true, false));
        assert_eq!(verify_logout_token(&jwks, ISS, AUD, &t).unwrap(), "user-123");
    }
    #[test] fn rejects_missing_events() {
        let (jwks, key, kid) = test_keys();
        let t = sign(&key, &kid, &claims(now_ts(), false, false));
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }
    #[test] fn rejects_nonce_present() {
        let (jwks, key, kid) = test_keys();
        let t = sign(&key, &kid, &claims(now_ts(), true, true));
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }
    #[test] fn rejects_wrong_aud() {
        let (jwks, key, kid) = test_keys();
        let mut c = claims(now_ts(), true, false); c["aud"] = serde_json::json!("other");
        let t = sign(&key, &kid, &c);
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }
    #[test] fn rejects_wrong_iss() {
        let (jwks, key, kid) = test_keys();
        let mut c = claims(now_ts(), true, false); c["iss"] = serde_json::json!("https://evil");
        let t = sign(&key, &kid, &c);
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }
    #[test] fn rejects_expired() {
        let (jwks, key, kid) = test_keys();
        let t = sign(&key, &kid, &claims(now_ts() - 10_000, true, false));
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }
    #[test] fn rejects_bad_signature() {
        let (jwks, _key, kid) = test_keys();
        let (_j2, key2, _k2) = test_keys(); // different key
        let t = sign(&key2, &kid, &claims(now_ts(), true, false));
        assert!(verify_logout_token(&jwks, ISS, AUD, &t).is_err());
    }

    fn now_ts() -> i64 { chrono::Utc::now().timestamp() }
}
```

(If the `rsa` crate isn't already available for the test key, check how `egret-machine-idp` generates a test key — reuse that mechanism / crate. The test must actually sign + verify, not stub.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p egret-auth logout_token_tests`
Expected: FAIL — `verify_logout_token` not defined.

- [ ] **Step 3: Implement `verify_logout_token`**

Add to `crates/egret-auth/src/oidc.rs`:

```rust
/// OIDC Back-Channel Logout 1.0 token validation. Verifies the JWS against `jwks`
/// (RS256, by `kid`), enforces `iss`/`aud`/`exp`, requires the back-channel-logout
/// `events` key, and rejects any token carrying a `nonce` (that's an ID token, not
/// a logout token). Returns the `sub` on success.
pub fn verify_logout_token(
    jwks: &jsonwebtoken::jwk::JwkSet,
    issuer: &str,
    audience: &str,
    token: &str,
) -> Result<String> {
    use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
    let kid = decode_header(token)
        .map_err(|e| oidc_err(e))?
        .kid
        .ok_or_else(|| Error::Unauthorized)?;
    let jwk = jwks.find(&kid).ok_or_else(|| Error::Unauthorized)?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| oidc_err(e))?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[issuer]);
    v.set_audience(&[audience]);
    v.validate_exp = true; // exp required + checked with default leeway
    let data = decode::<serde_json::Value>(token, &key, &v).map_err(|e| oidc_err(e))?;
    let c = data.claims;
    // back-channel-logout event must be present
    let has_event = c.get("events")
        .and_then(|e| e.as_object())
        .map(|o| o.contains_key("http://schemas.openid.net/event/backchannel-logout"))
        .unwrap_or(false);
    if !has_event {
        return Err(Error::Unauthorized);
    }
    // a logout token MUST NOT have a nonce
    if c.get("nonce").is_some() {
        return Err(Error::Unauthorized);
    }
    c.get("sub").and_then(|s| s.as_str()).map(String::from).ok_or(Error::Unauthorized)
}
```

Adjust to the file's real error type (`Error`/`Result`/`oidc_err` — grep the top of `oidc.rs`). `DecodingKey::from_jwk` requires the `jsonwebtoken` `use_pem`/jwk feature — confirm it compiles; if `from_jwk` isn't available, build the key from the JWK's `n`/`e` via `DecodingKey::from_rsa_components` (as `egret-machine-idp::verify_with_jwk` does).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p egret-auth logout_token_tests`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/oidc.rs
git commit -m "feat(auth): verify_logout_token — OIDC back-channel logout token validation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `backchannel-logout` endpoint

**Files:**
- Modify: `crates/egret-auth/src/oidc.rs` (add `OidcService::provider_jwks()`)
- Modify: `crates/egret-auth/src/oidc_routes.rs` (handler + route)
- Test: `crates/egret-auth/tests/backchannel_logout.rs` (new) or inline

- [ ] **Step 1: Expose the provider JWKS from `OidcService`**

Add a method that returns the discovered provider JWKS as a `jsonwebtoken::jwk::JwkSet`. The openidconnect provider metadata already holds the keys (`self.client` / the discovered metadata's `jwks`). If converting the openidconnect JWK set to a `jsonwebtoken::jwk::JwkSet` is awkward, instead fetch the `jwks_uri` once and parse it into a `jsonwebtoken::jwk::JwkSet` (store the `jwks_uri` on `OidcService` at `discover` time; use the crate's HTTP client). Also store `issuer` + `client_id` on `OidcService` (or expose accessors) so the handler can pass them to `verify_logout_token`:

```rust
impl OidcService {
    pub fn issuer(&self) -> &str { /* cfg.issuer_url */ }
    pub fn client_id(&self) -> &str { /* cfg.client_id */ }
    /// The provider's current JWKS, for verifying back-channel logout tokens.
    pub async fn provider_jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet> {
        // GET the jwks_uri (captured at discover()) and parse as JwkSet.
    }
}
```
(Confirm what `OidcService` already stores — it may already keep `cfg`; if so reuse `cfg.issuer_url`/`cfg.client_id`. Prefer reusing the metadata's in-memory JWKS over a network fetch if the crate exposes it.)

- [ ] **Step 2: Write the failing integration test**

Create `crates/egret-auth/tests/backchannel_logout.rs` — a DB test that seeds a user + a live session and drives the handler (or the route via axum `oneshot`). Since the real Keycloak isn't available in the unit test, inject a test JWKS/issuer/aud: structure the handler so its verification inputs come from `OidcService`, and in the test build an `OidcRoutesState` whose `oidc` is a test double OR call a lower-level handler-core fn `revoke_for_logout_token(state, token)` that takes the jwks/iss/aud. Simplest: extract a testable core:

```rust
#[sqlx::test]
async fn valid_logout_token_revokes_user_sessions(pool: sqlx::PgPool) {
    let db = egret_metastore::Db::from_pool(pool.clone());
    let (jwks, key, kid) = test_keys(); // reuse Task 1's helper (make it pub(crate) or dup)
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(db.clone(), chrono::Duration::hours(24), chrono::Duration::hours(1));
    // seed an OIDC user with subject "kc-sub-1"
    let uid = users.insert_oidc("kc-sub-1", "u@egret.local", "U").await.unwrap();
    let tok = sessions.create(uid, "", "").await.unwrap();
    assert!(sessions.validate(&tok.token).await.unwrap().is_some());
    // build + verify a logout token for that sub, then revoke
    let lt = sign_logout(&key, &kid, "kc-sub-1"); // events, no nonce, right iss/aud
    let sub = verify_logout_token(&jwks, ISS, AUD, &lt).unwrap();
    let user = users.find_by_external_subject(&sub).await.unwrap().unwrap();
    sessions.revoke_all_for_user(user.id).await.unwrap();
    assert!(sessions.validate(&tok.token).await.unwrap().is_none(), "session revoked");
}
```
This proves the verify→sub→lookup→revoke chain end to end (the handler is a thin wrapper over exactly this). If you prefer a full-route test, use the axum `oneshot` pattern the OIDC-callback tests use.

- [ ] **Step 3: Run to verify it fails, then implement the handler**

Run: `cargo test -p egret-auth valid_logout_token_revokes` → FAIL until wired.

In `crates/egret-auth/src/oidc_routes.rs`, add the route to the OIDC router (mounts under `/api/auth/oidc`, so this is `/api/auth/oidc/backchannel-logout`):

```rust
        .route("/backchannel-logout", post(backchannel_logout))
```
And the handler (form body `logout_token`; CSRF-exempt — this router handles the redirect callback and is not behind the CSRF-mutation layer, but CONFIRM by checking where the CSRF middleware is applied in `egret-bin`/`egret-server`; if a global CSRF layer would block a cookieless POST, exempt this path):

```rust
#[derive(serde::Deserialize)]
struct BackchannelLogoutForm {
    logout_token: String,
}

/// `POST /api/auth/oidc/backchannel-logout` — OIDC Back-Channel Logout 1.0.
/// Keycloak calls this server-to-server on logout; egret validates the logout
/// token and revokes ALL of the `sub`'s egret sessions. No cookie/CSRF (secured
/// by the token signature). Unknown sub → 200 no-op. Sets `Cache-Control: no-store`.
async fn backchannel_logout(
    State(st): State<OidcRoutesState>,
    axum::Form(form): axum::Form<BackchannelLogoutForm>,
) -> impl axum::response::IntoResponse {
    use axum::http::{header, StatusCode};
    let no_store = [(header::CACHE_CONTROL, "no-store")];
    let jwks = match st.oidc.provider_jwks().await {
        Ok(j) => j,
        Err(_) => return (StatusCode::BAD_REQUEST, no_store, "").into_response(),
    };
    let sub = match crate::oidc::verify_logout_token(&jwks, st.oidc.issuer(), st.oidc.client_id(), &form.logout_token) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, no_store, "").into_response(),
    };
    if let Ok(Some(user)) = st.users.find_by_external_subject(&sub).await {
        let _ = st.sessions.revoke_all_for_user(user.id).await;
    }
    (StatusCode::OK, no_store, "").into_response()
}
```
Match real imports (`post`, `OidcRoutesState`, `State`). Add `backchannel_logout` to the OpenAPI `paths(...)` if the file enumerates them.

- [ ] **Step 4: Run to verify it passes + build**

Run: `export DATABASE_URL=...; cargo test -p egret-auth valid_logout_token_revokes && cargo build -p egret-auth -p egret-bin`
Expected: PASS + builds. `cargo clippy -p egret-auth --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/oidc.rs crates/egret-auth/src/oidc_routes.rs crates/egret-auth/tests/
git commit -m "feat(auth): POST /api/auth/oidc/backchannel-logout revokes sessions by sub

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Keycloak realm config + best-effort live e2e

**Files:**
- Modify: `deploy/helm/egret/files/egret-realm.json`, `deploy/keycloak/egret-realm.json`
- Create: `frontend/tests/e2e/backchannel-logout-kind.spec.ts` + `justfile` recipe

- [ ] **Step 1: Add the backchannel-logout attributes to the `egret` client**

In BOTH realm JSONs, find the client `"clientId": "egret"` and extend its `attributes`:
```json
"attributes": {
  "pkce.code.challenge.method": "S256",
  "backchannel.logout.url": "http://egret.egret-system.svc.cluster.local:8080/api/auth/oidc/backchannel-logout",
  "backchannel.logout.session.required": "false"
}
```
(Use the in-cluster Service URL Keycloak can reach. Confirm the exact JSON key names Keycloak expects for realm import — `backchannel.logout.url` + `backchannel.logout.session.required` are the client-attribute keys; verify against the Keycloak version in `deploy/helm/egret/templates/deps-keycloak.yaml`.)

- [ ] **Step 2: Write the best-effort live e2e**

Create `frontend/tests/e2e/backchannel-logout-kind.spec.ts` (opt-in `EGRET_E2E_KIND=1`). Log in as an OIDC user (reuse `loginViaOidcAdminSso` / `loginViaOidcMember` from `helpers.ts`), confirm `GET /api/auth/me` is 200, then force-logout that user via the **Keycloak admin API** (get an admin token from the `master` realm, `POST /admin/realms/egret/users/{id}/logout`), then assert the user's `GET /api/auth/me` becomes 401. If wiring the Keycloak admin API is too brittle, mark the test `test.fixme` with a comment documenting the manual verification steps + note the integration test (Task 2) is the authoritative coverage. Do NOT fabricate a passing test.

- [ ] **Step 3: Add the recipe**

In `justfile`, after another `e2e-kind-*`:
```make
# Opt-in back-channel logout e2e: Keycloak force-logout → egret session revoked.
e2e-kind-backchannel-logout: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test backchannel-logout-kind
```

- [ ] **Step 4: Verify discoverable + commit (coordinator runs live)**

Run: `cd frontend && npx tsc --noEmit && npx playwright test backchannel-logout-kind --list`
```bash
git add deploy/helm/egret/files/egret-realm.json deploy/keycloak/egret-realm.json frontend/tests/e2e/backchannel-logout-kind.spec.ts justfile
git commit -m "feat(sso): wire Keycloak backchannel-logout URL + e2e

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p egret-auth` (DATABASE_URL set) — logout-token unit + revoke integration green
- [ ] Live (best-effort): rebuild backend + realm re-import (fresh Keycloak) + `just e2e-kind-backchannel-logout`, OR the documented manual verify (admin force-logout → session dies)
