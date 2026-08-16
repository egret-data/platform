# Data API P5 — giao diện quản trị

Hoàn tất `2026-08-14-data-api-design.md` bằng cách xây bề mặt mà tác giả endpoint thực sự
dùng: khu vực `Data API`, ba mục của nó, và trang chi tiết endpoint. Thay thế
`2026-08-16-data-api-p5-ui-decisions.md`, mà section 1 và 2 được gộp nguyên ý vào đây; file
đó có thể xoá khi tài liệu này được merge.

Chỗ nào tài liệu này và spec P4/P4.5 mâu thuẫn, tài liệu này có hiệu lực **chỉ với bề mặt
quản trị**. **Nó không thay đổi gì ở serving.** Quy tắc render của P4.5, hành vi cache và
rate-limit của P4, và invariant vòng đời §5.3 là đầu vào ở đây, không phải đối tượng sửa đổi.

*(Bản tiếng Anh tại `docs/en/specs/2026-08-16-data-api-p5-design.md` là bản chuẩn; bản này để
tham chiếu.)*

## 1. P5 là gì, và một thứ nó không phải

Mọi phần của Data API đã tồn tại trừ cách một con người vận hành nó. P1–P4.5 đã ship dialect
escaping, service account, định nghĩa endpoint, đường serving, và render tham số.
`frontend/src/api/generated/data-api/` đã có sẵn client có kiểu cho toàn bộ management API,
sinh từ OpenAPI mà server phát ra, và **không một component nào import nó.** Mục `Data API`
trong `Sidebar.tsx` không có `to:` và không dẫn đi đâu.

**Đối tượng: chỉ nội bộ.** Try-it và OpenAPI sinh tự động phục vụ *tác giả* endpoint.
Consumer nhận một URL và một file OpenAPI tải về — không gì khác, và không có portal. Skill
`ui-ux-pro-max` xếp developer portal vào một loại sản phẩm khác ("Trust & Authority +
Minimalism", code theme tối) so với "Data-Dense Dashboard" của egret; xây cả hai là đặt hai
ngôn ngữ thiết kế vào một sản phẩm. Đây là thứ duy nhất P5 không phải.

**Cố ý ngoài phạm vi:** endpoint không có bookmark, trash hay share. Chúng là hợp đồng vận
hành, không phải nội dung lưu trữ — các idiom Explore dành cho bộ sưu tập cá nhân không áp
dụng cho một URL mà client đã deploy đang phụ thuộc vào.

## 2. Điều hướng và cấu trúc trang

### 2.1 Vị trí

Nhóm sidebar `Data Engineering`, không phải Explore.
`frontend/src/components/layout/Sidebar.tsx` đã có sẵn placeholder trơ:

```tsx
{ heading: "Data Engineering", items: [
    { label: "Data API", icon: Plug },      // không có `to:` — placeholder trơ
    { label: "Pipelines", icon: Waypoints },
]},
```

P5 gán cho mục đó `to: "data-api"` và `feature: "data_api"` (feature key đã tồn tại tại
`crates/egret-workspace/src/features.rs:39`). Explore là nơi bạn *tiêu thụ* và phân tích;
Data API là nơi bạn *sản xuất* một interface, cạnh Pipelines.

### 2.2 Route và section nav

Route `/explore/:ws/data-api`, với **section-nav dọc bên trái** — idiom của Workspace
Settings, không phải dải tab, vì ba khu vực có bản chất khác nhau và sẽ còn thêm:

```text
Data API
  Endpoints     danh sách, scaffold Explore chuẩn
  Clients       service account + OAuth2 client
  Activity      thống kê từ dataapi.access_log
```

Giữ `Clients` *bên trong* Data API thay vì ở Workspace Settings → Connections là cố ý: bạn
ship một API từ một chỗ.

### 2.3 Danh sách Endpoints

Theo scaffold trang danh sách Explore chuẩn: `p-4`, `h1 text-lg`, nút `"+ Endpoint"`, một
`<table>` viết tay như Charts/Datasets — **không** dùng `FilterableTable`, vốn không mô hình
hoá được nhóm theo folder — `SummaryStrip` đếm số kiêm luôn filter chip, và `RowActionsMenu`
mỗi dòng.

Cột: tên · method + path · trạng thái (draft/live/disabled) · version · engine · sửa lần cuối.

### 2.4 Chi tiết endpoint

**Là route riêng** (`/data-api/:id`), không bao giờ là dialog — bài học từ Alerts #485 — với
section nav riêng:

```text
Definition · Response · Access · Try it · Versions
```

Là route thì bắt buộc phải trang bị `useUnsavedChangesWarning`. Theo phát hiện từ Alerts:
dirtiness phải là **so sánh với giá trị đã load**, không bao giờ là cờ `onChange`, nếu không
Radix hydration sẽ dựng lên một "Leave site?" ma trên trang không ai sửa.

**Không có mục `Activity` ở trang chi tiết.** Thống kê per-endpoint sống ở tab Activity cấp
workspace, lọc theo endpoint đó (§4). Một truy vấn, một chỗ.

### 2.5 Clients

Danh sách phẳng, nên `FilterableTable` + `SummaryStrip` là đúng ở đây (như `DatabasesPage`).
Hai cấp: service account → các client của nó.

**Client secret xuất hiện đúng một lần**, trong dialog sau khi tạo, có nút copy và một câu nói
rằng nó không thể hiện lại. Không bao giờ fetch lại được, không bao giờ nằm trong ô của danh
sách, và — theo quy tắc audit của P4 — không bao giờ nằm trong audit detail.

### 2.6 Không có control `location` ở đâu cả

`ParamLocation::Body` bị từ chối lúc publish và `serve_post` không có body extractor. Thay vì
offer một control rồi từ chối một giá trị của nó, **trình sửa tham số không có control
`location` nào**: mọi tham số đều là `query`. Không offer thì không có gì để chặn, và việc từ
chối không thể lệch pha với picker.

## 3. Try-it xác thực bằng proxy phía server

Đường serving chỉ nhận bearer token của service account — không nhận session cookie — nên
trình duyệt không thể gọi thẳng.

**Route mới:** `POST /api/ws/{ws}/data-api/endpoints/{id}/try`, gate
`data_api_endpoint:manage` (**không** phải `:read` — hành động này thay mặt người khác). Body:
`{ service_account_id, params }`.

### 3.1 Bắt buộc gọi router serving thật, in-process

Qua tower `Service` của nó — cùng router, cùng layer, cùng handler mà consumer thật đi qua —
sau khi mint một token ngắn hạn thật (60 giây). **Không** phải một đường "tương đương" viết
lại, và **không** phải HTTP loopback, vốn cần URL và TLS của chính server.

*Vì sao điều này không thương lượng:* P4.5 nhiều lần cho thấy đường mô phỏng vẫn xanh trong
khi đường thật hỏng. Một Try-it báo OK trong khi consumer nhận 403 còn tệ hơn không có Try-it
— nó biến công cụ duy nhất có thể phát hiện vấn đề thành lý do không ai nhìn nữa.

### 3.2 Không được vượt qua bất kỳ gate nào

Không có grant → console hiện `403 not_granted` thật, đây là lỗi đầu tiên thường gặp nhất;
màn hình đó phải nêu cách sửa và link tới mục `Access`. Rate limit, cache, Ranger và access
log đều áp dụng thật.

Ba hệ quả, ghi ra thay vì để người ta tự phát hiện:

- Một lần chạy thử **tiêu tốn ngân sách rate-limit của service account.**
- Một lần chạy thử **ghi vào `access_log`**, làm nhiễu thống kê Activity. Cột `actor_user_id`
  ở §6.1 là lời giải, và nó kiêm luôn audit trail.
- Trên endpoint `cache.scope: shared`, một lần chạy thử **có thể nạp cache mà consumer thật
  đọc.** Cùng dữ liệu nên không rò rỉ — nhưng phải nằm trong tài liệu.

### 3.3 Audit và token

Đây là hành động impersonation; ghi một dòng `audit.audit_log` (ai, endpoint, service account,
kết quả). Tiền lệ: user impersonation, #332.

**Token không bao giờ tới trình duyệt.** Console hiện request đầy đủ với
`Authorization: Bearer ••••`, kèm một khối `curl` copy được dùng placeholder `$TOKEN`.

## 4. Activity

### 4.1 Hai route, khoảng thời gian cố định

```text
GET /api/ws/{ws}/data-api/activity/summary?range=      → tiles + bảng per-endpoint
GET /api/ws/{ws}/data-api/activity/calls?…            → call list, phân trang cursor
```

`range` là một trong `1h` · `24h` · `7d` · `30d`. **Không có date picker tự do.** Các preset
chính là thứ giữ mọi truy vấn nằm trong tầm của index `access_log_ws_ts_idx` đã có
(`workspace_id, ts DESC`); một ô nhập cho phép gõ "một năm" trên bảng tăng nhanh nhất hệ thống
thì không có gì cản người gõ nó.

Gate: `data_api_endpoint:read` ở **workspace scope**, cho cả tab. Tab này cố ý là cấp
workspace và không cắt nhỏ theo endpoint; ai không được xem toàn bộ thì không vào. Nêu ở dạng
khẳng định để sau này không ai "sửa" nó thành bộ lọc theo dòng, khiến tổng không khớp với chính
các dòng của nó.

### 4.2 Tiles và bảng đến từ MỘT truy vấn

`GROUP BY GROUPING SETS ((), (endpoint_id))`. Không phải để nhanh — mà vì hai truy vấn chạy
cách nhau vài khoảnh khắc trên một bảng đang được ghi liên tục sẽ cho ra một tổng không bằng
tổng các dòng bên dưới nó, và người đọc sẽ kết luận là mình nhìn nhầm.

### 4.3 Bốn chỉ số, và ba trong số đó nói dối nếu lấy mẫu số ngây thơ

| Chỉ số | Tử số | Mẫu số |
|---|---|---|
| Calls | tất cả row | — |
| Error rate | `status_code >= 400` | tất cả row |
| p50 / p95 | `percentile_cont` trên `duration_ms` | **chỉ row 2xx** |
| Cache-hit rate | `cache_hit` | **chỉ row 2xx trên endpoint có bật cache** |

**Latency chỉ tính trên call thành công.** Một loạt 401 trả về trong 2 ms kéo p50 xuống và
làm latency trông *đẹp hơn* đúng vào lúc hệ thống đang hỏng. Nhãn phải ghi "trên các call
thành công" — một percentile không nói rõ quần thể là một con số không hành động được.

**Mẫu số của cache-hit rate loại trừ endpoint tắt cache.** Gộp chúng vào làm tỉ lệ thấp một
cách vĩnh viễn và vô nghĩa. Khi không endpoint nào trong khoảng bật cache, hiện `—`, không
hiện `0%`: không trên không thì không phải là không.

### 4.4 Tổng của bảng nhỏ hơn tile, và như vậy là đúng

`access_log` ghi một row cả khi routing chưa bao giờ resolve được endpoint — comment migration
0123 nói thẳng: "Null until routing resolves an endpoint, which is why a 404 still produces a
row." Những row đó có `endpoint_id IS NULL` và không thuộc dòng nào của bảng.

Vì vậy bảng kết thúc bằng một dòng **"Không khớp endpoint nào"** mang chúng. Quần thể đó
thường là client gọi sai URL hoặc gọi một endpoint vừa bị disable — một tín hiệu chứ không
phải rác, và đây là chỗ duy nhất trong sản phẩm nhìn thấy được nó.

**Nhưng log không nói được URL nào đã bị gọi.** `dataapi.access_log` không ghi method và không
ghi path (đã kiểm chứng với migration 0123 — hai cột đó đơn giản là không tồn tại), nên với
bảng như hiện tại, tầng này là một con số không hành động được. §6.1 thêm `request_method` và
`request_path` chính vì lý do đó. Không có chúng thì "một tín hiệu chứ không phải rác" là một
khẳng định mà schema không đỡ nổi.

### 4.5 Call thử bị loại khỏi thống kê và được hiện trong danh sách

Thống kê (tiles và bảng) lọc `actor_user_id IS NULL`.

**Call list thì hiện chúng**, gắn nhãn `Try it — <người>`, sau một toggle *"Bao gồm lần chạy
thử"* **mặc định tắt**. Chính những row này là audit trail của Try-it và là đúng thứ tác giả
cần khi debug endpoint của mình; giấu chúng ở mọi nơi là vứt đi bản ghi mà §3.3 sinh ra.

### 4.6 Call list

Cột: `ts` · endpoint · status + `error_code` · `duration_ms` · `cache_hit` · `client_id` ·
`request_id`. Bộ lọc: endpoint, status, `error_code`, và một ô tra thẳng `request_id` — đường
vào của support khi khách trích dẫn nó từ body lỗi.

Các row có `endpoint_id IS NULL` (§4.4) hiển thị `request_method` + `request_path` từ log
(§6.1) thay cho tên endpoint, đánh dấu là không khớp. Hai cột đó là toàn bộ giá trị chẩn đoán
của một row như vậy; không có chúng thì tầng này là một con số không ai hành động được.

**Truy vấn tra `request_id` vẫn bắt buộc mang `AND workspace_id = $ws`.**
`access_log_request_id_idx` cố ý toàn cục để support tra được mà không cần biết workspace; một
handler tin vào index và bỏ điều kiện sẽ cho một `request_id` đoán được đọc xuyên biên giới
tenancy. Hình dạng của index không phải là phân quyền của truy vấn.

### 4.7 Retention giới hạn các khoảng thời gian

`access_log` bị prune theo `retention_days` cấu hình được, có thể ngắn hơn 30 ngày, có thể là
`0` (không giữ gì — `AccessLogStore::prune` cố ý hiểu như vậy), và có thể không được schedule
(`scheduled_retention` trả `None`).

UI đọc giá trị đó và **vô hiệu hoá các preset dài hơn retention**, kèm một dòng "chỉ giữ N
ngày". Không có nó, người ta chọn `30d`, thấy dữ liệu thưa dần ở đoạn cũ, và kết luận traffic
đã giảm — một kết luận sai mà màn hình đang tích cực hỗ trợ.

### 4.8 Không có biểu đồ time-series

Cố ý. Ba tầng đã trả lời "có khoẻ không → endpoint nào → call nào", còn một truy vấn
`date_trunc` chia bucket trên bảng tăng nhanh nhất hệ thống đổi lấy một đường xu hướng bằng
cái giá kéo echarts vào một trang vốn không cần stack biểu đồ nào.

## 5. OpenAPI sinh tự động

### 5.1 Một route, một builder

```text
GET /api/ws/{ws}/data-api/openapi?service_account_id=<uuid>[&endpoint_id=<uuid>]
```

Gate `data_api_endpoint:read`. Per-endpoint là cùng route thêm `endpoint_id` — một builder,
không phải hai cơ chế có thể lệch nhau.

Đầu ra là **OpenAPI 3.0.3**, không phải 3.1: Postman và `openapi-generator` hỗ trợ 3.0 rộng
hơn nhiều, và mục đích duy nhất của artifact này là được tooling của người khác tiêu thụ.

### 5.2 Endpoint nào được đưa vào — phần hỏng im lặng

Hai điều kiện: `state = 'live'` (draft không có hợp đồng đã publish; endpoint disabled trả
404, nên ghi nó vào tài liệu là mô tả một điều dối), **và** service account được chọn thực sự
gọi được nó.

**Điều kiện thứ hai phải hỏi Cedar, không phải hỏi `access_grants`.** Sau migration 0124, một
membership `ws_admin` mang `data_api_endpoint:invoke`, và cascade workspace→object của Cedar
áp nó lên *mọi* endpoint trong workspace. Quét thô bảng grant sẽ bỏ sót đúng những endpoint
tới được qua đường cascade và sinh ra một tài liệu thiếu path — không lỗi, và người đọc không
có cách nào biết. Hãy hỏi authorizer đúng câu hỏi mà serving hỏi.

### 5.3 `servers[0].url` suy ra từ request

egret không có setting "base URL công khai của API". `catalog.public_url` là `Option` mặc định
`http://localhost:8080` và về ngữ nghĩa thuộc về catalog; `automation.browser_url` thuộc về
link trong thông báo. Mượn cái nào cũng là mượn nhầm knob, và cả hai đều có thể chưa set — cho
ra một file trỏ localhost trông hoàn toàn hợp lệ.

**Suy ra từ chính request tải file**: scheme + `X-Forwarded-Host` / `Host`, ghép với mount
serving `/data-api/v1` (`crates/egret-server/src/lib.rs:369`). Người đang tải file đang ngồi
trên đúng origin mà consumer sẽ dùng.

**Và hiển thị URL đó cạnh nút tải.** Một giá trị suy ra cần một chỗ để bị nhìn thấy là sai;
mắt người trên màn hình mới là phép kiểm tra, không phải một knob cấu hình không ai xem lại.

### 5.4 Nội dung

- `securitySchemes`: `oauth2` / `clientCredentials` trỏ tới token endpoint của machine-IdP —
  không phải `http bearer` trần. Việc thật sự của consumer là lấy được token, và chỉ dạng
  OAuth2 mới mô tả được điều đó.
- Tham số từ `snapshot.params`, bỏ qua `location: body` một cách phòng thủ cho các snapshot
  đóng băng trước khi điều đó bị từ chối.
- `enum` → `schema.enum` từ `allowed`.
- Response 200 dựng từ `snapshot.columns` và `snapshot.output`.
- Response lỗi cho các mã lỗi ở §7 của `2026-08-14-data-api-design.md` (không phải §7 của tài
  liệu này), dùng chung một schema `ServingErrorBody`.
- Mỗi path mang `x-egret-version: N`, để consumer biết mình đang cầm hợp đồng đóng băng nào.

### 5.5 Ba giới hạn, ghi ra thay vì để phát hiện

**`ColumnSpec.ty` là chuỗi kiểu engine thô**, và Trino với StarRocks đặt tên kiểu khác nhau.
Map thận trọng sang JSON Schema, mặc định `string`, và **luôn luôn** giữ nguyên bản gốc ở
`x-egret-engine-type`. Một ánh xạ sai đầy tự tin còn tệ hơn `string`.

**`ParamSpec` không có trường multiplicity, nhưng một tham số lặp lại là một list** (P4.5:
`?regions=EU&regions=US` → `IN (?, ?)`). Generator không thể biết tham số nào là list. Khai
báo scalar và nói rõ trong `description`; **không** phát ra `oneOf [scalar, array]`, thứ làm
hỏng phần lớn generator. Cùng `description` đó mang cái bẫy đã đo của P4.5: **`+` decode thành
dấu cách**, nên bất cứ thứ gì có `+` trong bảng chữ cái phải gửi dưới dạng `%2B` — một lỗi trả
về 200 với dữ liệu sai và không nói gì cả.

**Việc đổi tên cột phải gọi đúng hàm mà serving gọi**, không bao giờ viết lại. Và khi
`output.naming` ánh xạ hai cột vào một key, serving trả `output_naming_collision` (503) —
generator khi đó phải **từ chối sinh tài liệu và nói rõ lý do**, thay vì phát ra một schema
lặng lẽ thiếu một cột.

### 5.6 UI

Một nút trên danh sách Endpoints mở dialog chọn service account rồi tải về. Trên trang chi
tiết endpoint, mục `Response` có cùng nút đó với endpoint đã ghim sẵn.

## 6. Access, và cảnh báo §5.3

### 6.1 Migration: ba cột trên `dataapi.access_log`

```sql
ALTER TABLE dataapi.access_log ADD COLUMN actor_user_id  uuid;
ALTER TABLE dataapi.access_log ADD COLUMN request_method text;
ALTER TABLE dataapi.access_log ADD COLUMN request_path   text;
```

**`actor_user_id`** — nullable; non-null nghĩa là "chạy từ Try-it bởi người này". Không foreign
key — nhất quán với `endpoint_id` và `service_account_id` trong 0123, và cùng lý do đã nêu ở
đó: log phải sống sót qua việc xoá thứ mà nó mô tả. Nó vừa lọc các lần chạy thử ra khỏi thống
kê (§4.5) vừa là audit trail của Try-it.

**`request_method` và `request_path`** — thứ caller thực sự đã hỏi. Nullable, vì mọi row ghi
trước migration này không có câu trả lời, và bịa ra một giá trị còn tệ hơn NULL. Chúng tồn tại
vì §4.4: một row 404 mang `endpoint_id IS NULL`, và không có URL được yêu cầu thì không có gì
để nhìn — đường serving đang giữ sẵn cả hai giá trị tại thời điểm nó ghi log, nên đây là ghi
lại thứ đã có trong tay chứ không phải dựng lại.

Không thêm index trên `request_path`. Không có gì ở §4 tra row theo cột đó: call list đã thu hẹp
bằng `(workspace_id, ts)` và path được đọc ra từ chính các row truy vấn đó trả về, nên một index
ở đây chỉ làm chậm lệnh insert bận nhất hệ thống.

### 6.2 Mục `Access` là một checklist trạng thái thật

Mỗi service account một dòng, hai điều kiện.

**Điều kiện 1 — invoke, đọc trung thực qua Cedar** (không quét thô `access_grants`, theo
§5.2). Điều này buộc một phân biệt phải hiện lên UI mà một thiết kế ngây thơ sẽ gộp mất:

- *Được cấp trên endpoint này* → thu hồi được tại đây.
- *Kế thừa từ workspace* → nói thẳng rằng thu hồi ở đây không có tác dụng, và link tới
  membership, nơi thực sự sửa được.

Gộp cả hai thành một dấu ✓ cạnh nút "Thu hồi" sẽ tạo ra chuỗi sự kiện này: người dùng bấm thu
hồi, backend xoá một grant không tồn tại, dấu ✓ vẫn còn, và sản phẩm trông như hỏng trong khi
đang hành xử đúng.

**Điều kiện 2 — quyền Ranger: egret không biết, và nói đúng như vậy.** Không suy diễn từ SQL
của endpoint: trả lời đúng sẽ phải phân tích SQL để lấy các bảng và chiếu policy Ranger lên
chúng, mà một câu trả lời đúng một nửa ở đây còn tệ hơn không trả lời.

Cạnh đó là nút **"Chạy Try it"**. Đây là chỗ Try-it kiếm được chỗ đứng: nó là thứ duy nhất trả
lời được câu hỏi đó, vì nó đi qua router serving thật (§3.1), nên một kết quả xanh ở đây là
bằng chứng chứ không phải mô phỏng.

### 6.3 Cảnh báo §5.3 xuất hiện ở ba chỗ, mỗi chỗ một vai

Publish **không** cấp quyền dữ liệu: truy vấn chạy dưới danh nghĩa service account với Ranger
thực thi, nên admin còn phải cấp quyền cho service account đó trong Ranger.

1. **Dialog publish** — nói điều đó *trước* khi endpoint lên live.
2. **Mục `Access`** — trạng thái *hiện tại*, theo §6.2.
3. **Lỗi permission từ engine được map** thành thông điệp nêu đúng bước còn thiếu, thay vì rò
   lỗi engine thô. Spec §5.3 đã yêu cầu điều này và nó vẫn chưa tồn tại.

### 6.4 Cảnh báo feature-flag đặt ngay tại toggle

Tắt feature `data_api` **không dừng serving** (quyết định ở P2): endpoint đang live vẫn chạy
tiếp và chỉ bề mặt quản trị bị ẩn. Một operator với tay tới toggle đó để dừng một API sẽ tin
rằng mình đã dừng.

Chỗ duy nhất nói được điều này là **ngay tại toggle**, trong Workspace Settings → Features.
Nói nó trên trang Data API là vô ích: tắt feature thì trang đó bị ẩn, đúng lúc câu đó cần được
đọc.

## 7. Các ràng buộc kế thừa P5 phải đóng

### 7.1 Member picker phải lọc hoặc gắn nhãn `provider = 'service'`

`MemberRepo::add` không có bộ lọc `users.provider`, nên hôm nay một service account có thể
được mời làm member workspace. Nếu role là `ws_admin`, migration 0124 cho nó
`data_api_endpoint:invoke`, và cascade Cedar áp lên **mọi** endpoint trong workspace — một
grant toàn danh mục cấp qua một member picker.

`data_api_invoker` **không bao giờ** được offer làm membership role.

### 7.2 Arm `kind_perms` cho `data_api_endpoints` trong `egret-bi`

Để endpoint xuất hiện trong Access Control console như dashboard và dataset, thay vì chỉ tồn
tại trong một bảng grant chỉ tới được từ một trang.

### 7.3 Bốn route quản trị không phát audit event

`POST …/service-accounts`, `DELETE …/service-accounts/{id}`, `POST …/endpoints` và
`PATCH …/endpoints/{id}`. Spec P4 hoãn chúng tới "P5's management UI, where the surface stops
changing shape" — đây chính là thời điểm đó.

Nghiêm trọng nhất là **xoá service account**: nó hard-delete một machine principal kèm toàn bộ
`access_grants` và mọi client nó sở hữu, không để lại bản ghi nào rằng nó từng tồn tại. Nó
theo mẫu `delete endpoint` — mang các cột định danh ra từ `RETURNING` của chính lệnh `DELETE`,
vì event là bản ghi duy nhất còn sót lại.

Cả bốn đi qua `egret_audit::mutation_for`, nên workspace, principal thực hiện và impersonator
(nếu có) được đóng dấu theo cấu trúc. Không bao giờ có `client_secret` trong bất kỳ detail nào.

## 8. Kiểm thử

**Một test bị gate bởi DB mà skip thì không phải test.** Thiếu `DATABASE_URL` khiến nó báo `ok`
mà không assert gì; chỉ thời lượng phân biệt được hai kết cục. Mọi test backend dưới đây đều bị
gate bởi DB và phải chạy với biến đó được set — trỏ vào `egret_test`, không bao giờ trỏ vào DB
dev, vốn không thể vừa bị test trỏ vào vừa cho server boot được. Mục cuối cùng là test frontend
và không mang gate đó.

- **Try-it đi qua router thật.** Assert bằng cách *stub* handler: nếu thay handler serving mà
  test Try-it không vỡ thì test không hề chạm tới router, và toàn bộ đảm bảo ở §3.1 chỉ là
  trang trí.
- **Try-it không vượt gate.** Một service account không có grant phải cho `403 not_granted`
  qua `/try`, không phải thành công.
- **Thống kê loại trừ lần chạy thử**, và call list chỉ bao gồm chúng khi toggle bật.
- **Tra `request_id` bị giới hạn theo workspace:** một row ở workspace A không được trả về cho
  người đọc ở workspace B khi họ trích dẫn `request_id` của nó.
- **Mẫu số của latency và cache-hit:** một fixture gồm row 2xx và 4xx sao cho nếu tính cả 4xx
  thì p50 sẽ đổi, rồi assert giá trị báo cáo khớp với quần thể chỉ-2xx.
- **OpenAPI bao gồm endpoint tới được qua cascade:** một service account mà đường invoke duy
  nhất là membership `ws_admin` vẫn phải có những path đó trong tài liệu. Đây là lỗi ở §5.2 và
  một bản cài đặt quét bảng grant sẽ pass mọi test OpenAPI khác.
- **OpenAPI từ chối khi `output_naming_collision`** thay vì phát ra một schema thiếu cột.
- **Mục Access phân biệt được "được cấp" với "kế thừa"** trên cùng một service account.

Assertion phải được kiểm bằng mutation trước khi tin — stub phần cài đặt và xem test có vỡ
không. Sáu assertion trong chương trình chart đã sống sót qua việc phần cài đặt của chúng bị
gỡ bỏ. Và một mutation không *build* được thì không chứng minh điều gì: build trước, xác nhận
0 lỗi, rồi mới đọc kết quả.

## 9. Phân rã

Một spec, nhiều PR, theo thứ tự này — mỗi PR merge độc lập được:

1. **Route nền và migration** — ba cột ở §6.1, activity summary/calls, OpenAPI generator,
   `/try`, và bốn audit event ở §7.3. Mọi thứ ở mục 3–6 đều đọc từ đây.
2. **Arm `kind_perms`** (§7.2) và bộ lọc member-picker (§7.1) — nhỏ và độc lập với phần còn
   lại; xếp sớm vì §7.1 là một over-grant đang sống, không phải một khoảng hở hình thức.
3. **Danh sách Endpoints + trang chi tiết** (Definition · Response · Access · Versions).
4. **Mục Try it** (§3), cần 1 và 3.
5. **Mục Clients** (§2.5).
6. **Mục Activity** (§4).
7. **Cảnh báo feature-toggle** (§6.4) — một câu trong trang của người khác, tách riêng để nó
   không thất lạc bên trong một diff frontend lớn.

Đường nối giữa mục 1 và 3–6 chính là nơi lỗi tệ nhất của P4 đã sống: một cache key kiểm
`delegated` mà không kiểm `options.impersonate` sống sót qua năm vòng review vì nó nằm *giữa*
các task. Bắt buộc kiểm tra sống trên trình duyệt trước khi tuyên bố chương trình này hoàn tất
— mọi lỗi chart đáng kể đều được tìm thấy ở đó, không phải trong review và không phải bởi một
bộ test xanh.
