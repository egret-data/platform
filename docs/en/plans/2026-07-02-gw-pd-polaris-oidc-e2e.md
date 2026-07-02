# Gateway P-D: Polaris EXTERNAL-OIDC + end-to-end Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-polaris-gateway-machine-idp-design.md` (Pillar 4). **P-D (final) of the gateway epic.** Depends on P-A (proxy), P-B (egret IdP), P-C (roles/grants UI). VI reference: `docs/vi/plans/2026-07-02-gw-pd-polaris-oidc-e2e.md`.

**Goal:** Make Polaris trust egret's mini-IdP (EXTERNAL/OIDC), map the JWT's `roles` claim to Polaris catalog-roles, and prove the whole loop: create a Catalog Client → `/oauth/token` → `/catalog/v1/*` proxy → Polaris verifies via egret JWKS → read/write an Iceberg table on MinIO.

**Architecture:** Configure the dev Polaris container `authentication.type=external` (or `mixed`) with `quarkus.oidc.auth-server-url` = egret's issuer (reachable from the Polaris container) and `token.audience=polaris`. Confirm the claim→role mapping via a short spike against `apache/polaris`. Then wire an engine + run the e2e.

**Tech Stack:** docker-compose, Apache Polaris (Quarkus OIDC), egret mini-IdP, MinIO, Trino.

---

## Background the implementer needs

- Dev Polaris runs from `deploy/docker-compose.dev.yml` (currently INTERNAL auth, bootstrap `root/s3cr3t`). egret runs on the **host** (`localhost:8080`); a container reaches the host via `host.docker.internal`.
- egret mini-IdP (P-B) serves `/.well-known/openid-configuration`, `/.well-known/jwks.json`, `/oauth/token`; JWT `iss` = egret's configured public root (`EGRET_CATALOG__PUBLIC_URL` root). **Issuer consistency is the key wrinkle:** the `iss` egret stamps MUST equal the issuer Polaris resolves from `auth-server-url` (Quarkus validates `iss`). In dev, set egret's issuer + Polaris's `auth-server-url` to the SAME URL Polaris can reach — `http://host.docker.internal:8080` — and have external clients use the same (or relax issuer strictness). Report what worked.
- Polaris OIDC config keys + principal/role mapping live in Polaris's `application.properties` / docs — the exact `polaris.oidc.*` principal-role-mapper keys are the **least-pinned piece**; spike them (Task 1) before finalizing.

---

## Task 1: Polaris EXTERNAL-OIDC config (spike)

**Files:** Modify `deploy/docker-compose.dev.yml` (polaris env); Create `deploy/polaris/oidc.md` (findings)

- [ ] **Step 1: Configure OIDC against egret**

Set on the `polaris` service:
```yaml
      polaris.authentication.type: external          # or "mixed" to keep root for admin
      quarkus.oidc.auth-server-url: http://host.docker.internal:8080   # egret issuer (discovery lives here)
      quarkus.oidc.client-id: polaris                # audience
      quarkus.oidc.token.audience: polaris
      # roles mapping: point Polaris at the JWT "roles" claim (confirm exact key vs Polaris docs)
      quarkus.oidc.roles.role-claim-path: roles
```
Bring egret `serve` up first (so `/.well-known/openid-configuration` + JWKS are live), then restart Polaris. Confirm Polaris starts + fetches egret's discovery/JWKS (check Polaris logs for OIDC init, no JWKS error).

- [ ] **Step 2: Spike the claim → catalog-role mapping**

Create a Catalog Client (P-B) for `(lakehouse, writer)` → mint a token at `/oauth/token`. Call a Polaris **management or catalog** endpoint through the proxy with that Bearer and confirm Polaris (a) accepts the egret-signed JWT (signature + `iss` + `aud`), and (b) authorizes per `writer`'s grants. Determine the exact Polaris config that maps `roles:["writer"]` → the catalog-role `writer` on `lakehouse` (may be `polaris.oidc.principal-roles-mapper.*` / a role-claim path + a principal-mapper). **Document the working config in `deploy/polaris/oidc.md`.** If Polaris needs a principal per identity (mapping option (b) in the spec), fall back to that and note it.

- [ ] **Step 3: Commit**
```bash
git add deploy/docker-compose.dev.yml deploy/polaris/oidc.md && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(dev): Polaris EXTERNAL-OIDC trusting egret mini-IdP (+ mapping findings)"
```
(Co-Authored-By trailer.)

---

## Task 2: Trino connector via the egret proxy + Client credential

**Files:** `deploy/trino/catalog/iceberg.properties` (gitignored — document in `deploy/polaris/engine-wiring.md`)

- [ ] **Step 1: Point Trino at egret (not Polaris) using a Catalog Client**

Update the Iceberg REST connector to go through egret + authenticate via egret's IdP:
```properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://host.docker.internal:8080/catalog     # egret proxy, not polaris:8181
iceberg.rest-catalog.warehouse=lakehouse
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<client_id>:<client_secret>    # a Catalog Client (consumer_type=trino)
iceberg.rest-catalog.oauth2.server-uri=http://host.docker.internal:8080/oauth/token   # egret IdP
iceberg.rest-catalog.oauth2.scope=catalog
fs.native-s3.enabled=true
s3.endpoint=http://minio:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=egret
s3.aws-secret-key=egretsecret
```
Restart Trino; confirm the `iceberg` catalog loads (logs: "Added catalog iceberg"). Document in `deploy/polaris/engine-wiring.md` that both `uri` and `oauth2.server-uri` are egret (Polaris hidden), and the credential is a Catalog Client.

- [ ] **Step 2: Commit the doc**
```bash
git add deploy/polaris/engine-wiring.md && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "docs(dev): Trino via egret proxy + Catalog Client credential"
```

---

## Task 3: End-to-end (opt-in) + verify

**Files:** Create `frontend/tests/e2e/polaris-gateway.spec.ts`; modify `frontend/tests/e2e/README.md`

- [ ] **Step 1: e2e — the full gateway loop**

Skipped unless the live stack (Polaris EXTERNAL-OIDC + egret serve + Trino wired) is up:
1. Platform admin: create catalog `lakehouse` → add catalog-role `writer` + a `CATALOG_MANAGE_CONTENT` (or table write) grant → create a Catalog Client (`trino`) for `(lakehouse, writer)`.
2. Directly assert the machine path: `POST http://host.docker.internal:8080/oauth/token` (client creds) → JWT; `GET http://host.docker.internal:8080/catalog/v1/config` with the Bearer → 200 + `uri`/`s3.signer.uri` rewritten to egret; then a namespace/table create through the proxy → Polaris commits to MinIO → read back.
   (If driving via Trino: `CREATE TABLE lakehouse.e2e.t AS SELECT 42 n` then `SELECT n` → 42.)

- [ ] **Step 2: README** — document the setup (Polaris EXTERNAL-OIDC + egret issuer reachable from Polaris + a Trino Catalog Client + the issuer-consistency note from Task 1).

- [ ] **Step 3: Full CI (local; GitHub CI is out of quota)**

`cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `DATABASE_URL=… cargo test --workspace`; frontend `typecheck`+`lint`+`test`+`build`; kernel `pytest`.

- [ ] **Step 4: Commit**
```bash
git add frontend/tests/e2e && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "test(e2e): Polaris gateway loop (client -> egret IdP -> proxy -> Polaris -> MinIO)"
```

---

## Self-review notes (spec coverage for P-D)

- Spec **Pillar 4**: Polaris EXTERNAL-OIDC trusting egret's issuer + `aud=polaris` (Task 1), `roles` claim → catalog-role mapping (Task 1 spike, documented), engine wired through the proxy with a Catalog Client (Task 2), full e2e (Task 3).
- **Least-pinned (flagged):** exact Polaris `polaris.oidc.*` mapping keys + the issuer-URL consistency between egret's `iss` and Polaris's `auth-server-url` — resolved in the Task 1 spike + documented in `deploy/polaris/oidc.md`.
- **Epic complete after P-D:** P-A (proxy) → P-B (IdP + clients) → P-C (catalog RBAC UI) → P-D (Polaris trust + e2e). egret is the sole edge; machines authenticate via egret's mini-IdP; Polaris hidden + authorizes; no dependency on Polaris's deprecated token endpoint, no Keycloak/EntraID for machines.
