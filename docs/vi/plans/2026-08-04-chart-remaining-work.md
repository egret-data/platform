# Còn lại gì trong chương trình chart — kế hoạch sắp thứ tự

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-04-chart-remaining-work.md` là bản chuẩn.
>
> Đây là **kế hoạch chương trình**, không phải danh sách task cho một PR. Mỗi mục dưới đây là một PR
> riêng với bằng chứng, phạm vi và điều kiện dừng của nó. Đừng cố chạy tài liệu này từ trên xuống trong
> một nhánh.

**Hiện trạng:** chương trình parity với Metabase (PR 01–07) đã merge, cộng `#394` thứ tự trục danh
nghĩa, `#395` control sắp xếp trong studio, và `#396` backstop chống đè nhãn.

**Toàn bộ tài liệu này được kiểm lại ngày 2026-08-04.** Ba khẳng định mang từ trước sang là **sai** và
đã được đính chính tại chỗ.

---

## Thứ tự, và vì sao

| # | mục | vì sao ở đây |
|---|---|---|
| 1 | Issue #387 — drill trên trục thời gian | một tính năng âm thầm không làm gì; thứ **hỏng** duy nhất còn lại mà người dùng chạm tới |
| 2 | Mất phím giữa số thập phân | năm ô số nuốt input hợp lệ; nhỏ, dùng chung, PR 06 đã khoanh vùng sẵn |
| 3 | Thứ tự trục danh nghĩa của waterfall | đóng lại họ lỗi mà `#394` mở ra; khoảng một hàm |
| 4 | Mô tả aria đọc bucket thô | đúng/sai về accessibility, và hẹp hơn ghi chép cũ |
| 5 | Trục giá trị dọc chồng mốc | thấy rõ trên bar và box plot ở khổ card |
| 6 | PR 06b — panel thiếu | tính năng, không phải lỗi; lớn nhất và ít cấp bách nhất |
| 7 | Chạy lại seed harness | rẻ, và harness đang trôi khỏi `demo` |
| 8 | Dọn dẹp | quyết định của bạn, không phải của tôi |

---

## 1. Issue #387 — drill trên trục thời gian lọc bằng chuỗi rỗng

Bấm vào một điểm trên chart có trục x kiểu **Timeseries** thì drill ra rỗng. Filter sinh ra là
`<cột ngày> = ''` — chuỗi rỗng — không khớp dòng nào, nên trang drill mở ra trống mà không nói vì sao.

**Chú ý:** một trục đã bucket phải drill thành **khoảng nửa mở**, không phải phép bằng —
`ChartExplore.tsx:250` đã ghi vậy trong comment về hai filter trên cùng một cột. Bản sửa phải sinh ra
khoảng đó, không phải `= <timestamp>`.

**Nghiệm thu:** bấm một điểm trên `MB · Line` và ra được dữ liệu, trong trình duyệt. Test đơn vị về
hình dạng filter là cần nhưng chưa đủ — đây là lỗi về thứ **tới được truy vấn**.

## 2. Mất phím giữa số thập phân, ở năm ô

PR 06 đo bằng gõ phím thật (không phải `fill()`): mọi ô `type="number"` nhận số lẻ đều mất một phím
giữa chừng trên Chrome. Gõ `4` rồi `.` phát ra sự kiện `input` với `value === ""` và
`validity.badInput === true`, mà `Number("")` là 0.

**Năm ô:** `Y min`, `Y max` (ở **cả** `ChartAxesSection` lẫn `CartesianDisplaySection`), và
`Goal value` (cartesian, waterfall, box plot). Chúng **xoá trắng** chứ không làm sai giá trị, vì đã map
`"" → undefined` — nửa nhẹ hơn của lỗi. Ô ngưỡng lát pie, vốn còn lưu thành `0`, đã sửa ở PR 06.

**PR 06 dừng ở đây có chủ đích** — năm ô là một component dùng chung, không phải bản vá cục bộ. Đó
chính là PR này: tách thứ `SliceThresholdInput` đang làm thành một control và áp cho cả năm.

**Ô số nguyên không bị** (`Decimal places`, số vạch, `Freeze first columns`, `Max categories`).
**`Freeze first columns` KHÔNG phải lỗi** dù cùng dạng `Number.isNaN(n) ? 0`: `pinnedLeft ?? 0` là mặc
định của chính nó nên xoá về 0 là nói thật. Đừng "sửa" nó.

**Test bằng `userEvent.type`, không phải `fireEvent.change`** — lỗi nằm ở trạng thái trung gian.
`user-event` của jsdom tự nuốt dấu `.` nên chỉ thấy một nửa; phải nói rõ test của bạn phủ nửa nào.

## 3. Trục danh nghĩa của waterfall vẽ theo thứ tự engine

**Xác nhận 2026-08-04**, `options/waterfall.ts:115-119`: nhánh danh nghĩa rơi thẳng vào `rows`, tức thứ
tự đến. Cùng lỗi `#394` đã sửa cho bar, row và box plot. Waterfall của harness nhóm theo `order_month`
nên là trục thời gian và che mất.

**Bản sửa là `sortCategories`**, vốn đã được export từ `options/cartesian.ts` đúng cho việc này.

**Cẩn thận:** thứ tự của waterfall **không** chỉ là thẩm mỹ — tổng luỹ tiến được dựng theo nó. Sắp xếp
lại là đổi bước đầu tiên. `#394` đã miễn trừ bar mode của funnel qua `preserveCategoryOrder` vì lý do
tương tự; phải quyết định waterfall có cần cửa thoát đó không và nói rõ vì sao.

## 4. Mô tả aria đọc bucket thô — còn tooltip thì KHÔNG

**Đính chính.** Ghi chép cũ nói "tooltip và aria đều đọc bucket thô". Kiểm tại
`options/cartesian.ts:1495-1520`:

- **Tooltip**: `visible[0].axisValueLabel ?? visible[0].name` — `axisValueLabel` là nhãn trục **đã định
  dạng**, nên tooltip vốn đã hiện `January 2026`. **Không có lỗi.**
- **Aria**: `points: categories.map((c) => ({ name: String(c) }))` — giá trị **thô**. Nên trình đọc màn
  hình đọc `2026-01-01` trong khi trục ghi `January 2026`.

`drawLabel` đang có sẵn trong cùng builder. Kiểm luôn legend breakout — cùng khẳng định đó chưa được
kiểm lại.

## 5. Trục giá trị dọc chồng mốc

`#390` đã cho trục giá trị của **row chart** phép fit interval, cố ý chỉ áp cho chart ngang. Đo trên
dashboard ở svg 166×184, trường hợp dọc vẫn chồng: `100K/200K` và `200K/300K` trên bar, `20/40` và
`40/60` trên box plot.

**Máy móc đã có sẵn** trong `axisLabelFit.ts`. Nhưng ràng buộc của trục dọc là **chiều cao**, không phải
chiều rộng, và nhãn của nó ngắn hơn nhiều — nên đây không phải phép chuyển vị. Phải đo để xác lập.

**Cũng còn mở, và có lẽ cùng PR:** tiêu đề trục đè lên nhãn danh mục xoay trên bar và box plot, đúng
như row chart trước khi `#390` sửa.

**Liên quan nhưng ngoài phạm vi:** bar và box plot dành 29% chiều cao card cho vùng vẽ trong khi line
chart dành 58%. Đó là ladder xoay của `#386` và cần đối chiếu riêng.

## 6. PR 06b — panel thiếu

Nhóm X/Y cho tab Axes; `getHidden` (bar một series vẫn hiện `Stacking`); số dòng table (đã render nhưng
nằm sau `paginationEnabled` mặc định false); cấu hình line theo từng series; thang Linear/Power/Log;
Goal line bị chôn sau "More options"; hai danh sách cột của table; mặc định `showLabels`; và **đường nối
đơn vị trích xuất thời gian** — cái cuối cần `dimensionUnits` luồn qua `VizControlProps`, `StudioPanel`,
`CardVizPanel`, `ChartExplore`, tức **đổi contract của Chart Studio**, và đó là lý do PR 06 dừng.

**Quá lớn cho một PR.** Tách: đường nối và `getHidden` là hành vi; nhóm tab, số dòng và tuỳ chọn thang
là trình bày; cấu hình line theo series là một tính năng riêng.

## 7. Chạy lại seed harness

`demo` đã trôi khỏi thứ `scripts/harness/seed.ts` sinh ra — cả `#394` lẫn `#395` đều đổi output của
`buildSpec`, và `MB · Scalar` / `Gauge` / `Progress` trong `demo` vẫn lưu spec không xác định mà `#393`
đã phán quyết.

**Đọc `docs/en/guides/chart-parity-harness.md` trước.** Đo lại dashboard sau đó và cập nhật các bảng
mốc mà các PR parity đã ghi, nếu không mọi so sánh sau này sẽ dựa vào số cũ.

## 8. Dọn dẹp — quyết định của bạn

- **`egret-pg-test`** đang **chạy** (`postgres:16`). Không thứ gì trong chương trình này dùng nó; stack
  là cụm kind. **Xác nhận trước khi xoá** — có thể nó thuộc mạch việc khác.
- **54 tệp `.png`** rơi vãi trong `data-portal/` (không phải ~20 như ghi chép cũ). Playwright ghi tương
  đối với `data-portal/` chứ không phải scratchpad — đó là cách chúng tích lại. Chúng nằm ngoài mọi
  repo git nên `.gitignore` không giúp được; cách sửa bền là truyền đường dẫn tuyệt đối tới scratchpad
  cho mọi lệnh chụp màn hình.

---

## Tự soát

- **Ba khẳng định mang từ trước sang là sai và đã đính chính**: tooltip **không** đọc bucket thô (chỉ
  aria), số ảnh rơi vãi là 54 chứ không phải ~20, và `egret-pg-test` đang chạy chứ không nằm im.
- **Cố ý không viết chi tiết triển khai cho mục 5–6.** Chúng cần đo trước, và bài học lặp lại của
  chương trình này là một kế hoạch viết từ ghi chú cũ sẽ chỉ định sai bản sửa — `#394` từng đề xuất sắp
  xếp phía client mà đọc mã Metabase thì thấy là lệch, và kế hoạch `#396` nêu bốn ký hiệu bundle trong
  đó ba cái bị minifier đổi tên.
- **Mục 3 mang một rủi ro mà bản tóm tắt sẽ giấu đi**: thứ tự danh mục của waterfall quyết định tổng
  luỹ tiến của nó, nên "sắp xếp như mấy cái kia" không hiển nhiên đúng. Đã nêu ra thay vì mặc định.
