# Export chart (CSV + PNG) — Kế hoạch triển khai

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-chart-export.md`. Spec: `docs/en/specs/2026-06-23-chart-export-design.md`.

**Mục tiêu:** Tải dữ liệu chart ra **CSV** và ảnh render ra **PNG**, từ **chart builder** + **dashboard card**.
**FRONTEND-ONLY** (không backend). Branch `feat/chart-export` (off main, spec đã commit).

**Quy ước mọi task:** TDD (test đỏ trước). Full sweep trước mỗi commit (từ `frontend/`):
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage path tường minh; identity git + footer
Co-Authored-By. **jsdom KHÔNG có canvas/Image/Blob-download** → chỉ unit-test `rowsToCsv`/`exportFilename` + logic item của
`ChartExportMenu` (mock download); `svgToPngBlob`/`downloadBlob` verify ở live test (Task 5).

## Task 1 — `export-chart.ts` utils
- Test đỏ `__tests__/export-chart.test.ts`: `rowsToCsv` (header + body CRLF; quote khi có `,"`/newline, double `"`; null/undefined→rỗng;
  `0`/`false` stringify); `exportFilename` (sanitize `[\/:*?"<>|]`→`_`, fallback "chart", append `.ext`).
- Implement: `csvCell` (null→rỗng, regex `[",\n]`→quote), `rowsToCsv`, `exportFilename`, `downloadBlob` (createObjectURL +
  `a[download]` click + revoke), `downloadCsv`, `svgToPngBlob(svg, scale=2)` (XMLSerializer → Blob svg+xml → Image.onload →
  canvas (fill trắng) → drawImage → `toBlob('image/png')`), `downloadPng`. `ResultColumn` từ `@/api/charts`.

## Task 2 — `ChartExportMenu.tsx`
- Test đỏ `__tests__/ChartExportMenu.test.tsx` (mock `../export-chart`): có `<svg>` → CSV+PNG; không có → chỉ CSV; click CSV gọi
  `downloadCsv(title, columns, rows)`; `disabled` khóa trigger (`aria-label="export chart"`).
- Implement: dùng shadcn `dropdown-menu` (đọc file xác nhận export). Props `{title, columns, rows, containerRef, disabled?}`.
  Trigger Button ghost icon `Download`. `onOpenChange(open)` → `setHasSvg(!!containerRef.current?.querySelector("svg"))`.
  Item "Download CSV" luôn; "Download PNG" chỉ khi `hasSvg` → `downloadPng(...).catch(toast.error)`. Nếu Radix `onSelect`
  không fire trong jsdom → đổi sang `onClick` (giữ nguyên call).

## Task 3 — Wire `ChartExplore.tsx`
- Đọc file: tìm `resultColumns` (~dòng 202), `results.data?.rows`, `finished`, hàng nút Run/Save, tên chart hiển thị (truyền làm
  `title`; nếu không có sẵn → `undefined`, `exportFilename` fallback "chart"), container preview (~dòng 713+).
- Thêm `previewRef = useRef<HTMLDivElement>(null)` gắn vào div bọc preview. Render `<ChartExportMenu>` cạnh Run/Save với
  `columns={resultColumns} rows={results.data?.rows ?? []} containerRef={previewRef} disabled={!finished || resultColumns.length===0}`.

## Task 4 — Wire `DashboardCardView.tsx` / `ChartCardTile`
- Test đỏ `dashboard/__tests__/DashboardCardView.export.test.tsx` (mock `ChartRenderer` + `useChartData` — khớp path import thật):
  card chart đã load (view mode) → có button `export chart`; đang loading → không; `editing` → không.
- Implement: thêm prop `editing?: boolean` cho `DashboardCardView` + `ChartCardTile` (thread qua call cuối file). Trong
  `ChartCardTile`: `tileRef`, `canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length??0)>0`;
  outer div → `ref={tileRef} className="group relative flex h-full flex-col"`; affordance hover góc phải (`dash-nodrag absolute
  right-1 top-1 z-10 opacity-0 group-hover:opacity-100`) → `<ChartExportMenu title={headerText||chart?.name} columns={data.columns}
  rows={data.rows} containerRef={tileRef} />`. Trong `DashboardView.tsx`: truyền `editing={editing}` vào `<DashboardCardView>`.

## Task 5 — Live verify + PR
- Final sweep xanh. `npm run dev` (backend dev :8080 giữ nguyên — chỉ đổi frontend). Login admin@egret.local / Admin12345!, ws eng.
  **KHÔNG đụng a1** — dùng dashboard test, xóa sau.
- Playwright MCP, 0 lỗi console: builder chart ECharts → CSV (đúng header+rows) + PNG; chart **table** → chỉ CSV; card chart hover
  (view mode) → CSV+PNG; card table → chỉ CSV; affordance ẩn lúc edit/loading.
- push + PR `feat(chart): export chart to CSV + PNG (builder + dashboard cards)` + footer; squash-merge khi CI xanh (flaky
  `egret-ranger` có thể cần rerun, không liên quan).

## Thứ tự
Task 1 (utils) → Task 2 (menu) → Task 3 (builder) → Task 4 (dashboard card) → Task 5 (live + PR).
