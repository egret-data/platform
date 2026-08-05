# Chart nói gì với công nghệ trợ năng — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-05-chart-assistive-tech.md` là bản chuẩn; mã nguồn, số dòng và các bước chạy
> lệnh nằm ở đó.

**Mục tiêu:** Chấm dứt việc hai bề mặt chart loại người dùng bàn phím ra ngoài, và chấm dứt ba mô tả
aria nói những điều không đúng.

**Kiến trúc:** Hai nửa độc lập. **Task 1–3 là khả năng thao tác** — những control tự quảng cáo mình ra
mà không vận hành được. **Task 4–6 là tính trung thực** — những mô tả gọi tên dữ liệu chart không vẽ.
Task 7 là một design token và cố ý để cuối, vì bán kính ảnh hưởng của nó là **cả sản phẩm**, không
riêng chart.

---

## Xuất xứ

Ba lượt soát tại `1d9b0d30`. Chỗ ghi ĐO ĐƯỢC là đã lái hoặc chạy thật; còn lại là đọc. Lượt soát cũng
xác lập phần **phản chứng**, và phần này quan trọng cho việc định phạm vi: **198/199 phần tử tương tác
trên 24 component đã có tên khả truy cập**, cả hai ngữ cảnh `@dnd-kit` trong phạm vi đã có
`KeyboardSensor`, và mọi họ echarts đều đã phát ra mô tả. Đây là một danh sách lỗ hổng cụ thể, **không
phải** một vấn đề accessibility diện rộng.

---

# Phần một — khả năng thao tác

## Task 1 — `ChartTable` kéo giãn cột chỉ chuột dùng được

**ĐO ĐƯỢC.** `ChartTable.tsx:358-368` render `role="separator"` + `aria-label="Resize column"` mà chỉ
có `onMouseDown` / `onTouchStart`. Đo được `tabindex: null`, không handler phím nào, và panel cũng
không có ô nhập độ rộng. **Nó tự quảng cáo với trợ năng là một control mà không vận hành được.**

**Bản sửa đã có sẵn trong repo.** `pivot/PivotTable.tsx:12-51` là một component `ResizeHandle` hoàn
chỉnh: `tabIndex={0}`, mũi tên trái/phải theo `KEY_RESIZE_STEP`, chặn dưới bởi `MIN_COL_WIDTH`,
`focus-visible:bg-primary/60`, và một callback `getWidth` vì độ rộng đã lưu có thể vắng khi cột tự co
giãn. `ChartTable` chỉ đơn giản là bị bỏ lại.

**Tách ra dùng chung, ĐỪNG chép** — hai bản cài của một control chính là cách chương trình này từng có
bốn setting theo series nằm rải trên ba danh sách song song.

**Đặt tên từng tay nắm theo cột của nó.** `"Resize column"` đang là cùng một chuỗi trên mọi tay nắm,
nên người dùng trình đọc màn hình nghe thấy N control giống hệt nhau.

**Giữ nguyên ngữ nghĩa con trỏ:** tay nắm hiện tại của `ChartTable` mang `stopPropagation` vì **click
vào header sẽ đổi thứ tự sắp xếp, còn kéo thì không được phép**. Bộ handler nào sống sót cũng phải giữ
được bảo đảm đó — và phải có assertion cho nó.

## Task 2 — header của table khai "sortable" nhưng Enter lại kéo thả

**ĐO ĐƯỢC.** `ChartTable.tsx:338-357`, component header ở `:23-47`. Thẻ `<th>` có `tabindex="0"` và
`aria-roledescription="sortable"`, còn `aria-sort` thì vắng. Đo được: Enter không đổi chữ và đặt
`opacity: 0.6` — một cú nhấc của dnd-kit. Click chuột thì sắp xếp. **Sắp xếp hoàn toàn không có đường
bàn phím**; `SortRows.tsx` sửa `orderby` của truy vấn và chỉ tồn tại trong trình dựng.

`ChartTable.tsx:285-289` có ghi lại xung đột kéo-thả/sắp-xếp là có chủ ý. Nó **không** ghi rằng sắp xếp
vì thế trở nên không với tới được — mà đó mới là cái giá thật.

Cách thường dùng: một `<button>` thật bên trong `<th>` — header giữ phần kéo thả, nút nhận Enter/Space.
**Thêm `aria-sort` trên `<th>`** — hiện chiều sắp xếp chỉ được mang bởi ký tự `" ▲"`/`" ▼"`, thứ mà
trình đọc màn hình không thấy.

Và **cập nhật comment ở `:285-289`**. Một comment nêu sự thật đã kiểm mà nay không còn đúng thì tệ hơn
không có — #411 vừa tìm ra một cái khẳng định gauge của Metabase không drill, trong khi nó có.

## Task 3 — header chiều của pivot còn không focus được

**ĐO ĐƯỢC.** `pivot/PivotTable.tsx:154-174` là `<th onClick>` không `tabIndex`, không `onKeyDown`,
không `aria-sort`. Đo được: `th.focus()` không dời `activeElement`, Enter gọi handler **không lần nào**.
Tệ hơn Task 2 hẳn một bậc — không phải hành vi bàn phím sai, mà là không có gì cả.

---

# Phần hai — tính trung thực

## Task 4 — mô tả aria của pie gọi tên những lát chart không vẽ

**ĐO ĐƯỢC, và người soát đã tự kiểm lại độc lập từ mã nguồn sạch.** Mô tả dựng từ
`data = effectiveRings[0]` (`echarts/options/pie.ts:1697` → `:2063-2066`), còn thứ vẽ ra đến từ
`rootSlices = pieRootSlices(...)` (`:1805` → `seriesData` ở `:1999`), vốn lọc qua `toggledOffSlices`
(`:1393-1396`).

Với `toggledOffSlices: ["US"]`, mô tả đọc *"3 values: US 30, APAC 20, EU 10"* trong khi vòng 0 chỉ vẽ
`["APAC", "EU"]`. **Sai cả số đếm lẫn danh sách liệt kê.**

Phần `hiddenSlices` đã lưu thì xử lý đúng — các mutation của người soát trên nhánh đó đều chết. Chỗ
hỏng là **cú click legend tạm thời** mà aria không bao giờ biết tới.

Kiểm luôn nhánh ghi chú phân kỳ ở `options/pie.ts:2067-2069` — người soát nói rõ họ **chưa** xác minh
nó không bị xáo trộn.

## Task 5 — aria của scatter đọc `NaN` cho mọi y

**ĐO ĐƯỢC (lái thật, trong lúc làm #411).** Mô tả ra `"15.69 NaN, 19.87 NaN, …"`. Có sẵn từ trước và
không liên quan tới thay đổi của #411 — card đó không lưu `xScale` nên `resolveXScale` ngắt trước mọi
mã mới.

Mutation quan trọng: đọc `v[0]` thay vì `v[1]`. Trên scatter cả hai đều là số, nên **fixture phải dùng
giá trị x và y không thể nhầm lẫn được** — đúng cái bẫy #397 đã ghi lại, khi một epoch 13 chữ số khiến
hai bên hỏng theo hai hướng ngược nhau.

## Task 6 — aria của cartesian cũ đi khi click một mục legend

**SUY LUẬN — người soát không đo được vì không dùng browser, nên phải xác nhận trước khi sửa.**
`options/cartesian.ts:1684-1686` đặt `legend` của echarts; `:1713-1730` nướng `seriesCount` và các giá
trị thành **một chuỗi tĩnh lúc build**; `EChartsRenderer.tsx` chỉ đăng ký `click`, `mouseover`,
`mouseout` — không bao giờ `legendselectchanged`.

**Nếu không tái hiện được thì nói vậy và dừng** — sửa một lỗi không tồn tại còn tệ hơn không sửa.

---

## Task 7 — vòng focus theme sáng trượt ngưỡng tương phản

**Người soát ĐO ĐƯỢC** bằng cách chuyển token OKLCH sang sRGB và ghép alpha bằng số — cố ý tránh cái
bẫy "regex đọc ba số của `oklch()` thành RGB" mà chương trình này từng dính.

| | sáng | tối |
|---|---|---|
| `--ring` đặc | **2.58:1 TRƯỢT** | 4.08:1 đạt |
| `ring-ring/50` (Button, Input, Select) | **1.54:1 TRƯỢT** | **1.83:1 TRƯỢT** |
| fallback toàn cục `* { outline-ring/50 }` (`index.css:246`) | **1.54:1 TRƯỢT** | **1.83:1 TRƯỢT** |

**Ngưỡng ở đây là 3:1, không phải 4.5** — đây là chỉ báo focus phi văn bản (WCAG 1.4.11 / 2.4.11). Nói
thẳng điều đó ra chứ đừng lặng lẽ trượt ngưỡng văn bản. `--ring` sáng là `oklch(0.71 0.01 286.09)` tại
`index.css:114`; bản đè cho theme tối ở `:165`.

**Tự đo lại trước khi đổi bất cứ thứ gì.** Đây là thay đổi token chạm tới mọi phần tử focus được trong
sản phẩm, nên nó không được đi vào dựa trên một phép đo của người khác.

**Xác định bán kính ảnh hưởng.** Nếu hoá ra đây là thay đổi cấp sản phẩm chứ không phải cấp chart,
**nói vậy và dừng** — nó có thể thuộc về một PR riêng với người review khác.

Và một điều người soát không kết luận được, cần browser: **Chrome có tôn trọng `outline-color` do tác
giả đặt khi `outline-style: auto` hay không.** Nếu không, fallback toàn cục cần cách sửa khác, và nói
ra điều đó chính là phát hiện.

---

## Cố ý nằm ngoài phạm vi

`onPointClick` vẫn thiếu trên funnel, gauge, progress, smartscalar và object, cùng một comment ở
`ChartRenderer.tsx:319` khẳng định gauge của Metabase không drill trong khi `Gauge.tsx:186-188` cho
thấy có. Đó là **một PR riêng** — mỗi chart trong năm cái cần một quyết định về việc click vào nó lọc
theo cái gì, và các hình dạng drill theo từng chart ở chương trình này đều đã được lập luận riêng
(cạnh sankey lọc cả hai cột; boxplot đọc `dataIndex`; map lọc chiều thô chứ không bao giờ lọc khoá
geojson đã chuẩn hoá; waterfall từ chối trên cột Total). Nhét năm quyết định như vậy vào một PR
accessibility là chôn chúng.

Cũng ngoài phạm vi: bộ ba `view_as`/`link_text`/`link_url` chết trên 17/18 loại viz; bốn mutation sống
sót; và các khoảng cách phía Metabase.

## Tự soát

- **Task 1–3 dùng chung một control và phải dùng chung một bản cài.** Task 1 tách tốt thì Task 2 và 3
  rẻ đi; tách thành bản chép thì chúng đắt lên và chương trình lặp lại một sai lầm đã mắc hai lần.
- **Task 6 là task duy nhất khởi đi từ một phát hiện suy luận**, nên bước đầu của nó là xác nhận và
  điều kiện dừng được nêu rõ.
- **Task 7 có thể không thuộc PR này chút nào**, và bước 2 sinh ra để phát hiện điều đó sớm chứ không
  phải sau khi diff đã viết xong.
- **Chỗ yếu đã biết:** Task 5 giả định lỗi aria của scatter là lỗi bóc `[x, y]`. Cũng có thể nhánh aria
  của scatter được viết cho trục danh mục và chưa bao giờ cập nhật — khi đó bản sửa lớn hơn một phép
  truy cập trường. Xác định là đằng nào trước khi viết.

---

## Thực thi đã đính chính điều gì (bổ sung sau khi làm, PR #412)

Mọi phát hiện đều đúng. Năm điểm của kế hoạch thì không, và được ghi ở đây thay vì lặng lẽ sửa trong
diff.

1. **Việc tách ở Task 1 không thể là một phép dời nguyên khối.** Kế hoạch đọc như thể `ChartTable` chỉ
   cần dùng lại `ResizeHandle` của pivot. Nó không thể dùng lại phần *kéo*: `getResizeHandler` của
   tanstack là thứ điều khiển `columnSizingInfo`, và đó chính là thứ effect lưu-khi-kết-thúc-cử-chỉ
   của `ChartTable` theo dõi. Thay bằng vòng lặp pointer của pivot sẽ khiến mọi lần kéo hoặc không
   bao giờ được lưu, hoặc được lưu ở từng `pointermove`. Component dùng chung nhận prop
   `dragHandlers`; nửa bàn phím — chính là lỗi thật — thì dùng chung hoàn toàn.

2. **Task 1 còn cần một đường ghi riêng.** Một lần chỉnh kích thước bằng phím mũi tên là cử chỉ không
   có thao tác kéo bao quanh, nên `isResizingColumn` không bao giờ đổi và effect không bao giờ chạy.
   Thiếu điều đó, độ rộng đổi trên màn hình rồi mất ở lần render kế tiếp từ component cha.

3. **Chỗ yếu đã biết của Task 5 chỉ đúng một nửa, và bên dưới còn một lỗi thứ hai.** Giả định về phép
   bóc là đúng — `datumY` là bản sửa. Nhưng `applyXScale` còn LỌC: một datum mà x không phân tích được
   sẽ bị bỏ, nên `series.data` không còn khớp chỉ số với `drawnCategories`, và phép ghép cặp gán cho
   mỗi điểm còn sống giá trị của nhãn *kế tiếp*. Điều đó không đọc thành `NaN`; nó đọc thành một câu
   nghe hợp lý, nên đã sống lâu hơn cả lỗi nhìn thấy được. Có thể xảy ra trong sản phẩm — `Date.parse`
   từ chối cách Trino kết xuất timestamp có vùng theo tên miền (`… 00:00:00.000 Asia/Ho_Chi_Minh`).

4. **Task 6 tái hiện đúng như suy luận.** Đo trên "MB · Combo" đã triển khai: khi tắt `orders`, biểu đồ
   chỉ vẽ revenue mà container vẫn xướng *"Combo chart of orders … 2 series; January 2026 205, …"*.
   Sửa bằng cách dẫn xuất lại ở `legendselectchanged` chứ không phải bỏ phần liệt kê — phần liệt kê là
   thứ duy nhất trong câu đó mang con số, và biểu đồ nhiều chuỗi lại đúng là nơi người dùng đọc màn
   hình ít có lối khác nhất.

5. **Task 7 có phạm vi toàn sản phẩm và dừng ở bước 2, đúng điều kiện dừng của chính nó.** `--ring`
   chạm 34 điểm gọi trên 26 tệp — mười hai primitive shadcn cộng notebook, SQL Lab, control plane và
   các route explore; chỉ 5 tệp nằm dưới `components/chart/`. Các số đo đã được dẫn xuất lại độc lập
   (OKLCH→sRGB bằng số, alpha ghép nền) và khớp bảng của người soát tới hai chữ số thập phân. **Câu
   hỏi mở ở bước 4 đã có đáp án: `outline-style: auto` của Chrome CÓ tôn trọng `outline-color` do tác
   giả đặt** — đo bằng cách ép `outline-color: red` lên một phần tử đang focus mà style vẫn là `auto`
   rồi chụp màn hình, và nó vẽ viền đỏ. Vậy fallback toàn cục ở `index.css:246` không cần cách sửa
   khác; làm tối `--ring` và bỏ `/50` là chạm tới nó. Thứ ĐÃ vào PR này: hai control mà PR giới thiệu
   mang viền focus `--primary` (3.29:1 trên `--accent`, 3.58:1 trên `--background`) thay vì thừa
   hưởng một viền 1.54:1.

Hai điều nữa kế hoạch không lường trước, đều đến từ đo đạc:

- **Bọc chữ của header vào một button có nhãn khiến `<th>` mất tên trong Chrome.** Đọc từ cây
  accessibility trực tiếp bằng cách xoá thuộc tính khỏi một header: columnheader đó trở về hoàn toàn
  không có tên, bên cạnh chín anh em vẫn mang tên cột của mình. Vì vậy cả hai bảng đều đặt `aria-label`
  tường minh là tiêu đề thuần cho `<th>`. jsdom không tái hiện điều này.
- **Bản nháp đầu tiên chặn ngưỡng độ rộng cột ở hai nơi**, và mutation xoá phần chặn bên trong control
  dùng chung đã sống sót vì phần chặn ở nơi gọi che cho nó. Nay chỉ còn một.
