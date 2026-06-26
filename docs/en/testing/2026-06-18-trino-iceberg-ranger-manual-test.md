# Manual Integration Test — Trino × Egret Iceberg Catalog × Ranger

> **Status:** Manual pre-push gate (NOT run in CI) · **Date:** 2026-06-18
> Canonical (English). Vietnamese reference: [`docs/vi/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../../vi/testing/2026-06-18-trino-iceberg-ranger-manual-test.md).
> Run this before pushing changes that touch `egret-catalog`, `egret-storage`, `egret-authz`, or `egret-ranger`. It exercises the engine-facing contracts (Iceberg REST + Ranger policy download) against a **real Trino**, which surfaces gaps unit tests cannot (reserved properties, empty-location guard, policy `id`, plugin auth).

## What it proves

A real **Trino 481** uses Egret for BOTH governance-core features at once:
- **Iceberg REST catalog** — `CREATE SCHEMA / CREATE TABLE / INSERT / SELECT` flow data through Egret's `/catalog` into MinIO.
- **Apache Ranger access control** — Trino's built-in `ranger` plugin downloads policies from Egret's `/service` and enforces **deny / row-filter / column-mask** on live queries.

## Why it is not in CI

Needs Docker + a JVM-heavy Trino image + MinIO; the Trino Ranger plugin polls on a timer. Too slow/heavy for CI. The unit + DB-gated + `RangerPolicyEngineImpl` gate tests cover the logic in CI; this fills the **engine-contract** gap manually.

## Prerequisites

- Docker running.
- Repo built: `cargo build -p egret-bin`.
- A Postgres reachable at `postgres://egret:egret@localhost:5432/egret` (the dev `portal-pg` or `docker compose -f deploy/docker-compose.dev.yml up -d postgres`).
- Free host ports: 8080 (egret), 9000/9001 (MinIO), 8090 (Trino).

## Networking note (important)

Egret runs on the host; Trino + MinIO run in containers. Egret's signer has an SSRF host-check, and Trino's S3 endpoint must match what Egret signs/writes. Use the host's **LAN IP** as the single address everyone shares:

```bash
export HOST_IP=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)   # macOS; Linux: hostname -I | awk '{print $1}'
echo "HOST_IP=$HOST_IP"
```

---

## 1. Start MinIO + create the warehouse bucket

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d minio
# wait for ready
until [ "$(curl -s -o /dev/null -w '%{http_code}' http://localhost:9000/minio/health/ready)" = 200 ]; do sleep 2; done
NET=egret-dev_default
docker run --rm --network "$NET" --entrypoint sh minio/mc -c \
  'mc alias set m http://minio:9000 egret egretsecret && mc mb -p m/warehouse'
```

## 2. Run Egret with catalog + ranger enabled

`ranger.auth_token = ""` selects **anonymous `/service`** — the stock Trino Ranger plugin authenticates out-of-band (not via bearer), so it cannot use a token-protected endpoint.

```bash
mkdir -p /tmp/egret-itest
cat > /tmp/egret-itest/egret.toml <<EOF
[server]
host = "0.0.0.0"
port = 8080
[database]
url = "postgres://egret:egret@localhost:5432/egret"
[catalog]
enabled = true
warehouse = "s3://warehouse"
auth_token = "devtoken"
public_url = "http://$HOST_IP:8080"
[storage]
endpoint = "http://$HOST_IP:9000"
region = "us-east-1"
bucket = "warehouse"
access_key = "egret"
secret_key = "egretsecret"
path_style = true
[ranger]
enabled = true
auth_token = ""
EOF

BIN=$(pwd)/../target/debug/egret
(cd /tmp/egret-itest && "$BIN" migrate && nohup "$BIN" serve > /tmp/egret-itest/server.log 2>&1 &)
sleep 3
curl -s -o /dev/null -w "egret healthz: %{http_code}\n" http://$HOST_IP:8080/healthz
curl -s -H "Authorization: Bearer devtoken" "http://$HOST_IP:8080/catalog/v1/config?warehouse=<WS_SLUG>" | head -c 200; echo
curl -s -o /dev/null -w "anon /service: %{http_code}\n" http://$HOST_IP:8080/service/plugins/definitions
```
Expect: `healthz 200`, a `/v1/config` JSON whose `overrides` include `prefix=<WS_SLUG>` + `warehouse` + `s3.signer.uri` (omitting `?warehouse` returns signer keys only, no prefix), `anon /service: 200`.

## 3. Configure + start Trino

### 3a. Iceberg catalog → Egret (`deploy/trino/catalog/iceberg.properties`)

The catalog is now **per-workspace**: every Iceberg REST route is prefixed with
the workspace **slug** (`/v1/{prefix}/...`). Set `iceberg.rest-catalog.warehouse`
to the workspace slug — Trino calls `GET /v1/config?warehouse=<slug>`, reads back
`overrides.prefix=<slug>` + `overrides.warehouse`, and targets the prefixed routes
automatically. Replace `<WS_SLUG>` below with a real workspace slug (e.g. from the
`workspaces.slug` column, or one provisioned at workspace creation).

```bash
cat > trino/catalog/iceberg.properties <<EOF
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://$HOST_IP:8080/catalog
iceberg.rest-catalog.warehouse=<WS_SLUG>
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.token=devtoken
fs.native-s3.enabled=true
s3.endpoint=http://$HOST_IP:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=egret
s3.aws-secret-key=egretsecret
EOF
```

### 3b. Ranger access control (copied into the container's config dir)

```bash
cat > /tmp/ac.properties <<EOF
access-control.name=ranger
ranger.service.name=trino_prod
ranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml
EOF

cat > /tmp/ranger-trino-security.xml <<EOF
<?xml version="1.0"?>
<configuration>
  <property><name>ranger.plugin.trino.service.name</name><value>trino_prod</value></property>
  <property><name>ranger.plugin.trino.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>
  <property><name>ranger.plugin.trino.policy.rest.url</name><value>http://$HOST_IP:8080</value></property>
  <property><name>ranger.plugin.trino.policy.pollIntervalMs</name><value>5000</value></property>
  <property><name>ranger.plugin.trino.policy.cache.dir</name><value>/tmp/ranger-trino-cache</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.connection.timeoutMs</name><value>10000</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.read.timeoutMs</name><value>10000</value></property>
</configuration>
EOF
```
The Ranger `RangerAdminRESTClient` downloads from `<policy.rest.url>/service/plugins/policies/download/<service.name>` — i.e. Egret's `/service`. Trino 481 config keys: `ranger.service.name`, `ranger.plugin.config.resource`.

### 3c. Start Trino, inject the ranger config, restart

```bash
docker compose -f docker-compose.dev.yml up -d trino
# wait until ready (starting=False)
until [ "$(curl -s http://localhost:8090/v1/info | python3 -c 'import sys,json;print(json.load(sys.stdin)["starting"])' 2>/dev/null)" = False ]; do sleep 3; done
docker cp /tmp/ac.properties egret-dev-trino-1:/etc/trino/access-control.properties
docker cp /tmp/ranger-trino-security.xml egret-dev-trino-1:/etc/trino/ranger-trino-security.xml
docker restart egret-dev-trino-1
until [ "$(curl -s http://localhost:8090/v1/info | python3 -c 'import sys,json;print(json.load(sys.stdin)["starting"])' 2>/dev/null)" = False ]; do sleep 3; done
```

## 4. Catalog test (read-write through Egret)

```bash
TX() { docker exec egret-dev-trino-1 trino --server localhost:8080 --user admin --execute "$1"; }
TX "CREATE SCHEMA IF NOT EXISTS iceberg.demo"
TX "CREATE TABLE iceberg.demo.report (id bigint, item varchar, country varchar, amount double)"
TX "INSERT INTO iceberg.demo.report VALUES (1,'book','US',9.5),(2,'pen','VN',1.2),(3,'lamp','US',20.0),(4,'desk','VN',55.0)"
TX "SELECT country, count(*) n, sum(amount) total FROM iceberg.demo.report GROUP BY country ORDER BY country"
```
Expect: `CREATE SCHEMA`, `CREATE TABLE`, `INSERT: 4 rows`, and `US 2 29.5 / VN 2 56.2`. Verify the files landed in MinIO under `warehouse/demo/report-*/` (`data/*.parquet`, `metadata/0000N-*.metadata.json` — note there is **no** `00000`; it is written on the first commit, not on create, which is what lets Trino's empty-location guard pass).

## 5. Ranger test (policy enforcement on live queries)

The Trino catalog name is `iceberg`, so Ranger resources use `catalog=iceberg`. Default-deny means a user needs `execute` on `queryid` and `impersonate` on `trinouser` to run any query, plus `select` on the table.

```bash
P() { curl -s -o /dev/null -w "  %{http_code}\n" -X POST "http://$HOST_IP:8080/service/plugins/policies" -H "Content-Type: application/json" -d "$1"; }
# the service the plugin polls
curl -s -X POST "http://$HOST_IP:8080/service/plugins/services" -H "Content-Type: application/json" -d '{"name":"trino_prod","type":"trino","configs":{}}' >/dev/null
# global: everyone may run queries + set their own session user
P '{"service":"trino_prod","name":"g-execquery","policyType":0,"isEnabled":true,"resources":{"queryid":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"execute","isAllowed":true}],"groups":["public"]}]}'
P '{"service":"trino_prod","name":"g-impersonate","policyType":0,"isEnabled":true,"resources":{"trinouser":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"impersonate","isAllowed":true}],"groups":["public"]}]}'
# table access: alice + admin may select report
P '{"service":"trino_prod","name":"ice-access","policyType":0,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]},"column":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice","admin"]}]}'
# column mask: alice sees `item` masked
P '{"service":"trino_prod","name":"ice-mask","policyType":1,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]},"column":{"values":["item"]}},"dataMaskPolicyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"],"dataMaskInfo":{"dataMaskType":"MASK"}}]}'
# row filter: alice only sees US rows
P '{"service":"trino_prod","name":"ice-rowfilter","policyType":2,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]}},"rowFilterPolicyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"],"rowFilterInfo":{"filterExpr":"country = '"'"'US'"'"'"}}]}'

sleep 8   # let the plugin poll (pollIntervalMs=5000)
Q="SELECT * FROM iceberg.demo.report ORDER BY id"
echo "--- admin ---"; docker exec egret-dev-trino-1 trino --server localhost:8080 --user admin --execute "$Q"
echo "--- alice ---"; docker exec egret-dev-trino-1 trino --server localhost:8080 --user alice --execute "$Q"
echo "--- bob ---";   docker exec egret-dev-trino-1 trino --server localhost:8080 --user bob   --execute "$Q"
```

### Expected enforcement (PASS criteria)

| User | Rows returned | `item` column | Meaning |
|---|---|---|---|
| `admin` | 4 (all) | `book/pen/lamp/desk` | full access |
| `alice` | 2 (US only) | `xxxx` | **row-filter `country='US'` + column MASK** |
| `bob` | query fails | — | `Access Denied: Cannot access catalog iceberg` (default-deny) |

If you see those three outcomes, both Egret features are correctly enforced by a real engine. You can confirm the plugin pulled Egret's policies: `docker exec egret-dev-trino-1 cat /tmp/ranger-trino-cache/trino_trino_prod.json` shows the `ServicePolicies` with numeric policy `id`s.

## 6. Cleanup

```bash
pkill -f "target/debug/egret serve"
cd deploy && docker compose -f docker-compose.dev.yml rm -fs trino minio
rm -rf /tmp/egret-itest /tmp/ac.properties /tmp/ranger-trino-security.xml
rm -f trino/catalog/iceberg.properties   # host-specific; never commit
```

## Contract notes (what this test guards)

1. **Reserved table properties** — Trino sends `format-version` in `CREATE TABLE`; Egret must strip `iceberg`'s `RESERVED_PROPERTIES` before building metadata, or it 400s.
2. **Empty-location guard** — Egret must NOT pre-write `metadata.json` on create; Trino verifies the table location is empty. The DB jsonb is the source of truth; the artifact is written on first commit.
3. **Policy `id` in download** — the real Ranger engine NPEs on `RangerPolicy.getId()==null`; the download must inject the DB-assigned `id`/`version`/`isEnabled`.
4. **Anonymous `/service`** — stock Ranger plugins authenticate out-of-band; an empty `ranger.auth_token` opens `/service` for them. Set a token to require bearer.
5. **Networking** — one shared host IP keeps Egret's SSRF host-check, Trino's S3 endpoint, and the signed URLs consistent.
