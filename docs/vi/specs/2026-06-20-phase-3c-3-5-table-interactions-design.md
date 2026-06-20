# Phase 3c-3.5 — Tương tác bảng (resize / show-hide / sticky / pin / pagination)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-20-phase-3c-3-5-table-interactions-design.md`.
> Thuần frontend. Xây trên 3c-3.4 (đã merge `3b740c7`). Branch `feat/phase-3c-3-5-table-interactions`.

## Mục tiêu

Cho viz **Table** các tương tác cột/bảng kiểu Metabase, toàn bộ dùng TanStack-table native + CSS
(không thêm dependency, không đổi backend — [[metabase-fidelity-preference]]):

1. **Resize cột** — kéo mép header; width được lưu.
2. **Show/hide cột** — bật/tắt cột trong tab Display.
3. **Sticky header** — header đứng yên khi cuộn dọc.
4. **Pin cột đầu (freeze trái)** — N cột đầu đứng yên khi cuộn ngang.
5. **Pagination** — tùy chọn; toggle + chọn page size.

Lưu trong `params` JSONB của chart (giống `columnSettings`), hydrate/save bởi `ChartExplore`.

## Phạm vi

**Trong phạm vi (chỉ viz Table):** 5 tương tác trên cho `ChartTable.tsx` (đã dùng
`@tanstack/react-table`), một section mới `TableColumnsSection` trong tab Display, field `tableState`
trên `ChartParams`, và wiring hydrate/persist trong `ChartExplore`.

**Ngoài phạm vi:** viz **Pivot** (giữ nguyên — pivot dùng HTML table custom; tương tác riêng của nó
= collapse/sticky/sort nằm ở **3c-3.6**); **kéo-thả đổi thứ tự cột** (để 3c-3.6 — cần dnd-kit);
**dòng totals/grand-total footer** cho table (Table thường của Metabase không có — chỉ Pivot có
totals); mọi thay đổi backend/engine/query; lưu sort hay page-index. Virtualization (thừa với cap
≤1000 dòng).

Quyết định chốt khi brainstorm: **chỉ Table**; controls = **resize trực tiếp trên header + một
"Columns" manager trong Display**; pagination = **toggle, mặc định TẮT (cuộn + sticky header), page
size 25/50/100**.

## Kiến trúc

State controlled do `ChartExplore` làm chủ (chủ persistence, đúng như đã làm với
`columnSettings`/`conditionalFormatting`). Một object `tableState` truyền cho **cả** renderer
(`ChartTable`) **lẫn** manager trong Display (`TableColumnsSection`); cả hai cùng gọi `setTableState`.
`ChartTable` vẫn là renderer controlled — map `tableState` sang state của TanStack table và emit thay
đổi (resize) lên trên. Không thêm dependency; persistence thuần JSONB.

## 1. Persistence — `ChartParams += tableState`

`frontend/src/api/charts.ts`:

```ts
export interface TableState {
  columnWidths?: Record<string, number>;   // colName -> px (chỉ resize thủ công)
  hiddenColumns?: string[];                 // colName[] đang ẩn
  pinnedLeft?: number;                      // freeze N cột đầu (0 = không)
  pagination?: { enabled: boolean; pageSize: number };  // pageSize ∈ {25, 50, 100}
}

export interface ChartParams {
  // …các field hiện có…
  tableState?: TableState;   // chỉ cho viz table; pivot bỏ qua
}
```

Key theo **tên cột**, không theo index (bền, khớp cách keying của `columnSettings`/conditional).
Không đổi backend — `params` là blob JSONB round-trip nguyên vẹn.

## 2. `ChartTable.tsx` — renderer controlled

Props thêm `tableState?: TableState` và `onTableStateChange?: (next: TableState) => void`. Component
giữ `sorting` local; phần còn lại derive từ `tableState`.

**Reconciliation (phòng thủ):** trước khi dựng table state, loại key của cột không còn tồn tại và
clamp `pinnedLeft` theo số cột — để `tableState` cũ sau khi đổi dataset/cột không bao giờ crash hay
ẩn nhầm cột.

- **Resize** — `columnResizeMode: "onChange"`, `enableColumnResizing: true`; handle resize trên mỗi
  header cell. Derive `columnSizing` từ `tableState.columnWidths` (name → px, map sang id `col_${i}`).
  `onColumnSizingChange` ghi width mới lại **theo tên cột** qua `onTableStateChange`. Min `MIN_COL_WIDTH = 60`.
- **Show/hide** — derive `columnVisibility` từ `tableState.hiddenColumns` (ẩn = `false`). Bật/tắt từ
  Display manager (phase này không bật/tắt từ header).
- **Sticky header** — bảng nằm trong container cuộn có giới hạn (`max-h-[60vh] overflow-auto` trên
  frame có viền hiện tại); `thead th` thêm `sticky top-0 z-10` + `bg-muted/40` sẵn có để các dòng cuộn
  ở dưới.
- **Pin trái** — `columnPinning.left` = id của `pinnedLeft` cột đầu. Ô pinned thêm `sticky left-0 z-20`
  với `left` offset tính (tổng width các cột pinned trước), nền đục, và viền phải để phân tách khối
  đóng băng. Ô góc trên-trái (cột pinned trong sticky header) cần z cao nhất (`z-30`).
- **Pagination** — khi `tableState.pagination?.enabled`, thêm `getPaginationRowModel()` và state
  `pagination` controlled seed từ `pageSize`; render footer dưới bảng "Showing X–Y / Z", nút Prev/Next,
  và page size. Page index là **local** (reset về 0 khi đổi; không lưu). Khi tắt, render hết dòng (như
  hiện tại).

Nếu `ChartTable` phình to → tách phần dựng table-instance ra hook `useChartTableInstance` cùng thư mục
— giữ JSX renderer gọn.

## 3. `settings/TableColumnsSection.tsx` — mới (tab Display)

Section controlled mirror `FormatSection`/`ConditionalFormattingSection`:

```tsx
export function TableColumnsSection({
  columns, value, onChange,
}: { columns: ResultColumn[]; value: TableState; onChange: (next: TableState) => void }) { … }
```

- **Columns** — mỗi cột kết quả 1 checkbox (tick = hiện); toggle cập nhật `hiddenColumns`.
- **Freeze first N columns** — ô number nhỏ (0…columns.length) → `pinnedLeft`.
- **Pagination** — toggle (`pagination.enabled`); khi bật, `Select` page size (25/50/100) →
  `pagination.pageSize`.
- Hint empty-state ("Run the chart to configure columns") khi chưa có cột kết quả, như các section
  Display khác.

## 4. Wiring `ChartExplore.tsx`

- State mới `const [tableState, setTableState] = useState<TableState>({})`.
- Hydrate: trong effect hydrate chart đã lưu, `setTableState(p.tableState ?? {})`.
- Reset: `resetControls()` cũng clear `tableState` về `{}` (khi đổi dataset).
- Persist: `paramsForSave()` thêm `tableState` cho nhánh **table**
  (`{ spec, columnSettings, conditionalFormatting, tableState }`); nhánh pivot không đổi.
- Tab Display: khi `viz === "table"`, render `<TableColumnsSection columns={resultColumns}
  value={tableState} onChange={setTableState} />` phía trên các section Format/Conditional.
- Preview: truyền `tableState={tableState}` và `onTableStateChange={setTableState}` cho `<ChartTable>`.

## 5. Xử lý lỗi

- `tableState` cũ sau khi đổi cột/dataset → reconcile trong `ChartTable` (bỏ key lạ, clamp
  `pinnedLeft`); `TableColumnsSection` chỉ liệt kê cột hiện tại.
- Sticky cần container giới hạn → frame mang `max-h-[60vh]`; bảng ngắn thì đơn giản không cuộn.
- z-index pinned + sticky-header xử lý tường minh (header z-10, pinned z-20, pinned-header z-30).
- Page size luôn thuộc {25, 50, 100}; page index do TanStack clamp.

## 6. Kiểm thử

Unit (Vitest + jsdom, đặt cạnh trong `__tests__`):
- **ChartTable**: cột ẩn không render; `pinnedLeft=1` đặt class sticky lên ô cột đầu; bật pagination +
  pageSize 25 chỉ render 25 dòng body và footer hiện range; click Next sang trang; mô phỏng đổi
  column-sizing gọi `onTableStateChange` với width key theo tên cột; key `tableState` cũ (cột lạ) bị
  bỏ qua không lỗi.
- **TableColumnsSection**: render 1 checkbox/cột; bỏ tick phát `hiddenColumns`; đổi freeze-N phát
  `pinnedLeft`; toggle pagination + page size phát `pagination`.
- Test renderer/section/engine/gallery hiện có giữ xanh.

Smoke Playwright thủ công: resize cột → Save → mở lại → width khôi phục; ẩn cột → mất và lưu; freeze
cột đầu → đứng yên khi cuộn ngang; bật pagination (page size 25) → footer + Next chạy; cuộn dọc →
header dính.

## 7. Cấu trúc tệp

- Sửa `frontend/src/api/charts.ts` — thêm `TableState` + `ChartParams.tableState`.
- Sửa `frontend/src/components/chart/ChartTable.tsx` — resize/visibility/pin/sticky/pagination (có thể
  tách `useChartTableInstance.ts`).
- Tạo `frontend/src/components/chart/settings/TableColumnsSection.tsx` (+ test).
- Tạo/mở rộng `frontend/src/components/chart/__tests__/ChartTable.test.tsx`.
- Sửa `frontend/src/routes/explore/ChartExplore.tsx` — state `tableState` + hydrate + persist +
  section trong Display + props ChartTable.
- Tái dùng nguyên: `format.ts`, `conditional.ts`, `PivotTable`, `FormatSection`,
  `ConditionalFormattingSection`, các route/repo/backend chart.
