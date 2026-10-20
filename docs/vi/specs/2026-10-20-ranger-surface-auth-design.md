# Xác thực bề mặt Ranger (`/service`) và chuyển nó về dưới `/ranger`

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-20-ranger-surface-auth-design.md` là bản chuẩn; hai bản
> lệch nhau thì theo bản tiếng Anh. Kế hoạch dài `docs/en/plans/2026-10-20-ranger-surface-auth.md` không được dịch.

Trạng thái: đã triển khai trên `feat/ranger-surface-auth-b` (Task 3-8, 6b, 7b của kế hoạch; Task 1-2 ở PR A), 2026-10-20. §3
và §4 được sửa sau khi triển khai để mô tả đúng cái đã làm; §8 liệt kê các giới hạn đã biết. Live check trên kind là một bước
riêng và không được ghi ở đây. Hướng dẫn vận hành: [`docs/vi/guides/ranger-surface.md`](../guides/ranger-surface.md). Phán
quyết 2026-10-20 (người dùng): bỏ 2.5.0, mức sàn là 2.8.0; `required` là mặc định; chưa có gì triển khai cho người dùng thật, nên
**không có cơ chế chuyển tiếp** (không alias `/service`, không giai đoạn deprecate, không ngoại lệ ẩn danh). Sửa cùng ngày:
**plugin 2.5.0 bị bỏ khỏi danh sách hỗ trợ (phán quyết của người dùng); mức sàn được hỗ trợ là 2.8.0.** Nguồn gốc: review mức sẵn
sàng production của DG.
Chương trình: Control Plane & DG (sau #4b Classification #702, độc lập với #5 Lineage).

## 1. Vấn đề

Route `/service/plugins/policies/download/{service}` là thứ mà Ranger plugin của mọi engine poll.
Hiện nó được phục vụ **ẩn danh theo mặc định**:

- `ranger.auth_token` rỗng ⇒ `require_bearer` cho mọi người qua
  (`crates/egret-policy/src/ranger/auth.rs:23`); bước kiểm config cho phép rỗng
  (`crates/egret-config/src/lib.rs:1763`).
- File plugin egret sinh ra **không mang credential**
  (`crates/egret-engine-mgmt-api/src/ranger_config.rs`, `security_xml`).
- `/service` được nest trên cùng server và cổng với `/api`
  (`crates/egret-server/src/lib.rs:289`), nên thứ gì với tới API cũng với tới nó.

Một lần tải trả mọi policy đang bật của một profile, nên caller chưa xác thực biết được tên user, tên group, tên role, tên tài
nguyên, mask và row filter. Chỉ đọc: các route ghi của `/service` đã bị bỏ trước đó, nên không thay đổi được gì qua nó.

Tên `/service` cũng là prefix riêng của Ranger Admin và dễ nhầm với Kubernetes service và với `/api`.

## 2. Số đo (2026-10-20)

Đầu dò: một HTTP server giả ghi lại dòng request và header `Authorization`, được điều khiển bởi
`org.apache.ranger.admin.client.RangerAdminRESTClient` thật từ mỗi jar `ranger-plugins-common`, cấu hình như cách
`security_xml` cấu hình plugin, thêm `ranger.plugin.<p>.policy.rest.client.username/.password`.
Mã nguồn đầu dò: đã commit ở Task 1 của kế hoạch thành `tests/ranger/src/test/java/EgretPluginAuthTest.java`; bản nháp
không nằm trong repo.

| `ranger-plugins-common` | Đi kèm | Không có credential | Có `…rest.client.username/password` |
|---|---|---|---|
| 2.5.0 | Trino 470 | `GET /service/plugins/policies/download/<svc>`, không header | **request y hệt, không header — các property bị bỏ qua** |
| 2.8.0 | Trino 483, StarRocks 4.1.4 | như trên, không header | `GET /service/plugins/secure/policies/download/<svc>` với `Authorization: Basic base64(user:pass)` |
| 2.9.0 | StarRocks 4.1.6 | không chạy | như 2.8.0 (prefix `starrocks` đo trên 2.8.0; 2.9.0 chỉ với prefix `trino`) |

Hệ quả, mỗi cái mâu thuẫn với kiểu nghĩ ngây thơ "cứ thêm token":

1. **Plugin nói Basic, không nói Bearer.** Bước kiểm Bearer hiện có của egret không bao giờ được một plugin gốc thoả mãn.
2. **Có credential thì plugin đổi ĐƯỜNG** sang `/service/plugins/secure/policies/download/…`.
   egret chỉ phục vụ đường không-`secure` (`routes.rs:18`), nên chuyển một engine 2.8+ sang dùng credential mà không có
   route mới thì sẽ 404, và engine sẽ lùi về cache của nó, rồi từ chối tất cả.
3. **Plugin 2.5.0 (Trino 470) hoàn toàn không xác thực được** qua kênh này. Client của nó có `setBasicAuthInfo` nhưng không
   gì nối nó từ config; các cơ chế khác trong 2.5.0 là Kerberos, chứng chỉ TLS phía client, hoặc một class
   `policy.source.impl` tuỳ biến. Không cái nào tương xứng ở đây.
4. Mặc định hiện là Trino 483 và StarRocks 4.1.6 (`docs/en/guides/engine-versions.md`), nên **chỉ engine mà operator pin vào
   Trino 470 bị ảnh hưởng** bởi điểm 3. **Phán quyết (người dùng, 2026-10-20): 2.5.0 / Trino 470 rời khỏi tập được hỗ trợ;
   mức sàn là 2.8.0.** Điều đó gỡ bỏ engine duy nhất không xác thực được, nên mọi engine được hỗ trợ đều dùng được Basic và
   không cần ngoại lệ ẩn danh (§3 A8, §4.6).

Đo sau đó ở Task 1 của kế hoạch (`../../en/plans/2026-10-20-ranger-surface-auth-measurements.md`, jar 2.8.0 với một admin giả;
2.9.0 gửi cùng request):
- plugin đang chạy **không** đọc lại security xml (Q1), nên đổi credential cần restart pod;
- 401 để plugin có policy trong bộ nhớ tiếp tục cưỡng chế chúng, plugin khởi động không có cache thì từ chối tất cả, và plugin
  khởi động có file cache trên đĩa hành xử như plugin ấm; nó thử lại mỗi chu kỳ poll (Q2). Chưa đo: điều tương tự trên pod
  engine thật, và thư mục cache (`/tmp/ranger/<svc>/policycache`, không mount volume) có sống qua một lần restart pod không;
  live check ghi lại cả hai;
- ~~các endpoint secure roles / tags / userstore mà plugin có thể gọi~~ **Đã đo 2026-10-20**
  (`../../en/plans/2026-10-20-ranger-surface-auth-measurements.md`): ở chế độ secure, plugin 2.8.0 còn poll
  `GET /service/roles/secure/download/<svc>` mỗi chu kỳ (không thấy lời gọi tags hay userstore). egret không phục vụ nó và
  plugin vẫn cưỡng chế khi nhận 404, nên phục vụ nó vẫn nằm ngoài phạm vi; hệ quả duy nhất là middleware xác thực phải phủ cả
  các route không khớp dưới `/ranger` (credential sai ⇒ 401, không phải 404). Q1 (không đọc lại XML) và Q2 (cache ấm sống qua
  401, cache lạnh từ chối tất cả) khớp với điều bản nháp của spec này đã giả định.
  **Đính chính 2026-10-21 (Q4):** "không thấy lời gọi tags" chỉ đúng vì service-def của fixture ở Q3 không khai báo tag enricher
  (không có `contextEnrichers`, không có `tagPolicies`); `trino.json` của egret khai báo `RangerTagEnricher` với
  `RangerAdminTagRetriever`, poll qua cùng admin client. Gọi thẳng client, 2.8.0 và 2.9.0 có credential lấy tags từ `GET /service/tags/secure/download/<svc>` (đường thường khi không có
  credential), nên egret phục vụ cả đường đó (A4). Userstore đổi sang `/service/xusers/secure/download/<svc>`; egret không
  phục vụ biến thể userstore nào và plugin chỉ gọi nó khi cấu hình làm giàu user-store, nên vẫn ngoài phạm vi (A13).

## 3. Quyết định

| # | Quyết định |
|---|---|
| A1 | Bề mặt Ranger chuyển về **`/ranger`**, giữ đường con riêng của Ranger: `/ranger/service/plugins/...`. Plugin tự nối `/service/...` vào `policy.rest.url`, nên URL của plugin thành `<egret>/ranger` và **plugin không phải đổi gì**. |
| A2 | **`/service` bị bỏ hẳn**, không alias. Không engine nào ngoài thực tế phụ thuộc vào nó, nên không có chuyển tiếp: URL duy nhất là `<egret>/ranger`. |
| A3 | Engine **xác thực bằng HTTP Basic**, mỗi engine một credential: username `engine-<engine_id>`, password là một secret ngẫu nhiên được lưu (cùng kiểu với `egret-internal-{id}` / `egret-monitor-{id}`, có thao tác Rotate). Như đã làm: 32 byte ngẫu nhiên base64 URL-safe (43 ký tự, rút lại khi chứa `--`), lưu ở scope toàn cục của secret store với tên `engine-ranger-{engine_id}-{random}`; `POST /api/control-plane/engines/{id}/rotate-ranger-credential` (`engine:update`) rotate nó. |
| A4 | egret phục vụ **cả** `/ranger/service/plugins/policies/download/{svc}` lẫn `/ranger/service/plugins/secure/policies/download/{svc}` bằng một handler, và **cả** `/ranger/service/tags/download/{svc}` lẫn `/ranger/service/tags/secure/download/{svc}` bằng một handler (Q4: plugin có credential lấy tags từ đường secure). |
| A5 | **Phân quyền, không chỉ xác thực:** credential engine **chỉ** được tải **profile mà engine của nó được gắn** (và tag service profile đó nêu, thứ handler vốn đã nhúng vào). Engine bị chiếm quyền không được đọc policy của profile khác. Không khớp ⇒ 403, không phải 404, và được ghi log; engine không gắn profile nào nhận 403. Tên profile không tồn tại là 404 trước khi kiểm việc gắn, nên engine đã xác thực biết được một tên có tồn tại hay không (chấp nhận). |
| A6 | Bearer `ranger.auth_token` hiện có **được giữ** cho công cụ của operator và harness (`tests/ranger/README.md` dùng nó). Các scheme được nhận: Bearer(token) **hoặc** Basic(engine). Token rỗng không nhận bearer nào. Các route chỉ-đọc của operator (`/plugins/definitions…`, `/plugins/services`, `/plugins/policies/{id}`) từ chối credential engine (403). |
| A7 | Config mới `ranger.auth_mode` (env `EGRET_RANGER__AUTH_MODE`, Helm `ranger.authMode`): `required` (**mặc định**) \| `anonymous`; giá trị khác làm việc nạp config thất bại. Ở `required`, request không có credential hợp lệ nhận 401 kèm `WWW-Authenticate: Basic realm="egret-ranger"` và một body cho mọi nguyên nhân. `anonymous` là lựa chọn rút lui tường minh chỉ cho stack cục bộ dùng-rồi-bỏ: request không có credential hoặc có credential sai được phục vụ như ẩn danh (mọi profile, và các route của operator), trong khi credential engine hợp lệ vẫn bị giới hạn ở profile của nó. Nó ghi một `warn!` lúc khởi động nêu rủi ro lộ dữ liệu; nó không ghi log từng request ẩn danh. |
| A8 | **Đã quyết (người dùng): plugin 2.5.0 / Trino 470 không còn được hỗ trợ; mức sàn là 2.8.0.** Không listener nội bộ, không ngoại lệ ẩn danh. Chi tiết và danh mục thay đổi ở §4.6. |
| A9 | Credential vào một **Kubernetes Secret**, không bao giờ vào ConfigMap, và được mount vào đường config của plugin. Security XML được chuyển khỏi các ConfigMap nó từng nằm (Trino `{name}-config`, StarRocks `{name}-ranger-cm`) sang một Secret cho mỗi credential (A10); Trino mount nó bằng `subPath` tại `/etc/trino/ranger-trino-security.xml` ở coordinator và worker, StarRocks qua `starRocksFeSpec.secrets` trong thư mục conf của FE. Audit XML của StarRocks vẫn ở `{name}-ranger-cm`. |
| A10 | **Rotate** theo quy tắc thường trực *không bao giờ xoá một giá trị secret mà cluster có thể còn giữ*. Mỗi dòng engine giữ một credential **hiện tại** và một credential **trước đó** (`ranger_secret_id`, `ranger_secret_prev_id`); cả hai đều xác thực được. Rotate commit credential hiện tại mới và hạ credential cũ xuống **trước** lần apply; với engine **được provision**, credential trước đó chỉ bị retire **khi rollout của credential hiện tại được quan sát là hoàn tất**, không bao giờ chỉ vì Kubernetes nhận lần apply: mỗi lần apply render security XML của plugin vào một Secret **riêng của credential đó**, `{name}-rc-{tag}`, tag là 6 ký tự chữ-số đầu tiên của id (không bí mật) của credential và được giữ khác nhau giữa credential hiện tại và trước đó của một engine (lần mint trùng tag thì rút lại), Secret mà pod template mount, và đóng dấu id (`egret/ranger-credential`) lên các pod template đó và lên Secret — nên pod dựng từ một template nêu một credential chỉ có thể mount nội dung của chính credential đó (mount `subPath` giữ thứ pod thấy lúc khởi động; với một Secret dùng chung, các lần apply xen kẽ có thể để pod của template mới giữ nội dung cũ). Rollout chỉ hoàn tất khi Secret của credential đó tồn tại và mọi workload như vậy (Trino: Deployment coordinator và worker; StarRocks: StatefulSet FE, CN không chạy plugin) mang id đó trên pod template, `observedGeneration == generation`, mọi replica đã cập nhật và sẵn sàng, không còn gì của template cũ (Deployment: `updated == replicas == ready == available == spec.replicas`, không gì unavailable hay terminating; StatefulSet: `currentRevision == updateRevision`, `updated == replicas == ready == spec.replicas`), và mọi pod đang chạy của các vai trò đó mang id, Ready và không terminating. Thứ gì vắng mặt, không rõ, dở dang, bị scale về 0 hoặc đọc lỗi đều KHÔNG hoàn tất và giữ credential trước đó. Khi đó, và chỉ khi đó, credential trước đó bị xoá theo đúng id trong một compare-and-set trên cả hai cột (hiện tại = id mà rollout được quan sát cho nó VÀ trước đó = id đã đọc), nên rollout quan sát cho một credential cũ hơn không bao giờ retire credential trước đó của một lần rotate mới hơn; Kubernetes Secret `{name}-rc-{tag của id trước đó}` của nó bị xoá theo đúng tên sau lần retire đó, cùng với `{name}-ranger-secret` từ trước khi có Secret theo credential (không pod nào mount cái nào khi rollout đã hoàn tất). Xoá engine và gỡ engine khỏi profile thì xoá mọi Ranger Secret của engine đó, chọn theo label (`egret/engine={name}`, `egret/component=ranger-credential`), không bao giờ theo prefix tên. Tag ngắn vì operator StarRocks (`--volume-name-with-hash=true`) đặt tên mỗi volume của pod FE là `<tên object>-<4 ký tự>` và tên volume là một DNS label 63 ký tự: vì vậy tên engine StarRocks bị giới hạn 48 ký tự (quá thì 400, khi tạo và ở mọi lần apply của engine StarRocks đã gắn profile). Secret của credential bị retire chỉ bị xoá khi annotation của nó vẫn nêu credential đó, với điều kiện tiên quyết `resourceVersion`. Ngay trước mỗi lần apply egret đọc lại dòng và từ chối (409) request mà credential được render không còn là credential HIỆN TẠI (một lần build chỉ render credential hiện tại); việc này thu hẹp, nhưng khi chưa có khoá theo engine thì không đóng hẳn, cuộc đua giữa các lần apply đồng thời. Một lượt quét định kỳ (mỗi 30 s, trên mọi replica) và mỗi lần đọc engine thực hiện bước kiểm này. Với engine **external** (không lần apply nào chứng minh được operator đã cài lại), chỉ khi operator xác nhận tường minh credential NÀO đã được cài: `POST /engines/{id}/retire-previous-ranger-credential` với `{ "installed": "<credentialId>" }`, id mờ được trả kèm credential lúc đăng ký hay rotate. Retire là một compare-and-set trên cả hai cột (hiện tại = `installed` VÀ trước đó = id đọc từ dòng), nên một xác nhận cũ hay trùng lặp gửi trước một lần rotate sau đó là 409 và không xoá gì; với `installed` khớp và không có gì pending thì là 204 idempotent. Không bao giờ quét theo prefix, trừ lúc xoá engine. Rotate bị từ chối (409) khi credential trước đó còn đang chờ retire, với cả hai loại engine, nên rotate không bao giờ xoá credential; retire theo rollout quan sát được (provisioned) và retire do operator xác nhận (external) là hai lần xoá duy nhất ngoài lúc xoá engine. Khi credential trước đó của một engine provisioned còn pending, `rotate-ranger-credential` trả 409 nói rằng nó đang chờ rollout hoàn tất; 204 của một lần rotate chỉ có nghĩa Kubernetes đã nhận lần apply, và credential trước đó còn hiệu lực cho tới khi rollout hoàn tất. Plugin chỉ đọc XML lúc khởi động (đã đo, §2), nên rotate là một lần restart cuốn chiếu các pod của engine. Workload chạy plugin bị scale về 0 không bao giờ được quan sát là đã roll xong, nên nó giữ credential trước đó, và từ chối rotate, cho tới khi scale lên (§8). Route rotate trả 400 cho engine provisioned không gắn profile nào hoặc khi Kubernetes chưa được cấu hình, và 502 khi cluster từ chối lần apply (khi đó cả hai credential vẫn còn hiệu lực). |
| A11 | **Engine external** (đăng ký, do operator chạy, SP4d-4 Phần C) nhận cùng loại credential, hiện một lần lúc đăng ký/rotate. Operator của nó dán credential vào config plugin của mình. Credential được mint trước transaction đăng ký và được ghi bởi cùng câu INSERT, và bị huỷ nếu transaction đó thất bại. 201 của đăng ký mang engine cùng `rangerCredential: { username, password, credentialId }`, và 200 của rotate external mang `{ username, password, credentialId }`, cả hai kèm `Cache-Control: no-store`; không response nào khác mang password. `EngineDto` (mọi lần đọc engine) mang `rangerCredentialId` (id của credential hiện tại, nên ai có `engine:read` cũng thấy; nó không cấp gì) và `rangerCredentialPending`. Giao diện hiện password một lần trong một dialog không đóng được cho tới khi operator xác nhận đã lưu, và giữ `credentialId` của lần rotate trong state của trang làm token `installed` cho lần retire. Trang plugin-config render các file với username của engine và placeholder `REPLACE_WITH_THE_ENGINE_PASSWORD`, và báo `authMode` là `engine-credential` hoặc `anonymous` (giá trị cũ `bearer-configured` không còn: thay đổi phá tương thích cho client API); engine đăng ký trước khi có credential được bảo rotate để lấy một cái. Compare-and-set thua ở một lần rotate external là 409 (không bao giờ trả password của bên thắng). Xoá engine thì xoá các credential của nó (theo đúng id, rồi theo prefix `engine-ranger-{id}-`). |
| A12 | Xác thực thất bại được **ghi log và đếm** theo engine trong một bảng riêng, `policy.ranger_auth_failures (engine_id, count, last_at, ok_at)` (migration 0201), câu upsert của nó select từ `engine_mgmt.engines`, nên chỉ engine tồn tại mới có dòng và caller không làm phình bảng bằng cách bịa id; một trigger lúc xoá engine xoá dòng đó. Chỉ credential Basic bị từ chối có username phân tích được thành `engine-<uuid>` mới được đếm (cả ở chế độ `anonymous`); không có header, username khác hay bearer sai thì không có engine để đếm vào. **Thành công tính theo engine, không join sổ cái:** một lần tải được cho qua với credential engine đặt `ok_at` của engine đó khi nó có dòng. `ranger_plugin_downloads` không dùng được ở đây, vì nó được khoá theo profile và plugin id, nên một engine khoẻ trên cùng profile sẽ che engine đang lỗi; nó cũng không ghi được một lần poll thất bại, vốn không có service đã xác minh. **Gộp:** việc ghi nằm ngoài đường request (không bao giờ đổi hay làm chậm 401), theo từng process, tối đa một upsert lỗi và một `warn!` cho mỗi engine mỗi 10 s (những lần bị từ chối còn bị giữ khi một đợt dồn dập dừng thì không được ghi, và lần ghi bị bỏ khi đã có 8 lần ghi đang chạy, nên `count` là cận dưới), và tối đa một lần ghi `ok_at` cho mỗi engine mỗi 60 s; tối đa 4096 id được theo dõi mỗi cửa sổ. Overview (Governance, critical, cần `engine:read`) hiện `engine_auth_failing` cho mỗi engine có `last_at` mới hơn `ok_at` của nó (hoặc chưa có) **và chưa quá 24 h**, nên engine đã thôi trình credential sai sẽ rơi khỏi danh sách. Engine không cấu hình credential nào không được mục này nêu tên; engine provisioned như vậy hiện ở các mục drift có sẵn (`plugin_quiet` sau 5 phút, `plugin_silent` sau 24 h) khi không plugin nào khác poll profile của nó, còn engine external thì không được đánh giá. |
| A13 | Ngoài phạm vi: mTLS, Kerberos, JWT (có trong `RangerRESTClient` của nhánh master nhưng chưa đo trong các jar được phát hành), credential theo profile dùng chung cho nhiều engine, phục vụ các endpoint roles/userstore (tags được phục vụ, A4). |

## 4. Thiết kế

### 4.1 Định tuyến (`egret-server`)
```text
/api/...                      không đổi
/ranger/service/...           ranger_router(state)         ← chuẩn mới
/service/...                  đã bỏ (404)
```
`ranger_router` có thêm `/plugins/secure/policies/download/{service}` cạnh route hiện có, cùng handler.

### 4.2 Middleware xác thực (`egret-policy/src/ranger/auth.rs`)
`require_bearer` thành `require_surface_auth`, sinh ra một extension `Caller`:
`Operator` (Bearer khớp), `Engine(EngineIdentity)` (Basic được verifier được inject so với secret hiện tại và trước đó đã lưu
của engine, so sánh thời gian hằng, luôn so cả hai), hoặc `Anonymous` (chỉ ở chế độ anonymous). Layer bọc mọi đường dưới
`/ranger/service/`, kể cả một route bắt-tất-cả trả 404 cho caller đã xác thực, nên một đường không được phục vụ (lần poll roles
của plugin) là 401 khi không có credential. Các handler tải phân giải `{service}` được xin và kiểm caller `Engine` có được gắn
với nó hay với tag service mà profile của nó nêu không (A5); `require_operator` từ chối engine trên các route của operator.
Chế độ `anonymous` phục vụ caller không có credential hợp lệ, kèm một `warn!` lúc khởi động nêu rủi ro lộ dữ liệu.

### 4.3 Lưu trữ
Secret nằm trong secret store hiện có, tên `engine-ranger-{engine_id}-{random}` (`{random}`: một UUID v4 mới dạng simple cho mỗi credential, nên một lần rotate không bao giờ trùng tên với credential nó hạ xuống). Dòng engine tham chiếu chúng bằng hai cột
mới (`ranger_secret_id`, `ranger_secret_prev_id`). Cặp `internal_secret_id`/`monitor_secret_id` hiện có **chỉ dành cho Trino**
(`rotate_engine_secrets` từ chối StarRocks), nên không dùng lại được. Engine external có thể mang các cột này (CHECK của migration
0184 không nêu chúng).
Migration: `0200_engine_ranger_credential.sql` (hai cột, cùng một CHECK rằng credential trước đó chỉ tồn tại cạnh một credential
hiện tại và khác nó) và `0201_ranger_auth_failures.sql` (A12).

### 4.4 Provisioner (`egret-engine-mgmt-api/src/ranger_config.rs`)
`security_xml` nhận URL của egret và render `policy.rest.url` thành `<url>/ranger`. Khi binding mang credential, nó thêm
`ranger.plugin.<trino|starrocks>.policy.rest.client.username/password`, không escape; credential chỉ dựng được qua một
constructor có kiểm, từ chối `<`, `&` và `--`, và password được mint sẽ được rút lại cho tới khi không chứa `--`. Mọi lần apply
của engine provisioned đã gắn profile đều mang một credential (nó được đảm bảo ở mỗi lần build, bất kể `auth_mode` là gì), nên
lần apply đầu tiên sau nâng cấp mint một credential và roll pod. Kết quả vào Secret theo credential của A9/A10.

### 4.5 Triển khai
Một lần cắt, không chia pha: route, middleware, URL của provisioner và credential được phát hành cùng nhau với
`auth_mode = required`. Engine nào được provision trước thay đổi này (chỉ máy của developer) được apply lại một lần (một lần
update hoặc một lần rotate credential; restart không render lại), hoặc xoá đi tạo lại. Engine dev cũ không hiện ở
`engine_auth_failing`: nó không trình credential (hoặc poll đường `/service` đã bỏ, vốn chỉ là 404 thường), nên không có gì được
đếm cho nó (A12). Nó hiện ở các mục drift (`plugin_quiet`, rồi `plugin_silent`) khi không plugin nào khác poll profile của nó.

### 4.6 Bỏ plugin 2.5.0 (Trino 470)
Các cặp (loại, plugin) được hỗ trợ trở thành **Trino 2.8.0, StarRocks 2.8.0, StarRocks 2.9.0**. Cụ thể:

| Ở đâu | Thay đổi |
|---|---|
| `crates/egret-perm/src/actions.rs:84` (`("trino","2.5.0")`) và doc ở `:107` | bỏ cặp này khỏi danh sách được mô hình hoá |
| `crates/egret-perm/src/engine_images.rs:22,198` | bỏ ánh xạ `trinodb/trino:470` → 2.5.0; test ở `:233`, `:311` chuyển sang 483 |
| `crates/egret-engine-mgmt/src/external.rs:55` | engine external khai báo phiên bản dưới 2.8.0 bị **từ chối lúc đăng ký** (400), không chỉ là "không được mô hình hoá", vì nó không bao giờ giữ được credential |
| `crates/egret-engine-mgmt/tests/no_floating_images.rs`, `src/engines.rs:7279` | fixture dùng 483 |
| `crates/egret-engine-mgmt/tests/routes.rs` (khoảng 25 chỗ đăng ký một Trino external là `"2.5.0"`) và `registry_schema_constraints.rs` | phiên bản fixture thành `"2.8.0"`; test gần dòng 11300 khẳng định "2.5.0 modelled for Trino only" được viết lại, không xoá: 2.5.0 giờ phải bị **từ chối** |
| `tests/ranger/pom.xml` mặc định `<ranger.version>` | `2.5.0` → `2.8.0`; mục README "Why 2.5.0 is the default" được thay |
| `crates/egret-policy/tests/ranger_engine_gate.rs:40`, `justfile` (dòng 147–201), differential của `egret-ranger-eval`, `crates/egret-perm-compile/tests/gate_timing.rs` | danh sách phiên bản thành `2.8.0` và `2.9.0`. Việc này **bỏ nửa 2.5.0 của corpus differential** (155.880 case chạy trên cả hai): CI ngắn hơn, và "0 disagreements" sau đó chỉ nói cho 2.8.0 và 2.9.0 |
| `docs/en/guides/engine-versions.md` | bỏ dòng Trino 470; thêm ghi chú "dropped in <release>" và cách nâng cấp |
| Văn bản lịch sử (comment migration 0179/0190, các plan và spec cũ, `ambiguous_arbitration.rs:118`) | **giữ nguyên**: lịch sử chỉ-thêm, và nhiều chỗ mô tả số đo của jar đó vẫn còn đúng |

Engine trên Trino 470 / 2.5.0 (chỉ máy của developer, theo người dùng): **bị vô hiệu, không migrate.**
- Tạo hay cập nhật engine sang image có plugin dưới 2.8.0 bị từ chối (400), và đăng ký external khai báo phiên bản như vậy cũng bị
  từ chối.
- Dòng có sẵn vẫn đọc được và bị báo là không được hỗ trợ kèm cách nâng cấp (`PUT /engines/{id}` sang Trino 483, hoặc xoá nó); nó
  xác thực thất bại (nó không gửi credential, nên `engine_auth_failing` không nêu tên nó; các mục drift nêu khi không plugin nào
  khác poll profile của nó, A12). Không migrate dữ liệu, không đường tương thích.

Không làm ở đây (follow-up tuỳ chọn): xoá các nhánh riêng cho 2.5.0 trong `egret-ranger-eval` và compiler, vốn chỉ phản chiếu jar
đó. Chúng đã chết với engine được hỗ trợ nhưng vô hại; xoá chúng là một thay đổi riêng, rủi ro hơn, và không nên đi cùng một thay
đổi về xác thực.

## 5. Kế hoạch kiểm chứng
- **Task 1 (đo):** commit đầu dò thành một test có trong danh sách surefire (danh sách include là allow-list, xem `pom.xml` và
  `EgretSurefireIncludesTest`), chạy trên 2.8.0 và 2.9.0 với CẢ HAI prefix plugin (`trino`, `starrocks`); thêm ba mục chưa đo
  ở §2. Giữ kết quả 2.5.0 ở §2 làm lý do cho mức sàn, không làm case CI.
- **Task 2 (bỏ 2.5.0)** vào **trước** phần xác thực, thành PR riêng, để "xanh hết trên 2.8.0+" được xác lập trước.
- Test Rust: Basic đúng / sai password / sai profile (403) / Bearer đúng / ẩn danh ở mỗi chế độ / secret cũ-và-mới trong lúc
  rotate / đường secure và không secure trả body giống hệt / `/service` là 404 / chế độ `anonymous` cảnh báo / image 2.5.0 bị từ
  chối lúc tạo, cập nhật và đăng ký.
- **Live check** (còn chờ ở thời điểm sửa §3/§4; ghi trong tài liệu live-result riêng) trên kind với Trino 483 và StarRocks 4.1.6
  thật: poll thành công với credential, thất bại với credential sai, cache sống qua một lần egret sập, restart sau rotate nhận
  secret mới. Theo chính ghi chép của repo, xanh ở jsdom/unit không phải bằng chứng ở đây.
- Mutation: cho bước kiểm việc gắn profile luôn đúng và xác nhận có test thất bại.

## 6. Rủi ro
- Engine cũ (trước thay đổi, sai URL hoặc không có credential) mất cập nhật policy và, khi lạnh, từ chối tất cả. Chấp nhận: chỉ máy
  của developer có engine như vậy. Overview chỉ nêu tên chúng qua các mục drift (`plugin_quiet`, `plugin_silent`), và chỉ khi không
  plugin nào khác poll profile đó; `engine_auth_failing` chỉ nêu engine trình credential sai (A12).
- Basic qua HTTP thường làm lộ credential trên đường truyền; ghi rõ rằng `required` giả định có TLS hoặc đường đi trong cluster.
- Chuyển prefix làm hỏng mọi thứ curl `/service/...`: lệnh fixture trong `tests/ranger/README.md` và mọi script dev phải chuyển
  sang `/ranger/service/...` trong cùng PR.

## 7. Câu hỏi mở
Người dùng đã chốt ngày 2026-10-20: mặc định `required` (A7); bỏ 2.5.0 (A8); không alias và không giai đoạn deprecate (A2, §4.5, §4.6).
Bản tiếng Việt: tài liệu này (cùng hướng dẫn vận hành `docs/vi/guides/ranger-surface.md`); kế hoạch dài không được dịch.
Còn mở với người dùng: quy tắc zero-replica (§8), một route huỷ credential đang pending (§8), và có cập nhật `egret-data/docs`
trong PR này hay không.

## 8. Giới hạn đã biết / follow-up
- **Không có khoá apply theo engine.** Lần đọc lại ngay trước một lần apply từ chối (409) request render một credential không còn
  là hiện tại, nhưng một lần rotate và lần retire của nó vẫn có thể chen vào giữa lần đọc lại đó và lần apply lên Kubernetes; khi
  đó một lần apply cũ có thể đặt lại credential đã retire lên cluster, và engine nhận 401 cho tới lần apply kế tiếp. Đóng hẳn cần
  tuần tự hoá apply theo engine.
- **Workload zero-replica.** Workload chạy plugin có `replicas: 0` (engine Trino không có worker, KEDA `minReplicaCount: 0`) không
  bao giờ được quan sát là đã roll xong, nên credential trước đó còn hiệu lực và rotate vẫn bị từ chối cho tới khi nó scale lên.
  Một quy tắc rollout-rỗng chứng minh được là an toàn đã được đề xuất và đang chờ người dùng quyết; tới lúc đó việc này là
  fail-closed.
- **Không có route huỷ credential pending.** Operator external làm mất password mới trước khi cài chỉ có thể retire (xoá credential
  engine còn đang chạy) rồi rotate lại; engine nhận 401 ở khoảng giữa.
- **Bộ gộp của A12 không có negative cache.** Một đợt dồn username `engine-<uuid>` ngẫu nhiên có thể lấp đầy map 4096 mục, và khi
  đó lần bị từ chối đầu tiên của một engine thật không được ghi cho tới khi cửa sổ qua (10 s).
- **`credentialId` thấy được với `engine:read`.** Id của credential hiện tại nằm trên `EngineDto`. Tự nó không cấp gì (retire cần
  `engine:update`, và giao diện không bao giờ tự gửi nó), nên phần lộ ra là vô hại, nhưng rộng hơn cần thiết.
- **`engine_auth_failing` có thể bị làm chập chờn** bởi một caller không có credential nhưng biết id của engine: password sai cho
  id đó bật mục lên, và lần tải thật kế tiếp của engine xoá nó.
- **Plugin ấm và lạnh.** Plugin tiếp tục cưỡng chế policy đã cache qua các lần 401, nhưng khởi động lạnh không có cache thì từ chối
  tất cả (§2); thư mục cache có sống qua restart pod hay không là việc của live check.
- **Credential hiện tại hỏng + credential trước đó pending (chỉ database phát triển).** Dòng có credential hiện tại không render
  được (mint trước khi renderer có bước kiểm) và credential trước đó đang pending thì từ chối mọi lần apply và mọi lần rotate; gỡ
  bằng tay (module doc của `ranger_credential.rs`).
- **Bỏ gắn profile xoá Ranger Secret ngay lập tức.** Một lần apply không có policy profile xoá mọi Ranger Secret của engine
  (`{name}-rc-{tag}` và `{name}-ranger-secret` cũ) ngay, trong khi pod của template cũ (đã gắn profile) có thể vẫn chạy cho tới
  khi rollout thay chúng. Chúng vẫn chạy, nhưng một `rollout undo` về template cũ sẽ không mount được Secret của nó (hãy gắn lại
  profile). Giá trị credential trong secret store không bị xoá khi bỏ gắn.
- **Docker provisioner.** Engine Docker (chỉ Trino) nhận XML mang credential dưới dạng file `0600` trong mỗi container, và mỗi
  container mang id của credential ở label `egret/ranger-credential`. Rollout của nó hoàn tất khi mọi container của engine mang id
  hiện tại, đang chạy và không unhealthy; nó không có object nào theo credential, nên lần retire không xoá gì ngoài secret store.
