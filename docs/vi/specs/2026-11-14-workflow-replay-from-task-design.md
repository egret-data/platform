# Workflow package 7b: replay từ một task, trên nguồn được chọn, với input mới

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-14-workflow-replay-from-task-design.md). Mọi giá trị chính xác, tên hàm/lớp, trích dẫn file:line và chi tiết test nằm ở bản tiếng Anh; khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** thiết kế 2026-11-14. Nhánh `feat/workflow-replay-from-task`, một PR. Dòng #7 của roadmap (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), gói thứ hai trong sáu gói con 7a–7f. 7a, restart tại chỗ, đã merge ở #860 (`docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md`, dưới đây trích là "7a Dn"). Kestra là bản checkout tại `data-portal/kestra`, `96893b58` (2.0.0-SNAPSHOT). `ES` là `core/src/main/java/io/kestra/core/services/ExecutionService.java`, và `EC` là `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`.

## 1. Vấn đề

Replay của egret (`POST /api/ws/{ws}/workflows/executions/{id}/replay`, `routes.rs` `replay_execution`) bắt đầu một execution mới của cả flow. Nó dùng source đã đóng băng của execution cũ và cùng các input. Replay của Kestra (`EC:1437-1500`, `ES:393-512`) làm được thêm ba việc:

1. **Bắt đầu từ một task run được chọn** (`taskRunId`). Execution mới sao chép các task run của execution cũ dưới id mới, kèm output của chúng. Nó reset run được chọn và các tổ tiên của nó, và bỏ mọi thứ nằm sau run được chọn trong đồ thị, nên flow tiếp tục chạy từ task đó.
2. **Chạy trên một revision khác** của flow (`revision`).
3. **Chạy với input mới** (`/actions/replay-with-inputs`, body multipart được kiểm tra giống như một lần start).

Mọi replay của Kestra còn thêm label `system.replay: true` và `attempt + 1` (`ES:497-501`). Replay của egret không thêm cái nào.

Restart của Kestra cũng nhận `revision` (`ES:281-373`). Với revision, restart không làm tại chỗ: nó tạo một execution mới trên revision đó, sao chép các task run, reset những run bị lỗi và gắn label `system.restarted`. 7a từ chối tham số này bằng `restart_revision_unsupported`, và nêu tên 7b (7a Q2).

## 2. Các quyết định

### 2.1 Quyết định của người dùng, 2026-11-14 (đã hỏi, đã trả lời)

- **Q1. Một replay có thể chạy trên ba nguồn.**
  - **Source đã đóng băng** của execution cũ vẫn là mặc định, như hiện nay.
  - **Source hiện tại của flow** là bản nháp trong `workflow.flows.source`, tức cái mà một lần start chạy. Lựa chọn này dành cho "sửa flow, rồi replay từ task bị lỗi".
  - **Một version đã publish N** là một dòng `workflow.flow_versions`.
- **Q2. Restart với revision nằm trong 7b.**
  - Theo Kestra: một execution mới sao chép các task run và reset những run bị lỗi. Nó dùng chung cơ chế sao chép của 7b và chỉ khác ở việc reset những run nào.
  - Restart trên source đã đóng băng vẫn là tại chỗ, đúng như 7a đã xây.
- **Q3. Breakpoint không nằm trong 7b.**
  - Tham số `breakpoints` của Kestra dừng một replay trước các task được chọn và đi kèm "resume from breakpoint". Đó là tính năng debug có state pause và resume riêng, nên nó có gói riêng, đứng cạnh 7c (pause và resume).
  - 7b từ chối tham số này bằng 400, code `replay_breakpoints_unsupported`, và message nêu tên gói sau.

- **Q4–Q7 (2026-11-14, sau phần điều tra của plan; xem "Spec corrections" của plan).**
  - **Q4 (S1):** replay từ một task bỏ các run `errors:` của mọi scope mà nó đi ngược qua, để task được chọn thật sự chạy lại. Kestra giữ chúng, và executor của nó khi đó vẫn ở chế độ lỗi.
  - **Q5 (S2):** restart trên source khác mang theo output của các run được giữ, như replay từ một task. Restart với revision của Kestra không mang theo gì.
  - **Q6 (S4):** một run `Loop` bị reset trong bản sao chuyển sang `CREATED`, và các iteration của nó chạy lại dưới execution mới. Kestra làm `Loop` như vậy lỗi ngay.
  - **Q7 (S7):** replay từ một task và restart trên source khác cũng từ chối `files_pruned` và `after_execution_running`, như restart của 7a. Replay cả execution không đổi.

- **Q8 (2026-11-14, trong lúc làm A3). Variables đi theo source được chọn, như Kestra.** Khi text của source được chọn khác source đã đóng băng, execution mới lưu `variables` là NULL, để `variables:` của chính source đó có hiệu lực (Kestra `applyNewRevision`, `ES:514-517`). Trên source đã đóng băng, `variables` của execution cũ được mang theo nguyên vẹn. Plan đã nói cột cũ luôn được mang theo; người dùng chọn quy tắc trung thành với Kestra sau review A3 (phát hiện I1). Đây là dạng có ghi ngày của câu cuối của D6. Ghi lại thành điểm khác biệt số 12 ở §5.

### 2.2 Quyết định thiết kế (quyết định mặc định, 2026-11-14)

- **D1. Sao chép rồi reset, như Kestra.** Replay từ một task, hoặc restart trên source khác, tạo dòng execution mới và sao chép các task run của execution cũ vào đó. Mỗi bản sao nhận id mới (Kestra `mapTaskRunId`) với liên kết cha được ánh xạ lại và output được mang theo (`taskOutputService.copyOutputs`).
  - Việc reset dùng các lệnh thuần của 7a trong `executor::restart` trên các run đã sao chép, và executor chạy tiếp từ đó.
  - Phương án bị bác: bắt đầu mới và chèn các run `SUCCESS` giả cho những task đứng trước task được chọn. Nó mất output thật và để các flowable (`Parallel`, `Loop`, `Switch`) có state không khớp với các con của chúng.
- **D2. Replay từ một task reset run đó và bỏ những gì sau nó** (`ES:410-492`):
  - Run được chọn và các tổ tiên flowable của nó bị reset (Kestra `taskRunToRestart`, `ES:375-390`).
  - Mọi run đứng sau run được chọn trong đồ thị đều bị bỏ (`GraphUtils.successors`, `ES:437-454`). Chỉ bỏ các successor, không bỏ run được chọn hay tổ tiên của nó.
  - Một run chưa kết thúc trong execution cũ (ví dụ một sibling của `Parallel`) cũng bị reset (`ES:456-484`).
  - Các con worker-task của một run bị reset bị xoá (`removeWorkerTask`, `ES:491-492`).
  - Execution mới bắt đầu ở `RESTARTED` (`ES:494-496`).
  - Việc các successor trong đồ thị của Kestra có gồm các run `errors`/`finally`/`afterExecution` hay không được kiểm tra với Kestra khi lập plan, và egret theo đúng những gì Kestra làm. Restart của Kestra xoá chúng một cách tường minh (`ES:338-351`); replay của nó không nêu tên chúng.
- **D3. Restart trên source khác reset mọi run có thể restart** (`ES:311-351`). Tập reset là các run có state `canBeRestarted` (failed, hoặc paused, như 7a) và các tổ tiên của chúng. Các con worker-task bị xoá, cùng với các run của `errors`, `finally` (kèm các con của chúng) và `afterExecution`. Label là `system.restarted: true` và execution mới nhận `attempt + 1` (`ES:363-367`).
- **D4. Liên kết và file của execution mới.** Nó mang `replayed_from_execution_id` = id cũ, là cột mà replay hiện nay ghi (migration 0166). Các output được sao chép giữ nguyên URI `kestra://`, trỏ tới file của execution cũ.
  - Điều này đòi hỏi quy tắc đọc hiện nay, theo đó một replay đọc file của source của nó, phải với tới đủ xa. Plan kiểm tra độ sâu của quy tắc: replay của một replay phải đọc được output nằm lùi hai execution.
  - Nếu quy tắc chỉ phủ một bước, bản sao không ghi lại gì. Quy tắc đọc thay vào đó đi theo chuỗi `replayed_from`, bị chặn bởi độ dài chuỗi.
- **D5. Ánh xạ sang source khác.**
  - Các run được sao chép được khớp với task của source được chọn theo `taskId`, như Kestra.
  - Nếu task của task run được chọn, hoặc task của bất kỳ run nào được giữ và không bị reset, không có trong source được chọn, request bị từ chối bằng 400, code `replay_task_not_in_source`, và message nêu task id.
  - Kestra không kiểm tra điều này mà để lỗi sau đó trong executor, nên đây là một điểm khác biệt của egret (§5).
  - Các run bị reset hoặc bị bỏ không cần khớp: một run bị reset được lập plan lại từ source mới.
- **D6. Source mà một replay chạy.**
  - `frozen` dùng `executions.source`, như hiện nay.
  - `current` dùng bản nháp của flow, đọc trong transaction của replay.
  - `version:N` dùng source của dòng `flow_versions` đó. N không tồn tại trả 404, code `flow_version_not_found`.
  - Trong mọi trường hợp, execution mới đóng băng source mà nó đã chạy, như mọi execution.
  - Variables của flow đi theo source được chọn, như `applyNewRevision` của Kestra lấy `flow.getVariables()` (`ES:514-517`). `variables` của lần chạy của execution (7a D3 của W9) được mang theo nguyên vẹn.
- **D7. Input.**
  - Không có `inputs`, input của execution cũ được dùng lại, được hiển thị như hiện nay (KI-112).
  - Có `inputs`, body (JSON, hoặc multipart như `start_flow_execution` nhận) thay thế chúng. Nó được kiểm tra với các input khai báo của **source được chọn** bằng cùng bước kiểm tra của start, như `readExecutionInputs(flow, current, inputs)` của Kestra kiểm tra (`EC:1497-1498`).
  - Input `FILE` theo quy tắc của route start.
  - Thay input không reset run nào, như Kestra. Một run được giữ giữ output mà nó đã tính từ input cũ.
- **D8. Các cổng, tất cả đều là của replay hiện nay:**
  - `workflow:manage`;
  - `startable_flow` (flow đã vào thùng rác hoặc đã purge trả 404, flow bị disable trả 409);
  - execution cũ phải ở trạng thái kết thúc (409);
  - một iteration của loop bị từ chối (400, nêu tên loop root của nó);
  - `sql_principal::check_start` với **người gọi** trên source sẽ chạy (7a D24). Với `current` hoặc `version:N`, bước kiểm tra đọc source đó, nên người không phải creator không thể chạy một version thêm task `egret.sql`.
- **D9. Các từ chối riêng cho một task run:**
  - Một `taskRunId` không phải là run của execution này trả 404, code `task_run_not_found`, bất kể nó thuộc workspace hay execution nào. Kestra trả `IllegalArgumentException` (`EC:1503-1507`), hiện ra là 400 hoặc 500; egret dùng câu trả lời not-found mà các route của nó vốn đã đưa ra.
  - Một task run bên trong một iteration của `Loop` trả 400, nêu tên task `Loop` và execution gốc cần replay từ đó. Kestra replay sub-execution riêng của iteration (`ES:393-401`), nhưng egret không replay riêng các iteration (phán quyết W9a); replay của root từ run `Loop` cho cùng kết quả.
  - Một task run bên trong một khối `WorkingDirectory` trả 409 với code `inside_working_directory`, như mark-as của 7a (7a D15).
- **D10. Label và attempt trên mọi replay.** Mọi replay, kể cả replay cả execution đang có hôm nay, thêm `system.replay: true` (trừ khi label đã có) và đặt `attempt_number` = cũ + 1 (`ES:497-501`). Migration 0225 (7a) đã cho phép `attempt_number > 1` trên mọi dòng, nên không dự kiến migration nào cho việc này.
- **D11. Hình dạng API.**
  - `POST …/executions/{id}/replay` nhận các query parameter `taskRunId`, `source` (`frozen` | `current` | `version:N`) và `breakpoints` (bị từ chối), cùng một body tuỳ chọn có `inputs` (JSON hoặc multipart). Đây là một route trong khi Kestra có hai; route start của egret vốn đã nhận cả hai loại body.
  - `POST …/executions/{id}/restart` nhận `source` với cùng các giá trị. `frozen`, hoặc không có giá trị, giữ restart tại chỗ của 7a.
  - Trên cả hai route, tham số `revision` của Kestra được chấp nhận như một số nguyên N có nghĩa là `version:N`, nên một lệnh gọi theo dạng Kestra chạy được. Truyền cả `revision` và `source` trả 400, `invalid_source`. Code `restart_revision_unsupported` của 7a bị gỡ bỏ.
  - Mọi giá trị khác của cả hai tham số trả 400, code `invalid_source`.
- **D12. Response.** Một replay trả `201` với execution mới, như hiện nay. Một restart tạo execution mới trả `201` với execution mới; một restart tại chỗ giữ `200` của 7a. Mỗi route mô tả cả hai câu trả lời trong OpenAPI.
- **D13. Audit.**
  - `workflow_execution:replay` có thêm `task_run_id`, `source` và `inputs_replaced` (một boolean; bản thân các input không bao giờ được ghi log).
  - Một restart trên source khác được audit là `workflow_execution:restart`, với `replacement` đặt là id mới và có `source`.
- **D14. Frontend.**
  - Dialog **Replay** có thêm lựa chọn source: *Version của lần chạy này* (mặc định), *Version hiện tại của flow*, hoặc một version đã publish từ danh sách.
  - Nó cũng có thêm một mục tuỳ chọn **Change inputs**, một form dựng từ các input của source được chọn và điền sẵn giá trị cũ.
  - Menu của mỗi task run có thêm **Replay from this task**, mở cùng dialog đó với tên task trong tiêu đề.
  - Dialog **Restart** có cùng lựa chọn source. Khi chọn một source khác mặc định, text của nó nói rằng restart tạo một execution mới.
  - Mọi từ chối hiện message của nó trong dialog. Các chuỗi hiển thị cho người dùng đi qua i18n sẵn có của trang execution.

## 3. Hình dạng của thay đổi

| Khu vực | Thay đổi |
|---|---|
| `executor::restart` (thuần) | Một lệnh `replay_from` (D2) và tập reset cho restart trên source khác (D3), đặt cạnh các lệnh của 7a, kèm unit test. |
| `worker` | Một bản sao của cây task-run vào execution mới, với id và parent được ánh xạ lại và output được mang theo (D1, D4), được các route replay và restart gọi trong một transaction cùng dòng mới. |
| `routes.rs` | Replay và restart có thêm `taskRunId`, `source` và inputs. Các từ chối của D5, D8, D9 và D11, label và attempt (D10), audit (D13) và OpenAPI. |
| Quy tắc đọc storage | File của chuỗi source của một replay (D4), nếu kiểm tra độ sâu đòi hỏi. |
| Frontend | Các dialog Replay và Restart và mục menu task-run (D14), sinh lại bằng `npm run gen:api`. |
| Docs | Trang Run actions và ảnh chụp của `egret-data/docs`; bản tiếng Việt của spec này. |

Migration: không dự kiến (D10). Nếu cần một migration, số của nó được kiểm tra với main và các PR đang mở trước khi push.

## 4. Test

- **Thuần** (`executor::restart::tests`), các ca replay-from:
  - trong một `Sequential`, các run sau task được chọn bị bỏ;
  - trong một `Parallel`, các sibling chưa kết thúc bị reset;
  - trong một `Switch`;
  - tập reset gồm các tổ tiên flowable;
  - các con worker-task bị xoá.
- **Thuần**, restart trên source khác:
  - tập reset;
  - các run `errors`/`finally`/`afterExecution` bị xoá.
- **Integration** (`tests/replay.rs`, mới):
  - Replay từ một task trong một `Sequential`: các task trước không chạy lại và giữ output, các task sau chạy.
  - Replay từ một task bên trong một nhánh `Parallel`.
  - Replay từ một task bên trong một `Loop`: run nằm trong root, và request nêu tên run `Loop`.
  - Replay từ một run `Subflow`: run được replay bắt đầu một child mới.
  - `source=current` sau khi bản nháp được sửa chạy task đã sửa.
  - `source=version:N` chạy version N.
  - Một task không có trong source bị từ chối.
  - Input mới tới được các task được replay và bị bước kiểm tra của start từ chối khi không hợp lệ.
  - Label và `attempt_number` cũng được đặt trên replay cả execution.
  - Replay của một replay đọc được output lùi hai execution (D4).
  - Restart trên source khác: một execution mới trong đó task lỗi bị reset và các task thành công được giữ.
- **Routes** (`tests/replay_routes.rs`, mới):
  - mọi code từ chối của D5, D8, D9 và D11, mỗi cái kèm một lệnh gọi đối chứng được chấp nhận;
  - `workflow:read` bị từ chối;
  - các trường audit;
  - alias `revision=N`.
- **Frontend:** Vitest cho lựa chọn source và form inputs của dialog, và cho mục menu task-run.
- **Gate:** ratchet và census của executor giữ nguyên.
- **Kiểm tra trực tiếp** trên trình duyệt với một `egret serve` native: replay từ một task, replay trên source hiện tại sau khi sửa, restart trên source khác.

## 5. Các điểm khác biệt đã ghi nhận so với Kestra

1. Replay và restart trên source khác từ chối một task không có trong source đó (D5). Kestra để executor lỗi.
2. Một task run bên trong một iteration của `Loop` được replay từ run `Loop` của root (D9). Kestra replay sub-execution riêng của iteration.
3. "Revision" cũng có thể là bản nháp hiện tại của flow (Q1). Revision của Kestra là mỗi lần save; của egret là các version đã publish cộng bản nháp.
4. Breakpoint bị từ chối cho đến khi có gói riêng (Q3).
5. Replay và replay-with-inputs là một route (D11).

Đã được người dùng xác nhận ngày 2026-11-14 (Q4 đến Q7 và "Spec corrections" của plan):

6. **Replay từ một task bỏ các run `errors:` của mọi scope mà nó đi ngược qua (S1, Q4).** `GraphUtils.successors` của Kestra (`GraphUtils.java:144-181`) giữ chúng, trừ ba trường hợp (từ Kestra `7a48d1dca`, #6959, `:157-169`): run được chọn là task top-level đầu tiên của flow, là một flowable, hoặc là task đầu tiên của một danh sách `errors:`. Ở chỗ Kestra giữ chúng, executor của nó vẫn ở chế độ lỗi (`Execution.java:595-606`) và task được chọn không bao giờ chạy lại. egret bỏ các run errors của scope, cùng các cây con của chúng, khi run được chọn hoặc một tổ tiên đã đi ngược qua nằm trong `tasks` của scope đó. Kết quả chỉ khác Kestra ở chỗ kết quả của Kestra không thể chạy. Các run `finally:` và `afterExecution:` được xử lý đúng như Kestra (`GraphUtils.java:365-399`, `:413-421`, `:488-489`).
7. **Restart trên source khác mang theo output của các run được giữ (S2, Q5).** Restart với revision của Kestra không sao chép gì (`ES:316-331` không có lời gọi `copyOutputs`; chỉ replay mới có, `ES:425`; `TaskRun.forChildExecution`, `TaskRun.java:193-208`). Trong Kestra, một task sau đọc output được giữ sẽ lỗi. egret dùng chung cơ chế sao chép của replay.
8. **Một run `Loop` bị reset trong bản sao chuyển sang `CREATED` và sinh mọi iteration dưới execution mới (S4, Q6).** Kestra không tìm thấy sub-execution nào của execution mới (`ES:985-991`) và làm `Loop` được sao chép lỗi ngay (`ExecutorService.java:735-757`). Đây là điều làm cho câu "replay của root từ run `Loop` cho cùng kết quả" của D9 đúng.
9. **Lịch sử state của execution mới bắt đầu `CREATED`, rồi `RESTARTED`, rồi `RUNNING` (S10).** Bản sao của Kestra mang lịch sử của execution cũ và nối thêm `RESTARTED` (`Execution.childExecution`, `Execution.java:405-432`; `ES:494-496`). egret giữ lịch sử, timestamp và outbox của một dòng mới.
10. **Các run được chọn trên plan mà các run cũ đã chạy (S11).** Restart của Kestra chọn theo flow của execution cũ (`ExecutionCommandMessageHandler.java:90-94`); replay của nó dựng đồ thị từ flow của revision mới (`:176-187`). egret chọn cả hai trên plan cũ, vì `RunIx` và `TaskIx` đánh chỉ mục vào đó, và bước khớp của D5 sau đó từ chối một run được giữ đã bị di chuyển. Do đó một danh sách bị sắp xếp lại có thể bỏ một run mà Kestra sẽ giữ.

Phát hiện trong lúc xây:

11. **Một `Switch` được sao chép giữ case cũ.** Kestra render lại giá trị `Switch` trên source mới. egret mở lại flowable nhưng giữ case mà run cũ đã chọn (review A3, nhỏ). Một task bị di chuyển trong nhánh `If` hoặc `Switch` bị bước kiểm tra nhánh từ chối (`a0d0eb8be`).
12. **Variables đi theo source được chọn (Q8).** Khi text của source được chọn khác source đã đóng băng, execution mới lưu `variables` là NULL, như `applyNewRevision` của Kestra lấy `variables` của flow mới (`ES:514-517`). Trên source đã đóng băng, `variables` cũ được mang theo.
13. **Replay với input đã đổi thay thế input toàn bộ, như Kestra.** Một input `SECRET` tuỳ chọn để trống bị bỏ, không được giữ. Dialog nói "Enter it again: the stored value is hidden." trên một field secret. Không có `inputs`, input cũ được dùng lại, được hiển thị như hiện nay.
14. **Từ chối `files_pruned` chỉ đi theo chuỗi `replayed_from`.** Câu query trong `worker/copy.rs` (`OLD_ROW`, `chain` đệ quy) đi theo `replayed_from_execution_id`, không đi theo `parent_execution_id` của một retry replay. Một source có file bị pruner xoá sau một liên kết retry-parent không bị từ chối. Kestra không có từ chối như vậy; đây là một lỗ hổng trong gate của chính egret, cần xác nhận ở review cuối.

Các khác biệt do bảo mật, mang từ 7a D24 và mở rộng:

- Route cần `workflow:manage`.
- Gate creator `egret.sql` đánh giá người gọi trên source sẽ chạy (D8). Với `current` hoặc `version:N`, người không phải creator không thể chạy một version thêm task `egret.sql`. Nó được đánh giá trong transaction, trên text sẽ chạy.
- Flow đã vào thùng rác hoặc đã purge trả 404 và flow bị disable trả 409 (`startable_flow`). Kestra không có thùng rác.
- Một source được chọn (`current` hoặc `version:N`) có YAML riêng ghi `disabled: true` bị từ chối 409, như `FlowService.getFlowIfExecutableOrThrow(.., revision)` của Kestra từ chối một revision bị disable (`FlowService.java:861`). Mã 409 này không có code.
- `files_pruned` và `after_execution_running` áp dụng cho replay từ một task và restart trên source khác (Q7), không áp dụng cho replay cả execution.

## 6. Ngoài phạm vi

- Breakpoint (Q3).
- Replay và restart hàng loạt theo ids hoặc theo query: các thao tác hàng loạt của dòng #7, sau này trong 7a–7f.
- Restart `NEW_EXECUTION` của Subflow: dòng roadmap #12.
- Pause thủ công và resume với input `onResume`: 7c.

## 7. Như đã xây

Xây ngày 2026-11-14 trên `feat/workflow-replay-from-task`, base `origin/main` `844ec47db`. Không có migration (D10 đúng: 0225 đã cho phép `attempt_number > 1`; migration cao nhất vẫn là 0225). Ratchet của executor ở T0 là **301 / 25 / 25 / 0** và census **295 / 25 / 21**. Ratchet và census ở cuối không đổi: **301 / 25 / 25 / 0** và **295 / 25 / 21** (`--test executor`, 62 pass, trên `4b6bf572d`).

| Task | Commit | Đã làm gì | Vòng sửa |
|---|---|---|---|
| Spec, plan | `d1cc6c09c`, `d466cba59`, `e2d20e35d` (Q4 đến Q7) | Spec này, plan của nó, và các quyết định của người dùng sau phần điều tra của plan. | – |
| T0 | – | Container, ratchet và census nền, đo trên `e2d20e35d`: 301/25/25/0, test executor 62/62. | – |
| A1 | `8e1f7244b`, `604976df0`, fix `8ea71269e` | Test drive dùng chung (`executor/test_support.rs`); `replay_from` và `restart_copy` thuần trong `executor/replay.rs`, kèm `scope_errors` của S1. | 1: quy tắc root kích hoạt cho một flowable đứng đầu trong `errors:`; Kestra chỉ thêm root của chính flowable đó |
| A2 | `c44e50904` | `copy_onto` thuần (đánh số lại dày đặc, khớp theo id, parent, list và kind), `shift_ticks`, `reset_copied` (một `Loop` chuyển `CREATED`, S4). | 0 |
| A3 | `4193a282a`, `7ae1ec7f5`, fix `a0d0eb8be`, `3045e0f75` | Tầng worker: `insert_execution` tách khỏi lần settle đầu của start; `worker/copy.rs` `replay_execution` (cả execution, từ một task, restart trên một source); `Op::Replay`, label `system.replay`, `attempt_number` + 1. | 1: variables đi theo source được chọn (Q8); một task được giữ bị di chuyển sang nhánh `If` hoặc `Switch` khác bị từ chối |
| A4 | `40703a80c` | Integration test cho bản sao restart của một loop lỗi, một subflow lỗi và một execution paused, replay của replay, và một replay `afterExecution`. Không đổi code. | 0 |
| A5 | `94066375d` | Quy tắc đọc storage thêm `chain_family`: một bản sao đọc file iteration của loop trong chuỗi source của nó (S5). Một test `storage_index` đã ghim sự loại trừ cũ bị lật lại. | 0 |
| B1 | `563e1a44c`, fix `edea15b2b` | Routes: `taskRunId`, `source`, alias `revision`, từ chối `breakpoints`, input JSON và multipart; các từ chối có code; audit; OpenAPI; `restart_revision_unsupported` bị gỡ (S8). | 1: audit restart-trên-source nêu tên execution bị thay thế; một source được chọn bị disable là 409 |
| C1 | `ff67795e5` (client), `249e4f74b`, fix `4ed0eab19` | Client `gen:api`; `ReplayDialog.tsx` (lựa chọn source, change inputs, text restart); menu hàng task-run "Actions for {task}"; các câu từ chối. | 1: replay bị ẩn cho flow không có id thì đưa ra lý do; lỗi input hiện trong mục đang thu gọn |

Số đếm đã đo (các gate ở Z, 2026-11-14):

- `cargo fmt --check`, `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings` và `cargo check -p egret-bin`: sạch.
- `cargo test -p egret-workflow --no-fail-fast` trên `25fb40103`: 3782 pass, 55 ignored, 8 fail. Bảy cái là `sql_starrocks_live` (máy test không có StarRocks, như trên main). Cái thứ tám là `execution_timestamps::every_state_write_stamps_the_timestamps`: guard chưa biết bản chép nguyên văn. `4b6bf572d` dạy nó một trường hợp (cả `state`, `history`, `started_at` và `ended_at` lấy từ cùng một alias hàng của cùng bảng), kèm các ca tự kiểm chứng rằng bản chép lấy `state` từ chỗ khác vẫn bị từ chối; sau đó `execution_timestamps` 21/21, `replay` 29/29, `replay_routes` 20/20 và `restart` 34/34 pass.
- `cargo test -p egret-config -p egret-server -p egret-metastore`: 179 pass, gồm `committed_openapi_json_is_current` và digest của client.
- Frontend: `npm run typecheck` sạch; `npm run lint` 0 lỗi (154 cảnh báo, không cái nào trong file của gói này); `npx vitest run` 8722 pass và 2 fail do timeout 5 s khi máy tải nặng, ở `OutputsTree.test.tsx` và `ScheduleDetail.test.tsx` (không bị gói này đụng tới); cả hai file pass khi chạy riêng (79/79).
- Kiểm tra trực tiếp trên `egret serve` chạy native (database và storage riêng, cổng 18466), với `admin@egret.local`, sáng và tối: replay từ task `b` trên version của lần chạy này giữ outputs của `a` và chạy lại `b`, `c` (`system.replay: true`, attempt 2); replay từ `b` trên version hiện tại của flow chạy `c` đã sửa; restart một execution FAILED trên version 2 đã publish mở một execution mới SUCCESS gắn nhãn `system.restarted: true`. Ba dòng audit có `task_run_id`, `source`, `inputs_replaced` và `replayed_from`. Lần kiểm tra phát hiện icon trong menu dòng chưa đặt kích thước; `a4d6afaae` đã sửa.

### Sai lệch và lý do

- **Phần văn xuôi của plan mà quá trình xây đã sửa.** Plan nói `variables` cũ luôn được mang theo; Q8 thay thế điều đó (review A3 I1). `role_of` của plan gộp các nhánh `If` và `Switch` thành một danh sách, nên một task bị di chuyển thành mồ côi; một bước kiểm tra nhánh từ chối nó (review A3 I2).
- **A1: một run `LoopUntil` giữ count của nó.** Count lấy từ lịch sử của run, mà bản sao mang theo. Kestra cũng giữ `iteration` (`RestartCaseTest.restartOrReplayLoopUntil`).
- **A5 chạm vào một test ngoài danh sách file của nó** (`storage_index.rs`), vì test đó ghim sự loại trừ mà task gỡ bỏ. Spec W9a có một ghi chú đính chính có ngày.
- **C1 chỉ gửi `inputs` khi một field đã được sửa.** Nếu không, body là `{}` và input cũ được dùng lại. Một input `FILE` có giá trị `kestra://` cũ và không có file mới giữ giá trị cũ.

### Các vòng review

Mỗi task được controller review giữa các task. A1, A3, B1 và C1 mỗi cái cần một vòng sửa, tiếp theo là một re-review có phạm vi sạch cho B1, C1 và A3. A2, A4 và A5 được duyệt ngay. Review của A5 là review uỷ quyền: sự loại trừ cũ là một lựa chọn phạm vi của W9a, không phải một ranh giới, và schema giới hạn đệ quy trong các iteration loop của cùng workspace.

### Các phát hiện hoãn lại

- A2: tiền điều kiện của `copy_onto` (tập được giữ đóng dưới quan hệ cha) chưa được ghi lại; test A2.8 dừng ở danh sách lệnh; không có test cho một run được giữ bị di chuyển bên trong danh sách `errors:` hoặc `finally:` của chính một flowable.
- A3: `side_writes` nhận giá trị label qua một `debug_assert`; thiếu các test đỏ cho `AfterExecutionRunning`, `NotTerminal` với một task run, và các ca restart `NotRestartable` và `NothingToRestart`; `metadata.attempt` bị cũ trong lần settle đầu; `origin_offset_ms` không được mang theo (Kestra giữ `originalCreatedDate`); việc retry deadlock trong `copy.rs` lặp lại của 7a.
- A4: test paused không chạy bản sao, nên khẳng định "không chạy lại" của nó vô nghĩa; `failing_nth_run_of` không nguyên tử; test `afterExecution` không kiểm tra thứ tự.
- A5: câu chữ về pruner ở `storage/index.rs:175` gợi ý thời gian lưu giữ dài hơn thực tế; một khẳng định thừa trong `storage_index.rs`; không có test iteration lồng nhau hay test âm cho subflow.
- B1: một restart không-frozen chạy `startable_flow` hai lần; `refuse_disabled_source` parse source hai lần; không có test 413 trên route replay; `startable_flow` chỉ đánh giá `disabled:` trên YAML của bản nháp.
- C1: tiêu đề dialog bị đổi trong lúc đóng; một đường lỗi server chưa được test; text lý do `!hasFlow` hiện trong chế độ replay.
- Còn nợ ở Z: kiểm tra trực tiếp trên trình duyệt (sáng và tối, FE-TEST-2 của 7a) kèm ảnh chụp, và bundle test.

## 8. Phần còn lại của dòng #7

Xem §2.1 của spec 7a. Mỗi gói con có spec, plan và PR riêng.
