# Phân quyền tính năng theo workspace — Thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn:
> [`docs/en/specs/2026-07-23-workspace-feature-entitlements-design.md`](../../en/specs/2026-07-23-workspace-feature-entitlements-design.md)

## 1. Mục tiêu

Cho phép platform admin quyết định **workspace nào được dùng module nào**:

- workspace A — đầy đủ;
- workspace B — chỉ BI, không có Notebook;
- workspace C — bất kỳ tổ hợp nào khác của năm module.

Đây là **entitlement ở mức tenant**, cố ý tách khỏi hai cơ chế egret đã có:

| Cơ chế | Trả lời câu hỏi | Nằm ở đâu |
| --- | --- | --- |
| RBAC role (`ws_viewer/editor/admin`) + Cedar PDP | *User này có được làm hành động này không?* | `access_grants`, `egret-auth/pdp` |
| Ranger / classification policy | *User này có được thấy dữ liệu này không?* | data plane |
| **Feature entitlement (tài liệu này)** | *Workspace này có module này không?* | `workspace_features` |

Hai trục độc lập và kết hợp theo AND: `ws_editor` trong workspace tắt `notebooks`
thì không có notebook; `ws_viewer` trong workspace bật `notebooks` thì có
notebook ở chế độ chỉ đọc.

## 2. Phạm vi

**Feature = module**, mỗi module đúng một công tắc, năm key:

`sql_lab`, `datasets`, `charts`, `dashboards`, `notebooks`

Mức đọc/ghi vẫn thuộc về RBAC — feature không có trạng thái "chỉ đọc".

**Chỉ `platform_admin` bật/tắt**, tại Control Plane → Workspaces → chi tiết →
*Features*. `ws_admin` không được; entitlement do platform cấp cho tenant, không
phải do tenant tự chọn. Khớp mô hình ba tầng của ADR-19.

**Vắng bản ghi nghĩa là bật.** Không backfill; mọi workspace đang chạy giữ nguyên
hành vi. Chỉ ghi row khi admin thay đổi.

### 2.1 Ngoài phạm vi

- preset gói (`Full`, `BI-only`, …) — công tắc là nguyên thuỷ, preset chỉ là sugar
  ở UI, thêm sau không cần đổi schema;
- `ws_admin` tự thu hẹp;
- thu hồi tài nguyên đang chạy khi tắt (kill kernel pod, huỷ runtime token, dừng
  job định kỳ);
- bốn mục nav placeholder (`workflows`, `alerts`, `data_api`, `pipelines`) — hiện
  render `<Placeholder>` nên không có gì để gate; sẽ có key khi thành module thật;
- override theo từng user.

Dữ liệu của module bị tắt được **giữ nguyên** — chỉ là không truy cập được, bật
lại là có lại.

### 2.2 Platform admin KHÔNG bypass

`platform_admin` bypass mọi kiểm tra quyền khác trong egret, và điều đó đúng:
quyền mô tả *một người* được làm gì. Entitlement mô tả *một workspace* là gì, nên
áp dụng cho tất cả mọi người trong workspace đó, kể cả admin. Workspace B tắt
notebook thì platform admin vào workspace B cũng không thấy notebook.

Lối thoát là bật lại — hai cú click, có audit. Miễn trừ ngầm cho admin sẽ khiến
công tắc chỉ có nghĩa "ẩn với phần lớn mọi người", không đúng như nó tuyên bố.

## 3. Mô hình dữ liệu

Migration `0094_workspace_features.sql`:

```sql
CREATE TABLE workspace_features (
  workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  feature      TEXT        NOT NULL,
  enabled      BOOLEAN     NOT NULL,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by   UUID        REFERENCES users(id) ON DELETE SET NULL,
  PRIMARY KEY (workspace_id, feature)
);
```

- `feature` là `TEXT`, hợp lệ hoá theo catalog trong code thay vì enum Postgres,
  nên thêm module sau này không cần migration.
- `ON DELETE CASCADE` theo workspace; `ON DELETE SET NULL` cho `updated_by`, nhất
  quán với cách migration 0093 xử lý các cột attribution.
- Row được giữ lại khi bật lại (`enabled = true`) để `updated_by` / `updated_at`
  còn vết ai đã bật.

Vì thiếu row = bật, bảng rỗng ở lần deploy đầu và tính năng nằm im cho tới khi có
người dùng đến.

## 4. Catalog trong code

Module mới `crates/egret-workspace/src/features.rs` — nguồn sự thật duy nhất cho
middleware, response API, danh sách UI, và test phủ route. Cùng cách permission
catalog được định nghĩa trong code (`#174`).

```rust
/// Một bề mặt HTTP bị gate: path prefix dưới `/api/ws/{workspace}/`, kèm các
/// method nó áp dụng (`None` = mọi method).
pub struct GateRule {
    pub prefix: &'static str,
    pub methods: Option<&'static [Method]>,
}

pub struct Feature {
    pub key: &'static str,          // "notebooks"
    pub label: &'static str,        // "Notebooks"
    pub description: &'static str,  // hiển thị trong tab Features
    pub rules: &'static [GateRule],
}

pub const FEATURES: &[Feature] = &[ /* năm mục bên dưới */ ];

/// Các path prefix ws-scoped không bao giờ bị gate. Mọi prefix dưới
/// `/api/ws/{workspace}/` phải xuất hiện ở một `Feature` hoặc ở đây — xem test
/// phủ route ở §8.1.
pub const UNGATED: &[&str] = &[ /* … */ ];
```

### 4.1 Bản đồ

| Feature key | Bề mặt bị gate dưới `/api/ws/{workspace}/` |
| --- | --- |
| `sql_lab` | `sqllab/**` (mọi method) và `POST /queries` (submit SQL tự do) |
| `datasets` | `datasets/**` |
| `charts` | `charts/**` |
| `dashboards` | `dashboards/**` |
| `notebooks` | `notebooks/**`, `kernel/**`, `notebook-runtime/**`, `notebook-settings/**` |

Không bao giờ gate: `members`, `roles`, `shares`, `objects`, `folders`, `engines`,
`databases`, `access-requests`, và phần không-submit của `/queries` —
`GET /queries`, `GET /queries/{id}`, `GET /queries/{id}/results`,
`GET /queries/{id}/results.csv`, `POST /queries/{id}/cancel`.

Lý do chia `/queries` theo method: hành vi đặc quyền là **submit** SQL, không phải
đọc lại hay huỷ query của chính mình. Tác giả chart cần đường đọc lại (§6), gate
nó sẽ làm hỏng chart vì một lý do chẳng liên quan gì tới SQL Lab.

`folders` và `objects` không gate vì chúng trung lập với loại nội dung (một folder
chứa cả chart lẫn notebook; `objects` là bề mặt chia sẻ). Việc phân quyền theo
từng object của chúng vốn đã đi qua PDP.

## 5. Cưỡng chế — đặt trong `WsContext`

Kiểm tra nằm trong `WsContext::from_request_parts`
(`crates/egret-workspace/src/context.rs`), là bước mới giữa "load workspace đang
active" (bước 3) và "giải quyết membership" (bước 4):

```text
3.  Load workspace active theo slug.
3.5 Suy ra module của request từ `parts.uri.path()`.
    Nếu module đó tắt với workspace này → Error::FeatureDisabled(key).
4.  Membership / role (giữ nguyên).
```

Thứ tự quan trọng: kiểm tra feature chạy **trước** membership, để thành viên của
workspace tắt notebook và người ngoài đều nhận 403 giống nhau, không ai suy ra
thêm thông tin gì.

### 5.1 Vì sao ở đây thay vì một `.layer()`

Mọi route ws-scoped đều đã dùng `WsContext` — đã xác minh trên `egret-bi` (58 chỗ),
`egret-query` (execution + registry routes), `egret-notebook` (kể cả `runtime.rs`),
`egret-kernel`, và `egret-workspace`. Chúng *buộc* phải dùng: đó là cách duy nhất
để giải slug `{workspace}` thành workspace id.

Điều đó cho hai tính chất mà một layer riêng không có:

1. **Phủ sóng do cấu trúc.** Một route ws-scoped mới không thể tồn tại nếu không
   có `WsContext`, nên không thể nằm ngoài gate. Với layer, đảm bảo này phụ thuộc
   vào việc nhớ mount đúng chỗ — mà `egret-bi` mount một router chứa chung charts,
   dashboards, datasets và folders, nên layer per-router vẫn phải khớp path.
2. **Không tra cứu trùng.** `WsContext` đã giải workspace rồi; layer sẽ phải giải
   slug lần nữa.

Cái giá là `WsContext` giờ làm hai việc. Chấp nhận được: cả hai đều là "request này
có được chạm vào workspace này không", và phương án còn lại tệ hơn.

### 5.2 Khớp path

Lấy phần đuôi sau `/api/ws/{slug}/`, khớp prefix **dài nhất** với catalog (để
`sqllab/queries` được giải trước `sqllab`), rồi kiểm tra tập method. Prefix không
khớp thì tra trong `UNGATED`; thứ không nằm ở cả hai là bug bị bắt lúc test (§8.1),
không phải lúc chạy — lúc chạy, prefix không khớp được cho qua, vì fail-closed trên
route chưa khai báo sẽ biến một thiếu sót catalog thành sự cố production.

### 5.3 Hình dạng lỗi

Variant mới `Error::FeatureDisabled(&'static str)` trong `egret-core/src/error.rs`,
ánh xạ sang **403**.

`ErrorBody` (`egret-core/src/http.rs`) thêm một mã máy-đọc-được tuỳ chọn để
frontend phân biệt với lỗi phân quyền thông thường:

```rust
pub struct ErrorBody {
    pub error: String,                    // "feature disabled: notebooks"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,             // "feature_disabled"
}
```

Trường này là bổ sung và bị bỏ qua với mọi lỗi hiện có, nên không response nào
đang chạy bị đổi hình dạng.

### 5.4 Chi phí

Thêm một query có index (`SELECT feature FROM workspace_features WHERE
workspace_id = $1 AND enabled = false`) mỗi request ws-scoped. `WsContext` vốn đã
chạy hai query nên đây là cùng bậc. **Chưa cache ở v1** — chỉ thêm TTL cache kèm
invalidate khi ghi nếu đo thấy cần.

## 6. Tách đường chạy chart khỏi SQL tự do

`ChartExplore.tsx` hiện build SQL từ `query_spec` của chart rồi submit qua endpoint
chung `POST /api/ws/{workspace}/queries`. Nên gate endpoint đó theo `sql_lab` sẽ
giết luôn chart builder — một module chẳng liên quan.

Cách sửa, nằm trong module `charts`:

- **Mới** `POST /api/ws/{workspace}/charts/run`, body `{dataset_id, query_spec,
  limit}`. Nó thực hiện bước build sẵn có **ở server** rồi submit SQL kết quả cho
  executor, trả về đúng ack `{id, status}`. SQL thô không bao giờ đi qua dây khi
  tác giả chart.
- Trạng thái và kết quả vẫn dùng `GET /queries/{id}` và `GET /queries/{id}/results`,
  vốn không bị gate (§4.1).
- Nhờ đó `POST /queries` trở thành SQL tự do và chỉ vậy, và gate nó theo `sql_lab`
  là trung thực.

Việc này cũng bịt một lỗ có sẵn: hôm nay ai tạo được chart là submit được SQL tuỳ ý
qua chính endpoint đó.

## 7. API và frontend

### 7.1 API control-plane

Thêm vào `admin_workspace_router` (`crates/egret-workspace/src/routes.rs`), dùng
lại đúng các permission platform-scope mà route anh em đang dùng — `workspace:read`
cho đọc và `workspace:update` cho ghi, cả hai đã có trong catalog code
(`crates/egret-core/src/rbac.rs`). Không thêm permission key mới: bật/tắt một module
cùng hạng với đổi tên hay archive workspace, và hiện chỉ `platform_admin` giữ chúng
ở platform scope.

- `GET /api/admin/workspaces/{id}/features` →
  `[{key, label, description, enabled}]` — đủ năm, tính bằng
  *catalog trừ đi các row disabled*.
- `PUT /api/admin/workspaces/{id}/features`, body
  `{"features": {"notebooks": false, "sql_lab": true, …}}` — thay toàn bộ năm key.
  Key lạ → `400`. Upsert trong một transaction, rồi emit `AuditEvent`
  (`action = "workspace.features.update"`, `resource_type = "workspace"`, `detail`
  là map before/after) qua `AuditSink` sẵn có.

### 7.2 API phía thành viên

`GET /api/me/workspaces` (`list_my_workspaces`) — `WorkspaceDto` thêm
`features: Vec<String>`, là các key đang bật của workspace đó. Một query gộp cho cả
danh sách, không phải mỗi workspace một query.

### 7.3 Frontend

- **`components/layout/Sidebar.tsx`** — `NavItem` thêm `feature?: string`;
  `EXPLORE_GROUPS` được lọc theo feature của workspace hiện tại, group nào rỗng thì
  bỏ cả heading. Dùng lại đúng cách lọc đã viết cho nhóm control-plane.
- **`App.tsx` / `ExploreRoutes`** — route bị gate được bọc bằng guard
  `<RequireFeature name="…">`, render route `Forbidden` sẵn có. Redirect
  `<Route index>` phải trỏ tới **module bật đầu tiên** thay vì hard-code `sql-lab`,
  nếu không workspace tắt SQL Lab sẽ rơi vào trang bị chặn.
- **`ChartExplore.tsx`** — `run()` gọi `/charts/run` thay cho `useSubmitQuery`;
  poll trạng thái/kết quả giữ nguyên.
- **Control Plane → Workspaces → chi tiết → tab Features** — năm hàng `Switch` kèm
  label + mô tả, nút Save, toast, theo pattern bảng cấu hình đã lập ở `#328`.
- **API client** — `403` mang `code: "feature_disabled"` thì render trang Forbidden
  thay vì toast lỗi chung chung. Việc này xử lý tình huống admin tắt module trong
  lúc user đang mở tab.

## 8. Kiểm thử

### 8.1 Test phủ route (quan trọng nhất)

Một test Rust đọc `openapi.json` sinh ra, gom mọi path prefix khác nhau dưới
`/api/ws/{workspace}/`, và khẳng định mỗi prefix hoặc thuộc một `Feature` hoặc nằm
trong `UNGATED`. Thêm route module mới mà không khai báo thì CI đỏ — đây chính là
đảm bảo mà cách kiểm tra trong từng handler phải làm thủ công mới có.

### 8.2 Test tích hợp (sqlx, trên database test)

- Tắt `notebooks` → `GET /ws/{s}/notebooks` trả `403` với `code = "feature_disabled"`,
  còn `GET /ws/{s}/charts` vẫn `200`. Bật lại → `200`.
- Tắt `sql_lab` → `POST /queries` trả `403`, nhưng `POST /charts/run` trả `200` và
  `GET /queries/{id}/results` của lần chạy đó trả `200`.
- `platform_admin` nhận cùng `403` như mọi người (§2.2).
- `PUT …/features` với key lạ → `400`, và không ghi gì.
- Tắt module không đụng tới dữ liệu: đếm notebook trước khi tắt, bật lại, đếm lại.

### 8.3 Frontend (vitest)

- Sidebar bỏ mục của feature đã tắt và bỏ luôn group rỗng.
- `RequireFeature` render Forbidden cho module đã tắt.
- Redirect index chọn module bật đầu tiên.

### 8.4 End-to-end (Playwright)

Một spec: platform admin tắt Notebooks của một workspace → thành viên workspace đó
không thấy mục Notebooks trên nav, và vào thẳng URL notebook thì bị chặn.

## 9. Triển khai

Bổ sung và đảo ngược được. Bảng mới rỗng, vắng row = bật, nên deploy migration
không đổi gì quan sát được. Thay đổi hành vi duy nhất ship vô điều kiện là việc tách
đường chạy chart (§6), đã được phủ bởi test chart sẵn có cộng §8.2.

Thứ tự làm: catalog + migration + repo → gate trong `WsContext` + hình dạng lỗi →
`/charts/run` + đổi `ChartExplore` → API control-plane → tab Features → guard
sidebar/route → tests.
