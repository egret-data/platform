# Dashboard edit UX polish — Kế hoạch triển khai

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-dashboard-edit-ux.md`. Spec: `docs/en/specs/2026-06-23-dashboard-edit-ux-design.md`.

**Mục tiêu:** Polish edit mode dashboard — icon card/tab rõ, picker "đổi chart" có search, sidebar add-chart thu/mở được,
resize kiểu Metabase (lưới + bước mịn + tay cầm), auto-height card nội dung. **Frontend-only**; geometry lưu KHÔNG đổi (render-scale ×3).
Branch `feat/dashboard-edit-ux` (off main, spec đã commit).

**Quy ước mọi task:** TDD (test đỏ trước). Full sweep trước mỗi commit (từ `frontend/`):
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage path tường minh; identity git + footer
Co-Authored-By. KHÔNG push tới Task 6.

## Task 1 — Icon toolbar card + tab (①④)
- Test đỏ: `CardEditToolbar.test.tsx` (5 nút theo aria-label cho card chart; ẩn viz/replace với card text); `DashboardTabBar.test.tsx`
  (duplicate/delete mỗi tab; delete disabled khi 1 tab). Các aria-label giữ nguyên → test xanh trước, refactor giữ xanh.
- `CardEditToolbar.tsx`: lucide `Settings/BarChart3/Copy/RefreshCw/Trash2`, `h-4 w-4`, nút `size-7 rounded hover:bg-accent`, thêm `title`.
- `DashboardTabBar.tsx`: `Copy/X` (`h-3.5`), `Plus`; bọc mỗi tab `group`, nút duplicate/delete `opacity-0 group-hover:opacity-100`.

## Task 2 — ChartPickerList + shared icons (②)
- Test đỏ: `ChartPickerList.test.tsx` (filter theo query, click row gọi onPick(chart), empty state) — dùng `user-event`.
- Tạo `viz-icons.ts` (`ICON_BY_VIZ` từ `CHART_TYPES`). Tạo `ChartPickerList.tsx` (`{charts,onPick,autoFocus}`: search + list cuộn
  `max-h-[60vh]`, mỗi dòng icon viz + tên). `AddPanel.tsx` dùng lại ChartPickerList + viz-icons. `DashboardView` Replace dialog →
  `<ChartPickerList charts={charts??[]} onPick={ch=>replaceCard(replacingId!,ch)} autoFocus/>`.

## Task 3 — Thu sidebar (③) — không unit test, verify live
- `EditSidebar.tsx`: prop `onCollapse`, header thêm nút `PanelLeftClose` (aria "collapse sidebar").
- `DashboardView.tsx`: state `sidebarCollapsed`; helper `openPanel(p)= setEditPanel(p)+setSidebarCollapsed(false)` dùng cho
  toolbar onSettings/onViz + filter chip onSelect. Khi thu: rail `size-9` nút `PanelLeftOpen` (aria "expand sidebar"); grid full width.

## Task 4 — Resize: render-scale + lưới + handles (⑤)
- Test đỏ: `dashboard-grid.test.ts` (`toLayoutRows(4)=12`; round-trip `fromLayoutRows(toLayoutRows(r))=r`; `ROW_PX*ROW_SCALE=90`;
  `contentRowsFor` 89→1/90→1/91→2/0→1).
- `dashboard-grid.ts`: thêm `ROW_PX=30`, `ROW_SCALE=3`, `toLayoutRows`, `fromLayoutRows=round(/3)`, `contentRowsFor=max(1,ceil(px/90))`.
- `GridOverlay.tsx`: lớp absolute `pointer-events-none z-0`, ResizeObserver đo width, `backgroundSize colW×ROW_PX`, opacity 0.4.
- `DashboardView.tsx`: bỏ `const ROW_PX=90`; import từ dashboard-grid + GridOverlay. layout map: `y=toLayoutRows(grid_row)`,
  `h=toLayoutRows(size_y)`, `minH` scale. `onLayoutChange`: `grid_row=max(0,fromLayoutRows(l.y))`, `size_y=max(1,fromLayoutRows(l.h))`.
  Bọc `<Grid>` trong `<div className="relative">` + `{editing && <GridOverlay …/>}`; Grid `rowHeight={ROW_PX}` + `resizeHandles={['s','e','se']}`.
  **QUAN TRỌNG:** card cũ phải ra đúng pixel (6×4 → 360px).

## Task 5 — Auto-height (⑥)
- `contentRowsFor` đã test ở Task 4. Đo + khoá verify live. Trước khi code: xác nhận key viz_type table/pivot trong `chartTypes.ts`
  (dự kiến `"table"`/`"pivot_table"`).
- `AutoHeightMeasure.tsx`: ResizeObserver đo `scrollHeight` → `contentRowsFor` → `onRows` khi đổi ≥1 row.
- `DashboardCardView.tsx`: props `autoHeight`/`onAutoHeight`; bọc body bằng `AutoHeightMeasure` khi bật (cả chart table + text/heading/link).
- `CardSettingsPanel.tsx`: `eligible = text/heading/link || (chart && viz table/pivot)`; thêm toggle `Auto height` (checkbox) vào các block hợp lệ.
- `DashboardView.tsx`: `interactingRef` (set qua Grid onDragStart/Stop + onResizeStart/Stop); `applyAutoHeight(id,rows)` cập nhật size_y
  nếu khác + không lúc đang kéo; layout map: nếu autoHeight → `maxH=minH=h` (khoá dọc); truyền `autoHeight`/`onAutoHeight` (chỉ khi editing).

## Task 6 — Verify live + PR
- Final sweep xanh. `npm run dev`, login admin, **KHÔNG đụng a1/PARITY**; tạo dashboard **mới** (tránh 409 trùng tên), thêm 1 chart Table + 1 Bar, xoá sau.
- Playwright, 0 lỗi console: icon rõ; Replace có search; thu/mở sidebar + Settings tự bung; resize có lưới + bước ~30px + tay cầm cạnh/góc;
  **card PARITY (xem read-only, KHÔNG save) giữ nguyên pixel**; auto-height table khít + khoá dọc, chart bar không có toggle; export PNG vẫn ảnh thật.
- push + PR `feat(dashboards): edit-mode UX polish (...)` + footer; squash-merge khi CI xanh (flaky ranger rerun).

## Thứ tự
Task 1 → 2 → 3 → 4 → 5 → 6. (Task 5 mở rộng layout map + Grid mà Task 4 tạo — làm đúng thứ tự.)
