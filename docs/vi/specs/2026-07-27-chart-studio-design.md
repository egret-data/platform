# Chart Studio — Tài liệu thiết kế

**Trạng thái:** thiết kế đã duyệt, chờ lập kế hoạch triển khai
**Ngày:** 2026-07-27
**Sub-project:** A · Chart Studio (thuộc nỗ lực lớn hơn "chart platform")
**Bản canonical (tiếng Anh):** `docs/en/specs/2026-07-27-chart-studio-design.md`

## Bối cảnh

Nền tảng chart có năm lớp: (1) builder shell, (2) config model, (3) rendering
pipeline, (4) cross-cutting, (5) backend. Ta chia thành ba sub-project, mỗi cái
có chu trình riêng thiết kế → kế hoạch → build:

- **A · Chart Studio** — lớp 1 + 2 (tài liệu này).
- **B · Render & Consistency** — lớp 3 + 4 (sau).
- **C · Backend hardening** — lớp 5 (sau).

Sub-project A làm trước vì đây là mặt tiền người dùng chạm vào, và phần refactor
config-model trong đó gắn với một cải tiến **nhìn thấy được** thay vì là một cuộc
viết lại vô hình.

## Mục tiêu

Redesign chart builder (`ChartExplore.tsx`) thành **Chart Studio**: một builder
2-pane sạch, dễ khám phá, với cấu hình của từng loại viz nằm trong một **viz
registry** tách biệt — nhờ đó `ChartExplore` co từ god-component ~1100 dòng xuống
một shell mỏng, và mỗi loại chart có thể sửa / thêm / test độc lập.

## Phạm vi

**Trong scope**
- Bố cục & UX của builder: viz switcher, panel Data/Display, toolbar, preview.
- Hành vi Run: hybrid auto-run có toggle **Auto**.
- Trạng thái preview: empty / loading / error.
- Tách config-model: một viz registry; `ChartExplore` thành shell mỏng.

**Ngoài scope** (để dành cho B/C)
- Chuẩn hoá option render của ECharts (tooltip/legend/trục/màu) — B.
- A11y cho canvas đã render, engine format số, drill/export — B.
  (Studio *dùng lại* các renderer hiện tại nguyên trạng; không đổi chúng.)
- Validate params ở backend và query pipeline — C.
- Thêm loại chart mới.

## Các quyết định thiết kế (chốt trong lúc brainstorm)

1. **Bố cục: Refined 2-pane.** Giữ cấu trúc hiện tại (panel config luôn hiện bên
   trái + preview), làm chuẩn hơn — không theo kiểu drawer viz-first của Metabase,
   cũng không theo 3-pane của Superset. Lý do: config luôn hiện = dễ khám phá nhất
   cho người dùng nội bộ có nhiều loại chart; thay đổi nhỏ nhất, chắc nhất.
2. **Run: Hybrid có toggle Auto.** Mặc định bật: chart tự chạy lại (debounce
   ~500ms) khi một input *liên quan truy vấn* thay đổi (dataset, dimensions,
   metrics, filters, temporal units, row limit). Tắt **Auto** thì quay về bấm-Run
   cho truy vấn nặng (kiểu Superset). Thay đổi chỉ-hiển-thị (sort, màu, nhãn,
   stacking…) vốn đã re-render từ dữ liệu cache không cần query, và giữ nguyên vậy.
3. **Config model: một viz registry.** Mỗi visualization là một module khai báo
   mọi thứ shell cần; shell không chứa kiến thức riêng của viz nào.

## Kiến trúc

### Viz registry (lớp 2)

Một registry duy nhất ánh xạ mỗi `VizType` sang một descriptor:

```ts
interface VizDescriptor<Cfg> {
  key: VizType;
  meta: ChartTypeMeta;                 // tên, icon, mô tả (từ chartTypes.ts)
  defaultConfig: Cfg;                  // config khởi tạo cho chart mới
  DataControls: FC<VizControlProps<Cfg>>;    // tab Data (panel trái)
  DisplayControls: FC<VizControlProps<Cfg>>; // tab Display (panel trái)
  requiredFields(cfg: Cfg): string[];  // nhãn các field bắt buộc còn thiếu ("X-axis", "một metric")
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;  // → truy vấn
  toParams(cfg: Cfg, ...): ChartParams;      // lưu
  fromParams(params: ChartParams): Cfg;      // hydrate
}
```

`VizControlProps<Cfg>` cấp cho descriptor: `config`, một `onChange(patch)`, và các
`columns`/`metrics` của dataset đã resolve, cộng bộ render temporal-unit dùng chung.
`SharedQueryState` giữ phần query xuyên viz (filters, row limit, temporal units).

Các entry của registry đặt ở `components/chart/viz/<key>.tsx` (hoặc gộp lại, ví dụ
`viz/cartesian.tsx` cho sáu loại cartesian dùng chung controls). Các option builder
và renderer hiện có trong `components/chart/echarts` / `ChartRenderer` **giữ
nguyên** — registry chỉ nắm phần *cấu hình*, không nắm phần render.

### Studio shell (lớp 1)

`ChartExplore` trở thành một orchestrator mỏng, ghép từ các unit nhỏ:

- **VizSwitcher** — cái chip mở gallery loại chart.
- **StudioPanel** — dataset picker + tab Data/Display; render `DataControls` /
  `DisplayControls` của descriptor đang active. Không còn thang `if` theo viz.
- **StudioToolbar** — tên chart sửa inline, toggle **Auto**, Run, Save, và menu
  gộp `⋯` (Export / View as table / View SQL).
- **PreviewPane** — render chart qua `ChartRenderer` sẵn có, hoặc một trong ba
  trạng thái dưới; chứa cả fallback "view as table".
- **useChartRun** — hook chạy hybrid (xem Luồng dữ liệu).

Shell chỉ giữ state cấp-shell: `datasetId`, `viz` đang active, `saveName`,
`autoRun`, shared query state, và `config` của descriptor đang active (một object
mờ duy nhất, không phải 20 hook). Đổi viz thì thay `config` bằng `defaultConfig`
của descriptor mới.

### Trạng thái preview

Một module dùng chung `ChartPreviewStates` (empty / loading / error):

- **Empty** (chưa chạy): icon viz + "Set up your \<Viz\>" + `requiredFields(cfg)`
  hiển thị dạng "Add: X-axis, một metric".
- **Loading**: skeleton hình cột.
- **Error**: icon + message + **Retry**.

### Luồng dữ liệu

```
đổi config ──▶ có liên quan query? ──có──▶ (Auto bật?) ──▶ debounce 500ms ──▶ run()
                                    └─không──▶ re-render từ rows cache (không query)
              (Auto tắt) ──▶ đánh dấu "cần run" ──▶ user bấm Run ──▶ run()
run(): buildSpec(config, shared) ──▶ useSpecData ──▶ rows ──▶ ChartRenderer
mở chart đã lưu: fromParams(params) ──▶ config + spec ──▶ tự chạy khi mở
```

Mở một chart đã lưu sẽ hydrate spec của nó và render ngay (không cần bấm Run).

## Thành phần & tính tách biệt

| Unit | Mục đích | Phụ thuộc |
|---|---|---|
| descriptor `viz/<key>` | config, controls, ánh xạ spec/param của một viz | columns/metrics của dataset, kiểu config |
| `vizRegistry` | tra descriptor theo `VizType` | các descriptor |
| `VizSwitcher` | đổi loại viz | meta registry, gallery |
| `StudioPanel` | dataset + Data/Display cho viz active | descriptor active |
| `StudioToolbar` | name, Auto, Run, Save, overflow | state shell |
| `PreviewPane` + `ChartPreviewStates` | render chart hoặc một trạng thái | ChartRenderer (giữ nguyên) |
| `useChartRun` | chạy hybrid auto/manual | useSpecData |

Mỗi descriptor trả lời được: *viz này cần gì, cấu hình ra sao, biến thành
query/params thế nào?* — mà shell không phải đọc nội tại của nó.

## Chiến lược migration

Tăng dần và luôn ship được:

1. Thêm registry + khung shell; đưa **một** họ viz (cartesian) vào descriptor;
   shell điều khiển nó; phần còn lại vẫn inline qua một cầu nối tạm.
2. Đưa các viz còn lại vào descriptor vài cái một lần, xoá nhánh inline của chúng
   trong `ChartExplore` khi xong.
3. Làm luôn phần polish toolbar/Auto-toggle/states cùng bước 1 để mỗi slice thấy
   rõ tốt hơn.

Mỗi slice là một PR nhỏ, deploy và review được, với dataset demo + chart mẫu giữ
lại để xem.

## Kiểm thử

- **Descriptor**: unit-test `requiredFields`, `buildSpec`, `toParams`/`fromParams`
  cho từng viz (hàm thuần).
- **Shell**: test rằng thay đổi liên-quan-query có auto-run khi Auto bật và không
  khi tắt; thay đổi chỉ-hiển-thị không bao giờ bắn query; chart đã lưu tự chạy khi
  mở.
- **States**: empty hiện đúng field thiếu; loading/error render.
- **A11y**: toolbar/controls có nhãn; giữ coverage hiện có xanh.

## Tiêu chí thành công

- `ChartExplore.tsx` không còn thang `if` theo viz; thêm/sửa một loại chart chỉ
  đụng vào descriptor của nó.
- Dựng chart cảm giác sống (Auto bật) nhưng vẫn kiểm soát được (Auto tắt).
- Empty/loading/error luôn có ý nghĩa; chart đã lưu render khi mở.
- Không hồi quy hành vi chart hiện có (render giữ nguyên trong sub-project này).

## Rủi ro

- Tách registry đụng vào một component trung tâm lớn — giảm thiểu bằng migration
  tăng-dần luôn-ship-được và verify live theo từng slice.
- Auto-run có thể bắn truy vấn nặng — giảm thiểu bằng debounce + toggle Auto + chỉ
  bắn khi có thay đổi liên quan query.
