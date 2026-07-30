# Thiết kế lại chart 06 — Scalar (Số lớn)

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-30
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Quy tắc đối chiếu Metabase:** `docs/vi/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Đơn vị trước:** bar `1fe27573`, line `1ee0819e`, table `2638db51`, gauge `9a8e810b`, pie `d0d4a037`, dọn nợ `01e91ab9`
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-30-chart-redesign-06-scalar.md`

## Scalar dùng để làm gì

Đặt **một** con số vào chỗ không thể bỏ sót. Nó không có trục, không có chú giải, không có mark —
con số **chính là** biểu đồ. Vì vậy mọi thứ chỉ là "trau chuốt" ở biểu đồ cột thì ở đây lại là chịu
lực: con số to bao nhiêu, có vừa khung không, đọc lướt có ra không, và có được tô màu để nói lên
điều gì không. Không còn gì khác để nhìn.

Đó là lý do chart này ít code mà nhiều lỗi. `BigNumber.tsx` dài 30 dòng và cả 30 dòng đều chạy
đúng; vấn đề nằm ở 25 dòng nó **không** có.

## Hiện trạng đã kiểm chứng ở cả hai phía, 2026-07-30

Đọc code, không dựa trí nhớ — quy tắc của chương trình sau khi dòng table, gauge và pie trong bảng
so sánh đều sai.

**egret** — `frontend/src/components/chart/BigNumber.tsx`, render từ `ChartRenderer.tsx:353`:

```tsx
<div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{text}</div>
{metric && <div className="mt-2 text-sm text-muted-foreground">{metric}</div>}
```

`ScalarConfig` là `{ metric?: string }` — đúng một trường. Phần định dạng số đã chạy tốt:
`columnSettings[metric]` mang prefix, suffix, số lẻ, tiền tệ, hệ số nhân, dấu phân cách và cờ
`compact` qua các mục hiển thị dùng chung (#336), và `formatValue` tôn trọng tất cả. Cặp
`role="img"` + một `aria-label` duy nhất là điểm thực sự hay: trình đọc màn hình đọc
"total_revenue: 25,468" thành **một** ý. **Không đụng đến những thứ đó.**

**Metabase** — `visualizations/Scalar/Scalar.tsx` và `components/ScalarValue/`. Card 39 đang chạy
thật trong dashboard mẫu ("Total orders this quarter") có `scalar.segments` với ba dải màu, nên đây
không phải góc khuất không ai dùng.

## Năm khoảng cách

### 1. Con số chỉ có một cỡ chữ cố định, ở mọi card, mãi mãi

`text-[2.5rem]`, bất kể card to nhỏ. Card lớn thì con số trôi giữa một biển khoảng trắng; card nhỏ
thì nó không co lại cho vừa. Metabase chỉnh cỡ theo khung: `findSize` đo chữ trên canvas rồi hạ dần
cỡ chữ đến khi vừa cả hai chiều.

```
bước 0.2rem, weight 700, min 1rem, max = getMaxFontSize(...)
max: (cardColWidth / GRID_WIDTH) × (7 − 2.2) + 2.2, sàn 2.2rem, trần 7rem
     …nhưng là 2.5rem nếu card hẹp hơn 240px
```

Đó là hằng số hình học — **chép nguyên** (quy tắc 2). `GRID_COLS` của egret là 24, đúng bằng
`GRID_WIDTH` của Metabase, nên đến cả tỉ lệ cũng chuyển nguyên si. Ở mức 2.5rem hiện nay, con số
của egret còn thấp hơn **sàn** của Metabase với bất kỳ card nào rộng hơn một phần tư lưới.

`findSize` kèm hai cái bẫy, cả hai lộ ra khi đọc **cách dùng** chứ không phải cái tên:

- `measureText` gắn canvas vào `document.body` và đặt `canvas.style.fontSize` theo gốc tài liệu,
  **vì `context.font` với đơn vị `rem` quy chiếu theo font-size của chính phần tử đó**. Một canvas
  không gắn vào DOM sẽ lặng lẽ đo theo 16px và phép tính vừa-khung sai ở mọi nơi người dùng đổi cỡ
  chữ gốc. Chép cả mẹo này, không chỉ chép vòng lặp.
- `height` của nó là `actualBoundingBoxAscent + actualBoundingBoxDescent` — chiều cao **glyph**,
  không phải line-height. Đo theo line-height sẽ chừa một khoảng hở thấy rõ ở đáy mọi card.

Và bề rộng đo được phải **truyền vào**, tuyệt đối không đoán — cái container 300px bịa ra ở chart 05
đã cắt cụt cả 51 nhãn. `useElementSize` đã có sẵn đúng cho việc này.

### 2. Số dài không có chỗ nào để đi

egret không có cơ chế rút gọn theo bề rộng. `ColumnFormat.compact` có tồn tại nhưng là công tắc thủ
công, bật là bật mãi, nên người dùng phải chọn giữa "3.4M vĩnh viễn" và "3,428,911 bị cắt trên card
nhỏ".

Metabase quyết định theo từng lần render, dựa trên bề rộng đo được:

```ts
COMPACT_MIN_LENGTH = 6, COMPACT_MAX_WIDTH = 250, COMPACT_WIDTH_PER_DIGIT = 25
rút gọn khi  len > 6  &&  (width < 250 || width < len × 25)
```

và — phần khiến nó an toàn — giữ lại giá trị đầy đủ, hiện trong tooltip mỗi khi giá trị hiển thị
khác giá trị thật. Con số vẫn đọc được, còn con số chính xác chỉ cách một lần rê chuột. `compact:
true` đặt tay trong column settings vẫn thắng; cơ chế này chỉ quyết định trường hợp **chưa ai
quyết**.

### 3. Bấm vào con số thì không có gì xảy ra

`ChartRenderer` nhận `onValueClick` và chuyển cho nhánh pie. Nhánh scalar không nhận. Quy tắc 6 tồn
tại vì đây chính là phần lớn khác biệt **cảm nhận được** giữa hai sản phẩm: ở chart 05, trạng thái
tĩnh đã ngang bằng rồi mà biểu đồ vẫn bị thấy là kém hơn.

Drill của scalar đơn giản nhất chương trình — một giá trị, không có chiều, nên là `drillFilters` của
một ngữ cảnh rỗng — nhưng "xem các bản ghi đằng sau con số này" lại là điều hiển nhiên nhất người ta
muốn ở một con số to như thế. Metabase còn đổi con số sang màu thương hiệu khi rê chuột lúc nó bấm
được, và đó chính là tín hiệu cho biết nó **bấm được**.

### 4. Con số không nói được nó tốt hay xấu

`scalar.segments` — `{ min, max, color, label? }[]` — tô màu giá trị theo dải nó rơi vào, và tooltip
liệt kê các dải như một chú giải.

Đây chính là mô hình của gauge (chart 04) áp lên một con số thay vì một cung tròn. **Một bản nháp
trước của spec này nói rằng có thể tái dùng nguyên kiểu dữ liệu và editor của gauge. Đọc kỹ
`GaugeSegmentsSection.tsx` và `echarts/options/gauge.ts` trước khi viết kế hoạch cho thấy điều đó
sai ở bốn điểm** — và cả bốn đều là hành vi của gauge sinh ra để thoả một ràng buộc mà scalar không
có:

| | Gauge | Scalar | Vì sao |
|---|---|---|---|
| cận | `min: number; max: number`, bắt buộc cả hai | `min: number \| null; max: number \| null` | cung tròn cần hai đầu mới có độ dài; con số thì không. `segmentIsValid(s, { allowOpenEnded: true })` — "≥ 1701 là Cao" là cách tự nhiên viết dải trên cùng |
| sửa `max` | đẩy `min` của dải kế tiếp theo | độc lập | các stop `axisLine` của ECharts là **cộng dồn**, nên khe hở ở gauge nuốt mất một vùng. Scalar chỉ kiểm tra `min <= v && v <= max` |
| thứ tự | builder sắp theo `max` | **giữ nguyên thứ tự người dùng nhập** | `segments.find` — khớp đầu tiên thắng, nên thứ tự là ngữ nghĩa. Sắp xếp sẽ đổi dải mà giá trị rơi vào |
| xoá dòng cuối | không cho (không dải thì không cung) | cho phép | Metabase truyền `canRemoveAll: true` đúng vì lẽ đó. Không dải nào là trạng thái **mặc định** |
| chưa lưu gì | ba vùng suy từ giá trị | **không tô màu** | bài học chart 04: bịa ra dải với chiều phỏng đoán là phát biểu điều sai. Ở đây cái tương ứng là không nói gì |

Vậy nên `ScalarSegment` là kiểu riêng của nó, còn phần dùng chung — một dòng (ô màu, min, max, tên,
xoá), nút thêm dải xoay vòng `SERIES_PALETTE`, và quy tắc **không bao giờ sắp xếp lại các dòng trong
lúc đang sửa**, vì một dòng nhảy ra khỏi con trỏ giữa lúc gõ chính là lý do quy tắc đó tồn tại —
được tách ra thành `SegmentRangesSection` để cả hai chart điều khiển bằng props tường minh.
`GaugeSegmentsSection` trở thành lớp bọc mỏng cung cấp bốn hành vi của gauge, nên kết quả render và
test của nó không đổi. Một editor thứ hai chép tay sẽ trôi dạt; `01e91ab9` đã tốn công dọn đúng loại
trùng lặp đó.

**Ý nghĩa của dải không được nằm hoàn toàn ở màu sắc — đây là chỗ egret tách khỏi Metabase.**
`/ui-ux-pro-max --domain ux` gắn cờ `color-not-only` ở mức **High** ("đừng truyền đạt thông tin chỉ
bằng màu"), và hướng dẫn gauge/bullet của nó nói đúng điều đó trong ngữ cảnh KPI: *"Luôn hiển thị
giá trị số + % so với mục tiêu dưới dạng chữ bên cạnh biểu đồ. Không bao giờ chỉ dựa vào vị trí
màu."* Metabase đặt nhãn dải **chỉ trong tooltip khi rê chuột**, nên một con số đỏ và một con số
xanh là không phân biệt được với người mù màu, với người dùng màn hình đơn sắc, và với bất kỳ ai
không rê chuột.

Vì vậy khi giá trị rơi vào một dải **có nhãn**, egret render nhãn đó thành chữ hiện rõ dưới con số —
một dòng chú thích nhỏ, không phải tooltip. Màu vẫn là tín hiệu nhanh; con chữ mới mang ý nghĩa. Dải
không có nhãn thì vẫn chỉ có màu, vì không có gì để nói. Việc này tốn đúng một dòng layout và là
khác biệt giữa một KPI tiếp cận được với một KPI chỉ để trang trí; nó được ghi vào PR như một điểm
**cố ý không chép**, kèm trích dẫn quy tắc.

**Ở đây độ tương phản là nghĩa vụ, không phải điểm cộng.** Màu rơi vào **nét chữ**, không phải vào
mảng nền, nên một màu do người dùng chọn có thể đẩy con số tiêu đề xuống dưới chuẩn AA so với card.
Con số in đậm và ≥1rem, tức nằm vắt ngang ranh giới "chữ lớn" của WCAG (3:1 khi ≥24px đậm, 4.5:1 khi
nhỏ hơn), nên phải quét ở cỡ **nhỏ nhất**, ở **cả hai theme** — chart 05 từng chỉ quét light mode và
để 51/51 nhãn dark mode dưới AA. Kết quả được báo cáo kèm số, không giấu; và ở đâu màu người dùng
chọn không đạt chuẩn thì editor phải nói ra thay vì lặng lẽ render.

### 5. Tên metric bị in hai lần

`{metric && <div …>{metric}</div>}` in tên cột dạng máy ngay dưới giá trị, luôn luôn. Card dashboard
vốn đã render một header từ `titleOverride || chart.name` (`DashboardCardView.tsx:55`), nên trên
dashboard cái tên xuất hiện hai lần — một lần bằng văn xuôi, một lần là `total_revenue`. Metabase
không in nó chút nào; tiêu đề card gánh việc đó, còn `scalar.label` / `scalar.sublabel` của nó là
setting ẩn chỉ dùng cho metrics viewer.

egret đi theo Metabase, giữ label thành một **setting tuỳ chọn, mặc định tắt**. Cái giá được ghi
nhận: một scalar đã lưu sẽ mất phần tên cột in dưới số. Trên dashboard thì không mất gì, vì header
đã nói rồi; trong studio thì tên chart cũng đang hiện trên màn hình. Cái được là thật — label chiếm
tới 20% chiều cao card, và bộ tự-chỉnh-cỡ dùng toàn bộ chỗ đó cho con số.

## Mô hình

```
ScalarConfig {
  metric?: string;                 // giữ nguyên
  segments?: ScalarSegment[];      // mới
  label?: string;                  // mới — tuỳ chọn, thay cho tên metric luôn-hiện
  sublabel?: string;               // mới — dòng thứ hai tuỳ chọn
}

ScalarSegment { min: number | null; max: number | null; color: string; label?: string }
```

`min` và `max` là **nullable, không phải optional** — chép theo kiểu của Metabase. Khác biệt này có
ý nghĩa: một dải bị sửa thành ô trống phải round-trip qua JSONB thành "không có cận" một cách tường
minh, còn một khoá optional biến mất thì không phân biệt được với khoá chưa từng được đặt. Dải có cả
hai đầu null sẽ khớp mọi thứ và bị loại như dữ liệu không hợp lệ, đúng theo `segmentIsValid`.

**Không có migration.** Mọi khoá mới đều là bổ sung và mọi khoá cũ giữ nguyên ý nghĩa, nên không có
0104. Đây là chart đầu tiên trong chương trình không cần migration, và điều đó đáng nói rõ: việc
label luôn-hiện biến mất là thay đổi hành vi lúc **render**, không phải thay đổi dữ liệu đã lưu, nên
không có gì để chuyển đổi và không có gì có thể chuyển đổi nửa vời. Quy tắc thứ tự deploy của chart
02 không áp dụng ở đây.

`fromParams` chuyển tiếp mọi khoá dưới tiền tố `scalar.`, nên `segments`, `label` và `sublabel`
round-trip mà không cần đổi codec — đúng tính chất đã cho segments của gauge đi qua. Editor là một
control `display` trên descriptor scalar trong `viz/simple.tsx`, cạnh chỗ pie và gauge đặt của
chúng; tab Data vẫn chỉ hỏi mỗi metric.

Và theo `01e91ab9`: nhánh scalar của `ChartRenderer` phải **spread** config của nó. Hiện nó liệt kê
tay mỗi `metric`, đúng kiểu whitelist đã làm pie mất bốn setting và cartesian mất bốn trước đó — mà
đơn vị này lại thêm ba trường vào phía kia.

## Cố ý không làm

- **Gộp nhiều scalar thành biểu đồ cột.** `scalarToBarTransform` của Metabase biến nhiều card scalar
  thành một biểu đồ cột. egret không có card đa chuỗi, nên không có gì để biến đổi.
- **Vẫn không xuất PNG.** Metabase đặt `canSavePng = false` cho Scalar. egret đi tới cùng kết quả
  một cách tình cờ mà đúng: `ChartExportMenu` chỉ mời PNG khi container có `<svg>`, mà số lớn là DOM
  thuần. Phần kiểm tra thật xác nhận mục đó **vắng mặt**, chứ không phải có-mà-hỏng — bộ nhớ còn ghi
  một lần xuất PNG tóm nhầm icon UI 16px.
- **Tự mặc định `scalar.field`.** Metabase mặc định `cols[0]` và ẩn ô chọn khi chỉ có một cột.
  Descriptor của egret bắt chọn metric tường minh, đó là khuôn mẫu chung của cả chín descriptor đơn
  giản; đổi riêng ở đây sẽ là một điểm bất nhất, không phải một điểm ngang bằng.
- **`scalar.switch_positive_negative` và `scalar.compact_primary_number`** thuộc về SmartScalar,
  chart 11. Chúng được khai báo kiểu như setting của scalar trong Metabase nhưng Scalar của nó không
  bao giờ đọc.
- **Định dạng số.** Đã ngang bằng rồi. Không đụng vào `formatValue`.

## Đo thật đã thay đổi những gì

Đo trong trình duyệt ở 1440×920, với một harness render đúng component thật ở năm cỡ card, cả hai
theme. Ba thứ đã sai mà không unit test nào có thể phát hiện.

**1. Trần cỡ chữ là 7rem trong khi Metabase là 4rem.** Metabase đọc
`max: gridSize ? getMaxFontSize(…) : 4` — giá trị dự phòng khi không có lưới là số `4` phẳng, không
phải `MAX_SIZE_LARGE`. Đọc nhầm chỗ đó khiến một con số hai chữ số hiện ở **112px trên card
300×160**, chiếm trọn từ mép này sang mép kia. Đã sửa, và bề rộng lưới của card giờ được truyền
xuống (`DashboardCardView` → `ChartRenderer` → `BigNumber`) để card dashboard co giãn trần theo phần
lưới nó chiếm, đúng như Metabase.

**2. `SERIES_PALETTE` không dùng được làm màu nét chữ của dải, và cả bảng màu đều trượt.** Một dải tô
màu chính con số, đó là bài toán khác với tô một mảng. Đo trên card sáng, **cả 8 màu series đều trượt
4.5:1 và 6/8 trượt cả mốc 3:1** (tệ nhất `#F9D45C` ở **1.44**). Dark mode thì ổn (7/8 trên 4.5) — và
đó chính là cách một lần quét chỉ-light-mode sẽ bỏ sót nó; đúng bài học của chart 05, lặp lại.

Có một giới hạn cứng cần nói rõ, vì nó chặn mọi lời hứa của bất kỳ bảng màu nào ở đây: **không một
màu đơn nào đạt 4.5:1 ở cả hai theme.** Light đòi độ sáng tương đối ≤ 0.183, dark đòi ≥ 0.274; hai
khoảng không giao nhau. Đã kiểm 27 ứng viên — cả 27 trượt một phía. Vì con số in đậm và không bao giờ
render dưới 24px trong card thật, mốc áp dụng là **3:1** của chữ lớn, và bảng
`SCALAR_BAND_PALETTE` chuyên dụng đạt mốc đó ở cả hai theme:

| | light | dark |
|---|---|---|
| trước (`SERIES_PALETTE`) | **8/8 trượt 4.5, 6/8 trượt 3.0**, tệ nhất 1.44 | 1/8 trượt 4.5, tệ nhất 3.97 |
| sau (`SCALAR_BAND_PALETTE`) | 8/8 đạt 3.0, tệ nhất **3.73** | 8/8 đạt 3.0, tệ nhất **3.60** |

Editor cũng cảnh báo, nêu rõ theme nào trượt và con số bao nhiêu, khi người dùng tự chọn một màu
không đạt — im lặng đồng nghĩa với việc ship một con số tiêu đề không đọc nổi.

**Không chép màu dải của chính Metabase.** Card mẫu 39 của nó dùng `#51528D`, đo trên card tối của
egret ra **2.48**.

**3. Lỗi tích luỹ số thực trong `findSize` đã port.** Phép `size - step` trần trụi của Metabase bị
trôi: hạ từ 7 xuống chỗ đáng lẽ là 2.4 lại ra `2.399999999999996`, rồi vào DOM thành font-size mười
sáu chữ số thập phân và không bao giờ rơi đúng lưới bước mà chính hằng số của nó mô tả. Giờ mỗi bước
đều được làm tròn. Do chính test của module này bắt được, không phải do đọc ra.

Cũng xác nhận tốt trong cùng lượt đo: con số vẽ bằng **Inter** (font sản phẩm — lỗi `sans-serif` của
chart 05 không chạm tới một chart dạng DOM), không tràn ở cả năm cỡ, rút gọn hoạt động và tooltip giữ
giá trị đầy đủ (`3,428,911` → `3.4M`), mức chặn card hẹp rơi đúng 2.5rem ở 220px, và tên dải hiện
thành chữ dưới con số.

## Kiểm tra thật

Trên bộ dữ liệu Demo Sales, đối chiếu card 39 của Metabase ở cùng một viewport:

1. Đo **host box** ở cả hai bên **trước khi** chỉnh bất cứ thứ gì (quy tắc 4).
2. Scalar trên card lớn: con số lấp đầy card, và cỡ chữ nằm trong khoảng đã chép.
3. Cũng scalar đó trên card 2×2: nó co lại chứ không bị cắt.
4. Card hẹp hơn 240px bị chặn ở 2.5rem.
5. Số 9 chữ số bị rút gọn, rê chuột hiện giá trị đầy đủ.
6. `compact: true` trong column settings vẫn rút gọn cả số ngắn.
7. Bấm vào số mở menu drill; rê chuột thì số đổi sang màu thương hiệu.
8. Ba dải như card 39 — giá trị đỏ khi dưới 1500, xanh dương đến 1700, xanh lá khi trên.
9. Dải trên cùng để mở (`≥ 1701`, không có max) tô màu đúng.
10. Rê chuột hiện chú giải các dải kèm màu và nhãn.
11. Scalar chưa cấu hình dùng màu foreground, không khác gì `main`.
12. Tên metric không còn hiện dưới con số; header card vẫn nêu tên.
13. Đặt `label` thì in ra; đặt thêm `sublabel` thì in cả hai.
14. Menu xuất của scalar có CSV và **không** có PNG.
15. Cả hai theme, cho phần quét tương phản và cho màu các dải.

## Kiểm chứng

Cổng chất lượng của chương trình: lint, typecheck, test, build, deploy lên kind, kiểm tra thật trong
một **trang mới** với hash bundle đã xác nhận, ảnh chụp trước/sau ở cả hai cỡ card. Không có
migration nên cổng backend nằm ngoài phạm vi và không có ràng buộc thứ tự deploy.

Mọi test mới đều được kiểm bằng đột biến và báo cáo phép đột biến (quy tắc 7). Hai điểm mù áp thẳng
vào chart này và cả hai cần harness trình duyệt thật thay vì jsdom: **jsdom không chạy layout**, nên
mọi khẳng định về vừa khung, cỡ chữ và rút gọn đều không chứng minh được ở đó, và `measureText` trả
về số 0 — một bộ tự-chỉnh-cỡ test dưới jsdom sẽ "pass" ở đúng cái cỡ mà vòng lặp chạm đáy. Hãy mở
rộng cách làm của `EChartsRenderer.echarts.test.tsx` — đọc kết quả đã vẽ — thay vì khẳng định
component đã yêu cầu những gì.
