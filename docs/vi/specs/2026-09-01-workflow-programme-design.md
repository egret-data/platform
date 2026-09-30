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
   không phải 19 method ngữ nghĩa của `Storage`.
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
| **W5** Kestra plugin host — **tách ngày 2026-09-28 (quyết định của người dùng)** thành **W5a** *Container chạy trên production*: bốn script task của Kestra (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) với `taskRunner:` (Docker, và Kubernetes dưới FQCN của Kestra EE), feature `workflow_containers` (mặc định tắt), allowlist image `plugin_configurations`, gia hạn lease, runner Kubernetes và namespace task trong chart — `docs/{en,vi}/specs/2026-09-28-workflow-w5a-containers-in-production-design.md`; và **W5b** *Kestra plugin host*, dựng trên W5a: image JVM, `ShimRunContext`, cầu `StorageInterface`, điều phối `kestra.<FQCN>`, giải jar plugin và allowlist jar plugin. Plugin dựa trên storage và secret chờ W9. *(W5b, 2026-09-30: dựng xong mà không có cầu `StorageInterface` — mọi lời gọi storage đều ném lỗi và task thất bại nêu tên W9 — và có `secret()`, thứ W5b mang lên sớm: plugin dựa trên secret nay đã chạy; plugin dựa trên storage vẫn chờ W9. Xem đính chính W5b bên dưới.)* | W4 (W5a cần thêm W10, W11) |
| **W6** Trigger & subflow | `subflow` + replay `CREATE_NEW_EXECUTION` (**13–15 mục baseline, chung một migration**); rồi mới cron, webhook, flow trigger và backfill — vốn chạm **0** mục baseline và có test riêng. Dòng này viết TRƯỚC khi corpus được đo: bốn trong năm chữ ấy đáng **KHÔNG** mục nào, và backfill **không có lấy một dòng khai báo sống trong 345 flow**. Xem W6 spec §1. **Kết quả, đã đo: nửa bank được đã bank 16 mục** — 12 `Subflow` + 4 replay, tức ước lượng 13–15 thiếu một ở cận trên — **và bốn chữ trigger bank 0, đúng như dự đoán.** Cơ chế của từng con 0 nằm ở `oracle-exec/README.md`, *The coverage statement* → mục W6 | W3b |
| **W7** Frontend | nhóm nav Workflows; editor YAML; topology chỉ-đọc; gantt và log của execution. **Bị chặn bởi W10**: gantt và topology cần một tiến trình đẩy execution đi tiếp cùng một bề mặt REST execution, và lúc viết dòng này chưa có cái nào. **Panel log cần W11** và nằm ngoài lát cắt đầu của W7 — nói ra chứ không để phát hiện muộn. *(2026-09-27: W11 đã ship API mà panel đọc, `GET …/executions/{id}/logs`; bản thân panel vẫn là việc của W7 và chưa dựng.)* | W1, W3b, W10 |
| **W8** Ngữ nghĩa nâng cao | `pause`/`resume` **cùng** `loopUntil` (MỘT trạng thái, không được tách), `allowFailure`, và hai mục `EXECUTION_ASSERTION` của `sla:` — **18 mục baseline, số chặt**, đã đo. **Concurrency limit chạm 0 và `labels` chạm 0**; `WorkingDirectory` chạm 1 chứ không phải 4 như khai báo gợi ý. Dòng `concurrency:` → `CANCELLED` trong bảng residual là **quy sai** (`CANCELLED` là 2 của `sla:` + 1 của `Exit`, chưa bao giờ của một limit) nên dòng ấy bị XOÁ, không phải hạ số. Dòng này viết TRƯỚC khi corpus được đo. Xem W8 spec §1 | W3b, W6 |
| **W9** Hạ tầng Kestra còn lại | KV store, namespace file, internal storage, output | W4 |
| **W10** Runtime & bề mặt execution | bộ ticker worker trong `egret-bin` — đóng/mở bằng config, `MissedTickBehavior::Skip`, biết huỷ, kèm **một phát biểu an toàn cho mỗi lần gọi** `tick`, `wake`, `due_retries` và `reap`; reaper trên interval riêng, nêu rõ `working_timeout_s`; một bề mặt REST execution (list, get kèm task run, start, resume, kill, replay) — phạm vi workspace theo cấu trúc, có feature gate, tài liệu utoipa, sinh lại client frontend bằng `npm run gen:api`; **một test end-to-end publish, start và quan sát một flow tới trạng thái kết thúc mà KHÔNG có harness nào đẩy tick**; và **chính đính chính của bảng này**, để **bốn** hạng mục W7 cần có chủ — hai cái là của chính W10, hai cái được gọi tên là W11 và W12. **Ratchet không phán xét được gói này theo cả hai chiều**: `oracle-exec/driver.rs:884` gọi thẳng `worker::tick` với registry của riêng nó, nên oracle vốn đã chạy phần thân của vòng và mù với phần nối dây của binary **do chính cấu tạo** — dự kiến **109 trước và 109 sau**. Xem W10 spec §1 | W3b, W4 |
| **W11** Log execution | `workflow.task_logs`, một `LogSink` có ghi xuống, cửa sổ retention (§11 câu hỏi mở 1) và một API đọc có phân trang. **Hoãn khỏi W10 kèm PHÉP ĐO chứ không phải bỏ quên:** không migration nào tạo bảng `task_logs` — schema `workflow` có đúng bốn bảng: `flows`, `flow_versions`, `executions`, `task_runs`; `LogSink` có **ba** implementation dưới `crates/egret-workflow/src/` — `TracingSink` (ghi ra `tracing`), một `Recorder` `#[cfg(test)]` (một `Vec` trong bộ nhớ) và `NameHintSink` (chú thích rồi chuyển tiếp) — và **không cái nào ghi xuống lấy một dòng**; còn handler container ghi `stdOutLineCount` và `stdErrLineCount`, tức **đếm dòng chứ không phải nội dung**. Chừng nào chưa có, **W7 ship mà không có panel log** | W10 |
| **W12** Vòng trigger | `workflow.triggers` — đúng những hàng cron/webhook claim được mà §6.1 đã thiết kế — vòng lặp claim và kích hoạt chúng, cùng **lập luận an toàn concurrency của riêng nó** chứ không mượn lại lập luận của claim ở worker. **Hoãn khỏi W10 kèm phép đo, y như W11:** **không migration nào tạo `workflow.triggers`** (control: `workflow.executions` xuất hiện trong **ba** file migration, nên phép tìm có chạm tới migration, và schema `workflow` có đúng bốn bảng); `trigger::tick` **không có caller nào ngoài `crates/egret-workflow/src/trigger/`**; `TriggerState::initialise` có **đúng một** caller và đó là helper `#[cfg(test)]`; và `crates/egret-bin/src/main.rs` chứa chuỗi `trigger` **không lần nào** (control: `egret_workflow` xuất hiện **5** lần trong file đó). **Nên tại thời điểm dòng này được viết, một trigger `cron:` không thể nổ trong production, và W10 đã không đổi điều đó**: W10 nối vòng *worker*, vốn có bảng để claim; vòng trigger lúc đó không có bảng nào. Chừng nào chưa có W12, một flow chạy bằng **start thủ công qua API hoặc bằng webhook**, cả hai đều hoạt động — nói ra chứ không để phát hiện muộn **Thiết kế ngày 2026-09-24** ở `docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md`, phạm vi thu hẹp cùng người dùng về **chỉ schedule trigger**, tick bị lỡ được bù theo mặc định `ALL` của Kestra. Dòng này từng ghi *"hàng cron/webhook"*: webhook được đẩy qua HTTP và không cần hàng, nên bảng chỉ chứa schedule.

**Đính chính, 2026-09-25 (W12 Task 6): đã dựng xong, và mọi phép đo ở trên được RÚT LẠI chứ không xoá**, vì đó là ghi chép lý do dòng này từng tồn tại. Migration 0156 tạo `workflow.triggers`; `scheduler::sync` (gọi từ ba writer của `FlowRepo` có thể đổi thứ sẽ nổ — `create`, `update`, `soft_delete`; `set_live` được miễn, vì nó không đổi `source` lẫn `deleted_at`, và `state` được đọc lúc nổ) giữ nó khớp với `flow.source`; `scheduler::fire_due` claim và kích hoạt trigger `Schedule` tới hạn, mỗi trigger một transaction dưới `FOR UPDATE OF t SKIP LOCKED`, execution được tạo trong cùng transaction đẩy lịch đi tiếp qua một savepoint, với `workflow_executions_one_per_schedule_tick` (migration 0156) làm lưới chặn cho một lần đọc cũ không thể vượt qua. Nó chạy trên ticker riêng trong `runtime::spawn`, sau cần gạt `EGRET_WORKFLOW__SCHEDULER_ENABLED` — đóng/mở bằng config, **mặc định `false`**, đúng như `worker_enabled` ở W10, nên một deployment chưa gõ cờ này hành xử y hệt trước W12. **Một trigger `cron:` giờ CÓ THỂ nổ trong production, một khi cờ đó được bật**; ba phép đo call-site ở trên (số caller của `trigger::tick`, số caller của `TriggerState::initialise`, chuỗi `trigger` trong `main.rs`) đều đã bị thay thế — `scheduler::sync` và `scheduler::fire_due` là caller mới, không phải test, của cả hai; và `runtime.rs` giờ đọc `trigger` một số khác không lần. Ratchet của oracle không thấy được gì trong số đó, vì lý do phần W12 trong tuyên bố độ phủ của `oracle-exec/README.md` đã ghi: **109 trước, 109 sau, và 109 nếu xoá `fire_due`.** | W6, W10 |
| **W13** Flow trigger | nhánh không cửa sổ của `trigger/flow.rs` nối vào transaction settle của worker — **theo sự kiện, nên không cần bảng claim** — và sau đó nhánh cửa sổ `dependsOn`, mà 20 trên 21 khai báo trong corpus đi theo. **Được gọi tên ra khỏi phạm vi W12 ngày 2026-09-24** để có chủ thay vì vắng mặt. Endpoint backfill, `lateMaximumDelay`, `stopAfter` và việc đưa `trigger.*` vào render context được liệt kê ở §7 của spec W12 và vẫn **không có chủ** **Thiết kế ngày 2026-09-25** ở `docs/{en,vi}/specs/2026-09-25-workflow-w13-flow-triggers-design.md`, làm CẢ HAI nhánh theo quyết định của người dùng, giao thành **W13a** (chỉ mục listener do `scheduler::sync` giữ, outbox ghi trong `settle`, processor chạy trên ticker scheduler, nhánh không cửa sổ) và **W13b** (model `window`, port `TimeWindow`, `flow_trigger_windows`, `dependsOn`). Bất đồng bộ, không lồng trong `settle`: lồng bị chặn ở `MAX_SPAWN_CHAIN = 8` vì stack của worker, trong khi Kestra cho độ sâu 100. | W12 |

**Đính chính, 2026-09-25 (W13a Task 5): W13a đã dựng xong; W13b vẫn còn treo.** Migration 0157 thêm `workflow.flow_listeners` (giữ khớp bởi `scheduler::sync`, gọi từ đúng ba writer của `FlowRepo` mà dòng W12 đã nêu) và `workflow.flow_trigger_events`, outbox mà `settle` ghi vào — một `INSERT` có điều kiện, câu lệnh DUY NHẤT W13a thêm vào hot path đó (spec §4.3). `flow_trigger::process_events` rút cạn nó trên ticker scheduler (spec §4.4), gọi `trigger::flow::evaluate` lần đầu tiên trong production: nó render `when` và `inputs` trên context của chính nguồn, bắn trong một savepoint riêng cho từng listener để một listener hỏng không làm mất sự kiện đối với các listener anh em, retry một lỗi tạm thời tới `MAX_ATTEMPTS` rồi bỏ cuộc có tiếng past nó, và — đo trên artifact đã ghim, không suy đoán — tạo execution **born-FAILED** khi `inputs` không render được, đúng như Kestra, chứ không từ chối. Hai cầu nối mà mã trước-W13a để trống đều đã được xây: một sự kiện outbox từ nguồn PAUSED mang một `models::State::Paused` thật vào `evaluate` (hook của `settle` không gate theo tính kết thúc, vì `states` mặc định của Flow trigger có `PAUSED`), và `trigger_execution_id` của một execution vừa bắn cho phép bước đi nhân quả sẵn có của `worker::render::Surface::load` (đã sửa ở W6 Task 8) suy ra `labels.system.correlationId` là id gốc của chuỗi thay vì id của chính đứa con — đóng lại lỗ hổng mà module doc của `trigger::flow` từng mô tả là "thuộc về wiring seam". Ratchet của oracle không thấy được gì trong số này, cùng lý do cấu trúc W10 và W12 đã nêu: `oracle-exec/driver.rs` không bao giờ gọi `flow_trigger::process_events`, nên là **109 mismatching, 109 baselined, 0 broken trên 294 golden entry** trước task này, tại commit này, và cả khi `process_events` bị cho return sớm vô điều kiện — đo trên nhánh này, cả ba, không mang từ nơi khác sang. **W13b — nhánh `dependsOn`/window, 20 trên 21 khai báo của corpus — CHƯA bắt đầu**: chưa có model `window`, chưa có port `TimeWindow`, chưa có migration 0158, và bảng §7.2 của spec W13 vẫn là kế hoạch cho nó, không phải ghi chép đã làm. Xem phần W13a của `oracle-exec/README.md` để có sweep và suy dẫn đầy đủ. | W6, W10, W12 |

**Đính chính, 2026-09-26 (W13b Task 5): W13b đã dựng xong, và câu "CHƯA bắt đầu" trong đính chính W13a được cho nghỉ chứ không bị xoá.** Migration 0158 thêm `workflow.flow_trigger_windows`, mỗi listener `dependsOn` một hàng, bị xoá theo cascade cùng listener của nó (một khác biệt có chủ ý: Kestra giữ lại). `models::Window` model bảy trường mà artifact đã ghim chấp nhận, có cả `timezone`, và từ chối `fireOnce` ngay lúc parse như artifact. Module thuần `trigger::window` port biên của bốn loại `TimeWindow` từ số đo của W13b Task 1, có cả DST và nửa đêm. Module thuần `trigger::depends_on` port khoá điều kiện (murmur3, tái tạo đúng các khoá artifact đã lưu), `merge` và `satisfied` theo ALL / ANY / AT_LEAST, và một danh sách từ chối dùng chung cho lúc lưu và lúc chạy. `flow_trigger::process` chuyển listener `dependsOn` sang nhánh riêng: nhánh này lấy hàng cửa sổ `FOR UPDATE` ở REPEATABLE READ, thay cửa sổ đã hết hạn khi có sự kiện khớp kế tiếp (quyết định D1 của người dùng, khác với việc Kestra bỏ qua), bắn khi mode được thoả và xoá cửa sổ dưới mọi mode; một lần thua race với `40001` được thử lại mà không tốn lượt. Ticker scheduler quét các cửa sổ hết hạn mỗi lượt một lần. Trong 20 khai báo `dependsOn` của corpus, **18 resolve được**, 1 bị từ chối lúc lưu (over-refusal chỉ-có-namespace của W1, đã nằm trong `executor::PUBLISHED_BUT_REFUSED`) và 1 không parse được (`fireOnce`); không khai báo nào có `window:`. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W13b, tại đỉnh nhánh, và cả khi nhánh `dependsOn` bị xoá, cả ba đo trên nhánh này. Mọi dòng của bảng §7.2 trong spec W13 đều đã chạy mutation của nó và mutation đều bị giết. Xem phần W13b của `oracle-exec/README.md`.

**Đính chính, 2026-09-27 (W11 Task 6): W11 đã dựng xong, và phép đo trong dòng của nó được cho nghỉ chứ không bị xoá.** Dòng ở trên ghi lại vì sao log rời khỏi W10: không có bảng `task_logs`, ba implementation của `LogSink` mà *"không cái nào ghi xuống lấy một dòng"*, handler container chỉ lưu *"đếm dòng chứ không phải nội dung"*, và *"W7 ship mà không có panel log"*. Cả bốn đều đúng tại `6749a37b` và là hồ sơ của quyết định hoãn; không câu nào còn đúng hôm nay. Dòng `task_logs` ở §6.1 ghi *"CHƯA DỰNG: W11"* cho tới đính chính này. Migration 0159 thêm `workflow.task_logs`. `LogSink` thành async và `logs::PgLogSink` ghi xuống: một channel có giới hạn 10.000 dòng, đầy thì handler đang ghi phải **chờ** chứ không bỏ dòng; một writer chèn theo lô 500 dòng hoặc mỗi 1 s, thử lại một lô lỗi trong 30 s, và nếu vẫn lỗi thì nêu số dòng mất trong `tracing::error!`; khi tắt, writer xả hết channel trong vòng 10 s. Mọi handler ghi qua một `RunLog` cho từng attempt: tách dòng dài hơn 15.360 ký tự như Kestra, chặn một attempt ở 10.000 dòng hoặc 10 MiB, và ghi một dòng WARN tổng kết nằm ngoài giới hạn. stdout/stderr của container và python được lưu **từng dòng, đủ nội dung** (stdout INFO, stderr ERROR, đo trên artifact đã ghim). `RunLog::finish` là một flush barrier giới hạn 30 s, nên các dòng của một run được chèn trước khi outcome của nó commit. Một pruner xoá các dòng cũ hơn `workflow.log_retention_days` (mặc định 30, `0` là giữ mãi) mỗi giờ, theo lô 10.000. `GET /api/ws/{workspace}/workflows/executions/{id}/logs` đọc chúng theo cursor, trễ 2 s sau writer, và báo `complete`. **Những gì vẫn CHƯA đúng:** registry production không có handler container hay python, nên hôm nay không run production nào lưu stdout hay stderr; không có SSE; và panel log là việc của W7. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W11 và tại tip của nó, cả hai đều đo: oracle so trace chứ không so dòng log. Xem phần W11 trong `oracle-exec/README.md` và §11 của spec thiết kế W11 về những gì `complete` bảo đảm và không bảo đảm.

**Đính chính, 2026-09-28 (W5a Task 6): W5 đã được tách, W5a đã dựng xong, và câu *"registry production không có handler container hay python"* trong đính chính W11 được cho nghỉ đối với container chứ không bị xoá.** W5a theo hình dạng của Kestra thay vì nới rộng `egret.container`: bốn script task dưới FQCN của Kestra, `egret.container` và `egret.script.python` giữ lại làm alias, và `taskRunner:` chọn Docker (`io.kestra.plugin.scripts.runner.docker.Docker`) hoặc một pod Kubernetes cho mỗi attempt (`io.kestra.plugin.ee.kubernetes.runner.Kubernetes`, chỉ có ở bản Enterprise của Kestra, được hiện thực ở đây dưới cùng FQCN); `Process` bị từ chối lúc lưu. `workflow.container_runner` (`none` | `docker` | `kubernetes`, mặc định **`none`**) quyết định production có đăng ký chúng hay không: với `none` registry y hệt của W11, nên deployment không đặt gì vẫn không lưu output script nào. Chạy một script còn cần feature workspace `workflow_containers`, feature đầu tiên **mặc định tắt**, và một image được `plugin_configurations[type].values.allowed-images` cho phép (không có mục hoặc danh sách rỗng thì **không** image nào được phép); một hàm gate duy nhất kiểm cả hai lúc publish và lần nữa lúc chạy, sau khi render, và từ chối lúc chạy thì không tạo pod hay container nào. Worker gia hạn lease mỗi `working_timeout / 3` dưới một fence, đóng nguyên nhân thứ hai của mục 19 trong `oracle-exec/README.md` đối với worker còn sống, và script task không có `timeout:` chạy dưới `container_default_timeout_s` (3600). Bảy trên mười lăm kiểu publish được vẫn chưa có handler trên production. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W5a và tại tip của nó, cả hai đều đo: corpus không khai báo script task nào. Xem phần W5a trong `oracle-exec/README.md`. **W5b chưa bắt đầu.**

**Đính chính, 2026-09-30 (W5b Task 7): W5b đã dựng xong, và câu *"W5b chưa bắt đầu"* trong đính chính W5a được cho nghỉ chứ không bị xoá.** Một task có `type:` là `kestra.<FQCN>` chạy một plugin Kestra thật, không sửa đổi: mỗi task-run attempt một JVM, trong image `egret-plugin-host` (Java 25, `io.kestra:core:2.0.0-rc13`, một `ShimRunContext` trong chính package của Kestra), trên runner đã cấu hình của W5a — mỗi attempt một pod trên Kubernetes. Jar đúng ba cái, `io.kestra.plugin:plugin-jdbc-postgres`, `plugin-jdbc-trino` và `plugin-jdbc-mysql` bản 2.0.0, ghi kèm sha256 trong `plugin-host/plugins.lock` và được kiểm lúc dựng image; `catalog.json` sinh từ chúng (năm lớp task; các kiểu đã port native và bốn lớp cần internal storage bị loại) và publish kiểm kiểu và thuộc tính theo nó mà không cần JVM. FQCN plugin viết trần bị từ chối kèm gợi ý cách viết có tiền tố. Feature workspace `workflow_kestra_plugins` mặc định tắt và cần `workflow_containers`; mục `kestra` của `plugin_configurations` thu hẹp catalog bằng glob `allowed-types` (không có mục thì cho phép cả catalog, danh sách rỗng thì không cho gì); một gate chạy lúc publish và lần nữa lúc chạy sau khi render, và từ chối lúc chạy thì không tạo pod nào. `secret('NAME')` và `secret('NAME', subkey=…)` giải lúc chạy, trong workspace của execution, chỉ trên task `kestra.*` (ghi chú ở §6.2); thuộc tính của task plugin render một lần, như mặc định của Kestra; mọi giá trị đã giải được thay bằng `******` trong các dòng log của run, còn output không bị che, như Kestra. `pluginDefaults:` bị từ chối lúc publish trên mọi flow, như Kestra 2.0 từ chối nó. Timeout dừng host bằng SIGTERM kèm grace `workflow.plugin_host.kill_grace_s` (mặc định 15 s, tối thiểu 3), trong đó host gọi `kill()` của plugin: query Postgres bị huỷ trên server (đo sống trên Docker và trên một cụm kind), query StarRocks thì không (spec W5b §12). **Cái vẫn chưa đúng:** plugin cần internal storage, KV hay namespace files thất bại có tên và chờ W9; trigger của plugin và jar ngoài ba cái nằm ngoài phạm vi. Ratchet là **109 mismatching, 109 baselined, 0 broken trên 294** trước W5b và tại tip của nó, cả hai đều đo: corpus không khai báo task plugin nào. Xem phần W5b trong `oracle-exec/README.md`.

**W10, W11 và W12 được thêm ngày 2026-09-14, sau khi W7 hoá ra bị chặn, và lý do đáng giữ lại.** **Bốn** thứ dòng W7 cần — một tiến trình đẩy execution đi tiếp, một bề mặt REST execution, `workflow.task_logs`, và một vòng kích hoạt trigger `cron:` — đều được thiết kế ở §6.1 và §6.3 của chính tài liệu này mà **không dòng nào của bảng sở hữu**. Thứ tư tìm ra trong lúc đo schema cho dòng W11 chứ không phải bằng cách đọc bảng, nên nó được nêu riêng ở dưới. Kiểm bằng máy chứ không đọc bằng mắt: grep bảng này **ở trạng thái TRƯỚC khi có ba dòng W10, W11 và W12 bên trên** với `task_logs|execution api|ExecutionController|wire|binary|daemon` trả về **0**, trong khi cùng lượt đó `reaper` trả về 1 và `W3b` trả về 6; còn `ExecutionController` xuất hiện **đúng một lần** trong spec này — ở §8, *"Không port, nhưng bắt buộc đọc trước W7"*, nơi nó được nêu như một **hình dạng để nghiên cứu**, chưa bao giờ như một hạng mục phải giao. Một dòng gọi tên một thứ trong kiến trúc mà không bao giờ gọi tên nó trong kế hoạch, với người đi soát coverage, đọc y hệt một dòng đã xong. Suy dẫn đầy đủ nằm ở `docs/{en,vi}/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md` §1.

**Vòng trigger — thứ tư — chính là W12 ở trên.** Còn lại **hai cái tên không dòng nào trong bảng này sở hữu**, ghi ở đây thay vì để người sau phát hiện lại: hai function biểu thức `secret` và `http`, mà bảng `REFUSED` trong `crates/egret-workflow/src/runners/functions.rs` gán cho W9 **chỉ để thông điệp từ chối có một cái tên chủ sở hữu**, theo đúng lời chính file ấy — *"`secret` và `http` không nằm trong nội dung của gói nào cả"*. **Ở đây cố ý KHÔNG gán chủ cho chúng**: gán tên một lần nữa chỉ làm thông điệp thay đổi mà không làm nó đúng hơn. Chúng được đánh dấu cho ai viết bản review cấp chương trình tiếp theo, và chúng cùng một hình dạng với bốn thứ mà §1 của W10 ghi lại. *(Đính chính 2026-09-30, W5b Task 7: `secret` nay đã có chủ. W5b giải `secret()` trên task plugin `kestra.*`, từ secret của workspace chứa execution (ghi chú ở §6.2), và nó vẫn bị từ chối ở mọi chỗ khác — task script, input, `runIf`, `Log`, trigger — kèm lý do nêu chỗ nó được giải. `http` vẫn không dòng nào sở hữu.)*

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
   ở §9 cho nghỉ. Object storage vẫn là của W9.)*
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
