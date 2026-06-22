# Phase 3d-D1 — Dashboards: click behaviour (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-phase-3d-d1-click-behaviour-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Tiếp nối:** Phase A/B/C (đã merge PR #25/#26/#27, `903df67`)

## Mục tiêu

Cho card dashboard tương tác: bấm vào một giá trị có thể **cross-filter** dashboard (đặt
một parameter Phase C), mở **URL tùy biến**, hoặc hiện **drill menu** mặc định ("Filter
dashboard by …" / "See these records"). Tương tác đặc trưng nhất của dashboard Metabase.

Phase D chia thành các sub-phase độc lập; đây là **D1 (click behaviour)**. **D2 (tabs)**
ở spec riêng. Public sharing/embedding **ngoài phạm vi Phase D** (egret là tool nội bộ).

## Căn cứ Metabase (clean-room)

Đọc source (`data-portal/metabase`: `metabase-types/api/click-behavior.ts`,
`visualizations/click-actions/`): click behaviour là blob theo từng dashcard/cột trong
`visualization_settings.click_behavior`, union `type`: `"actionMenu"` (drill menu mặc
định), `"crossfilter"` (kèm `parameterMapping` từ cột bấm tới parameter), `"link"`
(`linkType:"url"` với `linkTemplate` chứa `{{column}}`, hoặc link tới dashboard/question).
Cross-filter dispatch `setParameterValue` cho từng param map từ giá trị bấm; URL link nội
suy `{{column}}`. Mặc định rơi về drill menu. **Không lưu backend** ngoài blob settings.

### egret tái dùng (chủ yếu là đấu nối, không hạ tầng mới)

egret đã có drill primitives ở trang chart Explore: `components/chart/drill.ts`
(`DrillContext`, `drillFilters`, `tableCellDrill`, `pivotCellDrill`), `DrillMenu` (popover
"Filter by … / See these records"), `DrillRecordsDialog` (chạy query đã lọc, hiện rows).
Phase C đã thêm dashboard `parameters` + `useDashboardParams.setValue` (sync URL) +
`options.paramMappings`. D1 nối click → các thứ này.

## Phạm vi (PR này)

**Có:** click behaviour per-card (`menu` / `crossfilter` / `url`) trên **table, pivot, và
ECharts (bar/line/area/scatter/combo/pie)**; control "On click" trong edit mode; drill menu
mặc định + "See these records" trên card dashboard; cross-filter vào parameter Phase C;
URL tùy biến với nội suy `{{column}}`/`{{value}}`.

**Defer (nêu rõ):** link tới dashboard/question khác, action button, click behaviour theo
từng cột, click trên scalar/gauge/funnel… (chỉ các viz có dimension nêu trên).

## Data model — không đổi backend

Per-card, trong `dashboard_cards.options` jsonb:
```ts
type ClickBehavior =
  | { type: "menu" }                                    // mặc định (hoặc absent)
  | { type: "crossfilter"; parameterId: string }        // bấm giá trị → đặt parameter này
  | { type: "url"; template: string };                  // bấm → mở URL ({{column}} token)
// options.clickBehavior?: ClickBehavior
```

## Plumbing value-click (mảnh hạ tầng mới duy nhất)

Một click trên card phải cho ra **`DrillContext`** (`{ dims: {col,value}[] }`) — thứ D1 đã
biết xử lý. Hai đường render:
- **Table / pivot** — `ChartRenderer` đã có thể nối `onCellDrill` sẵn (table/pivot tạo
  `DrillContext` qua `tableCellDrill`/`pivotCellDrill`). Renderer read-only của dashboard đã
  bỏ nó (Phase A); D1 bật lại sau một prop opt-in mới.
- **ECharts** — `EChartsRenderer` thêm prop optional
  `onPointClick?: (p: { name?; seriesName?; value? }) => void` nối vào instance
  `chart.on("click", …)`. `ChartRenderer` dịch point-click thành `DrillContext` dùng cột
  dimension của viz: cartesian → `cartesian.xAxis` (+ `series` làm dim 2 nếu có), pie →
  `pie.dimension`. Giá trị bấm = category (`params.name`).

`ChartRenderer` thêm 1 prop optional: **`onValueClick?: (ctx: DrillContext) => void`**. Khi
có, nó nối onCellDrill (table/pivot) + onPointClick (ECharts) để dựng `DrillContext` rồi gọi
`onValueClick`. Khi không có, hành vi y như hiện tại (read-only; ChartExplore + card chưa
map không đổi).

## Resolve click (helper thuần)

`resolveClick(clickBehavior, ctx, card, parameters)` quyết định click làm gì
(`components/chart/dashboard/click-behavior.ts`, unit-test):
- `url` → nội suy `{{col}}`/`{{value}}` trong `template` từ `ctx.dims` (dim đầu là primary),
  guard `^https?://` (dùng lại pattern safe-URL của Phase B); trả `{ kind:"url", href }`.
- `crossfilter` → tìm dim bấm có cột = mapping của param trên card này
  (`card.options.paramMappings[parameterId]`), nếu không thì primary dim; trả
  `{ kind:"crossfilter", parameterId, value }`.
- `menu` / unset → `{ kind:"menu", ctx }` (mở drill menu).

## Đấu nối frontend

- **`DashboardCardView`** thêm `onValueClick?` truyền xuống `ChartRenderer` (chỉ chart card).
- **`DashboardView`** cấp `onValueClick` per-card: đọc `options.clickBehavior`, gọi
  `resolveClick`, rồi:
  - `url` → `window.open(href, "_blank", "noreferrer")`.
  - `crossfilter` → `paramsState.setValue(param.slug, value)` (Phase C) → card map refetch.
  - `menu` → mở **`DrillMenu`** tại con trỏ. Mục "Filter dashboard by {col} is {val}"
    cross-filter parameter nào map (Phase C) tới cột đó; "See these records" mở
    **`DrillRecordsDialog`** cho dataset của card + `drillFilters(ctx)`.
  - State drill-menu + records-dialog ở `DashboardView` (giống ChartExplore).
- **Edit mode** — control **"On click"** per chart card: Menu (mặc định) / Cross-filter
  (→ `<select>` parameter) / Custom URL (→ input `template` kèm gợi ý `{{column}}`), ghi
  `options.clickBehavior` qua `updateCardOptions`.

## Luồng dữ liệu

```
bấm giá trị trên card
  → ChartRenderer (onValueClick) dựng DrillContext
  → DashboardView.onValueClick(card, ctx)
       → resolveClick(card.options.clickBehavior, ctx, card, parameters)
            ├ url        → window.open(href nội suy)
            ├ crossfilter→ paramsState.setValue(slug, value)  → card map refetch (Phase C)
            └ menu       → <DrillMenu> → Filter (cross-filter param map) | See records (<DrillRecordsDialog>)
```

## Xử lý lỗi

- `url` template ra URL không phải `http(s)` → không mở (guard XSS).
- `crossfilter` param đã xoá, hoặc không resolve được giá trị bấm → no-op.
- Click behaviour chỉ áp viz có dimension; scalar/gauge… bỏ qua click.
- Card không có `clickBehavior` và không có cột map được → menu vẫn có "See these records";
  mục "Filter by" chỉ hiện cho cột có param map.
- ChartExplore (truyền `onCellDrill` riêng, không phải `onValueClick`) không đổi.

## Kiểm thử

- **Vitest:** `resolveClick` (nội suy url + guard safe-URL; resolve giá trị crossfilter qua
  paramMappings vs primary dim; menu fallthrough); `ChartRenderer` gọi `onValueClick` đúng
  `DrillContext` cho cell table và cho point ECharts (mock click echarts); `EChartsRenderer`
  đăng ký click handler; config "On click" ghi `options.clickBehavior`; cross-filter của
  DashboardView đặt giá trị param; drill menu + records dialog mở.
- **Live (thủ công, Playwright MCP browser — egret không có suite Playwright tự động):**
  thêm 2 card chung 1 dimension + 1 param map vào dimension đó; đặt click của 1 card thành
  cross-filter → bấm 1 giá trị → card kia refetch đã lọc + URL có slug; đặt 1 card thành
  custom URL → bấm mở URL; menu mặc định → "See these records" hiện rows. **0 lỗi console.**
- **Full sweep** trước mỗi commit: `npm run typecheck && lint && build && vitest run`.

## Ngoài phạm vi (sau / phase khác)

Dashboard tabs (D2); link tới dashboard/question, action button, click behaviour theo cột,
public sharing/embedding.

## Quy ước

Metabase là chuẩn · clean-room (không AGPL) · TDD · full sweep trước mỗi commit · identity
git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · chỉ stage path tường minh · squash-merge.
