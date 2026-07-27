# Chart Studio A2 — Viz Registry & Params v2 — Đặc tả thiết kế

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-28
**Sub-project:** A2 · Viz registry (nửa sau của A · Chart Studio)
**Spec cha:** `docs/vi/specs/2026-07-27-chart-studio-design.md`
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md`

## Bối cảnh

A1 (PR #344, kèm #345) đã làm xong phần bề mặt của studio: hành vi chạy với công
tắc Auto, các trạng thái preview có nghĩa, và thanh công cụ. Layer 2 trong spec
cha — mô hình cấu hình — vẫn còn nguyên.

`ChartExplore.tsx` vẫn là god-component 1113 dòng: 40 `useState` và cùng một
if-ladder theo viz lặp ở sáu chỗ (`specForRun`, `paramsForSave`, phần validate
trong `run()`, JSX tab Data, effect hydration, `resetControls`). Thêm hoặc sửa một
loại chart là phải sửa cả sáu.

A2 tách phần đó thành một viz registry và thu nhỏ shell.

## Mục tiêu

Mỗi visualization trở thành một module độc lập, khai báo đủ mọi thứ shell cần.
`ChartExplore` chỉ còn giữ state cấp shell và đọc registry — không nơi nào khác
biết chi tiết của từng viz. Params lưu trong DB chuyển sang shape chuẩn hoá,
phẳng và có namespace (v2).

## Phạm vi

**Trong phạm vi**

- Interface `VizDescriptor`, registry, và 14 module descriptor phủ hết 19 viz type.
- Tách shell: `VizSwitcher`, `StudioPanel`, `PreviewPane`, `useChartRun`.
- `ChartParams` v2: settings phẳng có namespace + query spec tách riêng.
- Adapter `toRenderParams()` để `ChartRenderer` và dashboard giữ nguyên shape cũ.
- Migration `0099` backfill `analytics.charts.params` sang v2.

**Ngoài phạm vi**

- Phần render: `ChartRenderer`, `echarts/options/*`, `PivotTable`, `ChartTable`
  không đụng tới. Chuẩn hoá chúng là sub-project B.
- Validate params ở backend — sub-project C. Backend vẫn coi `params` là JSONB đục.
- Thêm loại chart mới, và mọi thay đổi về hình thức chart.

## Tham chiếu: Metabase và Superset thực sự làm gì

Cả hai được đọc trực tiếp (hai repo nằm cạnh repo này) chứ không dựa vào trí nhớ,
vì bản phác registry trong spec cha chưa từng được đối chiếu với chúng.

| Vấn đề | Metabase | Superset |
|---|---|---|
| Registry | `Map<display, Visualization>` + `registerVisualization` (`frontend/src/metabase/visualizations/index.ts`) | registry `ChartPlugin` |
| Module mỗi viz | `visualizations/Funnel/` | `plugins/plugin-chart-echarts/src/BoxPlot/` |
| Dựng query | ở tầng question/MBQL | `buildQuery.ts` mỗi plugin |
| Biến đổi để render | component viz | `transformProps.ts` mỗi plugin |
| Control | định nghĩa khai báo, section là **thuộc tính của từng setting** (`getSection: () => "Data"` — `Funnel.tsx:86`) | `controlPanel.ts` khai báo: `controlPanelSections` nhãn Query / Chart Options, ghép từ `sharedControls` |
| Cấu hình lưu trữ | `visualization_settings`: **phẳng, có namespace** (`funnel.dimension`, `graph.dimensions`, `column_settings`), tách khỏi `dataset_query` | `form_data`: **phẳng** |

Ba kết luận dẫn tới các quyết định bên dưới: registry-các-descriptor là phần lõi
đúng (cả hai đồng ý); tách Data/Display thuộc về từng setting chứ không thuộc ranh
giới module; và settings lưu trữ nên phẳng + có namespace, không lồng theo viz.

## Quyết định thiết kế

1. **Một descriptor cho mỗi họ viz, đặt trong registry.** Như spec cha đề xuất và
   cả hai tham chiếu xác nhận.
2. **Control là một danh sách, mỗi mục gắn nhãn section** — không phải hai
   component cố định `DataControls` / `DisplayControls`. Dời một control sang tab
   khác, hay thêm tab thứ ba sau này, chỉ sửa một mục thay vì 14 module.
3. **Params v2 phẳng và có namespace** (`cartesian.xAxis`, `pivot.rows`,
   `column_settings`), query spec để riêng. Cái này mua được thứ shape lồng không
   có: đổi bar→line giữ nguyên trục và metric vì cùng đọc `cartesian.*`;
   `vizOverride` của từng card dashboard merge theo khoá; thêm setting mới không
   bao giờ cần thêm migration.
4. **Có kiểu trong bộ nhớ, phẳng khi lưu.** Mỗi descriptor khai tiền tố và một
   codec (`fromSettings` / `toSettings`), nên descriptor vẫn làm việc với object
   `Cfg` có kiểu; chỉ lưu trữ là phẳng.
5. **Control vẫn là JSX, một cách có chủ đích.** Framework settings khai báo của
   Metabase (registry widget, đồ thị phụ thuộc `getValue` / `readDependencies`,
   `getHidden`) dài hàng nghìn dòng. Với 19 visualization nội bộ, chi phí lớn hơn
   lợi ích ở thời điểm này. Cái giá được chấp nhận có ý thức: logic default không
   dùng chung được với editor cấu hình card của dashboard, phần đó vẫn viết tay.
6. **Renderer được cách ly bằng adapter**, nên sub-project này không thể làm hỏng
   phần render hay dashboard (xem bên dưới).

Cố ý không sao chép: đồ thị phụ thuộc trong settings của Metabase (quyết định 5),
và `isSensible` / `checkRenderable` (gợi ý loại viz phù hợp với tập kết quả). Cái
sau khá rẻ và đáng làm sau, nhưng không thuộc A2.

## Kiến trúc

### Descriptor

```ts
interface VizDescriptor<Cfg> {
  key: VizType;
  meta: ChartTypeMeta;                       // tên, icon, mô tả (chartTypes.ts)
  prefix: string;                            // namespace settings, ví dụ "cartesian"
  defaultConfig: Cfg;

  fromParams(params: AnyChartParams): Cfg;   // đọc v2, và fallback sang v1
  toSettings(cfg: Cfg): VizSettings;         // ghi các khoá `<prefix>.*` của chính nó
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;
  requiredFields(cfg: Cfg): string[];        // "X-axis", "a metric" — chuyển từ requiredFields.ts

  controls: Array<{
    id: string;
    section: "data" | "display";
    render: FC<VizControlProps<Cfg>>;
  }>;
}
```

`VizControlProps<Cfg>` mang theo `config`, `onChange(patch)`, `columns` /
`metrics` của dataset, và bộ chọn temporal unit dùng chung. `SharedQueryState`
giữ phần query dùng chung mọi viz: filter, row limit, temporal unit.

`vizRegistry: Record<VizType, VizDescriptor>` là chỗ tra cứu duy nhất của shell.
Module nằm ở `components/chart/viz/`: `cartesian.tsx` phủ sáu loại cartesian (dùng
chung một config và một namespace, giống `graph.*` bên Metabase), mười ba loại còn
lại mỗi loại một file.

Descriptor chỉ đọc và ghi **namespace của chính nó**: `toSettings` trả về các khoá
`<prefix>.*`, shell merge chúng lên map settings đang giữ. Các khoá dùng chung
(`column_settings`, `conditional_formatting`) thuộc về shell, nên không descriptor
nào ghi đè settings của descriptor khác khi đổi loại viz.

Descriptor chỉ sở hữu phần cấu hình. Phần render vẫn ở `ChartRenderer` và
`echarts/options/*`.

### Shell

`ChartExplore` chỉ giữ state cấp shell: `datasetId`, `viz`, `saveName`, `autoRun`,
`shared` (filter / row limit / temporal unit), và một map `settings` đục. Đổi viz
thì giữ nguyên map, để `defaultConfig` của descriptor mới lấp chỗ thiếu.

Các đơn vị tách ra:

| Đơn vị | Nhiệm vụ |
|---|---|
| `VizSwitcher` | đổi loại viz (gallery) |
| `StudioPanel` | chọn dataset + hai tab Data/Display, vẽ control của descriptor theo section |
| `PreviewPane` | chart hoặc một trong các trạng thái preview của A1 |
| `useChartRun` | `run()` + auto-run + quy tắc đóng băng khi stale từ #345 |

`StudioToolbar` và `ChartPreviewStates` đã có sẵn từ A1.

Cả sáu if-ladder biến mất: `specForRun` → `buildSpec`, `paramsForSave` →
`toSettings`, validate trong `run()` → `requiredFields`, JSX tab Data →
`controls`, hydration → `fromSettings`, `resetControls` → `defaultConfig`.

### Params v2

```ts
interface ChartParams {
  version: 2;
  spec: QuerySpec;                    // query — giữ nguyên shape, vẫn ở top level
  settings: Record<string, unknown>;  // phẳng, có namespace
}
```

`spec` cố ý để ở top level: dashboard bơm filter tham số vào `params.spec`, nên
`dashboard/params.ts` không phải sửa.

Namespace:

| Tiền tố | Viz type | Khoá ví dụ |
|---|---|---|
| `cartesian.` | bar, row, line, area, scatter, combo | `cartesian.xAxis`, `cartesian.yMetrics`, `cartesian.series`, `cartesian.stacked`, `cartesian.seriesColors` |
| `pie.` | pie | `pie.dimension`, `pie.metric`, `pie.donut`, `pie.sliceColors` |
| `pivot.` | pivot_table | `pivot.rows`, `pivot.cols`, `pivot.metrics`, `pivot.showRowTotals`, `pivot.collapsedRows`, `pivot.sort` |
| `table.` | table | `table.columns`, `table.metrics`, `table.state` |
| `object.` | object | `object.columns` |
| `map.` | map | `map.key`, `map.region`, `map.metric` |
| `scalar.` `progress.` `gauge.` `funnel.` `waterfall.` `smartscalar.` `boxplot.` `sankey.` | mỗi loại một | ví dụ `gauge.metric`, `gauge.min`, `gauge.max` |
| *(dùng chung, không tiền tố viz)* | tất cả | `column_settings`, `conditional_formatting` |

Hai chỗ bẩn được dọn theo. Ở v1 có tám field của pivot nằm rời rạc ngay top level
(`rows`, `cols`, `metrics`, `showRowTotals`, `condense`, `collapsedRows`,
`pivotSort`, `showColTotals`) — nay chuyển hết vào `pivot.`. Và `table` / `object`
hiện **không lưu config gì cả**: cột của chúng được suy ngược từ `spec` lúc
hydration (`p.spec?.dimensions` + `p.spec?.columns`), nay lưu tường minh.

### Biên render

```
params v2 ──▶ toRenderParams(viz, params) ──▶ shape v1 ──▶ applyVizOverride(khoá v1) ──▶ ChartRenderer
```

`toRenderParams()` dựng lại shape lồng như hiện nay. `ChartRenderer`,
`PivotTable`, `ChartTable`, `MapChart` không bị đụng, và dashboard chạy y nguyên —
kể cả `vizOverride` của từng card lưu trong JSONB của dashboard, vốn là một object
partial theo shape v1. Áp override *sau* adapter nghĩa là **không override nào cần
migrate**. Adapter và các override sẽ cùng ra đi khi sub-project B làm lại phần
render.

## Migration

Hai lớp, vì mỗi lớp một mình đều hở:

- **Phía đọc** — mọi `fromSettings` chấp nhận v1 (không có trường `version`). Đây
  chính là chuỗi fallback đang có trong effect hydration (`p.cartesian ?? suy từ
  spec`) được chuyển vào descriptor. Chart phục hồi từ bản backup cũ vẫn mở được.
- **Backfill** — `0099_chart_params_v2.sql` viết lại `analytics.charts.params`
  theo `viz_type`, gồm cả row đã soft-delete. `analytics.dashboards` cố ý không
  đụng (xem phần biên render).

## Cách giao hàng

Ba PR tuần tự, mỗi PR đều deploy lên kind và live check, đúng chiến lược
"lúc nào cũng ship được" của spec cha:

| PR | Nội dung | Live check |
|---|---|---|
| 1 | type v2 + adapter + registry + `StudioPanel` + `useChartRun` + họ cartesian. 13 viz còn lại giữ code inline hiện tại sau một bridge tạm. | một chart cartesian mới, và một chart v1 cũ mở lên |
| 2 | `table`, `pivot_table`, `object`, `map` — bốn loại nhiều state nhất — cộng phần bóc tách `VizSwitcher` và `PreviewPane` | chart pivot + table đã lưu |
| 3 | chín viz còn lại; xoá bridge, `ChartSettingsSections` và `requiredFields.ts`; migration `0099` | mỗi viz một chart, cộng một dashboard có card override |

Backfill nằm ở PR 3 vì chỉ khi đó mọi descriptor mới đọc và ghi v2; chạy sớm hơn
sẽ đá nhau với bridge. `VizSwitcher` và `PreviewPane` chỉ là bóc tách cơ học, không
liên quan tới registry, nên đi cùng PR 2 thay vì phình PR 1 — nơi gánh toàn bộ rủi ro.

## Kiểm thử

- **Mỗi descriptor (hàm thuần):** `requiredFields`, `buildSpec`, `toSettings`,
  `fromSettings`.
- **Round-trip:** `params v1 → fromSettings → Cfg → toSettings → v2 →
  toRenderParams → v1`, khẳng định cho từng viz type. Đây là mối nối dễ vỡ nhất
  nên test đủ mọi viz, không lấy mẫu.
- **Shell:** auto-run bắn khi thay đổi liên quan query và Auto bật, không bắn khi
  Auto tắt; thay đổi display-only không bao giờ query; chart đã lưu tự chạy khi
  mở; query stale giữ nguyên kết quả trước đó (quy tắc của #345).
- **Dashboard:** một card có `vizOverride` render giống hệt trước và sau.
- Test chart hiện có phải xanh; không test render nào phải sửa — nếu phải sửa thì
  adapter sai.

## Tiêu chí thành công

- `ChartExplore.tsx` không còn nhánh rẽ theo viz; thêm một loại chart chỉ đụng một
  module descriptor và registry.
- Mọi chart đã lưu, kể cả row v1 và card dashboard có override, render y như trước.
- Đổi qua lại giữa hai viz cùng họ giữ được các setting còn áp dụng được.
- FE CI xanh ở local cho từng PR (lint, typecheck, test, build) cộng live check
  trên kind.

## Rủi ro

- **Params hỏng âm thầm** mới là rủi ro thật, không phải lỗi biên dịch — giảm nhẹ
  bằng test round-trip từng viz, migration hai lớp, và live check trên chart đã lưu
  từ trước khi đổi.
- **Refactor lớn trên một component trung tâm** — giảm nhẹ bằng chia ba PR, bridge
  giữ các viz chưa chuyển ở code hiện tại, và mỗi PR một lần deploy có live check.
- **Settings phẳng mất kiểu tĩnh theo viz** — giảm nhẹ bằng codec: descriptor làm
  việc với object `Cfg` có kiểu, chỉ khi serialize mới phẳng.
