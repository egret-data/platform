# Phase 1a-3 — OIDC / SSO Design Spec

> **Status:** Approved design · **Date:** 2026-06-17 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-17-phase-1a-3-oidc-sso-design.md`](../../vi/specs/2026-06-17-phase-1a-3-oidc-sso-design.md).
> Builds on [Phase 1a identity spec](2026-06-17-phase-1a-identity-design.md) §8 and the shipped [1a-1 AuthN](../plans/2026-06-17-phase-1a-1-authn-session.md) + [1a-2 Workspace & RBAC](../plans/2026-06-17-phase-1a-2-workspace-rbac.md) (group resolution already wired).

---

## 1. Goal & scope

Add OIDC/SSO login: authorization-code + PKCE against an external IdP, JIT user provisioning, and **IdP group synchronization** at login (roles flow through the existing group→role mechanism from 1a-2). Backend/API only.

**In scope**
- `OidcService` (over the `openidconnect` crate): discovery, authorize-URL with PKCE + nonce + state, code exchange, id_token validation.
- `/api/auth/oidc/login` + `/api/auth/oidc/callback` routes; ephemeral PKCE/nonce/state in an encrypted short-lived cookie.
- JIT provisioning: match by `sub`, upsert user, update profile; **group sync (replace, oidc-sourced only)** into `user_groups`.
- Real-IdP testing against **Keycloak** (testcontainers, gated) + Keycloak in the dev compose stack.

**Out of scope (deferred)**
- Email-based account linking (security: no auto-link; see §10) → future.
- Direct claim→role mapping via `idp_role_mapping` (the `map_idp_claims_to_roles` fn from 1a-2 stays unused until needed) → future.
- Workspace-level IdP mapping; multiple simultaneous IdPs → future.
- Frontend login button → Phase 0b.

## 2. Decisions (validated against Metabase & Superset)

| Decision | Choice | Reference evidence |
|---|---|---|
| OIDC library | **`openidconnect` crate** | Avoids hand-rolling signature/JWKS/nonce validation. |
| IdP→role mechanism | **Group sync → group→role** (idp_role_mapping deferred) | Superset `AUTH_ROLES_MAPPING` + `AUTH_ROLES_SYNC_AT_LOGIN` (group→role at login); Metabase `sso/` `group-membership`/`sync-group` machinery. |
| Group sync policy | **Replace (revoke on leave)**, oidc-sourced memberships only | Superset/Metabase sync (replace) group membership at each login. |
| Ephemeral state | **Encrypted short-lived cookie** (PrivateCookieJar) | Stateless, multi-instance-safe; protects PKCE verifier confidentiality + integrity. |
| Account matching | **By `sub`**; email collision → `Conflict` (no auto-link) | Prevents account takeover via unverified IdP email. |
| OIDC testing | **Real Keycloak (testcontainers, gated) — both tiers** | Real discovery/JWKS/token vs a mock; both ROPC (fast validate) + full auth-code+PKCE+nonce flow. |

## 3. Architecture

OIDC lives in `egret-auth` (authentication). New/changed pieces:

```mermaid
sequenceDiagram
  participant B as Browser
  participant A as egret-auth (oidc routes)
  participant K as IdP (Keycloak)
  B->>A: GET /api/auth/oidc/login
  A->>A: OidcService.start() → url+state+nonce+pkce
  A->>B: 302 to IdP; Set-Cookie egret_oidc_state (encrypted, 10m)
  B->>K: login form
  K->>B: 302 /callback?code&state
  B->>A: GET /api/auth/oidc/callback?code&state (+ state cookie)
  A->>A: verify state==cookie.state
  A->>K: OidcService.finish(code, pkce, nonce) → validate id_token → claims
  A->>A: provision_and_sync(claims): upsert user (by sub), sync oidc groups
  A->>A: SessionService.create → set session+csrf cookies; clear state cookie
  A->>B: 302 post_login_redirect
```

- `egret-auth/src/oidc.rs` — `OidcService`, `OidcClaims`, `OidcStart`, `provision_and_sync`.
- `egret-auth/src/groups.rs` — `GroupRepo` (upsert oidc group, sync user oidc-group memberships).
- `egret-auth/src/users.rs` — add `find_by_external_subject`, `insert_oidc`, `update_profile`.
- `egret-auth/src/routes.rs` — `oidc_login`, `oidc_callback` handlers; `AuthState` gains `oidc: Option<OidcService>` + `cookie_key: cookie::Key`.

## 4. `OidcService` (interface)

```rust
pub struct OidcClaims {
    pub subject: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

pub struct OidcStart {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

pub struct OidcService { /* openidconnect CoreClient + http client + groups_claim */ }

impl OidcService {
    /// Discover provider metadata and build the client from `[oidc]` config.
    pub async fn discover(cfg: &egret_config::OidcConfig) -> Result<Self>;
    /// Build the authorize URL with a fresh PKCE challenge, nonce, and state.
    pub fn start(&self) -> OidcStart;
    /// Exchange the code (with the PKCE verifier) and validate the id_token (signature
    /// via JWKS, iss, aud=client_id, exp, nonce). Extract claims incl. the groups claim.
    pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<OidcClaims>;
}
```
> The `groups` value is read from the configurable `groups_claim` (default `"groups"`); accepts a JSON array of strings.

## 5. Ephemeral state cookie

- `egret_oidc_state` — an **encrypted** cookie (`axum_extra::extract::cookie::PrivateCookieJar`, key = `cookie::Key` from `[security].secret_key`).
- Value: JSON `{ state, pkce_verifier, nonce }`. Attributes: `HttpOnly`, `SameSite=Lax`, `Secure` (per `session.secure`), `Max-Age=600` (10 min), `Path=/api/auth/oidc`.
- On callback: decrypt, assert `query.state == cookie.state`, then remove the cookie. A missing/expired/mismatched cookie → `InvalidInput` (400).

## 6. Routes

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/auth/oidc/login` | If `oidc` disabled → 404. Else `OidcService.start()`, set the encrypted state cookie, 302 to `authorize_url`. |
| GET | `/api/auth/oidc/callback?code&state` | Verify state vs cookie → `finish` → `provision_and_sync` → `SessionService.create` → set session+csrf cookies, clear state cookie → 302 to `post_login_redirect`. |

Both routes are CSRF-exempt (no app session yet; the OAuth `state` is the CSRF defense for this flow) and mounted on the public (non-`csrf_layer`) auth sub-router.

## 7. JIT provisioning + group sync

`provision_and_sync(users: &UserRepo, groups: &GroupRepo, claims: &OidcClaims) -> Result<UserId>`:

1. **Match by `sub`:** `users.find_by_external_subject(&claims.subject)`.
   - Found → `users.update_profile(id, email, name)` (refresh), `touch_last_login`.
   - Not found → `users.insert_oidc(sub, email, name)`. On the `email` unique violation (the email already belongs to a different account) → `Error::Conflict` (no auto-link).
2. **Group sync (replace, oidc-sourced only):**
   - For each name in `claims.groups`: `groups.upsert_oidc_group(name)` (insert with `source='oidc'` on conflict-do-nothing; return id).
   - `groups.sync_user_oidc_groups(user_id, &claims.groups)`: in one transaction — `DELETE FROM user_groups WHERE user_id=$1 AND group_id IN (SELECT id FROM groups WHERE source='oidc')`, then insert the claim set. **Manual (`source='local'`) memberships are untouched.**
3. Return `user_id`. Roles resolve via the existing group→role path (1a-2 `Authorizer` + `SessionService.platform_roles` group branch) — no direct role writes here.

`GroupRepo` (new, `egret-auth/src/groups.rs`):
```rust
pub struct GroupRepo { db: Db }
impl GroupRepo {
    pub fn new(db: Db) -> Self;
    pub async fn upsert_oidc_group(&self, name: &str) -> Result<uuid::Uuid>;
    pub async fn sync_user_oidc_groups(&self, user_id: UserId, names: &[String]) -> Result<()>;
}
```

## 8. Data model

No new tables. One migration:

**`migrations/0005_oidc.sql`**
```sql
CREATE UNIQUE INDEX users_oidc_subject_uniq
    ON users (external_subject)
    WHERE provider = 'oidc' AND external_subject IS NOT NULL;
```
(`groups.source`, `users.external_subject`, `user_groups` already exist from `0002`.)

## 9. Configuration additions

```toml
[oidc]                     # OIDC active only when enabled = true
enabled = false
issuer_url = ""
client_id = ""
client_secret = ""
redirect_url = "http://localhost:8080/api/auth/oidc/callback"
scopes = ["openid", "email", "profile"]   # groups come via the IdP group mapper, not a scope
groups_claim = "groups"
post_login_redirect = "/"

[security]
# base64-encoded ≥64-byte key for encrypting the OIDC state cookie (PrivateCookieJar).
secret_key = ""
```
- If `oidc.enabled = true` and `security.secret_key` is empty/too short → **fail at boot** with a clear message (and a hint to generate one).
- `egret-config` exposes `OidcConfig` and `SecurityConfig` structs.

## 10. Security

- **id_token validation** (in `finish`, via `openidconnect`): signature against the IdP JWKS, `iss` == issuer, `aud` == client_id, `exp`, and **nonce** equals the value bound in the state cookie.
- **OAuth state**: callback `state` must equal the cookie `state` (CSRF for the redirect).
- **PKCE**: verifier stored only in the encrypted cookie; sent in the token exchange; never exposed.
- **No auto-link**: matching by `sub` only; an email already used by another account causes `Conflict`, never silent linking (prevents takeover via unverified IdP email).
- **Secrets**: `client_secret` and `security.secret_key` are never logged; `secret_key` is required when OIDC is enabled.
- **Group sync** only ever rewrites `source='oidc'` memberships — it cannot escalate via, or wipe, manually-granted groups/roles.
- The state cookie is scoped to `/api/auth/oidc` and short-lived (10 min).

## 11. Testing

**Tier 0 — fast, no IdP:**
- Unit: `provision_and_sync` group-replace logic shape; config validation (secret_key required when enabled).
- DB-gated (`DATABASE_URL`): `provision_and_sync` with a hand-built `OidcClaims` — new `sub` creates an oidc user; existing `sub` updates profile + keeps id; email collision with a local user → `Conflict`; group sync replaces oidc groups while a manually-added (`source='local'`) group membership survives; `GroupRepo` upsert/sync.

**Tier 1 — real Keycloak (testcontainers, gated), both sub-tiers:**
- A pre-provisioned realm (`deploy/keycloak/egret-realm.json`): confidential client `egret` (secret, redirect URI, direct-access-grant enabled, PKCE), a user `tester` (password, email) in group `data-team`, and a **groups protocol mapper** so the id_token carries `groups`.
- The test spins Keycloak via testcontainers (import the realm), then:
  - **(a) ROPC validate:** obtain a real id_token via direct-access-grant; assert `OidcService` validates it (JWKS signature, iss, aud) and extracts `sub`/`email`/`groups`.
  - **(b) Full auth-code+PKCE+nonce:** `OidcService.start()` → drive Keycloak's login form headlessly (GET the login page, extract the form action + auth-session cookies, POST `tester`'s credentials, capture `code` from the redirect `Location`) → `OidcService.finish(code, pkce_verifier, nonce)` validates the real id_token end-to-end → assert claims.
- **Gating:** these tests run only when Docker/Keycloak is available (testcontainers detects Docker; otherwise self-skip, like the DB tests), keeping the suite offline-green. CI runs them.

**Dev stack:** add a `keycloak` service to `deploy/docker-compose.dev.yml` (import the same realm) so local/manual OIDC login works against a real IdP; document the `[oidc]` values matching the realm.

## 12. Decomposition for the plan

Suggested tasks: (1) migration `0005` + `egret-config` `OidcConfig`/`SecurityConfig` (+ boot validation); (2) `UserRepo` oidc methods + `GroupRepo` + DB tests; (3) `provision_and_sync` + DB tests (Tier 0); (4) `OidcService` over `openidconnect` (discover/start/finish); (5) encrypted state cookie helpers; (6) `oidc_login`/`oidc_callback` routes + `AuthState`/wiring; (7) Keycloak realm json + compose service; (8) Keycloak testcontainers harness + ROPC validate test; (9) full auth-code form-login test; (10) final verification + e2e.

## 13. References

- Superset: `superset/superset/security/manager.py` (`sync_role_definitions`, oauth user info), `config.py` (`AUTH_ROLES_MAPPING`, `AUTH_ROLES_SYNC_AT_LOGIN`).
- Metabase: `metabase/src/metabase/sso/` (`oidc`, `common.clj`; `group-membership`/`sync-group` semantics).
- Egret: [identity spec](2026-06-17-phase-1a-identity-design.md) §8; [1a-2 plan](../plans/2026-06-17-phase-1a-2-workspace-rbac.md) (group resolution + `map_idp_claims_to_roles`).
