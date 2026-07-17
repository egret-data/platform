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

# Build both egret images (backend + frontend).
images-build:
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
    docker build -f frontend/Dockerfile -t egret-frontend:dev frontend

# Build images, load them into the kind `egret` cluster, and (re)install the chart.
deploy-kind: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
    @echo "egret SPA: http://localhost:30080  (login admin@egret.local / Admin12345!)"

# Uninstall the egret release (leaves the kind cluster + operators up).
kind-undeploy:
    helm uninstall egret -n egret-system || true
