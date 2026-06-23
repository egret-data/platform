# Tier P1 — Export chart (CSV + PNG) (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-chart-export-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/chart-export` (off main)
**Thuộc:** giai đoạn roadmap "polish & usability" (P1 export → P2 drill→dashboard/question + per-column click → P3 date Exclude tab; time range/grain để pha time-series sau).

## Mục tiêu
Cho phép tải dữ liệu chart ra **CSV** và ảnh render ra **PNG**, từ **chart builder** và **dashboard card** — đóng gap so với
Metabase/Superset (egret nay chỉ export từ SQL Lab). Chỉ frontend: data đã có client-side, SVG rasterize trong trình duyệt;
không backend/migration.

## Phạm vi
**Có:** (1) util `rowsToCsv` + download; (2) `svgToPngBlob` rasterize; (3) `ChartExportMenu` (CSV luôn; PNG chỉ cho viz
ECharts); (4) wire vào chart builder (cạnh Run/Save) + dashboard card (hover ở view mode).
**Defer:** export "full result" vượt `row_limit` (MVP xuất theo dòng đang hiển thị); PNG cho table/pivot (không có `<svg>` →
chỉ CSV); Excel/XLSX; export server-side; export lịch/email.

## Code hiện có (đã verify)
- `ChartRenderer({ vizType, params, columns, rows })`: `columns: {name,type}[]`, `rows: unknown[][]`. Viz chart →
  `EChartsRenderer` (`init(el,null,{renderer:"svg"})` → render `<svg>`, không `foreignObject`); table → ChartTable; pivot → PivotTable.
- `ChartExplore`: có Run + state columns/rows + Save + preview pane.
- `DashboardCardView`/`ChartCardTile`: `useChartData` → `{columns,rows,isLoading,error}` → ChartRenderer.
- SQL Lab đã có download CSV qua `<a href download>` (precedent), nhưng dựa queryId/server; chart export là client-side.

## Components
1. **`components/chart/export-chart.ts`**: `rowsToCsv(columns,rows)` (RFC-4180: header + quote khi có `,"`/newline; null→rỗng;
   row sep CRLF); `exportFilename(title, ext)` (sanitize, fallback "chart"); `downloadBlob(filename, blob)`;
   `downloadCsv(title, columns, rows)`; `svgToPngBlob(svg, scale?)` (serialize → Image → canvas → toBlob('image/png'));
   `downloadPng(title, svg)`.
2. **`components/chart/ChartExportMenu.tsx`**: props `{ title, columns, rows, containerRef, disabled? }`. Nút **Download**
   (icon lucide): item **CSV** (luôn) + item **PNG** chỉ khi `containerRef.current?.querySelector("svg")` tồn tại (viz ECharts).
   `disabled` → khóa trigger. Tính PNG availability lúc mở menu (đọc DOM).

## Wiring
- **ChartExplore**: bọc preview bằng `previewRef`; render `<ChartExportMenu title={name} columns={resultColumns}
  rows={results.data?.rows ?? []} containerRef={previewRef} disabled={!hasResults} />` cạnh Run/Save.
- **DashboardCardView/ChartCardTile** (view mode): affordance hover góc card → `<ChartExportMenu title={cardTitle}
  columns={data.columns} rows={data.rows} containerRef={tileRef} disabled={data.isLoading||!!data.error||blocked} />`.
  Ẩn khi loading/blocked; gắn class `dash-nodrag` để RGL không cướp click.

## Data flow
```
CSV: columns+rows → rowsToCsv → Blob('text/csv') → downloadBlob
PNG: containerRef.querySelector('svg') → svgToPngBlob (serialize→Image→canvas→toBlob('image/png')) → downloadBlob
table/pivot (không <svg>) → chỉ CSV
```

## Xử lý lỗi
- 0 dòng → CSV chỉ header (vẫn tải). Trigger disabled khi loading/error/blocked hoặc chưa có result.
- Table/pivot → ẩn PNG, vẫn CSV. `svgToPngBlob` lỗi → toast, không chặn CSV. Card dùng `dash-nodrag`.

## Kiểm thử
- **Vitest:** `rowsToCsv` (header; quote `,"`/newline; null→rỗng; CRLF); `exportFilename` (sanitize + ext + fallback);
  `ChartExportMenu` (có `<svg>` → CSV+PNG; không có → chỉ CSV; disabled; click CSV gọi download — mock downloadBlob).
  (`svgToPngBlob` cần canvas+Image, jsdom không hỗ trợ → để live test.)
- **Live (Playwright, 0 lỗi console, dashboard test, KHÔNG đụng a1):** builder → Download CSV (đúng header+rows) + PNG (.png)
  cho chart ECharts; table → chỉ CSV; card chart (hover view-mode) → CSV+PNG; card table → chỉ CSV.
- **Full sweep** trước mỗi commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`.

## Quy ước
Metabase fidelity · clean-room · TDD · full sweep · identity git + footer Co-Authored-By · docs song ngữ · squash-merge ·
branch `feat/chart-export`.
