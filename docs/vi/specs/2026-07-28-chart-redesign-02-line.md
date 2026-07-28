# Thiết kế lại chart 02 — Line

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-28
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Đơn vị trước:** `docs/vi/specs/2026-07-28-chart-redesign-01-bar.md` (đã ship, main `1fe27573`)
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-28-chart-redesign-02-line.md`

## Line dùng để làm gì

Cho thấy một chỉ số biến động ra sao theo một chiều có thứ tự — thường là thời gian. Mọi
thứ bên dưới đều suy ra từ đó: hình dạng đường phải trung thực về chuyện gì xảy ra giữa các
điểm, khoảng trống dữ liệu không được lặng lẽ bịa ra, và khi nhiều đường cùng nằm trên một
chart thì phải phân biệt được với nhau.

Line đứng thứ hai vì đây là loại chart được dùng nhiều thứ hai, và vì nó thừa hưởng toàn bộ
những gì bar đã dựng — điều khiển theo nhóm, mặc định lúc render, nhãn rút gọn theo từng
trục, thứ tự và ẩn hiện series — mà không phải làm lại.

## Điều phải đúng trước khi cấu hình bất cứ thứ gì

Line đã có sẵn mặc định lúc render của bar qua builder cartesian dùng chung: legend tự xuất
hiện, tiêu đề trục lấy từ tên cột, số lớn tự rút gọn, khoảng cách nhãn để ECharts lo. Đơn vị
này không thêm gì ở đó.

Cái riêng của line là ba setting đang là boolean trong khi chúng mô tả nhiều hơn hai trạng thái:

| | Hôm nay | Sau khi sửa |
|---|---|---|
| Hình dạng đường | hai boolean rời `smooth` và `step` | một lựa chọn: thẳng / mượt / bậc thang |
| Giá trị thiếu | boolean `connectNulls` | nội suy / bằng 0 / để trống |
| Điểm đánh dấu | boolean `showMarkers`, luôn bật | auto / bật / tắt |
| Hình thức từng series | chỉ màu và nhãn | + hình dạng, nét, độ dày, marker |

## Ba lỗi sửa luôn trên đường đi

1. **`smooth` và `step` có thể cùng true.** Không có gì ngăn, và trạng thái đó vô nghĩa —
   ECharts vẽ ra bậc thang và bỏ qua phần làm mượt. Một lựa chọn ba trạng thái **xoá bỏ**
   trạng thái đó thay vì đi kiểm tra để chặn.

2. **Công tắc "Connect gaps" mâu thuẫn với chart.** Builder đọc `cfg.connectNulls ?? true`
   nên chart chưa cấu hình vẫn nối qua null; panel lại vẽ `checked={!!config.connectNulls}`
   nên công tắc hiện OFF trong khi đường rõ ràng đang nối liền. Đây đúng là lỗi bar đã gặp ở
   công tắc Legend và Shorten large numbers, cùng một nguyên nhân: một mặc định lúc render bị
   ép vào điều khiển hai trạng thái. Setting `missing` ba trạng thái làm mặc định đó hiện ra.

3. **Chú thích ghi ngược với code.** `showMarkers` được chú thích "off by default, like
   Metabase" trong khi builder viết `cfg.showMarkers !== false`, tức là *hiện* marker trừ khi
   bị tắt hẳn. Mặc định thật của Metabase là `auto`. Code mới là thứ chạy, nên xoá chú thích
   và đặt mặc định là auto cho chart mới.

## Các setting

Chung cả chart, trong namespace `cartesian.` dùng chung:

```
cartesian.lineShape  : "linear" | "smooth" | "step"      mặc định "linear"
cartesian.missing    : "interpolate" | "zero" | "none"   mặc định "interpolate"
cartesian.markers    : "auto" | "on" | "off"             mặc định "auto"
```

Theo từng series, ghi đè giá trị chung khi có:

```
cartesian.seriesLine : Record<tênSeries, {
                         shape?:   "linear" | "smooth" | "step"
                         style?:   "solid" | "dashed" | "dotted"
                         size?:    "S" | "M" | "L"
                         markers?: "auto" | "on" | "off"
                       }>
```

`seriesLine` theo đúng khuôn các map `seriesColors` và `seriesLabels` sẵn có, nên panel
per-series mở rộng cái đã có chứ không đẻ ra khuôn thứ hai. `style` và `size` **chỉ** có ở
per-series: trên chart một đường thì chẳng có gì để phân biệt, nên nét đứt áp cho cả chart
là trang trí chứ không phải thông tin.

**"auto" của marker nghĩa là gì.** Điểm được vẽ khi số điểm đủ ít để chúng đọc ra là điểm chứ
không thành một đường dày. Ngưỡng đó thuộc về option builder, nơi có dữ liệu, và sẽ được
chỉnh trên chart thật lúc kiểm tra thực tế — không đoán ở đây.

## Migration 0101

Chart hiện có mang `cartesian.smooth`, `cartesian.step`, `cartesian.connectNulls` và
`cartesian.showMarkers`. Migration ghi lại chúng và **phải giữ nguyên hình dạng từng chart
đang có**. Hai ca không máy móc:

- **`step` thắng `smooth`.** Hôm nay cả hai có thể cùng true, và ECharts vẽ ra bậc thang. Ánh
  xạ `smooth` trước sẽ lặng lẽ đổi hình dạng những chart đó.
- **`showMarkers` chưa đặt phải thành `"on"`, không phải `"auto"`.** Mặc định hiện tại của
  builder là hiện marker, nên để trống — hoặc ghi `"auto"` — sẽ đổi hình dạng của mọi chart
  line đang có. Chart mới nhận `"auto"`; chart cũ nhận giá trị ghi rõ khớp với thứ chúng đang vẽ.

`connectNulls` ánh xạ `false → "none"`, còn `true`/chưa đặt → `"interpolate"`. Không có trạng
thái cũ nào ánh xạ sang `"zero"`; đó là năng lực mới, theo `line.missing` của Metabase.

**Bốn key cũ bị bỏ hẳn** — migration xoá khỏi settings đã lưu, đồng thời xoá khỏi
`CartesianConfig`, `CartesianBuildConfig` và `KEYS` của codec. Để chúng lại là tái tạo đúng
trạng thái mà đơn vị này sinh ra để dẹp: hai nguồn sự thật cho một hình dạng, và chúng sẽ lệch
nhau ngay lần sửa kế tiếp. Vì `smooth` được cả area lẫn line đọc, điều này có nghĩa area và
combo bắt đầu đọc `lineShape` — đó chính là ý nghĩa thực tế của câu "chúng thừa hưởng
migration". Hình vẽ ra của chúng không đổi; chỉ đổi key mà nó lấy từ đó.

## Khoảng trống chart này lấp

Theo bảng so sánh của chương trình, phần riêng của line: `line.interpolate`, `line.missing`
(gồm cả lựa chọn `zero` mà egret chưa có gì tương đương), `line.marker_enabled`, `line.style`
và `line.size`. Cả năm lấy từ phần setting per-series của Metabase trong
`frontend/src/metabase/visualizations/lib/settings/series.ts`, đọc trực tiếp chứ không nhớ lại.

## Chủ ý không làm

- **Không đụng area và combo.** Chúng dùng chung namespace `cartesian.*` nên vẫn hưởng
  migration và các setting mới, nhưng đơn vị riêng của chúng ở sau, và PR này không đổi hành
  vi nào của chúng ngoài những gì hình dạng dùng chung tự mang lại.
- **Không có line style hay size cho cả chart** — lý do ở trên.
- **`graph.tooltip_columns`** vẫn để ngoài, như ở bar: tooltip là một đợt riêng cho mọi loại
  cartesian.
- **Trend line, goal line, trục đôi** đã là tính năng cartesian dùng chung, không mở lại ở đây.

## Kiểm tra thật

Trên dataset Demo Sales:

1. `order_date` (theo ngày) × `total_revenue`: đổi Line shape giữa thẳng, mượt, bậc thang thì
   đường đổi thấy rõ, và chỉ một cái được bật.
2. Một series có khoảng trống thật: cả ba chế độ `missing` khác nhau — nội suy bắc cầu qua,
   zero rơi xuống trục, none để đứt đoạn.
3. Marker ở auto: chấm hiện trên series ngắn và biến mất trên series dài, đúng ngưỡng builder chọn.
4. Breakout theo `category`, rồi đặt một series thành nét đứt và một series thành size L: chỉ
   những series đó đổi, và legend vẫn nhận ra chúng.
5. Mở **Revenue over time (line)** đã lưu trước thay đổi này: trông y hệt như cũ, với setting
   đã được migration ghi lại.
6. Cùng chart đó ở dạng card dashboard nhỏ: vẫn đọc được.

## Xác minh

Cổng của chương trình: lint, typecheck, test, build, deploy lên kind, kiểm tra thật, cộng ảnh
trước/sau ở cả hai kích thước. Chart 01 đã chứng minh kiểm tra thật chính là nơi lỗi đường ghép
nối lộ ra — setting không bao giờ tới renderer, điều khiển mâu thuẫn với chart — nên ở đây nó
cũng không phải tuỳ chọn.
