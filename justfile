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

# Opt-in Trino-on-k8s e2e harness: kind cluster + Stackable operators + namespace (NOT in CI). See deploy/k8s/README.md
k8s-up:
    kind create cluster --name egret --config deploy/k8s/kind.yaml
    helm repo add stackable-stable https://repo.stackable.tech/repository/helm-stable/
    helm repo update
    helm install --namespace stackable-operators --create-namespace commons-operator stackable-stable/commons-operator
    helm install --namespace stackable-operators --create-namespace secret-operator stackable-stable/secret-operator
    helm install --namespace stackable-operators --create-namespace listener-operator stackable-stable/listener-operator
    helm install --namespace stackable-operators --create-namespace trino-operator stackable-stable/trino-operator
    kubectl create namespace egret-engines

# Tear down the kind e2e cluster
k8s-down:
    kind delete cluster --name egret
