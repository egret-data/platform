# Workflow package #7b "Replay từ một task, trên nguồn được chọn, với input mới": kế hoạch triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-14-workflow-replay-from-task.md). Mọi giá trị chính xác, tên hàm/lớp, đường dẫn file và chi tiết test nằm ở bản tiếng Anh; khi hai bản khác nhau thì bản tiếng Anh đúng.

> **Dành cho agentic worker:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để thực hiện plan này từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** một người có `workflow:manage` có thể replay một execution đã kết thúc từ một task run được chọn, trên source đã đóng băng của nó, bản nháp hiện tại của flow hoặc một version đã publish, với input cũ hoặc input mới; và có thể restart một execution lỗi trên source khác dưới dạng một execution mới. Ngữ nghĩa là của Kestra (`ES:281-517`, `EC:1437-1507`) trừ những chỗ §"Spec corrections" và §5 của spec ghi nhận là khác biệt.

**Kiến trúc:** các task run của execution cũ được sao chép vào một execution mới, "copy then reset" của Kestra (spec D1).
- Hai hàm thuần trong một module mới `executor::replay` chọn các run cần giữ và các run cần reset: `replay_from` (D2) và `restart_copy` (D3, dùng lại `executor::restart` của 7a).
- Hàm thuần thứ ba, `copy_onto`, đánh số lại dày đặc các run được giữ và ánh xạ từng run vào plan của source đích theo task id (D5).
- Hàm thứ tư, `reset_copied`, reset các run đã sao chép bằng các quy tắc theo từng kind của 7a.
- Worker (`worker::copy`) chèn dòng execution mới qua bộ ghi dòng của chính start, được tách ra từ `start_execution_in_on`. Sau đó nó chèn các dòng đã sao chép, mang theo output và xoá lease, rồi chạy kết quả như `Job::Restart` của 7a, nên các side write và settle là của chính 7a.
- Replay cả execution giữ đường start hiện nay và thêm label cùng `attempt_number` (D10).
- Các route có thêm `taskRunId`, `source` / `revision`, `breakpoints` (bị từ chối) và một body JSON hoặc multipart tuỳ chọn.
- Quy tắc đọc storage có thêm các iteration loop của chuỗi replay (§S5).
- Frontend có một dialog cho Replay, Replay from this task và Restart.

**Tech stack:** Rust (axum, sqlx 0.9, Postgres, utoipa), `crates/egret-workflow`; React + client orval trong `frontend/`.

Spec: `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`. Q1–Q3 và D1–D14 của nó có tính ràng buộc. Chỗ nào plan này thấy một giả định của spec là sai, nó nói rõ trong "Spec corrections" và **không** âm thầm đổi quyết định; các mục S1, S2, S4 và S7 là các điểm khác biệt hoặc bổ sung mà controller phải trình người dùng trước khi B1 merge. Nhánh `feat/workflow-replay-from-task`, một PR. Base `origin/main` `844ec47db`.

Các task: T0, A1–A5 (engine), B1 (routes), C1 (frontend), Z (docs, gate, kiểm tra trực tiếp).

**Review không phải là một task của plan này.** Controller review từng task giữa các task và chạy một lần review toàn nhánh sau Z. Các bản sửa được ghi trong §7 "Như đã xây" của spec (sẽ tạo ở Z, như của 7a) dưới dạng các đợt sửa.

---

## Spec corrections

Mỗi mục là một chỗ mà giả định của spec sai hoặc chưa đủ, kèm bằng chứng. Plan thực hiện dòng được đánh dấu **Plan**; người dùng đã xác nhận S1, S2, S4 và S7 ngày 2026-11-14, mỗi mục là dòng được đánh dấu **Plan**.

- **S1 (D2: successor của Kestra gồm những gì).**
  - `GraphUtils.successors` của Kestra (`GraphUtils.java:144-181`) đi trên đồ thị flow từ node của run được chọn. Nó **luôn luôn** chạm tới các run `finally:` và `afterExecution:` của mọi scope mà nó đi ngược qua: node cuối của một list nối tới node finally của cluster (`GraphUtils.java:488-489`), chuỗi finally nối tới node afterExecution, rồi tới end (`:413-421`, `:365-399`).
  - Nó chạm tới các run `errors:` **chỉ qua root của cluster**, vì một nhánh lỗi treo dưới root của cluster đó (`root → e1`, `:468-472`). Từ commit `7a48d1dca` (Kestra #6959, `:157-169`), việc đi cũng bắt đầu từ root có cạnh nối thẳng tới node được chọn. Điều đó xảy ra trong ba trường hợp:
    - run được chọn là task top-level **đầu tiên** của flow (đồ thị trên cùng không có node task, nên `root → t1`, `:410`);
    - run được chọn là một **flowable**, mà root của chính nó nối tới node task của nó (`GraphCluster.java:74`), nên các errors của chính nó bị bỏ;
    - run được chọn là **task đầu tiên của một list `errors:`**.
  - Ở mọi trường hợp khác Kestra **giữ** các run errors của các scope mà run được chọn nằm trong. Trong executor của Kestra, một run errors được giữ đưa list vào chế độ lỗi (`Execution.java:595-606`: `errorsFlow` không rỗng, không có task lỗi, nên nó không trả về gì), và task được chọn không bao giờ chạy lại. Việc đi của egret cũng làm như vậy (`executor/flowable/mod.rs:215-260`, `errors_started`). Đây là "hồi quy của ghi chú roadmap 7" mà test 2 của 7a A1 canh giữ.
  - **Plan:** port trung thành việc đi successor của Kestra (A1 `successors`). Thêm một điểm khác biệt: khi run được chọn, hoặc một tổ tiên mà việc đi ngược qua, nằm trong **`tasks`** của scope P (bất kỳ nhánh nào), các run `errors:` của P, cùng cây con của chúng, cũng bị bỏ. Chính chú thích #6959 của Kestra (`GraphUtils.java:161-163`) nêu ý định này. Kết quả chỉ khác Kestra ở chỗ kết quả của Kestra không thể chạy. Được ghi là điểm khác biệt số 6 ở §5 của spec. Nếu người dùng bác bỏ, xoá `scope_errors` trong A1 và lật test A1.6.
- **S2 (D1, D3: output khi restart trên source khác).**
  - Restart với revision của Kestra **không** sao chép output. `ES:316-331` không có lời gọi `taskOutputService.copyOutputs` (chỉ replay mới có, `ES:425`). `TaskRun.forChildExecution` (`TaskRun.java:193-208`) không mang output, mà từ Kestra 2.0 nằm trong output repository (`TaskOutputService.java:288-294`, `TaskRun.java:80-86` đã deprecated).
  - Vì vậy trong Kestra, output của một task được giữ biến mất trong bản restart, và một task sau đọc chúng sẽ lỗi.
  - **Plan:** mang output cho cả hai, như spec D1 ("dùng chung cơ chế sao chép của 7b") nói. Được ghi là điểm khác biệt số 7 ở §5 của spec.
- **S3 (D5: "các run bị reset hoặc bị bỏ không cần khớp").** Điều này chỉ đúng với các run bị bỏ.
  - Trong egret mỗi dòng được sao chép lưu một `task_ix` của plan mà nó chạy dưới (`workflow.task_runs.task_ix`; `claim.rs:4826` nạp nó thành `TaskRun::task`). Một run bị reset (run được chọn, các tổ tiên của nó, một sibling chưa kết thúc) cũng là một dòng được sao chép.
  - Liên kết parent của run cũng phải khớp với plan đích: việc đi tìm một run theo `(parent run, task)` (`flowable::current_list`, `run_of`).
  - **Plan:** mọi run được sao chép phải khớp một task của source được chọn có cùng id, cùng parent task, cùng list (`tasks` / `errors` / `finally` / `afterExecution`) và cùng kind. Nếu không, request bị từ chối bằng 400 `replay_task_not_in_source`, và message nêu task id và nói rõ trong ba điều đó điều nào sai. Cùng code, kiểm tra rộng hơn; không quyết định nào đổi.
- **S4 (D9: một run `Loop` trong bản sao).**
  - 7a reset một run `Loop` bằng cách mở lại nó và restart iteration lỗi cuối **tại chỗ** (`worker/restart.rs:376-443`, 7a D8). Trong một bản sao, các iteration thuộc về execution cũ (`parent_execution_id` = id cũ), nên không có gì để restart trong execution mới.
  - Kestra làm `Loop` được sao chép lỗi ngay. Nó tìm các sub-execution của execution mới (`ES:985-991`), không thấy, và đánh dấu loop `FAILED` (`ExecutorService.java:735-757`). Vậy trong Kestra, replay từ một run `Loop` lỗi ngay lập tức, và câu "replay của root từ run `Loop` cho cùng kết quả" của D9 không đúng ở đó.
  - **Plan:** trong một bản sao, một run `Loop` bị reset chuyển sang `CREATED` và sinh mọi iteration dưới execution mới (A2 `reset_copied`). Đây là điều làm cho câu của D9 đúng. Được ghi là điểm khác biệt số 8 ở §5 của spec. Điều tương tự áp dụng cho restart trên source khác (D3).
- **S5 (D4: độ sâu của quy tắc đọc).**
  - Quy tắc nằm ở `storage/index.rs:114-157` (`readable_sql!`). CTE `chain` của nó là `WITH RECURSIVE` trên `replayed_from_execution_id` (và `parent_execution_id` của một retry replay). Nó có tính bắc cầu, bị chặn bởi độ dài chuỗi (`UNION` khử trùng).
  - Vậy một replay của một replay **đã** đọc được output lùi hai hay nhiều execution. Không cần sửa độ sâu; test 9 của A4 ghim điều đó.
  - Có một lỗ hổng: các iteration loop của chính chuỗi bị loại trừ (`index.rs:170`, "The chain's own loop iterations are NOT in the set"). Output được sao chép của một run `Loop` được giữ có thể nêu tên file dưới các iteration của execution cũ, và chúng sẽ không đọc được từ bản sao.
  - **Plan:** A5 thêm họ loop của mọi thành viên trong chuỗi, test đỏ trước. Nếu test đỏ đã xanh trước khi đổi, A5 dừng lại và báo cáo.
- **S6 (D6: "current được đọc trong transaction của replay").**
  - Một replay multipart upload file của nó trước khi mở bất kỳ transaction nào (`routes.rs:2970-3073`, thứ tự của start). Để làm vậy nó phải biết các input khai báo, namespace và id của source được chọn. Vì thế route đọc text source một lần, trước transaction.
  - **Plan:** mọi thứ đánh giá source đó xảy ra bên trong transaction, trên chính text ấy: `sql_principal::check_start` (trong bộ ghi dòng của start, `claim.rs:4341`), kiểm tra input, dựng plan và bước khớp của D5. Gate không bị làm yếu.
- **S7 (D8: các gate cho một bản sao).**
  - Replay hiện nay không kiểm tra file nào và không kiểm tra `afterExecution:`. Một bản sao mang các URI `kestra://` của các run cũ và không được sao chép các run đang chạy.
  - **Plan:** replay từ một task và restart trên source khác cũng qua các từ chối D10 của 7a:
    - `files_pruned` (409), cho execution nguồn hoặc bất kỳ execution nào trong chuỗi replay của nó;
    - `after_execution_running` (409).

    Replay cả execution không đổi.
- **S8 (D11: field bị gỡ bỏ).** `RestartQuery.revision` là `Option<String>` và mọi giá trị đều trả `restart_revision_unsupported` (`routes.rs:3587`, `:3705-3713`, `:3788-3794`). Test của nó là `tests/restart_routes.rs:253`. Comment client sinh ra của nó ở `frontend/src/api/generated/model/workflowRestartExecutionParams.ts:13`. B1 gỡ cả ba.
- **S9 (D14: hình dạng của frontend).**
  - Trang execution **không có i18n**: mọi chuỗi đều là tiếng Anh viết thẳng (`routes/workflows/ExecutionDetail.tsx`, `ExecutionOverview.tsx`); `package.json` không có thư viện i18n. **Plan:** tiếng Anh viết thẳng, như các action của 7a.
  - **Không có menu task-run**: 7a thêm một nút ghost "Change state" trên mỗi dòng (`ExecutionOverview.tsx:415-421`). **Plan:** ô action của dòng trở thành một `DropdownMenu` (`components/ui/dropdown-menu.tsx`) có nhãn "Actions for {task}", với "Change state" và "Replay from this task".
- **S10 (D2: lịch sử của execution mới; sự thật về Kestra).**
  - Bản sao của Kestra mang lịch sử state của execution cũ và nối thêm `RESTARTED` (`Execution.childExecution`, `Execution.java:405-432`, với `execution.withState(RESTARTED).getState()`, `ES:494-496`; `ExecutionServiceTest.replaySimple` khẳng định 4 mục).
  - Dòng mới của egret sinh ra ở `CREATED` (`claim.rs:4374-4378`), rồi `RESTARTED`, rồi `RUNNING`.
  - **Plan:** giữ lịch sử của egret (timestamp và outbox của dòng là của một dòng mới). Được ghi là điểm khác biệt số 9 ở §5 của spec.
- **S11 (D2/D3: plan nào chọn các run; sự thật về Kestra).**
  - Restart của Kestra chọn các run cần reset theo flow của execution cũ (`ExecutionCommandMessageHandler.java:90-94`: `findByExecutionForRuntime`).
  - Replay của nó dựng đồ thị từ flow của revision **mới** (`ExecutionCommandMessageHandler.java:176-187`).
  - **Plan:** egret chọn cả hai trên plan mà các run cũ đã chạy. `RunIx`/`TaskIx` của chúng là chỉ mục vào đó, và bước khớp của S3 sau đó từ chối bất kỳ run được giữ nào đã bị di chuyển. Do đó một danh sách bị sắp xếp lại có thể bỏ một run mà Kestra sẽ giữ; được ghi cùng điểm khác biệt 6.

---

## Hồ sơ điều tra (những sự thật mà người triển khai không được tự suy ra lại)

### Kestra
- `ES` = `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `EC` = `webserver/.../ExecutionController.java`, Kestra `96893b58`.
- **Restart** `ES:281-373`.
  - `canBeRestarted`, nếu không thì `IllegalStateException`.
  - `newExecutionId` chỉ với một revision (`:289`).
  - Một execution không có task run nhận một child mới (`:293-309`).
  - `taskRunToRestart(canBeRestarted)` (`:311-314`), `mapTaskRunId(keep = revision == null)` (`:316`), `mapTaskRun` (`:318-331`).
  - Nó xoá: các con worker-task (`:335-337`), `allErrorsWithChildren` (`:339-342`), `allFinallyWithChildren` (`:344-347`), `afterExecution` (`:349-351`).
  - Sau đó `childExecution(.., RESTARTED)`, label `system.restarted`, `nextAttempt()`, `withFlowRevision`.
- **`taskRunToRestart`** `ES:375-390`: các run của predicate cộng các tổ tiên của chúng (`taskRunWithAncestors`, `ES:1258-1263`, `getAncestors` `ES:1202-1212`). Nó ném "No task found to restart execution from!" khi tập rỗng.
- **Replay** `ES:393-512`.
  - `findExecutionWithTaskRun` đi xuống các sub-execution của loop (`:393-401`, `ES:569-583`).
  - `GraphUtils.of(flow, execution)` (`:410`), tập reset (`:412-415`), id mới cho mọi run (`:417`).
  - `mapTaskRun` cộng `copyOutputs` cho mỗi run (`:419-433`).
  - Successor trừ run được chọn trừ tập reset bị xoá (`:435-454`).
  - Các run chưa kết thúc (không thuộc tập reset, không bị xoá) được ánh xạ lại với `onRunningResend` và bị reset (`:456-484`).
  - `removeWorkerTask` (`:486-488`).
  - State: `CREATED` cho replay cả execution, nếu không thì lịch sử cũ cộng `RESTARTED` (`:491-496`).
  - Label `system.replay` (`Label.java:29`) và `nextAttempt()` (`:497-501`).
  - `applyNewRevision` lấy `variables` của flow mới (`ES:514-517`).
- **`mapTaskRun`** `ES:1227-1256`.
  - Một leaf, `WorkingDirectory`, `LoopUntil` hoặc `Loop` nhận state mới (`RESTARTED`). Mọi flowable khác nhận `RUNNING`.
  - Chỉ các run trong tập reset đổi state; các run khác giữ state của mình (`forChildExecution(.., null)`).
- **`removeWorkerTask`** `ES:1179-1200`: với mỗi `WorkingDirectory` trong tập reset, các successor của nó trừ chính nó, tức các con của nó.
- **Flow của revision.** Restart: `executorContext.getFlow()`, flow riêng của execution (`ExecutionCommandMessageHandler.java:90-94`). Replay: flow của revision khi được cho (`:176-187`).
- **Input khi replay.** `readExecutionInputs(flow, current, inputs)` với flow của revision (`EC:1497`). Run phải nằm trong execution (đích), nếu không là `IllegalArgumentException` (`EC:1502-1507`).
- **Tra cứu restart của Subflow** chỉ xảy ra dưới label `system.restarted` (`ExecutableUtils.java:90-150`), nên một run `Subflow` được replay sinh một child mới.

### egret

- **Task run** (`\d workflow.task_runs`, migration 0134, 0136, 0137, 0141, 0183, 0193, 0194, 0197, 0214).
  - Định danh và vị trí:
    - `id` (uuid, mặc định `gen_random_uuid()`);
    - `execution_id`;
    - `run_ix` (`UNIQUE (execution_id, run_ix)`; phải bằng vị trí của run, `claim.rs:4812-4824`, nếu không là `Corrupt`);
    - `task_ix`, `task_id`, `type_id` (NULL trừ khi `TaskKind::Runnable`, `claim.rs:5546-5549`);
    - `parent_ix`, `value`, `rendered`, `rendered_run_if`.
  - State: `state`, `history` (jsonb), `attempts` (jsonb `[{history, started_at, ended_at}]`; tick là mili giây sau `created_at` của **execution**, `routes.rs:1665-1674`, parse ở `claim.rs:5707-5746`).
  - Thời gian: `not_before_ms` (tick), `created_seq`, `ended_seq` (đồng hồ logic của execution `executions.seq`, `claim.rs:4910-4985`), `started_at`, `ended_at` (timestamptz; CHECK `ended_only_when_terminal`, `ends_after_it_starts`).
  - Lease: `claimed_at` (NULL nghĩa là chưa claim; claim chỉ lấy NULL).
  - Kết quả: `outputs`, `message`, `secret_outputs` (được seal không có associated data, `secret_inputs.rs:315-326`, nên một bản sao giải mã được), `outputs_bytes`.
  - Pause: `resume_with` (CHECK ghép nó với `not_before_ms`).
  - Working directory: `working_dir`, `working_dir_snapshot`, `working_dir_swept_at`. Các snapshot `_wd/{block run id}/` được quét bởi `storage/wd_sweep.rs:168-174` cho các block đã kết thúc chưa đóng dấu.
- **Bộ ghi** (`persist`, `claim.rs:5446-5600`) INSERT một `run_ix` mới và UPDATE một cái đã đổi. `states_json` (`claim.rs:5621`) và `attempts_json` (`claim.rs:5630`) serialise. Nó không bao giờ đụng `outputs` hay `message`.
- **Nạp.** `load`/`load_with` (`claim.rs:4746-4878`) khoá dòng execution (`FOR NO KEY UPDATE`). `ExecutionState` là `{runs, history, metadata{attempt, origin}}` (`executor/state.rs:93-111`); `TaskRun` là `executor/state.rs:114-160`.
- **Run bị làm rỗng.** Một run có history rỗng "bị xoá khỏi `taskRunList`" (`command.rs:224-240`). `current_list` đọc nó là chưa bắt đầu (`flowable/mod.rs:225-229`).
- **Start.**
  - `NewExecution` là `claim.rs:4126-4191`. Nó được dựng tại 27 chỗ: 9 trong `src` (routes ×2, `scheduler/fire.rs`, `flow_trigger/process.rs`, `claim.rs` ×5) và 18 trong test. **Plan này không thêm field nào.**
  - `start_execution_in_with` (`claim.rs:4222-4228`) dẫn tới `start_execution_in_on` (`claim.rs:4297-4452`), làm:
    - parse, `build_plan`, kiểm tra served;
    - `sql_principal::check_start(conn, &flow, flow_id, source, started_by)` (`:4341`);
    - kiểm tra input, **bị bỏ qua khi `replayed_from_execution_id` được đặt** (`:4357-4363`);
    - `secret_inputs::split`, INSERT (`:4367-4400`; dòng sinh ra ở `CREATED`);
    - `check_file_inputs`, `index_input_files`;
    - `settle` trên một state rỗng ở `Tick(0)`.
  - `checked_inputs` (`claim.rs:4243-4249`) là `pub(crate)` và được re-export (`worker/mod.rs:117`).
- **Route replay hiện nay** là `routes.rs:3240-3411`:
  - 400 cho loop-iteration (`:3312-3320`);
  - flow NULL nghĩa là 404 (`:3339-3342`);
  - `startable_flow` (`:3343`; định nghĩa `:2431-2458`: 404 thùng rác, 400 bản nháp không parse được, 409 bị disable);
  - kết thúc, hoặc 409 (`:3348-3353`);
  - `secret_inputs::reveal` (`:3356-3359`);
  - `start_execution_in_with` với `replayed_from_execution_id: Some(id)`, label được sao chép, variables được sao chép, `started_by: Some(caller)` (`:3362-3392`);
  - audit `workflow_execution:replay` `{replayed_from, replacement}` (`:3393-3401`);
  - trả 201 (`:3402-3410`).

  Dòng router của nó là `routes.rs:4474-4477`. Một test khẳng định đúng các label hôm nay: `tests/executions_routes.rs:2100-2146` (`a_replay_copies_the_labels_and_not_the_backfill`); B1 cập nhật nó cho D10.
- **Route restart** là `routes.rs:3755-3816`.
  - `RestartQuery` ở `:3705-3713`.
  - `rerun_gate` (`routes.rs:2474-2504`) chạy `startable_flow` và `check_start` trên `detail.source`.
  - `ExecOpError` (`:3589-3703`) ánh xạ một từ chối có code thành 409 hoặc nếu không thì 400. B1 thêm 404.
  - Dòng router `:4482-4485`; đăng ký OpenAPI `:4524-4526`.
- **Các loại body của route start.**
  - JSON: `routes.rs:2642-2684` (giới hạn 2 MiB, một body không header được đọc như JSON, `start_inputs` ở `:2510-2524`).
  - Multipart: `start_multipart` `:2935-2997`, `read_parts_and_start` `:3000-3099`, `read_text_part` `:3103+`.
  - Route tắt giới hạn body mặc định (`routes.rs:4289-4295`).
  - Kiểu: `StartExecutionRequest` `:2353`, `StartExecutionMultipart` `:2539-2563`.
- **Restart của 7a.**
  - Phía thuần: `executor/restart.rs`.
    - `restart` (`:68-116`) và `mark_as` (`:141-217`);
    - các hàm private `kind` (`:223`), `ancestors` (`:241-254`), `end_hook_runs` (`:261-275`), `empty` (`:278`), `reset` (`:288-322`), `to_created` (`:324`), `reopen` (`:333-340`);
    - các helper test `drive`, `drive_from`, `driven`, `last`, `is_terminal`, `run`, `leaf`, `flow` (`:349-470`).
  - Phía worker: `worker/restart.rs`.
    - `restart_execution` (`:84-99`, retry deadlock), `Nested` (`:107-119`), `Prepared`/`Op` (`:131-148`);
    - `prepare` (`:205-246`), `lock` (`:259-290`, khoá dòng run `FOR UPDATE`), `d10_refusal` (`:323-338`);
    - `open` (`:362-479`): loop, apply, `side_writes`, `Frame::loaded(..).without_sla()`;
    - `side_writes` (`:485-582`): `attempt_number + 1`, label `system.restarted`, xoá output và lease trên `reset ∪ emptied` trừ `loops`, làm mới các id bị làm rỗng.
  - Job: `claim::Job::Restart` (`claim.rs:3265-3285`), chạy bởi `claim::run_job` (`claim.rs:3135`).
- **Source.**
  - Bản nháp: `workflow.flows.source` (`FlowRepo::get`, `repo.rs:518`).
  - Version đã publish: `workflow.flow_versions.snapshot->>'source'`. `publish.rs:352-356` ghi `{"source", "plan", "warnings"}`; `routes.rs:1376-1391` đọc nó; `sql_principal.rs:68` so sánh nó.
  - Đọc từ repo là `FlowRepo::get_version(ws, flow, version)` (`repo.rs:893-911`), có scope, `NotFound("flow version")`.
- **Gate `egret.sql`.** `sql_principal::check_start` (`sql_principal.rs:90-113`): với `started_by = Some(caller)`, chỉ creator của flow qua được trên một flow chạy `egret.sql`. Vì bản sao đi qua bộ ghi dòng của start với `started_by: Some(caller)` và text source **được chọn**, gate của D8 đúng cho `current` và `version:N`.
- **Quy tắc đọc storage** (S5). `readable_executions` là `storage/index.rs:172-183`; SQL là `readable_sql!` `:114-157`. Pruner bảo vệ chiều ngược lại cho mọi execution không idle (`prune.rs:25-38`, kiểm tra muộn `:615-627`).
- **Label.** Cột là `[{"key","value"}]`, đọc bởi `labels::own_labels`. Chỉ engine ghi `system.*`.
- **Ratchet và census của executor**, được 7a đo và không đổi đến cuối: 301/25/25/0 và 295/25/21 (§7 của spec 7a).
- **Migration.** Không cần:
  - `attempt_number > 1` trên mọi dòng của D10 được cho phép từ 0225;
  - bản sao chỉ ghi các cột đã có;
  - A5 đổi một câu query, không đổi schema.

  Migration cao nhất trên `origin/main` là `0225` (`git ls-tree --name-only origin/main migrations/ | tail -1`). Nếu một migration trở nên cần thiết, báo lại và chọn số của nó so với main và các PR đang mở trước khi push (memory: số migration là một namespace dùng chung).
- **Frontend.**
  - Actions: `components/workflows/executions/actions.ts` (`showsReplay`/`canReplay` `:52-73`, restart `:155-162`, `restartRefusal` `:185-199`).
  - `ChangeStateDialog.tsx`, `useExecutionAction.ts`.
  - `ExecutionDetail.tsx`: `ExecutionActions` quanh `:358-570`; `ConfirmAction` của Replay `:544-552`; `runReplay` `:430-436`.
  - `ExecutionOverview.tsx` `TaskRuns` `:361-435`.
  - Form input: `components/workflows/run/RunFlow.tsx` và `inputModel.ts` (`inputsFromSource`, `formFromInputs`, `requestFromForm`, `validateForm`, `serverErrorsByInput`) và `inputControls.tsx` (`InputField`).
  - Hook được sinh: `useWorkflowListFlowVersions`, `useWorkflowGetFlowVersion`, `useWorkflowGetFlow`.
  - `npm run gen:api` = `orval && node scripts/encode-path-params.mjs && node scripts/write-api-digest.mjs`; không bao giờ chạy orval trần.

---

## Ràng buộc chung

- Q1–Q3 và D1–D14 của spec có tính ràng buộc, chỉ được sửa bởi các Spec corrections ở trên (S1, S2, S4 và S7 được người dùng xác nhận 2026-11-14). Khi spec và code của egret mâu thuẫn theo cách không được liệt kê ở trên, **dừng lại và báo cáo**; không tự ứng biến. Source Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` ở `96893b58` (chỉ đọc).
- **Tìm code:** CodeGraph trước, `codegraph_explore` với `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Chỉ dùng grep cho chuỗi literal. `rg` chưa được cài.
- **Môi trường:**
  - `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target`
  - `export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test` (container `egret-execops-postgres`)
  - `export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465` (container `egret-execops-valkey`)
  - **Không bao giờ chạy hai lệnh cargo cùng lúc.** Chạy `df -h` trước một lần build đầy đủ; dọn `target/debug/incremental` thay vì bắt đầu một thư mục target thứ hai.
- **Cấm đụng:** không bao giờ đụng cổng 5432 hay các container `egret-dev-*` / `egret-demo-*`. Không bao giờ commit `.superpowers/` hay file nháp.
- **Commit:** Conventional Commits, mỗi message kết thúc bằng
  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
- **Subagent không có mạng:** không `git push`/`fetch`/`pull`, không `gh`, không `npm ci`. Controller chạy `npm ci` / `npm run gen:api` khi C1 cần.
- **File test:** mọi binary test `#[sqlx::test]` mới gọi `egret_test_db::assert_database_url_is_exported()` trong phần setup, **và** có
  ```rust
  #[test]
  fn database_url_is_exported_rather_than_read_from_dotenv() {
      egret_test_db::assert_database_url_is_exported();
  }
  ```
  CI bắt buộc điều này; 7a từng bỏ sót một lần. Các binary mới của plan này là `tests/replay.rs` và `tests/replay_routes.rs`.
- **Đỏ trước:** test cho thấy lỗ hổng tồn tại trước khi sửa.
- **Test theo từng task:** mỗi task chỉ chạy các test nó đụng tới. Các suite đầy đủ chạy một lần, ở Z.
- **Ratchet và census của executor không được xê dịch:** **301 / 25 / 25 / 0** và census **295 / 25 / 21**, đo lại ở T0. Mọi thay đổi là một hồi quy cần báo cáo, không phải một khoản để ghi sổ.
- Không thêm `std::env::var` mới. Nếu cần, thêm nó vào `ENV_OUTSIDE_CONFIG` của `egret-config` và chạy `cargo test -p egret-config`.
- **Từ ngữ trong code và message.**
  - Các code từ chối mới, chính xác:
    - `invalid_source` (400);
    - `replay_breakpoints_unsupported` (400);
    - `replay_task_not_in_source` (400);
    - `flow_version_not_found` (404);
    - `task_run_not_found` (404).
  - Dùng lại: `inside_working_directory`, `not_restartable`, `nothing_to_restart`, `after_execution_running`, `files_pruned`.
  - Bị gỡ: `restart_revision_unsupported`.
  - Label: `system.replay` / `"true"` (replay) và `system.restarted` / `"true"` (restart trên source khác).
  - Audit action: `workflow_execution:replay` và `workflow_execution:restart`.
  - Cách viết source: `frozen`, `current`, `version:N` (N ≥ 1).

---

## Cấu trúc file

| File | Thay đổi | Task |
|---|---|---|
| `crates/egret-workflow/src/executor/test_support.rs` | **mới** (`#[cfg(test)]`): các helper drive của 7a được chuyển ra khỏi `restart::tests` | A1 |
| `crates/egret-workflow/src/executor/replay.rs` | **mới**: `CopySelection`, `CopyRefusal`, `MatchFault`, `replay_from`, `restart_copy`, `successors`, `scope_errors`; rồi `CopiedRuns`, `copy_onto`, `shift_ticks`, `reset_copied`; test | A1, A2 |
| `crates/egret-workflow/src/executor/restart.rs` | `kind`, `ancestors`, `reset`, `to_created`, `is_seed` → `pub(super)`; test import `test_support` | A1 |
| `crates/egret-workflow/src/executor/mod.rs` | `pub mod replay;`, `#[cfg(test)] pub(crate) mod test_support;`, re-export | A1, A2 |
| `crates/egret-workflow/src/worker/claim.rs` | `insert_execution` tách khỏi `start_execution_in_on`; `attempts_json` → `pub(super)`; `settle_new` | A3 |
| `crates/egret-workflow/src/worker/restart.rs` | `Op::Replay`, `copy_job`, `with_label`, `REPLAY_LABEL_KEY`/`VALUE`, `side_writes` theo op | A3, A4 |
| `crates/egret-workflow/src/worker/copy.rs` | **mới**: `ReplayRequest`, `ReplayKind`, `ReplayInputs`, `ReplayRefused`, `replay_execution`, `insert_runs` | A3, A4 |
| `crates/egret-workflow/src/worker/mod.rs` | `mod copy;` và các re-export | A3 |
| `crates/egret-workflow/tests/replay.rs` | **mới** test integration tầng worker | A3, A4, A5 |
| `crates/egret-workflow/src/storage/index.rs` | `readable_sql!`: các họ loop của chuỗi | A5 |
| `crates/egret-workflow/src/repo.rs` | `ExecutionRepo::loop_run_holding` | B1 |
| `crates/egret-workflow/src/routes.rs` | `SourceChoice`, `ReplayQuery`, `RestartQuery` (field mới), nhánh 404 của `ExecOpError` và `from_replay_refusal`, `read_json_body`, `receive_parts`/`discard_uploads`, `replay_execution` và `restart_execution` được viết lại, `rerun_gate` nhận text source, OpenAPI, layer router | B1 |
| `crates/egret-workflow/tests/replay_routes.rs` | **mới** test route | B1 |
| `crates/egret-workflow/tests/restart_routes.rs`, `tests/executions_routes.rs` | test `revision` và test replay-labels được cập nhật | B1 |
| `frontend/src/api/generated/**` | `npm run gen:api` (controller) | C1 |
| `frontend/src/components/workflows/executions/actions.ts` (+ test) | `SourceChoice`, `sourceParam`, `replayRefusal`, bổ sung `restartRefusal`, `showsReplayFromTask` | C1 |
| `frontend/src/components/workflows/executions/ReplayDialog.tsx` (+ test) | **mới**: lựa chọn source, mục inputs, cả hai chế độ | C1 |
| `frontend/src/routes/workflows/ExecutionDetail.tsx`, `ExecutionOverview.tsx` (+ test) | nối dialog; menu của dòng | C1 |
| `docs/en/specs/2026-11-14-…-design.md` §5, §7; roadmap; `docs/vi/...` | docs | Z |

---

## T0. Môi trường và baseline

- [ ] Kiểm tra các container: `docker ps --format '{{.Names}} {{.Ports}}' | grep egret-execops` phải liệt kê `egret-execops-postgres` ở 5465 và `egret-execops-valkey` ở 6465. Cả hai đã chạy ngày 2026-11-14. Nếu thiếu cái nào, khởi động nó như T0 của 7a (image `postgres:16`, user/password/db `egret`/`egret`/`egret_test`, cổng host 5465; Valkey ở 6465).
- [ ] Export ba biến (Ràng buộc chung).
- [ ] Chạy ratchet và census: `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t0-executor.txt"`. Đọc bộ số từ file, không đọc từ `tail` (memory: `tail` cắt báo cáo).
- [ ] Đọc lại số migration: `git ls-tree --name-only origin/main migrations/ | tail -1` (kỳ vọng `0225_workflow_restart_attempt_number.sql`).
- **Chấp nhận:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; migration cao nhất 0225. Báo cáo cả ba như đã đo. Không commit.

---

## A. Engine

### A1. Phần chọn thuần: `replay_from` và `restart_copy` (spec D2, D3; S1, S11)

**File:**
- Tạo: `src/executor/test_support.rs`, `src/executor/replay.rs`.
- Sửa: `src/executor/restart.rs`, `src/executor/mod.rs`.

**Interface.**

Dùng:
- `executor::restart::{restart, RestartCommands, RestartRefusal}`;
- các helper private được đổi thành `pub(super)`:
  ```rust
  pub(super) fn kind<'a>(plan: &'a Plan, state: &ExecutionState, run: RunIx) -> Option<&'a TaskKind>;
  pub(super) fn ancestors(state: &ExecutionState, run: RunIx) -> Vec<RunIx>;
  pub(super) fn reset(plan: &Plan, state: &ExecutionState, run: RunIx, now: Tick, out: &mut RestartCommands);
  pub(super) fn to_created(run: RunIx, out: &mut RestartCommands);
  ```
- `state::is_descendant_of` (đã là `pub(super)`) và `Plan::children_of` (đã là `pub(super)`).

Tạo ra (`executor/replay.rs`, được re-export từ `executor`):
```rust
/// Why a copy cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    /// The chosen run is not a run of this execution (out of range, or emptied).
    NoSuchRun,
    /// `restart_copy`: the execution is not `FAILED` or `PAUSED` (7a's rule).
    NotRestartable(RunState),
    /// `restart_copy`: no run is `FAILED` or `PAUSED`.
    NothingToRestart,
    /// `replay_from`: the chosen run has a `WorkingDirectory` ancestor (D9).
    InsideWorkingDirectory,
    /// `copy_onto` (A2): a copied run's task does not fit the target source (D5, S3).
    TaskNotInSource { task_id: String, why: MatchFault },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchFault { Missing, Moved, OtherKind }

/// The runs a copy keeps, and the kept runs it resets, as indexes of the OLD state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopySelection {
    /// Ascending. Never an empty run, never a run whose parent is not kept.
    pub keep: Vec<RunIx>,
    /// A subset of `keep`.
    pub reset: std::collections::BTreeSet<RunIx>,
}

/// Spec D2 (Kestra `ES:393-512`, with S1's divergence).
pub fn replay_from(plan: &Plan, state: &ExecutionState, chosen: RunIx)
    -> Result<CopySelection, CopyRefusal>;

/// Spec D3 (Kestra `ES:311-351`): 7a's `restart` sets, as a selection.
pub fn restart_copy(plan: &Plan, state: &ExecutionState)
    -> Result<CopySelection, CopyRefusal>;
```

**Các bước.**

- [ ] **Chuyển các helper test.** Tạo `src/executor/test_support.rs` chứa, nguyên văn từ `restart.rs:349-470`: `T0`, `NOW`, `LOG`, `leaf`, `flow`, `drive`, `drive_from`, `driven`, `last`, `is_terminal`, `run`. Đặt mỗi cái là `pub(crate)`. Trong `executor/mod.rs` thêm `#[cfg(test)] pub(crate) mod test_support;`. Trong `restart::tests` thay các mục đã chuyển bằng `use crate::executor::test_support::*;`. Chạy `cargo test -p egret-workflow --lib executor::restart`: cùng số lượng như trước (20, §7 của 7a), tất cả xanh. Commit: `refactor(workflow): share the executor's restart test drive`.
- [ ] **Viết các test đỏ** trong `executor/replay.rs` `#[cfg(test)] mod tests` (code bên dưới). Chạy `cargo test -p egret-workflow --lib executor::replay`. Chúng không compile được; đó là bước đỏ.
- [ ] **Triển khai** (code bên dưới).
- [ ] Chạy `cargo test -p egret-workflow --lib executor::replay` (tất cả qua), `cargo test -p egret-workflow --lib executor::restart` (20 qua), và ratchet `cargo test -p egret-workflow --test executor` (không đổi).
- [ ] Commit: `feat(workflow): pure replay-from and restart-copy selections (package 7b)`.

**Triển khai** (`src/executor/replay.rs`):

```rust
//! Replay from a task run, and restart on another source, as copies --
//! workflow package 7b (spec `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`
//! D1-D5; Kestra `ExecutionService.replay`, `ES:393-512`, and `.restart` with
//! a revision, `ES:281-373`).
//!
//! Pure, like `executor::restart`: these functions say which of the old
//! execution's runs the new execution keeps and which of those it resets.
//! The worker copies the kept runs (`worker::copy`) and runs the reset as
//! 7a's restart job.

use std::collections::BTreeSet;

use super::plan::{Plan, TaskIx, TaskKind};
use super::restart::{self, RestartRefusal};
use super::state::{is_descendant_of, ExecutionState, RunIx, RunState};

// (CopyRefusal, MatchFault, CopySelection as in Interfaces)

pub fn replay_from(
    plan: &Plan,
    state: &ExecutionState,
    chosen: RunIx,
) -> Result<CopySelection, CopyRefusal> {
    let n = state.runs.len();
    if chosen >= n || state.runs[chosen].history.is_empty() {
        return Err(CopyRefusal::NoSuchRun);
    }
    let above = restart::ancestors(state, chosen);
    if above.iter().any(|&a| {
        matches!(restart::kind(plan, state, a), Some(TaskKind::WorkingDirectory { .. }))
    }) {
        return Err(CopyRefusal::InsideWorkingDirectory);
    }
    // Kestra `taskRunToRestart` (`ES:375-390`): the run and its ancestors.
    let mut reset: BTreeSet<RunIx> = above.into_iter().collect();
    reset.insert(chosen);
    // `ES:435-454`, plus S1's divergence.
    let mut dropped = successors(plan, state, chosen);
    dropped.extend(scope_errors(plan, state, chosen));
    dropped.retain(|r| !reset.contains(r));
    // `removeWorkerTask` (`ES:1179-1200`): a reset `WorkingDirectory` re-runs whole.
    for &w in &reset {
        if matches!(restart::kind(plan, state, w), Some(TaskKind::WorkingDirectory { .. })) {
            dropped.extend((0..n).filter(|&ix| is_descendant_of(state, ix, w)));
        }
    }
    let keep = kept(state, &dropped);
    // `ES:456-484`: a kept run that had not ended is reset too.
    for &ix in &keep {
        if !state.runs[ix].history.last().is_some_and(|s| s.is_terminal()) {
            reset.insert(ix);
        }
    }
    reset.retain(|r| keep.contains(r));
    Ok(CopySelection { keep, reset })
}

pub fn restart_copy(plan: &Plan, state: &ExecutionState) -> Result<CopySelection, CopyRefusal> {
    // 7a's sets, its tick irrelevant: only the sets are read.
    let cmds = restart::restart(plan, state, super::state::Tick(0)).map_err(|r| match r {
        RestartRefusal::NothingToRestart => CopyRefusal::NothingToRestart,
        RestartRefusal::NotRestartable(s) => CopyRefusal::NotRestartable(s),
        RestartRefusal::InsideWorkingDirectory => unreachable!("only mark_as returns it"),
    })?;
    let dropped: BTreeSet<RunIx> = cmds.emptied.iter().copied().collect();
    let keep = kept(state, &dropped);
    let reset = cmds
        .reset
        .iter()
        .chain(&cmds.loops)
        .copied()
        .filter(|r| keep.contains(r))
        .collect();
    Ok(CopySelection { keep, reset })
}

/// Every run neither dropped nor empty, whose parent is kept too (parents
/// precede children, so one ascending pass decides it).
fn kept(state: &ExecutionState, dropped: &BTreeSet<RunIx>) -> Vec<RunIx> {
    let mut keep: Vec<RunIx> = Vec::new();
    for ix in 0..state.runs.len() {
        let run = &state.runs[ix];
        let parent_kept = run.parent.is_none_or(|p| keep.binary_search(&p).is_ok());
        if !dropped.contains(&ix) && !run.history.is_empty() && parent_kept {
            keep.push(ix);
        }
    }
    keep
}

/// The list a run's task sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role { Tasks, Errors, Finally, AfterExecution }

/// How a list's members follow each other in Kestra's graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// A chain (`GraphUtils.fillGraph`, SEQUENTIAL): every list but these two.
    Chain,
    /// `Parallel`'s tasks: each from the task node, none after another.
    Parallel,
    /// A `Dag`'s nodes: by `dependsOn` (`GraphUtils.fillGraphDag`, `:523-633`).
    Dag,
}

struct Slot {
    parent: Option<RunIx>,
    role: Role,
    shape: Shape,
    list: Vec<TaskIx>,
}

/// Where `run`'s task sits under its parent run (or the flow).
fn slot(plan: &Plan, state: &ExecutionState, run: RunIx) -> Option<Slot> {
    let task = state.runs[run].task;
    let parent = state.runs[run].parent;
    let lists: Vec<(Role, Shape, Vec<TaskIx>)> = match parent {
        None => vec![
            (Role::Tasks, Shape::Chain, plan.roots.clone()),
            (Role::Errors, Shape::Chain, plan.errors.clone()),
            (Role::Finally, Shape::Chain, plan.finally.clone()),
            (Role::AfterExecution, Shape::Chain, plan.after_execution.clone()),
        ],
        Some(p) => {
            let pt = &plan.tasks[state.runs[p].task];
            let mut v = vec![
                (Role::Errors, Shape::Chain, pt.errors.clone()),
                (Role::Finally, Shape::Chain, pt.finally.clone()),
            ];
            match &pt.kind {
                TaskKind::Sequential { tasks }
                | TaskKind::AllowFailure { tasks }
                | TaskKind::WorkingDirectory { tasks }
                | TaskKind::LoopUntil { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Chain, tasks.clone()))
                }
                TaskKind::Parallel { tasks, .. } => {
                    v.push((Role::Tasks, Shape::Parallel, tasks.clone()))
                }
                TaskKind::If { then_, else_, .. } => {
                    v.push((Role::Tasks, Shape::Chain, then_.clone()));
                    v.push((Role::Tasks, Shape::Chain, else_.clone()));
                }
                TaskKind::Switch { cases, default, .. } => {
                    for (_, c) in cases {
                        v.push((Role::Tasks, Shape::Chain, c.clone()));
                    }
                    v.push((Role::Tasks, Shape::Chain, default.clone()));
                }
                TaskKind::Dag { nodes } => {
                    v.push((Role::Tasks, Shape::Dag, nodes.iter().map(|d| d.task).collect()))
                }
                _ => {}
            }
            v
        }
    };
    lists
        .into_iter()
        .find(|(_, _, l)| l.contains(&task))
        .map(|(role, shape, list)| Slot { parent, role, shape, list })
}

/// Kestra `GraphUtils.successors(graph, {chosen})` (`GraphUtils.java:144-239`)
/// over egret's plan and runs (S1).
fn successors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let n = state.runs.len();
    let mut out = BTreeSet::new();
    let subtree = |out: &mut BTreeSet<RunIx>, r: RunIx| {
        out.insert(r);
        out.extend((0..n).filter(|&ix| is_descendant_of(state, ix, r)));
    };
    let under = |p: Option<RunIx>| (0..n).filter(move |&ix| state.runs[ix].parent == p);
    subtree(&mut out, chosen);

    // `:157-169`: a node whose incoming edge comes from a cluster ROOT is
    // also walked from that root -- the first top-level task (`root → t1`)
    // and the first task of an `errors:` list (`root → e1`, `:468-472`). A
    // flowable's own root reaches its own errors, already in its subtree.
    if let Some(s) = slot(plan, state, chosen) {
        let first = s.list.first() == Some(&state.runs[chosen].task);
        let top_tasks = s.parent.is_none() && s.role == Role::Tasks;
        if first && (top_tasks || s.role == Role::Errors) {
            for r in under(s.parent).collect::<Vec<_>>() {
                subtree(&mut out, r);
            }
        }
    }

    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        let task = state.runs[cur].task;
        let later: BTreeSet<TaskIx> = match s.shape {
            Shape::Chain => s.list.iter().skip_while(|&&t| t != task).skip(1).copied().collect(),
            Shape::Parallel => BTreeSet::new(),
            Shape::Dag => dag_dependents(plan, state, s.parent, task),
        };
        for r in under(s.parent).collect::<Vec<_>>() {
            let t = state.runs[r].task;
            // `fillGraph` chains a task's several runs in order, then the next task's.
            let same_task_later = s.shape == Shape::Chain && t == task && r > cur;
            if later.contains(&t) || same_task_later {
                subtree(&mut out, r);
            }
        }
        // The scope's later phases (`:488-489`, `:413-421`).
        let finally: Vec<TaskIx> = match s.parent {
            None => plan.finally.clone(),
            Some(p) => plan.tasks[state.runs[p].task].finally.clone(),
        };
        let after: &[TaskIx] = if s.parent.is_none() { &plan.after_execution } else { &[] };
        let phases: Vec<TaskIx> = match s.role {
            Role::Tasks | Role::Errors => finally.iter().chain(after).copied().collect(),
            Role::Finally => after.to_vec(),
            Role::AfterExecution => Vec::new(),
        };
        for r in under(s.parent).collect::<Vec<_>>() {
            if phases.contains(&state.runs[r].task) {
                subtree(&mut out, r);
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}

/// The `Dag` nodes that depend on `task`, transitively.
fn dag_dependents(plan: &Plan, state: &ExecutionState, parent: Option<RunIx>, task: TaskIx)
    -> BTreeSet<TaskIx>
{
    let Some(p) = parent else { return BTreeSet::new() };
    let TaskKind::Dag { nodes } = &plan.tasks[state.runs[p].task].kind else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    let mut frontier = vec![task];
    while let Some(t) = frontier.pop() {
        for d in nodes.iter().filter(|d| d.depends_on.contains(&t)) {
            if out.insert(d.task) {
                frontier.push(d.task);
            }
        }
    }
    out
}

/// **S1's divergence (spec §5 item 6):** every scope that the chosen run, or
/// an ancestor the walk climbs through, sits in through its `tasks` (any
/// branch) has its `errors:` runs dropped with their subtrees. Kestra keeps
/// them, and a kept errors run holds the list in error mode
/// (`Execution.java:595-606`; egret `flowable::current_list`), so the
/// chosen task would never run.
fn scope_errors(plan: &Plan, state: &ExecutionState, chosen: RunIx) -> BTreeSet<RunIx> {
    let n = state.runs.len();
    let mut out = BTreeSet::new();
    let mut cur = chosen;
    while let Some(s) = slot(plan, state, cur) {
        if s.role == Role::Tasks {
            let errors: Vec<TaskIx> = match s.parent {
                None => plan.errors.clone(),
                Some(p) => plan.tasks[state.runs[p].task].errors.clone(),
            };
            for r in (0..n).filter(|&ix| state.runs[ix].parent == s.parent) {
                if errors.contains(&state.runs[r].task) {
                    out.insert(r);
                    out.extend((0..n).filter(|&ix| is_descendant_of(state, ix, r)));
                }
            }
        }
        match s.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    out
}
```

- `keep.contains` trên một `Vec` là O(n): ổn với các kích thước này. Một reviewer có thể đề nghị `BTreeSet`; cái nào cũng chấp nhận được.
- `Option::is_none_or` ổn định từ 1.82; MSRV là 1.85.
- Dùng tên field thật của `TaskKind::LoopUntil`, `If` và `Switch` từ `plan.rs:376-640`. `If { then_, else_, .. }` và `Switch { cases, default, .. }` như đã liệt kê trong phần hồ sơ ở trên.

**Test** (`executor/replay.rs`, `mod tests`):

```rust
use super::*;
use crate::executor::test_support::{drive, driven, flow, leaf, run};
use crate::executor::{build_plan, ExecutionState, RunState};

const PAR: &str = "io.kestra.plugin.core.flow.Parallel";
const SEQ: &str = "io.kestra.plugin.core.flow.Sequential";
const WD: &str = "io.kestra.plugin.core.flow.WorkingDirectory";

fn ids(plan: &Plan, state: &ExecutionState, ixs: impl IntoIterator<Item = RunIx>) -> Vec<String> {
    let mut v: Vec<String> = ixs.into_iter().map(|ix| plan.tasks[state.runs[ix].task].id.clone()).collect();
    v.sort();
    v
}

fn block(id: &str, ty: &str, indent: usize, body: &str) -> String {
    let pad = " ".repeat(indent);
    format!("{pad}- id: {id}\n{pad}  type: {ty}\n{pad}  tasks:\n{body}")
}
```

1. `replay_simple_keeps_what_came_before` (Kestra `replaySimple`, `ExecutionServiceTest.java:197-215`).
   - Flow: các task `t1..t5` (`leaf(.., 2)`), tất cả `SUCCESS`; replay từ `t2`.
   - Khẳng định `keep` là `t1, t2` và `reset` là `t2`.
2. `replay_parallel_nested_keeps_eight_of_eleven` (Kestra `replayParallel`, `:239-258`).
   - Flow: `parallel-nested.yaml` dựng bằng `block`:
     ```text
     1_par (Parallel)
     ├── 1-1
     ├── 1-2
     ├── 1-3_par (Parallel)
     │   ├── 1-3-1
     │   ├── 1-3-2_par (Sequential)
     │   │   ├── 1-3-2-1
     │   │   └── 1-3-2-2_end
     │   └── 1-3-3_end
     └── 1-4_end
     2_end
     ```
   - Replay từ `1-3-2_par`.
   - Khẳng định 11 run trước, 8 được giữ, và `reset` = `{1-3-2_par, 1-3_par, 1_par}`.
3. `replay_from_a_flowable_drops_its_errors_runs` (Kestra `replaySequentialWithErrorHandler`, `:563-596`).
   - Flow: `before`, rồi `sequential` với `failing-task` (kết quả `Failed`) và `errors: error-handler`. 4 run.
   - Replay từ `sequential`. Khẳng định `keep` = `before, sequential`.
4. `the_first_top_level_task_drops_everything_else`: các task `a, b, c`; replay từ `a`. Khẳng định `keep` = `a`.
5. `finally_and_after_execution_runs_are_dropped`: các task `a, b`; `finally: f`; `afterExecution: x`; replay từ `b`. Khẳng định `keep` = `a, b`.
6. `a_kept_scope_errors_run_is_dropped_s1`.
   - Flow: các task `a, b`; `errors: e`; kết quả `b = Failed` (các run `a, b, e`). Replay từ `b`. Khẳng định `keep` = `a, b`.
   - Một comment nói Kestra giữ `e`, và nêu tên S1.
7. `the_second_error_task_keeps_the_first`: như 6 với `errors: e, e2`; replay từ `e2`. Khẳng định `keep` = `a, b, e, e2` và `reset` = `e2`.
8. `the_first_error_task_reaches_the_whole_scope`: như 7, replay từ `e`. Khẳng định `keep` = `e` (quy tắc root của Kestra).
9. `in_a_parallel_siblings_are_not_successors`: `p (Parallel) [x, y]`, rồi `z`; replay từ `x`. Khẳng định `keep` = `p, x, y` và `reset` = `p, x`.
10. `in_a_switch_later_case_tasks_are_dropped`.
    - Flow: `s` (`io.kestra.plugin.core.flow.Switch`, `value: "A"`, `cases: {A: [a1, a2]}`), rồi `after`.
    - Chạy: `drive`, đặt `state.runs[run(.., "s")].rendered = Some("A".into())`, `drive` lần nữa.
    - Replay từ `a1`. Khẳng định `keep` = `s, a1` và `reset` = `s, a1`.
11. `a_loop_until_iteration_drops_later_iterations` (`same_task_later`).
    - Flow: `LoopUntil lu [c1, c2]`. `drive_from` với `pass_time` và một condition trả lời `false` một lần, rồi `true`. Nếu drive dùng chung chỉ trả lời `false`, hãy viết tay một state hai iteration như các test của `state.rs`.
    - Replay từ `c1` đầu tiên. Khẳng định `c1` và `c2` của iteration thứ hai bị bỏ.
12. `a_dag_drops_only_dependents`: các node `Dag` `d1`, `d2 dependsOn d1`, `d3`; replay từ `d1`. Khẳng định `keep` chứa `d3` và không chứa `d2`.
13. `a_working_directory_is_replayed_whole`: `w (WorkingDirectory) [w1, w2]`, rồi `c`; replay từ `w`. Khẳng định `keep` = `w`.
14. `a_run_inside_a_working_directory_is_refused`: replay từ `w1`. Khẳng định `Err(CopyRefusal::InsideWorkingDirectory)`.
15. `an_unfinished_kept_run_is_reset` (Kestra `replayParallelRestartsRunningSibling`, `:260-284`).
    - Flow: `p (Parallel) [x, y]`, tất cả `SUCCESS`. Sau đó `state.runs[y].history.pop()` và push `Running`.
    - Replay từ `x`. Khẳng định `reset` = `p, x, y`.
16. `an_empty_or_unknown_run_is_refused`: `replay_from(.., state.runs.len())` và một run có history bị xoá, mỗi cái cho `NoSuchRun`.
17. `restart_copy_resets_the_failed_and_drops_the_hooks`.
    - Flow: các task `a, b, c`; `errors: e`; `finally: f`; `afterExecution: x`; `b = Failed`.
    - Khẳng định `keep` = `a, b` và `reset` = `b`.
18. `restart_copy_refusals`: một execution `SUCCESS` cho `NotRestartable(Success)`.

### A2. Phần sao chép thuần: `copy_onto`, `shift_ticks`, `reset_copied` (spec D1, D5; S3, S4)

**File:** sửa `src/executor/replay.rs` và `src/executor/mod.rs` (re-export).

**Interface.**

Dùng: `CopySelection`, `CopyRefusal`, `MatchFault` của A1; `restart::{reset, to_created, kind}`.

Tạo ra:
```rust
/// The new execution's runs: the kept runs renumbered densely in order and
/// mapped onto the target plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedRuns {
    pub runs: Vec<TaskRun>,
    /// `old_of[new] = old` run index.
    pub old_of: Vec<RunIx>,
    /// `selection.reset`, renumbered; ascending.
    pub reset: Vec<RunIx>,
}

pub fn copy_onto(from: &Plan, to: &Plan, state: &ExecutionState, sel: &CopySelection)
    -> Result<CopiedRuns, CopyRefusal>;

/// Move every tick of `runs` by `by` ms: the attempts' `started_at`/`ended_at`
/// and `not_before`. A copy's ticks are measured from the NEW execution's
/// `created_at`.
pub fn shift_ticks(runs: &mut [TaskRun], by: i64);

/// Reset the copied runs (new indexes). 7a's per-kind state (`restart::reset`),
/// except a `Loop`, which goes `CREATED` and spawns its iterations anew (S4).
/// Ends with `SetExecutionState { Restarted }`. `loops` is always empty.
pub fn reset_copied(plan: &Plan, state: &ExecutionState, reset: &[RunIx], now: Tick)
    -> RestartCommands;
```

**Triển khai:**

```rust
pub fn copy_onto(
    from: &Plan,
    to: &Plan,
    state: &ExecutionState,
    sel: &CopySelection,
) -> Result<CopiedRuns, CopyRefusal> {
    let parent_task = parents(to);
    let mut new_of = std::collections::HashMap::new();
    let mut runs = Vec::with_capacity(sel.keep.len());
    let mut old_of = Vec::with_capacity(sel.keep.len());
    for &old in &sel.keep {
        let r = &state.runs[old];
        let id = &from.tasks[r.task].id;
        let fault = |why| CopyRefusal::TaskNotInSource { task_id: id.clone(), why };
        let t = to.tasks.iter().position(|t| &t.id == id).ok_or_else(|| fault(MatchFault::Missing))?;
        let parent = r.parent.map(|p| new_of[&p]);
        let want_parent = parent.map(|p: RunIx| runs[p].task as TaskIx);
        if parent_task.get(&t).copied() != Some(want_parent)
            || role_of(from, r.parent.map(|p| state.runs[p].task), r.task)
                != role_of(to, want_parent, t)
        {
            return Err(fault(MatchFault::Moved));
        }
        if std::mem::discriminant(&from.tasks[r.task].kind) != std::mem::discriminant(&to.tasks[t].kind) {
            return Err(fault(MatchFault::OtherKind));
        }
        new_of.insert(old, runs.len());
        let mut copy = r.clone();
        copy.task = t;
        copy.parent = parent;
        runs.push(copy);
        old_of.push(old);
    }
    let reset = sel.reset.iter().map(|o| new_of[o]).collect();
    Ok(CopiedRuns { runs, old_of, reset })
}
```

- `parents(plan) -> HashMap<TaskIx, Option<TaskIx>>`: `None` cho mỗi phần tử của `roots`, `errors`, `finally` và `after_execution`; `Some(p)` cho mỗi `c` trong `plan.children_of(p)` trên mọi `p`.
- `role_of(plan, parent_task: Option<TaskIx>, task) -> Role`: từ chính các list mà `slot` đọc. `Errors` / `Finally` lấy từ các list của parent (hoặc của flow), `AfterExecution` từ của flow, còn lại là `Tasks`.
- `new_of[&p]` không thể thiếu: `kept` bảo đảm parent được giữ và parent đứng trước con.

```rust
pub fn shift_ticks(runs: &mut [TaskRun], by: i64) {
    for r in runs {
        for a in &mut r.attempts {
            a.started_at = Tick(a.started_at.0 + by);
            a.ended_at = a.ended_at.map(|t| Tick(t.0 + by));
        }
        r.not_before = r.not_before.map(|t| Tick(t.0 + by));
    }
}

pub fn reset_copied(plan: &Plan, state: &ExecutionState, reset: &[RunIx], now: Tick) -> RestartCommands {
    let mut out = RestartCommands::default();
    for &run in reset {
        match restart::kind(plan, state, run) {
            // S4: the iterations are the old execution's; this one spawns its own.
            Some(TaskKind::Loop { .. }) => restart::to_created(run, &mut out),
            _ => restart::reset(plan, state, run, now, &mut out),
        }
    }
    out.commands.push(Command::SetExecutionState { state: RunState::Restarted });
    out
}
```

Field bên trong của `Tick` là `i64` (`claim.rs:4874`). Nếu nó private, dùng constructor và accessor sẵn có; không mở rộng visibility khi không có lý do.

**Test** (cùng module):

1. `the_identity_copy_renumbers_densely`.
   - Flow: `p (Parallel) [s (Sequential) [s1, s2], y (Sequential) [y1, y2]]`.
   - Replay từ `s1`; `copy_onto(&plan, &plan, ..)`.
   - Khẳng định:
     - `runs.len() == keep.len()`;
     - `old_of` tăng chặt;
     - `parent` của `y2` được sao chép là chỉ mục mới của `y` được sao chép;
     - không `parent` được sao chép nào ≥ chỉ mục của chính nó.
2. `a_task_missing_from_the_target_is_refused`: source đích không có `t2`; replay từ `t2`. Khẳng định `TaskNotInSource { task_id: "t2", why: Missing }`.
3. `a_kept_task_moved_under_another_parent_is_refused`: đích mà `t1` nằm trong một `Sequential` mới. Khẳng định `Moved`.
4. `a_kept_task_of_another_kind_is_refused`: đích mà `t1` là một `Sequential`. Khẳng định `OtherKind`.
5. `an_added_task_shifts_the_task_index`: đích `t0, t1, t2, t3, t4, t5`. Khẳng định `task` của `t1` được sao chép là chỉ mục của `t1` trong đích.
6. `shift_moves_every_tick`.
7. `reset_copied_runs_on_from_the_chosen_task`.
   - Replay từ `t2` của `t1..t3`; sao chép; `reset_copied`; `apply` các lệnh của nó; `drive`.
   - Khẳng định mọi run kết thúc `SUCCESS`.
   - Khẳng định lịch sử của execution chứa `Restarted`.
   - Khẳng định `t3` được tạo sau lần reset (3 run).
8. `a_copied_loop_is_created_again`.
   - Flow: `l (Loop)`, các giá trị được `drive` render thành `["a"]`, rồi `after`; replay từ `l`.
   - Khẳng định lệnh cho `l` là `SetRunState { Created }` và `reset_copied(..).loops` rỗng.
9. `a_copied_sequential_ancestor_is_reopened`: replay từ `s1` trong `s (Sequential) [s1, s2]`. Khẳng định `ReopenFlowable` cho `s` và `SetRunState { Created }` cho `s1`.

- [ ] Viết các test, chạy `cargo test -p egret-workflow --lib executor::replay` (đỏ), triển khai, chạy lại (xanh), chạy ratchet (`--test executor`, không đổi).
- [ ] Commit: `feat(workflow): pure copy of task runs onto a chosen source (package 7b)`.

### A3. Worker: chèn dòng, replay cả execution, và bản sao (spec D1, D6, D7, D10; S6, S7)

**File:**
- Sửa: `src/worker/claim.rs`, `src/worker/restart.rs`, `src/worker/mod.rs`.
- Tạo: `src/worker/copy.rs`, `tests/replay.rs`.

**Interface.**

Dùng: A1/A2 (`replay_from`, `restart_copy`, `copy_onto`, `shift_ticks`, `reset_copied`); `claim::{load, settle, run_job, Job, Stack, checked_inputs, attempts_json}`; `restart::{lock, Prepared, Opening, Op, side_writes}`; `secret_inputs::reveal`.

Tạo ra:
```rust
// worker/claim.rs
/// The row half of a start (everything `start_execution_in_on` did before
/// its settle). No settle: a start settles the empty state; a copy inserts
/// runs first.
pub(super) async fn insert_execution(
    conn: &mut PgConnection,
    gate: &Gate,
    new: NewExecution<'_>,
) -> Result<ExecutionCtx>;
/// A fresh row's first settle, `Tick(0)`, from the empty state.
pub(super) async fn settle_new(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<()>;
pub(super) fn attempts_json(attempts: &[Attempt]) -> serde_json::Value; // was private
pub(super) fn states_json(states: &[RunState]) -> serde_json::Value;     // was private

// worker/restart.rs
pub const REPLAY_LABEL_KEY: &str = "system.replay";
pub const REPLAY_LABEL_VALUE: &str = "true";
/// `labels` with `key: "true"` appended unless a label of that key is there.
pub(super) fn with_label(labels: Option<&serde_json::Value>, key: &str, value: &str)
    -> serde_json::Value;
/// A copied execution's reset, as 7a's restart job: locked, its commands
/// computed by `executor::reset_copied`, opened by the walk.
pub(super) async fn copy_job(
    conn: &mut PgConnection,
    id: Uuid,
    stack: Stack,
    reset: &[RunIx],
    op: CopyOp,
) -> Result<claim::Job>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyOp { Replay, Restart }

// worker/copy.rs (re-exported from worker)
pub struct ReplayRequest<'a> {
    pub workspace_id: Uuid,
    /// The execution replayed or restarted; the route scoped it.
    pub from: Uuid,
    /// The source the new execution runs and freezes, chosen by the route (D6, S6).
    pub source: &'a str,
    pub kind: ReplayKind,
    pub inputs: ReplayInputs,
    /// The person (D8: `check_start`'s door 2 judges the caller).
    pub started_by: Uuid,
    /// Chosen first: a multipart body's files are stored under it.
    pub id: Uuid,
    pub input_files: Vec<crate::storage::StoredFile>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayKind { Whole, FromTaskRun(Uuid), RestartOnSource }
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayInputs { Reuse, Replace(serde_json::Value) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayRefused {
    LoopIteration { root: Uuid },
    /// Replay: the execution has not ended; its state.
    NotTerminal(String),
    /// Restart on a source: 7a's refusals.
    NotRestartable(String),
    NothingToRestart,
    FilesPruned,
    AfterExecutionRunning,
    TaskRunNotFound,
    InsideWorkingDirectory,
    TaskNotInSource { task_id: String, why: crate::executor::MatchFault },
}
/// Spec D1-D10. Unscoped by workspace, like every worker entry point. A
/// refusal writes nothing. A deadlock's victim is run again (7a's retry).
pub async fn replay_execution(pool: &PgPool, req: &ReplayRequest<'_>)
    -> Result<std::result::Result<Uuid, ReplayRefused>>;
```

**Các bước.**

- [ ] **Refactor `start_execution_in_on`** (`claim.rs:4297-4452`). Chuyển phần thân từ `let NewExecution { .. } = new;` đến `index_input_files(..)` và việc dựng `ExecutionCtx { .. }` vào `insert_execution`. Nó trả về ctx. `start_execution_in_on` trở thành:
  ```rust
  let born_failed = new.born_failed;
  let ctx = insert_execution(conn, gate, new).await?;
  let empty = ExecutionState::new(&ctx.plan);
  let mut state = empty.clone();
  if born_failed {
      apply(&mut state, &Command::SetExecutionState { state: RunState::Failed });
  }
  settle(conn, &ctx, &empty, state, Tick(0), None).await?;
  Ok(ctx.id)
  ```
  `settle_new` là ba dòng không có `born_failed`. Chạy `cargo test -p egret-workflow --test worker --test inputs_start --test file_inputs`: tất cả xanh, cùng số lượng như trước. Commit: `refactor(workflow): split the start's row insert from its first settle`.
- [ ] **Viết các test đỏ** trong `tests/replay.rs` (danh sách bên dưới). Chạy `cargo test -p egret-workflow --test replay`; đỏ (không compile được).
- [ ] **Triển khai các bổ sung của `worker/restart.rs`.**
  - `Op` có thêm `Replay`; `CopyOp` ánh xạ sang `Op::Replay` hoặc `Op::Restart`.
  - `side_writes` nhận `label_key: &str` và gọi `with_label(labels, label_key, "true")`. Nó chỉ ghi khi kết quả khác label đã lưu, giữ hành vi hiện nay.
  - `open` truyền `REPLAY_LABEL_KEY` cho `Op::Replay` và `RESTARTED_LABEL_KEY` trong các trường hợp còn lại. `debug_assert!` của nó đọc `matches!(op, Op::Restart | Op::Replay)`.
  - `copy_job`:
    ```rust
    let Locked { ctx, state, now, row } = lock(conn, id, stack).await?;
    let cmds = crate::executor::reset_copied(&ctx.plan, &state, reset, now);
    let op = match op { CopyOp::Replay => Op::Replay, CopyOp::Restart => Op::Restart };
    Ok(claim::Job::Restart(Box::new(Opening {
        id,
        prepared: Prepared { ctx, state, now, cmds, labels: row.labels, op },
    })))
    ```
- [ ] **Triển khai `worker/copy.rs::replay_execution`**, trong `transaction!` với retry deadlock của 7a (sao chép `restart_execution`, `worker/restart.rs:84-99`). Bên trong, `replay_locked(conn, gate, req)` chạy các bước sau. **Mọi từ chối đến trước lần ghi đầu tiên**, vì `transaction!` commit một `Ok`.
  1. Nạp execution cũ dưới khoá của nó: `claim::load(conn, req.from, Stack::Fresh(gate.clone()))`. Khoá các run của nó `FOR UPDATE` theo thứ tự `run_ix`, như `restart::lock`. Sau đó đọc một dòng:
     ```sql
     WITH RECURSIVE chain(id) AS (
       SELECT $1::uuid
       UNION
       SELECT e.replayed_from_execution_id FROM workflow.executions e JOIN chain c ON e.id = c.id
        WHERE e.replayed_from_execution_id IS NOT NULL)
     SELECT CASE WHEN x.loop_index IS NOT NULL THEN x.parent_execution_id END AS loop_root,
            EXISTS (SELECT 1 FROM workflow.executions p JOIN chain c ON p.id = c.id
                     WHERE p.files_pruned_at IS NOT NULL) AS files_pruned,
            COALESCE(workflow.execution_is_idle(x.id), false) AS idle,
            x.labels, x.attempt_number, x.seq, x.inputs, x.secret_inputs, x.variables, x.flow_id
       FROM workflow.executions x WHERE x.id = $1
     ```
  2. Một loop root cho `LoopIteration { root }`.
  3. `Whole` và `FromTaskRun`: mục history cuối phải là trạng thái kết thúc (`RunState::is_terminal`), nếu không là `NotTerminal(wire)`.
  4. `FromTaskRun` và `RestartOnSource` (S7): `files_pruned` cho `FilesPruned`; `is_terminal && !idle` cho `AfterExecutionRunning`.
  5. Chọn:
     - `FromTaskRun(t)`: `SELECT run_ix FROM workflow.task_runs WHERE execution_id = $1 AND id = $2`; không có thì cho `TaskRunNotFound`. Sau đó `replay_from`: `NoSuchRun` cho `TaskRunNotFound` (một run bị làm rỗng), `InsideWorkingDirectory` đi qua nguyên vẹn.
     - `RestartOnSource`: `restart_copy`, các từ chối được ánh xạ như của 7a.
  6. `let target = crate::executor::build_plan(req.source)`, ánh xạ lỗi thành `Error::InvalidInput(format!("the flow does not plan: {e}"))`. Với các bản sao, `copy_onto(&old_ctx.plan, &target, &state, &sel)`, ánh xạ `TaskNotInSource`.
  7. Input:
     - `Reuse`: `secret_inputs::reveal(old.inputs, old.secret_inputs.as_deref())` (KI-112, như route hiện nay).
     - `Replace(v)`: `v` phải là một object, nếu không là `InvalidInput("inputs must be a JSON object")`. Sau đó `claim::checked_inputs(&crate::spec::parse(req.source)?, &v)` được ánh xạ thành `Error::InvalidInput(e.message)` (D7: bước kiểm tra của start với source **được chọn**). Lần insert bỏ qua bước kiểm tra riêng của nó vì `replayed_from_execution_id` được đặt (`claim.rs:4357-4358`).
  8. Label: `Whole` dùng `Some(with_label(old.labels, REPLAY_LABEL_KEY, REPLAY_LABEL_VALUE))`. Các bản sao dùng `old.labels` (`side_writes` thêm label).
  9. `let ctx = claim::insert_execution(conn, gate, NewExecution { .. })`, với:
     - `workspace_id: req.workspace_id`, `flow_id: old.flow_id`, `source: req.source`, `inputs`;
     - `trigger: None`, `scheduled_date: None`, `trigger_execution_id: None`, `execution_depth: 0`, `flow_trigger_event_id: None`, `born_failed: false`;
     - `replayed_from_execution_id: Some(req.from)`, `id: Some(req.id)`, `input_files: req.input_files.clone()`;
     - `labels`, `backfill_id: None`, `variables: old.variables`, `started_by: Some(req.started_by)`.

     Đây là chỗ `check_start` đánh giá source được chọn với người gọi (D8), và là chỗ các input FILE được kiểm tra.
  10. `Whole`: `UPDATE workflow.executions SET attempt_number = $2 WHERE id = $1` với `old.attempt_number + 1` (D10), rồi `claim::settle_new(conn, &ctx)`. Trả `Ok(Ok(req.id))`.
  11. Các bản sao:
      - `UPDATE workflow.executions SET attempt_number = $2, seq = $3 WHERE id = $1` với các giá trị cũ: `side_writes` cộng 1, và `created_seq`/`ended_seq` được sao chép vẫn thấp hơn đồng hồ.
      - Tính độ dịch:
        ```sql
        SELECT (EXTRACT(EPOCH FROM (o.created_at - n.created_at)) * 1000)::bigint
          FROM workflow.executions o, workflow.executions n WHERE o.id = $1 AND n.id = $2
        ```
        rồi `shift_ticks(&mut copied.runs, by)`.
      - `insert_runs(conn, req.id, req.from, &target, &copied)` (bên dưới).
      - `let job = restart::copy_job(conn, req.id, Stack::Fresh(gate.clone()), &copied.reset, op)`, với `CopyOp::Replay` cho `FromTaskRun` và `CopyOp::Restart` cho `RestartOnSource`.
      - `claim::run_job(conn, job)`; trả `Ok(Ok(req.id))`.
- [ ] **`insert_runs`** mang mọi cột kết quả từ dòng cũ và xoá lease. `claimed_at` không được liệt kê, nên là NULL. Một block `WorkingDirectory` được sao chép không sở hữu snapshot `_wd/{block}/` nào dưới id mới của nó, nên nó được đóng dấu là đã quét.
  ```rust
  const INSERT_COPIED_RUNS: &str = "\
  INSERT INTO workflow.task_runs \
    (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, rendered, rendered_run_if, \
     state, history, attempts, not_before_ms, created_seq, ended_seq, resume_with, working_dir, \
     started_at, ended_at, outputs, secret_outputs, outputs_bytes, message, \
     working_dir_snapshot, working_dir_swept_at) \
  SELECT $1, m.new_ix, m.task_ix, m.task_id, m.type_id, m.parent_ix, o.value, o.rendered, \
         o.rendered_run_if, o.state, o.history, m.attempts, m.not_before_ms, o.created_seq, \
         o.ended_seq, o.resume_with, o.working_dir, o.started_at, o.ended_at, o.outputs, \
         o.secret_outputs, o.outputs_bytes, o.message, o.working_dir_snapshot, \
         CASE WHEN o.working_dir IS NOT NULL THEN COALESCE(o.working_dir_swept_at, now()) END \
    FROM unnest($3::int[], $4::int[], $5::int[], $6::text[], $7::text[], $8::int[], \
                $9::jsonb[], $10::bigint[]) \
         AS m(old_ix, new_ix, task_ix, task_id, type_id, parent_ix, attempts, not_before_ms) \
    JOIN workflow.task_runs o ON o.execution_id = $2 AND o.run_ix = m.old_ix";
  ```
  - Bind `Vec<i32>`, `Vec<i32>`, `Vec<i32>`, `Vec<String>`, `Vec<Option<String>>`, `Vec<Option<i32>>`, `Vec<serde_json::Value>` (từ `claim::attempts_json`) và `Vec<Option<i64>>`.
  - `type_id` là của task **đích** (`TaskKind::Runnable { type_id }` hoặc NULL, `claim.rs:5546-5549`).
  - `id` lấy giá trị mặc định (`gen_random_uuid()`), nên mỗi bản sao có một task-run id mới (Kestra `mapTaskRunId`).
  - Một `&str` tĩnh, như sqlx 0.9 yêu cầu (memory: audit sqlx 0.9).
  - State và history của task là của dòng cũ. Việc reset là `side_writes` của 7a (output, message và lease bị xoá trên các run bị reset) cộng `persist` của `settle`.
- [ ] **Re-export** trong `worker/mod.rs`: `mod copy; pub use copy::{replay_execution, ReplayInputs, ReplayKind, ReplayRefused, ReplayRequest};` và `pub use restart::{REPLAY_LABEL_KEY, REPLAY_LABEL_VALUE};`. Trong `executor/mod.rs`: `pub use replay::{copy_onto, replay_from, reset_copied, restart_copy, shift_ticks, CopiedRuns, CopyRefusal, CopySelection, MatchFault};`.
- [ ] Chạy `cargo test -p egret-workflow --test replay`, rồi `--test restart --test worker --test inputs_start`, rồi ratchet.
- [ ] Commit: `feat(workflow): replay into a new execution, whole or from a task run (package 7b)`.

**Test** (`tests/replay.rs`).

Doc của module nêu tên spec và lệnh chạy:
```text
export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
cargo test -p egret-workflow --test replay
```
Sao chép `Flaky` (`tests/restart.rs:37-112`, với `attempts_of`) và các helper `setup`, `start`, `exec_state`, `exec_history`, `exec_meta`, `runs_of`, `run`, `drive`, `history_tail`, `tasks`, `three` (`tests/restart.rs:116-232`). Thêm test `database_url_is_exported_rather_than_read_from_dotenv`. Thêm:

```rust
async fn replay(pool: &sqlx::PgPool, ws: Uuid, from: Uuid, source: &str, kind: ReplayKind, inputs: ReplayInputs)
    -> Result<Uuid, ReplayRefused>
{
    let user: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws).fetch_one(pool).await.expect("owner");
    egret_workflow::worker::replay_execution(pool, &ReplayRequest {
        workspace_id: ws, from, source, kind, inputs, started_by: user,
        id: Uuid::new_v4(), input_files: Vec::new(),
    }).await.expect("replay")
}
```

1. `replay_from_a_task_keeps_earlier_runs_and_their_outputs`.
   - Chuẩn bị: `three("seq")`; `Flaky::new(&[("b", 1)])`; drive tới `FAILED`.
   - Thực hiện: replay `FromTaskRun(b.id)`, source đã đóng băng, `Reuse`; drive.
   - Khẳng định:
     - execution mới là `SUCCESS`;
     - `reg.attempts_of("a") == [1]` (không chạy lại);
     - `a` mới có `outputs` của `a` cũ và một `id` khác;
     - `b` và `c` mới đã chạy;
     - `exec_history` của execution mới bắt đầu bằng `["CREATED", "RESTARTED", "RUNNING"]`;
     - `replayed_from_execution_id` của nó là id cũ;
     - các dòng của execution cũ không đổi (history và số run).
2. `replay_from_a_parallel_branch`: `p (Parallel) [x, y]`, rồi `z`; `x` lỗi một lần. Replay từ `x`. Khẳng định `y` không chạy lại, `x` và `z` chạy, và execution mới là `SUCCESS`.
3. `a_copied_run_holds_no_lease`: sau bước Thực hiện của test 1 (trước khi drive), mọi run mới có `claimed_at IS NULL`, và `run_ix` là `0..n` không có khoảng trống.
4. `replay_from_a_subflow_run_spawns_a_new_child`.
   - Chuẩn bị: một flow con và một flow cha có `Subflow` `call` rồi `after`; drive.
   - Thực hiện: replay từ `call`.
   - Khẳng định: đúng một dòng child mới có `trigger_execution_id` = execution mới; child cũ không đổi.
5. `replay_from_a_loop_run_runs_its_iterations_anew` (S4): flow `l (Loop, values ["a","b"]) [item]`, rồi `after`. Replay từ `l`. Khẳng định có hai dòng iteration mới với `parent_execution_id` = id mới, và execution mới là `SUCCESS`.
6. `replay_on_an_edited_source_runs_the_edited_task`: replay từ `b` với một source trong đó message của `c` đã đổi. Khẳng định `executions.source` của dòng mới bằng text đã sửa và `c` đã chạy (`TaskInput` của nó đã được thấy).
7. `a_task_missing_from_the_source_is_refused_and_writes_nothing`: source không có `b`. Khẳng định `Err(TaskNotInSource { task_id: "b", why: Missing })` và số execution không đổi.
8. `new_inputs_reach_the_replayed_task_and_bad_ones_are_refused`.
   - Flow có `inputs: [{id: n, type: INT, min: 1}]` và một task có `TaskInput` được ghi lại.
   - Thay bằng `{"n": 5}`: `inputs` của dòng mới là `{"n": 5}`.
   - Thay bằng `{"n": 0}`: `Err` (`Error::InvalidInput` với ``Invalid value for input `n``), không ghi gì.
9. `the_whole_replay_gets_the_label_and_the_attempt` (D10): một execution `SUCCESS` có `attempt_number` 1. `Whole`. Khẳng định labels chứa `{"key":"system.replay","value":"true"}` và `attempt_number == 2`. Replay replay đó bằng `Whole`: `attempt_number == 3` và label xuất hiện một lần.
10. `a_copy_gets_the_label_once_and_the_attempt` (D10): execution mới của test 1 có `system.replay` một lần và `attempt_number == 2`.
11. `restart_on_a_source_resets_the_failed_and_keeps_the_rest` (D3).
    - `three` có `errors: e`; `b` lỗi một lần; `RestartOnSource` với text đã đóng băng cộng một message đã đổi trên `c`.
    - Khẳng định:
      - một execution mới có `system.restarted` và `attempt_number == 2`;
      - `a` được giữ (attempts của `a` == `[1]`), `b` chạy lại, `e` không được sao chép (không có run của `e` trước khi nó lỗi lần nữa), `c` đã chạy;
      - execution cũ vẫn là `FAILED`.
12. `refusals_write_nothing`: một execution đang chạy (`NotTerminal`), một iteration của loop (`LoopIteration`), một task-run id lạ (`TaskRunNotFound`), một run bên trong `WorkingDirectory` (`InsideWorkingDirectory`), và `files_pruned_at` được đặt trên source replay của execution (`FilesPruned`). Mỗi cái được theo sau bằng việc số execution không đổi.
13. `the_egret_sql_gate_judges_the_chosen_source`.
    - Dùng một flow thuộc về người dùng khác (seed bằng `common::seed_flow` dưới một người dùng thứ hai).
    - Source đã đóng băng không có `egret.sql` (được phép).
    - Một replay với source được chọn chứa task `egret.sql`, bởi người không phải creator: `Err` (`InvalidInput`, text `NOT_THE_CREATOR`), không ghi gì.
14. `a_kept_attempt_keeps_its_wall_clock`: `started_at` của attempt đầu của `a` mới cộng `created_at` của execution mới bằng của execution cũ cộng `created_at` cũ, trong 1 ms.

### A4. Restart trên source khác: phủ không qua route các hình dạng còn lại (spec D3, D9)

Được phủ bởi test 11–12 của A3. Task này thêm các hình dạng mà restart tại chỗ của 7a xử lý đặc biệt, nay dưới dạng bản sao.

**File:** mở rộng `tests/replay.rs`; sửa trong `src/worker/copy.rs` hoặc `src/executor/replay.rs` chỉ khi một test đỏ.

1. `a_restart_copy_of_a_failed_loop_creates_new_iterations` (S4): một `Loop` có iteration thứ hai bị lỗi. `RestartOnSource` cho các iteration mới dưới id mới, không cái nào được restart trong execution cũ.
2. `a_restart_copy_of_a_failed_subflow_spawns_a_new_child`: child cũ không được restart (không có lần tra cứu nào trúng dưới id mới, tương đương của `ExecutableUtils.java:90-150` là `subflow::spawn_one`).
3. `a_restart_copy_of_a_paused_execution_leaves_the_old_one_paused`: trung thành với Kestra. Khẳng định cái cũ vẫn là `PAUSED`.
4. `a_replay_of_a_replay_reads_two_back` (D4, S5): `X`; `Y` = bản sao của `X` từ `b`; `Z` = bản sao của `Y` từ `c`. Khi đó `storage::index::readable_executions(&mut conn, ws, Z)` chứa `X`. Test này ghim "đã có tính bắc cầu" của S5; nó xanh trước A5.
5. `an_after_execution_task_can_be_replayed`: flow có `afterExecution: x`. Replay từ `x` chạy lại `x` sau khi execution mới kết thúc; các task trước nó không chạy lại. Nếu hình dạng này không đi đến trạng thái nghỉ, **dừng lại và báo cáo**: không thêm code từ chối.

- [ ] Viết, chạy (`--test replay`), chỉ sửa cái đang đỏ, chạy ratchet.
- [ ] Commit: `test(workflow): restart on another source and replay chains (package 7b)`.

### A5. Quy tắc đọc với tới các iteration loop của chuỗi (S5)

**File:** sửa `src/storage/index.rs` (`readable_sql!` và doc ở `:159-171`); mở rộng `tests/replay.rs`.

- [ ] **Đỏ trước.** Thêm `a_copy_reads_its_source_loop_iterations_files`.
  - `X` chạy một `Loop` trên `["a"]`, rồi `after`.
  - `Y` = replay của `X` từ `after` (run `Loop` được giữ cùng output của nó).
  - Khẳng định `readable_executions(.., Y)` chứa id iteration của `X`.
  - Chạy nó. **Nếu nó xanh, dừng lại và báo cáo** (tiền đề sai).
- [ ] **Triển khai.** Trong `readable_sql!` thêm, sau `chain`:
  ```sql
  chain_family(id) AS (
    SELECT id FROM chain
    UNION
    SELECT e.id FROM workflow.executions e JOIN chain_family f ON e.parent_execution_id = f.id
     WHERE e.loop_index IS NOT NULL AND e.workspace_id = $1
  ),
  ```
  và đổi `UNION SELECT id FROM chain` của select cuối thành `UNION SELECT id FROM chain_family`. CTE `triggers` giữ `e.id IN (SELECT id FROM chain)`: một cấp trigger, như trước. Viết lại đoạn doc "The chain's own loop iterations are NOT in the set" để nói rằng chúng nay nằm trong tập, vì sao (một bản sao giữ output của run `Loop`, package 7b), và rằng tập được bảo vệ của pruner tăng theo.
- [ ] Chạy `cargo test -p egret-workflow --test replay --test storage_index --test storage_prune --test file_inputs`.
- [ ] Commit: `fix(workflow): a replay reads its source's loop iteration files (package 7b)`.

---

## B1. Route, OpenAPI và audit (spec D5, D8, D9, D11, D12, D13; S6, S8)

**File:**
- Sửa: `src/routes.rs`, `src/repo.rs`, `tests/restart_routes.rs` (test `revision`), `tests/executions_routes.rs` (test replay-labels).
- Tạo: `tests/replay_routes.rs`.

**Interface.**

Dùng: `worker::{replay_execution, ReplayRequest, ReplayKind, ReplayInputs, ReplayRefused, restart_execution}`; `FlowRepo::get_version`; `storage::index`.

Tạo ra:
```rust
// routes.rs
pub const INVALID_SOURCE_CODE: &str = "invalid_source";
pub const REPLAY_BREAKPOINTS_UNSUPPORTED_CODE: &str = "replay_breakpoints_unsupported";
pub const REPLAY_TASK_NOT_IN_SOURCE_CODE: &str = "replay_task_not_in_source";
pub const FLOW_VERSION_NOT_FOUND_CODE: &str = "flow_version_not_found";
pub const TASK_RUN_NOT_FOUND_CODE: &str = "task_run_not_found";
// RESTART_REVISION_UNSUPPORTED_CODE is deleted.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChoice { Frozen, Current, Version(i32) }
impl SourceChoice { pub fn wire(self) -> String } // "frozen" | "current" | "version:N"
/// `source` and Kestra's `revision` (an integer N = `version:N`); both → 400.
fn source_choice(source: Option<&str>, revision: Option<&str>) -> Result<SourceChoice, ExecOpError>;
async fn source_text(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail,
                     flow: &crate::repo::FlowRow, choice: SourceChoice) -> Result<String, ExecOpError>;

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct ReplayQuery {
    /// Replay from this task run of the execution (Kestra `taskRunId`).
    #[serde(rename = "taskRunId")]
    pub task_run_id: Option<String>,
    /// `frozen` (default), `current`, or `version:N`.
    pub source: Option<String>,
    /// Kestra's spelling of `version:N`. Not with `source`.
    pub revision: Option<String>,
    /// **Not supported** (spec Q3): any value answers 400 `replay_breakpoints_unsupported`.
    pub breakpoints: Option<String>,
}
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct RestartQuery { pub source: Option<String>, pub revision: Option<String> }

// repo.rs
/// The `Loop` run of `root` whose iterations (at any depth) hold `task_run`:
/// `(loop run id, loop task id)`, or `None`. Scoped by `ws`.
pub async fn loop_run_holding(&self, ws: Uuid, root: Uuid, task_run: Uuid)
    -> Result<Option<(Uuid, String)>>;
```

**SQL của `loop_run_holding`:**
```sql
WITH RECURSIVE up(id, parent, parent_run_ix, loop_index) AS (
  SELECT e.id, e.parent_execution_id, e.parent_run_ix, e.loop_index
    FROM workflow.task_runs t JOIN workflow.executions e ON e.id = t.execution_id
   WHERE t.id = $3 AND e.workspace_id = $1
  UNION
  SELECT e.id, e.parent_execution_id, e.parent_run_ix, e.loop_index
    FROM workflow.executions e JOIN up ON e.id = up.parent
   WHERE up.loop_index IS NOT NULL AND e.workspace_id = $1)
SELECT t.id, t.task_id FROM up
  JOIN workflow.task_runs t ON t.execution_id = up.parent AND t.run_ix = up.parent_run_ix
 WHERE up.parent = $2 AND up.loop_index IS NOT NULL
```

**Hình dạng các handler.**

- **`ExecOpError` có thêm nhánh 404.** Trong `into_response`, `StatusCode::NOT_FOUND` dùng `Error::NotFound(message).to_string()`. Nó cũng có thêm `fn from_replay_refusal(id: Uuid, r: ReplayRefused) -> Self`:
  - `NotTerminal(s)`: `Core(Error::Conflict(..))`, với text hiện nay (`routes.rs:3349-3352`);
  - `LoopIteration`: `loop_iteration(id, root, "cannot be replayed")`;
  - `NotRestartable` / `NothingToRestart` / `FilesPruned` / `AfterExecutionRunning`: các text của 7a;
  - `TaskRunNotFound`: `Coded(NOT_FOUND, TASK_RUN_NOT_FOUND_CODE, "no task run {t} in execution {id}")`;
  - `InsideWorkingDirectory`: 409 của 7a;
  - `TaskNotInSource`: `Coded(BAD_REQUEST, REPLAY_TASK_NOT_IN_SOURCE_CODE, ..)`. Message nêu task id và lỗi: "is not in the chosen source", "sits under another parent or list in the chosen source", "is of another kind in the chosen source".
- **`source_choice`:**
  - cho cả hai: 400 `invalid_source`, "pass `source` or `revision`, not both";
  - `revision`: parse `i32` ≥ 1 thành `Version(n)`, nếu không `invalid_source`;
  - `source`: `frozen`, `current`, hoặc `version:N` với N ≥ 1, nếu không `invalid_source` nêu giá trị;
  - không cái nào: `Frozen`.
- **`source_text`:**
  - `Frozen`: `detail.source.clone()`;
  - `Current`: `flow.source.clone()`;
  - `Version(n)`: `st.repo.get_version(ws, flow.id, n)`. `Error::NotFound` trở thành `Coded(NOT_FOUND, FLOW_VERSION_NOT_FOUND_CODE, "flow version {n} not found")`. Sau đó `snapshot.get("source").and_then(as_str)`, nếu không `Internal` (như `routes.rs:1380-1391`).
- **`rerun_gate`** nhận text source cần đánh giá: `rerun_gate(st, ws, caller, detail, source: &str)`. `restart_execution` (frozen) và `set_task_run_state` truyền `&before.source`.
- **Các helper body, tách ra từ start.**
  - `async fn read_json_body(st: &WorkflowState, req: Request) -> Result<Option<StartExecutionRequest>, Response>`: giới hạn 2 MiB và quy tắc không-header-là-JSON của `routes.rs:2642-2683`, nguyên văn. Sau đó `start_flow_execution` gọi `start_inputs(body.map(Json))`.
  - `struct Uploaded { inputs: serde_json::Map<String, Value>, files: Vec<StoredFile> }`.
  - `async fn receive_parts(st, workspace, parsed: &Flow, execution: Uuid, req: Request) -> Result<Uploaded, Response>`: việc tra storage, giới hạn và dựng gate của `start_multipart` (`:2943-2969`) và vòng lặp part của `read_parts_and_start` (`:3013-3068`). Nó xoá các upload của chính nó khi tự lỗi.
  - `async fn discard_uploads(workspace: Uuid, parsed: &Flow, execution: Uuid)`: `delete_prefix` kiểu best-effort của `:2986-2995`.
  - `start_multipart` trở thành `receive_parts`, rồi transaction start (`:3073-3098`), rồi `discard_uploads` khi `Err`.
- **`replay_execution`** (`State`, `WsContext`, `Path((_ws, id))`, `Query(q): Query<ReplayQuery>`, `req: axum::extract::Request`), trả `Result<(StatusCode, Json<ExecutionDto>), Response>`:
  1. `require(WF_MANAGE)`.
  2. `q.breakpoints.is_some()`: 400 `replay_breakpoints_unsupported`, "breakpoints are not supported yet; they arrive with their own package, after 7c (pause and resume)".
  3. `let choice = source_choice(..)?`.
  4. `detail = executions.get` (404). Loop iteration: 400 hiện nay. Flow NULL: 404. `flow = startable_flow` (404/400/409). Chưa kết thúc: 409 hiện nay.
  5. `let source = source_text(..)?`.
  6. Kind:
     - không có `taskRunId`: `Whole`;
     - một giá trị không parse được thành UUID: 404 `task_run_not_found`;
     - một UUID nằm trong `detail.runs` có history không rỗng: `FromTaskRun(t)`;
     - nếu không thì `executions.loop_run_holding(ws, id, t)`: `Some((loop_run, loop_task))` cho 400 (không có code): "task run {t} is inside an iteration of the Loop `{loop_task}` (task run {loop_run}) of execution {id}; replay execution {id} from that Loop task run instead". `None` cho 404 `task_run_not_found`.
  7. `let new_id = Uuid::new_v4()`. Body:
     - multipart (`is_multipart`): `receive_parts` với `parse(&source)`, rồi `Replace(Value::Object(inputs))` và các file của nó;
     - nếu không, `read_json_body`: một body có `inputs` là object cho `Replace`; một body có `inputs` null hoặc vắng, hoặc không có body, cho `Reuse`; `inputs` không phải object cho 400 (message của `start_inputs`).
  8. `worker::replay_execution(&st.db.pool, &ReplayRequest { .. })`. Khi `Err` hoặc một từ chối sau một lần upload multipart, gọi `discard_uploads`. Ánh xạ các từ chối bằng `from_replay_refusal`.
  9. Audit `workflow_execution:replay` với `{"replayed_from": id, "replacement": new, "task_run_id": <uuid|null>, "source": choice.wire(), "inputs_replaced": bool}`. Không bao giờ ghi các input.
  10. Đọc lại, 201.
- **`restart_execution`** có thêm `Query(q): Query<RestartQuery>`, trả `Result<(StatusCode, Json<ExecutionDto>), ExecOpError>`.
  - `choice == Frozen`: đường của 7a không đổi, 200.
  - Nếu không:
    1. `detail` (404); `flow = startable_flow`; `source = source_text(..)`;
    2. `rerun_gate(.., &source)`;
    3. `replay_execution` với `RestartOnSource`, `Reuse`, `new_id`, không file;
    4. audit `workflow_execution:restart` với `{"from_state", "replacement": new, "source": choice.wire()}`;
    5. 201 với execution mới.
- **Router:** route replay có thêm `.layer(axum::extract::DefaultBodyLimit::disable())`, kèm comment của route start.
- **OpenAPI.**
  - Replay: `params(WorkspacePath, ("id" ..), ReplayQuery)`; `request_body(description = "Optional. ..", content((StartExecutionRequest = "application/json"), (StartExecutionMultipart = "multipart/form-data")))`; các response 201, 400 (mọi code kèm ý nghĩa), 401, 403, 404 (`task_run_not_found`, `flow_version_not_found`, không code), 409 (chưa kết thúc, bị disable, `inside_working_directory`, `files_pruned`, `after_execution_running`), 413, 415, 500, 503.
  - Restart: params `RestartQuery`; 200 (tại chỗ) và 201 (execution mới, label `system.restarted`, `attempt_number + 1`); 400 nay liệt kê `invalid_source` và `replay_task_not_in_source`, không còn `restart_revision_unsupported`; 404 thêm `flow_version_not_found`.
  - Viết lại doc của handler ("It is a start, deliberately, and from the FROZEN source") để nói ba source là gì và rằng source đã đóng băng vẫn là mặc định.

**Test** (`tests/replay_routes.rs`). Doc của module liệt kê hai route và lệnh chạy. Sao chép `Flaky`, `flow`, `setup`, `start`, `flow_of` từ `tests/restart_routes.rs:28-120`. Thêm test `database_url_is_exported_rather_than_read_from_dotenv`. Mỗi từ chối được ghép với một lệnh gọi đối chứng được chấp nhận:

1. `replay_from_a_task_answers_201_with_the_new_execution`: body `{}`, `?taskRunId=<b>`. 201, `replayed_from_execution_id` = cũ, id mới.
2. `revision_is_version` (alias Kestra):
   - publish version 1 bằng `INSERT INTO workflow.flow_versions (flow_id, version, snapshot, created_by) VALUES ($1, 1, jsonb_build_object('source', $2::text), $3)`;
   - `?revision=1`: 201, `source` = text của version;
   - `?revision=1&source=current`: 400 `invalid_source`;
   - `?source=version:x`: 400 `invalid_source`;
   - `?revision=9`: 404 `flow_version_not_found`.
3. `source_current_runs_the_draft`: cập nhật `workflow.flows.source`, rồi `?source=current`. Source của execution mới là bản nháp.
4. `breakpoints_are_refused`: `?breakpoints=a` cho 400 `replay_breakpoints_unsupported`; không ghi gì (số execution); lệnh đối chứng không có nó cho 201.
5. `a_foreign_or_unknown_task_run_is_404`:
   - một run của execution khác trong cùng workspace;
   - một run ở workspace khác;
   - `?taskRunId=nope`.

   Mỗi cái cho 404 `task_run_not_found`.
6. `a_task_run_inside_a_loop_iteration_names_the_loop`: 400, và message chứa task id của `Loop` và id root.
7. `a_task_missing_from_the_source_is_400`: `?source=current` sau khi xoá `b` khỏi bản nháp. 400 `replay_task_not_in_source`, và message chứa `` `b` ``.
8. `inside_working_directory_is_409`.
9. `new_inputs_by_json_and_by_multipart`:
   - JSON `{"inputs":{"n":5}}`: 201;
   - `{"inputs":{"n":0}}`: 400 nêu `n`;
   - multipart với một part text `n=7` (`common::send_raw`, boundary viết tay như trong `tests/inputs_start.rs`): 201, inputs `{"n":7}`.
10. `workflow_read_is_refused`: 403 cho một người dùng chỉ có `workflow:read`, trên cả hai route.
11. `the_audit_names_the_task_run_the_source_and_whether_inputs_changed`: `common::audit_mutations(&app)` có `workflow_execution:replay` với bốn field và không có key `inputs`.
12. `restart_on_a_source_answers_201_and_is_audited`: `?source=current`. 201, một id mới, label `system.restarted`; audit `workflow_execution:restart` với `replacement` và `source`. `?source=frozen` và không có query giữ 200 tại chỗ.
13. `the_egret_sql_gate_applies_to_the_chosen_source`: một người không phải creator replay `?source=current` mà bản nháp thêm `egret.sql` nhận 400, không ghi gì; lệnh đối chứng frozen cho 201.
14. `the_entitlement_gate_covers_replay` (như `the_workflows_entitlement_gates_these_routes`).

**Cập nhật các test hiện có:**
- `tests/restart_routes.rs:253`: ca `revision` trở thành `?revision=abc` → 400 `invalid_source`, cộng `?revision=1` khi không có version nào được publish → 404 `flow_version_not_found`.
- `tests/executions_routes.rs:2100-2146`: các label kỳ vọng có thêm `{"key":"system.replay","value":"true"}` (spec D10).

**Các bước.**
- [ ] Viết các test và chạy `cargo test -p egret-workflow --test replay_routes` (đỏ).
- [ ] Triển khai.
- [ ] Chạy `cargo test -p egret-workflow --test replay_routes --test restart_routes --test executions_routes --test routes --test inputs_start --test file_inputs --test secret_inputs`.
- [ ] Chạy `cargo clippy -p egret-workflow --all-targets -- -D warnings`.
- [ ] Chạy `cargo test -p egret-server committed_openapi_json_is_current`. Nếu nó đỏ, sinh lại file OpenAPI đã commit theo cách message của test chỉ dẫn (7a làm điều này ở B1) và commit nó.
- [ ] Commit: `feat(workflow): replay from a task run, choose the source, change inputs; restart on a source (package 7b)`.

---

## C1. Frontend (spec D14; S9)

**File:**
- Sinh lại: `frontend/src/api/generated/**` (controller: `npm run gen:api`, không bao giờ chạy orval trần; commit riêng là `chore(frontend): regenerate the API client (package 7b)`).
- Sửa: `components/workflows/executions/actions.ts` (+ `__tests__/actions.ops.test.ts`), `routes/workflows/ExecutionDetail.tsx`, `routes/workflows/ExecutionOverview.tsx` (+ `routes/workflows/__tests__/ExecutionDetail.test.tsx`).
- Tạo: `components/workflows/executions/ReplayDialog.tsx` và `__tests__/ReplayDialog.test.tsx`.

Đọc `docs/en/frontend-rules.md` (RULEBOOK của frontend) trước; mọi quy tắc đều áp dụng.

**Bổ sung cho `actions.ts`:**
```ts
export type SourceChoice = { kind: "frozen" } | { kind: "current" } | { kind: "version"; version: number };
export function sourceParam(choice: SourceChoice): string; // "frozen" | "current" | `version:${n}`
/** Replay is offered from a task run where the whole replay is (D14). */
export function showsReplayFromTask(execution: Pick<ExecutionDto, "state">): boolean; // = showsReplay
/** One sentence for each coded refusal of the replay route; `null` otherwise. */
export function replayRefusal(code: string | undefined): string | null;
```
- `replayRefusal` phủ `invalid_source`, `replay_breakpoints_unsupported`, `replay_task_not_in_source`, `flow_version_not_found`, `task_run_not_found`, `inside_working_directory`, `files_pruned` và `after_execution_running`.
- `restartRefusal` thêm `invalid_source`, `replay_task_not_in_source` và `flow_version_not_found`.
- Một câu cho `replay_task_not_in_source` nối thêm message của server, vốn nêu tên task.

**`ReplayDialog.tsx`** (một component, hai chế độ):
```ts
export function ReplayDialog(props: {
  ws: string;
  execution: ExecutionDto;            // its source, inputs, flow_id
  mode: "replay" | "restart";
  taskRun?: TaskRunDto | null;        // replay from this task when set
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element;
```
- **Tiêu đề:**
  - "Replay this execution?";
  - "Replay from {task_id}?";
  - "Restart this execution?".
- **Lựa chọn source:** một `RadioGroup` (`components/ui/radio-group.tsx`):
  - *This run's version* (mặc định);
  - *The flow's current version*;
  - *A published version*, với một `Select` của `useWorkflowListFlowVersions(ws, flowId)` (mới nhất trước, "Version N, {date}, {note}"). Bị disable kèm lý do khi danh sách rỗng.
- **Text restart:** với source mặc định, câu của 7a. Nếu không, "Restarting on another version creates a new execution: the failed tasks run again on that version and the tasks that succeeded keep their results."
- **Change inputs** (chỉ replay): một `Collapsible`, mặc định đóng.
  - Text của source được chọn là `execution.source` (frozen), `useWorkflowGetFlow(ws, flowId).data.source` (current), hoặc `useWorkflowGetFlowVersion(ws, flowId, n).data.snapshot.source` (version).
  - `inputsFromSource(text)` cho các input. Điền sẵn bằng `formFromInputs(inputs.map(i => i.id in old ? { ...i, initial: old[i.id] } : i))`, trong đó `old` là `execution.inputs`, trừ:
    - một input SECRET không bao giờ được điền sẵn (dòng chứa `******`) và được đánh dấu "Enter it again";
    - một input FILE không có file mới được gửi bằng giá trị `kestra://` cũ của nó: server đọc nó từ chuỗi replay.
  - Validate bằng `validateForm` trước khi gửi. Một message của server nêu tên một input được đặt bên cạnh nó (`serverErrorsByInput`).
  - Một kiểu input không được hỗ trợ disable "Change inputs" kèm lý do, như `RunFlow` làm.
- **Submit:**
  - Replay, input không đổi: `replay.mutateAsync({ workspace, id, params: { taskRunId, source }, data: {} })`.
  - Replay, input đổi bằng JSON: `data: requestFromForm(form).json`.
  - Replay, input đổi bằng multipart: `customInstance(getWorkflowReplayExecutionUrl(ws, id, params), { method: "POST", body: req.multipart })`, như `RunFlow.tsx:159-169`.
  - Khi 201, invalidate `workflowsKeys.executions(ws)` và điều hướng tới overview của execution mới (không toast, FE-ACTION-2).
  - Restart: `restart.mutateAsync({ workspace, id, params: { source } })`. Khi 200, refresh và toast của 7a. Khi 201, điều hướng tới execution mới.
  - Mọi từ chối ở lại trong dialog cùng câu của nó (FE-ACTION-3).
  - Kiểm tra hình dạng biến chính xác của hook được sinh sau `gen:api`; các tên ở trên là tên thường dùng của orval (`params`, `data`).

**Nối dây:**
- `ExecutionDetail.tsx`: `ConfirmAction` của Replay (`:544-552`) và `ConfirmAction` của Restart (`:526-534`) được thay bằng `ReplayDialog` ở `mode="replay"` và `mode="restart"`. `runReplay` và `runRestart` chuyển vào trong dialog. Một `flow_id` là `null` giữ lý do của `canReplay`.
- `ExecutionOverview.tsx` `TaskRuns`:
  - ô action trở thành một `DropdownMenu` với trigger `aria-label={`Actions for ${run.task_id}`}`;
  - mục "Change state" khi `changeable` (predicate của 7a);
  - mục "Replay from this task" khi `canManage && showsReplayFromTask(execution) && canReplay(execution).ok && !isLoopIteration(execution)`;
  - ô hiện khi một trong hai mục hiện;
  - mục thứ hai mở `ReplayDialog` với `taskRun={run}`.

**Test** (vitest):
- `actions.ops.test.ts`: `sourceParam` cho ba kind; mọi code của `replayRefusal` cho một câu khác nhau, khác null; một code lạ cho `null`.
- `ReplayDialog.test.tsx`:
  - source mặc định gửi `source: "frozen"` và `data: {}`;
  - chọn một version gửi `source: "version:2"`;
  - select version bị disable kèm lý do khi không có version nào;
  - Change inputs được điền sẵn từ `execution.inputs` và một SECRET thì không;
  - một input đã đổi gửi `data.inputs`;
  - một 400 `replay_task_not_in_source` hiện câu của nó và giữ dialog mở;
  - chế độ restart với `current` hiện text "creates a new execution" và điều hướng khi 201.
- `ExecutionDetail.test.tsx`: menu của dòng hiện "Replay from this task" cho một execution `SUCCESS` và không cho một execution đang chạy, cũng không cho một iteration của loop. Cập nhật snapshot ở `:849` (text của dòng không còn kết thúc bằng "Change state").
- [ ] Chạy `npm run typecheck`, `npm run lint`, và các file vitest bị đụng tới. Controller cung cấp `node_modules`: mượn nó, không bao giờ cài.
- [ ] Commit: `feat(workflows): replay from a task, choose the version, change inputs; restart on a version (package 7b)`.

---

## Z. Docs, ghi sổ, gate và kiểm tra trực tiếp

- [ ] **Spec** `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`:
  - §5 thêm các điểm khác biệt người dùng đã xác nhận: 6 (S1), 7 (S2), 8 (S4), 9 (S10), và ghi chú của S11 về các list bị sắp xếp lại;
  - một §7 "As built" mới, theo hình dạng của 7a: commit theo từng task, số đếm đã đo, sai lệch và lý do, ratchet ở T0 và ở cuối.
- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`: một ghi chú có ngày dưới dòng #7 rằng 7b đã xong. Các dòng của Phụ lục §5 cho replay từ một task, replay với revision và input, và restart với revision trở thành được hỗ trợ, kèm bằng chứng route.
- [ ] **Bản tiếng Việt:** `docs/vi/specs/2026-11-14-workflow-replay-from-task-design.md` và `docs/vi/plans/2026-11-14-workflow-replay-from-task.md` (quy ước song ngữ). Một subagent Sonnet có thể dịch.
- [ ] **`egret-data/docs`** (Platform, Workflows, executions, Run actions): replay từ một task, ba source, change inputs, restart trên version khác, mọi từ chối, và ảnh chụp mới. Controller mở PR đó.
- [ ] **Gate**, từng cái một:
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow --no-fail-fast`: báo cáo passed, failed và ignored; chỉ `sql_starrocks_live` được phép lỗi, vì thiếu engine;
  - `cargo test -p egret-config -p egret-server -p egret-metastore`, gồm `committed_openapi_json_is_current`;
  - ratchet và census: 301/25/25/0 và 295/25/21;
  - frontend `npm run typecheck`, `npm run lint`, vitest.
- [ ] **Kiểm tra trực tiếp** trên một `egret serve` native với các container riêng của worktree (không bao giờ dùng DB của demo stack dùng chung). Trước đó, grep binary tìm chuỗi code mới `replay_task_not_in_source` (memory: build cache ship một binary cũ). Trên trình duyệt, với `admin@egret.local`, sáng và tối (FE-TEST-2 còn nợ của 7a):
  1. Chạy `a, b (lỗi một lần), c`. Dùng **Replay from this task** trên `b`; xem execution mới chạy `b` và `c` và giữ output của `a`.
  2. Sửa `c` trong bản nháp, rồi replay từ `b` với **The flow's current version**; xem `c` đã sửa chạy.
  3. Restart một execution lỗi trên một version đã publish; xem một execution mới mở ra, có label `system.restarted`.

  Chụp ảnh cho docs.
- **Chấp nhận:** mọi gate qua; ratchet không đổi; kiểm tra trực tiếp qua, kèm ảnh chụp.
