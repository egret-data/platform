# egret Embedded Catalog — Design (Architecture B)

> Status: approved design (brainstorm). **Supersedes** `2026-07-02-control-plane-polaris-catalog-design.md` (merged in PR #115) and `2026-07-02-polaris-gateway-machine-idp-design.md`. VI reference: `docs/vi/specs/2026-07-02-egret-embedded-catalog-design.md`.

## Why this supersedes the Polaris direction

egret migrated from embedded lakekeeper → Apache Polaris chasing one feature: Polaris **issues its own tokens** (lakekeeper does not). But the target end-state has egret **owning machine auth** (a mini-IdP), because: Polaris's internal token endpoint is deprecated-for-removal; Keycloak is too heavy; and provisioning per-connection machine credentials in an enterprise IdP (EntraID) is impractical.

Once egret owns the IdP, Polaris's only advantage vanishes — and running a **JVM Polaris service + a reverse-proxy** is heavier than **embedding lakekeeper (Rust, in-process)**, which is *designed* to sit behind an external IdP. So the correct architecture is: **egret is a self-contained, all-Rust catalog** — it embeds lakekeeper, issues machine tokens itself, and enforces authorization with its own Authorizer. This is a cleaner version of egret's original embedded model, modernized with JWT machine identities + catalog-scoped RBAC.

## Core principle — egret owns the catalog and its auth

| Concern | Owner |
|---|---|
| **Iceberg REST catalog** (protocol, metadata, S3) | **embedded lakekeeper** (Rust crate, in egret-server process) |
| **AuthN of machines** (issue tokens) | **egret mini machine-IdP** (`/oauth/token` + JWKS) |
| **AuthZ** (who may do what) | **egret Authorizer** (custom, plugged into lakekeeper; no OpenFGA) |
| **Human SSO** (login to egret) | enterprise IdP (EntraID / Keycloak) — separate, unchanged |

No separate catalog service, no reverse-proxy (the catalog *is* egret's own `/catalog/v1/*` routes), no JVM, no enterprise-IdP dependency for machines, no dependency on any deprecated endpoint.

## Decisions locked (from brainstorm)

- **Architecture B:** embed lakekeeper + egret mini-IdP + egret-written Authorizer (no OpenFGA).
- **Consumers:** query engines (Trino/StarRocks), external/BI tools, notebook (Daft direct) — each via a machine credential ("Catalog Client"). No end-user connects directly.
- **Credential granularity:** one credential per consumer, revocable/auditable independently.
- **Warehouse model (hybrid):** a **warehouse per workspace/domain** (data organization); inside each, **catalog-roles keyed to consumer-type** (engine / notebook / external) carry the grants; a Catalog Client binds to one role.

## Architecture

```
                         egret-server (single Rust process)
Trino / StarRocks  ─┐   ┌──────────────────────────────────────────────────────────┐
Spark / pyiceberg  ─┼──▶│ POST /oauth/token          (mini machine-IdP) → egret JWT  │
Notebook (Daft)    ─┘   │ GET  /.well-known/jwks.json                                │
                        │ ANY  /catalog/v1/*         (EMBEDDED lakekeeper routes) ────┼──▶ MinIO / S3
                        │        ├─ authN: validate egret JWT (egret JWKS)           │
                        │        └─ authZ: egret Authorizer (roles → grants)         │
                        │ /api/control-plane/*       (warehouse/role/grant/client mgmt)│
                        └──────────────────────────────────────────────────────────┘
   consumer flow: clientId/secret ─▶ /oauth/token ─▶ JWT(sub,roles,warehouse) ─▶ /catalog/v1/* ─▶ lakekeeper(authN) + egret Authorizer(authZ) ─▶ S3
```

### Component 1 — Embedded lakekeeper

- Re-add the `egret-catalog-engine` crate (recover from git history pre-P1; git-pinned lakekeeper) and mount its Iceberg REST router at `/catalog/v1/*` inside `egret-server` (as the old `mount_catalog_engine` did).
- lakekeeper owns: the Iceberg REST protocol, table/namespace metadata, S3/MinIO IO, warehouses.
- **AuthN:** configure lakekeeper's OIDC to trust egret's mini-IdP (JWKS). Because it is in-process, egret may hand lakekeeper the verification key directly instead of an HTTP JWKS round-trip — decided in an integration spike (lakekeeper's auth extension points are the least-pinned piece).
- **AuthZ:** plug egret's `Authorizer` (Component 3) into lakekeeper's pluggable `Authorizer` trait (the same seam the old `EgretAuthorizer` used).

### Component 2 — egret mini machine-IdP

- `POST /oauth/token` (`grant_type=client_credentials`, `client_id`+`client_secret`) → verify against `catalog_clients` → sign a JWT: `iss`=egret, `sub`=client_id, `warehouse`, `roles:[<catalog_role>]`, short `exp`. RS256, rotatable key (`kid`).
- `GET /.well-known/{openid-configuration,jwks.json}` — for lakekeeper's OIDC validation (and standards-compliance for external Iceberg clients).
- Backed by `oidc_signing_keys`. (Reuses the P-B design, minus Polaris.)

### Component 3 — egret Authorizer

- Implements lakekeeper's `Authorizer` trait. On each catalog operation, given the authenticated principal (from the JWT `sub`/`roles`/`warehouse`) + the target (warehouse / namespace / table) + the action (read/write/create/drop), it returns allow/deny by reading egret's RBAC store: resolve the client's role → the role's grants on the warehouse's namespaces/tables.
- Table-level granularity (matching lakekeeper's max). Column/row governance stays at the engine (Ranger).
- No external authz service (no OpenFGA) — pure egret logic over its own tables.

### Data model (egret's own store)

- `warehouses { id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at }` — one per workspace/domain; links a lakekeeper warehouse to a workspace + a global storage connection (reuse `storage_connections` from P2, kept).
- `catalog_roles { id, warehouse_id, name, consumer_type (engine|notebook|external), created_at }` — roles inside a warehouse, keyed by consumer-type.
- `catalog_grants { id, catalog_role_id, securable_type (catalog|namespace|table), namespace text[], table_name, privilege }` — a role's privileges.
- `catalog_clients { id, name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id UNIQUE, client_secret_hash, created_at, last_used_at }` — machine identities; the credential is egret-owned.
- `oidc_signing_keys { kid, algorithm, private_pem, public_jwk, created_at, retired_at }`.

### Consumers

- **Engines** (Trino/StarRocks): a Catalog Client of `consumer_type=engine`; the engine's Iceberg REST connector points `uri` + `oauth2.server-uri` at egret; egret Authorizer grants the engine role (read+write governed).
- **Notebook** (Daft direct): a Catalog Client of `consumer_type=notebook`; reads governed data + writes to its warehouse per the notebook role (reverses the write_sql-only pivot).
- **External/BI tools**: a Catalog Client of `consumer_type=external` (typically read-only role).

## What is re-added / reverted

- **Re-add** (recover from git history, pre-P1): `egret-catalog-engine` (embedded lakekeeper) + the `/catalog/v1` mount; the lakekeeper Cargo pin. Modernize the old `catalog_tokens`/`workspace_warehouses` into `warehouses` + `catalog_clients` + `catalog_roles`/`catalog_grants` (JWT machine identities + RBAC, not opaque tokens).
- **Revert** (from PR #115): `egret-polaris` crate, the control-plane routes that proxy Polaris's management API, and Polaris in docker-compose. Keep the control-plane crate + Storage/Secrets (P2) + platform-admin gating.
- **Keep:** `storage_connections` (P2), the control-plane UI shell + Storage/Secrets pages; rework the catalog/RBAC pages onto the new store.

## Security

- `client_secret` argon2-hashed, shown once; independent revoke (delete the client row → its tokens can no longer be minted).
- JWT: short TTL, `aud`/`iss` validated by lakekeeper, RS256 with rotatable `kid`.
- egret Authorizer is deny-by-default; every allow traces to a role→grant row.
- `/oauth/token`: rate-limit + constant-time secret compare; never log tokens/secrets.

## Out of scope / future

- Un-pausing the notebook to use a Catalog Client for Daft-direct reads/writes (its own sub-epic).
- Auto-pushing a Catalog Client's credential into an engine's connector config (engines-centralization / K8s epic).
- Per-user identity at the catalog + column/row governance (stays at the engine / Ranger).
- Enterprise-IdP integration for machines (explicitly rejected).
- lakekeeper federation / advanced features not needed for v1.

## Risks

- **lakekeeper auth-integration** (Component 1) is the least-pinned piece — the exact way to feed egret-issued JWTs + the egret Authorizer into the embedded lakekeeper (OIDC config vs direct key vs custom authenticator) needs an integration spike against the pinned lakekeeper revision.
- **Churn:** this reverts a merged PR direction (#115) and re-adds removed code — significant but recoverable from git history; deliberate per the strategic choice.
- egret now owns authN + authZ code (more responsibility) — the intended trade for a self-contained, all-Rust, no-JVM stack.

## Decomposition (one plan per component)

1. **EB-1** — re-embed lakekeeper (`egret-catalog-engine` recovered + mounted at `/catalog/v1`) with an `AllowAll` authorizer first, reachable in-process; revert Polaris compose.
2. **EB-2** — egret mini machine-IdP (`/oauth/token`, JWKS, discovery, signing-key store) + `catalog_clients`.
3. **EB-3** — data model (`warehouses`, `catalog_roles`, `catalog_grants`) + the egret **Authorizer** plugged into lakekeeper (deny-by-default, roles→grants) + JWT authN wiring (the integration spike).
4. **EB-4** — control-plane management (warehouses, roles/grants, clients) + revert `egret-polaris` + the Polaris control-plane routes.
5. **EB-5** — catalog-scoped RBAC UI (warehouse detail: roles-by-consumer-type + grants + clients) on the new store; remove the Polaris-era pages.
6. **EB-6** — e2e (create warehouse + role + client → `/oauth/token` → `/catalog/v1` → lakekeeper authZ → read/write on MinIO) + engine wiring + full CI.
