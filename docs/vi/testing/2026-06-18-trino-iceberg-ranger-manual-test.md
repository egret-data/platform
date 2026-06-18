# Test tích hợp thủ công — Trino × Egret Iceberg Catalog × Ranger

> **Trạng thái:** Gate thủ công trước khi push (KHÔNG chạy trong CI) · **Ngày:** 2026-06-18
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../../en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Chạy bài này trước khi push thay đổi chạm `egret-catalog`, `egret-storage`, `egret-authz`, hoặc `egret-ranger`. Nó kiểm các hợp đồng phía engine (Iceberg REST + tải policy Ranger) với **Trino thật**, lộ ra lỗ hổng mà unit test không thấy (reserved properties, guard empty-location, policy `id`, auth plugin).

## Bài test chứng minh gì

Một **Trino 481** thật dùng Egret cho CẢ HAI tính năng lõi-governance cùng lúc:
- **Iceberg REST catalog** — `CREATE SCHEMA / CREATE TABLE / INSERT / SELECT` đẩy data qua `/catalog` của Egret vào MinIO.
- **Apache Ranger access control** — plugin `ranger` tích hợp sẵn của Trino tải policy từ `/service` của Egret và enforce **deny / row-filter / column-mask** trên query thật.

## Vì sao không nằm trong CI

Cần Docker + image Trino nặng JVM + MinIO; plugin Ranger của Trino poll theo timer. Quá chậm/nặng cho CI. Các test unit + gate-DB + `RangerPolicyEngineImpl` đã phủ logic trong CI; bài này lấp khoảng **hợp đồng-engine** một cách thủ công.

## Điều kiện tiên quyết

- Docker đang chạy.
- Repo đã build: `cargo build -p egret-bin`.
- Postgres truy cập được ở `postgres://egret:egret@localhost:5432/egret` (`portal-pg` dev hoặc `docker compose -f deploy/docker-compose.dev.yml up -d postgres`).
- Cổng host trống: 8080 (egret), 9000/9001 (MinIO), 8090 (Trino).

## Lưu ý mạng (quan trọng)

Egret chạy trên host; Trino + MinIO chạy trong container. Signer của Egret có kiểm SSRF host, và S3 endpoint của Trino phải khớp cái Egret ký/ghi. Dùng **IP LAN** của host làm địa chỉ chung duy nhất:

```bash
export HOST_IP=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)   # macOS; Linux: hostname -I | awk '{print $1}'
echo "HOST_IP=$HOST_IP"
```

---

## 1. Khởi động MinIO + tạo bucket warehouse

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d minio
until [ "$(curl -s -o /dev/null -w '%{http_code}' http://localhost:9000/minio/health/ready)" = 200 ]; do sleep 2; done
NET=egret-dev_default
docker run --rm --network "$NET" --entrypoint sh minio/mc -c \
  'mc alias set m http://minio:9000 egret egretsecret && mc mb -p m/warehouse'
```

## 2. Chạy Egret bật catalog + ranger

`ranger.auth_token = ""` chọn **`/service` ẩn danh** — plugin Ranger gốc của Trino xác thực ngoài-luồng (không qua bearer), nên không dùng được endpoint bảo vệ bằng token.

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
curl -s -H "Authorization: Bearer devtoken" http://$HOST_IP:8080/catalog/v1/config | head -c 200; echo
curl -s -o /dev/null -w "anon /service: %{http_code}\n" http://$HOST_IP:8080/service/plugins/definitions
```
Kỳ vọng: `healthz 200`, JSON `/v1/config` có `warehouse` + `s3.signer.uri`, `anon /service: 200`.

## 3. Cấu hình + khởi động Trino

### 3a. Iceberg catalog → Egret (`deploy/trino/catalog/iceberg.properties`)

```bash
cat > trino/catalog/iceberg.properties <<EOF
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=http://$HOST_IP:8080/catalog
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

### 3b. Ranger access control (copy vào thư mục config trong container)

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
`RangerAdminRESTClient` tải từ `<policy.rest.url>/service/plugins/policies/download/<service.name>` — tức `/service` của Egret. Key config Trino 481: `ranger.service.name`, `ranger.plugin.config.resource`.

### 3c. Khởi động Trino, đẩy config ranger, restart

```bash
docker compose -f docker-compose.dev.yml up -d trino
until [ "$(curl -s http://localhost:8090/v1/info | python3 -c 'import sys,json;print(json.load(sys.stdin)["starting"])' 2>/dev/null)" = False ]; do sleep 3; done
docker cp /tmp/ac.properties egret-dev-trino-1:/etc/trino/access-control.properties
docker cp /tmp/ranger-trino-security.xml egret-dev-trino-1:/etc/trino/ranger-trino-security.xml
docker restart egret-dev-trino-1
until [ "$(curl -s http://localhost:8090/v1/info | python3 -c 'import sys,json;print(json.load(sys.stdin)["starting"])' 2>/dev/null)" = False ]; do sleep 3; done
```

## 4. Test catalog (đọc-ghi qua Egret)

```bash
TX() { docker exec egret-dev-trino-1 trino --server localhost:8080 --user admin --execute "$1"; }
TX "CREATE SCHEMA IF NOT EXISTS iceberg.demo"
TX "CREATE TABLE iceberg.demo.report (id bigint, item varchar, country varchar, amount double)"
TX "INSERT INTO iceberg.demo.report VALUES (1,'book','US',9.5),(2,'pen','VN',1.2),(3,'lamp','US',20.0),(4,'desk','VN',55.0)"
TX "SELECT country, count(*) n, sum(amount) total FROM iceberg.demo.report GROUP BY country ORDER BY country"
```
Kỳ vọng: `CREATE SCHEMA`, `CREATE TABLE`, `INSERT: 4 rows`, và `US 2 29.5 / VN 2 56.2`. Kiểm file trong MinIO dưới `warehouse/demo/report-*/` (`data/*.parquet`, `metadata/0000N-*.metadata.json` — lưu ý KHÔNG có `00000`; nó được ghi ở commit đầu tiên, không phải lúc create, nhờ vậy qua được guard empty-location của Trino).

## 5. Test Ranger (enforce policy trên query thật)

Tên catalog trong Trino là `iceberg`, nên resource Ranger dùng `catalog=iceberg`. Default-deny nghĩa là user cần `execute` trên `queryid` và `impersonate` trên `trinouser` để chạy query, cộng `select` trên bảng.

```bash
P() { curl -s -o /dev/null -w "  %{http_code}\n" -X POST "http://$HOST_IP:8080/service/plugins/policies" -H "Content-Type: application/json" -d "$1"; }
curl -s -X POST "http://$HOST_IP:8080/service/plugins/services" -H "Content-Type: application/json" -d '{"name":"trino_prod","type":"trino","configs":{}}' >/dev/null
# toàn cục: ai cũng được chạy query + set session user của mình
P '{"service":"trino_prod","name":"g-execquery","policyType":0,"isEnabled":true,"resources":{"queryid":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"execute","isAllowed":true}],"groups":["public"]}]}'
P '{"service":"trino_prod","name":"g-impersonate","policyType":0,"isEnabled":true,"resources":{"trinouser":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"impersonate","isAllowed":true}],"groups":["public"]}]}'
# quyền bảng: alice + admin được select report
P '{"service":"trino_prod","name":"ice-access","policyType":0,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]},"column":{"values":["*"]}},"policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice","admin"]}]}'
# mask cột: alice thấy `item` bị che
P '{"service":"trino_prod","name":"ice-mask","policyType":1,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]},"column":{"values":["item"]}},"dataMaskPolicyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"],"dataMaskInfo":{"dataMaskType":"MASK"}}]}'
# row filter: alice chỉ thấy dòng US
P '{"service":"trino_prod","name":"ice-rowfilter","policyType":2,"isEnabled":true,"resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["demo"]},"table":{"values":["report"]}},"rowFilterPolicyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"],"rowFilterInfo":{"filterExpr":"country = '"'"'US'"'"'"}}]}'

sleep 8   # chờ plugin poll (pollIntervalMs=5000)
Q="SELECT * FROM iceberg.demo.report ORDER BY id"
echo "--- admin ---"; docker exec egret-dev-trino-1 trino --server localhost:8080 --user admin --execute "$Q"
echo "--- alice ---"; docker exec egret-dev-trino-1 trino --server localhost:8080 --user alice --execute "$Q"
echo "--- bob ---";   docker exec egret-dev-trino-1 trino --server localhost:8080 --user bob   --execute "$Q"
```

### Enforce kỳ vọng (tiêu chí PASS)

| User | Số dòng | Cột `item` | Ý nghĩa |
|---|---|---|---|
| `admin` | 4 (tất cả) | `book/pen/lamp/desk` | full access |
| `alice` | 2 (chỉ US) | `xxxx` | **row-filter `country='US'` + MASK cột** |
| `bob` | query fail | — | `Access Denied: Cannot access catalog iceberg` (default-deny) |

Thấy đúng ba kết quả trên = cả hai tính năng Egret được engine thật enforce đúng. Xác nhận plugin đã kéo policy của Egret: `docker exec egret-dev-trino-1 cat /tmp/ranger-trino-cache/trino_trino_prod.json` hiện `ServicePolicies` với `id` policy dạng số.

## 6. Dọn dẹp

```bash
pkill -f "target/debug/egret serve"
cd deploy && docker compose -f docker-compose.dev.yml rm -fs trino minio
rm -rf /tmp/egret-itest /tmp/ac.properties /tmp/ranger-trino-security.xml
rm -f trino/catalog/iceberg.properties   # host-specific; không bao giờ commit
```

## Ghi chú hợp đồng (bài test này canh gì)

1. **Reserved table properties** — Trino gửi `format-version` trong `CREATE TABLE`; Egret phải strip `RESERVED_PROPERTIES` của `iceberg` trước khi build metadata, nếu không sẽ 400.
2. **Guard empty-location** — Egret KHÔNG được ghi sẵn `metadata.json` lúc create; Trino kiểm location của bảng phải rỗng. DB jsonb là nguồn sự thật; artifact ghi ở commit đầu.
3. **Policy `id` trong download** — engine Ranger thật NPE khi `RangerPolicy.getId()==null`; download phải inject `id`/`version`/`isEnabled` từ DB.
4. **`/service` ẩn danh** — plugin Ranger gốc xác thực ngoài-luồng; `ranger.auth_token` rỗng mở `/service` cho chúng. Đặt token để bắt buộc bearer.
5. **Mạng** — một IP host chung giữ nhất quán kiểm SSRF của Egret, S3 endpoint của Trino, và URL đã ký.
