# Phase 3c-3 — Chart (Bảng + Bảng Pivot)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-19-phase-3c-3-chart-design.md`.

## Mục tiêu

Cho phép người dùng tạo, lưu và xem **chart** trên một dataset, hiển thị dưới dạng **Bảng
(Table)** hoặc **Bảng Pivot (Pivot Table)**. Một chart tiêu thụ lớp ngữ nghĩa (semantic
layer) đã xây ở 3c-2/3c-2.2:

- **Metrics** → measures (biểu thức tổng hợp trong `SELECT`).
- **Columns / calculated columns** → dimensions (`GROUP BY` hoặc `SELECT` thô).

Giai đoạn này chủ ý chỉ làm trực quan hoá **dạng bảng**. Các loại biểu đồ ECharts
(bar/line/pie/big-number), gom nhóm theo thời gian (time-grain), và định dạng nâng cao được
hoãn sang **3c-3.2**, dùng lại đúng cùng một pipeline truy vấn.

Đã đối chiếu với **Apache Metabase** (UX bảng/pivot — chuẩn mực) và **Apache Superset** (mô
hình `QueryObject → SQL`). Các quyết định kế thừa và nguồn gốc được chú thích trực tiếp.

## Phạm vi

**Trong phạm vi:**

- Bảng `charts` + `ChartRepo` CRUD (migration `0014_chart.sql`).
- Một **bộ dựng truy vấn (query builder)** thuần, có unit-test đầy đủ (`chartquery.rs`):
  `QuerySpec → SQL`, hỗ trợ chế độ tổng hợp (aggregate) vs bản ghi thô (raw-records), bộ
  lọc `WHERE`, `ORDER BY`, `LIMIT`, và `ROLLUP` (cho subtotal/grand-total của pivot).
- Một **bộ mã hoá literal SQL** theo kiểu dữ liệu cho giá trị bộ lọc (quan trọng về bảo
  mật — connector nhận chuỗi SQL thô, không có tham số bind).
- REST: CRUD chart + endpoint `charts/data` dựng SQL rồi đẩy vào `JobManager` hiện có (dùng
  lại thực thi bất đồng bộ + polling).
- Frontend: trang **Explore** (bảng điều khiển + xem trước trực tiếp), bộ render **Bảng**
  (`@tanstack/react-table`), bộ render **Bảng Pivot** (tự cross-tab trên các dòng `ROLLUP`),
  và **danh sách Charts** (thay placeholder `Charts.tsx`).

**Ngoài phạm vi (→ 3c-3.2 trừ khi ghi chú khác):**

- Trực quan hoá ECharts (bar/line/pie/big-number).
- Gom nhóm theo thời gian (`date_trunc` ngày/tuần/tháng).
- Bộ lọc trên metric (`HAVING`) — giai đoạn này chỉ lọc theo cột (`WHERE`).
- Định dạng có điều kiện/heatmap, kéo-thả sắp cột, đóng băng cột, mini-bar, drill-through.
- `WHERE`/`HAVING` SQL thô truyền thẳng (rủi ro injection — cố ý không bao giờ hỗ trợ).
- Toán tử lọc nâng cao (`ILIKE`, `BETWEEN`, thời gian tương đối), series-limit / "top-N +
  Other".

**Không thêm crate mới** — mở rộng `egret-bi` (như 3c-2 / 3c-2.2).

## Bối cảnh: kế thừa gì từ Metabase và Superset

**Superset (`superset/models/helpers.py`, `superset/common/query_object.py`):**

- `need_groupby = có metrics HOẶC có groupby` chọn chế độ **aggregate** vs **raw-records**
  (Superset dòng ~3074). Ta dùng `need_groupby = !metrics.is_empty()`.
- **Lọc theo cột → `WHERE`; lọc theo metric → `HAVING`** (nếu không, `WHERE SUM(x)=5` là SQL
  sai). MVP chỉ hỗ trợ lọc theo cột; lọc theo metric hoãn lại.
- **`GROUP BY` dùng nguyên cột/biểu thức, không dùng alias trong `SELECT`** — bắt buộc để
  dimension là calculated-column gom nhóm đúng trên Trino/StarRocks.
- Superset pivot **sau truy vấn** (pandas `pivot_table` với `margins`). Ta thay bằng đẩy
  tổng vào SQL qua `ROLLUP` (xem §4) để mọi aggregate (kể cả `AVG`/`MIN`/`MAX`) đều đúng ở
  tầng engine.
- Bộ toán tử lọc khởi đầu: `= != > >= < <= LIKE IN NOT IN IS NULL IS NOT NULL`.

**Metabase (`frontend/src/metabase/visualizations/`):**

- Bảng của Metabase dựng trên **TanStack Table** + ảo hoá (virtualization), cho sẵn
  click-để-sắp-xếp, đổi kích thước cột, ẩn/hiện cột, phân trang với chi phí thấp. Ta dùng
  `@tanstack/react-table` (mẫu data-table của shadcn) để có cảm giác tương tự.
- Tính năng bảng "cảm giác đẹp" cho MVP: **sắp xếp, đổi kích thước, ẩn/hiện cột, định dạng
  số/ngày**. Các tính năng nặng hơn (luật heatmap, kéo-thả cột, đóng băng, mini-bar,
  drill-through) là thứ làm Metabase sâu — đều hoãn.
- Điểm nổi bật của pivot là **nhóm dòng có thể thu/mở + subtotal/grand-total**. Metabase tính
  tổng bằng nhiều subquery powerset ở backend; ta dùng một truy vấn `ROLLUP` duy nhất.

## Kiến trúc

```
Trang Explore (frontend)
  │  dựng QuerySpec (metrics, dimensions, columns, filters, orderby, row_limit, rollup)
  ▼
POST /api/ws/{ws}/charts/data  { dataset_id, query_spec }
  │
  ├─ nạp dataset (nguồn + cột + metric)                 [DatasetRepo]
  ├─ kiểm tra tên trong QuerySpec so với dataset        [chartquery]
  ├─ dựng SQL                                           [chartquery::build]
  └─ trả { sql, database_id }                           [egret-bi]
  ▼
Frontend gửi sql → POST /api/ws/{ws}/queries → JobManager   (endpoint sẵn có)
  ▼
Frontend poll GET /api/ws/{ws}/queries/{id}/status + /results   (endpoint sẵn có)
  ▼
Render: Bảng (tanstack) | Pivot (cross-tab trên dòng ROLLUP)
```

CRUD chart (bảng `charts`) độc lập với thực thi: một chart đã lưu chứa `viz_type` + `params`
(`QuerySpec` cộng cách chia dòng/cột của pivot). Để xem chart đã lưu, frontend gửi `query_spec`
của nó tới `charts/data` y như xem trước trực tiếp, rồi gửi SQL trả về vào luồng `/queries` sẵn có.

`charts/data` trả SQL thay vì tự đẩy job để `BiState` tách rời khỏi `JobManager` (không cần
fixture nặng trong test http của BI), và SQL sinh ra trở thành tính năng "Xem SQL" hữu hình.
Thực thi vẫn chạy qua `JobManager` + polling (mô hình đã chọn); chỉ là submit do client khởi
tạo, giống SQL Lab. Vai trò xem (`ws_editor`/`ws_admin`) vốn đã có `query:run` nên polling chạy
được; tách hẳn xem-chart khỏi `query:run` qua endpoint kết quả riêng của chart là phần tinh chỉnh
ở 3c-3.2.

## Thành phần

### 1. Migration — `migrations/0014_chart.sql`

```sql
CREATE TABLE charts (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    dataset_id   uuid NOT NULL REFERENCES datasets(id)   ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    viz_type     text NOT NULL,            -- 'table' | 'pivot_table'
    params       jsonb NOT NULL,           -- QuerySpec + bố cục viz (chia dòng/cột pivot)
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX charts_workspace_idx ON charts (workspace_id);
CREATE INDEX charts_dataset_idx   ON charts (dataset_id);

-- Seed RBAC (giống dataset:* ở 0012): chart:read / chart:manage cho ws_editor, ws_admin.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

`params` là một cột JSONB (theo quy ước "JSONB rồi chuẩn hoá sau" từ phần catalog/dataset).
Nó chứa `QuerySpec` và bố cục pivot (dimension nào là dòng vs cột, và thứ tự metric pivot).

### 2. Query builder — `crates/egret-bi/src/chartquery.rs`

Hàm thuần, không I/O, unit-test cặn kẽ. Module quan trọng nhất.

```rust
pub struct QuerySpec {
    pub metrics:    Vec<String>,    // tên metric → giải ra biểu thức tổng hợp
    pub dimensions: Vec<String>,    // tên cột → GROUP BY (calc col dùng biểu thức của nó)
    pub columns:    Vec<String>,    // chế độ raw-records (chỉ dùng khi metrics rỗng)
    pub filters:    Vec<Filter>,    // lọc theo cột → WHERE
    pub orderby:    Vec<OrderBy>,   // { field, desc }
    pub row_limit:  i64,
    pub rollup:     bool,           // pivot → GROUP BY ROLLUP(...) + cờ GROUPING()
}

pub struct Filter   { pub col: String, pub op: FilterOp, pub value: FilterValue }
pub struct OrderBy  { pub field: String, pub desc: bool }

pub enum FilterOp { Eq, Ne, Gt, Ge, Lt, Le, Like, In, NotIn, IsNull, IsNotNull }

pub enum FilterValue { One(String), Many(Vec<String>), None }  // None cho IS [NOT] NULL
```

**Thuật toán dựng** — `build(dataset, columns, metrics, spec) -> Result<String, BiError>`:

1. **Giải + kiểm tra tên.** Mọi `metrics[i]` phải là metric đã biết; mọi `dimensions[i]` /
   `columns[i]` / `col` của filter / `field` của orderby phải là tên cột hoặc metric đã
   biết. Không hợp lệ → `BiError::BadRequest` (400). Metric → `expression` của nó; cột →
   tên vật lý, hoặc `expression` nếu là calculated. **GROUP BY / SELECT dùng biểu thức đã
   giải, không bao giờ dùng alias.**
2. **Chế độ.** `aggregate = !spec.metrics.is_empty()`.
   - **Aggregate:** `SELECT <biểu-thức-dim có alias>, <biểu-thức-metric có alias>
     [, GROUPING(dim)…] FROM <src> [WHERE …] GROUP BY <biểu-thức-dim |
     ROLLUP(<biểu-thức-dim>)> [ORDER BY …] LIMIT n`.
   - **Raw records:** `SELECT <biểu-thức-cột có alias> FROM <src> [WHERE …] [ORDER BY …]
     LIMIT n` (bỏ qua metrics).
3. **FROM** qua `expr.rs::source_ref(spec, source)` sẵn có (vật lý
   `catalog.schema.table` / ảo `(<sql>) AS _egret_src`).
4. **WHERE** từ `filters` (giai đoạn này chỉ lọc theo cột); mỗi cái render bằng
   `render_filter` + bộ mã hoá literal (§3).
5. **ROLLUP** khi `spec.rollup`: phát `GROUP BY ROLLUP(<biểu-thức-dim>)` và thêm một cột
   `GROUPING(<biểu-thức-dim>) AS __grp_<i>` cho mỗi dimension để frontend nhận diện dòng
   subtotal/grand-total (cờ grouping `= 1` nghĩa là dimension đó đã bị cuộn tổng).
6. **ORDER BY** theo biểu thức đã giải; **LIMIT** = `min(spec.row_limit, MAX_CHART_ROWS)`.

Alias của cột kết quả là **tên** metric/cột (có quote), để frontend ánh xạ cột kết quả về
tên dataset một cách xác định.

### 3. Bộ mã hoá literal SQL — quan trọng về bảo mật

Connector thực thi **chuỗi SQL thô** (đường REST statement của Trino không có tham số bind),
nên **giá trị** bộ lọc là đầu vào không tin cậy duy nhất và BẮT BUỘC mã hoá an toàn:

```rust
fn sql_literal(value: &str, type_generic: &str) -> Result<String, BiError> {
    match type_generic {
        "numeric"  => parse-kiểm-tra là số, phát không quote; sai → từ chối,
        "boolean"  => chuẩn hoá thành TRUE/FALSE; sai → từ chối,
        _          => single-quote và escape: mỗi ' thành '' ; bọc trong '…',
    }
}
```

- Định danh và biểu thức đến từ dataset (đã được `expr.rs` ở 3c-2.2 kiểm tra) → tin cậy;
  chỉ giá trị bộ lọc đi qua `sql_literal`.
- `IN` / `NOT IN` mã hoá từng phần tử rồi nối bằng `, `.
- Toán tử chọn từ enum `FilterOp` (không bao giờ là text tự do của người dùng) → không
  injection toán tử.
- `IS NULL` / `IS NOT NULL` không nhận giá trị.
- Không bao giờ chấp nhận `WHERE`/`HAVING` thô truyền thẳng.

Module này là **trọng tâm review code** của giai đoạn.

### 4. Tính đúng của ROLLUP (tổng pivot)

Subtotal/grand-total của pivot do **engine** tạo, không tính lại phía client, nên đúng với
mọi aggregate (`SUM`, `COUNT`, **và** `AVG`/`MIN`/`MAX`). Một truy vấn duy nhất, ví dụ
rows=`[region]`, cols=`[segment]`, metric=`SUM(sales)`:

```sql
SELECT region, segment, SUM(sales) AS "revenue",
       GROUPING(region) AS __grp_0, GROUPING(segment) AS __grp_1
FROM   ...
GROUP BY ROLLUP(region, segment)
```

- `__grp_0 = 0, __grp_1 = 0` → ô lá (dữ liệu thật).
- `__grp_1 = 1` → subtotal theo `segment` cho một `region` cố định.
- `__grp_0 = 1, __grp_1 = 1` → grand total.

Cả hai engine đều hỗ trợ: **Trino** `GROUP BY ROLLUP(...)` + `GROUPING(col)`; **StarRocks**
`GROUP BY ROLLUP(...)` + `grouping_id()/grouping()`. Khác biệt tên hàm xử lý theo từng engine
trong builder. Bài test tích hợp Trino thủ công được mở rộng để phủ đường ROLLUP/GROUPING
(nhạy với hợp đồng engine; unit test không bắt được lỗi phương ngữ).

### 5. Repository — `ChartRepo` (trong `egret-bi`)

Dòng `Chart` + CRUD: `create`, `get`, `list_by_workspace`, `update`, `delete`,
`chart_workspace(chart_id)` (cho guard sở hữu). Vi phạm tên trùng → `BiError::Conflict`
(409), giống `DatasetRepo`. Mutation là owner-or-admin.

### 6. REST routes — `chart_router` (trong `egret-bi/src/routes.rs`)

Gắn dưới phạm vi workspace cạnh datasets/sqllab:

| Method & path                              | Quyền            | Ghi chú                        |
|--------------------------------------------|------------------|--------------------------------|
| `GET    /api/ws/{ws}/charts`               | `chart:read`     | danh sách                      |
| `POST   /api/ws/{ws}/charts`               | `chart:manage`   | tạo                            |
| `GET    /api/ws/{ws}/charts/{id}`          | `chart:read`     | chi tiết                       |
| `PUT    /api/ws/{ws}/charts/{id}`          | `chart:manage`   | cập nhật (owner-or-admin)      |
| `DELETE /api/ws/{ws}/charts/{id}`          | `chart:manage`   | xoá (owner-or-admin)           |
| `POST   /api/ws/{ws}/charts/data`          | `chart:read`     | dựng SQL (trả sql)             |

- **Guard sở hữu lồng nhau:** mọi handler `/charts/{id}` xác minh chart thuộc workspace
  trong path trước khi xử lý (404 nếu lệch) — theo bài học IDOR ở 3c-2.2.
- `charts/data` nhận `{ dataset_id, query_spec }`, xác minh dataset thuộc workspace trong path,
  dựng SQL, rồi trả `{ sql, database_id }`. Frontend gửi cái đó tới endpoint
  `POST /api/ws/{ws}/queries` sẵn có và poll `/queries/{id}`.

### 7. Frontend

- **API client** `frontend/src/api/charts.ts`: kiểu `Chart`, `QuerySpec`; `useCharts(ws)`,
  `useChart(ws,id)`, `useChartMutations(ws)`, và `useBuildChartData(ws, body) → { sql,
  database_id }`, rồi dùng lại các hook `useSubmitQuery`/`useQueryStatus`/`useResults` sẵn có.
- **Trang Explore** `routes/explore/ChartExplore.tsx`: rail trái liệt kê metric và cột của
  dataset; nút chuyển Table/Pivot; bảng điều khiển (dimensions, metrics, lọc theo cột, sắp
  xếp, giới hạn dòng; pivot thêm phần chia dòng-vs-cột); nút **Run** điều khiển xem trước;
  hộp thoại **Save**.
- **Bảng** `components/chart/ChartTable.tsx`: `@tanstack/react-table` — sắp xếp, đổi kích
  thước, ẩn/hiện cột, và định dạng số/ngày theo `type_generic` của từng cột.
- **Pivot** `components/chart/PivotTable.tsx`: một transform thuần `buildPivot(rows, rowDims,
  colDims, metric)` đọc các cờ `__grp_*` để đặt dòng subtotal/grand-total, cộng bộ render
  lưới.
- **Danh sách Charts** `routes/explore/Charts.tsx` (thay placeholder): liệt kê / tạo / mở /
  xoá chart đã lưu.

## Xử lý lỗi

- Tên metric/cột/filter không xác định trong `QuerySpec` → 400 (`BiError::BadRequest`).
- Giá trị filter sai kiểu cột (vd phi-số cho cột số) → 400.
- Tên chart trùng → 409 (`BiError::Conflict`).
- Chart không thuộc workspace trong path → 404.
- Lỗi engine/SQL hiện qua trạng thái job bình thường (`failed` + message), y như SQL Lab —
  tầng chart không thêm đường lỗi thực thi mới.

## Kiểm thử

- **Unit test `chartquery`** (thuần, không DB): SQL aggregate, SQL raw-records, SQL ROLLUP +
  cờ grouping, mọi toán tử lọc, escaping `sql_literal` (mưu toan chèn quote vẫn bị quote;
  kiểm tra số/boolean), từ chối tên không xác định, dimension calc-column dùng biểu thức
  trong `GROUP BY`.
- **Test `ChartRepo`** (gated theo DB, tự bỏ qua khi thiếu `DATABASE_URL`): CRUD + trùng tên
  → conflict + tra cứu `chart_workspace`.
- **Smoke engine** (gated, tự bỏ qua khi thiếu `EGRET_TEST_TRINO_URL`): dựng một chart Bảng
  và một Pivot (ROLLUP) trên dataset seed, chạy qua `JobManager`, kiểm tra dòng + có dòng
  subtotal/grand-total.
- **Test frontend**: transform cross-tab `buildPivot` (kể cả đặt subtotal từ cờ) và render
  Bảng.
- **Test tích hợp Trino thủ công** mở rộng cho đường ROLLUP/GROUPING.

## Danh mục bảo mật

- Giá trị filter chỉ đi qua `sql_literal`; định danh/biểu thức từ dataset và đã kiểm tra
  trước.
- Toán tử đến từ enum, không bao giờ là text tự do.
- Không có `WHERE`/`HAVING` thô truyền thẳng.
- RBAC trên mọi route; owner-or-admin khi mutation; guard sở hữu lồng nhau trên `/charts/{id}`
  và kiểm tra workspace cho `dataset_id` của `charts/data` (dataset từ workspace khác → 404).
- `row_limit` kẹp về `MAX_CHART_ROWS`; thực thi kế thừa timeout + row cap của engine.

## Hoãn → 3c-3.2 và xa hơn

- Loại viz ECharts (bar/line/pie/big-number) dùng lại pipeline này.
- Gom nhóm theo thời gian (`date_trunc` theo từng engine).
- Lọc theo metric (`HAVING`).
- Định dạng heatmap/có điều kiện, kéo-thả sắp cột, đóng băng, mini-bar, drill-through.
- Toán tử lọc nâng cao, series-limit ("top-N + Other"), tương tác thu/mở pivot.
