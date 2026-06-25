# Kế hoạch: Dashboard Edit — Hoàn thiện kiểu Metabase

> Tóm tắt tiếng Việt. Bản chi tiết (canonical) cho người/agent thực thi:
> `docs/en/plans/2026-06-24-dashboard-edit-metabase-polish.md`.
> Spec: `docs/{en,vi}/specs/2026-06-24-dashboard-edit-metabase-polish-design.md`.

**Mục tiêu:** trang edit dashboard giống Metabase hơn — toolbar thêm ở trên, sidebar
phải chỉ list chart, heading sạch (hết ô vuông), "Add section" thay toggle Full/Fixed,
và sửa lỗi cuộn không tới đáy. Chỉ frontend, không đụng backend.

**Quy ước:** chạy trong `egret/frontend/`. Sweep trước mỗi commit:
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Branch
`feat/dashboard-edit-metabase-polish`. Commit kết bằng
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **Không động
dashboard `a1`** — verify trên dashboard nháp rồi xoá.

## Các task (8)

1. **`dashboard-sections.ts` + test** — 3 template (`kpi_grid` 4 ô 12×5; `large_chart_kpi_right`
   ô lớn 18×9 + 3 ô 6×3 phải; `kpi_chart_below` 4 ô 6×3 + ô lớn 24×9), mỗi cái
   `build(origin) → SectionDraftCard[]` (heading + placeholder). Thêm `dropPlaceholders()`.
   Unit test Vitest (6 ca) — port số liệu chính xác từ Metabase `sections.ts`.
2. **`PlaceholderCard.tsx`** — ô dashed trống, nút "+", `dash-nodrag`, click → `onPick()`.
3. **`DashboardEditToolbar.tsx`** — dải nút Chart/Text/Heading/Link/Section(dropdown
   `dropdown-menu` + mini preview)/Filter, đặt `-mx-4 border-b bg-card` dưới banner edit.
4. **`AddPanel` + `EditSidebar`** — bỏ hàng text/heading/link + prop `onAddVirtual`;
   sidebar phải chỉ còn "Add a chart" + `ChartPickerList`.
5. **`DashboardView` (toolbar + dọn header)** — render toolbar dưới banner; header edit
   chỉ còn Cancel/Save; xoá toggle Full/Fixed + state `draftWidth` (`effectiveWidth =
   dashboard.width`, save dùng `dashboard.width`); tách handler `addFilter` + `addSection`.
6. **`DashboardView` (placeholder + heading)** — render `PlaceholderCard` khi
   `card_type==="placeholder"`; heading thêm class `bg-dashboard` (che ô vuông);
   dialog "Add a chart" để lấp (giữ nguyên grid pos/size của ô); `onSave` lọc
   `dropPlaceholders` trước `serializeLayout`.
7. **`DashboardView` (lỗi cuộn)** — **reproduce live trước** (Playwright, dashboard nháp
   nhiều card); fix nhiều khả năng: thêm `pb-24` cho cột grid lúc edit; verify hàng cuối
   cuộn tới được, 0 console error; nếu nguyên nhân khác thì fix đúng chỗ clip.
8. **Sweep + verify + cleanup** — full sweep; Playwright verify 6 điểm (toolbar, heading
   sạch, 3 template chèn+lấp, save round-trip bỏ placeholder, cuộn đáy, 0 console error);
   xoá dashboard nháp + screenshot nháp (để ở thư mục cha `data-portal/`); final review →
   `finishing-a-development-branch`.

## Files
- **Mới:** `dashboard-sections.ts`, `PlaceholderCard.tsx`, `DashboardEditToolbar.tsx`,
  `__tests__/dashboard-sections.test.ts`.
- **Sửa:** `routes/explore/DashboardView.tsx`, `dashboard/AddPanel.tsx`,
  `dashboard/EditSidebar.tsx`. **Backend:** không.
