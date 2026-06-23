# Tier 3b — Kế hoạch triển khai: date picker, value sources, per-card viz override

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-tier3b-filters-and-viz-override.md`. Spec: `docs/en/specs/2026-06-23-tier3b-filters-and-viz-override-design.md`.

**Mục tiêu:** date picker hợp nhất kiểu Metabase, nguồn giá trị filter (static/from-card), override viz từng card.
**Chỉ frontend** (param trong `dashboards.parameters` jsonb, override trong `dashboard_cards.options` jsonb — không backend/migration).
Branch `feat/dashboards-tier3b` (off main, spec đã commit).

**Quy ước mọi task:** TDD (đỏ→xanh); full sweep trước commit (từ `frontend/`): `npm run typecheck && lint && build && npx vitest run`;
stage path tường minh; identity git + footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Defer:** mọi backend; does-not-contain; tab Exclude của date; "from connected fields"; override query từng card (vizOverride chỉ display).

## Các task (code đầy đủ ở bản tiếng Anh)

### Nhóm A — Date all-options
1. **Operator**: `api/dashboards.ts` ParamOperator thêm `"all-options"`; `ParameterEditor.tsx` OPERATORS.date thêm.
2. **params.ts**: paramToFilters nhánh all-options — value `{mode:"relative",dir,n,unit}` hoặc `{mode:"specific",op,from,to?}`;
   relative → `ge/le` quanh now (tổng quát hóa relative-day cho mọi unit); specific on→eq/before→lt/after→gt/between→ge+le;
   thiếu→`[]`. summarize gắn nhãn. Unit test.
3. **DateFilterPicker** (component mới): 2 tab Relative/Specific, emit value có cấu trúc. Vitest.
4. **FilterPill**: render DateFilterPicker khi `type==="date" && operator==="all-options"`. Vitest.

### Nhóm B — Value sources
5. **Type**: `api/dashboards.ts` thêm `ParamValuesSource {type:"distinct"|"static"|"card"; static?; cardId?; column?}` + `DashboardParameter.valuesSource?`.
6. **FilterSettingsPanel**: control "Where do values come from?" (category/id/location/text): Distinct / Custom list (textarea→static[]) /
   From a card (chọn card + cột, qua prop `cardColumns`); ghi `valuesSource`. EditSidebar truyền `cardColumns`. Vitest.
7. **Resolver + loader**: `valuesSource.ts` `resolveValuesSource(param, cards, chartById) → {kind:"static",values}|{kind:"query",datasetId,column}|null`;
   `DashboardView` ParamOptions resolve: static→`StaticOptions` emit thẳng; query→`ParamOptions`(+linked filters); null→bỏ. Cấp `cardColumns` cho sidebar. Vitest cho resolver.

### Nhóm C — Per-card viz override (lớn)
8. **Tách `ChartSettingsSections`** (component mới): nhận `{vizType, params, columns, rows?, onChange}`, render các display section
   (Cartesian/Pie/TableColumns/Format/ConditionalFormatting) y như Display-tab của `ChartExplore` (≈719-755) + các derived
   (cartesianSeries/pieSlices/styleColumns/resultColumns). **Refactor `ChartExplore` dùng nó, hành vi KHÔNG đổi** (test ChartExplore là guard).
   Nếu quá rối không giữ được hành vi → STOP & báo, không ship đổi hành vi chart builder. Vitest.
9. **applyVizOverride** (params.ts): `(chart, override?) → {viz_type, params}` deep-merge field display lên `chart.params`, override
   `viz_type` nếu có, **không bao giờ đụng `spec`**; no-override = identity. Unit test.
10. **DashboardCardView**: `eff = applyVizOverride(chart, card.options.vizOverride)` → `<ChartRenderer vizType params>`; giữ thứ tự
    applyParamsToChart (inject spec) trước. Vitest (mock ChartRenderer).
11. **CardVizPanel** (mới) + wiring: editPanel kind `"viz"` mở từ CardEditToolbar ("Visualization"); render ChartSettingsSections bound
    vào effective params (chart.params ⊕ override), ghi `card.options.vizOverride` (lưu full display đã sửa), nút **Reset** xóa override.
    EditSidebar + DashboardView wiring (`updateCardOptions(cardId, {vizOverride})`). Vitest.

### Nhóm D
12. **Verify live + PR**: full sweep; stack + login `admin@egret.local` (backend :8080 đã current); Playwright song song Metabase:
    date all-options (relative+specific) lọc card; value = custom list & từ card khác; override viz 1 card + Reset (chart gốc không đổi);
    Tier 1/2d/3a vẫn chạy; **0 lỗi console**; **không đụng a1**, xóa dashboard test; push + PR + squash-merge khi CI xanh.

## Thứ tự
A (1–4) · B (5–7) · C (8–11) · D (12).
