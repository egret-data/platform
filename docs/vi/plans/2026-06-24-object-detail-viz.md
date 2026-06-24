# Object Detail Visualization — Plan (tham chiếu VN)

**Bản chuẩn:** `docs/en/plans/2026-06-24-object-detail-viz.md`. **Spec:**
`docs/en/specs/2026-06-24-object-detail-viz-design.md`. **Branch:** `feat/object-detail-viz`. Chỉ frontend.
Sweep `npm run typecheck && lint && build && vitest` (từ `frontend/`) trước mỗi commit; footer
Co-Authored-By; không push tới Task 4.

**Task 1 — `ObjectDetail.tsx`** (mới): props `columns: ResultColumn[]` (từ ChartTable), `rows`,
`columnSettings?`. State `index`, `useEffect([rows])` reset 0. Rỗng → "No data". Render `rows[index]` thành
`<dl>` dọc `label : value`, format bằng `formatValue(row[i], col.type, columnSettings[col.name])`. Footer
nav (chỉ khi `rows.length>1`): `‹`/`›` (aria-label "previous/next record", disable ở biên) + "i / N". Test:
record đầu, next→record 2, prev disable đầu/next disable cuối, 1 record ẩn nav, rỗng "No data". Commit.

**Task 2 — Đăng ký viz:** `VizType` += `"object"` (`charts.ts`); `CHART_TYPES` += entry object (icon
`FileText`, `chartTypes.ts`); `ChartRenderer` thêm nhánh `if (vizType === "object") return <ObjectDetail
columns rows columnSettings />` (cạnh nhánh table). Sweep + commit.

**Task 3 — Builder (`ChartExplore.tsx`)** reuse cột table:
- `specForRun()` nhánh object (TRƯỚC fallback table): `{ metrics:[], dimensions:[], columns: tableCols,
  filters, orderby:[], row_limit: rowLimit, rollup_groups:[] }`.
- Hydrate: `else if (c.viz_type === "object") setTableCols(p.spec?.columns ?? [])` trước `else` cuối.
- Guard trong `run()` (và save): `if (viz === "object" && tableCols.length === 0) { toast.error("Object
  detail needs at least one column"); return; }`.
- Block điều khiển `{viz === "object" && (...)}`: CHỈ list checkbox cột (label "Fields", ghi `tableCols`
  qua `toggleTableCol`), không metrics/aggregate/unit-list. Đặt cạnh block `{viz === "table" && (...)}`.
- Preview: viz không phải table/pivot → qua ChartRenderer sẵn, không sửa.
Sweep + commit.

**Task 4 — Verify + PR:** sweep. Playwright (chart throwaway, 0 console error): Object detail trên
ecom_orders, chọn vài field, Run → record 1 hiện label:value + "1/N", prev disable; next → record 2; save +
đặt vào card dashboard throwaway, lật được; xoá throwaway. Push + PR.
