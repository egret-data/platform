# Dashboard Editor — Parity Metabase, Đợt 2 (Thiết kế)

**Trạng thái:** Đã duyệt · **Ngày:** 2026-06-24 · **Phạm vi:** chỉ frontend, không đổi backend.

> Tham chiếu tiếng Việt. Bản chuẩn: `docs/en/specs/2026-06-24-dashboard-editor-metabase-parity-r2-design.md`.

## Vấn đề
Editor egret chạy được nhưng chưa giống Metabase. Khác biệt (so sánh live egret vs Metabase):
nút thêm là dải có chữ (MB: icon gọn căn phải hàng title); Cancel/Save ở hàng title (MB: ở
banner); Text/Heading 2 nút (MB: 1 dropdown T▾); sidebar tự mở (MB: mở khi bấm +); tabs dùng
double-click + icon hover (MB: menu ▾ mỗi tab); không sửa tên dashboard inline (MB có); preview
"Add section" ra khối đặc (MB: hiện mini-layout); phải bấm bánh răng để cấu hình card (MB: click
card là chọn).

## Thiết kế (7 phần — số liệu/anchors xem bản EN)

- **A. Relayout chrome:** Cancel/Save vào **banner xanh** (căn phải); bỏ dải toolbar có chữ; hàng
  title (edit) hiện **hàng icon gọn căn phải**, (view) giữ controls cũ + Edit.
- **B. `DashboardEditToolbar` icon-only:** `+`(Chart) · `T▾`(dropdown Heading/Text) · `🔗`(Link) ·
  `▦`(Section dropdown) · `⏚`(Filter); icon + tooltip, không chữ.
- **C. Preview Section:** vẽ lại rõ — hộp viền nền card, mỗi ô card có **khe hở 1px**, thanh heading
  khác màu ô → đọc được layout (KPI grid 2×2 / large+KPIs phải / KPIs+chart dưới).
- **D. Sidebar theo `editPanel`:** `startEdit` đặt `editPanel=null` (canvas full, không sidebar);
  sidebar chỉ hiện khi `editing && editPanel`; bỏ `sidebarCollapsed`/pill. `+` mở add; click card mở
  card; close (X) → `editPanel=null` (ẩn); sub-panel có back `‹` về add.
- **E. Tabs menu ▾:** mỗi tab (edit) thêm nút **`▾`** mở menu Rename/Duplicate/Delete (thay icon
  hover Copy/X); giữ kéo-thả reorder + active underline; Delete tắt khi `<2` tab.
- **F. Sửa tên inline:** thêm `draftName`; edit → tiêu đề thành `<input>` (style như heading); Save
  gửi `name: draftName.trim() || dashboard.name`.
- **G. Click card để chọn + bỏ bánh răng:** edit → click thân card (inner wrapper, trừ placeholder)
  gọi `openPanel({kind:"card",cardId})` (ring sẵn có hiển thị chọn); **bỏ nút Settings (gear)** khỏi
  `CardEditToolbar` (giữ viz/duplicate/replace/remove). View mode không đổi.

## Kiểm thử
- **Unit:** name round-trip; T▾ ra Heading/Text gọi đúng handler; tab menu Rename/Duplicate/Delete;
  click chart card mở panel card, click placeholder mở chart picker; `startEdit` không có sidebar, `+`
  mở, close ẩn. Sửa các test cũ (dải có chữ / sidebar auto-open / Cancel-Save ở title / gear) cho khớp,
  không gút assertion.
- **Live (Playwright, 0 console error, dashboard nháp):** so cạnh Metabase 7 điểm; save round-trip còn
  nguyên; xoá dashboard nháp + screenshot sau.

## Files
- **Sửa:** `routes/explore/DashboardView.tsx`, `dashboard/DashboardEditToolbar.tsx`,
  `dashboard/DashboardTabBar.tsx`, `dashboard/EditSidebar.tsx`, `dashboard/CardEditToolbar.tsx`.
- **Backend:** không.

## Rủi ro
- Mọi nơi từng dùng `sidebarCollapsed` phải chuyển sang `editPanel`; sidebar hiện khi mở panel, ẩn khi
  close. Click-to-select không được kích khi bấm toolbar nổi / kết thúc kéo (đặt handler ở inner
  wrapper; placeholder giữ handler riêng). Chỉ dùng token OKLCH.
