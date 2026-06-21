# Phase 3c-3.8 — Biểu đồ ECharts (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-21-phase-3c-3-8-echarts-viz-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-21 · **Tiếp nối:** 3c-3.7 (drill-through + redesign config, đã merge PR #18)

## Mục tiêu

Thêm biểu đồ vào UI Explore — **bar, line, area, scatter, combo, pie,
big-number** — vẽ bằng Apache ECharts, **bám sát Metabase cả về hành vi lẫn
style** (đúng mức độ trung thực đã áp cho Table và Pivot).

## Vì sao ECharts / vì sao theo Metabase

Metabase vẽ chart bằng **ECharts 6 core** qua một React renderer mỏng
(`init(el, null, { renderer: "svg" })`, `setOption` khi đổi, resize khi đổi kích
thước, `dispose` khi unmount, đăng ký module một lần). Ta theo y hệt cách tích
hợp đó để thừa hưởng thế mạnh của ECharts (data lớn, mô hình cartesian phong
phú) và tái tạo style chart của Metabase bằng đúng hằng số style họ công bố
(palette, font, độ đậm, opacity).

## Phạm vi

Trong phạm vi (một phase, ~12–14 task):

- **Họ cartesian** — `bar`, `line`, `area`, `scatter`, `combo`. Dùng chung một
  config + một option builder; loại chart và type từng series là biến thể trên
  cùng một mô hình.
- **Pie** — config + builder nhỏ riêng.
- **Big number** (`scalar`) — một giá trị đã format; **không** dùng ECharts.

Ngoài phạm vi (phase sau): dashboard (3c-4), trend/goal line, nhiều trục y,
drill-through *từ* chart (phase này chart chỉ hiển thị; drill vẫn là tính năng
của Table/Pivot), funnel/map/sankey/waterfall, lưu màu series tùy biến.

## Kiến trúc

### Dependency

Thêm `echarts@^6` (khớp major với Metabase). Import từ `echarts/core` và chỉ
đăng ký module dùng đến — không import gói `echarts` tổng — để bundle được
tree-shake.

### `EChartsRenderer` (wrapper React mỏng)

`src/components/chart/echarts/EChartsRenderer.tsx`, theo `EChartsRenderer` của
Metabase:

- `init(el, null, { renderer: "svg" })` khi mount.
- `setOption(option, true)` (notMerge) khi mount và mỗi khi `option` đổi.
- `ResizeObserver` trên container gọi `chart.resize()` (Metabase truyền
  width/height tường minh; ta dùng ResizeObserver để chart lấp đầy flex cha một
  cách responsive).
- `dispose()` khi unmount; chống dispose hai lần để an toàn với double-mount của
  React 18 StrictMode.
- Props: `{ option: EChartsCoreOption; className?: string }`. Xử lý event ngoài
  phạm vi phase này (chart chỉ hiển thị).

### Đăng ký module

`src/components/chart/echarts/register.ts` xuất `registerEChartsModules()` gọi
một lần khi nạp module renderer. Đăng ký: `SVGRenderer`; `BarChart, LineChart,
ScatterChart, PieChart`; `GridComponent, TooltipComponent, LegendComponent,
DatasetComponent, MarkLineComponent` (markline để dành, phase này chưa dùng).
Area = `LineChart` + `areaStyle`; combo = từng series `bar`/`line`.

## Mô hình dữ liệu

### VizType

Mở rộng `VizType` trong `src/api/charts.ts`:

```ts
export type VizType =
  | "table" | "pivot_table"
  | "bar" | "line" | "area" | "scatter" | "combo" | "pie" | "scalar";
```

Thêm entry gallery vào `chartTypes.ts` với icon lucide: `BarChart3` (bar),
`LineChart` (line), `AreaChart` (area), `ScatterChart` (scatter), `Spline`
(combo), `PieChart` (pie), `Hash` (scalar). Giữ nguyên shape
`{ key, name, description, icon }` nên `ChartTypeGallery` không cần đổi.

### ChartParams

Thêm ba sub-object tùy chọn theo viz (song song cách field pivot gắn vào
`ChartParams` hiện nay). Chỉ object khớp viz đang chọn được điền:

```ts
export interface CartesianConfig {
  xAxis?: string;            // 1 dimension (category / x)
  yMetrics: string[];        // một hoặc nhiều metric (y)
  series?: string;           // dimension thứ 2 (breakout → mỗi giá trị 1 series)
  stacked?: boolean;         // bar/area
  horizontal?: boolean;      // bar
  smooth?: boolean;          // line/area
  showLabels?: boolean;      // nhãn dữ liệu trên điểm/cột
  showLegend?: boolean;      // mặc định true khi có series hoặc >1 metric
  xLabel?: string; yLabel?: string;
  seriesTypes?: Record<string, "bar" | "line">;  // chỉ combo: metric → type
}
export interface PieConfig {
  dimension?: string;        // category
  metric?: string;           // value
  donut?: boolean;
  showLegend?: boolean;      // mặc định true
  showLabels?: boolean;      // nhãn % lát
}
export interface ScalarConfig {
  metric?: string;
}
// trên ChartParams:
cartesian?: CartesianConfig;
pie?: PieConfig;
scalar?: ScalarConfig;
```

### Suy ra query (`specForRun`)

`ChartExplore.specForRun()` suy `QuerySpec` từ config viz đang chọn:

- **Cartesian:** `dimensions = [xAxis, series].filter(Boolean)`,
  `metrics = yMetrics`, kèm `filters`/`row_limit` sẵn có.
- **Pie:** `dimensions = [dimension]`, `metrics = [metric]`.
- **Scalar:** `dimensions = []`, `metrics = [metric]` (một dòng đã tổng hợp).

Backend `chartquery::build` đã hỗ trợ (group-by + aggregate); không đổi backend.

## Option builder (thuần, unit-test)

Hàm thuần nhận `(columns, rows, config)` trả về `EChartsCoreOption` — không
React, không side effect ECharts — nên unit-test như pivot engine và drill
builder.

### `buildCartesianOption(columns, rows, cfg)`

`src/components/chart/echarts/options/cartesian.ts`:

- Tra chỉ số cột cho `xAxis`, từng `yMetric`, và `series`.
- **Không có series dim:** mỗi y-metric là một ECharts series; trục category =
  các giá trị x phân biệt theo thứ tự dòng.
- **Có series dim:** pivot các dòng thành một ECharts series cho mỗi giá trị
  series phân biệt (với y-metric đầu tiên; nhiều metric × series ngoài phạm vi —
  khi đặt `series` thì dùng một y-metric, ghi rõ trong guard). Trục category =
  các x phân biệt; ô (x, series) thiếu → null.
- `kind` ánh xạ sang series `type`: bar/line/scatter; `area` = line + `areaStyle`
  (opacity 0.3); `combo` = type từng metric theo `seriesTypes` (mặc định bar).
- `stacked` → `stack: "total"` trên series bar/area. `horizontal` → hoán đổi
  trục x/y (category trên y). `smooth` → làm mượt line. `showLabels` →
  `label.show` với style series-label.
- Tooltip `trigger: "axis"` (item cho scatter), legend dưới khi `showLegend`.
  Format giá trị qua `columnSettings[metric]` + `formatValue` tái dùng ở
  axis/tooltip/label.

### `buildPieOption(columns, rows, cfg)`

`src/components/chart/echarts/options/pie.ts`: một pie series, `data` = dòng map
thành `{ name: dimensionValue, value: metricValue }`; `donut` →
`radius: ["45%", "70%"]` ngược lại `["0%", "70%"]`; nhãn hiện name + phần trăm
khi `showLabels`; legend dưới.

### Big number

`src/components/chart/BigNumber.tsx` — không ECharts. Hiển thị giá trị tổng hợp
lớn, canh giữa, format qua `ColumnFormat` của metric (`formatValue`), tên metric
làm caption. Theo layout Number viz của Metabase (giá trị 2–2.5rem đậm, caption
mờ).

## Style — bám Metabase (giá trị cụ thể)

`src/components/chart/echarts/palette.ts` + một object `chartTheme` dùng chung
cấp cho builder. Giá trị lấy từ Metabase:

- **Palette series** (xoay vòng): `#509EE3, #88BF4D, #A989C5, #EF8C8C, #F9D45C,
  #F2A86F, #98D9D9, #7172AD`.
- **Font:** tick & tên trục weight 400; font nhãn trục 13px; nhãn dữ liệu/series
  weight 700, size 13, offset 4.
- **Cột:** tỉ lệ `barWidth` 0.8; z của bar dưới line/area.
- **Scatter:** `symbolSize` 6, opacity 0.8. **Area:** fill opacity 0.3.
- **Độ dày line:** mặc định M = 2px.
- **Padding grid:** x 8, y 12; lề tên trục 12.
- **Đường grid/trục:** dùng token border của theme egret (hairline nhạt, khớp
  `--mb-color-cartesian-grid-line` của Metabase); splitLine chỉ ở trục giá trị
  (Metabase hiện gridline ngang, ẩn dọc với bar/line).
- **Tooltip/legend:** renderer SVG; legend dưới; tooltip card ECharts mặc định
  với chữ theo theme.

Container: chart lấp đầy vùng cao cố định responsive trong preview Explore (cùng
vùng `max-h` table dùng), `rounded-md border` khớp Table/Pivot.

## Display tab (theo viz)

Tái dùng pattern panel settings sẵn có (SectionLabel, Switch, Select,
FormatSection):

- **Cartesian:** Switch — Legend, Stacked (bar/area), Horizontal (bar), Smooth
  (line/area), Data labels. Input — nhãn X, nhãn Y. Combo: picker bar/line nhỏ
  theo từng metric. Format số của metric qua `FormatSection` (chi phối
  tooltip/axis/label).
- **Pie:** Switch — Legend, Donut, Hiện nhãn.
- **Scalar:** `FormatSection` của metric (prefix/suffix/decimals/compact).

## Guard, rỗng & lỗi

Chart chỉ vẽ khi các field bắt buộc đã đặt; nếu không hiện hint "Configure the
controls on the left and click Run." (cùng pattern guard pivot trong
`ChartExplore`):

- Cartesian: cần `xAxis` và ≥1 `yMetrics`.
- Pie: cần `dimension` và `metric`.
- Scalar: cần `metric`.

Trạng thái query/loading/failed tái dùng pipeline run sẵn có (`useBuildChartData`
→ submit → poll → results). Kết quả rỗng → thông báo "No data" canh giữa trong
khung chart.

## Kiểm thử

- **Unit (Vitest):** `buildCartesianOption` (đa metric không series; breakout
  series; stacked; horizontal; ánh xạ type area/scatter/combo; format giá trị)
  và `buildPieOption` (lát, donut, phần trăm) — thuần, không DOM. Format
  `BigNumber`. Reducer config / suy `specForRun` theo viz.
- **Smoke component:** `EChartsRenderer` mount, gọi `setOption`, dispose khi
  unmount — với `echarts/core` được mock (jsdom không đo canvas/svg thật).
- **Live (Playwright):** một smoke mỗi loại viz — cấu hình trục, Run, kiểm tra
  có SVG chart; bật một tùy chọn Display và thấy phản ánh.

## Cấu trúc file

```
src/components/chart/echarts/
  EChartsRenderer.tsx          # wrapper mỏng (init/setOption/resize/dispose)
  register.ts                  # registerEChartsModules() — một lần
  palette.ts                   # palette series Metabase + hằng chartTheme
  options/cartesian.ts         # buildCartesianOption (+ __tests__)
  options/pie.ts               # buildPieOption (+ __tests__)
src/components/chart/BigNumber.tsx
src/components/chart/settings/   # các section Display mới cho chart (cartesian/pie)
# sửa:
src/api/charts.ts              # VizType + CartesianConfig/PieConfig/ScalarConfig
src/components/chart/chartTypes.ts   # entry gallery
src/routes/explore/ChartExplore.tsx  # zone config, specForRun, switch render
```

## Rủi ro / ghi chú

- **jsdom + ECharts:** ECharts cần đo layout thật; unit test mock renderer và
  test builder thuần. Đó là lý do builder phải thuần.
- **Double-mount StrictMode:** guard dispose tránh cảnh báo "instance already
  disposed".
- **series + đa metric:** khi chọn breakout `series` thì giới hạn một y-metric
  (guard ghi rõ) để mô hình series không nhập nhằng.
