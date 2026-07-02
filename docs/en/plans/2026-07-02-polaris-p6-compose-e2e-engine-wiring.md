# Control-Plane Polaris — Plan 6: dev Polaris + e2e + engine wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P6 (final) of the epic** — depends on P1–P5. VI reference: `docs/vi/plans/2026-07-02-polaris-p6-compose-e2e-engine-wiring.md`.

**Goal:** Run **Apache Polaris** in the dev stack (docker-compose, replacing the removed embedded lakekeeper), wire the Keycloak realm for Polaris MIXED auth, and prove the full loop end-to-end: register connection → create a catalog on MinIO → assign to a workspace → an engine (Trino) reads/writes it → the notebook `write_table` roundtrip through the engine.

**Architecture:** Add a `polaris` compose service (Quarkus image, MIXED auth against the egret Keycloak realm, in-memory or Postgres persistence, MinIO storage, default realm). Add Keycloak clients/roles so egret-admin (Keycloak client-credentials) is a Polaris admin. Configure the Trino Iceberg REST connector against a Polaris catalog using an assignment's service credential. An opt-in Playwright e2e drives the whole loop.

**Tech Stack:** docker-compose, Apache Polaris (Quarkus/JVM), Keycloak, Trino, MinIO, Playwright.

---

## Background the implementer needs

- **Dev compose:** `deploy/docker-compose.dev.yml` — services `postgres` (16), `minio`, `trino` (`8090:8080`, `8453:8443`, JWT auth over TLS, catalog dir `./trino/catalog` mounted, file-ACL `rules.json`), `starrocks`, `keycloak` (`8081`, realm `egret`). P1 removed the embedded lakekeeper (was in-process, not a compose service).
- **Polaris config knobs** (from `polaris/runtime/defaults/.../application.properties`): `polaris.persistence.type` = `in-memory` (dev-simplest) | `relational-jdbc` (Postgres via `quarkus.datasource.*`); `polaris.realm-context.{type=default,realms=POLARIS,header-name=Polaris-Realm,require-header=false}`; **auth mode** enum `INTERNAL|EXTERNAL|MIXED`; external OIDC via `quarkus.oidc.auth-server-url` + roles mapping. Polaris config template: `polaris/regtests/docker-compose.yml` + `getting-started`. Image: the published `apache/polaris` (Quarkus) — confirm the exact tag.
- **Keycloak realm:** `deploy/keycloak/egret-realm.json` — client `egret` with audience mappers `aud-egret-catalog`/`aud-trino`/`aud-starrocks`. Needs a **Polaris audience** + an **egret-admin service-account** client (client-credentials) whose token maps to a Polaris admin principal-role.
- **From P3/P4:** `egret-polaris` gets its admin token via **Keycloak client-credentials**; assignment provisions a **Polaris-native principal** (`clientId`/`clientSecret`) stored as a global Secret — the **engine's** catalog credential (used via Polaris `/v1/oauth/tokens`).
- **Trino Iceberg REST** connector (validated in the earlier spike against lakekeeper — same shape for Polaris): `connector.name=iceberg`, `iceberg.catalog.type=rest`, `iceberg.rest-catalog.uri=<polaris>/api/catalog`, `iceberg.rest-catalog.warehouse=<catalog>`, `security=OAUTH2` + `oauth2.token`/credential, `fs.native-s3.enabled=true` + MinIO S3 creds. Trino reaches other containers by service name; the host via `host.docker.internal`.

---

## Task 1: Add the Polaris service to dev docker-compose

**Files:** Modify `deploy/docker-compose.dev.yml`; Create `deploy/polaris/` config if needed

- [ ] **Step 1: Add the service**

```yaml
  polaris:
    image: apache/polaris:latest   # confirm tag; or build from polaris/ per its getting-started
    depends_on: [postgres, keycloak, minio]
    ports:
      - "8181:8181"    # Polaris API
    environment:
      # Persistence: in-memory for dev (simplest). Switch to relational-jdbc → egret Postgres later.
      polaris.persistence.type: in-memory
      polaris.realm-context.type: default
      polaris.realm-context.realms: POLARIS
      polaris.realm-context.require-header: "false"
      # Auth: MIXED — internal token endpoint (engines) + external OIDC (egret-admin via Keycloak).
      polaris.authentication.type: mixed
      quarkus.oidc.auth-server-url: http://keycloak:8080/realms/egret
      quarkus.oidc.client-id: polaris
      # roles mapping so a Keycloak claim → Polaris principal-role (confirm exact keys vs Polaris OIDC docs)
      # Storage creds for MinIO (S3-compat)
      AWS_ACCESS_KEY_ID: egret
      AWS_SECRET_ACCESS_KEY: egretsecret
      AWS_REGION: us-east-1
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:8181/q/health || exit 1"]
      interval: 10s
      timeout: 5s
      retries: 12
```
Confirm the exact Polaris image tag + the precise Quarkus OIDC role-mapping + S3-endpoint keys against `polaris/regtests/docker-compose.yml` and `polaris/runtime/defaults/.../application.properties` (this container config is the least-pinned piece — treat Step 1 as a short config-spike against Polaris's own examples).

- [ ] **Step 2: Bring it up + health**

Run: `docker compose -f deploy/docker-compose.dev.yml up -d polaris` then wait for healthy; `curl -sf http://localhost:8181/q/health`.
Expected: Polaris healthy.

- [ ] **Step 3: Commit**

```bash
git add deploy/docker-compose.dev.yml deploy/polaris/
git commit -m "feat(dev): add Apache Polaris to the dev docker-compose (MIXED auth, MinIO storage)"
```

---

## Task 2: Keycloak realm — Polaris audience + egret-admin service account

**Files:** Modify `deploy/keycloak/egret-realm.json`

- [ ] **Step 1: Add a Polaris audience mapper + admin client**

Add to the realm: (a) an `aud-polaris` audience mapper (audience `polaris`) on the token, (b) a confidential client `polaris-admin` with **service accounts enabled** (client-credentials) for egret-admin, and (c) a realm role (e.g. `polaris_admin`) assigned to the `polaris-admin` service account, mapped by Polaris OIDC → a Polaris principal-role holding `SERVICE_MANAGE_ACCESS`. (Mirror the existing `egret` client + audience-mapper JSON structure.)

- [ ] **Step 2: Re-import + verify a token**

Restart Keycloak (or re-import the realm), then:
```bash
curl -s -X POST http://localhost:8081/realms/egret/protocol/openid-connect/token \
  -d grant_type=client_credentials -d client_id=polaris-admin -d client_secret=<sec> \
  | python3 -c "import sys,json;d=json.load(sys.stdin);import base64;p=d['access_token'].split('.')[1];p+='='*(-len(p)%4);print(json.loads(base64.urlsafe_b64decode(p)).get('aud'))"
```
Expected: `aud` includes `polaris`.

- [ ] **Step 3: Commit**

```bash
git add deploy/keycloak/egret-realm.json
git commit -m "feat(dev): Keycloak polaris audience + egret-admin service account"
```

---

## Task 3: Bootstrap Polaris + register the connection in egret

**Files:** Docs / a dev seed script (`deploy/polaris/bootstrap.md` or a `justfile`/make target)

- [ ] **Step 1: Bootstrap the Polaris realm/root**

Per Polaris's admin tool (`polaris-admin bootstrap`) or the `POST /management/v1/bootstrap` flow — seed the `POLARIS` realm + a root principal. Document the command.

- [ ] **Step 2: Register the connection via egret (P4 route)**

As a platform admin, `PUT /api/control-plane/polaris/connection` with `{baseUrl:"http://polaris:8181", kcTokenUrl:"http://keycloak:8080/realms/egret/protocol/openid-connect/token", kcClientId:"polaris-admin", kcClientSecret:"<sec>", kcScope:null}` (from egret running in dev; if egret runs on the host, use `http://localhost:8181` + `http://localhost:8081/...`). Then `GET .../connection/health` → ok.

- [ ] **Step 3: Commit the docs/seed**

```bash
git add deploy/polaris/
git commit -m "docs(dev): Polaris bootstrap + connection registration steps"
```

---

## Task 4: Engine connector wiring (Trino → assigned Polaris catalog)

**Files:** `deploy/trino/catalog/` (dev connector), `deploy/polaris/engine-wiring.md`

- [ ] **Step 1: Configure a Trino iceberg catalog against Polaris**

Given an assignment's service credential (the Polaris principal `clientId`/`clientSecret` egret stored), write `deploy/trino/catalog/iceberg.properties`:

```properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://polaris:8181/api/catalog
iceberg.rest-catalog.warehouse=lakehouse
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<clientId>:<clientSecret>
iceberg.rest-catalog.oauth2.server-uri=http://polaris:8181/api/catalog/v1/oauth/tokens
fs.native-s3.enabled=true
s3.endpoint=http://minio:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=egret
s3.aws-secret-key=egretsecret
```
(Trino exchanges the credential at Polaris's token endpoint. Restart Trino.) Document that in prod this connector config is produced from the egret assignment (the credential the workspace's engine uses) — the automated push into engine configs is the future engines-centralization / K8s-provisioning epic; here it is documented + manually applied for dev.

- [ ] **Step 2: Verify Trino sees the catalog**

Mint a `tester` Trino JWT (Keycloak, `aud=trino`), then `trino("SHOW SCHEMAS FROM iceberg")` via the kernel venv (as in the earlier spike). Expected: connects to Polaris; lists schemas.

- [ ] **Step 3: Commit**

```bash
git add deploy/trino/catalog/iceberg.properties deploy/polaris/engine-wiring.md
git commit -m "feat(dev): Trino Iceberg REST connector against a Polaris catalog"
```

---

## Task 5: End-to-end test (opt-in, live)

**Files:** Create `frontend/tests/e2e/polaris-catalog.spec.ts`; modify `frontend/tests/e2e/README.md`

- [ ] **Step 1: Write the opt-in e2e**

Drives the full loop (skipped unless the live stack + Polaris are up):
1. Login as a platform admin → `/control-plane/catalog` → register Polaris connection (if not) → create catalog `lakehouse` on the `minio` storage → assign it to `eng`.
2. Login as `tester` (SSO) → a notebook cell: `write_table(df, "lakehouse.e2e.roundtrip", engine="trino", mode="overwrite")` then `trino("SELECT who,n FROM lakehouse.e2e.roundtrip")` → assert `${marker} 42` (output-only).

- [ ] **Step 2: Update the e2e README**

Document the required setup: `docker compose up polaris` + Keycloak `polaris-admin` client + a Trino iceberg catalog wired to Polaris (Task 4) + a Trino JWT→group provider (governed writes) or relaxed ACL for the test.

- [ ] **Step 3: Typecheck**

Run: `cd frontend && npx tsc --noEmit -p tests/e2e/tsconfig.json` → clean. (A full live run requires Task 1–4 up.)

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/polaris-catalog.spec.ts frontend/tests/e2e/README.md
git commit -m "test(e2e): control-plane Polaris catalog → assign → engine roundtrip (opt-in)"
```

---

## Task 6: Verification + epic wrap-up

- [ ] **Step 1: Stack up + live loop**

Bring up the full stack (postgres, minio, keycloak, trino, polaris) + egret `serve`; run the e2e (or the manual loop from Task 5). Expected: the roundtrip passes (`${marker} 42`), proving control-plane Polaris + assignment + engine read/write end-to-end.

- [ ] **Step 2: Full CI locally (per the project rule — GitHub CI is out of quota)**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && DATABASE_URL=… cargo test --workspace` and `cd frontend && npm run typecheck && npm run lint && npm run test && npm run build` and `cd kernel-image && python -m pytest tests/ -q`.
Expected: all green.

- [ ] **Step 3: Commit any fixups**

```bash
git add -A && git commit -m "chore(catalog): P6 verification fixups"
```

---

## Self-review notes (spec coverage for P6)

- Spec **Ops prerequisites + dev e2e**: Polaris in compose (Task 1), Keycloak MIXED wiring (Task 2), bootstrap + connection registration (Task 3), Trino→Polaris connector using the assignment credential (Task 4), full e2e (Task 5–6).
- **Least-pinned pieces (flagged):** the exact Polaris container config (image tag + Quarkus OIDC role-mapping) — a short config-spike against Polaris's own `regtests/docker-compose.yml` + `application.properties`; and the *automated* push of assignment credentials into engine connector config — documented + manual for dev, automated later in the engines-centralization / K8s-provisioning future epic.
- **Epic complete after P6:** P1 (remove) → P2 (storage/secrets → control plane) → P3 (`egret-polaris`) → P4 (routes+RBAC+assignment) → P5 (UI) → P6 (dev Polaris + e2e + engine wiring). Future: K8s-provisioning epic (egret provisions engines + Polaris on K8s via operators/Helm).
