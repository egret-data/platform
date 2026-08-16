# Data API P4.5 — Tham số: giá trị bind và render template

Sửa đổi `2026-08-14-data-api-design.md` §3, §6.2 bước 7, §6.3 và §10. Chỗ nào hai tài liệu
mâu thuẫn thì tài liệu này có hiệu lực, và tài liệu cũ được **sửa lại** chứ không để nguyên.

## 1. Vì sao có tài liệu này

Spec §6.2 liệt kê mười bước của chuỗi phục vụ. **Bước 7 — render `params` đã khai báo vào
SQL — chưa bao giờ được cài đặt.** P3 kiểm hình dạng tham số, P4 đưa nó vào cache key, và
không chỗ nào thay thế nó cả. Đo trên mã đã ship: `?region=EU` và `?region=US` trả về **cùng
một tập dòng**, vì không một input nào của người gọi chạm tới engine.

Nên Data API hôm nay chỉ phục vụ `SELECT` tĩnh. Lời hứa trung tâm của cả chương trình — một
truy vấn có tham số được publish thành REST endpoint — đúng là phần chưa giao.

Bước 7 cũng là nơi **duy nhất** input của người gọi chạm vào SQL. Nó chính là toàn bộ bề mặt
injection của tính năng, và cách xây nó quyết định bề mặt đó có tồn tại hay không.

## 2. Các quyết định, và những gì đã đổi so với spec gốc

**Giá trị được bind, không nội suy.** §10 ghi rằng bound parameters bị hoãn "vì chi phí, không
phải vì nội suy được đánh giá là đủ", và liệt kê mọi lỗi escaping của tính năng này như hệ quả
của việc hoãn đó. Chi phí đã được đo (§3) và chấp nhận được.

**Render bằng `minijinja` với formatter tuỳ biến**, không phải cú pháp tự chế, cũng không port
MyBatis. Không thư viện Rust nào cài MyBatis lúc chạy: `rbatis` tự mô tả là "Compile Time Async
Dynamic SQL ORM" và `rbatis-codegen` chỉ phơi ra `rb_html`/`rb_py`, cả hai sinh `TokenStream`;
crate `mybatis` có bản cuối 2022-09-23 và phần XML chưa cài đặt. `minijinja` render lúc chạy từ
một `&str`, và — điểm quyết định — `Environment::set_formatter` chặn **mọi** lần xuất giá trị,
nên bind không phải quy ước mà tác giả phải tuân theo, mà là việc duy nhất engine làm được.

**§10 được sửa ở ba chỗ.** Nó nói bản sửa sẽ "trừ đi", xoá dialect escaper: **không đúng** —
Trino vẫn cần nó (§3.1), và `egret-bi` vẫn dùng nó. Nó nói một template engine đầy đủ sẽ là
"bề mặt tấn công mới… vùng rủi ro đã biết của Superset": đúng với Python Jinja2, **không đúng**
với minijinja (§4.4). Nó hoãn phần render điều kiện thành một cấu trúc tối giản làm sau: không
cần — điều kiện có sẵn cùng bộ render, và quy tắc thứ tự nó nêu ("bound parameters first") được
thoả ngay trong phase này vì formatter **chính là** cơ chế bind.

## 3. Cơ chế từng engine

Hai engine không làm cùng một việc, và spec nói thẳng điều đó thay vì gọi cả hai là "binding".

### 3.1 Trino — tách ở tầng phân tích cú pháp, egret tự render literal

Đo trên Trino 481 qua HTTPS với JWT của Keycloak:

```
POST /v1/statement
  Authorization: Bearer …
  X-Trino-User: tester
  X-Trino-Prepared-Statement: p1=SELECT%20%3F%20AS%20s%2C%20%3F%20AS%20n
  body: EXECUTE p1 USING 'hello', 42
→ 200, columns=[('s','varchar(5)'),('n','integer')], rows=[["hello",42]]
```

Bỏ header đi thì nhận `Prepared statement not found: p1`, nên header chính là thứ mang câu
lệnh. Các tính chất đo được:

- Sống chung với bearer token và `X-Trino-User` mà egret đang gửi. Mạo danh người khác bị Trino
  từ chối (`Access Denied: User tester cannot impersonate alice`) — hành vi sẵn có, không đổi.
- Header **chỉ cần ở POST**; vòng lặp GET theo `nextUri` không cần. Kiểm trên truy vấn 200 000
  dòng, 10 chặng: chỉ gửi header ở POST, mọi dòng vẫn đúng.
- Kích thước header không phải ràng buộc thực tế: SQL dài 120 024 byte vẫn trả 200.

**Danh sách `USING` là văn bản SQL do egret render.** Giao thức HTTP của Trino không có kênh
tham số ở tầng dây. Ranh giới bộ phân tích cú pháp là thật và mạnh — đo được:

| Nội dung `USING` | Kết quả |
|---|---|
| `'a'; DROP TABLE zz` | `SYNTAX_ERROR` — endpoint chỉ nhận một câu lệnh |
| `'' UNION ALL SELECT …` | `SYNTAX_ERROR` |
| `'a' \|\| (SELECT count(*) FROM …)` | từ chối — `Unsupported expression (SubqueryExpression)` |
| `'a' \|\| 'b'` | **`"ab"`** — chấp nhận |
| `current_user` | **`"tester"`** |
| `upper('ab')` | **`"AB"`** |

Nên một lỗi trong bộ render literal là injection **biểu thức vô hướng bất kỳ**, không phải SQL
bất kỳ. Đó là mức thu hẹp lớn, và nó không bằng không. Vì vậy `quote_string` **vẫn sống** trên
đường Trino và giữ nguyên test của nó. Backslash không phải ký tự escape trong Trino
(`USING 'a\\b'` → `a\\b`, 4 ký tự), nên vector backslash của P1 không áp dụng ở đây.

Giá trị có kiểu khứ hồi chính xác: `'x'`, `9007199254740993`, `DATE '2020-02-29'`,
`TIMESTAMP '2020-02-29 13:14:15.123'`, `DECIMAL '123.456'`, `true`, `NULL` quay về đúng
`varchar(1), bigint, date, timestamp, decimal(6,3), boolean, varchar`.

### 3.2 StarRocks — bind thật ở tầng dây

`conn.prep()` rồi `exec_iter()`, binary protocol, có tiền tố độ dài. Đo trên StarRocks 4.1.3:
`prep` trả về số tham số và đầy đủ metadata cột đầu ra; giá trị thù địch quay về nguyên văn là
dữ liệu và database mồi vẫn còn:

```
E-backslash  bound "a\' OR 1=1 -- "  ->  Bytes "a\' OR 1=1 -- "  length 14
F. p45_bind still present after hostile binds: true
```

Điều này đóng vector backslash của P1 **theo cấu trúc**, trên đúng engine mà nó đã được chứng
minh là khai thác được.

Các ràng buộc đo được, mỗi cái là một luật kiểm lúc publish:

- `?` bị từ chối trong `LIMIT`/`OFFSET` (`using parameter(?) as limit or offset not supported`).
- `?` bị từ chối ở vị trí định danh (`No viable statement for input '….?'`).
- `IN (?)` bind đúng **một** giá trị. Tham số dạng danh sách phải render N placeholder (§4.3).
- StarRocks báo mọi tham số là `MYSQL_TYPE_VAR_STRING`; không có suy luận kiểu để dựa vào.
- Prepared statement chạy được **sau** `EXECUTE AS <user> WITH NO REVERT`, nên đường impersonate
  không bị ảnh hưởng.
- **Chưa đo:** prepared statement qua đường JWT/`mysql_clear_password`; StarRocks dev không cấu
  hình `authentication_jwt`. Không có dấu hiệu nào cho thấy plugin xác thực ảnh hưởng tới
  `COM_STMT_PREPARE`. Coi là **chưa kiểm chứng** cho tới khi chạy thật.

**Chi phí:** thêm một round trip (`COM_STMT_PREPARE`) mỗi truy vấn — đo được 5,59 ms/op → 9,88
ms/op cục bộ cho connect+execute. Không đáng kể với truy vấn phân tích, thấy được ở đường
cache-miss rẻ của Data API.

### 3.3 Lỗi giải mã phải sửa trong cùng thay đổi này

Chuyển StarRocks sang `exec()` làm đổi biểu diễn trên dây. Chạy chính `row_to_json`/
`bytes_to_json` của egret, không sửa gì, trên cả hai giao thức:

| cột | text `query()` | binary `exec()` |
|---|---|---|
| INT / BIGINT / BOOLEAN | `Bytes` | `Int` |
| DOUBLE | `Bytes` | `Double` |
| LARGEINT / DECIMAL / VARCHAR | `Bytes` | `Bytes` |
| **DATE / DATETIME** | **`Bytes`** | **`Value::Date(..)`** |

`row_to_json` **không có nhánh `V::Date`**, nên DATE và DATETIME rơi xuống
`Value::String(format!("{:?}", other))` và tuần tự hoá thành chuỗi `Date("'2020-02-29'")`.
Đo được, không phải phỏng đoán.

**Mọi unit test hiện có của `bytes_to_json` vẫn xanh, vì `bytes_to_json` không còn được gọi
nữa.** Đây là lý do tiêu chí ở §9 được viết như vậy.

Việc ép kiểu theo kiểu cột vẫn đúng và trở nên thừa một phần: số nguyên và số thực về đã có
kiểu nên các nhánh đó ngừng chạy, còn tính chất giữ nguyên `"007"` của VARCHAR vẫn đúng vì
VARCHAR/DECIMAL/LARGEINT giữ nguyên `Bytes` ở cả hai giao thức.

### 3.4 Đường nối

Một phương thức mới trên `Connector`, mặc định là **từ chối**:

```rust
async fn execute_with_params(
    &self, sql: &str, params: &[Param], limits: &Limits, cancel: CancellationToken,
) -> Result<TabularResult, EngineError> {
    if params.is_empty() { self.execute(sql, limits, cancel).await }
    else { Err(EngineError::Engine("connector does not support bound parameters".into())) }
}
```

Không phải thêm đối số `params` vào `execute`. Đối số có mặc định buộc **mọi** implementor —
kể cả ba fake trong test và mọi connector tương lai — phải tự quyết, và lựa chọn sai rẻ nhất là
lờ nó đi rồi nội suy. Một phương thức riêng với mặc định là lỗi thì **hỏng theo hướng đóng**.
Codebase này đã mất một SEV-0 vì đúng một hành vi mặc định được kế thừa trong `describe`.

Cùng hình dạng cho `QueryExecPort::execute_as_with_params`, mặc định uỷ quyền khi
`params.is_empty()`. Điều này quan trọng hơn vẻ ngoài: `Connector` có 5 điểm gọi production,
nhưng `QueryExecPort` có **10 implementation, 9 trong đó là fake viết tay trong test** — đổi
chữ ký của nó là khối lượng gấp khoảng 9 lần, và mỗi fake là một chỗ mà mặc định sai sẽ âm
thầm làm test rỗng ruột. Xoá phần mặc định về sau, khi các fake được chuyển đổi có chủ đích.

`describe` cần đối xử tương tự, và không phải cho đối xứng: trên StarRocks, đường text với SQL
chứa `?` trả về **0 cột và không báo lỗi**, nên publish sẽ đóng băng một schema rỗng thay vì
thất bại. Cả hai engine đều mô tả được truy vấn có tham số mà không cần giá trị — Trino qua
`DESCRIBE OUTPUT p1` (và `DESCRIBE INPUT p1` cho danh sách tham số), StarRocks qua `prep()`
trên lớp bọc `LIMIT 0`.

**Không engine nào suy ra kiểu của tham số.** Một `?` trần trong danh sách select ra kiểu
`unknown` trên Trino và `VAR_STRING` trên StarRocks. Vì vậy tham số xuất hiện trong phần
projection **bắt buộc phải bọc `CAST`**; publish từ chối nếu không, thay vì đóng băng kiểu sai
vào snapshot.

## 4. Render

### 4.1 Formatter chính là cơ chế bind

`Environment::set_formatter` thay toàn bộ bước xuất giá trị:

```rust
pub fn set_formatter<F>(&mut self, f: F)
where F: Fn(&mut Output<'_>, &State<'_, '_>, &Value) -> Result<(), Error> + 'static + Sync + Send
```

Formatter của egret ghi `?` ra output và đẩy giá trị vào danh sách bind. **Không tồn tại đường
đi từ `{{ … }}` ra văn bản SQL thô**, nên tác giả endpoint không thể viết một template nội suy,
kể cả cố tình.

Kiểm bằng probe trên minijinja 2.24.0, formatter phát ra `?`:

| Cấu trúc | SQL | Số bind |
|---|---|---|
| `SELECT {{ a }}` | `SELECT ?` | `["EVIL' OR 1=1--"]` |
| `{{ a\|safe }}` | `SELECT ?` | 1 — `safe` chỉ đánh dấu, không đi vòng |
| `{{ a\|escape }}` | `SELECT ?` | 1 |
| `{{ 'pre' ~ a ~ 'post' }}` | `SELECT ?` | 1 — cả biểu thức ghép là một giá trị |
| thân `{% for %}` / `{% if %}` | `SELECT ?,?,` | mỗi giá trị một cái |
| `{% raw %}{{ a }}{% endraw %}` | `SELECT {{ a }}` | 0 — văn bản template nguyên văn |

Lập luận cấu trúc đằng sau probe: máy ảo chỉ ghi ra output ở đúng **hai** chỗ, và chỗ ghi văn
bản thô có kiểu `EmitRaw(&'source str)` — một lát cắt mượn từ mã nguồn template. **Không
`Value` lúc chạy nào chạm tới nó được.** Mọi lần xuất có mang giá trị đều là
`Instruction::Emit`, sinh ra ở ba chỗ: `{{ expr }}`, kết thúc `{% filter %}`, và `{% call %}`.

Một cái bẫy trong cơ chế đó: `Emit` có đường tắt gọi thẳng hàm escape mặc định, **bỏ qua
formatter**. Nó được canh bởi `is_default_formatter()`, và `set_formatter` xoá cờ đó. **Đường
tắt chỉ chết sau khi `set_formatter` đã được gọi** — nên nó phải được gọi vô điều kiện lúc dựng
environment, không bao giờ nằm sau một điều kiện.

### 4.2 Các cấu trúc gom thân làm lệch danh sách bind — từ chối lúc lưu

`{% filter %}`, `{% set x %}…{% endset %}`, `{% macro %}` và `{% call %}` gom thân vào buffer —
lúc đó `{{ }}` bên trong đã thành `?` — rồi đẩy **cả buffer** qua formatter thêm một lần nữa.
Chuỗi `"?"` bị bind như một tham số:

```
{% filter upper %}{{ a }}{% endfilter %}
   SQL   : "?"                      một placeholder
   BINDS : ["EVIL' OR 1=1--", "?"]  hai giá trị
```

`{% call %}` đo được còn tệ hơn: một placeholder, **ba** bind.

Đây không phải injection — không chữ nào của người gọi vào SQL — nhưng danh sách bind lệch khỏi
số placeholder, nghĩa là giá trị gán sai vị trí hoặc lỗi arity ở driver. Trên StarRocks, lệch
arity **giết chết kết nối**: `StmtParamsMismatch` được raise phía client và `SELECT 1` sau đó
trả về "Connection to the server is closed". egret mở `Conn` mới mỗi truy vấn nên bán kính hiện
tại là một truy vấn, nhưng một connection pool trong tương lai sẽ thừa hưởng kết nối đã hỏng.

**S-2 — có một lối gom thân THỨ BA, và nó không phải một tag.** Một vòng
`{% for … recursive %}` được gọi lại bằng `{{ loop(x) }}` **dùng như một giá trị** (chứ không
phải là toàn bộ phần emit) sẽ gom thân vòng lặp rồi phát lại, lệch đúng như `{% filter %}`.
Nó không được nêu trong bản gốc và được tìm ra bằng cách **đọc VM**, không phải bằng review.
Vậy tập cấu trúc gom thân là **bốn** thứ chứ không phải ba: `{% filter %}`,
`{% set x %}…{% endset %}`, `{% macro %}`/`{% call %}`, và vòng for đệ quy — và cái cuối **bị
từ chối lúc publish** như các cái khác (`RenderError::RecursiveLoop`), phát hiện trên **token**
của tag `for` chứ không bằng `split_whitespace`, vì `{% for x in (regions)recursive %}` không
cần dấu cách trước modifier.

**S-2 — `{% raw %}` là một vector làm lệch pha scanner.** Mọi thứ bên trong
`{% raw %}…{% endraw %}` là văn bản thuần với lexer nhưng trông như cú pháp template sống với
một byte scanner. Nên bất kỳ luật nào hiện thực bằng cách quét byte trên **source** của template
đều đọc phải những cấu trúc mà renderer sẽ không bao giờ chạy (từ chối thừa — hướng an toàn) và
có thể bị dắt lệch khỏi thứ thực sự chạy. Đây là lý do tổng quát khiến bảo đảm ở §4.2 được phát
biểu như một **tính chất của output đã render** — số placeholder bằng số bind — thay vì như một
danh sách cách viết bị cấm; và là lý do cổng so sánh trong `check_publish` mang tính **hành vi**
(render hai lần, một lần với giá trị opaque có kiểu và một lần với cùng giá trị đã "plain hoá";
mọi sai khác về văn bản SQL, số placeholder hay văn bản bind đều là một phép so sánh trần, dù
viết kiểu gì). Năm lượt review khác nhau lúc hiện thực, mỗi lượt tìm ra một cách né scanner
khác nhau; các cách viết đã đo được là `{%+` (marker GIỮ khoảng trắng), `{# {% #}`, `(mode)`,
`regions[0]`, `| first`, `is eq`, `{% raw %}` và một alias ở header vòng lặp. **Cái nào cũng
là scanner bất đồng với lexer**, và đó là lý do lời giải là thôi hỏi scanner mà quay sang hỏi
lexer (xem S-3 ở §4.3). Scanner chỉ còn sống như một lớp thực thi trên serving path, không bao
giờ là chỗ bảo đảm đứng lên.

`{% macro %}` và `{% call %}` biến mất cùng feature `macros` **khi feature đó thực sự vắng mặt
— xem S-1 ở §4.5, và đó là lý do chúng còn bị từ chối ở mức parse chứ không được giả định là đã
biến mất**. `{% filter %}` và `{% set x %}…{% endset %}` được parse vô điều kiện và không
feature nào bỏ được, nên kiểm lúc publish là đòn bẩy duy nhất: template chứa một trong hai sẽ
bị từ chối.

Chống lưng cho luật đó bằng một **bất biến** thay vì một quy tắc: mọi template lưu lại đều được
render lúc publish với giá trị đại diện, và **số placeholder phải bằng số bind**. Khi luật từ
chối bị hồi quy thì bất biến bắt được, chứ không phụ thuộc vào người review.

### 4.3 Một endpoint trông như thế nào

```sql
SELECT region, SUM(amount) AS total
FROM sales
WHERE 1=1
  {% if region %} AND region = {{ region }} {% endif %}
  {% if from %}   AND day >= {{ from }}     {% endif %}
  {% if regions %} AND region IN (
      {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %}
  ) {% endif %}
GROUP BY region
ORDER BY {{ sort | ident }}
```

`region=EU` render ra `AND region = ?` với một giá trị. `regions=[EU,US,APAC]` render ra
`IN (?, ?, ?)` với ba giá trị. Bài toán danh sách `IN` — nơi `IN (?)` chỉ bind đúng một giá
trị — **rơi ra miễn phí** từ formatter; không cần cấu trúc `foreach` riêng.

**S-3 — ĐÃ ĐÓNG.** Một bản trước của tài liệu này cảnh báo rằng ví dụ trên sẽ **không**
publish được, vì scanner placeholder của P3 coi toàn bộ phần bên trong `{{ }}` là *tên* tham
số: `{{ sort | ident }}` quay về thành `UndeclaredPlaceholder("sort | ident")` còn biến vòng
lặp `{{ r }}` thành `UndeclaredPlaceholder("r")`. Việc này được sửa bằng cách **hỏi lexer**
chứ không phải bằng cách viết một scanner giỏi hơn — `Template::undeclared_variables` của
minijinja trả lời câu "template này đọc những tên nào", và nó đã sẵn loại trừ biến vòng lặp
cùng các ràng buộc `{% set %}`/`{% with %}`, đồng thời đã sẵn **bao gồm** những tên chỉ được
đọc bởi một tag — thứ mà byte scan không thể thấy. Byte scan vẫn trả lời câu hỏi khác: "một
tham chiếu đứng ở đâu trong SQL", nay khoá theo văn bản thô của occurrence thay vì theo một tên
đã parse, nên `PlaceholderInsideQuotedText` không đổi về bản chất. **Ví dụ trên nay publish
nguyên văn, và có test nói vậy.** Đã xác nhận sống ngày 2026-08-16 trên StarRocks:
`?regions=EU&regions=US` trả hai dòng đã group, `?regions=EU` **một giá trị** trả một dòng
(một giá trị đơn resolve thành list một phần tử, không phải scalar — xem luật arity-vs-shape ở
§5), và `?sort=` ngoài `allowed` là 400.

### 4.4 Vị trí định danh

Định danh không bind được: `ORDER BY ?` là lỗi cú pháp ở cả hai engine. Có đúng **một** lối
thoát hẹp và có tên — filter `ident` — và đó là cách duy nhất phát ra văn bản dẫn xuất từ tham
số.

`{{ p | ident }}` yêu cầu `ParamSpec` của `p` là `ParamType::Enum` có danh sách `allowed`, khớp
giá trị người gọi với danh sách đó, và phát ra **phần tử khớp trong `allowed`** — chuỗi của
chính tác giả endpoint, không bao giờ là chuỗi của người gọi. Hai chuỗi bằng nhau nhưng khác
nguồn gốc; cái được ghi ra luôn là của tác giả. Giá trị không nằm trong `allowed` là lỗi 400.
Dùng `ident` trên tham số không phải enum bị từ chối lúc publish.

### 4.5 Environment đã siết

```toml
minijinja = { version = "2.24", default-features = false,
              features = ["builtins", "serde", "deserialization",
                          "std_collections", "debug", "fuel"] }
```

Bỏ `macros` và `multi_template`; thêm `fuel` vì nó **không** bật mặc định. `json`, `urlencode`,
`custom_syntax`, `loop_controls` và `unstable_machinery` giữ tắt. Tắt `custom_syntax` cũng
đồng nghĩa không tác giả nào đổi được dấu phân cách `{{ }}`.

**S-1 — danh sách feature là một sự GIẢM BỚT, không phải một RANH GIỚI, và một bản trước của
mục này đã trình bày nó như một bảo đảm.** `resolver = "2"` của Cargo hợp nhất feature trên
toàn workspace: ngay khi bất kỳ member nào khác phụ thuộc `minijinja` với default features,
`macros` và `multi_template` được biên dịch trở lại *ở đây*, không đổi một dòng nào trong
manifest này và không có cảnh báo nào. Vậy nên không được giả định `{% macro %}` và
`{% call %}` vắng mặt khỏi thứ mà parser chấp nhận. Thứ thực sự giữ được là **từ chối ở mức
parse lúc publish** (§4.2) cộng bất biến số-placeholder-bằng-số-bind — cả hai đều là tính chất
của code egret và không thể bị bật lại bởi một thay đổi dependency của bên thứ ba. Hãy đọc
danh sách feature như một cách giảm bề mặt tấn công khi nó tình cờ được giảm, đừng bao giờ đọc
nó như lý do một cấu trúc là an toàn.

Lúc dựng, theo thứ tự này:

1. `remove_global("debug")` — **lệnh gọi giá trị nhất**. `debug()` là `format!("{state:#?}")`;
   probe lấy về **toàn bộ context** cộng danh sách mọi filter/test/global đã đăng ký. Trên
   endpoint công khai đó là lộ dữ liệu.
2. `remove_global("range")`, `remove_global("dict")`, `remove_global("namespace")`.
3. `remove_filter("pprint")` — vector lộ dữ liệu thứ hai (biểu diễn `Debug` của một giá trị).
4. `set_undefined_behavior(UndefinedBehavior::Strict)` — gõ sai tên thành lỗi, thay vì âm thầm
   bind một giá trị rỗng.
5. `set_fuel(Some(…))` — bộ fuzzer render của chính dự án dùng 50 000. Hết fuel là **lỗi sạch**
   (`OutOfFuel`), không phải cắt cụt.
6. `set_recursion_limit(…)` — thấp hơn hẳn mặc định 500; hàm setter dùng `min(level,
   MAX_RECURSION)` nên chỉ hạ xuống được, không nâng lên.
7. `set_formatter(…)` — formatter bind, **vô điều kiện** (§4.1).

Không bao giờ gọi `set_loader`, `set_unknown_method_callback`, `add_function` hay `add_global`;
chỉ dùng `add_template_owned`. **Feature `loader` là no-op** — `set_loader` và `path_loader`
luôn được biên dịch vào và không bỏ được, nên một lệnh grep trong CI khẳng định chúng không bao
giờ được gọi là **một phần của việc này**, không phải việc làm sau.

Render vào một sink giới hạn kích thước. minijinja **không có giới hạn kích thước output**;
fuel giới hạn lệnh chứ không giới hạn byte, và `'a' * 99_999_999` là hợp lệ dưới ngưỡng chuỗi
lặp 100 MB.

**S-4 — mục này ban đầu còn nói "và dưới một timeout theo đồng hồ". Phần render KHÔNG có
timeout đó, một cách cố ý.** `render` là đồng bộ, nên muốn có đồng hồ bao quanh thì phải
`spawn_blocking`, mà hết hạn ở đó không dừng được công việc — nó **tách rời** một thread chạy
hoang, tiếp tục đốt một core mà không còn ai giữ. Vậy giới hạn của render là **fuel + sink
1 MiB + trần bind 1 MiB**, cả ba đều dừng công việc chứ không phải dừng chờ. Đồng hồ nằm ở lớp
ngoài, trên lệnh gọi engine: `port_impl::run_bounded` — dùng chung bởi `execute_as` và
`execute_as_with_params`, chính vì thế nó không thể có ở đường này mà thiếu ở đường kia — bọc
future của connector trong `tokio::time::timeout(limits.timeout, …)`, và hết hạn thì drop
future, kéo theo drop kết nối. Bản thân hai connector đều không đọc `limits.timeout`.

### 4.6 Template với tới được những gì

Chỉ những giá trị được đưa vào context. Truy cập thuộc tính chỉ có nghĩa với
`ValueRepr::Object` và uỷ quyền cho `get_value_by_str` của chính đối tượng đó; mọi nhánh khác
trả về `Undefined`. `ValueRepr` là enum **đóng, 13 nhánh**, không có đối tượng kiểu, không con
trỏ lớp, không liên kết cha. `Object::call_method` mặc định trả `UnknownMethod`, và map/chuỗi
thuần **không có phương thức nào**.

Probe: `{{ a.__class__ }}` render ra rỗng, `{{ a.upper() }}` lỗi "unknown method". **Lớp tấn
công `__class__`/`__globals__`/`__subclasses__` của Python Jinja2 là không với tới được theo
cấu trúc, không phải nhờ danh sách chặn** — nó dựa vào việc đối tượng Python mang theo đồ thị
kiểu phản chiếu lúc chạy, còn một `Value` của minijinja dựng từ dữ liệu serde chỉ mang dữ liệu.

Một điểm dự phòng cần biết: phương thức không rõ sẽ được thử lại như một callable lưu tại khoá
đó trên đối tượng. Với dữ liệu vô hướng thì không gì callable nên nó trơ — nó chỉ sống dậy nếu
có callable được đưa vào context, điều mà luật ở §4.5 cấm.

## 5. Mô hình tham số và kiểm tra

`ParamSpec` giữ nguyên: `name`, `ty` (`String|Number|Boolean|Date|Timestamp|Enum`), `location`,
`required`, `default`, `allowed`, `description`.

**Lúc publish**, bổ sung vào các kiểm tra sẵn có của P3 (trùng tên, placeholder không khai báo,
khai báo không dùng, `PlaceholderInsideQuotedText`):

- Template parse được dưới environment đã siết.
- Không chứa `{% filter %}` và không chứa `{% set x %}…{% endset %}` (§4.2).
- Render với giá trị đại diện thì **số placeholder bằng số bind** (§4.2).
- `| ident` chỉ áp lên tham số `Enum` có `allowed` (§4.4).
- Không tham số nào nằm trong `LIMIT`/`OFFSET` (§3.2).
- Tham số trong phần projection được bọc `CAST` (§3.4).
- `describe` chạy qua đường có tham số và trả về danh sách cột **không rỗng**; danh sách rỗng
  là lỗi publish, không bao giờ là một schema rỗng bị đóng băng (§3.4).
- **`location: body` bị từ chối.** Không có gì ở bề mặt phục vụ đọc request body, nên tham số
  body không bao giờ được cấp. Nếu `required`, đó là 400 vĩnh viễn — ồn ào, thấy ngay. Nếu
  tuỳ chọn thì tệ hơn: nó phân giải thành vắng mặt, khối `{% if %}` bị bỏ, và endpoint trả
  **200 kèm dữ liệu KHÔNG lọc, mãi mãi**, không có gì trong response nói rằng một điều kiện đã
  biến mất.
- **`default` được ép kiểu bằng đúng hàm mà đường phục vụ dùng**, và phải thoả `ty` của chính
  nó. Nếu không, một tham số `date` để mặc định `"yesterday"` khiến mọi lời gọi bỏ qua nó
  thành 400 `param_invalid` — đổ lỗi cho **người gọi** vì lỗi của tác giả, và ghi vào
  `access_log` dưới một mã không chỉ về đâu cả.
- **Mỗi tham số tuỳ chọn không có `default` đều được render ở trạng thái VẮNG MẶT, riêng lẻ.**
  Vắng mặt là trạng thái của phần lớn lời gọi, và nó là một hợp đồng thứ hai chứ không phải
  biến thể của hợp đồng thứ nhất: tham số vắng mặt là `none`, nên `{{ sort | ident }}` nằm
  ngoài `{% if %}` render tốt khi có giá trị và là 500 khi không. Định nghĩa không render được
  khi thiếu tham số thì không publish được.

Các luật cần tới câu lệnh đã render thì được thực thi bằng cách **render**, không phải bằng
cách quét template: `check_publish` (cổng hành vi ở §4.2) chạy lúc publish và chỉ lúc publish
— nó render tới sáu mươi tư lần, đúng cho mỗi lần publish một phiên bản và vô lý nếu mỗi
request. Đường phục vụ giữ các luật nguồn và bất biến số placeholder == số bind.

**Lúc phục vụ**, trước khi render:

- Tên tham số được **percent-decode trước khi khớp** với `ParamSpec.name`. Hiện chúng được
  khớp ở dạng thô, nên `?d%61y=` bị bỏ và dùng chung cache entry với request không có `day` —
  vô hại khi chưa render, nhưng sai dữ liệu ngay khi phase này lên. Mìn này được ghi trong
  `serving/router.rs` và đây là lúc tháo nó.
- Tham số client gửi mà không khai báo thì **bỏ qua**, giữ nguyên hành vi P4 và không làm hỏng
  client có gắn tham số theo dõi.
- Giá trị được kiểm kiểu theo `ParamSpec.ty`. `Date`/`Timestamp` parse thành giá trị thời gian
  thật và bind theo đúng kiểu, không phải chuỗi.
- Thiếu tham số bắt buộc là 400; sai kiểu là 400.
- Tham số query **lặp lại** trở thành một LIST, mỗi giá trị một bind — `ParamSpec` không có
  trường bội số, nên bội số đến từ request. `Enum` lặp lại là 400: enum là một lựa chọn đơn và
  là kiểu duy nhất có thể chạm tới vị trí định danh.
- **Số lượng là của người gọi; HÌNH DẠNG là của định nghĩa.** Tham số mà định nghĩa lặp qua
  (`{% for r in regions %}`) vẫn là list ngay cả khi chỉ có một giá trị, vì một scalar ở đó
  không phải vòng lặp một phần tử — nó là "plain is not iterable", tức 500 cho request hết sức
  bình thường `?regions=EU`. Publish suy ra cùng tập đó bằng cùng một hàm, nên hình dạng mà
  định nghĩa được kiểm và hình dạng nó được phục vụ không thể lệch nhau.
- **S-5 — `+` giải mã thành DẤU CÁCH ở cả tên lẫn giá trị**, đúng cách đọc
  `application/x-www-form-urlencoded` mà `axum` và các client phổ biến sinh ra. Nó **âm thầm**
  đổi một giá trị `String` chứ không báo lỗi: `?token=a+b` bind `"a b"`. Bất cứ thứ gì có `+`
  trong bảng chữ cái — base64 chuẩn, một literal dạng `C++`, một số có dấu — đều bị hỏng trừ
  khi client gửi `%2B`. **Đã đo sống 2026-08-16**, trên các dòng chứa cả hai cách viết:
  `?region=EU+X` và `?region=EU%20X` đều trả về dòng `EU X`, chỉ `?region=EU%2BX` trả về dòng
  `EU+X`. Đây là hành vi đã tài liệu hoá, không phải lỗi cần sửa ở server: đổi nó sẽ phá cách
  đọc `+`-là-dấu-cách mà mọi HTML form và mọi thư viện HTTP client đều đang sinh ra. **Tài
  liệu endpoint gửi cho client phải nói điều này**, vì không có gì trong response phân biệt
  "bạn nhận đúng dòng bạn hỏi" với "bạn đã hỏi một giá trị khác thứ bạn gõ".

**Thông báo lỗi không bao giờ nêu tên tham số.** Tên là của tác giả endpoint, và nêu tên biến
bề mặt lỗi thành danh mục các tham số mà endpoint khai báo. Đây là nhắc lại §7 của spec gốc,
giờ khi các mã lỗi đó thực sự với tới được.

Một **tham số tuỳ chọn không có giá trị thì đơn giản là khối `{% if %}` của nó bị bỏ đi**, và
đó là lý do luật "mọi tham số phải `required` hoặc có `default`" — từng cân nhắc lúc thiết kế —
**không được áp dụng**: điều kiện làm nó thành thừa, và nó sẽ cấm mất hình dạng REST phổ biến
nhất là bộ lọc tuỳ chọn.

## 6. Cache key

Hai quy tắc của §6.3 giữ nguyên: mọi trường có tiền tố độ dài, principal đứng cuối. Key phủ
`endpoint_id ‖ version ‖ declared params ‖ principal`, và **sự vắng mặt** của một tham số phải
biểu diễn được và khác với sự có mặt — lời gọi có `region` và lời gọi không có **không được**
dùng chung một entry, vì giờ chúng thực thi hai câu SQL khác nhau.

## 7. Tương thích

Endpoint publish **trước** phase này mà có khai báo tham số sẽ bắt đầu render chúng. Kiểm tra
lúc publish là nơi mọi luật mới sống, nên một snapshot cũ đi vòng qua tất cả chúng — một tham
số `location: body` âm thầm bỏ mất một bộ lọc, một `default` biến mọi lời gọi mặc định thành
400, một `| ident` là 500 mỗi khi người gọi bỏ qua giá trị.

Trước đây điều này được viết như một **chỉ dẫn** — "mọi endpoint có tham số phải publish lại
sau P4.5" — và chỉ dẫn thì không phải sự thực thi. **Snapshot ghi lại nó đã qua bản nào của
cổng kiểm** (`EndpointSnapshot::param_gate`, do publish đóng dấu và không nơi nào khác), và
một phiên bản có khai báo tham số nhưng mang giá trị nhỏ hơn `PARAM_GATE` hiện tại sẽ bị từ
chối lúc phục vụ bằng 503 `param_gate_stale` — trước khi nó được tính khoá cache, tra cache
hay render. Snapshot trước P4.5 deserialize thành `0` nên bị từ chối; publish lại một lần là
xong. Phiên bản không có tham số thì không render gì, nên không bị ảnh hưởng.

Cùng cơ chế đó gánh luôn lần chuyển đổi **kế tiếp**: khi một bản cổng kiểm sau này bịt một
luật mà bản này bỏ sót, việc tăng `PARAM_GATE` sẽ từ chối mọi snapshot mà cổng cũ đã duyệt,
cho tới khi chúng đi qua cổng mới. Đó là lý do trường này là số nguyên chứ không phải boolean
— "đã có một cổng nào đó chạy" chính là lời khẳng định lần này đã không đủ.

## 8. Ngoài phạm vi

- **Ghi dữ liệu.** Không đổi so với §10.
- **Bộ thẻ kiểu MyBatis.** `{% if %}`/`{% for %}` đã phủ; `<set>` là để ghi, `<bind>` là đánh
  giá biểu thức, còn `<where>`/`<trim>` chỉ là tiện nghi so với `WHERE 1=1`.
- **Chuyển hai bề mặt nội suy còn lại.** Sau phase này egret có **ba** bề mặt dựng SQL, không
  phải hai. `egret-bi/src/chartquery.rs::sql_literal` nội suy giá trị filter của chart và
  dashboard — đúng bề mặt mà vector backslash của P1 thực sự với tới được chỉ với quyền
  `chart:read`. `egret-query/src/execution/template.rs` nội suy tham số của SQL Lab; chính doc
  của nó ghi "deliberately NOT a general template engine". Cả hai đều là call site của
  `quote_string` và cả hai đều từng là call site của lỗi P1. Chuyển chúng là phần thưởng lớn
  hơn; riêng BI đã động vào 19 chart và khối lượng test lớn, và trộn vào đây sẽ buộc một lượt
  review phải giữ trong đầu cùng lúc cả phân tích injection lẫn phân tích hồi quy BI. **Cho tới
  khi xong, egret mang hai mô hình an toàn song song — Data API an toàn theo cấu trúc, hai cái
  kia an toàn nhờ escaper — và điều đó phải được ghi ở đầu MỖI module nội suy, không chỉ ở đây:
  người đọc `chartquery.rs` không có lý do gì để mở tài liệu này.**

  Còn một cái bẫy tài liệu đang sống: `egret-dataapi/src/lifecycle.rs` ghi ở `:551`, `:592` và
  `:629` rằng kiểm tra của P3 khớp với "what `template::render` will do at serve time".
  **Đường phục vụ của Data API chưa bao giờ gọi nó** — bộ render đó là của SQL Lab. Các luật
  của P3 vẫn đúng và `PlaceholderInsideQuotedText` chính là thứ khiến bind khả thi; chỉ có
  phần mô tả hành vi lúc chạy trong comment là sai. Phase này sửa chúng.

## 9. Nghiệm thu

**Tiêu chí quan trọng nhất.** Với **mỗi** engine, một test khứ hồi bind một ngày, một decimal,
một chuỗi có số 0 ở đầu (`"007"`), một `NULL`, một backslash và một dấu nháy đơn, rồi khẳng
định **chính xác** chuỗi JSON trả về. Chạy trên engine thật. **Stub phần cài đặt một lần và xác
nhận khẳng định thực sự đỏ.**

Lỗi ngày tháng (§3.3) làm hỏng dữ liệu trong khi **mọi unit test hiện có vẫn xanh**, vì hàm mà
các test đó phủ không còn được gọi. Một test chưa từng được chứng kiến đỏ thì không chứng minh
được gì.

Ngoài ra:

- Test giá trị thù địch của Trino phải khẳng định kết quả là **dữ liệu**, chứ không khẳng định
  "không có lỗi": một biểu thức vô hướng được chèn khéo sẽ parse thành công và không bao giờ
  hiện ra như lỗi cú pháp.
- Template chứa `{% filter %}` bị từ chối lúc publish, và bất biến placeholder-bằng-bind đúng
  với mọi template đã lưu.
- `describe` trên SQL có tham số trả về cột thật ở cả hai engine, và danh sách cột rỗng làm
  publish thất bại.
- Một kiểm tra CI khẳng định `set_loader` và `path_loader` không bao giờ được gọi (§4.5).

**Kiểm tra sống, theo §9 của spec gốc.** Publish một endpoint có tham số, gọi nó với hai giá
trị khác nhau, và **xác nhận kết quả thực sự khác nhau** — đó chính xác là thứ hôm nay không
hoạt động và là lý do phase này tồn tại. Sau đó gọi với tham số vắng mặt và xác nhận khối
`{% if %}` đã bị bỏ đi chứ không phải bind `NULL`.

**ĐÃ THỰC HIỆN 2026-08-16 trên StarRocks** (dev, `localhost:9030`), toàn bộ transcript ở
`.superpowers/sdd/2026-08-15-data-api-p45-parameters/task-8-report.md`. Tóm tắt: `?region=EU`
và `?region=US` trả về hai tập dòng rời nhau; thiếu `region` trả về **mọi** dòng, tức khối
`{% if %}` đã bị bỏ đi chứ không bind `NULL` (bind `NULL` sẽ trả về không dòng nào);
`?d=2020-02-29` khớp đúng ngày nhuận, tức date bind như một date; `?regions=EU&regions=US`
render `IN (?, ?)` còn `?regions=EU` một giá trị render `IN (?)` chứ không 500;
`describe_as_with_params` trả về năm cột có kiểu từ chính call site publish, trên một template
đầy tag `{% if %}`; và một snapshot bị xoá `param_gate` bị từ chối **503 `param_gate_stale`**.
Các giá trị thù địch (`EU' OR '1'='1`, một `\` ở cuối) quay về dưới dạng **dữ liệu** — không
dòng nào, HTTP 200 — không bao giờ là lỗi và không bao giờ là dòng thừa.

**CŨNG ĐÃ THỰC HIỆN 2026-08-16 trên Trino** (Trino 481, một coordinator dựng đúng theo cấu hình
mà chính egret provision — xem bên dưới). Cùng một chuỗi, cùng kết quả: `?region=EU` → id
`{1,2}` và `?region=US` → id `{3,4}`; thiếu `region` → đủ năm dòng; `?d=2020-02-29` → hai dòng
ngày nhuận; `IN`-list ở 2, 1 và 3 giá trị với ví dụ chuẩn ở §4.3 publish **nguyên văn**; và
`describe_as_with_params` trả về năm cột có kiểu từ chính call site publish qua `DESCRIBE OUTPUT`
của Trino.

Hai điều lần chạy này thêm được mà lần StarRocks không thể:

1. **Văn bản `EXECUTE … USING` đã render được đọc ngược lại từ coordinator**
   (`system.runtime.queries`), nên khẳng định ở §3.1 là **quan sát được** chứ không phải suy
   luận. Trino nhận `EXECUTE p1 USING 'EU', DATE '2020-02-29'` — một literal date **có kiểu** —
   và với payload `a' || 'b` thì nhận `EXECUTE p1 USING 'a'' || ''b', 'a'' || ''b'`. **Dấu nháy
   đã được nhân đôi.** Escape hỏng sẽ render `'a' || 'b'`, Trino tính ra `"ab"` với HTTP 200 và
   không lỗi; thực tế giá trị quay về đúng chuỗi 8 byte. Tương tự
   `x' || current_user || 'y` quay về 24 byte thay vì rò rỉ principal của engine, và một `\` ở
   cuối sống sót nguyên một byte — Trino không coi đó là ký tự escape nên **không được** nhân
   đôi ở đây, ngược hẳn với quy tắc của StarRocks.
2. **Sai lệch cách viết kiểu ở Task 4 đã được đo lại và chứng minh việc chuẩn hoá là có tác
   dụng thật tại publish.** Trên coordinator này, `DESCRIBE OUTPUT` thô trả về `decimal(10,2)`
   còn response của execute trả về `decimal(10, 2)`. Hai endpoint trên cùng một projection —
   một có tham số, một không, tức hai đường describe khác nhau — đều đóng băng
   `decimal(10, 2)` vào snapshot. `varchar(20)` giống nhau ở cả hai đường.

**Ghi chú môi trường, vì đây là một cái bẫy sống chứ không phải lỗi code.** Coordinator Trino
trong docker dev (`deploy/trino/etc/config.properties`) tin JWKS của *Keycloak* với
`required-audience=trino`. Nguồn phát engine-token duy nhất của egret là
`MachineIdpEngineMinter`, ký `aud: "engine"` bằng khoá của chính egret; không trường
`ConnOptions` nào mang bearer tĩnh, và nhánh non-delegated lẫn nhánh impersonate của
`delegation_for` đều không gửi bearer. **Không có thứ gì egret mint được mà coordinator đó chấp
nhận**, nên nó không bao giờ phục vụ được một request Data API. File đó đã cũ so với chính cấu
hình egret provision — ConfigMap `demo-trino-coordinator` trong cụm kind ghi
`key-file=<egret>/.well-known/jwks.json` với `required-audience=engine`, và đó là cấu hình lần
kiểm này dùng. Liên quan: `POST /databases/{id}/test` dựng kết nối với `bearer_token: None`
(`registry/tester.rs`), nên nó báo `{"ok":false,"detail":"forbidden"}` với **bất kỳ** Trino
chỉ-JWT nào kể cả khi endpoint đã publish vẫn chạy đúng — đừng đọc đó là kết nối hỏng.

## 10. Rủi ro còn lại, nói thẳng thay vì để ngầm

- **Danh sách `USING` của Trino là văn bản do ta render.** Một lỗi ở đó đánh giá được biểu thức
  vô hướng bất kỳ (`current_user`, `now()`). Nó không mở được câu lệnh thứ hai hay subquery —
  đã đo — nhưng không phải bằng không, và nó sẽ **không** hiện ra như lỗi cú pháp.
- **minijinja không có chính sách bảo mật.** Hôm nay không CVE nào — đã kiểm qua RustSec, GHSA,
  NVD và OSV kèm truy vấn đối chứng. Nhưng không có `SECURITY.md`, không có kênh báo lỗi, và
  các lỗi làm sập tiến trình từ văn bản template đã từng ship dưới dạng dòng changelog bình
  thường — bản 2.21.0 sửa một stack overflow do ghép chuỗi lặp lại, core-dump sau khoảng 1500
  lần ghép. Ghim phiên bản, theo dõi changelog, và **đừng giả định một cú abort không thể làm
  chết tiến trình**.
- **Crate không có giới hạn kích thước output.** Sink giới hạn và timeout ở §4.5 là phòng thủ
  duy nhất.
- **`path_loader` không bỏ ra khỏi bản biên dịch được.** Khả năng chạm filesystem luôn cách một
  lệnh gọi nhầm, vĩnh viễn; lệnh grep trong CI là chốt kiểm soát.
- **minijinja 3.0 đổi các feature mặc định** — bản alpha đã bỏ `serde` và `deserialization`
  khỏi danh sách mặc định. Chạy lại bản rà soát bề mặt trước khi nâng cấp, đừng giả định nó
  được giữ nguyên.
- **Ranh giới tin cậy không đổi vì tất cả những điều trên.** Tác giả template viết SQL thô theo
  thiết kế; `EmitRaw` là kênh đó và vốn luôn là vậy. Mọi việc siết ở trên bảo vệ trước **API
  client**, người chỉ cung cấp được giá trị context và giá trị đó **chứng minh được** là không
  thoát khỏi formatter. Ai được quyền soạn và publish endpoint mới là thứ làm công việc bảo mật
  thật sự, và điều đó nằm trong phân quyền của egret, không nằm trong minijinja.
