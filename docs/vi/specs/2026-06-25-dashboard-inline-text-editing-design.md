# Sửa Text/Heading inline trên canvas (+ hint `{{variable}}`) — Thiết kế

**Trạng thái:** Đã duyệt · **Ngày:** 2026-06-25 · **Phạm vi:** chỉ frontend, không đổi backend.

> Tham chiếu tiếng Việt. Bản chuẩn: `docs/en/specs/2026-06-25-dashboard-inline-text-editing-design.md`.

## Vấn đề
So với Metabase, egret sửa text/heading qua panel phải (textarea), Metabase gõ **inline trên card**.
- **#1** Inline editing — gõ thẳng trên card.
- **#2** Nội suy `{{slug}}` — **đã có sẵn**: `interpolateParams` (params.ts:310) + đã wire ở
  `DashboardCardView:62,66` cho cả text & heading. Chỉ thiếu **hint** để user biết.

## Mục tiêu
- Edit mode: text/heading gõ inline trên card, hiện **raw** text (thấy `{{slug}}` literal).
- Card vừa thêm thì **autofocus** editor (add → gõ ngay).
- Panel text/heading: **bỏ field text** (giờ inline), giữ auto-height/move-to-tab, **thêm hint
  `{{slug}}`** + liệt kê slug filter hiện có. View mode giữ nguyên (interpolate + markdown).

## Thiết kế (chi tiết code xem bản EN)
- **A. `HeadingCard`/`TextCard`** thêm props `editing`/`autoFocus`/`onChange`. Edit → render
  `<input>` (heading) / `<textarea>` (text) `dash-nodrag`, bind raw text, `aria-label="heading
  text"`/`"text body"`, placeholder gợi ý `{{filter}}`. Không editing → display cũ (markdown).
- **B. `DashboardCardView`** thêm `onEditText`/`autoFocusText`; text/heading: editing → raw,
  view → `interpolateParams`. Truyền xuống Text/HeadingCard.
- **C. `DashboardView`** thêm state `newCardId`; `addVirtualCard` hoist id + set `newCardId` cho
  text/heading (autofocus). Card map truyền `onEditText={... updateCardOptions(c.id,{text})}` +
  `autoFocusText={c.id===newCardId}`. Clear `newCardId` khi rời edit (cancel/save/[id] effect).
- **D. `CardSettingsPanel`** bỏ `<textarea>` text/heading; thêm `<p>` hint "Insert a filter value
  with `{{slug}}`" + list slug. Giữ auto-height/move-to-tab.

## Kiểm thử
- Unit: Text/HeadingCard editing render control bind text + onChange; non-editing render display
  (text raw không render markdown khi edit). **Sửa** 2 test đã khôi phục (`adds a text card`,
  `adds a heading card`) sang gõ INLINE (`aria-label` text body/heading text trên editor). Sửa
  `editsidebar` "clicking a card opens the card panel" sang assert hint/auto-height (panel hết
  field text). Thêm test interpolate ở view + autofocus khi add.
- Live (Playwright, 0 console error, dashboard nháp): add heading autofocus + gõ inline; text
  markdown inline; `{{slug}}` + set filter ở view → card hiện giá trị; panel có hint, hết field
  text; Save→reload còn nội dung. Xoá nháp + screenshot sau.

## Files
- **Sửa:** `dashboard/HeadingCard.tsx`, `dashboard/TextCard.tsx`, `dashboard/DashboardCardView.tsx`,
  `dashboard/CardSettingsPanel.tsx`, `routes/explore/DashboardView.tsx`. **Backend:** không.

## Rủi ro
- Inline editor bind **raw** `options.text` (không interpolate) để sửa được `{{slug}}`.
- `aria-label` editor trùng label panel cũ (cho test/screen reader). `dash-nodrag` để chọn text
  không kéo card. Auto-height vẫn là chuyện view mode. Chỉ token OKLCH.
