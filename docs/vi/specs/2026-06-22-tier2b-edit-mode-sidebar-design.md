# Tier 2b — Đại tu edit-mode dashboard (right sidebar kiểu Metabase) (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-tier2b-edit-mode-sidebar-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Branch:** `feat/dashboards-tier2b-edit-ux` (off main)
**Thuộc:** chương trình parity dashboard với Metabase ([audit backlog](./../../en/specs/2026-06-22-dashboard-metabase-parity-audit.md))

## Mục tiêu

Làm **edit mode** dashboard giống Metabase. Hiện mỗi card bị chôn dưới một đống control inline
(Move-to-tab, Title, map param, On-click) và thêm chart là bức tường ~47 nút. Thay bằng: **card
sạch** (chỉ chart + toolbar hover) và một **sidebar phải** đổi nội dung theo ngữ cảnh — thêm chart
(có search), cấu hình card đang chọn, hoặc cấu hình filter đang chọn. Chỉ frontend; view mode + mô
hình dữ liệu giữ nguyên.

## Căn cứ Metabase

Edit mode Metabase: một **sidebar phải** cố định, nội dung tuỳ ngữ cảnh — picker câu hỏi có search
khi thêm, panel settings card khi chọn card, panel filter khi chọn filter; card thì sạch với menu
hành động hiện khi hover. egret theo đúng hình dạng đó bằng primitive sẵn có (không thêm dep).

## Phạm vi

**Có:** state machine sidebar `editPanel` (`add` / `card` / `filter`); Add panel có search; Card
panel (title, map cột từng filter, on-click, move-to-tab); Filter panel (name/type/operator/required
+ delete); card sạch + toolbar hover `CardEditToolbar` (settings/duplicate/replace/remove) +
highlight card đang chọn; top-bar edit (`+ Chart`, `+ Filter`, Width, Cancel, Save); filter **chips**
ở edit mode mở filter panel.

**Ngoài:** view mode (giữ nguyên — value pills…); mô hình dữ liệu + payload lưu (giữ nguyên); kiểu
map-bằng-cách-bấm-card của Metabase (egret giữ map cột theo từng card, chỉ dời vào card panel);
action buttons / per-column click / sections (tier khác). Không đổi backend.

## Component & state

Tái dùng nguyên trạng mọi draft state + mutator trong `DashboardView`: `draftCards`,
`draftParameters`, `draftTabs`, `draftWidth`, `addCard`, `addVirtualCard`, `duplicateCard`,
`replaceCard` (+ dialog `replacingId`), `removeCard`, `updateCardOptions`, `moveCardToTab`,
`setDraftParameters`, các helper param. Tier này đổi **nơi** kích hoạt (vào sidebar), không đổi
việc chúng làm.

### State `editPanel` mới (trong `DashboardView`, trên early return)
```ts
type EditPanel =
  | { kind: "add" }
  | { kind: "card"; cardId: string }
  | { kind: "filter"; paramId: string }
  | null;
const [editPanel, setEditPanel] = useState<EditPanel>(null);
```
- `startEdit` đặt `editPanel = { kind:"add" }`. `cancelEdit`/`onSave` reset `null`.
- Guard target cũ: `card` mà không còn draft card id đó (đã xoá), hoặc `filter` không khớp param →
  fallback `{ kind:"add" }`.

### Top-bar edit (thay aside "Add a chart" tường-47-nút)
Toolbar gọn ở header (header view-mode không đổi):
- `+ Chart` → `setEditPanel({ kind:"add" })`.
- `+ Filter` → tạo draft param mới (`slugify("filter", existing)`, `{ type:"text", operator:"is" }`)
  thêm vào `draftParameters`, rồi `setEditPanel({ kind:"filter", paramId })`.
- toggle **Width** Full/Fixed (Tier 2a), **Cancel**, **Save**.

### Edit filter chips (`EditFilterChips.tsx` mới)
Chỉ ở edit mode: một hàng chip — mỗi `draftParameters` một chip (label = tên param) — bấm gọi
`onSelect(param.id)` → `setEditPanel({ kind:"filter", paramId })`. Chip của filter đang chọn được
highlight. (Không value popover ở edit mode — đó là view mode.)
```ts
interface EditFilterChipsProps { parameters: DashboardParameter[]; activeId: string | null; onSelect: (id: string) => void }
```

### Card sạch + `CardEditToolbar.tsx`
Ở edit mode, mỗi card render **hiển thị** qua `DashboardCardView` (chart, hoặc text/heading/link đã
render) — không input inline. `CardEditToolbar` overlay góc trên-phải khi hover:
```ts
interface CardEditToolbarProps {
  cardType: string;        // chỉ hiện ↺ replace cho "chart"
  onSettings: () => void;  // ⚙ → setEditPanel({ kind:"card", cardId })
  onDuplicate: () => void; // ⧉
  onReplace: () => void;   // ↺ (chart)
  onRemove: () => void;    // ✕
}
```
Wrapper card thêm `ring-2 ring-primary` khi `editPanel.kind==="card" && cardId===c.id`.
(`DashboardCardView` mở rộng để render text/heading/link cả ở edit mode; input dời vào card panel.)

### Sidebar phải (`EditSidebar.tsx` + 3 panel)
Panel cố định (`w-[340px] shrink-0 border-l bg-card`) cạnh grid ở edit mode. `EditSidebar` switch
theo `editPanel.kind`:
- **`AddPanel`** — `Input` search; danh sách chart cuộn (`useCharts`, lọc theo tên-chứa-search) mỗi
  dòng bấm gọi `addCard(chart)` (panel mở để thêm nhiều); nút `+ Text`/`+ Heading`/`+ Link` gọi
  `addVirtualCard(...)`.
- **`CardSettingsPanel`** — cho card đang chọn (tra trong `draftCards`):
  - chart card: **Title** input; mục **Filters** một `<select>` cột mỗi param (value =
    `options.paramMappings[param.id]`, options = `dimensions + columns` của chart) ghi `paramMappings`;
    mục **On click** (menu / cross-filter / custom-URL — dời từ inline); **Move to tab** (khi
    `tabs.length>=2`, gọi `moveCardToTab`).
  - text/heading: input **text**. link: input **url** + **label**.
- **`FilterSettingsPanel`** — các trường `ParameterEditor` xếp dọc (name, type, operator, required)
  cho param đang chọn, + nút **Delete filter** (xoá khỏi `draftParameters`, đặt `editPanel` về
  `{ kind:"add" }`).

`EditSidebar` nhận slice + callback từ `DashboardView` (không tự giữ draft state). Panel nhỏ, tách,
test riêng được.

## Layout (edit mode)
```
<canvas bg-muted/30 p-4>
  <top-bar edit: title · [+ Chart][+ Filter][Full|Fixed][Cancel][Save]>
  <EditFilterChips …>
  <DashboardTabBar … editing>
  <div class="flex gap-4">
     <div class="min-w-0 flex-1"> {grid card sạch + CardEditToolbar} </div>
     <EditSidebar class="w-[340px] shrink-0">
  </div>
</canvas>
```
View mode là layout 1 cột hiện tại (không sidebar). Wrapper fixed-width Tier 2a áp cho view mode; ở
edit mode sidebar đứng cạnh cột grid full-width.

## Luồng dữ liệu

```
top-bar / card toolbar / filter chip → setEditPanel(…)
EditSidebar(editPanel) → AddPanel | CardSettingsPanel | FilterSettingsPanel
  AddPanel → addCard / addVirtualCard
  CardSettingsPanel → updateCardOptions / moveCardToTab
  FilterSettingsPanel → setDraftParameters
onSave → PUT { name, description, parameters, tabs, cards, width } (không đổi)
```

## Xử lý lỗi

- `editPanel` cũ (card/filter đã xoá) → reset `{ kind:"add" }` (không crash, không panel trống).
- Xoá card đang chọn / xoá filter đang chọn → đóng/đổi panel.
- Thêm chart trên dashboard có tab → vào tab đang xem (luật `addCard` cũ).
- Không có chart nào → Add panel hiện "No charts yet".
- View mode không đụng: value pills, drill/click (D1), cross-filter, required-gating, fullscreen,
  auto-refresh, tabs — giữ nguyên, test cũ vẫn xanh.
- Rules of Hooks: `editPanel` useState trên early return `if (isLoading || !dash) return …`.

## Kiểm thử

- **Vitest:** `AddPanel` (search lọc + bấm chart gọi onAdd + +Text/Heading/Link); `CardSettingsPanel`
  (chart: Title + select map mỗi param + On-click + Move-to-tab gọi callback đúng; text: input text);
  `FilterSettingsPanel` (name/type/operator/required + Delete); `EditFilterChips` (1 chip/param,
  click gọi onSelect, chip active đánh dấu); `DashboardView` edit (vào edit hiện sidebar ở Add; bấm
  ⚙ card mở card panel + highlight; +Filter thêm param + mở filter panel; bấm chip filter mở panel;
  card render hiển thị, không input inline). Các suite `DashboardView.*` cũ vẫn xanh.
- **Live (Playwright MCP, song song Metabase `localhost:3001/dashboard/1` edit):** vào edit → card
  sạch + sidebar phải; thêm chart qua search; chọn card → cấu hình title/map/on-click/move-to-tab ở
  sidebar; thêm + cấu hình filter; reorder/rename/delete tab vẫn chạy; Save → bền; view mode không
  đổi. **0 lỗi console.**
- **Full sweep** trước mỗi commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`.

## Quy ước

Metabase là chuẩn hình ảnh · clean-room (không AGPL) · TDD · full sweep trước mỗi commit · dùng lại
primitive Radix (`scroll-area`, `input`, `button`, `select`, `popover`) — không thêm dep · identity
git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · stage path tường minh · squash-merge · branch `feat/dashboards-tier2b-edit-ux`.
