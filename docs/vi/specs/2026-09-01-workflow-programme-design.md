# Workflow — Thiết kế chương trình

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-01
Bản tiếng Anh là bản chính: `docs/en/specs/2026-09-01-workflow-programme-design.md`
Thay thế: hai mục nav placeholder `Workflows` và `Pipelines` (`App.tsx:206-207`, `Sidebar.tsx:50,54`)

## 1. Mục tiêu

Workflow trở thành một khu vực hạng nhất của egret, có nhóm nav riêng như Data
API — không phải một trang nằm trong module khác. Nó điều phối công việc trên
chính hạ tầng egret (SQL trên Trino/StarRocks, bảng Iceberg, pod notebook/Daft,
endpoint Data API, report) và về sau là sang hệ thống bên thứ ba.

Ba quyết định đã chốt với người dùng trước spec này:

1. **Điều phối dữ liệu trước** (ELT), tự động hoá nghiệp vụ sau, trên **một**
   engine và **một** mô hình task.
2. **YAML là nguồn sự thật**, soạn trong editor có schema; đồ thị chỉ để đọc.
   Không phải canvas kéo-thả.
3. **Engine viết bằng Rust bên trong egret**, mô phỏng có chủ đích theo Kestra,
   thay vì tích hợp Kestra/Airflow/n8n như một dịch vụ.

## 2. Vì sao không tích hợp engine có sẵn

Số liệu đo từ chính các checkout trong `data-portal/`, không phải phỏng đoán.

| | Ngôn ngữ flow | Cách ly workspace | Giấy phép | Hệ sinh thái |
|---|---|---|---|---|
| Kestra | YAML — khớp | namespace + proxy egret; RBAC/tenant là Enterprise | Apache-2.0 | vừa; plugin là JAR ngoài |
| Airflow | **Python — phải codegen** | không có; hoặc một deployment mỗi workspace | Apache-2.0 | lớn nhất (82 thư mục `providers/` cấp một, còn lồng tiếp) |
| n8n | canvas JSON — không khớp | mô hình phân quyền nằm sau `.ee` | **Sustainable Use — chặn** | lớn, thiên integration |

**Airflow.** `airflow-core/src/airflow/utils/file.py:110` chỉ nhận
`path.suffix == ".py"` hoặc zip chứa Python, và `dag_processing/processor.py:578`
(`DagFileProcessorProcess extends WatchedSubprocess`) fork tiến trình con để thực
thi. Một sản phẩm YAML-first sẽ phải sinh mã Python, và flow của một workspace
trở thành Python tuỳ ý chạy trong DAG processor dùng chung. Airflow không có khái
niệm tenant.

**n8n.** `LICENSE.md`: file có `.ee` trong đường dẫn **không** được cấp phép nếu
không có Enterprise Licence thương mại. Có 122 file như vậy, gồm
`packages/cli/src/permissions.ee` và `sso.ee` — đúng những mảnh multi-tenancy mà
egret cần. Sustainable Use Licence còn cấm cung cấp phần mềm như dịch vụ cho bên
thứ ba, mà đó chính là việc egret làm.

**Kestra** là lựa chọn tốt nhất trong ba; phần dưới giữ *thiết kế* của nó và từ
chối *runtime* của nó.

## 3. Spike W5 — kết quả (chạy ngày 2026-09-01)

Khẳng định rủi ro nhất của chương trình là "egret chạy được plugin Kestra sẵn có
mà không nhúng Kestra". Spike đã giải quyết bằng thực nghiệm.

Môi trường: JDK 25 (Kestra 2.x biên dịch ra class-file 69; Kestra 1.3.35 là 65 /
Java 21), `io.kestra:core:2.0.0-rc13` giải ra 207 jar trên classpath từ Maven
Central, cộng một `ShimRunContext` ~120 dòng.

**Kết quả: một plugin Kestra thật, không sửa đổi, đã chạy ngoài Kestra, không Micronaut.**

```
OUTPUT={"row":null,"rows":[{"one":1,"tag":"from-kestra-plugin","workspaces":1}],"size":1}
[host] RunContext methods touched: [logger, render(Property), render(String,Map), validate, metric]
```

Đó là `io.kestra.plugin.jdbc.postgresql.Query` (từ fat jar 12,7 MB đã publish)
truy vấn Postgres dev của egret, điều khiển bằng một mô tả task dạng JSON.

Các phát hiện, theo thứ tự lộ ra:

1. **Không cần Micronaut để giải kiểu plugin.** `PluginDeserializer` rơi về
   `DefaultPluginRegistry.getOrCreate()` khi không có `KestraContext` — đường mà
   chính Kestra duy trì cho unit test của họ. `PluginScanner` đăng ký **76 core
   plugin trong ~450 ms** từ classpath.
2. **Shim phải kế thừa `DefaultRunContext`, không phải `RunContext`.**
   Constructor của `RunContextProperty` ép kiểu sang `DefaultRunContext` để lấy
   `getTask()`/`getTrigger()`. `DefaultRunContext` có constructor no-arg public,
   và `setTask` là package-private — nên shim phải khai báo trong package
   `io.kestra.core.runners`.
3. **Render identity là đủ.** egret render `{{ }}` bằng minijinja trước khi gửi;
   mọi `render*` của shim trả về nguyên đầu vào, và cả hai plugin thử đều đúng.
4. **`storage()` không cần cho kết quả inline.** Với `fetchType: FETCH` không có
   gì ngoài logger/render/validate/metric bị chạm. Với `fetchType: STORE` plugin
   gọi `workingDir()` rồi `storage()`. `workingDir()` là một dòng — chính
   `LocalWorkingDir(Path)` của Kestra là public. `storage()` tái dùng được
   `InternalStorage(StorageContext, StorageInterface, NamespaceFactory)` public,
   nên host chỉ cài `StorageInterface` — **16 method của một blob store thuần**,
   không phải 19 method ngữ nghĩa của `Storage`. *(2026-10-04, W9a Task 14: cầu
   nối đã có. `EgretStorage` của `plugin-host` cài `StorageInterface` dưới chính
   `InternalStorage` của Kestra; nó đọc các file egret đã stage vào thư mục làm
   việc và ghi dưới prefix của task run, rồi egret upload chúng sau task. Pod
   không giữ credential storage nào. Namespace files (W9b) và KV store (W9c)
   vẫn exit 4. Spec W9a §9.1.)* *(2026-10-07, W9b Task 11: namespace files
   không còn exit 4. `storage().namespace(ns)` trả lời, chỉ đọc, trên các
   namespace file egret đã stage cho task, những file mà một giá trị thuộc
   tính nguyên vẹn nêu tên; một lần ghi ném `UnsupportedOperationException`
   kèm lý do. Chỉ KV store (W9c) vẫn exit 4. Spec W9b §10.)* *(2026-10-08, W9c Task 8: W9c
   cho flow KV store qua các task `io.kestra.plugin.core.kv` và `kv()`,
   **không** có cầu nối plugin, vì không plugin nào trong catalog gọi
   `namespaceKv()`. `namespaceKv()` của plugin vẫn exit 4, và thông báo của
   nó nay chỉ tới các task `core.kv` thay vì nêu W9c. Spec W9c §10.)*
5. **Plugin phát hành dạng fat jar tự chứa.** `plugin-jdbc-postgres-2.0.0.jar`
   nặng 12,7 MB / 10.608 entry, gói cả base `plugin-jdbc` lẫn driver PostgreSQL.
   Toạ độ Maven `io.kestra.plugin:plugin-jdbc:2.0.0` **không tồn tại** trên
   Central — plugin không dùng được như dependency Maven thường, chúng được thả
   lên classpath. Điều đó làm host **đơn giản hơn**, không phải khó hơn.
6. **Chi phí: 1,61 s / 1,64 s / 1,68 s** tổng thời gian mỗi tiến trình — boot
   JVM, quét plugin, deserialize, truy vấn, thoát. Chấp nhận được khi phân bổ vào
   task dữ liệu, và chỉ flow nào dùng plugin Kestra mới phải trả.

### 3.1 Hình dạng kéo theo

`kestra-plugin-host` là một container image (JVM + Kestra core + jar plugin). Nó
được điều khiển qua **đúng hợp đồng `egret.container` như mọi task container
khác** — image, command, env, working dir, output dir. Không phải khái niệm mới
trong engine, không có JVM trong `egret-server`, không có JNI.

Vì sao không JNI, nói một lần để khỏi bàn lại: cửa vào của plugin là
`run(RunContext)`, mà `RunContext` có 41 method trừu tượng dựa trên các interface
cộng tác tổng cộng ~75 method nữa. Thoả mãn nó in-process nghĩa là hoặc JNI hai
chiều, hoặc boot Micronaut ngay trong `egret-server`; đằng nào một OOM hay
`System.exit()` của plugin cũng giết SQL Lab, Data API serving và mọi request
đang bay, còn mỗi task chạy lâu sẽ ghim một luồng `spawn_blocking`. Pod cho ta
cách ly, đồng thời hoá theo k8s, và tái dùng bộ máy egret đã có.

### 3.2 Giới hạn đã biết của host

- `render()` là identity, nên plugin nào *cố tình hoãn* render theo từng vòng lặp
  sẽ chạy sai.
- Plugin sinh sub-task động (`dynamicWorkerResult`, ví dụ `ForEachItem`) không
  được hỗ trợ.

Cả hai nhóm đều là task flowable/dynamic, thứ egret port sang Rust. Plugin lá —
JDBC, object storage, Slack, Kafka, Databricks — mới là đích, và chúng chạy.

## 4. Biểu thức, và oracle Pebble

Kestra không sở hữu template engine của mình: `platform/build.gradle:105` ghim
`io.pebbletemplates:pebble:4.1.2`. Kestra viết thêm 1.251 dòng keo cộng 37 filter
(1.684 dòng) và 40 function (2.968 dòng).

Không có engine Pebble/Twig nào dùng được bằng Rust. `pebble-rust` là binding SDK
**đồng hồ** Pebble; trên crates.io `twig` là 0.1.0 "work in progress" còn
`oak-twig` là 0.0.11. Nên egret hiện thực ngôn ngữ biểu thức của Kestra **trên
minijinja**, thứ đã có sẵn trong cây và có cơ chế `set_formatter` — chính thứ
khiến việc dựng SQL của `egret-dataapi` an toàn theo cấu trúc.

Khoảng cách cú pháp hẹp hơn dự đoán. Đếm trên checkout Kestra: `is` test (12 lần)
và `~` nối chuỗi (1 lần) vốn đã có trong Jinja2; chỉ thiếu `??` (38 lần) và
ternary `? :` (8 lần), cả hai xử lý bằng một bước viết lại ở tầng biểu thức.

**Ngữ nghĩa mới là rủi ro thật, và giờ nó đo được.** Spike đã dựng một engine
Pebble độc lập từ `io.kestra.core.runners.pebble.Extension` (constructor no-arg
public), không Micronaut:

```
{{ missing ?? 'fallback' }}                      => fallback
{{ flag ? 'yes' : 'no' }}                        => yes
{{ name | slugify }}                             => egret-data-portal
{{ d | date("yyyy-MM-dd'T'HH:mm:ss", timeZone='Asia/Ho_Chi_Minh') }} => 2026-09-01T09:30:00
{{ json | jq('.a.b') }}                          => [42]
{{ [1,2,2,3] | distinct }}                       => [1, 2, 3]
```

Dòng `jq` là điểm mấu chốt: nó trả về **một mảng**, không phải `42`. Một filter
Rust viết tay rất có thể trả về số vô hướng, render ra thứ trông hợp lý, và
không test nào đỏ. `date` dùng mẫu `DateTimeFormatter` của Java với ký tự literal
trong nháy, không phải strftime.

Vì vậy **W2 kèm theo một differential test**: một bộ biểu thức mẫu được render
bằng Pebble thật trong CI rồi diff với bản Rust. Độ trung thành thôi là ý kiến
trong review, trở thành một phép đo. Oracle chỉ cần JVM + Kestra core + Pebble —
tập con chặt của host W5.

## 5. Mặt cần port

| Kestra | LOC (main) | Xử lý |
|---|---|---|
| `core/models` trừ dashboards/collectors/policies/stats/ui/settings | 14.842 | port |
| `core/runners` trừ pebble | 8.942 | port |
| `runners/pebble/{filters,functions}` | 4.652 | port lên minijinja + oracle |
| `runners/pebble` phần keo engine | 1.529 | thay bằng minijinja |
| `executor/` | 5.649 | port |
| `plugin/core` trừ dashboard | 12.336 | port — task dựng sẵn |
| `worker/` | 5.972 | một phần |
| `core/scheduler` + `scheduler/` | 5.583 | một phần; vòng claim đã có |
| `core/{services,utils,validations,serializers}` | 16.110 | chọn lọc |
| `core/{storages,plugins,http,repositories,metrics,secret,encryption,namespace,queues,lock,topologies}` | ~12.700 | chọn lọc |
| `webserver/` + `ui/` | 21.794 + 731 file | **không port — nguồn tham chiếu UX/API bắt buộc, xem §8** |
| `jdbc/` + `queue-jdbc/` | 11.890 | bỏ — Postgres là hàng đợi |
| `cli/`, `core/{ai,mcp,docs,collectors,reporter,migration,tenant,security}` | ~6.600 | bỏ |

Mặt cần port ≈ **86k dòng Java** → ước **35–40k dòng Rust**. Model nén rất mạnh
(hết Lombok, builder, `@Schema`); phần logic gần 1:1.

Để có cái neo: `egret-dataapi` hiện là 45.783 dòng. Đây là một Data API thứ hai.

## 6. Kiến trúc

### 6.1 Schema

Phản chiếu cách tách draft/version của `dataapi.endpoints` /
`dataapi.endpoint_versions`, đúng lý do ghi trong migration 0121: nếu execution
đọc draft thì sửa flow sẽ đổi hành vi của lần chạy đêm nay, và "version" chỉ còn
là đồ trang trí.

```
workflow.flows          draft: yaml nguồn + plan đã parse, state draft/live/disabled
workflow.flow_versions  snapshot đóng băng: yaml + plan chuẩn hoá + tham chiếu ĐÃ GIẢI
workflow.executions     một lần chạy, mang bản sao đóng băng yaml nguồn của DRAFT
                        -- không hề có cột flow_version_id (xem bên dưới)
workflow.task_runs      một lần thử của một node: attempt, state, not_before, deadline_at, output
workflow.triggers       mỗi trigger Schedule một hàng (webhook không cần hàng);
                        hàng đợi claim của vòng trigger -- do W12 DỰNG
                        (migration 0156, `scheduler::sync`/`fire_due`), thiết kế ở
                        docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md
workflow.task_logs      dòng log, một cửa sổ retention toàn cục (không theo schedule)
                        -- W11 dựng (migration 0159, `logs::{LogRepo, PgLogSink}`),
                        thiết kế ở docs/{en,vi}/specs/2026-09-27-workflow-w11-execution-logs-design.md
```

**Đính chính (W10 Task 5, vòng sửa 1): execution chạy DRAFT, và không hề có
`flow_version_id`.** Dòng trên từng ghi *"một lần chạy, ghim `flow_version_id`;
không bao giờ đọc draft"*. Cả hai vế đều sai, và vế về cột thì kiểm được bằng
một lệnh: `flow_version_id` xuất hiện **0** lần trong `migrations/` (đối chứng
dương: `flow_key`, tại `0134:32`). Thứ `workflow.executions` thật sự giữ là
`source` — một bản sao đóng băng của `workflow.flows.source`, chụp lúc start.

Có **ba** nơi khởi tạo execution và cả ba đều đọc hàng draft:

| nơi | đọc gì |
|---|---|
| `worker::subflow::resolve` | `SELECT id, source FROM workflow.flows …` |
| `webhook_routes::handle` | `SELECT id, source, state FROM workflow.flows …` |
| `routes::start_flow_execution` (W10 Task 5) | `FlowRepo::get` → `flows.source` |

Không đường chạy nào chạm vào `workflow.flow_versions`. Lý do ràng buộc đã ghi
tại `worker::subflow::resolve` và không xét lại ở đây: **cha và con không được
bất đồng về việc chúng đang ở phiên bản nào của thế giới**. Lý do thứ hai mà câu
này từng mang — *"một resolver đọc version snapshot sẽ với tới gần như không gì
cả (W6 Task 1 … đo được 16 trên 345 flow đạt tới live version)"* — **đã bị rút
tại W10 Task 6 fix round 1**: §8 mục 4 nói `type:` nhận FQCN của Kestra làm dạng
chuẩn, `spec::registry::TASK_TYPES` lại không cài đặt luật ấy, nên publish từ
chối mọi flow Kestra dán vào. Sau khi bốn FQCN lõi vào bảng, **248 trên 345**
flow corpus đạt tới live version và 18 trong 20 target `Subflow` dạng tên thật
là live — một resolver snapshot bây giờ với tới gần hết. Quyết định vẫn đứng trên
đúng lý do ràng buộc; `driver::target_census` khẳng định các con số mới mỗi lần
chạy. Cái giá đúng như đoạn đó đã nêu: tác giả sửa draft là đổi luôn thứ mà một
URL webhook đang sống — hay một nút start — sẽ chạy, không có bước publish nào ở
giữa.

**Tính cứng của §6.2 không suy suyển, nhưng bằng một CƠ CHẾ khác với điều câu
trên đã tuyên.** Publish vẫn giải mọi tham chiếu và vẫn đóng băng id đã giải vào
snapshot; thứ ngăn một start-từ-draft chạy với tham chiếu chưa giải không phải
là "executor không bao giờ giữ tham chiếu chưa giải" — nó CÓ giữ — mà là handler
từ chối. Đo được, và HẸP HƠN câu "start chỉ chạy được flow không có tham chiếu":

* **Đúng một handler trên tám** trong `worker/handlers/` parse một trường spec
  thành UUID và từ chối nếu không phải. `handlers::sql` trả lời *"`database` là
  … không phải UUID. `publish` viết lại trường này thành connection id đã giải;
  một execution mà nguồn còn giữ tên do tác giả viết là đã được khởi chạy từ
  nguồn chưa giải"* (`handlers/sql.rs:294`). Nghĩa là flow có task `egret.sql`
  với `database:` viết bằng tên vẫn **start được**, rồi **task đó fail** — chỗ
  từ chối nằm ở task, không nằm ở route.
* **Tham chiếu `Subflow` là trường hợp ngược lại và chạy bình thường**, đó chính
  là lý do câu tổng quát kia sai: `subflow::resolve` giải theo
  `(namespace, flowId)` trên bảng draft lúc chạy và không bao giờ đọc id đã viết
  lại. ~~và nó đóng đúng hai cánh cửa bằng SQL
  (`AND deleted_at IS NULL AND state <> 'disabled'`) mà
  `routes::startable_flow` đóng bằng Rust.~~

  **Mệnh đề cuối đó SAI và được đính chính ở đây (W10 Task 7 fix round 1,
  2026-09-15), sau khi `24855ba7` đã sửa trong mã mà không sửa trong file này.**
  `routes::startable_flow` đóng **ba** cánh cửa — thùng rác, cột `state`, và
  `disabled: true` trong chính YAML, thứ nó đọc bằng cách gọi `parse`.
  `subflow::resolve` đóng **hai**, và không thể đóng cửa thứ ba: `disabled:`
  không phải một cột, nên không predicate SQL nào với tới được. Khoảng hở ấy là
  thật — một operator viết `disabled: true`, `POST …/flows/{id}/executions` trả
  **409**, rồi một flow anh em vẫn chạy đúng flow đó với tư cách `Subflow`
  target. Cửa thứ ba nay nằm ở `worker::subflow::spawn_one`, ngay sau
  `spec::parse`, và trả lời **y nguyên** thông điệp miss của `resolve`, để hai
  cách viết "tắt" cho operator cùng một câu trả lời. **Không cửa nào trong hai
  cửa gốc của `resolve` có test**: xoá từng cái đều để `--test worker` ở 59/0,
  trong khi control workspace fail 58/1. Giờ cả ba đều có một test.

  **Bài học phải trả giá, ghi ngay chỗ câu tiếp theo kiểu đó sẽ được viết:** một
  comment khẳng định điều một hàm KHÁC làm thì cần test của riêng nó, đặt trong
  hàm đó. Khẳng định về mã của chính mình được test quanh nó kiểm; khẳng định về
  mã của người khác thì **không gì kiểm cả**, và đọc lên như một tham chiếu chéo
  chứ không như một khẳng định.

`a_start_runs_the_draft_even_when_a_published_version_exists`
(`tests/executions_routes.rs`) là thứ giữ cho đính chính này khỏi mốc: nó publish
một version, sửa draft bên dưới, rồi khẳng định start chạy draft. Trước nó, không
fixture nào trong cây có version đã publish, nên không gì phân biệt được đọc
draft với đọc `flow_versions`.

### 6.2 Tham chiếu được giải lúc publish, không phải lúc chạy

Một flow trỏ tới object của egret — `database`, `notebook`, `dataset`, `chart`,
`secret`, endpoint Data API. Mỗi cái là một thể hiện nữa của lớp lỗi `chart_id`:
migration 0128 tạo một FK trơn không có vị từ workspace, và phép kiểm Cedar theo
sau được cho biết workspace **của người gọi**, không phải của object.

Nên luật là cấu trúc, không phải quy trình: **mọi tham chiếu được giải và kiểm
workspace tại `publish`, rồi đóng băng vào snapshot của version. Executor không
bao giờ cầm một id chưa giải.** Publish từ chối cả flow nếu có tham chiếu lạ
workspace, trả `NotFound` — không phân biệt được với "không tồn tại", đúng như
`DataApiEndpointPort::workspace_of` đã yêu cầu, để lời gọi không dò được workspace khác.

Một flow xuyên workspace trở thành thứ **không publish nổi**, thay vì thứ mỗi
handler phải nhớ chặn.

*(W5b, 2026-09-30: `secret` là tham chiếu duy nhất được giải lúc chạy, không
phải lúc publish. Đóng băng giá trị của nó vào snapshot phiên bản sẽ lưu nó vào
`flow_versions`, điều W5b cấm. Mục đích của quy tắc vẫn giữ được nhờ cấu trúc:
`secret('NAME')` chỉ được tra trong workspace của chính execution, không đối số
nào chỉ được tới workspace khác (`namespace=` và `full=` bị từ chối có tên), nó
chỉ giải trên task `kestra.*`, và một secret không tồn tại làm task thất bại,
nêu tên secret, không bao giờ nêu giá trị. Xem spec thiết kế W5b §6. Vòng sửa 1
của W5b thêm ai được viết nó: một flow mà nguồn có thể gọi `secret()` chỉ được
lưu hay publish bởi người có `workflow:use_secrets`, cấp cho `ws_admin`.)*

### 6.3 Máy trạng thái executor là hàm thuần

```rust
/// Không I/O, không DB, không đồng hồ. Chụp trạng thái vào, lệnh ra.
fn advance(plan: &Plan, state: &ExecutionState, now: Instant) -> Vec<Command>
```

Kestra cần 69.179 dòng test cho core phần lớn vì các ca này chỉ chạm tới được qua
một hệ thống đang chạy: một nhánh `parallel` fail trong khi nhánh anh em đang
retry, subflow bị kill giữa chừng, worker chết mang theo task
(`WorkerJobLifecycle`), nhả concurrency slot, `ExecutionDelay`. Là hàm thuần trên
một snapshot, mỗi ca thành một unit test trên struct — không Postgres, không
Trino, không pod.

Vòng bền vững thì mỏng và mượn nguyên câu lệnh đã chứng minh trong
`egret-automation/src/scheduler.rs`:

- **vòng trigger** — claim trigger tới hạn, tạo execution và task_run gốc.
  **W12 đã DỰNG** (`scheduler::fire_due`), đúng như thiết kế ở
  `2026-09-24-workflow-w12-trigger-loop-design.md`: một trigger một transaction
  dưới `FOR UPDATE SKIP LOCKED`, execution được tạo trong cùng transaction đẩy
  lịch đi tiếp, không lease, và mỗi pass đánh giá mọi trigger đến hạn đúng một
  lần -- sau cần gạt cấu hình riêng, `scheduler_enabled`, mặc định `false`;
- **vòng worker** — claim `task_runs` có `state='pending' AND not_before <= now()`
  bằng `FOR UPDATE SKIP LOCKED`, chạy handler, và **trong cùng transaction** ghi
  kết quả, gọi `advance()`, chèn các node kế tiếp. Không cần vòng thứ ba, và
  không có khoảnh khắc nào task xong mà execution chưa đi tiếp;
- **thu hồi** — `working_timeout_s` như automation. Với task chạy bằng pod, thu
  hồi phải **kill pod trước** rồi mới thử lại, nếu không hai pod sẽ cùng ghi.

Retry là một attempt mới với `not_before = now + backoff`; timeout là
`deadline_at` do reaper quét. Cả hai rơi ra từ cùng một vị từ claim.

### 6.4 Task handler

Đặt tên `TaskHandler`, **cố ý không** dùng `TaskRunner`: trong Kestra tên đó nghĩa
là bộ chạy dòng lệnh shell (`TaskRunner.run(RunContext, TaskCommands,
filesToDownload)`, javadoc ghi "will be called by the script plugin"), dùng lại
tên đó sẽ tái tạo đúng sự hiểu nhầm mà chương trình này đã phải gỡ.

```rust
#[async_trait]
pub trait TaskHandler: Send + Sync {
    fn type_id(&self) -> &'static str;
    /// Lúc PUBLISH: kiểm tra, giải mọi tham chiếu trong workspace, đóng băng.
    async fn resolve(&self, spec: &Value, cx: &PublishCx<'_>) -> Result<ResolvedSpec>;
    async fn run(&self, cx: &TaskCx<'_>) -> Result<Outcome>;
    async fn cancel(&self, cx: &TaskCx<'_>) -> Result<()>;
}
```

| `type` | Dựa trên |
|---|---|
| `egret.sql` | `QueryExecPort` (đã fail-closed về scoping workspace) + đường `bind_formatter` |
| `egret.python` | `KernelLauncher`; cần port `egret-kernel-api` mới |
| `egret.container` | cùng launcher; image + command + file in/out — hợp đồng `TaskCommands`. *(W5a, 2026-09-28: nay là alias của `io.kestra.plugin.scripts.shell.Commands` của Kestra, bên cạnh `shell.Script`, `python.Commands`, `python.Script` và alias `egret.script.python`. Chạy ở đâu do thuộc tính `taskRunner:` hoặc `workflow.container_runner` quyết định: Docker khi phát triển cục bộ, một pod Kubernetes cho mỗi attempt của task run trên production, không bao giờ qua `KernelLauncher` của notebook. Xem spec thiết kế W5a.)* |
| `kestra.<FQCN>` | `egret.container` chạy image `kestra-plugin-host` *(W5b, đã dựng 2026-09-30. Nó có handler `kestra` riêng thay vì đi qua handler của `egret.container`: handler dựng cùng một `TaskCommands` — image `workflow.plugin_host.image`, `exec java … EgretPluginHost run task.json` — và giao cho runner đã cấu hình của W5a. `<FQCN>` phải là một lớp trong catalog sinh từ `plugin-host/plugins.lock`; FQCN trần bị từ chối kèm gợi ý cách viết có tiền tố. Xem spec thiết kế W5b.)* |
| `egret.dataapi.call` | `DataApiEndpointPort` |
| `egret.dataset.refresh`, `egret.report.send` | renderer + delivery của `egret-automation` |
| `core.http`, `core.slack` | `automation/deliver/*`; phải chặn SSRF (allow-list host, cấm IP nội bộ) |
| `core.{sequential,parallel,if,switch,dag,loop,loopUntil,subflow}` | không handler — máy trạng thái |

Handler phụ thuộc port trait, không bao giờ phụ thuộc crate cụ thể, đúng như
`egret-dataapi` giữ `Arc<dyn QueryExecPort>` mà không biết `egret-query`.

### 6.5 Crate

```
egret-workflow-api/   WorkflowPort::workspace_of
egret-workflow/
  models/     flows, tasks, executions, triggers, property   (không I/O)
  runners/    render biểu thức, biến của run                  (không I/O)
  executor/   advance() — máy trạng thái thuần                (không I/O)
  worker/     vòng claim, handler, reaper                     (I/O)
  plugins/    registry handler
  repo.rs / routes.rs
```

Ba thư mục đầu không chạm DB và không chạm mạng. Ranh giới đó giữ toàn bộ độ khó
ngữ nghĩa và test được mà không phải dựng gì. Nó cũng tồn tại để đây không trở
thành một `dataapi/src/lifecycle.rs` nữa — 12.091 dòng trong một file.

## 7. Luật trung thành

1. **Ghim Kestra hai lần, và ghi cả hai.** Không ghim thì "giống Kestra" là mục
   tiêu di động.
   - *Tham chiếu mã nguồn* — checkout mà bản port đọc theo: `data-portal/kestra`
     tại `96893b589` (2.0.0-SNAPSHOT).
   - *Tham chiếu artifact* — phiên bản đã publish mà plugin host và oracle Pebble
     liên kết tới: `2.0.0-rc13`.
   Hai thứ này khác nhau và phải nâng cùng nhau, có chủ đích.

   **Chúng đã lệch nội dung thật, và không phải lệch hình thức.** Oracle của W2 phát hiện `2.0.0-rc13`
   mang function `range` (`BoundedRangeFunction`) **không tồn tại ở đâu** trong checkout `96893b589` —
   xác nhận bằng cách decompile `Extension.class`. Nghĩa là artifact đã publish không được build từ
   commit ghim. Luật rút ra: **artifact là thẩm quyền về HÀNH VI**, vì đó là thứ oracle đo được và là
   thứ Kestra của người dùng thực sự chạy; checkout là thẩm quyền để **đọc** một bản hiện thực. Chỗ nào
   một bề mặt chỉ tồn tại trong artifact, hãy ghi lại rồi port theo hành vi đo được hoặc bỏ qua kèm lý
   do — tuyệt đối không lặng lẽ cho rằng hai bên trùng nhau. Lưu ý Kestra 2.x
   biên dịch cho Java 25, 1.3.x cho Java 21; JDK của image host bám theo tham
   chiếu artifact.
2. **Tên module Rust phản chiếu tên package Java**: `models/`, `runners/`,
   `executor/`, `worker/`, `plugins/`.
3. **Giữ nguyên tên kiểu**: `Execution`, `TaskRun`, `WorkerTask`,
   `WorkerTaskResult`, `FlowableUtils`. `State` giữ đủ **17** biến thể —
   `CREATED, SUBMITTED, RUNNING, PAUSED, RESTARTED, KILLING, SUCCESS, WARNING,
   FAILED, KILLED, CANCELLED, QUEUED, RETRYING, RETRIED, SKIPPED, BREAKPOINT,
   RESUBMITTED` — kể cả những cái v1 không bao giờ sinh ra. Thiếu một biến thể là
   chỗ ngữ nghĩa lệch đi âm thầm, và `RESUBMITTED` chứng minh điều đó: nó là mục
   cuối trong enum Java nên kết thúc bằng `;` chứ không phải `,`, khiến một cách
   đọc `State.java` theo dòng bỏ sót nó — trong khi `Type.isTerminated()` vẫn phụ
   thuộc vào nó.
4. **`type:` nhận FQCN đầy đủ của Kestra làm dạng chính tắc**
   (`io.kestra.plugin.core.log.Log`); alias ngắn là đường phụ. Flow Kestra dán
   vào egret chạy được, và tra một task trong doc Kestra ra đúng trang.
5. **Mỗi file port ghi nguồn ở dòng đầu**:
   `// kestra: core/src/main/java/io/kestra/core/runners/FlowableUtils.java`.
   Rẻ, và là thứ giữ cho luật 1–4 không mục.

## 8. UI và webserver của Kestra làm tham chiếu

Không port, nhưng bắt buộc đọc trước W7.

- `ui/src/components/executions/` — `Topology.vue`, `Gantt.vue`, `Logs.vue`,
  `Outputs.vue`, `Metrics.vue`, `TaskRunLine.vue`, `ExecutionProgress.vue`,
  `ChangeExecutionStatus.vue`, `ReplayWithInputs.vue`, `FilePreviewDrawer.vue`.
  Đây là phần Kestra làm tốt nhất và egret chưa có gì tương đương.
- `ui/src/components/flows/` — `MultiPanelFlowEditorView.vue` (câu hỏi UX khó
  nhất của W7), `FlowRevisions.vue`, `FlowTriggers.vue`, `FlowConcurrency.vue`,
  `BackfillBanner.vue`, `FlowRun.vue`, `Curl.vue`, và `no-code/` để dành sau.
- `webserver/.../controllers/api/` — hình dạng endpoint: `FlowController`,
  `ExecutionController`, `LogController`, `TriggerController`, `KVController`,
  `NamespaceFileController`.

**Luật:** lấy kiến trúc thông tin và tương tác; **không** lấy ngôn ngữ thị giác.
egret đã có `docs/en/ui-conventions.md`, scaffold trang danh sách Explore chính
tắc, và token OKLCH; Explore và Control Plane vốn đã dùng idiom header/button
khác nhau. Cùng kỷ luật đã dùng cho parity Metabase: sao chép cách nó *hoạt
động*, không sao chép cách nó *trông*.

Kestra có sẵn `docker-compose.yml`. Dựng lên và nhìn tận mắt trước khi thiết kế
W7 — một bản kiểm kê màn hình không phải một cuộc khảo sát UX.

## 9. Các gói

| Gói | Nội dung | Phụ thuộc |
|---|---|---|
| **W0** Nền & đổi tên | khung crate phản chiếu tên module Kestra; xoá hai placeholder; entitlement `workflows`; đổi `egret-orchestrator` → `egret-provisioner` (hiện nghĩa là cấp phát engine trên k8s, sẽ va với workflow orchestrator) thành một PR cơ học riêng | — |
| **W1** Model & ngôn ngữ | `models/{flows,tasks,executions,triggers,property}`; parse/validate YAML; bảng draft/version; publish + giải tham chiếu | W0 |
| **W2** Biểu thức | minijinja + 37 filter + 40 function + viết lại `??`/`?:` + **oracle differential Pebble** | W0 |
| **W3a** Máy trạng thái executor | `advance()`; `sequential/parallel/if/switch/dag/loop`; retry/timeout/`errors:`; oracle vết thực thi Kestra. Không I/O | W1, W2 |
| **W3b** Executor bền vững | vòng claim; reaper; `SpawnExecution`; handler `core.log`, `core.debug.Return`, `core.execution.Fail`, `core.flow.Sleep`, `egret.sql`; nối oracle vào driver thật. **Đính chính, 2026-09-14 (xem W10 spec §1): vòng claim và reaper ship ra dưới dạng hàm KHÔNG có caller production nào.** Grep `tick(`, `reap(` và `start_execution(` trên mọi `crates/*/src` không tìm ra lấy một call site, và `egret-bin` dựng một `WorkflowState` — flow repo cùng refs của nó — chứ không dựng vòng nào, nên thứ duy nhất đẩy vòng ấy chạy là `oracle-exec/driver.rs` và test của chính crate này. Đây là đính chính hồ sơ chứ không phải chê W3b — mục 15 trong *The coverage statement* của `oracle-exec/README.md` đã nói điều đó — và **W10 là dòng khép lại nó** | W3a |
| **W4** Runner & container | bộ đánh thức `RETRYING`, `timeout:` cấp task, đường kill — ba lỗ W3b chuyển giao, **đáng ~0 mục baseline, KHÔNG phải 20 như dòng này viết ban đầu** (con số ấy là hợp của ba lần grep khai báo; xem W4 spec §1.1); rồi mới tới `TaskCommands`, `egret.container`, `egret.script.python`, vốn chạm KHÔNG mục corpus nào và có test riêng. **`egret.python` KHÔNG phải của W4** — §6.4 giao tên ấy cho `KernelLauncher` của notebook, và `TASK_TYPES` giữ nó từ W1 | W3b |
| **W5** Kestra plugin host — **tách ngày 2026-09-28 (quyết định của người dùng)** thành **W5a** *Container chạy trên production*: bốn script task của Kestra (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) với `taskRunner:` (Docker, và Kubernetes dưới FQCN của Kestra EE), feature `workflow_containers` (mặc định tắt), allowlist image `plugin_configurations`, gia hạn lease, runner Kubernetes và namespace task trong chart — `docs/{en,vi}/specs/2026-09-28-workflow-w5a-containers-in-production-design.md`; và **W5b** *Kestra plugin host*, dựng trên W5a: image JVM, `ShimRunContext`, cầu `StorageInterface`, điều phối `kestra.<FQCN>`, giải jar plugin và allowlist jar plugin. Plugin dựa trên storage và secret chờ W9. *(W5b, 2026-09-30: dựng xong mà không có cầu `StorageInterface` — mọi lời gọi storage đều ném lỗi và task thất bại nêu tên W9 — và có `secret()`, thứ W5b mang lên sớm: plugin dựa trên secret nay đã chạy; plugin dựa trên storage vẫn chờ W9. Xem đính chính W5b bên dưới.)* *(W9a, 2026-10-04: cầu nối đã xong và plugin dựa trên storage chạy được — `Query`/`Queries` với `fetchType: STORE`, `postgresql.Batch`, `CopyIn`, `CopyOut`, `mysql.Batch`; xem đính chính W9a bên dưới.)* | W4 (W5a cần thêm W10, W11) |
| **W6** Trigger & subflow | `subflow` + replay `CREATE_NEW_EXECUTION` (**13–15 mục baseline, chung một migration**); rồi mới cron, webhook, flow trigger và backfill — vốn chạm **0** mục baseline và có test riêng. Dòng này viết TRƯỚC khi corpus được đo: bốn trong năm chữ ấy đáng **KHÔNG** mục nào, và backfill **không có lấy một dòng khai báo sống trong 345 flow**. Xem W6 spec §1. **Kết quả, đã đo: nửa bank được đã bank 16 mục** — 12 `Subflow` + 4 replay, tức ước lượng 13–15 thiếu một ở cận trên — **và bốn chữ trigger bank 0, đúng như dự đoán.** Cơ chế của từng con 0 nằm ở `oracle-exec/README.md`, *The coverage statement* → mục W6 | W3b |
| **W7** Frontend | nhóm nav Workflows; editor YAML; topology chỉ-đọc; gantt và log của execution. **Bị chặn bởi W10**: gantt và topology cần một tiến trình đẩy execution đi tiếp cùng một bề mặt REST execution, và lúc viết dòng này chưa có cái nào. **Panel log cần W11** và nằm ngoài lát cắt đầu của W7 — nói ra chứ không để phát hiện muộn. *(2026-09-27: W11 đã ship API mà panel đọc, `GET …/executions/{id}/logs`; bản thân panel vẫn là việc của W7 và chưa dựng.)* *(2026-10-10: **W7 được chia thành W7a–W7e**: W7a flow (nhóm nav, danh sách flow, editor flow nhiều panel, Revisions và Run), W7b execution (kèm panel log), W7c topology, W7d namespace, W7e trigger. **W7a đã xong, 2026-10-10**: `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`.)* *(2026-10-12: **W7b đã xong**: danh sách execution, chi tiết execution với Overview, Gantt, Logs và Outputs, Resume và Replay, kèm panel log mà dòng này từng hoãn sang W11; `docs/{en,vi}/specs/2026-10-11-workflow-w7b-executions-design.md`, đã đối chiếu với một Kestra đang chạy ở §2 và §11 của nó. Còn W7c–W7e.)* | W1, W3b, W10 |
| **W8** Ngữ nghĩa nâng cao | `pause`/`resume` **cùng** `loopUntil` (MỘT trạng thái, không được tách), `allowFailure`, và hai mục `EXECUTION_ASSERTION` của `sla:` — **18 mục baseline, số chặt**, đã đo. **Concurrency limit chạm 0 và `labels` chạm 0**; `WorkingDirectory` chạm 1 chứ không phải 4 như khai báo gợi ý. Dòng `concurrency:` → `CANCELLED` trong bảng residual là **quy sai** (`CANCELLED` là 2 của `sla:` + 1 của `Exit`, chưa bao giờ của một limit) nên dòng ấy bị XOÁ, không phải hạ số. Dòng này viết TRƯỚC khi corpus được đo. Xem W8 spec §1 | W3b, W6 |
| **W9** Hạ tầng Kestra còn lại — **tách ngày 2026-10-01 (quyết định của người dùng)** thành **W9a** *Internal storage* (file `kestra://` cho execution, task, plugin và các function file; `docs/{en,vi}/specs/2026-10-01-workflow-w9a-internal-storage-design.md`), **W9b** *Namespace files* và **W9c** *KV store*. W9a đã xong (xem đính chính của nó bên dưới); W9b và W9c chưa bắt đầu *(2026-10-07: W9b đã xong, `docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md`; xem đính chính của nó bên dưới. W9c chưa bắt đầu)* *(2026-10-08: W9c đã xong, `docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md`; xem đính chính của nó bên dưới)* | KV store, namespace file, internal storage, output | W4 (W9a còn W5a, W5b, W10, W11) |
| **W10** Runtime & bề mặt execution | bộ ticker worker trong `egret-bin` — đóng/mở bằng config, `MissedTickBehavior::Skip`, biết huỷ, kèm **một phát biểu an toàn cho mỗi lần gọi** `tick`, `wake`, `due_retries` và `reap`; reaper trên interval riêng, nêu rõ `working_timeout_s`; một bề mặt REST execution (list, get kèm task run, start, resume, kill, replay) — phạm vi workspace theo cấu trúc, có feature gate, tài liệu utoipa, sinh lại client frontend bằng `npm run gen:api`; **một test end-to-end publish, start và quan sát một flow tới trạng thái kết thúc mà KHÔNG có harness nào đẩy tick**; và **chính đính chính của bảng này**, để **bốn** hạng mục W7 cần có chủ — hai cái là của chính W10, hai cái được gọi tên là W11 và W12. **Ratchet không phán xét được gói này theo cả hai chiều**: `oracle-exec/driver.rs:884` gọi thẳng `worker::tick` với registry của riêng nó, nên oracle vốn đã chạy phần thân của vòng và mù với phần nối dây của binary **do chính cấu tạo** — dự kiến **109 trước và 109 sau**. Xem W10 spec §1 | W3b, W4 |
| **W11** Log execution | `workflow.task_logs`, một `LogSink` có ghi xuống, cửa sổ retention (§11 câu hỏi mở 1) và một API đọc có phân trang. **Hoãn khỏi W10 kèm PHÉP ĐO chứ không phải bỏ quên:** không migration nào tạo bảng `task_logs` — schema `workflow` có đúng bốn bảng: `flows`, `flow_versions`, `executions`, `task_runs`; `LogSink` có **ba** implementation dưới `crates/egret-workflow/src/` — `TracingSink` (ghi ra `tracing`), một `Recorder` `#[cfg(test)]` (một `Vec` trong bộ nhớ) và `NameHintSink` (chú thích rồi chuyển tiếp) — và **không cái nào ghi xuống lấy một dòng**; còn handler container ghi `stdOutLineCount` và `stdErrLineCount`, tức **đếm dòng chứ không phải nội dung**. Chừng nào chưa có, **W7 ship mà không có panel log** | W10 |
| **W12** Vòng trigger | `workflow.triggers` — đúng những hàng cron/webhook claim được mà §6.1 đã thiết kế — vòng lặp claim và kích hoạt chúng, cùng **lập luận an toàn concurrency của riêng nó** chứ không mượn lại lập luận của claim ở worker. **Hoãn khỏi W10 kèm phép đo, y như W11:** **không migration nào tạo `workflow.triggers`** (control: `workflow.executions` xuất hiện trong **ba** file migration, nên phép tìm có chạm tới migration, và schema `workflow` có đúng bốn bảng); `trigger::tick` **không có caller nào ngoài `crates/egret-workflow/src/trigger/`**; `TriggerState::initialise` có **đúng một** caller và đó là helper `#[cfg(test)]`; và `crates/egret-bin/src/main.rs` chứa chuỗi `trigger` **không lần nào** (control: `egret_workflow` xuất hiện **5** lần trong file đó). **Nên tại thời điểm dòng này được viết, một trigger `cron:` không thể nổ trong production, và W10 đã không đổi điều đó**: W10 nối vòng *worker*, vốn có bảng để claim; vòng trigger lúc đó không có bảng nào. Chừng nào chưa có W12, một flow chạy bằng **start thủ công qua API hoặc bằng webhook**, cả hai đều hoạt động — nói ra chứ không để phát hiện muộn **Thiết kế ngày 2026-09-24** ở `docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md`, phạm vi thu hẹp cùng người dùng về **chỉ schedule trigger**, tick bị lỡ được bù theo mặc định `ALL` của Kestra. Dòng này từng ghi *"hàng cron/webhook"*: webhook được đẩy qua HTTP và không cần hàng, nên bảng chỉ chứa schedule.

**Đính chính, 2026-09-25 (W12 Task 6): đã dựng xong, và mọi phép đo ở trên được RÚT LẠI chứ không xoá**, vì đó là ghi chép lý do dòng này từng tồn tại. Migration 0156 tạo `workflow.triggers`; `scheduler::sync` (gọi từ ba writer của `FlowRepo` có thể đổi thứ sẽ nổ — `create`, `update`, `soft_delete`; `set_live` được miễn, vì nó không đổi `source` lẫn `deleted_at`, và `state` được đọc lúc nổ) giữ nó khớp với `flow.source`; `scheduler::fire_due` claim và kích hoạt trigger `Schedule` tới hạn, mỗi trigger một transaction dưới `FOR UPDATE OF t SKIP LOCKED`, execution được tạo trong cùng transaction đẩy lịch đi tiếp qua một savepoint, với `workflow_executions_one_per_schedule_tick` (migration 0156) làm lưới chặn cho một lần đọc cũ không thể vượt qua. Nó chạy trên ticker riêng trong `runtime::spawn`, sau cần gạt `EGRET_WORKFLOW__SCHEDULER_ENABLED` — đóng/mở bằng config, **mặc định `false`**, đúng như `worker_enabled` ở W10, nên một deployment chưa gõ cờ này hành xử y hệt trước W12. **Một trigger `cron:` giờ CÓ THỂ nổ trong production, một khi cờ đó được bật**; ba phép đo call-site ở trên (số caller của `trigger::tick`, số caller của `TriggerState::initialise`, chuỗi `trigger` trong `main.rs`) đều đã bị thay thế — `scheduler::sync` và `scheduler::fire_due` là caller mới, không phải test, của cả hai; và `runtime.rs` giờ đọc `trigger` một số khác không lần. Ratchet của oracle không thấy được gì trong số đó, vì lý do phần W12 trong tuyên bố độ phủ của `oracle-exec/README.md` đã ghi: **109 trước, 109 sau, và 109 nếu xoá `fire_due`.** | W6, W10 |
| **W13** Flow trigger | nhánh không cửa sổ của `trigger/flow.rs` nối vào transaction settle của worker — **theo sự kiện, nên không cần bảng claim** — và sau đó nhánh cửa sổ `dependsOn`, mà 20 trên 21 khai báo trong corpus đi theo. **Được gọi tên ra khỏi phạm vi W12 ngày 2026-09-24** để có chủ thay vì vắng mặt. Endpoint backfill, `lateMaximumDelay`, `stopAfter` và việc đưa `trigger.*` vào render context được liệt kê ở §7 của spec W12 và vẫn **không có chủ** **Thiết kế ngày 2026-09-25** ở `docs/{en,vi}/specs/2026-09-25-workflow-w13-flow-triggers-design.md`, làm CẢ HAI nhánh theo quyết định của người dùng, giao thành **W13a** (chỉ mục listener do `scheduler::sync` giữ, outbox ghi trong `settle`, processor chạy trên ticker scheduler, nhánh không cửa sổ) và **W13b** (model `window`, port `TimeWindow`, `flow_trigger_windows`, `dependsOn`). Bất đồng bộ, không lồng trong `settle`: lồng bị chặn ở `MAX_SPAWN_CHAIN = 8` vì stack của worker, trong khi Kestra cho độ sâu 100. | W12 |

**Đính chính, 2026-09-25 (W13a Task 5): W13a đã dựng xong; W13b vẫn còn treo.** Migration 0157 thêm `workflow.flow_listeners` (giữ khớp bởi `scheduler::sync`, gọi từ đúng ba writer của `FlowRepo` mà dòng W12 đã nêu) và `workflow.flow_trigger_events`, outbox mà `settle` ghi vào — một `INSERT` có điều kiện, câu lệnh DUY NHẤT W13a thêm vào hot path đó (spec §4.3). `flow_trigger::process_events` rút cạn nó trên ticker scheduler (spec §4.4), gọi `trigger::flow::evaluate` lần đầu tiên trong production: nó render `when` và `inputs` trên context của chính nguồn, bắn trong một savepoint riêng cho từng listener để một listener hỏng không làm mất sự kiện đối với các listener anh em, retry một lỗi tạm thời tới `MAX_ATTEMPTS` rồi bỏ cuộc có tiếng past nó, và — đo trên artifact đã ghim, không suy đoán — tạo execution **born-FAILED** khi `inputs` không render được, đúng như Kestra, chứ không từ chối. Hai cầu nối mà mã trước-W13a để trống đều đã được xây: một sự kiện outbox từ nguồn PAUSED mang một `models::State::Paused` thật vào `evaluate` (hook của `settle` không gate theo tính kết thúc, vì `states` mặc định của Flow trigger có `PAUSED`), và `trigger_execution_id` của một execution vừa bắn cho phép bước đi nhân quả sẵn có của `worker::render::Surface::load` (đã sửa ở W6 Task 8) suy ra `labels.system.correlationId` là id gốc của chuỗi thay vì id của chính đứa con — đóng lại lỗ hổng mà module doc của `trigger::flow` từng mô tả là "thuộc về wiring seam". Ratchet của oracle không thấy được gì trong số này, cùng lý do cấu trúc W10 và W12 đã nêu: `oracle-exec/driver.rs` không bao giờ gọi `flow_trigger::process_events`, nên là **109 mismatching, 109 baselined, 0 broken trên 294 golden entry** trước task này, tại commit này, và cả khi `process_events` bị cho return sớm vô điều kiện — đo trên nhánh này, cả ba, không mang từ nơi khác sang. **W13b — nhánh `dependsOn`/window, 20 trên 21 khai báo của corpus — CHƯA bắt đầu**: chưa có model `window`, chưa có port `TimeWindow`, chưa có migration 0158, và bảng §7.2 của spec W13 vẫn là kế hoạch cho nó, không phải ghi chép đã làm. Xem phần W13a của `oracle-exec/README.md` để có sweep và suy dẫn đầy đủ. | W6, W10, W12 |

**Đính chính, 2026-09-26 (W13b Task 5): W13b đã dựng xong, và câu "CHƯA bắt đầu" trong đính chính W13a được cho nghỉ chứ không bị xoá.** Migration 0158 thêm `workflow.flow_trigger_windows`, mỗi listener `dependsOn` một hàng, bị xoá theo cascade cùng listener của nó (một khác biệt có chủ ý: Kestra giữ lại). `models::Window` model bảy trường mà artifact đã ghim chấp nhận, có cả `timezone`, và từ chối `fireOnce` ngay lúc parse như artifact. Module thuần `trigger::window` port biên của bốn loại `TimeWindow` từ số đo của W13b Task 1, có cả DST và nửa đêm. Module thuần `trigger::depends_on` port khoá điều kiện (murmur3, tái tạo đúng các khoá artifact đã lưu), `merge` và `satisfied` theo ALL / ANY / AT_LEAST, và một danh sách từ chối dùng chung cho lúc lưu và lúc chạy. `flow_trigger::process` chuyển listener `dependsOn` sang nhánh riêng: nhánh này lấy hàng cửa sổ `FOR UPDATE` ở REPEATABLE READ, thay cửa sổ đã hết hạn khi có sự kiện khớp kế tiếp (quyết định D1 của người dùng, khác với việc Kestra bỏ qua), bắn khi mode được thoả và xoá cửa sổ dưới mọi mode; một lần thua race với `40001` được thử lại mà không tốn lượt. Ticker scheduler quét các cửa sổ hết hạn mỗi lượt một lần. Trong 20 khai báo `dependsOn` của corpus, **18 resolve được**, 1 bị từ chối lúc lưu (over-refusal chỉ-có-namespace của W1, đã nằm trong `executor::PUBLISHED_BUT_REFUSED`) và 1 không parse được (`fireOnce`); không khai báo nào có `window:`. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W13b, tại đỉnh nhánh, và cả khi nhánh `dependsOn` bị xoá, cả ba đo trên nhánh này. Mọi dòng của bảng §7.2 trong spec W13 đều đã chạy mutation của nó và mutation đều bị giết. Xem phần W13b của `oracle-exec/README.md`.

**Đính chính, 2026-09-27 (W11 Task 6): W11 đã dựng xong, và phép đo trong dòng của nó được cho nghỉ chứ không bị xoá.** Dòng ở trên ghi lại vì sao log rời khỏi W10: không có bảng `task_logs`, ba implementation của `LogSink` mà *"không cái nào ghi xuống lấy một dòng"*, handler container chỉ lưu *"đếm dòng chứ không phải nội dung"*, và *"W7 ship mà không có panel log"*. Cả bốn đều đúng tại `6749a37b` và là hồ sơ của quyết định hoãn; không câu nào còn đúng hôm nay. Dòng `task_logs` ở §6.1 ghi *"CHƯA DỰNG: W11"* cho tới đính chính này. Migration 0159 thêm `workflow.task_logs`. `LogSink` thành async và `logs::PgLogSink` ghi xuống: một channel có giới hạn 10.000 dòng, đầy thì handler đang ghi phải **chờ** chứ không bỏ dòng; một writer chèn theo lô 500 dòng hoặc mỗi 1 s, thử lại một lô lỗi trong 30 s, và nếu vẫn lỗi thì nêu số dòng mất trong `tracing::error!`; khi tắt, writer xả hết channel trong vòng 10 s. Mọi handler ghi qua một `RunLog` cho từng attempt: tách dòng dài hơn 15.360 ký tự như Kestra, chặn một attempt ở 10.000 dòng hoặc 10 MiB, và ghi một dòng WARN tổng kết nằm ngoài giới hạn. stdout/stderr của container và python được lưu **từng dòng, đủ nội dung** (stdout INFO, stderr ERROR, đo trên artifact đã ghim). `RunLog::finish` là một flush barrier giới hạn 30 s, nên các dòng của một run được chèn trước khi outcome của nó commit. Một pruner xoá các dòng cũ hơn `workflow.log_retention_days` (mặc định 30, `0` là giữ mãi) mỗi giờ, theo lô 10.000. `GET /api/ws/{workspace}/workflows/executions/{id}/logs` đọc chúng theo cursor, trễ 2 s sau writer, và báo `complete`. **Những gì vẫn CHƯA đúng:** registry production không có handler container hay python, nên hôm nay không run production nào lưu stdout hay stderr; không có SSE; và panel log là việc của W7. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W11 và tại tip của nó, cả hai đều đo: oracle so trace chứ không so dòng log. Xem phần W11 trong `oracle-exec/README.md` và §11 của spec thiết kế W11 về những gì `complete` bảo đảm và không bảo đảm.

**Đính chính, 2026-09-28 (W5a Task 6): W5 đã được tách, W5a đã dựng xong, và câu *"registry production không có handler container hay python"* trong đính chính W11 được cho nghỉ đối với container chứ không bị xoá.** W5a theo hình dạng của Kestra thay vì nới rộng `egret.container`: bốn script task dưới FQCN của Kestra, `egret.container` và `egret.script.python` giữ lại làm alias, và `taskRunner:` chọn Docker (`io.kestra.plugin.scripts.runner.docker.Docker`) hoặc một pod Kubernetes cho mỗi attempt (`io.kestra.plugin.ee.kubernetes.runner.Kubernetes`, chỉ có ở bản Enterprise của Kestra, được hiện thực ở đây dưới cùng FQCN); `Process` bị từ chối lúc lưu. `workflow.container_runner` (`none` | `docker` | `kubernetes`, mặc định **`none`**) quyết định production có đăng ký chúng hay không: với `none` registry y hệt của W11, nên deployment không đặt gì vẫn không lưu output script nào. Chạy một script còn cần feature workspace `workflow_containers`, feature đầu tiên **mặc định tắt**, và một image được `plugin_configurations[type].values.allowed-images` cho phép (không có mục hoặc danh sách rỗng thì **không** image nào được phép); một hàm gate duy nhất kiểm cả hai lúc publish và lần nữa lúc chạy, sau khi render, và từ chối lúc chạy thì không tạo pod hay container nào. Worker gia hạn lease mỗi `working_timeout / 3` dưới một fence, đóng nguyên nhân thứ hai của mục 19 trong `oracle-exec/README.md` đối với worker còn sống, và script task không có `timeout:` chạy dưới `container_default_timeout_s` (3600). Bảy trên mười lăm kiểu publish được vẫn chưa có handler trên production. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W5a và tại tip của nó, cả hai đều đo: corpus không khai báo script task nào. Xem phần W5a trong `oracle-exec/README.md`. **W5b chưa bắt đầu.**

**Đính chính, 2026-09-30 (W5b Task 7): W5b đã dựng xong, và câu *"W5b chưa bắt đầu"* trong đính chính W5a được cho nghỉ chứ không bị xoá.** Một task có `type:` là `kestra.<FQCN>` chạy một plugin Kestra thật, không sửa đổi: mỗi task-run attempt một JVM, trong image `egret-plugin-host` (Java 25, `io.kestra:core:2.0.0-rc13`, một `ShimRunContext` trong chính package của Kestra), trên runner đã cấu hình của W5a — mỗi attempt một pod trên Kubernetes. Jar đúng ba cái, `io.kestra.plugin:plugin-jdbc-postgres`, `plugin-jdbc-trino` và `plugin-jdbc-mysql` bản 2.0.0, ghi kèm sha256 trong `plugin-host/plugins.lock` và được kiểm lúc dựng image; `catalog.json` sinh từ chúng (năm lớp task; các kiểu đã port native và bốn lớp cần internal storage bị loại) và publish kiểm kiểu và thuộc tính theo nó mà không cần JVM. FQCN plugin viết trần bị từ chối kèm gợi ý cách viết có tiền tố. Feature workspace `workflow_kestra_plugins` mặc định tắt và cần `workflow_containers`; mục `kestra` của `plugin_configurations` thu hẹp catalog bằng glob `allowed-types` (không có mục thì cho phép cả catalog, danh sách rỗng thì không cho gì); một gate chạy lúc publish và lần nữa lúc chạy sau khi render, và từ chối lúc chạy thì không tạo pod nào. `secret('NAME')` và `secret('NAME', subkey=…)` giải lúc chạy, trong workspace của execution, chỉ trên task `kestra.*` (ghi chú ở §6.2); thuộc tính của task plugin render một lần, như mặc định của Kestra; mọi giá trị đã giải được thay bằng `******` trong các dòng log của run, còn output không bị che, như Kestra. `pluginDefaults:` bị từ chối lúc publish trên mọi flow, như Kestra 2.0 từ chối nó. Timeout dừng host bằng SIGTERM kèm grace `workflow.plugin_host.kill_grace_s` (mặc định 15 s, tối thiểu 3), trong đó host gọi `kill()` của plugin: query Postgres bị huỷ trên server (đo sống trên Docker và trên một cụm kind), query StarRocks thì không (spec W5b §12). **Cái vẫn chưa đúng:** plugin cần internal storage, KV hay namespace files thất bại có tên và chờ W9 *(cho nghỉ với internal storage bởi đính chính W9a bên dưới, 2026-10-04: KV vẫn chờ W9c và namespace files chờ W9b; cho nghỉ với namespace files bởi đính chính W9b bên dưới, 2026-10-07: chỉ KV còn chờ, chờ W9c; cho nghỉ với KV bởi đính chính W9c bên dưới, 2026-10-08: W9c đã xong KV store mà **không** có cầu nối plugin, nên plugin gọi `namespaceKv()` vẫn thất bại có tên, nay chỉ tới các task `core.kv`, và không chờ gói nào)*; trigger của plugin và jar ngoài ba cái nằm ngoài phạm vi. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W5b và tại tip của nó, cả hai đều đo: corpus không khai báo task plugin nào. Xem phần W5b trong `oracle-exec/README.md`.

**Đính chính, 2026-10-04 (W9a Task 14): W9 được tách, W9a đã xong, và câu *"plugin cần internal storage … chờ W9"* trong đính chính W5b được cho nghỉ với internal storage chứ không bị xoá.** Người dùng tách W9 ngày 2026-10-01 thành **W9a** internal storage, **W9b** namespace files và **W9c** KV store. W9a cho execution các file `kestra:///{namespace}/{flow}/executions/{exec}/…` của Kestra: `egret-storage` stream (multipart trên 8 MiB; không file nào bị giữ trọn trong bộ nhớ egret), object store là bucket của mục `[storage]` dưới `workflow.storage.prefix` (`workflow/`) và id của workspace (URI không mang workspace), và `workflow.execution_files` (migration 0166) đánh chỉ mục mọi file đã commit. **Một function duy nhất, `storage::resolve_readable_many`, quyết định URI có đọc được không**: flow đang chạy đọc file của chính execution, của họ vòng lặp, của execution trigger và họ vòng lặp của trigger đó, còn replay đọc file của chuỗi nguồn — không bao giờ của workspace khác hay của execution không liên quan. File được tra trong một snapshot chụp trước khi render (phán quyết của người dùng): URI ghép lúc render là lỗi nêu tên nó. Trên đó: input `FILE`, upload bằng start multipart; các function file `read`, `fileSize`, `fileExists` và `isFileEmpty`; API file của execution (tải xuống, metadata, preview CSV/JSONL/Ion tối đa 5000 dòng); `inputFiles` của script task nhận URI, egret stage vào thư mục làm việc; `outputFiles` và `outputDirectory` được thu thành URI; **cầu storage cho plugin** — **egret chuyển file thay cho pod, pod không giữ credential storage và không mở kết nối nào tới store**; `egret.sql` với `fetch: STORE`; chín task storage lõi (`Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`, `Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems`); `Loop` (tên `ForEach` của chương trình, `io.kestra.plugin.core.flow.Loop`) trên các giá trị của một file (migration 0167); và một pruner retention riêng, `workflow.storage_retention_days`, mặc định 30 ngày, `0` giữ mãi, giữ nguồn khi còn thứ phụ thuộc đang chạy (migration 0168). Trần: `max_file_bytes` 1 GiB, `max_task_bytes` 5 GiB, `max_read_bytes` 10 MiB.

**Thay đổi phá vỡ, và release note của nó.** (1) **`outputs.<task>.outputFiles` đổi từ nội dung sang URI**: W5a lưu nội dung từng file khai báo (UTF-8, tối đa 1 MiB) trong output; W9a lưu `{đường dẫn tương đối → URI kestra://}`, như Kestra. Flow từng đọc `{{ outputs.t.outputFiles['out.txt'] }}` như văn bản nay đọc `{{ read(outputs.t.outputFiles['out.txt']) }}`, hoặc chuyển URI sang `inputFiles` của task sau. Chỉ ảnh hưởng script task, vốn chạy sau feature workspace `workflow_containers`, **tắt mặc định**. (2) Container runner (`workflow.container_runner` là `docker` hoặc `kubernetes`) cần `[storage]`: server từ chối khởi động khi có cái này mà thiếu cái kia. (3) Image helper của task pod (`egret-task-helper`) cần `cat` cùng `sh`, `tar`, `touch`, `wc`, `sleep` và `tail`; base Alpine có sẵn, và Dockerfile kiểm tra. (4) Các hành vi khác của W5a đã đổi: luật 1 MiB mỗi file và UTF-8 cho file output không còn (thay bằng các trần trên); symlink, hard link hay thư mục trong output được thu làm task thất bại có tên; `outputDirectory: true` đã có (`$OUTPUT_DIR`, `{{ outputDir }}`); file được stage mang thời điểm upload và thuộc uid 0 trong archive (của W5a mang ngày 1970). (5) Của W5b đã đổi: `storage()` hoạt động thay vì exit 4 nêu W9; catalog có thêm bốn lớp từng cần storage (`postgresql.Batch`, `CopyIn`, `CopyOut`, `mysql.Batch`) và không giá trị thuộc tính nào bị từ chối lúc publish vì cần storage; namespace files exit 4 nêu W9b *(cho nghỉ bởi đính chính W9b bên dưới, 2026-10-07)* và `namespaceKv()` nêu W9c *(cho nghỉ bởi đính chính W9c bên dưới, 2026-10-08: nó vẫn exit 4, nay chỉ tới các task `core.kv`)*. (6) **Điều operator thấy mới**: `workflow.storage_retention_days` mặc định 30 (`0` giữ file mãi); file đã bị prune trả 404 từ API file và làm `read()` cùng các function file khác thất bại, nêu cửa sổ retention; API file (`file`, `file/metas`, `file/preview`) cần `workflow:read` và không thêm permission; `[workflow.storage]` `prefix`, `max_file_bytes`, `max_task_bytes`, `max_read_bytes` là key config mới; replay thủ công bị từ chối (400) khi một input `FILE` mà nó sao chép đã bị prune; pod plugin có giới hạn bộ nhớ mặc định 512Mi trên `-Xmx384m` của JVM, còn 128Mi bị OOM-kill; migration 0166-0168 (DB dev đã áp 0161/0169/0170 cũ, hoặc áp 166-168 mới trước commit 40455dc4 vốn thêm comment vào header — sqlx băm cả comment nên báo `VersionMismatch` — phải reset bằng `just reset-testdata`, hoặc xoá `(166,'workflow execution files')`, `(167,'workflow loop file values')`, `(168,'workflow execution files pruned')` khỏi `_sqlx_migrations` rồi drop những gì chúng tạo, và drop `egret_test_template`; công thức trong `oracle-exec/README.md`; không file migration nào bị sửa sau khi merge).

**Cái vẫn chưa đúng:** namespace files (W9b) *(đã xong 2026-10-07; xem đính chính W9b bên dưới)* và KV store (W9c) *(đã xong 2026-10-08; xem đính chính W9c bên dưới)*; `outputs:` của chính `Loop` ở chế độ STORE (`valids/loop-outputs-auto` vẫn baselined, phán quyết của người dùng ngày 2026-10-03); object storage cho **dòng log**, vẫn nằm trong Postgres (§11 câu hỏi 1; spec W9a §14); và `http`, bị từ chối có chủ đích từ 2026-10-17 vì nó sẽ chạy trong control plane. Ratchet là **109 mismatching trước W9a và 108 sau**, cả hai đều đo: `valids/loop-values-from-uri` được bank (một `Loop` trên URI của `Write`), đo ở W9a Task 11. Xem phần W9a trong `oracle-exec/README.md`, nơi cũng ghi lại end to end trên một cụm kind tạm.

**Đính chính, 2026-10-07 (W9b Task 11): W9b đã xong, và câu *"Cái vẫn chưa đúng: namespace files (W9b)"* trong đính chính W9a cùng câu *"namespace files exit 4 nêu W9b"* trong release note của nó được cho nghỉ chứ không bị xoá.** W9b cho workspace namespace files của Kestra (`docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md`): các file do tác giả soạn, đặt dưới một namespace, giữ **revision như Kestra giữ** (revision 1 ở `path`, revision N ở `path.vN`; xoá là xoá mềm; `PurgeFiles` xoá cứng), được đánh chỉ mục bởi `workflow.namespace_files` (migration 0174) và lưu trong bucket của W9a tại `{workflow.storage.prefix}{workspace}/{ns-as-path}/_files{path}[.vN]`. Flow gọi tên chúng bằng `kestra:///{ns-as-path}/_files{path}[.vN]` (không có workspace trong đó, như URI của W9a), `nsfile:///path`, `nsfile://other.ns/path` hoặc một path trần. **Một hàm duy nhất, `storage::nsfiles::access::authorize`, quyết định mọi lần đọc, ghi và purge**, và mọi hàm của store nhận `Grant` nó trả về, nên một lời gọi chưa được phép không type-check được: flow đang chạy đọc file của mọi namespace trong workspace của nó, chỉ ghi namespace của chính nó, và purge namespace của chính nó cùng các namespace con (theo ranh giới dấu chấm); workspace luôn là của execution (hoặc `{ws}` của route API sau khi kiểm quyền), không bao giờ được đọc ra từ URI. Trên nền đó: `fileURI` (rời danh sách bị từ chối), `read`, `fileSize`, `fileExists` và `isFileEmpty` trên namespace files, tra lúc render qua pool, vì namespace files commit ngoài `settle`; `namespaceFiles` trên bốn script task (`include`/`exclude` như `PathMatcherPredicate` của Kestra, `namespaces`, `folderPerNamespace`, `ifExists`), được stage trước `inputFiles`, các namespace theo thứ tự trong danh sách; giá trị `inputFiles` là URI namespace file; bốn task `io.kestra.plugin.core.namespace` `UploadFiles`, `DownloadFiles`, `DeleteFiles` và `PurgeFiles`, là handler gốc trong worker; API namespace file dưới `/api/ws/{ws}/workflows/namespaces/{ns}/files`; và `storage().namespace(ns)` của cầu nối plugin, chỉ đọc trên các file egret đã stage cho task, nên **pod vẫn không giữ credential storage nào** (đo trên một cụm kind, kể cả pod của cầu nối). Revision được lấy dưới một advisory lock Postgres theo từng path, và body được **stage** trước: nó stream vào một object staging trong khi không giữ connection nào và lock nào, rồi một transaction ngắn copy nó lên key của revision ở phía server.

**Phán quyết của người dùng** (mỗi cái có ngày trong header spec W9b): revision như Kestra; flow đọc mọi namespace trong workspace của nó và chỉ ghi namespace của chính nó; đọc cần `workflow:read`, ghi cần `workflow:manage`, không có quyền mới; **`WorkingDirectory` nằm ngoài** (`namespaceFiles` của nó được kiểm lúc publish và giữ trong model, không bao giờ chạy); `inputFiles` luôn ghi đè namespace file cùng tên, bất kể `ifExists`; namespace có segment `_files` bị từ chối ở mọi nơi; URI namespace file được chấp nhận làm giá trị `inputFiles`; **R1** lịch sử đã xoá mềm vẫn thấy được (`revisions` liệt kê có đánh dấu, đọc có revision tường minh thành công, chỉ đọc bản mới nhất là not-found); **R2** `PurgeFiles.namespacePattern` là wildcard của Kestra; **R3** `fileURI` của thư mục là lỗi, search như Kestra, không route GET nào tạo ra thứ gì; `_files` chỉ đánh dấu namespace file khi đứng trước cặp `executions/{uuid}` đầu tiên, và flow id `_files` bị từ chối; path tối đa 512 byte UTF-8; move lên một đích có bất kỳ lịch sử nào bị từ chối và một lần ghi không bao giờ ngầm purge lịch sử; tên chứa ký tự glob được stage như Kestra, bốn thuộc tính danh sách/enum của `namespaceFiles` nhận biểu thức, file script của chính task thắng namespace file; `PurgeFiles` không có cả `keepAmount` lẫn `before` bị từ chối và `deleteParentFolder` như Kestra; body được stage trước mọi lock; ngân sách glob là 256 KiB mỗi danh sách; export đọc mỗi file hai lần (một lượt tính CRC trước) để `ZipInputStream` của Java, thứ mà phần import của Kestra dùng, đọc được, kèm `Content-Length` chính xác.

**Release note.** (1) **Một config key mới**, `workflow.storage.max_namespace_files` (env `EGRET_WORKFLOW__STORAGE__MAX_NAMESPACE_FILES`), mặc định **10 000**, tối thiểu 1: số dòng nhiều nhất mà một lần liệt kê, search, tập staging, upload hay một trang purge thấy, đếm trên những gì egret đã lấy về; vượt quá thì task thất bại hoặc trả 413, nêu tên giới hạn. Và `workflow.storage.prefix` dài hơn **200 byte** giờ bị từ chối lúc khởi động, để key của namespace file nằm dưới 1024 byte của S3. (2) **Migration 0174** tạo `workflow.namespace_files` và `workflow.namespace_file_staging`. Nó được viết là 0169 và đánh số lại trước khi merge, lúc rebase lên `edccf6c4`, nơi main sở hữu 0169–0173; một database dev đã áp `(169, 'workflow namespace files')` của nhánh thì reset bằng `just reset-testdata`, hoặc xoá dòng đó khỏi `_sqlx_migrations`, drop hai bảng và drop `egret_test_template` (công thức trong `oracle-exec/README.md`). Không file migration nào bị sửa sau khi merge. (3) **Bảng staging và đợt quét của nó.** Body của mọi lần ghi được upload trước tiên lên `{prefix}{ws}/_nsfiles_staging/{uuid}`, kèm một dòng trong `workflow.namespace_file_staging`; lần ghi xoá cả hai trên mọi lối ra, và một **đợt quét mỗi giờ**, chạy bởi mọi tiến trình egret có store, xoá mỗi object staging có dòng cũ hơn **24 giờ** (theo đồng hồ của database), rồi xoá dòng. **Giới hạn:** một upload qua API có body ngừng 30 giây bị từ chối (408), và upload nào mất hơn **12 giờ** tổng cộng cũng vậy; body phản hồi của export kết thúc bằng lỗi sau **12 giờ**, ngắn hơn `Content-Length` của nó; một export đọc tổng cộng tối đa `max_task_bytes` (413 trước byte đầu tiên). Một namespace file bị giới hạn ở `max_file_bytes` và, dù cấu hình thế nào, ở **5 GiB**, giới hạn của một lần S3 CopyObject: bị từ chối, không copy theo từng phần. *(2026-10-07, I-1 của review cuối.)* Một **move** chép mỗi revision ở phía server, không bao giờ qua egret, và chép tối đa `max_task_bytes` và `max_namespace_files` dòng (413 trước mọi lần chép, nêu trần và con số). Một upload, tạo thư mục, move hay delete qua API có path bị move hay lần ghi khác giữ thì chờ tối đa **5 s**, rồi trả **409** "the path is busy … try again"; `UploadFiles`, `DeleteFiles` và `PurgeFiles` của flow tiếp tục chờ. (4) **Publish giờ từ chối namespace có segment `_files`**, và flow id `_files`, có nêu tên. W9b Task 1 đếm được 0 trên 345 flow của corpus và 0 trên 5 flow đã publish mà nó tìm thấy trên Postgres dev (38 database), nên không flow nào đã biết bị ảnh hưởng; namespace có segment rỗng (`a..b`) vẫn publish được nhưng không dùng được namespace files. (5) **`namespaceFiles` publish được trên script task** (W5a từ chối nó lúc lưu), và giá trị `inputFiles` có thể là URI namespace file. (6) **`WorkingDirectory.namespaceFiles` được kiểm lúc publish**: một khối sai dạng, trước đây egret lặng lẽ bỏ đi, giờ publish thất bại và nêu tên thuộc tính; lúc chạy không có gì đổi. (7) **Chín route API cùng export**, dưới `/api/ws/{ws}/workflows/namespaces/{ns}/files`: download, `stats`, `revisions`, `search`, `directory` và `export` cần **`workflow:read`**; upload, tạo thư mục, move và delete cần **`workflow:manage`**; không có quyền mới. (8) **Plugin host không còn exit 4 với `storage().namespace(..)`**; một lần ghi namespace của plugin ném `UnsupportedOperationException` kèm lý do, và `namespaceKv()` vẫn exit 4 nêu W9c *(2026-10-08, W9c: nó vẫn exit 4, nay chỉ tới các task `core.kv`; xem đính chính W9c bên dưới)*. (9) **Giới hạn đã biết** (spec W9b §15): flow chỉ ghi namespace của chính nó, và `PurgeFiles` mặc định là nó, trong khi flow của Kestra ghi ở đâu cũng được; object thiếu sau một dòng là lỗi, không lùi về revision cũ hơn; `read` một file nhị phân là lỗi; `UploadFiles.files` glob trên các file của execution, không phải thư mục làm việc; không có retention cho namespace files; URI `kestra://` không được percent-encode, nên tên chứa dấu cách làm hỏng plugin nào parse nó bằng `new URI` (`CopyIn`), để làm tiếp sau; `filePattern` được so với path đã bỏ `/` đầu, như Kestra, nên `"/up/*"` không purge gì; plugin chỉ đọc được namespace file mà một giá trị thuộc tính nguyên vẹn nêu tên; namespace files sống lâu hơn flow của chúng (không có bảng namespace).

**Cái vẫn chưa đúng:** chạy `WorkingDirectory` (và do đó `namespaceFiles`, `inputFiles`, `outputFiles`, `cache` của nó); plugin ghi namespace files; KV store (W9c) *(đã xong 2026-10-08; xem đính chính W9c bên dưới)*; bung `.zip` và import `_flows` khi upload; UI (W7). Ratchet là **108 mismatching trước W9b và 108 sau** (294 golden entry, 0 broken), cả hai đều đo: không golden entry nào chạm tới namespace files, và hai flow `WorkingDirectory` của corpus khai báo `namespaceFiles` đứng ngoài ratchet, vì artifact đã từ chối cả hai lúc publish. End to end trên một cụm kind tạm qua 7 trên 7 kịch bản. Xem phần W9b trong `oracle-exec/README.md`.

**Đính chính, 2026-10-08 (W9c Task 8): W9c đã xong, và mục *"KV store (W9c)"* trong *"Cái vẫn chưa đúng"* của đính chính W9a và W9b, câu *"`namespaceKv()` vẫn exit 4 nêu W9c"* trong release note W9b và câu *"Chỉ KV store (W9c) vẫn exit 4"* ở phát hiện 4 của §3 được cho nghỉ chứ không bị xoá.** W9c cho mỗi workspace KV store của Kestra (`docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md`): giá trị có type (STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION, JSON) dưới một key theo namespace, **có revision, như Kestra giữ** (mỗi revision là một object Ion text tại `{workflow.storage.prefix}{workspace}/{ns-as-path}/_kv/{key}.ion`, hoặc `….ion.vN` với revision N > 1; xoá là xoá mềm), được đánh chỉ mục bởi `workflow.kv_entries` (migration 0175, một dòng mỗi revision, type được **lưu**), với TTL theo đó **hết hạn nghĩa là không có** và **đọc không bao giờ ghi**. Flow đọc nó qua function `kv()` (rời danh sách bị từ chối, nay còn sáu tên) và sáu task `io.kestra.plugin.core.kv`: `Set`, `Get`, `GetKeys`, `Delete`, `Put` merge sâu và `PurgeKV`, là các handler native trong worker; `kv()` không có namespace và `Get` trên namespace của chính flow đi ngược lên các namespace cha theo ranh giới dấu chấm, như của Kestra. **Cùng một hàm truy cập với W9b, `storage::nsfiles::access::authorize`, quyết định mọi lần đọc, ghi và purge KV**, nay với hai resource, namespace files và KV store (`Resource::Kv`); mọi hàm của KV store nhận `Grant` mà nó trả về và đòi đúng resource của mình, nên một grant namespace-file không chạm được KV và một grant đọc không ghi được: flow đang chạy đọc KV của mọi namespace trong workspace của nó, chỉ ghi và xoá trong namespace của chính nó, và `PurgeKV` tác động lên namespace của nó và các namespace con (theo ranh giới dấu chấm); workspace luôn là của execution, hoặc `{ws}` của route API sau khi kiểm quyền. Một lần ghi tái dùng staging của W9b: giá trị được stage khi không giữ connection hay lock nào, rồi một transaction ngắn dưới advisory lock của key kiểm `overwrite`, chọn revision, copy phía server và chèn dòng; `Put` đọc, merge và ghi dưới lock đó. API KV nằm dưới `/api/ws/{ws}/workflows/namespaces/{ns}/kv`. Plugin host **không** có cầu nối KV.

**Phán quyết của người dùng** (mỗi cái có ngày trong header spec W9c, 2026-10-07 và 2026-10-08): phạm vi trên, không có cầu nối plugin; phạm vi ghi của W9b; storage như Kestra, type được lưu; hết hạn nghĩa là không có và tôn trọng `errorOnMissing`, và một sweep mỗi giờ xoá cứng key hết hạn; kế thừa như Kestra; `secret()` trong giá trị `Set`/`Put` bị từ chối lúc publish; **lần ghi một giá trị bị chặn ở `max_read_bytes`**, không phải `max_file_bytes`, nên cái gì ghi được thì đọc được; version và xoá mềm như R1 của W9b; bulk delete chặn ở 1.000 key; các lời gọi store do server điều nhịp dưới lock của key được chấp nhận; `Put` trên key hết hạn bắt đầu lại từ đầu; namespace của workspace khác trả lời như namespace rỗng; `Put` là **merge sâu thật** với ngữ nghĩa `Put.mergeValues` của artifact, dù trên artifact giá trị của flow đến `Put` dưới dạng JSON text và được lưu thành STRING; body `PUT` của API là **đúng một giá trị Ion** lưu theo type của nó, còn lại là text thô, type STRING (R-b: body rỗng là 400; R-c: timestamp độ chính xác năm hoặc tháng là text thô); `before` của `PurgeKV` như Kestra (một revision bị purge khi revision kế tiếp của nó được tạo lúc hoặc trước mốc đó); R-a: `Set` lưu map hoặc list YAML có type; **số thập phân được giữ chính xác**; giới hạn giá trị dày; mọi số tối đa 1.000 chữ số; các key có dấu chấm xung đột trong một `Put` bị từ chối; cả hai giới hạn của `Put`; **so sánh trên số chính xác là chính xác**, qua phương án A (các toán tử so sánh được biên dịch thành lời gọi), float được so theo giá trị nhị phân chính xác của nó; renderer của egret giữ phép chia float (`7 / 2` là `3.5`), một việc tiếp theo cho toàn renderer.

**Release note.** (1) **Migration 0175** tạo `workflow.kv_entries`. Không có config key mới, không có permission mới. (2) **`workflow.storage.max_read_bytes` nay phải ít nhất 4096** (`egret_config::MIN_MAX_READ_BYTES`), **kiểm lúc boot**: một giá trị nhỏ hơn làm tiến trình từ chối khởi động, nêu mức tối thiểu, vì đó là mức ít nhất mà bộ đọc Ion của một giá trị KV cần. Không giá trị chart hay env deploy nào đặt nó (0 lần dưới `deploy/`, bên cạnh 18 dòng `EGRET_WORKFLOW__`). (3) **Sweep hết hạn mỗi giờ.** Mọi tiến trình egret có store đều chạy nó, lần đầu một giờ sau khi khởi động (`interval_at`), trên mọi workspace: nó xoá cứng mỗi key có revision mới nhất đã hết hạn, cả dòng lẫn object, theo row id dưới session lock của key, bỏ qua key đang bận, nên an toàn trên mọi replica. Trước khi nó chạy, key hết hạn vẫn giữ dòng và object, và mọi bộ đọc đã coi nó là không có. (4) **Một giá trị KV tối đa `max_read_bytes`** (mặc định 10 MiB) sau khi mã hoá, trên `Set`, `Put` (kết quả đã merge) và `PUT` của API; `max_file_bytes` không áp dụng. **Giới hạn giá trị dày**: một giá trị chứa tối đa `max_read_bytes`/64 giá trị Ion (163.840 ở mặc định, khoảng 320 KB số nhỏ), tính cả chính nó và mọi giá trị lồng bên trong. **Giới hạn 1.000 chữ số**: mọi số trong một giá trị (int, hệ số của decimal ở dạng plain, float, số mũ) có tối đa 1.000 chữ số, được đếm trước khi parser Ion thấy nó, trên mọi lần ghi và mọi lần đọc. Mỗi lần từ chối nêu giới hạn của nó (413 trên API), và không gì bị cắt cụt. (5) **Publish từ chối `secret()` trong giá trị `Set` hoặc `Put`** (`secret_in_kv_value`), với thông báo nói giá trị của secret sẽ bị lộ qua KV store, và **namespace có segment `_kv`** (0 trên 345 flow của corpus và 0 flow đã publish trên Postgres dev, W9c Task 1). (6) **Decimal và so sánh chính xác.** Một decimal Ion (chỉ `PUT` của API tạo ra) vẫn là decimal qua mọi lần đọc, merge và ghi, chữ số chính xác; nó được ghi với các số 0 cuối sau dấu chấm bị bỏ (`1.50` đọc về là `1.5`). Một decimal hoặc số nguyên lớn mà `kv()` trả về được so chính xác dưới `==`, `!=`, `<`, `<=`, `>`, `>=`, `in` và `not in`. Để làm vậy, **một pass rewrite so sánh mới** biên dịch mọi toán tử so sánh trong mọi template thành một lời gọi (`a > b` → `kestra_gt(a, b)`), trả lời như minijinja cho mọi cặp toán hạng khác. Hai hệ quả thấy được với mọi template, không chỉ template KV: **tên bắt đầu bằng `kestra_` được dành riêng**, và một `set`, `for` hay `with` gán một tên như vậy thất bại lúc biên dịch; và **một so sánh nối chuỗi (`a < b < c`) nay đánh giá mọi toán hạng**, nên một toán hạng mà việc đánh giá thất bại (hoặc `kv()` của một key không có) làm render thất bại kể cả khi một so sánh trước đó đã sai. Phép tính số học trên decimal chính xác là một lần từ chối có tên. (7) **Giới hạn của `Put`**: key có dấu chấm của một mục có key tối đa 100 segment, một patch tối đa `max_read_bytes`/64 segment có dấu chấm tổng cộng, và một patch lồng tối đa 100 tầng; các key có dấu chấm xung đột (`q.r` với `q`) bị từ chối, nêu cả hai. (8) **Bảy route API** dưới `/api/ws/{ws}/workflows/namespaces/{ns}/kv`: listing, `inheritance`, giá trị của một key (`?revision`) và `revisions` của nó cần **`workflow:read`**; `PUT` một key, `DELETE` một key và `DELETE` hàng loạt cần **`workflow:manage`**. Một writer chờ lock của key tối đa 5 s, rồi trả 409; body `PUT` giữ timeout của W9b (408); `DELETE` hàng loạt nhận **tối đa 1.000 key** và body tối đa **1 MiB**, một transaction tất-cả-hoặc-không, vượt quá trả 413 nêu giới hạn; không có store trả 503. (9) **Thông báo `namespaceKv()` của plugin host** nay là *"namespaceKv(): egret gives flows the KV store through the `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's RunContext; use a `core.kv` task before or after this one."*, và tiền tố exit-4 của worker là *"the plugin needs a Kestra service egret does not provide to plugins"*. (10) **Giới hạn đã biết** (spec W9c §15): flow chỉ ghi và xoá KV của namespace của chính nó, và `PurgeKV` mặc định là namespace của nó và các con, trong khi flow của Kestra chạm tới mọi namespace; `expiredOnly: false` bị từ chối, thay bằng `version` với `keepAmount: 0`; một key tên `inheritance` không được đọc qua `GET …/kv/inheritance` (đó là listing), chỉ đọc được khi một chữ cái được percent-encode; ttl tối đa 1.000 năm; một số chính xác so với float được so với giá trị nhị phân chính xác của float, trong khi Pebble so `doubleValue()`; test `is null` của Pebble không được bind (viết `== null`); `outputs.value` của `Get` đến một render sau dưới dạng `f64`; DATETIME, DATE và DURATION đọc về là chuỗi; mẫu đọc-tăng-`Set` có race, như trong Kestra; một lần xoá object thất bại của sweep để lại object mồ côi; xoá workspace để lại các object `_kv` của nó; khoảng bảy bulk delete 1.000 key đồng thời trên các replica khác nhau có thể lấp đầy bảng lock của một Postgres mặc định; và các cửa sổ do server điều nhịp dưới lock của key (đọc và ghi có giới hạn của `Put`, copy và `head` của một lần ghi, các lần xoá object của purge và sweep) được chấp nhận. (Task 9, phán quyết của người dùng về F2 của bản review cuối, 2026-10-08) **Một lần render bên trong transaction không còn đọc.** `kv()`, các hàm namespace file, `read()` và file values của một `Loop` trong `settle` đọc qua một cache theo lần gọi, được nạp khi không có transaction nào mở: một lần miss render một giá trị giữ chỗ để một lần thử tìm ra mọi lần đọc nó chạm tới, lần thử rollback cùng mọi thứ nó tính ra, các lần đọc được tải rồi transaction chạy lại; mỗi lần gọi tối đa 256 mục được tải, 64 vòng và 4 x `max_read_bytes` được cache, mỗi giới hạn là lỗi của tác giả nêu giới hạn đó và lần đọc, được ghi làm thông điệp của run thất bại (phán quyết của người dùng về C1 của bản review Task 9, 2026-10-08). Giá trị được commit là của một lần render trực tiếp; một lần render có đọc làm transaction tốn thêm một lần thử, cộng một lần cho mỗi tầng phụ thuộc điều khiển thật sự, chỉ xảy ra khi giá trị giữ chỗ chọn một nhánh mà giá trị thật không chọn (ví dụ một `runIf` mà giá trị giữ chỗ làm thành đúng còn giá trị thật là sai, nên lần đọc của task kế tiếp chỉ được chạm tới ở lần thử sau). **Hai lỗi có sẵn của `Loop` được sửa cùng lúc** (fix round 2 của Task 9): một `Loop` có các vòng lặp kết thúc ngay trong settle đầu tiên của chúng (một `If` không có `else` với điều kiện sai) không còn lồng thêm một settle trên stack cho mỗi vòng lặp -- 66 vòng lặp như vậy làm tiến trình dừng vì tràn stack, và một giá trị giữ chỗ có thể lái một flow vào đường đó; phần đuôi của một settle nay chạy từ một stack công việc tường minh -- và một `Loop` có `concurrencyLimit: 0` (hay bất kỳ lô nào) mà các vòng lặp đầu kết thúc ở đó không còn làm hỏng cả transaction với một `workflow_executions_loop_run_idx` trùng: mọi dòng của một lô được tạo trước khi dòng nào trong đó settle. *(Sửa 2026-10-09, fix round 3 của Task 9: vòng walk của round 2 chỉ sửa dạng một tầng. Một `Loop` tuần tự có `Subflow` bị từ chối (dừng tiến trình từ khoảng 10 vòng lặp) và một `Loop` tuần tự nằm trong một `Loop` tuần tự khác (từ 50 vòng lặp ngoài) vẫn lồng thêm một settle cho mỗi vòng lặp, và một giá trị giữ chỗ có thể lái một flow đang chạy tốt vào cả hai. Từ round 3, mọi settle của một transaction -- các settle đầu tiên của một lô, của một execution con `Subflow`, việc hoàn tất một `Subflow` bị từ chối hoặc `wait: false`, một replay -- đều chạy từ một vòng walk đó, nên stack Rust không còn lớn lên theo số vòng lặp hay độ lồng.)*

**Cái vẫn chưa đúng:** cầu nối KV cho plugin (`namespaceKv()` của plugin vẫn exit 4); `AllowFailure`, nên `valids/retry-with-flowable-errors` nay publish được nhưng vẫn baselined; che giá trị KV trong log hoặc output; UI (W7). Ratchet là **108 mismatching trước W9c và 108 sau** (294 golden entry, 0 broken), cả hai đều đo: không golden entry nào đọc KV store ngoài `retry-with-flowable-errors`. End to end trên một cụm kind tạm qua 6 trên 6 kịch bản, sweep mỗi giờ được quan sát trực tiếp; plugin host dưới một container runner, và các 409, 413, 408, 503 trực tiếp, không được chạy ở đó. Xem phần W9c trong `oracle-exec/README.md`.

**Đính chính, 2026-10-16 (AllowFailure flowable): đã dựng trên `feat/workflow-allow-failure-flowable` và đang review, chưa merge; câu *"`AllowFailure`, nên `valids/retry-with-flowable-errors` giờ publish được nhưng vẫn nằm trong baseline"* của đính chính W9c được gỡ bỏ thay vì xoá.** Một `AllowFailure` dạng block giờ chạy được: một con thất bại làm block thành `WARNING`, `errors:` và `finally:` của nó chạy, và task sau block vẫn chạy. Bảng được đo trên image đã ghim, `kestra/kestra@sha256:28f59bf175e1…` (`docs/en/specs/2026-10-16-workflow-allow-failure-flowable-design.md` §5.1). Đã bank: `valids/allow-failure`, `valids/allow-failure-with-retry` và `valids/finally-allowfailure`, ba entry rời `oracle-exec/baseline.txt` (107 entry trước, 104 sau). `valids/retry-with-flowable-errors` vẫn nằm trong baseline: `retry:` trên chính block bị từ chối là `unsupported_allow_failure_retry`, đúng non-goal của §2. Image retry con thất bại trong flow đó, nên retry trên một flowable là câu hỏi của gói sau, không phải một phép đo còn thiếu. Cờ `allowFailure:` (W8) không đổi.

**Đính chính, 2026-10-17 (WorkingDirectory): đã dựng trên `feat/workflow-working-directory`, chưa merge; mục *"chạy `WorkingDirectory` (và do đó `namespaceFiles`, `inputFiles`, `outputFiles`, `cache`)"* của đính chính W9b được gỡ bỏ thay vì xoá.** `WorkingDirectory` giờ chạy được, trên Docker runner. Các con của nó dùng chung một thư mục: executor điều khiển block như một flowable, và giữa các con thư mục được mang đi dưới dạng snapshot, copy ra từ container của con vừa xong rồi vào container của con kế tiếp (`docs/en/specs/2026-10-16-workflow-working-directory-design.md` §4.2 và §5.2). `namespaceFiles`, `inputFiles`, `cache` và `outputFiles` của block chạy trong các claim prepare và finish của nó (§4.4). Một con mà runner không phải Docker thì thất bại đích danh trước khi chạy, và block dừng, nên không có gì chạy trên thư mục rỗng. **Vẫn chưa đúng:** `WorkingDirectory` trên Kubernetes. Đường copy-in của nó chưa dựng, và điểm mở 1 của spec hoãn nó; cho tới lúc đó con trên Kubernetes bị từ chối đích danh, không được chạy. Các con flowable của một block vẫn bị từ chối (§9 của spec đó).

Oracle, đo trên image đã ghim: sáu flow `working-directory-real-*` là các entry corpus chạy thật (WorkingDirectory Task 8; corpus 345 lên 351, golden 294 lên 301). `valids/working-directory-taskrun` được bank và `valids/retry-failed-task-duration` nằm trong baseline kèm nguyên nhân (độ trễ scheduler mỗi lần retry của Kestra, khoảng 0,6 s, dừng nó ở ba lần thử; egret làm bốn lần trong 5 trên 5 vòng), nên `oracle-exec/baseline.txt` giữ **104** entry, census được tính lại cho khớp. Bộ test executor qua với baseline đó. Trong bốn valid WorkingDirectory, `working-directory-taskrun` khớp và ba cái còn lại nằm trong baseline theo nguyên nhân (`task-runif-workingdirectory`, `working-directory-invalid-runif`, `working-directory-taskrun-nested`). **Chạy thật, 2026-10-17:** flow corpus `working-directory-real-shared-file` chạy một lần qua Docker runner trên stack riêng (một Postgres riêng và object store trong bộ nhớ của tiến trình này, không phải stack dev dùng chung hay RustFS), với đường snapshot thật. Block `wdir`, con `writer` và con `reader` đều kết thúc `SUCCESS`, và dòng log mà reader đọc lại là `READER_SAW=shared-content`, nội dung `writer` đã ghi vào `shared.txt`. Xem `docs/en/specs/2026-10-16-workflow-working-directory-design.md` §7.

**Đính chính, 2026-10-18 (kill và `Exit`): kill ở mức execution và task `Exit` đã được ship**, trên `feat/workflow-kill-exit` (`docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`, plan `docs/en/plans/2026-10-17-workflow-kill-and-exit.md`). "kill/restart" của W7, vốn bị bỏ khỏi W7, và kill ở mức execution của W8, vốn chưa bao giờ được ship, được cho nghỉ về phần kill chứ không bị xoá. `POST /api/ws/{workspace}/workflows/executions/{id}/kill?cascade=true|false` (`workflow:manage`, có audit) chuyển execution sang `KILLING`, rồi sang `KILLED` khi công việc của nó đã dừng: worker phát hiện run của nó là `KILLING` bằng cách poll mỗi `workflow.kill_poll_ms` (env `EGRET_WORKFLOW__KILL_POLL_MS`, mặc định 5000, tối thiểu 100), cancel handler và ghi `KILLED`, và run có worker đã chết được claim kế tiếp hoàn tất. `cascade` (mặc định `true`) kill các child của subflow và của loop. Trang execution có nút Kill. `io.kestra.plugin.core.execution.Exit` chạy ở mức executor; bốn mục `valids/exit*` đã bank, nên `oracle-exec/baseline.txt` còn **100** mục (trước là 104; refused từ 45 xuống 41). Không có migration. **Vẫn chưa đúng:** kill một task run, kill hàng loạt, restart, đo live kill trên Kubernetes, và giữ lại container `taskRunner.delete: false` bị kill (cancel xoá nó, như cancel của W5a vẫn luôn làm; xem ghi chú §4.2 của spec đó).

**2026-10-18, nhóm C1: `OutputValues`, `Assert`, `PurgeLogs` (#691).** Ba task worker đã được port (`docs/{en,vi}/specs/2026-10-18-workflow-core-tasks-design.md` §2). Ratchet đi từ 100 xuống **84** mismatching / baselined, 16 entry được bank, và số LIVE của census từ 254 lên 281. `PurgeLogs` mặc định bị giới hạn trong namespace của flow và các namespace con, còn một `namespace` được ghi thì phải qua quy tắc purge (`Access::Purge`, như `PurgeFiles` và `PurgeKV`), theo phán quyết của người dùng (spec C1 §2.3); `valids/purge_logs_full_arguments` là dòng baseline vĩnh viễn vì lý do đó. Nhóm C2 (các task chạy trong executor) là bước tiếp theo.

**2026-10-19, gói nợ workflow, PR-1 (kill và WorkingDirectory).** Đã xây trên `feat/workflow-debt-kill-wd` (`docs/{en,vi}/specs/2026-10-19-workflow-debt-design.md`, commit `571f3d4e`, `f1d5d4b4`, `ba8c6d33`, `791d6d58`, `8a0e889e`, `51f5b8a2`, `de0e34db`, `df2759d5`, `d972fb0a`). Đã ra: container `taskRunner.delete: false` bị kill hoặc hết thời gian trên Docker được dừng và giữ lại (D1; Kubernetes vẫn xoá pod, D2); container được giữ bị orphan sweep xoá khi run của nó đã terminal và cũ hơn setting mới `workflow.kept_container_retention_days` (env `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS`, mặc định 7, `0` giữ mãi; D3); kill mà không load được execution thì đóng cưỡng bức thành `KILLED` thay vì lặp (D4); lần retry deadlock của `commit_kill` đã có test (D5); **migration 0196** thêm `task_runs.working_dir_swept_at` cho `_wd` sweep mỗi giờ của các snapshot mà một block WorkingDirectory bị crash bỏ lại, sweep này cũng quét các block `KILLED` bị force-close (D6); và phần dựng oracle của suite executor báo nguyên nhân thật thay vì "flows must be empty" (D7, "flake 335"). **D14:** driver của oracle nay chạy tiếp một run có hạn ngắn đã qua khi máy tải (một bản sửa flake trong harness test, không phải production). Giới hạn "mode 2" vẫn còn: khi máy tải rất nặng, `valids/waitfor-child-task-warning` vẫn có thể khớp ngẫu nhiên và làm ratchet fail với "PROGRESS NOT BANKED", vì thời gian thực rò vào đồng hồ Tick; cách sửa là một đồng hồ ảo trong driver, và entry vẫn nằm trong baseline. Ratchet là **301 golden entry, 73 mismatching, 73 baselined, 0 broken, với census LIVE 293 (351 flow corpus: 293 live, 48 draft-only, 6 unparseable, 4 name-contested)** (không đổi so với trước PR-1). Nguồn: kết quả in ra của `cargo test -p egret-workflow --test executor` trong các lần chạy Task 1.6 và Task 1.6b (`.superpowers/sdd/2026-10-19-workflow-debt/task-1.6-report.md` và `task-1.6b-report.md`), giống hệt baseline trước PR-1. PR-2 (phần theo sau của C2, nợ C1) là tiếp theo.

**W10, W11 và W12 được thêm ngày 2026-09-14, sau khi W7 hoá ra bị chặn, và lý do đáng giữ lại.** **Bốn** thứ dòng W7 cần — một tiến trình đẩy execution đi tiếp, một bề mặt REST execution, `workflow.task_logs`, và một vòng kích hoạt trigger `cron:` — đều được thiết kế ở §6.1 và §6.3 của chính tài liệu này mà **không dòng nào của bảng sở hữu**. Thứ tư tìm ra trong lúc đo schema cho dòng W11 chứ không phải bằng cách đọc bảng, nên nó được nêu riêng ở dưới. Kiểm bằng máy chứ không đọc bằng mắt: grep bảng này **ở trạng thái TRƯỚC khi có ba dòng W10, W11 và W12 bên trên** với `task_logs|execution api|ExecutionController|wire|binary|daemon` trả về **0**, trong khi cùng lượt đó `reaper` trả về 1 và `W3b` trả về 6; còn `ExecutionController` xuất hiện **đúng một lần** trong spec này — ở §8, *"Không port, nhưng bắt buộc đọc trước W7"*, nơi nó được nêu như một **hình dạng để nghiên cứu**, chưa bao giờ như một hạng mục phải giao. Một dòng gọi tên một thứ trong kiến trúc mà không bao giờ gọi tên nó trong kế hoạch, với người đi soát coverage, đọc y hệt một dòng đã xong. Suy dẫn đầy đủ nằm ở `docs/{en,vi}/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md` §1.

**Vòng trigger — thứ tư — chính là W12 ở trên.** Còn lại **hai cái tên không dòng nào trong bảng này sở hữu**, ghi ở đây thay vì để người sau phát hiện lại: hai function biểu thức `secret` và `http`, mà bảng `REFUSED` trong `crates/egret-workflow/src/runners/functions.rs` gán cho W9 **chỉ để thông điệp từ chối có một cái tên chủ sở hữu**, theo đúng lời chính file ấy — *"`secret` và `http` không nằm trong nội dung của gói nào cả"*. **Ở đây cố ý KHÔNG gán chủ cho chúng**: gán tên một lần nữa chỉ làm thông điệp thay đổi mà không làm nó đúng hơn. Chúng được đánh dấu cho ai viết bản review cấp chương trình tiếp theo, và chúng cùng một hình dạng với bốn thứ mà §1 của W10 ghi lại. *(Đính chính 2026-09-30, W5b Task 7: `secret` nay đã có chủ. W5b giải `secret()` trên task plugin `kestra.*`, từ secret của workspace chứa execution (ghi chú ở §6.2), và nó vẫn bị từ chối ở mọi chỗ khác — task script, input, `runIf`, `Log`, trigger — kèm lý do nêu chỗ nó được giải. `http` vẫn không dòng nào sở hữu.)* *(Quyết định 2026-10-17: `http` bị từ chối vĩnh viễn, `Refusal::Unsupported`; biểu thức được render trong server egret, nên một request phát ra từ đó là server-side request forgery. Xem `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §2.)*

Mỗi gói có spec và plan riêng, như chương trình DBAPI-parity đã làm.

## 10. Cố ý để ngoài v1

`pause`/`resume`, backfill, `allowFailure`, `loopUntil`, concurrency limit và
dynamic task thuộc W8, không thuộc W3. Đó là nơi khối test của Kestra nằm, và mỗi
thứ đều là phần **cộng thêm** trong YAML, nên hoãn chúng không bỏ rơi flow nào đã
được viết.

## 11. Câu hỏi mở

1. Dòng log nằm trong Postgres kèm cửa sổ retention ở v1; object storage là W9.
   Xác nhận như vậy là đủ. **Nửa Postgres là W11** — dòng W11 ở §9 mang phép đo
   cho thấy hôm nay không có gì ghi xuống lấy một dòng log. *(Đã trả lời
   2026-09-27: W11 dựng nửa Postgres — `workflow.task_logs`, một thiết lập toàn
   cục `workflow.log_retention_days`, mặc định 30 ngày, `0` là giữ mãi, prune mỗi
   giờ. "Hôm nay không có gì ghi xuống" đúng tại `6749a37b` và được đính chính W11
   ở §9 cho nghỉ. Object storage vẫn là của W9.)* *(2026-10-04, W9a Task 14: vẫn mở. W9 đã được tách và W9a chỉ dựng internal storage cho file của execution; object storage cho **dòng log** chưa được dựng và không thuộc gói nào (spec W9a §14). Dòng log vẫn ở Postgres theo `workflow.log_retention_days`.)*
2. Giải tham chiếu lúc publish làm flow đã publish trở nên cứng: đổi tên một
   dataset sẽ không trỏ lại. Tin là đúng nhưng đây là đánh đổi thật.
3. `kestra-plugin-host` cần chính sách về jar plugin nào được phép và ai được
   thêm. Chạy mã JVM bên thứ ba nhân danh một workspace là quyết định quản trị,
   không chỉ là kỹ thuật. *(Đã trả lời cho container ngày 2026-09-28, bởi người
   dùng, trong W5a: một feature theo workspace `workflow_containers`, mặc định
   tắt, cộng một allowlist image toàn cục do platform admin đặt trong
   `workflow.plugin_configurations`, không có mục thì không image nào được
   phép; cả hai chặt hơn Kestra OSS, vốn không có cái nào. Runner `Process` bị
   từ chối. **Vẫn còn mở cho jar plugin**: host W5b được nạp jar nào, và ai
   được thêm, là việc của W5b.)* *(Đã trả lời cho jar plugin ngày 2026-09-29,
   bởi người dùng, trong W5b: jar được dựng sẵn vào một image từ
   `plugin-host/plugins.lock` trong repo, mỗi jar ghi bằng toạ độ Maven và kiểm
   sha256 lúc dựng image; thêm một jar là một PR có review, và không gì được
   thêm lúc chạy. Hôm nay có ba jar: `plugin-jdbc-postgres`,
   `plugin-jdbc-trino` và `plugin-jdbc-mysql` 2.0.0. Cái được chạy còn bị thu
   hẹp thêm bởi glob `allowed-types` của platform admin và bởi feature
   workspace `workflow_kestra_plugins`, mặc định tắt.)*
