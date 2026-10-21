# Manual Integration Test — Trino × Egret Iceberg Catalog × Ranger

> **Status:** Manual pre-push gate (NOT run in CI) · **Date:** 2026-06-18
> Canonical (English). Vietnamese reference: [`docs/vi/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../../vi/testing/2026-06-18-trino-iceberg-ranger-manual-test.md).
> Run this before pushing changes that touch `egret-catalog`, `egret-storage`, `egret-authz`, or `egret-ranger`. It exercises the engine-facing contracts (Iceberg REST + Ranger policy download) against a **real Trino**, which surfaces gaps unit tests cannot (reserved properties, empty-location guard, policy `id`, plugin auth).

## What it proves

A real **Trino 481** uses Egret for BOTH governance-core features at once:
- **Iceberg REST catalog** — `CREATE SCHEMA / CREATE TABLE / INSERT / SELECT` flow data through Egret's `/catalog` into RustFS.
- **Apache Ranger access control** — Trino's built-in `ranger` plugin downloads policies from Egret's `/service` and enforces **deny / row-filter / column-mask** on live queries.

## Why it is not in CI

Needs Docker + a JVM-heavy Trino image + RustFS; the Trino Ranger plugin polls on a timer. Too slow/heavy for CI. The unit + DB-gated + `RangerPolicyEngineImpl` gate tests cover the logic in CI; this fills the **engine-contract** gap manually.

## Prerequisites

- Docker running.
- Repo built: `cargo build -p egret-bin`.
- A Postgres reachable at `postgres://egret:egret@localhost:5432/egret` (the dev `portal-pg` or `docker compose -f deploy/docker-compose.dev.yml up -d postgres`).
- Free host ports: 8080 (egret), 9000/9001 (RustFS), 8090 (Trino).

## Networking note (important)

Egret runs on the host; Trino + RustFS run in containers. Egret's signer has an SSRF host-check, and Trino's S3 endpoint must match what Egret signs/writes. Use the host's **LAN IP** as the single address everyone shares:

```bash
export HOST_IP=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)   # macOS; Linux: hostname -I | awk '{print $1}'
echo "HOST_IP=$HOST_IP"
```

---

## 1. Start RustFS + create the warehouse bucket

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d rustfs
# wait for ready
until [ "$(curl -s -o /dev/null -w '%{http_code}' http://localhost:9000/health)" = 200 ]; do sleep 2; done
NET=egret-dev_default
docker run --rm --network "$NET" -e AWS_ACCESS_KEY_ID=egret -e AWS_SECRET_ACCESS_KEY=egretsecret \
  -e AWS_DEFAULT_REGION=us-east-1 amazon/aws-cli:2.37.9 --endpoint-url http://rustfs:9000 s3 mb s3://warehouse
```

## 2. Run Egret with catalog + ranger enabled

`ranger.auth_mode = "anonymous"` lets the hand-written plugin XML below poll `/ranger` without a credential. Use it only on this throwaway stack: in anonymous mode anyone who can reach egret can download every profile's policies. The default, `required`, makes every plugin present its engine's own credential ([`docs/en/guides/ranger-surface.md`](../guides/ranger-surface.md)); `auth_token = ""` accepts no operator bearer.

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
auth_mode = "anonymous"
EOF

BIN=$(pwd)/../target/debug/egret
(cd /tmp/egret-itest && "$BIN" migrate && nohup "$BIN" serve > /tmp/egret-itest/server.log 2>&1 &)
sleep 3
curl -s -o /dev/null -w "egret healthz: %{http_code}\n" http://$HOST_IP:8080/healthz
curl -s -H "Authorization: Bearer devtoken" "http://$HOST_IP:8080/catalog/v1/config?warehouse=<WS_SLUG>" | head -c 200; echo
curl -s -o /dev/null -w "anon /ranger: %{http_code}\n" http://$HOST_IP:8080/ranger/service/plugins/definitions
```
Expect: `healthz 200`, a `/v1/config` JSON whose `overrides` include `prefix=<WS_SLUG>` + `warehouse` + `s3.signer.uri` (omitting `?warehouse` returns signer keys only, no prefix), `anon /ranger: 200` (`401` means egret is not in anonymous mode).

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
  <property><name>ranger.plugin.trino.policy.rest.url</name><value>http://$HOST_IP:8080/ranger</value></property>
  <property><name>ranger.plugin.trino.policy.pollIntervalMs</name><value>5000</value></property>
  <property><name>ranger.plugin.trino.policy.cache.dir</name><value>/tmp/ranger-trino-cache</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.connection.timeoutMs</name><value>10000</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.read.timeoutMs</name><value>10000</value></property>
</configuration>
EOF
```
The Ranger `RangerAdminRESTClient` downloads from `<policy.rest.url>/service/plugins/policies/download/<service.name>` — i.e. Egret's `/ranger/service`. Trino 481 config keys: `ranger.service.name`, `ranger.plugin.config.resource`.

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
Expect: `CREATE SCHEMA`, `CREATE TABLE`, `INSERT: 4 rows`, and `US 2 29.5 / VN 2 56.2`. Verify the files landed in RustFS under `warehouse/demo/report-*/` (`data/*.parquet`, `metadata/0000N-*.metadata.json` — note there is **no** `00000`; it is written on the first commit, not on create, which is what lets Trino's empty-location guard pass).

## 5. Ranger test (policy enforcement on live queries)

The Trino catalog name is `iceberg`, so Ranger resources use `catalog=iceberg`. Default-deny means a user needs `execute` on `queryid` and `impersonate` on `trinouser` to run any query, plus `select` on the table.

```bash
P() { curl -s -o /dev/null -w "  %{http_code}\n" -X POST "http://$HOST_IP:8080/ranger/service/plugins/policies" -H "Content-Type: application/json" -d "$1"; }
# the service the plugin polls
curl -s -X POST "http://$HOST_IP:8080/ranger/service/plugins/services" -H "Content-Type: application/json" -d '{"name":"trino_prod","type":"trino","configs":{}}' >/dev/null
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
cd deploy && docker compose -f docker-compose.dev.yml rm -fs trino rustfs
rm -rf /tmp/egret-itest /tmp/ac.properties /tmp/ranger-trino-security.xml
rm -f trino/catalog/iceberg.properties   # host-specific; never commit
```

## Contract notes (what this test guards)

1. **Reserved table properties** — Trino sends `format-version` in `CREATE TABLE`; Egret must strip `iceberg`'s `RESERVED_PROPERTIES` before building metadata, or it 400s.
2. **Empty-location guard** — Egret must NOT pre-write `metadata.json` on create; Trino verifies the table location is empty. The DB jsonb is the source of truth; the artifact is written on first commit.
3. **Policy `id` in download** — the real Ranger engine NPEs on `RangerPolicy.getId()==null`; the download must inject the DB-assigned `id`/`version`/`isEnabled`.
4. **Anonymous `/ranger`, for this test only** — the hand-written plugin XML carries no credential, so this test runs `ranger.auth_mode = "anonymous"`. With the default `required`, a plugin must present its engine's credential (`ranger.plugin.trino.policy.rest.client.username` / `.password`, plugin 2.8.0 or later) or it gets 401.
5. **Networking** — one shared host IP keeps Egret's SSRF host-check, Trino's S3 endpoint, and the signed URLs consistent.

---

## 7. The evaluator check (SP4d-1) — predict first, then query

> **Since the Ranger surface requires credentials (2026-10-20):** the egret on kind runs `ranger.authMode=required` by default, and the hand-made pods below carry no credential, so their plugins (and the in-pod `curl` of the download) get 401. For this check either deploy egret with `--set ranger.authMode=anonymous` (a throwaway cluster only), or register each hand-made engine as an external engine bound to its profile and add `ranger.plugin.<trino|starrocks>.policy.rest.client.username` / `.password` with the credential egret shows once (then `curl -u 'engine-<id>:<password>'` the `/ranger/service/plugins/secure/policies/download/<profile>` path). See [`docs/en/guides/ranger-surface.md`](../guides/ranger-surface.md).

Added 2026-09-28. Sections 1-6 prove that **Egret's policies reach a real Trino and are
enforced**. This section proves something else: that `egret-ranger-eval`, the Rust evaluator
that answers "would this be allowed?" without an engine, **agrees with the plugin that actually
answers it**. The differential gate in `crates/egret-ranger-eval/tests/differential.rs` compares
the crate against the real Java `RangerPolicyEngineImpl`, but that harness builds its own
requests — it cannot see how Trino names resources, at what granularity it asks, or which access
type it sends. This check covers that.

Run it after any change to `egret-ranger-eval`, to `crates/egret-policy/src/service-defs/*.json`,
or to the policy download route. The measured baseline is
[`docs/en/plans/2026-09-28-sp4d-1-live-result.md`](../plans/2026-09-28-sp4d-1-live-result.md).

### 7.1 The rule

**Compute the prediction, write it down, and only then run the query.** A prediction produced
after the result proves nothing. And **a disagreement is a finding**: either the evaluator, the
harness, or this repo's reading of Ranger is wrong. Work out which. Never adjust a case into
agreement.

The check covers **ten cases**: eight on a private Trino coordinator (§7.4) and two on a private
StarRocks FE + CN (§7.4b), because StarRocks runs a different plugin version against a different
service-def and its result cannot be inferred from Trino's.

### 7.2 The instrument: a private coordinator, so nothing shared is touched

Two properties are needed that a provisioned engine does not have, and neither is worth changing
a shared engine for:

- a Ranger profile with **no bound tag service** — `download.rs` embeds a `tagPolicies` block
  whenever the service's `config_jsonb` carries a `tagService`, and the evaluator reports any
  non-null `tagPolicies` as `Undecidable { EnvelopeConstruct(TagPolicies) }`, so an auto-bound
  profile yields no prediction at all;
- a profile with `origin = 'manual'`, because `POST /api/control-plane/policies` refuses to write
  to a `generated` one (409, "compiled by the SP4a permission model").

```bash
# a manual profile: nothing is seeded onto it, so every policy the plugin sees is yours
curl -s -b jar -X POST http://egret.localtest.me/api/control-plane/policies/services \
  -H 'Content-Type: application/json' -H "X-Egret-CSRF: $CSRF" \
  -d '{"name":"sp4d1-gate","type":"trino","config":{}}'
```

Then a coordinator of your own. HTTP with no authentication, so `trino --user <anything>` picks
the subject directly; the Iceberg catalog properties are copied verbatim from the provisioned
engine's catalogs ConfigMap, and `envFrom` reuses its secret for `EGRET_C1` rather than copying
the value.

```yaml
apiVersion: v1
kind: ConfigMap
metadata: {name: sp4d1-gate-trino, namespace: egret-engines}
data:
  node.properties: |
    node.environment=sp4d1gate
    node.data-dir=/data/trino
  config.properties: |
    coordinator=true
    node-scheduler.include-coordinator=true
    discovery.uri=http://localhost:8080
    http-server.http.port=8080
    access-control.config-files=/etc/trino/access-control.properties
  jvm.config: |
    -server
    -Xmx1G
    -Xms1G
    -XX:+UseG1GC
    -XX:+ExitOnOutOfMemoryError
  access-control.properties: |
    access-control.name=ranger
    ranger.service.name=sp4d1-gate
    ranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml
  ranger-trino-security.xml: |
    <?xml version="1.0" encoding="UTF-8"?>
    <configuration>
    <property><name>ranger.plugin.trino.service.name</name><value>sp4d1-gate</value></property>
    <property><name>ranger.plugin.trino.policy.rest.url</name><value>http://egret.egret-system.svc.cluster.local:8080/ranger</value></property>
    <property><name>ranger.plugin.trino.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>
    <property><name>ranger.plugin.trino.policy.cache.dir</name><value>/tmp/ranger/sp4d1-gate/policycache</value></property>
    <property><name>ranger.plugin.trino.policy.pollIntervalMs</name><value>10000</value></property>
    <property><name>ranger.plugin.trino.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>
    <property><name>ranger.plugin.trino.policy.rest.client.read.timeoutMs</name><value>30000</value></property>
    </configuration>
  sp4a10_wh.properties: |
    # copy verbatim from `kubectl get cm <engine>-catalogs -o jsonpath='{.data.<wh>\.properties}'`
---
apiVersion: v1
kind: Pod
metadata: {name: sp4d1-gate-trino, namespace: egret-engines}
spec:
  restartPolicy: Never
  containers:
  - name: trino
    image: trinodb/trino:470          # the image whose plugin version Task 1 measured
    imagePullPolicy: IfNotPresent
    envFrom: [{secretRef: {name: sp3c2-trino-live-env}}]   # for ${ENV:EGRET_C1}
    resources: {requests: {cpu: "500m", memory: "2Gi"}, limits: {cpu: "2", memory: "3Gi"}}
    volumeMounts:
    - {mountPath: /etc/trino/node.properties,             name: cfg, subPath: node.properties}
    - {mountPath: /etc/trino/config.properties,           name: cfg, subPath: config.properties}
    - {mountPath: /etc/trino/jvm.config,                  name: cfg, subPath: jvm.config}
    - {mountPath: /etc/trino/access-control.properties,   name: cfg, subPath: access-control.properties}
    - {mountPath: /etc/trino/ranger-trino-security.xml,   name: cfg, subPath: ranger-trino-security.xml}
    - {mountPath: /etc/trino/catalog/sp4a10_wh.properties, name: cfg, subPath: sp4a10_wh.properties}
    - {mountPath: /data/trino, name: data}
  volumes:
  - {name: cfg,  configMap: {name: sp4d1-gate-trino}}
  - {name: data, emptyDir: {}}
```

Write the baseline policies before creating the pod: `execute` on `queryid`, `impersonate` on
`trinouser`, `alter` on `systemproperty` and on `catalog`+`sessionproperty`, `execute`/`grant` on
`function`, `read_sysinfo` on `sysinfo`, `use`/`show` on `catalog: "*"` — and **no `select` on
anything**, so every allow in a case comes from that case's own policy.

### 7.3 The three steps, in this order

```bash
POD=sp4d1-gate-trino

# 1. capture the download the plugin polls, and PREDICT from it
kubectl --context kind-egret exec -n egret-engines $POD -- \
  curl -s http://egret.egret-system.svc.cluster.local:8080/ranger/service/plugins/policies/download/sp4d1-gate \
  > /tmp/dl.json
cargo run -p egret-ranger-eval --example live_gate -- /tmp/dl.json trino admin sp4a10_wh | tee /tmp/prediction.txt

# 2. confirm the plugin is holding THAT policyVersion — not the previous one
kubectl --context kind-egret exec -n egret-engines $POD -- \
  head -c 110 /tmp/ranger/sp4d1-gate/policycache/trino_sp4d1-gate.json
#   → {"serviceName":"sp4d1-gate","serviceId":56,"policyVersion":17,…}   must equal /tmp/dl.json's

# 3. only now, the query
kubectl --context kind-egret exec -n egret-engines $POD -- \
  trino --server localhost:8080 --user admin --execute "SELECT id, item FROM sp4a10_wh.sp4d1a.t1"
```

Step 2 is not optional. A provisioned coordinator polls every **30 s**
(`ranger.plugin.trino.policy.pollIntervalMs = 30000`); query before the poll lands and you record
the previous policy state as if it were a result.

`examples/live_gate.rs` carries the eight-case list as built-in defaults and also takes ad-hoc
`schema/table[/column]` arguments, which is how the StarRocks side is driven:

```bash
cargo run -p egret-ranger-eval --example live_gate -- \
  /tmp/sr.json starrocks analyst sp4a10_wh sp4a/report/item sp4a/report sp4a/secret/note
```

### 7.4 What the eight cases are for

Each is chosen to exercise **request construction**, not policy logic — that is what the Java
harness cannot see.

| # | policy | query | expected |
|---|---|---|---|
| 1 | `{catalog, schema}`, `select` | `SELECT id, item FROM <cat>.<schema>.t1` | **denied** — an ANCESTOR match is not an allow |
| 2 | the same + `table` + `column: ["*"]` | same on `t2` | 4 rows |
| 3 | 2 + a `policyType 1` mask on column `item` | same on `t3` | `item` = `xxxx` |
| 4 | 2 + a `policyType 2` filter `country='US'`, resource **without `column`** | `count(*)` on `t4` | `2` |
| 5a | allow + deny + `denyExceptions`, same user | on `t5a` | 4 rows |
| 5b | allow + `allowExceptions`, same user | on `t5b` | denied |
| 6 | `groups: ["public"]`, no users | on `t6` | 4 rows |
| 8 | **no policy at all** | on `t7` | denied |

1 and 2 together are the load-bearing pair: if both allow, the measurement is not sensitive and
proves nothing.

**Case 4 must be predicted at TABLE granularity** — `evaluate_row_filter` with a request carrying
no `column`. Both engines' `rowFilterDef.resources` stop at `table`, so a column-granular request
makes every row-filter policy an ANCESTOR match and returns `NoFilter`, which reads as "row
filters do not work". Task 6 lost six tests to this before spotting it.

### 7.4b The StarRocks side: a private FE + CN

StarRocks needs its own instrument for the same two reasons Trino did — the profile a provisioned
FE polls is `generated` (the policy API refuses to write to it) — plus one more: repointing a
running FE means editing `ranger-starrocks-security.xml` inside it and restarting, and that FE's
metadata is on an `emptyDir`. Build your own instead. It is worth it: StarRocks runs a different
plugin version (2.8.0) against a different service-def whose middle level is `database`, so a
Trino-only gate leaves the evaluator's StarRocks claim untested.

**Check capacity first and be willing to stop.** A NOT MEASURED with a capacity reason is a fine
outcome; a wedged cluster is not.

```bash
df -h /                                              # host disk
kubectl --context kind-egret describe node | sed -n '/Allocated resources/,/Events/p'
# images must already be on the node — a multi-GB pull is what would hurt
kubectl --context kind-egret get nodes -o json \
  | python3 -c 'import json,sys;[print(n) for i in json.load(sys.stdin)["items"][0]["status"]["images"] for n in (i.get("names") or []) if "starrocks" in n]'
```

Create the profile the same way (`POST /api/control-plane/policies/services`, type `starrocks`),
then:

```yaml
# Secret sp4d1-sr-gate-pw  { password: <choose one> }   — the FE root password AND the CN's MYSQL_PWD
# ConfigMap sp4d1-sr-gate-fe-cm:
  fe.conf: |
    JAVA_OPTS = "-Dlog4j2.formatMsgNoLookups=true -Xmx2048m -XX:+UseG1GC"
    http_port = 8030
    rpc_port = 9020
    query_port = 9030
    edit_log_port = 9010
    run_mode = shared_data
    cloud_native_meta_port = 6090
    cloud_native_storage_type = S3
    aws_s3_endpoint = http://rustfs.egret-system.svc.cluster.local:9000   # NOT http://rustfs:9000
    aws_s3_path = e2e/starrocks/sp4d1-sr-gate
    aws_s3_access_key = egret
    aws_s3_secret_key = egretsecret
    aws_s3_use_aws_sdk_default_behavior = false
    aws_s3_enable_path_style_access = true
    enable_load_volume_from_conf = true
    access_control = ranger
# ConfigMap sp4d1-sr-gate-ranger-cm:
  ranger-starrocks-security.xml: |
    <configuration>
    <property><name>ranger.plugin.starrocks.service.name</name><value>sp4d1-sr-gate</value></property>
    <property><name>ranger.plugin.starrocks.policy.rest.url</name><value>http://egret.egret-system.svc.cluster.local:8080/ranger</value></property>
    <property><name>ranger.plugin.starrocks.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>
    <property><name>ranger.plugin.starrocks.policy.cache.dir</name><value>/tmp/ranger/sp4d1-sr-gate/policycache</value></property>
    <property><name>ranger.plugin.starrocks.policy.pollIntervalMs</name><value>30000</value></property>
    </configuration>
  ranger-starrocks-audit.xml: |
    <configuration>
    <property><name>xasecure.audit.is.enabled</name><value>false</value></property>
    </configuration>
---
# StatefulSet sp4d1-sr-gate-fe  (1 replica, requests 500m/2Gi, limits 2/3Gi)
#   image starrocks/fe-ubuntu:4.1-latest, imagePullPolicy IfNotPresent
#   command ["/opt/starrocks/fe_entrypoint.sh"], args ["$(FE_SERVICE_NAME)"]
#   env  MYSQL_PWD (from the Secret), POD_NAME/POD_IP/HOST_IP/POD_NAMESPACE (fieldRef),
#        HOST_TYPE=FQDN, COMPONENT_NAME=fe,
#        FE_SERVICE_NAME=sp4d1-sr-gate-fe-service.egret-engines,
#        CONFIGMAP_MOUNT_PATH=/etc/starrocks/fe/conf
#   startup/readiness probe  GET :8030/api/health   (startup failureThreshold 60, period 5s)
#   mounts  /opt/starrocks/fe/meta (emptyDir), /opt/starrocks/fe/log (emptyDir),
#           /etc/starrocks/fe/conf (fe-cm),
#           /opt/starrocks/fe/conf/ranger-starrocks-{security,audit}.xml (ranger-cm, subPath)
#   NOTE: omit the ssl_* keys the provisioned FE carries, and you can skip its keystore init container.
# Services  sp4d1-sr-gate-fe-service (8030/9020/9030/9010)
#           sp4d1-sr-gate-fe-search  (headless, publishNotReadyAddresses, 9030)
---
# StatefulSet sp4d1-sr-gate-cn  (1 replica, requests 500m/2Gi, limits 2/3Gi)
#   image starrocks/cn-ubuntu:4.1-latest
#   command ["/opt/starrocks/cn_entrypoint.sh"], args ["$(FE_SERVICE_NAME)"]
#   env  MYSQL_PWD (same Secret), the four fieldRefs, HOST_TYPE=FQDN, COMPONENT_NAME=cn,
#        FE_SERVICE_NAME=sp4d1-sr-gate-fe-service, FE_QUERY_PORT=9030
#   ports 9060/8040/9050/8060; mount /opt/starrocks/cn/log (emptyDir)
# Service  sp4d1-sr-gate-cn-search (headless, publishNotReadyAddresses, 9050)
```

Three things will bite, in this order:

1. **The CN cannot register** — `ERROR 1045 (28000): Access denied for user 'root'` in a retry
   loop. A fresh FE has an *empty* root password while the CN's entrypoint authenticates with
   `MYSQL_PWD`. Fix on your own FE, once it is ready:
   ```bash
   mysql -h127.0.0.1 -P9030 -uroot --skip-password \
     -e "SET PASSWORD FOR 'root' = PASSWORD('<the Secret value>')"
   ```
   Then wait for `SHOW COMPUTE NODES\G` → `Alive: true`.
2. **The first query takes ~150 s.** Not Ranger. `shared_data` with one CN makes the FE retry
   creating its `_statistics_` tables at three replicas forever and the CN's agent queue starves
   the scan (`create table failed … elapsed: 160000 ms`). On your own FE:
   ```sql
   ADMIN SET FRONTEND CONFIG ("default_replication_num" = "1");
   ADMIN SET FRONTEND CONFIG ("enable_statistic_collect" = "false");
   ```
   The same read then returns in well under a second.
3. **The catalog.** Copy the Iceberg-REST properties from a provisioned FE — including the OAuth2
   credential, which is why they are not printed here:
   ```sql
   -- on the provisioned FE, as root:
   SHOW CREATE CATALOG sp4a10_wh\G
   -- then on yours, the same PROPERTIES, with
   --   aws.s3.endpoint = http://rustfs.egret-system.svc.cluster.local:9000
   ```

**The subject must be a native user, never `root`.** `root` bypasses the access check before
Ranger is consulted (`RangerAccessController.java:115-117`), so a `root` result proves nothing
about a deny. `CREATE USER 'sp4dsr' IDENTIFIED BY '<pw>'` on your own FE, use it for every
measurement, and keep `root` for DDL and for the data-path control. Confirm with
`SELECT current_user()`.

The two cases, with StarRocks' `database` where Trino says `schema`:

| # | policy | query | expected |
|---|---|---|---|
| S1 | `{catalog, database}`, `select` | `SELECT id, item FROM <cat>.sp4d1a.t1` | **denied** |
| S2 | the same + `table` + `column: ["*"]` | same on `sp4d1.t2` | 4 rows |

plus a baseline granting `usage` on the catalog and **no `select` on anything**. As on Trino,
S1 and S2 together are what make the measurement sensitive; if both deny, the setup is wrong.

### 7.5 Clean-up

Label every object you create with `egret/sp4d1-live-gate: "true"` and the whole instrument comes
down in one command:

```bash
kubectl --context kind-egret delete pod sp4d1-gate-trino -n egret-engines
kubectl --context kind-egret delete cm  sp4d1-gate-trino -n egret-engines
kubectl --context kind-egret delete sts,svc,cm,secret -n egret-engines -l egret/sp4d1-live-gate=true
# confirm, do not assume:
kubectl --context kind-egret get all,cm,secret -n egret-engines -l egret/sp4d1-live-gate=true
```

Keep the profiles and the fixture tables — they are additive, enforced by nothing once the pods
are gone, and they are what makes the check cheap to re-run. Both private engines read the same
Iceberg warehouse, which is what makes the Trino and StarRocks results comparable.

The private StarRocks also leaves a shared-data volume behind in RustFS, which the `kubectl delete`
does not touch. Remove it — but **list before you remove**, and scope to the exact prefix:

```bash
# The RustFS image ships no S3 client: port-forward it and use the AWS CLI from the host.
kubectl --context kind-egret port-forward -n egret-system svc/rustfs 9000:9000 &
export AWS_ACCESS_KEY_ID=egret AWS_SECRET_ACCESS_KEY=egretsecret AWS_DEFAULT_REGION=us-east-1
S3="aws --endpoint-url http://localhost:9000 s3"
$S3 ls s3://e2e/starrocks/                                        # the siblings that must survive
$S3 ls s3://e2e/starrocks/sp4d1-sr-gate/ --recursive --summarize  # the set you are about to delete
$S3 rm s3://e2e/starrocks/sp4d1-sr-gate/ --recursive              # exact prefix, trailing slash, no glob
$S3 ls s3://e2e/starrocks/                                        # confirm only yours went
```

Delete against a set you have **seen**, never against a pattern you believe. This repo has a
recorded incident where a cleanup helper's `LIKE 'egret_t_%'` matched `egret_test` and dropped a
live database; the shape to avoid is a pattern that looks specific and is not. A prefix with no
trailing slash and a glob are both that shape. Check the sibling object counts before and after —
the bucket total should fall by exactly the number you listed.

### 7.6 Contract notes this section adds

6. **An ANCESTOR match is not an allow.** A policy naming fewer hierarchy levels than the request
   grants nothing. Both the evaluator and a real Trino agree, and SP4a's T1 defect was exactly
   this shape reaching production from the compiler.
7. **A tag service makes the evaluator silent.** Any non-null `tagPolicies` in the download is
   `Undecidable { EnvelopeConstruct(TagPolicies) }`. Trino auto-bind creates a `<profile>_tag`
   service alongside the profile, so every profile it creates is affected — but only while that
   tag service still **exists**: `download.rs`'s lookup is best-effort, and a profile whose named
   tag service has been deleted (in this cluster, `demo-trino`) gets no `tagPolicies` block and
   decides normally. Check the download, not the `config_jsonb`. Predict on a profile without a
   tag service, or expect no answer.
8. **Never strip the input to get an answer.** If the evaluator says `Undecidable`, that is the
   result. Editing the captured download until it decides is the same error as editing a test
   until it passes.
