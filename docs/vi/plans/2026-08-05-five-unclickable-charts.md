# Năm chart click vào không phản hồi gì — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-05-five-unclickable-charts.md` là bản chuẩn; mã nguồn, số dòng và các bước
> chạy lệnh nằm ở đó.

**Mục tiêu:** Làm cho funnel, gauge, progress, smartscalar và object phản hồi khi bị click, mỗi cái
theo cách mà hình dạng của chính nó biện minh được — và sửa một comment đang lập luận rằng chúng không
nên phản hồi.

**Kiến trúc:** `ChartRenderer` nhận đúng **một** prop hướng ra ngoài: `onValueClick(ctx: DrillContext,
x, y)`. Mỗi nhánh tự dịch cú click của chart mình thành một `DrillContext` rồi gọi nó. Đơn vị này thêm
năm bộ chuyển đổi. Nó **không** đổi `DrillContext`, dialog drill, hay bộ máy filter — trừ khi Task 1
chứng minh là buộc phải.

---

## Xuất xứ

Một lượt soát cấp mã nguồn tại `1d9b0d30` đã đi qua từng nhánh của `ChartRenderer` và đối chiếu phần
nối click với bản tương ứng của Metabase. Đã nối và đúng: cartesian, pie, waterfall, boxplot, sankey,
map, table, pivot, scalar. Chưa nối:

| nhánh | egret | bản tương ứng của Metabase |
|---|---|---|
| `funnel` `ChartRenderer.tsx:730` | không handler | `FunnelNormal.tsx:197-204` drill một bước |
| `gauge` `ChartRenderer.tsx:709-723` | không handler | `Gauge/GaugeArc.tsx:42-60` drill một dải |
| `progress` `ChartRenderer.tsx:683-697` | không handler | `Progress/Progress.tsx:68-75` |
| `smartscalar` `ChartRenderer.tsx:778-794` | không handler | `SmartScalar/SmartScalar.tsx:88-102` |
| `object` `ChartRenderer.tsx:510-512` | không handler | `ObjectDetail/ObjectDetailBody.tsx:16-35` |

**Chương trình này đã quên đúng cái seam đó mười lần** — năm lần ở lượt trước (waterfall, boxplot,
sankey, map, object), năm lần ở đây. Lý do nó cứ lặp lại mang tính cấu trúc và cần nói thẳng: **test
cấp chart gọi thẳng `buildXOption` và không bao giờ đi qua ranh giới đó**, nên một suite xanh không
phải bằng chứng. Chỉ test ở seam `ChartRenderer` hoặc một cú click thật mới thấy được.

Chế độ `type: "bar"` của `FunnelChart` (`FunnelChart.tsx:242-262`) dựng một option cartesian thật và
**cũng** không truyền handler nào, nên funnel không click được ở **cả hai** chế độ. Phải nối cả hai.

## Một comment lập luận ngược lại, và nó sai

`ChartRenderer.tsx:316-319` viết: *"Cũng không có click handler — gauge vẽ một giá trị đơn, nên không
có phần tử nào trên đó gọi tên một hàng để drill tới; gauge của Metabase cũng không drill."*

Vế **đầu** là một lập luận thật và Task 1 phải trả lời nó. Vế **sau thì sai**: `Gauge.tsx:186-188`
truyền `onVisualizationClick` xuống `GaugeArc`, nơi dựng một `ClickObject` từ dải. Một comment nêu sự
thật đã kiểm mà lại không đúng thì tệ hơn không có comment — nói rõ điều đó khi thay nó, và ghi lại
thứ **thật sự đã đo được**.

---

## Task 1 — quyết định một cú click nghĩa là gì trên chart không có chiều, TRƯỚC khi viết bộ chuyển đổi nào

**Đây là toàn bộ câu hỏi thiết kế, và phải chốt trước.** Ba trong năm (gauge, progress, smartscalar)
vẽ một giá trị tổng hợp đơn. Dưới con trỏ không có giá trị chiều nào, nên "lọc theo thứ tôi vừa click"
không có gì để lọc.

**egret đã có tiền lệ và phải tìm ra rồi đi theo, đừng bịa cái mới.** `ChartRenderer.tsx:665-681` nối
`scalar` — cũng là một giá trị tổng hợp đơn — bằng cách truyền thẳng `onValueClick` vào `BigNumber`.
Phải xác định, bằng cách đọc: `BigNumber` phát ra `DrillContext` hình gì (`dims: []` rỗng? hay thứ
khác?), và **đầu nhận làm gì với nó** — `PreviewPane.tsx:133-140` gọi prop đó là `onCellDrill`, và
`DashboardCardView` là bên gọi còn lại. **Dialog drill có xử lý được danh sách dims rỗng không, và nó
hiện ra cái gì?** Đọc mà còn mơ hồ thì chạy thử.

Rồi: **drill của gauge Metabase thật sự LÀM GÌ?** Không phải "nó có drill không" — lượt soát đã chốt.
Mà click vào một dải thì *sinh ra* cái gì: một filter theo khoảng của dải, hay một khung xem bản ghi
gốc không thêm filter nào? Đọc `GaugeArc.tsx:42-60` và `ClickObject` nó dựng, và lái bản chạy thật ở
`http://localhost:3033` nếu nguồn còn mơ hồ. Làm y vậy với `Progress` và `SmartScalar`.

**Báo cáo quyết định trước khi cài.** Với từng chart trong năm, nêu cú click phát ra cái gì và vì sao.
**Nếu câu trả lời cho gauge/progress là "đúng cái drill không-chiều mà scalar đang làm" thì nói vậy —
đó là câu trả lời tốt, và nó khiến ba trong năm gần như miễn phí.** Còn nếu cần một filter theo khoảng
trên dải, đó là một hình dạng `DrillContext` mới và nó đổi kích cỡ đơn vị này; nói ra và **dừng lại
chờ quyết định**, đừng lặng lẽ phình phạm vi.

**Hình dạng drill của từng chart trong chương trình này đều đã được lập luận riêng, nên đọc trước khi
quyết**: cạnh sankey lọc **cả hai** cột; nút sankey chọn đích-hay-nguồn tuỳ có luồng chảy vào nó
không; boxplot đọc `dataIndex` vì phần tử boxplot không có `name` dùng được; map lọc giá trị chiều
**thô**, không bao giờ lọc khoá geojson đã chuẩn hoá; waterfall **từ chối** trên cột Total.

**Một chart nên từ chối cú click là kết cục hợp lệ** — cột Total của waterfall là tiền lệ. Nếu gauge
thuộc loại đó thì nói vậy kèm lý do, và sửa comment để nó nêu lý do **thật**.

## Task 2 — funnel, cả hai chế độ

Bước của funnel đến từ một chiều, nên đây là drill theo chiều thông thường và phải trông giống những
cái đã có.

Viết test đỏ **ở seam** — cấp `ChartRenderer`, không phải cấp builder; test builder không thể thấy lỗi
này, và đó chính là lý do nó lọt ra.

`funnelSteps` vốn khoá theo chuỗi chiều thô (`funnel.ts:54,57`) — **drill từ giá trị thô, không phải
nhãn**. Codebase này đã có bảy lỗi vì khoá theo chuỗi hiển thị, hai lỗi gần nhất chỉ chết vì mutation.
Mutate tên-hiển-thị → tên-thô và xem nó chết. **Fixture hai bước không thấy được lỗi này** — dùng ba
bước với bước đã đổi tên nằm giữa.

Chế độ `type: "bar"` dựng option cartesian, nên kiểm xem có dùng lại được bộ chuyển đổi cartesian
không thay vì viết cái thứ hai.

## Task 3 — object

Xác định `ObjectDetailBody.tsx:16-35` drill theo cái gì — một trường, một giá trị, hay cả hàng. Rồi
test đỏ ở seam, rồi bộ chuyển đổi.

## Task 4 — gauge, progress, smartscalar

Cài theo đúng thứ Task 1 đã quyết. Nếu đó là "dùng lại đường của scalar" thì task này là ba bộ chuyển
đổi nhỏ, và rủi ro chính nằm ở cái comment.

**Thay comment ở `ChartRenderer.tsx:316-319`** bằng thứ đã đo được. Nếu một chart từ chối click một
cách chính đáng, comment phải nói vì sao, và lý do phải là của chính nó — không phải một khẳng định về
Metabase mà người đọc sau này lại phải đi kiểm lại.

## Task 5 — bịt seam để không có lần thứ mười một

Mười lần trượt ở cùng một seam là vấn đề cấu trúc, không phải mười lần đãng trí.

Thêm một test **liệt kê mọi `vizType`** và khẳng định mỗi cái hoặc có đường click, hoặc nằm trong một
danh sách từ chối **tường minh và có ghi lý do**. Một chart mới sẽ làm test này đỏ cho tới khi có người
ra quyết định về nó.

**Kiểm test đó có đỏ được không** — gỡ dây của một nhánh và xác nhận nó đỏ. Không đỏ được thì nó vô
giá trị; chương trình này đã hai lần ship một suite xanh phủ lên nhánh mã đã chết.

## Task 6 — đối chiếu live và mở PR

**Click cả năm cái trên dashboard.** Drill là tương tác của **card trên dashboard** — #397 đã xác lập
rằng click vào một điểm trong preview của trình sửa chart không mở gì cả, và xác lập bằng đối chứng
(chuyển chart về thang mà drill chắc chắn chạy, bấm cũng không ra gì). Đừng lặp lại: dùng card trên
dashboard.

**Kiểm filter mỗi cái sinh ra có ĐÚNG không**, không chỉ kiểm là có dialog mở ra. Phép kiểm giá trị
nhất chương trình này tìm được là **đối chiếu số**: tổng bản ghi mà drill trả về phải khớp giá trị vừa
bị click. Đó là cách bắt được lỗi lệch một kỳ trên pivot mà không có thông báo nào.

---

## Cố ý nằm ngoài phạm vi

Vòng focus 2.58:1 ở theme sáng — #412 đã xác định nó là **cấp sản phẩm**: 34 chỗ gọi trên 26 tệp, chạm
tới notebook, SQL Lab và control plane, chỉ 5 tệp nằm trong `components/chart/`. Cần PR riêng.

Cũng ngoài phạm vi: bộ ba `view_as`/`link_text`/`link_url` chết trên 17/18 loại viz; bốn mutation sống
sót; tiêu đề trục trái của combo đọc tên series đã ẩn sau khi click legend (tìm được ở #412); và các
khoảng cách phía Metabase.

## Tự soát

- **Task 1 là điều tra và kết thúc bằng một quyết định, không phải một diff.** Ba trong năm chart
  không có chiều nào dưới con trỏ, và quyết sai nghĩa là hoặc một drill lọc theo hư không, hoặc một
  hình dạng `DrillContext` mới lọt vào mà không ai review.
- **Từ chối là kết cục hợp lệ cho bất kỳ cái nào trong năm.** Waterfall đã từ chối trên cột Total.
  Thứ **không** chấp nhận được là trạng thái hiện tại: không handler, kèm một comment biện minh bằng
  một khẳng định sai.
- **Task 5 mới là thứ hoàn vốn.** Không có nó thì seam này sẽ bị trượt lần nữa; cùng một lỗi đã ship
  mười lần và mọi lần sửa trước đều theo từng chart.
- **Chỗ yếu đã biết:** kế hoạch giả định `DrillContext` diễn đạt được mọi thứ năm chart cần. Nếu gauge
  hay smartscalar muốn một filter theo khoảng (biên của một dải, hay một kỳ so sánh), đó là hình dạng
  mà `dims: [{col, value, unit?}]` có thể không mang nổi. Chỗ đó phải lộ ra ở Task 1 — cho nó lộ ra ở
  đấy, đừng lặng lẽ bẻ cong kiểu dữ liệu.

---

## Kết quả — quyết định của Task 1, và những chỗ kế hoạch này sai

Ghi ở đây chứ không chỉ trong PR, vì tiền đề của chính kế hoạch hoá ra sai ở một chỗ và người đọc
file này sau không nên thừa hưởng nó.

### Quyết định, theo từng chart

Mọi thứ dưới đây được chốt bằng cách đọc mã nguồn Metabase trước khi viết bất kỳ bộ chuyển đổi nào.
**Không cần hình dạng `DrillContext` mới ở đâu cả** — chỗ yếu đã biết của kế hoạch không xảy ra.

| chart | phát ra | vì sao |
|---|---|---|
| funnel (cả hai chế độ) | `{dims: [{col: dimension, value: tên bước THÔ, unit?}]}` | `components/FunnelNormal.tsx:183-193` gửi `dimensions: [{column: dimCol, value: giá trị bước thô}]`; chế độ bar tới cùng một filter qua `funnelToBarTransform` + click object của cartesian. |
| object | `{dims: [{col: field, value: giá trị trường THÔ}]}`, chỉ ở ô giá trị | `components/ObjectDetail/ObjectDetailsTable.tsx:209-238` chỉ gắn `clicked` cho nửa giá trị; nửa nhãn không có gì và trơ. Rơi vào quick-filter, `column = value`. |
| gauge | `{dims: []}` | `Gauge/GaugeArc.tsx:47-61` dựng `{value: segment.min, column: cols[0], settings, event}` **không có `dimensions`**. `underlying_records.cljc:95-105,183-188` rồi duyệt một seq dimensions rỗng — **không thêm filter nào**. quick-filter/automatic-insights/column-filter đều từ chối. **Một dải gauge KHÔNG phải filter theo khoảng.** |
| progress | `{dims: []}` | `Progress/Progress.tsx:72-77`, cùng một object, handler đặt trên cả thanh — không có hành vi "click ở 60% của thanh" nào để sao chép. |
| smartscalar | `{dims: [{col: dimension, value: kỳ MỚI NHẤT, unit?}]}` | **Xem phần sửa bên dưới.** |

### Sửa 1 — trend không phải chart không-chiều

Kế hoạch này viết "Ba trong năm (gauge, progress, smartscalar) vẽ một giá trị tổng hợp đơn. Dưới con
trỏ không có giá trị chiều nào." Đúng với gauge và progress, và **sai với trend**. Trend có nhóm —
dimension + metric — và con số nó vẽ là hàng mới nhất của nhóm đó. `SmartScalar/compute.ts:311-325`
nói rõ điều này, gửi `dimensions: [{value: rows[latestRowIndex][dimensionColIndex], column:
cols[dimensionColIndex]}]`.

Vậy chỉ **hai** trong năm là không-chiều. Trend drill tới kỳ mới nhất, dưới dạng khoảng nửa mở khi
chiều được gom nhóm. Phần so sánh kỳ trước cố ý **không** là mục tiêu click: không file nào dưới
`PreviousValueComparison/` của Metabase có `onClick`, và phần so sánh là một số liệu dẫn xuất chứ
không phải một kỳ riêng.

### Sửa 2 — các tham chiếu file ở "Nó đến từ đâu" đã trôi

`FunnelNormal.tsx` và `ObjectDetailBody.tsx` nằm dưới `visualizations/**components**/`, không phải
`visualizations/visualizations/`. `ObjectDetailBody.tsx` chỉ là lớp chuyển tiếp — click object được
dựng ở `ObjectDetailsTable.tsx`. Của SmartScalar dựng ở `SmartScalar/compute.ts`, không phải
`SmartScalar.tsx`.

### Sửa 3 — một chỗ cố ý khác Metabase, ở funnel

**Bước đầu tiên của funnel Metabase không click được.** `FunnelNormal.tsx:140` dựng click infos từ
`funnelSteps.slice(1)` và treo handler lên phần dải, thứ mà bước đầu không có. Đó là hệ quả kỹ thuật
của việc bước đầu không có dải để vẽ, không phải một quyết định về ý nghĩa của bước — nó là một hàng
kết quả có giá trị chiều như mọi bước khác, và nó giữ con số mà mọi bước sau là phần trăm của.
egret cho click được. (File này đã có tiền lệ: nó vốn từ chối tái tạo lỗi lệch spacer cột đầu của
Metabase.)

### Một lỗi thứ hai, do drill tìm ra chứ không phải kế hoạch

Bộ lọc `valid` của `SmartScalarChart` hỏi `Number.isFinite(Number(r[mi]))`. `Number(null)` và
`Number("")` đều là `0`, và `0` là hữu hạn — nên một kỳ cuối không có dữ liệu vẫn được coi là kỳ
thật: thẻ đọc **0**, tự gán nhãn kỳ đó, và ghi "▼ 100.00% vs previous month". Tìm ra vì drill mới
trỏ vào đúng kỳ rỗng ấy. Đã sửa, kèm test riêng; bản sửa độc lập với drill và thay đổi thứ chart
**vẽ ra**.

### Task 5, như đã dựng

`__tests__/ChartRenderer.clickCoverage.test.tsx` render mọi `VizType` hai lần — có và không có
`onValueClick` — và đòi lần đầu phải sinh nhiều khả năng click hơn hẳn lần sau. Một khẳng định tổng
quát, không phải mười chín khẳng định phải bảo trì tay. `REFUSES_CLICK` là lối thoát và cố ý rỗng
hôm nay; thứ gì có trong đó bị kiểm cả hai chiều. Đã chứng minh test có thể đỏ bằng cách gỡ dây từng
nhánh trong **mười bốn** nhánh: cả mười bốn đều đỏ.
