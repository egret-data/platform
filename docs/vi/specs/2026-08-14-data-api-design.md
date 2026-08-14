# Data API — thiết kế

> **Trạng thái:** Đã duyệt thiết kế, chưa triển khai · **Ngày:** 2026-08-14 · **Tham chiếu:** [sqlrest](https://github.com/inrgihc/sqlrest) v1.9.0 (chỉ tham chiếu domain model + UX; không tái sử dụng code — sqlrest là Java/Spring, egret là Rust)
>
> *Bản tiếng Việt tham chiếu. Bản chuẩn là [docs/en/specs/2026-08-14-data-api-design.md](../../en/specs/2026-08-14-data-api-design.md).*

## 1. Vấn đề

egret truy vấn được Trino và StarRocks theo kiểu tương tác (SQL Lab, Explore, dashboard), nhưng chưa có cách nào biến một truy vấn đã được quản trị thành **endpoint HTTP cho máy gọi**. Các bên tiêu thụ phía sau — service nội bộ, công cụ low-code, tích hợp đối tác — hiện không có lựa chọn nào ngoài việc dùng thẳng credential của engine, vốn đi vòng qua policy row/column của Ranger và không để lại dấu vết audit theo từng bên tiêu thụ.

Data API lấp khoảng trống đó: thành viên workspace viết một câu SELECT có tham số, publish tại một URL ổn định, client bên ngoài gọi bằng API key. Mọi lời gọi đều chạy dưới một principal thật của egret, nên Ranger và audit vẫn áp dụng nguyên vẹn.

## 2. Các quyết định đã chốt

| Quyết định | Lựa chọn | Lý do |
|---|---|---|
| Cách xây | Crate Rust native trong egret; sqlrest là **tham chiếu** domain model và UX | Một hệ thống, một mô hình quyền, một UI. Chạy sqlrest song song nghĩa là hai identity store, hai datastore, hai UI. |
| Nguồn định nghĩa endpoint | SQL thô + tham số là lõi; thêm lối tắt "Publish as API" để seed SQL từ Saved Query / Dataset | SQL thô là nguyên thủy linh hoạt; lối tắt bỏ được bước copy-paste mà không bó buộc mô hình. |
| Định danh bên tiêu thụ | **Service account = principal thật của egret** | Query đi qua machine-idp → engine token → Ranger. Một mô hình quyền; audit truy về được một danh tính có tên. |
| Đọc/ghi | **Chỉ đọc (SELECT) ở v1** | Thu hẹp bề mặt tấn công; không cần thiết kế transaction/rollback/idempotency. Ghi để v2. |
| Phạm vi v1 | Version & rollback · rate limit / quota · response cache · OpenAPI tự sinh + Try-it | Chọn cả bốn. |
| Bề mặt phục vụ | Cùng process với `egret-server`, nhưng cấu trúc sao cho tách thành binary riêng được sau | Mặc định modular monolith mà không chặn đường cô lập về sau. |
| Tenancy / URL | Workspace-scoped, workspace nằm trong path: `/data-api/v1/{workspace}/{path}` | Duy nhất toàn cục, dễ debug, khớp với cách Cedar scope theo workspace. |

## 3. Điều kiện tiên quyết — escaping tham số hiện không an toàn cho bề mặt công khai

`crates/egret-query/src/execution/template.rs:62` render tham số string như sau:

```rust
_ => Ok(format!("'{}'", p.value.replace('\'', "''"))),
```

Chỉ nhân đôi dấu nháy đơn, **bỏ qua backslash**. StarRocks theo dialect MySQL và mặc định không bật `NO_BACKSLASH_ESCAPES`, nên `\` là ký tự escape và `\'` thoát ra khỏi literal. Trino không bị ảnh hưởng (nó không xử lý backslash escape trong string literal chuẩn).

Hiện tại lỗ hổng này chỉ với tới được bởi người dùng đã xác thực có quyền SQL Lab. Data API sẽ đưa **chính đường render đó ra Internet công khai sau một API key**. Vì vậy bản vá thuộc phạm vi v1, không phải việc làm sau.

Unit test hiện có `string_param_neutralizes_injection` (`template.rs:93`) xanh dù lỗ hổng còn hay đã hết — nó chỉ phủ ca kinh điển `' OR '1'='1`. Đó không phải bằng chứng.

`Connector::execute` (`crates/egret-query/src/execution/connector/mod.rs:70`) nhận `sql: &str` và không có đường bound parameter, nên không thể sửa bằng cách "cứ dùng prepared statement" mà không đổi trait.

**Bản vá (v1):** tách `literal()` thành escaper theo dialect trong `egret-query`, tham số hóa theo `Engine`:

- **Trino** — chỉ nhân đôi `'` (hành vi hiện tại, vốn đã đúng cho Trino).
- **StarRocks** — escape `\` → `\\` **trước**, rồi `'` → `''`. Thứ tự quan trọng; đảo lại là lỗ hổng quay về.

Vì bản vá nằm trong `egret-query`, SQL Lab được vá bằng chính thay đổi đó.

**Cố ý hoãn:** bound parameter ở tầng connector. StarRocks (`mysql_async`) hỗ trợ trực tiếp; Trino cần `X-Trino-Prepared-Statement` + `EXECUTE … USING`. Đó là thay đổi trait `Connector` đủ lớn để thành dự án riêng. Escaper theo dialect nằm sau một hàm duy nhất nên thay thế được ở một chỗ.

**Tham số là giá trị, không bao giờ là định danh.** `{{param}}` không được xuất hiện ở vị trí tên cột, tên bảng hay `ORDER BY`. Cột động được biểu diễn bằng tham số kiểu `enum` với allow-list tường minh, render bằng cách tra giá trị trong danh sách đó. Allow-list là dạng an toàn duy nhất cho định danh.

## 4. Kiến trúc

### 4.1 Các crate

| Crate | Vai trò |
|---|---|
| `egret-dataapi-api` | Contract crate lá, pure-serde: DTO định nghĩa endpoint, param spec, published snapshot. Dùng được bởi `egret-server` và frontend codegen mà không kéo theo sqlx/axum. |
| `egret-dataapi` | Seam crate: store (sở hữu schema `dataapi`), vòng đời publish, service account + key, rate limit, cache, access log, **hai router**. |

### 4.2 Hai router, tách rời có chủ đích

- `management_router()` → nest dưới `/api` như mọi seam khác. Session cookie + CSRF + Cedar. Soạn thảo, publish, quản lý key.
- `serving_router()` → nest dưới `/data-api/v1`. **Không cookie, không CSRF**, chỉ `Authorization: Bearer`. Middleware stack tách hẳn.

Ranh giới này quan trọng hơn vẻ ngoài của nó. CSRF từng giết một POST trong production của chính codebase này mà không router-level test nào thấy được. Serving surface **không** được thừa kế ngầm bất kỳ middleware nào của control plane; mọi layer trên nó đều phải khai báo tường minh.

### 4.3 Seam thực thi

Thêm trait `QueryExecPort` vào `egret-query-api`. `egret-dataapi` giữ `Arc<dyn QueryExecPort>`; `egret-query` implement nó trên state cụ thể của mình. Đây là bản sao pattern `NotebookDocsPort` ở `crates/egret-analytics-api/src/port.rs`.

```rust
// crates/egret-query-api/src/port.rs (mới)
#[async_trait::async_trait]
pub trait QueryExecPort: Send + Sync {
    /// Chạy SQL đã render dưới danh nghĩa `principal`, tôn trọng delegation
    /// (Trino `X-Trino-User` / StarRocks `EXECUTE AS`) và Ranger.
    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        limits: Limits,
    ) -> Result<TabularResult, egret_core::Error>;

    /// Render tham số thành SQL an toàn theo dialect của database đích.
    fn render(
        &self,
        database_id: Uuid,
        sql: &str,
        params: &[QueryParam],
    ) -> Result<String, egret_core::Error>;

    /// Phân giải schema cột output mà không chạy query. Dùng cho bước
    /// validate-trước-publish (§5.3) để đóng băng hình dạng output vào
    /// snapshot.
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;
}
```

**Thay đổi hợp đồng cần ghi nhận:** module doc của `egret-query-api` (dòng 10–11) hiện nói crate này **cố tình không có** trait `QueryPort`, vì "runtime surface của seam là concrete `Connector` data-plane". Thêm trait này là đảo lại chủ ý đó vì một lý do chính đáng — `egret-dataapi` cần một seam thay thế được để chuyển sang binary riêng ở v2. **Doc comment phải được cập nhật trong cùng thay đổi đó**, không để nó mâu thuẫn với code.

### 4.4 Đăng ký

- Migration `0119_dataapi.sql`, `CREATE SCHEMA IF NOT EXISTS dataapi` — mỗi seam một schema, khớp với `query`, `policy`, `automation`.
- `crates/egret-server/src/lib.rs`: một dòng `doc.merge(egret_dataapi::openapi())` và hai khối `.nest()`. Danh sách merge giữ tường minh — ghi chú tại dòng 90 của chính file đó nói "do NOT loop".

## 5. Data model

Hai thứ hạ tầng sẵn có của egret loại bỏ được các bảng mà sqlrest cần:

**`access_grants` là bảng grant hợp nhất** (`crates/egret-auth/src/grants.rs:47`) với `principal_type` ('user' | 'group') và `scope_type` ('platform' | 'workspace' | 'object') + `object_type`. Không cần bảng grant riêng (kiểu `ApiAssignment ↔ ClientGroup` của sqlrest). Cấp cho service account quyền gọi một endpoint chỉ là một hàng:

```text
principal_type='user', principal_id=<service_account>,
scope_type='object',  object_type='data_api_endpoint', scope_id=<endpoint_id>
```

**Service account là hàng trong `users`.** `migrations/0002_identity.sql:5` có `provider CHECK (provider IN ('local','oidc'))`; mở rộng thành `('local','oidc','service')`. Đây chính là thứ làm cho "principal thật" trở thành thật: Cedar, `access_grants`, Ranger, machine-idp và audit đều đối xử với service account y hệt một user, không có identity store thứ hai.

*Chỗ gợn đã biết:* `users.email` là `citext NOT NULL UNIQUE`. Service account không có email thật, nên sinh email tổng hợp dạng `svc.<slug>@<workspace-slug>.service.egret.local`. Nới `email` thành nullable sẽ đụng rất nhiều code đang giả định nó tồn tại; email tổng hợp là thay đổi nhỏ hơn. `users.status` vốn đã cho phép `'disabled'`, dùng làm công tắc tắt service account.

### 5.1 Schema `dataapi`

```sql
dataapi.endpoints            -- bản nháp; sửa tự do
  id, workspace_id, folder_id, name, description,
  path, method, database_id, sql,
  params jsonb, output jsonb, cache jsonb, limits jsonb, rate_limit jsonb,
  source_kind ('raw'|'saved_query'|'dataset'), source_ref uuid,
  live_version_id uuid NULL,          -- trỏ tới snapshot đang phục vụ
  state ('draft'|'live'|'disabled'),
  created_by, created_at, updated_at
  UNIQUE (workspace_id, method, path)

dataapi.endpoint_versions    -- snapshot BẤT BIẾN
  id, endpoint_id, version int, snapshot jsonb, note, created_by, created_at
  UNIQUE (endpoint_id, version)

dataapi.api_keys             -- credential cho serving surface
  id, workspace_id, service_account_id (→ users.id),
  name, prefix text, secret_hash text, expires_at, last_used_at,
  revoked_at, created_by, created_at
  UNIQUE (prefix)

dataapi.access_log           -- một hàng mỗi lần gọi
  id, ts, endpoint_id, version, service_account_id, api_key_id,
  status_code, row_count, duration_ms, cache_hit bool,
  engine_query_id, client_ip, error_code
```

### 5.1.1 Hình dạng cấu hình

Bốn cột cấu hình jsonb, cũng là hình dạng được đóng băng vào `endpoint_versions.snapshot`:

```jsonc
// params — các tham số đã khai báo, theo thứ tự khai báo
[ { "name": "date", "type": "date", "location": "query",
    "required": true, "default": null, "allowed": null,
    "description": "Ngày báo cáo" },
  { "name": "region", "type": "enum", "location": "query",
    "required": false, "default": "ALL",
    "allowed": ["ALL", "APAC", "EMEA"] } ]
// type:     string | number | boolean | date | timestamp | enum
// location: query | body        (không bao giờ là path — path là tự do, §5.2)
// allowed:  bắt buộc với enum; dạng an toàn duy nhất cho định danh (§3)

// output — định hình response
{ "shape": "objects",            // objects | arrays
  "naming": "as_is",             // as_is | camel | snake
  "columns": [ { "source": "total_revenue", "name": "revenue",
                 "format": "number:2" } ] }

// cache
{ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }

// limits
{ "max_rows": 10000, "timeout_seconds": 30 }

// rate_limit
{ "per_key_qps": 5, "burst": 10, "per_endpoint_qps": 50 }
```

### 5.1.2 Response thành công

```json
{
  "data": [ { "day": "2026-08-01", "revenue": 1234.50 } ],
  "meta": {
    "columns": [ { "name": "day", "type": "date" },
                 { "name": "revenue", "type": "decimal" } ],
    "row_count": 1,
    "version": 3,
    "cached": false,
    "request_id": "01J..."
  }
}
```

`meta.version` là version đã phục vụ lời gọi đó — nó giúp một bug phía client tái hiện được trên đúng snapshot đã sinh ra nó. `meta.cached` cũng không phải trang trí: thiếu nó thì khi debug không phân biệt được kết quả cũ với kết quả vừa lấy.

`dataapi.endpoints.folder_id` và `dataapi.api_keys.service_account_id` là **FK xuyên schema** trỏ vào `public.folders(id)` và `public.users(id)`. Điều này hơi nghịch với nguyên tắc mỗi seam sở hữu schema riêng, nhưng `folders` và `users` vốn đã là bảng dùng chung xuyên suốt, và nhân bản chúng còn tệ hơn. Chấp nhận và ghi nhận tại đây.

### 5.2 Folder không phải là segment của URL

`folders` của egret (`migrations/0021_folders.sql:4`) là `workspace_id + entity_type + name`, gắn qua `folder_id … ON DELETE SET NULL`. Người dùng đổi tên, di chuyển và xóa folder thoải mái. `SQLREST_API_GROUP` của sqlrest cũng chỉ có `id + name`, còn `path` là field riêng trên assignment — **sqlrest cũng không đưa group vào URL**.

URL là hợp đồng đã phát hành cho client bên ngoài; folder là tiện ích hiển thị. Nếu segment URL lấy từ tên folder thì đổi tên folder hoặc chuyển endpoint sẽ âm thầm làm chết mọi client đã deploy, và `ON DELETE SET NULL` sẽ để endpoint không còn segment nào cả.

| | Nguồn | Đổi được không? |
|---|---|---|
| Folder | `folders` với `entity_type='data_api_endpoint'`, giống chart/dashboard/dataset | Tự do; không ảnh hưởng URL |
| URL path | `dataapi.endpoints.path` — chuỗi tự do do user đặt, được chứa `/` | Đổi là breaking change; UI phải cảnh báo |

```text
GET /data-api/v1/sales/reports/daily-revenue
                 └─ws─┘ └─────path────────┘
```

`reports/` ở đó là một phần của `path` do user tự gõ, không liên quan tới việc endpoint nằm trong folder nào.

### 5.3 Vòng đời

```text
draft ──validate──> publish(v N) ──> live
  ▲                                   │
  └────── sửa tiếp (không ảnh hưởng) ─┘
                    rollback ──> trỏ live_version_id về v N-1
                    disable  ──> state='disabled', serving trả 404
```

**Bất biến: serving path không bao giờ đọc `dataapi.endpoints`.** Nó chỉ đọc `endpoint_versions.snapshot` qua `live_version_id`. Đây là thứ làm cho "sửa SQL không đổi hành vi của client đang chạy" thành đúng theo cấu trúc chứ không phải theo kỷ luật. Nếu serving đọc bảng draft thì tính năng version chỉ còn là trang trí.

`validate` chạy trước publish và từ chối mọi thứ không đạt:

- SQL là **đúng một** statement, và statement đó là `SELECT`. Cho phép mở đầu bằng `WITH … SELECT` (CTE); mọi thứ khác — DML, DDL, `SET`, hoặc một statement thứ hai sau dấu `;` — đều bị từ chối.
- Mọi `{{param}}` phân giải được về một tham số đã khai báo, và mọi tham số đã khai báo đều được tham chiếu.
- `describe_as` (§4.3) phân giải được schema output, và schema đó được đóng băng vào snapshot.
- Nếu `cache.scope` là `shared`, connection đích không được là delegated (§6.3).

**Method cho phép ở v1 là `GET` và `POST`, cả hai đều chỉ đọc.** `POST` tồn tại chỉ để nhận payload tham số quá lớn hoặc quá có cấu trúc cho query string; nó chịu cùng ràng buộc một-câu-SELECT. `PUT`/`PATCH`/`DELETE` bị từ chối — chấp nhận chúng sẽ ngụ ý một ngữ nghĩa ghi mà v1 không có.

Publish / rollback / tạo key / thu hồi key được phát ra dưới dạng sự kiện `egret-audit` — không thêm bảng.

**Cảnh báo vận hành phải hiện lên UI:** publish endpoint **không** cấp quyền dữ liệu. Vì query chạy dưới danh nghĩa service account với Ranger enforcement, admin còn phải cấp quyền cho chính service account đó trong Ranger. Endpoint publish mà thiếu bước này sẽ lỗi permission từ engine, và lỗi đó phải được map thành thông báo nói rõ điều này thay vì để lộ raw engine error.

## 6. Xác thực và luồng request

### 6.1 API key

Định dạng `egt_<prefix>_<secret>`: `prefix` 12 ký tự (index unique, dùng để tra cứu) và `secret` 32 byte ngẫu nhiên, base64url. Lưu `prefix` + `secret_hash`. Secret chỉ hiện đúng một lần, lúc tạo.

**Hash bằng SHA-256; không dùng `crates/egret-auth/src/password.rs`.** Argon2 ở đó đúng cho mật khẩu người chọn và sai cho API key: secret 32 byte ngẫu nhiên đã đủ entropy nên không cần KDF chống brute-force, trong khi trả ~50–100 ms KDF trên *mỗi* request API là không chấp nhận được. So sánh constant-time.

### 6.2 Chuỗi xử lý mỗi request

```text
GET /data-api/v1/{ws}/{path}
 │
 1. key → service account   (active, chưa revoke, chưa hết hạn)  ✗→ 401
 2. (ws, method, path) → endpoint → live_version_id → snapshot   ✗→ 404
 3. workspace của service account == {ws} trong path             ✗→ 404
 4. Cedar: dataapi:invoke trên Object data_api_endpoint:{id}     ✗→ 403
 5. rate limit (theo endpoint và theo key)                       ✗→ 429 + Retry-After
 6. cache lookup                                                 hit → trả
 7. render params → SQL an toàn theo dialect                     ✗→ 400
 8. machine-idp mint engine token cho SA
    → QueryExecPort::execute_as
 9. Ranger lọc row/column ở engine, dưới danh nghĩa SA
10. shape output → lưu cache → access_log → trả
```

Bước 3 trả **404 chứ không phải 403** là cố ý: một key thuộc workspace khác không được phép phân biệt "không có endpoint đó" với "có, nhưng anh không được gọi".

### 6.3 Cache phải scope theo principal

Bước 9 lọc dữ liệu **theo từng principal**, nên hai service account gọi cùng endpoint với cùng tham số có thể nhận về số dòng khác nhau một cách hợp lệ. Cache key thiếu principal là rò rỉ dữ liệu xuyên thẳng qua Ranger.

Vì vậy cấu hình cache mang theo một `scope`:

- `per_principal` (mặc định) — cache key gồm `service_account_id`.
- `shared` — **chỉ được phép khi connection không delegated**. Kiểm tại validate time, để cấu hình sai không thể publish ngay từ đầu.

Đặt kiểm tra này ở validate time thay vì runtime nghĩa là cấu hình không an toàn không bao giờ tới được production.

### 6.4 Rate limit

`crates/egret-auth/src/ratelimit.rs` có sẵn nhưng **ngữ nghĩa là lockout kiểu đăng nhập** (vượt ngưỡng → khóa một khoảng cố định), sai cho API. Data API cần token bucket: 429 + `Retry-After`, thử lại được ngay khi bucket hồi.

Viết `TokenBucket` trong `egret-dataapi`, nhưng **giữ nguyên hình dạng `check_at(key, now)`** của module đó — clock tiêm từ ngoài, để test không phụ thuộc thời gian thực.

Đặt sau một trait `RateLimitStore`. v1 in-memory theo process; khi serving surface tách thành binary nhiều replica, một implementation dùng store chung sẽ thay vào mà không đụng call site.

## 7. Xử lý lỗi

Một envelope ổn định, máy đọc được:

```json
{ "error": { "code": "param_invalid", "message": "...", "request_id": "01J..." } }
```

**Không bao giờ trả raw engine error cho caller bên ngoài.** Lỗi Trino/StarRocks mang theo tên bảng, tên cột và đôi khi cả thông tin host — trên một endpoint công khai sau API key thì đó là rò rỉ metadata. Map sang code cố định và ghi chi tiết đầy đủ vào log nội bộ kèm `request_id` để support tra được.

| HTTP | code | Khi nào |
|---|---|---|
| 400 | `param_invalid` / `param_missing` | sai kiểu, thiếu, hoặc ngoài allow-list |
| 400 | `row_limit_exceeded` | kết quả vượt row cap đã cấu hình |
| 401 | `key_invalid` | key không tồn tại, hết hạn, hoặc đã thu hồi |
| 403 | `forbidden` | Cedar từ chối `dataapi:invoke` |
| 403 | `data_access_denied` | Ranger từ chối — message nói rõ service account chưa được cấp quyền dữ liệu (§5.3) |
| 404 | `not_found` | không tồn tại, đã disable, hoặc sai workspace |
| 429 | `rate_limited` | kèm `Retry-After` |
| 502 | `engine_unavailable` | không kết nối được engine |
| 504 | `timeout` | vượt timeout trong snapshot |

**Serving path tự áp timeout của mình** thay vì dựa vào JobManager. `describe()` (`connector/mod.rs:81`) đã mang ghi chú y hệt — nó tự bọc timeout vì được gọi trực tiếp, không có JobManager bọc ngoài. Serving path cũng đi thẳng, nên gánh cùng nghĩa vụ.

**Vượt row cap là lỗi, không phải cắt bớt im lặng.** Trả 200 kèm dữ liệu đã bị cắt là kiểu hỏng tệ nhất có thể có với một API dữ liệu: client tin rằng mình có đủ mọi dòng và cho ra số liệu sai mà không có tín hiệu nào. `400 row_limit_exceeded`, nói rõ cap là bao nhiêu.

## 8. Observability

- `dataapi.access_log` ghi lại mọi lời gọi. Đây là bảng tăng nhanh nhất hệ thống: partition theo tháng cộng một job dọn theo retention, tái dùng gc-worker pattern có sẵn từ catalog CE6 thay vì viết mới.
- Thống kê trên management UI (số lần gọi, tỉ lệ lỗi, p50/p95, cache hit rate theo endpoint) tính từ `access_log` — tương đương `HourCount` / `DateCount` / `NameCount` của sqlrest.
- `request_id` được truyền xuống engine và lưu cạnh `engine_query_id`, để một lời gọi API chậm truy ngược về đúng query trên Trino/StarRocks.
- Publish, rollback, tạo key và thu hồi key là sự kiện `egret-audit`.

## 9. Kiểm thử

Test xanh không đồng nghĩa với hành vi đã được kiểm chứng, và repo này có đủ vết sẹo để chứng minh điều đó. Mỗi mục dưới đây nêu rõ cái gì phải đỏ thì test mới là bằng chứng.

1. **Escaping** — unit test theo từng dialect **cộng với** integration test chạy trên StarRocks thật. Unit test hiện có xanh dù lỗ hổng còn nguyên, đó đúng là cách nó bị bỏ lọt lần đầu; riêng unit test không phải bằng chứng ở đây.
2. **Mọi authz gate được mutation-check** — gỡ gate ra thì test phải đỏ. Test chạy bằng service account thật không có quyền admin: đo bằng danh tính admin sẽ bypass chính cái đang cần kiểm.
3. **Cách ly version** — publish v1, sửa draft, gọi lại, hành vi v1 phải giữ nguyên. Đây là test chứng minh bất biến ở §5.3. Kiểm chứng bằng cách trỏ serving vào draft và xác nhận test đỏ.
4. **Rò rỉ qua cache** — hai service account với quyền Ranger khác nhau, cùng endpoint và tham số, phải nhận kết quả khác nhau, và cái thứ hai không được phục vụ bằng cache của cái thứ nhất. Cần Ranger thật.
5. **Rate limit** — chạy qua `check_at(key, now)` với clock tiêm vào, không `sleep`. Hai mốc thời gian trong mọi assertion phải thực sự cách nhau, không bao giờ có thể trùng.
6. **Live check** — `curl` thật vào một endpoint đã publish trên kind, là bước bắt buộc trong plan chứ không phải tùy chọn.

CI chạy đầy đủ ở local trước khi merge, gồm cả `cargo fmt`. Không bao giờ chạy song song hai workspace test suite.

## 10. Ngoài phạm vi v1

Cố ý hoãn, mỗi mục đều được ghi lại để nó là một quyết định chứ không phải một thiếu sót:

- **Ghi (INSERT/UPDATE/DELETE)** và transaction nhiều statement — cần transaction boundary, rollback, audit ghi, và chính sách về việc ai được publish endpoint ghi.
- **Bound parameter ở tầng connector** — xem §3.
- **Engine thực thi Groovy/script** (`ExecuteEngineEnum::SCRIPT` của sqlrest) — bài toán sandbox lớn đổi lấy lợi ích không đáng kể so với SQL.
- **Xuất MCP tool** (`McpToolEntity` của sqlrest) — việc tiếp nối tự nhiên ở v2 khi endpoint đã ổn định.
- **Danh sách IP allow/deny** (`FirewallRulesEntity` của sqlrest) — giải ở tầng ingress tốt hơn trong application code.
- **Import/export hàng loạt định nghĩa endpoint** — đáng có, nhưng không đáng chặn v1.
- **Đưa `dataapi.api_keys` lên `egret-auth`** — key ở lại schema `dataapi` chừng nào Data API còn là bên tiêu thụ duy nhất. Khi có bề mặt thứ hai cần credential cho máy, chúng chuyển lên.
