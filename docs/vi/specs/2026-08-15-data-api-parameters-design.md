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

`{% macro %}` và `{% call %}` biến mất cùng feature `macros`. **`{% filter %}` và
`{% set x %}…{% endset %}` được parse vô điều kiện và không feature nào bỏ được**, nên kiểm lúc
publish là đòn bẩy duy nhất: template chứa một trong hai sẽ bị từ chối.

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

Render vào một sink giới hạn kích thước và dưới một timeout theo đồng hồ. minijinja **không có
giới hạn kích thước output**; fuel giới hạn lệnh chứ không giới hạn byte, và `'a' * 99_999_999`
là hợp lệ dưới ngưỡng chuỗi lặp 100 MB.

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

Endpoint publish **trước** phase này mà có khai báo tham số sẽ bắt đầu render chúng. Chưa có gì
chạy production, nên việc chuyển đổi được **nói ra** chứ không tự động hoá: **mọi endpoint có
tham số phải publish lại sau P4.5.** Kiểm tra lúc publish là nơi các luật mới sống, nên một
snapshot cũ sẽ đi vòng qua tất cả chúng.

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
