# Egret end-to-end tests (Playwright)

These drive a **real browser against a running local stack**. They are separate
from the Vitest unit suite (`npm test`) and are **not** run in standard CI —
they need a live backend and a real Jupyter kernel, so they are a manual /
opt-in smoke of the notebook execution money-path (Notebook epic E).

## Prerequisites

1. **Docker services** up:
   ```bash
   docker compose -f ../../deploy/docker-compose.dev.yml up -d
   ```
2. **A Python env with the kernel deps on PATH** (so notebook cells can spawn a
   real kernel — the backend's `SubprocessLauncher` runs `jupyter kernelgateway`):
   ```bash
   python3.12 -m venv /tmp/kernelenv
   /tmp/kernelenv/bin/pip install jupyter_kernel_gateway ipykernel   # + daft for %%egret_sql --target daft
   ```
3. **Backend** running on `:8080` with that venv on PATH + catalog enabled:
   ```bash
   cd ../..   # egret repo root
   PATH="/tmp/kernelenv/bin:$PATH" \
   EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" \
   EGRET_CATALOG__ENABLED=true EGRET_CATALOG__AUTH_TOKEN=dev \
   cargo run -p egret-bin -- serve
   ```
4. **Frontend dev server** on `:5173` (the Vite proxy forwards `/api` incl. the
   kernel WebSocket — `ws: true` in `vite.config.ts`):
   ```bash
   npm run dev
   ```
5. **A provisioned Iceberg warehouse** for the target workspace. The notebook
   kernel resolves the workspace's warehouse on connect; without one, Run shows
   `KernelError: no warehouse configured for this workspace`. Provision it once
   via the UI: **Storage → Add connection** (S3/MinIO: endpoint
   `http://localhost:9000`, bucket `warehouse`, access key `egret`, secret
   `egretsecret`) → **Catalog → Add warehouse** on that connection.

## SSO login (Keycloak)

The specs authenticate through the **real Keycloak** in
`docker-compose.dev.yml` (`:8081`, realm `egret`, user `tester@egret.local` /
`pw` in group `data-team`) — the `loginViaOidc` helper clicks **Sign in with
SSO**, fills the Keycloak-hosted login form, and follows the callback back into
a workspace. A break-glass `loginLocalAdmin` (local admin form) is also in
`helpers.ts`.

For this to work:

1. **Run the backend with the OIDC env** from `.env.example` — in addition to
   the env already shown in Prerequisite step 3, the backend also needs the OIDC
   vars (`EGRET_OIDC__ENABLED=true`,
   `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback`, plus
   the issuer / client id / secret vars) so `/api/auth/oidc/login` redirects to
   Keycloak and the callback resolves the user.
2. **Bind the `data-team` group to a workspace that has a provisioned
   warehouse** (the "admin grants access" step) so `tester` lands in a usable
   workspace. Do this once:
   ```bash
   cargo run -p egret-bin -- workspace add-group --workspace <your-workspace-slug> --group data-team --role ws_editor
   ```

SSO creds default to `tester@egret.local` / `pw` and can be overridden with
`EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`.

### Seeded SSO test users

The realm ships two extra fixed-identity users for manual/role testing:

| User | Password | Group | Privilege |
| --- | --- | --- | --- |
| `admin-sso@egret.local` | `Admin12345!` | `platform-admins` | platform admin |
| `member-sso@egret.local` | `Member12345!` | `data-team` | member |

OIDC users JIT-provision as plain members. `admin-sso` becomes a platform admin
because the `platform-admins` **group** is granted the `platform_admin` role —
apply that one-time RBAC seed against the egret DB:

```bash
docker exec -i egret-dev-postgres-1 psql -U egret -d egret < deploy/seed-sso-test-rbac.sql
```

A fresh stack (`docker compose ... up`, first import) creates both realm users
automatically; the seed SQL above wires the group→role grant on the egret side.
`member-sso` reaches a workspace via the same `data-team` binding as `tester`.

## Run

```bash
npm run test:e2e
```

Override the target / credentials via env: `EGRET_E2E_BASE_URL`, the SSO creds
`EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`, and the break-glass local
admin `EGRET_E2E_EMAIL` / `EGRET_E2E_PASSWORD`.

## What's covered

- `notebook.spec.ts`:
  - **run a Python cell end-to-end** — login → create notebook → set cell source
    → Run → assert the printed output renders live → reload → assert the output
    **restores** from the persisted `last_output` (E-P2 recorder + E-P3 restore).
  - **toolbar controls** — Run all / Interrupt / Restart / Clear outputs + the
    per-cell Run + Run options are present (E-P4).

## Dataframe-first spec (`notebook-dataframe.spec.ts`)

Opt-in, exactly like `notebook.spec.ts` (a live-stack money-path smoke, **not**
in CI). It exercises the dataframe-first helpers auto-loaded in the kernel:
`write_iceberg(df, table, mode=…)` writes a Daft DataFrame to the notebook's
read-write Iceberg catalog (auto-creating the table), `iceberg(table)` reads it
back, and `trino(sql)` / `starrocks(sql)` read the governed engines. The one
test does an Iceberg **write→read roundtrip** and asserts the round-tripped
marker renders.

In addition to the prerequisites above, the target workspace needs:

1. **A warehouse named `notebook`** — the notebook read-write Iceberg catalog
   (catalog B), backed by a provisioned **Storage** connection, so
   `write_iceberg` / `iceberg` have a read-write catalog to land in. Provision
   it via **Storage → Add connection** (MinIO defaults as above) → **Catalog →
   Add warehouse** with the warehouse name `notebook`.
2. *(Optional, for a governed-engine read via `trino(…)` / `starrocks(…)`)* at
   least one **registered + enabled engine** for the workspace.

Without (1), Run reports no read-write catalog and the roundtrip fails.

## Embedded-catalog e2e (`embedded-catalog.spec.ts`)

Opt-in machine-path loop for the embedded catalog: provision a warehouse + role
+ grant + Catalog Client → `/oauth/token` (mini-IdP) → `/catalog/v1` (embedded
lakekeeper) → egret guard. Also asserts a role with no grants is **403** on the
data plane.

```bash
EGRET_E2E_CATALOG=1 EGRET_E2E_BUCKET=e2e-auto npx playwright test embedded-catalog
```

Requirements:
- The live stack up (`serve` + Vite; Vite proxies `/api`, `/catalog`, `/oauth`).
- A **reserved, initially-empty** MinIO bucket (`EGRET_E2E_BUCKET`, default `e2e`)
  with **no prefix-less warehouse** on it — each run uses a unique `key_prefix`,
  so repeated runs on that bucket don't overlap. Create it once:
  `docker exec egret-dev-minio-1 mc mb --ignore-existing loc/e2e-auto`.
- S3 overrides (defaults match dev MinIO): `EGRET_E2E_S3_ENDPOINT`,
  `EGRET_E2E_S3_KEY`, `EGRET_E2E_S3_SECRET`.

Skipped unless `EGRET_E2E_CATALOG=1`. See `deploy/embedded-catalog/engine-wiring.md`
to wire a real engine (Trino / pyiceberg).

## Data-plane e2e on kind (`dataplane-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), and unlike the other specs it targets the
**in-cluster kind deploy** (`http://egret.localtest.me`), not the host dev stack.
It provisions the full lifecycle via the egret API — storage → warehouse → Trino
engine (1Gi, 1 worker) → enable egret-JWT → assign workspace — confirms the engine
lists + auto-selects in the **SQL Lab** Database picker (the real UI path), then
runs `SELECT current_user` through the SQL Lab query backend (`POST
/api/ws/{ws}/queries`, the exact endpoint the Run button calls) and asserts it
returns the **caller** (`admin`), proving per-user impersonation (OIDC → engine
JWKS → JWT → TLS:8443 → impersonation → query). The query is driven via the API
rather than the Monaco editor + results grid because that DOM chrome is flaky to
script; the money-path exercised is identical. It deletes the engine in a
`try/finally` (not `afterAll`, which runs after `page.request` is torn down) to
free node memory. A freshly JWT-re-provisioned coordinator 500s for ~30–40s until
Trino is `SERVER STARTED`, so the query step retries through that warmup.

### One-time prereq

```bash
just k8s-up            # kind `egret` cluster
just ingress-install   # nginx-ingress (kind flavor)
just coredns-rewrite   # egret.localtest.me resolves cluster-wide
```

**Reserved MinIO bucket.** lakekeeper rejects a new warehouse whose storage profile
overlaps an existing one, and a **prefix-less** warehouse claims a whole bucket — so
point the e2e at a bucket that has no root-level warehouse, via `EGRET_E2E_BUCKET`
(default `warehouse`, the chart-seeded bucket). If that bucket already holds a
prefix-less warehouse (e.g. a hand-provisioned `demo-catalog`), use a dedicated
bucket and create it once in the in-cluster MinIO, e.g.:

```bash
kubectl port-forward -n egret-system svc/minio 9000:9000 &
AWS_ACCESS_KEY_ID=egret AWS_SECRET_ACCESS_KEY=egretsecret \
  aws --endpoint-url http://localhost:9000 s3 mb s3://e2e
# then: EGRET_E2E_BUCKET=e2e just e2e-kind   (each run uses a unique key_prefix)
```

> Note: deleting a warehouse via the CP API removes egret's row but does **not**
> deregister it from the embedded lakekeeper, so its storage profile keeps claiming
> the bucket — prefer a dedicated bucket over reusing one that held a warehouse.

### Run

```bash
just e2e-kind          # deploy-kind-oidc → wait-ready → playwright dataplane-kind
```

`just e2e-kind` re-deploys (idempotent `helm upgrade --install`), waits for pods +
OIDC discovery, then runs the spec. Re-runnable on the same cluster — each run uses
a unique `EGRET_E2E_RUN_ID` suffix and cleans up its engine.

Env overrides: `EGRET_E2E_BASE_URL`, `EGRET_E2E_RUN_ID`, `EGRET_E2E_BUCKET`,
`EGRET_E2E_S3_ENDPOINT` / `EGRET_E2E_S3_KEY` / `EGRET_E2E_S3_SECRET`, and the
break-glass admin `EGRET_E2E_EMAIL` / `EGRET_E2E_PASSWORD`.

> If the single kind node is memory-wedged, delete stale engines first:
> `kubectl get deploy -n egret-engines` then `kubectl delete deploy <name>` (or
> delete the engine via the CP UI).
