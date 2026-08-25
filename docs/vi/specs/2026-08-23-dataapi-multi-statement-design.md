# Một endpoint có thể chạy nhiều câu lệnh, và schema của nó có thể đổi theo tham số

Gói **M** của chương trình Data API. Nó làm hai việc có chung một nguyên nhân: cả hai đều
bị ràng buộc bởi thứ mà `EndpointSnapshot` đóng băng lúc publish.

Mọi khẳng định về hành vi engine dưới đây đều được **đo trước khi viết**. Hai bản probe là
`.superpowers/sdd/multi-statement-transaction-probe.md` và
`.superpowers/sdd/frozen-columns-vs-conditional-projection.md`; tài liệu này trích dẫn
chúng chứ không chép lại, và đánh dấu rõ những gì **chưa xác lập được**.

## 1. Những gì đã đo

### 1.1 Không transaction nào bắc qua được hai câu lệnh, trên bất kỳ engine nào

Tầng quyết định là của chính egret, không phải của engine. `Connector::execute` đứng hoàn
toàn một mình: StarRocks gọi `Conn::new(...)` rồi `disconnect()` **bên trong** `execute` —
thậm chí không phải mượn từ pool — còn Trino dựng một `reqwest::Client` mới mỗi lần gọi.
Hai lời gọi `execute` trên cùng một `Connector` trả về `CONNECTION_ID()` **33564235** và
**33564236**; `SET @probe=42` đọc lại ra `null`; `SET autocommit=0` đọc lại ra
`@@autocommit=1` — trong khi **mọi câu lệnh đó, kể cả `START TRANSACTION`, đều trả `OK`**.

Không có phiên nào để giữ transaction, nên năng lực của bản thân engine là chuyện vô nghĩa
cho tới khi trait `Connector` thay đổi. Dưới tầng đó cả hai engine đều hỏng nốt: Trino trả
lỗi cứng `EngineError("Client does not support transactions")` trên đường đi của egret, vì
transaction id quay về trong một **HTTP response header** mà `TrinoResp` — vốn chỉ
deserialize body — không bao giờ đọc được. StarRocks chấp nhận `START TRANSACTION` mà không
cô lập gì — một commit song song trở nên nhìn thấy được **bên trong** transaction đang mở —
và `ROLLBACK` **im lặng giữ lại dòng đã ghi**.

**Hệ quả cho spec này:** M chạy các câu lệnh như N lần đọc độc lập và **không hứa hẹn nhất
quán giữa chúng**. §4.5 nói chỗ điều đó được ghi lại cho người gọi.

### 1.2 Ghim snapshot Iceberg thì chạy được, trên cả hai engine, không sửa gì

`FOR VERSION AS OF <snapshot_id>` ghim được kết quả đọc qua chính connector của egret trên
cả Trino lẫn StarRocks. Đúng cặp hình dạng M — hai lời gọi `execute` **riêng biệt**, cùng
ghim — trả về count 2 và đúng 2 dòng trong khi bảng thật đang có 5.

Dù vậy nó **không nằm trong M**; §7 nêu lý do.

### 1.3 Endpoint điều kiện publish xanh rồi 503 trên chính lời gọi mặc định của nó

Đây là **lỗi đang sống trên `main`**, và M1 tồn tại để vá nó.

Cột đầu ra được đóng băng đúng một lần, lúc publish: `lifecycle.rs:1546` gọi
`describe_as_with_params` với một bộ representative values duy nhất. Template điều kiện là
tính năng được hỗ trợ có chủ ý — `lifecycle.rs:1505` ghi rằng publish render template thật
qua renderer thật chính là để endpoint `{% if %}` describe được. Không có gì so cột của
nhánh này với nhánh kia, vì không có gì từng describe nhánh thứ hai.

Nên câu này publish được, trên cả hai engine:

```sql
SELECT id {% if detail %}, name, email {% endif %} FROM users
```

rồi:

| lời gọi | kết quả |
|---|---|
| `?detail=true` | `200`, ba cột |
| `?detail=false` | `503 {"code":"schema_drift"}`, log `frozen=["id","name","email"] live=["id"]` |
| không truyền query string — đúng mặc định đã ghi trong tài liệu | `503` |

OpenAPI sinh ra còn tệ hơn là mô tả sai tập cột: nó **quảng cáo những lời gọi không thể
thành công**. `detail` khai `"required": false` trong khi bỏ trống thì 503, và một tham số
`mode` khai `"enum":["summary","full"]` trong khi `mode=full` thì 503.

Nhánh nào được đóng băng là do representative values quyết định (`lifecycle.rs:1108`):
Boolean → `true`; Enum → **`allowed[0]`**. **Thứ tự tác giả viết `allowed` quyết định hợp
đồng được publish.** Đảo `["summary","full"]` thành `["full","summary"]` là đổi một schema
công khai, không một tiếng động.

**Điều cứu nó khỏi tệ hơn:** `shape_rows` (`serving/router.rs:1113`) từ chối khi tên/số cột
lệch **trước khi** shape, và lấy mọi khoá của response từ `snap.columns`, nên không thể có
cột nào bị rơi hay dán nhãn sai. Nó là một **cổng**, không phải bộ định dạng. Chính tính
chất đó là lý do §3 giữ lại hợp đồng cột thay vì theo câu trả lời của ngành ở §6.

**Nhưng kiểu thì cố ý không được so**, và lỗ đó là thật:
`SELECT id, {% if detail %}name{% else %}id{% endif %} AS v` phục vụ **200** với `v` là số
nguyên trong khi `meta.columns` và OpenAPI đều khai `varchar` / `"type":"string"`. Giống hệt
trên Trino và StarRocks.

### Chưa xác lập được

- Hành vi của một lần đọc đã ghim sau khi snapshot Iceberg hết hạn.
- Time travel trên REST catalog thật của egret. Phép đo chạy trên một Iceberg catalog nền
  JDBC, vì catalog `iceberg` trong dev đã chết — nó vẫn trỏ `polaris:8181`, thứ đã bị gỡ
  khỏi compose, và trả `Cannot obtain metadata`.
- Liệu transaction của Trino có ghim được kết quả đọc hay không nếu header được mang theo.
  Vô nghĩa dưới §1.1, và không được chạy thử.

### Một quả mìn triển khai không liên quan, ghi lại vì đã nhìn thấy

egret **luôn** gửi `X-Forwarded-Proto: https` tới Trino. Một coordinator không bật
`http-server.process-forwarded=true` trả **406 cho mọi request**. Coordinator trong dev có
bật nên không cắn ở đây; một image Trino stock thì có. Một quan sát, chưa điều tra thêm.

## 2. Phạm vi

**Trong:** nhiều câu lệnh chỉ-đọc trên một endpoint; một hợp đồng cột sống sót qua rẽ
nhánh; một vỏ response cố định; lỗ so kiểu ở §1.3.

**Không đổi, và chịu lực:** `ALLOWED_METHODS = ["GET", "POST"]` (`endpoints.rs:319`); chỉ
`SELECT` hoặc `WITH … SELECT` mới publish được; `#` bị cấm hẳn và `;` bị từ chối ở hai tầng
trước publish (`reject_unscannable_bytes`, rồi lượt quét bóc comment/literal ở
`lifecycle.rs:601`) và một lần nữa sau render (`render.rs:856`); OAuth2
`client_credentials` trên machine-IdP sẵn có là credential duy nhất.

**Ngoài:** ghi và transaction — **gói D bị huỷ**, xem §7; ghim snapshot; chế độ soạn thảo
bằng script; Z1 (IP allow/deny và endpoint công khai), vẫn hoãn.

M ship thành hai PR. **M1** là hợp đồng cột và vá §1.3; nó không đụng vỏ lẫn danh sách câu
lệnh. **M2** là nhiều câu lệnh và cái vỏ. M1 trước, vì cột theo từng câu lệnh của M2 chính
là hợp đồng của M1 lặp lại N lần.

## 3. M1 — cột khai báo là hợp của các nhánh, và tầng serving cưỡng chế nó

### 3.1 Publish describe mọi permutation render ra khác nhau

`probe_assignments` (`render.rs:1213`) đã liệt kê tới `MAX_PROBES = 64` bộ gán tham số cho
`check_dynamic`, theo vòng tròn để ngân sách hao đều trên các tham số thay vì bị tham số
sắp trước ăn hết. M1 dùng lại nó, và thêm một bước làm cho chi phí trở nên trung thực:

1. render từng bộ gán — máy móc đã có;
2. **khử trùng lặp theo chuỗi SQL đã render** — mới;
3. `describe_as_with_params` từng SQL **khác nhau** — trước đây là đúng một lời gọi.

Một template không có điều kiện render ra một chuỗi duy nhất, nên nó tốn **đúng một lần
describe, y như hôm nay**. Một `{% if %}` tốn hai. Chi phí bám theo số truy vấn khác nhau
về cấu trúc, không bám theo số tham số.

### 3.2 Hợp đồng

```rust
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
    pub required: bool, // MỚI: có mặt ở MỌI permutation đã describe
}
```

`columns` trở thành hợp của các permutation, xếp theo thứ tự xuất hiện lần đầu. Cột có mặt
ở mọi permutation là `required: true`; cột chỉ xuất hiện dưới một nhánh nào đó là
`required: false`. Đó đúng là ngữ nghĩa của một response thưa, và OpenAPI diễn đạt nó bằng
chính danh sách `required` của nó.

**Một tên xuất hiện với hai kiểu khác nhau giữa các permutation thì từ chối publish.** Hai
kiểu không thể hợp lại thành một lời khai trung thực, và đây chính là lỗ đang phục vụ cú
`200` im lặng ở §1.3. Từ chối là câu trả lời đúng duy nhất.

### 3.3 Tầng serving cưỡng chế nó, và fail đóng

`shape_rows` giữ nguyên vị trí một cái cổng; vị từ của nó đổi từ *bằng nhau* thành *chứa
trong*, và có thêm phép so kiểu:

- mọi tên trả về phải **nằm trong** `snap.columns` — nếu không, `503 schema_drift`;
- mọi cột `required` phải **có mặt** — nếu không, `503 schema_drift`;
- kiểu của cột trả về phải **bằng** kiểu đã khai — nếu không, `503 schema_drift`;
- response `objects` mang một khoá cho mỗi cột lần chạy này **thực sự trả về**; cột khai
  `required: false` mà không được trả về thì đơn giản là **vắng mặt** khỏi object.

`ServingMeta.columns` giữ nguyên ý nghĩa và nguyên comment hiện tại — hợp đồng đã đóng
băng, không phải hình dạng của lần chạy này. Nay nó là *hợp*, nên nó vẫn là mô tả đúng và
đầy đủ về những gì endpoint có thể trả về, còn danh sách `required` trong OpenAPI mới là
thứ nói cho người dùng biết khoá nào chắc chắn có. Không có gì về `meta.columns` thay đổi.

### 3.3.1 `output.shape: "arrays"` thì không được rẽ nhánh

`shape_rows` so danh sách cột sống và cột đóng băng **theo vị trí**, vì shape `arrays` là
theo vị trí, và `data_schema` ghi thứ tự cột bằng lời văn cũng vì lẽ đó — OpenAPI 3.0 không
có kiểu tuple.

Một tuple theo vị trí mà số phần tử đổi theo tham số thì không phải hợp đồng: `[[1]]` dưới
một tập khai `[id, name, email]` thì không ai đọc được. Nên **một endpoint có
`output.shape` là `arrays` mà các permutation của nó không cùng describe ra một tập cột,
cùng một thứ tự, thì bị từ chối publish.** Tác giả muốn projection rẽ nhánh thì dùng
`objects`, nơi một khoá hoặc có hoặc không, không bao giờ nhập nhằng.

Việc này giữ nguyên si đường đi theo vị trí như hôm nay, và giam mọi thay đổi của gói này
vào đường `objects`.

**Cả hai luật về `output` đều chạy trong `validate()`, TRƯỚC transaction của publish, mà
`output` lại patch được độc lập** — nên phép so sánh chống lỗi thời của publish phải tính cả
nó, bên cạnh `sql`, `database_id` và `params`. Thiếu điều đó, tác giả có thể validate một
định nghĩa rẽ nhánh ở dạng `objects`, để một `PATCH` đặt `shape: arrays` rơi vào giữa các
vòng describe, và đóng băng một snapshot `arrays` trên một union rẽ nhánh: `503` vĩnh viễn
cho mọi nhánh trừ nhánh rộng nhất. Chạy lại validator thuần sau lock không bịt được — nó
không thấy `output` và không describe gì cả — nên `output` được xử như `sql`: một conflict
có thể thử lại.

### 3.3.2 `output.naming` không được va chạm xuyên nhánh

`shape_rows` và `data_schema` đều áp `output.naming` lên **union đóng băng** và từ chối khi
va chạm; phép kiểm trùng lặp phía publish chỉ so tên **thô**. Trước khi có union, danh sách
đóng băng là cột của một nhánh, nên muốn va chạm thì hai cách viết phải nằm trong cùng một
projection — mà phép kiểm tên thô đã bắt được. Chính việc mở rộng danh sách đóng băng thành
union đã làm cho va chạm *xuyên nhánh* trở nên khả dĩ.

`SELECT {% if d %}total_revenue{% else %}totalRevenue{% endif %} FROM t` với `naming: camel`
đưa cả hai tên vào union mà không câu lệnh nào lặp tên. Nó publish xanh, rồi mọi lời gọi ở
cả hai nhánh đều trả `503 output_naming_collision` — và OpenAPI sinh ra hỏng ở mức **tài
liệu**, kéo theo mọi endpoint không liên quan trong workspace. Đó là hỏng hóc duy nhất trong
gói này có bán kính ảnh hưởng vượt ra ngoài một endpoint.

Vì vậy đúng phép gấp `apply_naming` ấy chạy trên union đã describe, ngay trong `validate()`,
cạnh phép kiểm `arrays`, và trả lại cho tác giả cả hai tên đã khai.

Ngân sách 64 probe là một cái trần, không phải bằng chứng vét cạn: một template có thể rẽ
theo nhiều tổ hợp hơn 64. **Điều này đã được đo, không phải phỏng đoán:** năm tham số
không-enum, mỗi cái thu hoạch mười sáu literal, vắt cạn đúng ngân sách và các ứng viên
cuối của tham số sắp cuối bảng chữ cái không bao giờ được probe. Một lượt chạy đối chứng
với cùng số tham số đói ở đúng điểm đó, nên giới hạn nằm ở chính ngân sách round-robin và
có trước gói này. Hệ quả là một endpoint nhiều tham số như vậy vẫn có thể gặp một nhánh mà
publish chưa từng describe.

**Đường thứ ba, do review toàn nhánh tìm ra: chính `default` của tham số.** Một tham số có
`default` thì không bao giờ vắng — serving thay thế bằng default — nên nó không có ngữ cảnh
vắng nào, và đường duy nhất còn lại để default lọt vào tập render là làm *candidate* của
probe. Với **enum**, đường đó vốn không tồn tại: `render::candidates` trả về đúng danh sách
`allowed` của enum và không bao giờ đọc literal của định nghĩa, còn danh sách đó bị cắt tại
`MAX_ENUM_CANDIDATES = 8`. Một enum có mười hai giá trị `allowed` với `default` ở chỉ số 10
vì thế publish xanh, rồi trả `503 schema_drift` cho lời gọi **không có query string** — đúng
lỗi §1.3, qua đúng cái kiểu tham số lẽ ra được mô tả chặt nhất. Một `string` thường thì đã
được probe bằng chính giá trị ấy, vì scalar không-enum thu candidate từ literal của định
nghĩa. Đã vá theo hai đường, mỗi đường được ghim bởi một test mà đường kia không thoả được:
`default` khai báo nay là một **ngữ cảnh** (`lifecycle::resting`, luôn được render, không thể
bị đói), và `MAX_ENUM_CANDIDATES` được nâng lên `MAX_PROBES`.

**Một đường thứ hai, độc lập, khiến publish bỏ sót nhánh**, tìm thấy lúc review: các ngữ
cảnh vắng-tham-số là `{không vắng cái nào}`, `{vắng từng cái một}`, và `{vắng hết}` — thừa
hưởng từ `check_render_rules`, vốn xưa nay chỉ dựng đúng tập đó. **Các cặp thì thiếu.** Với
ba tham số tuỳ chọn, một định nghĩa rẽ theo tổ hợp kiểu `(a and b) or (a and c)` sẽ describe
như thể cột đó luôn có mặt, và người gọi chỉ truyền một trong hai sẽ gặp một cột mà publish
đã đóng băng là `required`. Hơi khiên cưỡng, nhưng cùng loại với giới hạn ngân sách ở trên,
và an toàn vì cùng một lý do: phép kiểm lúc serving fail đóng. Vì vậy phép kiểm lúc serving là lưới đỡ cho mọi thứ publish không
với tới, và nó **phải fail đóng**. Một cột không được khai không phải lỗi tài liệu — nó là
một trường chưa ai duyệt lúc publish đang đi tới tay người gọi.

### 3.4 Việc này vá được gì

Bảng ở §1.3 thành `200` ở mọi dòng, với `?detail=false` trả `{id}` và `?detail=true` trả
`{id, name, email}`; `name` và `email` khai `required: false`, `id` khai `required: true`;
OpenAPI thôi quảng cáo những lời gọi không thể thành công; và mối nguy thứ-tự-enum *giảm*
chứ không biến mất — lần render đại diện không còn một mình quyết định hợp đồng, nhưng không
phải mọi giá trị `allowed` đều được describe.

**Vế cuối trước đây viết "mối nguy thứ-tự-enum biến mất, vì mọi giá trị `allowed` đều được
describe chứ không chỉ `allowed[0]`", và câu đó SAI khi enum có hơn tám giá trị.**
`render::candidates` cắt danh sách của enum tại `MAX_ENUM_CANDIDATES`, khi đó là `8`. Đo qua
đúng route publish thật, một enum sáu mươi tư giá trị `allowed` mà giá trị ở chỉ số 62 bật
thêm một cột: **một describe** ở cap cũ, nên cột đó không bao giờ vào union và `?p=v62` chỉ
có thể là `503`. Cap nay là `MAX_PROBES`, và trường hợp đó describe hai câu lệnh. Đây là
**tồn dư thứ ba**, bên cạnh hai cái dưới:

* `MAX_PROBES` đếm cả ngữ cảnh gốc, nên một tham số đơn lẻ với tới tối đa sáu mươi ba
  candidate, và mọi tham số chia nhau ngân sách đó theo vòng tròn. Một enum có nhiều giá trị
  `allowed` hơn mức ngân sách với tới — hoặc vài enum cùng chia ngân sách — vẫn còn nhánh
  chưa được describe. Nâng cap thêm nữa không sửa được; chỉ ngân sách probe mới sửa được.
* `default` khai báo của tham số miễn nhiễm với tất cả những cái đó, vì nó là ngữ cảnh chứ
  không phải candidate. Đó là cố ý: default là giá trị mà mọi lời gọi không query string đều
  gửi, nên nó là giá trị duy nhất không được phép thua trong cuộc tranh ngân sách.

## 4. M2 — nhiều câu lệnh, và một cái vỏ

### 4.1 Danh sách câu lệnh

```rust
pub struct StatementSnapshot {
    pub name: String,               // ^[a-z][a-z0-9_]*$, duy nhất trong endpoint
    pub sql: String,
    pub columns: Vec<ColumnSpec>,   // §3.2, đóng băng theo từng câu lệnh
}

pub struct EndpointSnapshot {
    pub statements: Vec<StatementSnapshot>, // thay cho `sql: String` và `columns`
    // …mọi thứ khác không đổi
}
```

**Mỗi `sql` đi qua đúng cổng một-câu-lệnh hiện có, không sửa gì.** Dây không bao giờ mang
`;`. Nới quy tắc đó ra để "hỗ trợ multi-statement" là xoá một phòng thủ được dựng từ một
bypass StarRocks đã chứng minh sống, trong đó `#` được scanner đọc là comment còn engine đọc
là không có gì cả. Nhiều câu lệnh là tính chất của **cấu hình**, không bao giờ là tính chất
của một chuỗi đưa cho engine.

Các câu lệnh được **đặt tên**, không đánh theo vị trí. Tên tự mô tả trong response, và nó
không vỡ khi tác giả chèn thêm một câu lệnh vào giữa. §6 ghi rằng SQLREST chọn vị trí và vì
sao ta làm khác.

### 4.1.1 M1 đã chiếm mất từ "statement", và nó mang nghĩa khác

Viết sau khi M1 đã lên, vì §4.1 ở trên ra đời trước nó.

Trong mã đã ship, **"statement" nghĩa là một permutation đã render của MỘT định nghĩa
SQL**: `MAX_PUBLISHED_STATEMENTS = 64` chặn số *nhánh* của một định nghĩa mà publish sẽ
describe, còn `DescribedBranches::per_statement` giữ một danh sách tên cột cho mỗi *nhánh*.
Ở §4.1 cùng từ đó lại nghĩa là **một trong N định nghĩa SQL của endpoint**. Hai nghĩa, một
module — và va chạm này không phải chuyện thẩm mỹ: sau M2, ai gặp `MAX_PUBLISHED_STATEMENTS`
sẽ hiểu là giới hạn số câu lệnh mỗi endpoint, mà nó không phải vậy.

M2 đổi tên cách dùng của M1 sang *branch* — `MAX_DESCRIBED_BRANCHES`, `per_branch` — và giữ
"statement" cho thứ mà tác giả viết ra. Việc đổi tên là cơ học và thuộc về một commit
riêng, đi trước mọi thay đổi hành vi, để diff hành vi còn đọc được.

**Và bản thân cái trần phải được quyết lại, vì trần của M1 nay sai hình dạng.** Publish
describe mọi nhánh của mọi câu lệnh, nên tổng là **tổng trên các câu lệnh**, không phải 64
cho mỗi câu — nếu không thì mười câu lệnh cho phép 640 lượt round-trip xuống engine từ một
lần publish. Giới hạn áp cho cả endpoint. `MAX_DECLARED_PARAMS = 32` giữ nguyên phạm vi
endpoint vì tham số dùng chung, nhưng chi phí render trong tiến trình mà nó chặn nay bị
nhân với N, và ca xấu nhất đã đo (~122 ms) phải **đo lại** chứ không mang sang.

### 4.1.2 Và một trần thứ ba, cho SỐ LƯỢNG câu lệnh

Viết sau khi đã đo lại theo yêu cầu ở §4.1.1. `MAX_STATEMENTS = 16` (`egret-dataapi-api`),
do `validate_statements` từ chối — nghĩa là ở create, ở PATCH và ở publish, trước lượt scan
đầu tiên và trước lượt describe đầu tiên rất xa.

**Đã đo, bản release, N = 1…1024.** Chi phí publish trước lượt describe đầu tiên, với câu
lệnh đắt nhất mà hai trần kia còn cho qua (32 tham số boolean, 530 nhánh): **121 ms mỗi câu
lệnh, tuyến tính theo N** — 0,99 s ở 8, 1,97 s ở 16, 7,9 s ở 64, **31,6 s ở 256**, toàn bộ
trả trước khi trần nhánh từ chối endpoint, trên một đường đồng bộ CHIẾM một worker của
tokio. Phía serving: 42 ms mỗi câu lệnh ở mọi N với engine giả tốn 40 ms (99,0 ms ở N = 2,
2,7 s ở 64), nên phần của egret chỉ ~2 ms và ca xấu nhất là `N × min(timeout_seconds, 300)`.

`64` là nơi trần nhánh ở §4.1.1 vốn đã chặn N — mỗi câu lệnh tốn ít nhất một lượt describe —
nhưng chỉ với câu lệnh không có nhánh nào, và chỉ *sau khi* cả N câu đã render xong. `16` là
8× endpoint chuẩn `rows` + `total`, đúng hệ số mà `MAX_DECLARED_PARAMS` lấy so với bốn tham
số ở §4.3, và là chỗ hai trần GẶP nhau: 64 / 16 = 4 nhánh mỗi câu lệnh, dưới mức đó thì số
câu lệnh từ chối, trên mức đó thì trần nhánh từ chối.

**Là một trần đếm, không phải ngân sách thời gian trải trên các câu lệnh.** Ngân sách chỉ có
thể hỏng GIỮA vòng lặp, và envelope ở §4.4 không diễn đạt được một phản hồi một phần: `data`
khoá theo tên câu lệnh và `meta.statements` mang một mục cho mỗi câu, nên một body có ba
trên bốn khoá là client đọc phải một lỗ hổng trong im lặng — đúng kiểu hỏng mà vòng lặp ở
§4.3 đã từ chối với drift giữa chừng. Chạy hết ngân sách rồi vứt tất cả thì tiêu hết tài
nguyên trước khi từ chối, và chỉ chạm tới nửa serving; chi phí publish ở trên trả trước khi
bất kỳ câu lệnh nào chạy. `limits.timeout_seconds` vẫn là núm cho thời gian MỘT câu lệnh.

**Không suy ra từ một mốc thời gian phản hồi.** `Ingress` đang triển khai không đặt
`proxy-read-timeout`, nên mặc định của ingress-nginx được áp — `proxy_read_timeout 60s`, đọc
ra từ chính config của controller đang chạy. Mặc định `timeout_seconds` là 30, nên hai câu
lệnh đã chạm mốc cắt đó trong ca xấu nhất, và không N nào làm ca xấu nhất lọt vào trong đó.

### 4.2 Tham số dùng chung

Một request mang một bộ tham số. `resolve_supplied` (`serving/params.rs:119`, đi tới từ
`resolve` ở `:108`) hiện nhận `&snap.sql` để quyết định template thật sự dùng những tham số
khai báo nào; nó lấy **hợp trên các câu lệnh**. Một tham số chỉ được một câu lệnh dùng vẫn
chỉ khai một lần, và ngữ nghĩa `required` của nó không đổi.

Thay đổi này thuộc về `resolve_supplied` và không thuộc chỗ nào khác. Chính comment của nó
ghi lý do: serving đi tới nó qua `resolve`, **preview gọi thẳng nó**, và điều đó là cố ý —
"có đúng một chỗ mà một tham số khai báo trở thành một `Param`". Tính hợp ở bất kỳ đâu phía
trên sẽ khiến preview và serving bất đồng về cùng một endpoint. Chương trình này đã ship một
lỗi đúng hình dạng đó rồi, khi preview báo `who=admin` còn lời gọi thật báo service account.

### 4.3 Thực thi và giới hạn

Các câu lệnh chạy **tuần tự**, mỗi câu qua một `Connector::execute` riêng. `max_rows` áp
dụng **cho từng câu lệnh**: một câu đếm trả về một dòng không được ăn vào ngân sách của câu
lấy dữ liệu.

### 4.4 Cái vỏ

```json
{ "status": "ok", "message": null, "data": …, "meta": { … } }
```

- **`status`** — `"ok"` hoặc `"error"`. Nó **phản chiếu** mã HTTP; không bao giờ thay thế.
  egret vẫn trả `403` / `422` / `503` thật với `ServingErrorBody`. §6 ghi rằng SQLREST trả
  `200` cho mọi thứ và vì sao ta không theo.
- **`message`** — chuỗi cho người đọc, `null` khi thành công.
- **`data`** — các dòng, khi endpoint có một câu lệnh; một object khoá theo tên câu lệnh,
  khi có nhiều. Đây không phải union type ở mức hợp đồng: OpenAPI sinh theo từng endpoint,
  nên schema của mỗi endpoint vẫn đơn nghĩa.
- **`meta`** — `{ version, cached, row_count, columns }` khi một câu lệnh;
  `{ version, cached, statements: { <tên>: { row_count, columns } } }` khi nhiều câu.
  `meta` được giữ chứ không gộp đi, vì `version` là cách duy nhất client biết được hợp đồng
  đã được publish lại.

**Một công tắc theo từng endpoint**, `output.envelope: "wrapped" | "bare"`, mặc định
`wrapped`. `bare` chỉ ghi ra giá trị của `data`. Ý này lấy từ `USE_SYSTEM_RESPONSE_FORMAT`
của SQLREST và là phần duy nhất trong thiết kế response của họ đáng chép.

**Đây là breaking change với một API đang sống.** Hình hiện tại là `{data, meta}`, và các
endpoint P4 đã phục vụ traffic thật từ 2026-08-16.

**Đo trên dây sau khi cài đặt, và cách diễn đạt trước đó là nói quá:** với endpoint **một
câu lệnh**, body thành công là **thuần bổ sung** — `data` và `meta` giữ nguyên hình, chỉ
thêm khoá `status`/`message` — nên một consumer dễ tính **không hỏng gì cả**. Thứ hỏng là
consumer **nghiêm ngặt**: client từ chối trường lạ, hoặc client đọc `meta.row_count` trên
một endpoint nay có nhiều câu lệnh. Quyết định phá thay vì version hoá vẫn giữ; còn lời
khẳng định rằng mọi consumer đều hỏng thì không. Quyết định phá thay vì version hoá được
đưa ra có chủ ý: consumer còn ít và nội bộ, API chưa 1.0, và gánh hai hình vỏ mãi mãi đắt
hơn một lần migrate bây giờ. Migration đặt `envelope = "wrapped"` cho mọi endpoint hiện có,
và thay đổi này **phải xuất hiện trong ghi chú triển khai**. Nó không im lặng và không được
phép làm cho im lặng.

### 4.5 Người gọi được nói gì về tính nhất quán

N câu lệnh là N lần đọc độc lập (§1.1). Phần mô tả OpenAPI sinh ra cho bất kỳ endpoint nào
có hơn một câu lệnh sẽ nói thẳng điều này trong mô tả operation: các câu lệnh không dùng
chung transaction hay snapshot, nên một con số tổng và những dòng mà nó đếm có thể bất đồng
nếu bảng thay đổi giữa hai lần.

Ghi nó ra chính là toàn bộ lời hứa. §7 ghi cần gì để hứa mạnh hơn.

### 4.6 Cache key không đổi

Chuỗi khoá là `endpoint(16) ‖ version(4) ‖ params ‖ principal(16)`. Giá trị được cache là cả
response, và publish lại thì `version` tăng, nên danh sách câu lệnh đã được phủ sẵn. Ghi lại
vì "cache key phải đổi" là phỏng đoán hiển nhiên, và nó sai.

## 5. Soạn thảo

M ship một chế độ soạn thảo duy nhất: **SQL + Jinja**, như hôm nay, cho từng câu lệnh.

Điều này đáng nói ra vì nó từng là một câu hỏi mở thật sự. Đề xuất là một script sinh SQL,
với lý do tham số khác nhau thì cần SQL khác nhau. Tiền đề đó **đúng**, và nó **đã được đáp
ứng**: `render.rs` render lại trên **mỗi request**, với `{% if %}`, `{% for %}`, và
`{{ sort | ident }}`, còn `set_formatter` khiến `{{ x }}` phát ra `?` và đẩy một bind. Tham
số khác nhau đã sinh ra SQL khác nhau, một cách an toàn.

Một script trả về **chuỗi** SQL sẽ trả lại toàn bộ chỗ đó — mọi giá trị nó nội suy trở thành
SQL thô — và egret đã bị chính vết này cắt, ở ba call site, chứng minh sống trên StarRocks.
§6 ghi script engine của SQLREST là gì và nó trả giá bằng gì.

Cửa mở bằng bằng chứng, không mở bằng giả định: một endpoint thật mà Jinja không diễn đạt
nổi mới là thứ mở lại chuyện này.

## 6. Chỗ ta làm khác SQLREST và DBAPI, và vì sao

Đọc từ `/Users/1pro-l4r8/Documents/github/data-portal/sqlrest` tại `e580f60`.

| Câu hỏi | SQLREST | egret M | Vì sao |
|---|---|---|---|
| Vỏ | `ResultEntity{code, message, data}`, `code=0` là thành công | `{status, message, data, meta}` | Cùng ý. Thêm `meta` vì `version` không có chỗ trong ba khoá |
| Công tắc vỏ | `USE_SYSTEM_RESPONSE_FORMAT=false` → `data` trần | `output.envelope: "bare"` | Chép |
| Mã HTTP | Luôn `200`; `HttpApiServlet.process` không bao giờ gọi `setStatus` | `403`/`422`/`503` thật | Luôn-200 phá cache HTTP, proxy, monitoring, và buộc mọi client mở body ra mới biết là hỏng |
| N kết quả | `execute(...) → List<Object>`; `results.size() > 1 ? results : …` — **theo vị trí** | khoá theo tên câu lệnh | Tên sống sót khi tác giả chèn thêm câu lệnh vào giữa |
| Lưu câu lệnh | Mỗi câu một dòng, `SQLREST_API_CONTEXT(api_id, sql_text)` | `Vec<StatementSnapshot>` trong snapshot | Cùng hình; egret đóng băng cột kèm theo |
| Script | Groovy, `GroovyShell.evaluate` **mỗi request**, không `SecureASTCustomizer`; module có `DbVarModule`, `TxVarModule`, `HttpModule` | không có trong M | Đó là thực thi mã JVM tuỳ ý theo thiết kế. Nó nhất quán — tác giả API của SQLREST là người vận hành được tin — và nó không phải mô hình tin cậy của egret |
| Schema đầu ra | `OutParam` tự khai trên UI; `getOutputs()` chỉ xuất hiện ở sinh Swagger, export/import, và `checkValid` lúc lưu — **không bao giờ trong đường thực thi** | suy ra từ engine và **được cưỡng chế** bởi `shape_rows` | SQLREST không có cách nào phát hiện một endpoint âm thầm đổi cột. `shape_rows` là tính chất ta không sẵn lòng đánh đổi lấy sự linh hoạt |

Tóm tắt trung thực: SQLREST và DBAPI đứng ở đầu **động, không cưỡng chế, tin tác giả**.
egret đứng ở đầu **suy ra, cưỡng chế, chống injection bằng cấu trúc**. M dịch egret về phía
họ ở phần **vỏ** — phần thuộc về tiện dụng — và cố ý **không** dịch ở phần **hợp đồng** —
phần vốn là một cái cổng.

## 7. Hoãn lại, kèm lý do

- **Gói D — ghi và transaction. HUỶ.** §1.1 đã rút mất nền của nó: không có transaction nào
  trên bất kỳ engine nào dưới mô hình kết nối của egret, nên "nhiều câu lệnh có rollback"
  không phải lời hứa có thể đưa ra. Muốn hồi sinh thì cần trước hết một thiết kế cho hỏng
  giữa chừng, và một trait `Connector` có phiên — chứ không phải một bản cài đặt.
- **Ghim snapshot Iceberg.** Nó chạy được (§1.2) và là đường duy nhất tới các lần đọc nhất
  quán. Nó ngoài M vì ba lý do: nó **chỉ chạy trên Iceberg** (`Unsupported table type for
  temporal clauses` với mọi thứ khác), nên không hứa được cho một Trino catalog trên
  postgres hay mysql; giá trị phải tới được SQL dưới dạng **text**, nghĩa là một đường phát
  raw thứ hai trong `render.rs`, file mà toàn bộ lời khẳng định an ninh nằm ở chỗ có đúng
  **một** lỗ cố ý (`| ident`); và M vốn đã mang một breaking change về vỏ. Một người dùng
  phân trang thật, trên một bảng Iceberg thật, mới là bằng chứng quyết định lỗ thứ hai có
  đáng hay không.
- **Chế độ soạn thảo bằng script.** §5.
- **Z1 — IP allow/deny và endpoint công khai.** Vẫn hoãn; lý do là tư thế phòng thủ ở biên
  đã ghi trong §7 của spec chương trình.
- **Câu lệnh đánh theo vị trí.** §6.

## 8. Mỗi khẳng định được chứng minh thế nào

- **§1.3 được *vá*, không phải chỉ được *phủ*.** Ca hỏng phải được chạy từ đầu tới cuối
  trước khi vá và sau khi vá: publish endpoint `{% if detail %}`, gọi nó với tham số bỏ
  trống, và đòi `503` trước, `200` với `{id}` sau. Một test chỉ khẳng định "hợp đã được
  tính" sẽ vẫn xanh trong khi `shape_rows` vẫn đang từ chối.
- **Việc từ chối khi kiểu xung đột được chứng minh bằng mutation.** Cắt bỏ phép so kiểu và
  đòi test publish phải đỏ. Sáu assertion trong chương trình chart đã sống sót qua mutation;
  cái này thì không được phép.
- **Số lần describe được *đo*, không được *suy*.** Một template không điều kiện phải phát ra
  **đúng một** `describe_as_with_params` — khẳng định bằng một con số đếm, không phải "ít
  nhất một" — nếu không thì bước khử trùng lặp không làm đúng điều §3.1 tuyên bố và mọi lần
  publish âm thầm đắt hơn.
- **Cổng một-câu-lệnh được chứng minh vẫn đóng.** Toàn bộ test từ chối `;` và `#` hiện có
  chạy nguyên vẹn trên danh sách câu lệnh mới, theo từng câu. Nếu có test nào phải sửa mới
  xanh thì thay đổi đó sai.
- **Việc phá vỏ được chứng minh ở biên.** Một endpoint publish trước migration và được gọi
  sau migration phải trả `{status, message, data, meta}` — khẳng định trên route serving,
  không phải trên unit test của bộ định dạng.
- **Chạy trên kind và kiểm trên trình duyệt trước khi coi là xong.** Trên chương trình này,
  mỗi lần kiểm sống đều tìm ra một defect mà bộ test không thấy: header CSV đọc thành
  `column_1`, editor gửi `database_id: ""`, một tầng CSRF giết chết `POST` trên production
  mà không test cấp router nào nhìn thấy.

## 9. Thứ tự chương trình sau M

`M → C → Z3`, với Z1 treo và D huỷ. C — quy tắc validate tham số và biến đổi response —
không bị spec này làm thay đổi, ngoại trừ việc nó nay thừa hưởng một hợp đồng cột **theo
từng câu lệnh** thay vì một hợp đồng duy nhất. Z3 vẫn ở cuối vì định dạng export **chính
là** snapshot, và M đổi snapshot.
