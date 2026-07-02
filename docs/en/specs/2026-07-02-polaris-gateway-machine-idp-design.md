# egret Polaris Gateway + Machine-IdP — Design

> Status: approved design (brainstorm). Supersedes parts of `2026-07-02-control-plane-polaris-catalog-design.md` (the connection/catalog/RBAC/assignment control-plane, now merged in PR #115). VI reference: `docs/vi/specs/2026-07-02-polaris-gateway-machine-idp-design.md`.

## Motivation

The merged control-plane Polaris epic lets a platform admin drive Polaris's **management** API (catalogs, RBAC, assignment) and mints a Polaris-native principal per assignment. Live UI testing + review surfaced four corrections:

1. **Expose Polaris through egret** — engines/tools/notebook should reach the Iceberg REST catalog via a single egret edge; Polaris must not be exposed directly.
2. **RBAC belongs inside a catalog** — catalog-roles + grants are per-catalog in Polaris; the flat `/control-plane/rbac` page mixed them with global identities.
3. **Surface the machine credential** — the "internal client" engines use to reach Polaris was buried in Secrets, not a first-class managed entity.
4. **Don't tie machine credentials to the enterprise IdP** — provisioning per-connection client credentials in an enterprise IdP (EntraID) goes through security/IT and is impractical. And Polaris's own `/v1/oauth/tokens` is deprecated-for-removal, so it is not a durable foundation either.

## The core principle — three identity planes, three issuers

| Plane | Identity | Issuer | Who provisions |
|---|---|---|---|
| **Human** — login to egret, per-user engine governance | real user | **Enterprise IdP** (EntraID / their Keycloak) — egret already does OIDC login | enterprise security (SSO) |
| **Machine** — Catalog Client (engine / tool / notebook) → catalog | service identity | **egret mini machine-IdP** (embedded, egret-owned) | egret, freely, no external dependency |
| **AuthZ** — what a machine identity may do on a catalog | Polaris principal/role + grants | **Polaris** (RBAC) | egret control plane, server-side |

**AuthN of machines is egret's; authZ is Polaris's; human SSO is the enterprise IdP's.** Machine credentials never touch the enterprise IdP, and consumers never call Polaris's deprecated token endpoint.

## Decisions locked (from brainstorm)

- **Consumers** of the catalog (through the egret edge): query engines (Trino/StarRocks, service connections), external/BI tools (Spark/pyiceberg/DuckDB), and the notebook (Daft, direct Iceberg). No end-user connects to Polaris directly.
- **Credential granularity:** one credential **per consumer** ("Catalog Client"), revocable + auditable independently.
- **Proxy surface:** transparent reverse-proxy for the Iceberg REST **data-plane** only (`/catalog/v1/*`); Polaris management API stays internal (egret server-side); Polaris is not exposed.
- **Machine auth:** an **egret-embedded mini machine-IdP** issues machine tokens (client_credentials). Not Polaris-native (deprecated endpoint), not Keycloak (too heavy), not the enterprise IdP.
- **The proxy adds NO auth** — it forwards; Polaris authenticates (via egret's JWKS) and authorizes.

## Architecture — four pillars

```
                                  egret-server (single edge)
Trino / StarRocks  ─┐   ┌───────────────────────────────────────────────┐
Spark / pyiceberg  ─┼──▶│ POST /oauth/token        (mini machine-IdP)     │→ signs JWT (egret key)
Notebook (Daft)    ─┘   │ GET  /.well-known/{openid-configuration,jwks}   │
                        │ ANY  /catalog/v1/*       (transparent proxy) ───┼──▶ Polaris (internal only)
                        │ /api/control-plane/*     (mgmt, server-side) ───┼──▶ Polaris (internal only)
                        └───────────────────────────────────────────────┘        └──▶ MinIO / S3
   consumer flow:  clientId/secret ─▶ /oauth/token ─▶ JWT ─▶ /catalog/v1/* ─▶ Polaris verifies JWT via egret JWKS
```

### Pillar 1 — Transparent Iceberg-REST proxy (`/catalog/v1/*`)

- A reverse-proxy in `egret-server` mounts `/catalog/v1/*` and **forwards** method + path + `Authorization` header + body + query to Polaris's Iceberg REST base (`{polaris}/api/catalog/v1/*`). No reading, no auth injection, no opaque-token layer (unlike the removed lakekeeper `front_auth`, which existed only because lakekeeper could not issue/validate tokens).
- **One technical concern (not auth):** the `GET /v1/config` response may advertise Polaris's internal URLs (`overrides.uri`, `s3.signer.uri`). Rewrite those to egret's public host so follow-up calls (incl. S3 remote-signing) route back through egret, not straight to internal Polaris.
- Polaris binds to the internal network only; egret is the sole ingress.

### Pillar 2 — egret mini machine-IdP + Catalog Clients

A minimal OAuth2/OIDC **provider** embedded in egret (NOT a full Keycloak):

- `POST /oauth/token` — `grant_type=client_credentials`, `client_id` + `client_secret`. Verifies against the `catalog_clients` store, mints a **signed JWT**: `iss` = egret issuer URL, `sub` = client id, `aud` = `polaris`, a roles/principal claim (see Pillar 4), `exp` (short TTL, e.g. 1h).
- `GET /.well-known/openid-configuration` — OIDC discovery (issuer, `jwks_uri`, `token_endpoint`) so Polaris auto-configures.
- `GET /.well-known/jwks.json` — public signing key(s) with `kid`, so Polaris verifies signatures.
- Signing: RS256 (or ES256); egret generates + persists a keypair (rotatable, published in JWKS with `kid`). Library: `jsonwebtoken` (Rust). No external service.

**Catalog Client** = a first-class entity (the "internal client" made explicit):
- `catalog_clients { id, name, consumer_type (trino|starrocks|notebook|external), workspace_id, catalog_name, catalog_role, client_id, client_secret_hash, created_at, last_used_at }`.
- Lifecycle: **create** (admin picks consumer + workspace + catalog + catalog-role → egret generates `client_id`/`client_secret` [secret shown once], ensures the matching Polaris authZ objects exist [Pillar 4], stores the client) → **use** (consumer configures the credential + points its catalog URI + token URI at egret) → **revoke** (delete the row → its token can no longer be minted; independent per client).
- The distinct set of `(workspace_id, catalog_name)` across clients **replaces** the `workspace_catalog_assignments` table — creating a client for `(ws, catalog, consumer)` IS the act of granting that consumer access.

### Pillar 3 — Catalog-scoped RBAC

Matches Polaris's two tiers; **removes the flat `/control-plane/rbac` page**:
- **Inside the catalog detail page** (`/control-plane/catalog/{name}`):
  - **Catalog roles** (e.g. `reader`, `writer`, `admin`) + **Grants** (privileges on this catalog: catalog/namespace/table level).
  - **Clients** — the Catalog Clients accessing this catalog, each bound to a catalog-role.
- **Global (light):** a read-only overview of clients/identities; no free-standing RBAC editing.

### Pillar 4 — Polaris EXTERNAL-OIDC trusting egret

- Polaris runs `authentication.type = external` (or `mixed`) with `quarkus.oidc.auth-server-url = {egret issuer}` and `token.audience = polaris` → it validates egret-signed JWTs via egret's JWKS.
- **Principal/role mapping:** Polaris maps a JWT claim to its RBAC. Two options, decided during a config spike:
  - (a) **roles claim → catalog-role**: egret's JWT carries `roles: [<catalog-role>]`; Polaris maps the roles claim to catalog-roles directly (fewest Polaris objects — egret creates only catalog-roles + grants, no per-client principal).
  - (b) **sub → principal**: egret creates a Polaris principal per client + Polaris maps `sub` → that principal (more Polaris objects, closer to Polaris's native model).
  - **Default: (a)** — egret owns authN + client identity; Polaris only holds catalog-roles + grants for authZ.
- **egret-admin (management token):** egret's control plane calls Polaris management server-side. It authenticates with a token from the **same egret mini-IdP** (an internal admin client mapped to a Polaris admin role), OR — dev/simplest — a bootstrap Polaris root. (No enterprise IdP, no separate Keycloak.)

## Data model changes

- **New:** `catalog_clients` (above). Signing key material persisted (a `oidc_signing_keys { kid, algorithm, private_pem, public_jwk, created_at, retired_at }` table, or a secured file — decided in the plan).
- **Dropped/superseded:** `workspace_catalog_assignments` (folded into `catalog_clients`). The connection's Keycloak-token fields become optional/unused for the machine path.
- Migrations are additive; the assignment table drop is a greenfield migration (no prod data).

## What this supersedes (from PR #115)

- Assignment provisioning of a Polaris-native principal per `(ws, catalog)` → replaced by per-consumer Catalog Clients authenticated by the egret mini-IdP.
- Flat `/control-plane/rbac` page → catalog-scoped RBAC in the catalog detail page.
- Polaris dev auth INTERNAL (root/s3cr3t) → EXTERNAL-OIDC trusting egret (dev may keep root for the admin path only).
- The `egret-polaris` client (management API) is **kept** — still used server-side for catalog/role/grant management.

## Security considerations

- `client_secret` stored **hashed** (argon2/bcrypt) in `catalog_clients`; shown once at creation.
- JWT: short TTL, `aud=polaris` enforced by Polaris, signature via rotatable JWKS (`kid`); key rotation retires old keys after TTL.
- The proxy forwards `Authorization` verbatim — it must not log tokens.
- `/oauth/token` is rate-limited + constant-time secret comparison.

## Out of scope / future

- Un-pausing the notebook to read/write Iceberg directly via Daft + a Catalog Client (its own sub-epic).
- Auto-pushing a Catalog Client's credential into an engine's connector config (engines-centralization / K8s-provisioning epic).
- Per-user OIDC-to-Polaris + column/row governance at the catalog (stays at the engine / Ranger).
- Enterprise-IdP integration for machines (explicitly rejected — machines use the egret mini-IdP).

## Risks

- **Polaris OIDC mapping** (Pillar 4) is the least-pinned piece — the exact Quarkus/Polaris principal-role-mapping config needs a short spike against `apache/polaris` before the routes/e2e are finalized.
- Running an embedded IdP adds signing-key lifecycle + JWKS/discovery correctness as egret responsibilities (bounded, but real).

## Decomposition (one plan per pillar)

1. **P-A** — transparent `/catalog/v1/*` proxy + `/v1/config` URL rewrite.
2. **P-B** — egret mini machine-IdP (`/oauth/token`, JWKS, discovery, signing-key store) + `catalog_clients` + management.
3. **P-C** — catalog-scoped RBAC UI (catalog detail: roles + grants + clients); remove flat `/rbac`; fold assignment.
4. **P-D** — Polaris EXTERNAL-OIDC config (dev compose) + principal/role mapping spike + e2e (client → token → proxy → Polaris → MinIO).
