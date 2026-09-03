# Workflow W2 — Biểu thức

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-02
Bản chính (tiếng Anh): `docs/en/specs/2026-09-02-workflow-w2-expressions-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§4, §9 dòng W2)
Phụ thuộc: W1 (`Property<T>`, mô hình flow, `spec::validate`)

## 1. Phạm vi

W2 cho `Property<T>` khả năng `render`. Nó port ngôn ngữ biểu thức của Kestra lên minijinja, và — phần
khiến bản port **kiểm chứng được** thay vì chỉ hy vọng — dựng một **oracle so sánh**: render cùng những
biểu thức đó qua Pebble thật trong CI rồi diff kết quả.

Nó không giao executor và không giao task handler. Sau W2, biểu thức của một flow render được trên một
ngữ cảnh; chưa gì dựng ngữ cảnh đó từ một execution đang chạy.

## 2. Bề mặt, đã đo

Đọc từ `core/src/main/java/io/kestra/core/runners/pebble/Extension.java` tại commit ghim `96893b589`:

| Loại | Số lượng |
|---|---|
| Filter | **35** |
| Function | **37** |
| Test | **1** |
| Toán tử nhị phân tuỳ biến | **7** |

Kestra không sở hữu template engine của mình: `platform/build.gradle:105` ghim
`io.pebbletemplates:pebble:4.1.2`. Thứ Kestra thêm vào là extension này — 4.652 dòng trong
`runners/pebble/{filters,functions}` cộng 1.251 dòng keo.

### 2.1 Toán tử mới là phần khó

`Extension.getBinaryOperators()` đăng ký bảy cái, và bốn trong số đó **ghi đè toán tử Pebble đã có**:

```java
"??"   NullCoalescingExpression        "isIn"  InExpression
"???"  UndefinedCoalescingExpression   ">" ">=" "<" "<="   (ghi đè)
```

minijinja không có `??`, `???`, và không có ternary `? :`. Nó **có** `~`, `is` test, inline
`a if c else b`, và — đã xác minh theo tài liệu của chính nó — **`in` / `not in`**, ứng viên để ánh xạ
`isIn`. Việc `InExpression` của Kestra và `in` của minijinja có đồng ý trên mọi cặp toán hạng hay không
là câu hỏi dành cho oracle ở §4, không phải để đọc mà kết luận: `isIn` được đăng ký như một **toán tử
nhị phân** chứ không phải một test, và toán tử chính là nơi các sai lệch của bản port này trú ngụ.

**`??` và `???` không phải cùng một toán tử**, và một bản port gộp chúng lại sẽ sai theo cách không lần
đọc lướt nào bắt được:

- `??` — tính vế trái; **non-null** → trái; **null** → phải; **thiếu thuộc tính** → phải.
- `???` — tính vế trái và trả về **kể cả khi null**; chỉ **thiếu thuộc tính** mới rơi sang phải.

**Các toán tử so sánh bị ghi đè là một sai lệch sống.** `GreaterThanExpression` thêm so sánh chuỗi lên
trên bản mặc định chỉ-số của Pebble:

```java
if (left instanceof String sLeft && right instanceof String sRight) {
    return sLeft.compareTo(sRight) > 0;
}
```

`>` của minijinja có thể đồng ý hoặc không. Đó không phải câu hỏi giải quyết bằng cách đọc — nó chính
xác là thứ oracle ở §4 sinh ra để trả lời, cho cả bốn toán tử và cho các cặp toán hạng khác kiểu.

## 3. Chiến lược

**minijinja, cộng một bộ viết lại biểu thức dựa trên tokenize, được chứng minh bằng oracle so sánh.**

Đã loại, kèm lý do:

- **Tự viết engine tương thích Pebble bằng Rust.** Trung thành theo cấu trúc, nhưng đặt một template
  engine viết từ đầu lên đúng bề mặt SQL injection của egret. Lập luận an toàn của
  `egret-dataapi/src/render.rs` dựa vào nội tại minijinja — `set_formatter` cộng cái bẫy
  `is_default_formatter()` mà file đó viết hẳn một đoạn để cảnh báo. Dựng lại từ số không để đổi lấy độ
  trung thành biểu thức là đánh đổi sai.
- **Gọi Pebble thật lúc chạy qua plugin host của W5.** Trung thành tuyệt đối, nhưng biểu thức được
  render ở mọi thuộc tính task và mọi vòng lặp. Một RPC cho mỗi lần render là sai hình dạng.
- **Vá parser của minijinja để thêm toán tử.** Vendor một fork của chính crate mà an toàn SQL của egret
  dựa vào là một gánh nặng bảo trì nhắm sai rủi ro.

### 3.1 Bộ viết lại phải tokenize, không được regex

`a ?? b` → `kestra_null_coalesce(a, b)` và ternary → `a if c else b` là **phép biến đổi** đơn giản
nhưng là **phép thay chuỗi** nguy hiểm: `??` bên trong chuỗi ký tự, ternary lồng nhau, và `???` chồng
lên `??` đều phá vỡ một regex. Bộ viết lại đọc biểu thức theo token — chuỗi ký tự, ngoặc, toán tử — và
viết lại ở mức gần-AST.

Tính đúng đắn của nó không được khẳng định mà được **đo** bởi §4, gồm cả mọi biểu thức trong corpus có
toán tử nằm bên trong một chuỗi ký tự.

`kestra_null_coalesce`/`kestra_undefined_coalesce` là lời gọi hàm thuần, không phải filter (`a |
kestra_null_coalesce(b)`), vì Kestra đăng ký `??`/`???` như *toán tử* với vế phải được đánh giá lười,
mà cả filter lẫn function của minijinja đều không nhận tham số chưa được đánh giá — không có móc "chỉ
đánh giá vế phải khi cần" nào mà một bộ viết lại chỉ-dựa-trên-văn-bản chạm tới được, dù chọn hình thức
nào. Dạng lời gọi được chọn cho Task 2 vì brief định hình nó mô tả đích đến là "lời gọi tới hai hàm tuỳ
biến"; việc trao cho chúng ngữ nghĩa short-circuit/thuộc-tính-thiếu thật sự thuộc về task sau nào đăng
ký chúng, không phải bộ viết lại này.

### 3.2 Phụ thuộc

| Cần | Crate | Ghi chú |
|---|---|---|
| filter `jq` | `jaq` 3.1 | thuần Rust, còn bảo trì (cập nhật 2026-08) |
| `toIon` / `fromIon` | `ion-rs` 1.0 | bản Rust chính thức của Amazon |
| **Mẫu ngày của Java** | **không có crate nào** | phải tự viết |

Cái thứ ba là khoản đắt. `{{ d | date("yyyy-MM-dd'T'HH:mm:ss") }}` là mẫu `DateTimeFormatter` của Java
— không phải strftime, không phải cú pháp chrono, với quy tắc trích dẫn riêng cho văn bản nguyên văn.
Không crate nào cung cấp. Một bộ dịch từ mẫu Java sang chrono là mảnh lớn nhất của W2 và là nguồn sai
lệch âm thầm khả dĩ nhất — đó chính là lập luận cho việc dựng §4 **trước**.

## 4. Oracle, và vì sao nó được dựng đầu tiên

Spike W5 đã chứng minh dựng được engine Pebble độc lập **không cần Micronaut**:
`io.kestra.core.runners.pebble.Extension` có constructor no-arg public, nên một `PebbleEngine` thô kèm
extension của Kestra chỉ là vài dòng Java. Đó là toàn bộ oracle.

**Hình dạng.** Một corpus gồm biểu thức và ngữ cảnh, lưu dạng dữ liệu trong repo. Một harness Java nhỏ
render từng mục qua Pebble thật rồi ghi ra golden file. Một test Rust render cùng corpus và diff.
**Corpus là artifact**; harness chỉ tái sinh nó.

**Thứ tự.** Oracle nằm ở task **đầu tiên** của gói, trước khi port bất kỳ filter nào. Một filter viết
theo phỏng đoán rồi mới đem kiểm là filter mà tác giả **đã quyết định trước câu trả lời**; một filter
viết đối chiếu golden value thì không.

**Nó đã bắt được gì, trước cả khi được viết.** Trong spike W5, engine độc lập render:

```
{{ json | jq('.a.b') }}   =>  [42]
```

Một **mảng**, không phải `42`. Một filter `jq` viết tay bằng Rust rất có thể trả về số vô hướng, render
ra thứ trông đúng, và không bao giờ làm đỏ một test viết từ cùng giả định đó.

**CI.** Harness Java cần JDK, thứ CI có thể không có. Nên golden file được **commit**, và test Rust chạy
đối chiếu với nó mà không cần JVM. Một job riêng, opt-in, tái sinh và diff golden file để bắt trôi dạt
khi phiên bản Kestra ghim thay đổi — cùng hình dạng với `committed_openapi_json_is_current`, và cùng lý
do.

## 5. `Property<T>::render`

W1 định nghĩa `Property<T>` là nguồn thô, không render. W2 thêm:

```rust
impl<T: DeserializeOwned> Property<T> {
    pub fn render(&self, cx: &RenderContext) -> Result<T, RenderError>;
}
```

Render và ép kiểu là một bước, như trong Kestra: `runContext.render(prop).as(Integer.class)`. Một
`Property<T>` mà nguồn không chứa `{{ }}` render ra chính nó — đó là thứ khiến identity-render của
plugin host W5 mạch lạc.

**Mối nối SQL không thuộc W2.** `egret.sql` render qua `bind_formatter` của `egret-dataapi`, nơi
`{{ }}` không có đường nào chạm tới SQL thô theo cấu trúc. W2 không được tạo ra đường thứ hai render
SQL, và `spec/` với `models/` giữ nguyên ranh giới không-I/O.

## 6. Ngoài phạm vi

Không executor, không task handler, không dựng ngữ cảnh execution. `{{ outputs.x.y }}` parse và render
được trên một ngữ cảnh mà W2 chưa biết cách dựng — W3 dựng nó. Các function `secret()`, `kv()`,
`namespaceFile()`, `render()` của Kestra chạm tới dịch vụ chưa tồn tại ở đây; chúng được port thành
**những lời từ chối tường minh** có nêu tên gói sẽ hiện thực, không bao giờ thành giá trị rỗng im lặng.

## 7. Câu hỏi mở

1. **Bốn toán tử so sánh bị ghi đè cần trung thành tới mức nào?** Oracle sẽ nói minijinja có sẵn đồng ý
   hay không. Chỗ nào không, bản sửa là một toán tử tuỳ biến qua bộ viết lại — đắt hơn một filter. Nên
   quyết sau khi có số đo, không phải trước.
2. **`fuel`** đã được bật sẵn trên minijinja của egret. Đánh giá biểu thức trên template do tác giả cung
   cấp phải mang một giới hạn fuel, và W2 là nơi giới hạn đó có một con số. Guard tương ứng của Kestra
   là `MAX_RENDERING_AMOUNT` trong `VariableRenderer`; hai cơ chế không giống nhau và phép ánh xạ cần
   một quyết định.
3. **Ai sở hữu corpus.** Golden file là hợp đồng. Ai thêm một filter thì thêm mục corpus trong cùng thay
   đổi đó, nếu không filter ấy chưa được chứng minh — đúng luật W1 rút ra sau khi `Quota`/`Check` rơi ra
   ngoài một cuộc quét theo nhãn.
