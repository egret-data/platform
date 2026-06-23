# Tier 2d — Đại tu Filter UX (kiểu Metabase) (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-tier2d-filter-ux-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Branch:** `feat/dashboards-tier2d-filter-ux` (off main)

## Mục tiêu

Đưa **filter** của egret theo UX Metabase (từ walkthrough): map filter→card bằng cách chọn cột
**trên từng card**, chọn **widget** giá trị, đặt **default**, **link filter** (chain), thêm **type**.
Chỉ frontend (param nằm trong `dashboards.parameters` jsonb — không đổi schema backend).

## Phạm vi

**Có:** (A) map cột trên card khi chọn filter; (B) widget (dropdown/search/input); (C) ô default;
(D) **linked filters** (chain-filter trên query options); (E) type/operator mới: **boolean**,
**location**, date **month-year** / **quarter-year**.

**Defer (nêu rõ lý do):** text **does-not-contain** (cần backend op `not_like` — `ChartFilter` chỉ
có `like`); date **all-options** (picker tổng hợp của Metabase — egret giữ single/range/relative);
per-card viz-settings override; link→dashboard/question. Không đổi backend trong tier này.

## Data model (`api/dashboards.ts` — jsonb, không migration)

```ts
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"
  | "=" | "!=" | "between" | ">=" | "<="
  | "range" | "relative" | "single" | "month-year" | "quarter-year";
export type ParamWidget = "dropdown" | "search" | "input";
export interface DashboardParameter {
  id; name; slug; type; operator;
  default?: unknown; required?: boolean;
  widget?: ParamWidget;
  filteringParameters?: string[];
}
```

`OPERATORS` (trong `ParameterEditor.tsx`) thêm: `boolean: ["is"]`, `location: ["is","is-not"]`, date
thêm `"month-year"`, `"quarter-year"`. (`does-not-contain` cố ý không có.)

## A. Map cột trên card khi sửa filter (luồng Metabase)

Khi `editPanel.kind === "filter"`, mỗi **chart** card hiện overlay trên thân: nhãn "Column to filter
on" + `<select>` các cột chart (`dimensions + columns`), value = `paramMappings[activeParamId]`,
`onChange` → `updateCardOptions(card.id, { paramMappings: { ...m, [paramId]: col } })`. Chart mờ phía
sau. Đây là cách map filter→card (filter-centric như Metabase).

⇒ **Bỏ** mục map per-parameter khỏi `CardSettingsPanel` (map giờ qua luồng filter). CardSettingsPanel
giữ Title / On-click / Move-to-tab.

## B. Widget (`FilterSettingsPanel` + `FilterPill`)

- `FilterSettingsPanel` thêm "How should people filter on this?" (radio: **Dropdown** / **Search** /
  **Input**) ghi `param.widget`. Hiện cho category / id / location / text.
- `FilterControl` của `FilterPill` tôn trọng `widget` (mặc định theo type): **dropdown** → checkbox
  list có search (hiện tại); **search** → cùng list, search-first; **input** → input text (1 giá trị).

## C. Default value (`FilterSettingsPanel`)

Control "Default value" theo type (dùng lại control của `FilterPill`, hoặc input/select đơn giản),
ghi `param.default`. `useDashboardParams` đã apply default lúc load (không đổi).

## D. Linked filters (chain-filter)

- `FilterSettingsPanel` thêm mục "Linked filters": checkbox mỗi param KHÁC, ghi `filteringParameters`.
- `useColumnValues` thêm arg `filters?: ChartFilter[]` → nối vào query distinct (`{ columns:[column],
  filters }`).
- `ParamOptions` (trong `DashboardView`) tính filters đó: nó đã tìm card map param → `(datasetId,
  column)`. Với mỗi linked param `L` có giá trị, đọc `colL = card.paramMappings[L]` (cùng card/dataset);
  nếu có, thêm `paramToFilters(L, values[L.slug], colL)` vào query options → dropdown chỉ hiện giá trị
  hợp lệ với linked params. (Linked param phải map cùng card/dataset; nếu không thì bỏ qua — không lỗi.)

## E. Type / operator mới (`params.ts` `paramToFilters`)

- **boolean** (`is`): value `"true"`/`"false"`/bool → `[{col, op:"eq", value:<bool>}]`; rỗng → `[]`.
  Widget: select 3 trạng thái **Any / True / False**.
- **location** (`is`/`is-not`): như category string — multi-select → `in`/`not_in`. Distinct qua
  `useColumnValues`.
- **date month-year** (`"YYYY-MM"`): → `[ge "YYYY-MM-01", le <ngày cuối tháng>]`. Widget `<input type=month>`.
- **date quarter-year** (`"YYYY-Q"`): → `[ge <đầu quý>, le <cuối quý>]`. Widget year + select quý.
- `summarize` xử boolean ("True"/"False"), month-year, quarter-year ("Q2 2024"), location (đếm).

## Xử lý lỗi

- Linked param không map cùng card/dataset → bỏ qua (không crash, options không bị ràng buộc).
- Bind boolean: `eq` với JS bool gửi JSON `true/false`; nếu engine từ chối bind bool, fallback string
  `"true"/"false"` (verify live; ghi chú trong task).
- month-year/quarter-year giá trị thiếu → không filter.
- `widget:"input"` trên category → 1 giá trị string (mất multi) — chấp nhận.
- View mode + mọi behaviour cũ (drill, cross-filter, required-gating, tabs, edit sidebar) không đổi.

## Kiểm thử

- **Vitest:** `paramToFilters` (boolean→eq bool; location→in/not_in; month-year/quarter-year→ge/le);
  `summarize` (boolean/month-year/quarter-year/location); `useColumnValues` (truyền `filters` vào
  spec); `FilterSettingsPanel` (widget/default/linked + type mới); `FilterPill` (widget input/boolean/
  month-year/quarter-year + onChange đúng); `DashboardView` (chọn filter → in-card "Column to filter
  on" ghi paramMappings; CardSettingsPanel hết mục map; helper linked-filter).
- **Live (Playwright MCP, song song Metabase):** thêm filter → map cột trên card → widget=Dropdown →
  có giá trị; link 2 filter → dropdown phụ thuộc thu hẹp; thêm boolean/location/month-year lọc card;
  set default → apply khi reload. **0 lỗi console.**
- **Full sweep** trước mỗi commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`.

## Quy ước

Metabase là chuẩn · clean-room · TDD · full sweep trước mỗi commit · identity git đã cấu hình +
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage path tường minh ·
squash-merge · branch `feat/dashboards-tier2d-filter-ux`.
