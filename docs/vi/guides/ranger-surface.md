# Bề mặt Ranger: URL, xác thực và credential của engine

> Bản tham khảo tiếng Việt. Bản gốc (canonical) là [docs/en/guides/ranger-surface.md](../../en/guides/ranger-surface.md); khi hai bản khác nhau, bản tiếng Anh thắng. Tên route, khoá config, thông báo lỗi và nhãn giao diện giữ nguyên tiếng Anh.

egret là Ranger Admin của các engine của nó; không có Ranger server riêng. Mỗi engine Trino và StarRocks chạy một Ranger plugin gốc, và plugin đó poll egret để lấy policy của profile nó được gắn. Tài liệu này nói về URL plugin poll, ai được gọi nó, credential riêng của từng engine mà plugin trình ra, và cách rotate, theo dõi và nâng cấp lên credential đó.

Thiết kế: [`docs/en/specs/2026-10-20-ranger-surface-auth-design.md`](../../en/specs/2026-10-20-ranger-surface-auth-design.md) (bản tiếng Việt: [`docs/vi/specs/2026-10-20-ranger-surface-auth-design.md`](../specs/2026-10-20-ranger-surface-auth-design.md)). Số đo trên jar plugin thật: [`docs/en/plans/2026-10-20-ranger-surface-auth-measurements.md`](../../en/plans/2026-10-20-ranger-surface-auth-measurements.md). Image engine và phiên bản plugin: [engine-versions.md](engine-versions.md).

## Bố cục URL

Bề mặt được phục vụ dưới `/ranger`, cùng server và cổng với `/api`. `policy.rest.url` của plugin là `<egret>/ranger`. Plugin tự nối thêm đường `/service/...` của Ranger, nên plugin không phải đổi gì.

| Đường | Ai gọi | Được phục vụ |
|---|---|---|
| `GET /ranger/service/plugins/policies/download/{profile}` | plugin cấu hình không có username và password | có |
| `GET /ranger/service/plugins/secure/policies/download/{profile}` | plugin 2.8.0+ cấu hình có username và password (đường mà engine của egret gọi) | có, cùng handler và cùng body với đường thường |
| `GET /ranger/service/tags/download/{profile}` | plugin Trino, cho tag service của profile | có |
| `GET /ranger/service/plugins/definitions`, `…/definitions/name/{name}`, `…/services`, `…/policies/{id}` | chỉ công cụ của operator | có, chỉ với bearer của operator |
| `GET /ranger/service/roles/secure/download/{profile}` | plugin 2.8.0+ ở chế độ secure, mỗi chu kỳ poll | **không**: 404 với caller đã xác thực, 401 với mọi caller khác |
| `/service/...` ở gốc server | URL cũ | **đã bỏ**: 404, không có alias |

Plugin chịu được 404 ở route roles và vẫn cưỡng chế (đã đo với jar 2.8.0, tài liệu số đo, Q3). egret không phục vụ Ranger roles.

Khi Ranger tắt (`[ranger] enabled = false`, Helm `ranger.enabled`) thì không có gì dưới `/ranger`.

## Ai được gọi

Mọi request dưới `/ranger/service/` đi qua một bước xác thực, dù egret có phục vụ đường đó hay không.

| Caller | Header | Được đọc |
|---|---|---|
| Engine | `Authorization: Basic base64(engine-<engine uuid>:<password>)` | profile mà engine của nó được gắn, và tag service mà profile đó nêu. Mọi thứ khác là 403 và được ghi log. Engine không gắn profile nào nhận 403 ở mọi nơi. Các route của operator trả 403. |
| Operator | `Authorization: Bearer <ranger.auth_token>` | mọi thứ. `auth_token` rỗng (mặc định) không nhận bearer nào. |
| Ẩn danh | không có | chỉ ở chế độ `anonymous`, xem bên dưới |

Token của operator là khoá config `[ranger] auth_token`, env `EGRET_RANGER__AUTH_TOKEN`. Helm chart render `EGRET_RANGER__AUTH_TOKEN` thành chuỗi rỗng, nên bản cài Helm không nhận bearer của operator trừ khi bạn ghi đè biến đó.

Tải một profile không tồn tại là 404 với mọi caller đã qua bước xác thực.

## Chế độ xác thực

| Thiết lập | Ở đâu |
|---|---|
| `[ranger] auth_mode = "required"` hoặc `"anonymous"` | file config của egret |
| `EGRET_RANGER__AUTH_MODE=required` hoặc `anonymous` | biến môi trường |
| `ranger.authMode: required` hoặc `anonymous` | Helm values (render ra `EGRET_RANGER__AUTH_MODE`) |

- **`required` (mặc định).** Request không có credential engine hợp lệ hay bearer operator hợp lệ nhận 401 kèm `WWW-Authenticate: Basic realm="egret-ranger"`. Mọi nguyên nhân (không có header, header sai dạng, engine không tồn tại, sai password, sai bearer) nhận cùng một body, nên lời từ chối không nói phần nào sai.
- **`anonymous`.** Request không có credential, hoặc có credential sai, được phục vụ như ẩn danh và đọc được mọi profile lẫn các route của operator. Credential engine hợp lệ vẫn bị giới hạn ở profile của nó, nên engine trình credential nhận 403 ở profile khác trong khi caller không có credential thì không. egret ghi một `warn!` lúc khởi động. Nó không ghi log cho từng request ẩn danh.

Giá trị khác là lỗi cấu hình, và egret không khởi động.

> **Cảnh báo.** Ở chế độ `anonymous`, ai với tới được URL cũng tải được policy của mọi profile: tên user, group và role, tên tài nguyên, mask và row filter. Chỉ dùng cho stack cục bộ dùng-rồi-bỏ, không bao giờ cho một egret mà mạng ngoài tầm kiểm soát của bạn với tới được.

Credential của engine đi bằng HTTP Basic. Qua HTTP thường, ai trên đường truyền cũng đọc được. `required` giả định có TLS, hoặc đường đi nằm trong cluster.

## Credential của engine

Mỗi engine có credential riêng:

- **Username:** `engine-<engine uuid>`.
- **Password:** 32 byte ngẫu nhiên, base64 URL-safe (43 ký tự). Password không bao giờ chứa `<`, `&` hay `--`, nên được đưa vào XML của plugin mà không cần escape.
- **Lưu trữ:** một secret trong secret store của egret (scope toàn cục), tên `engine-ranger-{engine uuid}-{random}`. Dòng engine nêu một credential **hiện tại** và tối đa một credential **trước đó**. Cả hai đều xác thực được.

### Engine được provision

egret tạo credential ở lần apply đầu tiên của một engine đã gắn với một policy profile. Mỗi lần apply đều kiểm tra engine đã có credential chưa. API không bao giờ trả password.

Credential tới pod qua một Kubernetes Secret, không bao giờ qua ConfigMap:

- Mỗi credential một Secret, tên `{tên engine}-rc-{tag}`. Tag là 6 chữ cái và chữ số đầu tiên của id credential (id không phải bí mật). Secret có label `egret/engine={tên engine}` và `egret/component=ranger-credential`, và annotation `egret/ranger-credential=<id credential>`.
- Nó chứa security XML của plugin (`ranger-trino-security.xml` hoặc `ranger-starrocks-security.xml`) với `policy.rest.url`, username và password.
- Trino mount nó ở coordinator và worker tại `/etc/trino/ranger-trino-security.xml`. StarRocks mount nó vào thư mục conf của FE qua `starRocksFeSpec.secrets` của cluster. Audit XML của StarRocks vẫn ở ConfigMap `{tên engine}-ranger-cm`. CN của StarRocks không chạy plugin.
- Pod template mang cùng annotation `egret/ranger-credential`. Credential mới đổi tên Secret và pod template, nên pod được roll.

Tên engine StarRocks tối đa 48 ký tự. Operator đặt tên mỗi volume của FE bằng tên object nó mount cộng 4 ký tự hash, và tên volume tối đa 63 ký tự. Tên dài hơn bị từ chối với 400 khi tạo và ở mọi lần apply của engine StarRocks đã gắn profile.

### Engine external

Engine external (đăng ký, do operator chạy) nhận credential lúc đăng ký. egret không bao giờ kết nối tới cluster của nó, nên bạn tự cài credential.

- `POST /api/control-plane/engines/external` trả 201 với engine và `rangerCredential: { username, password, credentialId }`, kèm `Cache-Control: no-store`. **Đây là lần duy nhất password được hiện ra.** Trang đăng ký hiện nó một lần trong một dialog có nút copy, và dialog không đóng được cho tới khi bạn xác nhận đã lưu password và credential id.
- `credentialId` đặt tên cho credential này. Bạn gửi lại nó khi retire credential trước đó sau một lần rotate.
- Không response nào khác mang password. `GET /api/control-plane/engines/{id}` hiện `rangerCredentialId` (id của credential hiện tại) và `rangerCredentialPending` (credential trước đó còn xác thực được hay không).
- Tab Plugin config (`GET /api/control-plane/engines/{id}/ranger-plugin-config`) render các file plugin với username của engine và placeholder `REPLACE_WITH_THE_ENGINE_PASSWORD`. `authMode` của nó là `engine-credential`, hoặc `anonymous` khi egret chạy chế độ đó.
- Engine đăng ký trước khi có credential thì không có credential nào. Tab Plugin config của nó nói vậy. Rotate một lần để lấy credential đầu tiên (xem bên dưới). Sau đó không có gì để retire.
- Xoá engine thì xoá các credential của nó.

### Cài credential bằng tay

Đặt username và password vào security XML của plugin, cạnh `policy.rest.url`. Với Trino, file là `ranger-trino-security.xml` và prefix là `ranger.plugin.trino`. Với StarRocks là `ranger-starrocks-security.xml` và `ranger.plugin.starrocks`.

```xml
<property><name>ranger.plugin.trino.policy.rest.url</name><value>https://egret.example.com/ranger</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.username</name><value>engine-0b8f6c1e-…</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.password</name><value>password egret hiện một lần</value></property>
```

Sau đó restart engine. Plugin đang chạy chỉ đọc file này lúc khởi động (đã đo, Q1). Dùng nguyên các file còn lại từ tab Plugin config.

Plugin phải từ 2.8.0 trở lên. Plugin 2.5.0 bỏ qua hai property này và không gửi header `Authorization` nào (đã đo). egret từ chối engine như vậy (xem [mức sàn phiên bản](#mức-sàn-phiên-bản)).

Để kiểm credential trước khi restart, gọi đúng đường plugin sẽ gọi, và xem cả content type và body chứ không chỉ mã trạng thái:

```bash
curl -s -o ranger-check.json -w '%{http_code} %{content_type}\n' \
  -u 'engine-<engine uuid>:<password>' \
  'https://egret.example.com/ranger/service/plugins/secure/policies/download/<profile>'
grep -o '"serviceName":"[^"]*"' ranger-check.json
```

`200 application/json` kèm `"serviceName":"<profile>"` nghĩa là credential và việc gắn profile đều đúng. Một `200` với `text/html` không phải egret: một proxy hay ingress đứng trước egret đang chuyển `/ranger` sang web console, và console trả 200 cho mọi credential. Hãy route `/ranger` tới backend của egret. Ingress của Helm chart và nginx của console (`frontend/nginx-prod.conf`) làm việc này từ bản này; proxy của riêng bạn cần cùng quy tắc đó. Các mã khác: xem [xử lý sự cố](#xử-lý-sự-cố). Xoá `ranger-check.json` sau đó: file chứa policy của profile.

## Rotate và retire

Quy tắc thường trực: egret không bao giờ xoá một credential mà engine có thể còn giữ. Vì vậy rotate giữ credential cũ còn hiệu lực ở vai trò credential trước đó, và credential trước đó chỉ bị xoá khi đã biết chắc nó không còn được dùng. Cho tới lúc đó, lần rotate thứ hai bị từ chối (409), với cả hai loại engine, nên rotate không bao giờ đẩy một credential ra ngoài.

### Engine được provision

1. Rotate: `POST /api/control-plane/engines/{id}/rotate-ranger-credential` (cần `engine:update`), hoặc **Rotate credential** trong mục Ranger credential ở tab Overview của engine.
2. `204` chỉ có nghĩa Kubernetes đã nhận lần apply. Sau đó pod roll sang Secret của credential mới.
3. Cả hai credential đều xác thực được cho tới khi egret quan sát thấy rollout hoàn tất. Hoàn tất nghĩa là: Secret của credential mới tồn tại và nêu đúng credential đó; mọi workload chạy plugin (Trino: Deployment coordinator và worker; StarRocks: StatefulSet FE) nêu credential mới trên pod template, đã quan sát generation mới nhất, mọi replica đã cập nhật và sẵn sàng, không còn gì của template cũ; và mọi pod đang chạy của các vai trò đó nêu credential mới, Ready và không đang terminate.
4. egret kiểm việc này mỗi 30 s (trên mọi replica của egret) và ở mỗi lần đọc engine. Khi rollout hoàn tất, credential trước đó bị xoá theo đúng id của nó, và Kubernetes Secret của nó bị xoá theo đúng tên, chỉ khi Secret đó vẫn nêu credential bị retire. Secret dùng chung cũ `{tên engine}-ranger-secret` (có từ trước khi mỗi credential có Secret riêng) bị xoá cùng lúc.
5. Cho tới lúc đó trang engine hiện **Previous credential pending**, `rangerCredentialPending` là `true`, và lần rotate tiếp theo bị từ chối với 409.

Thứ gì egret không đọc được, hoặc đọc thấy dở dang, đều tính là chưa hoàn tất và giữ credential trước đó. Đây là fail-closed, và các trường hợp sau còn pending chừng nào chúng còn kéo dài:

- rollout bị kẹt (một pod không bao giờ Ready);
- workload chạy plugin bị scale về 0, ví dụ engine Trino không có worker, hoặc bị autoscale về 0 (một giới hạn đã biết, xem bên dưới);
- engine bị gỡ khỏi profile trong khi một lần rotate còn pending;
- lần dò liên tục thất bại.

Trong lúc pending, credential trước đó còn hiệu lực lâu hơn bạn muốn, và bạn không rotate lại được.

Các câu trả lời khác của route rotate:

| Trả lời | Ý nghĩa |
|---|---|
| 409, "rotation is pending" | rollout của credential trước đó chưa được quan sát là hoàn tất; thử lại khi đã xong |
| 409, "the engine's ranger credential changed while the request was being built" | có thay đổi đồng thời; thử lại |
| 400 | engine không gắn policy profile nào (nên không có credential), Kubernetes chưa được cấu hình, hoặc tên engine StarRocks dài quá 48 ký tự |
| 502 | Kubernetes từ chối lần apply. Dòng engine giờ nêu credential mới và giữ credential trước đó; cả hai xác thực được cho tới khi một lần apply sau của credential mới roll xong |

### Engine external

1. Rotate: `POST /api/control-plane/engines/{id}/rotate-ranger-credential`, hoặc **Rotate credential** ở tab Overview của engine. Trả lời là 200 với `{ username, password, credentialId }` và `Cache-Control: no-store`, hiện một lần, trong cùng dialog như lúc đăng ký.
2. Cài password mới lên engine (như trên) và restart nó. Trong lúc đó credential trước đó vẫn xác thực được.
3. Xác nhận credential nào đang được cài: **Retire previous credential**, hoặc

   ```bash
   curl -s -X POST \
     -H 'Content-Type: application/json' \
     -d '{"installed": "<credentialId của credential mới>"}' \
     'https://egret.example.com/api/control-plane/engines/<id>/retire-previous-ranger-credential'
   ```

   (kèm session egret hay credential API của bạn, và `engine:update`). `204`: credential trước đó đã bị xoá và không còn xác thực được. Gửi lại đúng request đó cũng là `204`.
4. Lại được phép rotate.

Retire là một compare-and-set: nó chỉ xoá credential trước đó nếu `installed` vẫn là credential hiện tại. Một xác nhận cũ hoặc lặp lại, gửi trước một lần rotate sau đó, nhận 409 và không xoá gì. Retire trên engine được provision bị từ chối với 409, vì egret tự retire những engine đó.

Giao diện giữ `credentialId` của lần rotate trong state của trang (không bao giờ trong storage hay URL). Sau khi tải lại trang, nó yêu cầu bạn gõ id đã lưu. Đừng copy id hiện tại từ trang engine, trừ khi đó đúng là credential engine đang chạy.

**Nếu bạn làm mất password mới trước khi cài, ở bản này bạn bị kẹt.** Rotate bị từ chối khi credential trước đó còn pending, và lối ra duy nhất là retire, mà retire thì xoá credential engine còn đang chạy. Để gỡ: retire với `credentialId` mới, rotate lại, cài credential đó và retire lần nữa. Từ lần retire đầu tiên tới khi engine restart với credential mới nhất, plugin của nó nhận 401: nó tiếp tục cưỡng chế policy đã cache, không nhận thay đổi, và một pod restart không có cache thì từ chối mọi thứ. Chưa có route để huỷ một credential đang pending.

## Mức sàn phiên bản

egret hỗ trợ Ranger plugin từ 2.8.0 trở lên. Các cặp (engine, plugin) được hỗ trợ là Trino 2.8.0, StarRocks 2.8.0 và StarRocks 2.9.0. Engine mới mặc định là Trino 483 (plugin 2.8.0) và StarRocks 4.1.6 (plugin 2.9.0).

Plugin 2.5.0, đi kèm `trinodb/trino:470`, không gửi được credential, nên không còn được hỗ trợ:

- tạo engine trên `trinodb/trino:470`, hoặc cập nhật một engine sang nó, bị từ chối với 400;
- đăng ký engine external khai báo plugin cũ hơn 2.8.0 bị từ chối với 400 ("Ranger plugin 2.5.0 is not supported: egret requires 2.8.0 or later");
- engine Trino 470 có sẵn vẫn đọc được nhưng xác thực thất bại. Nâng nó lên Trino 483 hoặc xoá nó.

Chi tiết và cách nâng cấp: [engine-versions.md](engine-versions.md).

## Theo dõi xác thực thất bại

Khi một engine trình username của nó (`engine-<uuid>`) với password sai, egret đếm lần bị từ chối cho engine đó trong `policy.ranger_auth_failures` và ghi `warn!` "ranger credential rejected" kèm engine id và số lần bị từ chối. Cả password lẫn header đều không được ghi log.

**Overview** của Governance khi đó hiện mục cần chú ý `engine_auth_failing` (nhãn "engine Ranger credentials", mức critical, thấy được với `engine:read`). Nó nêu từng engine có lần bị từ chối mới nhất mới hơn lần tải thành công mới nhất và chưa quá 24 giờ, kèm số lần bị từ chối và thời điểm lần cuối, và link tới engine.

- **Nó tự xoá.** Lần tải thành công kế tiếp của engine đó ghi `ok_at`, và mục biến mất. Lần tải thành công được ghi tối đa mỗi phút một lần cho mỗi engine, nên mục có thể còn khoảng một phút cộng một chu kỳ poll sau khi engine đã được sửa. Lần bị từ chối cũ hơn 24 giờ không còn được hiện.
- **Số đếm là cận dưới.** Các lần bị từ chối được gộp theo engine mỗi 10 s và theo từng replica của egret; những lần còn đang bị giữ lại khi một đợt dồn dập dừng thì không được ghi, và lần ghi bị bỏ qua khi đã có 8 lần ghi đang chạy.
- **Chỉ đếm engine tồn tại.** Username không nêu engine nào thì không thêm gì, nên caller không làm phình bảng bằng cách bịa id.
- **Không được đếm:** request không có credential, username không phải dạng `engine-<uuid>`, và bearer operator sai. Vì vậy một engine mà plugin hoàn toàn không cấu hình credential (engine từ trước thay đổi này, hoặc XML viết tay không có credential) không được mục này nêu tên. Engine được provision như vậy hiện ở các mục "plugin liveness" thay vào đó, `plugin_quiet` sau 5 phút và `plugin_silent` sau 24 giờ, nhưng chỉ khi không có plugin nào khác poll cùng profile. Engine external không được các mục đó đánh giá.
- **Ở chế độ `anonymous`** credential engine sai vẫn được đếm, dù request sau đó được phục vụ như ẩn danh, và lần tải ẩn danh đó không xoá mục.
- **Mục này có thể bị giả mạo.** Ai với tới egret và biết id của một engine (id engine thấy được với mọi người có `engine:read`) có thể gửi password sai cho nó và bật mục lên. Lần tải thật kế tiếp của engine xoá nó, nên nó chập chờn. Nó không ảnh hưởng tới engine.
- **Một đợt dồn dập có thể che một lỗi thật trong chốc lát.** egret theo dõi tối đa 4096 engine id mỗi cửa sổ 10 s trong bộ nhớ. Caller dồn id ngẫu nhiên có thể lấp đầy nó, và khi đó lần bị từ chối đầu tiên của một engine thật không được ghi cho tới khi cửa sổ qua.

## Khi egret không với tới được, hoặc từ chối plugin

Plugin 2.8.0 làm gì, đã đo với jar thật trên một admin giả (tài liệu số đo, Q1 và Q2):

- **Plugin đang chạy có policy trong bộ nhớ thì tiếp tục cưỡng chế chúng qua các lần 401**, và thử lại mỗi chu kỳ poll. Nó không nhận thay đổi nào cho tới khi lại tải thành công.
- **Plugin khởi động không có policy cache và nhận 401 thì từ chối mọi thứ**, và tiếp tục thử lại.
- **Plugin khởi động có file cache trên đĩa thì hành xử như plugin đang chạy**: nó cưỡng chế policy đã cache.
- Plugin đang chạy không đọc lại security XML. Đổi credential cần restart.

Đã đo trên kind với pod Trino 483 và StarRocks 4.1.6, với chính egret ([kết quả live](../../en/plans/2026-10-20-ranger-surface-auth-live-result.md), kịch bản 4):

- **Engine đang chạy tiếp tục cưỡng chế từ bộ nhớ khi egret sập.** Một FE StarRocks, với egret scale về 0 lâu hơn một chu kỳ poll, vẫn trả lời query theo policy của nó.
- **Cache policy không sống qua một lần restart pod.** Thư mục cache là `/tmp/ranger/<profile>/policycache` trong container, nằm trên filesystem riêng của container, và egret không mount volume nào ở đó. Một pod mới (reschedule, drain node, OOM kill, rollout của một lần rotate, pod bị xoá) khởi động không có cache.
- **FE StarRocks restart khi egret sập thì từ chối mọi thứ**, kể cả với người giữ các policy `egret-admin`: một query nhận `Access denied; you need (at least one of) the SELECT privilege(s)` và `SHOW DATABASES` không trả dòng nào. Plugin log `cache file does not exist or not readable`. Quyền truy cập trở lại 16 s sau khi egret trở lại. (Khi egret sập, một Iceberg catalog do egret phục vụ cũng hoàn toàn không dùng được, bất kể Ranger quyết định gì.)
- **Engine Trino xác thực JWT (mặc định) hoàn toàn không khởi động được khi egret sập.** Coordinator đọc JWKS của egret lúc khởi động (`Error reading JWK keys from http://egret…/.well-known/jwks.json`) và rơi vào `CrashLoopBackOff`; nó khởi động khi egret trở lại. Worker không bị restart vẫn giữ cache.
- Bản thân việc rotate không gặp chuyện này: pod mới khởi động với một credential đã xác thực được, khi egret đang chạy.

Vậy mọi lần restart pod khi egret không với tới được đều đưa engine đó về từ-chối-tất-cả (StarRocks) hoặc ngừng chạy (Trino JWT) cho tới khi egret trở lại. **Còn mở, chưa quyết:** mount một `emptyDir` ở thư mục cache (sống qua restart container, không sống qua xoá pod hay reschedule), mount một PVC nhỏ ở đó (sống qua cả hai, đổi lại mỗi pod một volume), hoặc chấp nhận rủi ro như mô tả ở đây.

## Xử lý sự cố

| Mã | Ở | Ý nghĩa và cách sửa |
|---|---|---|
| 401 | `/ranger/service/...` | Không có credential, credential sai dạng, engine không tồn tại, password sai hoặc đã retire, engine đã bị xoá, hoặc bearer operator sai. Body giống nhau cho tất cả. Kiểm `policy.rest.client.username` và `.password` của plugin, và plugin từ 2.8.0 trở lên (2.5.0 không gửi header). Engine provision trước thay đổi này chưa có credential cho tới lần apply kế tiếp (xem [ghi chú nâng cấp](#ghi-chú-nâng-cấp)). |
| 403 | một lần tải | Credential engine hợp lệ, nhưng profile nó xin không phải profile engine của nó được gắn (cũng không phải tag service của profile đó), hoặc engine không gắn profile nào. Sửa `service.name` của plugin, hoặc gắn engine. egret ghi log engine id và tên được xin. |
| 403 | `/ranger/service/plugins/definitions`, `…/services`, `…/policies/{id}` | Credential engine trên route của operator. Dùng bearer của operator. |
| 404 | `/service/...` ở gốc | URL cũ. Đặt `policy.rest.url` thành `<egret>/ranger`. |
| 404 | `/ranger/service/roles/secure/download/...` | Bình thường với plugin đã xác thực; egret không phục vụ roles và plugin vẫn chạy tiếp. |
| 404 | một lần tải, sau khi xác thực | Không có profile nào tên đó. |
| 404 | mọi thứ dưới `/ranger` | Ranger đang tắt trên egret này (`ranger.enabled`). |
| 200 `text/html` | mọi thứ dưới `/ranger` | Không phải egret: một proxy hay ingress chuyển `/ranger` sang web console, và console trả 200 cho mọi request. Route prefix `/ranger` tới backend của egret (ingress của Helm và nginx của console làm việc này từ bản này). |
| 500 | `/ranger/service/...` | egret không kiểm được credential (database hoặc secret store). Log ghi "ranger: engine credential check failed" kèm engine id. |

Các câu trả lời của control plane:

| Mã | Route | Ý nghĩa |
|---|---|---|
| 409 | rotate | Credential trước đó còn pending: chờ rollout (provisioned) hoặc retire nó (external). |
| 409 | retire | `installed` không phải credential hiện tại, hoặc engine là provisioned. |
| 409 | mọi lần apply | Credential của engine đổi trong lúc request đang được dựng; thử lại. Hoặc credential đã lưu không render được (chỉ credential từ bản build phát triển trước khi renderer có bước kiểm); rotate nó. |
| 400 | tạo hoặc cập nhật | Trino 470 hoặc plugin khai báo dưới 2.8.0, hoặc tên engine StarRocks quá 48 ký tự. |

## Giới hạn đã biết

- **Không có khoá apply theo engine.** egret đọc lại engine ngay trước mỗi lần apply và từ chối (409) request render một credential không còn là hiện tại. Giữa lần đọc lại đó và lần apply lên Kubernetes, một lần rotate đồng thời và lần retire của nó vẫn có thể chen vào, và khi đó một lần apply cũ có thể đặt lại credential đã retire lên cluster. Engine nhận 401 cho tới lần apply kế tiếp.
- **Workload chạy plugin bị scale về 0 chặn việc retire.** Rollout của nó không bao giờ được quan sát là hoàn tất, nên credential trước đó còn hiệu lực và rotate vẫn bị từ chối cho tới khi nó scale lên lại.
- **Operator external làm mất password mới trước khi cài thì bị kẹt** (xem trên). Không có route để huỷ một credential đang pending.
- **Bộ gộp lỗi không có negative cache** cho engine id không tồn tại, nên một đợt dồn id ngẫu nhiên có thể che lần bị từ chối đầu tiên của engine thật tới 10 s.
- **Mọi user có `engine:read` thấy `credentialId` hiện tại của mỗi engine.** Tự nó không cấp gì: retire cần `engine:update`, và giao diện không bao giờ tự gửi id đó.
- **Plugin còn ấm tiếp tục cưỡng chế policy đã cache qua các lần 401, nhưng plugin lạnh từ chối mọi thứ,** và cache không sống qua một lần restart pod. Pod restart khi egret sập thì từ chối mọi thứ (StarRocks) hoặc không khởi động (Trino JWT) cho tới khi egret trở lại. Xem [phần trên](#khi-egret-không-với-tới-được-hoặc-từ-chối-plugin).

## Ghi chú nâng cấp

Đây là thay đổi phá vỡ tương thích, không có giai đoạn chuyển tiếp.

**URL đã đổi.** `/service/...` ở gốc server không còn, không có alias; nó trả 404. `policy.rest.url` của plugin phải là `<egret>/ranger`. egret render lại nó cho engine được provision ở lần apply kế tiếp. Với engine external hay XML viết tay, bạn tự đổi. Script nào curl `/service/...` chuyển sang `/ranger/service/...`.

**Xác thực được bật mặc định.** `ranger.auth_mode` mặc định là `required`, và trước bản này không thứ gì trình credential. Khi nâng cấp, mọi plugin nhận 401 cho tới khi có credential. Plugin đang chạy tiếp tục cưỡng chế policy đã cache và không nhận thay đổi; pod khởi động không có cache thì từ chối mọi thứ. Vì vậy:

- **Engine được provision** nhận credential ở lần apply kế tiếp, và lần apply đó roll pod của chúng. Apply mỗi engine đã gắn profile một lần: lưu settings của nó (`PUT /api/control-plane/engines/{id}`), hoặc gọi `POST /api/control-plane/engines/{id}/rotate-ranger-credential`. **Restart** không render lại cấu hình và không đủ.
- **Engine external** đăng ký trước bản này không có credential. Rotate một lần để lấy, cài nó và restart engine. Không có gì để retire.
- **StarRocks của dev-compose** (`deploy/docker-compose.dev.yml`) mount một XML viết tay không có credential và nhận 401. Hoặc đăng ký nó là engine external gắn với `sr1` rồi thêm hai property với credential vào `deploy/starrocks/ranger/ranger-starrocks-security.xml`, hoặc chạy egret trên host với `EGRET_RANGER__AUTH_MODE=anonymous` (chỉ cho phát triển cục bộ).
- `just deploy-kind-governance` không còn đặt `ranger.authMode=anonymous`.
- Đặt `ranger.authMode=anonymous` trong lúc chuyển đổi sẽ khôi phục hành vi cũ, kèm rủi ro lộ dữ liệu nói ở trên.

**Engine Trino đã log password Ranger của nó trước bản sửa này.** Ranger plugin của Trino in mọi property của plugin ở mức INFO lúc khởi động, kể cả password (`AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=…`, logger `org.apache.ranger.audit.provider.AuditProviderFactory`). Dòng đó nằm trong stdout của mọi coordinator và worker, nên ai đọc được log pod (`kubectl logs`, hoặc màn log pod của egret với `engine:read`) đều đọc được credential. egret nay render logger đó ở `WARN` trong `log.properties` của mọi engine Trino đã gắn profile, sau các dòng log của bạn, nên mức bạn đặt cho logger đó bị thay thế. Thay đổi này nằm trong checksum của pod template, nên mỗi engine Trino đã gắn profile roll ở lần apply kế tiếp. Màn log pod của egret và luồng log trực tiếp cũng che các giá trị kiểu `…password=`, `…secret=`. **Rotate credential của mọi engine Trino đã gắn profile chạy trước bản sửa này**: password cũ có thể nằm cả trong kho log ngoài egret. Một lần rotate cũng là một lần apply, nên nó render luôn `log.properties` mới, và pod khởi động với credential mới không log nó. StarRocks không log password.

**Response plugin-config đã đổi.** `GET /api/control-plane/engines/{id}/ranger-plugin-config` báo `authMode` là `engine-credential` hoặc `anonymous`. Giá trị `bearer-configured` không còn. Client ngoài egret nào so khớp giá trị đó phải sửa.

**Engine Trino 470** phải chuyển sang Trino 483 trước. Xem [engine-versions.md](engine-versions.md).
