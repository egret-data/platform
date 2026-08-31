#!/usr/bin/env bash
# Seed the fixture table that `crates/egret-dataapi/tests/serving_starrocks_live.rs`
# queries: three rows in `egret_dataapi_live.regions`, which the endpoint groups into
# `EU => 42` and `US => 7`.
#
# The values matter. `42` is a sum of two rows, so it is something StarRocks COMPUTED
# rather than echoed — it appears nowhere in the request, the endpoint definition, or any
# fixture in that crate, and `serving_common::FakePort` (whose rows derive from the calling
# principal's UUID) cannot produce it. That is what makes the test's assertion evidence
# about the engine rather than about the harness.
#
# Idempotent: `CREATE ... IF NOT EXISTS` plus a TRUNCATE before the insert, so re-running
# it leaves exactly three rows rather than three more.
#
# Usage:  ./deploy/seed-starrocks-dataapi.sh [container-name]
set -euo pipefail

CONTAINER="${1:-egret-dev-starrocks-1}"

if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "error: container '$CONTAINER' is not running. Start it with:" >&2
  echo "  docker compose -f deploy/docker-compose.dev.yml up -d starrocks" >&2
  exit 1
fi

# Wait for the FE to accept queries. The allin1 image takes ~30-60s from `up` to ready,
# and a seed that ran too early would fail in a way that reads like a SQL error.
for _ in $(seq 1 60); do
  if docker exec "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot -e 'SELECT 1' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

docker exec -i "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot <<'SQL'
CREATE DATABASE IF NOT EXISTS egret_dataapi_live;
USE egret_dataapi_live;
CREATE TABLE IF NOT EXISTS regions (
  region VARCHAR(8),
  units  INT
) ENGINE=OLAP
  DUPLICATE KEY(region)
  DISTRIBUTED BY HASH(region) BUCKETS 1
  PROPERTIES('replication_num' = '1');
TRUNCATE TABLE regions;
INSERT INTO regions VALUES ('EU', 10), ('EU', 32), ('US', 7);
SQL

echo "seeded egret_dataapi_live.regions:"
docker exec "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot -e \
  'SELECT region, sum(units) AS units FROM egret_dataapi_live.regions GROUP BY region ORDER BY region;'
