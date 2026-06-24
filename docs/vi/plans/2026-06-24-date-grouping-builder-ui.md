# Date Grouping — Sub-project B (Breakout đơn vị trong builder) — Plan (tham chiếu VN)

**Bản chuẩn:** `docs/en/plans/2026-06-24-date-grouping-builder-ui.md`. **Spec:**
`docs/en/specs/2026-06-24-date-grouping-builder-ui-design.md`. **Branch:** `feat/date-grouping-builder-ui`.
**Phụ thuộc:** A (đã merge, PR #47). Chỉ frontend. Sweep `npm run typecheck && lint && build && vitest` (từ
`frontend/`) trước mỗi commit; footer Co-Authored-By; không push tới Task 7.

**Token PHẢI khớp A:** trunc `minute,hour,day,week,month,quarter,year`; extract `minute_of_hour,
hour_of_day,day_of_week,day_of_month,day_of_year,week_of_year,month_of_year,quarter_of_year`.

**Task 1 — `temporal-units.ts`** (TEMPORAL_UNITS 15 token + label/group, `isExtractUnit`, `temporalLabel`
map day_of_week→Mon…Sun / month_of_year→Jan…Dec / quarter_of_year→Q1…Q4 / còn lại→String, `pruneUnits`)
+ test. Commit.

**Task 2 — `TemporalUnitSelect.tsx`** (shadcn Select nhóm Group by/Extract + None, `h-7 text-xs`) + test
(chỉ assert nhãn đóng — Radix không mở list trong jsdom). Commit.

**Task 3 — type + plumbing:** thêm `temporal_units?` vào `QuerySpec` (charts.ts); state `temporalUnits` +
hydrate từ `p.spec?.temporal_units`; helper `setUnit(col, token?)`; chèn `temporal_units:
pruneUnits(temporalUnits, dims)` vào MỌI nhánh aggregate của `specForRun()` (cartesian/pie/funnel/
waterfall/smart/sankey/pivot/table-agg); `useEffect` auto-default `month` cho dimension temporal mới (dep
`[activeDims.join("|"), columns]`). Sweep + commit. (Chưa có UI; units = {} lúc chạy.)

**Task 4 — selector inline single-dim:** import `TemporalUnitSelect`; helper `unitRow(col)` (chỉ hiện khi
cột `is_temporal`); chèn `{unitRow(field)}` sau control: cartesian.xAxis (~427), pie.dimension (~468),
funnel.dimension (~551), waterfall.xAxis (~574), smart.dimension (~601). Sweep + commit.

**Task 5 — table + pivot:** helper `unitList(dims)` (sub-block "Date grouping" liệt kê cột temporal +
TemporalUnitSelect). Chèn `{unitList([...pRows,...pCols])}` ở pivot (~416) và
`{unitList(tableMetrics.length>0?tableCols:[])}` ở table (~403). Sweep + commit.

**Task 6 — nhãn extract ở renderer:** cartesian.ts thêm cfg `xAxisUnit?`, map category hiển thị qua
`temporalLabel` khi `isExtractUnit` (giữ giá trị số để sort); pie.ts thêm `dimensionUnit?`, map tên slice;
ChartRenderer truyền `params.spec.temporal_units?.[cartesian.xAxis]` / `[pie.dimension]`. Thêm test pie
slice → "Mon"/"Sun". Sweep + commit.

**Task 7 — verify + PR:** sweep. Playwright (chart throwaway, KHÔNG đụng "a1", 0 console error): date
x-axis → mặc định Month gom tháng; đổi Day of week → trục Mon…Sun; đổi x-axis sang cột không-temporal →
không 400 (prune); pie Month of year → slice Jan…Dec; save+reload → khôi phục unit. Push + PR.
