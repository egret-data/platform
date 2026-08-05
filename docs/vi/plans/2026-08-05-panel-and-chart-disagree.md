# Chỗ panel và chart bất đồng về việc cái gì có thật — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-05-panel-and-chart-disagree.md` là bản chuẩn; mã nguồn, số dòng và các bước
> chạy lệnh nằm ở đó.

**Mục tiêu:** Bỏ ba control không chart nào đọc, thêm control cho bốn setting mà mọi chart đã tôn
trọng sẵn, và ghim bốn hành vi mà một comment khẳng định nhưng không assertion nào giữ.

**Kiến trúc:** Ba nhóm độc lập, đều nằm ở tầng settings. **Nhóm A bỏ đi** một control chết trên 17/18
loại viz. **Nhóm B thêm vào** control cho những setting mà bộ định dạng đã đọc. **Nhóm C chỉ là
test** — không đổi mã sản phẩm.

---

## Xuất xứ

Ba lượt soát tại `1d9b0d30` cùng lượt mutation chạy song song. Chỗ ghi ĐO ĐƯỢC là đã lái hoặc chạy
thật; còn lại là đọc. **Số dòng dưới đây đúng tại `1d9b0d30`, và các PR #410, #412, #413 đã dịch
chuyển vùng mã này — hãy định vị theo NỘI DUNG chứ đừng theo số dòng, và báo lại nếu có gì trôi.**

Hình dạng chung: **một control và chart bất đồng về việc cái gì tồn tại.** Nhóm A là control không ai
đọc; Nhóm B là chỗ đọc không có control — chính codebase này đã đặt tên cho lỗi đó hai lần
(`FormatSection.tsx` với `scale`/`number_separators`, và "đúng họ lỗi từng làm pie mất bốn setting").
Nhóm C là comment không có test.

---

# Nhóm A — "Display as / Link text / Link URL" chết trên 17/18 loại viz

**Chào ra:** `settings/FormatSection.tsx`, trong nhánh `!isNumericType(c.type) && !isTemporalType(c.type)`
— một select `Display as` (`auto/text/link/email/image`) cộng hai ô `Link text` và `Link URL` khi chọn
`link`.

**Gắn vào:** `settings/SharedDisplaySections.tsx`, cho **mọi viz trừ `table`**.

**Đọc:** chỉ `ChartTable.tsx:68-82`. `format.ts:146` nói thẳng: `view_as: false, link_text: false,
link_url: false`.

Nên chiều-hàng của pivot, trục x của cartesian, chiều của pie, cột nút của sankey và trường của
object-detail đều hiện ra select "Display as → Link" cùng hai ô template — tất cả đều lưu được và
không cái nào vẽ ra gì.

**Bản sửa đã có sẵn trong chính tệp này và đang được dùng.** `show_mini_bar` từng dính đúng lỗi này và
đã đóng bằng prop `allowMiniBar`; `SharedDisplaySections` truyền `allowMiniBar={false}` cho mọi viz
không phải table, kèm comment giải thích vì sao `false` là phẳng chứ không phải điều kiện. **Cái gate y
hệt chỉ đơn giản là chưa bao giờ nối sang `view_as`.**

`FormatSection.tsx:52-55` và `:142-145` vốn đã lập luận đúng nguyên tắc — *chào ra một setting không
thể có tác dụng thì tệ hơn không chào* — trong khi ba control ngay dưới chúng làm đúng điều đó.

**Quyết định xem giá trị đã lưu thì sao** và nói rõ đã chọn cách nào: một chart đã lưu `view_as:"link"`
trên chiều của pie nay có một setting không còn control. Vì ngoài table chưa ai từng đọc nó, để nguyên
là vô hại — nhưng phải **nói ra** chứ đừng bỏ lửng, và kiểm `ColumnSettingsPopover` (#410) vẫn chào ra
nó ở chỗ nó **có** được đọc.

**Nó với tới cả card dashboard:** `DashboardView.tsx:340-341` gán `type: ""` cho cột chiều, rơi thẳng
qua `isNumericType`/`isTemporalType` vào đúng nhánh này. Kiểm gate có phủ đường đó không.

# Nhóm B — bốn setting bộ định dạng tôn trọng mà không có control nào

**B1 — conditional formatting không đặt được khoảng tuỳ chỉnh.** `conditional.ts:91-97` tôn trọng đầy
đủ `min_type`/`max_type` (`"custom"` và `"all"`) cùng `min_value`/`max_value`. Nhưng
`settings/RuleEditor.tsx` **cắm cứng** `{ min_type: null, max_type: null }` khi tạo rule khoảng và
không chào ra control nào cho cả bốn. Metabase có đủ (`ChartSettingsTableFormatting/RuleEditor.tsx:203-261`).
**Đọc widget của Metabase trước khi thiết kế của egret** — luật của chương trình này là chép LUẬT chứ
không chép giá trị, và tập tuỳ chọn (`all` so với `custom`) chính là cái luật.

**B2 — hai toán tử không dropdown nào sinh ra được.** `conditional.ts:33-34` cài `is-true`/`is-false`
trong `matchOperator`, và `RuleEditor.tsx` liệt chúng trong `NO_VALUE_OPS` — nhưng không `NUMERIC_OPS`,
`STRING_OPS` hay `NULL_OPS` nào chào ra chúng.
**Xác định egret có kiểu cột boolean nào biện minh cho một danh sách `BOOLEAN_OPS` không. Nếu không,
bản sửa trung thực là XOÁ phần cài đặt chết đi chứ không phải thêm control cho một kiểu không thể xảy
ra** — nói rõ chọn đằng nào và vì sao.

**B3 — ba trường định dạng ngày/giờ không có control.** `format.ts:102` đọc `date_separator`;
`format.ts:105-108` đọc `time_enabled` và `time_style`; `CONSULTED_BY_FORMAT_DATE` đánh dấu cả ba là
`true`. Không component settings nào ghi chúng — nhánh temporal của `FormatSection` chỉ chào `date_style`
và `date_abbreviate`. Metabase chào cả ba (`lib/settings/column.ts:177`, `:214`, `:230`).
**`time_enabled` cổng cho `time_style`** — đọc `getHidden` của Metabase chứ đừng đoán quan hệ đó.
Và **chúng cũng phải vào `ColumnSettingsPopover`** (#410 đã biến phần định dạng của table thành
drill-in), nên kiểm cả hai bề mặt.

# Nhóm C — bốn assertion sống sót qua cả suite

**ĐO ĐƯỢC.** Lượt mutation chạy 64 mutation trên 12 tệp nguồn, chấm bằng 15/145 tệp test; 55/59 mutant
chấm được đã chết. Bốn con này sống sót **toàn bộ** test, và cả bốn cùng một hình dạng: **một hành vi
mà comment khẳng định rõ ràng, và không assertion nào ghim.**

| mutation | nó làm hỏng cái gì |
|---|---|
| mũi tên sắp xếp: `disabled={!!q \|\| i === 0}` → `disabled={i === 0}` | đang lọc mà bấm "lên" thì tráo với dòng ngoài màn hình — đúng điều comment nói |
| icon kiểu: `c.type ? colIcon(c.type) : null` → `colIcon(c.type)` | kiểu lạ hiện icon đoán mò; comment lập luận dài rằng phải hiện kiểu VẮNG. Chỉ nửa badge được ghim |
| freeze: `Math.max(0, Math.min(n, columns.length))` → `n` | freeze nhận số lớn hơn số cột |
| `options/pie.ts`: `if (level === 0 && cfg.sliceOrder?.length)` → `if (cfg.sliceOrder?.length)` | sliceOrder áp cả vòng trong; comment nói nó chỉ dành cho vòng 0 |

Với mỗi cái: **viết đúng test mà comment ngụ ý**, chạy nó trên mã đã mutate, xác nhận nó đỏ. Lượt soát
đã nêu hình dạng tối thiểu: gõ vào ô tìm rồi khẳng định `Move X up` là `toBeDisabled()`; khẳng định
dòng không kiểu không render `svg` nào ở ô icon; bắn `change` với `"99"` trên fixture 2 cột rồi khẳng
định `pinnedLeft` phát ra; fixture hai vòng khẳng định vòng trong giữ thứ tự `buildSlices` dưới một
`sliceOrder`.

**Nhóm này không đổi mã sản phẩm.** Nếu viết test mà phát hiện comment SAI chứ không phải chưa ghim,
**sửa mã và nói ra** — đó là một phát hiện, không phải chệch hướng.

---

## Cố ý nằm ngoài phạm vi

Vòng focus 2.58:1 ở theme sáng — #412 đã xác định nó là **cấp sản phẩm** (34 chỗ gọi trên 26 tệp, chạm
notebook, SQL Lab, control plane; chỉ 5 tệp trong `components/chart/`). PR riêng, và có lẽ người review
riêng.

Cũng ngoài phạm vi: số của trend tràn card hẹp và tiêu đề trục trái của combo đọc tên series đã ẩn sau
khi click legend (đều tìm được ở #412/#413); đường bàn phím tới drill theo ô, thứ mà `ChartTable`,
`PivotTable` và object detail đều thiếu như nhau; và các khoảng cách phía Metabase.

## Tự soát

- **Nhóm A bỏ một control còn nhóm B thêm bốn.** Không mâu thuẫn — đó là cùng một luật áp theo hai
  chiều: panel phải chào ra đúng thứ chart đọc.
- **Nhóm C rẻ nhất và dễ bị bỏ qua nhất**, nên nó là một nhóm có tên chứ không phải một dòng chú
  thích. Cả bốn đều là một test mỗi cái.
- **B2 có thể là một phép XOÁ, không phải phép thêm.** Đừng chiều theo sức hút của sự đối xứng; nếu
  không cột boolean nào tới được `matchOperator` thì phần cài đặt mới là thứ chết.
- **Chỗ yếu đã biết:** Nhóm A giả định `view_as` **chỉ** được `ChartTable` đọc. Điều đó đến từ một lượt
  đọc mã, không phải một lượt chạy. Kiểm lại trước khi xoá control khỏi 17 loại viz — nếu có renderer
  khác tham chiếu nó thì cái gate sai và phát hiện đổi hình dạng.

---

## Những chỗ khi thi hành đã phải sửa lại kế hoạch

Ghi ở đây thay vì sửa trực tiếp các mục trên, để kế hoạch và kết quả thực tế đọc được cạnh nhau.

- **"17/18 loại viz" thật ra là 18/19.** `VizType` có mười chín thành viên —
  `ChartRenderer.clickCoverage.test.tsx` khẳng định đúng con số đó — và `table` là loại duy nhất có
  renderer đọc `view_as`, nên control chết được gắn trên mười tám loại.

- **Điểm yếu đã biết của Nhóm A vẫn đứng vững, và đã được kiểm bằng cách CHẠY.** Một probe dùng một
  lần đã render cả mười chín loại viz qua `ChartRenderer` với `view_as` / `link_text` / `link_url`
  nằm sau getter ghi lại stack ở mỗi lần truy cập. `table` là loại duy nhất có stack chạm tới một lần
  đọc thật sự (`ChartTable.renderTableCell`). Mọi truy cập khác đều là SPREAD chép cả format sang một
  object mới để đưa vào `formatValue` — `{ ...col, compact }` của `cartesian.ts`, `compactifyValue`
  của `scalar.ts`, `fmt` của `boxplot.ts` — hoặc là khoá memo `JSON.stringify` trong `ChartRenderer`.
  Probe không được commit (nó không assert gì); nó trở thành `__tests__/viewAsIsTableOnly.test.ts`,
  ghim hai điều giữ cho kết luận còn đúng: `formatValue` mù với cả ba trường, và không tệp sản phẩm
  nào ngoài danh sách cho phép nhắc tới chúng trong mã.

- **B2 là THÊM VÀO chứ không phải xoá đi — xác lập trên môi trường chạy thật.** Chạy
  `SELECT true AS is_paid, CAST(1 AS TINYINT) AS tiny, 'x' AS label` qua SQL Lab trên bản kind trả về
  `{"name":"is_paid","type":"boolean"}` với giá trị ô là JSON `true`. `ColumnMeta.type` chính là tên
  kiểu do engine báo, và cả `isNumericType` lẫn `isTemporalType` đều không nhận `boolean`, nên cột
  như vậy đã được chào ra cho rule tô màu điều kiện và rơi vào danh sách toán tử STRING. Phần cài đặt
  trong `matchOperator` là dữ liệu với tới được mà không dropdown nào tạo ra; `BOOLEAN_OPS` là nửa
  còn thiếu. (StarRocks là một bất đối xứng chứ không phải phản ví dụ: connector của nó báo kiểu theo
  giao thức MySQL, nên `BOOLEAN` của StarRocks đến dưới dạng `MYSQL_TYPE_TINY` và `isNumericType`
  khớp ở chữ `tiny` — đúng giới hạn mà `typegen.rs` đã ghi lại.)

- **Ràng buộc mà B3 nêu là đúng, nhưng không phải vì Metabase làm thế.** Kế hoạch bảo kiểm
  `getHidden` của Metabase xem `time_enabled` có che `time_style` không. Đo trên bản tham chiếu đang
  chạy ở :3033: với `time_enabled` bằng `null`, Metabase vẫn render nhóm radio `time_style`, và vẫn
  render nhóm `date_separator` bất kể date style là gì. egret vẫn che cả hai — `formatDate` chỉ đọc
  `time_style` bên trong `if (fmt.time_enabled)`, và chỉ áp `date_separator` qua
  `dateFmt.replace(/\//g, …)` — vì một control không che ở đây chính là đúng cái lỗi mà Nhóm A đang
  bỏ đi. Tập lựa chọn và nhãn là của Metabase; quy tắc ẩn/hiện là của PR này.

- **Phát hiện khi nối B2, và đã sửa: select toán tử có thể hiện ra trống trơn.** Rule mới được tạo với
  `operator: ">"` (`ConditionalFormattingSection`), vốn không nằm trong `STRING_OPS` — nên chọn một
  cột text sẽ để Select mang một giá trị không option nào có, Radix render thành trigger RỖNG, và lưu
  lại sẽ cất một dấu `>` mà `matchOperator` sau đó đem so sánh như chuỗi. Lỗi có sẵn từ trước;
  `BOOLEAN_OPS` sẽ thêm đường thứ ba dẫn vào đó. `RuleEditor` nay lùi về toán tử đầu tiên mà các cột
  đang chọn diễn đạt được, và những gì trigger hiện ra đúng bằng những gì `onSave` ghi.

- **Cả bốn comment của Nhóm C hoá ra đều ĐÚNG, không cái nào sai.** Mỗi mutation đều được áp và mỗi
  test mới đều đổ với nó; nhóm đó không cần đổi mã sản phẩm nào.
