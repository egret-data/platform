# Phase 3c-3.2 — Bảng Pivot đa chiều (chuẩn Metabase)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-19-phase-3c-3-2-pivot-multidim-design.md`.
> Mở rộng Phase 3c-3 (`docs/en/specs/2026-06-19-phase-3c-3-chart-design.md`) trên cùng
> branch `feat/phase-3c-3-chart`.

## Mục tiêu

Thay thế pivot một-chiều ở 3c-3 bằng **pivot đa mức chuẩn Metabase**: N chiều hàng × N chiều
cột × N measure, với nhóm hàng/cột xếp tầng, subtotal ở mọi mức, một grand total, nhóm hàng
thu/mở được, và nhiều measure đặt ở mức cột trong cùng.

Đối chiếu với pivot của **Apache Metabase** (`metabase/pivot/core.cljc` + `PivotTable/`): ta
dùng lại mô hình dữ liệu của nó (cây hàng / cây cột → danh sách header phẳng có
`depth`/`offset`/`span`; node subtotal + grand-total; nhiều measure ở mức top-header trong
cùng; thu/mở phía client). Ta khác ở **chiến lược truy vấn**: Metabase chạy N subquery (powerset
các breakout) và đánh dấu hàng bằng bitmask; ta dùng **một truy vấn engine-native** với hai
ROLLUP.

## Bối cảnh — 3c-3 có gì, đổi gì

Pivot ở 3c-3 dùng một `GROUP BY ROLLUP(rowDim, colDim)` và `buildPivot` phẳng, chỉ đúng cho một
chiều hàng + một chiều cột. Giai đoạn này tổng quát hoá cả builder SQL lẫn transform frontend
lên độ sâu tuỳ ý. Backend builder, hình dạng `ChartParams` của pivot, `buildPivot` + `PivotTable`
ở frontend, và phần điều khiển pivot trong explore đều thay đổi. RBAC, `sql_literal`, kiểm tra
tên, `MAX_CHART_ROWS`, và viz Table giữ nguyên.

## 1. Chiến lược SQL — ROLLUP × ROLLUP trong một truy vấn

Một pivot trên chiều hàng `R = [r1..rm]` và chiều cột `C = [c1..cn]` với measure `M` phát:

```sql
SELECT <r1..rm exprs>, <c1..cn exprs>, <M exprs>,
       GROUPING(r1) AS __grp_0, …, GROUPING(rm) AS __grp_{m-1},
       GROUPING(c1) AS __grp_{m}, …, GROUPING(cn) AS __grp_{m+n-1}
FROM   <source>
[WHERE …]
GROUP BY ROLLUP(<r1..rm exprs>), ROLLUP(<c1..cn exprs>)
LIMIT n
```

Tích chéo của hai ROLLUP cho đúng `(m+1)·(n+1)` grouping set trong **một truy vấn**:

- cả hai prefix đầy đủ → **ô lá (leaf)**,
- prefix hàng rút ngắn, cột đầy → **subtotal hàng**,
- hàng đầy, prefix cột rút ngắn → **subtotal cột**,
- cả hai rút ngắn → **cross-subtotal**, và `(0,0)` → **grand total**.

Vì ROLLUP sinh *prefix*, các chiều "active" trong một hàng luôn là prefix đầu; nên các cờ
`GROUPING()` của một hàng quy về hai số nguyên: **độ dài prefix hàng active `i`** (0..m) và
**độ dài prefix cột active `j`** (0..n), đủ để phân loại hàng. `GROUPING(dim) = 1` nghĩa là
chiều đó bị cuộn (inactive) trong hàng.

Cả **Trino** và **StarRocks** đều nhận nhiều phần tử grouping trong một `GROUP BY`
(`GROUP BY ROLLUP(a,b), ROLLUP(c,d)`) và hàm scalar `GROUPING()` — SQL chuẩn. Test gated theo
engine và test tích hợp Trino thủ công kiểm chứng phương ngữ (tên hàm marker và cú pháp ROLLUP
nhạy hợp đồng engine).

Nếu `C` rỗng, phát một `ROLLUP(R)`; nếu `R` rỗng, một `ROLLUP(C)`.

## 2. Backend — `egret-bi`

### 2.1 `chartquery` — tổng quát hoá rollup

Thay trường `rollup: bool` trên `QuerySpec` bằng:

```rust
pub struct QuerySpec {
    pub metrics:    Vec<String>,
    pub dimensions: Vec<String>,        // dims trong SELECT, theo thứ tự: hàng rồi cột (pivot)
    pub columns:    Vec<String>,
    pub filters:    Vec<Filter>,
    pub orderby:    Vec<OrderBy>,
    pub row_limit:  i64,
    pub rollup_groups: Vec<Vec<String>>, // mỗi Vec con = tên các dim để ROLLUP thành một nhóm
}
```

`build()` chế độ aggregate:

- SELECT mỗi biểu thức đã giải của `dimensions[i]` `AS "<name>"`, rồi mỗi metric `AS "<name>"`.
- **GROUP BY:** nếu `rollup_groups` rỗng → `GROUP BY <tất cả dim exprs>` (thuần, trường hợp
  Table). Ngược lại → `GROUP BY ROLLUP(<group0 exprs>), ROLLUP(<group1 exprs>), …`. Mọi tên
  trong mọi nhóm cũng phải có trong `dimensions` (kiểm tra; sai → 400).
- **Cờ GROUPING:** khi `rollup_groups` không rỗng, thêm `GROUPING(<expr>) AS __grp_<i>` cho mỗi
  `dimensions[i]` (chỉ số = vị trí trong `dimensions`), để frontend ánh xạ cờ `i` → dimension
  `i`.
- Bên gọi pivot truyền `dimensions = rows ++ cols` và `rollup_groups = [rows, cols]` (bỏ nhóm
  rỗng). Cờ `0..m-1` là chiều hàng, `m..m+n-1` là chiều cột.

Test ROLLUP một-chiều từ 3c-3 viết lại theo trường mới; các test aggregate/raw/filter/
`sql_literal` giữ nguyên.

### 2.2 Không đổi backend khác

`charts/data`, `ChartRepo`, routes, RBAC, và cột JSONB `params` giữ nguyên (`params` đã lưu JSON
tuỳ ý). Bố cục pivot nằm hoàn toàn trong `params`.

## 3. Frontend — engine pivot (`buildPivot` v2)

`src/components/chart/pivot/engine.ts` — thuần, unit-test cặn kẽ. Port từ `multiLevelPivot` của
Metabase. Đầu vào: kết quả phẳng `{ columns, rows }` + `{ rowDims, colDims, metrics,
showRowTotals, showColTotals, condense, collapsed }`. Đầu ra một `Pivot`:

```ts
interface HeaderItem {
  path: (string | null)[];   // giá trị dim thô gốc→node
  value: string;             // giá trị hiển thị
  depth: number;             // 0 = ngoài cùng
  span: number;              // số hàng/cột lá phủ
  offset: number;            // vị trí trong trục phẳng
  hasChildren: boolean;
  isSubtotal: boolean;       // node "Totals for X"
  isGrandTotal: boolean;
  isCollapsed: boolean;
  isMetric?: boolean;        // mức top-header trong cùng khi >1 metric
  metric?: string;
}
interface Pivot {
  leftHeaderItems: HeaderItem[];   // cây hàng đã làm phẳng
  topHeaderItems: HeaderItem[];    // cây cột đã làm phẳng (+ mức metric)
  rowDimNames: string[];
  colDimNames: string[];
  // giá trị ô: (rowLeafPath, colLeafPath, metric) → giá trị đã format (+ cờ isSubtotal)
  cell: (rowPath: (string|null)[], colPath: (string|null)[], metric: string) => Cell;
}
```

Thuật toán (theo báo cáo Metabase):

1. **Lập chỉ mục** các cột marker (`__grp_*`) và cột giá trị dim/metric.
2. Mỗi hàng phẳng tính `(i, j)` = độ dài prefix hàng/cột active từ cờ.
3. **Dựng cây hàng** từ các hàng `(i=m)` (prefix hàng đầy) theo giá trị chiều hàng; **cây cột**
   từ các hàng `(j=n)`. Node mang `value`, `children`, một map `value→childPos`.
4. **Chèn node subtotal** (khi `showRowTotals`/`showColTotals`) dưới mỗi nhóm không-lá; bỏ qua
   khi `condense` và nhóm chỉ có một con. **Thêm grand total.**
5. **Làm phẳng** mỗi cây theo chiều sâu thành `HeaderItem[]` tính `depth`, `offset`, `span`,
   `hasChildren`.
6. **Map giá trị:** key ô lá theo `rowPath ++ colPath ++ metric`; key ô subtotal theo các hàng
   grouping prefix-một-phần tương ứng (các hàng `(i<m)` / `(j<n)`). `cell()` tra map đúng theo
   độ dài path yêu cầu.
7. **Đa measure:** khi `metrics.length > 1`, thêm một mức metric cuối vào top header (một lá mỗi
   metric dưới mỗi lá cột). Một metric → không thêm mức.
8. **Thu/mở:** `collapsed` là tập path nhóm-hàng đã serialize; node bị thu ẩn lá con và chỉ hiện
   subtotal. Trạng thái client thuần.

## 4. Frontend — bộ render (`PivotTable` v2)

`src/components/chart/pivot/PivotTable.tsx` — một `<table>` HTML điều khiển bởi danh sách header
phẳng (không ảo hoá ở giai đoạn này):

- **Top headers:** một `<tr>` mỗi mức chiều-cột (+ một hàng metric khi >1 metric); mỗi ô
  `colSpan = item.span`; một nhóm "Row totals" ở cuối khi `showRowTotals`.
- **Left headers:** các hàng body lá mang ô chiều-hàng dùng `rowSpan = item.span` để gom nhóm;
  hàng subtotal span các cột chiều-hàng bên trong và style khác biệt; một hàng "Grand total" ở
  cuối khi `showColTotals`. Một caret thu/mở trên nhóm có con bật/tắt `collapsed`.
- **Body cells:** `cell(rowLeafPath, colLeafPath, metric)`, canh phải + format locale cho metric
  số (chuỗi-số được ép kiểu, như `ChartTable`).

## 5. Điều khiển Explore

Điều khiển pivot trong `ChartExplore` thành ba **multi-select có thứ tự** (Rows/Columns/Values
kiểu Metabase): **Rows** (chiều hàng có thứ tự), **Columns** (chiều cột có thứ tự), **Values**
(metric có thứ tự), cộng checkbox **Row totals**, **Column totals**, **Condense duplicate
totals**. Hiện thực bằng danh sách chip thêm/bớt có thứ tự trên cột và metric của dataset (một
cột chỉ ở Rows hoặc Columns, không cả hai; calc column là dim hợp lệ). `specForRun` dựng
`dimensions = rows ++ cols`, `rollup_groups = [rows, cols]` (bỏ rỗng), `metrics = values`. Guard:
pivot cần ≥1 chiều hàng và ≥1 metric.

Hình dạng `ChartParams` pivot:

```ts
{ spec: QuerySpec, rows: string[], cols: string[], metrics: string[],
  showRowTotals: boolean, showColTotals: boolean, condense: boolean }
```

(Trạng thái thu/mở là session-local, không lưu giai đoạn này.)

## 6. Phạm vi

**Trong:** pivot N×N×N hàng/cột/metric; header đa mức xếp tầng; subtotal mọi mức + grand total
với toggle `showRowTotals`/`showColTotals`/`condense`; nhóm hàng thu/mở; đa measure ở mức
top-header trong cùng; format số.

**Ngoài (→ 3c-3.3):** render ảo hoá cho pivot rất lớn; đổi kích thước cột; định dạng có điều
kiện/heatmap ô; drill-through trên ô; lưu trạng thái thu/mở; xuất pivot CSV/XLSX; thu/mở nhóm cột
(giai đoạn này chỉ thu/mở nhóm hàng).

## 7. Xử lý lỗi

- Pivot không có chiều hàng hoặc không có metric → chặn phía client (toast) và, phòng thủ,
  builder vẫn sinh SQL hợp lệ.
- Tên dim/metric không xác định trong `rollup_groups`/`dimensions` → 400 (kiểm tra sẵn có).
- Kết quả suy biến (không có cột `__grp_*`) render như bảng một-nhóm phẳng — không crash.

## 8. Kiểm thử

- **Unit test `chartquery`:** `rollup_groups = [[r1,r2],[c1]]` phát
  `GROUP BY ROLLUP(...), ROLLUP(...)` với cờ `__grp_<i>` đúng cho mọi dim; `rollup_groups` rỗng
  → GROUP BY thuần; quote backtick StarRocks qua đường này.
- **Unit test `engine.ts` (lõi):** pivot 2×1, 1×2, 2×2, đa metric, chèn node subtotal + condense,
  grand total, thu ẩn lá con, phân loại marker→(i,j), giá trị dim null.
- **Test gated Trino:** chạy pivot ROLLUP×ROLLUP 2×1 trên `tpch.tiny.orders`, kiểm tra leaf +
  subtotal + grand-total xuất hiện (theo cờ).
- **Test render frontend:** pivot 2×1 render header trái xếp tầng (rowSpan), một hàng subtotal,
  một hàng grand-total.
- **Smoke Playwright thủ công** trên stack chạy thực; **test tích hợp Trino thủ công** mở rộng cho
  đường ROLLUP×ROLLUP.

## 9. Cấu trúc tệp

- Sửa `crates/egret-bi/src/chartquery.rs` (rollup_groups), test của nó, và test engine.
- Mới `frontend/src/components/chart/pivot/engine.ts` (+ tests) và
  `frontend/src/components/chart/pivot/PivotTable.tsx` (+ test); xoá
  `frontend/src/components/chart/PivotTable.tsx` cũ và test của nó (đã thay thế).
- Sửa `frontend/src/api/charts.ts` (hình dạng pivot `ChartParams`, `rollup_groups` trên
  `QuerySpec`) và `frontend/src/routes/explore/ChartExplore.tsx` (điều khiển + `specForRun` +
  hydration).
