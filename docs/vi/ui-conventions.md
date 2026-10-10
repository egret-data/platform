# Quy ước UI của egret

> **Các rule nay nằm ở [`frontend-rules.md`](frontend-rules.md), và đó là bản chuẩn.** Tài liệu này là phần giải thích lý do và ghi lại các quyết định đã đảo mà bộ rule trỏ tới.

Cách frontend egret dựng trang danh sách, luồng tạo mới, bảng cấu hình, và những bài test
bảo vệ chúng.

Mỗi quy tắc dưới đây đều dẫn ra đoạn code hiện thực nó và, nếu có, cả lỗi đã thúc đẩy nó ra
đời. **Một quy tắc không có cả hai chỉ là ý kiến cá nhân, và ý kiến cá nhân thuộc về comment
review chứ không thuộc về một chuẩn.** Những điều không tìm được bằng chứng đã bị cắt khỏi
tài liệu này thay vì viết ra.

Tài liệu được viết *từ* những gì đã tồn tại. Chỗ nào một quy ước mới chỉ áp dụng một phần thì
nói thẳng ra chứ không giấu — xem [Tình trạng áp dụng](#tình-trạng-áp-dụng).

*(Bản tiếng Anh tại `docs/en/ui-conventions.md` là bản chuẩn; bản này để tham chiếu.)*

Đường dẫn tính từ `frontend/` trừ khi ghi rõ khác. Số dòng sẽ trôi; tên symbol mới là mốc ổn
định.

---

## 1. Trang danh sách

### 1.1 `ListPageShell` sở hữu `h1` duy nhất

`src/components/explore/ListPageShell.tsx`

Mọi trang danh sách Explore đều bọc trong `ListPageShell`. Nó cung cấp `p-4`, `h1` duy nhất
của trang, một slot `toolbar` và một slot `action`. Trang **không** tự viết header của mình.

```tsx
<ListPageShell title="Endpoints" toolbar={…} action={…}>
  …
</ListPageShell>
```

**Vì sao là một component chứ không phải một comment.** Header `p-4` + `flex items-center
justify-between` từng được viết tay ở cả sáu trang danh sách Explore, và ba trong số đó mang
comment gọi nó là "the canonical Explore list-page scaffold" — một quy ước được ghi lại sáu lần
mà không nơi nào bắt buộc. Điều này nằm trong doc comment của chính component.

**Lỗi đã xảy ra.** Ở phase trước khi có shell, việc gỡ một wrapper layout đã lặng lẽ mang theo
`h1` duy nhất của một route, và không test nào phát hiện — trình duyệt mới phát hiện. Chốt chặn
bây giờ là test đầu tiên trong `src/components/explore/__tests__/ListPageShell.test.tsx`
(`"renders exactly one h1, and it is the title"`), assert *số lượng* đúng bằng một, chứ không
chỉ là có tồn tại một cái. `EndpointDetail.tsx` — một route chi tiết, không phải danh sách —
chịu cùng nghĩa vụ và thoả mãn nó bằng `<h1 className="sr-only">{headingName}</h1>` đặt dưới
một `h2` hiển thị, có assertion đếm riêng.

**Shell nhận node, không nhận capability.** `toolbar` và `action` là `ReactNode`. Một shell
biết về bookmark, bulk mode và trash sẽ phải mọc thêm một prop cho mỗi capability, mà các trang
lại khác nhau ở chỗ có capability nào.

**Ba hành vi của shell là chịu lực và đã có test, đừng "đơn giản hoá" chúng đi:**

- `flex-wrap` đặt trên chính nhóm action, không chỉ trên hàng header. `Endpoints.tsx` nhét ba
  control vào slot đó (bulk toggle, "Download OpenAPI", "+ Endpoint"), tự nó đã tràn ở viewport
  hẹp. Được chốt bởi `"wraps the action group, not only the header row"`.
- Thẻ `<p>` description bị **bỏ hẳn** khi không có description, chứ không render rỗng. Một
  đoạn văn rỗng vẫn chiếm chiều cao và đẩy bảng xuống ở mọi trang không có description.
  Được chốt bởi `"omits the description element entirely when there is none"`.
- Canh dọc là có điều kiện: `items-center` khi không có description, `items-start` khi có. Có
  test, và test cố ý chỉ assert class canh lề chứ không assert cả chuỗi `className`, để nó
  không vỡ vì một thay đổi class không liên quan.

### 1.2 Thứ tự control trong header

`ListPageShell` render `{toolbar}` rồi `{action}` trong cùng một nhóm
`flex flex-wrap items-center gap-2` bên phải header. Nghĩa là: **control xem trước, action
chính cuối cùng.**

`src/routes/explore/dataapi/Endpoints.tsx` là bản tham chiếu:

- `toolbar`: `BookmarkToggle`, `BulkToggle`, `FolderToolbar` — đúng thứ tự đó.
- `action`: `Download OpenAPI` (phụ, `variant="outline"`), rồi `+ Endpoint` (chính).

Toolbar **chỉ render khi `hasData`**: bộ lọc và bulk mode trên không dòng nào là những control
không làm được gì.

### 1.3 Nút icon trên toolbar lấy từ `ListToolbarButton`

`src/components/explore/ListToolbarButton.tsx`

Cụm `Tooltip` + `TooltipTrigger asChild` + `Button` icon + `TooltipContent side="bottom"` từng
được viết tay bốn lần mỗi trang, ở `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` và
`Alerts.tsx`, và các bản sao đã trôi khỏi nhau: có bản truyền
`variant={active ? "secondary" : "outline"}`, có bản không, và **không bản nào đặt
`aria-pressed`**, nên screen reader không phân biệt được bộ lọc đang bật với bộ lọc đang tắt.

Hai hệ quả được mã hoá vào chữ ký hàm:

- `label` là **bắt buộc**, không phải tuỳ chọn. Nội dung hiển thị duy nhất của nút là một icon,
  nên thiếu nó thì control hoàn toàn không có tên gọi truy cập được.
- `filled` tách khỏi `active` có chủ đích. Bookmark và trash tô đặc glyph khi đang bật; bulk và
  folder thì không. Đó là quyết định theo từng icon, không phải hệ quả của việc đang bật.

### 1.4 Ô tìm kiếm nằm ở toolbar phía trên bảng

`src/routes/explore/dataapi/Clients.tsx` (`data-testid="page-toolbar"`)

Tìm kiếm đặt ở một hàng toolbar phía trên summary strip, đúng chỗ năm trang Explore anh em đặt.
Hộp tìm kiếm riêng của `FilterableTable` render *bên dưới* strip, bên trong khối bảng — nơi duy
nhất không trang danh sách nào khác đặt — nên các trang truyền `hideSearch` và tự lọc `rows`.
Hai ô tìm kiếm cách nhau một hàng còn tệ hơn cả hai phương án.

Hệ quả kéo theo dễ làm sai và đã được comment tại chỗ: với `hideSearch`, biến `q` nội bộ của
`FilterableTable` không bao giờ rời `""`, nên nó không còn phân biệt được "lần đầu chưa có gì"
với "tìm không ra" và luôn render `emptyText`. Điều đó **chỉ đúng vì** trường hợp lần-đầu thật
sự là `empty` của `ListStates` và không bao giờ chạm tới bảng. Xem §2.

---

## 2. Loading, empty, error — ba trạng thái, không bao giờ hai

`src/components/explore/ListStates.tsx`

Mọi danh sách mở đầu bằng một nhánh bốn chiều, theo đúng thứ tự này:

```text
isLoading  →  skeleton
isError    →  Alert destructive
!hasData   →  trạng thái rỗng
còn lại    →  các dòng
```

### 2.1 `isError` được kiểm tra TRƯỚC `hasData`

Được viết tay thành ternary lồng nhau ở cả sáu trang, và chúng đã trôi khỏi nhau:
`Dashboards.tsx` kiểm `hasData` trước `isError`, nên một refetch thất bại mà vẫn còn dòng
trong cache sẽ render bảng và nuốt mất lỗi, trong khi năm trang kia render alert.

### 2.2 Lỗi không bao giờ được gộp vào rỗng

Đây là quy tắc mà component này sinh ra để giữ.

**Lỗi đã xảy ra.** Hai picker service account của Data API gộp hai nhánh làm một. Một lần fetch
thất bại render đúng dòng chữ "workspace này chưa có service account nào" trong khi nút Run bị
disable — một ngõ cụt *kèm một lời giải thích sai*, và người vận hành đi tạo một service
account vốn đã tồn tại.

Cả hai nay đều tường minh và có comment tại chỗ:

- `src/routes/explore/dataapi/detail/TryIt.tsx` — `accountsQuery.isError` render Alert
  destructive; chỉ nhánh không-lỗi mới rơi xuống gợi ý "chưa có service account nào".
- `src/routes/explore/dataapi/detail/AccessSection.tsx` — `accountsQuery.isError ||
  grantsQuery.isError` đứng trước nhánh `accounts.length === 0 && orphanGrants.length === 0`.

Cùng một hình dạng lặp lại ở bất cứ đâu kết quả của một query được dùng để mô tả thế giới.
`src/routes/explore/dataapi/detail/ResponseSection.tsx` phòng nó hai lớp: react-query xoá
`isLoading` khi **bất kỳ** lần settle nào xảy ra, kể cả lỗi — nên một lần fetch version thất bại
trên endpoint đang live sẽ rơi thẳng vào nhánh `!liveVersion` và đọc thành "chưa từng publish"
với một tác giả đã publish rồi. Vì thế nó tính `columnsErrored` tách khỏi `columnsLoading`, và
cả hai đều gắn với `endpoint.live_version_id` để endpoint thật sự chưa publish cũng không bị
mô tả sai.

### 2.3 "Rỗng" nghĩa là danh sách CHƯA LỌC rỗng

`Clients.tsx` tính `hasData` từ số lượng **chưa lọc**, kèm lý do trong code: call-to-action
lần-đầu không được xuất hiện chỉ vì một lần tìm kiếm trượt. Tìm kiếm thu về không kết quả là
phần thân rỗng của chính bảng, một câu khác cho một tình huống khác.

### 2.4 Bản thân trạng thái rỗng

`src/components/explore/EmptyState.tsx`. Dùng `variant="dashed"` cho khối lần-đầu của trang và
`plain` (mặc định) bên trong slot `emptyText` của `FilterableTable`. Nó là một component có
variant chứ không phải hai component, vì nội dung y hệt nhau và chỉ khung là khác.

---

## 3. Luồng tạo mới: khi nào là trang, khi nào là dialog

**Một luồng tạo mới trở thành trang khi thứ nó tạo ra có một URL đáng để liên kết tới, hoặc khi
nó sinh ra thứ người dùng không được phép mất vì một cú click lạc.** Ngoài ra, dialog vẫn
đúng.

Cả ba luồng tạo của Data API đều là route, khai báo trong `src/App.tsx` kèm lý do viết ngay
phía trên:

| Route | Component | Vì sao là trang |
|---|---|---|
| `data-api/endpoints/new` | `EndpointCreate` | Endpoint tạo ra có URL chi tiết; form có thể liên kết, bookmark và quay lại bằng Back. Điều hướng **vào** endpoint vừa tạo. |
| `data-api/clients/new` | `ServiceAccountCreate` | Tương tự, chỉ khác là service account không có route chi tiết — nó là một dòng trong bảng Clients — nên trang này điều hướng **về danh sách**. |
| `data-api/clients/:accountId/new-client` | `ClientCreate` | Response chứa một secret chỉ hiện **một lần**. |

### 3.1 Secret một lần là lý do mạnh nhất

`src/routes/explore/dataapi/ClientCreate.tsx`

Khi còn là dialog, đây là một modal mở đè lên một modal khác: dialog issue bị unmount trong lúc
vẫn đang mở, đúng lúc dialog secret mount — đúng hình dạng kinh điển của `pointer-events` kẹt.
Secret nay rơi vào cùng route đó ở trạng thái **created**, thứ không thể bị đóng bằng click ra
ngoài hay `Escape` lỡ tay.

Trang có ba trạng thái, và trạng thái thứ ba mới là điểm mấu chốt: **form**, **created**
(secret, tại chỗ, với đúng một nút copy, một link "Done" và cố ý không có điều hướng nào bên
cạnh), và **spent** — tab này đã issue một client nhưng không còn giữ secret nữa, ví dụ sau khi
reload. Trạng thái spent nói thẳng ra điều đó và chỉ lối đi tiếp (revoke rồi issue lại) thay vì
render trắng. Commit message ghi rõ: đừng "cải tiến" bằng cách cache secret cho sống qua
refresh.

### 3.2 Component riêng, hay dùng lại component chi tiết không có id?

Tiền lệ đã đo, cả hai đều có thật:

- `control-plane/engines/new` → `EngineCreate` — một trang tạo riêng.
- `charts/new` → `ChartExplore` và `alerts/new` → `ScheduleDetail` — dùng lại component chi
  tiết, không id.

Cách dùng lại chạy được với chart và alert vì các đối tượng đó không có sub-resource phụ thuộc
id. Nó sẽ không chạy được với endpoint: `App.tsx` đăng ký **sáu** section và cả sáu đều đọc id
(Preview của `Query` `POST` tới `…/endpoints/{id}/preview`; Response, Access, Try it, Versions và
Config đều nạp bản ghi đã lưu — Preview là một phần của `Query`, không phải section thứ bảy), nên
dùng lại shell đó sẽ cho ra một trang mà Preview không chạy được
và các tab bên phải vô nghĩa cho tới lần save đầu tiên. Vì vậy `EndpointCreate` là component
riêng.

### 3.3 Điểm vào là link, không phải push mệnh lệnh

`Clients.tsx` và `Endpoints.tsx` đều dùng `<Button asChild><Link …>`. Một nút tự push route
trông y hệt trên màn hình nhưng không thể click chuột giữa, mở tab mới, hay copy.

### 3.4 Viết `new` trước `:id` — như một QUY ƯỚC, không phải một bản vá

Làm vậy cho người đọc. Nó **không** chịu lực, và các comment trong `App.tsx` nói rõ như vậy, vì
trước đây chúng khẳng định điều ngược lại. Đã đo trên react-router-dom bản đang cài (7.18.2):
với `:id` đăng ký trước, `/endpoints/new` vẫn khớp route tĩnh, vì v7 xếp hạng theo độ cụ thể của
segment chứ không theo thứ tự khai báo. Khẳng định sai đó nằm ở hai chỗ — comment mới của Data
API và comment `alerts/new` có sẵn — và cả hai đã được sửa.

### 3.5 Trang tạo mới cũng dùng `ListPageShell`

`EndpointCreate`, `ServiceAccountCreate` và `ClientCreate` đều render bên trong `ListPageShell`
với `title` và `description` — nên trang tạo có cùng header một-`h1` và cùng padding như danh
sách nó đi ra từ đó, và §1.1 áp dụng cho nó luôn. Slot `description` của shell tồn tại đúng cho
việc này.

### 3.6 Khi thành công: invalidate, điều hướng, không toast

`EndpointCreate` và `ServiceAccountCreate` làm cả ba, và `EngineCreate` của control-plane là
tiền lệ chúng dẫn ra. Việc bàn giao **chính là** lời xác nhận — tác giả đáp xuống hoặc trình
soạn thảo của đối tượng mới với cái tên họ vừa gõ ở header, hoặc danh sách đang chứa dòng họ vừa
tạo — và một toast đè lên đó chỉ báo lại thứ đã hiện trên màn hình.

**`ClientCreate` invalidate và cố tình KHÔNG điều hướng**, điều mà bản trước của mục này viết
sai khi nói "cả ba luồng tạo đều làm vậy". Nó render trạng thái đã tạo ngay tại chỗ, vì response
mang client secret và secret đó chỉ hiện đúng một lần (§3.1) — một developer làm theo câu cũ ở
đây sẽ điều hướng đi và phá mất secret. Vậy nửa "invalidate, không toast" là phổ quát; nửa "điều
hướng" chỉ áp dụng khi response không giữ thứ gì tác giả còn cần đọc.

Dialog cần toast vì nó đóng lại trên một danh sách trông như không đổi. Trang thì không. (Việc
invalidate chạy trước điều hướng, nên dòng mới đã có sẵn khi danh sách render.)

### 3.7 Khi thất bại: không xoá gì cả

Cũng ở cả ba: thông báo lỗi của chính server hiện inline phía trên form, và **mọi trường giữ
nguyên giá trị**. Dialog có thể bị đóng rồi mở lại; một trang tự reset khi thất bại làm mất
công sức mà tác giả không lấy lại được.

### 3.8 Các form này không lưu bản nháp

Được ghi trong comment của route để không ai báo đây là bug: form là state component thuần và
reload sẽ xoá sạch. Với `ClientCreate` đây là chủ đích — hậu quả xấu nhất của việc không nhớ là
một form lần-đầu tệ hơn; hậu quả xấu nhất của việc nhớ là một secret bị rò.

---

## 4. Bảng `Setting | Details | Value`, và khi nào card vẫn đúng

`src/components/config/ConfigTable.tsx` — `ConfigTable`, `ConfigTableGroup`, `ConfigTableRow`.

Một nhóm setting là một **bảng**, không phải một chồng card. Ba cột: nhãn của setting, chữ trợ
giúp mờ, và control chỉnh sửa trực tiếp. Không có toggle xem/sửa — bề mặt này là một bề mặt
chỉnh sửa duy nhất với một nút Save.

### 4.1 Nhóm là DÒNG, không phải card

`ConfigTableGroup({ label })` render một dòng tiêu đề chiếm trọn chiều ngang bên trong bảng.

**Lỗi đã xảy ra.** Các section chi tiết của Data API từng xếp mỗi nhóm một card `ConfigSection`
— `Endpoint` đóng khung một form bốn field viết tay, `Cache` ba dòng bảng, `Limits` hai,
`Rate limit` ba. Đọc dọc trang, đó là một cột toàn hộp gần rỗng.
`src/routes/explore/dataapi/detail/Config.tsx` nay là **một** bảng với bốn dòng
`ConfigTableGroup` (`Endpoint`, `Cache`, `Limits`, `Rate limit`), và các trường định danh
(name, description, method, path) nay là những dòng bình thường, nên mọi setting trên tab đọc
giống nhau và cột Details có thể giải thích chúng. Các trường đó nằm *bên trong* card đầu tiên,
không phải một form riêng phía trên các card như bản trước của mục này viết.

Chi tiết chịu lực của `ConfigTableGroup`:

- Nó `colSpan={3}`. Một dòng nhóm chỉ chiếm cột Setting sẽ đọc thành một setting không có giá
  trị — đúng sự nhầm lẫn mà việc bỏ card sinh ra để loại trừ.
- `role="rowheader"`, và chỉ nó. Ô này từng mang thêm `scope="colgroup"`, và đợt sửa cuối đã bỏ:
  `scope` là thuộc tính chỉ dành cho `<th>`, trên một `<td>` nó vô tác dụng. Chính mutation của
  branch này đã nói vậy mà bị đọc thành bằng chứng cho cả cặp — bỏ `role` mà **giữ** `scope` làm
  chết bốn test, nên `scope` không gánh phần ý nghĩa nào. Nó vẫn là `<td role="rowheader">` chứ
  không thành `<th scope="colgroup">`, vì `<th>` mang theo implicit role của riêng nó để tranh
  chấp.

Một cột, có chủ đích. Lưới hai cột kéo các nhóm ngắn cao bằng nhóm dài bên cạnh, còn CSS
`columns` xung đột với flex container mà trang này nằm trong và cắt cụt form ở nếp gấp. Cả hai
đều đã thử trên chính editor này và cả hai đều sai.

### 4.2 Khi nào card vẫn đúng

`src/routes/explore/dataapi/detail/ResponseSection.tsx` giữ lại hai card có chủ đích, và doc
comment của nó ghi lý do:

- **Frozen columns** — một bảng `Name | Type` chỉ-đọc của hợp đồng *đã publish*. Không có Value
  để sửa. Ép nó vào bảng chung thì hoặc phải bịa ra cột Value, hoặc phải lồng một bảng vào
  trong một ô, và sẽ làm dữ liệu chỉ-đọc trông như sửa được.
- **OpenAPI document** — một đoạn văn, một nút tải, và một base URL suy ra. Đó là hành động,
  không phải setting.

Chỉ `Output shape` mới có hình dạng cấu hình, và chỉ nó trở thành `ConfigTableGroup` + các dòng.

**Quy tắc:** lời phàn nàn đã xoá các `ConfigSection` là về việc card đóng khung những bảng gần
rỗng, không phải về card. Một bảng liệt kê chỉ-đọc không phải setting, và một hành động không
phải setting.

---

## 5. Chiều rộng

### 5.1 Không đặt cap cấp trang trên route chứa bảng cấu hình

`src/routes/explore/dataapi/EndpointDetail.tsx` — wrapper của `<Outlet/>` không mang `max-w-*`
nào, và có một test nói đúng như vậy.

**Lỗi đã xảy ra.** Wrapper đó từng mang `max-w-3xl`, thứ đã để trống một phần ba bên phải trang
trong khi cột Details của Config xuống ba dòng. Một cap cấp trang duy nhất là dụng cụ sai: sáu
section không muốn cùng một chiều rộng. Config và Query nên dùng hết trang — Config chứa hai
control rộng nhất trong mọi tab, Query chứa editor SQL và lưới kết quả; Access, Try it, Versions
và Response là văn xuôi, danh sách cột ngắn và control ngắn, đọc tệ hơn khi kéo dãn trên màn
hình rộng.

**Vậy nên section tự sở hữu cap của mình**, trên chính element gốc của nó, nằm ngay cạnh layout
nó ràng buộc: `AccessSection.tsx`, `TryIt.tsx`, `Versions.tsx` và `ResponseSection.tsx` đều dùng
`max-w-5xl`, và bảng frozen-column của `ResponseSection.tsx` thêm `max-w-xl`. Query không cần cap nào và được
lợi nhiều nhất — editor SQL từ 768px lên 1424px (đo trong đợt sửa chiều rộng, không phải
bởi tài liệu này; các con số ở §5.4 và §5.5 cũng vậy).

### 5.2 Gỡ cap là chưa đủ, vì `ConfigTable` là `table-fixed`

Đây là lý do chiều rộng ở đây không phải chuyện khẩu vị. Dưới `table-layout: fixed`, một cột
rộng đúng bằng những gì `<th>` của nó nói. `ConfigTable` đặt kích thước cho Setting (`w-72`) và
Value (`valueClassName`, mặc định `w-52`) và để **Details** không kích thước — nên Details là
cột duy nhất có thể hút phần dư, và nó hút *toàn bộ*. Gỡ cap cấp trang vì thế trao 100% chiều
rộng mới cho chữ trợ giúp mờ và để các control đứng nguyên chỗ cũ.

Số đo, trước đợt sửa chiều rộng: ở `w-52` (208px), Description của Config (Textarea `rows={2}`)
hiện khoảng 50 ký tự và ô Path khoảng 25 trước khi phải cuộn. Config nay chọn
`valueClassName="w-52 lg:w-96"`; đo sau khi sửa, cả hai control render 368px ở viewport 1728px.

Hai hệ quả:

- **Núm chỉnh chiều rộng phải nằm ở component, không nằm trong ô của caller.** Một wrapper
  `w-96` bên trong ô `w-52` sẽ tràn sang cột bên cạnh chứ không nới rộng cột. Vì vậy mới có
  `valueClassName` trên `ConfigTable`.
- **Chặn chiều rộng của CHỮ, không chặn cột.** `detailsClassName="max-w-prose"` bọc *nội dung*
  Details, nên cột có thể giữ chiều rộng của nó còn câu chữ dừng ở một độ dài dễ đọc.

Cả hai prop đều mặc định về đúng những gì mọi consumer cũ đã render — `w-52`, và không có
element wrapper nào cả — xem §7.

### 5.3 Chữ `lg:` trong `w-52 lg:w-96` là một số đo, không phải một linh cảm

Dưới `lg`, Details bị `hidden` nhưng hai cột có kích thước **không** co lại cho vừa:
`table-fixed` tôn trọng chiều rộng pixel của chúng và wrapper `Table` của shadcn
(`overflow-x-auto`) sẽ cuộn thay. Ở viewport 500px, hộp nội dung là 356px, nên bảng đã cuộn
140px ngay ở `w-52`; một `w-96` phẳng sẽ đẩy con số đó lên 316px. Đặt điều kiện `lg` giữ nguyên
chế độ hẹp y như cũ.

### 5.4 Một cột chết dưới `lg`, do CHÍNH phase này tạo ra

Ghi lại để không ai "sửa" chiều rộng mà không biết, và được viết đầy đủ trong comment của
`Config.tsx`. `ConfigTableGroup` `colSpan={3}`, khiến bảng vẫn rộng ba cột ngay cả nơi Details
là `display:none` — nên fixed layout trao phần dư cho một cột thứ ba ma không có ô ở hàng đầu,
và nó đáp xuống thành khoảng chết **bên phải** Value. Đo ở viewport 1010px (hộp nội dung 706px):
các cột 288/0/208 với 210px chết; xoá bốn dòng nhóm khỏi chính bảng đang chạy đó cho ra
410/0/296 và không còn khoảng chết.

**Nó KHÔNG có sẵn từ trước, và ba chỗ trong tài liệu này từng nói là có.** `ConfigTableGroup`
là mới trong phase này và là nguồn duy nhất của `colSpan={3}` bên trong một `ConfigTable` trong cây: đo tại merge base
(`63a4fb90`), không file nào import `ConfigTable` chứa một `colSpan` nào cả — nên so với `main`
thì không hề có cột ma, trên bất kỳ trang nào. Điều hẹp hơn mà *đúng*: các dòng nhóm đã đáp
xuống ở những task trước, nên cột đó đã có mặt khi đợt **sửa chiều rộng** đo, và đợt sửa chiều
rộng không tạo ra nó. Chính số đo ở đoạn trên là bằng chứng phản lại — xoá bốn dòng nhóm thì
khoảng chết biến mất.

Đợt sửa chiều rộng có làm nó to hơn trong một chế độ con, và điều đó phụ thuộc vào việc sidebar
là một thanh ray người dùng gập được (`w-56` khi mở, `w-16` khi gập): ở 1010px với ray gập,
370px chết, trong khi cap cũ cho 272px (đo trong đợt sửa chiều rộng). Cách sửa là để
Value thành cột không kích thước dưới `lg`; nó bị hoãn vì **chưa kiểm chứng** rằng một Value
không kích thước sẽ hút hết cột ma chứ không chia đôi với nó.

`ResponseSection` có đúng cột ma đó, từ một dòng nhóm `Output shape` của nó.

### 5.4a Section rơi vào giữa hai task

`ResponseSection` được dựng trước khi có cap cấp route để mà gỡ, còn đợt sửa chiều rộng thì đo
`Config` cùng các tab nó dựng lại — Response không thuộc tập nào, nên nó ship ra mà không có cap.
Đo ở hộp nội dung ~1470px: Setting 288 / Value 208 / Details ~975, với `max-w-prose` chặn câu chữ
ở khoảng 585px — tức ~390px cột rỗng, và hai Select Shape với Key-naming nằm cách nhãn của chúng
~1260px về bên phải, chỗ mà cap cấp route cũ đặt chúng ở ~560px. Đó đúng là lời phàn nàn đã cho
`Versions` cái cap của nó, nên đợt sửa cuối cho Response cùng một gốc `max-w-5xl` (~816px), thứ
cũng thu lại hai Card `Frozen columns` và `OpenAPI document` — *bảng* frozen có `max-w-xl`, Card
của nó thì không.

Quy tắc chung: **một task dựng lại bề mặt và một task đo chiều rộng phải thống nhất cùng một
danh sách bề mặt**, nếu không phần chênh giữa hai danh sách sẽ ship ra.

### 5.5 Test chiều rộng nói được gì và KHÔNG nói được gì

`src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`,
`describe("EndpointDetail — width")`.

Chốt chặn cap-cấp-route mount một **sentinel** (`<div data-testid="section-content"/>`) làm
route con rồi đi ngược lên từ đó, assert không ancestor nào tới gốc route mang `max-w-*`. Hai
hình dạng trước đó sai theo hai hướng ngược nhau và cả hai đều được nêu tên trong test:

- `container.querySelector(".max-w-3xl")` quét cả cây con, cấm một tên class trên toàn trang —
  nhưng một section hoàn toàn có quyền chặn chiều rộng con của chính nó.
- Đi ngược lên từ nội dung của một section *thật* rồi dừng ở "con của route container" sẽ đọc
  chính cái wrapper đang bị săn **thành** gốc section mỗi khi nó tồn tại. Đã đo trong đợt sửa chiều rộng: khôi
  phục `max-w-3xl` quanh `Outlet` vẫn qua 31/31 dưới hình dạng đó. Chốt chặn sẽ chết đúng trong
  trường hợp nó được viết ra để bắt.

Sentinel làm cho mâu thuẫn trở nên bất khả *về mặt cấu trúc*, chứ không phải nhờ chọn đúng
điểm xuất phát.

Điểm mạnh của sentinel cũng là một lỗ hổng, và đợt sửa cuối đã bịt nó. Vì được thu hẹp về route
theo thiết kế, chốt chặn không nói gì về việc một section làm gì với chính nó — nên đặt lại
`max-w-3xl` lên **gốc của chính `Config`**, đúng cái lỗi mà phase này tồn tại để loại bỏ, vẫn qua
chốt chặn sentinel và qua mọi test Config khác: 230/230 xanh. `Versions` đã có chốt chặn nói nó
*sở hữu* cap của mình; `Config` — tab mà cả đợt sửa chiều rộng sinh ra vì nó — thì không có cái
tương ứng. Nay thì có: assertion nghịch đảo, rằng gốc của Config không mang `max-w-*` nào (kể cả
tiền tố responsive), và nó đã được kiểm bằng mutation đúng bằng chính mutation đó — mutation này
nay làm chết một test, và chỉ một. Cap mới của `Response` được chốt theo đúng cách của `Versions`.

Comment của chính test thành thật về giới hạn của nó: các assertion tên class là yếu và cố ý yếu
— chúng chỉ tồn tại để một cap không thể quay lại, hoặc biến mất, trong im lặng. **Chiều rộng có
được dùng tốt hay không là câu hỏi chỉ trình duyệt trả lời được.** Xem §10.

---

## 6. `ConfigTableRow.details` là chuỗi thuần

Không markdown. Không backtick. Chúng render ra thành ký tự backtick nguyên văn.

Prop có kiểu `details?: string | null` trong `ConfigTable.tsx`, và `null` mang nghĩa tường minh
"ép ô Details rỗng dù trường này có `fieldHelp`", khác với việc bỏ prop (khi đó nó lùi về
`fieldHelp[fieldKey]`).

**Đừng nới prop thành `ReactNode`** để có thể chèn markup. Lỗi này được tìm thấy trong trình
duyệt, không phải bằng test, và nó nằm trong danh sách các lỗi chỉ-trình-duyệt-thấy ở
`docs/en/plans/2026-08-17-data-api-p6-authoring-loop.md` và
`docs/en/plans/2026-08-18-explore-list-primitives.md`.

Một lưu ý thực tế rút ra từ đợt dựng lại: một bộ quét byte tìm backtick sẽ báo động ở **comment
JSX** gần đoạn chữ (`unlike \`location\``) — một dương tính giả. Hãy kiểm tra các string
literal, không phải cả file.

---

## 7. Sửa component dùng chung: chỉ được cộng thêm, và đo theo đường import

### 7.1 Chỉ cộng thêm

Đo trên cây code hiện tại:

| Component | Số file import |
|---|---|
| `@/components/config/ConfigTable` | **14** (13 production + test của chính nó) |
| `@/components/form/ConfigSection` | **13** |

Vì thế hai núm chiều rộng của `ConfigTable` là tuỳ chọn với giá trị mặc định được chọn để mọi
consumer hiện có không đổi một byte: `valueClassName` mặc định `"w-52"` — đúng literal trước đây
được hardcode — và `detailsClassName` mặc định `undefined`, tức **không phát sinh element
wrapper nào cả**. Mười một consumer khác đang render ô Details đó hôm nay và không cái nào từng
được đo, đó chính là lý do mặc định không phát sinh gì thay vì phát sinh một thứ nghe hợp lý.

Dạng mạnh nhất của "chỉ cộng thêm" là một diff mà danh sách file của nó tự chứng minh. Khi thêm
`ConfigTableGroup`, chỉ `ConfigTable.tsx` và một file test mới thay đổi, còn `ConfigTable` /
`ConfigTableRow` giống hệt từng byte — nên không consumer nào *có thể* bị ảnh hưởng, mà không
cần suy luận về bất kỳ consumer nào.

### 7.2 Đo theo import specifier, không bao giờ theo tên symbol

`grep -rl "ConfigSection"` là tìm kiếm **chuỗi con**. Trên cây này nó trả 19 file;
`grep -rl 'from "@/components/form/ConfigSection"'` trả 13. Sáu file dư nên được gọi tên từng
cái, vì cách nói "những lần nhắc trong doc comment" là tô hồng chúng:
`src/components/form/ConfigSection.tsx` là định nghĩa của chính component;
`src/routes/control-plane/StarRocksConfigTab.tsx` **không hề** import `ConfigSection` — nó tự
định nghĩa `FeConfigSection` và `CnConfigSection` cục bộ; `ConfigTable.tsx` mang `{@link
ConfigSection}` thật duy nhất; còn `Config.tsx`, `ResponseSection.tsx` và
`EndpointDetail.test.tsx` nhắc nó trong văn xuôi thuần, khi kể lại thứ chúng đã thay thế. Phần
nội dung vẫn đúng — grep chuỗi con đếm dư sáu — nhưng đúng một trong sáu là `{@link}`.

Con số này đã sai ba lần, và mỗi kiểu sai đều đáng nhận diện:

1. **Đoán** — "chín chỗ dùng ở Control Plane", khẳng định mà chưa từng đo.
2. **Đo sai** — chính lệnh grep chuỗi con ở trên, cho ra 16/15. *Đây mới là kiểu đáng học: nó
   TRÔNG như cẩn thận vì có dẫn ra một câu lệnh.*
3. **Phép trừ không nói ra** — sau đó một comment ghi "12 file khác", chỉ đúng sau khi lặng lẽ
   trừ đi chính file đó và file test của component khỏi con số 14.

Hãy dùng import specifier — thứ thật sự tạo ra phụ thuộc — và nói rõ bạn đã đếm cái gì.

---

## 8. Một assertion mà một bản hiện thực hardcode cũng qua được thì không chốt được gì

**Không phải quy tắc UI. Đây là bài học đắt nhất của chương trình này, và nó tổng quát ra ngoài
codebase này.**

Khoảng mười commit fix-round trải năm phase đã dồn vào đúng một lớp lỗi này; theo cách đếm của
chính một ledger nhánh, bảy trong số đó xảy ra trước khi một implementer cuối cùng chịu rà mọi
trường theo tiêu chí này ngay từ đầu. Mọi trường hợp dưới đây đều có thật, và tất cả đều được
bắt ở hạ nguồn chứ không phải bởi người viết assertion.

### Phép kiểm

**Trước khi viết một assertion, hãy hỏi hằng số nào sẽ thoả mãn nó.** Nếu một giá trị hardcode,
một id cố định, hay một `return null` cũng qua được, thì assertion không chốt gì cả. Khi đó hãy
dựng fixture sao cho không hằng số nào qua được.

### Các trường hợp

| Fixture / assertion | Hằng số qua được nó |
|---|---|
| **Danh sách một dòng.** | Một ngôi sao, một nhãn, hay một id hardcode vào *đúng dòng đó* trông y hệt trên một dòng và sai trên mọi dòng khác. Nó tốn đủ nhiều vòng để lý do nay được viết thẳng vào code: doc comment của `toggleBookmark` trong `Endpoints.tsx`, test `"gives two rows two different accessible names"` trong `__tests__/BookmarkToggle.test.tsx`, và `crates/egret-dataapi/tests/bookmarks.rs`. |
| **Danh sách folder rỗng.** Tab saved của SQL Lab đoản mạch bằng `folders.length === 0 ? <danh sách phẳng> : groups.map(…)`, nên các dòng mồ côi vốn đã hiện ra trên workspace không có folder nào. Lỗi chỉ cắn khi đã tồn tại ít nhất một folder. | Một phép gom nhóm ném dòng vào một bucket mà vòng lặp đầu ra không bao giờ đọc — đúng lỗi mà `groupByFolder` trong `src/components/explore/folders.ts` nay ghi lại và phòng. (Chính sự bất đối xứng đó — cùng dữ liệu, hiện hay không tuỳ vào việc có tồn tại một folder *không liên quan* — đã là bằng chứng nó chưa bao giờ là hành vi có chủ đích.) |
| **Một user.** Chưa test nào từng có user thứ hai với một dòng để phá, nên **cả hai** vị từ trên câu `DELETE` của `remove_bookmark` đều tự do. | Một `DELETE` thiếu `user_id = $1`. Đóng lại bằng một fixture nơi A và B cùng star một endpoint rồi A xoá; mutation nay chết một mình ở *"B's star on the same endpoint must survive A's DELETE"* (`crates/egret-dataapi/tests/bookmarks.rs:463`). |
| **Một database trong picker.** | `database_id = databases[0].id`. Đóng lại bằng fixture hai database và chọn cái thứ hai. |
| **Giá trị test trùng với chính placeholder của trường.** Cả năm test đều gõ `"reporting-bot"`, cũng chính là `placeholder` của ô nhập. | `name: "reporting-bot"` hardcode vào body request — nó qua cả năm test. Đóng lại bằng cách **không test nào ASSERT `name` trong body** còn gõ placeholder, và không hai test trong số đó gõ cùng một giá trị; lý do được viết ở đầu `__tests__/ServiceAccountCreate.test.tsx`. (Các test không assert gì về body thì vẫn gõ nó, nên quy tắc là về những test có assert, không phải về cả file.) |
| **Một enum mà bạn assert giá trị KHÔNG mặc định.** Chọn POST rồi assert POST *vẫn* qua được một body hardcode POST. | Assert giá trị không-mặc-định không làm một enum hai giá trị trở nên bác bỏ được — **nó chỉ dời xem hằng số nào là không bác bỏ được.** Phải chốt cả hai giá trị: một test chọn POST và assert POST, một test khác không chạm picker và assert GET. Chỉ bản hiện thực thật sự đọc control mới qua cả hai. |
| **Một test đặt tên theo một quan hệ mà các assertion của nó không bao giờ chạm tới thứ liên quan.** `"keeps ordinary rows in their three columns"` sống sót qua `ConfigTableGroup → return null`; nó chỉ tái xác nhận hành vi có sẵn của `ConfigTableRow`. | Bất cứ thứ gì. Đây là kiểu khó thấy nhất, vì *tên* test mô tả một tương tác và từng assertion nhìn riêng đều hợp lý. Cách sửa không phải thêm assertion thứ ba bên cạnh hai cái đã qua, mà làm cho các assertion sẵn có **phụ thuộc** vào dòng nhóm — `getByRole("rowheader")` làm tiền điều kiện, cộng `nextElementSibling` cho tính kề nhau, một tính chất mà `return null` không thể thoả mãn. |

Một trường hợp suýt lọt cùng hình dạng, bắt được trước khi commit: assertion "một bảng, bốn dòng
nhóm, đúng thứ tự" vẫn qua với một trang Config có `Name` nằm dưới `Cache`. Chỉ assertion về
**thành viên hàng** (`endpointAt < fieldAt < cacheAt`) mới chết dưới mutation đó.

### Hệ quả 1 — một mutation không biên dịch được thì không chứng minh gì

Ba lần riêng biệt trong chương trình này, bản cắt đầu tiên của một mutation không build được:
`TS6133` tham số không dùng dưới `noUnusedParameters`, và một import bị bỏ không. **Một mutation
không build được trông y hệt một mutation mà không test nào chết.** "Không có gì fail" khi đó là
sản phẩm của một lỗi gõ, không phải bằng chứng.

Vậy: **build mutation trước, xác nhận 0 lỗi, rồi mới đọc kết quả test.** Cắt lại nếu nó không
build — `(_label)` thay cho `(label)`, hoặc `{false && …}` để giữ import còn sống với tsc trong
khi không render gì — và nói rõ vì sao bản cắt lại có cùng sức nặng.

### Hệ quả 2 — một mutation sống sót là hai điều rất khác nhau

Hoặc assertion **rỗng nghĩa**, hoặc mutation là một **tương đương thật sự** (nó không thật sự
đổi hành vi). Hai điều đó đòi phản ứng ngược nhau — siết chặt test, so với cắt một mutation tốt
hơn — và phải phân biệt được trước khi làm bất cứ điều nào. Trong chương trình này, một hướng
sửa đã được kê đơn suýt ship ra một *chốt chặn chết* đúng vì một mutation sống sót bị quy sai
sang một trong hai; xem §5.5.

### Hệ quả 3 — một báo cáo "xanh" chỉ là một lời khẳng định

Khi kết quả mutation quan trọng, hãy chạy lại thay vì chấp nhận báo cáo. Chương trình này đã làm
đúng vậy hai lần, một lần nói thẳng lý do *"vì đây là lần thứ hai độ phủ của đúng test này bị
đánh giá sai"*, và một lần bằng cách tái hiện mutation đấu chéo để xác nhận `path` vắng mặt còn
`name` mang giá trị lộn xộn.

---

## 9. Hãy ĐO, đừng nhớ lại

Bảy khẳng định trong chương trình này nói về trạng thái của thế giới mà không đo. Cả bảy đều chỉ
được bắt ở hạ nguồn.

- Một trường bị báo là **thiếu** trong khi nó đã tồn tại (`folder_id`).
- Một component bị báo là **không ai dùng** — `EmptyState`, thứ hôm nay có **mười file** import
  (đã đo: `grep -rl 'components/explore/EmptyState'`).
- Một mục permission bị báo là **vắng mặt**, chặn việc tạo folder. Nó đã ship từ **nhiều tháng
  trước**: `kind_perms` tại `crates/egret-bi/src/routes.rs`, xác định thời điểm bằng
  `git log -S`, cùng migration `0122` gieo `data_api_endpoint:read` và `:manage` cho `ws_admin`.
- Một con số consumer bị đoán, rồi bị đo sai, rồi bị lặng lẽ làm tròn — §7.2.

Niềm tin sai đó còn được **viết thẳng vào codebase**, trong ba doc comment khẳng định folder cho
endpoint là không tạo được. Hai cái được sửa và cái thứ ba được tìm ra sau — đúng cái mà người
đọc thật sự đáp xuống, vì nó nằm *trên* chính dòng nó giải thích. Một trong số đó dùng niềm tin
sai làm **lý do biện minh** cho một hành vi có thật (truyền `folder_id: None` cho PDP). Đó mới là
hình dạng nguy hiểm: code đúng tựa trên một lý do không còn đúng nữa.

**Khẳng định sự VẮNG MẶT mới là kiểu sai đặc thù.** "Không có X" là một khẳng định về toàn bộ cây
code và không thể nhớ lại — nó phải được chạy.

---

## 10. Đọc code không quyết định được layout

Năm lời giải thích nối tiếp nhau về hành vi của một bảng dưới breakpoint `lg` đều sai, cái sau
sửa cái trước. Chỉ một phép đo trên trình duyệt mới kết thúc được phần hình học — và một
`git grep` ở merge base mới kết thúc được phần quy trách nhiệm.

1. Hai implementer suy từ các class rằng cột Value nở ra dưới `lg`.
2. Người thứ ba mở trình duyệt: **sai** — và tiện thể phát hiện rằng `max-w-md` vừa thêm vào hai
   `SelectTrigger` là **vô tác dụng**, vì shadcn đặt `w-fit`.
3. Một reviewer dựng bản tái hiện trong Chrome: **cả hai lời giải thích trước đều sai cơ chế.**
   Phần dư đi vào một cột thứ ba ma không có ô ở hàng đầu (§5.4) — không phải một Value rộng
   hơn, cũng không phải một thanh cuộn.
4. Implementer tái hiện trên bảng **thật** (288/0/208, 210px chết ở 1010px) và bổ sung rằng cột
   chết vốn đã có sẵn.
5. Reviewer thu hẹp thêm một lần nữa: số đo chỉ đúng khi thanh ray sidebar **đang mở**. Khi gập
   (`w-16`), cap cũ có ràng buộc thật, và đợt sửa nới cột chết từ 272px lên 370px ở đó.
6. Đợt review toàn branch cuối cùng phát hiện **cái nhãn** mới là chỗ sai, sau khi năm vòng đã
   chấp nhận nó: "đã có sẵn" chỉ đúng khi so với đợt sửa chiều rộng. `ConfigTableGroup` — và cùng
   nó là `colSpan={3}` duy nhất bên trong một `ConfigTable` — là mới trong branch này, nên so với `main` thì cột
   chết là của chính branch này (§5.4). Không ai đo lại cái khẳng định cần một `git grep` chứ
   không cần trình duyệt, và tài liệu thì đã chứa bằng chứng phản lại chính nó ở câu ngay trước.

Những quy tắc thực tế rút ra:

- **jsdom không phán xét được layout.** Một assertion tên class chốt rằng class có mặt, chứ
  không chốt gì về hình học kết quả. Hãy nói điều đó ngay trong test, như §5.5 làm.
- **Kiểm tra dụng cụ trước.** Hai lần trong chương trình này, dev server đang chạy cũ hơn binary
  đã build (một lần 66 phút, một lần 9 tiếng). Không rebuild thì phép kiểm chẳng đo được gì — và
  một trong hai lần, mọi cú click trong bài kiểm sẽ 404.
- **Nêu rõ viewport VÀ trạng thái sidebar** kèm mọi số đo. Chỉ nói "1010px" đã đủ mơ hồ để một
  khẳng định đúng trông như sai.
- **Một tab có cột Details là `hidden lg:table-cell` có hình dạng KHÁC dưới 1024px.** Hai
  viewport 1440px và 1280px là *cùng* một chế độ; phép kiểm trình duyệt cần một viewport thứ ba
  dưới `lg`.

---

## Tình trạng áp dụng

Phạm vi thật, đo trên cây code hiện tại, để không ai đọc tài liệu này như mô tả code chưa tồn
tại:

- `ListPageShell` — 5 file route (`Endpoints`, `Clients`, `EndpointCreate`,
  `ServiceAccountCreate`, `ClientCreate`), toàn bộ thuộc Data API. Các trang danh sách Explore
  còn lại vẫn tự viết header.
- `ListStates` — 2 file route (`Endpoints`, `Clients`).
- `EmptyState` — 10 file (2 Data API, 8 control-plane).
- `ConfigTable` — 13 file production; `ConfigTableGroup` được dùng bởi `Config` và
  `ResponseSection` của Data API.
- `ConfigSection` — 13 file, giảm từ **15** ở merge base. Đây là lần đếm thứ năm của con số này
  trong chương trình và là lần đầu đo ở cả hai đầu:
  `grep -rl 'from "@/components/form/ConfigSection"' src` cho 15 ở `63a4fb90` và 13 ở `HEAD`.
  14 là một trạng thái giữa branch, bị dẫn ra như thể nó là base. Nếu thêm section chuyển sang dòng nhóm, hãy xem lại
  xem nó còn xứng đáng tồn tại không.

Đưa một trang lên các primitive này là một thay đổi *của trang đó*. **Bản thân các primitive chỉ
thay đổi theo kiểu cộng thêm** (§7).

## Nguồn

- Component: `frontend/src/components/explore/`,
  `frontend/src/components/config/ConfigTable.tsx`,
  `frontend/src/components/form/ConfigSection.tsx`
- Trang tham chiếu: `frontend/src/routes/explore/dataapi/`
- Route và lý do của chúng: `frontend/src/App.tsx`
- Spec thiết kế: `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md`
- Plan mang kỷ luật kiểm-trên-trình-duyệt:
  `docs/en/plans/2026-08-18-explore-list-primitives.md`,
  `docs/en/plans/2026-08-17-data-api-p6-authoring-loop.md`
