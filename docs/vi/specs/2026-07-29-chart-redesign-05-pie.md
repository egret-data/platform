# Thiết kế lại chart 05 — Pie

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-29
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Đơn vị trước:** bar `1fe27573`, line `1ee0819e`, table `2638db51`, gauge `9a8e810b`
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-29-chart-redesign-05-pie.md`

## Dòng pie cũng sai — dòng thứ ba liên tiếp

Bảng so sánh nói pie là dòng egret **đi trước**, Metabase chỉ hơn mỗi `pie.decimal_places`. Chart 05
được xếp thứ năm **chính vì** điều đó: một đơn vị thuần UX, không có khoảng cách nào để lấp.

Đọc cả hai phía ngày 2026-07-29:

| | egret | Metabase |
|---|---|---|
| Chiều | một — `dimension?: string` | **`pie.dimension: string[]`, tối đa ba vòng lồng nhau** (`DimensionsWidget`, `dimensions.length < 3`) |
| Đổi tên lát | — | `SliceNameWidget` |
| Ẩn / sắp xếp lát | — | `PieRowsPicker`, qua `pie.rows` |
| Số thập phân của phần trăm | — | `pie.decimal_places` |
| Donut, tổng ở tâm, ngưỡng lát, màu từng lát, hiện %, sắp xếp, legend, nhãn | có | có |

Đã kiểm ba dòng của bảng so sánh này — table, gauge, pie — và **cả ba đều sai cùng một chiều**: đánh
giá thấp Metabase, đánh giá cao egret. Bản kiểm đã đánh dấu mười lăm dòng còn lại là chưa kiểm chứng;
đây là bằng chứng thứ ba cho thấy "chưa kiểm chứng" nên được đọc là "nhiều khả năng cũng sai".

## Pie dùng để làm gì

Cho thấy một tổng thể chia ra sao. Mỗi lát một con số, và các lát cộng lại thành thứ có nghĩa. Vòng
thứ hai trả lời câu hỏi tiếp theo — *và mỗi phần đó lại chia thế nào?* — mà người đọc không phải mở
thêm một chart khác.

## Khoảng cách: chỉ một tầng chia

Pie của egret trả lời được "doanh thu theo vùng". Nó không trả lời được "doanh thu theo vùng, và
trong mỗi vùng thì theo nhóm hàng" — đúng câu người đọc hỏi ngay sau đó. Metabase làm được, tới ba tầng.

## Các setting

```
pie.dimensions   : string[]                       tối đa 3; thay cho `dimension`
pie.sliceNames   : Record<string, string>         đổi tên lát
pie.hiddenSlices : string[]                       lát bị bỏ ra
pie.sliceOrder   : string[]                       thứ tự hiển thị
pie.decimals     : number                         số chữ số thập phân của phần trăm
```

`pie.dimension` **bị bỏ**, không giữ song song — cùng lý lẽ với `smooth`/`step` của line và
`min`/`max` của gauge. Hai nguồn sự thật cho một thứ sẽ lệch nhau ngay lần sửa đầu tiên.

**Tên, thứ tự và ẩn/hiện áp cho các lát của chiều thứ nhất**, khớp với `pie.rows` của Metabase. Mở
rộng sang các vòng trong sẽ cần khoá ghép và không đáng phức tạp, với một chart mà vòng hai và vòng
ba sinh ra để **quét mắt** chứ không phải để chăm chút từng lát. Nói rõ điều đó trong panel.

**Đơn vị thời gian có sẵn miễn phí.** `dimensionUnit` vốn đã được suy ra lúc render từ
`params.spec.temporal_units[dimension]` — một map dùng chung khoá theo tên cột — nên mỗi vòng tự tra
phần của mình, không cần setting mới.

## Màu các vòng

Một pie ba vòng sẽ không đọc nổi nếu mỗi lát nhận một màu palette không liên quan. Metabase giải bằng
cách cho mỗi vòng một **biến thể độ sáng của cùng một màu họ** — trong đậm, ngoài nhạt
(`getRingColorAlias`).

Palette của egret là OKLCH, đúng không gian màu cho việc này: chỉnh L, giữ C và H, thì màu họ vẫn nhận
ra được còn độ sâu đọc được ngay. Vậy lát vòng ngoài thừa hưởng màu của lát cha, làm nhạt dần theo
vòng. Màu do người dùng đặt riêng cho một lát vẫn thắng.

## `fieldDescriptor` có thêm kind `dimensions`

Pie khai báo qua `fieldDescriptor`, mà `fields` chỉ hỗ trợ bốn kind vô hướng. Một multi-select có thứ
tự tối đa N cột không thuộc kind nào, và khác gauge, nó **không thể** là control `display` — chiều là
**truy vấn**, nên nó thuộc section Data, nơi chỉ `fields` với tới.

Vậy `fields` có thêm kind `dimensions`: danh sách chọn cột có thứ tự kèm giới hạn. Đó là một mở rộng
thật, nhưng hẹp và đồng nhất — một danh sách tên cột, không phải bản ghi có cấu trúc — và đúng thứ mà
bất kỳ viz nào sau này muốn nhiều chiều có thứ tự cũng sẽ cần.

## Migration 0103

`pie.dimension` thành `pie.dimensions: [<giá trị đó>]`, và khoá cũ bị xoá. Hai nơi lưu như mọi lần:
`charts.params.settings` và `dashboard_cards.options.vizOverride`, với override chỉ ghi ở nơi nó thật
sự mang khoá cũ.

Pie chưa lưu chiều nào thì không nhận gì — không có giá trị nào để nâng lên.

**Migration 0102 đã dạy hai điều mà bản này không được học lại:** gate bằng
`jsonb_typeof(...) = 'string'` chứ không bằng sự tồn tại của khoá, vì một khoá có mặt với giá trị JSON
`null` không phải là một giá trị; và đừng để một hàng hỏng làm chết câu lệnh cho cả bảng.

## Chủ ý không làm

- **Không có vòng thứ tư.** Metabase dừng ở ba và bản này cũng vậy; quá ba vòng thì pie chỉ là một
  treemap mặc sai hình.
- **Không chăm chút lát theo từng vòng** — lý do ở trên.
- **Không đổi hành vi click kiểu sunburst.** Click một lát hiện đã drill-through; đổi ý nghĩa đó trên
  pie nhiều vòng là một quyết định riêng.
- **Donut, ngưỡng lát, sắp xếp, hiện % giữ nguyên setting.** Chúng chạy tốt, và bản kiểm đúng ở chỗ
  egret đang đi trước. Tổng ở tâm cũng giữ setting, nhưng không giữ kiểu chữ — xem mục dưới.

## Pie một vòng không còn giống bản trên `main`

Spec này ban đầu hứa rằng pie một chiều sẽ render y hệt như trước. **Lời hứa đó đã bị rút lại**, một
cách có chủ ý, ở `935e74d0`: vòng đơn được cho luôn hình học của Metabase thay vì bị rào ra ngoài.
Rào ra ngoài sẽ để egret có hai loại pie khác nhau — một loại lồng vòng đọc giống Metabase và một
loại phẳng thì không — chỉ vì chính lời hứa đó.

Trước nhánh này, pie một vòng vẽ theo mặc định của ECharts: nhãn nằm **ngoài** hình tròn, mỗi nhãn
nối về lát của nó bằng một đường dẫn, cỡ chữ và độ đậm mặc định; các lát chạm nhau, không có khe; và
khi bật `Donut` cùng `Show total` thì có một con số đậm nằm trên dòng chú thích "Total" 13px không
đậm.

Bây giờ:

- **Nhãn nằm bên trong lát**, 14px, weight 700, lấy một trong hai màu chữ của theme — màu nào thật
  sự đọc được trên nền của chính lát đó. Không còn đường dẫn. Nhãn mà lát không chứa nổi sẽ bị cắt
  kèm dấu ba chấm, hoặc bỏ hẳn nếu phần cắt còn dưới ba ký tự thật.
- **Một khe 1° tách hai lát cạnh nhau**, nên ranh giới đọc được mà không cần viền.
- **Chú thích ở tâm to và đậm hơn**: con số vẫn 22px, còn chữ "Total" từ 13px thường lên 14px
  weight 700.
- **Legend và nhãn lát mặc định bật.** Cả hai là Auto khi chưa đặt, và Auto nghĩa là hiện — màu của
  một pie không nói lên điều gì nếu thiếu chú giải. Giá trị `false` đã lưu tường minh vẫn tắt được.
- **Bán kính là thứ duy nhất không đổi.** `Donut` tắt vẫn là hình tròn đặc; `Donut` bật vẫn mở đúng
  cái lỗ cũ. Tỉ lệ một vòng của Metabase được chủ ý không áp, vì tỉ lệ của một pie phẳng chưa bao giờ
  là chỗ khó đọc.

**Vậy nên mọi pie một vòng đã lưu đều đổi diện mạo sau khi nâng cấp.** Không có gì lưu khác đi và
không cần migration cho việc này: đây hoàn toàn là thay đổi lúc render, nên nó chạm tới mọi pie đã
lưu, và không thể hoàn tác riêng cho một chart — muốn hoàn tác thì phải sửa code.

## Kiểm tra thật

Trên dataset Demo Sales:

1. Pie một chiều vẽ ra **một** vòng trên đúng các lát như `main`, cùng bán kính — nhưng với nhãn nằm
   trong, khe 1° và chú thích ở tâm to hơn như mô tả ở trên. Thứ migration thật sự được kiểm là vẫn
   một vòng trên đúng các lát đó, chứ không phải từng pixel giống nhau.
2. Thêm `category` làm chiều thứ hai thì vẽ ra vòng thứ hai, các lát nằm lồng trong vùng của nó.
3. Màu vòng ngoài là thành viên nhạt hơn, nhận ra được, của họ màu lát trong.
4. Chiều thứ ba chạy được; gỡ về một chiều chạy được.
5. Đổi tên một lát thì đổi cả trên chart lẫn legend.
6. Ẩn một lát thì nó biến mất và phần trăm còn lại tính lại theo cơ sở mới.
7. Đổi số thập phân thì nhãn đổi theo.
8. Cùng pie đó ở dạng card dashboard nhỏ, với hai vòng.

## Xác minh

Cổng của chương trình: lint, typecheck, test, build, deploy lên kind, kiểm tra thật, ảnh trước/sau ở
cả hai kích thước. Có migration nên cổng backend nằm trong phạm vi và backend roll trước.

**Như gauge, migration cần một fixture chưa tồn tại** — kiểm tra database kind có pie nào lưu
`pie.dimension` không, nếu không thì tạo một cái trên `main` trước khi deploy nhánh. Một migration chỉ
được kiểm trên bảng rỗng là một migration chưa được kiểm.
