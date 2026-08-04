# Các danh sách cột của table — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-04-table-column-lists.md`
> là bản chuẩn; mã nguồn, số dòng và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Chấm dứt việc egret báo với người dùng rằng một cột đang bật trong khi vẫn không vẽ nó,
và đưa phần cấu hình cột về đúng hình dạng **một danh sách** như Metabase.

**Kiến trúc:** Hai PR. **PR A chỉ sửa lỗi** — một bước hoà giải cộng một sửa lỗi thứ tự ghim, không
động vào cấu trúc UI. **PR B gộp ba danh sách cột song song thành một.** A đứng được một mình và ship
trước; nếu sau này bỏ B thì A vẫn đã đóng lỗi.

---

## Những gì đã đo

Một lượt điều tra đã **lái cả hai app** — Metabase ở `localhost:3033` và egret trên bản kind
(`:30080`, phục vụ ở `:5174`). Toàn bộ phần này là đo được, trừ chỗ ghi rõ là suy luận.

**egret có BA danh sách cột, không phải hai** (bốn trên table đã tổng hợp):

| | **A** — Data ▸ "Columns" | **B** — Display ▸ "Visible columns" | **C** — Display ▸ "Formatting" |
|---|---|---|---|
| Component | `TableColumnsControl` (`viz/table.tsx:46-58`) | `TableColumnsSection.tsx` | `FormatSection.tsx` |
| Thành viên | mọi cột của dataset | cột trong kết quả | cột trong kết quả |
| Ghi vào | `TableConfig.columns` → `table.columns`, và `spec.columns`/`spec.dimensions` qua `buildSpec` | `table.state.{hiddenColumns,pinnedLeft,rowIndex,pagination}` | `column_settings[name]` |
| Làm được | bật/tắt cột **trong truy vấn** | ẩn/hiện, Show all / Hide all | drill-in → đổi tên, số lẻ, tiền tệ, mini-bar |

Sắp xếp lại và đổi độ rộng **không có control nào trong panel** — chỉ kéo trên bảng đã vẽ mới làm được
(`ChartTable.tsx:282` / `:259`).

**A và B thật sự là hai việc khác nhau** — chiếu truy vấn so với hiện/ẩn khi trình bày — và Metabase
cũng có cả hai. Nên không danh sách nào thừa. Lỗi nằm ở chỗ chúng được đặt tên gần như giống nhau,
trình bày y hệt nhau dưới dạng danh sách checkbox phẳng, nằm ở hai tab khác nhau, và **không bao giờ
được hoà giải với nhau**.

### Bốn chỗ bất đồng

1. **Thành viên khác nhau.** Trên table đã tổng hợp, `revenue` có trong B và vắng trong A (nó nằm ở
   danh sách Metrics riêng) — bốn danh sách trường song song trên một chart.
2. **Thứ tự khác nhau, và bảng vẽ ra không theo danh sách nào cả.** `TableColumnsSection` không hề đọc
   `columnOrder`; bảng theo `tableState.columnOrder` (`ChartTable.tsx:206-213`).
3. **"Freeze first columns" đếm theo thứ tự KẾT QUẢ, không theo thứ tự hiển thị.** Đo được: kéo `state`
   lên đầu rồi đặt freeze = 1 thì ghim `id`, và vì tanstack vẽ cột ghim trước nên **thao tác kéo bị
   hoàn tác trên màn hình**.
4. **Bật lại một cột ở tab Data thì nó vẫn âm thầm bị ẩn.** Ẩn `vendor` ở B → bỏ nó khỏi A → thêm lại ở
   A. Tab Data báo ON; bảng không có `vendor`; `hiddenColumns` vẫn giữ mục cũ và không ai dọn.
   **Người dùng không nhận được tín hiệu nào rằng chỗ sửa nằm ở tab khác.**

Không phải lỗi: ẩn ở B không cắt truy vấn. Metabase cũng vậy. (SUY LUẬN từ `buildSpec` — khẳng định
duy nhất trong phần này không được lái thử.)

### Metabase làm gì

- Tab bên là `Columns | Display | Conditional Formatting`. **Một** danh sách cột, nằm ở **Columns**.
  Tab Display chỉ có số thứ tự dòng và freeze — **không có danh sách cột nào**.
- **Dòng theo đúng khuôn danh sách series** (egret đã tiếp nhận ở #408): chỉ danh tính — icon kiểu dữ
  liệu và tên — cộng tay kéo, nút mắt, và nút `…` mở drill-in. Cột đã ẩn **vẫn nằm trong danh sách**
  với `data-enabled="false"`, không tách ra nhóm riêng.
- Định dạng theo cột nằm trong popover drill-in, kể cả đổi tên.
- Chiếu truy vấn **dùng chung chỗ**: nút `Add or remove columns` thay danh sách bằng bảng chọn trường
  và đổi thành `Done picking columns`.
- `table.columns: [{name, enabled}]` — **một mảng mang cả thứ tự lẫn trạng thái hiện/ẩn**.
- **Bước hoà giải mà egret thiếu:** `syncTableColumns` bỏ cấu hình của cột đã biến mất và thêm cột mới
  với `{name, enabled: true}`. Đo được: cùng kịch bản số 4 của egret, cột quay lại **đang bật**.

---

# PR A — hai lỗi

Chỉ sửa lỗi. Không thêm control, không dời control.

**Task A1 — hoà giải trạng thái cột đã lưu với kết quả.** Viết test đỏ theo **chuỗi thao tác**, không
theo trạng thái cuối, và **khẳng định sau từng bước** — vì lỗi nằm ở chỗ bước 3 không huỷ được bước 1,
nên một test chỉ nhìn trạng thái cuối sẽ xanh trên mã hỏng. Khi cài: dọn `hiddenColumns`, `columnOrder`,
`columnWidths` khỏi những tên không còn trong kết quả, và **tuyệt đối không làm sống lại cờ ẩn cũ cho
một cột vừa quay lại** — nửa sau mới là bản sửa thật. Phải nói rõ nó **chạy ở đâu** và cái gì canh nó:
hoà giải mỗi lần render mà ghi ngược vào settings sẽ nổ trên một kết quả rỗng tạm thời và phá sạch
trạng thái.

**Task A2 — freeze theo thứ tự hiển thị.** Đây là **đổi hành vi của một key đã có**, không đổi hình
dạng — không cần migration, nhưng nó âm thầm trỏ lại mọi chart đang giữ đồng thời `pinnedLeft > 0` và
một `columnOrder`. **Đếm số chart đó trên deployment trước khi merge** và đưa con số vào PR.

---

# PR B — một danh sách

Chỉ làm sau khi A đã merge.

Gộp B vào C — dòng của C đã đúng giải phẫu, chỉ thiếu nút mắt và tay kéo. **Khuôn đã có sẵn trong
repo**: `SeriesListSection.tsx` + `SeriesSettingsPopover.tsx` (#408), dùng lại chứ đừng viết cái thứ
hai. Giữ ở tab Display, **không** dời sang Data — `CardVizPanel` không có tab Data. Cho việc sắp xếp
một chỗ ở trong danh sách. Cột đã ẩn **ở nguyên trong danh sách**, có đánh dấu — đó là thứ khiến "bật
lại" tìm thấy được.

Rồi gấp A lại sau nút `Add or remove columns`. Nếu quá lớn, phương án tối thiểu là **đổi nhãn** —
"Columns in this query" so với "Columns shown" — và dừng ở đó; A1 mới là thứ đóng lỗi, nhãn chỉ khiến
nó bớt khó hiểu. **Nói rõ đã chọn cách nào và vì sao.**

**ĐÃ SỬA khi thực thi PR B: khuôn KHÔNG có tay kéo, và không nên mọc thêm.** `SeriesListSection` sắp
xếp bằng một cặp NÚT ▲▼ chứ không phải grabber. Danh sách cột gộp chép đúng cách đó, vì một cái nút
mang sẵn đường bàn phím của nó — PR #356 trong chính chương trình này đã ship một tính năng sắp xếp
cột chỉ dùng được bằng chuột vì danh sách `dnd-kit` không được cấp `KeyboardSensor`. Thao tác kéo trên
header của bảng đã render vẫn giữ nguyên; nó vốn đã có `KeyboardSensor`.

Cũng đã sửa: "nút mắt" ở đây đúng là một CON MẮT chứ không phải `Switch` của khuôn. Hàng cột của
Metabase dùng con mắt, và bốn mươi cái switch trong một panel rộng 300px là một sức nặng khác hẳn mười
cái của danh sách series. `aria-pressed` mang trạng thái nên nó không chỉ dựa vào màu.

Một điểm kế hoạch không nhắc và việc gộp bắt buộc phải xử lý: `FormatSection` vẫn phục vụ MỌI viz
khác, nơi tập cột định dạng được thực sự là tập con của kết quả. Accordion chỉ bị bỏ **cho riêng
table**, và trình soạn theo cột của nó được tách ra (`ColumnFormatFields`) để cả hai bên vẫn là MỘT
trình soạn trên MỘT key `column_settings`.

**ĐÃ LÀM PHƯƠNG ÁN ĐẦY ĐỦ, không dùng lối thoát.** Nó không hề xung đột với `viz/table.tsx`, vì nó
không đi qua descriptor: `SharedDisplaySections` nhận một `queryFields` đục
(`{label, options, selected, onChange}[]`) và `ChartExplore` đổ đầy từ `descConfig` cùng dataset.
`CardVizPanel` bỏ trống nên nút đó vắng mặt ở đó — đúng, vì một card trên dashboard ghi đè cách VẼ một
cột chứ không đổi được truy vấn chọn gì.

Nó mang một nhóm cho mỗi danh sách trường, nên danh sách METRICS cũng nằm trong đó. Đó là thứ đóng
bất đồng số 1 — điều mà chỉ đổi nhãn không làm được.

**Các control ở tab Data VẪN GIỮ, và đây là một điểm phải sửa lại của kế hoạch.** Thứ nhất,
`TableColumnsControl` là chỗ neo của các bộ chọn đơn vị thời gian theo cột (`unitRow`) — đó là ĐỘ MỊN
của truy vấn, không phải hiển thị. Thứ hai, Metabase cũng giữ cả hai: bước Data trong notebook có bộ
chọn trường VÀ viz settings có `Add or remove columns`, trên cùng một danh sách trường đã lưu. Hai lối
vào cho một setting chính là cách Metabase bố trí; thứ PR này bỏ đi là hai lối vào KHÔNG ĐỒNG Ý với
nhau.

**Test đã dời: mười cái, trong đúng hai tệp**, và cả hai lần dời đều cùng một lý do — control theo cột
đổi từ checkbox có `<Label>` là TIÊU ĐỀ HIỂN THỊ sang nút mắt gắn nhãn theo TÊN THÔ. Tám cái trong
`TableColumnsSection.test.tsx`, hai cái trong `ChartExplore.tableColumns.test.tsx` của PR A. Một trong
tám cái bị ĐẢO chứ không phải dịch: danh sách cũ hiện tiêu đề THAY CHO tên thô, hàng gộp hiện cả hai.
Không test nào khác dời.

**Kiểm tra trực tiếp trên deployment kind:** cả bốn bất đồng đều đã lái tay. Số 1 và số 2 do PR B ĐÓNG
MỚI; số 3 và số 4 là của PR A và được KIỂM LẠI ở đây, riêng số 3 kiểm với thao tác sắp xếp trong panel
— thứ chưa tồn tại lúc A ship. Cả 19 card parity render nguyên vẹn. Lần lái tay này tìm ra ba lỗi mà
không test nào và không lần review nào thấy: hàng của card trên dashboard đều bị gắn nhãn TEXT (cột của
card không mang kiểu), nhãn nhóm trong bảng chọn lặp lại nhãn của chính section, và vòng focus trên các
mũi tên 10px chỉ là nét mảnh mặc định của trình duyệt.

---

## Cố ý nằm ngoài phạm vi

**Không tiếp nhận mảng `table.columns: [{name, enabled}]` của Metabase.** Đó là đổi tên và xoá
`hiddenColumns` + `columnOrder`, cần migration cho mọi table chart đã lưu, đổi lại người dùng không
được gì — hai mảng song song của egret đã diễn đạt đúng chừng ấy. Toàn bộ chi phí lưu trữ của kế hoạch
này là **không key mới, không key đổi tên, không migration**.

## Tự soát

- **PR A mới là phần quan trọng.** Lỗi người dùng thấy được và im lặng; gộp danh sách chỉ là trình bày.
  Tách ra để A không bị giam theo một thay đổi UI bảy tệp.
- **A2 mới là nửa rủi ro của A**, không phải A1 — nó đổi ý nghĩa của một setting đã lưu.
- **Chỗ yếu đã biết:** Task A1 chưa nói hoà giải chạy ở đâu, vì cuộc điều tra chưa xác định được chỗ
  `resultColumns` được đặt cho table. Tìm ra trước khi viết, và nếu câu trả lời trung thực là *không có
  một chỗ duy nhất* thì nói vậy, đừng rắc lời gọi khắp nơi.

  **ĐÃ TRẢ LỜI khi thực thi PR A: có đúng một chỗ — `ChartExplore`.** `resultColumns` được suy ra ở hai
  component (`ChartExplore.tsx:199` từ `data.columns`, và `CardVizPanel.tsx:62` từ prop `columns`),
  nhưng chỉ một trong hai có thể khiến một cột RỜI KHỎI kết quả: hoà giải chỉ cần ở nơi truy vấn sửa
  được, và truy vấn chỉ sửa được từ tab Data của `ChartExplore`. `CardVizPanel` dùng đúng các section
  hiển thị đó nhưng không có tab Data; drill dialog và preview dataset render `ChartTable` mà không có
  `TableState`. Nó được chặn bởi `data.hasResult` + danh sách cột không rỗng + `viz === "table"`, và
  helper trả về chính đối số theo identity khi không có gì để cắt, nên trường hợp thường là React
  bail-out. PR B không cần thêm chỗ gọi thứ hai.
- Backend không kiểm gì ở đây, nên trạng thái lưu sai định dạng vẫn tới được mã này và mọi chỗ đọc đều
  phải chịu được.
