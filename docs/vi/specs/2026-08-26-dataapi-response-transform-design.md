# Data API — biến đổi kết quả bằng Starlark (gói C2)

**Trạng thái:** thiết kế, chưa lên kế hoạch
**Chương trình:** DBAPI parity, `U → E → Z2 → Z1 → M → C → D → Z3`. Đây là nửa sau của
gói **C**; C1 (ràng buộc tham số khai báo) đã merge ở `975a1632`.
**Phụ thuộc:** M2 (`ebf3791c`) cho nhiều câu lệnh mỗi endpoint, và C1 (`975a1632`) —
C2 không dùng lại gì của C1 nhưng nằm cạnh nó.

## 1. Đây là gì, và ranh giới nó không vượt qua

Tác giả endpoint định hình lại response bằng một script nhỏ:

```python
def transform(params, statements):
    return {
        "region": params["region"],
        "items": statements["rows"],
        "total": statements["total"][0]["n"],
    }
```

Chỉ có thế. **Không `db`, không `http`, không `tx`, không `cache`, không đồng hồ, không
ngẫu nhiên.** `ScriptContext` của pydbapi phơi ra tất cả những thứ đó qua ~525 dòng; đó
là một framework backend, không phải một Data API.

**Các giai đoạn đã chốt, không mở lại ở đây:** 1 transform → 2 `db.query` (chỉ SELECT,
danh tính người gọi) → 3 ghi → 4 HTTP ra ngoài. **Ranh giới nằm giữa 2 và 3.** Giai đoạn
1–2 giữ egret là **router**: mọi phép đọc vẫn tới engine **với tư cách người gọi**, nên
Ranger và Cedar vẫn phân xử, và rollback, cache, retry giữ nguyên ý nghĩa. Giai đoạn 3–4
biến egret thành **runtime** nắm giữ thẩm quyền mà engine không kiểm được. Spec này
**chỉ là giai đoạn 1**. Giai đoạn 2 đụng tới connection pooling, N+1 và trần đồng thời
còn bỏ ngỏ, nên có spec riêng.

## 2. Starlark, và vì sao lựa chọn này không phải chuyện cú pháp

`starlark-rust` (Meta/Buck2), thay vì Rhai hay rquickjs. Lập luận quyết định:

**Tính an toàn là thuộc tính của NGÔN NGỮ, không phải của cấu hình ta viết.** Starlark
không có I/O để mà gỡ. Một sandbox JS chỉ an toàn chừng nào danh sách global bạn tước đi
còn đúng — và cái danh sách đó là thứ chỉ cần sai **một lần**. Ở đây không có gì để sai.

Nó cũng **tất định** — không đồng hồ, không ngẫu nhiên — và đó chính là thứ làm preview
lặp lại được và làm một response đã cache có nghĩa đúng như nó nói.

Hai cái giá, nêu ra vì chúng có thật:

- **`starlark` là dependency MỚI.** Nó **không** có trong `Cargo.lock` hôm nay — đã
  kiểm. Đây không phải kiểu thêm `regex` miễn phí như C1, nơi crate đã được resolve sẵn
  theo đường bắc cầu. Thời gian biên dịch và bề mặt cần audit đều tăng, và kế hoạch phải
  **đo** phần tăng thời gian build chứ không nói lướt qua.
- Trình soạn thảo thì miễn phí: `lib/monaco.ts` đã đăng ký đúng `sql` và `python`, mà
  Starlark đủ gần Python để dùng lại định nghĩa đó. Rhai sẽ cần một Monarch tokenizer
  viết tay.

**Đã đo, nên không cần suy lại:** `Evaluator` có `set_max_tick_count` (một tick = một
lời gọi hoặc một vòng lặp quay lại — **đây chính là nhiên liệu**), `set_max_heap_size`,
`set_max_callstack_size`, và `set_check_cancelled` — cái móc để dựng deadline theo đồng
hồ. **Không có timeout dựng sẵn**; deadline là việc của ta. Một ghi chú cũ nói Rhai mạnh
hơn về giới hạn là **sai**.

## 3. Phần khó: hình dạng đầu ra của script KHÔNG suy ra được

Toàn bộ mô hình hợp đồng của Data API dựa trên một tính chất: **publish đóng băng cột,
serving kiểm và fail closed.** Cột đến từ `describe_as` — **engine** mô tả SQL một cách
tĩnh. Điều đó hoạt động vì hình dạng đầu ra của một câu SQL là hàm của *văn bản truy vấn*
và *nhánh mà tham số đi vào*, **không bao giờ** của dữ liệu.

Một transform phá vỡ đúng chỗ đó:

```python
if len(statements["rows"]) > 0:
    return {"ok": True, "first": statements["rows"][0]}
return []
```

Hình dạng giờ phụ thuộc **dữ liệu**, mà publish không nhìn thấy. M1 đã ghi nhận một
khoảng trống có giới hạn ở đây — `MAX_PROBES` phép gán mỗi ngữ cảnh, và các ngữ cảnh
tham số vắng mặt không bao gồm cặp — nhưng khoảng trống đó nói về nhánh của **văn bản**.
Đây là loại khác: một hình dạng biến thiên theo số đếm hàng mà không ai liệt kê được.

Nên "chạy script lúc publish rồi suy ra cột" **không dùng được**, và nên nói thẳng vì
sao: nó sẽ tạo ra một hợp đồng đúng với dữ liệu thăm dò và sai vào một ngày thứ Ba.

### 3.1 Cách giải: tác giả KHAI BÁO hình dạng, egret kiểm

`output.transform` mang theo một **schema đầu ra khai báo** bên cạnh script, đúng tinh
thần của `params`:

```json
{
  "transform": {
    "script": "def transform(params, statements): ...",
    "returns": [
      { "name": "region", "ty": "varchar", "required": true },
      { "name": "items",  "ty": "array",   "required": true },
      { "name": "total",  "ty": "bigint",  "required": false }
    ]
  }
}
```

Việc này biến một bài toán suy diễn **không giải được** thành một bài toán **kiểm được**:

- **Publish** chạy script trên chính các phép gán thăm dò mà M1 đã dựng, và từ chối nếu
  lần chạy nào không khớp `returns`. Việc đó bắt được các lỗi thông thường với giá rẻ —
  nó **không** và **không thể** chứng minh script luôn tuân thủ.
- **Serving** kiểm mọi response so với `returns` và **fail closed** khi lệch, đúng như
  `shape_rows` đang làm với cột SQL hôm nay. Đây mới là bảo đảm thật, và là cái còn đúng
  vào ngày thứ Ba.
- **Hợp đồng** sinh ra từ `returns`, nên nó mô tả điều endpoint **hứa**, chứ không phải
  điều một lần chạy thăm dò tình cờ tạo ra.

**Chế độ hỏng mới, chấp nhận có chủ ý:** một script có đầu ra biến thiên hợp lệ sẽ 503 ở
những hình dạng tác giả không khai báo. Đó đúng là đánh đổi mà `shape_rows` đã chọn và
đúng lý do nó tồn tại — một trường không khai báo mà tới được người gọi là một trường
**không ai duyệt lúc publish** đang được phục vụ.

Cờ `required` của `ColumnSpec` từ M1 áp dụng nguyên vẹn và làm đúng việc cũ: một trường
được phép vắng khi khai `required: false`, không bao giờ được phép có mặt khi không khai.

## 4. Vị trí trong đường ống

**Một transform cho mỗi ENDPOINT**, không phải cho mỗi câu lệnh. Ghép `rows` với `total`
thành một object mới là công dụng thật, và một móc theo từng câu lệnh không diễn đạt được.

```
các câu lệnh chạy (M2, theo thứ tự, không chung transaction, không chung snapshot)
        │
        ├─ meta.statements  ← KHÔNG đổi: SQL đã trả về gì, theo từng câu
        │
        └─ transform(params, statements) → data
                 │
                 └─ envelope (wrapped | bare)  ← KHÔNG đổi
```

- `statements` là một dict khoá theo tên câu lệnh, mỗi giá trị là danh sách object hàng.
  Khi N=1 nó **vẫn** khoá theo tên — `{"main": [...]}` — vì một script ngừng chạy khi tác
  giả thêm câu lệnh thứ hai là một cái bẫy.
- **`meta.statements` vẫn mô tả SQL**, không mô tả đầu ra của transform. Hai sự thật khác
  nhau, giữ tách nhau: engine trả về gì, và endpoint hứa gì.
- **`meta.row_count` là quyết định thật sự còn mở duy nhất ở đây.** Đã kiểm khi viết
  spec này: nó được tính bởi `MetaContract::total_row_count()`, cộng dồn các câu lệnh.
  Nên nó **nhất quán** với `meta.statements` — cả hai đều mô tả SQL — và nói "nó trở nên
  vô nghĩa" là quá mạnh. Vấn đề thật là người tiêu thụ sẽ đọc `row_count` như thể nó mô
  tả **`data`**, mà sau transform thì `data` có thể là một object đơn. Hai câu trả lời
  đều biện hộ được:

  - **Giữ lại**, và bắt hợp đồng sinh ra nói thẳng rằng nó đếm số hàng SQL đã tạo ra,
    không phải hình dạng mà `data` mang.
  - **Bỏ đi** với endpoint có transform, để không mời gọi cách đọc sai.

  **Đề xuất: bỏ đi.** M2 đã đổi ý nghĩa trường đó mà không đổi kiểu — đo được là một rủi
  ro với SQL vận hành và ngưỡng cảnh báo — và một trường *đúng về mặt kỹ thuật* nhưng
  **luôn bị đọc sai** chính là cùng một mối nguy lần thứ hai. Chốt việc này lúc lên kế
  hoạch, và dù chọn hướng nào cũng phải nói ra trong hợp đồng.
- **Envelope không đụng tới.** Script tạo ra *giá trị* của `data`; `wrapped` và `bare`
  hành xử y như bây giờ.

### 4.1 `shape` và `naming` bị TỪ CHỐI khi có transform

`output.shape` (`objects` | `arrays`) và `output.naming` (`as_is` | `camel` | `snake`)
tồn tại để định hình lại **hàng**. Khi script đã quyết định hình dạng thì chúng hoặc chết
hoặc nhập nhằng — `naming: camel` áp trước khi script nhìn thấy hàng, hay áp lên các khoá
mà script tự viết ra?

Publish **từ chối** tổ hợp đó thay vì tự chọn. Script nhận hàng ở một dạng chuẩn duy nhất
(object, tên gốc của engine) và sở hữu mọi thứ sau đó.

## 5. Giới hạn, và mỗi lời từ chối rơi ở đâu

Mọi giới hạn phải là **một lời từ chối có nguyên nhân được gọi tên**, ở publish hoặc ở
serve — không bao giờ là treo, không bao giờ là cắt cụt im lặng.

| Giới hạn | Cơ chế | Ở đâu |
|---|---|---|
| Ngân sách lệnh | `set_max_tick_count` | serve → 500, lỗi tác giả |
| Heap | `set_max_heap_size` | serve → 500 |
| Độ sâu gọi hàm | `set_max_callstack_size` | serve → 500 |
| Đồng hồ treo tường | `set_check_cancelled` + deadline | serve → 500 |
| Độ dài mã nguồn script | trần byte | **publish → 400** |
| Kích thước đầu ra | trần byte trên `data` đã serialize | serve → 500 |

**Vì sao script hỏng là 500 chứ không phải 400.** Người gọi gửi một request hợp lệ;
**endpoint** mới là thứ hỏng. Đổ lỗi cho người gọi vì script của tác giả chính là lỗi mà
`check_defaults` sinh ra để chặn — và nó sẽ ghi sai mã vào `access_log`.

**Còn mở, chốt lúc lên kế hoạch:** các con số. Mỗi con số phải được **đo**. Một ngân sách
tick chọn bằng trực giác thì hoặc thấp tới mức từ chối script lương thiện, hoặc cao tới
mức không còn là giới hạn. Tiền lệ từ C1: `compile_pattern` được đo ~60 µs điển hình và
~650 µs xấu nhất trong năm hình dạng ở sát trần **trước khi** câu hỏi cache được trả lời,
và chính phép đo đó mới chốt được vấn đề.

## 6. Preview, và đường nối đã hỏng HAI lần

Preview phải **chạy** transform và hiện đầu ra của nó. Bất cứ điều gì ít hơn sẽ lặp lại
defect dai dẳng nhất của chương trình: preview từng mù hình dạng suốt một bản phát hành
và nói với tác giả "được" ở chỗ publish nói "không"; còn lỗi Critical của M2 chính là
publish và serving bất đồng về hình dạng một tham số.

Luật rút ra: **preview, publish và serving phải tới cùng một evaluator qua cùng một
hàm**, đúng cách C1 đặt mọi ràng buộc vào trong `coerce` và cách M2 bắt `validate_endpoint`
**trả về** phép hợp để không nơi nào tính ra câu trả lời thứ hai. Nếu lúc lập kế hoạch
thấy mình đang viết một đường đánh giá thứ hai cho preview, thì thiết kế đã sai.

## 7. Cache và phiên bản

Cả hai đều không cần cơ chế mới, và nên ghi ra vì sao:

- Script nằm **trong snapshot**, nên đổi script là một phiên bản mới, và khoá cache vốn
  dẫn xuất từ snapshot đã tự đổi theo.
- Tính tất định của Starlark là thứ làm một response đã cache trở nên trung thực: cùng
  đầu vào thì luôn cùng đầu ra.
- Rollback giữ nguyên ý nghĩa vì script **không ghi được gì**. Đây đúng là tính chất mà
  giai đoạn 3 sẽ phá huỷ, và là lý do ranh giới nằm ở chỗ nó đang nằm.

## 8. Rõ ràng KHÔNG nằm trong C2

- **Giai đoạn 2 (`db.query`)** — spec riêng.
- **Biến đổi đường lỗi.** Một lời từ chối vẫn trả `ServingErrorBody`, không đụng tới. Một
  script viết lại được lỗi thì có thể rò tên tham số của tác giả vào một 400 — đúng thứ
  mà C1 §2.1 sinh ra để chặn.
- **Dùng chung script giữa các endpoint.** Một thư viện transform là nhu cầu có thật và là
  một tính năng khác; một script cho mỗi phiên bản endpoint giữ snapshot tự chứa.
- **Transform ở phía request.** Tham số là khai báo theo thiết kế (C1), và một script viết
  lại đầu vào sẽ nằm **phía trên** mọi phép kiểm làm publish và serving đồng ý với nhau.

## 9. Điều gì sẽ khiến spec này sai

Ghi ra để kiểm được, thay vì giả định:

- Nếu chạy script lúc publish hoá ra cần thứ mà `validated_draft` chưa có — hàng dữ liệu
  thật, một connection sống ngoài `describe_as` — thì phép kiểm phía publish ở §3.1 đắt
  hơn vẻ ngoài và phải cân nhắc lại **trước** khi lên kế hoạch, không phải trong lúc làm.
- Nếu chi phí thời gian build của `starlark-rust` đủ lớn để đáng kể so với các bản build
  vốn đã chậm của workspace này, thì đó là lý do **mở lại** lựa chọn ngôn ngữ, không phải
  một chi tiết. **Đo nó ở task đầu tiên.**
- Nếu `set_check_cancelled` không thể được thúc bởi đồng hồ mà không cần một thread cho
  mỗi lần đánh giá, thì thiết kế deadline ở §5 phải làm lại.
- Nếu hoá ra tác giả cần transform **theo từng câu lệnh** hơn là theo endpoint, thì quyết
  định một-móc ở §4 là sai và hình dạng `output.transform` phải đổi.
