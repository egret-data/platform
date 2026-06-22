# Tier 1 — Restyle dashboard kiểu Metabase + filter pills (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-tier1-dashboard-metabase-restyle-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Branch:** `fix/dashboard-parity-gaps`
**Thuộc:** chương trình parity dashboard với Metabase ([audit backlog](./../../en/specs/2026-06-22-dashboard-metabase-parity-audit.md))

## Mục tiêu

Làm dashboard egret **trông và dùng giống Metabase** và đóng các khoảng cách UX giá trị cao
nhất: canvas + card kiểu Metabase, **filter pills + popover** (thay các widget `<select>` thô),
tab tô màu brand, enforce parameter required, và empty-tab state. Chỉ frontend; không đổi backend.
Clean-room (giống look & behaviour, không copy code Metabase).

## Căn cứ Metabase

Tham chiếu: dashboard Metabase đang chạy `E-commerce Insights` (localhost:3001/dashboard/1):
canvas xám nhạt; card trắng có shadow nhẹ, bo góc, padding rộng, title inline trên-trái; một hàng
**filter pills** bo tròn (`[icon] Tên` → `[icon] Tên: 4 selections`) mở popover có ô search +
danh sách checkbox (category), control date/number… ; tab active gạch chân màu brand. egret hiện
tại: hộp viền chứa các widget `<select>`/`<select multiple>` thô, card hộp với header có divider,
tab gạch chân tối, canvas trắng.

## Phạm vi (tier này)

**Có:** (A) restyle canvas + card; (B) filter pills + popover theo từng loại param thay control
native của `DashboardFilterBar`; (C) tab active màu brand; (D) enforce param required (gate fetch
card + empty state); (E) empty-tab state.

**Ngoài (tier sau / defer):** night/theme mode, PDF export, value sources, linked filters, map
viz, sharing/embedding, action buttons, per-column click, hover `⋯` menu của card (defer cho gọn
tier này), nút "Apply filters" + toggle auto-apply của Metabase (egret **auto-apply** khi đổi —
giữ nguyên).

## Component

### A. Canvas + vỏ card (`DashboardView.tsx`, `DashboardCardView.tsx`)

- Wrapper nội dung dashboard thêm canvas xám: `bg-muted/30` (cả container fullscreen), giữ `p-4`.
- Mỗi card `<div>`: từ `rounded-md border bg-background` → `rounded-lg border bg-card shadow-sm`.
  Heading vẫn không khung. Thêm padding trong để nội dung không sát mép.
- Header của `ChartCardTile`: bỏ divider `border-b`; render title inline (`px-3 pt-2 pb-1 text-sm
  font-medium`), nội dung dưới `px-2 pb-2`.
- Khoảng cách grid: truyền `margin={[12, 12]}` cho `<Grid>` RGL (gap kiểu Metabase).

### B. Filter pills + popover (`DashboardFilterBar.tsx` → hàng pill; thêm `FilterPill.tsx`)

`DashboardFilterBar` giữ props (`parameters`, `values`, `onChange`, `optionsBySlug`) nhưng render
một hàng pill flex-wrap (bỏ hộp viền). Mỗi pill là `FilterPill`:

```ts
interface FilterPillProps {
  param: DashboardParameter;
  value: unknown;
  options: string[];           // giá trị distinct category/id (từ optionsBySlug)
  onChange: (v: unknown) => void;
}
```

- **Nút pill** (trigger `Popover`): icon theo loại (lucide: `Tag`/`List` cho category, `Hash` cho
  id, `Type` cho text, `Hash` cho number, `Calendar` cho date) + tên param; khi có giá trị, thêm
  `: <summary>` và hiện nút clear (×) (stopPropagation, reset). `summarize(param, value)`:
  category/multi → `"N selections"` nếu >1, ngược lại giá trị đơn; scalar → giá trị; range/between
  → `"a – b"`; relative → nhãn option; rỗng → không.
- **Nội dung popover** rẽ theo `param.type` (một `FilterPill`, switch bên trong):
  - **category** → `Input` search + danh sách checkbox cuộn (`Checkbox` mỗi option, lọc theo
    search), **multi-select** thành string[] (→ filter `in`); nút "Clear".
  - **id** → `Input` search + danh sách **single-select** cuộn (một giá trị → filter `eq`), có
    "Clear"; fallback input text khi chưa nạp options. (id là giá trị đơn, khác category — giữ
    tách biệt.)
  - **text** → `Input` bind giá trị string (operator đã nằm trên param).
  - **number** → `Input type=number`; operator `between` → hai input (min/max → `[min,max]`).
  - **date** → `single` một input date; `range` hai input (`[start,end]`); `relative` select Last
    7/30/90/365 days. (Cùng encoding mà `paramToFilters` hiện tiêu thụ.)
- Chọn giá trị gọi `onChange` (giống hợp đồng hiện tại → `paramsState.setValue` → card refetch).
  Popover mở khi multi-select; đóng khi click ngoài.

Việc dịch giá trị→filter (`paramToFilters`) và nạp option (`useColumnValues`) không đổi — đây
thuần là lớp widget.

### C. Tab (`DashboardTabBar.tsx`)

Tab active: đổi `border-foreground font-medium` → `border-primary text-primary font-medium`;
inactive vẫn muted. Không đổi behaviour.

### D. Enforce param required (`DashboardView.tsx`, `useChartData.ts`, `DashboardCardView.tsx`)

- Tính `requiredUnset = parameters.filter(p => p.required && isBlank(values[p.slug]))` (dùng lại
  `isBlank` của `params.ts`; export nó).
- Khi `requiredUnset.length > 0`: hàng filter hiện notice (`Set the required filter(s): Region…`),
  và **card chart không fetch** — render placeholder muted "Set the required filters to load this
  card".
- Đấu nối: thêm arg optional `enabled` cho `useChartData(ws, chart, nonce, enabled = true)` để
  gate effect build/submit (khi `false`, không request, `isLoading` false). `ChartCardTile` nhận
  prop `blocked`, truyền `enabled={!blocked}`, render placeholder khi blocked. `DashboardCardView`
  truyền `blocked` xuống; `DashboardView` set từ `requiredUnset`. Pill required hiện dấu hiệu
  (asterisk, đã có).

### E. Empty-tab state (`DashboardView.tsx`)

Khi dashboard có tab và tab active không có card (`tabs.length > 0 && visibleCards.length === 0`),
render thông báo muted ở giữa "No cards on this tab." thay vì grid rỗng. (Thông báo "No cards yet"
toàn-dashboard vẫn cho dashboard không tab.)

## Luồng dữ liệu

```
parameters + values + optionsBySlug
  → DashboardFilterBar → FilterPill mỗi param
       click → Popover (control theo loại) → onChange(value) → paramsState.setValue → URL + refetch
  requiredUnset = param required có giá trị rỗng
       → notice trong hàng pill
       → DashboardCardView(blocked=true) → ChartCardTile(enabled=false) → placeholder (không fetch)
visibleCards (tab active) rỗng + có tab → "No cards on this tab."
```

## Xử lý lỗi

- Param category/id chưa nạp options → popover hiện "Loading…"; id fallback input text để filter
  vẫn dùng được.
- `onChange` với multi-select rỗng → value thành `[]` (coi như rỗng → không filter), pill về nhãn
  chưa-set.
- Gate required không bao giờ deadlock: set giá trị required xong là clear `blocked`, card fetch.
- Response column-values rỗng/đang tải → danh sách checkbox rỗng với ghi chú "No values"; input
  search vẫn render.
- Restyle thuần đổi class — không hồi quy tab, click behaviour (D1), cross-filter, fullscreen,
  auto-refresh.

## Kiểm thử

- **Vitest:**
  - `FilterPill`: render icon loại + tên; text `summarize` cho category multi ("2 selections"),
    giá trị đơn, range, rỗng; popover category liệt kê options, gõ để lọc, tick option gọi
    `onChange` với mảng, "Clear" làm rỗng; control text/number/date gọi `onChange` đúng encoding;
    clear (×) reset.
  - `DashboardFilterBar`: render một pill mỗi parameter; hiện notice required khi param required
    rỗng.
  - `useChartData`: `enabled=false` không request và không loading.
  - `DashboardView`: param required-chưa-set chặn card chart (hiện placeholder, không có nội dung
    card); set xong bỏ chặn. Tab active rỗng hiện "No cards on this tab." Các test tabless +
    click-behaviour cũ vẫn xanh.
- **Live (Playwright MCP, song song Metabase):** canvas/card/tab restyle khớp look Metabase; filter
  pills mở popover, category search+checkbox multi-select lọc card + cập nhật summary pill + URL;
  enforce required gate rồi bỏ chặn; thông báo tab rỗng hiện. **0 lỗi console.**
- **Full sweep** trước mỗi commit (từ `frontend/`): `npm run typecheck && npm run lint && npm run
  build && npx vitest run`.

## Quy ước

Metabase là chuẩn hình ảnh · clean-room (không AGPL) · TDD · full sweep trước mỗi commit · dùng lại
primitive Radix sẵn có (`popover`, `checkbox`, `input`, `button`, `scroll-area`) — không thêm dep ·
identity git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · stage path tường minh · squash-merge · branch `fix/dashboard-parity-gaps`.
