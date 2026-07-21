# Egret dev commands. Run `just --list` to see all.

set dotenv-load := true

# Start the local dev stack (Postgres, MinIO, Trino, StarRocks)
up:
    docker compose -f deploy/docker-compose.dev.yml up -d

# Stop the dev stack
down:
    docker compose -f deploy/docker-compose.dev.yml down

# Format, lint, and test everything
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Apply migrations against the running Postgres
migrate:
    cargo run -p egret-bin -- migrate

# Reset local test data (keeps users + demo/eng; wipes catalog, test workspaces, MinIO)
reset-testdata:
    ./deploy/reset-testdata.sh

# Seed a ready-to-test demo catalog (warehouse + role + client + demo_ns.events). Needs a running serve.
seed-demo:
    ./deploy/seed-demo-catalog.sh

# Run the server
serve:
    cargo run -p egret-bin -- serve

# Opt-in Trino-on-k8s e2e harness: kind cluster + target namespace (NOT in CI). See deploy/k8s/README.md
k8s-up:
    kind create cluster --name egret --config deploy/k8s/kind.yaml
    # Trino provisions as plain Deployments egret applies directly — no operator.
    # StarRocks/RisingWave phases will add their own operators here later.
    kubectl create namespace egret-engines

# Tear down the kind e2e cluster
k8s-down:
    kind delete cluster --name egret

# Build all egret images (backend + frontend + kernel).
images-build:
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
    docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
    docker build -t egret-kernel:dev kernel-image

# Build images, load them into the kind `egret` cluster, and (re)install the chart.
# Force fresh pods onto the just-loaded `:dev` images. A `helm upgrade` with an
# unchanged image tag does NOT restart pods, so a rebuilt `:dev` image is ignored
# without this — the classic same-tag gotcha. Also refreshes per-user kernel pods
# (spawned on-demand, they'd otherwise keep the old kernel image). NOTE: never run
# two image builds concurrently — both tag `:dev` and the last to finish wins,
# which can deploy stale code.
_kind-refresh:
    kubectl rollout restart deploy/egret deploy/egret-frontend -n egret-system
    kubectl rollout status deploy/egret -n egret-system --timeout=180s
    kubectl delete pods -n egret-notebooks --all --ignore-not-found=true

deploy-kind: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
    just _kind-refresh
    @echo "egret SPA: http://localhost:30080  (login admin@egret.local / Admin12345!)"

# Like deploy-kind but OIDC-on + ingress (Milestone B) — required for the data-plane e2e.
# Also enables the k8s per-user kernel launcher: this is the canonical notebook-execution path.
deploy-kind-oidc: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true --set kernel.launcher=kubernetes
    just _kind-refresh
    @echo "egret SPA: http://egret.localtest.me  (login admin@egret.local / Admin12345!)"

# Opt-in data-plane e2e against the in-cluster kind deploy (egret.localtest.me).
# One-time prereq: just k8s-up && just ingress-install && just coredns-rewrite
e2e-kind: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test dataplane-kind

# Opt-in notebook write-isolation e2e: an admin reserves the workspace's notebook
# (scratch) warehouse + namespace, then a real per-user kernel round-trips a Daft
# frame through that private Iceberg namespace and a raw out-of-namespace write is
# denied by the catalog guard. NO Ranger — the notebook kernel stack suffices.
e2e-kind-notebook-write: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test notebook-write-isolation-kind

# Opt-in notebook visualization e2e: a real per-user kernel renders a plotly figure
# (application/vnd.plotly.v1+json → notebook-plotly + lazy plotly.js) and a matplotlib
# figure (image/png → inline <img>). NO Ranger — the notebook kernel stack suffices.
e2e-kind-notebook-viz: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test notebook-viz-kind

# Like deploy-kind-oidc but ALSO Ranger-on — provisioned Trino engines bind Ranger
# (egret embeds the Ranger admin at /service). For the governance enforcement e2e.
deploy-kind-governance: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true --set kernel.launcher=kubernetes
    just _kind-refresh
    @echo "egret (ranger-on): http://egret.localtest.me"

# Opt-in governance enforcement e2e (mask + row-filter on a live Trino query).
e2e-kind-governance: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test governance-kind

# Opt-in auto-admin e2e: admin queries a governed engine with NO hand-seeded policy
# (proves the backend auto-seeds the egret-admin all-access Ranger policy on provision).
e2e-kind-ranger-auto: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test ranger-auto-admin-kind

# Opt-in per-user-enforcement e2e: drives two OIDC identities (admin-sso full vs
# member-sso masked+row-filtered) against the same table/dataset/chart/dashboard/
# notebook, proving per-user Ranger enforcement is consistent across all surfaces.
e2e-kind-per-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test per-user-enforcement-kind

# Opt-in STS token-broker e2e: a break-glass local admin (NO Keycloak refresh token)
# queries a governed StarRocks engine from a notebook purely off an egret-minted
# token, and admin-sso/member-sso Ranger enforcement (full vs masked) still holds
# under that same egret-minted-token path.
e2e-kind-sts: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test sts-broker-kind

# Opt-in: default provision (no delegationMode) => both engines come up jwt-delegated.
e2e-kind-delegation-default: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test delegation-default-kind

# Opt-in local user-management e2e: admin creates a local user, the user logs in and
# holds a live session, then admin disables them — proving the disabled user's live
# session is revoked IMMEDIATELY (not just blocked on their next login). Also covers
# password-reset session invalidation and hard-delete.
e2e-kind-local-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test local-user-mgmt-kind

# Opt-in back-channel logout e2e: Keycloak force-logout → egret session revoked.
# Port-forwards Keycloak to loopback so the admin API (used to wire the client +
# force-logout) is reachable over HTTP — Keycloak's sslRequired=external rejects
# admin-cli over plain HTTP from non-loopback but exempts 127.0.0.1 (weakens no config).
e2e-kind-backchannel-logout: deploy-kind-governance
    #!/usr/bin/env bash
    set -euo pipefail
    ./deploy/k8s/wait-ready.sh
    kubectl port-forward -n egret-system svc/keycloak 18080:8080 >/tmp/egret-kc-pf.log 2>&1 &
    PF=$!
    trap 'kill $PF 2>/dev/null || true' EXIT
    sleep 3
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_KC_ADMIN_URL=http://127.0.0.1:18080 \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test backchannel-logout-kind

# Uninstall the egret release (leaves the kind cluster + operators up).
kind-undeploy:
    helm uninstall egret -n egret-system || true

# Install the kind-flavored ingress-nginx controller (needs a cluster created with the ingress-ready label + :80 map).
ingress-install:
    kubectl apply -f https://raw.githubusercontent.com/kubernetes/ingress-nginx/main/deploy/static/provider/kind/deploy.yaml
    kubectl wait --namespace ingress-nginx --for=condition=ready pod --selector=app.kubernetes.io/component=controller --timeout=180s

# Make egret.localtest.me resolve cluster-wide to the ingress controller (so in-cluster
# pods — incl. provisioned engines fetching the OIDC JWKS — reach it). Run AFTER ingress-install.
coredns-rewrite:
    ./deploy/k8s/coredns-rewrite.sh
