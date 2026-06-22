# Phase 3d-C — Dashboards: filter / parameter (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-phase-3d-c-dashboard-filters-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Tiếp nối:** Phase A (lưới, PR #25) + Phase B (card variety + view controls, PR #26, `b66cde3`)

## Mục tiêu

Thêm **dashboard filter** kiểu Metabase (parameters): một thanh widget filter trên
lưới, giá trị map vào cột của từng card và inject vào query mỗi card, kèm nội suy
`{{slug}}` trong text/heading card. Tính năng đặc trưng nhất của dashboard Metabase
— làm đúng mức trung thực như Phase A/B.

## Căn cứ Metabase (clean-room)

Đọc source (`data-portal/metabase`: `metabase-types/api/parameters.ts`,
`metabase-lib/v1/parameters/constants.ts`, `parameter-substitution.ts`):

- Dashboard sở hữu `parameters: Parameter[]`. `Parameter` có `id, name, slug, type,
  default?, required?, sectionId?, filteringParameters?`. `type` encode cả
  **operator** (`string/=`, `string/contains`, `number/=`, `number/between`,
  `date/range`, `date/relative`, `id`, `category`…).
- Per-dashcard: `parameter_mappings: [{ parameter_id, target }]`,
  `target = ["dimension", ["field", id, opts]]` — map param tới một field của card.
- Giá trị param tới query card dưới dạng filter trên field đã map (request mang
  `parameters: [{type,target,value}]`). Live (không Apply); `required` dùng
  `default`. Category mặc định **multi-select** (`in`); nguồn giá trị = distinct
  values của field đã nối.
- Giá trị sync **URL** theo slug. Text/heading thay `{{slug}}` qua
  `fillParametersInText`.
- **Linked filters** (`filteringParameters`) — **defer**.

### Khớp egret (không đụng engine)

egret build SQL từ `QuerySpec { metrics, dimensions, columns, filters:
ChartFilter[], ... }`. `ChartFilter = { col, op, value }`, op
`eq|ne|gt|ge|lt|le|like|in|not_in|is_null|is_not_null` (đã kiểm: `like` render
`col LIKE <value>` nguyên văn; `in` render `col IN (...)`). Nên một param value map
vào cột của card = inject thêm 1–2 `ChartFilter` **client-side** vào spec trước
pipeline `build-chart-data` → submit → poll sẵn có. Map theo **tên cột** (egret
không có field id). Lưu params cần **migration 0016** (`dashboards.parameters
jsonb`); mapping per-card để trong `dashboard_cards.options` (không đổi schema).

## Phạm vi (PR này)

**Có:** param text / number / date / id / category kèm operator chọn được; map cột
per-card tường minh; thanh widget filter; inject client-side; sync URL theo slug;
nội suy `{{slug}}`; category multi-select với dropdown distinct-values từ field nối.

**Defer (nêu rõ):** linked filters, location types, nguồn giá trị static/from-card,
date picker đầy đủ của Metabase (ta làm range + relative preset + single), và "does
not contain" (egret không có `NOT LIKE`).

## Data model

### Backend — migration `0016_dashboard_parameters.sql`
```sql
ALTER TABLE dashboards ADD COLUMN parameters jsonb NOT NULL DEFAULT '[]';
```
Thread `parameters` qua: struct `Dashboard` + `COLS`; `DashboardDetailDto`;
`UpdateDashboard`; `DashboardRepo::update_meta` → **`update`** (set `name,
description, parameters`). Create khởi tạo `[]`. `replace_cards` không đổi (mapping
trong `dashboard_cards.options`). `get` trả `parameters`.

### Parameter (lưu trong `dashboards.parameters`)
```ts
type ParamType = "text" | "number" | "date" | "id" | "category";
type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"   // text
  | "=" | "!=" | "between" | ">=" | "<="                         // number
  | "range" | "relative" | "single";                            // date
interface DashboardParameter {
  id: string; name: string; slug: string;   // slug = url key + token {{slug}}, unique
  type: ParamType; operator: ParamOperator;  // text/number/date chọn op; id/category cố định
  default?: unknown; required?: boolean;
}
```
`id` (cố định eq) và `category` (cố định multi → `in`) không hiện picker operator;
`text`/`number` có; `date` chọn mode (range / relative / single).

### Map per-card (trong `dashboard_cards.options`)
```ts
options.paramMappings: { [parameterId]: string /* tên cột */ }
```
Card chỉ bị filter nếu có mapping cho param đó. Cột map được = `params.spec.dimensions ∪ columns` của chart (field không phải metric).

## Map param → ChartFilter (`paramToFilters`)

Hàm thuần `paramToFilters(param, value, column): ChartFilter[]` (rỗng khi value trống):

- **text** is→eq · is-not→ne · contains→`like "%v%"` · starts-with→`like "v%"` · ends-with→`like "%v"`
- **number** =→eq · !=→ne · >=→ge · <=→le · between→ **2** filter ge(min)+le(max)
- **date** range→ge(start)+le(end) · relative→tính `[start,end]` từ preset (7/30/90 ngày…) rồi ge+le · single→eq(day)
- **id** → eq
- **category** → `in` (mảng giá trị, multi-select)

## Kiến trúc frontend

1. **`api/dashboards.ts`** — thêm `parameters` vào `DashboardDetail`; payload update
   thêm `parameters`. Helper thuần `paramToFilters` + `applyParamsToChart(chart,
   values, mappings)` (trả chart có `spec.filters` đã inject cho param card map).
2. **`useDashboardParams(parameters)`** — đọc/ghi value mỗi param vào URL theo slug
   (`?<slug>=value`, JSON-encode cho mảng/range), seed từ `default`; trả
   `{ values, setValue }`. Theo pattern `useDashboardViewControls` của Phase B.
3. **`useColumnValues(ws, chart, column)`** — chạy QuerySpec chỉ-dimension (distinct)
   qua build→submit→poll, trả distinct values cho dropdown category/id. Nguồn = card+cột
   đầu tiên param map tới.
4. **Thanh widget** `<DashboardFilterBar>` (view + edit) — 1 widget/param theo type:
   text input; number (2 ô cho `between`); date range / relative / single; id
   input/dropdown; category multi-select (values từ `useColumnValues`).
5. **Parameter manager** (edit) — "+ Filter" thêm param (name→slug, type, operator,
   default, required); editor sửa/xoá; mutate draft `parameters` (cạnh `draftCards`),
   lưu qua whole-layout PUT (gửi kèm `parameters`).
6. **Map per-card UI** (edit) — mỗi card, mỗi param → dropdown chọn cột của chart
   (hoặc "none"), ghi `options.paramMappings[paramId]`.
7. **Inject** — `DashboardView` tính `applyParamsToChart(chart, values,
   card.options.paramMappings)` cho mỗi card, truyền chart đã inject xuống. Vì
   `useChartData` key theo `JSON.stringify(spec)`, đổi value param → đổi filter →
   đổi key → card **tự refetch**. Không cần nonce mới.
8. **Nội suy `{{slug}}`** — `interpolateParams(text, parameters, values)` thay token;
   TextCard/HeadingCard nhận text đã nội suy (DashboardCardView truyền values xuống).

## Xử lý lỗi

- Param map vào cột card không còn → card đó không bị filter (bỏ qua mapping), không crash.
- Required không có value lẫn default → widget gắn cờ; card vẫn render (không chặn cứng).
- Value trống → `paramToFilters` trả `[]`.
- `between`/`range` chỉ 1 cận → chỉ inject cận có.
- Query distinct lỗi → dropdown fallback nhập tay, không crash.
- Trùng slug → uniquify khi tạo.

## Kiểm thử

- **Backend:** mở rộng `dashboard_repo.rs` (parameters round-trip create/get/update) +
  http auth smoke. (`0016` apply — nhớ gotcha sqlx-migration-staleness khi verify
  local: rebuild `egret-metastore`.)
- **Vitest:** `paramToFilters` (mọi type+operator → đúng ChartFilter[], kể cả `%`,
  between→2 filter, category→in, relative→range); `applyParamsToChart` (chỉ inject
  param đã map, card chưa map giữ nguyên); `interpolateParams`; `useDashboardParams`
  (URL round-trip, seed default); widget render theo type; parameter manager
  add/edit/delete; mapping dropdown ghi `options.paramMappings`.
- **Live (thủ công qua Playwright MCP browser):** thêm param text + category + date,
  map vào card, đặt value → card map refetch đã lọc; `{{slug}}` hiện trong text card;
  value persist URL + reload; **0 lỗi console**. (Đây là lái browser MCP thủ công,
  KHÔNG phải test Playwright tự động — egret không có suite Playwright; test tự động
  là Vitest.)
- **Full sweep** trước mỗi commit: `npm run typecheck && lint && build && vitest run`.

## Ngoài phạm vi (sau)

Linked filters, location, nguồn static/card, full date picker, "does not contain";
click-behaviour, tabs, public share (Phase D).

## Quy ước

Metabase là chuẩn · clean-room (không AGPL) · TDD · full sweep trước mỗi commit ·
identity git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus
4.8 (1M context) <noreply@anthropic.com>` · chỉ stage path tường minh · squash-merge.
