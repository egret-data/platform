# EB-6: engine wiring + end-to-end Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 6). **EB-6 (final).** Depends on EB-1…EB-5. VI: `docs/vi/plans/2026-07-02-eb6-e2e-engine-wiring.md`.

**Goal:** Prove the whole embedded-catalog loop and wire an engine: create warehouse + role + Catalog Client → `/oauth/token` → `/catalog/v1/*` (embedded lakekeeper) → egret Authorizer → read/write an Iceberg table on MinIO; then point Trino at egret using a Catalog Client.

**Architecture:** All in one egret process + MinIO. Trino's Iceberg REST connector points `uri` + `oauth2.server-uri` at egret. An opt-in Playwright e2e drives the loop.

**Tech Stack:** docker-compose (postgres, minio, trino), egret (embedded lakekeeper + mini-IdP), Playwright.

---

## Background the implementer needs

- After EB-1…EB-5: egret serves `/catalog/v1/*` (embedded lakekeeper), `/oauth/token` + `/.well-known/*` (mini-IdP), and the control-plane warehouse/role/grant/client mgmt + UI. No Polaris, no proxy.
- egret runs on the host (`localhost:8080`); Trino (container) reaches it via `host.docker.internal:8080`.
- Trino JWT-auth-to-egret is unrelated to Trino's OWN user auth (Keycloak JWT over TLS) — the `oauth2.*` here is Trino→catalog (egret), a service credential.

---

## Task 1: Wire Trino at egret via a Catalog Client

**Files:** `deploy/trino/catalog/iceberg.properties` (gitignored — document in `deploy/embedded-catalog/engine-wiring.md`)

- [ ] **Step 1:** Create a warehouse `lakehouse` + a role `engine` (grant table read+write) + a Catalog Client (`consumer_type=engine`) → clientId/secret. Write:
```properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://host.docker.internal:8080/catalog       # egret embedded catalog
iceberg.rest-catalog.warehouse=lakehouse
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<client_id>:<client_secret>       # a Catalog Client
iceberg.rest-catalog.oauth2.server-uri=http://host.docker.internal:8080/oauth/token   # egret mini-IdP
iceberg.rest-catalog.oauth2.scope=catalog
fs.native-s3.enabled=true
s3.endpoint=http://minio:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=egret
s3.aws-secret-key=egretsecret
```
Restart Trino; confirm the `iceberg` catalog loads (logs). Document in `deploy/embedded-catalog/engine-wiring.md` (both `uri` + `oauth2.server-uri` are egret; the credential is a Catalog Client).

- [ ] **Step 2: Commit** — `docs(dev): Trino via embedded egret catalog + Catalog Client`.

---

## Task 2: End-to-end (opt-in)

**Files:** Create `frontend/tests/e2e/embedded-catalog.spec.ts`; modify `frontend/tests/e2e/README.md`

- [ ] **Step 1: e2e loop** (skipped unless the live stack is up):
1. Platform admin: create warehouse `lakehouse` → role `engine` + a table write grant → a Catalog Client (`engine`).
2. Machine path (direct): `POST http://localhost:8080/oauth/token` (client creds) → JWT; `GET /catalog/v1/config` with Bearer → 200; create a namespace + table through `/catalog/v1/*` → commits to MinIO → read back. A client whose role lacks the grant → 403 (Authorizer).
   (Or via Trino: `CREATE TABLE lakehouse.e2e.t AS SELECT 42 n` then `SELECT n` → 42.)
- [ ] **Step 2: README** — document the setup (embedded catalog, a Catalog Client, Trino pointed at egret).
- [ ] **Step 3: Commit** — `test(e2e): embedded-catalog loop (client -> IdP -> lakekeeper -> MinIO)`.

---

## Task 3: Full CI + epic wrap-up

- [ ] **Step 1: Full local CI** (GitHub CI out of quota): `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `DATABASE_URL=… cargo test --workspace`; frontend `typecheck`+`lint`+`test`+`build`; kernel `pytest`.
- [ ] **Step 2:** Live loop passes (read/write on MinIO through egret; deny path returns 403).
- [ ] **Step 3: Commit** any fixups.

---

## Self-review notes (spec coverage for EB-6)

- Spec **Component 6**: engine wired through egret with a Catalog Client (Task 1), full e2e create→token→catalog→authZ→MinIO (Task 2), full CI (Task 3).
- **Epic complete after EB-6:** EB-1 (embed lakekeeper) → EB-2 (mini-IdP) → EB-3 (store + Authorizer + authN) → EB-4 (mgmt + revert Polaris) → EB-5 (UI) → EB-6 (e2e). egret is a self-contained all-Rust catalog: owns authN (mini-IdP) + authZ (Authorizer), embeds lakekeeper, no JVM, no proxy, no enterprise-IdP for machines, no deprecated endpoint.
- **Future:** notebook Daft-direct un-pause; auto-push credential into engine configs; K8s provisioning.
