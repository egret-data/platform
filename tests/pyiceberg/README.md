# pyiceberg e2e

End-to-end test using a real pyiceberg client against a running Egret catalog server.

## Prerequisites

- Docker / `just up` running (Postgres + MinIO)
- MinIO CLI (`mc`) installed
- Python 3.9+ with pip
- Egret server running with catalog enabled

## Runbook

```text
# pyiceberg e2e
1. just up            # Postgres + MinIO
2. mc alias set local http://localhost:9000 minioadmin minioadmin && mc mb -p local/warehouse
3. configure egret.toml: [catalog].enabled=true, auth_token=devtoken, [storage] creds=minioadmin
4. cargo run -p egret-bin -- serve         # in another shell
5. pip install -r tests/pyiceberg/requirements.txt
6. EGRET_TEST_E2E=1 EGRET_CATALOG_URI=http://localhost:8080/catalog EGRET_CATALOG_TOKEN=devtoken \
   EGRET_TEST_S3_ENDPOINT=http://localhost:9000 \
   cargo test -p egret-catalog pyiceberg_gate
```

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `EGRET_TEST_E2E` | Yes | unset | Set to `1` to run (skips otherwise) |
| `EGRET_CATALOG_URI` | Yes | — | Base URL of the catalog, e.g. `http://localhost:8080/catalog` |
| `EGRET_CATALOG_TOKEN` | Yes | — | Bearer token matching `[catalog].auth_token` in egret.toml |
| `EGRET_TEST_S3_ENDPOINT` | Yes | — | MinIO endpoint, e.g. `http://localhost:9000` |
| `EGRET_TEST_S3_KEY` | No | `minioadmin` | S3 access key |
| `EGRET_TEST_S3_SECRET` | No | `minioadmin` | S3 secret key |
