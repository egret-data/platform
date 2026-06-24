# Object Detail Visualization (Thiết kế)

**Bản tham chiếu tiếng Việt.** Bản chuẩn: `docs/en/specs/2026-06-24-object-detail-viz-design.md`.

## Mục tiêu

Thêm viz **Object detail**: hiển thị một bản ghi dạng danh sách dọc `trường: giá trị`, có nút prev/next
lật qua các bản ghi đã truy vấn. Đây là "Object detail" của Metabase, lấp chỗ clone E-commerce đang tạm
dùng table cho "Most recent subscription". **Chỉ frontend** — backend đã hỗ trợ truy vấn raw (`spec.columns`).

## Quyết định (brainstorm)

- **Chọn trường:** người dùng chọn cột hiển thị, reuse column-checkbox của table (ghi vào `tableCols`).
- **Phân trang:** truy vấn trả nhiều hàng (tới row_limit); viz hiện 1 bản ghi/lần + nút prev/next + chỉ số "i / N".

## Component

**1. Đăng ký viz**
- `charts.ts`: `VizType` += `"object"` (không phải cartesian → `isCartesianViz` giữ nguyên).
- `chartTypes.ts`: thêm `{ key:"object", name:"Object detail", description:"Inspect one record as
  field/value pairs, paging through results.", icon: FileText }`.

**2. `ObjectDetail.tsx`** (mới). Props: `columns: ResultColumn[]`, `rows`, `columnSettings?`.
- State `index` (bản ghi hiện tại), bắt đầu 0; `useEffect([rows])` reset 0 khi data đổi.
- Rỗng → "No data". Clamp `index` ≤ `rows.length-1`.
- Render `rows[index]` thành `<dl>` dọc: mỗi cột `label : value`, format bằng
  `formatValue(row[i], column.type, columnSettings[column.name])` (reuse formatter của table).
- Footer nav (chỉ khi `rows.length > 1`): nút `‹` (disable ở đầu), chỉ số `i+1 / N`, nút `›` (disable ở
  cuối); aria-label "previous record"/"next record".
- `h-full`, danh sách cuộn → lấp đúng card dashboard (theo quy ước fill-card sau PR #45).

**3. Render path** (`ChartRenderer.tsx`): `if (vizType === "object") return <ObjectDetail columns rows
columnSettings />`. Tự hoạt động ở builder preview lẫn card dashboard (đều qua ChartRenderer).

**4. Builder (`ChartExplore.tsx`)** — reuse cột của table:
- Block điều khiển khi `viz === "object"`: CHỈ list checkbox cột (markup giống phần "Columns" của table,
  ghi `tableCols`). Không metrics, không aggregate, không unit list.
- `specForRun()` nhánh object (đặt TRƯỚC fallback table): `{ metrics:[], dimensions:[], columns: tableCols,
  filters, orderby:[], row_limit: rowLimit, rollup_groups:[] }`.
- Hydrate: `else if (c.viz_type === "object") setTableCols(p.spec?.columns ?? [])`.
- Validation: `if (viz === "object" && tableCols.length === 0) { toast.error("Object detail needs at least
  one column"); return; }`.
- Preview: viz không phải table/pivot → qua ChartRenderer sẵn, không cần sửa.

## Test

Vitest `ObjectDetail.test`: render bản ghi đầu (label:value); next → bản ghi 2; prev disable ở đầu, next
disable ở cuối; chỉ số "i/N" đúng; rỗng → "No data"; 1 hàng → ẩn nav. Live verify (chart throwaway, 0
console error): object trên ecom_orders chọn vài cột → record 1 hiện trường:giá trị, lật prev/next; đặt vào
1 card dashboard throwaway, lật được; xoá throwaway. Sweep xanh.

## File

Sửa: `charts.ts`, `chartTypes.ts`, `ChartRenderer.tsx`, `ChartExplore.tsx`. Tạo: `ObjectDetail.tsx` + test.

## Ngoài phạm vi

Không đụng backend; không Map viz; không deep-link/drill từng record; không UI format riêng cho object
(reuse columnSettings). Object reuse `tableCols` — đổi Table↔Object giữ lựa chọn cột (chấp nhận được).
Sweep + git identity + footer Co-Authored-By; stage path tường minh.
