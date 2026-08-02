# Chart parity PR 04 — gán màu — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-02-chart-parity-04-colour-assignment.md` là bản chuẩn; mã nguồn, chữ ký hàm
> và các bước chạy lệnh nằm ở đó và không lặp lại đầy đủ tại đây.

**Mục tiêu:** Gán màu series từ hash của TÊN series, giống Metabase, để một cột giữ nguyên màu trên
mọi card của cùng một dashboard, và dashboard thôi trông đơn sắc.

**Kiến trúc:** Một module mới `echarts/keyColors.ts` xuất `colorsForKeys(keys, existing?)` —
clean-room của `ui/colors/charts.ts` bên Metabase. Mọi option builder và mọi panel cấu hình có ô màu
mặc định đều gọi nó. Bảng màu hai bên vốn đã giống hệt nhau, nên PR này chỉ đổi *cách gán*.

---

## Số đo trước khi sửa

Hai dashboard, cùng dữ liệu, đọc từ DOM (`fill` trên `path`/`rect`/`circle`, bỏ phần tử dưới 3px).

| chart | egret | Metabase |
|---|---|---|
| bar | `#689735` ×4 | `#f9d45c` ×4 |
| row | `#689735` ×4 | `#f9d45c` ×4 |
| line | `#689735` | `#f9d45c` |
| area | `#689735` ×2 | `#f9d45c` |
| scatter | `#689735` ×168 | `#ef8c8c` ×196 |
| pie | `#509ee3` `#88bf4d` `#a989c5` `#ef8c8c` | `#f2a86f` `#f9d45c` `#a989c5` `#88bf4d` |
| sankey (9 nút) | `#509ee3` ×10, `#f9d45c`, `#f2a86f`, `#7172ad`, `#88bf4d` | `#509ee3`, `#227fd2`, `#88bf4d`, `#689636`, `#a989c5` |
| waterfall | `#88bf4d` ×12 | `#88bf4d` ×13 |
| box plot | `#509ee3` | `#88bf4d` |

Ba kết luận, và điều đầu tiên **mâu thuẫn với cả spec lẫn hai comment trong mã**:

1. **Metabase KHÔNG tô series đơn bằng xanh summarize.** `options/cartesian.ts:591` và `:730` đều
   khẳng định như vậy, và spec thiết kế nhắc lại. Đo thật thì Metabase vẽ chính những card series
   đơn đó bằng `#f9d45c` và `#ef8c8c` — hash từ tên cột. `#689735` là `palm[50]`, màu `success` của
   Metabase, trả về từ `getPreferredColor` cho các tên như `profit`, `completed`. Đó không phải quy
   tắc dành cho series đơn. **Hai comment kia phải được sửa, không chỉ sửa code.**
2. **Lỗi `i % 8` quan sát được ngay trong harness.** Sankey của egret có 9 nút và cấp `#509ee3` cho
   hai nút. Nút thứ chín bên Metabase nhận `#227fd2` — `accent0-dark` — từ phần mở rộng harmony.
   Vậy sankey chính là ca kiểm thử sống cho nhánh `> 8`.
3. **Waterfall vốn đã khớp** và phải giữ nguyên: xanh/đỏ của nó mang nghĩa tăng/giảm, không phải ô
   bảng màu. Gauge cũng vậy.

## Thuật toán của Metabase

`getColorsForValues(keys, existingMapping)`:

- `keys.length <= 8` → **hash-based** trên tám accent chính.
- ngược lại → **order-based** trên danh sách harmony đan xen.

**Hash-based**, ba lượt trên `[...keys].sort()`:
1. Key có sẵn trong `existingMapping` lấy màu đó, và màu đó bị **đánh dấu đã dùng** — một lựa chọn
   thủ công CHIẾM một ô.
2. Key còn lại lấy màu ngữ nghĩa nếu màu đó chưa bị dùng.
3. Key còn lại: `values[(hash(key) + attempt) % values.length]`, chỉ nhận nếu chưa dùng. Khi hết màu
   chưa dùng thì nạp lại toàn bộ, nên vòng lặp kết thúc bằng màu trùng chứ không treo.

**Order-based** cũng ba lượt như vậy nhưng theo thứ tự key đã cho và không hash.

**Hash**: `h = (Math.imul(31, h) + s.charCodeAt(i)) | 0`, rồi `Math.abs(h)`.

**Màu ngữ nghĩa** (`getPreferredColor`), so khớp theo chữ thường:

| tên | màu |
|---|---|
| `success` `succeeded` `pass` `passed` `valid` `complete` `completed` `accepted` `active` `profit` | `#689735` |
| `cancel` `canceled` `cancelled` `error` `fail` `failed` `failure` `failures` `invalid` `rejected` `inactive` `loss` `cost` `deleted` `pending` | `#e3595e` |
| `warn` `warning` `incomplete` `unstable` | `#e8b921` |
| `count` / `sum` / `average` | accent0 / accent1 / accent2 |

Ba mã hex chuyển từ `hsla()` gốc của Metabase. `palm[50]` = `hsla(89, 48%, 40%)` = `#689735`, trùng
từng byte với `SUMMARIZE_COLOR` sẵn có của egret — chính sự trùng khớp đó là phép kiểm chéo cho việc
chuyển đổi, và Task 1 khẳng định nó bằng test.

**Danh sách harmony**: các dải được đẩy theo thứ tự main, light, dark rồi `unzip`. Với 9–16 key danh
sách là `[a0, a0-dark, a1, a1-dark, …]` (16 màu); quá 16 key thì thành `[a0, a0-light, a0-dark, a1,
…]` (24 màu). egret đã có `chartTint`/`chartShade` với đúng ±12,5 điểm lightness mà Metabase dùng.

## Sai khác có chủ đích so với spec

- **Spec nói đặt `colorsForKeys` vào `echarts/palette.ts`.** Kế hoạch đặt vào `echarts/keyColors.ts`
  mới: `palette.ts` đã ~700 dòng và chứa *nguyên thuỷ* về màu (phân tích, trộn, tương phản), còn đây
  là *chính sách gán* dựa trên chúng. Ghi ra để đây là một quyết định, không phải sự trôi dạt.
- **Không port `seriesVizSettingsDefaultKeys`** — egret không có khái niệm key gốc được lưu.
- **Không port tham số `palette`** — egret chỉ có một bảng màu.

## Hệ quả chấp nhận

- **Mọi chart chưa đặt màu thủ công sẽ đổi diện mạo một lần.** Đã quyết trong spec; phương án migrate
  ghi màu hiện tại vào `seriesColors` bị loại vì làm phình cấu hình lưu trữ chỉ để giữ một diện mạo
  không ai chọn.
- **Màu thủ công nay CHIẾM ô của nó**, nên đặt màu cho một series có thể đổi màu series khác. Đó là
  hành vi của Metabase và là thứ ngăn hai series bị vẽ cùng màu sau khi ghim một cái.
- **Với ≤ 8 key, thứ tự series không còn ảnh hưởng** vì lượt hash duyệt key đã sắp xếp.

## Cấu trúc tệp

| tệp | trách nhiệm |
|---|---|
| `echarts/keyColors.ts` (mới) | `colorsForKeys`, hash, bảng màu ngữ nghĩa, danh sách harmony |
| `echarts/__tests__/keyColors.test.ts` (mới) | thuật toán, đối chiếu các ca của Metabase |
| `echarts/options/cartesian.ts` | nhóm breakout và series metric gọi nó; bỏ xanh series đơn |
| `echarts/options/sankey.ts` | tên nút gọi nó |
| `echarts/options/pie.ts` | tên lát ring-0 gọi nó |
| `settings/CartesianDisplaySection.tsx` | `defaultSeriesColor` uỷ quyền |
| `settings/PieDisplaySection.tsx` | ô màu mặc định uỷ quyền |
| `echarts/palette.ts` | `SUMMARIZE_COLOR` thành nguồn của mục success, hoặc bị xoá |

Ngoài phạm vi, mỗi thứ đều có lý do: `INCREASE`/`DECREASE` của waterfall và `ZONE_*` của gauge mang
nghĩa chứ không phải ô bảng màu; box plot, map, progress, funnel nhận một màu do người dùng đặt với
`SERIES_PALETTE[0]`/`[1]` làm mặc định — đó là mặc định, không phải phép gán; up/down của
`SmartScalarChart` mang nghĩa.

## Các task

| # | nội dung |
|---|---|
| 1 | `colorsForKeys` — nhánh hash, bảng màu ngữ nghĩa, khẳng định `success === "#689735"` |
| 2 | nhánh harmony quá tám key; light chỉ vào cuộc khi quá 16 |
| 3 | cartesian: ba chỗ gọi, xoá xanh series đơn, **sửa hai comment nói sai về Metabase** |
| 4 | sankey và pie |
| 5 | panel cấu hình phải hiện đúng màu chart vẽ |
| 6 | đối chiếu live + mở PR (KHÔNG merge) |

Task 5 là cái bẫy của PR này: hai panel tự tính ô màu mặc định **độc lập** với renderer. Nếu để
nguyên, người dùng sẽ thấy một màu mà chart không hề vẽ, rồi "sửa" một màu vốn không sai.

## Tiêu chí nghiệm thu (Task 6)

1. **Không chart nào vẽ toàn bộ series bằng `#689735`.** Hôm nay có năm: bar, row, line, area, scatter.
2. **Chín nút sankey có chín màu khác nhau.** Hôm nay hai nút chung `#509ee3`.
3. **Một cột giữ nguyên màu qua các card.**
4. **Waterfall và gauge không đổi** — màu của chúng mang nghĩa.
5. **Ô màu trong panel bằng đúng màu được vẽ**, kiểm bằng cách mở panel trong trình duyệt.

Không kỳ vọng egret và Metabase gán *giống hệt* nhau: hash chạy trên tên cột của egret, còn key
series của Metabase mang thêm trang trí tên bảng/tên hiển thị mà egret không có. Thứ phải khớp là
*chính sách*: màu phân biệt, ổn định theo tên, tôn trọng tên ngữ nghĩa.

Đây là bài kiểm định thật. Ba PR trước trong chương trình này, mỗi PR đều từng ship một lỗi vượt qua
toàn bộ unit test và chỉ bị bắt ở bước này. Mô phỏng không phải bằng chứng.
