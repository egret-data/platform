# Date Grouping — Sub-project B: Breakout đơn vị thời gian trong builder (Thiết kế)

**Bản tham chiếu tiếng Việt.** Bản chuẩn: `docs/en/specs/2026-06-24-date-grouping-builder-ui-design.md`.
**Thuộc chương trình:** Date Grouping (A → B → C). Đây là **B** (UI builder), phụ thuộc **A** (đã merge,
PR #47 — backend `temporal_units` + SQL). **B chỉ frontend, không đụng backend.**

## Mục tiêu

Trong Explore builder, cho người dùng chọn **đơn vị thời gian** cho bất kỳ dimension temporal (dropdown
nhỏ cạnh field) → date/timestamp gom theo day/week/month/quarter/year (truncate) hoặc hour-of-day/
day-of-week/month-of-year… (extract). Đơn vị chảy vào `spec.temporal_units` (A xử lý). Chart gom theo
extract hiển thị **nhãn thân thiện** (Mon…Sun, Jan…Dec, Q1…Q4) thay vì số nguyên.

## Quyết định (từ brainstorm)

- **Phạm vi selector:** *mọi* dimension temporal — nơi cột đã chọn `is_temporal`: cartesian xAxis,
  pie/funnel/waterfall/smartscalar dimension, và từng cột temporal trong list table/pivot.
- **Auto-default `month`** khi chọn cột temporal vào ô dimension (kiểu Metabase).
- **Nhãn extract thân thiện** map trong B.

## Ngoài phạm vi (ranh giới rõ)

- **Nhãn thân thiện chỉ render ở cartesian + pie** (trục categories + nhãn slice/legend). Table cell,
  funnel, waterfall vẫn hiện SỐ NGUYÊN extract — selector vẫn hiện cho các dimension đó, nhưng map nhãn
  ở đó để polish sau. Truncate render là ngày (format ngày sẵn có lo).
- Không có dashboard Date-Grouping param (đó là C). Không đụng backend.

## Token đơn vị (PHẢI khớp A)

Truncate: `minute, hour, day, week, month, quarter, year`.
Extract: `minute_of_hour, hour_of_day, day_of_week, day_of_month, day_of_year, week_of_year,
month_of_year, quarter_of_year`. Vắng key = raw (lựa chọn "None").

## Component

**1. `frontend/src/components/chart/temporal-units.ts`** (module chung — nguồn duy nhất):
- `TEMPORAL_UNITS`: list `{ token, label, group: "trunc"|"extract" }`.
- `isExtractUnit(token)`.
- `temporalLabel(token, value)`: day_of_week 1–7 → Mon…Sun, month_of_year 1–12 → Jan…Dec,
  quarter_of_year 1–4 → Q1…Q4; còn lại + truncate + undefined → `String(value)`. Không bao giờ throw.

**2. `TemporalUnitSelect.tsx`**: shadcn `<Select>` (`h-8 text-sm`), props `value?: string`,
`onChange(token?)`. Option "None" (clear) + 2 nhóm "Group by"/"Extract" từ `TEMPORAL_UNITS`.

## Wiring `ChartExplore.tsx`

1. State `temporalUnits: Record<string,string>`, hydrate từ `p.spec?.temporal_units`.
2. Auto-default: cột `is_temporal` được chọn vào dimension mà chưa có key → set `"month"`.
3. Render `<TemporalUnitSelect>` sau mỗi control dimension **chỉ khi** cột đó `is_temporal`
   (cartesian.xAxis, pie/funnel/waterfall/smart dimension, từng cột temporal trong tableCols/pRows/pCols).
   `setUnit(col, u)`: set key, hoặc xoá key nếu `u` undefined.
4. **Prune (BẮT BUỘC — A chặn orphan key):** helper
   `unitsFor(dims) = Object.fromEntries(Object.entries(temporalUnits).filter(([k]) => dims.includes(k)))`.
   Mỗi nhánh viz trả `dimensions: dims` thêm `temporal_units: unitsFor(dims)` trong `specForRun()` +
   `paramsForSave()`. Đổi x-axis khỏi cột temporal → key cũ tự rớt.
5. Thêm `temporal_units?: Record<string,string>` vào kiểu spec ở `frontend/src/api/charts.ts`.

## Renderer: nhãn extract

`ChartRenderer.tsx` đọc `params.spec.temporal_units` rồi truyền vào builder:
- **cartesian.ts**: cfg thêm `xAxisUnit?`; nếu `isExtractUnit(xAxisUnit)` → map category + tooltip qua
  `temporalLabel`. Renderer truyền `params.spec.temporal_units?.[cartesian.xAxis]`.
- **pie.ts**: cfg thêm `dimensionUnit?`; map tên slice. Renderer truyền `…?.[pie.dimension]`.
Truncate không cần map. Map chỉ đổi nhãn HIỂN THỊ; drill-through vẫn dùng giá trị thô.

## Test (Vitest)

`temporal-units.test`: `temporalLabel` map đúng (day_of_week 1→Mon/7→Sun, month 1→Jan/12→Dec, quarter
2→Q2; truncate + ngoài dải → String); `isExtractUnit` phân loại đúng.
`TemporalUnitSelect.test`: render nhóm + "None"; chọn option → onChange(token); chọn None → onChange(undefined).
`unitsFor` prune + auto-default `month`.
Live verify (Playwright, 0 console error, chart throwaway): chọn date x-axis → mặc định Month gom tháng;
đổi "Day of week" → trục Mon…Sun; đổi x-axis sang cột không-temporal → request OK (không 400 orphan-key).
Sweep frontend xanh.

## File

Tạo: `temporal-units.ts`, `TemporalUnitSelect.tsx`, 2 test. Sửa: `ChartExplore.tsx`, `api/charts.ts`,
`ChartRenderer.tsx`, `echarts/options/cartesian.ts`, `echarts/options/pie.ts`.

## Quy ước

Sweep `npm run typecheck && lint && build && npx vitest run` (từ `frontend/`); git identity + footer
Co-Authored-By; stage path tường minh.
