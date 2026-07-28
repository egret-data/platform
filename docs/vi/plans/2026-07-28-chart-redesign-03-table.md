# Kế hoạch triển khai — Thiết kế lại chart 03: Table

> **Cho agent thực thi:** BẮT BUỘC DÙNG SUB-SKILL superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để làm từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-28-chart-redesign-03-table.md` — khi hai bản lệch nhau, theo bản tiếng Anh. Code và test đầy đủ của từng bước nằm ở bản đó.

**Mục tiêu:** Cho phép đổi tên một cột — ở mọi nơi cột đó được gọi tên — và làm panel Display (thuộc shell) của table chịu được kết quả nhiều cột, rồi chứng minh trên cluster thật rằng những tính năng table đã có code thực sự chạy được.

**Kiến trúc:** `ColumnFormat.column_title` đã tồn tại và đã được tiêu đề trục cartesian đọc; việc còn lại là nối hai đầu còn thiếu — một ô nhập trong `FormatSection`, và bên đọc trong `ChartTable` và pivot. Phần panel làm bên trong các section dùng chung, vì descriptor của table **không khai báo control display nào**. Không đổi hình dạng settings và **không migration**.

**Spec:** `docs/vi/specs/2026-07-28-chart-redesign-03-table.md`

---

## Sự thật rút ra từ đọc code — kiểm lại, đừng suy lại

Đã đối chiếu với cây mã tại `1ee0819e`. Nếu đoạn code trong task lệch với thứ bạn thấy, **DỪNG và báo lại**.

1. `FormatSection` là accordion, mỗi cột một panel. Settings của cột đang mở là `fmt`, và `patch(p)` merge vào `value[open]`. Thân panel mở đầu bằng `<div className="space-y-3 border-t p-3">`, và mọi thứ bên trong hiện đều bị chặn bởi `isNumericType(c.type)` hoặc bởi `view_as`.
2. `ChartTable` **đã** nhận `columnSettings` và dùng nó trong `cell` — nên sửa header chỉ một dòng, và `columnSettings` đã nằm trong deps của `useMemo`. Dòng ~155: `header: c.name`.
3. Chữ trên header metric của pivot đến từ `pivot/engine.ts` — `topHeader.push(colLeaves.map((cl) => ({ label: cl.metric, colSpan: 1 })))` — hàm thuần, không thấy `column_settings`. `PivotTable` render `{c.label}` cho **mọi** ô header và không thể biết level nào là metric nếu không đếm.
4. `SharedDisplaySections` trả về đúng **một** câu — *"Run the chart to configure column formatting."* — cho **mọi** viz khi `resultColumns` rỗng.
5. `TableColumnsSection` là ba khối không nhóm: một checkbox mỗi cột (không giới hạn), ô số "Freeze first columns", và công tắc phân trang kèm select page size.
6. Descriptor của table chỉ khai báo hai control `data` và **không có control display**. **Đừng** thêm control display vào đó — tab Display của table thuộc shell theo thiết kế, và chart 01 đã từng hồi quy vì kéo section của shell vào descriptor.

---

## Các task

### Task 1: Ô nhập ghi `column_title`
Thêm ô "Column title" làm thứ **đầu tiên** trong thân accordion, phía trên khối numeric — đổi tên không phải chuyện của số. Ô rỗng ghi `undefined`, **không** ghi `""`: rỗng nghĩa là "dùng tên thật của cột", không phải "header trắng". Hàng accordion cũng hiện tên mới kèm tên thô mờ phía sau, để cột đổi tên vẫn tìm được mà vẫn truy ngược được về truy vấn. Lưu ý đọc `value[c.name]` chứ **không** phải `fmt` ở hàng đó — `fmt` chỉ là settings của cột đang mở.

### Task 2: Bảng đọc nó
`header: columnSettings[c.name]?.column_title || c.name`. Kèm test bấm sort trên header đã đổi tên — header chính là nút sort, đổi tên không được làm hỏng nó.

Thêm **test ghim thứ tự ưu tiên** giữa hai kênh đổi tên: `seriesLabels` thắng `column_title` trên chart cartesian. Quy tắc đó đã có sẵn trong `titleOf` từ chart 01 nhưng **chưa có test nào**, nên nếu nó lật ngược thì không ai biết. Đây là test đặc tả, phải xanh ngay; nếu đỏ thì dừng và báo.

Kiểm tra và báo: độ rộng cột, `hiddenColumns`, `columnOrder` đều khoá theo **tên cột** — xác nhận đổi tên không đụng tới chúng, tức không chỗ nào khoá theo **chữ trên header**.

### Task 3: Pivot đọc nó
Thay vì đếm xem level nào là metric — cách sẽ hỏng ngay khi hình dạng header đổi — cho engine **gắn nhãn** ô nó dựng từ metric: thêm `metric: cl.metric` vào ô đó. Đây là thêm dữ liệu, không đổi hành vi. `PivotTable` render `c.metric ? (columnSettings[c.metric]?.column_title || c.label) : c.label`. Có test chứng minh header **dimension** không bị đổi bởi một `column_title` trùng tên — cái nhãn chính là thứ phân biệt hai loại.

### Task 4: Empty state nói chart này đang chờ gì
Thay một câu dùng chung bằng câu chọn theo viz. Test phải khẳng định câu của table **khác** câu của cartesian — đó mới là trọng tâm; một test chỉ kiểm một câu sẽ vẫn xanh với câu dùng chung.

### Task 5: Danh sách cột chịu được kết quả rộng
Ô lọc chỉ hiện khi `columns.length > COLUMN_FILTER_MIN` (đặt 12, **tạm**, chỉnh ở task 7). Show all / Hide all luôn tác động lên **toàn bộ** cột, không phải danh sách đang lọc — ghim bằng test, vì đây đúng là lỗi mà cặp lọc + chọn-tất-cả mời gọi. Bộ lọc là cách **tìm** một cột, không phải một phép chọn. Rồi đặt tiêu đề cho hai khối ít dùng (Freeze, Paginate) để danh sách cột không dính liền vào một ô số.

### Task 6: Chứng minh những tính năng đã có thực sự chạy
Deploy nhánh lên kind (không cần build backend — task này không có migration), mở app **kèm tham số cache-busting**, rồi lái tay 8 ca: phân trang bật/đổi page size/đổi dữ liệu, đóng băng rồi cuộn ngang, kéo header đổi thứ tự rồi reload, kéo đổi độ rộng rồi reload, và xoá cột rồi thêm lại để xem có sót độ rộng cũ không.

Ghi lại **thực tế xảy ra** cho từng ca. Cái nào hỏng thì thành một commit trong PR này, vẫn TDD: test tái hiện trước, xác nhận đỏ, rồi mới sửa. Nếu không có gì hỏng thì nói thẳng — "không tìm thấy gì" là một kết quả thật, và là bằng chứng đầu tiên rằng đống này từng được chạy.

### Task 7: Verify đầy đủ, deploy, kiểm tra thật, mở PR
Cổng FE (lint, typecheck, test, build). Chạy 8 bước kiểm tra thật của spec; chỉnh `COLUMN_FILTER_MIN` theo bề rộng thật của panel. Bước số 2 là bước chứng minh cả đơn vị này: đổi tên `total_revenue` thành `Revenue` rồi xác nhận **header bảng và tiêu đề trục của một bar chart trên cùng cột** đều đổi theo — một lần đổi tên, hai nơi.

Ảnh trước/sau ở cả hai kích thước: deploy `main` trước để có ảnh "trước" thật, rồi deploy lại nhánh.

**Mở PR rồi DỪNG — không merge, chờ duyệt.** Nội dung PR phải nói rõ: tiền đề của đơn vị này trong spec chương trình đã sai và đã được sửa; người dùng giờ làm được gì mà trước không làm được; task 6 tìm thấy gì; và còn gì chưa làm.
