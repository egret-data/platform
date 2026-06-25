# Kế hoạch: Dashboard Editor — Parity Metabase Đợt 2

> Tóm tắt tiếng Việt. Bản chi tiết (canonical) cho người/agent thực thi:
> `docs/en/plans/2026-06-24-dashboard-editor-metabase-parity-r2.md`.
> Spec: `docs/{en,vi}/specs/2026-06-24-dashboard-editor-metabase-parity-r2-design.md`.

**Mục tiêu:** editor egret giống Metabase — toolbar icon trên hàng title, Cancel/Save ở banner,
`T▾` gộp Heading/Text, sidebar mở theo nhu cầu, tabs có menu `▾`, sửa tên inline, preview Section
rõ, click card để chọn. Chỉ frontend.

**Quy ước:** chạy trong `egret/frontend/`. Sweep trước mỗi commit:
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Branch
`feat/dashboard-edit-metabase-polish`. Commit kết bằng
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **Không động `a1`** —
verify trên dashboard nháp rồi xoá.

## Các task (7)

1. **`DashboardEditToolbar` icon-only** — `+`(Chart) `T▾`(dropdown Heading/Text) `🔗`(Link)
   `▦`(Section) `⏚`(Filter); vẽ lại `SectionLayoutPreview` (hộp viền + khe hở giữa ô → đọc được
   layout). Props giữ nguyên (DashboardView chưa cần đổi).
2. **Bỏ bánh răng** khỏi `CardEditToolbar` (bỏ prop `onSettings` + nút Settings); gỡ prop ở chỗ
   gọi trong DashboardView. Giữ viz/duplicate/replace/remove.
3. **DashboardView chrome (A+F)** — Cancel/Save vào banner xanh (căn phải); bỏ block toolbar
   full-width; hàng title: tên thành `<input draftName>` + đặt `DashboardEditToolbar` bên phải
   (edit) / Edit (view). Thêm `draftName` (startEdit seed, cancel clear, onSave gửi
   `draftName.trim()||dashboard.name`). Thêm test name round-trip.
4. **Sidebar theo `editPanel` (D)** — `startEdit` đặt `editPanel=null`; render `EditSidebar` khi
   `editing && editPanel`; bỏ `sidebarCollapsed`/pill/`PanelLeftOpen`; `openPanel=setEditPanel`.
   `EditSidebar`: nút close (X)→`onClose`(ẩn, =null), nút back ‹→`onBack`(về add); bỏ `onCollapse`.
5. **Click card để chọn (G)** — inner wrapper thêm `onClick` (edit, trừ placeholder) →
   `openPanel({kind:"card",cardId})` + `cursor-pointer`; ring sẵn có hiển thị chọn. Placeholder giữ
   handler riêng; view mode không đổi.
6. **Tabs menu ▾ (E)** — `DashboardTabBar`: thay 2 icon hover Copy/X bằng 1 nút `▾` mở menu
   Rename/Duplicate/Delete (Delete ẩn khi `<2` tab); giữ kéo-thả + double-click rename. Sửa
   `DashboardView.tabs.test.tsx`.
7. **Sweep + live verify + cleanup** — full sweep; Playwright so cạnh Metabase 9 điểm (banner
   Cancel/Save, icon toolbar, T▾, section preview, sidebar on-demand, tabs ▾, tên inline persist,
   click-to-select không gear, 0 console error); xoá dashboard nháp + screenshot; final review →
   `finishing-a-development-branch`.

## Files
- **Sửa:** `dashboard/DashboardEditToolbar.tsx`, `dashboard/CardEditToolbar.tsx`,
  `dashboard/DashboardTabBar.tsx`, `dashboard/EditSidebar.tsx`, `routes/explore/DashboardView.tsx`
  (+ tests trong `routes/explore/__tests__/`). **Backend:** không.

## Lưu ý
- Mọi nơi dùng `sidebarCollapsed`/`onCollapse`/`onSettings` phải chuyển/gỡ đồng bộ (component +
  chỗ gọi) để cây vẫn compile sau mỗi task.
- Radix dropdown (T▾, Section, tab ▾) mở-through có thể không ổn trong jsdom → unit test assert
  nút trigger tồn tại, verify mở-through bằng Playwright (Task 7), không bỏ coverage thầm lặng.
