# Tier 3b — Date picker, filter value sources, per-card viz override (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-tier3b-filters-and-viz-override-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/dashboards-tier3b` (off main)

## Mục tiêu

Đóng 3 gap parity với Metabase, **chỉ frontend** (param trong `dashboards.parameters` jsonb, override card trong
`dashboard_cards.options` jsonb — không backend/migration):

- **A. Date "all-options" picker** — widget date hợp nhất Relative/Specific kiểu Metabase (egret hiện có
  single/range/relative/month-year/quarter-year rời).
- **B. Filter value sources** — giá trị dropdown của param lấy từ **danh sách tĩnh tự nhập** hoặc **cột của card khác**,
  không chỉ distinct-query.
- **C. Per-card viz override** — sửa viz settings của card ngay trên dashboard, tái dùng các section của chart builder;
  override lưu trên card, merge lên display settings của chart đã lưu khi render.

## Phạm vi

**Có:** A (operator `all-options` + picker), B (`valuesSource` trên param + picker nguồn + resolve ở loader),
C (`vizOverride` chỉ-display trên card + component dùng chung `ChartSettingsSections` + panel viz trên dashboard +
merge khi render + reset).

**Defer:** mọi thay đổi backend; `does-not-contain` (cần `not_like`); tab "Exclude" của date (chỉ Relative + Specific);
"From connected fields" graph (chỉ card+column tường minh); override **query** từng card (vizOverride chỉ display — query
giữ spec của chart + inject param dashboard).

## Data model (jsonb, không migration)

```ts
export type ParamOperator = ... | "all-options";   // thêm vào nhóm date
export interface DashboardParameter { /* ...; */ valuesSource?: ParamValuesSource; }
export interface ParamValuesSource {
  type: "distinct" | "static" | "card";   // mặc định "distinct"
  static?: string[];                       // type==="static"
  cardId?: string; column?: string;        // type==="card"
}
// DashboardCard.options thêm: vizOverride?: VizOverride  (CHỈ display, không có `spec`)
export interface VizOverride { viz_type?: VizType; [k: string]: unknown; }
```

## A. Date all-options
`OPERATORS.date` thêm `"all-options"`. `FilterPill` render `DateFilterPicker` 2 tab: **Relative** (Past/Next/Current + N +
days/weeks/months/quarters/years) và **Specific** (On/Before/After/Between + 1-2 ô date). Value là object có cấu trúc.
`paramToFilters` thêm nhánh `all-options` → `ge`/`le` (relative tổng quát hóa logic relative-day sẵn có; specific
On→eq, Before→lt/le, After→gt/ge, Between→ge+le; thiếu → `[]`). `summarize` gắn nhãn. Operator date cũ giữ nguyên.

## B. Value sources
`FilterSettingsPanel` thêm "Where do values come from?" (cho category/location/id/text): **Distinct** (hiện tại) /
**Custom list** (textarea, mỗi dòng 1 giá trị → `static`) / **From a card column** (chọn card + cột). Loader `ParamOptions`
resolve: distinct→như cũ; static→emit thẳng list (không query); card→distinct-query card/cột đã chọn. Linked filter
(Tier 2d) chỉ áp cho nguồn query (distinct/card); static bỏ qua.

## C. Per-card viz override (full editor reuse)
- **Tách** phần compose section của `ChartExplore` thành component dùng chung `ChartSettingsSections`
  (`<... vizType params onChange />`) render đúng các display section theo vizType (cartesian/pie/scalar/... + Format /
  Conditional formatting / Table columns). `ChartExplore` đổi sang dùng nó, **hành vi không đổi** (test cũ là guard).
- **Merge** `applyVizOverride(chart, override) → { viz_type, params }`: deep-merge field display của override lên
  `chart.params`, override `viz_type` nếu có, **không bao giờ đụng `params.spec`**. Thứ tự ở `DashboardCardView`:
  `applyParamsToChart` (inject filter vào spec) → `applyVizOverride` (display) → `<ChartRenderer>`.
- **Panel viz** mở từ card toolbar, render `ChartSettingsSections` bound vào effective params (chart.params ⊕ override),
  ghi vào `card.options.vizOverride`; nút **Reset to chart defaults** xóa override. Card không override render như cũ.

## Data flow
```
render card: chart → applyParamsToChart → applyVizOverride → <ChartRenderer>
loader: source = valuesSource?.type ?? "distinct"; distinct/card → useColumnValues(+linked); static → emit list
date all-options → paramToFilters → ge/le
```

## Xử lý lỗi
- vizOverride sai viz type → ChartRenderer bỏ qua field không liên quan (inert, không crash); đổi viz_type → render type đó.
- card/column nguồn trỏ card/cột đã xóa → query rỗng → "No values". static rỗng → dropdown rỗng.
- all-options thiếu giá trị → chỉ emit bound có sẵn (hoặc `[]`).
- Tách `ChartSettingsSections` không đổi hành vi chart builder (test ChartExplore là guard).
- vizOverride không chứa `spec`; merge bỏ qua spec → inject param dashboard luôn thắng ở query.

## Kiểm thử
Vitest: paramToFilters/summarize all-options; DateFilterPicker (tab + encoding); resolve value source (static/card/distinct
fallback) + FilterSettingsPanel ghi valuesSource; applyVizOverride (merge display, override viz_type, không đụng spec,
no-override = identity); ChartSettingsSections render đúng section + onChange round-trip, test ChartExplore vẫn xanh; panel
viz ghi vizOverride + Reset; DashboardCardView render effective params.
Live (Playwright song song Metabase, 0 lỗi console): date all-options (relative+specific) lọc card; value = custom list &
từ card khác; override viz 1 card (màu series/axis/đổi viz type) + Reset.
Full sweep trước mỗi commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`.

## Quy ước
Metabase là chuẩn · clean-room · TDD · full sweep trước commit · identity git + footer Co-Authored-By · docs song ngữ ·
squash-merge · branch `feat/dashboards-tier3b`.
