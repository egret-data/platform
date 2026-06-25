# Kế hoạch: Sửa Text/Heading inline trên canvas

> Tóm tắt tiếng Việt. Bản chi tiết (canonical): `docs/en/plans/2026-06-25-dashboard-inline-text-editing.md`.
> Spec: `docs/{en,vi}/specs/2026-06-25-dashboard-inline-text-editing-design.md`.

**Mục tiêu:** gõ text/heading thẳng trên card (raw `{{slug}}`), autofocus khi vừa thêm; panel bỏ
field text + thêm hint `{{slug}}`; view mode giữ nguyên (interpolate + markdown). `{{variable}}` đã
có sẵn — chỉ thêm hint. Chỉ frontend.

**Quy ước:** chạy trong `egret/frontend/`. Sweep trước mỗi commit:
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Branch
`feat/dashboard-inline-text-editing`. Commit kết bằng
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **Không động `a1`** —
verify trên dashboard nháp rồi xoá.

## Các task (3)

1. **Inline editor cho `HeadingCard` + `TextCard`** — thêm props `editing/autoFocus/onChange`; edit →
   `<input>`(heading)/`<textarea>`(text) `dash-nodrag`, `aria-label="heading text"/"text body"`, bind
   raw text. Không editing → display cũ. Test component trong `cards.test.tsx`. **App chưa đổi** (chưa
   ai truyền `onChange`) → suite vẫn xanh.
2. **Wire end-to-end + panel hint + sửa test tích hợp (1 commit)** —
   `DashboardCardView`: thêm `onEditText`/`autoFocusText`; text/heading edit→raw, view→`interpolateParams`.
   `CardSettingsPanel`: bỏ `<textarea>`, thêm `<p>` hint "Insert a filter value with `{{slug}}`" + list slug.
   `DashboardView`: state `newCardId` (addVirtualCard hoist id + set cho text/heading; clear khi cancel/save/[id]),
   truyền `onEditText={updateCardOptions(c.id,{text})}` + `autoFocusText={c.id===newCardId}`.
   Sửa 3 test: `adds a text card`/`adds a heading card` (gõ INLINE qua aria-label), `editsidebar
   "clicking a card opens the card panel"` (assert hint, panel hết field text). (Inline + panel field
   cùng aria-label nên phải lật chung 1 commit để khỏi đụng `getByLabelText`.)
3. **Sweep + live verify + cleanup** — full sweep; Playwright (0 console error, dashboard nháp): add
   heading autofocus + gõ inline; text markdown inline; panel có hint hết field text; `{{slug}}` + set
   filter ở view → hiện giá trị; Save→reload còn nội dung. Xoá nháp + screenshot; final review →
   `finishing-a-development-branch`.

## Files
- **Sửa:** `dashboard/HeadingCard.tsx`, `dashboard/TextCard.tsx`, `dashboard/DashboardCardView.tsx`,
  `dashboard/CardSettingsPanel.tsx`, `routes/explore/DashboardView.tsx` (+ tests). **Backend:** không.
