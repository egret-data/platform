# Thiết kế — thu hẹp khác biệt chart giữa egret và Metabase

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-08-01
**Phạm vi:** sửa cái gì hỏng và thu hẹp khoảng cách UX rõ ràng; giữ những chỗ egret cố ý khác
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-08-01-chart-parity-design.md`

## Vì sao có tài liệu này

Chương trình thiết kế lại 19 chart đã hoàn tất mọi loại chart, nhưng các lần đối chiếu trước đều thực
hiện trên **dữ liệu khác nhau**: dataset demo của egret là dữ liệu sinh tổng hợp, còn dashboard
Metabase chạy trên Sample Database. Vì bản thân con số đã khác, không thể quy khác biệt về module
chart được.

Thiết kế này dựa trên một harness so-một-đối-một dựng ngày 2026-08-01. **Kết quả truy vấn** của
Metabase được lấy qua chính `/api/dataset` của nó rồi vật chất hoá trong egret thành chín dataset ảo
dựng từ `VALUES` của Trino, nên hai bên nhận đúng cùng một bảng số. Mỗi bên có một dashboard tên
**ZZ Chart Parity** chứa cả 19 loại chart. Đã kiểm chứng trùng khít từng hàng trước khi so
(ví dụ `2026-01 → 205 đơn / 11.094,70` ở cả hai).

Mọi điều dưới đây đều quan sát trên harness đó rồi truy ngược về code hai phía.

## Nguyên nhân gốc

Đã thống kê 26 khác biệt. Gần như toàn bộ là hệ quả của **một thiếu sót kiến trúc**.

Mọi quyết định về nhãn — xoay hay không, bao nhiêu tick, cắt chữ ở đâu, gauge vẽ mấy vạch, pivot còn
chỗ cho thân bảng không — đều là quyết định **phụ thuộc kích thước**. Ở egret, các option builder
không nhìn thấy kích thước của chính mình. Code tự nói ra điều đó tại
`frontend/src/components/chart/echarts/options/cartesian.ts:874`:

> the thing that decides whether labels fit is the axis WIDTH — which the option builder never sees

Nên nó đoán theo **số lượng** nhãn:

```ts
const rotate = !horizontal && longest > 10 && categories.length > 6 ? 30 : 0;
```

rồi giao phần còn lại cho `interval: "auto"` của ECharts — cơ chế âm thầm bỏ bớt nhãn bị chồng. Trên
card dashboard rộng một phần ba với bốn category ngắn, phép đoán từ chối xoay, ECharts bỏ ba trong
bốn nhãn, và trục đọc ra như bị mất dữ liệu.

Metabase không đoán: nó chạy một tầng layout tính `boundaryWidth`, đo chữ đã render, rồi mới dựng
option.

### egret đã có sẵn phần lớn mảnh ghép

| Mảnh | Trạng thái ở egret |
|---|---|
| Đo chữ bằng canvas | `components/chart/measure-text.ts` — port từ Metabase, gắn vào document để `rem` phân giải đúng, có `measurer` tiêm được cho test |
| Option là hàm của kích thước đo được | Đang chạy thật — nhưng **chỉ pie** (`ChartRenderer.tsx`, `PieChart`) |
| Gộp sự kiện resize | `EChartsRenderer` đã coalesce bằng rAF, kèm bẫy hiệu năng đã ghi lại |
| Panel trục nhóm X/Y kèm trường xoay | `settings/ChartAxesSection.tsx` đã có `xLabelRotation?: 45 \| 90`; waterfall, box plot và sankey đang dùng |

Vì vậy công việc chủ yếu là **mở rộng một khuôn mẫu đã có và đã được kiểm chứng**, không phải dựng
máy móc mới.

## Không làm

Cố ý nằm ngoài phạm vi, theo đúng định hướng đã chốt "sửa lỗi và thu hẹp khoảng cách UX rõ ràng, giữ
nét riêng của egret":

- Parity tuyệt đối từng setting. Những thứ Metabase có mà egret không — `graph.split_panels`,
  `graph.tooltip_columns`, pin/heat map, `table.pivot`, trend so sánh tới ba mốc — vẫn để ngoài.
- Gộp `ChartAxesSection` vào `AxesOptions` của cartesian. Doc comment của file đó lập luận đúng:
  waterfall và box plot không honour cả x-scale select lẫn y-axis split, gộp vào sẽ hiện hai setting
  không làm gì. Chỉ dùng chung **cách trình bày** (nhóm X-axis / Y-axis), không dùng chung bộ trường.
- Những chỗ egret đang hơn: màu và sắp xếp từng series ngay trong panel, tab Colors, progressive
  disclosure, empty state nói rõ thiếu trường nào, và pivot/trend/gauge chạy được trên SQL thô trong
  khi Metabase từ chối.

## Thiết kế

### Hạng mục 1 — option builder biết kích thước (hạ tầng)

Tổng quát hoá khuôn mẫu `PieChart` đang dùng: `option` của chart trở thành **hàm của** `EChartsSize`
đo được, thay vì một giá trị. Mở rộng cho họ cartesian, gauge và sankey.

**Đã đính chính 2026-08-01 khi lập kế hoạch:** bản nháp trước của mục này có kể funnel là builder thứ
tư. Không phải — `options/funnel.ts` chỉ export `funnelSteps` và `stepOpacity`, **không có** option
builder nào, vì funnel đã là component React vẽ hình thang từ PR #378 và không bao giờ đi qua
`EChartsRenderer`. Nó tự đo container bằng ref, và việc đó thuộc về hạng mục 5 cùng với phần sửa cắt
nhãn. Nhánh funnel duy nhất *có* dùng ECharts là `StepBars`, vẽ `funnel.type: "bar"` qua
`buildCartesianOption` nên đã được phần cartesian bao phủ.

Tiền lệ pie cũng kèm cảnh báo: dựng option phụ thuộc kích thước có thể chạy một lượt `measureText`
cho từng phần tử, nên **định danh của hàm option phải ổn định** qua các lần re-render không liên
quan. `PieChart` tồn tại như một component riêng chính vì lý do đó. Bản tổng quát hoá phải giữ điều
này — memo theo từng viz, không viết arrow function thẳng trong `ChartRenderer`.

Hạng mục này **không đổi hành vi vẽ**. Nó tồn tại để hạng mục 2, 3 và 5 trở nên nhỏ.

**Kiểm thử.** Khẳng định builder nhận kích thước tường minh sinh ra option khác nhau ở 300px và
1200px. `measureText` cố ý trả về 0 dưới jsdom (xem doc comment: *"Size claims belong in a
browser"*), nên test tiêm measurer giả và khẳng định **quyết định** (`rotate: 45`, `show: false`,
interval được chọn), tuyệt đối không khẳng định pixel.

### Hạng mục 2 — nhãn trục X ordinal

Port bậc thang của Metabase từ `echarts/cartesian/layout/index.ts`:

1. Tự chọn **chỉ áp dụng khi setting còn là mặc định** và scale là ordinal. Người dùng đã chọn tay
   thì tôn trọng tuyệt đối, không bao giờ ghi đè.
2. `dimensionWidth = boundaryWidth / valuesCount`.
3. Kiểm chồng lấn **theo từng cặp kề nhau**, trên nhãn đã đệm để bi quan có chủ ý:
   `leftW/2 + rightW/2 + GAP > dimensionWidth`, hoặc riêng một nhãn vượt `dimensionWidth - GAP/2`.
4. Không chồng → in thẳng đứng toàn bộ.
5. Có chồng → `dimensionWidth >= fontSize × 2,1` cho 45°, không thì `>= fontSize × 1,2` cho 90°, mỗi
   trường hợp chỉ khi chiều cao nhãn sau xoay còn dưới 70% chiều cao chart; không thì **giấu hẳn cả
   trục**.
6. Chọn nhãn nào để đo là bước duy nhất egret **không** chép nguyên. `getXTicksToMeasure`
   (`layout/index.ts:347-373`) rẽ nhánh theo kiểu trục, mà trục ordinal chính là trục *category*:
   Metabase đo **toàn bộ** nhãn category, chỉ lùi về 50 nhãn **dài nhất theo số ký tự** khi
   `dimensionWidth <= fontSize` — tức dưới bậc thang mà không gì vừa nữa. Lấy mẫu cách đều
   (`getEvenlySpacedIndices`) là nhánh numeric/time-series, không bao giờ chạy ở đây. Và không chỗ
   nào lấy 50 tick *đầu* cả.

   egret đo nhiều nhất 50: cách đều, **cộng thêm** nhãn dài nhất theo số ký tự. Giới hạn này là của
   egret — Metabase tự tính layout và chỉ đo một lần, còn hàm này chạy lại mỗi lần dựng option — và
   chính phần hợp của hai chiến lược làm cho giới hạn đó an toàn. Chỉ cách đều thì có thể bỏ sót
   nhãn rộng nhất, vì số ký tự chỉ là ước lượng gián tiếp của bề rộng (`WWWW` rộng hơn
   `iiiiiiiiiiii`); chỉ lấy nhãn dài nhất cũng bỏ sót y như vậy theo chiều ngược lại. Gộp cả hai thì
   chắc chắn chạm tới nhãn quyết định chốt chặn chiều cao, phần đệm dưới và cả hai điều kiện chồng
   lấn tính trên từng nhãn.

   Còn lại hai khác biệt, theo hai chiều ngược nhau, và quá 50 nhãn thì cả hai đều có thể xảy ra.
   Một nhãn rộng hơn mọi nhãn đã lấy mẫu *và* không phải nhãn dài nhất theo ký tự, lại rơi đúng vào
   khoảng trống lấy mẫu, thì không bao giờ được đo — egret vẽ thẳng trong khi Metabase xoay. Và vì
   quá 50 nhãn thì các nhãn đã lấy mẫu không còn kề nhau thật, điều kiện theo cặp có thể kích hoạt
   trên một sự kề nhau *bịa ra*: hai nhãn đã lấy mẫu rộng 10 với `dimensionWidth` 15 thì chồng nhau
   khi coi là một cặp, trong khi hàng xóm thật rộng 1 của chúng thì không — egret xoay trong khi
   Metabase vẽ thẳng. Cả hai đều gượng ép, và không cái nào tạo ra một trục in dở dang, vốn là tính
   chất mà hạng mục này sinh ra để bảo đảm.

**Tính chất then chốt: nhãn không bao giờ in một phần.** Hoặc đủ, hoặc xoay, hoặc không có gì.
`interval: "auto"` hiện tại của egret cho ra "một số" — đúng kết cục Metabase cố tình tránh.

Thêm `xLabelRotation?: 45 | 90` vào `CartesianConfig`, đứng cạnh `xAxisTicks?: boolean` đã có, và
cho tab Axes của cartesian đúng select 4 mức mà `ChartAxesSection` đang render:
Hide / Show / Rotate 45° / Rotate 90°.

Metabase gói tất cả vào **một** key (`graph.x_axis.axis_enabled`, còn có mức thứ năm là "Compact").
egret **cố ý không** theo ở đây: waterfall, box plot và sankey đã lưu theo hình dạng hai trường, và
nếu họ cartesian lưu một hình dạng khác cho cùng một điều khiển thì mô hình settings sẽ vỡ ra chỉ để
giống một mô hình ngoại lai. Select chỉ là **phép chiếu UI** trên hai trường đó. Mức "Compact" — rút
gọn nhãn chứ không xoay — là tính năng riêng, nằm ngoài phạm vi.

**Chữa:** bar, row, box plot, waterfall.

### Hạng mục 3 — trục X thời gian và số

Hai lỗi chung một gốc: `xScale` của egret mặc định `"ordinal"` và chỉ đổi được bằng tay, nên cột ngày
bị vẽ trên trục category với nhãn chuỗi thô.

1. Nhận diện cột thời gian và tự chọn scale timeseries, theo cách Metabase quyết định loại trục từ
   **bản chất cột** chứ không từ setting.
2. Port thang interval từ `echarts/cartesian/utils/timeseries.ts`: một danh sách cố định các interval
   ứng viên từ 1 ms tới 100 năm; bắt đầu ở vị trí khớp với đơn vị gom nhóm của dữ liệu; đi lên tới
   khi `expectedTickCount <= maxTicksForChartWidth`; lùi lại một bậc nếu kết quả cho dưới hai tick.
3. `maxTicksForChartWidth` đo **nhãn tệ nhất có thể của đơn vị đó** — qua các ngày đại diện — chứ
   không đo dữ liệu thật, cộng đệm 10px.
4. Đặt tên kỳ. Bucket theo tháng đọc là "January 2026", không phải "2026-01-01".
5. Áp dụng cùng cách chọn interval "đẹp" cho trục X kiểu số.

**Chữa:** nhãn X của line, area, combo; tick `15,69 / 48,93 / 84,14` của scatter; và thẻ trend in
`December 1, 2026` trong khi lẽ ra phải gọi tên kỳ.

### Hạng mục 4 — gán màu

Bảng màu vốn đã giống hệt: `SERIES_PALETTE` của egret chính là tám accent của Metabase, và
`SUMMARIZE_COLOR = "#689735"` đúng bằng `palm[50]` của Metabase. Khác nhau nằm ở **cách gán**.

Thêm `colorsForKeys(names, existing?)` vào `echarts/palette.ts`, theo `ui/colors/charts.ts` của
Metabase:

- Màu suy ra từ **hash tên series/cột**, nên một cột giữ nguyên màu trên mọi card của dashboard.
- Tên ngữ nghĩa thắng hash: `success`/`profit`/`completed`/`active` → xanh success;
  `error`/`failed`/`loss`/`cost`/`cancelled` → đỏ error.
- Quá tám key thì mở rộng hoà sắc thay vì quay vòng `i % 8` — cách hiện tại khiến hai series khác
  nhau trùng màu.
- Bỏ nhánh series đơn tại `options/cartesian.ts:526`
  (`groups.length === 1 ? SUMMARIZE_COLOR : seriesColor(si)`). Đây là lý do tám trong mười chín chart
  của egret cùng một màu xanh olive và dashboard nhìn đơn sắc.

Mọi option builder gọi chung một hàm này.

**Hệ quả đã chấp nhận:** màu mặc định tính lúc render và không lưu, nên mọi chart đã lưu mà chưa tự
đặt màu sẽ đổi diện mạo một lần. Đây là quyết định có chủ ý — phương án migration ghi màu hiện tại
vào `seriesColors` đã bị loại vì làm phình dữ liệu setting chỉ để giữ một diện mạo không ai chọn.

### Hạng mục 5 — cắt nhãn và ba lỗi lẻ

- **Sankey** nhãn node phía đích cụt còn một hai ký tự; **funnel** tên bước cụt còn `A.` `C.` `P.` và
  hàng giá trị bị cắt; **pie** nhãn lát chồng lên nhau trên bánh. Cả ba đều là quyết định kích thước
  nên đi sau hạng mục 1. `echarts/sliceLabel.ts` là phần của pie, sẽ đọc trọn trong hạng mục này.
- **Gauge**: `options/gauge.ts:142` đặt `axisLabel` mà không đặt `splitNumber`, nên ECharts dùng mặc
  định 10 và vẽ 11 nhãn sáu ký tự quanh một cung nhỏ. Suy số vạch từ bán kính đo được.
- **Pivot trong card nhỏ**: chạy đúng trong editor (đã kiểm chứng: lưới đủ 12×4 kèm grand totals,
  khớp Metabase) nhưng ở card dashboard chỉ còn header và hàng grand totals. Cần cuộn trong card thay
  vì rụng mất thân bảng.
- **Row chart**: tiêu đề trục sau khi xoay đè lên nhãn category, và tick trục giá trị dính thành
  `100K200K300K400K`.

### Hạng mục 6 — panel cấu hình

- Nhóm tab Axes của cartesian theo đề mục X-axis / Y-axis, như `ChartAxesSection` đang làm. Chỉ dùng
  chung cách trình bày; bộ trường vẫn tách, vì lý do nêu ở phần **Không làm**.
- Phơi `showTotal` của pie. Setting có thật và mặc định bật cho pie nhiều vòng, tắt cho một vòng —
  một lựa chọn có chủ ý, đã ghi chú — nhưng không có control trong panel, nên donut một vòng không
  bao giờ hiện được tổng ở tâm như Metabase mặc định hiện.
- Thay select "Percentages" đơn lẻ của pie bằng các lựa chọn tường minh khớp Metabase:
  Off / In legend / On the chart / Both. Control gộp hiện tại là lý do nhãn lát và legend cùng vẽ.
- Ẩn những setting không áp dụng, như `getHidden` của Metabase — stacking không nên hiện khi chỉ có
  một series.
- Sửa ô số hiện `2,5` thay vì `2.5` ở "Group slices below".
- Thêm số hàng vào chân bảng, như "Showing 4 rows" của Metabase.

### Hạng mục 7 — script seed harness

Commit một script dựng lại chín dataset và dashboard 19 chart **ZZ Chart Parity**, để phép đối chiếu
sống sót qua một lần reset database và ai cũng tái lập được.

Script phải mã hoá lại các cạm bẫy đã gặp khi dựng tay:

- Metric của chart phải là **metric đã khai báo trên dataset** (`POST .../datasets/{id}/metrics` với
  biểu thức kiểu `SUM(revenue)`), không phải tên cột thô, nếu không chart trả
  `bad request: unknown metric`.
- Sai key settings thì **hỏng im lặng** thành chart rỗng: pie nhận `pie.dimensions` (mảng, số nhiều),
  waterfall nhận `waterfall.xAxis`, box plot nhận `boxplot.category` + `boxplot.value` trên `columns`
  thô và **không** metric, vì tứ phân vị tính ở client còn hàm tổng hợp sẽ ép mỗi nhóm về một điểm.
- Tạo dataset nhận `source: {kind: "virtual", sql}`.
- PUT dashboard là thay thế toàn phần, cần `name` bên cạnh `cards`, và hình học card dùng
  `grid_col` / `grid_row` / `size_x` / `size_y`.

## Trình tự

Hạng mục 1 chặn 2, 3 và 5. Hạng mục 4, 6, 7 độc lập.

| PR | Hạng mục | Bị chặn bởi |
|---|---|---|
| 1 | Option builder biết kích thước | — |
| 2 | Trục X ordinal | 1 |
| 3 | Trục X thời gian / số | 1 |
| 4 | Gán màu | — |
| 5 | Cắt nhãn + gauge, pivot, row | 1 |
| 6 | Panel cấu hình | — |
| 7 | Script seed harness | — |

## Kiểm thử

Mỗi fix kèm một vitest khẳng định **luật**, không khẳng định pixel đã render:

- Hành vi phụ thuộc kích thước được test bằng cách tiêm measurer giả và khẳng định quyết định builder
  đi tới. jsdom không chạy layout; `measure-text.ts` cố ý báo 0 ở đó để một test không thể vô tình
  trông như đã kiểm chứng việc vừa khít.
- Màu được test bằng: cùng một tên cho ra cùng một accent qua hai lần dựng chart khác nhau, tên ngữ
  nghĩa thắng hash, và series thứ chín không lặp lại màu series đầu.
- Bậc thang trục được test tại các biên: vừa đủ chỗ, cần 45°, cần 90°, và quá cao cho cả hai — để
  nhánh "giấu cả trục" thực sự được chạy chứ không phải được giả định.

Kiểm chứng live chạy trên harness đã seed, đặt cạnh Metabase. Ảnh chụp là bằng chứng của một lần
kiểm tra thủ công, không phải test tự động, và sẽ được mô tả đúng như vậy.

## Rủi ro

- **Chart đã lưu sẽ đổi màu.** Đã chấp nhận và đã quyết; cần nêu rõ trong mô tả PR để không bị đọc
  thành hồi quy.
- **Truyền kích thước vào builder có thể làm giảm hiệu năng.** Tiền lệ pie đã ghi lại đúng cái bẫy
  này. Biện pháp là giữ ranh giới memo theo từng viz, không dựng hàm option inline.
- **Tự chọn scale timeseries sẽ đổi các chart line/area hiện có** đang vẽ trên trục category. Thứ tự
  và khoảng cách có thể xê dịch khi ngày không đều. Cần kiểm trên harness trước khi merge.

## Đính chính các kết luận trước

Ghi lại để danh sách trước không bị trích dẫn nguyên trạng:

1. **"Pie thiếu Show total."** Sai — `cfg.showTotal` có thật và được đặt mặc định có chủ ý. Khoảng
   cách thật là panel không phơi nó ra.
2. **"`pie.slice_threshold` chỉ là hằng số, không phải setting."** Sai — panel có
   "Group slices below … %". Kết luận này đến từ việc đọc code mà không mở UI.
3. **"egret không có xoay nhãn."** Sai — cartesian tự xoay 30°, nhưng chỉ khi nhãn dài nhất vượt mười
   ký tự **và** có hơn sáu category, tức là mù với chiều rộng — thứ thật sự quyết định vấn đề.
4. **Bốn chart ban đầu vẽ trống** trên harness (pie, waterfall, box plot, pivot). Đó là **lỗi đặt sai
   key settings khi dựng phép đối chiếu**, không phải lỗi egret; cả bốn vẽ bình thường khi dùng đúng
   key.
