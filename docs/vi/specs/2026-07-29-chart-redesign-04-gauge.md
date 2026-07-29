# Thiết kế lại chart 04 — Gauge

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-29
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Đơn vị trước:** bar `1fe27573`, line `1ee0819e`, table `2638db51`
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-29-chart-redesign-04-gauge.md`

## Vì sao gauge chen lên trước hàng

Thứ tự theo mức sử dụng xếp gauge thứ mười ba. Bản kiểm đưa nó lên thứ tư, và đọc kỹ renderer thì
mức độ nghiêm trọng còn tăng thêm một bậc nữa. Cả hai phía đều đã được kiểm chứng ngày 2026-07-29.

## Gauge dùng để làm gì

Nói xem **một** con số có đang ở chỗ nó nên ở hay không. Không phải con số đó là bao nhiêu — scalar
làm việc đó tốt hơn — mà nó có **tốt** hay không. Các dải màu chính là toàn bộ lý do để chọn gauge,
và mọi thứ bên dưới suy ra từ đó.

## Lỗi: các dải bị đóng cứng, và chiều của chúng chỉ là phỏng đoán

`echarts/options/gauge.ts` vẽ ba dải:

```ts
axisLine: { lineStyle: { width: 18, color: [[1 / 3, ZONE_LOW], [2 / 3, ZONE_MID], [1, ZONE_HIGH]] } },
```

Cứng ở đúng một phần ba của `[min, max]`. Màu cứng. Và **chiều** cứng — thấp là đỏ, cao là xanh.

Điều cuối không phải là thiếu tính năng, nó là một **câu trả lời sai**. Một gauge đo độ trễ p99, tỉ
lệ lỗi, chi phí hay tỉ lệ rời bỏ sẽ được tô xanh đúng ở đầu mà tin xấu nhất nằm. Chart phát biểu
ngược sự thật, một cách chắc chắn, và không có cách nào sửa. Một tín hiệu thiếu khiến người đọc đi
tìm chỗ khác; một tín hiệu sai khiến họ **hành động**.

Mọi thứ còn lại của gauge đều ổn: giá trị, tự chọn thang 2× khi không đặt max, định dạng số, và mô
tả aria nêu rõ giá trị cùng thang đo.

## Mô hình: dải trước, thang suy ra sau

`gauge.segments` của Metabase là mảng `{ min, max, color, label }`, còn `gauge.range` được **tính**
từ đó — min nhỏ nhất và max lớn nhất. Dải màu *là* mô hình, thang đo rơi ra từ nó. egret đang làm
ngược: đặt thang bằng tay rồi chia ba phần bằng nhau, bất kể ba phần đó có nghĩa gì.

egret theo mô hình của Metabase:

```
gauge.segments : Array<{ min: number; max: number; color: string; label?: string }>
```

và **`gauge.min` cùng `gauge.max` bị bỏ**. Giữ lại nghĩa là hai nguồn sự thật cho một thang đo, và
chúng sẽ lệch nhau ngay lần sửa đầu tiên — đúng cái bẫy chart 02 vừa dọn khỏi phần settings của line.

`label` đáng giữ: một dải tên là "Healthy" cho người đọc biết màu đó nghĩa gì mà không cần legend —
thứ mà một chart chỉ có một con số không có chỗ để chứa.

### Gauge chưa cấu hình thì sao

Không có gì được lưu cho tới khi người dùng cấu hình, nên mặc định được suy ra **lúc render** — quy
tắc chương trình chọn từ chart 01, và là lý do một chart lưu từ nhiều tháng trước vẫn nhận được mặc
định mới. Khi không có segment nào, builder tạo ra đúng ba dải bằng nhau trên đúng thang tự động của
hôm nay, nên **một gauge chưa cấu hình render y hệt hiện tại**. Cái sai không bị sửa bằng mệnh lệnh;
nó trở nên **sửa được**, và chỉ còn sai ở nơi chưa ai nói khác đi.

## Migration 0102

Chỉ những chart đã lưu `gauge.min` hoặc `gauge.max` mới có gì để chuyển. Chúng thành ba segment tại
đúng các mốc một phần ba đang được vẽ hôm nay, với đúng màu hôm nay, nên hình dạng giữ nguyên tuyệt
đối. Chart không lưu cái nào thì **không** nhận segment nào và rơi vào mặc định lúc render — ghi
segment cho chúng sẽ đóng băng một thang tự động mà hiện đang bám theo dữ liệu.

Cả hai key cũ bị xoá.

## Ô sửa dải nằm ở đâu — đã sửa trước khi viết plan

Spec này ban đầu đề xuất thêm kind `custom` cho `fieldDescriptor`, với lý lẽ rằng bốn kind vô hướng
của nó — dimension, metric, number, boolean — không diễn tả nổi một danh sách lặp
`{min, max, color, label}`.

**Điều đó là thừa, và chính việc đọc `viz/fields.tsx` trước khi viết plan đã bắt được.**
`FieldDescriptorSpec` **đã có** `display?: VizControl<Cfg>[]` — control tuỳ ý với hàm render riêng —
và **pie đang dùng nó** (`display: pieDisplay`) cho đúng loại việc này. Chú thích ngay trong file nói
thẳng: *"a viz's display settings (pie's donut, legend, slice colours…) are edited by its display
controls"*. `fromParams` cũng đọc **mọi key dưới prefix** của viz chứ không chỉ các trường đã khai
báo, nên `gauge.segments` tự round-trip mà không cần đụng codec.

Vậy ô sửa dải là một control `display` trên descriptor của gauge, `fields.tsx` **không** bị đụng, và
không phát minh lối thoát nào. Đó cũng là chỗ đúng hơn về bản chất: các dải màu là **trình bày**,
không phải hình dạng truy vấn, và tab Data nên tiếp tục chỉ hỏi metric.

Đây là lỗi cấp spec thứ ba mà quy tắc "đọc code trước khi thiết kế dựa trên nó" của chương trình bắt
được — sau dòng table và dòng gauge. Quy tắc này đang chứng minh giá trị của nó.

## Chủ ý không làm

- **Không giới hạn số dải và không tự chọn màu.** Người dùng thêm dải thứ tư là đang làm việc hợp lý;
  đoán màu thay họ chính là cách cái lỗi hiện tại bắt đầu.
- **Không có công tắc "đảo chiều".** Khi dải đã sửa được thì đảo chiều chính là việc của các dải; một
  cờ riêng sẽ là cách thứ hai để nói cùng một điều.
- **Kim chỉ, thang tự động và định dạng số giữ nguyên.** Chúng chạy tốt.
- **Progress và waterfall giữ nguyên màu.** Chúng sẽ dùng chính lối thoát này ở đơn vị riêng của mình.

## Kiểm tra thật

Trên dataset Demo Sales:

1. Gauge chưa cấu hình render **y hệt** bản trên `main` — cùng dải, cùng thang.
2. Gauge lưu từ trước, có đặt min và max, trông giống hệt sau migration.
3. Sửa biên của một dải thì ranh giới màu trên cung dịch theo.
4. Đặt dải trên cùng màu đỏ và dải dưới cùng màu xanh — ca "thấp mới là tốt" — render đúng như vậy;
   đây chính là lỗi mà đơn vị này sinh ra để sửa.
5. Đặt tên cho một dải thì tên hiện ra.
6. Thêm dải thứ tư chạy được; xoá xuống còn một dải chạy được.
7. Cùng gauge đó ở dạng card dashboard nhỏ.

## Xác minh

Cổng của chương trình: lint, typecheck, test, build, deploy lên kind, kiểm tra thật, ảnh trước/sau ở
cả hai kích thước. Có migration nên cổng backend nằm trong phạm vi, và **backend phải roll trước
frontend** — chart 02 đã cho thấy vì sao.
