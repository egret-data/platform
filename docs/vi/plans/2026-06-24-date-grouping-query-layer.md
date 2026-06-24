# Date Grouping — Sub-project A (Temporal Units tầng query) — Plan (tham chiếu VN)

**Bản chuẩn:** `docs/en/plans/2026-06-24-date-grouping-query-layer.md`. **Spec:**
`docs/en/specs/2026-06-24-date-grouping-query-layer-design.md`. **Branch:** `feat/date-grouping-query-layer`.
**Chỉ 1 file:** `crates/egret-bi/src/chartquery.rs`. TDD, commit từng task. `Engine` chỉ có Trino +
StarRocks → mọi `match engine` đủ nhánh, không cần wildcard.

**Task 1 — Thêm `QuerySpec.temporal_units`** (`HashMap<String,String>`, `#[serde(default)]`). Sửa 12 literal
`QuerySpec{}` trong test (10 ở chartquery.rs, 2 ở tests/chart_engine.rs) thêm `temporal_units:
HashMap::new()`. Test regression: map rỗng → SQL không đổi, không có `date_trunc`. Commit.

**Task 2 — enum `TemporalUnit`** (7 truncate + 8 extract) + `parse(token)->Option` + `is_extract()`.
Test parse + phân loại. Commit.

**Task 3 — `temporal_expr(engine, inner, unit)` truncate**: `date_trunc('<u>', inner)` cho minute…year,
cùng cú pháp 2 engine. Tạm có stub `extract_expr` trả `inner` để compile. Test truncate Trino+StarRocks.
Commit.

**Task 4 — `extract_expr` thật** (per-engine, chuẩn hoá): minute/hour/month/quarter giống nhau;
`day_of_week` → Trino `day_of_week()` / StarRocks `(weekday()+1)`; `week_of_year` → `week_of_year()` /
`week(x,3)`; day_of_month/day_of_year khác tên hàm. Test so khớp từng cặp. Commit.

**Task 5 — `dim_expr(engine,columns,spec,name)`** (resolve cột → bọc unit nếu có; cột không temporal hoặc
unit lạ → `BadRequest`). Thay `column_sql` trong vòng dimension (`build()` 228–235) bằng `dim_expr` →
GROUP BY/GROUPING SETS tự đúng. Test: truncate SELECT+GROUP BY khớp; non-temporal → lỗi; unit lạ → lỗi.
Commit.

**Task 6 — ORDER BY** dùng cùng biểu thức: `orderby_sql` + `push_orderby` nhận `spec`; field là dimension
có unit → resolve qua `dim_expr`. Sửa 2 call site (`build()` 303 & 323). Test ORDER BY khớp GROUP BY.
Commit.

**Task 7 — Sweep + verify thật:** `cargo fmt --all --check`, `cargo test -p egret-bi`. Chạy `month` và
`day_of_week` trên **Trino + StarRocks thật** với cột temporal seed: month ra 1 dòng/tháng nhãn ngày
giống nhau; day_of_week ra 1–7 Mon=1 giống nhau. Kiểm tra coercion StarRocks trả số nguyên cho extraction
(không phải kiểu timestamp gốc → nếu sai là bug coerce theo kiểu cột dataset, sửa tại đây). Push + PR (ghi
2 SQL sinh ra + kết quả parity vào PR body).
