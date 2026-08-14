# Data API — thiết kế

> **Trạng thái:** P2 (service account + client OAuth2 + route quản trị) ĐÃ SHIP; P3–P5 chưa triển khai · **Ngày:** 2026-08-14 · **Tham chiếu:** [sqlrest](https://github.com/inrgihc/sqlrest) v1.9.0 (chỉ tham chiếu domain model + UX; không tái sử dụng code — sqlrest là Java/Spring, egret là Rust)
>
> *Bản tiếng Việt tham chiếu. Bản chuẩn là [docs/en/specs/2026-08-14-data-api-design.md](../../en/specs/2026-08-14-data-api-design.md).*

## 1. Vấn đề

egret truy vấn được Trino và StarRocks theo kiểu tương tác (SQL Lab, Explore, dashboard), nhưng chưa có cách nào biến một truy vấn đã được quản trị thành **endpoint HTTP cho máy gọi**. Các bên tiêu thụ phía sau — service nội bộ, công cụ low-code, tích hợp đối tác — hiện không có lựa chọn nào ngoài việc dùng thẳng credential của engine, vốn đi vòng qua policy row/column của Ranger và không để lại dấu vết audit theo từng bên tiêu thụ.

Data API lấp khoảng trống đó: thành viên workspace viết một câu SELECT có tham số, publish tại một URL ổn định, client bên ngoài gọi bằng token OAuth2 được mint cho một service account (§6.1). Mọi lời gọi đều chạy dưới một principal thật của egret, nên Ranger và audit vẫn áp dụng nguyên vẹn.

## 2. Các quyết định đã chốt

| Quyết định | Lựa chọn | Lý do |
|---|---|---|
| Cách xây | Crate Rust native trong egret; sqlrest là **tham chiếu** domain model và UX | Một hệ thống, một mô hình quyền, một UI. Chạy sqlrest song song nghĩa là hai identity store, hai datastore, hai UI. |
| Nguồn định nghĩa endpoint | SQL thô + tham số là lõi; thêm lối tắt "Publish as API" để seed SQL từ Saved Query / Dataset | SQL thô là nguyên thủy linh hoạt; lối tắt bỏ được bước copy-paste mà không bó buộc mô hình. |
| Định danh bên tiêu thụ | **Service account = principal thật của egret**, sở hữu các client OAuth2 `client_credentials` trên chính machine-IdP của egret | Query đi qua machine-idp → engine token → Ranger. Một mô hình quyền; audit truy về được một danh tính có tên. |
| Đọc/ghi | **Chỉ đọc (SELECT) ở v1** | Thu hẹp bề mặt tấn công; không cần thiết kế transaction/rollback/idempotency. Ghi để v2. |
| Phạm vi v1 | Version & rollback · rate limit / quota · response cache · OpenAPI tự sinh + Try-it | Chọn cả bốn. |
| Bề mặt phục vụ | Cùng process với `egret-server`, nhưng cấu trúc sao cho tách thành binary riêng được sau | Mặc định modular monolith mà không chặn đường cô lập về sau. |
| Tenancy / URL | Workspace-scoped, workspace nằm trong path: `/data-api/v1/{workspace}/{path}` | Duy nhất toàn cục, dễ debug, khớp với cách Cedar scope theo workspace. |

## 3. Điều kiện tiên quyết — escaping tham số hiện không an toàn cho bề mặt công khai

Có **hai** helper anh em cùng render một giá trị string thành SQL literal, và cả hai đều mù backslash:

- `literal` — `crates/egret-query/src/execution/template.rs:62` (SQL Lab `{{param}}`)
- `sql_literal` — `crates/egret-bi/src/chartquery.rs:135` (giá trị filter của chart và dashboard), mà chính doc comment của nó gọi là "the security-critical primitive"

Cả hai đều làm:

```rust
format!("'{}'", value.replace('\'', "''"))
```

Chúng chỉ nhân đôi dấu nháy đơn, **bỏ qua backslash**.

Đây là sự không nhất quán giữa các helper anh em chứ không phải điểm mù: `like_literal` (`crates/egret-bi/src/schema.rs:161`) và `sr_escape_dq` (`crates/egret-engine-mgmt/src/engines.rs:3062`) đều đã escape `\` đúng. `sql_literal` với tới được bằng quyền BI thấp nhất (`CH_READ`), nên hiện tại nó là chỗ phơi nhiễm nhiều hơn trong hai chỗ. StarRocks theo dialect MySQL và mặc định không bật `NO_BACKSLASH_ESCAPES`, nên `\` là ký tự escape và `\'` thoát ra khỏi literal. Trino không bị ảnh hưởng (nó không xử lý backslash escape trong string literal chuẩn).

Hiện tại lỗ hổng này chỉ với tới được bởi người dùng đã xác thực có quyền SQL Lab. Data API sẽ đưa **chính đường render đó ra Internet công khai sau một credential cho máy**. Vì vậy bản vá thuộc phạm vi v1, không phải việc làm sau.

Unit test hiện có ở cả hai chỗ — `string_param_neutralizes_injection` (`template.rs:93`) và `sql_literal_encodes_by_type` (`chartquery.rs:684`) — đều xanh dù lỗ hổng còn hay đã hết, vì không test nào dùng giá trị có chứa backslash. Chúng không phải bằng chứng.

`Connector::execute` (`crates/egret-query/src/execution/connector/mod.rs:70`) nhận `sql: &str` và không có đường bound parameter, nên không thể sửa bằng cách "cứ dùng prepared statement" mà không đổi trait.

**Bản vá (v1):** một escaper theo dialect duy nhất đặt trong **`egret-query-api`**, tham số hóa theo `Engine`, được gọi bởi cả hai chỗ. Đặt ở contract crate thay vì `egret-query` vì `Engine` vốn đã nằm đó và `egret-bi` cũng phụ thuộc vào nó — như vậy giữ được một implementation duy nhất thay vì hai bản có thể trôi khỏi nhau, mà đó chính là cách sự không nhất quán này phát sinh ngay từ đầu. Crate này là leaf thuần (serde, uuid, utoipa, egret-core), còn escaper là thao tác chuỗi thuần túy, nên không kéo theo gì cả.

- **Trino** — chỉ nhân đôi `'` (hành vi hiện tại, vốn đã đúng cho Trino).
- **StarRocks** — escape `\` → `\\` và `'` → `''`. Nháy đơn được nhân đôi thay vì escape bằng backslash chính là để hai lượt thay thế độc lập với thứ tự: lượt quote không sinh ra backslash nào, lượt backslash không sinh ra quote nào. Nếu dùng dạng `\'` thì lượt backslash bắt buộc phải chạy trước, nếu không nó sẽ phá hỏng phần quote mà lượt kia vừa tạo ra — một sự mong manh đáng được thiết kế cho biến mất thay vì chỉ ghi chú lại.

Vì bản vá nằm trong contract crate dùng chung, SQL Lab **và** filter của chart/dashboard được vá bằng chính thay đổi đó.

Một chỗ thứ ba, `starrocks_create_service_user_sql` (`crates/egret-engine-mgmt/src/engines.rs`), có cùng lỗ hổng và **đã được vá trong cùng nhánh này**. Một bản trước của spec gọi nó là tiềm ẩn với lý do username mà nó nội suy là hằng số; lập luận đó sai, vì nó chỉ xét tới username. Password không phải hằng số: `ensure_delegator_secret` trả về `resolve_plaintext(None, id)` ở mọi lần gọi sau lần đầu tiên, nên thứ được nội suy là giá trị đang lưu trong secret có tên đó, không phải giá trị ngẫu nhiên mà egret sinh ra ban đầu. Một giá trị đã lưu có chứa `\'` sẽ thoát ra khỏi literal, và DDL này chạy với quyền `root` trên StarRocks FE. Giờ nó gọi chính `quote_string`, hàm này tự cung cấp cặp nháy bao ngoài.

**Cố ý hoãn:** bound parameter ở tầng connector. StarRocks (`mysql_async`) hỗ trợ trực tiếp; Trino cần `X-Trino-Prepared-Statement` + `EXECUTE … USING`. Đó là thay đổi trait `Connector` đủ lớn để thành dự án riêng. Escaper theo dialect nằm sau một hàm duy nhất nên thay thế được ở một chỗ.

**Tham số là giá trị, không bao giờ là định danh.** `{{param}}` không được xuất hiện ở vị trí tên cột, tên bảng hay `ORDER BY`. Cột động được biểu diễn bằng tham số kiểu `enum` với allow-list tường minh, render bằng cách tra giá trị trong danh sách đó. Allow-list là dạng an toàn duy nhất cho định danh.

## 4. Kiến trúc

### 4.1 Các crate

| Crate | Vai trò |
|---|---|
| `egret-dataapi-api` | Contract crate lá, pure-serde: DTO định nghĩa endpoint, param spec, published snapshot. Dùng được bởi `egret-server` và frontend codegen mà không kéo theo sqlx/axum. |
| `egret-dataapi` | Seam crate: store (sở hữu schema `dataapi`), vòng đời publish, service account + các client OAuth2 của chúng, rate limit, cache, access log, **hai router**. |

### 4.2 Hai router, tách rời có chủ đích

- `management_router()` → nest dưới `/api` như mọi seam khác. Session cookie + CSRF + Cedar. Soạn thảo, publish, quản lý service account và client.
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

dataapi.service_accounts     -- ràng buộc workspace cho một machine principal
  user_id uuid PK (→ users.id ON DELETE CASCADE),
  workspace_id, name, description, created_by, created_at
  UNIQUE (workspace_id, name)

dataapi.clients              -- client OAuth2 client_credentials (P2, §6.1)
  id, service_account_id (→ dataapi.service_accounts.user_id ON DELETE CASCADE),
  name, client_id text UNIQUE, client_secret_hash text,
  expires_at, revoked_at, last_used_at, created_by, created_at

dataapi.access_log           -- một hàng mỗi lần gọi
  id, ts, endpoint_id, version, service_account_id, client_id,
  status_code, row_count, duration_ms, cache_hit bool,
  engine_query_id, client_ip, error_code
```

**Vì sao ràng buộc workspace là bảng riêng chứ không phải một cột trên `users`.** `users` ở tầng platform, và mọi bên tiêu thụ khác của nó — trang Users của admin, principal picker của Access console, `find_by_id` để hiển thị — đều sẽ phải học cách bỏ qua một cột workspace vô nghĩa với con người. Ràng buộc đó nằm ở `dataapi.service_accounts`, còn `users` chỉ nhận đúng phần bắt buộc: `provider = 'service'`, cộng một `CHECK` rằng hàng service không bao giờ mang `password_hash`. Handler login vốn đã đòi có password_hash, nhưng nó chỉ cách sai vĩnh viễn đúng một code path; ràng buộc đặt ở chỗ không thể quên.

`dataapi.clients` được nắn theo đúng hình dạng `catalog.catalog_clients` (0053) — cùng ý tưởng, cùng machine-IdP, audience thứ hai. N client mỗi service account, nên xoay vòng là mint-mới-rồi-thu-hồi-cũ chứ không phải một cửa sổ hỏng-rồi-sửa.

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
{ "per_client_qps": 5, "burst": 10, "per_endpoint_qps": 50 }
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

`dataapi.endpoints.folder_id` và `dataapi.service_accounts.user_id` / `.workspace_id` là **FK xuyên schema** trỏ vào `public.folders(id)`, `public.users(id)` và `public.workspaces(id)`. Điều này hơi nghịch với nguyên tắc mỗi seam sở hữu schema riêng, nhưng `folders` và `users` vốn đã là bảng dùng chung xuyên suốt, và nhân bản chúng còn tệ hơn. Chấp nhận và ghi nhận tại đây.

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

Publish và rollback sẽ được phát ra dưới dạng sự kiện `egret-audit` — không thêm bảng. **Cấp client và thu hồi client cũng thuộc P3, và hiện chưa được phát ra**: các route quản trị P2 đã tồn tại và đã mint credential, nhưng `DataApiState` không mang audit sink nào, nên việc cấp hay thu hồi một client hiện không để lại bản ghi audit. Đây là một khoảng trống được nói rõ, không phải chi tiết để đọc lướt — cho tới khi P3 đấu sink vào, dấu vết duy nhất về vòng đời của một credential là chính dòng `dataapi.clients` (`created_by`, `created_at`, `revoked_at`, `last_used_at`).

**Cảnh báo vận hành phải hiện lên UI:** publish endpoint **không** cấp quyền dữ liệu. Vì query chạy dưới danh nghĩa service account với Ranger enforcement, admin còn phải cấp quyền cho chính service account đó trong Ranger. Endpoint publish mà thiếu bước này sẽ lỗi permission từ engine, và lỗi đó phải được map thành thông báo nói rõ điều này thay vì để lộ raw engine error.

## 6. Xác thực và luồng request

### 6.1 Credential: OAuth2 `client_credentials`, không phải API key tự chế

Một bản trước của spec này quy định một key tự chế, `egt_<prefix>_<secret>`, hash bằng SHA-256 và so sánh ở mỗi request. **Đó không phải thứ đã được xây, và lý do từ bỏ nó đáng được nói rõ**: egret vốn đã chạy một machine-IdP (`egret-machine-idp`) mint JWT có chữ ký cho các client của Iceberg catalog nhúng. Bịa ra một định dạng credential thứ hai đồng nghĩa với hai issuer, hai câu chuyện thu hồi, hai chỗ phải làm đúng so sánh constant-time, và không tương thích với bất kỳ thư viện OAuth2 nào.

Thứ thực sự ship là **một audience thứ hai trên chính IdP sẵn có**:

- Mỗi service account sở hữu **N client** trong `dataapi.clients`. Mỗi client gồm `client_id` (`egret-dataapi-<uuid>`) và `client_secret` 32 byte ngẫu nhiên, base64url. Secret chỉ hiện đúng một lần lúc tạo và không lấy lại được — DTO danh sách của management API **không hề có** trường secret, không phải là trường bị skip hay Option.
- Client đổi cặp đó tại `POST /oauth/token` (`grant_type=client_credentials`) lấy JWT với `aud: "data-api"`, kèm claim `ws` và `egret_user_id`. `ClientStore` của `egret-dataapi` implement đúng trait `ClientVerifier` mà registry của Catalog đang implement; IdP giữ một danh sách verifier và các audience không biết gì về nhau.
- Mọi kiểu thất bại khi verify — `client_id` lạ, đã thu hồi, đã hết hạn, principal bị vô hiệu hoá, workspace đã archive, sai secret — đều trả về *cùng một response*. Một lỗi phân biệt được sẽ là oracle miễn phí cho bất kỳ ai dò client id. **Hai lưu ý, vì "cùng một response" hẹp hơn "không phân biệt được".** Thứ nhất, *thời gian* khác nhau: chỉ request đi được tới bước sai secret mới phải trả chi phí verify Argon2, nên một secret sai trên `client_id` có thật sẽ chậm hơn một cách đo được so với mọi kiểu từ chối trả về trước khi hash — và điều đó phân biệt được "client id này tồn tại" với "không tồn tại". Muốn cân bằng lại thì phải verify một dummy hash trên các nhánh trả về sớm, và P2 cố ý chưa làm. Thứ hai, *hash lưu bị hỏng* là thất bại duy nhất không phải `Ok(None)`: `PasswordHash::new` lỗi, `verify` trả `Err`, và caller nhận 500 chứ không phải một lỗi xác thực.

**Hash là Argon2, và phản đối trước đây với nó không còn đúng nữa.** Phản đối đó là trả ~50–100 ms KDF trên *mỗi request API* là không chấp nhận được — điều đó đúng, và chính vì thế KDF không nằm trên đường đó. Secret được verify **một lần mỗi token**, không phải một lần mỗi lời gọi; serving surface sau đó chỉ verify chữ ký JWT. Argon2 tốn vài chục mili-giây mỗi 10 phút cho mỗi client, và khớp với `CatalogStore::verify_client` thay vì đẻ ra một cơ chế hash thứ hai.

**Đánh đổi về thu hồi, nói thẳng.** Một token stateless không thể bị thu hồi trước khi hết hạn. Thu hồi client sẽ set `revoked_at`, nên không token *mới* nào mint được nữa, nhưng token đã cấp vẫn còn hiệu lực tới lúc hết hạn. `DATA_API_TTL_SECS = 600` chính là cận đó: sau khi thu hồi, cửa sổ phơi nhiễm tối đa là 10 phút. Thu hồi tức thì đã được cân nhắc và bị loại — nó đòi một lần tra cứu mỗi request, tức trả lại đúng phần throughput mà thiết kế này được chọn để có.

**Phải nói chính xác một "công tắc tắt" mua được gì, vì chỗ này rất dễ nói quá.** Có bốn thao tác chặn được một client bị lộ mint *thêm* token, và `verify` tôn trọng cả bốn (mỗi thao tác đều có test trong `clients_store.rs`): thu hồi client (`revoked_at`), vô hiệu hoá principal (`users.status <> 'active'`), archive workspace (`workspaces.status <> 'active'`), và xoá service account (cascade xuống các client của nó). **Không thao tác nào thu hồi được token đã cấp** — serving surface verify JWT theo kiểu stateless, đó chính là toàn bộ ý nghĩa của §6.2 bước 1. Vì vậy TTL 600s là một cận không rút ngắn được, không phải trường hợp xấu nhất mà một thao tác nhanh hơn có thể phá.

**Vì sao `verify` phải đọc `workspaces.status`, và không phải chỉ cho gọn.** Archive một workspace là thao tác ngừng hoạt động tự nhiên nhất của người vận hành, mà mọi route quản trị Data API đều đi qua `WsContext` — thứ trả 404 với workspace không còn active. Nghĩa là archive *xoá mất chính những route có thể thu hồi client hoặc vô hiệu hoá principal của nó*. Nếu không kiểm trạng thái workspace, archive sẽ để lại mọi credential trong workspace đó vừa **còn sống vừa không thu hồi được** — người vận hành làm đúng cái thao tác trông giống "tắt hết" nhất, và nhận về điều ngược lại. Vì lúc đó bề mặt khắc phục đã biến mất, chính bước xác thực phải tôn trọng trạng thái workspace; không còn chỗ nào khác làm được.

Vô hiệu hoá principal là thao tác gọi được qua API: `PATCH /api/control-plane/users/{id}` với `{"status":"disabled"}` chấp nhận service account. Chỉ các trường *hồ sơ* của route đó (tên/email) mới từ chối hàng `provider = 'service'` — nhánh status thì không, đúng vì nó chính là công tắc tắt này, và nó vẫn áp dụng kiểm lại quyền `user:disable`, guard platform admin cuối cùng, và thu hồi session ngay lập tức.

**Ai mint được credential, và bản nâng cấp bật sẵn những gì.** `service_account:read` và `service_account:manage` được seed vào role dựng sẵn `ws_admin` (migration 0120) và dừng ở đó — không xuống `ws_editor`, vì service account là một principal máy mà admin sau đó cấp quyền dữ liệu cho trong Ranger, tức một hành vi quản trị chứ không phải biên tập. Phải nói thẳng hệ quả thay vì để người khác tự phát hiện: `ws_admin` là một hàng role toàn cục duy nhất, nên migration đó chạm tới **mọi assignment `ws_admin` đang có, trong mọi workspace đang có, cùng một lúc**, và feature module `data_api` coi việc thiếu hàng entitlement là đang bật — tức là mọi workspace có trước nó. Vậy nên ngay khi nâng cấp, mọi workspace admin hiện tại đều mint được credential máy cho workspace của mình, không cần thao tác nào từ người vận hành và không cần opt-in theo từng workspace. Phạm vi đó là có chủ ý, nhưng nó xuất hiện lúc nâng cấp chứ không phải do ai bật lên, nên được công bố ở đây và trong comment của migration. Người vận hành nếu không muốn thì phải hoặc ghi một hàng entitlement `data_api` tắt module cho các workspace đó, hoặc gỡ hai quyền này khỏi `ws_admin`.

`client_credentials` không có refresh token, đúng theo thiết kế: client cứ xin lại bằng credential của mình. Mọi thư viện OAuth2 đều đã làm vậy sẵn.

**Bề mặt quản trị** (P2, scope theo workspace, session + CSRF như mọi nhóm `/api/ws/…` khác):

```text
GET    /api/ws/{workspace}/data-api/service-accounts
POST   /api/ws/{workspace}/data-api/service-accounts
DELETE /api/ws/{workspace}/data-api/service-accounts/{id}
GET    /api/ws/{workspace}/data-api/service-accounts/{id}/clients
POST   /api/ws/{workspace}/data-api/service-accounts/{id}/clients   -> secret, đúng một lần
DELETE /api/ws/{workspace}/data-api/clients/{id}
```

Hai permission, đều `Scope::Workspace` và đều chỉ seed cho `ws_admin` (migration 0120): `service_account:read` để xem account và client, `service_account:manage` để tạo, xoá và **cấp client**. Cấp client gate trên `:manage`, không bao giờ trên `:read` — client là credential bearer cho quyền truy cập dữ liệu của account đó, nên phát nó ra mạnh hơn hẳn việc đọc danh sách. Mọi handler đều resolve account (hoặc client) theo workspace lấy từ extractor `WsContext`, không bao giờ đọc lại path segment lần hai, và trả **404 chứ không phải 403** khi không khớp — đúng luật như §6.2 bước 3.

### 6.2 Chuỗi xử lý mỗi request

```text
GET /data-api/v1/{ws}/{path}
 │
 1. verify JWT qua JWKS → principal + workspace từ claims        ✗→ 401
 2. (ws, method, path) → endpoint → live_version_id → snapshot   ✗→ 404
 3. workspace của service account == {ws} trong path             ✗→ 404
 4. Cedar: dataapi:invoke trên Object data_api_endpoint:{id}     ✗→ 403
 5. rate limit (theo endpoint và theo client)                    ✗→ 429 + Retry-After
 6. cache lookup                                                 hit → trả
 7. render params → SQL an toàn theo dialect                     ✗→ 400
 8. machine-idp mint engine token cho SA
    → QueryExecPort::execute_as
 9. Ranger lọc row/column ở engine, dưới danh nghĩa SA
10. shape output → lưu cache → access_log → trả
```

**Bước 1 là stateless và không tốn round-trip DB nào.** Bearer token là JWT do chính machine-IdP của egret ký, nên serving path chỉ verify chữ ký với JWKS rồi đọc thẳng `egret_user_id` và `ws` từ claim — không có tra cứu credential, và không có Argon2, trên hot path. Đó chính là lý do §6.1 chọn đổi token thay vì key kiểm mỗi request. Cái giá là cửa sổ thu hồi mà §6.1 chặn ở 600s.

Bước 3 trả **404 chứ không phải 403** là cố ý: một token thuộc workspace khác không được phép phân biệt "không có endpoint đó" với "có, nhưng anh không được gọi". Management API áp dụng đúng luật đó cho service account và client (§6.1).

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

**Không bao giờ trả raw engine error cho caller bên ngoài.** Lỗi Trino/StarRocks mang theo tên bảng, tên cột và đôi khi cả thông tin host — trên một endpoint công khai sau một credential cho máy thì đó là rò rỉ metadata. Map sang code cố định và ghi chi tiết đầy đủ vào log nội bộ kèm `request_id` để support tra được.

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
- Publish, rollback, cấp client và thu hồi client sẽ là sự kiện `egret-audit`. Cả bốn đều thuộc **P3** — chưa cái nào được phát ra tính đến P2 (§5.3).

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
- **Đưa `dataapi.clients` lên `egret-auth`** — registry client ở lại schema `dataapi` chừng nào Data API còn là bên tiêu thụ duy nhất. Bản thân *giao thức* thì đã dùng chung sẵn: `egret-machine-idp` verify cả registry này lẫn của Catalog qua một trait `ClientVerifier`. Khi có bề mặt thứ ba cần credential cho máy, phần lưu trữ cũng chuyển lên.
