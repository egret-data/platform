# Date Grouping — Sub-project A: Temporal Units ở tầng query (Thiết kế)

**Bản tham chiếu tiếng Việt.** Bản chuẩn (canonical) tiếng Anh:
`docs/en/specs/2026-06-24-date-grouping-query-layer-design.md`.

**Thuộc chương trình:** Date Grouping (A → B → C). Đây là **A** — nền tảng backend.

## Mục tiêu

Cho phép chart gom/breakout một **dimension thời gian** theo **đơn vị thời gian** — hoặc *truncate*
(minute…year, vd "theo tháng"), hoặc *extract* (hour-of-day, day-of-week, month-of-year…) — sinh SQL
đúng và **ra kết quả giống nhau trên cả Trino + StarRocks**. Chỉ backend. Không đụng frontend (B), không
thêm dashboard param (C).

## Thay đổi wire (tương thích ngược)

Thêm 1 field optional vào `QuerySpec` (`chartquery.rs:17`):

```rust
#[serde(default)]
pub temporal_units: std::collections::HashMap<String, String>, // tên cột dimension -> token đơn vị
```

`#[serde(default)]` → chart đã lưu + payload cũ deserialize nguyên vẹn (map rỗng = hành vi cũ). Không
cần sửa `model.rs` (params đi qua serde).

## Enum TemporalUnit

Truncate: `minute, hour, day, week, month, quarter, year`.
Extract: `minute_of_hour, hour_of_day, day_of_week, day_of_month, day_of_year, week_of_year,
month_of_year, quarter_of_year`. (Đúng bộ của Metabase.)
`parse(token)->Option`, `is_extract()->bool`. Token lạ → `BadRequest`.

## Sinh SQL: `temporal_expr(engine, inner, unit)`

**Truncate — cùng cú pháp 2 engine:** `date_trunc('<unit>', e)` cho minute…year. Cả hai coi `week` là
bắt đầu Thứ Hai → khớp.

**Extract — khác hàm theo engine, đã chuẩn hoá cho ra cùng số (ISO, khớp Trino/Metabase):**

| đơn vị          | Trino             | StarRocks          | dải |
|-----------------|-------------------|--------------------|-----|
| minute_of_hour  | `minute(e)`       | `minute(e)`        | 0–59 |
| hour_of_day     | `hour(e)`         | `hour(e)`          | 0–23 |
| day_of_week     | `day_of_week(e)`  | `(weekday(e)+1)`   | 1–7 (T2=1) |
| day_of_month    | `day_of_month(e)` | `dayofmonth(e)`    | 1–31 |
| day_of_year     | `day_of_year(e)`  | `dayofyear(e)`     | 1–366 |
| week_of_year    | `week_of_year(e)` | `week(e, 3)`       | 1–53 (ISO) |
| month_of_year   | `month(e)`        | `month(e)`         | 1–12 |
| quarter_of_year | `quarter(e)`      | `quarter(e)`       | 1–4 |

Hai chỗ cần chú ý: `day_of_week` (StarRocks `dayofweek` đánh số T2≠1 → dùng `weekday(e)+1`),
`week_of_year` (StarRocks `week(e,3)` = chế độ ISO).

## Áp dụng trong `build()`

Thêm helper `dim_expr(engine, columns, spec, name)`: resolve cột qua `column_sql` rồi bọc
`temporal_expr` nếu `spec.temporal_units` có unit cho cột đó (cột phải `is_temporal`, không thì
`BadRequest`).

1. **Vòng dimension** (`:228–235`): thay `column_sql` bằng `dim_expr` → `dim_exprs` thành biểu thức đã
   bọc → **GROUP BY + GROUPING SETS tự đúng**.
2. **ORDER BY** (`push_orderby`/`orderby_sql`): truyền `spec` vào; nếu field là dimension có unit thì bọc
   y hệt để ORDER BY khớp GROUP BY.
3. **Filter**: **giữ nguyên** (lọc theo timestamp thật, không truncate).
4. **Raw-records mode**: giữ nguyên (unit chỉ áp cho aggregate-mode).

## Kiểu kết quả

Truncate → giá trị temporal (format ngày như cũ). Extract → integer (engine trả bigint/int → coerce
thành số). **Điểm cần verify:** coerce phải theo kiểu của result-set, không theo kiểu cột dataset — chạy
thật extraction trên StarRocks phải ra số nguyên (nếu coerce theo kiểu cột dataset → bug, sửa tại đây).

## Test

Unit test (`build()`, không DB): truncate Trino+StarRocks ra `date_trunc('month',…)`; extract ra đúng
hàm mỗi engine (`day_of_week` vs `(weekday()+1)`, `week_of_year` vs `week(...,3)`); ORDER BY bọc giống
GROUP BY; token lạ → BadRequest; unit trên cột không temporal → BadRequest; **regression:** map rỗng →
SQL y hệt trước.

Manual integration test (Trino + StarRocks thật): chạy group `date_trunc('month')` và extract
`day_of_week` trên cả 2 engine với demo data, assert nhãn bucket + số nguyên giống nhau. Chạy trước khi
push.

## File đụng tới

Chỉ `crates/egret-bi/src/chartquery.rs`. Frontend + dashboard param là B và C.

## Quy ước

Sweep backend trước commit (`cargo test -p egret-bi` + `cargo fmt --all --check`); chạy manual
Trino+StarRocks test trước push; git identity cấu hình + footer Co-Authored-By; stage path tường minh.
