# Test tích hợp thủ công — Trino × Egret Iceberg Catalog × Ranger

> **Trạng thái:** Gate thủ công trước khi push (KHÔNG chạy trong CI) · **Ngày:** 2026-06-18
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../../en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Chạy bài này trước khi push thay đổi chạm `egret-catalog`, `egret-storage`, `egret-authz`, hoặc `egret-ranger`. Nó kiểm các hợp đồng phía engine (Iceberg REST + tải policy Ranger) với **Trino thật**, lộ ra lỗ hổng mà unit test không thấy (reserved properties, guard empty-location, policy `id`, auth plugin).

## Bài test chứng minh gì

Một **Trino 481** thật dùng Egret cho CẢ HAI tính năng lõi-governance cùng lúc:
- **Iceberg REST catalog** — `CREATE SCHEMA / CREATE TABLE / INSERT / SELECT` đẩy data qua `/catalog` của Egret vào RustFS.
- **Apache Ranger access control** — plugin `ranger` tích hợp sẵn của Trino tải policy từ `/service` của Egret và enforce **deny / row-filter / column-mask** trên query thật.

## Vì sao không nằm trong CI

Cần Docker + image Trino nặng JVM + RustFS; plugin Ranger của Trino poll theo timer. Quá chậm/nặng cho CI. Các test unit + gate-DB + `RangerPolicyEngineImpl` đã phủ logic trong CI; bài này lấp khoảng **hợp đồng-engine** một cách thủ công.

## Điều kiện tiên quyết

- Docker đang chạy.
- Repo đã build: `cargo build -p egret-bin`.
- Postgres truy cập được ở `postgres://egret:egret@localhost:5432/egret` (`portal-pg` dev hoặc `docker compose -f deploy/docker-compose.dev.yml up -d postgres`).
- Cổng host trống: 8080 (egret), 9000/9001 (RustFS), 8090 (Trino).

## Lưu ý mạng (quan trọng)

Egret chạy trên host; Trino + RustFS chạy trong container. Signer của Egret có kiểm SSRF host, và S3 endpoint của Trino phải khớp cái Egret ký/ghi. Dùng **IP LAN** của host làm địa chỉ chung duy nhất:

```bash
export HOST_IP=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)   # macOS; Linux: hostname -I | awk '{print $1}'
echo "HOST_IP=$HOST_IP"
```

---

## 1. Khởi động RustFS + tạo bucket warehouse

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d rustfs
until [ "$(curl -s -o /dev/null -w '%{http_code}' http://localhost:9000/health)" = 200 ]; do sleep 2; done
NET=egret-dev_default
docker run --rm --network "$NET" -e AWS_ACCESS_KEY_ID=egret -e AWS_SECRET_ACCESS_KEY=egretsecret \
  -e AWS_DEFAULT_REGION=us-east-1 amazon/aws-cli:2.37.9 --endpoint-url http://rustfs:9000 s3 mb s3://warehouse
```

## 2. Chạy Egret bật catalog + ranger

`ranger.auth_mode = "anonymous"` cho XML plugin viết tay bên dưới poll `/ranger` mà không cần credential. Chỉ dùng trên stack dùng-rồi-bỏ này: ở chế độ anonymous, ai với tới egret cũng tải được policy của mọi profile. Mặc định `required` buộc mỗi plugin trình credential riêng của engine nó ([`docs/vi/guides/ranger-surface.md`](../guides/ranger-surface.md)); `auth_token = ""` không nhận bearer operator nào.

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
Kỳ vọng: `healthz 200`, JSON `/v1/config` có `overrides` chứa `prefix=<WS_SLUG>` + `warehouse` + `s3.signer.uri` (bỏ `?warehouse` thì chỉ trả về khóa signer, không có prefix), `anon /ranger: 200` (`401` nghĩa là egret không ở chế độ anonymous).

## 3. Cấu hình + khởi động Trino

### 3a. Iceberg catalog → Egret (`deploy/trino/catalog/iceberg.properties`)

Catalog giờ là **per-workspace**: mọi route Iceberg REST đều có tiền tố slug của
workspace (`/v1/{prefix}/...`). Đặt `iceberg.rest-catalog.warehouse` bằng slug của
workspace — Trino gọi `GET /v1/config?warehouse=<slug>`, đọc lại
`overrides.prefix=<slug>` + `overrides.warehouse`, rồi tự động nhắm tới các route
có tiền tố. Thay `<WS_SLUG>` bên dưới bằng slug workspace thật (ví dụ từ cột
`workspaces.slug`, hoặc slug được cấp khi tạo workspace).

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
  <property><name>ranger.plugin.trino.policy.rest.url</name><value>http://$HOST_IP:8080/ranger</value></property>
  <property><name>ranger.plugin.trino.policy.pollIntervalMs</name><value>5000</value></property>
  <property><name>ranger.plugin.trino.policy.cache.dir</name><value>/tmp/ranger-trino-cache</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.connection.timeoutMs</name><value>10000</value></property>
  <property><name>ranger.plugin.trino.policy.rest.client.read.timeoutMs</name><value>10000</value></property>
</configuration>
EOF
```
`RangerAdminRESTClient` tải từ `<policy.rest.url>/service/plugins/policies/download/<service.name>` — tức `/ranger/service` của Egret. Key config Trino 481: `ranger.service.name`, `ranger.plugin.config.resource`.

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
Kỳ vọng: `CREATE SCHEMA`, `CREATE TABLE`, `INSERT: 4 rows`, và `US 2 29.5 / VN 2 56.2`. Kiểm file trong RustFS dưới `warehouse/demo/report-*/` (`data/*.parquet`, `metadata/0000N-*.metadata.json` — lưu ý KHÔNG có `00000`; nó được ghi ở commit đầu tiên, không phải lúc create, nhờ vậy qua được guard empty-location của Trino).

## 5. Test Ranger (enforce policy trên query thật)

Tên catalog trong Trino là `iceberg`, nên resource Ranger dùng `catalog=iceberg`. Default-deny nghĩa là user cần `execute` trên `queryid` và `impersonate` trên `trinouser` để chạy query, cộng `select` trên bảng.

```bash
P() { curl -s -o /dev/null -w "  %{http_code}\n" -X POST "http://$HOST_IP:8080/ranger/service/plugins/policies" -H "Content-Type: application/json" -d "$1"; }
curl -s -X POST "http://$HOST_IP:8080/ranger/service/plugins/services" -H "Content-Type: application/json" -d '{"name":"trino_prod","type":"trino","configs":{}}' >/dev/null
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
cd deploy && docker compose -f docker-compose.dev.yml rm -fs trino rustfs
rm -rf /tmp/egret-itest /tmp/ac.properties /tmp/ranger-trino-security.xml
rm -f trino/catalog/iceberg.properties   # host-specific; không bao giờ commit
```

## Ghi chú hợp đồng (bài test này canh gì)

1. **Reserved table properties** — Trino gửi `format-version` trong `CREATE TABLE`; Egret phải strip `RESERVED_PROPERTIES` của `iceberg` trước khi build metadata, nếu không sẽ 400.
2. **Guard empty-location** — Egret KHÔNG được ghi sẵn `metadata.json` lúc create; Trino kiểm location của bảng phải rỗng. DB jsonb là nguồn sự thật; artifact ghi ở commit đầu.
3. **Policy `id` trong download** — engine Ranger thật NPE khi `RangerPolicy.getId()==null`; download phải inject `id`/`version`/`isEnabled` từ DB.
4. **`/ranger` ẩn danh, chỉ cho bài test này** — XML plugin viết tay không mang credential, nên bài test chạy `ranger.auth_mode = "anonymous"`. Với mặc định `required`, plugin phải trình credential của engine (`ranger.plugin.trino.policy.rest.client.username` / `.password`, plugin 2.8.0 trở lên), nếu không sẽ nhận 401.
5. **Mạng** — một IP host chung giữ nhất quán kiểm SSRF của Egret, S3 endpoint của Trino, và URL đã ký.

---

## 7. Kiểm evaluator (SP4d-1) — DỰ ĐOÁN TRƯỚC, rồi mới chạy query

> **Từ khi bề mặt Ranger đòi credential (2026-10-20):** egret trên kind mặc định chạy `ranger.authMode=required`, còn các pod dựng tay bên dưới không mang credential, nên plugin của chúng (và lệnh `curl` tải policy trong pod) nhận 401. Cho bài kiểm này, hoặc deploy egret với `--set ranger.authMode=anonymous` (chỉ cluster dùng-rồi-bỏ), hoặc đăng ký mỗi engine dựng tay là external engine gắn với profile của nó và thêm `ranger.plugin.<trino|starrocks>.policy.rest.client.username` / `.password` bằng credential egret hiện một lần (rồi `curl -u 'engine-<id>:<password>'` đường `/ranger/service/plugins/secure/policies/download/<profile>`). Xem [`docs/vi/guides/ranger-surface.md`](../guides/ranger-surface.md).

Thêm 2026-09-28. Mục 1-6 chứng minh **policy của Egret tới được Trino thật và được cưỡng chế**.
Mục này chứng minh chuyện khác: `egret-ranger-eval` — bộ đánh giá Rust trả lời "cái này có được
phép không?" mà không cần engine — **đồng ý với chính plugin đang trả lời câu đó**. Gate vi phân
(`crates/egret-ranger-eval/tests/differential.rs`) so crate với `RangerPolicyEngineImpl` Java
thật, nhưng harness đó **tự dựng request**: nó không thấy Trino đặt tên tài nguyên thế nào, hỏi
ở độ mịn nào, gửi access type nào. Mục này phủ đúng khoảng trống đó.

Nội dung đầy đủ (manifest của coordinator riêng, 8 case, thứ tự ba bước, dọn dẹp) là **bản
tiếng Anh chuẩn**:
[`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../../en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md) §7.
Kết quả đã đo: [`docs/en/plans/2026-09-28-sp4d-1-live-result.md`](../../en/plans/2026-09-28-sp4d-1-live-result.md).

Kiểm này gồm **mười case**: tám trên một Trino coordinator riêng, và hai trên một StarRocks
FE + CN riêng (S1 hình dạng ANCESTOR `{catalog, database}` → BỊ TỪ CHỐI; S2 cùng thế nhưng có
`column: ["*"]` → ĐƯỢC PHÉP). Phải đo StarRocks riêng: nó chạy plugin 2.8.0 trên một service-def
khác, tầng giữa tên là `database` chứ không phải `schema`, nên KHÔNG được suy từ kết quả Trino.

Bốn điều bắt buộc, tóm tắt:

1. **Tính dự đoán TRƯỚC, ghi xuống, rồi mới chạy query.** Dự đoán viết sau khi thấy kết quả
   không chứng minh gì. Và **bất đồng là một PHÁT HIỆN**, không phải test cần chỉnh: hoặc
   evaluator sai, hoặc harness sai, hoặc cách repo này đọc Ranger sai — phải kết luận là cái nào.
2. **Xác nhận plugin đang giữ đúng `policyVersion` mình vừa dự đoán từ đó**, bằng cách đọc
   `/tmp/ranger/<service>/policycache/trino_<service>.json` ngay trong pod. Coordinator do egret
   cấp phát poll mỗi **30 giây**; query trước khi poll về là đo trạng thái CŨ.
3. **Case row-filter phải hỏi ở độ mịn BẢNG**, không kèm `column`. `rowFilterDef.resources` của
   cả hai engine dừng ở `table`, nên request theo cột biến mọi policy row-filter thành khớp
   ANCESTOR và trả `NoFilter` — đọc ra như "row filter không chạy". Task 6 mất sáu test vì điều
   này.

4. **Chủ thể trên StarRocks phải là user native, KHÔNG BAO GIỜ là `root`.** `root` đi vòng qua
   kiểm tra trước khi Ranger được hỏi (`RangerAccessController.java:115-117`), nên kết quả đo
   bằng `root` không chứng minh được gì về một deny. Dùng `root` chỉ để DDL và để làm control
   xác nhận đường dữ liệu đọc được.

### Ghi chú hợp đồng mục này thêm vào

6. **Khớp ANCESTOR KHÔNG phải là cho phép.** Policy nêu ít tầng hơn request thì không cấp gì.
   Evaluator và Trino thật đồng ý; defect T1 của SP4a chính là hình dạng này.
7. **Có tag service thì evaluator im lặng.** `tagPolicies` khác null trong download là
   `Undecidable { EnvelopeConstruct(TagPolicies) }`. Auto-bind của Trino tạo kèm một service
   `<profile>_tag`, nên mọi profile nó tạo đều dính — nhưng CHỈ khi tag service đó còn **tồn
   tại**: lookup trong `download.rs` là best-effort, nên profile có tag service đã bị xoá (trong
   cụm này là `demo-trino`) không nhận khối `tagPolicies` và vẫn quyết bình thường. Hãy kiểm
   DOWNLOAD, đừng kiểm `config_jsonb`. Dự đoán trên profile KHÔNG có tag service, hoặc chấp nhận
   không có câu trả lời.
8. **Đừng bao giờ cắt bớt input để lấy câu trả lời.** Evaluator nói `Undecidable` thì đó LÀ kết
   quả. Sửa download đã bắt được cho tới khi nó quyết được cũng là lỗi y như sửa test cho tới
   khi nó xanh.
