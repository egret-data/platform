# Apache Polaris — dev runbook (control-plane catalog)

egret drives an **external** Apache Polaris as its Iceberg REST catalog, from the
control plane (platform-admin only). This is the dev/e2e runbook.

## 1. Bring up Polaris

Polaris is a service in `deploy/docker-compose.dev.yml`:

```bash
docker compose -f deploy/docker-compose.dev.yml up -d polaris
curl -sf http://localhost:8182/q/health   # {"status":"UP"}
```

- Image `apache/polaris:latest`; API + management on `:8181`, health on `:8182`.
- **Auth (dev): INTERNAL.** `POLARIS_BOOTSTRAP_CREDENTIALS: POLARIS,root,s3cr3t`
  bootstraps realm `POLARIS` with a root principal (clientId `root`, secret `s3cr3t`).
- Storage: RustFS (`AWS_ENDPOINT_URL_S3=http://rustfs:9000`, `ALLOW_INSECURE_STORAGE_TYPES`).
- Persistence: in-memory — **catalogs/principals reset when the Polaris container restarts.**

> **Prod target = MIXED / Keycloak OIDC** (see the control-plane-polaris spec). There egret-admin
> and engines would authenticate via Keycloak service accounts mapped to Polaris principals.
> Dev takes a shortcut: since `egret-polaris`'s token source is a generic OAuth2
> `client_credentials` client, it points straight at **Polaris's own token endpoint** — no
> Keycloak OIDC mapping needed to exercise the full flow. Wiring Keycloak (an `aud-polaris`
> mapper + a `polaris-admin` service-account client mapped to a Polaris admin principal-role)
> is deferred to the MIXED prod setup.

Get an admin token (the same `client_credentials` flow `egret-polaris` uses):

```bash
curl -s -X POST http://localhost:8181/api/catalog/v1/oauth/tokens \
  -d grant_type=client_credentials -d client_id=root -d client_secret=s3cr3t \
  -d scope=PRINCIPAL_ROLE:ALL   # -> {"access_token": "...", ...}
```

## 2. Register the connection in egret

As a platform admin (egret `serve` + frontend up), `Control Plane → Catalog → Connect Polaris`,
or directly:

```bash
curl -X PUT http://localhost:8080/api/control-plane/polaris/connection \
  -H 'Content-Type: application/json' --cookie "<admin session>" -d '{
    "baseUrl":       "http://localhost:8181",
    "kcTokenUrl":    "http://localhost:8181/api/catalog/v1/oauth/tokens",
    "kcClientId":    "root",
    "kcClientSecret":"s3cr3t",
    "kcScope":       "PRINCIPAL_ROLE:ALL"
  }'
# then: GET /api/control-plane/polaris/connection/health -> {"ok":true}
```

(If egret `serve` runs inside the compose network instead of on the host, use `http://polaris:8181`.)

## 3. Create a catalog + assign it to a workspace

`Control Plane → Catalog`: create a catalog (pick a global **storage connection** —
maps to `s3://{bucket}/{name}/` allowed location) → `POST /polaris/catalogs`.
Then per-catalog **Assign…** to a workspace → `POST /control-plane/workspaces/{ws}/catalog-assignments`,
which provisions a Polaris principal + principal-role + catalog-role + `CATALOG_MANAGE_CONTENT`
grant and stores the principal's `clientId`/`clientSecret` as a global secret (the engine credential).

## 4. Wire Trino to the catalog

`deploy/trino/catalog/iceberg.properties` points Trino's Iceberg REST connector at Polaris
(`iceberg.rest-catalog.uri=http://polaris:8181/api/catalog`, `warehouse=lakehouse`, OAUTH2 via
Polaris's token endpoint, `fs.native-s3` → RustFS). Restart Trino to load it. In real use the
`oauth2.credential` is an **assignment's** Polaris principal (clientId:clientSecret) scoped to
that workspace's catalog-role; auto-pushing it into the connector is the future
engines-centralization / K8s-provisioning epic — here it is manual (dev uses `root:s3cr3t`).

Trino enforces JWT auth over TLS (`:8453`); a `SHOW SCHEMAS FROM iceberg` query needs a
per-user Keycloak JWT (egret mints one via its own machine-IdP token broker). Governed **writes**
also need a Trino JWT→group mapping (dev's `rules.json`); without a group provider, relax the
ACL for the test (see `deploy/trino/etc/README.md`).

## 5. End-to-end (the full loop)

With the stack up (postgres, rustfs, keycloak, **polaris**, trino wired) + egret `serve`: via
SQL Lab, `CREATE TABLE lakehouse.eda.summary AS SELECT ...` then `SELECT * FROM
lakehouse.eda.summary` — the engine performs the governed write into the Polaris catalog on
RustFS and reads it back. See `frontend/tests/e2e/README.md`.

## What is live-validated (this repo)

Against a real `apache/polaris:latest` on MinIO, all `egret-polaris` API calls egret makes were
confirmed (HTTP 201/200): `client_credentials` token; `list_catalogs`/`list_principals` (response
shapes match `egret-polaris`); `create_catalog` on S3/MinIO; and the full assignment sequence
(`create_principal` + credentials, principal-role, `assign_principal_role`, catalog-role,
`assign_catalog_role`, `add_grant CATALOG_MANAGE_CONTENT`). Trino loads the Polaris Iceberg REST
connector cleanly. The full SQL-Lab-through-Trino write/read roundtrip is the operator-run
validation above (needs the whole stack + the Trino group-mapping, which is environment-specific).
