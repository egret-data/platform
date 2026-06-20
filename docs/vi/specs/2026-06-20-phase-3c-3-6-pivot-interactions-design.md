# Phase 3c-3.6 — Tương tác pivot (collapse / sticky / dimension-sort) + đổi thứ tự cột bảng

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-20-phase-3c-3-6-pivot-interactions-design.md`.
> Thuần frontend. Xây trên 3c-3.5 (đã merge `565cb64`). Branch `feat/phase-3c-3-6-pivot-interactions`.

## Mục tiêu

Đưa viz **Pivot** ngang Metabase về tương tác, và thêm **kéo-thả đổi thứ tự cột** (hoãn từ 3c-3.5)
cho viz **Table**. Tất cả client-side trên kết quả ROLLUP sẵn có (không re-query), lưu trong `params`
JSONB ([[metabase-fidelity-preference]]):

**Phần A — Pivot:**
1. **Collapse/expand nhóm dòng** — `[–]`/`[+]` trên ô header nhóm có subtotal; nhóm gập chỉ hiện dòng
   subtotal.
2. **Sticky header + sticky cột trái** — header và các cột row-header đứng yên khi cuộn.
3. **Dimension sort** — sắp xếp giá trị một row-dimension tăng/giảm (đúng Metabase: áp vào thứ tự cây,
   KHÔNG phải sort theo measure).

**Phần B — Table:**
4. **Kéo-thả đổi thứ tự cột** — kéo header để đổi thứ tự (dnd-kit), lưu theo tên cột.

## Phạm vi

**Trong phạm vi:**
- Phần A trên engine pivot (`engine.ts`) + renderer (`PivotTable.tsx`): wire `PivotInput.collapsed`
  (đã khai báo sẵn) vào `buildPivot`; thêm input sort theo dimension; thêm UI toggle + sticky CSS.
- Phần B trên `ChartTable.tsx`: `@dnd-kit/core` + `@dnd-kit/sortable` đổi thứ tự cột qua TanStack
  `columnOrder`, lưu trong `tableState.columnOrder`.
- Field mới `ChartParams`: `collapsedRows` + `pivotSort` (pivot-only) và `TableState.columnOrder`
  (table-only); wiring hydrate/persist trong `ChartExplore`.

**Ngoài phạm vi:** mọi thay đổi backend/engine(SQL)/query (tất cả client-side trên kết quả ROLLUP);
sort theo **column-dimension** (chỉ row-dimension có chỗ click tự nhiên — để sau); **measure sort**
cho pivot (Metabase không có; phá cấu trúc cây/subtotal/collapse); resize/pin/pagination cho pivot
(pivot là table custom, không TanStack — không theo đuổi); virtualization. Bảng Table thường đã có
resize/show-hide/pin/sticky/pagination từ 3c-3.5 — phase này chỉ thêm reorder cho nó.

Quyết định chốt khi brainstorm: pivot = **collapse + sticky + dimension sort (row dims, click ô tên
row-dim để cycle none→asc→desc)**; **gộp** kéo-thả đổi thứ tự cột bảng bằng dnd-kit.

## Kiến trúc

Cùng cách tách như các phase chart trước: logic thuần trong `engine.ts` (TDD), UI/CSS trong renderer,
`ChartExplore` làm chủ state tương tác và lưu vào `params`. Không re-query: collapse và sort tái-định-
hình kết quả ROLLUP đã fetch ở client; SQL (luôn phát mọi grouping set) không đổi.

## 1. Persistence — field mới

`frontend/src/api/charts.ts`:

```ts
export interface TableState {
  // …đã có: columnWidths, hiddenColumns, pinnedLeft, pagination…
  columnOrder?: string[];   // chỉ table: tên cột theo thứ tự hiển thị
}

export interface ChartParams {
  // …các field đã có…
  collapsedRows?: string[];                       // chỉ pivot: path nhóm dòng gập (serialized)
  pivotSort?: Record<string, "asc" | "desc">;     // chỉ pivot: tên row-dimension -> hướng sort
}
```

Đều key theo **tên / path serialized**, reconcile với dimension/cột hiện hành (bỏ entry lạ). Không đổi
backend — `params` là JSONB round-trip nguyên vẹn.

## 2. Phần A1 — Collapse/expand (engine + renderer)

### Engine (`pivot/engine.ts`)
`PivotInput.collapsed: Set<string>` đã có nhưng `buildPivot`/`emit` bỏ qua. Wire vào:

- Path serialized = `JSON.stringify(path)` với `path` là các giá trị của nhóm dòng (vd `["2-HIGH"]`),
  khớp convention `pathKey` sẵn có.
- Trong `emit`, khi sắp đệ quy vào node nhóm con có path bị gập VÀ nhóm collapsible (có subtotal —
  `showColTotals && children.length>0 && (!condense || children>1)`): **không** emit subtree; emit
  **1 dòng** `kind: "subtotal"` cho path đó (tổng của nhóm gập), và không emit thêm subtotal trailing
  (dòng gập CHÍNH LÀ subtotal).
- Đánh dấu collapsibility lên ô header nhóm cho renderer vẽ toggle. Mở rộng `RowHeaderCell`:
  ```ts
  export interface RowHeaderCell {
    label: string; depth: number; rowSpan: number; colSpan: number;
    path?: (string | null)[];   // path nhóm mà ô này đại diện (cho toggle)
    collapsible?: boolean;      // nhóm có subtotal → gập được
    collapsed?: boolean;        // đang gập (renderer hiện [+] vs [–])
  }
  ```
  Nhóm mở collapsible: ô header nhóm (ô có `rowSpan = childSpan`) nhận `collapsible:true, collapsed:false, path`.
  Nhóm gập: dòng đơn được emit nhận `collapsible:true, collapsed:true, path`, label = giá trị nhóm.

### Renderer (`pivot/PivotTable.tsx`)
- Prop mới `collapsed?: Set<string>` (đã truyền qua `input.collapsed`) và
  `onToggleCollapse?: (serializedPath: string) => void`.
- Khi `RowHeaderCell.collapsible`, vẽ nút toggle nhỏ trước label: `▸` (gập) / `▾` (mở), gọi
  `onToggleCollapse(JSON.stringify(hc.path))`. Giữ truy cập bàn phím (dùng `<button>` thật).
- Có thể tách `pivot/RowToggle.tsx` nếu `PivotTable.tsx` phình.

### Wiring (`ChartExplore.tsx`)
- State `const [collapsed, setCollapsed] = useState<Set<string>>(new Set())`.
- Hydrate `setCollapsed(new Set(p.collapsedRows ?? []))`; reset khi đổi dataset.
- Persist (nhánh pivot) `collapsedRows: [...collapsed]`.
- Truyền `collapsed` + `onToggleCollapse` (thêm/bỏ path khỏi Set) cho `PivotTable`; input pivot đã nhận
  `collapsed`.

## 3. Phần A2 — Sticky header + sticky cột trái

Trong `PivotTable.tsx` (table thuần), tái dùng pattern sticky của 3c-3.5:
- Bọc table trong `max-h-[60vh] overflow-auto` (frame `overflow-auto rounded-md border` hiện có thêm
  `max-h`).
- Ô `thead` → `sticky top-0` + `bg-muted/40` sẵn có + `z-10`.
- `rowDimNames.length` cột `<th>` row-header đầu + ô góc/tên-row-dim → `sticky left-0` với `left` offset
  tính, nền đục, `z-20`; ô góc (sticky cả hai trục) `z-30`.
- Ô row-header dùng `rowSpan`; `position: sticky` chạy được. Offset: cột row-dim mục đích cố định;
  dùng `left: 0` cho cột đầu và offset cộng dồn cho các cột sau (hoặc `left-0` cho trường hợp 1 cột
  row-dim — case phổ biến).

## 4. Phần A3 — Dimension sort (row dimensions)

### Engine
- `buildPivot` nhận hướng sort (thêm `sortDims?: Record<string, "asc"|"desc">` vào `PivotInput`, mặc
  định `{}`). Trong `buildTree` (hoặc sort sau khi build mỗi `children`), sắp con của mỗi cấp theo giá
  trị dimension của cấp đó bằng `localeCompare` (so sánh số khi cả hai parse được số), asc/desc; vắng
  ⇒ giữ thứ tự xuất hiện (như hiện tại).
- Dimension của cấp cây `d` (0-based) là `rowDims[d]`; tra `sortDims[rowDims[d]]`.

### Renderer + wiring
- Ô tên row-dimension (các `<th>` `rowDimNames` ở top header) thành clickable: click cycle dimension đó
  `none → asc → desc → none`, hiện ` ▲`/` ▼`.
- `ChartExplore` state `pivotSort`; hydrate từ `p.pivotSort ?? {}`; reset khi đổi dataset; persist
  (nhánh pivot); truyền vào input `PivotTable` dưới dạng `sortDims` + callback `onCycleSort(dimName)`.

## 5. Phần B — Kéo-thả đổi thứ tự cột bảng (dnd-kit)

- Thêm dep `@dnd-kit/core` + `@dnd-kit/sortable` (+ `@dnd-kit/utilities`).
- Trong `ChartTable.tsx`: derive TanStack `columnOrder` từ `tableState.columnOrder` (tên →
  id `col_${i}`, reconcile: bỏ tên lạ, cột thiếu nối thêm theo thứ tự tự nhiên). Bọc các ô header trong
  `SortableContext` của dnd-kit (ngang); mỗi header là item sortable kéo bằng grip nhỏ (hoặc chính
  header với sensor có activation distance để click vẫn sort). Khi thả (`onDragEnd`), tính thứ tự tên
  mới bằng `arrayMove` và persist `onTableStateChange({ ...tableState, columnOrder })`.
- Reorder không được phá resize/sort/pin: dnd-kit activation constraint (vd 8px) để click vẫn toggle
  sort và handle resize vẫn chạy; cột pinned có thể loại khỏi reorder (tắt drag trên header pinned) để
  giữ khối freeze liền mạch.

## 6. Xử lý lỗi

- `collapsedRows` trỏ path không còn tồn tại (đổi dataset/dimension) đơn giản không match trong `emit`
  — vô hại; khi persist chỉ giữ path còn hiện diện (lọc theo row paths đã build, hoặc chấp nhận thừa
  vô hại — ưu tiên lọc để tránh phình blob).
- Key `pivotSort` / `columnOrder` cho dimension/cột đã xoá bị bỏ qua trong engine/derivation.
- Sticky cần container bounded (`max-h`); bảng ngắn thì không cuộn. z-index: góc `z-30` > sticky-left
  `z-20` > sticky-top header `z-10`.
- Collapse chỉ cho nhóm thực sự có subtotal (tôn trọng `showColTotals`/`condense`).

## 7. Kiểm thử

Engine unit (Vitest, `pivot/__tests__/engine.test.ts` — mở rộng):
- Gập nhóm cấp trên → emit dòng subtotal của nó và **bỏ** dòng con + sâu hơn; ô header dòng gập là
  `collapsible && collapsed`.
- Ô header nhóm mở collapsible là `collapsible && !collapsed` kèm `path`.
- `sortDims` asc/desc sắp lại con của một cấp đúng (kể cả giá trị dạng số); vắng ⇒ thứ tự xuất hiện
  không đổi.
- Collapse + sort kết hợp: thứ tự áp dụng, subtree gập vẫn bị bỏ.

Renderer/unit:
- `PivotTable`: header collapsible vẽ nút toggle; click gọi `onToggleCollapse` với path serialized;
  header gập hiện `▸`, mở `▾`; class sticky có trên header + cột row-header đầu.
- `ChartTable`: `columnOrder` đổi thứ tự header render; `onDragEnd` reorder gọi `onTableStateChange`
  với thứ tự tên mới (test ở mức handler/state, không drag-pointer thật).

Smoke Playwright thủ công: pivot 2 row dims → gập một nhóm (con biến mất, hiện `[+]`) → mở lại; cuộn →
header + cột trái dính; click header row-dim → ▲/▼ đổi thứ tự nhóm; table → kéo header đổi thứ tự cột;
**Save → mở lại → collapse/sort/order hydrate hết**.

## 8. Cấu trúc tệp

- Sửa `frontend/src/api/charts.ts` — `TableState.columnOrder`, `ChartParams.collapsedRows` + `pivotSort`.
- Sửa `frontend/src/components/chart/pivot/engine.ts` — wire `collapsed`; thêm `sortDims`; mở rộng
  `RowHeaderCell`.
- Sửa `frontend/src/components/chart/pivot/PivotTable.tsx` — UI toggle, sticky CSS, header sort.
- Có thể tạo `frontend/src/components/chart/pivot/RowToggle.tsx`.
- Sửa `frontend/src/components/chart/ChartTable.tsx` — dnd-kit đổi thứ tự cột.
- Sửa `frontend/src/routes/explore/ChartExplore.tsx` — state collapsed/pivotSort + hydrate/reset/persist
  + props cho PivotTable; tableState.columnOrder đi qua wiring sẵn có.
- Sửa `frontend/package.json` — thêm `@dnd-kit/core`, `@dnd-kit/sortable`, `@dnd-kit/utilities`.
- Tests: mở rộng `pivot/__tests__/engine.test.ts`; mở rộng/thêm test `PivotTable` + `ChartTable`.
- Tái dùng nguyên: `format.ts`, `conditional.ts`, các settings section, route/repo/backend chart.
