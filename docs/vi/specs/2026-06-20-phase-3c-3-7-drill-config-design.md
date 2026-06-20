# Phase 3c-3.7 — Drill-through + pivot full-width + redesign config panel

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-20-phase-3c-3-7-drill-config-design.md`.
> Thuần frontend. Xây trên 3c-3.6 (đã merge `e3a9b4e`). Branch `feat/phase-3c-3-7-drill-config`.

## Mục tiêu

Ba cải tiến cho trang Chart Explore, đều thuần frontend và bám Metabase ([[metabase-fidelity-preference]]):

1. **Drill-through** trên Table + Pivot — click ô value → menu **"Filter by {dim} is {value}"** (thêm
   filter `=`, chạy lại cùng viz) và **"See these records"** (mở modal hiện các dòng thô đứng sau ô).
   Một **thanh filter chips** hiện/gỡ filter đang áp.
2. **Pivot full-width** — bảng pivot lấp đầy chiều rộng preview (thay vì chừa khoảng viền trống), tinh
   chỉnh để không nhìn thưa thớt.
3. **Redesign config panel** (cả tab **Data** lẫn **Display**) — hệ thị giác kiểu Metabase đồng nhất:
   chip field kéo-thả, section header in hoa nhất quán, toggle dạng switch.

## Phạm vi

**Trong phạm vi (thuần frontend):**
- Drill: hàm thuần `cellDrill` dựng filter, `DrillMenu` popover trên ô value của cả 2 renderer,
  `DrillRecordsDialog` (query dòng thô trong modal), thanh `FilterChips`; state `filters` đi qua
  `specForRun()` và lưu trong `params.spec.filters` (đã có trong `QuerySpec`).
- CSS pivot full-width.
- Redesign config: `FieldZone` tái dùng (chip dnd-kit + reorder) thay `OrderedPicker` cho Rows/Columns/
  Values của pivot; section header + shadcn `Switch` đồng nhất ở tab Display; restyle các section
  Format/Conditional/Columns.

**Ngoài phạm vi:** mọi thay đổi backend/engine/SQL — `chartquery::build` đã render `filters`
(eq/ne/gt/ge/lt/le/like/in/not_in/is_null/is_not_null qua `sql_literal` security-critical) vào `WHERE`,
và đã có raw-records mode (`metrics: []` → `SELECT columns … WHERE …`). Drill vào chart đồ thị (chưa
có). Kéo field giữa Rows↔Columns — nice-to-have, để sau. Drill trên bảng thô (dòng đã là record). Toán
tử filter mới ngoài `=` từ drill (thanh chips vẫn hiện được filter bất kỳ đã lưu).

Quyết định chốt khi brainstorm: gộp cả 3 trong một phase; drill = kiểu Metabase (cả "Filter by" +
"See these records"); see-records → modal tại chỗ; redesign config gồm **cả 2 tab**.

## Kiến trúc

Cùng cách tách như các phase chart trước: filter-builder thuần (TDD), UI ở component, `ChartExplore`
làm chủ state `filters` + persistence. Không cần hạ tầng re-query: drill tái dùng
`useBuildChartData` → `/charts/data` → `useSubmitQuery`/`/queries` polling. `switch.tsx` đã có (không
thêm primitive). dnd-kit (thêm ở 3c-3.6) chạy reorder của FieldZone.

## 1. Drill-through

### 1.1 Filter-builder thuần — `frontend/src/components/chart/drill.ts`

```ts
import type { ChartFilter } from "@/api/charts";

export interface DrillContext { dims: { col: string; value: unknown }[] }

/** Ô value pivot → các dim của (rowPath, colPath). */
export function pivotCellDrill(
  rowDims: string[], rowPath: (string | null)[],
  colDims: string[], colPath: (string | null)[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  rowDims.forEach((c, i) => { if (i < rowPath.length && rowPath[i] != null) dims.push({ col: c, value: rowPath[i] }); });
  colDims.forEach((c, i) => { if (i < colPath.length && colPath[i] != null) dims.push({ col: c, value: colPath[i] }); });
  return { dims };
}

/** Ô value bảng tổng hợp → các dim (cột dimension) của dòng. */
export function tableCellDrill(
  dimensionColumns: string[], columns: string[], row: unknown[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  for (const c of dimensionColumns) {
    const i = columns.indexOf(c);
    if (i >= 0 && row[i] != null) dims.push({ col: c, value: row[i] });
  }
  return { dims };
}

/** Dựng filter `=` cho "Filter by" / "See records" từ một drill context. */
export function drillFilters(ctx: DrillContext): ChartFilter[] {
  return ctx.dims.map((d) => ({ col: d.col, op: "eq" as const, value: d.value }));
}
```

### 1.2 Menu drill — `frontend/src/components/chart/DrillMenu.tsx`
Một `Popover` (hoặc `DropdownMenu`) của shadcn neo tại ô click. Props: `ctx: DrillContext`,
`onFilterBy: (filter: ChartFilter) => void`, `onSeeRecords: (filters: ChartFilter[]) => void`. Render:
- mỗi `ctx.dims` một mục **"Filter by {col} is {value}"** → `onFilterBy({col, op:"eq", value})`;
- một mục **"See these records"** → `onSeeRecords(drillFilters(ctx))`.
Khi `ctx.dims` rỗng (ô grand-total), chỉ hiện "See these records".

Mỗi renderer (ô value `<td>` của ChartTable, PivotTable) thêm `onClick` tính `DrillContext` và mở menu
neo tại đó. Ô thành trigger kiểu button (`cursor-pointer`); renderer nhận callback `onCellDrill?(ctx,
anchorEl)` để state menu nằm ở ChartExplore (hoặc một popover wrapper nhỏ). Pivot chỉ drill ô value
**leaf** với `ctx` khác rỗng; dòng subtotal/grand drill với dim một phần mà chúng mang.

### 1.3 Modal see-records — `frontend/src/components/chart/DrillRecordsDialog.tsx`
Một `Dialog` của shadcn (modal — `dialog.tsx` đã có và đã forwardRef đúng; KHÔNG thêm `Sheet`). Props:
`open`, `onOpenChange`, `ws`, `datasetId`, `columns: ResultColumn[]` (mọi cột dataset), `filters:
ChartFilter[]`. Khi mở, dựng `QuerySpec` **raw** (`metrics: []`, `columns: <mọi tên cột>`,
`dimensions: []`, `filters`, `orderby: []`, `row_limit`, `rollup_groups: []`), chạy qua
`useBuildChartData` + `useSubmitQuery` + `useQueryStatus`/`useResults`, và render `ChartTable` của kết
quả. Xử lý loading / empty / failed trong modal. Tiêu đề tóm tắt filter (vd "Records where priority =
1-URGENT").

### 1.4 Thanh filter chips — `frontend/src/components/chart/FilterChips.tsx`
Render `filters` đang áp (`ChartFilter[]`) dạng chip gỡ được ("`{col}` = `{value}` ×"), đặt phía trên
preview. Props: `filters`, `onRemove(index)`. Gỡ chip → chạy lại.

### 1.5 Wiring (`ChartExplore.tsx`)
- State mới `const [filters, setFilters] = useState<ChartFilter[]>([])`; hydrate từ
  `p.spec?.filters ?? []`; reset khi đổi dataset; **`specForRun()` set `filters`** (thay vì `[]`). Tự
  persist qua `paramsForSave` (filters nằm trong `spec`).
- **Filter by:** append filter (dedupe theo `col` — thay eq cũ cùng cột), rồi `run()`.
- **See records:** mở `DrillRecordsDialog` với drill filters + cột của dataset hiện tại.
- Thanh `FilterChips` render `filters`; gỡ → chạy lại.

## 2. Pivot full-width

`PivotTable.tsx`: `<table>` thêm `w-full`; các ô `<th>` row-header (ô `scope="row"` và header tên-row-
dim/góc) đặt `w-px whitespace-nowrap` để co về nội dung, dồn phần dư vào các cột value (số canh phải
giãn ra nhìn ổn). Tinh chỉnh trực tiếp bằng screenshot để tránh lỗi "giãn thưa" của 3c-3.4; nếu phân bổ
dư bằng `w-px` vẫn xấu, fallback dùng một cột spacer flex ở cuối. Offset sticky-left (3c-3.6) không đổi
(vẫn pin cột row-dim đầu).

## 3. Redesign config panel (Data + Display)

Hệ thị giác chung cho cả 2 tab:
- **Section header** (component/style): in hoa, nhỏ, semibold, muted, có divider dưới nhẹ — áp cho mọi
  nhóm (Dataset, Rows, Columns, Values, Totals, Formatting, Conditional, Columns).
- **`FieldZone`** — `frontend/src/components/chart/config/FieldZone.tsx`: thay `OrderedPicker`. Một zone
  có label chứa các **chip** (grip icon + tên field + `×`), **kéo-thả sắp xếp trong zone** bằng dnd-kit
  (`SortableContext`), và nút **"+ Add"** (button nhỏ mở popover list các field còn lại, thay `Select`
  thô). Props: `label`, `selected: string[]`, `options: string[]`, `onChange`. Dùng cho Rows / Columns
  / Values của pivot. (Metrics/Columns checkbox của table dùng cùng section-header + chip styling chỗ
  hợp lý, giữ semantics checkbox.)

### Tab Display
- Toggle (Row totals / Column totals / Condense; và Paginate của table) → shadcn **`Switch`**
  (`ui/switch.tsx` đã có) kèm label, layout hàng nhất quán.
- Section Format / Conditional / Columns dùng lại section-header chung; bộ chọn cột của Format restyle
  theo ColumnItem của Metabase (icon kiểu dữ liệu + tên); rule conditional render dạng card sạch hơn
  (ô màu + điều kiện + ×). Không đổi hành vi — chỉ bọc logic `FormatSection` /
  `ConditionalFormattingSection` / `TableColumnsSection` sẵn có.

## 4. Persistence / hydration
`filters` nằm trong `params.spec.filters` (đã có trong `QuerySpec`) — `paramsForSave`/hydration chỉ cần
đưa `filters` vào `specForRun` và đọc lại. Pivot full-width và restyle config là trình bày (không đổi
param). Không đổi backend.

## 5. Xử lý lỗi
- Drill ô không có dim (grand total) → chỉ "See these records"; query raw filter rỗng trả mọi dòng (giới
  hạn bởi `row_limit`).
- "Filter by" trùng cột → thay giá trị cũ (không chồng).
- Query modal lỗi → báo lỗi trong modal; đóng thì giải phóng state query.
- FieldZone có `selected` cũ không nằm trong `options` → vẫn render chip (gỡ được); options loại bỏ cái
  đã chọn.

## 6. Kiểm thử
Unit (Vitest):
- `drill.ts`: `pivotCellDrill` (leaf → mọi row+col dim; subtotal → một phần; grand → không),
  `tableCellDrill` (chỉ cột dimension, bỏ null), `drillFilters` (mỗi dim một eq).
- `DrillMenu`: render một mục "Filter by" mỗi dim + "See these records"; gọi đúng callback.
- `FilterChips`: render một chip mỗi filter; gỡ gọi `onRemove(i)`.
- `FieldZone`: render một chip mỗi selected; "+ Add" thêm; `×` gỡ; reorder cập nhật thứ tự.
- Test renderer/engine/section sẵn có giữ xanh.

Smoke Playwright thủ công: pivot → click ô leaf → "Filter by priority = 1-URGENT" → chip xuất hiện +
pivot thu hẹp; click ô → "See these records" → modal hiện dòng thô; gỡ chip → khôi phục; pivot full
width; tab Data+Display hiện chip/switch/section header mới; Save → mở lại → filters hydrate.

## 7. Cấu trúc tệp
- Tạo `frontend/src/components/chart/drill.ts` (+ test).
- Tạo `frontend/src/components/chart/DrillMenu.tsx` (+ test).
- Tạo `frontend/src/components/chart/DrillRecordsDialog.tsx`.
- Tạo `frontend/src/components/chart/FilterChips.tsx` (+ test).
- Tạo `frontend/src/components/chart/config/FieldZone.tsx` (+ test).
- Sửa `frontend/src/components/chart/ChartTable.tsx` — onClick drill ô value + prop `dimensionColumns`.
- Sửa `frontend/src/components/chart/pivot/PivotTable.tsx` — onClick drill ô value + `w-full` full-width.
- Sửa `frontend/src/routes/explore/ChartExplore.tsx` — state filters + wiring drill + modal + FilterChips
  + FieldZone cho pivot + restyle tab Display (Switch + section header).
- Sửa các section Display (`settings/FormatSection.tsx`, `ConditionalFormattingSection.tsx`,
  `TableColumnsSection.tsx`) — chỉ styling (section header, Switch, look ColumnItem).
- Tái dùng nguyên: `format.ts`, `conditional.ts`, `engine.ts`, route/repo/backend chart, và các primitive
  UI sẵn có `ui/switch.tsx`, `ui/dialog.tsx`, `ui/popover.tsx`, `ui/dropdown-menu.tsx` (đều có — không cần
  thêm primitive shadcn mới).
