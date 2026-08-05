# Bốn phát hiện cartesian từ lượt soát — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-04-cartesian-audit-fixes.md` là bản chuẩn; mã nguồn, số dòng và các bước chạy
> lệnh nằm ở đó.

**Mục tiêu:** Đóng bốn phát hiện cartesian từ ba lượt soát: một thang làm trắng chart, một combo vẽ đè
bar lên nhau, ba cấu hình line combo không với tới được, và hai từ vựng Axes khác nhau trong cùng một
sản phẩm.

**Kiến trúc:** Task 1–3 độc lập với nhau. Task 4 làm sau cùng, vì nó sửa đúng tệp mà Task 1 và 3 cũng
sửa.

Ba lượt soát chạy đồng thời tại `1d9b0d30`: một lượt panel-parity lái cả hai app, một lượt cấp mã
nguồn quét năm họ lỗi tái diễn, và một lượt mutation + accessibility. Chỗ nào ghi ĐO ĐƯỢC là đã lái
hoặc đã chạy thật; còn lại là đọc.

---

## Task 1 — thang mà cột không nhận được thì đừng chào ra, và cũng đừng tôn trọng

**ĐO ĐƯỢC (lái thật, `MB · Bar`).** Axes → X axis scale của egret chào `Ordinal / Linear / Timeseries`
**đang bật** trên một cột kiểu chữ. Chọn Timeseries thì vùng vẽ trắng hoàn toàn — không cột, không giá
trị trục y, không nhãn trục x. Metabase, lái song song trên cùng card, **khoá** select đó và chỉ chào
`Ordinal` khi cột x là chuỗi. Hai cú nhấp là mất card, và lưu lại được.

**Bước 1 phải là điều tra, chưa sửa gì.** `resolveXScale(kind, config.xScale, xColumnType)` đã tồn tại
và đã nhận kiểu cột — panel gọi nó ở `:242` để hiện thang ĐÃ GIẢI. Nên một trong hai điều đúng, và phải
tìm ra là điều nào: hoặc `resolveXScale` đã từ chối `timeseries` trên cột chữ, nghĩa là builder không
đi qua nó và chỗ sửa nằm ở builder; hoặc nó không từ chối, và chỗ sửa nằm trong chính `resolveXScale`.

**Sửa mỗi panel là KHÔNG chấp nhận được.** #407 đã dạy đúng bài này với Decimal places: sửa control
không với tới được chart đã lưu sẵn giá trị xấu. Làm gì thì làm, một chart đã lưu `xScale:"timeseries"`
trên cột chữ **phải vẽ ra được**.

Khi khoá các mục không áp dụng được: **cơ chế đã có sẵn trong chính tệp này** — `:444` làm mờ
`Rotate 45°/90°` khi thang không phải ordinal, lý lẽ ở `:428-437`. Theo khuôn đó, đừng bịa khuôn thứ
hai.

## Task 2 — combo có breakout vẽ đè bar lên nhau

**ĐO ĐƯỢC.** `echarts/options/cartesian.ts:868` tra `seriesDisplayFor(cfg.kind, "", si, ...)` — tên
series **cắm cứng chuỗi rỗng**, nên `cfg.seriesTypes[""]` không bao giờ khớp và nó luôn rơi về mặc định
theo vị trí. Dạng thật của từng band được giải mười lăm dòng sau, ở `:883`, theo `groupName(sv)`. Hai
bên bất đồng.

Đo trên combo hai band cùng đặt `bar`: cả hai ra `type:"bar"` **và cùng mang `barWidth:"80%"`**, vì
`grouped` tính ra `false`. ECharts vẽ chúng ở cùng một x và một cột che cột kia. Nhánh không breakout ở
`:897` truyền metric thật và đúng.

**Lưu ý khi sửa:** `groupName` định nghĩa ở `:871`, **sau** dòng hỏng ở `:868` — nên đây là một phép
chuyển chỗ, không chỉ là đổi tham số.

Test phải khẳng định trên **thứ ECharts nhận được**, không phải trên biến `grouped` nội bộ — một test
thò tay vào trong sẽ xanh cả trên bản sửa tính đúng boolean rồi dùng sai.

## Task 3 — combo không với tới ba cấu hình line mà chính nó tôn trọng

**ĐO ĐƯỢC.** `settings/CartesianDisplaySection.tsx:77` chặn các control line cấp chart bằng
`kind === "line" || kind === "area"`, loại combo ra. Nhưng `decorateSeries` áp chúng cho **mọi series
giải ra là line** (`cartesian.ts:1035-1077`).

Đo trên `kind:"combo"` với `missing:"zero"`, `lineShape:"smooth"`, `markers:"off"`: band line trả về
`data:[1,0,3]` (null đã bị viết thành 0), `smooth:true`, `showSymbol:false`. **Cả ba đều có tác dụng;
panel không chào ra cái nào cho combo.**

`shape` và `markers` còn cửa qua `SeriesSettingsPopover` (#408). **`missing` thì không còn cửa nào** —
`LineSeriesStyle` cố ý bỏ nó, lập luận ở `cartesian.ts:1047-1057`, nên control cấp chart là cửa duy
nhất và cửa đó đóng với combo.

Chuyện này không chỉ là hình thức: `missing:"zero"` **viết lại dữ liệu**, nên nó âm thầm đổi tổng
stack, chuẩn hoá và đường xu hướng.

**Cổng nào mới đúng:** "combo là line-ish" là câu trả lời rẻ tiền và nó **sai** khi mọi series đều là
bar. Cổng trung thực là **có series nào giải ra là line không** — thứ mà `seriesDisplayFor` +
`echartsTypeOf` đã trả lời được, và #408 đã import đúng chúng vào một component settings cho đúng loại
câu hỏi này.

## Task 4 — một từ vựng Axes, không phải hai

**ĐO ĐƯỢC (lái thật, sáu chart).** Waterfall và box plot đi qua `settings/ChartAxesSection.tsx` và khớp
Metabase: có switch **"Auto y-axis range"** (`:177`) cổng cho Y min / Y max, và lines/ticks là một
*select*. Bar, line, area, combo, row và scatter đi qua `settings/CartesianDisplaySection.tsx` và hiện
panel khác: **không có Auto y-axis range**, Y min/Y max luôn hiện, lines/ticks là *switch*.

**Cùng key lưu trữ, hai cách trình bày, tuỳ người dùng mở chart nào.** `CartesianDisplaySection.tsx:492`
vốn đã thừa nhận cặp min/max bị trùng; cái cổng thì chưa bao giờ được chép sang.

**Không tốn key lưu trữ nào.** `ChartAxesSection.tsx:149` suy ra công tắc:
`const autoRange = config.yMin == null && config.yMax == null`.

**Ưu tiên dùng lại component hơn là chép markup** — hai bản cài của một control chính là cách chương
trình này từng có bốn setting theo series nằm rải trên ba danh sách song song. Nếu không dùng lại được,
nói rõ cái gì chặn.

**Đừng làm mất phần thêm của panel cartesian:** nó mang những thứ panel waterfall không có (gom theo
từng trục từ #403, select Scale từ #406, phần xoay nhãn). Liệt kê từng bên có gì trước khi gộp, và báo
lại thứ nào phải dời chỗ.

---

## Cố ý nằm ngoài phạm vi

Các phát hiện còn lại của lượt soát, ghi lại để không mất: mô tả aria của pie gọi tên lát chart không
vẽ; `ChartTable` kéo giãn cột chỉ chuột dùng được và header khai "sortable" mà Enter lại kéo thả (bản
sửa đã có ở `pivot/PivotTable.tsx:12-51`); vòng focus theme sáng 2.58:1 so với ngưỡng 3:1; `onPointClick`
vẫn thiếu trên funnel/gauge/progress/smartscalar/object cùng một comment ở `ChartRenderer.tsx:319`
khẳng định một sự thật đã kiểm mà lại sai; bộ ba `view_as`/`link_text`/`link_url` chết trên 17/18 loại
viz; bốn mutation sống sót; và các khoảng cách phía Metabase (Bubble size + khối trục giá trị của
scatter, Comparisons của trend, chế độ nhãn Compact, Unpin from zero, Show total của pie).

## Tự soát

- **Task 1 là task duy nhất có thể phá card đã lưu của người dùng**, nên bước đầu của nó là điều tra
  chứ không phải thay đổi, và phương án sửa-mỗi-panel bị loại ngay từ đầu.
- **Chỗ yếu đã biết:** Task 3 giả định dạng hiển thị đã giải tính được bên trong
  `CartesianDisplaySection`. #408 đã xác lập `seriesDisplayFor` import được từ component settings và
  luật thật sự chi phối `cfg.seriesLine` là `echartsTypeOf(display) === "line"` — nhưng khi đó nó có
  sẵn tên series trong tay, còn panel này thì có thể không. Kiểm trước khi giả định.

---

## Các điểm đã hiệu chỉnh khi thực thi kế hoạch

Ghi lại thay vì lặng lẽ tiếp thu, vì mỗi điểm đều là tiền đề của một bước.

- **Task 1, Bước 1 — nhánh thứ hai đúng, và luật rộng hơn phát hiện.** `resolveXScale` KHÔNG từ chối:
  `if (xScale != null) return xScale` trả nguyên thang đã lưu, nên bản sửa nằm ở đó và mọi caller đều
  hưởng. Nhưng ca đo được (`timeseries` trên cột text) chỉ là một trong bốn — `applyXScale` loại mọi
  điểm có toạ độ không hữu hạn, nên `linear` trên cột text và `timeseries` trên cột SỐ cũng làm trắng
  chart y hệt (`Date.parse("15.69")` là NaN). Vì vậy phép từ chối là một luật khớp kiểu cột tổng quát
  (`xScaleFitsColumn`), cũng chính là luật của Metabase: Timeseries chỉ cho cột thời gian, Linear chỉ
  cho cột số, Ordinal luôn có.

  **Cái giá phải trả, nêu rõ chứ không giấu:** cột `varchar` chứa chuỗi ngày ISO vẫn ép kiểu tốt qua
  `Date.parse` nhưng nay bị từ chối, nên chart như vậy lùi về trục ordinal. Mọi điểm vẫn được vẽ,
  đúng thứ tự. Phương án khác — xét GIÁ TRỊ đã ép kiểu — không chia sẻ được với panel (panel không
  giữ dòng dữ liệu), và sẽ khiến select làm mờ theo một luật còn chart tuân theo luật khác.

- **Task 1, Bước 4 — lựa chọn, và vì sao nó ngược với các dòng xoay nhãn.** Panel hiện thang ĐÃ GIẢI
  ("Ordinal"), không phải chữ đã lưu bị làm mờ. Điều đó suy ra từ việc select vốn đã gắn với
  `resolveXScale`, và in "Timeseries" phía trên một chart đang vẽ category chính là lỗi mà cách gắn
  đó sinh ra để loại bỏ. Khoá đã lưu không bao giờ bị ghi đè, nên trỏ chart sang cột ngày là
  Timeseries quay lại. Các mục xoay nhãn khác ở chỗ `xTicksChoiceOf` đọc CONFIG; select này đọc kết
  quả giải.

- **Task 3, Bước 2 — chỗ yếu đã nêu không tồn tại.** `SeriesOptions` vốn đã nhận `seriesNames`, do
  `viz/cartesian.tsx` truyền vào cho phần đếm Stacking, có fallback về danh sách metric. Cổng điều
  kiện tính được ngay tại đây. Ca duy nhất thực sự rỗng là breakout chưa có kết quả truy vấn; khi đó
  fallback là hỏi builder xem series 0 sẽ là gì.

- **Task 4, Bước 2 — không thể tái dùng nguyên `ChartAxesSection`, và lý do liệt kê được.** Nó không
  có scope theo trục, không có select thang trục x, không có tách metric trái/phải, không có
  `autoSplitY`, không làm mờ xoay nhãn trên trục giá trị, và cứng "Y" ở chỗ cartesian phải gọi tên
  trục mà người đọc thấy. Vì vậy phần tái dùng là bốn component con được export —
  `CategoryTicksSelect`, `ValueTicksSelect`, `ValueRangeRows`, `TickCountRow` — do cả hai panel render,
  khác biệt truyền vào qua prop. Mọi phần riêng của cartesian được giữ; không phần nào bị dời.

- **Task 4 còn khép lại một khác biệt từ ngữ mà phát hiện không nêu.** Cùng những dòng đó được gán
  nhãn "X axis lines and ticks" / "Y tick marks" ở tab này và "Show lines and tick marks" / "Number of
  tick marks" ở tab kia. Chia sẻ component chốt lại theo từ ngữ của Metabase.

- **Một test cũ phải đổi, và nó đang neo giữ một lỗi.** `scatterValueX.echarts.test.ts` khẳng định
  thang `timeseries` tường minh thắng trên cột `double`, chỉ kiểm `xAxis.type`. Cả năm điểm đều bị
  loại xuống một trục `time` không có biên — đúng lỗi Task 1 nói tới, mà suite vẫn xanh.

## Phát hiện khi thực thi, chưa sửa

- **Mô tả aria của scatter đọc ra `NaN` ở mọi giá trị y.** Đo trực tiếp trên `MB · Scatter`: "Scatter
  chart of rating by price. 168 values: 15.69 NaN, 19.87 NaN, …". Lỗi có sẵn, không liên quan tới thay
  đổi lần này — card đó không lưu `xScale` nên `resolveXScale` thoát sớm trước mọi mã mới. Cùng họ với
  phát hiện aria của pie đã nằm trong danh sách ngoài phạm vi ở trên (từ #387, series trên trục giá trị
  mang dữ liệu dạng object `{value: [x, y], name}`, còn bộ dựng aria đọc chúng như số trần). Thuộc về
  nhóm đó.
