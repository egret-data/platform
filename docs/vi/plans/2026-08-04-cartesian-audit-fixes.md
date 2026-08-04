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
