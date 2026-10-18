# Follow-up workflow, nhóm C: các task lõi Kestra còn lại

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-18-workflow-core-tasks-design.md` là bản chuẩn; hai bản lệch nhau thì theo bản tiếng Anh.

**Trạng thái:** thiết kế ngày 2026-10-18. **C1 đã xây và kiểm chứng ngày 2026-10-18** trên `feat/workflow-kestra-core-tasks`; C2 chưa xây. Ratchet của oracle (`cargo test -p egret-workflow --test executor`, bản in của `executor_matches_golden`) đi từ 100 mismatching / 100 baselined xuống **84 / 84**, 0 broken: 16 entry được bank (`OutputValues` 11, `Assert` 1, `PurgeLogs` 4), các entry còn nằm trong baseline được nêu theo nguyên nhân trong `oracle-exec/README.md`. Số LIVE của census đi từ 254 lên **281**, trần handler-surface (`python3 handler_surface.py`) từ 232 lên **250** (quần thể 228). Theo từng task: 100 xuống 89, 89 xuống 88, 88 xuống 84.

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
  - Không ghi audit record riêng; bản thân run và các dòng log của nó đã là bản ghi.
- **Log ngoài execution:** `purgeNonExecutionLogs` không xoá gì (D2). Với `purgeExecutionLogs: false` thì không xoá gì cả.
- **Output:** `{count, executionLogsCount, nonExecutionLogsCount}`, với `nonExecutionLogsCount = 0`.
- **Handler cần pool**, nên được đăng ký giống `Delete` (`handlers::storage::register`, `worker/handlers/storage.rs:336`), không nằm trong `with_core_tasks`.
- **Một divergence đã biết:** `valids/purge_logs_full_arguments` chạy trong `io.kestra.tests` với `namespace: purge.namespace`. Kestra trả SUCCESS; egret trả FAILED theo quy tắc đã chốt. Entry này chuyển từ danh sách bị từ chối sang `baseline.txt`, kèm comment ghi lý do đã chốt. Đó là dòng vĩnh viễn, không phải chỗ trống chờ làm.

**(2026-10-18, C1 đã xây)** Những chỗ bản xây khác §2.1-§2.3:
- **Đăng ký.** Cả ba còn có một key trong `runtime::served_for`, bản sao thứ hai của các key `with_core_tasks` mà kiểm tra "deployment này không có handler" của publish đọc. Các mục trên không nêu nó. `PurgeLogs` được đăng ký bằng `worker::handlers::purge_logs::register(&mut handlers, pool)` riêng trong `runtime::handlers_for`, cạnh `kv::register`, chứ không qua `handlers::storage::register` như §2.3 ghi.
- **`Assert`.** Thiếu `conditions` thì run FAILED với `conditions: must not be null` (Kestra khai báo `@NotNull`). `is_truthy` được dùng qua `crate::executor::is_truthy`, vì `executor::flowable` là private với `worker`.
- **`OutputValues`.** `models::property::render_map` là hàm render map đệ quy dùng chung. Hai key render ra cùng một chuỗi thì key đầu thắng; mảng, số, boolean hay null ở cấp cao nhất là lỗi. Có hai mép đã biết và hoãn: va chạm key giữ giá trị đầu kể cả khi nó là null (`putIfAbsent` của Java sẽ thay một null), và `values` là một biểu thức nguyên khối render ra `null` thì lỗi, trong khi `asMap` của Kestra trả null.
- **Một entry oracle bị giữ bởi construct chưa port, không phải bởi C1.** `valids/if-in-parallel` chạy và `assert` của nó FAILED trong khi Kestra SUCCEEDED, vì điều kiện đọc `outputs:` của chính `Loop`, thứ egret chưa sinh ra. Cùng nguyên nhân giữ `loop-outputs*`.

## 3. C2: ba task chạy trong executor

### 3.1 Plan và trạng thái

- **Plan:** thêm `TaskKind::Update { op }` trong `executor/plan.rs`, với `op` là một trong `Set`, `Unset`, `Labels`. Mỗi type là một dòng `spec::registry` không có handler và được liệt kê trong `EXECUTOR_LEVEL` (`spec/registry.rs:333`), nên mọi deployment đều phục vụ được, giống `Exit`.
- **Phần cập nhật được tính khi run còn ở `CREATED`**, trong pass render (`worker/render.rs::render_expressions`), giống `values:` của `Loop` (`renders_at`, `worker/render.rs:1345`). Pass này render các thuộc tính của task dựa trên variables và labels hiện tại của execution, rồi tính giá trị mới.
  - **Lỗi:** run fail kèm message, rồi `advance()` ghi `CREATED FAILED`.
  - **Thành công:** giá trị mới được ghi vào `RenderSide`. Sau đó `advance()` ghi `CREATED RUNNING SUCCESS` trong cùng một round, với một attempt `RUNNING SUCCESS`. Bước settle ghi `variables` hoặc `labels` mới vào dòng execution, trong cùng transaction đang giữ khoá execution.
- **Nhiều run cập nhật trong cùng một round** (hai nhánh của một `Parallel`) được áp dụng lần lượt theo thứ tự run. Run sau thấy kết quả của run trước, nên kết quả không phụ thuộc thời điểm.
- **`runIf` falsy** cho ra `CREATED SKIPPED` qua đường sẵn có, không ghi gì.

### 3.2 Variables của execution (migration 0195)

- **Cột:** `ALTER TABLE workflow.executions ADD COLUMN variables jsonb`, kèm CHECK chỉ cho phép NULL hoặc một object JSON. NULL nghĩa là "dùng `variables:` của flow".
- **Render:** `vars` là `variables` của execution nếu đã có, nếu chưa thì là `variables:` của flow (`worker/render.rs:183`). Đây là quy tắc trong `RunVariables` của Kestra: biến của execution trước, biến của flow làm dự phòng (`core/…/runners/RunVariables.java:541-550`).
- **`SetVariables`:**
  - `variables: Property<Map>` bắt buộc và được render; `overwrite` mặc định `true`.
  - Kết quả được deep-merge vào biến hiện có: map lồng nhau thì merge, giá trị khác thì thay.
  - Với `overwrite: false`, nếu một key cấp trên cùng đã tồn tại thì run fail với đúng message của Kestra: ``"`overwrite` is set to false and the following variables already exist: <k1>,<k2>"``.
- **`UnsetVariables`:**
  - `variables: Property<List<String>>` bắt buộc; `ignoreMissing` mặc định `false`.
  - Key có dấu chấm đi qua các map lồng nhau (`a.b.c`).
  - Thiếu đoạn cuối thì không làm gì, giống Kestra: `vars.remove(key)` với key không tồn tại thì không đổi gì (`UnsetVariables.java`, `removeVar`). Vì vậy trong Kestra, `ignoreMissing` không ảnh hưởng gì tới đoạn cuối.
  - Đoạn cha bị thiếu, hoặc không phải map, thì run fail với message nêu tên key, trừ khi bật `ignoreMissing` (khi đó không làm gì). **Đây là chỗ cố ý khác Kestra:** đệ quy của Kestra truyền `null` xuống và ném `NullPointerException`, dù `ignoreMissing` là gì. Đó là crash, không phải hành vi nên chép lại.
- **Sao chép `variables`:**
  - Replay chép `variables` (D3).
  - Execution con của Subflow và của loop không kế thừa; `vars` của chúng là của flow riêng, giống Kestra.

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

## 4. Test và nghiệm thu

- **Unit test:** cho từng handler, và cho phần tính toán thuần của từng op (deep merge, unset theo dấu chấm, ba dạng label, mọi message từ chối), theo kiểu test của `ret.rs`.
- **Test `advance()`:** ba dạng trạng thái ở §1, gồm fail từ `CREATED`, `runIf` bị skip, và hai lần cập nhật trong cùng một round `Parallel`.
- **Test DB:**
  - Phạm vi của `PurgeLogs`: namespace của mình, một namespace con, một namespace anh em `a.bc` không được với tới, một namespace khác bị từ chối, một workspace khác không bao giờ với tới, và vòng xoá theo lô.
  - CHECK của migration 0195.
  - Settle ghi `variables` và `labels`.
  - Replay chép `variables`.
- **Ratchet oracle:** `cargo test -p egret-workflow --test executor`.
  - Mỗi entry khớp được thì ra khỏi `baseline.txt` và danh sách bị từ chối.
  - Không entry nào mới bị vỡ.
  - Không sửa `golden.json`.
  - `purge_logs_full_arguments` nằm lại trong `baseline.txt`, với lý do ở §2.3.
- **Full gate trước mỗi lần merge:** fmt, clippy, suite của egret-workflow, egret-query, egret-dataapi và egret-server; thêm vitest, lint và typecheck nếu có đổi frontend. Chạy trên Postgres và Valkey riêng, không bao giờ dùng dev stack chung.

## 5. Phạm vi

**Trong phạm vi:** sáu task, migration 0195, quy tắc render cho `vars`, và phần sổ sách của oracle.

**Ngoài phạm vi:**
- kho metric (D1);
- log ngoài execution (D2);
- mô hình quyền theo namespace, ngoài quy tắc purge của W9b;
- thay đổi frontend. Trang execution đã hiển thị labels và outputs; hiển thị `variables` để cho một việc frontend sau.
