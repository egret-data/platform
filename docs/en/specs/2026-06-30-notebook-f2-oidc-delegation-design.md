# Notebook F2 — Unified OIDC Delegation (design)

> Status: draft for review · 2026-06-30 · depends on F1 ([[egret-notebook-access-model]]), D1a (kernel callback + helpers)
> Bilingual: English canonical; Vietnamese reference at `docs/vi/specs/2026-06-30-notebook-f2-oidc-delegation-design.md`.

## 1. Goal & motivation

Make the notebook kernel (Daft) read **Iceberg, Trino, and StarRocks as the end user**, governed per-user, with **no privileged credential ever held by the kernel** — because the kernel runs the user's arbitrary code.

This replaces F1's two stopgaps with one unified primitive:
- F1 Trino **impersonation** (a privileged principal sets `X-Trino-User`) → **per-user OIDC JWT** (the user authenticates as themselves).
- F1 per-user **opaque catalog token** (mint/revoke lifecycle) → the **same OIDC JWT**, validated by the catalog directly.

Result: **one OIDC issuer (Keycloak)** authenticates the user everywhere — egret, the Iceberg catalog, Trino, StarRocks. This is the platform's monetised governance feature, so it is built and tested against a **real Keycloak**.

### Why not impersonation in the kernel
A privileged "act-as" credential in the kernel process is readable by the user's code → they could forge any identity (privilege escalation). The fix is **delegation, not impersonation**: the kernel only ever holds a short-lived token that *is* the user, scoped to the user's own access. See the rejected alternatives in §9.

## 2. Scope

In scope (F2):
1. **Keycloak realm** wiring: one issuer, group claims, clients/audiences for egret + catalog + Trino + StarRocks; dev seed users/groups.
2. **egret token broker**: exchange the logged-in user's session for a short-lived, audience-scoped **engine JWT**, delivered to the kernel via the F1 runtime manifest; refreshed before expiry.
3. **egret-catalog JWT authn** + default authorization policy (`rw` on `notebook_{user}`; group-based `read` on shared namespaces). Opaque `catalog_tokens` retained for service/engine tokens (hybrid).
4. **Trino** OIDC/JWT authentication against Keycloak + **native group-based authorization** (file-based access control; no Ranger in MVP).
5. **StarRocks** JWT security integration against Keycloak + native RBAC keyed on groups.
6. **Daft client seam**: connection factory + pyiceberg `RestCatalog` that present the per-user JWT, fetching a fresh token from egret per connection.
7. **Real-Keycloak end-to-end integration test** proving per-user authorization boundaries.

Out of scope (later / separate):
- **Apache Ranger** as the engine authorizer (a separate ops epic; F2 uses native engine authz keyed on the same Keycloak groups).
- **Per-user S3 vended credentials** for data-file isolation — see §7; F2 governs *catalog/query metadata* per-user; data-file (MinIO) credential isolation is flagged as a dependency that may phase into F2-P6 or a storage sub-spec (B).
- The notebook **execution UX** (cell run, output render, recorder) — that is spec **E**, built on F2.

## 3. Architecture

```
                 Keycloak (1 OIDC issuer, 1 JWKS, group claims)
                        ▲ login (code flow)          ▲ JWKS validate
                        │                            │
   browser ──login──> egret ──token-exchange (RFC 8693)──> short-lived engine JWT
                        │   (egret = confidential client; holds user's refresh token)
                        │
                        │  GET /api/ws/{ws}/notebook-runtime  (F1 manifest, extended)
                        ▼
                     kernel (Daft + helpers)  ── connection factory fetches fresh JWT ──┐
                        │                                                               │
                        ├─ pyiceberg RestCatalog(token=JWT) ──► egret-catalog (JWT authn)│
                        ├─ read_sql(trino,  conn=factory)   ──► Trino   (JWT authn + ACL)│
                        └─ read_sql(starrocks, conn=factory)──► StarRocks (JWT sec-int)  ┘
```

Identity flows once (Keycloak); every downstream system validates the same signed token (issuer + audience + JWKS) and maps `principal`/`groups` to its own authorization.

## 4. Components

### 4.1 Keycloak realm (`deploy/keycloak/egret-realm.json`, extend existing)
- One realm `egret`. Existing egret client + group-membership mapper already present.
- Add audiences/clients so a single token can be accepted by catalog + Trino + StarRocks. Approach: a **protocol mapper that adds the required `aud`** entries (`egret-catalog`, `trino`, `starrocks`) to tokens egret mints via exchange, OR per-service clients each configured to accept the shared audience. (Decision recorded in §6: align on a shared/multi-audience token; each service's `required_audience` matches.)
- Enable **token exchange** (Keycloak feature) for the egret confidential client.
- Dev seed: groups (`data-engineers`, `analysts`, …) + ≥2 test users in different groups, for the authz boundary test.

### 4.2 egret token broker (`egret-auth` + `egret-bin`)
- egret keeps the user's **refresh token** from OIDC login (already does code-flow login; persist refresh token in the session).
- New seam `EngineTokenBroker::mint_engine_token(user, workspace) -> { access_token, expires_at }`:
  - Performs **RFC 8693 token exchange** against Keycloak: subject = user's token, requested audience = engines+catalog, downscoped. Returns a short-TTL access JWT.
- **Manifest extension** (F1 `GET /api/ws/{ws}/notebook-runtime`): replace the per-user 1-grant `catalog.token` with the **engine JWT** (or carry both during migration); add `token_expires_at` and a refresh endpoint `POST /api/ws/{ws}/notebook-runtime/token` (kernel-token authed, D1a) that returns a fresh engine JWT. The kernel/connection-factory calls this when the cached token nears expiry.
- The kernel→egret call is authenticated by the **kernel-token** (D1a), which identifies the (user × workspace); egret returns *that user's* engine JWT only. The kernel never receives a privileged/impersonation credential.

### 4.3 egret-catalog JWT authn + authz (`egret-catalog-engine/front_auth.rs`, `egret-catalog`)
- **AUTHN** (extend `front_auth`): if the `Bearer` is a JWT signed by the trusted issuer → validate via the egret-auth OIDC verifier (JWKS, `iss`, `aud`, `exp`) → resolve principal → egret user (`find_by_external_subject`). Else fall back to the existing opaque `catalog_tokens` lookup (service/engine tokens, hybrid).
- **AUTHZ** (default policy from principal): `user → rw` on namespace `notebook_{username}` within the caller's workspace warehouse; `read` on shared/governed namespaces granted to the user's **groups**/membership. The existing prefix(warehouse)-belongs-to-workspace ownership check is unchanged.
- `notebook_{username}` namespace auto-provisioned (F1 already provisions it). "Full rights" = create/read/write/drop tables **within their own namespace only**.

### 4.4 Trino (`deploy/trino/`)
- `config.properties`: enable JWT authentication (`http-server.authentication.type=JWT`, `http-server.authentication.jwt.key-file=<keycloak JWKS url>`, principal field + user-mapping rules). Keep OAuth2 available for browser login if desired (Trino supports multiple auth types).
- **Authorization**: file-based system access control (`rules.json`) keyed on the token's **groups** (Keycloak group claim → Trino group → catalog/schema/table rules). No Ranger in MVP.
- TLS/JWKS reachability between Trino and Keycloak inside the compose network documented.

### 4.5 StarRocks (`deploy/starrocks/` init)
- `CREATE SECURITY INTEGRATION <name> PROPERTIES("type"="authentication_jwt", "jwks_url"=<keycloak>, "principal_field"=…, "required_issuer"=…, "required_audience"=…)` (server plugin `AUTHENTICATION_JWT`, confirmed in StarRocks `AuthPlugin.Server`).
- Group provider + RBAC: map principal/groups → roles/grants (native RBAC). Provisioned via init SQL run against the allin1 container.
- **Client-side spike (tracked):** confirm the Python MySQL driver Daft uses (SQLAlchemy → pymysql/mysqlclient) can present the JWT to StarRocks' `AUTHENTICATION_OPENID_CONNECT_CLIENT` plugin (commonly via the cleartext client plugin). This gates the StarRocks-direct path; until proven, StarRocks SQL falls back to the D1a governed callback.

### 4.6 Daft client seam (`kernel-image/egret_kernel_helpers`)
- A **connection-factory broker**: `Callable[[], Connection]` that fetches a fresh engine JWT from egret (via the kernel-token + refresh endpoint) and builds a SQLAlchemy connection presenting it — Trino (`trino` dialect, access-token), StarRocks (mysql dialect, JWT via client plugin). Daft `read_sql` accepts a connection factory and calls it per scan task (verified in Daft source `daft/sql/sql_connection.py:from_connection_factory`, `daft/sql/sql_scan.py`); ConnectorX is bypassed (it requires a URL string) — fine, since Trino/StarRocks are SQLAlchemy-only anyway.
- `catalog()` (D1a) updated: `RestCatalog(token=<engine JWT>)` instead of the opaque catalog token; refresh on `401`/near-expiry.
- The `%%egret_sql --target` magic (spec E) routes `daft` → Daft-direct over the attached Iceberg catalog; `<engine>` → either Daft-direct `read_sql` (governed by JWT) once §4.5 spike passes, or the D1a callback as the safe default.

## 5. Data flow — running an engine query as the user

1. Kernel needs to query Trino. Connection factory calls `POST /api/ws/{ws}/notebook-runtime/token` (kernel-token auth) → egret token-exchange → fresh engine JWT for *that user*.
2. Factory builds a Trino SQLAlchemy connection presenting the JWT; Daft `read_sql` runs (pushdown/partitioning available).
3. Trino validates the JWT (Keycloak JWKS, iss/aud) → principal = user; file-based ACL enforces by group → only the user's authorized catalogs/schemas are readable.
4. Iceberg path: `catalog()` builds `RestCatalog(token=JWT)`; egret-catalog validates the JWT → default policy `rw notebook_{user}` + group read on shared; Daft reads via pyiceberg.
5. StarRocks path: connection presents the JWT; StarRocks JWT security integration validates → RBAC by group.

The user cannot forge another identity: the token is signed by Keycloak and minted for them by egret; the kernel holds no credential that grants more than the user's own access.

## 6. Key decisions (recorded)

- **Issuer**: Keycloak (external IdP already in dev stack + egret OIDC login). egret is a confidential client + token broker (not the issuer).
- **Kernel token model**: short-lived, **downscoped engine JWT via RFC 8693 token exchange**; refreshed on demand; never the user's full SSO token, never a privileged principal.
- **Audience**: align all services on a shared/multi-audience token; each service's `required_audience` matches what egret requests in the exchange.
- **Authorization**: **native engine authz keyed on Keycloak groups** (Trino file-based ACL, StarRocks RBAC, catalog default+group policy). Ranger deferred.
- **Catalog auth**: **hybrid** — JWT for users (default `notebook_{user}` rw), opaque `catalog_tokens` retained for service/engine tokens.
- **Revises** [[egret-notebook-access-model]]: "1-grant catalog token" → "OIDC JWT delegation".

## 7. Per-user data-file (S3) isolation — dependency note

The JWT governs **catalog metadata and query authorization**. Reading Iceberg **data files** in MinIO still uses storage credentials via pyiceberg FileIO. True per-user storage isolation requires the catalog to **vend short-lived, scoped S3 credentials per request** (lakekeeper-style vended-credentials). This is flagged as **F2-P6 (optional)** or a storage (B) concern; MVP may use a shared workspace storage credential while metadata/authz is per-user. Documented so it is a conscious gap, not an accident.

## 8. Error handling

- Invalid/expired JWT at any service → `401`; the connection factory refreshes once and retries; persistent failure surfaces a clear error to the cell.
- Token exchange failure (Keycloak down, exchange disabled, refresh token expired) → egret returns a typed error; kernel reports "re-login required".
- Audience/issuer mismatch → `401` at the service; covered by the integration test (negative case).
- Authz denial (group lacks grant) → `403`; surfaced verbatim (no silent empty results).
- StarRocks client-plugin spike unresolved → engine SQL stays on the D1a callback (no silent bypass).

## 9. Rejected alternatives

- **Impersonation header from the kernel** (`X-Trino-User` set in-kernel): requires a privileged principal in the kernel → user code forges identities. Rejected (privilege escalation).
- **Identity-injecting Trino proxy** (egret rewrites `X-Trino-User`, holds the privileged principal): safe and keeps Trino's impersonation model, but adds a Trino-protocol proxy to build/run. Kept as a fallback if per-user OIDC on an engine proves infeasible; not the primary design.
- **Full SSO access token to the kernel**: over-scoped; rejected in favour of downscoped exchange.

## 10. Testing

- **Unit (Rust, `#[sqlx::test]`)**: token-broker exchange (mocked Keycloak token endpoint); catalog `front_auth` JWT branch (valid/expired/wrong-aud/wrong-iss → 200/401) + default-grant authz (own namespace rw, other user's namespace 403); manifest/refresh endpoint (kernel-token auth, returns user-scoped token only).
- **Unit (Python pytest, extends D1a)**: connection-factory builds a JWT-bearing connection (mocked egret token endpoint, faked SQLAlchemy/daft); `catalog()` passes JWT + refreshes on 401.
- **Real-Keycloak end-to-end integration (scripted, documented; not CI — like the existing manual Trino test [[egret-manual-trino-integration-test]])**:
  - Bring up `deploy/docker-compose.dev.yml` (keycloak + trino + starrocks + minio + egret).
  - Two Keycloak users in different groups.
  - As **user A**: Daft reads (a) Iceberg `notebook_A` (rw), (b) a Trino schema A's group is granted, (c) StarRocks (if spike passed). As **user B**: assert A's `notebook_A` is **not** readable (403), and a Trino/StarRocks object B's group lacks is denied.
  - Assert the kernel holds only the downscoped token; a forged `aud`/`iss` is rejected.
- **Security review (adversarial, required — this is the money feature)**: token confusion/replay, audience misuse, kernel credential reach, refresh-token handling, downscope correctness, ACL bypass attempts.

## 11. Phasing (one spec, phased plan)

- **F2-P1** — Keycloak realm (clients/audiences/groups/seed users, token-exchange enabled) + egret token broker (exchange + manifest extension + refresh endpoint).
- **F2-P2** — egret-catalog JWT authn + default/group authz (hybrid with opaque tokens).
- **F2-P3** — Trino OIDC/JWT config + file-based group ACL (dev) + Daft Trino connection factory.
- **F2-P4** — StarRocks JWT security integration + RBAC + Daft StarRocks factory (incl. the client-plugin spike).
- **F2-P5** — real-Keycloak end-to-end integration test + adversarial security review.
- **F2-P6 (optional)** — per-user vended S3 credentials (§7).

## 12. Self-review checklist

- No placeholders/TBDs except the explicitly-flagged §4.5 client-plugin spike and §7 S3 dependency.
- Internal consistency: issuer=Keycloak, delegation-not-impersonation, hybrid catalog auth — applied consistently across §3–§11.
- Scope: foundation only; execution UX is spec E. Phased for independent delivery.
- Ambiguity: audience strategy (§4.1/§6) and StarRocks client plugin (§4.5) are the two integration risks; both have a defined fallback.
