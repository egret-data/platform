# Follow-up workflow, nhóm C: các task lõi Kestra còn lại

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-18-workflow-core-tasks-design.md` là bản chuẩn; hai bản lệch nhau thì theo bản tiếng Anh.

**Trạng thái:** thiết kế ngày 2026-10-18. **C1 đã xây và kiểm chứng ngày 2026-10-18** trên `feat/workflow-kestra-core-tasks`; **C2 đã xây và kiểm chứng ngày 2026-10-18** trên `feat/workflow-kestra-executor-tasks`. Ratchet của oracle (`cargo test -p egret-workflow --test executor`, bản in của `executor_matches_golden`) đi từ 100 mismatching / 100 baselined xuống **84 / 84**, 0 broken: 16 entry được bank (`OutputValues` 11, `Assert` 1, `PurgeLogs` 4), các entry còn nằm trong baseline được nêu theo nguyên nhân trong `oracle-exec/README.md`. Số LIVE của census đi từ 254 lên **281**, trần handler-surface (`python3 handler_surface.py`) từ 232 lên **250** (quần thể 228). Theo từng task: 100 xuống 89, 89 xuống 88, 88 xuống 84.

**Kết quả C2** (báo cáo Task 5, bản in của `executor_matches_golden` và `python3 handler_surface.py`, trên corpus 351 flow): ratchet đi từ 84 mismatching / 84 baselined xuống **73 / 73**, 0 broken. 11 trong 12 entry C2 đã được bank; `valids/task-runif-taskswithstate` nằm trong baseline vì `finally:` cấp flow không chạy (egret không có `write_execution_context` và không có run `skipped_when_failures_exist`; `finally-flow*` nằm trong baseline vì cùng lý do). Số LIVE của census đi từ 281 lên **293**, `UNKNOWN_TYPE_REFUSED` từ 33 xuống 21, `VALIDATE_REFUSED` giữ 27, bảng `PUBLISHED_BUT_REFUSED` từ 18 dòng xuống 6, trần handler-surface từ 250 lên **261** (quần thể 228).

**Nguồn của các khẳng định:** mọi khẳng định về code egret đều đã kiểm tra ở worktree `egret-coretasks`, commit `15406ad9` (main sau #689). Đường dẫn code tính từ `crates/egret-workflow/src/`, trừ khi ghi gốc khác. Kestra là bản checkout tại `data-portal/kestra`.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9. Từ no-silent-stall Task 1, sáu type này là lỗi `unknown_task_type`. Vì chúng mà oracle từ chối 40 entry đã publish trong corpus (danh sách `PublishedButRefused` trong `executor/mod.rs`).

**Đã chốt với user (2026-10-18):**
- **`PurgeLogs` chỉ với tới namespace của flow và các namespace con.** Đây là quy tắc ghi đã chốt ở W9b cho `PurgeFiles` và KV (`storage/nsfiles/access.rs:201`, `Access::Purge`). Kestra OSS không kiểm tra gì ở đây.
- **Chia hai PR: C1 cho các task chạy trên worker, C2 cho các task chạy trong executor.** C1 làm trước, vì mọi entry của C2 đều đọc lại kết quả qua một `OutputValues`.
- **Thiết kế §2-§4 được duyệt như đã trình bày.**

**Quyết định mặc định (2026-10-18).** Nguyên tắc: theo Kestra, trừ khi egret có lý do làm khác; giữ tính đúng chặt chẽ.
- **D1.** `Assert` không ghi metric `success`/`failed`, vì egret chưa có kho metric. Số đếm được ghi vào log và vào message lỗi.
- **D2.** `PurgeLogs.purgeNonExecutionLogs` không xoá gì và đếm 0. Mọi dòng trong `workflow.task_logs` đều thuộc về một execution (migration 0159), nên egret không có log ngoài execution.
- **D3.** Replay chép `variables` của execution, giống cách đã chép `labels` (`worker/replay.rs:275`). `childExecution` của Kestra chép toàn bộ execution.
- **D4.** `SetVariables`, `UnsetVariables` và `Labels` bị từ chối ngay ở bước validate khi nằm trong `WorkingDirectory`, giống `Exit` (`spec/validate.rs:301`). Kestra chỉ nhận con là `RunnableTask` ở đó, còn ba task này là `ExecutionUpdatableTask`.
- **D5.** `SetVariables` merge đúng như `deepMerge` của Kestra (`MapUtils.java:62-133`), gồm cả việc nối list không trùng và "giá trị mới là null thì giữ giá trị cũ". §3.2 trước đây ghi "giá trị khác thì thay", không phải quy tắc của Kestra cho list và null; §3.2 đã được sửa.
- **D6.** Update lỗi ghi đúng một attempt `[FAILED]`, như golden, qua `Command::FailUnstarted` mới. `Loop` fail ở `CREATED` thì không giữ attempt nào.
- **D7.** Map của `Labels` có hai key render ra cùng một chuỗi thì run fail (`labels: two keys render to "<k>"`). Kestra ném `IllegalStateException` ở đó; run fail kèm message là cùng kết quả, được nói rõ.
- **D8.** `variables:` là một biểu thức nguyên khối render ra `null` thì run fail (lỗi `render_map` sẵn có). `asMap` của Kestra trả null rồi `SetVariables` ném `NullPointerException` ở nơi gọi `deepMerge`; một lỗi có tên là cùng kết quả mà không crash.
- **D9 (2026-10-18, quyết định mặc định của controller trong Task 4).** Update được tính bất cứ khi nào `advance()` còn điều khiển execution, tức `executor::drives_forward`: chưa terminal và không phải `KILLING`. Vì vậy execution `PAUSED` và `RETRYING` vẫn tính update, và một `SetVariables` cạnh một `Pause` đang chờ chạy trước khi resume. Điều này thay cho câu "chỉ khi RUNNING" trong plan.
- **D10 (2026-10-18, quyết định mặc định của controller sau final review).** Một lần lặp của `Loop` kế thừa `variables` của execution cha, giống như đã kế thừa `inputs` và `labels`: câu INSERT lần lặp (`worker/spawn.rs`) chép cột này từ dòng cha. `Execution.loopExecution` của Kestra dựng lần lặp với `this.variables` (`kestra/core/src/main/java/io/kestra/core/models/executions/Execution.java:438-460`). Đây là bản chép tại thời điểm spawn, như của Kestra: một `SetVariables` bên trong lần lặp chỉ đổi lần lặp đó, không có gì chảy ngược về cha. Execution con của `Subflow` vẫn không kế thừa gì (§3.2).

## 1. Mỗi task cần cơ chế nào (đã đo)

Kestra chạy `RunnableTask` trên worker, còn `ExecutionUpdatableTask` thì chạy trong executor (`executor/…/ExecutorService.java:1371`). Golden cho thấy sự khác biệt này ở mọi entry:

| Task | Interface trong Kestra | Trạng thái run trong golden |
|---|---|---|
| `io.kestra.plugin.core.output.OutputValues` | `RunnableTask` | `CREATED SUBMITTED RUNNING SUCCESS` (cả 28 entry) |
| `io.kestra.plugin.core.execution.Assert` | `RunnableTask` | `… SUBMITTED RUNNING SUCCESS` / `… FAILED` |
| `io.kestra.plugin.core.log.PurgeLogs` | `RunnableTask` | `CREATED SUBMITTED RUNNING SUCCESS` (cả 5) |
| `io.kestra.plugin.core.execution.SetVariables` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS`; lỗi thì `CREATED FAILED` (`set-variables-duplicate`) |
| `io.kestra.plugin.core.execution.UnsetVariables` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS` |
| `io.kestra.plugin.core.execution.Labels` | `ExecutionUpdatableTask` | `CREATED RUNNING SUCCESS`; lỗi thì `CREATED FAILED` (`labels-update-task-empty`) |

Task chạy trong executor không bao giờ qua `SUBMITTED`. Khi lỗi, chúng đi thẳng từ `CREATED` sang FAILED, vì Kestra chạy `update()` trước khi ghi attempt `RUNNING` (`ExecutorService.java:1389-1408`). `runIf` falsy cho ra `CREATED SKIPPED` (`task-runif-executionupdating`); đây là đường xử lý `runIf` sẵn có.

Nhiều entry trong 40 entry còn bị chặn bởi những tính năng ngoài nhóm này: `WaitFor`, `afterExecution`, webhook, flow concurrency, `Pause`. Plan sẽ đo xem entry nào khớp được; spec này không hứa entry nào.

## 2. C1: ba handler trên worker

Mỗi task là một `TaskHandler` trong `worker/handlers/` cộng một dòng `spec::registry`, theo mẫu của `Return` (`worker/handlers/ret.rs`). C1 không cần migration.

### 2.1 `OutputValues`

- `values: Property<Map<String,Object>>` được render đệ quy, tức là cả giá trị trong map và list lồng nhau cũng được render, như `render(values).asMap` của Kestra.
- Thành công thì output là `{"values": <map đã render>}`. Không có `values` thì output là `{"values": {}}`.
- Render lỗi thì run FAILED, message chính là lỗi render.

### 2.2 `Assert`

- `conditions: List<String>` (bắt buộc) và `errorMessage: Property<String>`.
- Từng điều kiện được render riêng. Điều kiện hỏng khi kết quả render là falsy theo `executor::flowable::branch::is_truthy`, cùng cách xét truthy mà task `If` dùng (`executor/flowable/branch.rs:93`). Điều kiện render lỗi cũng tính là hỏng. Mỗi điều kiện hỏng được ghi một dòng log `ERROR`, dùng đúng message của Kestra: ``Assertion `<c>` failed!`` và ``Assertion `<c>` failed, failed to render `<err>` ``.
- Có điều kiện hỏng thì run `FAILED` với message `"<n> assertion failed!"` hoặc `"<n> assertions failed!"`. Nếu có `errorMessage` thì thêm một dòng mới và `errorMessage` đã render. Không có điều kiện nào hỏng thì run `SUCCESS`, không có output.
- Không ghi metric (D1). Thay vào đó, một dòng log INFO `"<s> succeeded, <f> failed"` ghi số đếm.

### 2.3 `PurgeLogs`

- **Thuộc tính:**
  - `namespace`, `flowId`, `executionId` (đều là `Property<String>`);
  - `logLevels: Property<List<Level>>`;
  - `startDate` và `endDate`, đều là `Property<String>` đọc theo dạng ngày-giờ ISO-8601; `endDate` bắt buộc;
  - `purgeExecutionLogs` (mặc định `true`) và `purgeNonExecutionLogs` (mặc định `true`);
  - `batchSize: Property<Integer>`.
- **Phạm vi (đã chốt):**
  - Mọi lệnh xoá đều giới hạn trong workspace của execution đang chạy.
  - Không ghi `namespace` thì phạm vi là namespace của flow và các namespace con.
  - Có ghi `namespace` thì namespace đó phải qua được `storage::nsfiles::access::authorize` với `Access::Purge`, qua một resource mang tên log, để lời từ chối ghi "may not purge logs of namespace …". Không qua thì run `FAILED` với message đó và không xoá gì.
  - Namespace con nghĩa là khớp theo ranh giới dấu chấm: `a.b` với tới `a.b.c` nhưng không với tới `a.bc`.
- **Lệnh xoá:**
  - Xoá các dòng trong `workflow.task_logs` join với `workflow.executions` theo `execution_id`, lọc theo workspace và phạm vi namespace, cộng thêm `flow_id`, `execution_id`, `level IN (…)` và `ts` trong `[startDate, endDate]`. Mỗi bộ lọc chỉ áp khi thuộc tính tương ứng được ghi.
  - **(2026-10-18, C1 Task 4 review)** Ba chỗ sửa, chỗ nào cũng theo quy tắc của Kestra (`AbstractJdbcLogDataStore.buildDeleteCondition`):
    - Khoảng thời gian là `[startDate, endDate]`: cả hai đầu đều tính (`ts <= endDate`, tức `lessOrEqual` của Kestra). Trước đây ghi là `[startDate, endDate)`.
    - Có `flowId` thì namespace phải khớp đúng. Chỉ khi không có `flowId` mới với tới các namespace con.
    - Namespace của flow được đọc từ dòng execution (`workflow.executions.namespace`, theo execution id và workspace), không đọc từ render scope, nên task chạy được cả trên deployment không có `[storage]`.
  - Có `batchSize` thì xoá theo từng lô tối đa bấy nhiêu dòng, mỗi lô một câu lệnh, cho tới khi một lô xoá ít hơn. Không có `batchSize` thì xoá bằng một câu lệnh. Một `endDate` ở tương lai trên một namespace đang bận có thể giữ vòng lặp chạy tiếp trong lúc dòng mới vẫn tới; vòng lặp của Kestra cũng như vậy.
  - **(2026-10-18, review cuối)** Một `endDate` ở tương lai như thường dùng còn xoá cả các dòng trước đó của chính execution đang purge, và các dòng của những execution khác trong phạm vi còn đang chạy. Dòng log được lưu bất đồng bộ (log writer gom theo lô), nên `count` phụ thuộc thời điểm. Kestra cũng như vậy.
  - **(2026-10-18, review cuối) Một khác biệt có chủ ý:** `executionId` render ra không phải UUID thì run FAILED, nêu tên `executionId`. Kestra đem nó so với không dòng nào và SUCCESS với count 0. Execution id của egret là UUID, nên một giá trị không phải UUID chỉ có thể là lỗi của người viết flow.
  - Không ghi audit record riêng; bản thân run và các dòng log của nó đã là bản ghi.
- **Log ngoài execution:** `purgeNonExecutionLogs` không xoá gì (D2). Với `purgeExecutionLogs: false` thì không xoá gì cả.
- **Output:** `{count, executionLogsCount, nonExecutionLogsCount}`, với `nonExecutionLogsCount = 0`.
- **Handler cần pool**, nên được đăng ký giống `Delete` (`handlers::storage::register`, `worker/handlers/storage.rs:336`), không nằm trong `with_core_tasks`.
- **Một divergence đã biết:** `valids/purge_logs_full_arguments` chạy trong `io.kestra.tests` với `namespace: purge.namespace`. Kestra trả SUCCESS; egret trả FAILED theo quy tắc đã chốt. Entry này chuyển từ danh sách bị từ chối sang `baseline.txt`, kèm comment ghi lý do đã chốt. Đó là dòng vĩnh viễn, không phải chỗ trống chờ làm.

**(2026-10-18, C1 đã xây)** Những chỗ bản xây khác §2.1-§2.3:
- **Đăng ký.** Cả ba còn có một key trong `runtime::served_for`, bản sao thứ hai của các key `with_core_tasks` mà kiểm tra "deployment này không có handler" của publish đọc. Các mục trên không nêu nó. `PurgeLogs` được đăng ký bằng `worker::handlers::purge_logs::register(&mut handlers, pool)` riêng trong `runtime::handlers_for`, cạnh `kv::register`, chứ không qua `handlers::storage::register` như §2.3 ghi.
- **`Assert`.** Thiếu `conditions` thì run FAILED với `conditions: must not be null` (Kestra khai báo `@NotNull`). **(review cuối)** Phần tử boolean hay số được đọc thành chuỗi của nó (`true`, `1`, `1.5`), như Jackson ép một scalar sang `String`; phần tử null, list hay map thì run FAILED, nêu tên `conditions[i]`. `is_truthy` được dùng qua `crate::executor::is_truthy`, vì `executor::flowable` là private với `worker`.
- **`OutputValues`.** `models::property::render_map` là hàm render map đệ quy dùng chung. Hai key render ra cùng một chuỗi thì key đầu thắng; mảng, số, boolean hay null ở cấp cao nhất là lỗi. Va chạm key thay giá trị đầu nếu nó là null, như `putIfAbsent` của Java (đã sửa ở review cuối). Còn một mép đã biết và hoãn: `values` là một biểu thức nguyên khối render ra `null` thì lỗi, trong khi `asMap` của Kestra trả null.
  **(2026-10-19, gói nợ)** Quy tắc cho `values` là chuỗi, được nêu rõ: một chuỗi mà sau khi trim bắt đầu bằng `{{` hoặc `{%` và kết thúc bằng `}}` hoặc `%}` thì được render nguyên khối và phần text đọc như một object JSON (`models/property.rs`; kiểm tra dấu phân cách `PebbleUtil` của Kestra). Nên `"{% if … %}{…}{% endif %}"` được đọc là object JSON sau khi render. Mọi chuỗi khác được parse JSON trước, rồi mới đi qua cấu trúc.
- **Một entry oracle bị giữ bởi construct chưa port, không phải bởi C1.** `valids/if-in-parallel` chạy và `assert` của nó FAILED trong khi Kestra SUCCEEDED, vì điều kiện đọc `outputs:` của chính `Loop`, thứ egret chưa sinh ra. Cùng nguyên nhân giữ `loop-outputs*`.

## 3. C2: ba task chạy trong executor

### 3.1 Plan và trạng thái

- **Plan:** thêm `TaskKind::ExecutionUpdate { op, spec }` trong `executor/plan.rs`, với `op` là một trong `Set`, `Unset`, `Labels`. Mỗi type là một dòng `spec::registry` không có handler và được liệt kê trong `EXECUTOR_LEVEL` (`spec/registry.rs:333`), nên mọi deployment đều phục vụ được, giống `Exit`.
- **Phần cập nhật được tính khi run còn ở `CREATED`**, trong một pass render riêng (`worker/render.rs::render_updates`, sau `render_expressions`), giống `values:` của `Loop` (`renders_at`, `worker/render.rs:1345`). Pass này render các thuộc tính của task dựa trên variables và labels hiện tại của execution, rồi tính giá trị mới.
  - **Lỗi:** run fail kèm message: `Command::FailUnstarted`, `CREATED FAILED` với một attempt `[FAILED]` (D6).
  - **Thành công:** giá trị mới được ghi vào `RenderSide` và run được hoàn tất ngay trong pass đó: `CREATED RUNNING SUCCESS`, với một attempt `CREATED RUNNING SUCCESS`. Bước settle ghi `variables` hoặc `labels` mới vào dòng execution, trong cùng transaction đang giữ khoá execution.
- **Nhiều run cập nhật trong cùng một round** (hai nhánh của một `Parallel`) được áp dụng lần lượt theo thứ tự run. Chúng render theo execution như lúc pass bắt đầu và merge lên giá trị hiện tại, theo thứ tự run, nên kết quả không phụ thuộc thời điểm. **(2026-10-19, gói nợ D10)** Gạch đầu dòng này lúc đầu ghi "Run sau thấy kết quả của run trước". Điều đó đã bị rút lại: trong Kestra mọi update của một chu kỳ render bằng `RunContext` đã đóng băng lúc chu kỳ bắt đầu (`handleWorkerTasks`), còn `overwrite` và phép merge dùng variables sống (`handleExecutionUpdatingTasks`). Xem spec nợ §4.2.
- **`runIf` falsy** cho ra `CREATED SKIPPED` qua đường sẵn có, không ghi gì.

**(2026-10-18, final review)** Đoạn trên đã được sửa cho khớp với bản build. Pass là `render_updates`, tách khỏi `render_expressions` vì update LÀM ĐỔI surface mà mọi lần render sau trong settle đọc; kind trong plan là `ExecutionUpdate { op, spec }`; attempt là `CREATED RUNNING SUCCESS`, đúng golden. Và việc hoàn tất run đã chuyển từ `advance()` vào pass (final review I1): khi còn để cho `advance()`, một update đã tính nằm cạnh một update lỗi trong cùng `Parallel` hoặc `Dag` bị `fail_the_rest` đánh fail (`CREATED FAILED`, không attempt) trong khi settle vẫn ghi thay đổi của nó. Giờ thay đổi và `SUCCESS` đi cùng nhau, và chỉ update lỗi bị fail, giống Kestra. Nhánh `ExecutionUpdate` của `flowable::open` được giữ lại với cùng cách hoàn tất, cho caller tự đặt `rendered`.

### 3.2 Variables của execution (migration 0196)

> **Đổi số (2026-10-19):** viết ban đầu là migration 0195; #694 đã merge `0195_perm_engine_grants.sql` trước, nên gói này ship thành `0196_workflow_execution_variables.sql`.

- **Cột:** `ALTER TABLE workflow.executions ADD COLUMN variables jsonb`, kèm CHECK chỉ cho phép NULL hoặc một object JSON. NULL nghĩa là "dùng `variables:` của flow".
- **Render:** `vars` là `variables` của execution nếu đã có, nếu chưa thì là `variables:` của flow (`worker/render.rs:183`). Đây là quy tắc trong `RunVariables` của Kestra: biến của execution trước, biến của flow làm dự phòng (`core/…/runners/RunVariables.java:541-550`).
- **`SetVariables`:**
  - `variables: Property<Map>` bắt buộc và được render; `overwrite` mặc định `true`.
  - Kết quả được deep-merge vào biến hiện có theo quy tắc D5, tức `deepMerge` của Kestra: giá trị mới là null thì giữ giá trị cũ, hai map merge đệ quy, hai list nối lại (các phần tử mới chưa có trong list cũ được thêm vào theo thứ tự), còn lại thì thay.
  - Với `overwrite: false`, nếu một key cấp trên cùng đã tồn tại thì run fail với đúng message của Kestra: ``"`overwrite` is set to false and the following variables already exist: <k1>,<k2>"``.
- **`UnsetVariables`:**
  - `variables: Property<List<String>>` bắt buộc; `ignoreMissing` mặc định `false`.
  - Key có dấu chấm đi qua các map lồng nhau (`a.b.c`).
  - Thiếu đoạn cuối thì không làm gì, giống Kestra: `vars.remove(key)` với key không tồn tại thì không đổi gì (`UnsetVariables.java`, `removeVar`). Vì vậy trong Kestra, `ignoreMissing` không ảnh hưởng gì tới đoạn cuối.
  - Đoạn cha bị thiếu, hoặc không phải map, thì run fail với message nêu tên key, trừ khi bật `ignoreMissing` (khi đó không làm gì). **Đây là chỗ cố ý khác Kestra:** đệ quy của Kestra truyền `null` xuống và ném `NullPointerException`, dù `ignoreMissing` là gì. Đó là crash, không phải hành vi nên chép lại.
- **Sao chép `variables`:**
  - Replay chép `variables` (D3), qua cả hai cửa: replay khi retry (`worker/replay.rs`) và `POST .../executions/{id}/replay` của user (`routes.rs::replay_execution`, qua `NewExecution::variables`).
  - Một lần lặp của `Loop` kế thừa `variables` của cha tại thời điểm spawn (D10), giống `Execution.loopExecution` của Kestra.
  - Execution con của `Subflow` không kế thừa; `vars` của nó là của flow riêng, giống Kestra.

**(2026-10-18, final review)** Danh sách này từng ghi "execution con của Subflow và của loop không kế thừa … giống Kestra", sai với loop: `loopExecution` của Kestra truyền `this.variables`. Đã sửa bằng D10. Route replay phía user cũng chưa chép `variables` (final review I3); giờ đã chép.

### 3.3 `Labels`

- **`labels` có ba dạng:**
  - chuỗi, được render rồi parse thành một object JSON chứa chuỗi;
  - list `{key, value}`, key trùng thì cái sau thắng;
  - map.

  Key được render. Giá trị là map thì được render và ghi dưới dạng chuỗi JSON. Giá trị khác được render thành chuỗi.
- **Từ chối:**
  - Key bắt đầu bằng `system.` làm run fail: ``"System labels can only be set by Kestra itself, offending label: <k>=<v>"``.
  - Giá trị rỗng làm run fail: ``"Label values cannot be empty, offending label: <k>"``.
- **Kết quả** ghi vào `executions.labels`, tức label riêng của execution (`crate::labels`). Xuất phát từ label hiện tại của execution (label của flow, rồi label riêng; cái sau thắng), các key mới đè lên, và cột lưu danh sách đã gộp. Các lần đọc `{{ labels.x }}` về sau thấy giá trị mới, qua hàm sẵn có `labels::execution_labels`.

**(2026-10-18, C2 đã xây)** Những chỗ bản xây khác §3:
- **`Command::FailUnstarted` và attempt `[FAILED]` (D6).** Update lỗi ở `CREATED` được ghi `CREATED FAILED` với đúng một attempt `[FAILED]`, như golden (`valids/set-variables-duplicate`, `labels-update-task-empty`). `render::fail` sẵn có không đẩy attempt nào, nên một command mới làm việc này.
- **`taskrun.id`.** Id được đặt trước cho run update tạo trong cùng một settle, nên `{{ taskrun.id }}` render được trong `Labels` ở `CREATED` (`valids/labels-update-task`, `npe-labels-update-task`). Mọi loại khác vẫn thấy `taskrun.id` null trong settle tạo ra nó. Đây là khoảng trống đã biết, không đổi.
- **Update khi bị kill (Task 3).** Một update ở `CREATED` dưới lệnh kill đóng `CREATED` sang `KILLING` sang `KILLED` qua hai round, vì `kill::is_leaf` không đếm nó, giống `Exit`.
- **Thứ tự key.** `executions.variables` là `jsonb`, nên thứ tự key sau một lần settle sau đó là của Postgres (key ngắn đứng trước), không phải thứ tự chèn. Kestra giữ thứ tự chèn.
- **Đăng ký.** `runtime::served_for` không cần key cho các type `EXECUTOR_LEVEL`, nên cả ba publish được trên deployment không có handler (test `the_executor_level_types_publish_on_a_deployment_with_no_handlers`).
- **Các khác biệt đã biết so với `Labels` của Kestra** (không golden flow nào phủ):
  - Giá trị label là list hoặc số thực được render thành chuỗi JSON (`["a","b"]`, `1e20`), trong khi `toString` của Java cho `[a, b]` và `1.0E20`.
  - Khi nhiều label vi phạm quy tắc, egret nêu cái đầu theo thứ tự nguồn; Kestra nêu cái đầu theo thứ tự `HashMap`.

**(2026-10-19, gói nợ)** Thay đổi và ghi nhận sau khi C2 được xây (`docs/{en,vi}/specs/2026-10-19-workflow-debt-design.md` §4; số D ở đó là số của spec nợ, không phải của spec này):
- **Giới hạn kích thước (nợ D8, D9).**
  - `executions.variables` bị chặn ở 1 MiB JSON đã serialise (`runners::env::MAX_OUTPUT_BYTES`). `SetVariables` có kết quả merge lớn hơn thì run FAILED với `` `variables`: the execution's variables would be <n> bytes, over egret's limit of 1048576 ```. `UnsetVariables` chỉ làm nhỏ đi nên không bị kiểm.
  - Label của một task `Labels` qua các giới hạn theo-label của backfill: key tối đa 128 ký tự và value tối đa 1.024. Danh sách sau merge có tối đa 100 label, với `` `labels`: the execution would carry <n> labels, over egret's limit of 100 ```. Hai kiểm tra của Kestra vẫn chạy trước, nên message golden không đổi.
  - Đây là giới hạn riêng của egret; Kestra không có. Test: `set_variables_over_the_cap_fails_with_egrets_message` và `more_than_a_hundred_execution_labels_fails` (`cargo test -p egret-workflow --lib -- executor::update`).
- **M5, thứ tự render (nợ D10).** Các update được tạo trong cùng một lượt render thì render theo execution như lúc lượt bắt đầu và merge lên giá trị hiện tại, theo thứ tự run (§3.1 đã sửa). Test: `two_updates_in_one_parallel_render_the_start_and_merge_in_run_order` (`cargo test -p egret-workflow --test worker`). Đã đo: `{{ vars.x }}` trong flow không có `variables:` thì fail trong egret (`undefined value`), như trong renderer chặt của Kestra, nên M5 không thêm khác biệt nào.
- **M6 được ghi lại (nợ D11), một khác biệt đã biết.** Trong Kestra, exception của update lỗi đi vào `handleFailedExecutionFromExecutor`, nơi ép task run chưa xong cuối cùng, hoặc execution nếu không có, sang `FAILED` (`ExecutorService.java:1425-1432` và `:247-252`, `Execution.java:860-902`). Nên update lỗi ở cấp gốc làm execution fail thẳng và `errors:` của flow không chạy, còn update lồng có thể ép fail một run anh em vô tội hoặc `AllowFailure` bao ngoài. Egret chỉ làm fail run của chính update (`FailUnstarted`); `errors:` và `AllowFailure` sau đó áp dụng như với mọi lỗi. Các dạng golden duy nhất (`valids/set-variables-duplicate`, `valids/labels-update-task-empty`) đều ở cấp cao nhất, không có `errors:`, và hai hệ thống khớp nhau ở đó. Không đổi: đường của Kestra là đường dự phòng cho một exception, không phải hành vi được thiết kế.
- **M7, task-run id mới cho mỗi lần lặp `LoopUntil` (nợ D12).** Mỗi lần lặp cấp cho mọi descendant được restart một task-run id mới, vì Kestra tạo lại các task run. Nên `{{ taskrun.id }}`, `task_run_id` của các dòng log, URI file của execution và tên container khác nhau giữa các lần lặp. `maxIterations: 3` chạy body bốn lần (lượt đầu và ba lần restart), nên có bốn id, như Kestra (`LoopUntil.java:142`). Test: `each_loop_until_iteration_has_its_own_task_run_id` (`--test worker`). Việc đổi id này nghĩa là gì với container được giữ và với `WorkingDirectory` trong `LoopUntil` là D15 của spec nợ (§4.7).
- **M8 được ghi lại (nợ D13), một khác biệt đã biết.** Kestra đánh giá Flow trigger trên object `Execution` cuối chu kỳ (`DefaultExecutor.java:719-726`). Egret đọc dòng execution khi xử lý event (`flow_trigger/process.rs`), nên một task `Labels` ở settle sau có thể đổi label mà trigger của event trước thấy. Không sửa: cần một snapshot label trên `workflow.flow_trigger_events`, mà `vars` và `outputs` cũng cũ như vậy, nên snapshot chỉ cho label sẽ làm quy tắc đúng một nửa. Không golden flow nào phủ.

## 4. Test và nghiệm thu

- **Unit test:** cho từng handler, và cho phần tính toán thuần của từng op (deep merge, unset theo dấu chấm, ba dạng label, mọi message từ chối), theo kiểu test của `ret.rs`.
- **Test `advance()`:** ba dạng trạng thái ở §1, gồm fail từ `CREATED`, `runIf` bị skip, và hai lần cập nhật trong cùng một round `Parallel`.
- **Test DB:**
  - Phạm vi của `PurgeLogs`: namespace của mình, một namespace con, một namespace anh em `a.bc` không được với tới, một namespace khác bị từ chối, một workspace khác không bao giờ với tới, và vòng xoá theo lô.
  - CHECK của migration 0196.
  - Settle ghi `variables` và `labels`.
  - Replay chép `variables`.
- **Ratchet oracle:** `cargo test -p egret-workflow --test executor`.
  - Mỗi entry khớp được thì ra khỏi `baseline.txt` và danh sách bị từ chối.
  - Không entry nào mới bị vỡ.
  - Không sửa `golden.json`.
  - `purge_logs_full_arguments` nằm lại trong `baseline.txt`, với lý do ở §2.3.
- **Full gate trước mỗi lần merge:** fmt, clippy, suite của egret-workflow, egret-query, egret-dataapi và egret-server; thêm vitest, lint và typecheck nếu có đổi frontend. Chạy trên Postgres và Valkey riêng, không bao giờ dùng dev stack chung.

## 5. Phạm vi

**Trong phạm vi:** sáu task, migration 0196, quy tắc render cho `vars`, và phần sổ sách của oracle.

**Ngoài phạm vi:**
- kho metric (D1);
- log ngoài execution (D2);
- mô hình quyền theo namespace, ngoài quy tắc purge của W9b;
- thay đổi frontend. Trang execution đã hiển thị labels và outputs; hiển thị `variables` để cho một việc frontend sau.
