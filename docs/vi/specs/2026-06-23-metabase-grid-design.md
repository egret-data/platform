# Grid dashboard bám sát Metabase — thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-metabase-grid-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/dashboard-edit-ux` (tiếp tục; gộp vào PR #39)
**Thay thế:** mục resize (⑤) của `2026-06-23-dashboard-edit-ux-design.md` — gỡ hack ×3 render-scale, thay bằng thuật toán grid Metabase thật. Các mục ①④②③⑥ giữ nguyên.

## Mục tiêu
Làm grid dashboard egret giống Metabase bằng cách copy thuật toán thật: **rowHeight động theo aspect**, **compaction dọc**, **margin 6px**, **kích thước default/min theo viz**, **nền lưới SVG kiểu Metabase**, **1 tay cầm resize góc SE**. egret tự ghi "mirrors Metabase" và đã có `gridRowHeight()` nhưng chưa dùng. Frontend-only; data model (24 cột, size theo grid-unit) giữ nguyên.

## Tham chiếu Metabase (từ `metabase_bk`, đã verify)
- `lib/dashboard_grid.js`: `GRID_WIDTH=24`, `GRID_ASPECT_RATIO=10/9`, `MIN_ROW_HEIGHT=40`, `DEFAULT_CARD_SIZE 4×4`, `getPositionForNewDashCard` (quét trái→phải, trên→dưới), `intersects`.
- `DashboardGrid.tsx getRowHeight()`: `max((width/24)/(10/9), 40)` (floor; ceil khi có scrollbar). 1200px → 45px.
- RGL (`grid/GridLayout.tsx`): `Responsive`, `cols desktop:24`, `margin [6,6]`, `containerPadding [0,0]`, `compactType "vertical"`, `draggableCancel "[data-dontdrag]"`, handle SE mặc định. `cellWidth=(width-(cols-1)*6)/cols`.
- `grid/utils.ts generateGridBackground`: 1 `<rect>` mỗi cột → data-URL SVG, đặt làm `background` chỉ khi edit.
- `constants.cljc card-size-defaults`: table 4×3/12×9, scalar 2×2/6×3, bar/line/area/pie 4×3/12×6, heading 1×1/24×1, text 1×1/12×3, link 1×1/8×1, sankey 4×3/16×10...
- CSS handle: vùng 40×40 góc SE, `::after` viền mờ, ẩn tới khi hover card.

## Code egret hiện có (đã verify)
- `dashboard-grid.ts`: `GRID_COLS=24`, `GRID_ASPECT_RATIO=10/9`, `GRID_MIN_ROW_HEIGHT=40`, `CARD_DEFAULT_SIZE 6×4`, `CARD_MIN_SIZE 2×2`, `gridRowHeight()` (có nhưng **chưa dùng**). PR #39 thêm `ROW_PX/ROW_SCALE/GRID_MARGIN_X/Y/toLayoutRows/fromLayoutRows/contentRowsFor` — **spec này gỡ** (trừ `contentRowsFor`, sửa lại theo rowHeight động).
- `DashboardView.tsx`: `Grid=WidthProvider(RGL)`, layout map scale ×3, `onLayoutChange` chia lại, `margin [12,4]`, `resizeHandles ['s','e','se']`, `draggableHandle ".dash-draghandle"` + `.dash-nodrag` (giữ), `GridOverlay`, `interactingRef`/`applyAutoHeight`, `addCard/addVirtualCard`.
- `GridOverlay.tsx` (CSS gradient) → **thay** bằng SVG. `AutoHeightMeasure.tsx` (`contentRowsFor` 90 cứng) → **sửa** nhận rowHeight động.
- `chartTypes.ts CHART_TYPES`: key gồm table, pivot_table, bar, line, area, pie, scatter, row, scalar, smartscalar, gauge, progress, funnel, waterfall, combo, sankey, boxplot, trend; + card type text/heading/link.

## Thiết kế
**1. `dashboard-grid.ts`** — Giữ GRID_COLS/ASPECT/MIN; thêm `GRID_MARGIN=[6,6]`, `GRID_PADDING=[0,0]`, `cellWidth(width)`. **Gỡ** ROW_PX/ROW_SCALE/GRID_MARGIN_X/Y/toLayoutRows/fromLayoutRows. `contentRowsFor(px, rowHeight)=max(1, ceil(px/(rowHeight+marginY)))`. Thêm `getPositionForNewDashCard`/`intersects` (port Metabase). Thêm `CARD_SIZES_BY_VIZ` (default+min theo viz, mirror Metabase; `pivot`→`pivot_table`, `trend`/`smartscalar`→scalar-like 2×2/6×3, `boxplot`→generic 4×3/12×6) + `sizeForViz(key)` fallback generic `{default 12×6, min 4×3}`.

**2. `grid-background.ts`** (`generateGridBackground`) — port `grid/utils.ts`: 1 rect/cột, data-URL SVG. Stroke dùng `var(--border)` (OKLCH-safe, KHÔNG `hsl(var(--border))`). Xoá `GridOverlay.tsx`.

**3. `DashboardGridLayout.tsx`** (mới) — wrapper kiểu Metabase: đo width (ResizeObserver, guard jsdom) → `rowHeight=gridRowHeight(width)`, `cellW`. Nền SVG (khi edit) sau grid. Render **RGL thường** (không WidthProvider, tự cấp `width`) với `cols=24, rowHeight, margin [6,6], containerPadding [0,0], compactType "vertical", isDraggable/isResizable={editing}, draggableHandle ".dash-draghandle", draggableCancel ".dash-nodrag", layout, onLayoutChange` + 4 handler drag/resize start/stop. Handle SE mặc định (không truyền `resizeHandles`). Lộ `rowHeight` ra ngoài cho auto-height.

**4. `DashboardView.tsx`** — thay `<Grid>` bằng `<DashboardGridLayout>`. layout map grid-unit thô: `{i,x:grid_col,y:grid_row,w:size_x,h:size_y, minW:sizeForViz.min.w (clamp≤size_x), minH:sizeForViz.min.h (clamp≤size_y), maxH nếu autoHeight}`. `onLayoutChange` ghi thẳng `grid_col/grid_row/size_x/size_y`. `addCard` lấy `sizeForViz(viz).default` + `getPositionForNewDashCard`; `addVirtualCard` tương tự (heading 24×1, text 12×3, link 8×1). Giữ toolbar ①, interactingRef/applyAutoHeight ⑥.

**5. Auto-height** — `AutoHeightMeasure` nhận prop `rowHeight`, gọi `contentRowsFor(px, rowHeight)`. Khoá dọc `maxH=minH=size_y` giữ nguyên.

**6. CSS handle** — handle SE mặc định thành chỉ-báo góc mờ, ẩn tới khi hover card (edit mode), `cursor nwse-resize`.

## Data flow
```
width (ResizeObserver) → rowHeight=max((width/24)/(10/9),40), cellW=(width-23*6)/24
layout (grid-unit thô) → RGL(width, rowHeight, margin[6,6], padding[0,0], compactType vertical) → cards
onLayoutChange → grid_col/grid_row/size_x/size_y thô  (compaction có thể dời y khi load — chấp nhận)
card mới → sizeForViz(viz).default + getPositionForNewDashCard
auto-height → ResizeObserver px → contentRowsFor(px, rowHeight) → size_y (khoá maxH=minH)
edit → nền SVG (ô cellW × rowHeight) sau RGL
```

## Backward-compat / rủi ro
- **Dashboard cũ (gồm a1) re-flow**: rowHeight động → card thấp hơn (4 row ≈180px thay vì 360px); `compactType vertical` dồn card lên khi load. Đây là hành vi Metabase, đã chốt chấp nhận. Geometry trên disk không đổi cho tới khi save.
- `minW/minH` clamp ≤ size hiện tại (RGL cảnh báo nếu không).
- jsdom không có ResizeObserver: guard (width 0 → rowHeight 40); test không có nền SVG; verify nền SVG bằng live.

## Kiểm thử
- **Vitest:** `gridRowHeight(1200)===45` + clamp 40; `cellWidth`; `contentRowsFor(px,rh)` biên ở `rh+marginY`; `getPositionForNewDashCard`/`intersects` (chỗ trống đầu tiên); `sizeForViz` (viz biết → size Metabase; `boxplot`→generic; `pivot_table`/`heading`/`text`/`link` map đúng).
- **Live (Playwright, 0 lỗi console, dashboard test, KHÔNG đụng a1):** đo rowHeight ≈ công thức ở width thật; margin 6px; tỉ lệ card khớp Metabase cùng grid-size (mở cạnh nhau); compaction dồn lên; nền SVG hiện khi edit + căn đúng card; handle SE hover resize; card Table mới 12×9, Scalar 6×3; auto-height vẫn fit+khoá trên rowHeight động; PARITY (read-only) re-flow nhưng render mạch lạc.
- **Full sweep** trước mỗi commit; xoá/sửa test cũ assert ×3 scale.

## Quy ước
Metabase fidelity · clean-room (reimplement thuật toán public, không copy code proprietary) · TDD · full sweep · identity git + footer · docs song ngữ · gộp PR #39 · squash-merge.

## Thứ tự task
1. `dashboard-grid.ts` (constants/helpers Metabase + gỡ scale + test). 2. `grid-background.ts` + xoá GridOverlay. 3. `DashboardGridLayout.tsx`. 4. `DashboardView.tsx` (wrapper + layout thô + sizing/placement + auto-height). 5. CSS handle. 6. Live verify + PR.
