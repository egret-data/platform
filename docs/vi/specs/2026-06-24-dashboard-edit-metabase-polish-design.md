# Dashboard Edit Mode — Hoàn thiện kiểu Metabase (Thiết kế)

**Trạng thái:** Đã duyệt
**Ngày:** 2026-06-24
**Phạm vi:** Chỉ frontend (React/TS). Không đổi backend / API / migration.

> Tài liệu tham chiếu tiếng Việt. Bản chuẩn (canonical) là
> `docs/en/specs/2026-06-24-dashboard-edit-metabase-polish-design.md`.

## Vấn đề

Trang *edit* dashboard gần ổn nhưng chưa giống Metabase. 4 điểm từ phản hồi người dùng:

1. **Nút thêm bị phân tán.** "Add chart/filter/width" ở header; "Add text/heading/
   link" ở đáy sidebar phải. Sidebar phải nên **chỉ còn list chart**; mọi nút *thêm*
   dồn lên **toolbar trên cùng**.
2. **Heading lộ ô vuông grid.** Lúc edit, nền SVG grid lộ qua heading card → rối mắt.
3. **Toggle Full/Fixed vô dụng.** Thay bằng **Add section** kiểu Metabase (layout dựng sẵn).
4. **Không cuộn tới đáy trang khi edit.** Nội dung cuối trang bị cắt / không tới được.

## Mục tiêu

- Nút thêm gom vào toolbar trên; sidebar phải = chỉ list chart.
- Heading hiển thị sạch, không có ô vuông phía sau.
- "Add section" chèn heading + các ô placeholder theo 3 template; bấm placeholder để lấp chart.
- Cuộn được tới đáy trang khi edit.

## Ngoài phạm vi

- Không đổi backend. Card `placeholder` không bao giờ lưu (bị bỏ khi save) nên API
  cards giữ nguyên. Field `width` của dashboard vẫn còn trong model — chỉ bỏ UI chỉnh.
- Không đổi view mode (chỉ đọc), ngoài việc nền heading vô hại ở đó.
- Không thêm template ngoài 3 cái port từ Metabase.

## Thiết kế (tóm tắt — chi tiết số liệu xem bản EN)

### Phần 1 — Toolbar edit; sidebar chỉ list chart
- Component mới `DashboardEditToolbar.tsx`: dải icon ngay **dưới banner "You're
  editing"**, chỉ hiện khi `editing`. Nút: **Chart, Text, Heading, Link, Section
  (dropdown), Filter**. Toolbar không chứa business logic — chỉ gọi callback.
- Header lúc edit chỉ còn **Cancel / Save** (bỏ `+ Chart`, `+ Filter`, toggle Full/Fixed).
- `AddPanel.tsx`: bỏ hàng `+ Text/Heading/Link` + prop `onAddVirtual` → còn "Add a
  chart" + `ChartPickerList`. `EditSidebar.tsx` bỏ prop `onAddVirtual`.

### Phần 2 — Heading hết ô vuông
- Nguyên nhân: heading card không có nền nên grid SVG (z-0) lộ qua; chart/text/link
  có `bg-card` che được.
- Fix: nhánh `card_type === "heading"` đổi class từ `""` → `"bg-dashboard"` (màu
  canvas) → che ô grid mà vẫn liền canvas, không viền. View mode không vẽ grid nên
  vô hại. `HeadingCard.tsx` giữ nguyên.

### Phần 3 — Add section (thay Full/Fixed)
- Module mới `dashboard-sections.ts` (port `sections.ts` của Metabase): `SectionLayout`
  với `build(origin) => SectionDraftCard[]` (heading + placeholder). 3 template:
  - **KPI grid:** heading 24×1 + lưới 2×2 placeholder 12×5.
  - **Large chart + KPIs phải:** heading + 1 ô lớn 18×9 trái + cột 3 ô 6×3 phải.
  - **KPIs + chart dưới:** heading + hàng 4 ô 6×3 + 1 ô lớn 24×9 dưới.
  - Số liệu vị trí/size xem bản EN (port chính xác từ Metabase).
- `card_type` là `string` → thêm `"placeholder"` không cần đổi type/backend.
- `PlaceholderCard.tsx`: ô dashed trống, nút "+" giữa; bấm → `onPick()`. Trong
  `DashboardView`, `card_type === "placeholder"` render `PlaceholderCard` thay cho
  `DashboardCardView` (vẫn có drag handle + `CardEditToolbar`).
- **Chèn:** `addSection(layout)` tính `nextRow` dưới các card của tab hiện tại,
  `build({col:0,row:nextRow})`, map sang `DraftCard` (id mới, `chart_id:null`,
  `tabKey` của tab hiện tại; heading `options:{text:""}`), append vào `draftCards`.
- **Lấp:** bấm placeholder → set `fillingPlaceholderId`, mở chart picker (tái dùng
  dialog Replace chart). Chọn chart → thay placeholder bằng card `chart`, **giữ
  nguyên grid_col/row/size_x/size_y của ô** (không dùng default size của chart).
- **Save:** lọc bỏ `card_type === "placeholder"` trước `serializeLayout` → placeholder
  không bao giờ tới API.
- **Bỏ toggle width:** xoá UI Full/Fixed + state `draftWidth`. `effectiveWidth =
  dashboard.width` ở cả 2 mode; `onSave` gửi `width: dashboard.width` (giữ nguyên).

### Phần 4 — Lỗi cuộn tới đáy (edit mode)
- Scroll container là `<main overflow-auto>` của `AppShell`. Lỗi kiểu "phải nhìn
  mới biết": **bắt buộc reproduce live bằng Playwright** (dashboard nháp đủ nhiều
  card để tràn viewport) trước khi fix. Fix nhiều khả năng: thêm khoảng đệm dưới
  vùng grid lúc edit (vd `pb-24`/spacer) để hàng card cuối + resize handle vào được
  tầm nhìn; hoặc gỡ cap chiều cao đang cắt. Yêu cầu: hàng cuối cuộn tới được, không
  đổi spacing view mode. Nếu repro lộ nguyên nhân khác (overflow ancestor, sticky
  sidebar cap flex row) thì fix cái đó.

## Kiểm thử

- **Unit (Vitest)** `__tests__/dashboard-sections.test.ts`: mỗi template ra đúng số
  card + vị trí; origin offset (`row:7`) dịch đúng. Test drop-on-save: lọc bỏ
  placeholder khỏi payload, giữ chart/heading.
- **Live (Playwright, 0 console error)** trên dashboard **nháp** (không động `a1`):
  toolbar đủ nút + sidebar chỉ list chart; heading không ô vuông; 3 template chèn +
  lấp được; Save → reload → placeholder mất, chart/heading còn; cuộn tới đáy được.
  Xoá dashboard nháp sau; screenshot nháp để ở thư mục cha `data-portal/`, dọn sạch.

## Files

- **Mới:** `DashboardEditToolbar.tsx`, `dashboard-sections.ts`, `PlaceholderCard.tsx`,
  `__tests__/dashboard-sections.test.ts`.
- **Sửa:** `routes/explore/DashboardView.tsx`, `dashboard/AddPanel.tsx`,
  `dashboard/EditSidebar.tsx`.
- **Backend:** không.

## Rủi ro / Ghi chú

- `placeholder` cố ý chỉ tồn tại trong draft; kiểm tra mọi path đọc card lúc edit
  chịu được `chart_id:null` + `card_type:"placeholder"` (code value-source đã lọc
  `card_type === "chart"` nên placeholder tự bị bỏ qua — xác nhận khi code).
- Dùng token OKLCH `bg-dashboard`, không literal color.
