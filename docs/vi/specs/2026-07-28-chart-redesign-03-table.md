# Thiết kế lại chart 03 — Table

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-28
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Đơn vị trước:** bar (main `1fe27573`), line (main `1ee0819e`)
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-28-chart-redesign-03-table.md`

## Tiền đề khởi đầu của đơn vị này đã sai

Bảng so sánh của chương trình gọi table là "khoảng cách lớn nhất" và nói egret "không có phân trang,
không đóng băng cột". Đọc code thay vì nhớ lại: `TableState` đã có sẵn `pagination`, `pinnedLeft`,
`columnWidths`, `hiddenColumns` và `columnOrder`, còn `ChartTable` hiện thực cả năm, cộng header dính
khi cuộn và kéo-thả đổi thứ tự ngay trên hàng header. Spec chương trình đã được sửa.

Vậy đơn vị này không phải là lấp tính năng. Nó là **một** khoảng cách thật, một đợt rà soát UX cho
panel, và — vì chart 01 lẫn 02 đều tìm thấy lỗi thật ở chỗ ghép nối chứ không phải trong hàm thuần —
một đợt kiểm chứng thực tế cho những tính năng đã có code nhưng chưa có bằng chứng từng được ai chạy.

## Table dùng để làm gì

Đọc giá trị chính xác, và so sánh các hàng bằng cách quét mắt. Mọi thứ bên dưới suy ra từ đó: một cột
phải nhận diện được, cái panel cấu hình bốn mươi cột phải còn dùng nổi, và một bảng chưa chạy phải nói
nó cần gì thay vì xin lỗi một câu chung chung.

## Khoảng cách thật duy nhất: không đổi tên cột được

`ColumnFormat.column_title` có trong type. Chart 01 đã dạy tiêu đề trục cartesian đọc nó. **Không ai
ghi nó**, và `ChartTable` render `header: c.name` — tên cột thô từ kết quả truy vấn. Nên người dùng
nhìn thấy `total_revenue` không có cách nào bắt nó hiện `Revenue`, ở bất cứ đâu.

Đơn vị này nối dây từ đầu đến cuối:

- một ô nhập trong accordion từng cột của `FormatSection`, nơi đã có sẵn đường `patch()` cho mọi
  trường `ColumnFormat` khác;
- header của `ChartTable` đọc nó;
- header metric của pivot đọc nó.

Pivot cần cẩn thận, và spec nói rõ ở đây thay vì để plan tự vấp phải: pivot **có** nhận
`columnSettings` nhưng chỉ dùng để định dạng **giá trị** (`formatValue`). Chữ trên header metric của
nó được dựng trong `pivot/engine.ts` dưới dạng `{ label: cl.metric }` — một hàm thuần không có quyền
truy cập `column_settings`. Nên việc đổi tên phải giải quyết lúc **render** header, chứ không phải ở
chỗ dựng label. Nó vẫn nằm trong phạm vi, vì một bảng hiện `Revenue` bên cạnh một pivot vẫn hiện
`total_revenue` đúng là kiểu bất nhất người dùng nhận ra ngay.

**Quy tắc cho hai kênh đổi tên**, để chuyện này không thành mớ rối mà review chart 01 đã cảnh báo:
`column_title` đổi tên một **cột**, ở mọi nơi một cột được gọi tên. `seriesLabels` đổi tên một
**series**. Trên chart cartesian, một metric vừa là cột vừa là series, và bản đổi tên series thắng vì
nó là lựa chọn cụ thể hơn — thứ tự ưu tiên đó đã có sẵn trong `titleOf` và không đổi.

## Rà soát theo chuẩn

Đối chiếu với bảy chuẩn của chương trình, chỉ ghi chỗ table lệch:

**Chuẩn 5 — empty state phải nói chart này cần gì.** Hôm nay tab Display của một bảng chưa chạy hiện
*"Run the chart to configure column formatting."* Đó là một câu dùng chung cho mọi viz, nó nói về
formatting chứ không nói về bảng, và nó xuất hiện đúng chỗ đáng lẽ là danh sách cột.

**Chuẩn 2 — progressive disclosure.** `TableColumnsSection` là ba khối không nhóm: một checkbox mỗi
cột, một ô số "Freeze first columns", và một công tắc phân trang. Bar và line lấy phần nhóm từ
`ControlGroup` của `StudioPanel`, nhưng descriptor của table **không khai báo control display nào** —
cả tab Display của nó thuộc về shell. Nên việc nhóm phải xảy ra bên trong section dùng chung, không
phải qua descriptor.

**Chuẩn 3 — dữ liệu khó.** Danh sách cột hiển thị vẽ một checkbox mỗi cột, không có giới hạn. Kết quả
bốn mươi cột cho bốn mươi checkbox, một trăm cột cho một trăm. Panel cần chịu được kết quả rộng theo
cách chart chịu được cái đuôi dài: một ô lọc khi danh sách đã dài, cộng Show all / Hide all.

**Chuẩn 6 — cả hai kích thước.** Tất cả những điều trên, ở khổ card dashboard.

## Kiểm chứng thứ đã có, trên UI thật

Phân trang, đóng băng cột, kéo đổi thứ tự và độ rộng đã lưu đều có code và test. Không cái nào có bằng
chứng từng được chạy trong trình duyệt. Chart 01 ship `maxCategories` với unit test xanh và một
renderer không bao giờ nhận được nó; control của chart 02 nói dối về chính trạng thái của mình. Cả hai
chỉ lộ ra khi chạy thật.

Vì vậy mỗi mục dưới đây là một bài kiểm tra thật với kết quả nêu rõ, và **bất cứ cái nào hỏng sẽ thành
việc phải làm của PR này**:

- phân trang: đổi page size, Prev/Next chạy, và trang reset khi dữ liệu đổi;
- đóng băng: N cột đầu đứng yên khi cuộn ngang và có màu nền đánh dấu;
- đổi thứ tự: kéo header thì lưu lại và sống sót qua reload;
- độ rộng: kéo xong thì lưu, và cột bị xoá không để lại độ rộng cũ.

## Chủ ý không làm

- **Không đóng băng hàng.** Metabase cũng không có, và header dính đã giải quyết đúng vấn đề mà đóng
  băng hàng nhắm tới.
- **Không đổi `TableState`.** Nó lưu đúng thứ cần lưu; đây là đơn vị nối dây và UX. **Không migration.**
- **Không đụng `ChartTable`** ngoài phần đọc `column_title` và những gì kiểm tra thật chứng minh là hỏng.
- **Đổi thứ tự cột vẫn là thao tác trực tiếp** — kéo header, không phải một danh sách trong panel. Đó
  là chuẩn 4 vốn đã đạt, và thêm cách thứ hai trong panel nghĩa là hai nguồn sự thật cho một thứ tự.

## Kiểm tra thật

Trên dataset Demo Sales:

1. Bảng không cấu hình gì: đủ cột, header đọc được, không có chữ empty state.
2. Đổi tên `total_revenue` thành `Revenue` trong Formatting: header bảng đổi, và tiêu đề trục của một
   bar chart dựng trên cùng cột đó cũng đổi — một lần đổi tên, hai nơi.
3. Kết quả rộng (nhiều cột): danh sách cột vẫn dùng được; lọc và Show/Hide all chạy.
4. Bật phân trang 25: Prev/Next chạy, trang reset khi truy vấn chạy lại.
5. Đóng băng 2 cột rồi cuộn sang phải: chúng đứng yên và có màu đánh dấu.
6. Kéo header đổi thứ tự, reload: thứ tự còn nguyên.
7. Cùng bảng đó ở dạng card dashboard nhỏ.
8. Một bảng chưa chạy: empty state nói đúng thứ một bảng cần.

## Xác minh

Cổng của chương trình: lint, typecheck, test, build, deploy lên kind, kiểm tra thật, ảnh trước/sau ở
cả hai kích thước. Lần này không có migration nên cổng backend không thuộc phạm vi, trừ khi có gì đó
kéo nó vào.
