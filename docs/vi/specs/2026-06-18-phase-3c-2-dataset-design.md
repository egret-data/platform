# Phase 3c-2 — Dataset (thiết kế)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (tiếng Việt). Bản gốc tiếng Anh:
> `docs/en/specs/2026-06-18-phase-3c-2-dataset-design.md`.

## Mục tiêu

Giới thiệu **Dataset** — một nguồn ngữ nghĩa có tên, quản trị được, dựa trên một database
đã đăng ký, mang theo một bản snapshot các cột đã có kiểu (typed columns). Dataset chính
là đối tượng mà chart (Phase 3c-3) sẽ được dựng lên. v1 làm cho dataset *dùng được*: nó
có một danh sách cột ổn định kèm kiểu tổng quát để trình dựng chart phân biệt được
dimension, measure và cột thời gian.

## Phạm vi

**Trong phạm vi (v1):**

- Hai loại dataset trên một model thống nhất (giống `SqlaTable` của Superset):
  - **Physical** — trỏ tới một `catalog.schema.table` có sẵn.
  - **Virtual** — bọc một câu lệnh `SELECT` (đích "promote" tự nhiên từ SQL Lab).
- Snapshot cột được đồng bộ từ nguồn lúc tạo, có thể đồng bộ lại bằng nút **Refresh**.
- Phân loại `type_generic` theo từng cột: `string | numeric | temporal | boolean`, kèm cờ
  `is_temporal` — metadata tối thiểu mà chart cần.
- REST API CRUD + Refresh, phạm vi theo workspace, có RBAC, mutation theo owner-or-admin.
- Frontend: danh sách dataset, luồng tạo (từ bảng / từ SQL), trang chi tiết có cột và
  Refresh, và cầu nối **"Save as dataset"** từ trình soạn SQL Lab.

**Tạm hoãn (3c-2.2, không thuộc phase này):**

- Metrics (`COUNT(*)`, `SUM(x)`, …).
- Calculated columns (cột có `expression`).
- `verbose_name` / mô tả từng cột / gợi ý định dạng.

## Kiến trúc

Mở rộng crate **`egret-bi`** sẵn có. Nó đã sở hữu `SchemaService` (introspect physical qua
connector), phần ráp `BiState` (databases, sessions, workspaces, authz), và các pattern
RBAC + owner-gate mà saved query đang dùng. Dataset cùng dạng đối tượng với saved query
nên đặt cạnh nhau thay vì tạo crate mới.

Phần việc mới ở tầng dưới nằm tại **`egret-engine`**: một khả năng `describe(sql)` trên
connector trả về cột **có kiểu** cho một `SELECT` bất kỳ. Hiện connector chỉ phơi *tên*
cột cho kết quả chứ không phơi kiểu cho query bất kỳ; virtual dataset cần kiểu.

### Đơn vị mới / sửa

| File | Trách nhiệm |
|---|---|
| `crates/egret-bi/src/dataset.rs` (mới) | `DatasetRepo` — CRUD dataset + snapshot cột, reconcile cột |
| `crates/egret-bi/src/typegen.rs` (mới) | Hàm thuần ánh xạ `type_generic` (theo engine), có unit test |
| `crates/egret-bi/src/routes.rs` (sửa) | Thêm `dataset_router`; handler create/list/get/update/refresh/delete |
| `crates/egret-bi/src/model.rs` (sửa) | DTO dataset + body request tạo/cập nhật |
| `crates/egret-bi/src/schema.rs` (tái dùng) | `columns()` cho introspect physical |
| `crates/egret-engine/src/connector.rs` (sửa) | Thêm `describe(sql) -> Vec<DescribedColumn>` vào trait `Connector` |
| `crates/egret-engine/src/trino.rs` (sửa) | Hiện thực `describe` qua `LIMIT 0` + metadata `columns` của statement |
| `crates/egret-engine/src/starrocks.rs` (sửa) | Hiện thực `describe` qua `LIMIT 0` + `column_type()` của result |
| `migrations/0012_dataset.sql` (mới) | Bảng `datasets` + `dataset_columns` + seed `dataset:read`/`dataset:manage` |
| `frontend/src/api/datasets.ts` (mới) | Hook react-query |
| `frontend/src/routes/explore/Datasets.tsx` (viết lại stub) | Trang danh sách |
| `frontend/src/components/dataset/*` (mới) | Luồng tạo, trang chi tiết, bảng cột |
| `frontend/src/components/sqllab/SqlTab.tsx` (sửa) | Hành động "Save as dataset" |

## Mô hình dữ liệu

Migration `0012_dataset.sql`:

```sql
CREATE TABLE datasets (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    -- nguồn physical (tất cả NULL nếu là virtual):
    catalog     text,
    schema_name text,
    table_name  text,
    -- nguồn virtual (NULL nếu là physical):
    sql         text,
    created_by  uuid REFERENCES users(id),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- đúng một loại nguồn:
    CONSTRAINT dataset_one_kind CHECK ((table_name IS NOT NULL) <> (sql IS NOT NULL)),
    CONSTRAINT dataset_name_uniq UNIQUE (workspace_id, name)
);

CREATE TABLE dataset_columns (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    column_name  text NOT NULL,
    data_type    text NOT NULL,          -- kiểu thô của engine, vd "varchar(20)"
    type_generic text NOT NULL,          -- string | numeric | temporal | boolean
    is_temporal  boolean NOT NULL DEFAULT false,
    ordinal      int NOT NULL,
    UNIQUE (dataset_id, column_name)
);
CREATE INDEX dataset_columns_dataset_idx ON dataset_columns(dataset_id);

-- seed RBAC (giống cách cấp query:run cho ws_editor + ws_admin trong 0009_engine.sql)
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

> Lưu ý: tên cột/bảng (`schema_name`, `table_name`) tránh từ khóa SQL. Câu seed theo đúng
> mẫu `0009_engine.sql`: `role_permissions(role_id, permission)` chọn `roles.id` theo
> `scope='workspace'` + `name`.

`kind` được suy ra chứ không lưu: `sql IS NULL` → physical, ngược lại virtual.

## Introspect cột

Một hàm `DatasetRepo::sync_columns(spec, source) -> Vec<DescribedColumn>` trừu tượng hóa cả
hai loại, đúng như `external_metadata()` của Superset:

- **Physical:** `SchemaService::columns(spec, catalog, schema, table)` (đường DESCRIBE sẵn
  có) → `(name, raw_type)`.
- **Virtual:** `connector.describe(sql)` — bọc SELECT thành
  `SELECT * FROM (<sql>) AS _egret_ds LIMIT 0` và đọc metadata cột có kiểu từ phản hồi
  engine:
  - **Trino:** statement protocol trả về mảng `columns: [{name, type}]` ở phản hồi đầu,
    có sẵn ngay cả khi không có dòng nào.
  - **StarRocks:** result của `mysql_async` phơi định nghĩa cột (`column.column_type()`)
    độc lập với số dòng.

`DescribedColumn { name: String, raw_type: String }` là dạng dùng chung. Repo sau đó ánh xạ
mỗi cột qua `typegen` và gán `ordinal` theo vị trí.

### Ánh xạ `type_generic` (`typegen.rs`)

Một hàm thuần `generic_type(engine: Engine, raw_type: &str) -> (GenericType, bool)` trả về
lớp tổng quát và `is_temporal`. So khớp không phân biệt hoa thường trên từ khóa đầu của kiểu
thô (bỏ phần hậu tố `(...)` về độ chính xác/độ dài):

| Generic | Kiểu thô Trino | Kiểu thô StarRocks |
|---|---|---|
| `numeric` | tinyint, smallint, integer, int, bigint, real, double, decimal | tinyint, smallint, int, bigint, largeint, float, double, decimal |
| `temporal` | date, time, timestamp, timestamp with time zone | date, datetime |
| `boolean` | boolean | boolean |
| `string` | varchar, char, varbinary, json, uuid, *(fallback)* | char, varchar, string, json, *(fallback)* |

`is_temporal` là `true` khi và chỉ khi generic là `temporal`. Kiểu lạ rơi về `string` (mặc
định an toàn — dùng được làm dimension). Đây là nguồn chân lý duy nhất mà chart dựa vào; nó
được unit-test cạn kiệt theo từng engine.

## REST API

Gắn trên `BiState` thành `dataset_router`, phạm vi workspace dưới `/api`.

| Method | Path | Gate | Body / trả về |
|---|---|---|---|
| POST | `/ws/{ws}/datasets` | `dataset:manage` | `CreateDataset` → `DatasetDetail` |
| GET | `/ws/{ws}/datasets` | `dataset:read` | → `[DatasetSummary]` |
| GET | `/ws/{ws}/datasets/{id}` | `dataset:read` | → `DatasetDetail` (kèm cột) |
| PUT | `/ws/{ws}/datasets/{id}` | `dataset:manage` + owner-or-admin | `UpdateDataset` → `DatasetDetail` |
| POST | `/ws/{ws}/datasets/{id}/refresh` | `dataset:manage` + owner-or-admin | → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}` | `dataset:manage` + owner-or-admin | → `{ ok: true }` |

Dạng request/response:

```rust
// kind có tag: đúng một trong physical / virtual
enum CreateSource {
    Physical { catalog: Option<String>, schema: String, table: String },
    Virtual  { sql: String },
}
struct CreateDataset { name: String, database_id: Uuid, description: String, source: CreateSource }
struct UpdateDataset { name: String, description: String, sql: Option<String> } // sql chỉ có nghĩa với virtual

struct DatasetColumnDto { column_name: String, data_type: String, type_generic: String, is_temporal: bool, ordinal: i32 }
struct DatasetSummary { id, name, description, database_id, kind: "physical"|"virtual", column_count, updated_at }
struct DatasetDetail  { ..summary, catalog, schema, table, sql, columns: Vec<DatasetColumnDto> }
```

`database_id` phải được cấp cho workspace (tái dùng guard `conn_spec`/`list_for_workspace`
sẵn có trong `routes.rs`).

### Transaction & xử lý lỗi

- **Create** theo nguyên tắc introspect-trước: dựng `ConnSpec`, lấy cột từ nguồn, rồi mới
  `INSERT` dataset + cột trong một transaction DB. Bảng sai hoặc SQL lỗi sẽ trả lỗi engine
  và **không tạo gì**.
- **Refresh** introspect lại, rồi reconcile `dataset_columns` (thêm mới, xóa thiếu, cập
  nhật `data_type`/`type_generic` thay đổi) trong một transaction. Nếu introspect lỗi
  (nguồn bị xóa, SQL nay không hợp lệ), trả lỗi và **giữ nguyên** snapshot hiện có.
- **Update** `sql` của virtual dataset **không** tự refresh cột — người dùng refresh tường
  minh (dễ đoán, giống Superset). v1 chỉ phơi nút Refresh.
- Secret DB không bao giờ được trả về hay ghi log (bất biến giữ nguyên).

## Frontend

- **`Datasets.tsx`** — bảng danh sách: name (link), badge kind, database, số cột, updated;
  nút **Create** góc trên-phải. Empty state có giải thích ngắn + Create.
- **Luồng tạo** (`CreateDatasetDialog`) — hai tab:
  - *Từ bảng*: chọn database → cascading catalog → schema → table (tái dùng logic cascading
    select của `DataPanel`), kèm name + description.
  - *Từ SQL*: chọn database + editor SQL (Monaco) + name + description. Điền sẵn khi mở qua
    cầu nối SQL Lab.
- **Trang chi tiết** (`DatasetDetail`) — header (name, kind, vị trí nguồn/SQL, mô tả) +
  bảng cột (column_name, data_type, badge generic-type, cờ temporal) + nút **Refresh**
  (toast khi thành công/thất bại) + Edit + Delete (chỉ owner-or-admin).
- **Cầu nối SQL Lab** — thêm nút **"Save as dataset"** vào toolbar `SqlTab` (cạnh "Save");
  mở luồng tạo-từ-SQL điền sẵn `sql` và `dbId` của tab.
- **`api/datasets.ts`** — hook react-query: `useDatasets`, `useDataset`, `useCreateDataset`,
  `useUpdateDataset`, `useRefreshDataset`, `useDeleteDataset`. List và detail được key sao
  cho refresh invalidate được detail.

## Kiểm thử

**Rust**

- `typegen` — unit test dạng bảng cạn kiệt theo từng engine, gồm hậu tố độ chính xác
  (`decimal(10,2)` → numeric), `timestamp with time zone` → temporal, và lạ → string. Thuần,
  không DB.
- `DatasetRepo` — DB-gated (tự skip khi không có Postgres): tạo physical + virtual, list,
  get-kèm-cột, update, delete cascade, và **reconcile** (thêm / xóa / đổi kiểu một cột khi
  refresh).
- `connector.describe` — engine-gated (tự skip khi không có Trino:8090 / StarRocks:9030):
  một `SELECT` đã biết trả về đúng cột có kiểu trên mỗi engine.
- Routes — từ chối RBAC khi thiếu `dataset:read`/`dataset:manage`; gate owner-or-admin trên
  update/refresh/delete; create từ chối `database_id` chưa được cấp.

**Frontend**

- vitest: form tạo render cả hai tab và validate field bắt buộc; list render dòng từ
  `useDatasets` mock; "Save as dataset" điền sẵn SQL + db.

## Ngoài phạm vi / không làm

- Không metrics, calculated columns, hay verbose name (3c-2.2).
- Không bảo mật row/column mức dataset ngoài enforcement Ranger sẵn có lúc chạy query (chart
  ở 3c-3 đi qua cùng đường engine).
- Không fetch cột live — cột luôn là snapshot đã lưu.
- Không dataset join / liên database — một dataset thuộc đúng một database.
```
