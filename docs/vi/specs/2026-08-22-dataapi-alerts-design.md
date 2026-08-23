# Cảnh báo endpoint Data API, và việc áp quy ước Explore mà nó kéo theo

*(Bản tiếng Anh tại `docs/en/specs/2026-08-22-dataapi-alerts-design.md` là bản chuẩn; bản này để tham chiếu.)*

Ba sản phẩm nhưng là một việc. Data API cần cảnh báo khi lỗi, editor của alert là nơi
đặt cấu hình đó, và editor ấy nằm trên một trang mà phần list của nó chưa bao giờ áp
quy ước mà chính repo này đã tự viết ra. Thêm một section vào trang dựng theo idiom cũ
sẽ làm trang đọc ra hai idiom cùng lúc, nên việc áp quy ước đi trước và tính năng hạ
cánh trên nền đã sạch.

Việc này bắt nguồn từ một phép so sánh với DBAPI Enterprise Edition (tài liệu sản phẩm
72 trang, đọc hết). Phép so sánh đó nằm ở Phụ lục A, vì nó là thứ quyết định phạm vi —
kể cả bốn thứ nó quyết định **không** làm.

## 1. Việc này đến từ đâu

DBAPI là công cụ low-code biến SQL cộng cấu hình tham số thành một HTTP API. Data API
của egret cùng ý tưởng nhưng có engine được quản trị bên dưới. Đo theo danh sách tính
năng của DBAPI, egret ngang hoặc hơn ở phần lõi — định nghĩa endpoint, vòng đời
publish/version, credential client, rate limit, cache, access log, OpenAPI, Try-it — và
thua ở bề rộng.

Bốn tính năng bề rộng của DBAPI đã được cân nhắc và **loại**, kèm lý do, ở Phụ lục A:
HTTP proxy executor, Elasticsearch executor, MCP server, và canvas orchestration. Một
thứ bị loại vì nó **đã tồn tại ở dạng tốt hơn**: `render.rs` cho egret SQL điều kiện
kiểu MyBatis mà không mang theo bề mặt inject của MyBatis (§A.2).

Thứ còn lại là tám gói. Spec này phủ ba gói đầu; năm gói còn lại được nêu tên ở §7 để
thứ tự nằm trên giấy tờ.

| | Gói | Spec này |
|---|---|---|
| 1 | **U** — bốn trang list Explore lên primitives dùng chung | §3 |
| 2 | **E** — editor Alerts lên `ConfigTable` | §4 |
| 3 | **Z2** — cảnh báo endpoint Data API | §5 |
| 4 | Z1 — IP allow/deny và endpoint public | §7 |
| 5 | M — nhiều câu SQL trong một endpoint (chỉ đọc) | §7 |
| 6 | C — rule validate tham số và biến đổi dữ liệu trả về | §7 |
| 7 | D — câu lệnh ghi và transaction | §7 |
| 8 | Z3 — import/export cấu hình endpoint | §7 |

Mỗi gói là một PR riêng. U và E không đổi hành vi; Z2 thêm tính năng. Tách ra để một
regression ở gói này revert được mà không mất gói kia, và để người review tính năng
không phải review kèm 1.788 dòng refactor.

## 2. Cái gì đã đo, cái gì thì không

Mọi khẳng định dưới đây được **đo trên cây mã tại thời điểm viết**, không phải nhớ lại.
Hai thứ spec này tưởng sẽ tìm thấy mà **không** có được ghi ở §3.2, vì một danh sách
lỗi chỉ dài thêm là danh sách không ai kiểm.

Hai con số kế thừa **cố ý không** mang sang: eslint baseline của plan cũ ("0 errors /
178 warnings") và mọi con số trong mục Adoption status của tài liệu quy ước. Cả hai
phải đo lại khi bắt đầu thực thi. Trích một con số đo ở revision khác như thể nó là
hiện tại chính là lỗi repo này đã ghi lại hai lần.

---

## 3. U — bốn trang list Explore lên primitives dùng chung

### 3.1 Đây là một khoản hoãn được giao lại, không phải ý mới

`docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` §10 có ghi:

> **Migrating Datasets / Charts / Dashboards / Alerts onto the primitives.** The point
> of doing Data API first is that it cannot regress anything; migrating four working
> pages is a separate risk with a separate review.

và `docs/en/plans/2026-08-18-explore-list-primitives.md` mang nó thành ràng buộc toàn cục:

> **Do not touch `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` or `Alerts.tsx`.**
> Migrating them is a separate risk with a separate review (spec §10). This plan proves
> the primitives on pages that have none of this today, precisely so nothing working
> can regress.

"Separate review" đó chính là gói này. Primitives giờ đã được chứng minh trên năm route
Data API — đúng điều kiện mà khoản hoãn đang chờ.

### 3.2 Số đo

```text
                 aria-pressed  ListPageShell  ListStates  ListToolbarButton   dòng
Alerts.tsx                  0              0           0                  0    687
Datasets.tsx                0              0           0                  0    387
Charts.tsx                  0              0           0                  0    333
Dashboards.tsx              0              0           0                  0    381
```

**Lỗi đã xác nhận là `aria-pressed`.** Trạng thái bật/tắt chỉ được mang bởi
`variant={onlyBookmarked ? "secondary" : "outline"}` — một class hình ảnh. Screen reader
đọc "Bookmarks" y hệt nhau dù filter đang bật hay tắt. Đây đúng là §1.3 của tài liệu quy
ước, nơi đã nêu tên bốn trang này là nguồn của quy tắc và ghi rằng không trang nào đặt
thuộc tính đó.

**Hai thứ spec này tưởng sẽ tìm thấy mà không có.** Cả hai đã được kiểm trước khi định
viết thành lỗi, và cả hai đã đúng sẵn:

- Nút Trash **không** bị gate bởi `hasData` ở cả bốn trang — nó nằm ngoài mọi điều kiện.
  §1.2 "giữ đường ra khỏi thùng rác rỗng" đã đúng.
- Vào chế độ trash có clear filter bookmark và bulk mode
  (`setOnlyBookmarked(false); bulk.setBulkMode(false)`). Nửa sau của §1.2 cũng đã đúng.

Mọi nút toolbar cũng đã có `aria-label`, nên vấn đề là **trạng thái**, không phải tên.

Một chi tiết cho thấy primitive đã được thiết kế cho đúng những trang này: `Star` và
`Trash2` dùng `fill-current` khi bật, `ListChecks` và `FolderPlus` thì không. Đó chính là
lý do `ListToolbarButton` tách `filled` khỏi `active` (§1.3) — việc chuyển đổi không cần
thêm prop nào cho chuyện đó.

### 3.3 Nội dung chuyển đổi

Mỗi trang: `ListPageShell` (§1.1), `ListStates` (§2), `ListToolbarButton` (§1.3),
`EmptyState` (§2.4). Không đổi hành vi.

`Alerts.tsx` tự viết tay đúng cái header mà shell sinh ra để sở hữu:

```tsx
<div className="p-4">
  <div className="mb-4 flex items-center justify-between gap-2">
    <h1 className="text-lg font-semibold">…</h1>
```

và rẽ nhánh trạng thái theo thứ tự khác thứ tự đã ghi
(`isError ? … : isLoading ? … : hasData ? …`, trong khi §2 quy định
`isLoading → isError → !hasData → rows`).

**Chỗ lệch thứ tự của Alerts KHÔNG được khẳng định là bug.** Với TanStack Query hai cờ
hiếm khi cùng đúng, và không ca hỏng nào được tái hiện. Nó được ghi là một chỗ lệch quy
ước mà việc chuyển sang `ListStates` làm thành vô nghĩa — ghi lại để người đọc sau không
nhầm "không có báo cáo lỗi" với "không ai kiểm".

**`Dashboards.tsx` thì khác, và đó là một lỗi thật.** Nó rẽ
`isLoading ? … : hasData ? … : isError ? …` — kiểm `hasData` **trước** `isError` — đúng
bằng chỗ trôi mà §2.1 của tài liệu quy ước đã ghi:

> `Dashboards.tsx` checked `hasData` before `isError`, so a failed refetch that still
> held cached rows rendered the table and swallowed the error, while the other five
> rendered the alert.

Lỗi đó vẫn còn. `Datasets.tsx` và `Charts.tsx` đều dùng đúng thứ tự đã ghi, nên đây là
một trang chứ không phải bốn. **Vì vậy U không thuần tuý là refactor: nó sửa một lỗi bị
nuốt.**

**Một sửa lỗi thứ hai cả bốn trang được miễn phí.** Trang nào cũng render
`(error as Error)?.message ?? "Failed to load X."`. Một error không có message — một
`UnauthorizedError`, hoặc một `ApiError` dựng từ `statusText` rỗng — cho ra `""`, và `??`
render nó thành một alert destructive trắng trơn. `ListStates` dùng `|| errorText` chính
vì lý do đó.

### 3.4 Rủi ro cấu trúc duy nhất: thanh tab của Alerts

`Alerts.tsx` render một `Tabs` (Alerts / Reports) **giữa header và bảng**.
`ListPageShell` có slot `toolbar` và slot `action`, cả hai đều nằm trong hàng header, và
không slot nào là chỗ của một thanh tab.

**Cách giải: render `Tabs` làm con đầu tiên trong body của shell. Không sửa primitive.**
§7.1 yêu cầu component dùng chung chỉ đổi theo hướng cộng thêm, và một slot thêm cho một
consumer là khởi đầu của cái shell một-prop-mỗi-năng-lực mà §1.1 đã bác.

**Việc này phải kiểm bằng trình duyệt, không phải bằng cách đọc diff.** Mép trái thanh
tab phải trùng mép header, và `ScheduleEditor.tsx` đã có sẵn comment về đúng lỗi này trên
đúng trang này:

> When the form carried its own `max-w-3xl` it ended short of the tabs above it, which
> reads as a rendering fault rather than a choice.

§10 của tài liệu quy ước nói cùng điều đó ở mức tổng quát: đọc mã không giải quyết được
layout.

---

## 4. E — đưa các TRƯỜNG của editor Alerts lên `ConfigTable`

**Mục này được viết lại sau khi đọc chính đoạn mã nó mô tả. Bản trước khẳng định ba thay
đổi, và hai rưỡi trong số đó sai.** Các lỗi được ghi lại chứ không lặng lẽ bỏ đi, vì cả ba
đều đến từ việc đọc một quy tắc trong `docs/en/ui-conventions.md` rồi áp dụng mà không kiểm
xem tình huống quy tắc đó sinh ra để giải có phải tình huống ở đây không.

### 4.1 Cái thật sự sai, và chỉ có một

*(Đã giải quyết bởi package E — xem plan. Mục này mô tả code TRƯỚC package đó; các chồng
viết tay và grid hai cột nó nhắc đến không còn tồn tại.)*

Bên trong mỗi section của `ScheduleEditor.tsx`, mọi trường đều là một chồng viết tay: một
`<Label>`, một control, và thường có `<p class="text-xs text-muted-foreground">` chèn phía
dưới. Quy tắc **thuận** của §4.1 áp dụng — một nhóm setting là bảng `Setting | Details |
Value` — và cái được là cụ thể: helper text ra khỏi chỗ chen dưới control để vào cột
Details, đúng nơi quy ước đặt nó.

Grid hai cột `Name` + `Description` của section `General` cũng chuyển được. Comment của nó
nói grid tồn tại vì "một hàng full-width cho mỗi trường khiến section chủ yếu là khoảng
trắng còn field thì bị kéo dài quá mọi độ dài nó sẽ có" — và `ConfigTable` giải đúng điều
đó, tốt hơn: cột Value có cap (`w-52` mặc định, `valueClassName` để nới), nên control không
thể giãn, và phần rộng dôi ra thuộc về Details chứ không thuộc về khoảng trắng.

### 4.2 Ba thứ được giữ, và vì sao

**Khung `ConfigSection` được giữ.** Defect ở §4.1 là *xếp chồng* — "Cache three table rows,
Limits two, Rate limit three. Read down the page, that was a column of mostly-empty boxes."
`ScheduleEditor` render `{section === "general" && …}`, mỗi lúc một card, do tab bar chọn.
Không có cột nào, nên cũng không có hộp rỗng nào trong đó.

**`mx-auto max-w-4xl` ở `ScheduleDetail.tsx` được giữ.** Nó mang sẵn lý do tại
`ScheduleDetail.tsx:114-116`: *"`4xl`, not `6xl`: with the section nav now horizontal there
is no column beside the form to fill, and a 1150px-wide single-column form is past a
readable measure. Everything on the page shares this one edge."* Quy tắc §5.1 tồn tại vì
sáu section của Data API **muốn bề rộng khác nhau** — Config và Query muốn cả trang, Access
và Try-it thì không. Ở đây cả bốn section đều là cùng một form một cột, và header, tab bar,
form dùng chung một mép. Bỏ cap là kéo cả ba quá khổ đọc — đúng thứ comment đã từ chối.

**`DestinationList` giữ nguyên.** Câu "một listing chỉ đọc không phải setting" của §4.2
không mô tả nó: mỗi dòng là một *form* có trường khác nhau theo kind — email nhận danh sách
địa chỉ, Slack nhận channel, webhook nhận URL và secret. Đó là một tập do người
dùng soạn ra, không phải một danh sách bản ghi, và ép form-theo-kind vào hàng bảng sẽ tệ
hơn thứ đang có.

### 4.3 Phạm vi

Chỉ `ScheduleEditor.tsx`. Hai thứ trong file đó đã đúng, có comment tự giải thích, và không
được động vào: dirtiness tính bằng **so sánh snapshot** chứ không phải nghe sự kiện change
(`ScheduleEditor.tsx:367-391`), và form là **một cột không tự giữ width** để trang đặt khổ.

## 5. Z2 — cảnh báo endpoint Data API

### 5.1 Hình dạng

Không scheduler mới, không tầng gửi mới. Một alert Data API là một hàng trong
`automation.schedules` với `type = 'dataapi_alert'`. Scheduler hiện tại nhặt nó **mà không
cần sửa**, vì claim query của nó là `WHERE active AND next_run_at <= now()` và không đọc
`type`.

Mọi thứ chương trình Alerts & Reports đã dựng đều dùng lại nguyên: cron và timezone,
`grace_period_s`, owner số nhiều, recipient qua email / webhook / Slack, run log, folders /
trash / bookmarks, trang editor có section nav, và token `{{date}}` cho subject.

Nửa so sánh cũng dùng lại. `validator_type = 'operator'` đang so một scalar với ngưỡng. Một
metric Data API cũng là scalar. **Cái mới là NGUỒN của scalar đó, không phải phép đánh giá
nó.**

Đúng hai thứ mới: một evaluator đọc `dataapi.access_log`, và một section nguồn trong editor.
**Một chi phí spec này nói nhẹ đi, đo được sau khi viết.** `evaluate_and_deliver`
(`runner.rs:185-190`) KHÔNG phải `match` theo `type` — nó là `if s.r#type == "report"
{ … }` rồi rơi thẳng xuống nhánh alert, nên một type thứ ba tự động rơi vào nhánh alert.
Có cơ chế chặn (`NotRunnable("alert is missing its query")` ở `:194-200`), nên hỏng ồn ào
chứ không chạy sai — nhưng làm dispatch tường minh là việc phải làm, không miễn phí.

Runner rẽ nhánh theo `type` — `alert` đi `EngineRunner` (SQL qua `QueryExecPort` dưới danh
tính owner), `dataapi_alert` đi metric evaluator (pool metastore, không dính danh tính engine
nào).

### 5.2 Vì sao đây không thể là chuyện cấu hình

`automation.schedules.database_id` là
`uuid REFERENCES query.databases(id) ON DELETE SET NULL`. Một alert chỉ chạy SQL được trên
engine. `dataapi.access_log` nằm ở Postgres control-plane. **Không alert nào hiện có quan
sát được nó bằng bất kỳ cấu hình nào.** Khoảng trống đó chính là toàn bộ Z2.

**Một hướng nghe rẻ bị loại.** Đăng ký Postgres control-plane thành một hàng
`query.databases` sẽ làm alert SQL chạy được mà không cần code mới — và sẽ phơi `users`,
tham chiếu secret và `access_grants` cho bất kỳ ai có `alert:manage`.

### 5.3 Vì sao chọn cron chứ không phải event

`egret-events` là bus broadcast in-process mà tài liệu của chính nó ghi rằng *một subscriber
bị chậm sẽ làm rơi các event cũ*. Bắn thông báo ngay lúc request phục vụ thất bại vì thế
nghĩa là alert có thể **im lặng bỏ mất đúng cái lỗi mà nó sinh ra để báo**, khi tải cao —
tức là đúng lúc nó quan trọng nhất. Làm đường đó tin cậy cần một outbox bền và cơ chế chống
ngập, và đó là một gói riêng chứ không phải một hook.

`dataapi.access_log` là bền. Một cron alert đọc một cửa sổ của nó không thể mất event.

### 5.4 Mô hình dữ liệu — `0127_dataapi_alerts.sql`

```sql
-- Mở rộng từ vựng type. Đây là CHECK có sẵn DUY NHẤT bị đổi.
ALTER TABLE automation.schedules DROP CONSTRAINT schedule_type_valid;
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_type_valid
  CHECK (type IN ('alert', 'report', 'dataapi_alert'));

ALTER TABLE automation.schedules
  ADD COLUMN dataapi_endpoint_id uuid REFERENCES dataapi.endpoints(id) ON DELETE CASCADE,
  ADD COLUMN dataapi_metric      text,
  ADD COLUMN dataapi_error_code  text,
  ADD COLUMN dataapi_window_s    integer;

CREATE INDEX access_log_ws_ep_ts_idx
  ON dataapi.access_log (workspace_id, endpoint_id, ts DESC);
```

**Dùng `type` mới chứ không thêm cột `subject` vào `type = 'alert'`.**
`schedule_alert_needs_query` và CHECK hình dạng của report đều viết dạng
`type <> 'x' OR (…)`, nên một type mới tự loại trừ khỏi cả hai và **không cái nào bị đụng**. (CHECK của report tên là
`schedule_report_has_format`, không phải `schedule_report_needs_target` như bản trước của mục
này ghi — `0115` đã đổi tên nó. Kiểm chứng chỉ dựa vào `0110` sẽ bỏ sót điều đó, và đã bỏ sót.)
Một discriminator `subject` thì ngược lại, buộc phải nới một constraint đang bảo vệ dữ liệu
thật — đúng chỗ một hàng hỏng lọt vào.

Bốn ràng buộc, mỗi cái làm một trạng thái sai trở nên **không biểu đạt được** chứ không chỉ
là bị khuyên tránh:

```sql
-- Từ vựng metric đóng tại database. Một metric gõ sai không phải hàng hơi sai: nó là
-- hàng mà phép khớp của evaluator không bao giờ chọn, hỏng im lặng và vĩnh viễn. Cùng
-- lý lẽ với các CHECK enum của chính 0110.
CHECK (dataapi_metric IS NULL OR dataapi_metric IN
  ('error_rate', 'error_count', 'p50_duration_ms', 'p95_duration_ms',
   'call_count', 'error_code_count'))

-- error_code khi và chỉ khi metric là error_code_count. Hai mệnh đề một chiều thay vì
-- một phép bằng, để ngữ nghĩa NULL vẫn hiển nhiên.
CHECK (dataapi_error_code IS NULL OR dataapi_metric = 'error_code_count')
CHECK (dataapi_metric <> 'error_code_count' OR dataapi_error_code IS NOT NULL)

-- Hình dạng của một dataapi_alert. Nửa load-bearing là `sql IS NULL`: nó làm cho
-- "một hàng mà cả hai evaluator đều nhận là của mình" không viết ra được.
CHECK (type <> 'dataapi_alert' OR (
  dataapi_metric IS NOT NULL
  AND dataapi_window_s IS NOT NULL AND dataapi_window_s > 0
  AND validator_type = 'operator'
  AND database_id IS NULL AND sql IS NULL
  AND chart_id IS NULL AND dashboard_id IS NULL))

-- Và chiều ngược lại: một alert engine không được mang các cột Data API.
CHECK (type = 'dataapi_alert' OR (
  dataapi_metric IS NULL AND dataapi_endpoint_id IS NULL
  AND dataapi_window_s IS NULL AND dataapi_error_code IS NULL))
```

`dataapi_endpoint_id` nullable và khi NULL nghĩa là "mọi endpoint trong workspace" — một
cột, hai phạm vi, không bảng nối. FK cascade khi xoá vì một alert về một endpoint đã xoá
cứng là vô nghĩa; endpoint bị xoá mềm trong vận hành bình thường, và §5.6 xử lý ca đó riêng.

### 5.5 Định nghĩa metric là sao chép, không phát minh lại

Một alert báo con số khác con số trên panel thống kê là một ticket support. `activity.rs` đã
chốt cả ba mẫu số, và evaluator lấy nguyên văn:

| Metric | Định nghĩa | Vì sao |
|---|---|---|
| `error_rate`, `error_count` | `status_code >= 400`, mẫu số là mọi dòng | |
| `p50_duration_ms`, `p95_duration_ms` | `percentile_cont(…) FILTER (WHERE status_code BETWEEN 200 AND 299)` | một loạt 401 trả về trong 2 ms kéo p50 xuống, làm độ trễ trông đẹp hơn **đúng lúc hệ thống đang hỏng** |
| tất cả | `actor_user_id IS NULL` | loại trial. Không có nó, một dev bấm **Try it** hai chục lần trên draft hỏng sẽ bắn alert production |

Phần loại trial là thứ dễ bị bỏ nhất bởi một người đơn giản hoá câu truy vấn về sau, và cũng
là thứ có kiểu hỏng ồn ào nhất.

### 5.6 Cửa sổ rỗng, và vì sao cách xử lý bất đối xứng

**Bất đối xứng này là cố ý. Nó trông như một sự thiếu nhất quán và sẽ bị "dọn dẹp" ở lần
refactor sau nếu lý do không đi kèm nó.**

- `error_rate`, `p50_duration_ms`, `p95_duration_ms` → `NULL` → trạng thái run `noop` (đã có
  sẵn trong từ vựng `schedule_last_state_valid`). Không có traffic thì không có tỉ lệ. Trả
  `0` sẽ cho phép một ngưỡng `< 0.99` bắn mỗi năm phút trên một endpoint không ai gọi.
- `call_count`, `error_count`, `error_code_count` → **`0`**, không phải NULL. Đây chính là
  toàn bộ điểm của `call_count`: "endpoint tắt tiếng" *chính là* sự vắng mặt của các dòng, và
  ánh xạ nó thành `noop` sẽ làm metric đó không thể phát hiện được, theo cấu trúc.

**Chưa làm (§8).** Một endpoint đang trong thùng rác hoặc `disabled` LẼ RA phải cho ra `noop`
kèm lý do ghi vào run log — không phải `error` — vì bỏ endpoint vào thùng rác là hành động cố
ý và báo lỗi mỗi năm phút vì chuyện đó là nhiễu. Phần đó đã bị hoãn: doc comment của
`evaluate_and_deliver_dataapi_alert` (`runner.rs`) giải thích vì sao, và hiện chưa có lý do
riêng "endpoint đã vào thùng rác" nào trong run log.

Điều thực sự xảy ra thay vào đó, như hệ quả phụ của cách xoá mềm hoạt động chứ không phải do
thiết kế: cột `endpoint_id` trên `dataapi.access_log` vốn không có foreign key nào cả — xem
comment trong `migrations/0123_dataapi_access_log.sql`, giải thích khá dài vì sao một access log
phải sống sót qua việc xoá cái nó mô tả. Nên một endpoint bị xoá mềm hoặc `disabled` không phải
bị "tháo tham chiếu" khỏi các dòng log cũ; nó chỉ đơn giản là không còn khớp điều kiện
`WHERE e.state = 'live'` của `live_snapshot` (`crates/egret-dataapi/src/serving/router.rs`),
router trả 404 trước khi `entry.endpoint_id` từng được gán, và không có dòng mới nào cho
endpoint đó được ghi ra nữa — không có tham chiếu nào để mà xoá cả. Hiệu ứng thực tế thì vẫn
vậy: một watch theo endpoint đơn giản là ngừng tích luỹ traffic từ lúc đó. `call_count`/
`error_count`/`error_code_count` khi đó đọc ra `0` thật — không phải `noop` giả — còn
`error_rate`/`p50_duration_ms`/`p95_duration_ms` đọc ra `noop` từ tập rỗng kết quả, theo đúng sự
bất đối xứng mục này đã mô tả cho một cửa sổ rỗng bình thường, chỉ là đi tới đó bằng một con
đường khác. Một watch toàn workspace (`dataapi_endpoint_id IS NULL`) không bị ảnh hưởng — nó
đếm traffic của mọi endpoint, dù có bị xoá mềm hay không.

**Một giới hạn được nói rõ.** Alert này đọc một log do egret ghi ra. Nếu egret không chạy thì
không có dòng nào, cửa sổ rỗng, và alert im lặng. Nó phát hiện endpoint hỏng; nó **không**
phát hiện hệ thống chết. Việc đó cần probe từ bên ngoài và nằm ngoài phạm vi.

### 5.7 Phân quyền

`alert:manage` chi phối tạo và sửa, như hiện nay. Thêm: **endpoint được chọn phải đọc được
bởi người tạo.** Tỉ lệ lỗi và lưu lượng của một endpoint là thông tin về nó; không có kiểm
tra này thì `alert:manage` trở thành đường vòng để quan sát một endpoint mình không được thấy.

Kiểm lúc lưu **và** kiểm lại lúc chạy dưới danh tính owner — owner có thể mất quyền sau khi
schedule đã ghi. Mất quyền thì ghi `error` kèm lý do và không gửi gì. Tiền lệ đã có: một
schedule mà mọi owner đều bị vô hiệu hoá thì ngừng chạy.

**Một chi phí thứ hai bị nói nhẹ.** State của *route* trong `egret-automation` có PDP
(`routes.rs:61`), nên nửa kiểm-lúc-lưu không cần thêm gì — nhưng `runner.rs` thì không có.
Kiểm lại lúc chạy đòi phải luồn một PDP vào runner. Một alert phạm vi workspace
(`dataapi_endpoint_id IS NULL`) đòi quyền đọc Data API cấp workspace.

### 5.8 Giao diện

Vẫn danh sách Alerts trong Explore, có badge phân biệt loại. Editor giữ các section lịch,
người nhận và nội dung thông báo; chỉ khác section nguồn — picker endpoint (hoặc "toàn
workspace"), metric, cửa sổ, toán tử và ngưỡng, dựng thành các hàng `ConfigTable` trên nền §4
đã dọn.

`grace_period_s` mặc định **14.400 giây (bốn giờ)**, kế thừa từ Superset qua `0110`. Con số đó
hợp lý cho alert nghiệp vụ và dài với alert vận hành; loại này có mặc định ngắn hơn của riêng
nó thay vì kế thừa im lặng.

---

## 6. Chứng minh từng khẳng định thế nào

Assertion phải qua kiểm chứng bằng đột biến trước khi được tin, và **một đột biến không biên
dịch được thì không chứng minh gì** — build về 0 lỗi, xác nhận điều đó, rồi mới đọc kết quả.

**Z2:**

- Bỏ filter `actor_user_id IS NULL` → một test phải đỏ.
- Đổi `BETWEEN 200 AND 299` trong biểu thức p95 → một test phải đỏ.
- Cửa sổ rỗng: `call_count` trả `0` và `error_rate` trả `noop` — hai test riêng biệt, vì một
  test xanh không nói gì về nhánh còn lại.
- `INSERT` một hàng `dataapi_alert` có mang `sql` → phải bị **database** từ chối, không phải
  bị Rust từ chối. Một assertion phía Rust vẫn xanh khi CHECK đã bị xoá.

**U và E:** chạy **toàn bộ** suite frontend trước mỗi commit chạm component dùng chung — một
file dùng chung bị đổi dưới một lần chạy test hẹp chính là cách một regression lọt vào `main`
ở phiên trước. Guard sẵn có của `ListPageShell` khẳng định **số lượng** `h1` đúng bằng một,
không chỉ là có tồn tại một cái; mỗi trang chuyển đổi kế thừa nghĩa vụ đó.

**Cả ba:** chạy trên kind và kiểm bằng trình duyệt trước khi gọi là xong. Trên codebase này
mọi defect UI đáng kể đều được tìm thấy khi chạy thật chứ không phải bằng review hay bằng bộ
test, và riêng layout thì không thể chốt bằng cách đọc mã (§10).

## 7. Phần tiếp theo, nêu tên để thứ tự nằm trên giấy tờ

`U → E → Z2 → Z1 → M → C → D → Z3`.

- **Z1 — danh sách IP allow/deny và endpoint public (không xác thực).** Một gói chứ không
  phải hai: IP allow-list chính là biện pháp bù cho endpoint public, và ship tách nhau nghĩa
  là ship một endpoint public trần trước. Endpoint public phá ba giả định của đường phục vụ —
  khoá rate limit thôi là client key mà thành IP, `access_log` không có actor, và grant invoke
  của Cedar bị bỏ qua hoàn toàn — nên nó cần một feature gate cấp workspace, không phải một
  checkbox. Dùng lại cơ chế trusted-proxy/XFF đã thiết lập chứ không đọc lại header.
- **M — nhiều câu SQL trong một endpoint, chỉ đọc.** Ca dùng chính của DBAPI cho việc này là
  phân trang: một câu lấy dòng, một câu lấy tổng. `EndpointSnapshot.sql: String` thành danh
  sách statement có tên, kéo theo thay đổi ở cột output đóng băng, envelope trả về, cache key
  và OpenAPI sinh ra. Đặt trước C và D để format snapshot chỉ đổi **một lần**.
- **C — rule validate tham số và biến đổi dữ liệu trả về.** DBAPI dùng script để validate vì
  nó không có mô hình tham số có kiểu; egret có, và cả hai ví dụ trong tài liệu DBAPI
  (`name.length <= 10`, regex email) đều biểu đạt được bằng ràng buộc khai báo trên
  `ParamSpec`. Nửa validate có thực sự cần một runtime script hay không là câu hỏi mở cho
  spec đó. Nửa biến đổi là engine Starlark đang tạm dừng, chỉ transform.
- **D — câu lệnh ghi và transaction.** Hai việc phải chốt trước khi viết dòng code nào. Một,
  **đo** xem transaction bắc qua nhiều câu lệnh có tồn tại trên Trino và trên StarRocks dưới
  mô hình kết nối của egret hay không; nếu không thì "nhiều statement có rollback" không phải
  lời hứa được phép đưa ra. Chính tài liệu DBAPI nói điều tương tự về HIVE. Hai, trả lời "ai
  là người ghi": trên connection delegated thì service account ghi bằng chính nó và Ranger
  áp dụng — chạy được; trên connection không delegated thì mọi client ghi bằng một danh tính
  chung, và điều đó phải bị **từ chối lúc publish** chứ không phải cảnh báo.
- **Z3 — import/export cấu hình endpoint giữa các môi trường.** Cuối cùng, vì format xuất
  *chính là* snapshot và cả C lẫn D đều thêm trường vào đó.

## 8. Hoãn, kèm lý do

- **HTTP proxy executor và Elasticsearch executor.** Bỏ sau khi trao đổi: egret phục vụ những
  database mà egret hỗ trợ. Cả hai còn đòi một mô hình datasource nằm ngoài `query.databases`
  (vốn gắn engine và được Ranger quản trị), một chỗ giữ credential, và một chính sách egress —
  một endpoint proxy một URL tuỳ ý là một máy quét mạng nội bộ được publish sau lớp xác thực
  của egret.
- **MCP server.** DBAPI phơi các API đã publish thành MCP tool qua Streamable HTTP trên một
  cổng riêng. Nó sẽ dùng lại OAuth2, Cedar và access log của egret không đổi. Không làm lúc này.
- **Canvas orchestration.** Ghép các endpoint bằng decision node là một workflow engine; nó
  thuộc về data pipeline chứ không thuộc lớp phục vụ.
- **Nội suy `${}` và chợ plugin bằng class Java.** Cả hai là cơ chế của DBAPI mà thứ tương
  đương ở egret hoặc đã có sẵn ở dạng an toàn hơn (§A.2), hoặc đã được khoanh vào engine Starlark.
- **Chuyển các trang list Explore ngoài bốn trang nêu ở đây**, và loại bỏ `ConfigSection` ở
  Control Plane. Không liên quan việc này.
- **Đọc trạng thái thùng rác/`disabled` của `dataapi.endpoints` từ runner**, để cách xử lý
  `noop` kèm lý do mà §5.6 dự định cho một endpoint bị xoá mềm hoặc disabled thực sự có thể
  xây được. Doc comment của `evaluate_and_deliver_dataapi_alert` (`runner.rs`) giải thích vì
  sao: trạng thái đó nằm trên một bảng thuộc `egret-dataapi`, và `egret-automation` cố tình
  không phụ thuộc nó ngoài leaf `egret-dataapi-api` (cùng ranh giới mà re-check
  `owner_can_read_dataapi_endpoint` của Task 4 đã tôn trọng). Hình dạng tự nhiên của nó đã
  được phác trong đó: một DTO `EndpointState { trashed: bool, enabled: bool }` cộng một port
  trait nhỏ, nối dây giống cách `pdp`/`query`/`renderer` đã có. Trong lúc chờ, phương án dự
  phòng không phải im lặng — xem ghi chú của §5.6 về điều thực sự xảy ra hiện nay.

---

## Phụ lục A — phép so sánh với DBAPI

Đọc từ tài liệu DBAPI Enterprise Edition (72 trang) đối chiếu với `crates/egret-dataapi` và
`crates/egret-dataapi-api` (31.339 dòng Rust kể cả test, migration `0119`–`0126`).

### A.1 Bảng đối chiếu tính năng

| Mảng | DBAPI EE | egret hôm nay |
|---|---|---|
| SQL executor | mọi database JDBC, driver nạp từ `extlib` | Trino / StarRocks qua engine của egret |
| Elasticsearch executor | DSL query thành API | không làm (§8) |
| HTTP proxy executor | chuyển tiếp sang service ngoài, pass-through hoặc mapping | không làm (§8) |
| Câu lệnh ghi | SELECT / INSERT / UPDATE / DELETE, stored procedure | chỉ đọc — gói D |
| Nhiều SQL, transaction | nhiều câu trong một API, bật tắt transaction | một câu — gói M, rồi D |
| Dynamic SQL | MyBatis `<if>`, `#{}` và `${}` | **đã có và an toàn hơn** — xem §A.2 |
| Tham số | form-urlencoded và JSON lồng nhau tuỳ ý | phẳng, có kiểu, `query` hoặc `body`, enum `allowed`, default, required |
| Validate tham số | rule bằng script kèm message riêng | chỉ kiểu và enum — gói C |
| Nhóm | API group mang quyền datasource | folders, bookmarks, trash |
| Vòng đời | save → offline → publish | draft → validate → preview → publish (snapshot đóng băng) → rollback → disable → trash / restore |
| Credential client | clientId/secret gọi `/token/generate`, token có hạn, tuỳ chọn mã hoá RSA response | **mạnh hơn** — OAuth2 client credentials trên machine IdP, grant invoke từng endpoint, Cedar |
| API public | có | không có — gói Z1 |
| Rate limit | QPS theo API và theo client | `per_endpoint_qps`, `per_key_qps`, burst |
| Cache | plugin do người dùng tự viết | dựng sẵn, scope `shared` / `principal`; không cắm thêm được |
| IP firewall | danh sách allow và deny | không có — gói Z1 |
| Giám sát | dashboard và call log; sink file, database hoặc Kafka; kho ClickHouse | Activity summary và call list trên `access_log` Postgres, partition theo tháng |
| Cảnh báo lỗi | plugin: email, SMS, DingTalk, Feishu, WeCom | **gói Z2** |
| Biến đổi dữ liệu | plugin theo từng SQL, plugin global, script Groovy inline | chỉ `output.shape` và `output.naming` — gói C |
| Hệ sinh thái plugin | năm loại plugin cộng một marketplace | engine Starlark, đang tạm dừng — gói C |
| Xuất tài liệu | export API doc | tải OpenAPI theo workspace |
| Import/export cấu hình | chuyển test → production | không có — gói Z3 |
| MCP | server riêng, bật tắt theo từng API | không làm (§8) |
| Triển khai | standalone và cluster | Kubernetes |

### A.2 Phép so sánh duy nhất bị đảo ngược sau khi kiểm

Một lần đọc trước của phép so sánh này đã ghi egret là **thiếu** dynamic SQL. Điều đó **sai**,
và việc sửa lại quan trọng vì nó gỡ hẳn một gói ra khỏi kế hoạch.

`crates/egret-dataapi/src/render.rs` (1.845 dòng) chạy minijinja với `set_formatter` thay thế
toàn bộ đường phát giá trị: `{{ x }}` ghi một `?` ra output và đẩy giá trị vào bind list.
**Không tồn tại đường đi từ `{{ … }}` sang SQL thô**, nên tác giả không thể viết một template
nội suy ngay cả khi cố ý. Hôm nay đã có:

```sql
WHERE 1=1 {% if region %} AND region = {{ region }} {% endif %}
          {% if regions %} AND region IN (
            {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %}) {% endif %}
ORDER BY {{ sort | ident }}
```

`{% for %}` nở ra N bind. `| ident` là chỗ hở cố ý duy nhất — định danh không bind được — và
nó phát ra **mục khớp trong danh sách `allowed` của chính tác giả**, không bao giờ là chuỗi
của caller.

`#{}` của DBAPI tương ứng với `{{ }}`. `${}` của DBAPI là nội suy thô, và egret cố ý không có
thứ tương đương: nó cùng loại lỗ hổng với vector backslash trên StarRocks đã vá ở `#504`, và
cổng chặn vượt template lúc publish là một phép kiểm **hành vi** (render hai lần, giá trị
opaque so với giá trị thường) chính vì năm lượt review độc lập đã tìm ra năm cách khác nhau
để vượt một bộ quét byte.
