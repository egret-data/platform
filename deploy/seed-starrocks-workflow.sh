#!/usr/bin/env bash
# Seed the fixture `crates/egret-workflow/tests/sql_starrocks_live.rs` queries:
# four rows in `egret_workflow_live.shipments`, which the flow groups into
# `EU => 91` and `US => 13`.
#
# The values matter, and they are deliberately NOT the ones
# `deploy/seed-starrocks-dataapi.sh` writes (`EU => 42`, `US => 7`). Each figure
# here is a SUM of two rows, so it is something StarRocks COMPUTED rather than
# echoed: `91` and `13` appear nowhere in the flow source, in the test, or in any
# fixture in `egret-workflow`, and the `Spy` port that every other test in that
# crate drives could not invent them. That is what makes the assertion evidence
# about the engine rather than about the harness.
#
# `region` deliberately contains a row whose value is the string `EU' OR '1'='1`.
# It is there so the injection assertion has a POSITIVE control: a bound
# parameter carrying that text must match that row and nothing else, which
# distinguishes "the value was bound" from "the statement was broken".
#
# Idempotent: `CREATE ... IF NOT EXISTS` plus a TRUNCATE before the insert, so
# re-running it leaves exactly four rows rather than four more.
#
# Usage:  ./deploy/seed-starrocks-workflow.sh [container-name]
set -euo pipefail

CONTAINER="${1:-egret-dev-starrocks-1}"

if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "error: container '$CONTAINER' is not running. Start it with:" >&2
  echo "  docker compose -f deploy/docker-compose.dev.yml up -d starrocks" >&2
  exit 1
fi

for _ in $(seq 1 60); do
  if docker exec "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot -e 'SELECT 1' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

docker exec -i "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot <<'SQL'
CREATE DATABASE IF NOT EXISTS egret_workflow_live;
USE egret_workflow_live;
CREATE TABLE IF NOT EXISTS shipments (
  region VARCHAR(32),
  units  INT
) ENGINE=OLAP
  DUPLICATE KEY(region)
  DISTRIBUTED BY HASH(region) BUCKETS 1
  PROPERTIES('replication_num' = '1');
TRUNCATE TABLE shipments;
INSERT INTO shipments VALUES ('EU', 40), ('EU', 51), ('US', 6), ('US', 7), ('EU\' OR \'1\'=\'1', 0);
SQL

echo "seeded egret_workflow_live.shipments:"
docker exec "$CONTAINER" mysql -h127.0.0.1 -P9030 -uroot -e \
  'SELECT region, sum(units) AS units FROM egret_workflow_live.shipments GROUP BY region ORDER BY region;
   SELECT count(*) AS all_rows FROM egret_workflow_live.shipments;'
