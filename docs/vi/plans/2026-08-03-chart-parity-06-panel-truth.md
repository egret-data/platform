# Chart parity PR 06 — panel nói thật — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-03-chart-parity-06-panel-truth.md` là bản chuẩn; mã nguồn và các bước chạy
> lệnh nằm ở đó.

**Mục tiêu:** Chấm dứt việc panel cấu hình nói dối. Một chart table đã lưu mở ra trống, một công tắc
ẩn không làm gì, một ô số nuốt mất số đang gõ, và danh sách lát pie xếp theo thứ tự chart không hề vẽ —
mỗi thứ đó khiến người dùng mất tin vào mọi control còn lại trên trang.

---

## Phạm vi: đây là nửa ĐÚNG/SAI của workstream 6

Spec liệt kê sáu mục. **Đối chiếu live cho thấy ba mục đã xong** và danh sách đã cũ, đồng thời lòi ra
bốn lỗi spec không hề nhắc — trong đó một lỗi khiến chart đã lưu không mở được.

**PR 06 (kế hoạch này) — panel SAI.** Control nói dối, hỏng, hoặc làm mất việc của người dùng.

**PR 06b — panel THIẾU.** Hoãn lại, có kèm bằng chứng ở cuối: Axes tab chưa nhóm X/Y, `getHidden`,
số dòng của table mặc định tắt, cấu hình line theo từng series, và thang Power của Metabase.

Ranh giới cố ý: mọi thứ ở đây là thứ người dùng sẽ gọi là **lỗi**; mọi thứ ở 06b là thứ họ gọi là
**thiếu tính năng**. Trộn lại sẽ khiến phần thẩm mỹ làm loãng việc review các bản sửa đúng/sai.

## Phán quyết cho sáu mục của spec, đo trực tiếp

| # | mục spec | phán quyết |
|---|---|---|
| 1 | Nhóm Axes tab theo X-axis / Y-axis | **còn mở** → PR 06b |
| 2 | Mở `showTotal` cho pie | **xong** — control ở `PieDisplaySection.tsx:121-129`; còn lệch mặc định |
| 3 | Percentages Off / In legend / On the chart / Both | **xong** — Auto / Off / In legend / In chart / Both; còn lệch mặc định |
| 4 | Ẩn setting không áp dụng (`getHidden`) | **còn mở** → PR 06b |
| 5 | Ô số hiện `2,5` | **một phần — và giờ tệ hơn**, xem Task 3 |
| 6 | Số dòng ở chân table | **một phần** — `ChartTable.tsx:415` có render, nhưng nằm sau `paginationEnabled` mặc định false → PR 06b |

Ghi chú của spec cho mục 1 gây hiểu lầm: `settings/ChartAxesSection.tsx` **có** heading X/Y, nhưng chỉ
waterfall và box plot dùng (`viz/simple.tsx:187,218`), và chính docstring của nó nói nó cố ý không phải
`AxesOptions` của cartesian. Live, tab Axes của MB · Bar phẳng và trộn lẫn.

## Các task

| # | nội dung |
|---|---|
| 1 | **Chart table đã lưu không được mở ra trống — CHẨN ĐOÁN TRƯỚC** |
| 2 | Hide/rename/màu của pie phải trúng đúng lát người dùng bấm |
| 3 | "Group slices below" phải nhận số người dùng gõ |
| 4 | Danh sách lát xếp đúng thứ tự chart vẽ |
| 5 | Hai mặc định lệch Metabase |
| 6 | Đối chiếu live + mở PR (KHÔNG merge) |

### Task 1 — lỗi nặng nhất, và không nằm trong danh sách nào

`viz/table.tsx:77-88`: nhánh v1 khôi phục cột từ `spec`, nhánh **v2 thì không**. Một chart lưu dạng
`{version: 2, settings: {}, spec: {columns: [...]}}` sẽ mở ra màn hình **"Set up your Table"** — trông
như mất chart.

Đo được: **ba trong bốn** chart table/object của workspace `demo` đang ở tình trạng đó — `MB · Table`,
`Customer survey responses`, `Most recent subscription`.

**Chẩn đoán trước khi sửa.** Pivot của PR 05b trông y hệt và hoá ra là **lỗi seed harness**. Câu hỏi:
ba chart này có settings rỗng vì bị seed thẳng, hay vì một đường lưu đã ghi như vậy? Và `paramsForSave()`
có thể tạo ra hình dạng đó không? **Cả hai câu trả lời đều quan trọng và dẫn tới việc khác nhau.** Nếu
chỉ các dòng seed bị ảnh hưởng thì vẫn nên sửa — chart không mở được là kiểu hỏng tệ dù ai ghi ra nó —
nhưng PR phải nói thẳng thay vì ám chỉ người dùng đang mất chart.

### Task 2 — đường nối raw/formatted của pie

`echarts/options/pie.ts:900-902` lọc `hiddenSlices` theo tên **đã định dạng**, còn panel giữ giá trị
**thô**. Tái hiện được trên UI, pie theo `order_month` với Date style `January 2024`:

- Chart vẽ `December 2026, November 2026, …`; panel liệt kê `2026-12-01, 2026-11-01, …`.
- Tắt **"Show 2026-12-01"** **không làm gì** — chart vẫn 12 lát.
- **Đối chứng**: đổi Date style sang `2024-01-31` (định dạng ra đúng chuỗi ISO thô) thì chính công tắc
  đó hoạt động — còn 11 lát.

Test phải phủ cả `sliceColors` và `sliceNames`, không chỉ `hiddenSlices` — bản sửa chỉ chữa một khoá
vẫn sẽ xanh.

### Task 3 — ô số nuốt số đang gõ

Dấu phẩy `2,5` đã hết, nhưng ô nhập **giờ tệ hơn lỗi cũ**, đo bằng gõ phím thật chứ không phải `fill()`:
xoá trắng lưu thành **0** (một setting khác hẳn: không bao giờ gộp), và gõ `4` rồi `.` làm ô sập về **0**.

Test phải dùng `userEvent.type`/`userEvent.clear`, không dùng `fireEvent.change` với giá trị cuối —
lỗi nằm ở các trạng thái trung gian, và test đặt thẳng giá trị cuối sẽ xanh trên mã hỏng.

### Task 4 — thứ tự danh sách lát

Panel xếp theo thứ tự thô `Gizmo, Widget, Gadget, Doohickey`; chart vẽ theo giá trị giảm dần
`Widget, Gadget, Gizmo, Doohickey`. Metabase xếp theo giá trị và khớp chart. Ngoài ra, khi gộp Other,
panel vẫn liệt kê các lát đã gộp với control **không còn tác dụng** và không có dòng cho "Other".

**Đây là task dễ phình nhất.** Nếu việc luồn measure vào panel biến thành thay đổi cách descriptor
nhận dữ liệu thì **dừng và báo cáo** — đó là việc của Chart Studio, không phải một bản sửa settings.

### Task 5 — hai mặc định lệch

`resolvePercentVisibility(undefined, 1)` trả `"off"`; Metabase trả `"legend"` vô điều kiện. Và Metabase
luôn hiện control `Show total` còn egret chặn theo `hasHole`.

**Đọc lý do đã ghi trước khi lật.** Cả hai đều là quyết định có chủ đích và có tài liệu; "Metabase làm
thế" tự nó không phải lý do đảo ngược một lựa chọn đã cân nhắc. Nếu lập luận không đứng vững, giữ mặc
định của egret và ghi lý do vào PR mới là kết quả đúng.

## Tiêu chí nghiệm thu (Task 6)

1. **`MB · Table` mở ra với đủ mười cột**, không phải "Set up your Table". Tương tự hai chart kia.
2. **Pie theo `order_month` với Date style `January 2024`: tắt một lát thì lát đó biến mất.**
3. **Gõ `4` rồi `.` vào "Group slices below" thì ô giữ `4.`**, và xoá trắng thì về mặc định chứ không lưu 0.
4. **Thứ tự dòng của panel bằng thứ tự chart vẽ**, và lát đã gộp không còn dòng thao tác được.
5. **Pie một vòng hiện phần trăm ở legend và có control Show total** — hoặc, nếu Task 5 lập luận giữ
   mặc định egret, thì PR nói rõ vì sao.
6. **Không thứ gì đang chạy bị hỏng** — kiểm lại các chart PR 01–05b đã sửa.

**Kiểm hash bundle trước khi tin bất kỳ phép đo live nào** — phép đo đầu của PR 05b sai vì trình duyệt
cache `index.html` và đang chạy bundle cũ.
