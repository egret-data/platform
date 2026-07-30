# Chép Metabase sang egret — quy tắc cho chart 06–19

> Viết sau khi chart 05 (pie/sunburst) tốn 54 commit, phần lớn lẽ ra tránh được. Mỗi quy tắc dưới đây
> tồn tại vì đã phải trả giá bằng việc làm lại trên chart đó. Đọc trước khi thiết kế chart tiếp theo.
>
> Bản tiếng Anh là bản chuẩn: `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`

---

## 0. Bài học một câu

**Khớp hằng số của Metabase không phải là khớp Metabase.** Ở chart 05, mọi hằng số trong
`pie/constants.ts` đều đã được xác nhận trùng khớp — tỉ lệ bán kính, độ dày viền, cỡ chữ — mà kết quả
vẫn kém hẳn, vì các hằng số thì đúng còn mọi thứ bao quanh chúng thì sai: khung chứa, loại series,
cách dẫn xuất màu nhạt, font chữ. Hằng số chỉ là 10% cuối cùng.

---

## 1. Chạy cả hai app. Dựng cùng một chart ở mỗi bên. Đo.

Bắt buộc, và là bước đã tìm ra gần như mọi lỗi thật ở chart 05.

```bash
docker run -d --name metabase-ref -p 3033:3000 metabase/metabase:latest
# đợi /api/health, rồi:
TOKEN=$(curl -s localhost:3033/api/session/properties | python3 -c 'import sys,json;print(json.load(sys.stdin)["setup-token"])')
curl -s -X POST localhost:3033/api/setup -H 'Content-Type: application/json' -d "{\"token\":\"$TOKEN\",
  \"user\":{\"first_name\":\"Ref\",\"last_name\":\"User\",\"email\":\"ref@example.com\",
  \"password\":\"Refuser12345!\",\"site_name\":\"Ref\"},\"prefs\":{\"site_name\":\"Ref\",\"allow_tracking\":false}}"
```

Dashboard **E-commerce Insights** và các question đã lưu phủ hầu hết loại chart trên sample database.
`GET /api/dashboard/1` liệt kê các card kèm `visualization_settings` — đọc phần này trước: nó cho thấy
Metabase thực sự xuất xưởng chart với những cài đặt nào.

Sau đó dựng chart tương đương trong egret trên dữ liệu tương đương, **cùng kích thước cửa sổ**, rồi so
bằng phép đo. Chỉ chụp ảnh thì bỏ sót tương phản và hình học; chỉ đo thì bỏ sót bố cục — dùng cả hai.

### Đo những gì, mỗi lần

| Hạng mục | Cách đo |
|---|---|
| Hình học | hộp bao các phần tử; với chart tròn lấy tâm hộp bao của **các cung đã vẽ** |
| Màu | fill theo từng series/tầng, và độ bão hoà |
| Tương phản | mực chữ so với nền của chính nó, tỉ lệ WCAG, đếm số dưới 4,5:1 |
| Font | `getComputedStyle(text).fontFamily` — **kiểm tra đúng font sản phẩm, không phải `sans-serif`** |
| Tương tác | chụp fill trước/sau khi hover; đếm bao nhiêu phần tử phản hồi |
| Cắt chữ | đếm nhãn chứa `…` |

### Những bẫy đo đã tạo ra năm kết luận sai ở chart 05

- `getComputedStyle` trả về **`oklch(...)`** cho token của egret. Regex `rgb` ngây thơ đọc ba số đó
  thành RGB và cho tương phản bịa. Hãy vẽ màu lên canvas 1×1 rồi đọc ngược lại.
- `getBoundingClientRect()` của nhãn **đã xoay** là hộp bao trục, to hơn chữ nhiều. Nó báo 23 cặp nhãn
  chồng nhau trên một biểu đồ nhìn hoàn toàn sạch.
- **"Lấy cột cao nhất" tìm sai tâm hình tròn** — nhiều cột gần tâm cao xấp xỉ nhau. Cách này đã bịa ra
  một độ lệch ngang 22px không hề tồn tại.
- Ô màu của legend nằm chung `<svg>` với chart và làm phồng mọi phép hợp hộp bao.
- **Tab giữ mở qua một lần deploy vẫn chạy bundle cũ**, kể cả có tham số phá cache, vì module graph đã
  nạp từ trước. Kiểm tên file băm trong `script[src]` và mở **trang mới**. Lỗi này đã tạo ra một "bước
  lùi" không có thật và đã được giao đi sửa.

**Trước khi báo lỗi, hãy kiểm chứng phương pháp đo.** Hãy chờ subagent phản biện bằng bằng chứng tốt
hơn — ở chart 05 chúng đúng hai lần còn tôi sai.

---

## 2. Chép quy tắc, không chép giá trị

Ba nhóm. Xác định mỗi thứ thuộc nhóm nào trước khi chép.

**Chép nguyên** — tỉ lệ hình học, ngưỡng, hằng số bố cục, thuật toán:
- tỉ lệ bán kính, độ dày viền, giới hạn kích thước, khoảng đệm
- ngưỡng gộp (`SLICE_THRESHOLD = 0.025`), kích thước vẽ tối thiểu (`OTHER_SLICE_MIN_PERCENTAGE = 0.005`)
- số học canh nhãn, cách dẫn xuất tint/shade
- các quy tắc thích ứng, ví dụ tính số thập phân từ dữ liệu thay vì cố định

**Chép quy tắc, thay bằng giá trị của egret** — mọi thứ gắn với thương hiệu:
- **Font chữ.** Chart của Metabase dùng Lato vì sản phẩm là Lato. Chart egret phải dùng font egret. Ở
  chart 05 chart egret **không dùng font nào cả** — nó rơi về `sans-serif` mặc định trình duyệt trong
  khi giao diện là Inter. Đặt `textStyle.fontFamily` ở gốc option. Phần tử `graphic` **không** kế thừa
  nó, phải đặt trực tiếp.
- **Màu bề mặt.** Tooltip của Metabase là màu xám đậm cứng. egret cần token, nếu không tooltip sẽ biến
  mất trong dark mode.

**Không chép** — chỗ Metabase kém hơn có thể đo được:
- **Tương phản nhãn.** Sunburst của chính Metabase hỏng chuẩn WCAG AA ở **27/53** nhãn, thấp nhất
  2,70:1. Hàm `getTextColorForBackground` của nó lệch 3× về phía trắng; chép trung thành sang bảng màu
  egret thì nó chọn trắng ở 2,86:1 trong khi màu đậm cho 6,30:1. **Bản chép đó đã lên nhánh và phải
  quay lại.** Quy tắc của egret giữ nguyên: bên nào tương phản cao hơn thì thắng, chỉ ưu tiên trắng khi
  cả hai đã vượt 4,5:1.
- **Bôi sáng theo tên.** Metabase gửi `{type:"highlight", name}`, giải theo phép quét tuyến tính duyệt
  trước. `"Other"` xuất hiện ở mọi tầng nên nó sáng nhầm node. Hãy dùng `dataIndex`.

Khi một bản chép làm mọi thứ tệ đi và đo được, hãy nói ra và giữ bản của egret.

---

## 3. Đọc cách hằng số được **dùng**, đừng đọc tên hay comment

`borderProportion: 360, // 1 degree` bị hiểu thành "khe hở một độ" và cài thành `padAngle: 1`. Thực tế
nó là **bề rộng nét vẽ**: `(π × innerSideLength) / 360`. Nét vẽ vắt lên ranh giới và hai bên chia nhau
— thành một đường kẻ mảnh. Pad góc thì cắt bớt diện tích thật và **nở theo bán kính**: khe 4px ở vòng
ngoài, băm vòng đó thành 36 nêm trắng. Một lần đọc nhầm duy nhất đó là lỗi bị phàn nàn nhiều nhất.

Thêm nữa: **Metabase có hằng số chết.** `twoRingBorderWidth` và `threeRingBorderWidth` nằm trong file
hằng số và `getBorderWidth` không hề đọc tới. Chart 05 đã chép cả hai. Hãy grep chỗ dùng của từng hằng
số trước khi nhận.

---

## 4. Kiểm khung chứa trước khi kiểm chart

Vòng của chart 05 mỏng hơn Metabase 32% trong khi tỉ lệ đã trùng khớp. Thẻ chứa bị đóng cứng
`h-[60vh]`, vứt đi 231px trong 783px được cấp — cho mọi loại chart.

Trước khi tinh chỉnh bất kỳ chart nào, hãy đo **hộp chứa** ở cả hai app. Nếu hộp của egret nhỏ hơn, đó
chính là lỗi.

Liên quan: **option builder tuyệt đối không được đoán kích thước pixel.** Chart 05 bịa ra khung tham
chiếu 300px trong khi thực tế ~530px, nên mọi nhãn bị đo với 43% chỗ ít hơn và cả 51 nhãn cụt thành
mẩu. `EChartsRenderer` vốn đã có `ResizeObserver`; prop `option` của nó giờ nhận `(size) => option`.
Hãy dùng.

---

## 5. Ưu tiên đúng loại series mà Metabase dùng

Chart 05 vẽ pie nhiều chiều bằng **N series `pie` độc lập xếp chồng**; Metabase dùng **một `sunburst`**.
Các series độc lập không biểu đạt được quan hệ cha–con, nên:
- `emphasis: { focus: "ancestor" }` là bất khả — hover chỉ đổi **5%** số lát so với **96%** của
  Metabase, mà chính cái hover đó làm cho biểu đồ phân cấp đọc được;
- phải viết tay cả đống máy móc mà một cây `children` cho không: suy góc từng lát từ tổng luỹ kế, sắp
  lại theo cha, tính phần trăm theo cha.

Chuyển sang sunburst sửa hết — nhưng đó là viết lại lõi của một nhánh đã 40 commit. **Hãy kiểm loại
series trong option builder của Metabase trước khi viết của egret.**

Các bẫy khi chuyển, tìm ra bằng đường khó:
- Component **`legend` của ECharts không làm việc với `sunburst`**. Metabase render legend bằng React
  ngoài SVG. Đưa legend ra **trước**, nếu không nó biến mất lặng lẽ.
- `percent` và `percentPrecision` **chỉ có ở `PieSeriesModel`**. Datum của sunburst chỉ mang
  `treePathInfo`. Tự tính phần trăm rồi gắn lên datum.
- Drill theo `p.seriesName` hỏng. Dùng `treePathInfo`, mà **phần tử đầu là root nhân tạo của ECharts**,
  nên chỉ số vòng là `length - 2`.
- Sunburst gán màu bảng màu **theo tên**, bỏ qua node đã có màu — một override làm trượt màu của mọi
  lát phía sau. Hãy ghim màu vòng neo một cách tường minh.
- `sort` phải **có mặt và bằng `undefined`**, nếu không ECharts gộp mặc định `'desc'` và phá mọi thứ tự
  đã đặt.

---

## 6. Tương tác chiếm phần lớn chất lượng cảm nhận

Ở chart 05, trạng thái tĩnh đã ngang hoặc hơn, mà biểu đồ vẫn bị đánh giá kém hơn. Thiếu chính là toàn
bộ phần tương tác:

| | Metabase | egret trước đó |
|---|---|---|
| phản hồi khi hover | 96% phần tử | 5% |
| tâm / lỗ | hiện lát đang trỏ | đứng yên |
| legend | làm nổi mục tương ứng, mờ phần còn lại | trơ |
| tooltip | tiêu đề, mọi anh em kèm giá trị + %, dòng đang trỏ, chân `Total` | một dòng |
| bấm | menu drill | **không gì cả** |

Mục cuối không phải lỗi của chart: `PreviewPane` chưa bao giờ truyền `onValueClick` cho
`ChartRenderer`, nên **chưa chart ECharts nào từng bấm được trong trình soạn chart** — chỉ chạy trong
dashboard. Hãy kiểm loại đường dây này ở **mối nối**, đừng tìm trong chart.

**Soát tương tác một cách tường minh cho mọi chart.** Nó vô hình với unit test và với ảnh chụp.

---

## 7. Test không thể fail, và vùng mù của jsdom

Chart 05 đã xuất xưởng vài test xanh vì lý do sai. Yêu cầu thường trực: **viết test xong thì phá thứ nó
canh, chạy lại, xác nhận fail, rồi khôi phục.** Báo cáo phép phá đó. Yêu cầu subagent làm y hệt. Cách
này đã bắt được một lỗi gán màu nhãn bằng chính màu nền của nó — nhãn tàng hình — mà assertion
`expect(a).not.toBe(b)` vẫn xanh trong cả hai trường hợp.

Hai vùng mù về cấu trúc:
- **jsdom không chạy layout.** Mọi khẳng định về kích thước, chồng lấn, vừa khít đều cần trình duyệt.
- **Mock `echarts/core` khiến test chỉ khẳng định được component *yêu cầu* gì, không bao giờ biết
  ECharts *làm* gì.** Cả hai giả thuyết trong một báo cáo lỗi sai đều nằm gọn ở đó.
  `EChartsRenderer.echarts.test.tsx` chạy **SVG renderer thật trong jsdom** và đọc kết quả vẽ ra — hãy
  mở rộng bộ khung đó thay vì khẳng định hình dạng option, mỗi khi điều cần chứng minh là *cái hiện ra*.

Cảnh giác test tự quy chiếu: test nào tính lại đúng công thức nó đang kiểm thì không chứng minh gì.
Hãy khẳng định giá trị biết độc lập (`calcChordLength(10, π) === 20` là đường kính).

---

## 8. Danh sách kiểm cho mỗi chart

- [ ] Metabase đang chạy; cùng một chart dựng ở cả hai; cùng kích thước cửa sổ
- [ ] Đã đọc `visualization_settings` của card qua `/api/dashboard/1`
- [ ] Đã đọc hết option builder của Metabase — **loại series**, và chỗ dùng của từng hằng số
- [ ] Đã đo hộp chứa ở cả hai **trước khi** đụng vào chart
- [ ] Đã đo hình học, màu, tương phản, font, cắt chữ ở cả hai
- [ ] Đã soát tương tác: hover, legend, tooltip, bấm/drill
- [ ] Mỗi hằng số đã phân loại: chép / thay / bỏ — phần bỏ phải kèm con số biện minh
- [ ] Quét tương phản toàn bảng màu × mọi tầng, cả hai theme; cái nào dưới 4,5:1 phải **báo**, không giấu
- [ ] Chart vẽ bằng font của egret, đã kiểm trên trình duyệt
- [ ] Mọi test mới đều đã mutation-check
- [ ] Kiểm trên kind bằng **trang mới**, đã xác nhận mã băm bundle
- [ ] Mọi thứ cố ý không chép đã ghi vào PR, kèm phép đo

---

## 9. Số liệu tham chiếu từ chart 05

Dùng làm mốc kiểm tra nhanh cho các chart tròn.

| | Metabase | egret (cuối) |
|---|---|---|
| hộp chứa | 735×735 | 830×661 |
| đường kính ngoài | 550 (chặn ở `maxSideLength`) | 542 |
| tỉ lệ lỗ, 3 vòng | 0,244 | 0,242 |
| độ dày vòng | 69px | 69px |
| viền lát | 1px đều, không khe góc | như vậy |
| font nhãn | 12px w700, font sản phẩm | như vậy |
| nhãn bị cắt | 24/53 | 0/51 |
| nhãn dưới 4,5:1 | **27/53, thấp nhất 2,70** | 1/51, thấp nhất 4,32 |
| phản hồi khi hover | 96% | 96% |

Một nhãn dưới chuẩn AA của egret là cái giá cố ý của việc nhận màu vòng trong đậm hơn theo Metabase;
nó được ghi lại chứ không giấu. `#7172AD` (accent thứ 8) không thể đạt 4,5:1 với bất kỳ màu mực nào ở
cả hai theme — giới hạn của bảng màu, ảnh hưởng mọi chart từ 8 series trở lên.
