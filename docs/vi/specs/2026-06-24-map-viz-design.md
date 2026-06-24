# Map Visualization (Thiết kế)

**Bản tham chiếu tiếng Việt.** Bản chuẩn: `docs/en/specs/2026-06-24-map-viz-design.md`.

## Mục tiêu

Thêm viz **Map** (choropleth): tô màu vùng theo metric. MVP có **3 map** — **US states**, **World
countries**, **Vietnam (tỉnh)** — chọn trong builder. Lấp chỗ clone đang tạm dùng row chart cho "Revenue by
state". Reuse ECharts (`map` series) → **không thêm dependency runtime**.

## Quyết định

3 map; chỉ choropleth (không point/bubble); **geojson preprocess + commit sẵn**, keyed bằng đúng mã/tên data
dùng → runtime so khớp = so chuỗi đơn giản, CI không cần mạng.

## Asset geojson (commit) + script prep 1 lần

Giá trị nhận diện vùng: US `state` = mã 2 ký tự (NH); World `country` = ISO-A2 (NO); VN = tên tỉnh.
Script `scripts/build_map_geojson.py` (commit) tải 3 nguồn công khai, chuẩn hoá mỗi feature.properties về
đúng `{ key, name }`, làm tròn toạ độ 2 chữ số, ghi 3 asset vào
`frontend/src/components/chart/echarts/maps/`:

| asset | nguồn | key (khớp data) | name (tooltip) |
|---|---|---|---|
| `us-states.geo.json` | PublicaMundi us-states | mã bang (bảng name→abbr 51 dòng trong script) | tên bang |
| `world-countries.geo.json` | Natural Earth ne_110m_admin_0_countries | `ISO_A2_EH` (uppercase; fix `-99` của NE) | `NAME` |
| `vietnam-provinces.geo.json` | geoBoundaries gbOpen VNM ADM1 (qua API `gjDownloadURL`) | `shapeName` (tên tỉnh) | `shapeName` |

Script commit để tái tạo; **asset sinh ra cũng commit** → app/CI không tải gì lúc build/run.

## Registry — `echarts/maps/registry.ts`

`MapDef = { key, label, echartsName, load: ()=>Promise<GeoJson>, normalize: (v)=>string }`. `MAPS` có
`us_states`/`world`/`vietnam`. US/World `normalize` = uppercase mã; VN = trim tên. `load` = dynamic
`import("./*.geo.json")` (lazy chunk).

## `MapChart.tsx`

Props columns/rows/params. Lấy `def = MAPS[params.map?.mapKey ?? "us_states"]`. `useEffect([def.key])`:
`await def.load()` → `echarts.registerMap(def.echartsName, geojson, { nameProperty: "key" })` (idempotent) →
`ready`. Chưa xong → "Loading map…"; rỗng → "No data". Xong → `buildMapOption` → `<EChartsRenderer>`.
`h-full w-full`.

## `buildMapOption` (`echarts/options/map.ts`)

`data = rows.map(r => ({ name: normalize(r[regionIdx]), value: Number(r[metricIdx]) }))` (bỏ key rỗng/NaN).
`visualMap` continuous min/max từ values, gradient teal (`--primary`), góc dưới-trái, `calculable`.
`series:[{ type:"map", map: echartsName, nameProperty:"key", roam:false, data }]`. Tooltip name+giá trị
format.

## Register / route / đăng ký viz

`register.ts`: thêm `MapChart` (echarts/charts) + `VisualMapComponent` (echarts/components).
`ChartRenderer`: `if (vizType==="map") return <MapChart columns rows params/>`. `charts.ts`: `VizType +=
"map"` + `ChartParams.map?: { mapKey, region?, metric? }`. `chartTypes.ts`: entry Map (icon `Map`).

## Builder (`ChartExplore.tsx`)

Block `{viz==="map" && (...)}`: Select **Map** (từ `MAPS`), Select **Region** (columns), Select **Metric**
(metrics). State `mapCfg = { mapKey:"us_states", region?, metric? }`. specForRun nhánh map: `{ metrics:
[metric?], dimensions: [region?], columns:[], ... }`. paramsForSave: `{ spec, map: mapCfg }`. Hydrate: `else
if (c.viz_type==="map") setMapCfg(p.map ?? {mapKey:"us_states"})`. Guard run/save: thiếu region/metric →
toast. Preview: map qua ChartRenderer sẵn (không phải table/pivot).

## Test

`map.test`: `buildMapOption` ra `series[0].data` key chuẩn hoá (nh→NH) + visualMap min/max; NaN/region rỗng
bị bỏ; `map === echartsName`. Registry normalize từng map. (registerMap/load verify live — jsdom không render
map.) Live (chart throwaway, 0 console error): US "Revenue by state"; đổi sang World + accounts.country; đổi
sang Vietnam (vẽ outline tỉnh dù chưa data); save vào card dashboard → render + legend. Xoá throwaway. Sweep
xanh. Follow-up sau merge: trỏ "Revenue by state" trong seed sang US map thật.

## File

Tạo: `scripts/build_map_geojson.py`; `echarts/maps/registry.ts` + 3 `*.geo.json` (sinh); `MapChart.tsx`;
`echarts/options/map.ts` + test. Sửa: `register.ts`, `charts.ts`, `chartTypes.ts`, `ChartRenderer.tsx`,
`ChartExplore.tsx`.

## Ngoài phạm vi

Không backend; không point/bubble/heat map; không pan/zoom (`roam:false`) ở MVP; không drill từng vùng; map
ngoài 3 cái đã ship (registry cho phép thêm dễ). Sweep + git identity + footer Co-Authored-By; stage path
tường minh.
