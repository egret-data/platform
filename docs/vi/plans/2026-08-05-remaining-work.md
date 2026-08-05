# Tất cả những gì còn mở — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-05-remaining-work.md` là
> bản chuẩn; mã nguồn, số dòng và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Đóng những gì thật sự còn lại sau chín PR (#407–#415), trong ba đơn vị không trộn lẫn
mối bận tâm.

**Kiến trúc:** **PR A là accessibility** — những thứ người dùng bàn phím không thấy hoặc không với
tới. **PR B là hai lỗi chart.** **PR C là ba tính năng Metabase có mà egret không có.** Chúng độc lập;
làm A trước, vì đó là cái duy nhất có lỗi mà người dùng gặp ở **mọi** menu.

---

## Về nguồn gốc danh sách này, và những gì đã bị loại

Ba lượt soát và chín PR để lại một danh sách tồn đọng. **Trước khi viết kế hoạch này, từng mục đã được
kiểm lại với mã nguồn, và năm mục KHÔNG sống sót:**

- **Bubble size của scatter** — đã có đủ. `bubbleMetric` nằm trong codec, builder đọc ở
  `cartesian.ts:784`, tooltip dùng ở `:850`, và có test.
- **Show total của pie** — đã có đủ. `pie.ts:857` đọc `cfg.showTotal ?? true`, có gate theo lỗ donut,
  control ở `PieDisplaySection.tsx:153-160`.
- **`resizable.tsx` "đặt màu vòng mà không có độ dày"** — sai. `focus-visible:ring-1` nằm ngay dòng
  dưới dòng đặt màu.
- **Khối trục giá trị Y của scatter** — không phải sơ suất. `CartesianDisplaySection.tsx:265-266` nói
  rõ scatter metric-vs-metric đi qua `scatterXY`, nơi **dựng hai trục giá trị riêng và không bao giờ
  đọc các key đó**. Một chỗ rẽ có lập luận, không phải khoảng trống.
- **`ring-offset-2` thiếu `ring-offset-background`** — có thật nhưng chỉ **một** chỗ, không phải một
  mẫu hình: `routes/control-plane/access/AccessOverview.tsx:376`. Gộp vào PR A thành một dòng.

**Hai trong số đó đến từ một lượt soát panel lái thật, một từ mục "found, not fixed" của chính một
PR.** Cả hai đều là nguồn thường đáng tin. Bài học nằm ở phần tự soát cuối; hãy coi mọi mục dưới đây là
**đã kiểm lại**, vì đúng là đã kiểm.

---

# PR A — thứ người dùng bàn phím không thấy hoặc không với tới

## Task A1 — mục menu đang focus thì vô hình

**ĐO ĐƯỢC.** Một mục đang focus trong dropdown, select, context menu hay command palette **không có
vòng focus nào cả** — nó mang `outline-none` và chỉ báo hiệu focus bằng cách đổi nền sang `--accent`.
`--accent` sáng là `oklch(0.97 0 0)` trên một `--popover` gần như trắng: **1.03:1**.

Các chỗ (kiểm lại danh sách, có thể đã dài thêm): `components/ui/dropdown-menu.tsx:28, 84, 100, 124`;
`components/ui/select.tsx:108`; kiểm thêm `command.tsx`, `context-menu.tsx`, `menubar.tsx`,
`navigation-menu.tsx` xem có cùng hình dạng không.

**Đây KHÔNG phải lỗi mà #415 đã sửa.** #415 nâng `--ring` để các *vòng* focus đạt 3:1. Những phần tử
này **không có vòng nào để nâng** — chúng đã tự loại mình ra bằng `outline-none`. Nên bản sửa là một
quyết định khác, và là quyết định thật:

- khôi phục một chỉ báo focus nhìn thấy được (một vòng, hoặc một vạch bên trái) trên mục đang focus;
- làm tối `--accent` để chính cú đổi nền mang được 3:1;
- hoặc cả hai.

**Làm tối `--accent` có bán kính ảnh hưởng rộng hơn hẳn** — nó là nền hover/active khắp sản phẩm chứ
không riêng menu, nên đổi nó là đổi trạng thái hover ở mọi nơi. **Kiểm xem còn gì đọc nó trước khi
chọn.** Menu của Metabase đáng xem để lấy *hình dạng*, không phải lấy *giá trị*.

Test: #415 đã thêm `frontend/src/__tests__/focusIndicatorContrast.test.ts` tính tương phản từ
`index.css` một cách tất định — **mở rộng nó** chứ đừng viết bộ đo thứ hai. Ghim **cả hai theme** và
ghim **3:1**, kèm comment nói vì sao là 3 chứ không phải 4.5.

Rồi **NHÌN nó**. Một con số tương phản không phải một lượt review thiết kế — chương trình này có sẵn ca
một caption đạt mọi phép kiểm bằng số trong khi nằm cách thứ nó chú thích 300px.

## Task A2 — chỗ thiếu ring offset duy nhất

`AccessOverview.tsx:376` dùng `focus-visible:ring-offset-2` mà không có `ring-offset-background`, nên
`ring-offset-color` mặc định về trắng — một dải trắng 2px bên trong vòng khi ở theme tối. Sửa một dòng.

## Task A3 — drill theo ô không có đường bàn phím

`ChartTable`, `pivot/PivotTable` và object detail **đều** mở drill bằng `onClick` chuột, không có
tương đương bàn phím. #412 đã cho *header* phần sắp xếp và kéo giãn bằng bàn phím; các **ô** thì bị bỏ
lại.

**Đây là MỘT quyết định, không phải ba bản vá** — cùng một câu hỏi ở ba chỗ, và nhánh object của #413
đã ghi lại vì sao nó không tầm thường: một ô object-detail có thể chứa `<pre>`, `<img>` hoặc một nút
lồng, nên biến chính cái ô thành nút không đương nhiên là đúng.

**Chốt mẫu hình một lần.** Roving-tabindex trên một lưới (`role="grid"`) là câu trả lời thường dùng
cho bảng có ô thao tác được, và nó tránh nhét hàng trăm phần tử vào thứ tự tab. Nói rõ chọn gì, vì sao,
và cái giá ở từng chỗ trong ba chỗ.

**Nếu kết luận trung thực là việc này cần một lượt thiết kế riêng thì nói vậy và dừng sau A1, A2.** Đó
là kết cục chấp nhận được — một bản sửa accessibility làm việc điều hướng tệ đi thì không phải tiến bộ.
Đừng ship nửa mẫu hình trên ba component.

---

## CẬP NHẬT — PR A đã tìm thấy gì khi kiểm lại chính mục này

Viết sau khi làm xong, đúng tinh thần của phần "những gì đã bị loại" ở trên: **ba khẳng định trong
chính các task của PR A không sống sót khi chạm vào mã nguồn**, và một giả định đưa ra *trong lúc
triển khai* không sống sót khi chạm vào trình duyệt. Văn bản gốc được giữ nguyên để còn đọc được
khoảng cách giữa điều đã tin và điều đúng.

**Danh sách chỗ của A1 sai theo cả hai chiều.** Bốn tệp bị nghi — `command.tsx`, `context-menu.tsx`,
`menubar.tsx`, `navigation-menu.tsx` — **không tồn tại trong repo này**. Nhưng command palette nhắc
trong phần mô tả thì có thật: đó là `components/notebook/CommandPalette.tsx`, viết tay trên primitive
Dialog, không dùng cmdk, và mang đúng lỗi 1.03:1 ở hàng đang được chọn. Vậy danh sách vừa thừa bốn tệp
vừa thiếu một tệp. PR A sửa bốn mục trong `dropdown-menu.tsx`, mục của `select.tsx`, và hàng của
palette, đồng thời thay danh sách tệp bằng một **lượt quét cả thư mục `components/ui`** để lần sau có
ai dán thêm một menu shadcn thì bị bắt mà không cần kiểm tay.

Palette không chỉ là bản sao thứ tư — nó cần cơ chế khác. Focus DOM không bao giờ rời ô tìm kiếm; hàng
sáng là focus **ảo** do chỉ số `selected` điều khiển, nên không pseudo-class focus nào khớp được và chỉ
báo phải đi cùng chính điều kiện đang vẽ nền.

**A2 là bốn chỗ, không phải một.** Kế hoạch đã rút gọn thành "có thật nhưng chỉ một chỗ, không phải một
mẫu hình". Lượt quét PR A thêm vào tìm thấy `ring-offset-<n>` không kèm `ring-offset-<màu>` ở **bốn**
chỗ: `AccessOverview.tsx:376` (chỗ đã nêu), `ui/resizable.tsx:42`, và `RuleEditor.tsx` hai lần. Hai chỗ
cuối không phải vòng focus mà là trạng thái **đã chọn** của ô màu, nơi một sợi trắng vẽ quanh một MÀU
sẽ bị đọc thành một phần của chính màu đó. Cả bốn đã sửa và bất biến này giờ được quét toàn repo.

Đáng ghi lại nguồn gốc của sai sót: `resizable.tsx` có mặt trong danh sách bị loại ở trên dưới một
khẳng định **khác** ("đặt màu vòng mà không có độ dày"), và khẳng định đó đúng là sai. Sau khi bác bỏ
đúng một khẳng định về tệp này, khẳng định thứ hai nằm ngay trên cùng dòng đã không được nhìn tới.

**Quyết định của A1: thêm chỉ báo, không đụng `--accent`.** Đã đo trước khi chọn. `--accent` được 63
tệp đọc với khoảng 110 lần dùng; **55 trong số đó là `hover:bg-accent`** và 16 lần nữa là
`hover:bg-accent/40|/50|/60`, so với chỉ 5 lần `focus:bg-accent`. Nó là nền hover ở trạng thái nghỉ của
cả sản phẩm. Để chính cú đổi nền đạt 3:1 so với `--popover`, `--accent` sáng phải tụt từ L 0,97 xuống
**0,654** và bản tối phải lên từ 0,27 tới **0,504** — một mảng xám trung tính dưới mọi cú hover chuột
trong sản phẩm, và khi đó `--accent-foreground` sẽ trượt ngưỡng **4,5:1** dành cho chữ, ngưỡng mà nó
thật sự phải đạt. Stylesheet đã build của Metabase được đọc để lấy hình dạng và cho kết quả trùng: các
control của nó dùng `:focus-visible { outline: 2px solid var(--mb-color-focus) }` — một token focus
riêng, kèm `outline-offset: -2px` hoặc `box-shadow: inset 0 0 0 2px` ở nơi phần tử không được vẽ ra
ngoài chính nó. Luật được chép; giá trị thì không — egret dùng `--ring` của mình.

**A3 dừng, theo đúng điều kiện dừng của nó.** Mẫu hình không ra được thành một quyết định, và các rào
cản mang tính cấu trúc chứ không phải chuyện công sức:

- **ObjectDetail không phải lưới.** Nó là một `<dl>` gồm các hàng trường/giá trị, nên roving-tabindex
  kiểu `role="grid"` không phải mẫu hình của nó — nó cần mẫu list/listbox. "Chốt mẫu hình một lần" ra
  ít nhất hai mẫu hình chứ không phải một.
- **Phím mũi tên của ChartTable đã có chủ.** `useSensor(KeyboardSensor, { coordinateGetter:
  sortableKeyboardCoordinates })` gán phím mũi tên cho việc di chuyển cột đang nhấc, và dnd-kit đặt
  `tabIndex=0` lên từng `<th>`. Roving tabindex `role="grid"` cần phím mũi tên để di chuyển ô và cần cả
  bảng chỉ là MỘT điểm dừng tab — tức là phải tháo các điểm dừng ở header mà #412 đã cố ý tạo ra
  (`SortButton`, `ResizeHandle`, activator kéo thả). Đó là làm lại mô hình của #412 bên trong một mô
  hình mới, không phải mở rộng nó.
- **Header của PivotTable không phải hình chữ nhật.** Header nhiều tầng với `colSpan`/`rowSpan`, nút
  gập và vòng đổi sắp xếp; toạ độ roving-tabindex không xác định được nếu không có một mô hình lưới
  thật.
- **Callback drill nhận toạ độ viewport.** `onCellDrill(ctx, x, y)` dùng vị trí chuột để đặt popup.
  Kích hoạt bằng bàn phím không có toạ độ đó, nên cả ba nơi gọi đều cần một điểm neo suy từ rect, cộng
  thêm việc trả focus khi đóng — cả ba đều chưa viết.
- **Một ô có thể chứa nội dung tương tác của riêng nó** (`<pre>`, `<img>`, nút "Show more"), cần tới
  chế độ con vào-ô/thoát-ô của mẫu hình lưới, phần dễ sai nhất của nó.

Vậy A3 cần lượt thiết kế riêng và PR riêng. `ObjectDetail.tsx:123-133` đã có sẵn ghi chú nói đúng điều
đó; không có gì trong PR A mâu thuẫn với nó.

**Một giả định lúc triển khai bị trình duyệt bác bỏ.** Vòng focus được treo vào `focus-visible:` với kỳ
vọng giữ nó khỏi đường đi của chuột, vì Radix chuyển focus DOM khi con trỏ di qua. Đo trong Chrome trên
bundle đã build: một cú mousemove thật lên mục menu vẫn khiến nó khớp `:focus-visible` và vòng vẫn vẽ.
Phép đối chứng cho thấy bộ phân biệt vẫn chạy — một link điều hướng được focus bằng cú **click** chuột
thật thì khớp `:focus` chứ không khớp `:focus-visible` — nó chỉ đơn giản là không nhìn xuyên được lời
gọi `focus()` từ script của Radix. Hành vi này được chấp nhận (trong menu Radix, mục đang sáng chính là
mục Enter sẽ chạy, bất kể thiết bị nào đưa nó tới đó) và khẳng định sai được **sửa lại** trong comment
của `dropdown-menu.tsx` chứ không để nguyên.

---

# PR B — hai lỗi chart

**B1 — số của trend tràn card hẹp.** ĐO ĐƯỢC ở #413: 203,75px số trong card 166px. Có sẵn từ trước —
#413 đã ghim rằng vỏ nút mới của nó đóng góp 0px.
**Tìm xem scalar làm gì**, vì `BigNumber` đã giải đúng bài toán này bằng `measureText` trên canvas +
`findSize`, nén theo chiều rộng và tooltip giá trị đầy đủ. **Dùng lại nếu vừa.** Nếu bố cục của trend
khiến không dùng được thì nói chính xác vì sao.
jsdom không chạy layout và `measureText` trả về 0, nên một bộ tự-chỉnh-cỡ sẽ "đạt" ở bất kỳ cỡ nào
vòng lặp chạm đáy. Test viết ra phải nói rõ nó chứng minh và không chứng minh được gì; phép kiểm thật
là ảnh chụp ở đúng bề rộng 166px của card harness.

**B2 — tiêu đề trục trái của combo đọc tên series đã ẩn.** Tìm được ở #412, **SUY LUẬN — phải tái hiện
trước khi sửa**. Cùng họ "cũ đi sau khi click legend" với hai lỗi aria #412 đã đóng bằng cách đăng ký
`legendselectchanged` trong `EChartsRenderer`.
**Nếu không tái hiện được thì nói vậy và dừng.** Nếu tái hiện được, kiểm xem đăng ký #412 đã thêm có
gánh luôn được không, thay vì thêm cơ chế thứ hai.

---

## CẬP NHẬT — PR B đã tìm thấy gì khi thực thi chính mục này

Viết sau khi làm xong, cùng tinh thần với ghi chú cắt tỉa và phần cập nhật của PR A ở trên. **Cả hai
lỗi đều có thật; cái duy nhất mới chỉ là SUY LUẬN đã tái hiện được, và còn nặng hơn mô tả.** Hai điểm
trong đề bài sai ở chi tiết, và một cạm bẫy thì không thể lường trước từ bản kế hoạch. Văn bản gốc
được giữ nguyên.

**B2 tái hiện được, trên Chrome, trước khi viết một dòng code — và tệ hơn mô tả.** Trên "MB · Combo"
đã deploy, click tắt `orders` khiến ECharts chỉ vẽ revenue và **bỏ hẳn** nhãn vạch của trục trái
(0/100/200/300/400/500 biến mất hết: trục không còn series nào, không còn biên độ, không còn gì để
đánh số). Tiêu đề xoay dọc "orders" vẫn được vẽ cạnh cái trục rỗng đó, và hộp `<text>` của nó còn dịch
từ x=573 sang x=549 khi vùng vẽ nở ra chiếm chỗ các con số vừa nhường. Vậy câu "trục đã đổi thang" của
kế hoạch là nói giảm: trục **không còn thang nào cả**, và thứ duy nhất còn trên đó là tên một series
không ai nhìn thấy. Đây là mục thứ sáu trong loạt này được kiểm lại trước khi hành động và **là mục
đầu tiên sống sót** — đáng ghi lại, vì kỷ luật kiểm lại không phải thủ tục lúc nào cũng trả lời "đã sửa
rồi".

**Câu hỏi của B1 — "dùng lại bộ chỉnh cỡ của scalar nếu vừa" — câu trả lời là vừa, và đã dùng lại
nguyên khối.** Không có bộ chỉnh cỡ thứ hai: `compactifyValue` → `findSize` → `measureText`,
`maxFontSizeRem`, `SCALAR_PADDING`, `MIN_SIZE_REM`, `SIZE_STEP_REM` và tooltip giá trị đầy đủ đều được
mang sang nguyên vẹn, còn `gridCols` được truyền từ `ChartRenderer` đúng như của scalar. Thứ KHÔNG
mang sang chỉ là những gì trend không có: dải màu điều kiện và `label`/`sublabel` của `BigNumber`. Một
khác biệt cấu trúc phải xử lý chứ không chép được: `BigNumber` chỉ có một `<div>` chú thích để đo,
trong khi trend có hai khối anh em (nhãn kỳ và dòng so sánh), nên chúng được bọc thành một khối đo
được duy nhất với `gap-1` bên ngoài được lặp lại bên trong. Khoảng cách trên màn hình không đổi; không
có lớp bọc thì không có một chiều cao duy nhất để trừ đi.

**Một cạm bẫy kế hoạch không thể gọi tên: bản vá merge của ECharts có khoá `name` sẽ được khớp THEO
name.** Bản B2 chạy được đầu tiên vá hai trục y dưới dạng mảng, và `mappingToExists` của ECharts —
khớp theo `id` trước, theo `name` sau — đã khớp phần tử thứ *hai* của bản vá vào trục *thứ nhất* qua
cái tên rỗng. Kết quả đo được: `["", "rev"]`, tiêu đề nằm sai phía biểu đồ, chỉ sau thao tác tắt rồi
bật lại một mục legend. Trường đang được vá chính là trường dùng để khớp. Nay cả hai trục giá trị đều
mang `id` tường minh và bản vá gọi đích danh theo id.

**Hai test có sẵn phải đổi, và cái thứ hai là lỗ hổng trong độ phủ của #412 chứ không phải cái giá của
thay đổi này.** `sizeAware.test.ts` ghim đúng số closure một option mang theo, nên một closure mới có
chủ đích đưa con số 4 → 5. Trong lúc sửa đó, hàm `probe` của nó lộ ra là render giá trị trả về của mọi
hàm qua `String()` — thứ biến *mọi object* thành hằng `"[object Object]"`. `__ariaFor` (#412) và
`__axisNamesFor` đều trả object, nên cả hai đang được "probe" thành sáu ký tự y hệt nhau bất kể chúng
tính ra gì, và phép kiểm rò-rỉ-size mà file đó tồn tại vì nó thì không thể thấy được rò rỉ ở cả hai.
`probe` nay tuần tự hoá giá trị trả về dạng object.

**Tìm được nhưng không sửa (1).** Ở card 166px của harness, *dòng so sánh* của trend (`▲ 9.54% / vs
previous month: 22.577,58`) xuống dòng giữa chừng thành bốn hàng. Nó không tràn — mọi hộp đều nằm
trong card — nên không thuộc nhiệm vụ này, và rút gọn nó là câu hỏi thiết kế về đối chiếu Metabase
(Metabase chỉ còn để lại phần trăm trên card hẹp) chứ không phải lỗi chỉnh cỡ. Ghi ra đây để nó là một
quyết định có hồ sơ.

**Tìm được nhưng không sửa (2), và đã khoanh vùng trước khi báo.** Trình sửa chart bật hộp thoại
"chưa lưu thay đổi" (`beforeunload`) sau BẤT KỲ cú click nào trong vùng canvas — click legend, hay
click vào vùng vẽ trống ở (700, 300) — dù không cái nào làm đổi thứ được lưu. Nó lộ ra khi điều khiển
legend cho B2, nên nghi vấn đầu tiên là do `setOption` trên `legendselectchanged` của chính PR này.
**Không phải:** cô lập bằng ba lần chạy — mở rồi rời trang chart mà KHÔNG tương tác thì không có gì,
click legend thì có, click vào canvas trống cũng có. Vậy là bất kỳ cú click nào, không phải legend, và
nó có trước cả PR này lẫn handler của #412. Cả hai chart đã điều khiển đều được fetch lại sau đó và
`updated_at` không đổi (`2026-08-04T08:48Z`), tức là không có gì bị lưu.

**Không phải chỗ sai của kế hoạch, nhưng là bẫy cho người đọc kế tiếp:** các UUID chart/dashboard trích
trong những tài liệu kế hoạch trước của loạt này vẫn còn sống, nhưng bản lưu trong scratchpad thì
không — dữ liệu demo đã được seed lại, và mọi id trong `charts-zzparity.json` giờ đều 404. Hãy đọc lại
danh sách từ `/explore/demo/charts` thay vì từ fixture đã lưu.

---

# PR C — ba tính năng Metabase có mà egret không có

Cả ba đã kiểm bằng grep là **thật sự vắng**. Đây là **khoảng cách, không phải lỗi** — không có gì
hỏng, egret chỉ đơn giản là ít hơn.

**C1 — "Unpin from zero".** `graph.y_axis.unpin_from_zero` có trên line, scatter và box plot của
Metabase. egret không có gì tương đương.
Đọc bản cài của Metabase trước khi thiết kế. **Chép LUẬT, không chép giá trị** — bài học lặp nhiều
nhất của chương trình này, đã dính sáu chart. Xác định nó thật sự làm gì với biên độ trục, và ECharts
diễn đạt nó bằng `min: "dataMin"` hay bằng gì khác.
Key lưu trữ mới ⇒ phải có mục trong `KEYS` của `viz/cartesian-codec.ts` (`satisfies keyof CartesianConfig`)
nếu không nó sẽ không được lưu. Backend không cần gì: `chartparams.rs` chỉ kiểm hình dạng.

**C2 — chế độ nhãn "Compact" trên trục x.** Select "Show lines and tick marks" của Metabase có 5 lựa
chọn; của egret có 4 — **thiếu Compact**.
**Một quyết định đã ghi, đọc trước khi bắt đầu:** một đơn vị trước **cố ý không** chép "Compact" sang
**waterfall**, với lý do nó rút gọn nhãn THỜI GIAN còn trục x của waterfall là danh mục. **Lý do đó
không tổng quát hoá được** — với một chart cartesian có trục x thời gian thì nó đúng y. Xác định chart
nào nên có và ghi lại lý do.

**C3 — "Comparisons" của trend.** Tab Data của trend Metabase có danh sách Comparisons; trend của egret
không có control so sánh nào.
**Đây là cái lớn nhất trong ba và có thể xứng đáng một PR riêng.** Xác định kích cỡ trước — một phép so
sánh là một **hình dạng truy vấn thứ hai**, không chỉ là một setting. Nếu hoá ra cần việc phía backend
thì dừng và báo chứ đừng phình phạm vi; `chartparams.rs` dễ dãi nhưng *truy vấn* thì có thể không.

---

## CẬP NHẬT — PR C đã tìm thấy gì khi thực thi chính mục này

Viết sau khi làm xong, cùng tinh thần với ghi chú loại bớt và các phần cập nhật của PR A, PR B ở
trên. **PR C giao C1 và C2 rồi DỪNG TRƯỚC C3**, và lý do không phải là lý do kế hoạch này dự đoán.
Hai điểm của đề bài phải sửa, một quyết định đã ghi lại bị phép đo lật ngược, và hai tệp test mà một
lượt thử bị ngắt để lại trên nhánh đều được GIỮ — nhưng một trong hai có fixture không bao giờ chạm
tới đúng nhánh code mà nó khẳng định. Văn bản gốc được giữ nguyên.

**Cả hai tệp test có sẵn đều được GIỮ, và mọi trích dẫn Metabase trong đó đều đúng nguyên văn.** Mọi
số dòng trong phần header đã được kiểm lại với bản checkout Metabase và đều chính xác:
`option/axis.ts:380` đúng là `scale: !!settings["graph.y_axis.unpin_from_zero"]` và đó là TOÀN BỘ
phần cài đặt; `model/axis.ts:716/778/856` đúng là nói `compact: settings[...] === "compact"` ba lần;
`cartesian-chart.ts:245-260` và `:242` đúng nguyên văn; `formatting/date.tsx:714-718` là phép hoán
đổi `compact || date_abbreviate`. Cả hai tệp cũng đều fail vì đúng lý do trước khi viết dòng code nào
— 18 fail trên hai tệp.

**Một khẳng định trong đó bị sai, và sai đúng theo kiểu chương trình này hay sai: một tiền đề không ai
đo.** Fixture của box plot là hai tên category ngắn trên thẻ 166x184, ở kích thước đó
`verticalValueAxisFit` thấy đủ chỗ cho năm vạch nên TỪ CHỐI — không `min` nào được ghi và
`expect(pinned.min).toBe(0)` đọc ra `undefined`. Đó là fixture không chạm tới nhánh đo đạc chứ không
phải trục bị ghim. Đã sửa thành bốn tên, đúng hình dạng mà `options/boxplot.ts` đã đo lỗi của chính
nó, và nhánh "pinned" giờ khẳng định `toBeDefined()` trước khi khẳng định giá trị.

**Cờ unpin không làm gì ngoài đặt `scale` của ECharts, và chép mỗi điều đó thôi thì sẽ giao ra một
control lưu được mà không vẽ.** Metabase không tính con số nào. Nhưng builder của egret không nhường
extent cho ECharts — trên một thẻ đã đo, `verticalValueAxisFit` ghi `min`/`max` tường minh, và **một
`min` tường minh thắng đứt `scale`**. Vì vậy cả hai hạt giống extent đều đi theo cờ. Dưới jsdom
`measure-text` trả về số 0 theo thiết kế, nên một bộ test không mock không thể thấy nửa đó.

**Đo trực tiếp trên trình duyệt của chính Metabase tại :3033**: "MB · Line" khi ghim vẽ
`0 / 5.000 / … / 25.000`; cùng thẻ đó khi bỏ ghim vẽ `9.000 / 12.000 / … / 27.000`. Ma trận hiển thị
được lái chứ không đọc: **Area ẩn, Bar ẩn, Combo ẩn, Waterfall ẩn, Boxplot hiện và chưa tick, Scatter
hiện và ĐÃ TICK SẴN**. Không lưu gì cả.

### Quyết định đã ghi của C2 ĐÚNG VỚI BOX PLOT VÀ SAI VỚI WATERFALL

Đây chính là chỉ dẫn ở Task C2 của kế hoạch — "một đơn vị trước đã cố ý KHÔNG chép Compact sang
waterfall … **Lý do đó không tổng quát hoá được**" — đã được xác nhận, và cần ghi rõ nửa nào còn
đứng vững để không ai phải suy lại.

**Lý do đã ghi, nguyên văn** (trong phần doc của `settings/ChartAxesSection.tsx` và lặp lại ở
`CartesianDisplaySection.tsx`): Compact "rút ngắn nhãn trục thời gian, mà trục x của các chart này là
category chạy qua format cột của nó, nên tuỳ chọn này sẽ chẳng làm gì ở đây". Một câu, viết một lần,
phủ lên HAI chart cùng dùng tab Axes đó.

**Đúng với box plot.** `options/boxplot.ts` **không có `axisLabel.formatter` nào** trên trục category
— nó vẽ thẳng khoá nhóm thô và có nói rõ điều đó. Compact là cờ định dạng; không có formatter thì
không có gì để nó tác động qua, và mở tuỳ chọn ở đó chính là kiểu control chết mà chương trình này
liên tục gỡ bỏ. Chart đó vẫn không được nhận.

**Sai với waterfall, và điểm bác bỏ là cấu trúc chứ không phải mức độ.**
`options/waterfall.ts:234-242` mang `bucketedX = namesAPeriod(cfg.xAxisUnit)` và một `drawLabel` có
nhánh thứ hai là `periodName(cfg.xAxisUnit, v)` — **đúng hai nhánh đó, gọi đúng module đó, giống
`drawLabel` của builder cartesian**. Đó không phải trùng hợp: chính comment của waterfall nói hình
dạng này là "cùng ba nhánh theo cùng thứ tự với cartesian, và cố ý không phải một lối viết thứ hai".
Nên waterfall gom theo tháng HÔM NAY vẽ "January 2026" — một nhãn thời gian, đúng thứ Compact rút
ngắn. Tiền đề "trục x của chart này là category chạy qua format cột" mô tả nhánh 1 và lặng lẽ bỏ qua
nhánh 2, mà nhánh 2 mới là nhánh quan trọng.

Bài học tổng quát, vì đây là lần thứ ba trong đợt này một lý do đã ghi bị áp rộng hơn phạm vi nó được
đo: **lý do đó viết về cả TAB, mà tab có hai chart khác nhau đúng ở chỗ nó khẳng định.** Vì vậy
Compact nay là một PROP (`offerCompact`), không còn là sự thật về tab. Chiều ngược lại cũng vậy với
"Unpin from zero" — box plot có, waterfall không, vì `isYAxisUnpinFromZeroValid` loại display
waterfall theo tên. Hai chart, hai setting, mỗi chart đúng một cái.

**Việc hoãn của riêng cartesian sai vì một lý do thứ ba.** Nó hoãn vì "cartesian là họ duy nhất có
thang x `timeseries`, đúng chỗ một bộ rút gọn thời gian sẽ có tác dụng, nhưng trục đó là `scaleAxis`
mà nhãn vạch do ECharts sinh". Cả hai mệnh đề đều đúng; kết luận thì không. Trục x ORDINAL — mặc định,
và là thứ mọi chart thời gian gom nhóm thực sự vẽ — đi qua `drawLabel`, đúng chỗ Compact tác động.
`scaleAxis` vẫn ngoài tầm với và nay bị LÀM MỜ ở đó, theo đúng dữ kiện `valueX` mà các lựa chọn xoay
đã bị làm mờ, thay vì bị giữ lại khỏi mọi chart.

**Compact là MỘT cờ của Metabase và HAI trường của egret** (`date_abbreviate` cho ngày, `compact` cho
số). Chỉ đặt `compact` sẽ khiến trục NGÀY compact trở thành no-op im lặng.

**Một cái bẫy kế hoạch không thể gọi tên: phải hỏi `specifiesDateRendering` về format ĐÃ LƯU, không
bao giờ hỏi về format đã thêm compact.** Compact thêm `date_abbreviate`, vốn là một trong bảy trường
hàm đó tra — nên hỏi bản đã thêm sẽ làm nó true trên mọi trục gom nhóm và vẽ "Jan 1, 2026" thay vì
"Jan 2026".

**Một chỗ phải sửa trong chính chỉ dẫn của kế hoạch.** Mục `KEYS` trong `viz/cartesian-codec.ts` là
**cần nhưng chưa đủ**: các interface config của BUILDER là kiểu riêng, và một key chỉ thêm vào kiểu
phía API sẽ qua được codec mà không bao giờ tới chart. `tsc` bắt được (7 lỗi trong 3 tệp).

### C3: ĐIỀU KIỆN DỪNG KHÔNG KÍCH HOẠT, VÀ TÔI VẪN DỪNG VÌ MỘT LÝ DO KHÁC

**Chỗ phân biệt này mới là phần hữu ích của mục này, nên nói trước các con số.** Điều kiện dừng của kế
hoạch là: *"một phép so sánh là một **hình dạng truy vấn thứ hai** … Nếu hoá ra cần việc phía backend
thì dừng và báo; `chartparams.rs` dễ dãi nhưng *truy vấn* thì có thể không."*

**Rủi ro đó không tồn tại.** Đó là phỏng đoán hợp lý và nó sai, ở cả hai nửa:

- **`chartparams.rs` chấp nhận**, đúng như kế hoạch dự đoán — dễ dãi về hình dạng `settings` theo
  thiết kế, có test khoá đúng điều đó (`accepts_settings_keys_this_build_does_not_know`).
- **Đường truy vấn cũng chấp nhận, vì KHÔNG có truy vấn thứ hai.** `computeTrend` ở thượng nguồn là
  **hàm thuần trên đúng những dòng thẻ đã lấy về**. Theo từng loại: `previousValue` lùi về DÒNG không
  rỗng liền trước; `previousPeriod` là cùng phép quét với `dateUnitsAgo = 1`; `periodsAgo` cùng phép
  quét với `dateUnitsAgo = N`; `staticNumber` chỉ đọc setting; `anotherColumn` đọc cột thứ hai của
  CÙNG dòng. Không có gì phát request. Trend của egret vốn đã lấy toàn bộ chuỗi — `dimsAndMetrics` đặt
  `row_limit` (mặc định 1000) và **không có LIMIT riêng cho trend ở đâu cả**.

Nên tiền đề của kế hoạch sai hẳn với bốn trên năm loại, và chỉ đúng với `anotherColumn` theo nghĩa yếu
là nó thêm một CỘT vào truy vấn sẵn có — đúng khuôn mẫu `goalColumn` đã dùng ở
`viz/simple.tsx:283-286`. **Không việc backend. Không đổi truy vấn. Không hình dạng truy vấn mới.**

**Tôi vẫn dừng, vì kích cỡ và khả năng review, và đó là một phán đoán chứ không phải một luật kích
hoạt.** Chi phí đã đo, để người tiếp nhận C3 bắt đầu từ con số thay vì từ phỏng đoán của kế hoạch:

| phần | chi phí |
|---|---|
| backend | **không** — `chartparams.rs` giữ nguyên |
| truy vấn / spec | **không** — các dòng đã được lấy về sẵn |
| khoá lưu mới | một, `smartscalar.comparisons`; tự động persist qua `fieldDescriptor` |
| tính toán | một module mới port từ `SmartScalar/compute.ts:341-655`. **Rủi ro ngữ nghĩa thật sự duy nhất** là guard lệch múi giờ trong `areDatesTheSame` |
| vẽ chú thích | nhỏ; trend đã in sẵn một dòng so sánh |
| **UI settings** | **phần tốn kém nhất.** Metabase dùng sáu component trên chín tệp cho bộ chọn so sánh, so với khoảng hai tệp cho toàn bộ phần số học |
| **tổng** | **5–7 tệp frontend**, phần lớn là UI |

**Tập con rẻ nhất mà vẫn trung thực**, nếu có PR tiếp theo: mô hình danh sách cộng `staticNumber` +
`previousValue` + `previousPeriod` sau một `<Select>` đơn giản mỗi dòng — khoảng bốn tệp và một module
mới, vẫn không backend và không đổi truy vấn.

**Tìm thấy khi đo C3, chưa sửa (1).** Trend của egret lấy `valid[n - 1]` và `valid[n - 2]` — dòng
không rỗng liền trước, tức `previousValue` của Metabase — rồi chú thích là "vs previous month", tức
`previousPeriod`. Đó là hai phép so sánh khác nhau ở thượng nguồn; một khoảng trống trong chuỗi làm
chú thích nói sai.

**Tìm thấy, chưa sửa (2).** Chỗ lệch đã ghi ở `CartesianDisplaySection.tsx:265-266` nay có một ngoại
lệ: `scatterXY` tôn trọng mặc định unpin nhưng không có hàng panel nào để ghi đè.

---

## Cố ý nằm ngoài phạm vi

Hai thế hệ shadcn (vòng đặc `ring-2 ring-ring ring-offset-2` so với hai phần `border-ring` +
`ring-ring/50` + `ring-[3px]`) đáng được hợp nhất, và đó là việc riêng với lượt review riêng.

Phần dọn dẹp người dùng chưa quyết: container `egret-pg-test`, và các ảnh `.png` vương trong thư mục
cha của repo.

## Tự soát

- **Năm mục bị loại khỏi danh sách này nhờ đọc lại mã**, hai trong số đó từ một lượt soát panel lái
  thật và một từ mục "found, not fixed" của chính một PR. **Một phát hiện chuyển tiếp mà không kiểm
  lại chỉ là tin đồn**, dù nguồn có cẩn thận đến đâu. Mọi mục ở trên đã được kiểm lại; hãy làm y vậy
  với bất cứ thứ gì kế hoạch này giao cho bạn.
- **Task A3 có điều kiện dừng tường minh**, vì một mẫu hình bàn phím làm nửa vời trên ba component thì
  tệ hơn không làm.
- **Task C3 cũng có một điều kiện dừng**, vì kích cỡ.
- **Chỗ yếu đã biết:** lựa chọn ở Task A1 giữa "thêm một vòng" và "làm tối `--accent`" là một quyết
  định thiết kế thật mà tôi **không** quyết thay bạn, và bán kính ảnh hưởng của hai phương án lệch nhau
  cả một bậc độ lớn. Xác định còn gì đọc `--accent` trước khi đụng vào nó.
