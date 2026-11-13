# Workflow package #7a "Restart, task-run state và execution status"

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-13-workflow-execution-ops-restart-design.md). Mọi giá trị chính xác, tên hàm/lớp, trích dẫn file:line và chi tiết test nằm ở bản tiếng Anh; khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** thiết kế 2026-11-13; đã xây 2026-11-13 (Task T0, A1–A5, B1, C1; §7). Nhánh `feat/workflow-execution-ops`, một PR. Dòng #7 và ghi chú 7 của `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Dòng #7 cỡ L và được tách thành sáu gói con (quyết định của người dùng, 2026-11-13); đây là gói đầu tiên. Plan: `docs/en/plans/2026-11-13-workflow-execution-ops-restart.md`. Kestra là bản checkout tại `data-portal/kestra`, `96893b58` (2.0.0-SNAPSHOT).

Viết tắt: `EW` = `crates/egret-workflow/`; `ES` = `kestra/core/src/main/java/io/kestra/core/services/ExecutionService.java`; `EC` = `kestra/webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`; `ExS` = `kestra/executor/src/main/java/io/kestra/executor/ExecutorService.java`; `KS` = `kestra/core/src/main/java/io/kestra/core/models/flows/State.java`.

## 1. Vấn đề

egret có thể start, kill, resume và replay một execution, ngoài ra không làm được gì khác (`EW src/routes.rs:3112-3527`). Ba thao tác của Kestra tác động lên execution đã kết thúc vẫn thiếu:

1. **Restart** (`POST /executions/{id}/actions/restart`, `ES:281-373`). Chạy lại một execution `FAILED` (hoặc `PAUSED`) **tại chỗ**: cùng execution id, cùng task-run id, các task run lỗi và tổ tiên của chúng được reset, các run end-hook bị gỡ, và executor chạy tiếp từ đó.
2. **Đổi state của một task run** (`POST /executions/{id}/actions/state`, `ES:521-559`, `ES:603-662`). Ghi đè state của một task run ("mark as SUCCESS") rồi restart execution để phần phía sau task run đó được chạy.
3. **Đổi status của execution** (`POST /executions/{id}/actions/change-status`, `ES:1385-1392`). Chỉ ghi lại state kết thúc của một execution đã xong.

Phần còn lại của dòng #7 thuộc các gói con 7b–7f (§8).

## 2. Các quyết định

### 2.1 Tách gói (người dùng, 2026-11-13)

| Gói con | Phạm vi |
|---|---|
| **7a** | Spec này. |
| 7b | Replay từ một task được chọn, với input mới và lựa chọn revision. **Restart với `revision`** cũng thuộc đây (Q2 bên dưới). |
| 7c | Pause thủ công; resume với input `onResume`; resume/validate. |
| 7d | Xoá một execution (log, storage); API label; tải và xoá log. |
| 7e | SSE follow (execution, log); endpoint eval biểu thức; export, distinct, average-duration, latest. |
| 7f | Mọi thao tác hàng loạt `by-ids` / `by-query`. |

### 2.2 Quyết định của người dùng, 2026-11-13 (đã hỏi, đã trả lời)

- **Q1. Một child `Subflow` bị lỗi được restart tại chỗ**, như `restartBehavior: RETRY_FAILED` mặc định của Kestra (`kestra/core/.../runners/ExecutableUtils.java:91-170`). Child không ở trạng thái `FAILED` thì không được restart, và run `Subflow` của parent kết thúc `SUCCESS`. Lựa chọn `NEW_EXECUTION` và bản thân property `restartBehavior` vẫn thuộc dòng #12 (D9).
- **Q2. Restart với `revision` thuộc 7b.** Trong Kestra nó hoàn toàn không phải tại chỗ: nó tạo execution mới trên một revision khác của flow (`ES:289`, `ES:367`), tức là một replay. 7a từ chối tham số `revision`, nêu tên 7b.

### 2.3 Quyết định thiết kế (quyết định mặc định, 2026-11-13)

Được đưa ra theo các quy tắc tự chủ (theo Kestra trừ khi có lý do; giữ chặt tính an toàn và đúng đắn).

- **D1. Reset trong bộ nhớ, không bao giờ xoá row.** Restart là một hàm thuần của executor, `executor::restart(plan, &state) -> Vec<Command>`, được áp dụng rồi settle theo cách `resume_by_hand` làm (`EW src/worker/claim.rs:4469-4485`). Một run mà Kestra *gỡ bỏ* thì được **làm rỗng** trong bộ nhớ, như `Command::RestartLoop` đã làm cho một iteration của `LoopUntil` (`EW src/executor/state.rs:998-1041`), và walk dựng lại nó bằng `RecreateRun` khi đến lượt. Xoá row không phải lựa chọn: `load_with` từ chối khoảng trống trong `run_ix` (`claim.rs:4771-4782`), `persist` không bao giờ xoá (`claim.rs:5404-5577`), và `task_logs` / `execution_files` khoá theo task-run id mà không có FK (migration 0159, 0166).
- **D2. Những run nào bị reset.** Mọi run có state riêng là `FAILED` hoặc `PAUSED`, cộng với mọi tổ tiên của chúng (`ES:311-314`, `ES:375-391`). Các run `KILLED`, `WARNING`, `CANCELLED`, `RETRIED`, `SKIPPED` và `SUCCESS` được giữ nguyên trừ khi là tổ tiên. Nếu không run nào đủ điều kiện, restart bị từ chối (409, "no task to restart from", tương ứng `"No task found to restart execution from!"` của Kestra). **Mọi** run lỗi đều được reset, không chỉ một: một `Parallel` mà rule 1 đã làm fail các sibling đang sống (`EW src/executor/flowable/parallel.rs:192-220`) nếu không sẽ làm chúng fail lại cùng lúc.
- **D3. State mới của một run bị reset.** Kestra đặt `RESTARTED` lên leaf bị reset và lên `WorkingDirectory`/`LoopUntil`/`Loop`, và `RUNNING` lên các flowable tổ tiên khác (`ES:1227-1256`). egret không bao giờ đặt `RESTARTED` lên một run, vì `sequential::walk` coi run mà nó không biết là `Waiting` và sẽ bị đứng (survey §1). Kestra coi `RESTARTED` như "created" (`KS:300-302`), nên egret ánh xạ nó:

  | Run | Kestra | egret |
  |---|---|---|
  | Runnable leaf, `Subflow` | `RESTARTED` | `CREATED`. `open` của walk thêm `SUBMITTED` và một attempt mới, như `retry::reopen_due` (`EW src/executor/retry.rs:511-534`). |
  | Flowable tổ tiên (`Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `AllowFailure`, …) | `RUNNING` | `RUNNING` |
  | `Pause` (một flowable trong Kestra) | `RUNNING` | `RUNNING`. History của nó đã chứa `PAUSED`, nên nó đi tiếp như thể đã được resume (Kestra `Pause.java:269-272`). |
  | `WorkingDirectory` | `RESTARTED`, mọi descendant bị gỡ (`ES:335-337`, `ES:1179-1200`) | `CREATED`, mọi descendant bị làm rỗng, `working_dir` bị xoá, để block chạy lại toàn bộ trong một thư mục mới. |
  | `LoopUntil` | `RESTARTED`, giữ children; đồng hồ `maxDuration` của nó khởi động lại (`LoopUntil.java:148-153`) | `RUNNING`, children bị reset theo D2. Đồng hồ khởi động lại (D6). |
  | `Loop` | `RESTARTED`; executor sau đó restart iteration lỗi cuối cùng (`ExS:735-762`) | `RUNNING`, không bao giờ `CREATED` (vì như thế sẽ spawn lại iteration 0 và va chạm trong `spawn::open_batch`, `EW src/worker/spawn.rs:136-266`). Iteration lỗi cuối cùng được restart tại chỗ (D8). |

- **D4. Các run end-hook bị làm rỗng (roadmap note 7).** Kestra gỡ mọi run của `errors:` và `finally:` của flow và của mọi flowable (cùng children của chúng), và mọi run của `afterExecution:` (`ES:339-351`). egret làm rỗng đúng tập đó và cấp cho mỗi run bị làm rỗng một task-run id mới (`renew_restarted_ids`, `claim.rs:5266-5279`, debt D12), để log, file và container được giữ lại của nó vẫn thuộc id cũ. Nếu không, `run_of` (lấy run đầu tiên của một task, `EW src/executor/flowable/mod.rs:359-366`) làm `current_list` tiếp tục pha `errors`/`finally` cũ, và `after_execution::terminated` (`EW src/executor/after_execution.rs:236-250`) đọc pha after là đã xong.
- **D5. Attempt được giữ lại và nối thêm.** Kestra sao chép `attempts` nguyên vẹn khi restart, và worker nối thêm một attempt mới khi task chạy lại (`TaskRun.java:193-208`; `RestartCaseTest:64`: một run được restart và thành công có 2 attempt). egret làm giống vậy. Ba thứ phụ thuộc vào điều này: tên container hoặc pod `egret-run-{run_id}-{attempt}` (`EW src/worker/commands.rs:808-810`), chỉ số attempt của dòng log (`task_logs.attempt`, migration 0159), và việc lưu file theo phạm vi attempt. Khi attempt được giữ, attempt mới nhận tên và chỉ số mới và không bao giờ gắn lại vào một container `delete: false` còn giữ của attempt trước.
  - Hệ quả, như trong Kestra: `retry:` của một run đếm mọi attempt của nó và đo `maxDuration` từ attempt đầu tiên (`EW src/executor/retry.rs:253-293`), nên một run được restart mà retry đã dùng hết thì không retry lại.
- **D6. Đồng hồ của flowable được restart khởi động lại.** `Pause` và `LoopUntil` đo timeout từ `attempt_started_at` (`EW src/executor/flowable/pause.rs:132`, `flowable/loop_until.rs:360`). Một flowable được đặt lại `RUNNING` sẽ mở một attempt mới tại tick của restart, nên đồng hồ khởi động lại như mục lịch sử `RESTARTED` của Kestra làm.
- **D7. Tác động ở mức execution.**
  - **History:** `RESTARTED` được nối thêm, rồi nhánh `Created | Restarted` hiện có (`EW src/executor/mod.rs:331-346`) nối thêm `RUNNING` trong cùng một lần settle. History luôn chỉ nối thêm (outbox kiểm tra điều đó, `claim.rs:3900-3905`).
  - **`attempt_number` tăng một** (`ES:358`, `ExecutionMetadata.nextAttempt()`). CHECK `workflow_executions_metadata_belongs_to_a_replay` của migration 0139 chỉ cho phép điều này trên một row replay, nên migration của gói (D12) nới lỏng nó. `origin_offset_ms` không đổi, để một retry `CREATE_NEW_EXECUTION` vẫn đo `maxDuration` từ lúc tạo.
  - **Label `system.restarted: "true"`** được thêm nếu chưa có (`ES:360-364`). Chỉ engine được ghi label `system.*` (`EW src/labels.rs:52`). Restart `Subflow` (D9) dựa vào nó, như của Kestra.
  - **Flow trigger bắn lại.** Outbox ghi một event cho mỗi state được nối thêm (`claim.rs:3869-3933`), nên `RESTARTED`, `RUNNING` và state kết thúc mới đều đến được một listener. Kestra làm như vậy.
  - **SLA** `EXECUTION_ASSERTION` được đánh giá lại khi execution còn sống (`claim.rs:3830-3835`). Kestra tạo lại các monitor (`ExecutionEventMessageHandler:160`).
  - **Flow outputs** giữ giá trị cũ cho đến state kết thúc mới, lúc đó chúng được đánh giá lại (`became_terminal`, `claim.rs:3713-3722`).
- **D8. `Loop`: restart iteration lỗi cuối cùng tại chỗ.** Như Kestra (`ES:985-991`, `ExS:735-762`): iteration có `loop_index` cao nhất mà state là `FAILED` hoặc `PAUSED` được restart bằng cùng hàm đó, tại chỗ, dưới lock của chính nó. Run `Loop` quay lại `RUNNING` và không spawn. Nếu không iteration nào đủ điều kiện, run `Loop` kết thúc `FAILED` với dòng log "No restartable loop sub-execution found", như Kestra. Vì `spawn::on_execution_terminal` return ngay khi run loop đang terminal (`spawn.rs:634-640`), run loop phải `RUNNING` trước khi iteration có thể kết thúc lần nữa. Plan chốt cách bộ đếm của loop và danh sách outputs #6 của nó xử lý một iteration kết thúc lần thứ hai (một entry cho mỗi index, không phải hai).
- **D9. `Subflow`: restart child bị lỗi tại chỗ (Q1).** Khi walk mở một run `Subflow` bị reset của một execution có label `system.restarted`, nó tìm child trước đó, được liên kết bằng `trigger_execution_id`/`trigger_run_ix` (`EW src/worker/subflow.rs`) với cùng `value` và iteration; chỉ khớp đúng một thì mới tính.
  - Child là `FAILED` hoặc `PAUSED`: restart nó tại chỗ bằng cùng hàm, và run `Subflow` chờ nó như chờ một child mới.
  - Child ở bất kỳ state kết thúc nào khác: run `Subflow` kết thúc `SUCCESS`. Kestra làm vậy, với comment "don't restart it as it's terminated successfully" (`ExS:1311-1318`).
  - Không khớp hoặc khớp nhiều: spawn child mới, như hiện nay.
  - Thứ tự lock là parent rồi child, như cascade của kill (`EW src/worker/kill.rs:82-104`). Plan dùng lại cơ chế retry deadlock của kill.
- **D10. Các từ chối vượt ngoài Kestra.**
  - Một execution mà `afterExecution:` vẫn đang chạy bị từ chối (409). Restart của Kestra sẽ gỡ các run đang sống đó; kill của egret thậm chí không với tới một block đang được chuẩn bị (`KILL_NOT_APPLIED_CODE`, `routes.rs:3382`), nên gỡ một run đang sống là không an toàn.
  - Một execution mà file đã bị dọn (`files_pruned_at` được đặt, migration 0168) bị từ chối (409): các task của nó sẽ đọc outputs `Unreadable::Pruned`.
  - Một iteration của loop được restart thông qua loop root của nó, không bao giờ riêng lẻ (như replay từ chối một iteration, `routes.rs:3253`). Kestra cũng không có lời gọi nào như vậy; restart loop của nó đi qua parent.
- **D11. Task-run state và execution status.**
  - **State đích.** Cả hai route nhận `SUCCESS`, `WARNING`, `FAILED` hoặc `CANCELLED`, bốn state mà UI của Kestra đưa ra (`ui/src/components/executions/ChangeStatus.vue:101-121`, `ChangeExecutionStatus.vue:96-97`). Server của Kestra nhận bất kỳ state nào cho một task run và bất kỳ state kết thúc nào cho một execution. egret giữ cánh cửa hẹp hơn; state ngoài bốn state đó là 400.
  - **State nguồn.** Cả hai yêu cầu một execution đã kết thúc và không phải `KILLED` (`KS:189-191`). Một iteration của loop được đổi thông qua root của nó.
  - **No-op.** State đích bằng state hiện tại là 409. Kestra ghi một cảnh báo và trả về không đổi (`KS:73-77`); resume của egret đã trả 409 cho một lời gọi không thay đổi gì (`routes.rs:3125`).
  - **Task-run state** theo `markAs` (`ES:603-662`):
    1. Run nhận state đích và state của attempt cuối cùng của nó bị ghi đè, không có attempt mới.
    2. Các tổ tiên của nó chuyển `RUNNING` và các run end-hook bị làm rỗng (D4).
    3. Execution nhận label `system.restarted` và `attempt_number + 1`.
    4. Sau đó execution chuyển `RESTARTED` và executor chạy tiếp.
    5. Đích `CANCELLED` làm execution `CANCELLED` ngay lập tức.
  - **Execution status** nối thêm một state vào history và không đụng đến run nào. Một parent mà run `Subflow` đã terminal thì bỏ qua state mới của child (`subflow.rs:731-737`), như Kestra trên thực tế (`ChangeStateTestCase:43-68`, mà FIXME của nó ghi lại điều này).
- **D12. Một migration**, tạm thời là **0220** (main đang ở 0219 và không PR mở nào thêm migration; kiểm tra lại trước khi push, theo quy tắc namespace dùng chung). Nó nới lỏng CHECK của 0139 để `attempt_number > 1` được phép trên mọi execution, giữ `origin_offset_ms <> 0` chỉ cho dạng replay.
- **D13. Route, permission và audit.** Đây là các path kiểu egret, như kill và resume, không phải các path `/actions/` của Kestra:
  - `POST /api/ws/{ws}/workflows/executions/{id}/restart`. Tham số query `revision` bị từ chối với 400, nêu tên 7b.
  - `POST /api/ws/{ws}/workflows/executions/{id}/task-runs/{task_run_id}/state`, body `{ "state": … }`.
  - `POST /api/ws/{ws}/workflows/executions/{id}/status`, body `{ "state": … }`.

  Cả ba:
  - yêu cầu `workflow:manage`;
  - đọc execution theo phạm vi trước, nên id ngoài workspace là 404;
  - kiểm tra lại state dưới lock của execution;
  - trả **200** với `ExecutionDto`;
  - phát các audit event `workflow_execution:restart`, `:set_task_run_state` và `:set_status`, mỗi cái kèm `from_state`.

  Kestra trả lời bất đồng bộ, qua một command queue (`EC:3016-3059`). egret trả lời đồng bộ, như resume và kill của nó.
- **D14. Frontend.** Trên trang execution (`frontend/src/routes/workflows/ExecutionDetail.tsx:358-524`, `frontend/src/components/workflows/executions/actions.ts`):
  - một nút **Restart**, hiện cho `FAILED` và `PAUSED`;
  - một dialog **Change status**, hiện cho execution đã terminal và không phải `KILLED`;
  - một mục **Change state** trên mỗi hàng task run của execution đã terminal và không phải `KILLED`, chỉ sửa attempt cuối cùng, như UI của Kestra.

  Mỗi action dùng một `ConfirmAction` với lý do cho nút bị vô hiệu, invalidate query của execution để polling tiếp tục, và ánh xạ các 409 có mã. Client được sinh lại bằng `npm run gen:api`, không bao giờ dùng orval trần.
- **D15 (2026-11-13, trong A1). `mark_as` dưới một `WorkingDirectory` bị từ chối** (`inside_working_directory`, 409). Một run bên trong block không thể bị ghi đè mà không phá bất biến một-thư-mục của block, và quy tắc 2 của D10 đã làm rỗng các block như vậy khi restart. `EW src/executor/restart.rs`; commit `e371889f`.
- **D16 (2026-11-13, trong A2). Migration 0220 cũng định nghĩa lại `workflow.execution_is_idle`** để bỏ qua các run có history rỗng. Các run end-hook bị làm rỗng được lưu là `CREATED` với history `[]`, và hàm cũ không bao giờ coi chúng là idle, nên chúng không bao giờ được dựng lại. `migrations/0220_workflow_restart_attempt_number.sql`; commit `50872d45`.
- **D17 (2026-11-13, trong A2). Các run bị reset và bị làm rỗng được xoá `claimed_at`.** Một lease cũ trên run bị reset làm một `Parallel` đang chờ nó bị đứng. `EW src/worker/restart.rs`; commit `50872d45`.
- **D18 (2026-11-13, trong A5). `set_execution_status` từ chối iteration của loop và `afterExecution:` đang chạy, không từ chối file đã bị dọn.** State của một iteration thuộc về Loop của nó, và `afterExecution:` sở hữu state cuối cùng khi nó chạy; đổi status chỉ nối thêm một state và không chạy lại gì, nên file bị dọn không quan trọng. `EW src/worker/restart.rs`; commit `cffe6172`.
- **D19 (2026-11-13, trong A3). Một iteration của Loop kết thúc sau khi run Loop của nó đã terminal vẫn được ghi nhận** (entry outputs và bộ đếm), mà không làm Loop di chuyển. Nếu không, một Loop được restart xong sẽ có danh sách thiếu iteration đó và bộ đếm không khớp với các row. `handleTerminated` của Kestra cũng ghi nhận nó. `EW src/worker/spawn.rs`; commit `10124879`.
- **D20 (2026-11-13, trong A4). Một child `Subflow` được restart thông qua một walk job, không có settle lồng nhau.** `claim::walk` cấm settle lồng nhau, nên commit mở `claim::Job::Restart` và walk thực hiện restart của child. Việc tra cứu chỉ áp dụng cho một run đã bị reset (terminal, rồi `CREATED`) và mang label `system.restarted`; nếu không có cổng này, một iteration `LoopUntil` mới trong một execution được restart sẽ dùng lại child của iteration trước. Một row child không load được thì nhận child mới và một `warn`. `EW src/worker/claim.rs`, `EW src/worker/restart.rs`; commit `1ed811fa`, `2c511ad9`.
- **D21 (2026-11-13, trong B1). Lỗi body của task-run state và execution status trả 400, không bao giờ 422.** `axum::Json` trơn trả 422 với body sai; các handler nhận `JsonRejection` và ánh xạ nó sang `InvalidInput`, theo mẫu của `triggers/routes.rs::body_refusal`. `EW src/routes.rs`; commit `6e094226`.
- **D22 (2026-11-13, trong B1). Các run bị làm rỗng bị ẩn khỏi chi tiết execution và graph read model.** Chúng được lưu là `CREATED` với history rỗng, còn Kestra gỡ chúng. `ExecutionRepo::task_runs` và `GRAPH_RUNS_SQL` lọc `history <> '[]'`; các lần đọc phía worker không bị đụng đến. `EW src/repo.rs`, `EW src/routes.rs`; commit `d686973a`.
- **D23 (2026-11-13, trong A1). Một `Pause` mà history đã chứa `PAUSED` thì đi tiếp** (`needPause` của Kestra). `pause::drive` của egret không có kiểm tra này và sẽ pause lại, nên một Pause được restart được đặt `SUCCESS` dưới dạng `SetRunState`. Nó không thể kích hoạt ngoài một restart: resume kết thúc run thẳng từ `PAUSED`. Điều này thay đổi `advance`; ratchet không dịch chuyển. `EW src/executor/flowable/pause.rs`; commit `878d09a8`.

## 3. Hình dạng của thay đổi

| Lớp | Thay đổi |
|---|---|
| `executor` (thuần) | `restart.rs` mới:<br>• `restart(plan, &state) -> Result<Vec<Command>, NothingToRestart>`<br>• `mark_as(plan, &state, run, target) -> Vec<Command>`<br>• một "reset set" chung (D2), "end-hook set" (D4) và ánh xạ state (D3)<br>• một `Command::EmptyRun(RunIx)` mới (hoặc `RestartLoop` được tổng quát hoá) và `Command::OpenFlowableAttempt` (D6)<br>• unit test trên các state dựng tay, không DB |
| `worker` | `restart_execution`, `set_task_run_state`, `set_execution_status`:<br>• `transaction!`, rồi `load` (lock), kiểm tra lại, apply, `settle`, như `resume_by_hand`<br>• Loop (D8) và Subflow (D9) tại chỗ<br>• `outputs`/`message`/`secret_outputs` bị xoá trên các run bị reset và bị làm rỗng<br>• `attempt_number`, label |
| `routes` | Ba handler (D13), OpenAPI, audit; các 409 có mã (`nothing_to_restart`, `after_execution_running`, `files_pruned`, `not_restartable`, `state_unchanged`) |
| migration | 0220 (D12) |
| frontend | D14; `npm run gen:api` |
| docs | `egret-data/docs` Platform → Workflows → trang executions; ảnh chụp màn hình của ba action |

## 4. Test

Oracle của executor chạy một corpus các flow, không phải các thao tác, nên ratchet **không đổi** (301 / 25 / 25 / 0) và gói này không ghi nhận thêm gì (roadmap: "0 (operability)"). Mỗi kịch bản test của Kestra trở thành một test của egret:

| Test của Kestra | Test của egret (`EW tests/`) |
|---|---|
| `ExecutionServiceTest.restartSimple` (:73-91) | cùng id, `RESTARTED` trong history, run lỗi được reset, label được đặt |
| `RestartCaseTest.restartFailedThenSuccess` (:38-71) | run 3 → 4, run lỗi kết thúc với 2 attempt, tất cả `SUCCESS` |
| `…WithGlobalErrors` / `…WithLocalErrors` (:73-137) | run errors bị làm rỗng, chạy lại trên lỗi mới |
| `restartFailedWithFinally` / `…WithAfterExecution` (:397-461) | finally / afterExecution bị làm rỗng và chạy lại |
| `restartLoop` (:224-311) | ba lần restart, mỗi lần tiếp tục từ iteration lỗi; cùng run id của Loop |
| `restartSubflow` / `restartSubflowWithLoop` (:339-395) | child lỗi được restart, không bao giờ bị thay; đúng N child |
| `restartMultiple` (:313-337) | restart lặp lại của một execution |
| `restartOrReplayLoopUntil` (:463-505) | LoopUntil chờ lại từ lúc restart (D6) |
| `changeStateWithErrorBranch` (ES test :544-560) | mark SUCCESS làm rỗng nhánh error; `attempt_number` 2 |
| `ChangeStateTestCase` (:28-40) | task đầu FAILED được mark SUCCESS → execution SUCCESS, 2 run |
| `ExecutionControllerRunnerTest` (:665-684, :1750-1915, :3303-3342) | status code của route: 409 khi restart SUCCESS/KILLED, 409 khi đổi KILLED, các đường 200 |

Các test chỉ có ở egret:
- `Parallel` với các sibling bị fail bởi rule 1;
- `WorkingDirectory` chạy lại trong một thư mục mới;
- một container `delete: false` không được gắn lại (D5);
- các từ chối 409 của D10 và D11;
- CHECK 0220 theo cả hai chiều;
- flow-trigger event cho mỗi state được nối thêm;
- một id ngoài workspace là 404;
- logic nút ở frontend (vitest);
- kiểm tra trực tiếp trên trình duyệt ở demo stack.

## 5. Các follow-up đã ghi lại

- Restart với `revision`, tức là một execution mới trên revision khác: 7b.
- `restartBehavior: NEW_EXECUTION` và bản thân property: dòng #12.
- Restart và change-status hàng loạt: 7f.
- Một `Pause` đạt được bằng đổi task-run state (đường resume của Kestra qua `markAs`): 7c sở hữu resume với input.

## 6. Các khác biệt đã ghi nhận

1. State đích bị giới hạn ở bốn state mà UI của Kestra đưa ra (D11).
2. Đổi state no-op là 409, không phải 200 im lặng (D11).
3. Restart bị từ chối khi `afterExecution:` đang chạy, và sau khi pruner đã xoá file (D10).
4. `outputs` của một run bị reset được xoá khi restart. Kestra giữ chúng, khoá theo task-run id, cho đến khi run ghi lại.
5. Các thao tác trả lời đồng bộ; Kestra dùng command queue (D13).
6. Route dùng path của egret, không phải `/actions/…` (D13).
7. Một `Subflow` bên trong `LoopUntil` chỉ được khớp với child trước đó theo `run_ix`, nên một iteration được restart sẽ spawn child mới. Row child không mang iteration id; sửa cần một migration (follow-up, dòng roadmap #12). Ghi nhận 2026-11-13, A4.
8. Một `Subflow` được restart mà child vẫn đang chạy thì chờ nó. Kestra kết thúc run `SUCCESS`. Ghi nhận 2026-11-13, A4.
9. Xoá `claimed_at` (D17) cho phép một sibling đã fail theo rule 1 chạy hai lần: handler cũ bị chặn lúc commit, nhưng side effect của nó có thể lặp lại. Kestra cư xử giống vậy. Ghi nhận 2026-11-13, A2.
10. Mark một run `CANCELLED`, hoặc để một run vẫn `PAUSED`, để lại các tổ tiên được mở lại ở `RUNNING` dưới một execution terminal. Điều này trung thành với Kestra và là một vấn đề đã biết; nó chưa được test ở mức worker. Ghi nhận 2026-11-13, A1 và A5.

## 7. Như đã xây

Xây 2026-11-13 trên `feat/workflow-execution-ops`, base `origin/main` `9996fde4`. Mọi commit bên dưới đều nằm trên nhánh đó. Migration `0220_workflow_restart_attempt_number.sql` (số được kiểm tra lại với main và các PR mở trước khi push). Ratchet của executor là **301 / 25 / 25 / 0** và census **295 / 25 / 21**, đo tại T0 và không đổi ở mỗi task.

| Task | Commit | Đã vào những gì | Test thêm | Vòng sửa |
|---|---|---|---|---|
| Spec, plan | `5c440e09`, `a3bb2ee7` | Spec này và plan của nó. | – | – |
| T0 | – | Container, ratchet và census nền. | – | – |
| A1 | `a3bb2ee7..e371889f` (`878d09a8`, `e371889f`) | Các command thuần `executor::restart` (restart và mark-as); Pause `needPause` (D23); `SetRunState{Created}` xoá `working_dir`. | `executor::restart::tests` trong `src/executor/restart.rs`, 20 | 1: iteration `LoopUntil` được restart không bị đếm lại; `mark_as` dưới một `WorkingDirectory` bị từ chối (D15) |
| A2 | `e371889f..50872d45` (`50872d45`) | `restart_execution` tại chỗ: reset, run end-hook bị làm rỗng với id mới, label, `attempt_number` + 1, side write, đánh giá lại SLA; migration 0220 (nới lỏng CHECK 0139, D12; định nghĩa lại `execution_is_idle`, D16). | `tests/restart.rs` 20, `tests/migration_0220_restart_attempt.rs` 5; test replay-metadata của `tests/worker.rs` được cập nhật | 0 |
| A3 | `50872d45..10124879` (`e5f24b50`, `10124879`) | Restart Loop từ iteration lỗi; bộ đếm được tính lại theo từng index cho các iteration được restart. | `tests/restart.rs` +4 (lên 24 sau khi sửa) | 1: iteration đến muộn được ghi nhận (D19) |
| A4 | `10124879..2c511ad9` (`1ed811fa`, `2c511ad9`) | Child Subflow được restart tại chỗ qua một walk job (D20). | `tests/restart.rs` +7 (lên 31) | 0 (một sửa trước review: child không load được thì spawn child mới) |
| A5 | `2c511ad9..cffe6172` (`cffe6172`) | `set_task_run_state` (mark-as) và `set_execution_status` (D11, D18). | `tests/mark_as.rs` 17 | 0 |
| B1 | `cffe6172..6e094226` (`d686973a`, `6e094226`) | Các route `restart_execution`, `set_task_run_state`, `set_execution_status` trong `EW src/routes.rs`; các từ chối được ánh xạ sang 409 có mã; run bị làm rỗng bị ẩn (D22); OpenAPI và audit. | `tests/restart_routes.rs` 21 | 1: 400, không phải 422 (D21) |
| C1 | `2f44ed20..a28207d9` (`2f44ed20` sinh lại client, `ddbd58ca`, `a28207d9`) | Các action trên trang execution: dialog restart, change status, task-run state; hook dùng chung `useExecutionAction.ts`. | `actions.ops.test.ts` 9; `ExecutionDetail.test.tsx` 9 mới (66 trong file) | 1: hook và helper dùng chung |

Đo ở cuối mỗi task, `cargo test -p egret-workflow`: `--test restart` 31, `--test mark_as` 17, `--test worker` 169, `--test kill_route` 29, `--test end_hooks` 20, `--test executor` 62; ở B1 `--test restart_routes` 21, `--test executions_routes` 42, `--test routes` 47.

Cũng từ quá trình xây: dưới `transmitFailed: true`, một sibling kết thúc `SUCCESS` sau lỗi đầu tiên nay được đếm và liệt kê (khớp `handleTerminated` của Kestra); bộ đếm của một iteration được restart được tính lại từ các row, nên một iteration trước đó mà giới hạn 16 MiB đọc là `FAILED` được đếm lại là `SUCCESS`. Kiểm tra trình duyệt cho frontend (FE-TEST-2, sáng và tối) còn nợ ở Z.

Các cổng ở Z (2026-11-13, trên `a28207d9`, commit code cuối cùng): `cargo fmt --check` sạch; `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings` sạch; `cargo check -p egret-bin` sạch; `cargo test -p egret-workflow --no-fail-fast` 3684 passed, 55 ignored, 7 failed, cả bảy trong `sql_starrocks_live` (không có engine StarRocks trên máy test); `cargo test -p egret-config -p egret-server -p egret-metastore` 179 passed, gồm `committed_openapi_json_is_current`. Ratchet 301/25/25/0 và census 295/25/21, không đổi so với T0.

### Phát hiện hoãn lại

Mọi dòng `minor (deferred)` của sổ cái controller, cho review cuối.

**A1**
- Guard "tổ tiên đã `RUNNING`" (`restart.rs`) chưa có test (Pause bên trong Sequential).
- Test 14 (`mark_as` `CANCELLED`) không có vòng advance cho thấy `b` không được tạo.
- `mark_as` chấp nhận một run có history rỗng (chuyển sang A5, nơi từ chối nó).
- `mark_as` `CANCELLED` để lại các tổ tiên được mở lại ở `RUNNING`; thứ tự `CANCELLED`-trước-`PAUSED` khác Kestra `ES:656-661` (nay là §6 mục 10).
- `matches!` của `WorkingDirectory` bị trùng trong `restart.rs`; `pub mod restart` so với `pub(crate)`.

**A2**
- Xoá `claimed_at` cho phép một sibling đã fail theo rule 1 chạy hai lần (nay là §6 mục 9).
- `a_loop_restart_is_not_built_yet` và `a_subflow_restart_is_not_built_yet` chỉ assert `exec_meta` (A3 và A4 đã thay chúng).
- `side_writes` chuyển đổi `i32` hai lần; `UPDATE` id cho từng run có thể gộp thành một câu lệnh.
- Cách đặt tên `Row0220`.
- Test SLA cố định phần đuôi history là `RESTARTED CANCELLED` (không có `RUNNING`): SLA bắn trong settle của chính restart. Xác nhận đây là chủ ý (review cuối).
- `working_dir_snapshot` giữ URI cũ cho đến lần prepare mới; chưa được ghi chú trong code.

**A3**
- Một iteration kết thúc lại mà không sinh entry nào để lại entry cũ (đã gợi ý dùng một null marker).
- Một iteration mà giới hạn đọc là `FAILED` được đếm lại là `SUCCESS`; dưới `transmitFailed: true` một lỗi do giới hạn trả "No restartable…"; chưa test.
- Không có test cho nhánh "restart riêng của iteration bị từ chối", cũng không cho đường thay frame SECRET.
- Dưới `transmitFailed: true` một iteration fail lại không được đếm lại (chỉ về hình thức); danh sách state terminal bị trùng trong `terminated_by_state`; comment deadlock ở bước prepare iteration; dòng log iteration bị từ chối nên là `warn`, không phải `info`.
- Không có test cho iteration đến muộn của Loop đã terminal với over-cap, SECRET hoặc `transmitFailed`.

**A4**
- `RestartCommands.subflows` không còn được đọc trong production (cổng history đã thay nó): dùng hoặc ghi chú.
- State của child được so khớp dưới dạng chuỗi, và một state không biết rơi vào `_ if wait` và chờ mãi: parse sang `RunState` và coi state không biết là lỗi nội dung (review cuối).
- Chưa test: hoàn tất `RESTARTED` với `wait: false`, một child `PAUSED`, một child `KILLED` hoặc `WARNING` được sao chép thành `SUCCESS`, một lần chờ vẫn đang chạy.
- Một restart child bị từ chối chỉ ghi `tracing::warn`, không có dòng `task_logs` trên run.
- Test deadlock mô phỏng settle của child bằng lock thô.
- Một iteration Loop không load được làm lời gọi restart fail (không bị đứng; đường Subflow thì spawn).

**A5**
- `mark_as` với một run vẫn `PAUSED` giữ state terminal; chỉ có unit test (A1), không có worker test.
- Mark `CANCELLED` với một run lồng nhau hoặc vẫn `PAUSED` để lại các tổ tiên được mở lại ở `RUNNING` dưới một execution terminal (trung thành với Kestra; nay là §6 mục 10; chưa test ở mức worker).
- Đường `MarkAs` trong `open` không assert gì về cách nó kết thúc (đã gợi ý một kiểm tra dương).
- Vòng lặp retry deadlock được viết ba lần (`restart`, `set_task_run_state`, `set_execution_status`); `set_execution_status` viết lại hai predicate của D10 inline (review cuối).
- Thứ tự từ chối khác nhau: `mark_as` kiểm tra D10 trước `NotChangeable`, còn đổi status kiểm tra `NotChangeable` trước `afterExecution`.
- Việc tra cứu run giả định `run_ix` bằng chỉ số trong `state.runs`.

**B1**
- Doc comment bị rối ở `repo.rs:29-31`; một assertion có điều kiện (`if let Some`) trong `restart_routes.rs`; rác test gần ba chỗ.
- Test cổng entitlement chỉ có cho restart.
- Các test 409 cho `files_pruned`, `after_execution_running` và `inside_working_directory` không assert audit rỗng hay state không đổi.
- Không có test loop-iteration cho route task-run-state; assertion về graph chỉ là phủ định.
- `from_state` được đọc trước lock (race: audit cũ).
- Từ chối do kích thước body nay là 400, không phải 413 (không đáng kể).

**C1**
- Tính khác biệt của câu từ chối chỉ được test cho các mã restart; không có test polling-tiếp-tục.
- `stateWord` bị trùng ba lần; thụt lề của fragment Overview; một ép kiểu `as TaskRunDto`; tiêu đề dialog đổi trong lúc animation thoát; dialog có thể bị đóng giữa chừng request.
- Kiểm tra trình duyệt FE-TEST-2, sáng và tối, còn nợ (live check ở Z).
- `listReads` đếm theo tiền tố path (cả query children); helper trong `components` import `routes/workflows/queryKeys`.

## 8. Phần còn lại của dòng #7

Xem §2.1. Mỗi gói con có spec, plan và PR riêng.
