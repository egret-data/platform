# Grid dashboard Metabase — Kế hoạch triển khai

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-metabase-grid.md`. Spec: `docs/en/specs/2026-06-23-metabase-grid-design.md`.

**Mục tiêu:** Thay grid egret bằng thuật toán Metabase thật — rowHeight động, compaction dọc, margin 6px, kích thước theo viz,
nền lưới SVG, handle SE. **FRONTEND-ONLY**. Branch `feat/dashboard-edit-ux` (gộp PR #39, thay phần ⑤ ×3).

**Quy ước:** TDD; full sweep trước mỗi commit (`npm run typecheck && lint && build && npx vitest run`); identity git + footer.
**⚠️ Sequencing:** gỡ các export scale (Task 1) làm hỏng import ở DashboardView/AutoHeightMeasure/GridOverlay → **Task 1→4 + 5.1-5.2 phải là MỘT commit** (1 sweep ở cuối Task 4). CSS (5.3) + live (6) commit riêng.

## Task 1 — `dashboard-grid.ts` (Metabase math)
- Test (viết lại `dashboard-grid.test.ts`, bỏ case ×3/pixel-identity): `gridRowHeight(1200)=45` + clamp 40; `cellWidth`; `contentRowsFor(px,rowHeight)` biên ở `rowHeight+6` (51→1, 52→2); `getPositionForNewDashCard` (rỗng→{0,0}, kế→{4,0}, đầy hàng→{0,2}); `sizeForViz` (table 12×9/min4×3, scalar 6×3/min2×2, heading 24×1, pivot_table→12×9, smartscalar min2×2, boxplot/unknown→generic 12×6/min4×3).
- Giữ `GRID_COLS 24`, `GRID_ASPECT_RATIO 10/9`, `GRID_MIN_ROW_HEIGHT 40`; thêm `GRID_MARGIN=[6,6]`, `GRID_PADDING=[0,0]`, `cellWidth(w)=(w-(24-1)*6)/24`. **Gỡ** `ROW_PX/ROW_SCALE/GRID_MARGIN_X/Y/toLayoutRows/fromLayoutRows/CARD_DEFAULT_SIZE`. `contentRowsFor(px,rh)=max(1,ceil(px/(rh+6)))`. Thêm `getPositionForNewDashCard(cards,sx,sy,width=24)`+`intersects` (port, dùng field grid_col/grid_row/size_x/size_y). Thêm `CARD_SIZES_BY_VIZ` + `sizeForViz(key)` fallback generic.

## Task 2 — `grid-background.ts` + xoá GridOverlay
- Test `grid-background.test.ts`: trả `url("data:image/svg+xml...")`; chứa `var(--border)`, KHÔNG `hsl(`; đúng số `<rect>`=cols + `width`/`height` ô.
- `generateGridBackground({cellWidth,rowHeight,cols,marginX,marginY,stroke})`: 1 rect/cột (`x=i*(cellWidth+marginX)`), `<svg>` → encodeURIComponent → `url(...)`. `git rm GridOverlay.tsx`.

## Task 3 — `DashboardGridLayout.tsx` (wrapper)
- Đo width (ref+ResizeObserver, guard jsdom) → `rowHeight=gridRowHeight(width)`, `cellWidth(width)`. Nền SVG (khi `editing && width>0`) `absolute inset-0 z-0` sau RGL.
- **RGL thường** (không WidthProvider, tự cấp `width`): `cols=24, rowHeight, margin=GRID_MARGIN, containerPadding=GRID_PADDING, compactType="vertical", isDraggable/isResizable={editing}, draggableHandle ".dash-draghandle", draggableCancel ".dash-nodrag", layout, onLayoutChange` + 4 handler drag/resize. Handle SE mặc định.
- `children: (rowHeight)=>ReactNode` (render-prop) để card nhận rowHeight cho auto-height. Tạo `dashboard-grid.css` rỗng tạm (Task 5 điền).

## Task 4 — `DashboardView.tsx`
- Import: bỏ `WidthProvider(RGL)` + css RGL (chuyển vào wrapper), chỉ giữ `type Layout`; import `GRID_COLS, sizeForViz, getPositionForNewDashCard` + `DashboardGridLayout`; bỏ import GridOverlay.
- `addCard`: `sizeForViz(chart.viz_type).default` + `getPositionForNewDashCard(cards,w,h)`. `addVirtualCard`: tương tự (heading 24×1, text 12×3, link 8×1).
- layout map **grid-unit thô**: `{i,x:grid_col,y:grid_row,w:size_x,h:size_y, minW:min(sizeForViz(vizKey).min.w, size_x), minH: autoH?size_y:min(min.h,size_y), maxH nếu autoH}`; `vizKey`= chart.viz_type (card chart) else card_type.
- `onLayoutChange` ghi thô `grid_col:l.x, grid_row:max(0,l.y), size_x:l.w, size_y:l.h` (giữ short-circuit same-ref).
- Render `<DashboardGridLayout editing layout onLayoutChange onResize/Drag…>{(rowHeight)=>visibleCards.map(...giữ nguyên body card, thêm rowHeight={rowHeight} vào <DashboardCardView/>)}</DashboardGridLayout>`.
- **Sweep + commit gộp Task 1-4 + 5.1-5.2** (`feat(dashboards): adopt Metabase grid math ...`).

## Task 5 — Auto-height động + CSS handle
- `AutoHeightMeasure.tsx`: thêm prop `rowHeight`, `contentRowsFor(px, rowHeight)`, dep `[onRows, rowHeight]`. `DashboardCardView.tsx`: thêm prop `rowHeight?`, truyền vào AutoHeightMeasure (cả wrap + ChartCardTile); Task 4 đã truyền `rowHeight` vào DashboardCardView.
- `dashboard-grid.css` (commit riêng): `.react-grid-item > .react-resizable-handle{opacity:0;transition}` + `:hover>` → `opacity:1`; `-se{cursor:nwse-resize}`; dragging/resizing `z-index:3`.

## Task 6 — Live verify + finalize PR #39
- Sweep xanh. `npm run dev`, login admin, **KHÔNG đụng a1**; tạo dashboard mới, thêm Table+Bar+Scalar; mở Metabase (3001) cạnh để so. Xoá sau.
- Playwright, 0 lỗi: rowHeight≈`max(floor(W/24/(10/9)),40)` (~45px @ ~1100-1200); margin ~6px; compaction dồn lên; nền SVG căn đúng + ẩn ở view; handle SE hover-resize + min giữ; card mới Table 12×9 / Scalar 6×3 / Bar 12×6; auto-height fit+khoá trên rowHeight động; PARITY (read-only, KHÔNG save) re-flow mạch lạc.
- Xoá throwaway; `git push`; cập nhật mô tả PR #39 (grid → Metabase, thay ×3); squash-merge khi CI xanh (ranger flaky → rerun).

## Thứ tự
Task 1→4 + 5.1-5.2 (1 commit) → 5.3 CSS → 6 live + PR.
