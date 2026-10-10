# Wiring an engine to egret's embedded catalog

egret embeds the Iceberg REST catalog (lakekeeper) in-process and issues machine
credentials from its own mini-IdP. An engine authenticates with a **Catalog
Client** (client id + secret) — no Polaris, no external IdP, no proxy.

Both the catalog `uri` **and** the OAuth2 `server-uri` point at egret. The
credential is a Catalog Client, created under a warehouse's role in
**Control Plane → Warehouses → {warehouse} → Catalog Clients** (the secret is
shown once).

egret runs on the host at `localhost:8080`; containers reach it via
`host.docker.internal:8080`.

## Provision (once)

Control Plane → Warehouses:
1. **Add warehouse** (`lakehouse`) over a storage connection (S3/RustFS). This
   provisions the lakekeeper warehouse.
2. Open it → **Roles & grants** → add a role (`engine`) → add a `write` grant
   (`write`/`manage` permit writes; any grant permits reads).
3. **Catalog Clients** → add a client bound to that role → copy the
   `clientId` / `clientSecret` (shown once).

## Trino (`deploy/trino/catalog/iceberg.properties`, gitignored)

```properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://host.docker.internal:8080/catalog          # egret embedded catalog
iceberg.rest-catalog.warehouse=lakehouse
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<clientId>:<clientSecret>           # a Catalog Client
iceberg.rest-catalog.oauth2.server-uri=http://host.docker.internal:8080/oauth/token   # egret mini-IdP
iceberg.rest-catalog.oauth2.scope=catalog
# Direct S3 access (the embedded mount does not expose remote request-signing).
fs.native-s3.enabled=true
s3.endpoint=http://rustfs:9000
s3.region=us-east-1
s3.path-style-access=true
s3.aws-access-key=egret
s3.aws-secret-key=egretsecret
```

Restart Trino; the `iceberg` catalog loads. `CREATE TABLE lakehouse.demo.t AS
SELECT 42 n` then `SELECT n FROM lakehouse.demo.t` → `42`.

## pyiceberg (verified)

```python
from pyiceberg.catalog.rest import RestCatalog
import pyarrow as pa

cat = RestCatalog("egret",
    uri="http://localhost:8080/catalog",
    warehouse="lakehouse",
    credential="<clientId>:<clientSecret>",
    **{"oauth2-server-uri": "http://localhost:8080/oauth/token", "scope": "catalog",
       "s3.endpoint": "http://localhost:9000", "s3.access-key-id": "egret",
       "s3.secret-access-key": "egretsecret", "s3.region": "us-east-1",
       "s3.path-style-access": "true"})
cat.create_namespace_if_not_exists("demo")
t = cat.create_table_if_not_exists("demo.t", schema=pa.schema([("n", pa.int64())]))
t.append(pa.table({"n": [1, 2, 3]}))
print(t.scan().to_arrow().num_rows)   # 3 — data lands in s3://<bucket>/... on RustFS
```

## Notes

- **Access delegation**: warehouses are provisioned with **STS enabled**
  (remote-signing stays off — the embedded mount exposes no signer endpoint). A
  client that sends `X-Iceberg-Access-Delegation: vended-credentials` gets
  **short-lived S3 credentials downscoped to just the table it loaded** (lakekeeper
  calls AssumeRole on the S3 endpoint; RustFS needs no role-arn). Enable it with
  `iceberg.rest-catalog.vended-credentials-enabled=true` (Trino),
  `"iceberg.catalog.vended-credentials-enabled" = "true"` (StarRocks 4.0+), or
  `"header.X-Iceberg-Access-Delegation": "vended-credentials"` (pyiceberg) — then the
  static `s3.*` keys below are no longer required. Clients that don't request
  delegation still fall back to their own access-key credential.
- **Advertised `uri`**: lakekeeper hardcodes `http://localhost:8181` in `/config`;
  egret rewrites it to the request's `Host`, so both host and container clients
  get a reachable catalog uri.
- **Storage overlap**: lakekeeper rejects a warehouse whose storage location
  overlaps an existing one. A warehouse with no `key_prefix` claims the whole
  bucket — give each warehouse its own bucket or `key_prefix`.
