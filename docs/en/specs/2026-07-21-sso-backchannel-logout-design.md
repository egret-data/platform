# SSO Back-Channel Logout — Design

> **Status:** Design approved (brainstorm). Next: implementation plan.
> **Builds on:** the STS effort (#304–307). Closes the central-revocation gap
> deferred in SP1 (the egret session was made independent of Keycloak).

**Goal:** When a user is logged out / force-logged-out in Keycloak, egret revokes
that user's egret session(s) immediately — instead of the session living on until
its own idle/absolute expiry. Implements OIDC Back-Channel Logout 1.0: a
Keycloak-called endpoint on egret that receives a signed logout token and revokes.

## 1. Decision (settled)

**Granularity = by `sub`** — the logout token's `sub` maps to an egret user, and
egret revokes **all** of that user's sessions (`SessionService::revoke_all_for_user`).
No schema change (no `sid` column, no login-time capture); consistent with the
existing disable→revoke-all behavior; sufficient for a data portal where a user
typically has one session. (The alternative — per-`sid` revocation — was rejected
as more machinery than needed.)

## 2. Existing seams (reused)

- `OidcService` (`crates/egret-auth/src/oidc.rs`) already discovers the Keycloak
  provider metadata + JWKS and validates ID tokens — the same trust used to verify
  the logout token's signature/`iss`.
- `UserRepo::find_by_external_subject(sub) -> Option<User>` (`users.rs:89`) — the
  `sub`→egret-user mapping (already used by `provision_and_sync` at login).
- `SessionService::revoke_all_for_user(user_id)` (`session.rs`) — deletes the
  user's `sessions` rows; the next request with a stale cookie fails `validate`.
- `OidcRoutesState` already carries `oidc`, `users`, `sessions` — the handler needs
  exactly these; no new state.

## 3. Components

### 3.1 `verify_logout_token(&OidcService, token) -> Result<String>` (returns `sub`)
A pure-ish validator in `crates/egret-auth/src/oidc_routes.rs` (or `oidc.rs`),
verifying a Keycloak-signed logout token per OIDC Back-Channel Logout 1.0. Accept
only if ALL hold, else error:
- signature verifies against the provider JWKS (RS256, by `kid`);
- `iss` == the configured Keycloak issuer; `aud` contains egret's `client_id`;
- `iat` present, `exp` fresh (small clock leeway);
- the `events` claim is a JSON object containing the key
  `http://schemas.openid.net/event/backchannel-logout`;
- **no `nonce` claim** (distinguishes a logout token from an ID token);
- `sub` present → returned.
Use the `openidconnect` crate's logout-token support if available; otherwise verify
the JWS manually against the discovered JWKS (the `jsonwebtoken` crate is already a
dependency via egret-machine-idp).

### 3.2 `POST /api/auth/backchannel-logout` handler
Mounted alongside the OIDC callback (CSRF-exempt — Keycloak calls it
server-to-server, `application/x-www-form-urlencoded` body `logout_token=<JWT>`, no
session cookie). Flow:
```
logout_token → verify_logout_token → sub
  → UserRepo::find_by_external_subject(sub)
      → Some(user) → SessionService::revoke_all_for_user(user.id) → 200
      → None (never logged into egret)                            → 200 (no-op)
```
Sets `Cache-Control: no-store` on the response (per the RFC).

### 3.3 Keycloak realm config
Add to the `egret` client in `deploy/helm/egret/files/egret-realm.json` (and
`deploy/keycloak/egret-realm.json`):
- `Backchannel Logout URL` = a Keycloak-reachable egret URL, e.g.
  `http://egret.egret-system.svc.cluster.local:8080/api/auth/backchannel-logout`
  (in-cluster Service DNS — the same host `catalog.public_url` uses), or
  `http://egret.localtest.me/api/auth/backchannel-logout` (resolves cluster-wide).
- `Backchannel Logout Session Required` = **false** (we key on `sub`, not `sid`).
In the realm JSON these are client `attributes`:
`"backchannel.logout.url"` and `"backchannel.logout.session.required": "false"`.
Documented as an admin step for non-kind deployments.

## 4. Error handling

Per OIDC Back-Channel Logout 1.0:
- Invalid logout token (bad signature / `iss` / `aud` / expired / has `nonce` /
  missing `events`) → **400** with an OAuth-style JSON error body; nothing revoked.
- Unknown `sub` (user never logged into egret) → **200** no-op (don't leak user
  existence).
- Provider-JWKS unreachable → **400** (can't validate → don't revoke).
- `Cache-Control: no-store` on every response.
- Revocation is idempotent (a replayed/duplicate logout token just re-revokes) — no
  `jti`/replay tracking needed.

## 5. Security

The endpoint is unauthenticated by cookie but secured by the logout token's
Keycloak signature — only Keycloak can mint a token that passes `verify_logout_token`.
It is CSRF-exempt because it's a server-to-server POST with no cookie/CSRF context
(same posture as the kernel-token callbacks). No per-user data is returned, so the
unknown-`sub`→200 path leaks nothing.

## 6. Testing

- **Unit (`verify_logout_token`):** sign tokens with a test key + stub JWKS (mirror
  the machine-idp verify tests). Accept a valid logout token (has `events`, no
  `nonce`, correct `iss`/`aud`). Reject each failure mode: bad signature, wrong
  `aud`, wrong `iss`, expired, **has `nonce`**, **missing `events`**.
- **Integration (DB + handler):** seed a user with a live session, `POST` a valid
  logout token for their `sub` → assert the session's `validate` now returns `None`;
  unknown `sub` → `200`, no revoke, no error.
- **Live e2e (best-effort, optional):** configure the realm's backchannel URL, then
  use the Keycloak **admin API to force-logout the user** (fires back-channel logout
  to the client) and assert the user's egret session dies. If brittle (realm config
  + reachability), fall back to a documented manual verify + the integration test
  (authoritative — it exercises verify→sub→revoke end-to-end).

## 7. Task decomposition (for the plan)

1. `verify_logout_token` validator + unit tests (accept + 6 reject modes).
2. `POST /api/auth/backchannel-logout` handler + route mount (CSRF-exempt) +
   integration test (revoke on valid token; unknown-sub no-op).
3. Keycloak realm-config for the backchannel URL (both realm JSONs) + best-effort
   live e2e / documented manual verify.

## 8. Out of scope

- RP-initiated / front-channel logout (egret logout → end the Keycloak session) —
  the reverse direction, a separate feature.
- Per-`sid` revocation (rejected in §1).
- Front-channel logout iframe support.
