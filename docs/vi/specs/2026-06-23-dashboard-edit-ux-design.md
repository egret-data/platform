# Dashboard edit UX polish — thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-dashboard-edit-ux-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/dashboard-edit-ux` (off main)
**Thuộc:** pha roadmap "polish & usability". Một tier khép kín xử lý 6 vấn đề UX khi **edit dashboard**.

## Mục tiêu
Edit mode mượt & dễ dùng: control card rõ ràng, picker "đổi chart" gọn, sidebar add-chart thu/mở được, tab sạch hơn, resize kiểu
Metabase (lưới + bước nhỏ + nhiều tay cầm), và auto-height cho card nội dung. **Frontend-only** — không backend, không migration.
Geometry lưu (`grid_col/grid_row/size_x/size_y`) **không đổi trên disk**; bước dọc mịn hơn nhờ transform lúc render.

## Code hiện có (đã verify)
`DashboardView.tsx` (~523 dòng): edit state + `<Grid>` RGL (cols 24, `ROW_PX=90`, margin 12, `isDraggable/isResizable={editing}`),
layout (~184-189), toolbar wrapper (~393-402), Dialog "Replace chart" (~490-501), sidebar (~423), `editPanel` (add|card|viz|filter).
`CardEditToolbar.tsx` (emoji ⚙⧉↺✕ + 1 lucide). `AddPanel.tsx` (search + list, `ICON_BY_VIZ`). `EditSidebar.tsx` (aside 340px).
`DashboardTabBar.tsx` (emoji ⧉✕+). `dashboard-grid.ts` (GRID_COLS=24, CARD_DEFAULT 6×4, CARD_MIN 2×2, `gridRowHeight()` chưa dùng).
`DashboardCardView.tsx` (`ChartCardTile`: header + container `min-h-0 flex-1 overflow-auto` = `chartRef`).

## Thiết kế từng mục
**① Icon toolbar card** — thay emoji bằng lucide đồng bộ: `Settings`, `BarChart3` (đổi viz), `Copy`, `RefreshCw` (đổi chart),
`Trash2`. Icon `h-4 w-4`, nút `size-7 rounded hover:bg-accent`, thêm `title`. Giữ `dash-nodrag`. Vị trí wrapper giữ nguyên.

**② "Change chart" picker** — component dùng chung mới `ChartPickerList.tsx` (`{charts,onPick,autoFocus}`): search + list cuộn
`max-h-[60vh]`, mỗi dòng icon viz + tên, click→onPick. Chuyển `ICON_BY_VIZ` ra `viz-icons.ts` (AddPanel + ChartPickerList dùng
chung). Dialog "Replace chart" → `<ChartPickerList … onPick={ch=>replaceCard(replacingId!,ch)} autoFocus/>`. Hết "hết hồn".

**③ Toggle sidebar** — `DashboardView`: `sidebarCollapsed` state. Mở: EditSidebar 340px. Thu: thanh hẹp `w-9` + nút mở
(`PanelLeftOpen`); grid full width. `EditSidebar` thêm nút thu (`PanelLeftClose`). Bấm settings/viz/filter của card → tự bung.

**④ Icon tab** — `⧉`→`Copy`, `✕`→`X` (`h-3.5`), `+`→`Plus`; nút duplicate/delete mỗi tab `opacity-0 group-hover:opacity-100`
(bọc `group`) → chỉ hiện khi hover. Delete vẫn disabled khi `<2` tab.

**⑤ Resize — lưới + bước nhỏ + tay cầm** (KHÔNG đổi geometry lưu):
- **Render-scale dọc:** `ROW_PX=30`, `ROW_SCALE=3`. layout: `y=row*3, h=size_y*3` (+`minH` scale); `onLayoutChange`:
  `grid_row=round(y/3), size_y=max(1,round(h/3))`. → mịn gấp 3 theo dọc, card cũ vẫn đúng pixel. Round cẩn thận (no-op không drift).
- **Lưới nền:** `GridOverlay.tsx` lớp absolute sau `<Grid>`, chỉ khi edit, vẽ ô bằng `repeating-linear-gradient` (cột
  `(width-2·pad)/24` × 30px), `pointer-events-none`. Đo width qua ResizeObserver.
- **Tay cầm:** `resizeHandles={['s','e','se']}` (thêm cạnh phải + đáy).

**⑥ Auto-height (table/pivot + text/heading/link, KHÔNG chart):**
- `card.options.autoHeight` (boolean). Toggle trong panel card settings, chỉ hiện cho loại hợp lệ.
- Bật: `DashboardCardView` đo content (ResizeObserver) → rows = `ceil(px/90)` → `onAutoHeight(cardId,rows)` cập nhật `size_y`
  nếu đổi ≥1 row (debounce, không lúc đang kéo). Layout item đặt `minH=maxH=size_y*3` → khoá dọc, ngang vẫn resize. Tắt → mở lại.

## Data flow
```
geometry (không đổi): grid_col,grid_row,size_x,size_y
  render: layout{x, y:row*3, w, h:size_y*3, minH, [maxH nếu autoHeight]}  (rowHeight 30)
  lưu: onLayoutChange → grid_row=round(y/3), size_y=round(h/3)
autoHeight: ResizeObserver content → ceil(px/90) rows → onAutoHeight → size_y → maxH=minH khoá dọc
ChartPickerList: charts→filter→onPick→(AddPanel:addChart | Dialog:replaceCard)
sidebarCollapsed: EditSidebar 340px ↔ rail w-9; mở panel settings → tự bung
```

## Xử lý lỗi / biên
- Round-trip ×3 rồi ÷3 của card không đụng phải = identity (Math.round, chỉ ghi item đổi); `size_y≥1`.
- Overlay `pointer-events-none`, ẩn ngoài edit. Auto-height: ngưỡng ≥1 row + bỏ qua lúc đang kéo + bỏ qua đo 0px → tránh loop.
- Tab delete disabled khi `<2`. Thu sidebar không mất `editPanel`. Chuyển `ICON_BY_VIZ`: cập nhật cả 2 importer cùng lúc.

## Kiểm thử
- **Vitest:** `ChartPickerList` (filter/onPick/empty); helper render-scale `toLayoutRow/fromLayoutRow` (round-trip identity, clamp);
  `contentRowsFor(px)=ceil(px/90)` (89→1, 90→1, 91→2); `CardEditToolbar` (5 nút theo aria-label); `DashboardTabBar`
  (duplicate/delete mỗi tab, delete disabled khi 1 tab).
- **Live (Playwright, 0 lỗi console, dashboard test, KHÔNG đụng a1):** icon rõ; Replace = list có search; thu/mở sidebar đổi width +
  bấm Settings tự bung; tab duplicate/delete hover bằng lucide; resize hiện lưới + bước ~30px + tay cầm cạnh/góc; **card dashboard
  cũ giữ nguyên pixel** sau đổi rowHeight/scale; bật auto-height card table → khít nội dung + khoá dọc.
- **Full sweep** trước mỗi commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`.

## Quy ước
Metabase fidelity · clean-room · TDD · full sweep · identity git + footer Co-Authored-By · docs song ngữ · squash-merge ·
branch `feat/dashboard-edit-ux`.

## Thứ tự task (cho plan)
①+④ icon → ② ChartPickerList + Replace dialog (+ tách ICON_BY_VIZ) → ③ thu sidebar → ⑤ lưới + render-scale + handles →
⑥ auto-height → live verify + PR.
