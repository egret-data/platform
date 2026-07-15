# Secure delegated Trino queries — jwt mode (TLS + bearer) design

**Status:** design · **Date:** 2026-07-15 · **Author:** egret

**Goal:** A second, maximally-secure delegation mode (`delegationMode="jwt"`) where
the coordinator runs over **TLS** and each delegated query authenticates with the
**caller's own short-lived JWT** (bearer). The JWT binds the query identity — no
shared credential, no impersonation.

This complements impersonate mode (#254/#255, the default). The framework
(`delegationMode` on `Engine`, the jwt mint path in `execution/routes.rs`) already
exists from #254; this spec fills in TLS provisioning + connector cert trust.

---

## Why this + why NOT shared with impersonate

Over plain HTTP, Trino authorizes on the client-supplied `X-Trino-User` header, so
a bearer JWT does not bind the query identity (proven — closed PR #253). Over
**TLS**, the JWT DOES bind it: `bearer` works with no 401 (`current_user` = the JWT
user), and `X-Trino-User`/`X-Trino-Original-User` spoofing is rejected (verified on
a throwaway TLS Trino).

jwt mode is **exclusive per engine** — a jwt engine is JWT-only with **no
`egret_delegator`** user/policy. Sharing a coordinator with impersonate would keep
the delegator (a long-lived credential that can impersonate anyone) and thus cap
the blast-radius at impersonate level; jwt mode's whole value is per-user,
short-lived (300s) tokens with no delegator. Trade-off: jwt engines serve **OIDC
users only** (mint needs the caller's refresh token; non-OIDC ⇒ 403). Mixed /
non-OIDC users use an impersonate engine.

**TLS is orthogonal to the mode — provisioned for BOTH.** The delegation *mode*
decides only the auth (impersonate = delegator PASSWORD; jwt = bearer JWT); TLS is
a separate, shared concern that every delegation-capable engine gets. This means
**impersonate mode also runs over TLS**: the delegator password is now genuinely
encrypted on the wire (previously it relied on the HTTP `X-Forwarded-Proto: https`
trick from #255, which only satisfied Trino's check without real encryption). With
real TLS the connector omits `X-Forwarded-Proto` (redundant) and pins the cert.
"Sharing a coordinator" (rejected above) means sharing the two *auth mechanisms* on
one engine — NOT sharing TLS, which is safe and beneficial for both.

---

## Architecture

A `delegationMode="jwt"` engine:
- Coordinator serves **HTTPS on 8443** with an egret-generated **self-signed cert**;
  HTTP:8080 stays for internal discovery only.
- Auth = **JWT only** (`principal-field=preferred_username`, JWKS from the OIDC
  issuer). No PASSWORD authenticator, no `password.db` delegator, no impersonate
  Ranger policy.
- Delegated queries mint the caller's JWT (existing) and connect over TLS with the
  engine's cert **pinned** by egret's connector.

**Query flow:** user → egret `/queries` → `routes.rs` (`db.delegated &&
options.impersonate != true` ⇒ jwt) → mint caller JWT (403 if non-OIDC) →
`session_user = caller`, `bearer = JWT` → `TrinoConnector(base=https://coord:8443,
bearer, X-Trino-User=caller, tls_cert=pinned)` → Trino validates the JWT via JWKS →
runs **as the caller** (self; a spoofed X-Trino-User is rejected).

| Unit | Responsibility | Depends on |
|---|---|---|
| Cert gen (`egret-engine-mgmt`) | Generate self-signed cert+key (rcgen), SAN = coordinator `.svc` + localhost | rcgen |
| Engine model/migration | `tls_secret_id` (keystore secret), `coordinator_cert` (public PEM); `is_jwt_delegation()` | migration |
| Provisioner (`egret-orchestrator/trino.rs`) | Render `http-server.https.*`, mount keystore Secret; JWT-only auth for jwt mode | keystore content |
| Materialization (`assignment.rs`) | jwt DB row: `options={http_scheme:"https", tls_cert:<PEM>}`, port 8443, no secret/impersonate | engine cert |
| Connector (`connector/trino.rs`) | `ConnSpec.tls_cert` → reqwest client with `add_root_cert` | reqwest TLS |

---

## 1. Cert generation & storage

At provision (jwt mode, **idempotent** — generate once, reuse via the stored refs):
- Generate a self-signed cert+key with **`rcgen`**. **SAN** = the coordinator
  Service host (`<name>-coordinator.<ns>.svc`) **and** `localhost` + `127.0.0.1`
  (localhost so a dev `kubectl port-forward` also validates).
- **Keystore** = the cert+key concatenated PEM → stored in an **egret secret**;
  the engine row references it via a new column **`tls_secret_id`**.
- **Public cert** PEM → stored in a new engine column **`coordinator_cert`**
  (public; the connector pins it — never the private key).

Both are generated together; on re-provision, if `tls_secret_id` is already set the
existing cert is reused (no churn). Rotation = clear the refs + re-provision.

## 2. Provisioner (`egret-orchestrator/src/trino.rs`)

For a jwt-mode engine (auth block present, jwt selected):
- Coordinator config adds:
  - `http-server.https.enabled=true`
  - `http-server.https.port=8443`
  - `http-server.https.keystore.path=/etc/trino/tls/keystore.pem` (Trino accepts a
    PEM keystore = cert chain + private key)
  - keep `http-server.http.port=8080` + `discovery.uri=http://localhost:8080`
    (internal single-node loopback)
- Render a **k8s Secret** holding `keystore.pem` (from the resolved keystore
  content) and **mount** it read-only at `/etc/trino/tls/` on the coordinator.
- Auth = `http-server.authentication.type=JWT` (not PASSWORD,JWT) +
  `jwt.principal-field=preferred_username` + `jwt.key-file=<jwks>`. Do **not**
  inject the `egret_delegator` password user or seed impersonate policies for jwt
  mode (those are impersonate-only, gated on `is_impersonate_delegation()`).

## 3. Materialization (`egret-engine-mgmt/src/assignment.rs`)

Extend `delegated_conn_options(engine)`:
- **jwt** engine ⇒ `options = { "http_scheme": "https", "tls_cert":
  <engine.coordinator_cert> }`, port = **8443** (the https port), `secret_id =
  None`. No `impersonate` and no `user` (the bearer JWT authenticates; the
  connector's basic-auth principal is irrelevant on the bearer path).
- **impersonate** engine ⇒ unchanged (`{user, impersonate:true}` + delegator
  secret, http port).

The jwt DB row is `delegated=true` with the mint path selected in routes (already:
`impersonate != true`). The cert is public, so it rides in `options.tls_cert`.

## 4. Connector (`egret-query/src/execution/connector/trino.rs`)

- Add `ConnSpec.tls_cert: Option<String>` (PEM), populated from `options.tls_cert`
  in `ConnSpec::from_database_as`.
- In `TrinoConnector::execute`, build the reqwest client with
  `.add_root_cert(reqwest::Certificate::from_pem(cert.as_bytes())?)` when
  `tls_cert` is set (pins the engine cert = server-auth + encryption). Confirm
  reqwest's TLS feature is enabled in `egret-query/Cargo.toml`.
- Everything else already works over TLS: the base URL uses `http_scheme` (=https,
  port 8443), the bearer path sends `bearer + X-Trino-User=self`, and the #255
  `rebase` keeps `nextUri` on our https base.

## 5. Routes (`egret-query/src/execution/routes.rs`)

No change — the delegated branch already: `impersonate == Some(true)` ⇒ impersonate;
else ⇒ **mint the caller's JWT (403 on `NoRefreshToken`)**, which is the jwt path.
The jwt DB row (no `impersonate`) takes this branch, and `tls_cert` flows via
`options`.

## 6. Security model

- **Identity** is bound to the JWT (minted for the caller, validated by JWKS); a
  spoofed `X-Trino-User` (different user) is rejected over TLS.
- **Wire encryption** via TLS; the JWT never travels cleartext.
- **Server auth** via cert pinning (`add_root_cert` of the engine's cert) — no
  MITM even on the internal network.
- **No shared credential** — no delegator, no password. Compromise blast-radius =
  one user for the token TTL (~300s).
- **OIDC-only**: mint requires the caller's stored refresh token; non-OIDC callers
  get 403 (they must use an impersonate engine).

## 7. Out of scope

- cert-manager / external CA issuance (egret self-signed is the chosen approach).
- Rotating certs automatically (manual re-provision rotates).
- Mutual TLS (client certs) — the JWT is the client credential.
- StarRocks jwt delegation parity.

## 8. Testing

- **Unit:**
  - cert-gen: produces a parseable cert whose SAN contains the `.svc` host +
    `localhost`.
  - provisioner: jwt mode renders `https.enabled/port/keystore.path` + the keystore
    Secret + mount; does NOT render the delegator user/policy; impersonate mode is
    unchanged (no https).
  - `is_jwt_delegation()` on `Engine`.
  - materialization: jwt engine ⇒ `options.http_scheme=https` + `tls_cert` set +
    port 8443 + no secret; impersonate unchanged.
  - connector: with `tls_cert` set, the built client is configured (assert via a
    small constructor test / that `Certificate::from_pem` succeeds).
- **Live (manual):** provision a jwt-mode engine (TLS boots), assign to a
  workspace, and as an **OIDC** user run a delegated query → runs as that user over
  TLS (`SELECT current_user` = the caller); a non-OIDC caller gets 403.

## Success criteria

1. A jwt-mode coordinator boots over HTTPS with the egret cert.
2. A delegated query by an OIDC user runs as that user over TLS (JWT-bound).
3. No `egret_delegator`/impersonate artifacts exist on a jwt engine.
4. egret's connector validates the engine's pinned cert (no accept-invalid).
5. Non-OIDC callers are denied (403) on a jwt engine; impersonate engines unchanged.
