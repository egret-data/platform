# Egret STS / Token Broker — Design

> **Status:** Design approved (brainstorm). Next: implementation plan.
> **Scope:** Sub-project 1 of 2. Local-user management (CRUD local password users
> + groups + control-plane UI) is **sub-project 2**, a separate spec that depends
> on this one.

**Goal:** Make egret the single issuer of data-plane (engine) tokens. Every
authenticated user — local password **or** SSO — gets a short-lived, per-user JWT
**signed by egret's machine-IdP** for engine access, exactly as catalog tokens
already work. SSO/Keycloak is demoted to an authentication source used once at
login. This removes the Keycloak-refresh-token dependency that blocks users
without a refresh token (e.g. local admin) from querying engines.

**Architecture in one line:** separate *authentication* (who you are, at login)
from *token issuance* (what the data plane sees); egret's machine-IdP owns
issuance; engines trust only egret.

---

## 1. Background & motivation

Today the notebook/SQL-Lab/BI engine path mints a per-user engine JWT via a
**Keycloak refresh grant** (`EngineTokenMinter` → `egret-bin/src/engine_broker.rs`,
using the user's stored refresh token). Consequences:

- A user with **no Keycloak refresh token** (local/break-glass admin, any
  non-OIDC user) cannot mint an engine token → `BrokerError::NoRefreshToken` →
  401 in the notebook. This is a hard block, observed live.
- Engine access lifetime is tied to Keycloak's refresh-token lifetime: when it
  expires, the user is forced to re-login for data access.
- When OIDC is **disabled** on a deployment, `engine_jwks_uri`/`engine_oidc_issuer`
  are empty, so a jwt-delegation engine's JWT security integration is templated
  with an empty issuer/jwks — jwt-delegation is effectively broken with OIDC off.

Meanwhile the **catalog** side already does the right thing: `sign_claims` +
`PgSigningKeyStore` (the `egret-machine-idp` crate) mint an `aud:"catalog"` JWT
with **no refresh token**, and the embedded catalog trusts egret's own issuer.
This spec finishes the catalog↔engine symmetry: engine tokens become egret-signed
too.

## 2. Decisions (settled during brainstorming)

| Decision | Choice |
| --- | --- |
| Unify scope | **Full broker** — all users (local + SSO) → egret-signed engine tokens. Keycloak = authN-only. |
| Mint mechanism | **Approach A** — internal `sign_claims` minter via the machine-IdP active key (no new public grant; mirrors catalog). |
| Session lifetime | **egret session is independent** (sliding lifetime + idle timeout). Keycloak token/refresh expiry is irrelevant to engine access. |
| Central revocation | **Deferred.** egret-side user-disable invalidates the session now; IdP-driven revocation + back-channel logout are future work. |
| Engine trust | Engines trust **egret's issuer/JWKS only** (end state), via a dual-trust transition. |
| Groups source | egret's `user_groups` table (persisted at login via JIT), authoritative for token claims. |
| Local user CRUD/UI | **Out of scope** here — sub-project 2. Existing local users benefit automatically. |

## 3. Architecture & trust model

```
                         ┌──────────────── egret (trusted core) ─────────────────┐
 login (authN source)    │                                                       │
  ├─ local password ─────┼─▶ egret session ─▶ EngineTokenMinter (machine-IdP) ───┼─▶ egret-signed
  └─ SSO / OIDC ──────────┼─▶ (+ JIT provision)     sign_claims(active key)       │    engine JWT
        (validated once   │        │                aud:"engine"                  │   (per-user,
         at login only)   │        └─ groups → user_groups (persisted)           │    short-lived)
                          └───────────────────────────────────────────────────────┘
                                                    │  trusts egret JWKS/issuer ONLY
                                                    ▼
                                    StarRocks / Trino  (+ embedded catalog, already trusts egret)
```

Three trust pieces:

1. **egret's machine-IdP is the single data-plane issuer.** It already signs
   `aud:"catalog"`; we add `aud:"engine"`. Engines and catalog both validate
   against one JWKS (`/jwks`) + issuer — egret's.
2. **Keycloak/SSO is an authN source only.** Validated once at login to establish
   identity + groups; never touched afterward for engine access; no refresh token
   stored or used for tokens.
3. **egret's session is the lifetime authority.** Sliding lifetime + idle timeout,
   owned by egret. Keycloak token expiry is irrelevant.

**The one real cost:** egret's signing key becomes the trust root for *all* engine
data access (already true for catalog). `PgSigningKeyStore` supports rotation, so
this stays manageable (§7).

## 4. Components

**New**

1. **`MachineIdpEngineMinter`** (`egret-bin`, replaces `engine_broker.rs`) — the new
   `EngineTokenMinter` impl. `mint(user_id)` loads identity, builds the claim set,
   signs with the machine-IdP `active` key via `sign_claims`. No Keycloak, no
   refresh token. The single swapped seam: notebook kernel, SQL Lab, and BI all
   route through the `EngineTokenMinter` trait, so swapping the impl switches every
   consumer at once.
2. **`engine_claims(user, issuer, ttl)`** — a pure, unit-testable builder returning
   `{iss, sub, preferred_username, groups, aud:"engine", iat, exp}`.
   `preferred_username` == the slug `UserRepo::ensure_username` produces everywhere,
   so Ranger `principal_field=preferred_username` matches identically.
3. **Identity loader** — loads `(username, groups)` for a `user_id` from `users` +
   `user_groups`. egret's DB is the runtime source of truth for authz claims.

**Modified**

4. **OIDC login → JIT provisioning** (`egret-auth/oidc_routes.rs`) — on SSO login:
   validate the OIDC token once, **create-or-link** the egret user, **persist**
   group claims into `user_groups` (write-through, since the DB now feeds issuance).
   Stop storing/using the refresh token for engine access.
5. **Engine trust wiring** (`egret-bin/main.rs:~1514` + provisioning) —
   `engine_jwks_uri`/`engine_oidc_issuer` point at the **machine-IdP's** JWKS +
   issuer (a pod-reachable URL), **independent of `config.oidc.enabled`**. Audience
   unified to `aud:"engine"` (StarRocks `required_audience="engine"` in the
   `CREATE SECURITY INTEGRATION` template; Trino JWT config to match) — one small
   DDL-template change, uniform across engines.
6. **Session lifetime policy** (`sessions` table + auth middleware) — sliding
   lifetime + idle timeout; **user-disable invalidates sessions** (the revocation
   path kept for now).

**Retired**

7. The Keycloak refresh-grant engine path (`engine_broker.rs`) and the
   `oidc_refresh_tokens` dependency **for engine access**. The table may stay for
   any non-engine use or be dropped in a cleanup follow-up (not here).

## 5. Data flow

**Login (two authN sources → one session):**

- **SSO:** browser → egret OIDC login → Keycloak → callback. egret validates the
  OIDC token once, extracts identity + groups, JIT create-or-link user, persist
  groups → `user_groups`, mints an egret session. Keycloak token/refresh discarded.
- **Local (incl. local admin):** browser → egret local login → validate against the
  egret user store → egret session. No IdP touched.

**Engine query (notebook / SQL Lab / BI — same seam):**

```
kernel/backend ──kernel-token or session──▶ EngineTokenMinter::mint(user_id)
                                              ├─ load (username, groups) from users + user_groups
                                              ├─ engine_claims{ sub, preferred_username, groups, aud:"engine", exp }
                                              └─ sign_claims(active machine-IdP key)  ─▶ egret-signed JWT
kernel ──JWT (cleartext-pw / JWTAuth) over pinned TLS──▶ StarRocks / Trino
engine ──validate vs egret JWKS + issuer + aud:"engine"──▶ principal = preferred_username
Ranger ──enforce per-user by preferred_username──▶ (mask / row-filter / allow — unchanged)
```

Enforcement is **byte-identical** to today: engines key off `preferred_username`;
Ranger policies match the same username. Only the token's signer and issuer-trust
change.

**Token refresh (long-lived kernel):** the engine JWT is short-lived (~15 min). The
kernel caches until near expiry, then calls back (`/api/kernel/engine-token` /
`notebook-runtime/token`) → egret re-mints from the session. No Keycloak. Works as
long as the egret session is valid.

**The one re-login trigger:** when the egret session expires the callback returns
401 → the user re-logins. Keycloak token/refresh expiry is irrelevant to querying.

**Motivating case, resolved:** local admin → local login → session → `mint` works
(no refresh token). `NoRefreshToken` disappears.

## 6. Migration & rollout

**Ordering invariant:** an engine must *trust* egret **before** its users' tokens
become egret-signed, or it rejects them. Rollout is trust-first, mint-second.

- **Existing engines** (trust Keycloak's issuer): re-wire via the existing engine
  **update** path (update already re-runs the FE bootstrap DDL, idempotent).
  **Dual-trust transition:** add `egret_machine` (egret JWKS) alongside `egret_oidc`
  (Keycloak) in the `authentication_chain` so both old and new tokens authenticate
  — zero downtime. Once all mint traffic is egret-signed, a follow-up drops the
  Keycloak integration → **egret-only end state**. For dev/kind, simpler to delete
  + re-provision (fresh engines trust egret from the start).
- **OIDC on/off — strict improvement:** egret's machine-IdP issuer exists
  regardless of `config.oidc.enabled`, so `engine_jwks_uri`/`issuer` point at it
  unconditionally → jwt-delegation now works with OIDC **off** too. The current
  empty-issuer broken path (and its warning) disappears.
- **Consumers unchanged:** kernel + SQL Lab + BI go through the `EngineTokenMinter`
  trait and never inspect claims/aud — no kernel-image or consumer change.
- **Data backfill:** `user_groups` becomes the token's group source; SSO users get
  groups written-through on next login (JIT), existing users lazily the same way.
  No blocking migration — worst case a first post-cutover token lacks groups until
  re-login (Ranger falls back to user-level policies).
- **Rollback safety (optional):** keep the Keycloak minter behind a transitional
  config flag (`engine_token_issuer: egret | keycloak`, default `egret`) for
  no-redeploy fallback; remove the flag + dead path in cleanup. Trim if a clean
  cutover is preferred.

## 7. Error handling & security

**Preserved invariants:**
- **Per-user scoping stays.** Each engine JWT is scoped to one user and
  short-lived; the untrusted kernel only holds a token for its user and cannot
  impersonate anyone else. (This is why we mint via the machine-IdP rather than
  putting a service credential — impersonate mode — into the kernel.)
- **Transport unchanged.** The fail-closed bearer-requires-TLS guard (#302) still
  applies; egret-signed tokens still ride pinned TLS.

**Trust root & key management:** signing key in `oidc_signing_keys` (already there
for catalog). `PgSigningKeyStore` rotates via the `active` key while `/jwks`
publishes old+new keys → zero-downtime rotation. Recommend periodic rotation +
short token TTL. Blast radius on key compromise = mint-any-user; mitigate with DB
access control + rotation + short TTL (KMS/HSM is a future note).

**Revocation posture (accepted trade-off):** egret user-disable invalidates the
session → next mint/refresh fails; an already-issued token is valid only until
expiry. **Token TTL is the revocation granularity** (≈15 min bounds a disabled
user's residual access). Deferred: IdP-driven revocation + back-channel logout.

**Failure modes:**
1. **Mint fails** (DB down / no active key) → `mint` errors → consumer shows "cannot
   get engine token, retry". No insecure fallback.
2. **Session expired** → callback 401 → clear re-login prompt.
3. **Engine can't reach egret JWKS** → rejects all JWTs. JWKS URL must be
   pod-reachable (same requirement Keycloak's JWKS had — `egret.localtest.me`
   resolvable cluster-wide via the CoreDNS rewrite); engines cache the keyset, so a
   brief egret blip doesn't break auth.
4. **Clock skew** → `exp`/`iat` fail. Small validation leeway + NTP assumption.
5. **Missing groups** (user_groups not yet backfilled) → Ranger falls back to
   user-level policies. Degraded, not broken.

## 8. Testing

**Unit (pure):** `engine_claims` table-driven (username == `ensure_username` slug,
groups present, `aud:"engine"`, `exp == iat + ttl`, `iss` set); sign→verify
round-trip against a test JWKS; expiry-boundary + clock-leeway.

**Integration (DB-backed, `sqlx::PgPool`):**
- `MachineIdpEngineMinter::mint(user_id)`: seed `users` + `user_groups` → mint →
  decode → assert claims. No Keycloak.
- JIT provisioning: simulate an OIDC callback → assert user create-or-link + groups
  persisted.
- **Local-admin regression:** a local user with no refresh token mints
  successfully — guard that `NoRefreshToken` can never recur.
- Session policy: expired / disabled session → mint & refresh fail clearly.

**Contract:** the `EngineTokenMinter` swap — existing notebook-runtime / SQL Lab /
BI tests pass with the new impl injected.

**Live e2e (kind, opt-in):**
- **Local admin → notebook `starrocks("SELECT 1")` → row** (the case that failed
  this whole thread; now green with no Keycloak/refresh token). Headline proof.
- **SSO parity:** admin-sso (OIDC → JIT) full vs member-sso masked/row-filtered on
  the same query — proves enforcement parity. Reuses the `e2e-kind-per-user`
  harness, now driven by egret-minted tokens.
- **OIDC-off deployment:** provision jwt-delegation with OIDC disabled → query still
  works (the previously-broken empty-issuer path).
- **Migration:** dual-trust engine accepts a Keycloak-minted and an egret-minted
  token; egret-only after dropping the Keycloak integration.

## 9. Out of scope (this spec)

- Local-user CRUD + control-plane UI (sub-project 2, depends on this).
- IdP-driven revocation + back-channel (SSO single-logout) — future.
- KMS/HSM for the signing key — future.
- Dropping the `oidc_refresh_tokens` table / other Keycloak cleanup — follow-up.
