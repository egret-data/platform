# Data API — SQL sinh từ script (gói S)

**Trạng thái:** thiết kế, chưa lên kế hoạch
**Chương trình:** DBAPI parity. Gói này **thay thế giai đoạn 2–4** của script engine
(`db.query`, ghi, HTTP ra ngoài) — ba giai đoạn đó **bị loại bỏ**.
**Phụ thuộc:** C2 (`5774bc77`) cho evaluator và các giới hạn của nó; M1/M2
(`1140dff4`, `ebf3791c`) cho cỗ máy thăm dò và hợp đồng cột theo từng câu lệnh;
P4.5 cho bộ định dạng bind mà thiết kế này **từ chối đi vòng qua**.

## 1. Đây là gì

SQL của một câu lệnh có thể **được viết** (như hôm nay) hoặc **được sinh** bởi một
script Starlark:

```python
TABLES = {"daily": "sales_daily", "hourly": "sales_hourly"}

def build_sql(params):
    table = TABLES[params["grain"]]
    where = ["region = {{ region }}"]
    if params["min_total"]:
        where.append("total >= {{ min_total }}")
    return "SELECT region, sum(total) AS total FROM " + table + \
           " WHERE " + " AND ".join(where) + " GROUP BY region"
```

**Script trả về một template, không phải SQL hoàn chỉnh.** `{{ region }}` vẫn là
`{{ region }}` khi script trả nó về, và egret cho nó chạy qua **đúng** đường
minijinja đang chạy SQL viết tay. Script quyết định **cấu trúc** câu lệnh; bộ định
dạng sẵn có quyết định **giá trị**.

### 1.1 Vì sao là cách này chứ không phải `db.query`

Giai đoạn 2–4 được xếp là `db.query` → ghi → HTTP ra ngoài, với ranh giới nằm giữa 2
và 3, vì giai đoạn 3 biến egret từ **router** thành **runtime**.

Cách này lấy được giá trị thực dụng của giai đoạn 2 mà vẫn đứng phía router của ranh
giới đó, và còn đơn giản hơn: script tạo ra **văn bản**, và egret thực thi văn bản
ấy y hệt cách nó thực thi SQL của chính tác giả. Không xử lý connection mới, không
N+1, không trần đồng thời, không đường danh tính thứ hai, không câu hỏi "ai là người
đọc". Ranger và Cedar phân xử y như trước, vì truy vấn tới engine theo đúng con
đường cũ.

**Tóm tắt lập luận an toàn trong một câu: script không thể tạo ra thứ gì mà tác giả
không thể tự gõ.** Mọi luật đang canh SQL viết tay đều canh SQL sinh ra, vì chúng đi
qua cùng một đường ống.

## 2. Tính chất an toàn, và chỗ duy nhất nó có thể vỡ

### 2.1 Giá trị vốn đã an toàn, và không có gì về chúng thay đổi

Bộ định dạng bind biến mọi `{{ x }}` thành một bind marker. Một script phát ra
`{{ region }}` tạo ra một bind, y như câu lệnh viết tay. **Đường đi của giá trị người
gọi tới engine KHÔNG đổi vì tính năng này** — và đó chính là toàn bộ lý do script
trả về template thay vì một chuỗi.

Đây không phải sở thích về phong cách. P4.5 đã chứng minh, với giá đắt, rằng **một bộ
quét byte canh một ngôn ngữ template sẽ thua** — năm vòng review độc lập tìm ra năm
cách vượt khác nhau. Và `sql_literal` từng có lỗ thật: nó nhân đôi `''` nhưng bỏ qua
`\`, mà `\` **là ký tự escape trên StarRocks** (Trino thì an toàn), đã vá ở #504 và
chứng minh sống chứ không phải bằng suy luận. Một thiết kế để script trả về SQL hoàn
chỉnh sẽ đặt **mọi tham số** trở lại phía sai của bài học đó.

### 2.2 Định danh — câu hỏi thật sự

`ident_filter` (`render.rs:569`) so byte của người gọi với danh sách `allowed` của
tác giả bằng `==`, rồi **phát ra chuỗi `String` của tác giả** — chính bình luận trong
mã gọi việc clone `entry` thay vì `supplied` là "toàn bộ tính chất bảo mật".

Script được phép phát ra định danh từ đúng hai nguồn, và **không nguồn nào cần cơ chế
mới**:

- **Một chuỗi hằng nằm trong chính script.** `TABLES["daily"]` cho ra `sales_daily`,
  chuỗi đó được viết trong script, mà script do tác giả viết và **đóng băng trong
  snapshot**. Xuất xứ là của tác giả — đúng xuất xứ mà một mục trong `allowed` của
  enum có. Cái script thêm vào là khả năng *quyết định chuỗi nào xuất hiện*, bằng
  logic tuỳ ý. Đó chính là ca "chọn bảng theo tham số", và nó an toàn vì **tập bảng
  khả dĩ do tác giả viết ra**.
- **`{{ x | ident }}`** với `x` là tham số `enum`. Luật hôm nay, giữ nguyên.

Thứ **không** được phép là dựng định danh từ byte của người gọi:
`"sales_" + params["region"]`.

### 2.3 Phép thăm dò bằng giá trị mốc, và điều nó KHÔNG chứng minh

Phát hiện ca cấm ở §2.2 bằng cách quét template chính là sai lầm mà P4.5 đã trả giá.
Cổng ở đây là **hành vi**, cùng hình dạng với "render hai lần, so opaque với chuỗi":

**Publish chạy script với một giá trị mốc đặc biệt làm giá trị cho từng tham số, rồi
kiểm xem giá trị mốc đó có xuất hiện trong template phát ra hay không.** Nếu script
nối tham số vào văn bản SQL, cái mốc nằm đó. Nếu nó chỉ dùng tham số để rẽ nhánh,
hoặc truyền qua `{{ }}`, cái mốc không xuất hiện. M1 **vốn đã** chạy định nghĩa dưới
nhiều phép gán thăm dò, nên việc này tốn thêm một phép gán, không phải một cỗ máy mới.

**Nói thẳng, vì một giới hạn đọc lên như một bảo đảm thì tệ hơn một thiếu sót được ghi
rõ:** một script **biến đổi** giá trị trước — viết hoa, băm, cắt chuỗi — sẽ vượt được
cái mốc. Phép thăm dò bắt được ca tác giả tạo ra lỗ injection **do vô ý**, và đó là ca
thật sự hay xảy ra. Nó **không** phòng được một tác giả cố tình dựng ra một lỗ.

Phần dư đó chấp nhận được, và nên nói rõ vì sao thay vì mong không ai hỏi. Tác giả đã
xác thực, giữ `data_api_endpoint:manage`, và vốn đã được tin để viết SQL. Một tác giả
quyết tâm phơi endpoint của chính mình thì làm được; điều đó cũng đúng với người cấp
quyền quá tay. Mối đe doạ mà thiết kế này **phải** chặn là **người gọi**, và với người
gọi thì tính chất không đổi: giá trị là bind, định danh là của tác giả.

## 3. Hợp đồng cột không cần gì mới

Đây là phần làm cho tính năng này rẻ, và đáng nói tường minh.

**SQL của một câu lệnh sinh ra là hàm thuần của `params`.** Script **không bao giờ**
nhìn thấy một hàng — đó là transform của C2, một pha khác. Nên tình huống đúng bằng
tình huống mà M1 đã giải cho nhánh `{% if %}`: hình dạng đầu ra biến thiên theo tham
số và **không theo gì khác**.

Vậy publish chạy script dưới các phép gán thăm dò sẵn có của M1, `describe_as` mô tả
từng câu lệnh thu được, và endpoint đóng băng **phép hợp cột trên mọi lần thăm dò**,
với `required` đánh dấu cột có mặt ở tất cả. Serving kiểm với phép hợp đó và fail
closed, không đổi.

Khoảng trống mà M1 đã ghi nhận cũng chuyển sang nguyên vẹn, và **không được mô tả nhỏ
hơn thực tế**: ngân sách thăm dò có giới hạn (`MAX_PROBES`), và một định nghĩa có thể
rẽ trên nhiều tổ hợp hơn số nó phủ. Một script còn rẽ được nhiều hơn nữa. Lưới chắn
vẫn là lưới cũ — `shape_rows` từ chối một cột không ai khai báo.

## 4. Vị trí, và nó ghép với những gì

- **Theo từng câu lệnh, không theo endpoint.** M2 cho mỗi câu lệnh một cái tên, một
  SQL và một hợp đồng cột riêng. Một câu lệnh sinh ra là một trong số đó, nên trường
  `sql` của `StatementSpec` trở thành **hoặc** văn bản **hoặc** một script. Một script
  sinh cả N câu lệnh sẽ đụng độ với snapshot theo từng câu lệnh mà không được lợi gì.
- **Ghép được với transform của C2**, vốn chạy **sau** khi các câu lệnh thực thi. Hai
  pha khác nhau, không tương tác. Một endpoint có thể có cả hai, một, hoặc không.
- **Cổng một-câu-lệnh áp dụng nguyên vẹn.** Đầu ra của script đi qua
  `reject_unscannable_bytes`, lượt quét comment/literal và phép kiểm sau render, nên
  `#` bị cấm và `;` bị từ chối ở mọi chỗ nó bị từ chối hôm nay. **Một script không thể
  tạo ra một định nghĩa nhiều câu lệnh.** Nếu một test trong
  `crates/egret-dataapi/tests/template_render.rs` có vẻ cần sửa vì tính năng này, thì
  thay đổi đó **sai**.
- **Evaluator là của C2**, không đổi: `Globals::standard()`, `enable_load: false`, các
  ngân sách tick/heap/callstack, phần chặn panic, và **một** `AstModule::parse` duy
  nhất. Một chỗ parse thứ hai sẽ bị một test sẵn có từ chối.

## 5. Giới hạn

Dùng lại của C2 ở chỗ áp dụng được. Hai giới hạn riêng của tính năng này, và **cả hai
phải được ĐO**, không phải được chọn:

- **Độ dài template phát ra.** Một script trả về một megabyte SQL không phải câu lệnh
  nên đưa cho engine lập kế hoạch. Trần script của C2 là 8 KiB so với một ví dụ ~200
  byte; hãy đo trần này dựa trên một câu lệnh sinh ra thật.
- **Phép đánh giá chạy ở đâu.** C2 đã đo rằng chạy Starlark trên async worker là thứ
  nên tránh, và chuyển preview sang `spawn_blocking`. Việc sinh SQL diễn ra ở **mọi
  request được phục vụ**, tức là ca mạnh hơn preview. Quyết định bằng phép đo, không
  bằng phép loại suy — và lưu ý rằng `/validate` với `publish` **hôm nay vẫn** chạy
  inline, nên gói này là thời điểm tự nhiên để sửa nốt.

## 6. Preview

Preview vốn đã chạy một câu lệnh. Một câu lệnh sinh ra sẽ preview bằng cách chạy
script, **hiển thị SQL nó tạo ra**, rồi thực thi SQL đó.

**Hiển thị SQL sinh ra là trọng tâm, không phải điểm cộng.** Một tác giả đang gỡ lỗi
một script nhiều nhánh **cần nhìn thấy câu lệnh**, và preview là bề mặt duy nhất hiển
thị được — transform của C2 vốn không preview được chút nào, và câu trả lời trung thực
khi đó là nói ra điều đó. Ở đây câu trả lời trung thực tốt hơn: **tạo phẩm có sẵn, nên
phải đưa lên màn hình.**

Luật của C2 vẫn giữ: preview, publish và serving phải tới **cùng một** bộ sinh qua
**cùng một** hàm. Nếu lúc lên kế hoạch thấy mình đang viết một đường sinh thứ hai cho
preview, thì thiết kế đã sai. Đường nối đó đã hỏng **hai lần** trong chương trình này.

## 7. Rõ ràng KHÔNG nằm trong gói này

- **`db.query`, ghi, HTTP ra ngoài.** **Loại bỏ**, không phải hoãn. Gói này là thứ
  thay thế chúng.
- **Script chọn connection.** Database là của endpoint, đóng băng trong snapshot; một
  script tự chọn database là một quyết định thẩm quyền mới, và đó đúng là ranh giới
  router/runtime.
- **Dùng chung script sinh giữa các endpoint.** Nhu cầu có thật và là tính năng khác;
  một script cho mỗi phiên bản câu lệnh giữ snapshot tự chứa.
- **Sinh ra danh sách tham số.** Tham số vẫn khai báo (C1). Một script tự bịa ra tham
  số của mình sẽ nằm **phía trên** mọi phép kiểm làm publish và serving đồng ý nhau.

## 8. Điều gì sẽ khiến spec này sai

Ghi ra để kiểm được, thay vì giả định:

- ~~Nếu template phát ra hoá ra **không** đi qua đúng con đường render của SQL viết
  tay, thì câu "đường đi không đổi" ở §2.1 là sai.~~ **Đã kiểm khi viết spec này — nó
  đi đúng đường đó.** `render(sql: &str, ctx: &ParamValues)` (`render.rs:771`) là entry
  point công khai **duy nhất**, và đường phục vụ gọi nó bằng
  `crate::render::render(&stmt.sql, &resolved.values)` (`router.rs:1076`). Sinh ra
  `stmt.sql` rồi đưa vào đúng lời gọi ấy là **theo nghĩa đen cùng một đường**.
  `render_permutations` (`render.rs:1275`) — thứ mà phép thăm dò của M1 dùng — cũng
  nhận `&str`, nên §3 đứng vững vì cùng lý do.

- **Một hệ quả phải tính vào kế hoạch, không phải một rủi ro:** `StatementSpec` là
  `{ name: String, sql: String }` và là một type `ToSchema`, nên biến `sql` thành
  *hoặc văn bản hoặc script* sẽ làm **đổi `openapi.json`** và client sinh ra. Cổng đó
  nằm ở crate `egret-server` và đã **đỏ âm thầm suốt sáu task** trong C1 — phải sinh
  lại và báo cáo kết quả đó **tách riêng**.
- Nếu SQL của script có thể phụ thuộc vào bất cứ thứ gì ngoài `params`, thì §3 sụp và
  hợp đồng không thể đóng băng từ các phép thăm dò.
- Nếu phép thăm dò bằng giá trị mốc không luồn được vào các phép gán sẵn có của M1 mà
  không cần một đường đánh giá thứ hai, thì §2.3 tốn hơn "một phép gán nữa" và cần
  cân nhắc lại đánh đổi.
- Nếu việc sinh theo từng request trên đường phục vụ đo ra tệ tới mức cần cache, thì
  đó là một **thay đổi thiết kế** (cache khoá theo params, vô hiệu theo version) và
  thuộc về kế hoạch, không phải thứ hàn vào giữa chừng.
