# Data API P6 — đóng vòng lặp soạn endpoint

Sửa đổi `2026-08-16-data-api-p5-design.md` §2.4 (các mục của trang detail) và thêm một route cạnh serving. Chỗ nào hai tài liệu mâu thuẫn, tài liệu này có hiệu lực.

*(Bản tiếng Anh tại `docs/en/specs/2026-08-17-data-api-authoring-loop-design.md` là bản chuẩn; bản này để tham chiếu.)*

## 1. Vì sao có tài liệu này

Viết một endpoint Data API hiện tốn một vòng đi qua UI cho mỗi lần thử. SQL nằm trong `Definition`, nơi cũng chứa cache, limits và rate limit — nên editor bị bóp giữa ba nhóm cấu hình. Muốn biết truy vấn trả đúng dòng chưa, tác giả phải rời sang `Try it`, đọc kết quả, rồi quay lại — và `Try it` chạy version **đã publish**, nên một sửa đổi draft còn không nhìn thấy được ở đó.

Vậy vòng lặp mà tác giả thực sự chạy — viết SQL, nhìn dòng, điều chỉnh — bị cắt ở hai chỗ cùng lúc: editor không có chỗ, và không gì trên trang cho thấy truy vấn trả về gì.

Hai tham chiếu quyết định hình dạng. **pydbapi** (`data-portal/pydbapi`, cùng sản phẩm nhưng làm độc lập) tách editor endpoint thành `API Configuration` và `API Execution Content` — truy vấn có bề mặt riêng. Và **ChartExplore** của chính egret là một studio: control một bên, preview kết quả sống bên kia, cập nhật liên tục. Mẫu thứ hai là mẫu codebase này đã chứng minh; tài liệu này áp nó vào việc soạn endpoint.

**Đây là thay đổi UX kèm một route backend.** Nó không thêm năng lực nào cho endpoint đã publish: không script engine, không multi-statement SQL, không thêm vị trí tham số. Những thứ đó là dự án riêng và cố ý ngoài phạm vi.

## 2. Route preview

```text
POST /api/ws/{workspace}/data-api/endpoints/{id}/preview
```

Gate **`data_api_endpoint:manage`** — cùng gate `validate` mang, và cùng lý do đã nêu: nó chạy một truy vấn tới engine dưới danh nghĩa caller, đó là hành động soạn thảo chứ không phải đọc metadata.

### 2.1 Nó chạy như tác giả, không như service account

`validate` đã làm đúng điều này. `validated_draft` (`crates/egret-dataapi/src/routes.rs:1096`) truyền `ctx.principal.user_id` vào `validate`, và comment ở đó ghi lý do: `describe_as` resolve dưới danh nghĩa chính caller, nên một connection delegated áp policy của **họ**, và tác giả không thể publish một hợp đồng trên bảng mà chính họ không xem được.

Preview giữ nguyên danh tính đó và chỉ đổi động từ — `execute_as_with_params` (`crates/egret-query-api/src/port.rs:196`) thay cho `describe_as`.

**Hệ quả phải tới được UI, không chỉ nằm trong tài liệu này.** Preview xanh **không** chứng minh consumer gọi được endpoint: service account là principal khác và Ranger có thể từ chối nó. Đó là câu `Try it` trả lời, và là lý do cả hai cùng tồn tại chứ không cái nào dư.

### 2.2 Body mang draft đang trên màn hình, không chỉ giá trị test

```jsonc
{
  "sql": "SELECT …",                        // đang có trong editor
  "params": [ /* ParamSpec[], như khai báo trên màn hình */ ],
  "values": { "region": ["EU"] },           // giá trị test của tác giả
  "database_id": "…"                        // giá trị hiện tại của picker
}
```

**Đọc draft đã lưu từ database sẽ phá chính tính năng này.** Tác giả sẽ phải Save trước mỗi lần preview, và vòng lặp vẫn bị cắt — bằng một cái nút thay vì bằng điều hướng. Mục đích là thấy cái đang trên màn hình.

Điều này không mở bề mặt tấn công mới. SQL Lab vốn đã chạy SQL tùy ý do tác giả cung cấp ở quyền tương đương, và payload ở đây đi qua đúng những cổng mà publish đi qua (§2.3).

### 2.3 Phải tái dùng đường render của serving, không tạo đường thứ hai

Preview render qua **`check_render_rules` và cổng tham số hành vi** — render hai lần, một lần với giá trị opaque có kiểu và một lần với cùng giá trị dưới dạng chuỗi thuần, rồi từ chối nếu lệch — sau đó bind đúng như serving bind.

Đây không phải phòng thủ hình thức. P4.5 mất năm vòng review độc lập cho một bộ quét byte không đồng ý với lexer của minijinja về cùng những byte đó, và lời giải là ngừng quét và bắt đầu **so sánh output đã render**. Một đường render thứ hai là chỗ lớp lỗi đó trở lại, nên preview không có đường riêng.

Chú ý khác biệt với `validate`, và vì sao nó quan trọng: `validate` render bằng `representative(&draft.sql, &draft.params, &[])` — giá trị thay thế tổng hợp, vì nó không có input thật. Preview có giá trị thật của tác giả và render bằng chúng. Cả hai đi qua cùng một cổng.

**Quy tắc một-statement ở đây là chuyện phân quyền, không phải chuyện nhất quán.** `data_api_endpoint:manage` KHÔNG phải `sqllab:write`. Một preview thực thi bất cứ gì body mang tới sẽ cấp cho một role chỉ có `:manage` quyền ghi tuỳ ý lên mọi engine workspace chạm được. `check_single_select` chạy đầu tiên trên đường này vì lý do đó, và nó là quy tắc duy nhất trong bộ publish **không bao giờ** được nới cho preview với lý do "preview chỉ là draft".

### 2.4 Bốn thứ preview KHÔNG làm

Preview không phải một call của consumer, và mỗi thứ dưới đây nếu có sẽ khiến nó nói sai:

- **Không ghi dòng `access_log`.** Activity đo lưu lượng thật; một lần bấm preview không phải lưu lượng. Ghi vào sẽ làm phồng số call và lệch mọi percentile trên trang đó.
- **Không tiêu rate-limit.** Một tác giả đang thử nghiệm không được phép throttle consumer đang chạy.
- **Không đọc và không ghi cache.** Đọc sẽ hiện câu trả lời cũ cho SQL vừa sửa; ghi sẽ đầu độc thứ consumer đọc bằng một kết quả sinh ra dưới principal khác.
- **`limits.max_rows` không áp dụng.** Preview cap ở **100 dòng**, cố định, không cấu hình được, vì tác giả muốn thấy hình dạng dữ liệu chứ không phải một triệu dòng trong trình duyệt. Con số nêu ở đây để không phó cho implementer, và UI phải nói khi nó đã cắt — một preview bị cap không bao giờ được đọc như một tập kết quả nhỏ.
- **`limits.timeout_seconds` cũng không áp dụng.** Ngân sách của preview là **30 giây**, cố định — thấp hơn nhiều so với trần 300 giây của serving, vì người đang chờ ngồi trước bàn phím với editor mở, không phải một batch client. Body của draft không mang `limits` nào cả, nên đây là toàn bộ ngân sách chứ không phải một cái kẹp lên input của tác giả.

### 2.4.1 Preview hiện tên cột của engine và lỗi của engine

Hai khác biệt cố ý so với thứ consumer thấy, cả hai vì preview phục vụ một người đọc khác.

**Tên cột trả về đúng như engine trả — `output.naming` KHÔNG được áp.** Tác giả đang debug SQL cần đúng những tên họ viết; `Response` mới là chỗ xem hợp đồng đã publish, gồm cả phép đổi tên. Phải nói rõ đâu là đâu trên UI, nếu không hai thứ sẽ bị lẫn.

**Lỗi engine hiện nguyên văn.** Serving cố ý map lỗi permission của engine thành `data_access_denied` và không bao giờ vọng lại text của engine, vì consumer không được biết hình dạng các bảng. Preview là trường hợp ngược lại: tác giả sở hữu SQL, đang chạy như chính mình, và không thể sửa một lỗi cú pháp từ một thông báo đã bị làm sạch. Nên preview trả về thông báo của engine y nguyên — và chính vì thế nó không bao giờ được với tới được dưới quyền `data_api_endpoint:manage`.

### 2.5 Không audit

`validate` không phát audit event, và preview theo đúng nó. Cả hai chạy một truy vấn chỉ-đọc dưới danh nghĩa chính caller — cùng thứ SQL Lab làm, và SQL Lab cũng không audit. Điều này là cố ý và ghi ở đây để sự vắng mặt của nó không bị đọc thành thiếu sót: những hành động được audit là những hành động **thay đổi thứ consumer nhận** (publish, rollback, disable, enable, delete) cộng với impersonation (`try`).

## 3. Các mục của trang detail

Sáu mục, chia theo câu hỏi mà mỗi mục trả lời:

```text
Query · Config · Response · Access · Try it · Versions
```

### 3.1 `Query` — mọi thứ quyết định truy vấn trả ra gì

Là route index. Chứa picker database, editor SQL, phần **khai báo** tham số, và preview.

**Layout dọc — editor trên, kết quả dưới — theo SQL Lab, không theo split trái/phải của ChartExplore.** ChartExplore sở hữu toàn bộ bề rộng trang; một mục bên trong trang detail thì không, và một cột kết quả bên phải sẽ bóp cả hai nửa.

**Tham số mang hai vai ở đây, và gộp chúng lại là cái bẫy.** Một tham số có *khai báo* (tên, kiểu, required, default, allowed) và, chỉ dành cho preview, một *giá trị test*. Đặt giá trị test vào cùng hàng với khai báo sẽ mời người đọc tưởng nó được lưu cùng endpoint. Nên `ParamEditor` giữ phần khai báo, và khối preview có input riêng cho từng tham số đã khai báo, đúng cách `Try it` làm. Điều đó trùng lặp với hàng input của `Try it`, và chấp nhận được: hai mục không bao giờ cùng trên màn hình, và chúng chạy dưới principal khác nhau.

### 3.2 `Config` — mọi thứ về endpoint như một đối tượng HTTP

Bốn nhóm, mỗi nhóm một heading:

1. **Endpoint** — name, description, method, path. Hợp đồng URL.
2. **Cache** · 3. **Limits** · 4. **Rate limit** — các bảng `Setting | Details | Value` từ #513, không đổi.

### 3.3 Không đổi

`Response`, `Access`, `Try it` và `Versions` giữ nguyên nội dung và hành vi.

### 3.4 Ba nút, ba quan hệ khác nhau với `dirty`

Trang detail dùng chung một form cho mọi mục, và sau thay đổi này cùng một trang mang ba hành động không đồng ý với nhau về sửa-đổi-chưa-lưu:

| Hành động | Khi có sửa đổi chưa lưu | Chạy |
|---|---|---|
| **Preview** | Bật — đó là cả mục đích | Cái đang trên màn hình |
| **Validate** | Tắt | Draft đã lưu |
| **Publish** | Tắt | Draft đã lưu |

Vì chúng khác nhau, **mỗi nút phải nói nó chạy cái nào.** "Preview" mà không có nhãn đó sẽ mời tác giả preview, thấy xanh, publish, và nhận một thứ khác — đúng thất bại mà toàn bộ tài liệu này tồn tại để ngăn.

## 4. Những gì tài liệu này KHÔNG thay đổi

Nêu ở dạng khẳng định để không bị đọc thành thiếu sót:

- Đường serving không bị chạm. Không năng lực mới nào tới được endpoint đã publish.
- **SQL một statement vẫn được thực thi.** pydbapi cho phép multi-statement; egret từ chối statement thứ hai lúc publish, và P3 từng có SEV-0 vì một dấu `;` lọt qua một bộ quét. Nới lỏng điều đó là một dự án riêng với threat model riêng.
- **Tham số vẫn chỉ ở query.** `location: body` vẫn bị từ chối và vẫn không có control `location`.
- Không có script engine.

## 5. Kiểm thử

Mọi test backend ở đây đều bị gate bởi DB: thiếu `DATABASE_URL` thì test báo `ok` mà không assert gì, nên hãy chạy với `egret_test` và đọc thời lượng.

- **Preview chạy như tác giả, không như service account.** Với một connection mà engine báo lại session user, assert các dòng trả về nêu tên *caller*. Một preview chạy như service account chính là `Try it` mang nhãn sai.
- **Preview phản ánh SQL chưa lưu.** Gửi body có `sql` khác draft đã lưu và assert kết quả khớp body. Đây là tính chất §2.2 tồn tại vì nó, và nó vỡ nếu sau này ai đó "đơn giản hoá" handler thành đọc draft.
- **Preview không ghi dòng `access_log`.** Đếm số dòng trước và sau; số phải không đổi. Rồi mutate: ghi một dòng có chủ ý và xác nhận test vỡ.
- **Preview không nạp cache.** Preview, rồi gọi endpoint như service account, và assert response serving là `cached: false`.
- **Preview từ chối những gì publish từ chối.** Một payload tham số mà cổng hành vi bác bỏ cũng phải bị bác ở đây, với cùng lỗi — chứng minh preview không có đường render riêng.
- **Preview vọng lại thông báo của engine.** Cho port trả một lỗi cú pháp và assert tác giả thấy đúng text của engine, không phải `data_access_denied`. Đây là chỗ duy nhất quy tắc làm-sạch bị cố ý đảo, nên nó cần một test nói vậy.
- **Frontend: Preview vẫn bật khi dirty, Validate và Publish thì không.** Một test, ba assertion; nó ghim §3.4 như một bộ chứ không phải từng nút một.

Assertion phải được kiểm bằng mutation trước khi tin. Và một mutation không build được thì không chứng minh gì — build trước, xác nhận 0 lỗi, rồi mới đọc kết quả.

## 6. Phân rã

1. **Route preview** — backend, với các test ở §5.
2. **Mục `Query`** — tách SQL, database và khai báo tham số khỏi `Definition`; nối khối preview.
3. **Mục `Config`** — chuyển name/description/method/path vào cạnh ba bảng cấu hình đã có; cho `Definition` nghỉ.
4. **Nhãn ba nút** (§3.4) — nhỏ, nhưng để riêng để không thất lạc trong một diff lớn.

Bước 1 ship độc lập được và tự test được. Bước 2 và 3 cùng nhau mới là cái cho `Definition` nghỉ, nên chúng nên land gần nhau để tránh một bản mà cùng một field sửa được ở hai chỗ.

## 7. Hoãn lại, kèm lý do

Những thứ này đến từ việc so sánh với pydbapi và **không** nằm trong tài liệu này:

- **Script engine Python** (sandbox RestrictedPython, các context object `db`/`http`/`cache`/`tx`) — khoảng cách năng lực lớn nhất, và là một bề mặt tấn công mới trong một sản phẩm vốn đã chạy truy vấn engine có quyền. Cần threat model riêng.
- **Multi-statement SQL** — xem §4.
- **Vị trí tham số** ngoài query — header, body, path.
- **Giới hạn đồng thời** (số request đang bay tối đa mỗi client). egret có rate limit nhưng không cap concurrency, nên một caller chậm vẫn có thể chiếm pool.
- **Result transform**, **macro definitions**, **validation tham số bằng regex/script**, và bộ filter Jinja phong phú hơn (`sql_like`, `in_list`, `compare`, `{% where %}`).
- **Endpoint public (không auth).**
