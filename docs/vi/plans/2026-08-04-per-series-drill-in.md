# Drill-in cấu hình theo từng series — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-04-per-series-drill-in.md`
> là bản chuẩn; mã nguồn, số dòng và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Đưa hai cấu hình line theo series mà egret **đã vẽ đúng** ra chỗ người dùng với tới được,
và cho chúng một chỗ ở vẫn dùng được khi chart có mười series.

**Kiến trúc:** Mỗi dòng series thêm một nút mở popover chứa cấu hình hình thức của riêng series đó —
đúng hình dạng của Metabase, đo trên bản chạy thật. `SeriesLineStyleSection` thôi làm lưới N dòng và
trở thành phần thân của popover cho một series.

---

## Những gì đã đo

Một lượt điều tra trước đã **lái Metabase thật** ở `localhost:3033`:

- Tab Display của Metabase **không có control line nào cấp chart**. Shape, style, size, dots và cách xử
  lý giá trị thiếu chỉ tồn tại trong drill-in từng series. Ba key chart-wide còn lại đánh dấu
  `// DEPRECATED: moved to series settings` (`lib/settings/graph.ts:297-308`) và không có widget.
- Dòng trong danh sách series chỉ mang **danh tính**: tay kéo, ô màu, tên, nút `…`, nút ẩn. Nút `…` mở
  popover neo phải (`ChartSettingsWidgetPopover.tsx`, `miw 336px`, `mah 40rem`, cuộn dọc).
- Popover **tự cắt bớt**: series nào đang hiển thị dạng `bar` thì không hiện dòng line nào. Đo trên
  card 12 bằng cách đọc thuộc tính `hidden`.
- Danh sách cắt ở mười dòng (`graph.ts:186` `truncateAfter: 10`), phần dư nằm sau nút "N more series".

**egret là tập cha ở cấp chart và tập con ở cấp series.** Giữ mặc định cấp chart là chỗ rẽ có chủ ý và
đáng giữ — đặt shape cho chart mười band ở egret là một cú nhấp, ở Metabase là mười — và
`cartesian.ts:1035/1041` vốn đã ưu tiên giá trị của series đè lên giá trị chung.

## Lỗi mà kế hoạch này đóng lại

`api/charts.ts:62-67` khai bốn trường theo series. `cartesian.ts:1030-1075` tôn trọng cả bốn.
`settings/SeriesLineStyleSection.tsx` chào ra **hai**. `shape` và `markers` là *đã cài mà không với tới
được* — đúng họ lỗi mà `FormatSection.tsx:152-154` đặt tên cho `scale` và `number_separators`, và là họ
lỗi từng "làm pie mất bốn setting".

Chỗ thiếu này hiện được *lập luận* chứ không phải sơ suất, và lập luận đó đúng trong phạm vi của nó:
thêm Select thứ ba, thứ tư vào một danh sách không giới hạn số dòng thì tệ hơn. Kế hoạch này **gỡ tiền
đề** thay vì bác kết luận.

## Chi phí

**Không thêm key lưu trữ nào.** `shape` và `markers` đã nằm trong `seriesLine`, vốn đã là key của codec
và đã round-trip (`cartesian-codec.test.ts:99-108`). Không sửa codec, không sửa backend, không migration,
không phá tương thích — bản build cũ vốn đã vẽ đúng chart mang hai trường này.

---

## Các task

| # | nội dung |
|---|---|
| 1 | Mở ra hai trường chưa với tới được (một tệp) |
| 2 | Drill-in: nút trên mỗi dòng + popover, cắt bớt theo dạng hiển thị |
| 3 | Cắt danh sách ở mười dòng |
| 4 | Viết lại comment về chỗ rẽ `missing` cho đúng căn cứ |
| 5 | Đối chiếu live + mở PR (KHÔNG merge) |

### Task 1 — hai trường chưa với tới được

Viết test đỏ trước, xem nó đỏ đúng lý do, rồi mới nới union `field` ở `SeriesLineStyleSection.tsx:22`.
Tuỳ chọn và mặc định phải khớp thứ builder giải: shape `linear | smooth | step`, markers
`auto | on | off`. "Auto" ghi `undefined` để mặc định cấp chart có hiệu lực — handler hiện tại đã làm
đúng vậy cho `style`/`size`.

**Không tự thêm tuỳ chọn `step-after`.** `cartesian.ts:1036` map `step` sang `"middle"` của ECharts và
comment nói rõ đó là step mode sẵn có của egret, không phải của Metabase. Đổi nó nằm ngoài phạm vi.

### Task 2 — drill-in

Nút đặt ở `SeriesListSection.tsx:84`, giữa ô đổi tên và công tắc Show, `aria-label` là
`settings <tên series>`. Giữ nguyên mọi thành phần đang có trên dòng. Popover dựng trên shadcn Popover
đã có sẵn trong codebase, đầu popover là ô màu + tên series.

**Cắt bớt theo dạng hiển thị đã giải:** series đang là `bar` thì không có dòng line nào. Dùng lại
`seriesDisplayFor` (`cartesian.ts:939`), **không chép lại luật** — nếu không import được từ component
settings thì báo lại và đề xuất chỗ đặt, đừng nhân bản.

Bỏ đăng ký control `seriesLineStyle` ở `viz/cartesian.tsx:206-209`.

**Hai test sẽ đỏ theo đúng thiết kế, và đó là dấu hiệu thay đổi đã có hiệu lực:**
`viz/__tests__/cartesian.controls.test.tsx` khẳng định `advanced === true`, và
`StudioPanel.cartesianGroups.test.tsx:72-104` khẳng định các control vắng mặt cho tới khi bấm "More
options". Sửa cả hai và nói rõ trong PR vì sao mỗi cái phải đổi. **Bất kỳ test nào KHÁC bị đổi đều phải
hiểu và giải thích được** — chương trình này đã hai lần ship một suite xanh phủ lên nhánh mã đã chết.

### Task 4 — viết lại chỗ rẽ `missing`

Comment ở `cartesian.ts:1042-1045` lập luận rằng `LineSeriesStyle` không có `missing` vì "hai series
bất đồng sẽ khiến chart nói hai điều cùng lúc". **Kết luận vẫn đứng vững; tiền đề ngầm về Metabase thì
sai.** Đo được: đặt một band trong bốn thành Zero còn lại interpolate, Metabase lưu và vẽ bình thường,
không cảnh báo, không badge, legend lẫn tooltip đều không nói gì.

Viết lại thành một quyết định đứng bằng lý lẽ của chính nó: Metabase làm theo series, egret cố ý không,
và vì sao. Đừng ngụ ý Metabase cấm chuyện đó.

---

## Cố ý nằm ngoài phạm vi

`missing` theo series (Task 4 giữ chỗ rẽ), trend line theo series, show-values theo series, và vị trí
trục y theo series. Mỗi thứ cần một map cấp cao mới trong `KEYS` của codec cùng một bản sao trên
`CartesianBuildConfig` — năm tệp mỗi thứ, so với một tệp của Bucket A.

**Một trong số đó là khoảng cách năng lực thật, không phải khác cách gọi tên**, và phải ghi lại làm việc
tiếp theo chứ đừng quên: lựa chọn trục trái/phải của egret là theo **metric**
(`rightAxisMetrics`, `cartesian.ts:1078`). Dưới một breakout, mọi band dùng chung `cfg.yMetrics[0]`, nên
egret **không** tách được các band sang hai trục còn Metabase thì được.

## Tự soát

- **Task 1 gần như miễn phí, Task 2 mới là việc thật.** Task 1 đứng được một mình: nếu Task 2 hoá ra tệ
  hơn lưới hiện tại, Task 1 vẫn đã đóng lỗi setting không với tới được.
- **Backend không kiểm gì ở đây.** `crates/egret-bi/src/chartparams.rs:83-91` chỉ kiểm `params.settings`
  là object JSON, và test ở `:125-131` *ghim* sự dễ dãi đó. Nên JSON chưa kiểm có thể tới được builder —
  đó là lý do `cartesian.ts:1070` phải canh `ln.size && ln.size in LINE_WIDTHS`. Mọi control thêm vào
  đây cần phòng thủ y như vậy lúc đọc.
- **Chỗ yếu đã biết:** Task 2 giả định `seriesDisplayFor` gọi được từ component settings. Nếu không,
  đừng chép luật — một bộ giải dạng hiển thị thứ hai đúng là cách luật màu từng phân kỳ trước đây
  (`viz/cartesian.tsx:130-135` ghi lại chuyện đó).
