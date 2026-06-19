# Phase 3c-2.2 — Dataset Semantics (metrics, calculated columns, metadata cột)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (tiếng Việt). Bản gốc tiếng Anh:
> `docs/en/specs/2026-06-19-phase-3c-2-2-dataset-semantics-design.md`.

## Mục tiêu

Hoàn thiện semantic layer của dataset để chart (3c-3) có sẵn measures + dimensions phong phú:

- **Metrics** — biểu thức tổng hợp có tên (`SUM(totalprice)`, `COUNT(*)`).
- **Calculated columns** — cột định nghĩa bằng biểu thức SQL (`price * qty`,
  `EXTRACT(year FROM orderdate)`) thay vì cột vật lý từ nguồn.
- **Metadata cột** — sửa được `verbose_name` (nhãn hiển thị) + `description` cho mọi cột
  (physical lẫn calculated).

Đã đối chiếu Apache Superset: mô hình lưu trữ và hành vi refresh-reconcile bám theo
`TableColumn`/`SqlMetric` + `fetch_metadata` của Superset. Khác biệt cố ý duy nhất là cách
validate expression (xem §4).

## Phạm vi

**Trong phạm vi:**

- Bảng `dataset_metrics` + `dataset_columns` thêm `expression` / `verbose_name` /
  `description`.
- **Refresh reconcile** viết lại để upsert cột physical theo tên (giữ metadata user đã sửa)
  và không đụng calculated columns / metrics.
- Validate expression + suy type qua `Connector::describe` đã có.
- REST CRUD cho metrics, calculated columns + sửa metadata cột.
- Trang detail dataset chia tab (Columns / Metrics).

**Ngoài phạm vi (YAGNI / hoãn):**

- `metric_type` / `d3format` / định dạng tiền tệ của metric (việc lúc chart).
- Cờ `groupby` / `filterable` của cột (việc của chart-builder).
- Certification, tags, advanced data types.

## Kiến trúc

Mở rộng `egret-bi` (không crate mới). Trang detail chia tab. Điểm tái dùng liên-crate duy
nhất là `Connector::describe` của `egret_engine` (thêm ở 3c-2) để validate expression.

### Đơn vị mới / sửa

| File | Trách nhiệm |
|---|---|
| `migrations/0013_dataset_semantics.sql` (mới) | `ALTER dataset_columns` + bảng `dataset_metrics` |
| `crates/egret-bi/src/dataset.rs` (sửa) | Type `Metric`/`NewMetric`; CRUD metrics; CRUD calc-column; **reconcile** (thay `replace_columns`) |
| `crates/egret-bi/src/expr.rs` (mới) | `validate_expression` — guard single-statement + `describe`-probe → `(data_type, type_generic)`; builder source-ref theo engine |
| `crates/egret-bi/src/routes.rs` (sửa) | handler metric + calc-column + metadata cột; detail DTO mang metrics + field cột mới |
| `crates/egret-bi/src/model.rs` (sửa) | `MetricDto`, body request, mở rộng `DatasetColumnDto` + `DatasetDetailDto` |
| `crates/egret-bi/tests/dataset.rs` (sửa) | test reconcile-giữ-calc; CRUD metrics + calc |
| `crates/egret-bi/tests/expr.rs` (mới) | validate expression (engine-gated) |
| `frontend/src/api/datasets.ts` (sửa) | hook + type metric/calc-column |
| `frontend/src/routes/explore/DatasetDetail.tsx` (sửa) | tab Columns / Metrics |
| `frontend/src/components/dataset/MetricsTab.tsx` (mới) | bảng metrics + add/edit/delete |
| `frontend/src/components/dataset/ColumnsTab.tsx` (mới) | bảng cột + sửa metadata + thêm calculated |

## Mô hình dữ liệu

Migration `0013_dataset_semantics.sql`:

```sql
ALTER TABLE dataset_columns
  ADD COLUMN expression   text,                       -- NULL = physical; có giá trị = calculated
  ADD COLUMN verbose_name text NOT NULL DEFAULT '',
  ADD COLUMN description  text NOT NULL DEFAULT '';

CREATE TABLE dataset_metrics (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    metric_name  text NOT NULL,
    expression   text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    verbose_name text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    ordinal      int  NOT NULL,
    UNIQUE (dataset_id, metric_name)
);
CREATE INDEX dataset_metrics_dataset_idx ON dataset_metrics (dataset_id);
```

`is_calculated` được suy ra (`expression IS NOT NULL`), không lưu. Cột physical và calculated
dùng chung `dataset_columns` (mô hình thống nhất của Superset).

## Refresh reconcile (thay đổi cốt lõi)

`replace_columns` ở 3c-2 làm DELETE-toàn-bộ + chèn lại — sẽ xóa sạch calculated columns và
metadata user. Thay bằng `reconcile_physical_columns(dataset_id, fresh)`, bám theo
`fetch_metadata` của Superset (diff theo `column_name`):

Với mỗi cột physical vừa introspect (`fresh`):
- **Upsert theo `(dataset_id, column_name)`**: nếu dòng đã có, cập nhật `data_type`,
  `type_generic`, `is_temporal`, `ordinal`; **giữ** `verbose_name`, `description`. Nếu mới,
  chèn với metadata rỗng.

Sau upsert:
- **Xóa** cột physical (`expression IS NULL`) mà `column_name` không còn trong `fresh`.
- **Không đụng** calculated columns (`expression IS NOT NULL`) hay `dataset_metrics`.

Calculated columns được đánh lại ordinal để xếp sau physical. Calculated column mà cột
physical nền của nó biến mất thì để nguyên (sẽ lỗi lúc query — như Superset); v1 không chủ
động vô hiệu hóa.

Chạy trong một transaction, đúng chỗ `replace_columns` được gọi bởi `ds_refresh`.

## Validate expression (`expr.rs`)

`validate_expression(spec, source, expr) -> Result<DescribedExpr, BiError>` với
`DescribedExpr { data_type, type_generic }`:

1. **Guard** — từ chối nếu `expr` rỗng hoặc chứa dấu kết thúc câu lệnh khiến nó thành
   multi-statement (`;` ngoài chuỗi literal). Đây là cổng an toàn duy nhất; user thao tác đã
   có `dataset:manage` và có thể chạy SQL tự do ở SQL Lab, Ranger vẫn enforce lúc query.
2. **Probe** — dựng `SELECT <expr> AS _egret_expr FROM <source>` với `source` là:
   - physical: `catalog.schema.table` đã quote (tái dùng quoting identifier theo engine sẵn
     trong `schema.rs`),
   - virtual: `(<dataset.sql>)` bọc subquery có alias.

   Gọi `build_connector(spec).describe(<probe sql>, 15s)`. Việc `describe` bọc thêm tạo
   `SELECT * FROM (SELECT <expr> AS _egret_expr FROM <source>) AS _egret_ds LIMIT 0`, vừa
   validate expression vừa trả về cột kết quả kèm kiểu.
3. **Kết quả** — lấy kiểu của cột duy nhất trả về làm `data_type`, ánh xạ qua
   `typegen::generic_type(spec.engine, &data_type)` ra `type_generic`. Lỗi engine (expression
   sai) lan thành `BiError::Engine` → HTTP 502 kèm thông điệp engine.

Metrics và calculated columns dùng chung đường này (biểu thức tổng hợp của metric hợp lệ
trong `SELECT <expr> FROM <source>` không cần `GROUP BY`).

## REST API

Tất cả dưới `dataset_router` sẵn có, gate `dataset:manage` + owner-or-admin, dùng
`conn_spec_perm` để resolve engine + kiểm tra database đã được grant.

| Method | Path | Body → trả về |
|---|---|---|
| POST | `/ws/{ws}/datasets/{id}/metrics` | `{ metric_name, expression, verbose_name?, description? }` → `DatasetDetail` |
| PUT | `/ws/{ws}/datasets/{id}/metrics/{metric_id}` | cùng body → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}/metrics/{metric_id}` | → `DatasetDetail` |
| POST | `/ws/{ws}/datasets/{id}/columns` | `{ column_name, expression, verbose_name?, description? }` (calculated) → `DatasetDetail` |
| PUT | `/ws/{ws}/datasets/{id}/columns/{column_name}` | `{ verbose_name?, description?, expression? }` → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}/columns/{column_name}` | → `DatasetDetail` (chỉ calculated; xóa physical trả 400) |

Hành vi:
- **Thêm metric / thêm calculated column / sửa expression của calc column**: validate
  expression (§4) trước; chỉ lưu khi thành công (introspect-first). `data_type` +
  `type_generic` lấy từ probe.
- **Sửa cột physical**: chỉ đổi được `verbose_name` / `description`; `expression` bị từ chối
  với cột physical (400).
- **Xóa cột**: chỉ cho khi `expression IS NOT NULL` (calculated). Xóa cột physical trả 400 —
  cột physical do refresh quản.
- Mọi mutation trả về `DatasetDetail` đầy đủ (đã refresh) để UI render lại từ một phản hồi.

`DatasetDetailDto` thêm `metrics: Vec<MetricDto>`; mỗi `DatasetColumnDto` thêm
`expression: Option<String>`, `verbose_name: String`, `description: String`, và
`is_calculated: bool` suy ra.

## Frontend

`DatasetDetail.tsx` giữ header (name, kind, source, Refresh/Edit/Delete) và thêm dải tab bên
dưới:

- **Tab Columns** (`ColumnsTab.tsx`): bảng cột — name, data_type, badge generic-type, cờ
  temporal, badge **calculated** cho cột có expression, và `verbose_name`/`description`. Sửa
  inline `verbose_name`/`description` cho mọi cột; sửa `expression` cho calculated. **+
  Calculated column** mở form nhỏ (column_name + expression + nhãn/mô tả tùy chọn); submit
  gọi API, validate phía server — expression sai hiện toast kèm lỗi engine. Delete chỉ trên
  calculated column.
- **Tab Metrics** (`MetricsTab.tsx`): bảng metrics — name, expression, badge generic-type,
  nhãn. **+ Metric** / edit / delete, cùng luồng validate-on-submit.

`api/datasets.ts` thêm: type `DatasetMetric`; `DatasetColumn` thêm
`expression`/`verbose_name`/`description`/`is_calculated`; hook `useAddMetric`,
`useUpdateMetric`, `useDeleteMetric`, `useAddColumn`, `useUpdateColumn`, `useDeleteColumn` —
mỗi cái invalidate query detail dataset.

## Xử lý lỗi

- Expression sai → lỗi engine hiện HTTP 502 kèm thông điệp engine; không lưu gì
  (introspect-first).
- Trùng `metric_name` / `column_name` → unique-violation map sang 4xx rõ ràng (không phải
  500 trơ) — sửa luôn lỗi 3c-2 nơi trùng tên dataset trả 500.
- Sửa/xóa cấu trúc cột physical → 400 kèm thông điệp rõ.
- Secret DB không bao giờ trả về hay ghi log (giữ nguyên).

## Kiểm thử

**Rust**

- `expr` unit: guard single-statement (chặn `a; b`, nhận `SUM(x)` / `price*qty`); builder
  source-ref theo engine (quote physical, bọc subquery virtual). Thuần, không engine.
- `expr` engine-gated (tự skip khi không có Trino): expression hợp lệ trả về kiểu suy ra;
  expression sai trả `BiError::Engine`.
- `DatasetRepo` DB-gated:
  - **reconcile**: seed dataset có cột physical + 1 calculated column + `verbose_name` đã
    sửa; chạy reconcile với nguồn thay đổi (1 physical bị bỏ, 1 đổi kiểu, 1 thêm mới); kiểm
    cột physical được upsert, cột bỏ bị xóa, **calculated column và `verbose_name` đã sửa
    được giữ nguyên**. (Regression then chốt cho lỗi clobber 3c-2.)
  - CRUD metrics + ordinal; CRUD calc-column; cascade delete theo dataset.
- Routes: từ chối RBAC khi thiếu `dataset:manage`; gate owner-or-admin; xóa cột physical bị
  từ chối; trùng metric/column map sang 4xx.

**Frontend**

- vitest: tab Columns và Metrics render; form add-metric và add-calculated-column render +
  validate field bắt buộc; calculated column hiện badge.

## Migration / tương thích

`0013` chỉ thêm (cột nullable/có default + bảng mới); dataset hiện có vẫn chạy (không metric,
không calculated column, metadata rỗng). Handler `ds_refresh` đổi `replace_columns` →
reconcile, không đổi API.
