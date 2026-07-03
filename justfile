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

# Run the server
serve:
    cargo run -p egret-bin -- serve
