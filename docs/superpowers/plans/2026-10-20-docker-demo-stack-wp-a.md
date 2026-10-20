# Docker Demo Stack, Work Package A (compose demo) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `just demo-up` starts a complete Egret on Docker Compose (backend, frontend, Postgres, RustFS with its bucket, Keycloak SSO, Mailpit, renderer, a Trino with sample data), seeded and ready to log in, with no Kubernetes.

**Architecture:** A new `deploy/docker-compose.demo.yml` mirrors what the Helm chart already wires (`deploy/helm/egret/templates/configmap-env.yaml`, `deps-*.yaml`, `init-job.yaml`). One **edge** nginx on a single origin (`http://egret.localtest.me:${DEMO_PORT}`) fronts the frontend and Keycloak (`/auth`), so the browser and the backend see the same OIDC issuer string. The backend container runs `migrate`, `init`, `serve` from an entrypoint script. No product code changes in this package.

**Tech Stack:** Docker Compose v2, nginx (templates via envsubst), Postgres 16, RustFS 1.0.1, Keycloak 26.0, Mailpit, Trino 483, bash + curl + python3 for the seed. `just` recipes.

Spec: `docs/superpowers/specs/2026-10-20-docker-demo-stack-design.md` (WP-A). WP-B (Apps) and WP-C (Docker engine provisioner) are separate plans.

## Global Constraints

- Pin every image by tag; copy the digests the dev compose already uses for `trinodb/trino:483` and `starrocks` where reused. Never `latest` (spec Risks: MinIO precedent). `axllent/mailpit` and the renderer are pinned to explicit tags chosen in Task 5.
- Demo secrets are fixed, documented, public development values; the compose file and the docs carry a banner: "Demo only. Never expose to a network."
- Do not touch `deploy/docker-compose.dev.yml` (contributor stack stays as is).
- Do not edit product source (`crates/`, `frontend/src`); only `deploy/`, `justfile`, `.env.example`, `docs/`, `README.md`.
- Conventional Commits; commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; `Closes #N` only in the PR description.
- The repo rule: the PR updates the docs in `egret-data/docs` (Platform section) or says in the PR body which follow-up PR does; tick the Docs box in the PR template.
- **Testing port.** This machine already runs another service on :8080, so while developing run compose with `DEMO_PORT=18080 DEMO_URL=http://egret.localtest.me:18080` exported in the shell (shell variables override `--env-file`). Never commit those values; `deploy/demo/demo.env` keeps 8080. Mailpit's UI port 8025 and no other host port are published. Do not stop or touch the running containers named `egret-dev-*`, `dgcl-*`, `egretdocs-*`.
- Browser URL for everything is `http://egret.localtest.me:${DEMO_PORT:-8080}` (`*.localtest.me` resolves to 127.0.0.1 through public DNS; offline machines add `127.0.0.1 egret.localtest.me` to `/etc/hosts`).
- Fixed values used throughout (public dev values):
  - `EGRET_SECRETS__MASTER_KEY=q1IaZ2X7N3i/um/U/aJ2lsmVrRE5pTZChAVG9ezDKPw=`
  - `EGRET_SECURITY__SECRET_KEY` = the value already in `.env.example`
  - `EGRET_CATALOG__AUTH_TOKEN=b06ed0d0fa62f7e963e7cb95680ecb5b75bab4cc`
  - local admin `admin@egret.local` / `Admin12345!`; storage `egret` / `egretsecret`; bucket `warehouse`.

## File Structure

| File | Responsibility |
|---|---|
| `deploy/docker-compose.demo.yml` | The stack: services, networks, volumes, healthchecks |
| `deploy/demo/demo.env` | Public dev values shared by compose and the scripts (`DEMO_PORT`, keys, URLs) |
| `deploy/demo/edge.conf.template` | Edge nginx: `/auth/` to Keycloak, everything else to the frontend |
| `deploy/demo/trino/config.properties` | Plain-HTTP Trino, `process-forwarded=true` |
| `deploy/demo/trino/catalog/{tpch,memory}.properties` | Sample catalogs |
| `deploy/demo/egret-entrypoint.sh` | `migrate`, then `init`, then `serve` for the `egret` service |
| `deploy/demo/wait.sh` | Wait for readiness, print URL and credentials |
| `deploy/demo/seed.sh` | Idempotent API seed: Trino database, SMTP to Mailpit, dataset, charts, dashboard |
| `justfile` | `demo-up`, `demo-down`, `demo-reset`, `demo-seed`, `demo-logs` |
| `.env.example` | Add the three storage variables (KI-136) |
| `docs/en/guides/demo-stack.md` | How to run it, ports, credentials, limits, safety banner |
| `README.md` | Link to the guide as the quick start |

---

### Task 1: Compose skeleton with Postgres, RustFS and its bucket

**Files:**
- Create: `deploy/demo/demo.env`
- Create: `deploy/docker-compose.demo.yml`

**Interfaces:**
- Produces: compose project `egret-demo`; services `postgres`, `rustfs`, `rustfs-init`; named volumes `demo-pgdata`, `demo-rustfsdata`; env file `deploy/demo/demo.env` that every later task adds to.

- [ ] **Step 1: Create `deploy/demo/demo.env`**

```bash
# DEMO ONLY. Public development values; never expose this stack to a network.
DEMO_PORT=8080
DEMO_HOST=egret.localtest.me
DEMO_URL=http://egret.localtest.me:8080

POSTGRES_USER=egret
POSTGRES_PASSWORD=egret
POSTGRES_DB=egret

S3_ACCESS_KEY=egret
S3_SECRET_KEY=egretsecret
S3_BUCKET=warehouse
S3_REGION=us-east-1

EGRET_ADMIN_EMAIL=admin@egret.local
EGRET_ADMIN_PASSWORD=Admin12345!
EGRET_SECRETS__MASTER_KEY=q1IaZ2X7N3i/um/U/aJ2lsmVrRE5pTZChAVG9ezDKPw=
EGRET_SECURITY__SECRET_KEY=Vi2XPRkbfyTLbSzhWv0VBYeOozGGoUnVSL3pIx2Oy1kbBphJyL0CXpomCNKdrYndzYnOX8MH0oaxhcY6Bt/x8g==
EGRET_CATALOG__AUTH_TOKEN=b06ed0d0fa62f7e963e7cb95680ecb5b75bab4cc
```

`DEMO_URL` must equal `http://${DEMO_HOST}:${DEMO_PORT}`; the `just` recipes in Task 7 recompute it when `DEMO_PORT` is overridden.

- [ ] **Step 2: Create `deploy/docker-compose.demo.yml` with the three base services**

```yaml
# DEMO ONLY. Public development secrets. Never expose this stack to a network.
# Everything runs on Docker, no Kubernetes. See docs/en/guides/demo-stack.md.
name: egret-demo

x-demo-env: &demo-env
  env_file: [demo/demo.env]

services:
  postgres:
    image: postgres:16
    <<: *demo-env
    volumes:
      - demo-pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U $${POSTGRES_USER} -d $${POSTGRES_DB}"]
      interval: 5s
      timeout: 3s
      retries: 20

  rustfs:
    # STS AssumeRole is required: the embedded catalog vends credentials through it.
    image: rustfs/rustfs:1.0.1
    environment:
      RUSTFS_ACCESS_KEY: egret
      RUSTFS_SECRET_KEY: egretsecret
    volumes:
      - demo-rustfsdata:/data
    healthcheck:
      test: ["CMD-SHELL", "curl -fsS http://127.0.0.1:9000/health || exit 1"]
      interval: 5s
      timeout: 3s
      retries: 30

  # One-shot: creates the bucket the Helm chart's deps-rustfs-bucket-job creates (KI-137).
  rustfs-init:
    image: amazon/aws-cli:2.37.9
    <<: *demo-env
    depends_on:
      rustfs: { condition: service_healthy }
    restart: "no"
    environment:
      AWS_ACCESS_KEY_ID: egret
      AWS_SECRET_ACCESS_KEY: egretsecret
      AWS_DEFAULT_REGION: us-east-1
      AWS_ENDPOINT_URL: http://rustfs:9000
    entrypoint: ["/bin/sh", "-c"]
    command:
      - |
        aws s3api head-bucket --bucket "$${S3_BUCKET}" 2>/dev/null \
          || aws s3api create-bucket --bucket "$${S3_BUCKET}"
        echo "bucket $${S3_BUCKET} ready"

volumes:
  demo-pgdata:
  demo-rustfsdata:
```

- [ ] **Step 3: Check the RustFS image has `curl` for the healthcheck**

Run: `docker run --rm --entrypoint sh rustfs/rustfs:1.0.1 -c 'command -v curl wget'`
Expected: a path for `curl` or `wget`. If only `wget` exists, change the healthcheck to `wget -qO- http://127.0.0.1:9000/health`. If neither exists, drop the container healthcheck and make `rustfs-init` retry (`until aws s3api list-buckets; do sleep 2; done`) and depend on `service_started`.

- [ ] **Step 4: Bring it up and verify the bucket**

Run: `docker compose -f deploy/docker-compose.demo.yml up -d && sleep 20 && docker compose -f deploy/docker-compose.demo.yml logs rustfs-init | tail -3`
Expected: `bucket warehouse ready`; `docker compose -f deploy/docker-compose.demo.yml ps` shows `postgres` and `rustfs` healthy, `rustfs-init` exited 0.
Run it a second time (`up -d` again): `rustfs-init` must still end with `bucket warehouse ready` (idempotent).

- [ ] **Step 5: Commit**

```bash
git add deploy/demo/demo.env deploy/docker-compose.demo.yml
git commit -m "feat(deploy): demo compose skeleton with Postgres, RustFS and bucket init"
```

---

### Task 2: Demo Trino with sample data

**Files:**
- Create: `deploy/demo/trino/config.properties`
- Create: `deploy/demo/trino/catalog/tpch.properties`
- Create: `deploy/demo/trino/catalog/memory.properties`
- Modify: `deploy/docker-compose.demo.yml` (add `trino-demo`)

**Interfaces:**
- Produces: service `trino-demo` reachable inside the network as `trino-demo:8080` over plain HTTP, no authentication; catalogs `tpch`, `memory`. Task 6 registers it as a database.

- [ ] **Step 1: Create the config files**

`deploy/demo/trino/config.properties`:

```properties
# DEMO ONLY: plain HTTP, no authentication. Egret sends X-Forwarded-Proto; without
# process-forwarded Trino answers 406 and the UI shows 502 (KI-001).
coordinator=true
node-scheduler.include-coordinator=true
http-server.http.port=8080
discovery.uri=http://localhost:8080
http-server.process-forwarded=true
query.max-memory=1GB
```

`deploy/demo/trino/catalog/tpch.properties`:

```properties
connector.name=tpch
tpch.splits-per-node=2
```

`deploy/demo/trino/catalog/memory.properties`:

```properties
connector.name=memory
```

- [ ] **Step 2: Add the service**

Insert under `services:` (use the same digest as `deploy/docker-compose.dev.yml`'s `trinodb/trino:483@sha256:...`):

```yaml
  trino-demo:
    image: trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534
    environment:
      JAVA_TOOL_OPTIONS: "-Xmx1g"
    volumes:
      - ./demo/trino/config.properties:/etc/trino/config.properties:ro
      - ./demo/trino/catalog:/etc/trino/catalog:ro
    healthcheck:
      test: ["CMD-SHELL", "curl -fsS http://127.0.0.1:8080/v1/info | grep -q '\"starting\":false'"]
      interval: 10s
      timeout: 5s
      retries: 30
```

- [ ] **Step 3: Verify it answers a query**

Run:

```bash
docker compose -f deploy/docker-compose.demo.yml up -d trino-demo
until docker compose -f deploy/docker-compose.demo.yml ps trino-demo | grep -q healthy; do sleep 5; done
docker compose -f deploy/docker-compose.demo.yml exec trino-demo trino --execute "select count(*) from tpch.tiny.orders"
docker compose -f deploy/docker-compose.demo.yml exec trino-demo curl -s -H 'X-Forwarded-Proto: http' -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/v1/info
```

Expected: `"15000"`, then `200` (not `406`).

- [ ] **Step 4: Commit**

```bash
git add deploy/demo/trino deploy/docker-compose.demo.yml
git commit -m "feat(deploy): demo Trino with tpch and memory catalogs"
```

---

### Task 3: The egret backend service

**Files:**
- Create: `deploy/demo/egret-entrypoint.sh`
- Modify: `deploy/docker-compose.demo.yml` (add `egret`)

**Interfaces:**
- Consumes: Task 1 services and `demo.env`.
- Produces: service `egret` listening on `egret:8080` inside the network (not published), health at `/readyz`, image tag `egret-demo-backend:dev` built from `deploy/docker/backend.Dockerfile`.
- OIDC/edge-dependent keys use `${DEMO_URL}` and are exercised in Task 4.

- [ ] **Step 1: Create the entrypoint**

```bash
#!/bin/sh
# DEMO ONLY. Bring the schema up, create the first admin and workspace if missing, then serve.
# `egret init` is idempotent: without --force it does nothing when a platform_admin exists.
set -eu
egret migrate
egret init
exec egret serve
```

Make it executable: `chmod +x deploy/demo/egret-entrypoint.sh`.

- [ ] **Step 2: Add the `egret` service**

```yaml
  egret:
    image: egret-demo-backend:dev
    build:
      context: ..
      dockerfile: deploy/docker/backend.Dockerfile
    <<: *demo-env
    depends_on:
      postgres: { condition: service_healthy }
      rustfs-init: { condition: service_completed_successfully }
    entrypoint: ["/bin/sh", "/demo/egret-entrypoint.sh"]
    volumes:
      - ./demo/egret-entrypoint.sh:/demo/egret-entrypoint.sh:ro
    environment:
      EGRET_SERVER__HOST: 0.0.0.0
      EGRET_SERVER__PORT: "8080"
      EGRET_DATABASE__URL: postgres://egret:egret@postgres:5432/egret
      EGRET_STORAGE__ENDPOINT: http://rustfs:9000
      EGRET_STORAGE__BUCKET: warehouse
      EGRET_STORAGE__REGION: us-east-1
      EGRET_STORAGE__PATH_STYLE: "true"
      EGRET_STORAGE__ACCESS_KEY: egret
      EGRET_STORAGE__SECRET_KEY: egretsecret
      EGRET_CATALOG__ENABLED: "true"
      EGRET_CATALOG__WAREHOUSE: s3://warehouse
      # Address other containers use to reach egret (engines, renderer): the service name.
      EGRET_CATALOG__PUBLIC_URL: http://egret:8080
      EGRET_RANGER__ENABLED: "true"
      EGRET_RANGER__AUTH_TOKEN: ""
      EGRET_WORKFLOW__WORKER_ENABLED: "true"
      EGRET_WORKFLOW__SCHEDULER_ENABLED: "true"
      EGRET_LOG__FILTER: info,egret=debug
    healthcheck:
      test: ["CMD-SHELL", "bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080 && printf \"GET /readyz HTTP/1.0\\r\\n\\r\\n\" >&3 && head -1 <&3 | grep -q 200'"]
      interval: 10s
      timeout: 5s
      retries: 30
      start_period: 20s
```

The runtime image (`debian:bookworm-slim`) has `bash` but no `curl`; the healthcheck above uses bash's `/dev/tcp`. If `bash` is missing in that image, add `curl` to the runtime stage of `deploy/docker/backend.Dockerfile` instead and use `curl -fsS http://127.0.0.1:8080/readyz`.

OIDC, IdP, renderer and public-URL keys are added in Tasks 4 and 5, because they depend on the edge and on Mailpit/renderer.

- [ ] **Step 3: Build and start (long: the first build compiles the workspace, 20 to 40 minutes)**

Run: `docker compose -f deploy/docker-compose.demo.yml up -d --build egret`
Expected: `egret` reaches `healthy`. If `egret migrate` fails with `the apps API requires [storage] ...`, a storage variable above is missing or misspelled.

- [ ] **Step 4: Verify login from inside the network**

Run:

```bash
docker compose -f deploy/docker-compose.demo.yml exec -T egret bash -c \
  'exec 3<>/dev/tcp/127.0.0.1/8080; printf "POST /api/auth/login HTTP/1.0\r\ncontent-type: application/json\r\ncontent-length: 54\r\n\r\n{\"email\":\"admin@egret.local\",\"password\":\"Admin12345!\"}" >&3; head -1 <&3'
```

Expected: `HTTP/1.0 200 OK`. (`content-length` must equal the body's byte length, 54 for that body; recompute if the body changes.)
Also: `docker compose -f deploy/docker-compose.demo.yml logs egret | grep -i "password"` must not print the admin password a second time on restart (`init` no-ops when an admin exists). Restart `egret` and confirm.

- [ ] **Step 5: Commit**

```bash
git add deploy/demo/egret-entrypoint.sh deploy/docker-compose.demo.yml
git commit -m "feat(deploy): demo egret backend service with migrate and init entrypoint"
```

---

### Task 4: Keycloak, frontend and the single-origin edge

**Files:**
- Create: `deploy/demo/edge.conf.template`
- Modify: `deploy/docker-compose.demo.yml` (add `keycloak`, `frontend`, `edge`; add OIDC/IdP env to `egret`)

**Interfaces:**
- Consumes: `egret` (`egret:8080`).
- Produces: `edge` published on `${DEMO_PORT}`, with the network alias `${DEMO_HOST}` so containers resolve `http://egret.localtest.me:${DEMO_PORT}` to the edge; Keycloak at `/auth`; OIDC issuer `http://egret.localtest.me:${DEMO_PORT}/auth/realms/egret`.
- Why the alias: the issuer string must be identical in the browser and in the backend's discovery fetch, and `localhost` means a different host inside each container.

- [ ] **Step 1: Create the edge template**

`deploy/demo/edge.conf.template` (only `$DEMO_*` variables are substituted; nginx's own `$host` etc. stay):

```nginx
# DEMO ONLY. One origin for the app and Keycloak. Listens on the same port inside and outside
# the container so the OIDC issuer URL is the same string for the browser and for egret.
server {
    listen ${DEMO_PORT};
    server_name _;
    client_max_body_size 64m;

    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $connection_upgrade;
    proxy_set_header Host $host:$server_port;
    proxy_set_header X-Forwarded-Host $host;
    proxy_set_header X-Forwarded-Port $server_port;
    proxy_set_header X-Forwarded-Proto http;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_buffering off;
    proxy_read_timeout 3600s;

    location /auth/ {
        proxy_pass http://keycloak:8080;
    }

    location / {
        proxy_pass http://frontend:80;
    }
}

map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

(Place the `map` at the top of the file if nginx rejects it after `server`; `map` is valid only at `http` level, which template files in `conf.d` are included into.)

- [ ] **Step 2: Add the services**

```yaml
  keycloak:
    image: quay.io/keycloak/keycloak:26.0
    command: ["start-dev", "--import-realm"]
    environment:
      KC_BOOTSTRAP_ADMIN_USERNAME: admin
      KC_BOOTSTRAP_ADMIN_PASSWORD: admin
      KC_HTTP_RELATIVE_PATH: /auth
      KC_HOSTNAME: ${DEMO_URL:-http://egret.localtest.me:8080}/auth
      KC_HOSTNAME_STRICT: "false"
      KC_HTTP_ENABLED: "true"
      KC_PROXY_HEADERS: xforwarded
    volumes:
      - ./keycloak:/opt/keycloak/data/import:ro
    healthcheck:
      test: ["CMD-SHELL", "exec 3<>/dev/tcp/127.0.0.1/8080 && printf 'GET /auth/realms/egret HTTP/1.0\\r\\n\\r\\n' >&3 && head -1 <&3 | grep -q 200"]
      interval: 10s
      timeout: 5s
      retries: 40
      start_period: 30s

  frontend:
    image: egret-demo-frontend:dev
    build:
      context: ../frontend
    depends_on:
      egret: { condition: service_healthy }

  edge:
    image: nginx:1.27-alpine
    depends_on:
      frontend: { condition: service_started }
      keycloak: { condition: service_healthy }
    ports:
      - "${DEMO_PORT:-8080}:${DEMO_PORT:-8080}"
    environment:
      NGINX_ENVSUBST_FILTER: ^DEMO_
      # Interpolated, not env_file: a shell override such as DEMO_PORT=18080 must reach the container too.
      DEMO_PORT: ${DEMO_PORT:-8080}
    volumes:
      - ./demo/edge.conf.template:/etc/nginx/templates/default.conf.template:ro
    networks:
      default:
        aliases:
          - ${DEMO_HOST:-egret.localtest.me}
```

The `frontend` build uses `frontend/Dockerfile`; its nginx already proxies `/api` to `http://egret:8080` (see `frontend/nginx-prod.conf`), so the service name must stay `egret`.

- [ ] **Step 3: Add the OIDC and IdP keys to the `egret` service `environment:`**

```yaml
      EGRET_OIDC__ENABLED: "true"
      EGRET_OIDC__ISSUER_URL: ${DEMO_URL:-http://egret.localtest.me:8080}/auth/realms/egret
      EGRET_OIDC__CLIENT_ID: egret
      EGRET_OIDC__CLIENT_SECRET: egret-secret
      EGRET_OIDC__REDIRECT_URL: ${DEMO_URL:-http://egret.localtest.me:8080}/api/auth/oidc/callback
      EGRET_OIDC__GROUPS_CLAIM: groups
      EGRET_IDP__ISSUER: ${DEMO_URL:-http://egret.localtest.me:8080}
      EGRET_DATA_API__PUBLIC_URL: ${DEMO_URL:-http://egret.localtest.me:8080}
```

Compose interpolates `${DEMO_URL}` from the shell or `--env-file`. The `just` recipes (Task 7) pass `--env-file deploy/demo/demo.env` so `DEMO_URL`, `DEMO_PORT` and `DEMO_HOST` resolve. The `egret` service must also join the network where the edge alias lives (the default network, so no change).

- [ ] **Step 4: Start and verify the single origin**

Run:

```bash
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml up -d --build
curl -sI http://egret.localtest.me:8080/ | head -1
curl -s http://egret.localtest.me:8080/auth/realms/egret | python3 -c 'import json,sys;print(json.load(sys.stdin)["issuer"])'
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml exec egret bash -c 'exec 3<>/dev/tcp/egret.localtest.me/8080 && echo alias-ok'
```

Expected: `HTTP/1.1 200 OK`; the issuer printed is exactly `http://egret.localtest.me:8080/auth/realms/egret`; `alias-ok`.

- [ ] **Step 5: Verify both sign-in routes in a browser**

Open `http://egret.localtest.me:8080`. Expected: the sign-in page offers SSO. Sign in as `admin-sso` / `Admin12345!` (realm user in `deploy/keycloak/egret-realm.json`); you land in the app. Sign out; use **Admin sign-in (break-glass)** with `admin@egret.local` / `Admin12345!`; you land in the app. If SSO fails with an issuer mismatch, check the three issuer strings (Keycloak `KC_HOSTNAME`, `EGRET_OIDC__ISSUER_URL`, the printed `issuer`) are byte-identical.

- [ ] **Step 6: Commit**

```bash
git add deploy/demo/edge.conf.template deploy/docker-compose.demo.yml
git commit -m "feat(deploy): demo single-origin edge with Keycloak SSO and frontend"
```

---

### Task 5: Mailpit and the renderer

**Files:**
- Modify: `deploy/docker-compose.demo.yml` (add `mailpit`, `renderer`; add automation keys to `egret`)

**Interfaces:**
- Produces: SMTP relay `mailpit:1025` (UI published at `http://localhost:8025`); renderer at `renderer:8080`; `EGRET_AUTOMATION__*` set so PNG/PDF reports render and mail links work.

- [ ] **Step 1: Pick and record the Mailpit tag**

Run: `docker pull axllent/mailpit:v1.21 && docker image inspect axllent/mailpit:v1.21 --format '{{index .RepoDigests 0}}'`
Expected: a digest. Use `axllent/mailpit:v1.21@sha256:<that digest>` below. If `v1.21` does not exist, use the newest `vX.Y` tag from the registry and record it.

- [ ] **Step 2: Add the services**

```yaml
  mailpit:
    image: axllent/mailpit:v1.21@sha256:REPLACE_WITH_DIGEST_FROM_STEP_1
    ports:
      - "8025:8025"   # web UI: every mail the demo sends lands here
    environment:
      MP_SMTP_AUTH_ACCEPT_ANY: "1"
      MP_SMTP_AUTH_ALLOW_INSECURE: "1"

  renderer:
    image: egret-demo-renderer:dev
    build:
      context: ../renderer
    shm_size: 1gb
    healthcheck:
      test: ["CMD-SHELL", "node -e \"fetch('http://127.0.0.1:8080/healthz').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))\""]
      interval: 10s
      timeout: 5s
      retries: 20
      start_period: 15s
```

`REPLACE_WITH_DIGEST_FROM_STEP_1` is the digest printed in Step 1; replace it before committing (the plan cannot know it ahead of time, and `latest` is not allowed).

- [ ] **Step 3: Add the automation keys to `egret`**

```yaml
      EGRET_AUTOMATION__RENDERER_URL: http://renderer:8080
      # The renderer fetches /embed/print/... from egret's frontend by service name.
      EGRET_AUTOMATION__PUBLIC_URL: http://frontend
      # The links people open in mails.
      EGRET_AUTOMATION__BROWSER_URL: ${DEMO_URL:-http://egret.localtest.me:8080}
```

Add `renderer: { condition: service_started }` and `mailpit: { condition: service_started }` to the `egret` service's `depends_on`.

- [ ] **Step 4: Verify the renderer renders a page**

Run:

```bash
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml up -d --build renderer mailpit egret
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml ps renderer mailpit
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:8025/api/v1/info
```

Expected: `renderer` healthy, `mailpit` running, `200`. A real PNG/PDF check needs a dashboard; it is part of the Task 8 acceptance run.

- [ ] **Step 5: Commit**

```bash
git add deploy/docker-compose.demo.yml
git commit -m "feat(deploy): demo Mailpit and renderer, automation keys"
```

---

### Task 6: Idempotent seed

**Files:**
- Create: `deploy/demo/seed.sh`

**Interfaces:**
- Consumes: a running stack reachable at `${DEMO_URL}`; admin credentials from `deploy/demo/demo.env`.
- Produces: in workspace `default`: database `trino-demo` (host `trino-demo`, port 8080, catalog `tpch`), dataset `orders` (metrics `revenue`, `order_count`), charts "Revenue by order status", "Revenue by month", "Orders by priority", dashboard "Sales overview", and the workspace SMTP pointing at `mailpit:1025` (tls `none`). Re-running changes nothing.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# DEMO ONLY. Seed a running demo stack with sample content. Idempotent: every object is looked up
# by name first and created only when missing. Usage: just demo-seed
set -euo pipefail
cd "$(dirname "$0")/../.."
set -a; . deploy/demo/demo.env; set +a

BASE="${BASE:-http://${DEMO_HOST}:${DEMO_PORT}}"
WS="${WS:-default}"
JAR=$(mktemp); trap 'rm -f "$JAR"' EXIT

curl -fsS -c "$JAR" -H 'content-type: application/json' \
  -d "{\"email\":\"$EGRET_ADMIN_EMAIL\",\"password\":\"$EGRET_ADMIN_PASSWORD\"}" "$BASE/api/auth/login" >/dev/null
CSRF=$(awk '$6=="egret_csrf"{print $7}' "$JAR")

get()  { curl -fsS -b "$JAR" "$BASE$1"; }
call() { curl -fsS -b "$JAR" -H "X-Egret-CSRF: $CSRF" -H 'content-type: application/json' -X "$1" -d "$3" "$BASE$2"; }
id()   { python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])'; }
find_id() { # find_id <path> <name>: id of the first object with that name, or empty
  get "$1" | python3 -c '
import json,sys
want=sys.argv[1]
rows=json.load(sys.stdin)
rows=rows.get("items",rows) if isinstance(rows,dict) else rows
print(next((r["id"] for r in rows if r.get("name")==want),""))' "$2"
}

# 1. Trino database
DB=$(find_id "/api/ws/$WS/databases" trino-demo)
if [ -z "$DB" ]; then
  DB=$(call POST "/api/ws/$WS/databases" '{"name":"trino-demo","engine":"trino","host":"trino-demo","port":8080,"options":{"user":"egret","http_scheme":"http","catalog":"tpch"}}' | id)
  echo "database trino-demo $DB"
else echo "database trino-demo exists"; fi

# 2. Dataset and metrics
DS=$(find_id "/api/ws/$WS/datasets" orders)
if [ -z "$DS" ]; then
  DS=$(call POST "/api/ws/$WS/datasets" "{\"name\":\"orders\",\"database_id\":\"$DB\",\"description\":\"TPC-H sample orders\",\"source\":{\"kind\":\"physical\",\"catalog\":\"tpch\",\"schema\":\"tiny\",\"table\":\"orders\"}}" | id)
  call POST "/api/ws/$WS/datasets/$DS/metrics" '{"metric_name":"revenue","verbose_name":"Revenue","expression":"sum(totalprice)"}' >/dev/null
  call POST "/api/ws/$WS/datasets/$DS/metrics" '{"metric_name":"order_count","verbose_name":"Orders","expression":"count(*)"}' >/dev/null
  echo "dataset orders $DS"
else echo "dataset orders exists"; fi

# 3. Charts and dashboard
spec() { echo "{\"metrics\":$1,\"dimensions\":$2,\"columns\":[],\"filters\":[],\"orderby\":$3,\"row_limit\":1000,\"rollup_groups\":[]$4}"; }
chart() { # chart <name> <viz> <spec-json>
  local c; c=$(find_id "/api/ws/$WS/charts" "$1")
  if [ -z "$c" ]; then
    c=$(call POST "/api/ws/$WS/charts" "{\"dataset_id\":\"$DS\",\"name\":\"$1\",\"viz_type\":\"$2\",\"params\":{\"version\":2,\"settings\":{},\"spec\":$3}}" | id)
  fi
  echo "$c"
}
BAR=$(chart "Revenue by order status" bar "$(spec '["revenue"]' '["orderstatus"]' '[]' '')")
LINE=$(chart "Revenue by month" line "$(spec '["revenue"]' '["orderdate"]' '[{"field":"orderdate","desc":false}]' ',"temporal_units":{"orderdate":"month"}')")
TBL=$(chart "Orders by priority" table "$(spec '["order_count"]' '["orderpriority"]' '[]' '')")

DASH=$(find_id "/api/ws/$WS/dashboards" "Sales overview")
if [ -z "$DASH" ]; then
  DASH=$(call POST "/api/ws/$WS/dashboards" '{"name":"Sales overview","description":"Demo dashboard"}' | id)
  call PUT "/api/ws/$WS/dashboards/$DASH" "{\"name\":\"Sales overview\",\"description\":\"Demo dashboard\",\"parameters\":[],\"width\":\"full\",\"tabs\":[],\"cards\":[
   {\"chart_id\":\"$BAR\",\"card_type\":\"chart\",\"grid_col\":0,\"grid_row\":0,\"size_x\":12,\"size_y\":4,\"options\":{}},
   {\"chart_id\":\"$LINE\",\"card_type\":\"chart\",\"grid_col\":12,\"grid_row\":0,\"size_x\":12,\"size_y\":4,\"options\":{}},
   {\"chart_id\":\"$TBL\",\"card_type\":\"chart\",\"grid_col\":0,\"grid_row\":4,\"size_x\":24,\"size_y\":5,\"options\":{}}]}" >/dev/null
  echo "dashboard Sales overview $DASH"
else echo "dashboard Sales overview exists"; fi

# 4. Workspace SMTP -> Mailpit (a PUT replaces; safe to repeat). Needs EGRET_SECRETS__MASTER_KEY on the
#    server: without it this call answered 500 during the October 2026 docs run.
call PUT "/api/ws/$WS/notifications/smtp" '{"host":"mailpit","port":1025,"from_address":"egret@demo.local","from_name":"Egret demo","tls_mode":"none"}' >/dev/null
echo "smtp -> mailpit:1025"
echo "seed done: $BASE (workspace $WS)"
```

`chmod +x deploy/demo/seed.sh`.

- [ ] **Step 2: Run it twice and verify idempotency**

Run: `deploy/demo/seed.sh && deploy/demo/seed.sh`
Expected: the first run prints `database ... dataset ... dashboard ... smtp -> mailpit:1025 seed done`; the second prints `... exists` for each object and the same `smtp` and `seed done` lines, with no error. In the app, `default` workspace shows one `orders` dataset, three charts and one "Sales overview" dashboard (no duplicates).
If the SMTP step returns 500, the server lacks the master key: check `docker compose ... exec egret env | grep SECRETS__MASTER_KEY`.

- [ ] **Step 3: Verify mail delivery path**

In the UI: Workspace settings, Notifications, **Send test message**; or `curl` the API `POST /api/ws/default/notifications/smtp/test` with `{}`. Then `curl -s http://localhost:8025/api/v1/messages | python3 -c 'import json,sys;print(json.load(sys.stdin)["total"])'`
Expected: `1` or more.

- [ ] **Step 4: Commit**

```bash
git add deploy/demo/seed.sh
git commit -m "feat(deploy): idempotent demo seed (Trino database, dataset, charts, dashboard, SMTP)"
```

---

### Task 7: `just` recipes and the readiness script

**Files:**
- Create: `deploy/demo/wait.sh`
- Modify: `justfile` (add recipes after `down:`)

**Interfaces:**
- Produces: `just demo-up`, `just demo-down`, `just demo-reset`, `just demo-seed`, `just demo-logs`. `demo-up` builds if needed, starts, waits, seeds, and prints the URL and credentials.
- Note for WP-C: `demo-down` already removes containers labelled `egret.managed=true` (none exist until WP-C), so WP-C only has to create them with that label.

- [ ] **Step 1: Create `deploy/demo/wait.sh`**

```bash
#!/usr/bin/env bash
# DEMO ONLY. Wait until the demo answers, then print how to use it.
set -euo pipefail
cd "$(dirname "$0")/../.."
set -a; . deploy/demo/demo.env; set +a
URL="http://${DEMO_HOST}:${DEMO_PORT}"
echo -n "waiting for $URL "
for _ in $(seq 1 180); do
  if curl -fsS -o /dev/null "$URL/api/auth/config" 2>/dev/null || curl -fsS -o /dev/null "$URL/" 2>/dev/null; then
    echo " ready"
    cat <<EOF

  Egret      $URL
  Admin      $EGRET_ADMIN_EMAIL / $EGRET_ADMIN_PASSWORD   (SSO users: admin-sso / Admin12345!, member-sso / Member12345!)
  Mail       http://localhost:8025    (Mailpit: every mail the demo sends)
  Trino      database "trino-demo" in workspace "default" (catalogs tpch, memory)

  DEMO ONLY: public development secrets, never expose this to a network.
EOF
    exit 0
  fi
  echo -n "."; sleep 5
done
echo; echo "timed out; see: just demo-logs" >&2; exit 1
```

`chmod +x deploy/demo/wait.sh`. If `/api/auth/config` does not exist the second `curl` still passes once the edge serves `/`.

- [ ] **Step 2: Add the recipes to `justfile`** (after the existing `down:` recipe)

```just
# ---- Demo stack: the whole platform on Docker, no Kubernetes (see docs/en/guides/demo-stack.md) ----
demo_compose := "docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml"

# Build, start, seed, and print the URL and credentials
demo-up:
    {{demo_compose}} up -d --build
    ./deploy/demo/wait.sh
    ./deploy/demo/seed.sh

# Stop the demo; removes engine containers egret provisioned (label egret.managed=true); keeps data volumes
demo-down:
    -docker ps -aq --filter label=egret.managed=true | xargs -r docker rm -f
    {{demo_compose}} down

# Stop the demo and DELETE its data volumes
demo-reset:
    -docker ps -aq --filter label=egret.managed=true | xargs -r docker rm -f
    {{demo_compose}} down -v

# Re-run the idempotent seed
demo-seed:
    ./deploy/demo/seed.sh

# Follow the demo's logs (optionally one service: just demo-logs egret)
demo-logs service="":
    {{demo_compose}} logs -f {{service}}
```

`xargs -r` is a GNU flag; on macOS use `xargs` without `-r` guarded by a non-empty check. Use this portable form instead of the two `xargs -r` lines:

```just
    -ids=$(docker ps -aq --filter label=egret.managed=true); [ -z "$ids" ] || docker rm -f $ids
```

- [ ] **Step 3: Verify the recipes**

Run: `just demo-down && just demo-up`
Expected: ends with the printed URL block and `seed done`; `just demo-down` leaves volumes (`docker volume ls | grep egret-demo` shows two); a second `just demo-up` reuses data, and the seed prints only `exists` lines. `just demo-reset` removes the volumes.

- [ ] **Step 4: Commit**

```bash
git add deploy/demo/wait.sh justfile
git commit -m "feat(deploy): just demo-up/down/reset/seed/logs and readiness script"
```

---

### Task 8: Fix `.env.example`, write the guide, acceptance run, PR

**Files:**
- Modify: `.env.example` (KI-136)
- Create: `docs/en/guides/demo-stack.md`
- Modify: `README.md` (link)

- [ ] **Step 1: Add the missing storage variables to `.env.example`**

Append after the RustFS block:

```bash
# Object store used by the apps/workflow/catalog features (required: Config::validate fails without them).
# The bucket must exist: `just up` does not create it; `just demo-up` does.
EGRET_STORAGE__ENDPOINT=http://localhost:9000
EGRET_STORAGE__BUCKET=warehouse
EGRET_STORAGE__ACCESS_KEY=egret
EGRET_STORAGE__SECRET_KEY=egretsecret
EGRET_STORAGE__REGION=us-east-1
EGRET_STORAGE__PATH_STYLE=true
EGRET_SECRETS__MASTER_KEY=q1IaZ2X7N3i/um/U/aJ2lsmVrRE5pTZChAVG9ezDKPw=
```

Verify: `cp .env.example /tmp/e && grep -c EGRET_STORAGE__ /tmp/e` prints `6`, and `set -a; . ./.env.example; set +a; cargo run -p egret-bin -- migrate` no longer fails with `the apps API requires [storage] ...` (needs a running Postgres from `just up`; if the bucket is missing that is the documented gap KI-137, not this check).

- [ ] **Step 2: Write `docs/en/guides/demo-stack.md`**

Sections, in this order: *What you get* (service table with URLs and credentials from Task 7's output); *Requirements* (Docker with Compose v2, about 6 to 8 GB RAM free, the first build takes 20 to 40 minutes for the Rust compile, later builds are cached; `*.localtest.me` needs DNS, offline add `127.0.0.1 egret.localtest.me` to `/etc/hosts`); *Run it* (`just demo-up`, URL, logins); *Stop and reset* (`demo-down` vs `demo-reset`); *Change the port* (`DEMO_PORT` in `deploy/demo/demo.env`, and set `DEMO_URL` to match); *What is not in this stack yet* (Data Apps and engine provisioning arrive in the next packages; say so plainly); *Safety* (public dev secrets, bind only to localhost, never expose; the stack is not a production topology); *Troubleshooting* (SSO issuer mismatch, port in use, `demo-logs egret`).

- [ ] **Step 3: Link it from `README.md`**

In the quick start section, add: `Try everything on Docker with one command: see [docs/en/guides/demo-stack.md](docs/en/guides/demo-stack.md).`

- [ ] **Step 4: Acceptance run on a clean state (spec Testing, package A)**

Run `just demo-reset && just demo-up` from a fresh clone path (or after `git clean -xfd deploy/demo` plus `docker image rm` of the four demo images) and tick every line:

- [ ] `/readyz` of `egret` is healthy; `docker compose ... ps` shows no unhealthy service.
- [ ] Browser: SSO login as `admin-sso`; local break-glass login as `admin@egret.local`.
- [ ] `default` workspace shows the dataset, three charts, dashboard with data (Trino answers).
- [ ] SQL Lab runs `select count(*) from tpch.tiny.orders` against `trino-demo` and returns 15000.
- [ ] A report with CSV format sends a mail visible in Mailpit; a report with PDF format on "Sales overview" finishes with state Success and the mail has a PDF attachment (proves renderer, `AUTOMATION__PUBLIC_URL`).
- [ ] Workflows: `Run` on a new flow finishes (worker and scheduler on) instead of staying Running.
- [ ] `just demo-seed` twice more: only `exists` lines.
- [ ] `just demo-down` removes the containers and keeps the volumes; `just demo-up` returns with the data intact; `just demo-reset` removes the volumes.
- [ ] `gitleaks detect --no-git -c .gitleaks.toml` reports nothing beyond the allow-listed public dev values (add allow-list entries for `deploy/demo/demo.env` if gitleaks flags the documented keys, with a comment explaining they are public).

Record anything that did not pass in the PR description under "Known gaps" rather than hiding it.

- [ ] **Step 5: Docs PR in `egret-data/docs`, or say where it will be**

Open a follow-up PR in `egret-data/docs` that adds a "Run the demo stack" page and makes it the primary route in Quick start (the spec's "Docs follow-up"), or write in this PR's description that it follows after the acceptance screenshots are re-shot. Tick the Docs box in the PR template accordingly.

- [ ] **Step 6: Commit, push, open the PR**

```bash
git add .env.example docs/en/guides/demo-stack.md README.md
git commit -m "docs: demo stack guide; fix .env.example storage variables"
git push -u origin HEAD
gh pr create --title "feat(deploy): Docker demo stack (compose, WP-A)" --body "<summary, acceptance checklist results, known gaps, docs follow-up>

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Do not merge; the owner decides.

---

## Self-review against the spec (WP-A)

| Spec requirement | Task |
|---|---|
| Services `postgres`, `rustfs`, `keycloak` | 1, 4 |
| `rustfs-init` bucket one-shot (KI-137) | 1 |
| `mailpit`, workspace SMTP pre-seeded | 5, 6 |
| `renderer`, `render.url` | 5 (`EGRET_AUTOMATION__RENDERER_URL`) |
| `trino-demo` plain HTTP, `process-forwarded`, `tpch`+`memory`, registered by the seed | 2, 6 |
| `egret` with migrate, init, serve entrypoint, waits for deps | 3 |
| `frontend` proxying `/api`; one port a user opens | 4 (edge) |
| `.env.example` storage variables (KI-136) | 8 |
| `just demo-up/down/reset/seed` | 7 |
| Readiness script prints URL and credentials | 7 |
| Banner and docs say never expose | 1 (compose header), 8 (guide) |
| `demo-down` removes engines by label, keeps volumes; `demo-reset` deletes volumes | 7 |
| Test: fresh clone, readyz, login, SSO, seed twice, `demo-down` leaves no managed containers | 8 |
| Master key (cause of the SMTP 500 seen in docs) | Global Constraints, 6 |

Differences from the spec worth noting: the spec lists the frontend on 8080 only; the plan adds an **edge** container so Keycloak shares the origin (needed for one OIDC issuer string). The spec's "apps host second port" belongs to WP-B and is intentionally absent. The workflow container runner (`EGRET_WORKFLOW__CONTAINER_RUNNER`) is not configured in WP-A; container tasks are expected to fail in this package and are checked off in the acceptance run only for plain tasks.
