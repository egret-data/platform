# W9c — KV store: `kv()`, các task `core.kv`, đợt quét hết hạn và API KV

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-07-workflow-w9c-kv-store-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-07; chưa cài.
**Phụ thuộc:** W9b (namespace files: `nsfiles::access::authorize` và `Grant`, các staging object và đợt quét của chúng, các helper advisory lock, kết nối namespace lúc render trong `storage/scope.rs`, quy ước route của `nsfiles/routes.rs`) — đã merge, #653. W9a (`FileStore` toàn process, `ObjectStore` của `egret-storage`), W5b (plugin host) — đã merge. Base `origin/main` `11782600`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 phát hiện 4, §9 (dòng W9: "tách ngày 2026-10-01 … W9c *KV store*"); spec W9b `docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md` §14 (KV store để lại cho W9c).
**Đã quyết với người dùng, 2026-10-07:**
- **(2026-10-07) Phạm vi.** W9c giao hàm `kv()`; các task `Set`, `Get`, `GetKeys` và `Delete`; `PurgeKV`; task `Put` deep-merge; và API KV. Plugin host **không** có cầu KV, vì không plugin nào trong catalog gọi `namespaceKv`. Thông báo từ chối của nó đổi sang chỉ tới các task `core.kv.*` (§10).
- **(2026-10-07) Phạm vi ghi là của W9b.** Một flow đọc KV của mọi namespace trong workspace của nó. Nó chỉ ghi và xoá trong namespace của chính nó. `PurgeKV` tác động lên namespace của chính flow và các namespace con, theo ranh giới dấu chấm. Một `authorize` dùng chung cho W9b và W9c, thêm một loại tài nguyên KV. Kestra cho một flow ghi vào mọi namespace đã tồn tại; §2 ghi lại chỗ khác này (§5).
- **(2026-10-07) Lưu trữ theo Kestra.** Giá trị của mỗi revision là một object Ion dạng text ở `{prefix}{ws}/{ns-as-path}/_kv/{key}.ion`, hoặc `….ion.vN` cho revision N > 1. Một bảng Postgres, `workflow.kv_entries`, giữ một dòng mỗi revision: workspace, namespace, key, revision, type, size, description, `expires_at`, deleted, `created_at` và `created_by` (user, execution hoặc task run). egret **lưu** type, còn Kestra suy lại khi đọc; fixture của Task 1 quyết định kết quả đọc có còn khớp Kestra không, và mỗi khác biệt đo được là một dòng trong §2. Một lần ghi dùng lại staging của W9b. Body được stage khi không giữ kết nối hay lock nào. Rồi một transaction ngắn có lock khoá `(ws, ns, key)`, kiểm `overwrite`, chọn revision, copy phía server, chạy `head`, chèn dòng và commit. Nhờ vậy `overwrite: false` và read-merge-write của `Put` không có race. Một giá trị bị chặn bởi `max_file_bytes`; Kestra không có trần (§2). *(Sửa ngày 2026-10-07 theo phán quyết của người dùng về trần giá trị, bên dưới: trần là `max_read_bytes`.)* Key theo regex của Kestra `[a-zA-Z0-9][a-zA-Z0-9._-]*`, và độ dài của nó có giới hạn (§3). Migration lấy số trống kế tiếp trên main: hôm nay là 0175, bước rebase của plan kiểm lại (§3, §4).
- **(2026-10-07) TTL: hết hạn nghĩa là không có.** Đọc một key đã hết hạn cư xử y như đọc một key không có, và tuân theo `errorOnMissing`. Đọc không bao giờ ghi. Một đợt quét mỗi giờ xoá cứng các key hết hạn, cả dòng lẫn object, theo row id, trên mọi workspace, và chạy an toàn trên mọi replica. Kestra xoá mềm khi đọc, báo lỗi bất kể `errorOnMissing`, và chỉ quét tenant `main` cùng các namespace có flow (§2, §4.5, §11).
- **(2026-10-07) Kế thừa theo Kestra.** Một `kv()` không có namespace, hoặc một `Get` có namespace là của chính flow, đi ngược lên các namespace cha theo ranh giới dấu chấm. Một namespace ghi rõ thì không đi. API có một route liệt kê kế thừa (§4.6, §9).
- **(2026-10-07) `secret()` trong giá trị của `Set` hay `Put` bị từ chối lúc publish.** Lỗi nói rằng giá trị của secret sẽ bị lộ qua KV store. Điều này khác Kestra (§2, §7).
- **(2026-10-07, phán quyết của người dùng về trần giá trị) Các lần ghi một giá trị KV bị chặn ở `max_read_bytes`, không phải `max_file_bytes`, nên mọi thứ ghi được thì cũng đọc được.** `Set`, `Put` (kể cả kết quả đã merge của nó) và `PUT` của API từ chối một giá trị đã mã hoá vượt `max_read_bytes`, nêu trần (§2, §4.2, §4.3, §8.1, §8.5, §9).
- **(2026-10-07) Các điểm thiết kế đã duyệt (phần 2–4 của buổi brainstorm).**
  - Version và xoá mềm theo R1 của W9b. Mỗi lần ghi tạo một revision mới. `Delete` đánh dấu revision mới nhất. Lịch sử thấy được qua API, các revision đã xoá được đánh dấu. Ghi lại sau khi xoá tạo một revision mới.
  - `PurgeKV` chỉ xoá các key hết hạn, hoặc xoá theo version (`before` / `keepAmount`). Nó lọc bằng `keyPattern` và xoá theo row id, phân trang keyset. Quy tắc của W9b áp dụng: purge theo version cần `keepAmount` hoặc `before`.
  - `Set` nhận `key`, `value`, `overwrite` (mặc định true), `ttl`, `kvType` và `kvDescription`. BOOLEAN chặt, như trong artifact.
  - `Get` xuất `value`. `GetKeys` nhận `prefix`, chỉ liệt kê key còn sống, và bị chặn bởi `max_namespace_files`. `Delete` xuất `deleted`. `Put` deep-merge dưới lock của key, ngữ nghĩa merge đo ở Task 1.
  - `kv()` dùng kết nối theo-render của W9b và `authorize(Read)`. `errorOnMissing` của nó mặc định true; của các task mặc định false.
  - API nằm dưới `/api/ws/{ws}/workflows/namespaces/{ns}/kv…`. Nó có list, list kế thừa, get (kèm type), revisions, put (qua staging, có giới hạn body và timeout), delete và bulk delete. Đọc cần `workflow:read`, ghi cần `workflow:manage`. Chờ lock quá 5 s trả 409. Status theo quy ước của W9b.
  - Hình dạng của plan (§12) và cách kiểm thử (§13), với bài học của W9b làm ràng buộc toàn cục.

**Giao hàng:** một PR (docs và code), tám task trong §12, cài theo một plan viết sau spec này.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái ở `11782600` |
|---|---|
| `kv()` | bị từ chối theo tên: `("kv", not_yet("the namespace KV store", "W9c"))` trong `REFUSED` (`runners/functions.rs:276`, bảy mục); ghim bởi `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven` (`:3325-3370`) |
| `io.kestra.plugin.core.kv.*` | không có dòng trong `spec/registry.rs`, nên publish từ chối từng cái như một type không biết (`publish.rs:449-459`; `spec/validate.rs:1654` cảnh báo `unknown_task_type`) |
| `secret()` | chỉ phân giải trên plugin task `kestra.*`; mọi chỗ khác là `Refusal::Confined(SECRET_REFUSAL)` (`runners/functions.rs:229-232`, `:275`), và publish báo `CODE_SECRET_OUTSIDE_PLUGIN_TASK` với lý do chung đó (`spec/validate.rs:395-403`) |
| plugin host | `ShimRunContext.namespaceKv` ném `EgretUnsupported` nêu tên W9c (`plugin-host/…/ShimRunContext.java:340-341`, doc lớp `:52`; ghim bởi `EgretPluginHostTest.java:125-126`; doc nhắc tới ở `EgretUnsupported.java:4`, `EgretPluginHost.java:58`). Thông báo exit-4 của handler nêu "the KV store — W9c" (`worker/handlers/kestra.rs:379`, doc module `:79`, ghim ở `:1317-1321`). Ba jar plugin đã khoá (postgres, mysql, trino 2.0.0) có **0** tham chiếu tới `namespaceKv` |
| những gì dùng lại từ W9b | `nsfiles::access::{authorize, Actor, Access, Grant, check_namespace, check_files_segment, API_LOCK_WAIT}` (`access.rs:23-176`); `nsfiles::staging::{record, forget, sweep, spawn_staging_sweeper}` và bảng `workflow.namespace_file_staging` (`staging.rs:47-192`); đường ghi `store::put` → `stage_body` → `commit_staged` (`store.rs:1065`, `:1108`, `:1146`) và lock transaction của nó `pg_advisory_xact_lock(hashtextextended(…, 0))` (`store.rs:567-605`); `storage::lock::LockedSession` (lock session, `lock_timeout`, `is_lock_timeout`); `NsError` và `CreatedBy` (`nsfiles/mod.rs:37-73`); `nsfiles::db_options()` và một kết nối namespace của render (`storage/scope.rs:26-42`); `ns_refusal`, `no_store`, `audit`, các timeout body 408 của routes (`nsfiles/routes.rs:202`, `:245`, `:349`, `:821-887`) |
| trần | `Caps { max_file_bytes (1 GiB), max_task_bytes (5 GiB), max_read_bytes (10 MiB), max_namespace_files (10 000) }` (`storage/mod.rs:61-67`, mặc định `egret-config/src/lib.rs:898-957`) |
| migration | cái cuối là `0174_workflow_namespace_files.sql`; W9c lấy **0175**, kiểm lại khi rebase |
| corpus | 2 trên 345 flow của `oracle-exec` dùng KV. `valids/kv.yaml` (`kv('my-key').field ?? null`, `kv('my-key', inputs.namespace, inputs.errorOnMissing)`) là `not-standalone`, không có mục golden, và là một chốt chống từ-chối-thừa (`oracle-exec/README.md:3627-3637`). `valids/retry-with-flowable-errors.yaml` (`kv.Set` với `kvType: NUMBER`, rồi `AllowFailure` tăng nó qua `kv(namespace=…, key=…) + 1`) đang baselined (`baseline.txt:614`) và bị giữ bởi cả `core.kv.Set` lẫn flowable `AllowFailure` còn opaque (`README.md:6420`). `function-kv` của oracle biểu thức (`oracle/corpus.json:302`) chỉ ghim "Function or Macro [kv] does not exist" (`oracle/golden.json:2201-2205`) |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (source `96893b589`; artifact `2.0.0-rc13`, nightly `169ca7f2`, image mà W9a và W9b đã đo). Chỗ hai bên khác nhau, artifact quyết định. Đường dẫn tính từ `core/src/main/java/io/kestra/plugin/core/kv/` trừ khi có nêu.
- **Task.** `Set` (`Set.java:63-147`) render `value` bằng `renderTyped` (`:111`), mặc định `namespace` là `{{ flow.namespace }}` (`:86`) và `overwrite` là true (`:93`), và ép một String theo `kvType`: NUMBER bằng Jackson; BOOLEAN chặt trong artifact (`equalsIgnoreCase("true"|"false")`, nếu không thì `Cannot parse '%s' as a BOOLEAN value: expected 'true' or 'false'.`; `TypeConverter.toBoolean` của source ở `:120` là bản cũ hơn); DATETIME bằng `toInstant`; DATE bằng `LocalDate.parse`, lùi về ngày UTC của một `Instant` (`:156-162`); DURATION được kiểm rồi lưu nguyên chuỗi; JSON bằng `JacksonMapper.toObject`. Một Number với `kvType: STRING` thành `toString()` của nó (`:131-133`). `Get` (`Get.java:55-122`) có `errorOnMissing` false (`:74`), đi lên cha khi namespace bằng của flow (`:82-83`, `:99-111`), nếu không thì kiểm ACL (`:85`); không có mà `errorOnMissing` bật thì ném `No value found for key '…' in namespace '…' and \`errorOnMissing\` is set to true` (`:91`); một key hết hạn làm task thất bại bất kể `errorOnMissing` (`:77-97`, suy ra). `GetKeys` (`GetKeys.java:49-88`) liệt kê key còn sống theo `prefix`, có kiểm ACL (`:65`). `Delete` (`Delete.java:47-90`) xoá mềm, `errorOnMissing` false (`:66`), xuất `deleted`. `Put` (`Put.java:72-121`) đọc-sửa-ghi: một Map deep-merge vào một Map, một list các `{key, value}` vá các đường dẫn có chấm, một `[{value}]` đơn thay thế; nó giữ description và hạn, và luôn ghi đè (`:100`, `:121`). `PurgeKV` (`PurgeKV.java:57-132`) nhận `keyPattern`, `namespaces` xor `namespacePattern` (không có cái nào: mọi namespace), `includeChildNamespaces` true (`:90`), `behavior` mặc định `{type: key, expiredOnly: true}` (`:83`, `Key.java:33`) hoặc `{type: version, before, keepAmount}` (`Version.java:30-69`), và xoá cứng.
- **`kv()`** (`core/runners/pebble/functions/KvFunction.java:23-104`): tham số `key`, `namespace`, `errorOnMissing` (`:34-36`); `errorOnMissing` mặc định **true** (`:53`, xác nhận trong bytecode của artifact); không có namespace nghĩa là của flow có kế thừa cha (`:61-63`, `:84-96`), một namespace ghi rõ, kể cả của chính flow, thì không (`:64-66`); không có: `The key '…' does not exist in the namespace '…'.` (`:78`); thiếu key: `The 'kv' function expects an argument 'key'.` (`:100`); một key hết hạn bị nuốt thành `null` khi `!errorOnMissing` (`:68-72`).
- **Lưu trữ.** `kvFileName` là `key + ".ion"` cộng `".v"+rev` khi rev > 1 (`KVStore.java:41-43`), dưới `/{ns-as-path}/_kv` (`StorageContext.java:43`, `:328-329`). Một `Duration` được ghi nguyên chuỗi ISO, mọi giá trị khác dưới dạng Ion text (`InternalKVStore.java:80`, `JacksonMapper.java:138-141`); khi đọc, mọi String trông giống một duration ISO thành `Duration` (`:134-142`). Type không được lưu: `KVType.from` suy lại cho API. Description và hạn là metadata của object (`KVMetadata.java:43-53`) và cột DB; hạn là `now + ttl` lúc ghi (`KVMetadata.java:25`). Bảng `kv_metadata` (`jdbc-postgres/…/baseline-postgres.sql:383-409`) giữ các revision: một lần ghi là `previous + 1` và đánh dấu dòng trước là không-cuối; xoá đánh dấu dòng mới nhất là deleted mà không thêm revision (`AbstractJdbcKvMetadataRepository.java:160-180`). `put` lưu dòng trước khi ghi object (`InternalKVStore.java:82-96`). Đọc một dòng mới nhất đã hết hạn thì xoá mềm nó và ném `The requested value has expired` (`:150-160`).
- **Truy cập.** `KVStoreService.checkAccessNamespaceIsAllowed` (artifact): namespace của chính flow hay một tổ tiên theo ranh giới dấu chấm là "self"; mọi namespace khác phải qua `checkAllowedNamespace` (luôn true trong OSS, `NamespaceService.java:43-45`) và phải **tồn tại** (có một flow hay KV trong đó), nếu không thì `Cannot access the KV store. The namespace '%s' does not exist.` Source (`KVStoreService.java:165-196`) không có ranh giới dấu chấm; artifact quyết định. `RunContext.namespaceKv(ns)` truyền namespace của flow làm `from` (`DefaultRunContext.java:626-628`).
- **Quét.** `KVPurgeCleaner` chạy mỗi giờ sau 1 h trễ, không trên worker, theo lô 1000 (`KVPurgeCleaner.java:44`), chỉ trên tenant `main` và chỉ trên các namespace có flow (`:89-93`).
- **API** (`webserver/…/KVController.java`, `/api/v1/{tenant}`): `GET /kv` phân trang qua mọi namespace (`:56-70`); `GET /namespaces/{ns}/kv/inheritance` (chỉ tổ tiên, sâu nhất trước, lần xuất hiện đầu thắng, `:73-100`); `GET /namespaces/{ns}/kv/{key}` → `KvDetail{type, value, revision, updated}` (`:103-121`); `PUT /namespaces/{ns}/kv/{key}` (`text/plain` parse như Ion, lùi về chuỗi thô; header `description`, `ttl`; luôn ghi đè, `:124-141`); `DELETE …/kv/{key}` → boolean (`:144-150`); `DELETE /namespaces/{ns}/kv` với `{keys: […]}` → `{keys: deleted}`, bỏ qua lỗi IO từng key (`:153-182`).

| Kestra | egret W9c | vì sao |
|---|---|---|
| một flow đọc KV của mọi namespace đã tồn tại (OSS) | mọi namespace **của workspace của nó**, tồn tại hay không; một namespace không có KV thì không có key (§5) | workspace là tenant của egret. egret không có bảng namespace, nên bỏ kiểm tra tồn tại của Kestra. Một lần đọc không thể lộ gì từ một namespace không chứa gì |
| một flow `Set` hay `Delete` KV của mọi namespace đã tồn tại, và của các tổ tiên của nó (`checkAccessNamespaceIsAllowed`) | chỉ namespace của chính nó; `PurgeKV` của chính nó và các con theo ranh giới dấu chấm (§5) | quyết định của người dùng (2026-10-07), quy tắc của W9b: một tác giả có `workflow:manage` trên một flow không được viết lại trạng thái dùng chung của đội khác từ một task |
| `overwrite: false` là kiểm-rồi-làm, và `revision = previous + 1` tính không có lock; `Put` đọc và ghi không có lock | kiểm overwrite, revision và merge của `Put` chạy dưới một advisory lock theo `(ws, ns, key)`, trong transaction chèn dòng (§4.3, §8.5) | quyết định của người dùng (2026-10-07): hai người ghi không bao giờ chung một revision, và `overwrite: false` và `Put` không có race |
| dòng được lưu trước khi ghi object | object được copy vào chỗ, rồi dòng được chèn, trong một transaction (§4.3) | thứ tự của W9b: người đọc không bao giờ thấy một revision không có object phía sau |
| đọc một key hết hạn thì xoá mềm nó; `Get` và API rồi báo lỗi bất kể `errorOnMissing`; `kv()` chỉ nuốt nó khi `errorOnMissing` là false | một key hết hạn là một key không có ở mọi nơi: `errorOnMissing` quyết định, và không lần đọc nào ghi (§4.5) | quyết định của người dùng (2026-10-07): hết hạn là vắng mặt, không phải lỗi, và một lần đọc mà ghi làm kết quả phụ thuộc vào ai nhìn trước |
| lần đi lên cha của `Get` dừng với lỗi ở một key hết hạn | lần đi coi nó là không có và đi tiếp lên cha (§4.6) | cùng quy tắc: hết hạn nghĩa là không có |
| đợt quét hết hạn phủ tenant `main` và các namespace có flow | mọi workspace và mọi namespace có KV, mỗi giờ, trên mọi replica (§11) | quyết định của người dùng (2026-10-07): nếu không, một giá trị hết hạn trong một namespace không có flow sẽ giữ storage mãi mãi |
| `secret()` trong giá trị `Set` lưu secret dạng text thường, không che | bị từ chối lúc publish, nêu tên task và nói secret sẽ bị lộ qua KV (§7) | quyết định của người dùng (2026-10-07): bất kỳ ai có `workflow:read` đều đọc lại được |
| không có trần cho một giá trị | object Ion được lưu tối đa `max_read_bytes` (mặc định 10 MiB), trên mọi đường ghi (§4.2) | bộ nhớ và storage có giới hạn; `kv()` chèn giá trị vào một lần render. Phán quyết của người dùng (2026-10-07): trần ghi bằng giới hạn đọc, nên mọi thứ ghi được thì cũng đọc được |
| không có trần độ dài key ngoài cột `VARCHAR(350)` | tối đa 350 byte; một key dài hơn là lỗi tác giả nêu quy tắc (§3) | mọi key Kestra lưu được, egret lưu được; một key dài thất bại nêu tên nó, không phải như một lỗi DB |
| type được suy lại khi đọc (`KVType.from`), nên một STRING `"P1D"` đọc về thành DURATION | type được lưu cùng mỗi revision và đọc về đúng như đã ghi (§4.2) | quyết định của người dùng (2026-10-07). Những kết quả đọc nào khác Kestra được **đo ở Task 1**; mỗi khác biệt đo được thêm vào bảng này thành một dòng riêng |
| một segment namespace `_kv` (`a._kv`) được chấp nhận | bị từ chối lúc publish và bởi mọi thao tác KV và namespace-file (§3) | key của flow và file của nó sẽ nằm dưới tiền tố `_kv` của namespace `a`; hai không gian key không được chồng nhau (quy tắc `_files` của W9b) |
| không có route revisions; một key đã xoá trả 404 | `GET …/kv/{key}/revisions` liệt kê mọi revision, đánh dấu cái đã xoá, và đọc một revision chỉ rõ của một key đã xoá mềm thành công; đọc bản mới nhất là 404 (§9) | R1 của W9b: lịch sử còn thấy được tới khi một lần purge xoá nó |
| `GET /kv` liệt kê mọi namespace, phân trang | các key còn sống của một namespace, chặn bởi `max_namespace_files` (§9) | quy tắc liệt kê của W9b; liệt kê xuyên namespace nằm ngoài phạm vi (§14) |
| `PurgeKV` không có `namespaces` xoá mọi namespace; trong source, con được khớp theo tiền tố chuỗi | mặc định là namespace của chính flow (và các con của nó); con khớp theo ranh giới dấu chấm (§8.6) | quy tắc purge (§5) |
| `PurgeKV` `behavior: {type: key, expiredOnly: false}` xoá cứng mọi key khớp | bị từ chối, nêu `behavior: {type: version, keepAmount: 0}` là dạng xoá mọi revision (§8.6) | phán quyết của người dùng rằng `PurgeKV` xoá "chỉ cái hết hạn, hoặc theo version"; xoá toàn bộ có một cách viết tường minh, như ở W9b |
| `PurgeKV` `behavior: {type: version}` không có cả `keepAmount` lẫn `before`: **đo ở Task 1** | bị từ chối, nêu cả hai thuộc tính (§8.6) | quy tắc của W9b (2026-10-06, review Task 7 C2), áp dụng ở đây theo quyết định của người dùng |
| bulk `DELETE` bỏ qua lỗi IO từng key | một transaction, tất cả hoặc không gì cả; câu trả lời liệt kê các key đã còn sống (§9) | một câu trả lời nói "đã xoá" nghĩa là đã xoá |
| `RunContext.namespaceKv` phục vụ plugin | plugin host từ chối nó với một lý do chỉ tới các task `core.kv.*` (§10) | quyết định của người dùng (2026-10-07): không plugin nào trong catalog gọi nó |

## 3. Bố cục storage và key

- **Key của object:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_kv/{key}.ion` cho revision 1, `…/{key}.ion.v{N}` cho N > 1 (`kvFileName` của Kestra). Cùng bucket và prefix với W9a và W9b. Workspace luôn đến từ grant (§5), không bao giờ từ đầu vào. `FileStore` có thêm `namespace_kv_uri(grant, key, revision)` bên cạnh `namespace_object_uri`. **Không URI nào gọi tên một giá trị KV:** một flow chỉ tới được giá trị qua `kv()` và các task, nên `storage::uri` không đổi.
- **Quy tắc segment `_kv`.** Một namespace có segment `_kv` (`a._kv`, hợp lệ theo `is_valid_namespace`) sẽ đặt key execution của flow và namespace file của nó dưới tiền tố KV của namespace `a`. `access::check_namespace` có thêm một mệnh đề bên cạnh `_files` (một `check_kv_segment`), nên `authorize` từ chối namespace như vậy cho mọi thao tác KV và mọi thao tác namespace-file, và publish từ chối nó như từ chối `_files`, mỗi lỗi nêu namespace và segment. Task 1 đếm các flow đã publish trong một database dev và các mục corpus có namespace chứa segment `_kv`, và liệt kê từng cái; dự kiến không có.
- **Quy tắc key**, một hàm (`kv::key::check_key`) cho mọi lối vào (hàm, task, API): regex của Kestra `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` (`KVStore.java:137`) và tối đa **350 byte**. Thông báo của Kestra cho key rỗng và key sai regex được tái tạo như Task 1 đo; lỗi về độ dài nêu giới hạn. **Vì sao 350:** đó là độ rộng cột của chính Kestra (`kv_metadata.name VARCHAR(350)`), nên mọi key Kestra giữ được, egret giữ được. Theo regex, key là ASCII, nên số byte bằng số ký tự. Một key không bao giờ chứa `/`, nên nó là một segment của key object, và không bao giờ bắt đầu bằng `_`, nên không key nào mang tên giống một dấu mốc.
- **Ngân sách key.** Sau prefix, một key object KV chứa tối đa 36 (workspace) + 1 + 150 (namespace) + 1 + 3 (`_kv`) + 1 + 350 (key) + 4 (`.ion`) + 12 (`.v` và một revision) = 558 byte. Quy tắc lúc boot của W9b (prefix tối đa 200 byte) vì vậy cũng giữ mọi key KV dưới 1024 byte của S3; không cần kiểm tra mới.
- **Staging** dùng lại của W9b: các byte đã mã hoá của một giá trị đi tới `{prefix}{ws}/_nsfiles_staging/{uuid}`, được ghi trong `workflow.namespace_file_staging` (`staging::record`, `forget`), và được đợt quét mỗi giờ sẵn có quét (`spawn_staging_sweeper`). Bảng và segment giữ tên W9b của chúng.

## 4. Lõi

### 4.1 Bảng metadata

`workflow.kv_entries` (migration 0175, kiểm lại khi rebase):
- `id bigserial` khoá chính; `workspace_id uuid` FK tới workspace, `ON DELETE CASCADE`; `namespace text`; `key text` (quy tắc key dưới dạng CHECK: độ dài ≤ 350 và regex); `revision int` (≥ 1).
- `value_type text`, CHECK là một trong `STRING`, `NUMBER`, `BOOLEAN`, `DATETIME`, `DATE`, `DURATION`, `JSON`; `size bigint` (số byte của object).
- `description text` (nullable); `expires_at timestamptz` (nullable; null thì không bao giờ hết hạn).
- `deleted bool` mặc định false; `deleted_at timestamptz` và `deleted_by_user`, `deleted_by_execution`, `deleted_by_task_run` (nullable, đặt cùng lúc với `deleted`).
- `created_at timestamptz` mặc định `now()`; `created_by_user`, `created_by_execution`, `created_by_task_run` với CHECK một-người-ghi của W9b (`CreatedBy::{User, Execution, TaskRun}`).
- `UNIQUE (workspace_id, namespace, key, revision)`; một index trên `(workspace_id, namespace, key, revision DESC)` cho dòng mới nhất; một index một phần trên `expires_at` với `expires_at IS NOT NULL` cho đợt quét.

Các cột `deleted_by_*` vượt ra ngoài danh sách cột người dùng đã nêu: chúng cho một `Delete` được claim lại sau sự cố nhận ra lần xoá trước của chính nó (§8.4, §11). **Một giá trị KV chỉ tồn tại khi dòng của nó tồn tại.** Dòng mới nhất của một key là revision cao nhất của nó; dòng đó là trạng thái của key.

### 4.2 Giá trị và type

- **Mã hoá.** egret ghi mỗi giá trị như Kestra: một DURATION dưới dạng các byte chuỗi ISO-8601 nguyên bản, mọi type khác dưới dạng Ion **text**. Các byte chính xác theo từng type được **đo ở Task 1** (STRING, NUMBER (số nguyên, thập phân), BOOLEAN, DATETIME, DATE, DURATION, object và mảng JSON), và bộ mã hoá của egret được kiểm với các byte đó, nên một bucket egret ghi đọc giống của Kestra. Plan chọn crate Ion.
- **Type.** Type được lưu trong `value_type`: `kvType` đã cho, hoặc, nếu không có, type của giá trị đã render có kiểu (chuỗi → STRING, số → NUMBER, boolean → BOOLEAN, object hay mảng → JSON). `PUT` của API suy nó từ giá trị Ion đã parse như `KVType.from` sẽ làm (§9). Một lần đọc trả giá trị theo kiểu của `value_type`, không bao giờ bằng cách suy lại (§2).
- **Đọc về.** `kv()` và `Get` trả STRING, NUMBER, BOOLEAN và JSON thành giá trị JSON tương ứng. DATETIME, DATE và DURATION không có type JSON trong render của egret; chúng trả về dưới dạng các chuỗi mà `kv()` của Kestra in ra cho chúng, được **đo ở Task 1** (ví dụ, một DURATION ghi là `P1D` in ra `P1D` hay `PT24H`). Một khác biệt mà fixture cho thấy là một dòng §2.
- **Kích thước** *(phán quyết của người dùng, 2026-10-07)*. Object đã mã hoá tối đa `max_read_bytes` (mặc định 10 MiB), trên mọi đường ghi: `Set`, `Put` (kết quả đã merge của nó) và `PUT` của API. Một giá trị lớn hơn là lỗi tác giả nêu `max_read_bytes` (413 trên API), và không gì được ghi. Một lần đọc giải mã giá trị vào bộ nhớ (`kv()`, `Get`, merge của `Put`, `GET` của API) bị chặn bởi cùng trần đó, nên mọi giá trị đã lưu đều đọc được. `max_file_bytes` không áp dụng cho giá trị KV. Lần đọc vẫn kiểm giới hạn, và một object đã lưu vượt nó là lỗi tác giả nêu trần; chỉ một object ghi trước khi trần bị hạ mới gây ra được.

### 4.3 Ghi

Mọi lần ghi (`Set`, `Put`, `PUT` của API) có hình dạng này. Bài học của W9b giữ nguyên: **không giữ kết nối database và lock nào qua một body do người gọi điều nhịp.**

1. **Kiểm** key (§3), namespace và quy tắc ghi (§5) qua `authorize(…, Resource::Kv, Access::Write)`, rồi render và gán kiểu cho giá trị (một task, §8.1) hoặc parse nó (API, §9). Mã hoá nó (§4.2) và kiểm kích thước của nó với `max_read_bytes`, trước khi stage bất cứ gì.
2. **Stage.** Ghi một dòng staging (`staging::record`) và đặt các byte đã mã hoá vào staging object của nó, không giữ kết nối và lock nào.
3. **Commit**, trên một task tách rời như `commit_staged` của W9b, để một request bị bỏ hay một future bị huỷ vẫn hoàn tất. Trong một transaction:
   - lấy `pg_advisory_xact_lock(hashtextextended('w9c:{ws}:{ns}:{key}', 0))`, dưới `SET LOCAL lock_timeout` khi grant có `lock_wait` (5 s của API; một flow chờ bao lâu cũng được);
   - đọc dòng mới nhất;
   - áp dụng bỏ qua khi claim lại: khi chính task run này đã ghi revision mới nhất (`created_by_task_run`), trả "đã ghi" và không ghi gì (§11);
   - áp dụng `overwrite: false`: một dòng mới nhất còn sống, chưa hết hạn là một xung đột nêu key; một dòng không có, đã xoá mềm hay đã hết hạn thì không;
   - đặt `revision = max + 1` (1 cho key mới);
   - copy staging object vào key của revision phía server (`ObjectStore::copy`);
   - `head` nó, kích thước phải bằng kích thước đã stage;
   - chèn dòng;
   - commit.
4. **Quên** staging object và dòng của nó trên mọi lối ra (đã ghi, bỏ qua, bị từ chối, thất bại).

`Put` bỏ bước 2 và khác ở bước 3: nó đọc, merge và ghi dưới lock (§8.5). Một `Delete` lấy cùng lock và cập nhật dòng mới nhất (§4.4). Lock tuần tự hoá mọi người ghi của một key, và khoảng thời gian nó bao do server điều nhịp: một lần copy phía server, hoặc lần đọc và ghi có giới hạn của `Put`.

### 4.4 Revision và xoá mềm

Theo R1 của W9b:
- Mỗi lần ghi tạo một revision mới; các revision trước và object của chúng ở lại.
- `Delete` đánh dấu revision mới nhất là `deleted` (kèm `deleted_at` và `deleted_by_*`), dưới lock của key. Không thêm revision, như Kestra.
- Đọc bản mới nhất của một key đã xoá mềm là không có. Đọc một revision chỉ rõ (`GET …/kv/{key}?revision=N`) của một key đã xoá mềm thành công, trừ khi revision đó đã hết hạn (§4.5).
- `GET …/kv/{key}/revisions` liệt kê mọi revision, đánh dấu cái đã xoá.
- Một lần ghi sau khi xoá tạo revision `max + 1`, còn sống.
- Revision và object của chúng ở lại tới khi `PurgeKV` hay đợt quét hết hạn xoá chúng.

### 4.5 Đọc và hết hạn

- **Còn sống** nghĩa là dòng mới nhất không bị xoá và `expires_at` là null hoặc muộn hơn `now()` của database. Mọi lần đọc bản mới nhất (`kv()`, `Get`, `GetKeys`, list, get và inheritance của API) chỉ trả lời từ các dòng còn sống.
- **Hết hạn nghĩa là không có.** Một key hết hạn đọc y như một key chưa từng được ghi: `kv()` và `Get` tuân theo `errorOnMissing`, `GetKeys` và các danh sách bỏ nó ra, `Set` với `overwrite: false` được ghi đè lên nó. Đọc một revision chỉ rõ đã hết hạn cũng là không tìm thấy.
- **Đọc không bao giờ ghi.** Không gì trên đường đọc chèn, cập nhật hay xoá; đợt quét (§11) và `PurgeKV` xoá các key hết hạn.
- **Hạn được cố định lúc ghi:** `expires_at = now() + ttl`, theo đồng hồ của database trong transaction của lần commit. `Put` giữ `expires_at` và description của revision mới nhất (Kestra, `Put.java:121`).
- Một lần đọc phân giải dòng còn sống mới nhất, rồi đọc object của revision đó (chặn bởi `max_read_bytes`, §4.2). **Một object thiếu phía sau một dòng là lỗi tác giả nêu key và revision**, không có lùi về (quy tắc của W9b). Store hay database không với tới được là lỗi hạ tầng (§11).

### 4.6 Kế thừa

- `kv(key)` không có `namespace`, và `Get` có `namespace` đã render bằng namespace của chính flow đang chạy, tìm key trong namespace của flow rồi trong từng cha, cắt tại dấu chấm (`a.b.c` → `a.b` → `a`), và trả lời từ namespace đầu tiên mà key còn sống. Một key hết hạn hay đã xoá mềm ở một tầng là không có ở đó, và lần đi tiếp tục.
- Một `kv(key, namespace)` ghi rõ thì không đi, kể cả khi namespace là của chính flow (`KvFunction.java:64-66`). `Get` với bất kỳ namespace nào khác không đi. `GetKeys`, `Delete`, `Set`, `Put` không bao giờ đi.
- Lần đi đọc mỗi tầng qua `authorize(Read)`, mà mọi namespace của workspace đều qua. Một truy vấn đọc dòng mới nhất của mọi tầng (tối đa số segment của namespace, ≤ 75); một object được đọc.
- Khi không tầng nào có key, thông báo không-có-key nêu namespace mà thông báo của Kestra nêu, điều Task 1 đo; egret tái tạo nó.

### 4.7 Một hàm truy cập

`nsfiles::access::authorize` của W9b có thêm một loại tài nguyên: `authorize(workspace, actor, target, resource: Resource::{NamespaceFiles, Kv}, access)`. Quy tắc giống nhau cho cả hai loại (§5); tài nguyên chỉ đổi lời của lần từ chối ("namespace files" hay "the KV store"), và `Grant` ghi lại nó. Mọi hàm store KV nhận một `Grant` và khẳng định tài nguyên và quyền của nó (`require`), nên một grant namespace-file không chạm được KV và một grant Read không ghi được. Quy tắc truy cập ở trong một module duy nhất, `nsfiles::access`, cho cả hai gói.

## 5. Ai được đọc và ghi

- **Workspace:** luôn là của execution đang chạy (với API, `{ws}` của route sau khi kiểm quyền). Không gì trong một thuộc tính hay tham số gọi tên một workspace.
- **Đọc** (`kv()`, `Get`, `GetKeys`, lần đi kế thừa): mọi namespace của workspace. Một namespace không có KV đơn giản là không có key.
- **Ghi** (`Set`, `Put`, `Delete`): chỉ namespace của chính flow đang chạy, so sánh chính xác.
- **Purge** (`PurgeKV`): namespace của chính flow và các namespace con của nó (`ns == own`, hoặc `ns` bắt đầu bằng `own + "."`).
- **API:** mọi namespace của workspace của route, sau `workflow:read` (đọc) hoặc `workflow:manage` (ghi) (§9).
- Một lần từ chối là lỗi tác giả nêu namespace và quy tắc; nó không bao giờ nói một key có tồn tại ở đó không. Mỗi lần từ chối có một test thất bại khi nó bị bỏ đi.

## 6. Hàm `kv()`

- **Chữ ký như Kestra:** `kv(key, namespace?, errorOnMissing?)`, theo vị trí hoặc theo tên (`KvFunction.java:34-36`). `errorOnMissing` mặc định **true** (`:53`). Thiếu `key` là lỗi `The 'kv' function expects an argument 'key'.`
- **Tra cứu:** không có `namespace` thì namespace của flow có kế thừa; có thì chỉ namespace đó (§4.6). Giá trị trả về có kiểu (§4.2). Không có (chưa từng ghi, đã xoá mềm, hết hạn) với `errorOnMissing: true` là lỗi mang thông báo của Kestra như Task 1 đo; với `false` là `null`.
- **Kết nối:** một kết nối namespace của lần render (`storage/scope.rs`: mở ở lần tra namespace đầu tiên từ `nsfiles::db_options()`, đóng khi render kết thúc), dùng chung với các hàm file của W9b. Một lần connect hay truy vấn thất bại đặt cờ `infrastructure_failure` của scope, nên run được claim lại mà không tốn attempt (§11). Lần tra cứu và lần đọc object đi qua lần đọc có giới hạn không-kẹt của W9a (`off_the_runtime`).
- **Không snapshot.** KV commit bên ngoài `settle`, như namespace file, nên một lần render thấy mọi dòng đã commit, và một key ghép lúc render vẫn chạy.
- **Ở đâu:** mọi lần render có execution, gồm `runIf`, điều kiện `If`/`Switch`, `Log` và thuộc tính của task. Một lần render không có execution (điều kiện của trigger, publish) từ chối `kv()` theo tên, như các hàm của W9b. Việc đó có khớp ô `function-kv` của oracle biểu thức không được **đo ở Task 1**.
- `kv` rời `REFUSED` (`functions.rs:276`), còn sáu; `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven` được thay bằng một test ghim sáu tên và một test chứng minh một lần đọc.

## 7. Secret trong giá trị KV

- **Lúc publish**, một `Set` hay `Put` có `value` gọi `secret(…)` ở bất kỳ đâu trong template của nó bị từ chối với một mã mới, `secret_in_kv_value`. Thông báo nêu task và thuộc tính và nói: *giá trị của secret sẽ được lưu trong KV store, nơi bất kỳ ai đọc được KV của namespace này (`workflow:read`) đều đọc được nó, không che; hãy truyền secret cho task cần nó thay vào đó.* Nó được báo **thay cho** `CODE_SECRET_OUTSIDE_PLUGIN_TASK` chung cho thuộc tính đó, để tác giả đọc đúng lý do áp dụng.
- Lần quét là lần quét `secret()` sẵn có của publish (`spec/validate.rs:395-403`), trỏ vào `value` của hai task. Lần từ chối tự đứng được: nó vẫn giữ nếu sau này `secret()` được phép bên ngoài các task `kestra.*`.
- `PUT` của API không bị ảnh hưởng: body của nó là một giá trị literal mà người gọi đã giữ sẵn.
- Một giá trị chạm tới secret gián tiếp (một biến hay một output chứa secret) không phát hiện được lúc publish; lúc chạy, `secret()` vẫn bị giới hạn (`SECRET_REFUSAL`), nên không secret đã render nào tới được giá trị KV qua các lần render của egret.

## 8. Các task `io.kestra.plugin.core.kv`

Handler native trong worker, như các task `core.namespace` của W9b; mỗi cái có một dòng trong `spec/registry.rs`. Thuộc tính và output theo các lớp của artifact; Task 1 trích từng schema và đo từng hành vi. `namespace` mặc định là của chính flow trên mọi task.

### 8.1 `Set`

- `key` (bắt buộc, quy tắc key); `value` (bắt buộc); `namespace` (quy tắc ghi, §5); `overwrite` (mặc định true); `ttl` (duration ISO-8601; giá trị âm là lỗi tác giả, và hành vi của số không được **đo ở Task 1**); `kvType` (một trong bảy type); `kvDescription`.
- **Render có kiểu**, như `renderTyped` của Kestra (`Set.java:111`): một giá trị là một biểu thức đơn giữ type đã render của nó (`{{ 1 + 1 }}` là NUMBER); mọi giá trị khác là chuỗi. Những template nào trả về có kiểu được **đo ở Task 1**.
- **Ép kiểu** một String theo `kvType`, như artifact: NUMBER thành số JSON; BOOLEAN chặt, `true`/`false` không phân biệt hoa thường, mọi thứ khác là thông báo của artifact; DATETIME thành một instant; DATE thành một ngày địa phương, lùi về ngày UTC của một instant; DURATION được kiểm và giữ như đã viết; JSON được parse. Một Number với `kvType: STRING` thành chuỗi của nó. Các trường hợp lệch khác (một Boolean với `kvType: NUMBER`, một giá trị JSON với `STRING`) được **đo ở Task 1** và tái tạo. Ép kiểu thất bại là lỗi tác giả mang thông báo đã đo.
- Giá trị đã mã hoá tối đa `max_read_bytes`; một giá trị lớn hơn làm task thất bại, nêu trần, trước khi stage bất cứ gì (§4.2).
- Ghi theo §4.3; `overwrite: false` lên một key còn sống làm task thất bại, nêu key (thông báo là của Kestra, **đo ở Task 1**). Output: không có, như `VoidOutput` của Kestra.

### 8.2 `Get`

`key`, `namespace` (quy tắc đọc; kế thừa khi bằng namespace của flow, §4.6), `errorOnMissing` (mặc định false). Output `{value}`: giá trị có kiểu, hoặc `null` khi không có và `errorOnMissing` là false; không có với `true` làm task thất bại với thông báo của Kestra (`Get.java:91`).

### 8.3 `GetKeys`

`prefix` (tuỳ chọn; một `startsWith` đơn giản trên key), `namespace` (quy tắc đọc; không kế thừa). Output `{keys}`: các key còn sống, theo thứ tự mà Task 1 đo của Kestra. Tối đa `max_namespace_files` key; nhiều hơn làm task thất bại, nêu trần và số đếm, trước khi có output. Danh sách chỉ đọc dòng, không bao giờ đọc object.

### 8.4 `Delete`

`key`, `namespace` (quy tắc ghi), `errorOnMissing` (mặc định false). Dưới lock của key, một dòng mới nhất còn sống được đánh dấu xoá (§4.4) và output là `{deleted: true}`; một key không có hoặc hết hạn cho output `{deleted: false}`, hoặc thất bại với thông báo của Kestra khi `errorOnMissing` là true. **Claim lại:** khi dòng mới nhất đã bị chính task run này xoá (`deleted_by_task_run`), output là `{deleted: true}` và không ghi gì (§11).

### 8.5 `Put`

- `key`, `value` (mọi type), `namespace` (quy tắc ghi), `errorOnMissing` (mặc định false).
- **Quy tắc merge** theo artifact (`Put.java:72-121`): một map deep-merge vào một map; một list các mục `{key, value}` vá các đường dẫn có chấm; một mục `[{value}]` đơn thay cả giá trị. Ngữ nghĩa chính xác được **đo ở Task 1** và ghim bằng các ô fixture: map lồng nhau, mảng trong map, xung đột type giữa một map và một scalar, một key không có với từng `errorOnMissing`, và một đường dẫn có chấm đi qua một tầng không có. egret cài theo fixture.
- **Dưới lock của key.** `Put` chạy bước 1 của §4.3 cho bản vá của nó và không stage gì, vì các byte đã merge chỉ tồn tại dưới lock. Rồi, trong transaction commit, giữ lock:
  - đọc dòng còn sống mới nhất, và object của nó (chặn bởi `max_read_bytes`);
  - merge;
  - kiểm kích thước đã mã hoá của kết quả merge với `max_read_bytes`: một kết quả lớn hơn làm task thất bại, nêu trần, và không gì được ghi;
  - ghi object đã merge thẳng vào key của revision;
  - `head` nó;
  - chèn dòng, giữ description và `expires_at` của revision mới nhất, với type của giá trị đã merge;
  - commit.

  Lock bao một lần đọc store có giới hạn do server điều nhịp và một lần ghi store, không bao giờ một body của người gọi.
- **Vì sao merge chạy dưới lock** thay vì lạc quan bên ngoài: với một vòng lặp thử lại, một key bận sẽ làm một flow thất bại vì nhịp của người ghi khác, điều W9b loại trừ cho flow. Bỏ qua khi claim lại áp dụng như với `Set`. Output: không có.

### 8.6 `PurgeKV`

- **Namespace:** `namespaces` xor `namespacePattern` (ngữ pháp wildcard của Kestra, matcher Commons-IO trong `glob` của W9b); mặc định là namespace của chính flow. `includeChildNamespaces` mặc định true và khớp theo ranh giới dấu chấm. Ứng viên chỉ là các namespace có KV là của chính flow hoặc con của nó, nên một pattern không bao giờ chạm tới namespace khác. Một namespace được liệt kê nằm ngoài quy tắc purge bị từ chối theo tên trước khi xoá bất cứ gì. Tối đa 64 namespace được liệt kê hay được phủ (`MAX_NAMESPACES` của W9b).
- **`keyPattern`:** cùng ngữ pháp wildcard, trên key.
- **`behavior`:**
  - `{type: key, expiredOnly: true}` (mặc định) xoá mọi revision của mỗi key khớp có revision mới nhất đã hết hạn.
  - `{type: key, expiredOnly: false}` bị từ chối. Lỗi nêu `behavior: {type: version, keepAmount: 0}` là dạng xoá mọi revision.
  - `{type: version, keepAmount, before}` giữ `keepAmount` revision cao nhất của mỗi key khớp, và xoá các revision tạo vào hoặc trước `before`, không bao giờ cái mới nhất. Một key đã xoá mềm bị xoá hoàn toàn (quy tắc của W9b). `keepAmount: 0` xoá mọi revision. Một behaviour `version` không có cả hai thuộc tính bị từ chối, nêu cả hai và nói rằng `keepAmount: 0` xoá tất cả.
- **Thực thi, như `PurgeFiles` của W9b:**
  - Dòng được đọc theo trang keyset `max_namespace_files + 1` theo `(key, revision)`.
  - Các key trọn vẹn của mỗi trang được xét và xoá theo từng key, dưới lock session của key (`LockedSession`). Mỗi lần xoá xoá dòng theo id, commit, rồi xoá object, rồi mở khoá. Lock vẫn được giữ khi xoá object, nên một người ghi không thể dùng lại key object của một số revision vừa được giải phóng ở giữa.
  - Một key giữ nhiều revision hơn trần làm task thất bại, nêu nó, với các key trước nó đã được xoá.
  - Lần purge chạy tới cùng trên một task tách rời.
- **Output** `{size}`: số đếm Kestra báo, revision hay key, **đo ở Task 1**.

## 9. API KV

Dưới `/api/ws/{ws}/workflows/namespaces/{ns}/kv`, phản chiếu `KVController`, với quy ước route của W9b (`WsContext`, rồi `egret_core::authz::require` với `WF_READ` hay `WF_MANAGE`, rồi `authorize(…, Actor::Api, …)`; `no_store()` trả 503 khi không có `[storage]`; mỗi lần ghi phát một audit mutation `workflow_namespace_kv:{ns}/{key}`; không có quyền mới).

| route | quyền | trả lời |
|---|---|---|
| `GET …/kv` | `workflow:read` | các key còn sống của namespace, mỗi cái có `key`, `type`, `size`, `description`, `expiresAt`, `revision`, `updated`; tối đa `max_namespace_files`, nếu không thì 413 |
| `GET …/kv/inheritance` | `workflow:read` | chỉ các key còn sống của tổ tiên, sâu nhất trước, lần xuất hiện đầu của một key thắng (Kestra, `:73-100`), mỗi cái kèm namespace của nó; cùng giới hạn |
| `GET …/kv/{key}?revision` | `workflow:read` | `{type, value, revision, updated, description, expiresAt}`, giá trị có kiểu (§4.2); đọc bản mới nhất của một key không có, đã xoá mềm hay hết hạn là 404; một `revision` đã cho của một key đã xoá mềm được phục vụ, một revision hết hạn là 404 (§4.4, §4.5) |
| `GET …/kv/{key}/revisions` | `workflow:read` | mọi revision: `revision`, `type`, `size`, `description`, `expiresAt`, `created`, `deleted`, `expired`; 404 khi key không có dòng nào |
| `PUT …/kv/{key}` | `workflow:manage` | body `text/plain`, parse như Ion và lùi về chuỗi thô (Kestra, `:124-141`); header `description` và `ttl` (ISO-8601); luôn ghi đè; §4.3; 200 không có body |
| `DELETE …/kv/{key}` | `workflow:manage` | §4.4; `true` khi một key còn sống đã bị xoá, `false` khi không có (200 cả hai trường hợp) |
| `DELETE …/kv` | `workflow:manage` | body `{keys: […]}`, tối đa `max_namespace_files` key; một transaction lấy lock của từng key theo thứ tự tăng dần, tất cả hoặc không gì cả; `{keys: […]}` liệt kê các key đã còn sống và nay đã bị xoá |

- **Body của `PUT`** bị chặn bởi `max_read_bytes` (nó được parse trong bộ nhớ để gán kiểu, §4.2), và giá trị đã mã hoá cũng vậy (phán quyết của người dùng, 2026-10-07). Nó được đọc với các timeout body của W9b: không byte nào trong 30 giây, hoặc hơn 12 giờ tổng cộng, trả 408 và không ghi gì. Body được đọc và stage trước khi lấy lock nào.
- **Chờ lock:** một lần ghi qua API chờ lock của key tối đa 5 s (`API_LOCK_WAIT`), rồi trả 409 "the key is busy (another write holds it); try again", nêu key, không giữ gì.
- **Lỗi → status** (một ánh xạ cho mọi route, `ns_refusal` của W9b với các lỗi của KV):

| lỗi | status |
|---|---|
| quy tắc key, một namespace bị từ chối (không hợp lệ, segment rỗng, segment `_files` hay `_kv`), một `ttl` sai dạng hay âm, một `revision` không phải số nguyên, một body bulk sai dạng | 400 |
| không có dòng còn sống: một key không biết, đọc bản mới nhất của một key đã xoá mềm, một key hay revision đã hết hạn, một revision không biết, namespace của workspace khác | 404 |
| lock của key bị giữ lâu hơn 5 s | 409 |
| một body `PUT` hay giá trị đã mã hoá của nó vượt `max_read_bytes`, một giá trị đã lưu vượt `max_read_bytes` khi `GET`, một danh sách hay body bulk vượt `max_namespace_files` | 413 |
| một body đứng 30 s hay kéo dài quá 12 h | 408 |
| store hay database không với tới được | 503, ghi log với một thông báo cố định; lỗi của database không bao giờ được gửi đi |

Không thông báo nào nêu key storage hay workspace khác. Một namespace mà workspace khác giữ trả lời y như một namespace chưa từng chứa gì. **Không route GET nào ghi** (§4.5). utoipa mô tả mọi route; client frontend được sinh lại bằng `npm run gen:api`. UI là của W7.

## 10. Plugin host

- Không có cầu KV: `ShimRunContext.namespaceKv` tiếp tục ném `EgretUnsupported`, với một thông báo mới: *namespaceKv(): egret gives flows the KV store through the `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's RunContext; use a `core.kv` task before or after this one.* `EgretPluginHostTest.java:125-126` ghim thông báo mới, và các chỗ doc nhắc tới (`ShimRunContext.java:52`, `EgretUnsupported.java:4`, `EgretPluginHost.java:58`) bỏ "W9c".
- Thông báo exit-4 của handler (`worker/handlers/kestra.rs:379`) bỏ "the KV store — W9c": *the plugin needs a Kestra service egret does not provide to plugins*, theo sau là lý do của host, nêu `namespaceKv` và các task `core.kv`. Doc module (`:79`) và test ghim (`:1317-1321`) đi theo.
- Không gì được stage cho KV, nên `task.json` và kiểm tra số byte của task (`check_task_bytes`) không đổi.

## 11. Vòng đời và phân loại lỗi

- **Đợt quét hết hạn.** Mọi process egret có store khởi động `spawn_kv_expiry_sweeper` bên cạnh staging sweeper của W9b. Nó chạy mỗi giờ, lần đầu sau một giờ, trên mọi workspace:
  - Nó liệt kê, theo trang keyset 1000 theo row id, các key có dòng mới nhất với `expires_at <= now()` (index một phần).
  - Với mỗi key, nó lấy lock session của key bằng `pg_try_advisory_lock` và bỏ qua key khi nó bận; lượt sau thử lại.
  - Dưới lock nó đọc lại dòng mới nhất. Nếu dòng đó vẫn hết hạn, nó xoá các dòng của key theo các id đã đọc, commit, xoá object, rồi mở khoá.
  - Một key được ghi lại ở giữa giữ revision mới của nó.
  - Hai replica quét cùng lúc là vô hại: một key bận bị bỏ qua, và xoá theo id một dòng đã mất là không làm gì.
  - Một lần xoá object thất bại được ghi log; dòng của nó đã mất, nên object thành mồ côi (§15).
  - Tuổi tính theo đồng hồ của database.
- **Claim lại là idempotent.** Sau một lỗi hạ tầng, chính task run đó được claim lại.
  - `Set` và `Put` bỏ qua khi chính task run này đã ghi revision mới nhất (`created_by_task_run`, xét dưới lock của key, §4.3), nên lần thử lại không thêm revision và `overwrite: false` không bao giờ xung đột với lần ghi của chính nó.
  - `Delete` trả `deleted: true` khi chính task run này đã xoá revision mới nhất (§8.4).
  - `PurgeKV` tính lại từ các dòng như hiện có; `size` của nó đếm những gì lần claim lại tự xoá.
  - Lần bỏ qua dựa trên task run, nên nó cũng phủ một lần thử lại `RETRY_FAILED_TASK`, như của W9b.
- **Lần ghi của một flow chờ** một key bận bao lâu cũng được (quy tắc của W9b): cái nó chờ phía sau do server điều nhịp, hoặc một lần copy hoặc lần đọc và ghi có giới hạn của một `Put`. Một lần ghi qua API chờ 5 s (§9).
- **Phân loại lỗi** (KV dùng lại `NsError` của W9b và cách chia của nó):

| lỗi | loại | hậu quả |
|---|---|---|
| quy tắc key, một namespace bị từ chối, một lần từ chối theo phạm vi ghi, một `ttl` sai dạng, một lần ép `kvType` thất bại, một hình dạng `Put` mà merge từ chối, `expiredOnly: false`, một purge `version` không có thuộc tính nào, vượt một trần (`max_read_bytes`, `max_namespace_files`, 64 namespace), `overwrite: false` lên một key còn sống, không có với `errorOnMissing: true`, một object thiếu phía sau một dòng | tác giả (`Author`, `NotFound`, `Conflict`, `TooLarge`) | task thất bại với thông báo nêu key, namespace hay trần; API trả 4xx |
| `secret()` trong giá trị `Set`/`Put` | tác giả, lúc publish | flow không được publish (§7) |
| store hay database không với tới được, lúc chạy hay lúc render | hạ tầng (`Store`, `Db`) | handler trả `Err`, hoặc lần render đặt `infrastructure_failure`; run được claim lại mà không tốn attempt; API trả 503 |

Không lỗi nào nêu một secret, một key storage hay workspace khác.

## 12. Task

1. **Đo Kestra** vào `oracle-exec/w9c-kv.json`, theo định dạng hợp đồng của W9b: câu trả lời của egret ở cấp cao nhất và của Kestra dưới `kestra`, với `chosen_cells` ở chỗ egret cố ý khác và các tiền đề mà mỗi ô dựa vào. Các ô:
   - schema của từng task (thuộc tính, mặc định, bắt buộc, output);
   - vòng đi-về của type giá trị qua `Set` → `kv()`/`Get`/API cho mọi type, gồm DATE, DATETIME, DURATION và JSON, và các byte Ion được lưu;
   - các lần ép kiểu của `Set` và thông báo của chúng (BOOLEAN chặt, các trường hợp lệch ở §8.1), render có kiểu, `ttl` bằng không và âm;
   - merge của `Put` (§8.5);
   - TTL: đọc một key hết hạn bằng từng cách đọc với từng `errorOnMissing`, và dòng và object trông thế nào sau đó;
   - kế thừa: `kv()` có và không có `namespace`, `Get` với namespace của flow, một tầng hết hạn, và các thông báo không-có-key;
   - các behaviour của `PurgeKV`, `size`, và một purge `version` không có thuộc tính nào;
   - các route và body của API (`KvDetail`, parse Ion và lùi về của PUT, cả hai lần xoá, `inheritance`);
   - cây storage sau các lần ghi, một lần xoá, một lần ghi lại và một lần purge;
   - số đếm và danh sách flow và mục corpus có segment namespace `_kv`;
   - `function-kv` trong oracle biểu thức;
   - các baseline (test, clippy, ratchet ở 108).
2. **Bảng, truy cập, đọc:** migration 0175, `check_key`, quy tắc segment `_kv` (publish và `check_namespace`), `Resource::Kv` trong `authorize`, lần đọc còn-sống-mới-nhất của store, lần đi kế thừa, danh sách, revisions.
3. **Ghi, staging, purge, quét:** lần ghi §4.3 trên staging của W9b, xoá mềm, merge có lock của `Put`, purge từng key theo row id với phân trang keyset, `spawn_kv_expiry_sweeper`; các test đồng thời.
4. **`kv()` và lần từ chối secret:** hàm trên kết nối của render, `REFUSED` còn sáu, `secret_in_kv_value` lúc publish.
5. **Sáu task:** `Set`, `Get`, `GetKeys`, `Delete`, `Put`, `PurgeKV`, với các dòng registry của chúng, theo fixture.
6. **API KV:** route, ánh xạ lỗi, utoipa, `npm run gen:api`.
7. **Các thông báo của plugin host** (§10).
8. **Rebase, end to end, docs, CI:** rebase lên `origin/main` và kiểm lại số migration; end to end trên một cụm kind tạm; ratchet trước và sau; dòng W9 của spec chương trình và một đính chính W9c; `oracle-exec/README.md`; full CI cục bộ.

**Ràng buộc toàn cục cho mọi task** (bài học của W9b):
- Với mọi đường ghi, review kiểm, và báo cáo của task nêu, rằng không kết nối database và lock nào được giữ qua một body do người gọi điều nhịp. Các khoảng do server điều nhịp (một lần copy, lần đọc và ghi có giới hạn của `Put`, các lần xoá object của một purge) được nêu tên ở chỗ chúng xảy ra.
- Test phá huỷ (delete, purge, quét) chỉ chạy trên `MemStore` hoặc dưới một prefix dùng-một-lần trên MinIO, không bao giờ trên gốc của bucket dùng chung.
- Mỗi branch dùng Postgres, Valkey và MinIO riêng của nó.
- Ratchet không được thoái lui: 108 lệch trước và sau. `retry-with-flowable-errors` còn cần flowable `AllowFailure`, nên riêng W9c không đưa được nó vào, và `valids/kv` không có mục golden.

## 13. Kiểm thử

- **Unit:** quy tắc key (mỗi lần từ chối có một test thất bại khi nó bị bỏ, 350 được nhận và 351 bị từ chối); các quy tắc segment `_kv` và `_files`; `authorize` với `Resource::Kv` (ghi của chính mình, đọc namespace khác, ghi namespace khác bị từ chối, purge con, `a.bc` không phải con của `a.b`, một grant namespace-file bị một hàm store KV từ chối); mã hoá Ion so với byte của fixture theo từng type; các lần ép kiểu của `Set`; merge của `Put` so với các ô của fixture; thứ tự của lần đi kế thừa.
- **Postgres:**
  - Hai `Set` đồng thời với `overwrite: false` trên một key: đúng một cái ghi.
  - Hai `Put` đồng thời merge các trường khác nhau: cả hai trường còn lại, trong hai revision.
  - Đánh số revision dưới các người ghi đồng thời.
  - Xoá mềm rồi ghi lại; đọc revision chỉ rõ.
  - Hết hạn: một lần đọc sau `expires_at` là không có và không ghi gì (số dòng và danh sách object không đổi).
  - Đợt quét xoá một key hết hạn và giữ một key được ghi lại giữa lúc liệt kê và lúc lấy lock.
  - Hai đợt quét song song.
  - `PurgeKV` theo từng behaviour, các lần từ chối, và phân trang qua hơn một trang.
  - Bỏ qua khi claim lại cho `Set`, `Put`, `Delete`.
  - Cô lập workspace.
- **Test phá huỷ** chỉ trên `MemStore` hoặc một prefix MinIO dùng-một-lần (§12).
- **Render:** `kv()` trong `runIf`, `If`, `Switch` và một thuộc tính task; một sự cố đặt `infrastructure_failure` và claim lại; một lần render không có execution từ chối nó theo tên.
- **Publish:** `secret()` trong giá trị `Set` và `Put` bị từ chối với `secret_in_kv_value` và thông báo của nó; một namespace `_kv` bị từ chối; `valids/kv.yaml` vẫn publish được.
- **API:** `workflow:read` so với `workflow:manage`; mọi dòng của bảng status; 409 bận sau 5 s; các giới hạn body và các 408; 404 cho namespace của workspace khác, giống từng byte với câu trả lời của một namespace rỗng.
- **Plugin host:** thông báo mới của `namespaceKv` và exit-4, được ghim.
- **Oracle:** các ô egret của fixture chạy lại với bản build; ratchet đo trước và sau và ghi vào `oracle-exec/README.md`.
- **End to end** trên một cụm kind tạm: một flow `Set`, đọc bằng `kv()` và `Get`, `Put`, `Delete` và purge; một TTL hết hạn và được quét; API từ bên ngoài.

## 14. Ngoài phạm vi

Một cầu KV cho plugin (§10); một danh sách phân trang xuyên namespace (`GET /kv` qua các namespace); xoá namespace và quyền theo namespace; UI (W7); `AllowFailure` (và vì thế việc đưa `retry-with-flowable-errors` vào); che giá trị KV trong log hay output.

## 15. Giới hạn đã biết

- Một flow chỉ ghi và xoá KV của namespace của chính nó; một flow Kestra `Set` key của namespace khác thất bại trên egret, nêu namespace (§5).
- `PurgeKV` không có `namespaces` xoá namespace của chính flow và các con của nó, không phải mọi namespace; `expiredOnly: false` bị từ chối để dùng `version` với `keepAmount: 0` (§8.6).
- `secret()` trong giá trị `Set` hay `Put` bị từ chối lúc publish (§7).
- Một giá trị KV tối đa `max_read_bytes` (mặc định 10 MiB), trong khi Kestra không có trần; một `Set`, `Put` hay `PUT` của API với giá trị lớn hơn thất bại, nêu trần (§4.2; phán quyết của người dùng, 2026-10-07).
- Giá trị DATETIME, DATE và DURATION trả về dưới dạng chuỗi trong render của egret, nên một template so sánh chúng là so sánh chuỗi (§4.2).
- Mẫu đọc-tăng-`Set` (`retry-with-flowable-errors`) có race, như ở Kestra; `Put` là dạng không race cho một map, và không có phép tăng nguyên tử.
- Một key hết hạn giữ dòng và object của nó tới đợt quét kế tiếp, tới khoảng một giờ; các lần đọc đã coi nó là không có (§4.5).
- Một lần xoá object thất bại của đợt quét để lại một object mồ côi, mà sau đó không gì liệt kê, như các lần purge của W9a và W9b.
- Xoá một workspace cascade xoá các dòng `workflow.kv_entries` của nó và để lại các object `_kv` của nó trong storage, như namespace file của W9b bị để lại.
- Lúc render, mọi lỗi database của một lần tra `kv()` là sự cố, được claim lại mà không tốn attempt, và một lần tra chỉ bị chặn bởi TCP timeout của hệ điều hành; đây là giới hạn render của W9b, dùng chung (W9b §15).
- Một namespace có segment `_kv` bị từ chối (§3).
