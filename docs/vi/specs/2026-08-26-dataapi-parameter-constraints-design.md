# Data API — ràng buộc tham số khai báo (gói C1)

**Trạng thái:** thiết kế, chưa lên kế hoạch
**Chương trình:** DBAPI parity, `U → E → Z2 → Z1 → M → C → D → Z3`. Đây là nửa đầu của
gói **C**; nửa transform (Starlark) có spec riêng.
**Phụ thuộc:** M2 (`ebf3791c`), gói đã làm `statements` thành số nhiều và đóng băng hợp
đồng cột theo từng câu lệnh.

## 1. Thêm gì, và tại sao nó không phải một script

Tác giả endpoint hiện đã nói được `type` của tham số, nó có `required` không, `default`
của nó là gì, và — với `Enum` — danh sách `allowed` chính xác. Cái họ **không** nói được
là *"một số từ 1 đến 100"*, *"chuỗi dài tối đa 10 ký tự"*, hay *"thứ gì đó có dạng email"*.

DBAPI diễn đạt những thứ đó bằng script, và lý do nằm ngay trong tài liệu của chính nó:
**DBAPI không có mô hình tham số có kiểu**, nên script là chỗ duy nhất một ràng buộc có
thể sống. Egret thì có. Cả hai ví dụ tài liệu DBAPI đưa ra — `name.length <= 10` và một
regex email — đều là ràng buộc khai báo trên một tham số có kiểu, và đặt một runtime ngôn
ngữ vào đường request để diễn đạt chúng thì không mua được gì mà phải trả giá cả runtime.

Vậy nên C1 thêm **trường**, không thêm runtime.

### 1.1 Các trường

Trên `ParamSpec`, tất cả đều optional, tất cả đều `#[serde(default)]`:

| trường | áp dụng cho | ý nghĩa |
|---|---|---|
| `minimum` / `maximum` | `Number` | biên bao gồm hai đầu |
| `min_length` / `max_length` | `String` | biên tính theo **số ký tự Unicode**, không phải byte |
| `pattern` | `String` | regex mà **toàn bộ** giá trị phải khớp |
| `not_after` / `not_before` | `Date`, `Timestamp` | biên bao gồm hai đầu |

`Enum` không thêm gì: `allowed` đã chính là ràng buộc. `Boolean` không thêm gì: kiểu đã
chính là ràng buộc.

**Độ dài đếm theo ký tự, không theo byte.** Một `max_length: 10` mà tên tiếng Việt chết ở
chữ thứ bảy là lỗi tác giả không thể nhìn ra từ phần khai báo, và giao diện của chính
egret là tiếng Việt. Ghi ra đây vì `String::len()` trong Rust trả về **byte**, và đó sẽ là
cách cài đặt vô tình.

## 2. Chỗ duy nhất chúng được phép sống

`serving::params::coerce(spec, text) -> Result<Param, CoerceError>` **vốn đã dùng chung
giữa publish và serving**: `lifecycle.rs:1179` kiểm `default` của tác giả bằng đúng hàm mà
đường request dùng. Doc comment của nó nói rõ vì sao, và đó là tính chất mà thiết kế này
dựa vào.

**Mọi ràng buộc đi vào bên trong `coerce`.** Không đặt cạnh nó, không đặt ở một lượt quét
thứ hai trong router. M2 đã ship một lỗi Critical sinh ra từ đúng hình dạng này — publish
suy ra hình dạng tham số từ một câu lệnh trong khi serving tính trên tất cả, nên một
endpoint publish xanh rồi 500 ở mọi lần gọi. Bài học rút ra từ nó là: cách sửa không phải
thêm một kiểm tra nữa, mà là một chữ ký hàm **không thể biểu đạt** sự bất đồng đó.

Cụ thể, điều đó cho ba hệ quả, và chúng là **yêu cầu** chứ không phải tác dụng phụ:

1. **`default` của tác giả vi phạm chính ràng buộc họ đặt sẽ bị từ chối lúc publish**,
   miễn phí, vì publish vốn đã chạy `default` qua `coerce`.
2. **Giá trị của người gọi bị từ chối bằng cùng một luật**, miễn phí, cùng lý do.
3. **Không thể thêm ràng buộc vào serving mà publish không thấy**, vì chỉ có một hàm.

### 2.1 `CoerceError` cần biến thể thứ ba

Hiện tại: `Invalid` (giá trị người gọi không thoả kiểu → 400) và `Malformed` (bản thân
*spec* không thể thực hiện được, ví dụ `Enum` không có `allowed` → không phải lỗi người
gọi, nên không phải 400).

Vi phạm ràng buộc là thứ thứ ba: giá trị **đúng kiểu**, spec **lành lặn**, mà giá trị vẫn
bị từ chối. Nó phải báo được **ràng buộc nào** đã hỏng, vì chữ "invalid" trơ trọi không
nói gì cho người gọi vượt `maximum` đúng một đơn vị. Thêm một biến thể mang thông tin đó,
và đưa nó ra thân 400.

Đây là thay đổi hình dạng lỗi duy nhất mà người dùng nhìn thấy trong C1.

### 2.2 Giá trị thăm dò của publish CỐ Ý bỏ qua các ràng buộc này

Đã kiểm khi viết spec này: ngoài `coerce`, `Param` chỉ được dựng ở hai loại chỗ — giá trị
thăm dò tổng hợp của publish (`representative_value`, và cặp `true`/`false` dùng để thăm
dò boolean) và các phép chuyển đổi chỉ đọc. **Không** giá trị nào của người gọi tới được
`Param` bằng đường khác, và đó là thứ §2 dựa vào.

`representative_value` trả `Param::I64(1)`. Dưới `minimum: 100` thì giá trị đó **vi phạm
chính ràng buộc của tác giả**, và nó **phải** giữ nguyên như vậy.

Giá trị thăm dò tồn tại để quyết định *định nghĩa đi vào những nhánh nào*, và các nhánh
được đi chính là thứ M1 đóng băng thành phép hợp cột của endpoint. Làm cho phép thăm dò
biết tới ràng buộc sẽ đổi nhánh nào được thăm dò, và do đó **âm thầm đổi một hợp đồng cột
đã publish** — hậu quả tệ hơn nhiều so với một giá trị thăm dò không sống nổi qua một
request thật. Ghi ra vì "giá trị thăm dò vi phạm ràng buộc" trông y hệt một cái bug với
bất kỳ ai gặp nó sau này.

## 3. `pattern`, và câu hỏi an toàn thật sự duy nhất

`pattern` do **tác giả** viết và chạy trên input của **người gọi**. Với một engine có
backtracking thì đó là một bề mặt từ chối dịch vụ: tác giả viết `(a+)+$` mà không có ác ý,
người gọi gửi ba mươi chữ `a` rồi một chữ `b`, và đường request đốt CPU. Tác giả **không
cần** phải có ý xấu thì chuyện này mới xảy ra.

**Dùng crate `regex`** (đã có sẵn `1.13.1` trong `Cargo.lock`). Nó biên dịch sang ô-tô-mát
hữu hạn và không backtracking, nên thời gian khớp tuyến tính theo độ dài input và lớp lỗi
này **không tồn tại**. Giống hệt lập luận đã chọn Starlark cho spec transform đang tạm
dừng: **tính an toàn là thuộc tính của engine, không phải của cấu hình ta viết** — không
có danh sách cấu trúc nguy hiểm nào để mà nhớ cho đúng.

Ba thứ vẫn phải tự quyết chứ không thừa hưởng được:

- **Biên dịch lúc publish và từ chối ở đó.** Một pattern không parse được phải là 400 của
  tác giả lúc publish, không bao giờ là 500 của người gọi lúc serve.
- **Đặt `size_limit` tường minh.** Mặc định của `regex` là 10 MB chương trình đã biên
  dịch; một pattern hợp lệ mà biên dịch ra hàng megabyte không phải thứ một tham số
  endpoint cần. Đặt trần nhỏ và ghi ra.
- **Neo hai đầu là ngầm định.** Giá trị phải khớp **toàn bộ**. `is_match` là *tìm kiếm*,
  nên `pattern: "abc"` sẽ chấp nhận `xxabcxx` — không phải điều tác giả viết một luật kiểm
  tra muốn nói. Neo nó lại, và ghi rõ trong tài liệu của trường, vì tác giả tự viết `^…$`
  phải nhận được cùng một câu trả lời.

**Còn mở, chốt lúc lên kế hoạch:** có cache `Regex` đã biên dịch giữa các request hay
không. Snapshot được deserialize từ `jsonb` theo từng request (`router.rs:1289`), nên cách
cài đặt ngây thơ sẽ biên dịch pattern ở mọi lần gọi. **Hãy đo** `Regex::new` với một
pattern thực tế so với chi phí sẵn có của request trước khi thêm cache — cache theo chuỗi
pattern thì dễ, và là non nếu chi phí biên dịch chỉ là nhiễu bên cạnh một vòng gọi Trino.

## 4. Hợp đồng cũng phải học chúng

`openapi_gen::param_schema` hiện phát ra `type`, `format` và `enum`. Một hợp đồng sinh ra
mà thiếu `maximum` là đang mô tả một endpoint **dễ tính hơn thực tế**, và client sinh từ
nó sẽ vui vẻ gửi đi giá trị mà server từ chối.

Mọi trường ở §1.1 đều có đối ứng trực tiếp trong JSON Schema — `minimum`, `maximum`,
`minLength`, `maxLength`, `pattern`, và `format: date`/`date-time` với biên diễn đạt qua
`minimum`/`maximum` trên dạng chuỗi. Phải phát ra hết.

**Chỗ này kiểm được đúng theo cách giới hạn của M2 đã được kiểm**: tài liệu sinh ra được
commit và bị crate **`egret-server`** khẳng định **giống từng byte** — một crate *khác* với
crate đang sửa. Mọi thay đổi ở đây đều buộc phải regenerate.

## 5. Trình soạn thảo

Mục Parameters của editor mọc thêm các trường, và **chỉ hiện đúng cho kiểu áp dụng được** —
một ô `pattern` trên tham số `Boolean` là nhiễu, và nó dạy tác giả một mô hình sai.

Hai bài học từ chính các defect của chương trình này áp dụng thẳng vào đây:

- **Giới hạn phải nhìn thấy được ở chỗ tác giả bị chặn.** M2 chặn ở 16 câu lệnh và đặt lý
  do vào `title` của một nút bị vô hiệu hoá — rồi `disabled:pointer-events-none` khiến cái
  title đó **không bao giờ hiện ra** trên bất kỳ trình duyệt nào. Thứ giải thích một ràng
  buộc bị từ chối phải đọc được mà không cần hover một control đã chết.
- **Preview phải phán xét giá trị y như serving sẽ phán xét.** Preview đã mù hình dạng
  suốt đúng một bản phát hành và nói với tác giả "được" ở chỗ publish nói "không". Preview
  chạy qua `coerce` nên thừa hưởng ràng buộc miễn phí — nhưng đó là tính chất phải **kiểm
  bằng test**, không phải thứ để giả định, vì đây đúng là cái seam đã hỏng **hai lần**.

## 6. Tương thích

`ParamSpec` có `#[serde(deny_unknown_fields)]`, và nó cũng chính là cách đọc lại một
snapshot `jsonb` đã lưu. Vì vậy thêm trường là **tương thích tiến, không tương thích lùi**:
một binary rollback về trước C1 sẽ từ chối deserialize mọi snapshot publish sau đó, dưới
dạng 500 lúc đọc.

Đó là cái giá tiêu chuẩn của deserialization nghiêm ngặt ở đây và đã được ghi sẵn trên
type. Nêu ra vì nó làm C1 thành một **lần triển khai một chiều**, đúng nghĩa như envelope
của M2, và kế hoạch rollback là "tiến tiếp", không phải "lùi lại". **Không snapshot nào
đang có bị đổi**: mọi trường mới đều optional, và vắng mặt nghĩa là hành vi hôm nay, y
nguyên.

## 7. Rõ ràng KHÔNG nằm trong C1

- **Nửa transform Starlark của C.** Spec riêng, sau khi C1 đã chạy thật. Các quyết định
  đã chốt cho nó — chỉ transform, `script(params, rows) → JSON`, không `db`/`http`/`tx`,
  publish phải chạy script để biết cột thật, serve phải kiểm lại và fail closed — đã được
  ghi lại và không mở lại ở đây.
- **Ràng buộc liên tham số** (*"`to` phải sau `from`"*). Nhu cầu có thật và hình dạng thực
  sự khác: đó là ràng buộc trên **request**, không phải trên một tham số, và nó không có
  chỗ nào để sống trong `ParamSpec`. Hoãn có chủ ý, thay vì hàn tạm vào.
- **Thông điệp lỗi tuỳ biến cho từng ràng buộc.** Thông điệp sinh ra nêu tên ràng buộc và
  biên; cho tác giả tự viết văn là một câu hỏi riêng, về i18n và về việc một endpoint công
  khai nên tiết lộ những gì.

## 8. Điều gì sẽ khiến spec này sai

Ghi ra để kiểm được, thay vì giả định:

- ~~Nếu hoá ra `coerce` **không** phải con đường duy nhất mà giá trị người gọi đi tới một
  `Param`, thì §2 sụp.~~ **Đã kiểm khi viết spec này — nó là con đường duy nhất.** Xem
  §2.2 để biết các chỗ dựng `Param` còn lại là gì và vì sao chúng giữ nguyên.
- Nếu `Regex::new` với pattern thực tế tốn đủ để đáng kể bên cạnh một request vốn đã đi
  qua mạng tới engine, thì "đo trước khi cache" ở §3 trở thành "cache", và chỗ của nó là
  cache snapshot.
- Nếu mục Parameters của editor không thể hiện trường theo kiểu mà không tái cấu trúc, thì
  §5 là một việc lớn hơn vẻ ngoài và phải tách ra.
