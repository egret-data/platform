# Panel cấu hình chart: đưa hình học về ngang Metabase

> Bản tiếng Việt tham chiếu. Bản tiếng Anh tại
> `docs/en/specs/2026-08-10-settings-panel-geometry-design.md` là bản chuẩn — khi hai bản lệch
> nhau, lấy bản tiếng Anh.

**Trạng thái:** đã duyệt 2026-08-10
**Chương trình:** tiếp nối đợt chart-parity và dashboard-parity

## 1. Vấn đề

Đợt chart-parity đã đưa panel cấu hình về ngang Metabase ở mức *có những control nào*. Nó không
đụng tới *cách sắp đặt chúng*. Một lượt quét trực tiếp cả 19 loại chart ở hai phía — egret tại
`localhost:30080`, Metabase OSS tại `localhost:3033`, cùng bộ card "MB · …" — đã đo mọi control
trong mọi panel Display và Axes.

| | Metabase | egret |
|---|---|---|
| Số bề rộng khác nhau (19 panel Display) | **37** | **85** |
| Số chiều cao khác nhau | **34** | **67** |

Con số 37 của Metabase còn bị thổi lên: các bộ `104 / 112 / 137` lặp lại chỉ là **một** control
phân đoạn (`Auto | Compact | Full`) mà ba nút tự co theo chữ. Đếm theo *component* thay vì theo
*hộp*, Metabase dùng **một** bề rộng nội dung (350–352 px) và **một** chiều cao (40 px), cộng một
switch canh phải 40×21. egret không có quy ước nào như vậy.

Tệ nhất:

| chart | bề rộng | chiều cao | ví dụ |
|---|---|---|---|
| line, combo | 8 | 5 | có cả `55` lẫn `253` trong một panel |
| bar, row, area | 7 | 5 | |
| pie | 7 | 4 | |
| scatter, gauge, table | 6 | 5 | |

Mọi tab Axes của họ cartesian đều lặp đúng một cặp: **`X axis scale` rộng 112 px nằm ngay trên
`Show lines and tick marks` rộng 255 px.** Cả hai đều là select, cùng nhóm X-axis. Bên Metabase cả
hai đều 352.

### Control nhỏ tới mức khó bấm

Có mặt ở **10/19** chart:

- Mũi tên sắp xếp `8×10 px` — bar, row, line, area, combo, scatter, và **table** (22 nút, mỗi cột 2)
- `9×16 px` ở pie (8 nút)
- `18×18` xoá range ở gauge, `16×16` ở boxplot
- `94×16` "More options", `47×16` "Show all", `42×16` "Hide all", `134×16` "Add or remove columns"

Metabase dùng nút icon 22×22 cho việc tương đương. `8×10` nhỏ hơn khoảng sáu lần về diện tích, và
đây là những control người dùng bấm đi bấm lại khi sắp xếp một chart.

## 2. Nguyên nhân gốc — và vì sao đếm được

Đây không phải 19 vấn đề riêng của 19 chart. Là bốn lỗi dùng chung:

**2.1 Chiều cao: 92 lần ghi đè tại call site.** `components/ui/select.tsx` mặc định
`data-[size=default]:h-9` (36 px), `components/ui/input.tsx` mặc định `h-9`. Không nơi nào trong
thư mục settings dùng mặc định đó. Thay vào đó call site ghi đè **92 lần**: `h-8` ×58 (32 px) và
`h-7` ×34 (28 px). Ba chiều cao đo được — 28, 32, 36 — chính là đây.

**2.2 Bề rộng: 22 lần ghi đè cố định.** `w-24` ×8 (96 px), `w-16` ×6 (64 px), `w-28` ×5 (112 px),
cộng `w-72`, `w-64`, `w-56`, `w-[18px]`. Các số 64/96/112 đo được chính là đây.

**2.3 Nút không có kích thước.** Control sắp xếp là ký tự `▲`/`▼` trần ở `text-[10px]`, không có
hộp (`SeriesListSection.tsx:134`, và cùng khuôn ở `TableColumnsSection.tsx`). Kích thước của nó là
kích thước ngẫu nhiên của glyph — nên ra 8×10.

**2.4 Panel quá hẹp để chứa hình học của Metabase.** `ChartExplore.tsx:633` đặt `w-72` (288 px) với
`p-4`, còn 256 px nội dung. Của Metabase là 400 px với 352 nội dung. Nhãn dài như "Show lines and
tick marks" không vừa ở 255 — và **đó chính là lý do** các biến thể hẹp ra đời. Chuẩn hoá control
mà không nới panel là đổi lỗi này lấy lỗi khác.

## 3. Đích đến

Theo đúng hình học của Metabase, theo quyết định ngày 2026-08-10.

| | giá trị | cách làm |
|---|---|---|
| Panel | **400 px** | `w-[400px] p-6` tại `ChartExplore.tsx:633` — 400 − 2×24 = **352** nội dung |
| Control full-width | **352×40** | primitive mặc định như vậy; call site thôi ghi đè |
| Nút icon | **≥24×24** | `IconButton` dùng chung, thay cho glyph trần |
| Switch | **40×21, canh phải** | đã gần đúng; giữ nguyên |

Hai hình dạng trong cả panel, như Metabase. `352` là **hệ quả** của bề rộng panel và padding, không
phải con số để gõ cứng ở call site — gõ cứng chính là thứ tạo ra mớ đang sửa.

### Cũng nằm trong phạm vi (đã duyệt)

1. **Nút icon ≥24 px** — §2.3.
2. **`Group order_month by`** — nhãn lặp tên cột đã hiện ngay trên, control lại là cái 112 px lạc
   lõng. Gộp bucket vào chính control cột như Metabase (`Created At: Month`): một control, không
   còn dòng thứ hai.
3. **Bỏ VIẾT HOA section label.** (Phần "mép lệch 1 px" ban đầu đi kèm mục này hoá ra không tồn
   tại — xem §6.)
4. **Bỏ `More options (2)`** — hiện đủ như Metabase. Một disclosure có đếm số báo rằng đang giấu
   thứ gì đó mà không nói là thứ gì.

## 4. Không làm

- **Không** mở lại chuyện có những setting nào. Danh mục control đã ngang bằng (19 = 19; loại thứ
  20 của Metabase là `list`, `hidden: true`, người dùng không chọn được).
- **Không** đụng panel cấu hình card của dashboard. Bề mặt khác, cần lượt quét riêng.
- **Không** đụng UI cấu hình cột của table/pivot ngoài phần primitive dùng chung. Lượt quét không
  đo được phần tương đương bên Metabase (§6), nên chưa có đích để nhắm.

## 5. Cách kiểm — và một thứ không kiểm được theo lối thường

**jsdom không kiểm được bất kỳ điều gì ở đây.** Nó không chạy layout: mọi
`getBoundingClientRect()` trả về số 0. Một test jsdom khẳng định "select rộng 352" sẽ xanh với mọi
bản cài đặt, kể cả bản chưa sửa gì. Viết nó ra là tạo thêm đúng loại assertion xanh-mà-rỗng đã cắn
codebase này sáu lần.

Nên chốt phải đặt ở nơi lỗi thật sự sống — **trong source**:

```ts
it("no call site overrides the primitive control geometry", () => {
  const offenders = filesIn("components/chart/settings")
    .flatMap(f => matches(f, /\b(h-7|h-8|w-16|w-24|w-28)\b/g));
  expect(offenders).toEqual([]);
});
```

Cái này mutation-check được bình thường: thêm lại một `h-8` là đỏ. Nó ghim đúng thứ sẽ tái phát —
một người sau này với tay lấy `h-8` cho vừa chỗ.

Còn hình học thì xác nhận **một lần, trực tiếp**, bằng cách chạy lại lượt quét §1 và yêu cầu số bề
rộng khác nhau của egret giảm từ 85 xuống ≤ 25, chiều cao từ 67 xuống ≤ 25. Phép đo đó là bằng
chứng cho PR, không phải cổng CI — nó cần hai ứng dụng đang chạy.

## 6. Những phát hiện KHÔNG sống sót qua kiểm chứng

Ghi lại để lần sau không "phát hiện" lại:

**Control 830 px ở pie và 609 px ở funnel không tồn tại.** Lượt quét đầu đặt biên panel ở
`x < 760`, hút cả vùng canvas vào; đó là mục chú giải của pie và các bước của funnel, vốn bấm được
thật. Chạy lại với biên lấy từ chính hộp của tablist làm đổi 3/19 chart (pie, funnel, object) và
giữ nguyên 16 chart còn lại. Mọi con số trong tài liệu này là của lượt đã sửa.

**Mép lệch 1px không phải lỗi.** Hàng FORMATTING đo được `253 @ x=265` so với `255 @ x=264` ở
mọi chỗ khác, nhìn như lệch. Đo cái bọc thay vì đo cái nút thì rõ: card có viền tại
`FormatSection.tsx:80` chính nó là **`255 @ x=264`**, thẳng hàng với mọi anh em, còn 253 là nút nằm
trong viền 1px của card — render đúng. Không có gì để sửa. Ghi lại vì con số thô trông như lỗi, và
sẽ còn trông như vậy lần sau.

**Con số 0 của `table` và `pivot` bên Metabase không có nghĩa Metabase không có gì ở đó.** Selector
không với tới UI cấu hình cột của họ. Hai ô đó không dùng để so sánh được — nên §4 loại bề mặt này
khỏi đích đến.

**`map`, `scalar`, `gauge`, `pivot`, `object` bên Metabase không có tab Display riêng** — cấu hình
của chúng là một khối. Số đo là của khối đang mở, nên với năm loại này không thể kết luận "egret
nhiều bề rộng hơn Metabase".

## 7. Chỗ egret vốn đã hơn

Đừng làm mất trong lúc copy Metabase:

- **Sort, Row limit, Max categories, Tooltip metrics nằm ngay trong panel.** Bên Metabase, sắp xếp
  và giới hạn dòng phải sửa query trong notebook editor. Giữ nguyên.
- **Đổi bucket thời gian trên kết quả SQL.** Metabase không làm được với câu hỏi SQL — phải sửa
  SQL. Mục 3.2 chỉ đổi *cách trình bày*, không bao giờ đổi việc control đó tồn tại.
- **Tab có địa chỉ URL** (`?tab=display`). Trạng thái sidebar của Metabase thì không.
- **Một hàng series gộp đủ màu, đổi tên, kiểu vẽ và ẩn/hiện.**
