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
workflow.executions     một lần chạy, ghim flow_version_id; không bao giờ đọc draft
workflow.task_runs      một lần thử của một node: attempt, state, not_before, deadline_at, output
workflow.triggers       cron/webhook, hàng claim được
workflow.task_logs      dòng log, có retention_days như automation.schedules
```

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

- **vòng trigger** — claim trigger tới hạn, tạo execution và task_run gốc;
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
| `egret.container` | cùng launcher; image + command + file in/out — hợp đồng `TaskCommands` |
| `kestra.<FQCN>` | `egret.container` chạy image `kestra-plugin-host` |
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
| **W3** Executor lõi | `advance()`; vòng claim; `sequential/parallel/if/switch/dag/loop/loopUntil`; retry/timeout/`errors:`; task `core.log`, `egret.sql` | W1, W2 |
| **W4** Runner & container | hợp đồng `TaskCommands`; `egret.container`, `egret.python`; stream log; file output; cancel/kill | W3 |
| **W5** Kestra plugin host | image JVM, `ShimRunContext`, cầu `StorageInterface`, điều phối `kestra.<FQCN>`, giải jar plugin | W4 |
| **W6** Trigger & subflow | cron, webhook, flow trigger, `subflow`, backfill | W3 |
| **W7** Frontend | nhóm nav Workflows; editor YAML; topology chỉ-đọc; gantt và log của execution | W1, W3 |
| **W8** Ngữ nghĩa nâng cao | `pause`/`resume`, `allowFailure`, `loopUntil`, concurrency limit, `WorkingDirectory`, label | W3 |
| **W9** Hạ tầng Kestra còn lại | KV store, namespace file, internal storage, output | W4 |

Mỗi gói có spec và plan riêng, như chương trình DBAPI-parity đã làm.

## 10. Cố ý để ngoài v1

`pause`/`resume`, backfill, `allowFailure`, `loopUntil`, concurrency limit và
dynamic task thuộc W8, không thuộc W3. Đó là nơi khối test của Kestra nằm, và mỗi
thứ đều là phần **cộng thêm** trong YAML, nên hoãn chúng không bỏ rơi flow nào đã
được viết.

## 11. Câu hỏi mở

1. Dòng log nằm trong Postgres kèm cửa sổ retention ở v1; object storage là W9.
   Xác nhận như vậy là đủ.
2. Giải tham chiếu lúc publish làm flow đã publish trở nên cứng: đổi tên một
   dataset sẽ không trỏ lại. Tin là đúng nhưng đây là đánh đổi thật.
3. `kestra-plugin-host` cần chính sách về jar plugin nào được phép và ai được
   thêm. Chạy mã JVM bên thứ ba nhân danh một workspace là quyết định quản trị,
   không chỉ là kỹ thuật.
