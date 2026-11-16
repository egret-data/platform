# Gói workflow 7c: pause bằng tay, resume với input `onResume`, kiểm tra input resume

> Bản dịch tiếng Việt của `docs/en/specs/2026-11-15-workflow-pause-resume-design.md`. Khi hai bản khác nhau, bản tiếng Anh là chuẩn.

**Trạng thái:** thiết kế ngày 2026-11-15. Branch `feat/workflow-pause-resume`, một PR. Dòng #7 của roadmap (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), gói con thứ ba trong sáu gói 7a–7f. 7a (restart tại chỗ, #860) và 7b (replay từ một task, #866) đã merge; spec của chúng là `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md` ("7a Dn") và `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md` ("7b Dn"). Kestra là bản checkout tại `data-portal/kestra`. `ES` là `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `EC` là `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`, và `Pause.java` là `core/src/main/java/io/kestra/plugin/core/flow/Pause.java`.

## 1. Vấn đề

egret đã chạy task `Pause` (`pauseDuration`, `timeout`) và đã có `POST /api/ws/{ws}/workflows/executions/{id}/resume` (`routes.rs` `resume_execution`). Route đó không nhận body: mọi run `Pause` đang dừng đều kết thúc `SUCCESS`. Kestra có ba thứ mà egret còn thiếu:

1. **Pause bằng tay** (`POST /{id}/actions/pause`, `EC:2017-2035`; `ES.pause`, `ES:907-916`). Nó đặt một execution đang chạy sang `PAUSED` và không đụng tới task run nào. Các run đang chạy vẫn chạy xong, nhưng executor không lên lịch thêm gì (`ExecutorContext.canBeProcessed()` trả false với `PAUSED`). Resume không tìm thấy run `PAUSED` nào nên đặt execution trở lại `RUNNING` (`ES:880-897`).
2. **Input `onResume`.** Task `Pause` khai báo `onResume: List<Input>` (`Pause.java:193`). Resume (`EC:1872-1906`, multipart) gán kiểu và kiểm tra các giá trị gửi lên theo `onResume` của run `PAUSED` đầu tiên (`ES.readInputs`, `FlowInputOutput.readExecutionInputs`). Nó ghi chúng vào output của run đó dưới dạng `{onResume: {...}, resumed: {by, on, to}}` (`Pause.java:305-330`; `ES.markAs`, `ES:603-661`), nên flow đọc được `{{ outputs.<pause>.onResume.<input> }}`.
3. **Kiểm tra input resume** (`POST /{id}/actions/resume/validate`, `EC:1854-1870`; `ES.validateForResume`, `ES:791-822`). Nó kiểm tra các giá trị gửi lên mà không resume, và trả về cùng dạng với API kiểm tra input lúc bắt đầu chạy.

Hiện egret chỉ cảnh báo khi gặp `onResume` và ghi rằng nó thuộc dòng #9 của roadmap (`spec/keys.rs`). `docs/en/specs/2026-10-28-workflow-inputs-design.md` cũng giao nó cho #9, trong khi spec của 7a giao nó cho 7c. Câu Q1 ở §2.1 chốt điểm này.

## 2. Quyết định

### 2.1 Quyết định của người dùng, 2026-11-15 (đã hỏi, đã trả lời)

- **Q1. `onResume` thuộc 7c.** 7c mô hình hoá `Pause.onResume`, bỏ cảnh báo, kiểm tra input resume theo nó và ghi hai output `onResume` và `resumed`. Phần còn lại của `Pause` vẫn thuộc dòng #9: `behavior`, `onPause`, `errors` và `finally` vẫn bị từ chối khi publish (`unsupported_pause_property`). Spec inputs và roadmap được ghi chú sửa có ngày.
- **Q2. Pause bằng tay làm theo Kestra.** Nó chỉ chuyển execution sang `PAUSED` và không đổi task run nào. Các run đang `RUNNING` hoặc `SUBMITTED` chạy xong và ghi kết quả. Không run nào được tạo hay bắt đầu cho đến khi resume. Resume đặt lại `RUNNING`. Phương án bị loại: dừng các task đang chạy như kill rồi chạy lại khi resume, vì cách đó chạy lại những việc không idempotent và Kestra không làm vậy.
- **Duyệt thiết kế, 2026-11-15:** người dùng đã duyệt phần 1 (engine và API, D1–D10 bên dưới) và phần 2 (giao diện, test, tài liệu, D11–D13).
- **Q3–Q6 (2026-11-15, sau phần điều tra của plan; xem "Spec corrections" S1, S2, S3, S7 của plan).**
  - **Q3 (S1): pause loop gốc không ngừng việc tạo iteration mới, như ở Kestra.** Khi một iteration kết thúc, `spawn::on_execution_terminal_inner` bắt đầu iteration kế tiếp dưới khoá của loop gốc, bất kể trạng thái của loop gốc (`worker/spawn.rs:735-746`). Kestra cũng làm vậy: `LoopExecutionEventMessageHandler.handleTerminated` (`LoopExecutionEventMessageHandler.java:94-182`) phát ra iteration kế tiếp mà không hỏi `canBeProcessed()`. Run `Loop` hoàn tất khi iteration cuối kết thúc; không có gì sau nó bắt đầu cho đến khi resume. D3 được sửa.
  - **Q4 (S2): input `onResume` kiểu FILE được lưu dưới run `Pause`**, tại `tasks/{pause-task-slug}/{pause_run_id}/resume/{request_id}/{input}/{file}`, và được index trong `workflow.execution_files` dưới run đó. `resume/{task_run_id}/…` trong spec không phải là URI file của execution mà egret parse hay đọc được. D6 và §5 điểm khác 6 được sửa.
  - **Q5 (S3): pause bằng tay bị từ chối khi còn bất kỳ run nào đang chờ.** Nếu trạng thái cuối của bất kỳ run nào là `PAUSED` (một `Pause` hay một `LoopUntil` đang chờ), pause trả 409 `not_running`, và thông báo nói rằng đã có một task đang chờ. Nhờ vậy D1 luôn đúng: một execution `PAUSED` bị pause bằng tay đúng khi không run nào đang chờ. Kestra thì chấp nhận. §5 điểm khác 8. D3 và D10 được sửa.
  - **Q6 (S7): input `onResume` kiểu SECRET được che và seal**, như các giá trị SECRET hiện nay: `******` trong `outputs`, giá trị rõ được seal trong `task_runs.secret_outputs`, và chỉ được tiết lộ cho render surface. Khi không có `secrets.master_key`, resume bị từ chối với 400 nêu tên input. D5 có thêm điều này.

### 2.2 Quyết định thiết kế (mặc định, 2026-11-15)

- **D1. "Pause bằng tay" là một sự kiện suy ra, không phải một cột mới.** Một execution bị pause bằng tay khi trạng thái cuối của nó là `PAUSED` và không run nào có trạng thái cuối là `PAUSED`. Đây chính là phép thử mà `ES.resume` của Kestra dùng (`findFirstByState(PAUSED)` rỗng, `ES:880-897`). Một task `Pause` hay một `LoopUntil` đang chờ luôn để lại một run `PAUSED`, nên hai trường hợp không thể lẫn nhau. Một hàm thuần duy nhất, `executor::paused_by_hand(&ExecutionState) -> bool`, giữ quy tắc này. Mọi nơi gọi (executor, route resume, `ExecutionDto`) đều dùng nó.
- **D2. Execution bị pause bằng tay không được đẩy tiếp.** `executor::drives_forward` trả false với nó, như với execution đã kết thúc hoặc đang `KILLING`. Bước cập nhật của worker (`worker::render::render_updates`) cũng gác theo cùng hàm đó, nên hai nơi luôn khớp nhau. Các run đang sống vẫn tiếp tục chạy. Worker vẫn ghi trạng thái kết thúc và output của chúng. Executor không dùng chúng để tạo, bắt đầu hay hoàn tất run nào khác cho đến khi resume. Plan phải chứng minh bằng test rằng một run kết thúc trong lúc pause vẫn được ghi lại, còn run cha của nó không được hoàn tất và run anh em kế tiếp không được tạo.
  - *Đã sửa 2026-11-15, điều tra của plan (S4, S5):* còn hai nơi nữa phải dừng cùng executor. Các lượt render khác của worker (`render_run_if`, `render_expressions`, được gọi từ `claim::drive`) cũng dừng khi execution bị pause bằng tay, nên một `LoopUntil` có body đã kết thúc được hỏi điều kiện lúc resume, không phải trong lúc pause. Waker (`claim::due_retries`) không đánh thức một execution bị pause bằng tay, nên một run `RETRYING` đến hạn trong đó không đánh thức nó mỗi tick một cách vô ích. Một iteration `Loop` kết thúc vẫn bắt đầu iteration kế tiếp (Q3).
- **D3. Pause bằng tay.**
  - Route: `POST /api/ws/{ws}/workflows/executions/{id}/pause`, không có body.
  - Quyền: `workflow:manage`.
  - Trạng thái được nhận: `RUNNING` và `RESTARTED`. Mọi trạng thái khác trả 409 mã `not_running`.
  - *Đã sửa 2026-11-15 (Q5):* nó cũng bị từ chối với 409 `not_running` khi trạng thái cuối của bất kỳ run nào là `PAUSED`; thông báo nói rằng một `Pause` hay `LoopUntil` đã đang chờ và hãy resume task đó thay vào. §5 điểm khác 8.
  - `KILLING` cũng trả 409. Ở Kestra, `isRunning()` đúng với cả `KILLING` (`State.java:304`), nên Kestra cho pause một execution đang bị kill. Trong egret, `PAUSED` ghi sau `KILLING` sẽ chặn `advance_killing`. Xem §5 điểm khác 1.
  - Một iteration của loop bị từ chối với 400, không có mã; thông báo nêu loop gốc của nó để pause loop gốc thay thế, giống restart của 7a. Các iteration đang chạy vẫn tiếp tục. *Đã sửa 2026-11-15 (Q3):* pause loop gốc không ngừng tạo iteration mới: mỗi iteration kết thúc sẽ bắt đầu iteration kế tiếp, như ở Kestra (`worker/spawn.rs:735-746`; `LoopExecutionEventMessageHandler.java:94-182`). Run `Loop` hoàn tất khi iteration cuối kết thúc, và không có gì sau nó bắt đầu cho đến khi resume.
  - Thao tác chạy trong khoá execution (`claim::load`), có kiểm tra lại trạng thái (và, từ Q5, cả các run). Nó ghi một `SetExecutionState { PAUSED }` rồi gọi `settle`.
  - Trả 200 kèm `ExecutionDto`.
  - Audit: `workflow_execution:pause`, entity `workflow_execution`, payload `{from_state}`.
- **D4. `Pause.onResume`.**
  - `models::task::Pause` có thêm `on_resume: Option<Vec<Input>>`.
  - Mỗi phần tử được kiểm tra lúc publish bằng chính đoạn code kiểm tra `inputs:` của flow (kiểu, id, giá trị mặc định, `dependsOn`), và lỗi mang đường dẫn `tasks[..].onResume[i]`. *Đã sửa 2026-11-15, điều tra của plan (S8):* egret nêu tên input theo id trong đường dẫn finding, nên đường dẫn đọc là `tasks.<pause>.onResume.<input id>.<field>`.
  - `onResume` trong `afterExecution:` không thể xảy ra: `Pause` ở đó đã bị từ chối.
  - Các id phải khác nhau trong cùng một `onResume`. Chúng không cần khác id trong `inputs:` của flow, vì chúng nằm dưới `outputs.<pause>.onResume`.
  - Cảnh báo trong `keys.rs` và test của nó bị bỏ. Một test chứng minh `onResume` publish được mà không có cảnh báo.
- **D5. Resume nhận body tuỳ chọn.**
  - `POST …/resume` nhận: không có body, body JSON `{"inputs": {...}}`, hoặc `multipart/form-data` với mỗi input một part (tên part là id của input). Dạng này giống replay của 7b (`StartExecutionRequest`, `StartExecutionMultipart`).
  - **Pause bằng tay (D1):** gửi `inputs` khác rỗng thì trả 400 mã `resume_inputs_unexpected`. Ngược lại execution nhận `RUNNING`, lần `settle` kế tiếp đẩy nó tiếp, và payload audit ghi `by_hand: true`.
  - **Có một run `Pause` đang chờ:** run `Pause` `PAUSED` đầu tiên theo thứ tự run (không phải `LoopUntil`) là run mà `onResume` của nó được áp dụng, như `findFirstByState(PAUSED)` của Kestra.
    - Các giá trị gửi lên được giải quyết bằng `inputs_check::resolve` theo `onResume` của run đó, render theo execution như khi bắt đầu chạy. Khoá không khai báo trả 400. Giá trị bị từ chối trả 400, thông báo nêu tên input. *Đã sửa 2026-11-15, điều tra của plan (S6):* lúc bắt đầu chạy, một khoá JSON không khai báo bị bỏ đi (`inputs_check.rs:175-195`, test ở `:1541`) và chỉ part multipart không khai báo mới bị từ chối, nên "như lúc bắt đầu chạy" là sai. Resume từ chối cả hai, đó là quy tắc không làm rơi dữ liệu âm thầm.
    - Output của run đó nhận `{"onResume": {...}, "resumed": {"by": null, "on": <now ISO-8601>, "to": "SUCCESS"}}`.
    - Mọi run `Pause` `PAUSED` khác được giải phóng như hiện nay, chỉ có `resumed`. Chúng nhận `onResume: {}` để template không lỗi.
    - Execution chuyển sang `RESTARTED` (quy tắc `pause_mark` hiện có).
    - `by` để null, như Kestra OSS (`Pause.Resumed.now()`). Ai đã resume được ghi trong dòng audit.
    - *Thêm 2026-11-15 (Q6):* giá trị `onResume` kiểu SECRET là `******` trong `outputs` và được seal trong `task_runs.secret_outputs`; render surface tiết lộ nó cho các task sau và che nó trong log của chúng. Khi không có `secrets.master_key`, resume bị từ chối với 400 nêu tên input.
  - **`Pause` không có `onResume`** giữ hành vi hiện tại. Body rỗng hoặc không có body được nhận. `inputs` khác rỗng trả 400 `resume_inputs_unexpected`.
  - **Chỉ có `LoopUntil` đang chờ:** 409 "Nothing to resume" hiện nay giữ nguyên.
  - *Như đã xây, 2026-11-16 (D5):* resume có kèm inputs hoặc phần file khi chỉ có một `LoopUntil` đang chờ trả 400 `resume_inputs_unexpected`, không phải 409 (`routes.rs`, `resume_inputs_unexpected`); không có body thì vẫn trả 409 "Nothing to resume". Nói rộng hơn, inputs JSON không rỗng (và mọi phần multipart) khi không có `onResume` nào áp dụng bị từ chối ngay tại route, trước worker; worker từ chối lại lần nữa dưới lock.
  - *Như đã xây, 2026-11-16 (D5, D7, race):* worker trả `Moved` (409) khi cái nó thấy dưới lock khác cái route đã đọc: route chọn một run `Pause` nhưng execution nay đã bị pause thủ công, hoặc một run `Pause` khác nay là run đầu tiên đang chờ, hoặc route không thấy `Pause` nhưng nay có một `Pause` đang chờ và inputs hoặc file đã được gửi (`worker/pause.rs`, `ResumeRefused::Moved`). *Sửa 2026-11-16 (final review I1):* khi route không thấy `Pause` nhưng nay có một `Pause` đang chờ khai báo bất kỳ `onResume` nào, worker cũng trả `Moved`, kể cả với resume rỗng: route chưa hỏi cổng D7 cho `Pause` đó, và giải phóng nó bằng default của tác giả vẫn là một quyết định chỉ người tạo được đưa ra. Resume rỗng mà route không thấy `Pause` và `Pause` nay đang chờ không khai báo `onResume` thì vẫn giải phóng nó.
  - *Như đã xây, 2026-11-16 (audit):* bản ghi audit `workflow_execution:resume` mang `from_state`, `by_hand`, `task_run_id` (null khi pause thủ công) và `inputs_given: bool`, không bao giờ mang giá trị (có thể có SECRET trong đó).
- **D6. Input `onResume` kiểu FILE.**
  - Mỗi part file được stream vào storage dưới task prefix của run `Pause`, tại `tasks/{pause-task-slug}/{pause_run_id}/resume/{request_id}/{input}/{file}` (*đã sửa 2026-11-15, Q4*; spec ghi `resume/{task_run_id}/{input}/{file}`, mà `storage::uri` không parse được, `storage/uri.rs:291-317`). Đường dẫn này khác `inputs/{input}/{file}`, nên không ghi đè lên input lúc bắt đầu chạy có cùng id. `{request_id}` mới cho mỗi request, nên một resume bị từ chối chỉ xoá file của chính nó.
  - Các file được index trong `workflow.execution_files` dưới run `Pause`, trong transaction của resume; một file chỉ đọc được sau khi đã được index.
  - Giá trị lưu trong output là URI storage, như input lúc bắt đầu chạy.
  - Giới hạn kích thước và ngân sách header là của cổng multipart (`workflow.storage.max_file_bytes` + 1 MiB).
  - Nếu sau đó resume bị từ chối (lỗi kiểm tra, cổng D7, thua cuộc đua giành khoá), các file đã upload bị xoá (`discard_uploads`, được tổng quát hoá để nhận một prefix: chỉ prefix của request này).
  - Quy tắc đọc storage cho phép đọc các file đã được index của chính execution. Plan thêm test rằng một task sau và file API đọc được file đã lưu.
- **D7. Cổng người tạo `egret.sql` cho input (bảo mật).** Flow có thể chạy `egret.sql`, và task `Pause` đang chờ có thể khai báo `onResume` khác rỗng. Khi đó chỉ người tạo flow mới được resume (400, không có mã; thông báo nói rằng các task chạy dưới danh tính người tạo). Các giá trị gửi lên trở thành giá trị template mà câu SQL của người tạo đọc được; nếu không chặn, người có `workflow:manage` có thể định hình câu SQL chạy dưới danh tính của người khác. Đây là quy tắc D24 của 7a và 7b (`sql_principal::check_start`, door 2). Resume một `Pause` không có `onResume`, hay một execution bị pause bằng tay, không làm thay đổi những gì sẽ chạy, nên vẫn mở cho mọi người có `workflow:manage`, như hiện nay. Phép kiểm tra dùng source đã đóng băng của execution, vì đó là thứ sẽ chạy. Xem §5 điểm khác 2.
- **D8. Kiểm tra input resume.**
  - Route: `POST …/resume/validate`, cùng các dạng body như D5.
  - Quyền: `workflow:manage` (nó là một nửa của resume).
  - Execution phải ở `PAUSED` (nếu không thì 409 `not_paused`).
  - **Có một run `Pause` đang chờ:** trả về DTO kiểm tra input lúc bắt đầu chạy (response của `validate_execution_inputs`, `inputs_check::resolve_all`) cho `onResume` của run đó. Body rỗng trả về từng input với giá trị mặc định; đó là cách giao diện biết form gồm những ô nào (D11).
  - **Pause bằng tay, hoặc `Pause` không có `onResume`:** trả về danh sách input rỗng.
  - Các part file multipart được đọc rồi bỏ đi, không bao giờ được lưu, như `uploadFiles=false` của Kestra. Kết quả mang giá trị dự kiến, không phải URI.
  - Không ghi audit: nó không thay đổi gì.
  - *Như đã xây, 2026-11-16 (D8):* `resume/validate` chạy cùng phép từ chối như resume cho phần file gửi tới một input không phải `FILE` (400), qua `receive_parts` dùng chung với sink bỏ đi.
- **D9. Cổng theo flow.** Pause, resume và validate từ chối flow trong thùng rác hoặc đã xoá hẳn với 404, như 7a. Flow bị tắt không chặn pause hay resume: Kestra không kiểm tra điều nào, và dừng hay cho chạy nốt một run của flow bị tắt là an toàn. Xem §5 điểm khác 3.
- **D10. Tương tác.**
  - Kill một execution bị pause bằng tay hoạt động như với một execution đang chạy (`kill_leaves`). Các lá đang sống chuyển `KILLING`, và execution chuyển `KILLING`. `drives_forward` vốn đã dừng ở `KILLING`.
  - Restart của 7a cho một execution bị pause bằng tay theo nguyên các quy tắc của 7a. Nếu không có run `FAILED` hoặc `PAUSED`, nó trả 409 `nothing_to_restart`. Nếu có run đã fail trong lúc pause, nó reset run đó và execution chuyển `RESTARTED`, trạng thái này được đẩy tiếp. Đây là `ES.restart` của Kestra trên một execution `PAUSED`.
  - Bộ hẹn giờ `pauseDuration` hoặc `timeout` của một run `Pause` không thể kích hoạt trên execution bị pause bằng tay, vì pause bằng tay đòi `RUNNING` hoặc `RESTARTED` và không có run nào đang chờ. *Đã sửa 2026-11-15 (Q5):* lý do ban đầu ("khi đó không run nào ở `PAUSED`") là sai: hai `Pause` kết thúc vào các thời điểm khác nhau để lại một execution `RUNNING` với một run `PAUSED` (`executor/mod.rs:8365-8445`). Việc từ chối ở Q5 làm câu này đúng.
  - Bộ hẹn giờ SLA vẫn kích hoạt. Vi phạm SLA trên một execution đang pause xử lý như hiện nay với một execution `PAUSED`.

### 2.3 Giao diện, test, tài liệu (phần 2 đã duyệt)

- **D11. Giao diện** (trang execution, `docs/en/frontend-rules.md`):
  - Nút **Pause**, là một `ReasonButton` cạnh nút Kill. Nút hiện khi `RUNNING` và `RESTARTED`, bị khoá kèm lý do khi `KILLING`. Bấm vào mở hộp thoại xác nhận: "Runs that are already running finish; nothing new starts until you resume."
  - Hộp thoại **Resume** gọi `resume/validate` với body rỗng khi mở. Không có input thì giữ dạng xác nhận như hiện nay. Có input thì hiện form nhập input mà hộp thoại "Run flow" dùng, gửi JSON (multipart khi có input FILE), và hiện lỗi từng input lấy từ 400 hoặc từ validate.
  - Execution bị pause bằng tay hiện "Paused by hand" trong phần tổng quan, lấy từ trường mới `ExecutionDto.paused_by_hand: bool` do hàm ở D1 điền.
  - Client được sinh lại bằng `npm run gen:api`.
- **D12. Dự kiến không cần migration.** `PAUSED` đã là trạng thái hợp lệ của execution. CHECK của 0142 chỉ ràng buộc run `PAUSED`, mà pause bằng tay không ghi run nào như vậy. Output dùng output run hiện có. Plan kiểm chứng điều này. Nếu hoá ra sai, số migration lấy trên 0228 và trên mọi PR đang mở (memory: số migration là không gian chung).
- **D13. Tài liệu.**
  - `egret-data/docs`:
    - Executions → Run actions: thêm Pause, Resume kèm input và các mã từ chối mới.
    - Trang task `Pause`: thêm `onResume` và `outputs.<id>.onResume` / `resumed`.
    - Ảnh chụp mới.
  - Repo này:
    - spec này và plan, mỗi file có bản tiếng Việt dưới `docs/vi/`;
    - ghi chú sửa có ngày trong roadmap (dòng #7 và dòng #9) và trong spec inputs (`onResume` chuyển sang 7c).

## 3. Hình dạng thay đổi

- **`executor`:** `paused_by_hand`; `drives_forward` hiểu nó; `resume_by_hand_commands` trả về `SetExecutionState { RUNNING }` cho trường hợp D1; và `release_now` nhận thêm payload output cho run được chọn. *Đã sửa 2026-11-15, điều tra của plan (S9):* `ExecutionState` không giữ output, nên `release_now` giữ nguyên; một hàm thuần `executor::resumed_outputs` dựng payload và worker ghi nó trong transaction của resume.
- **`models::task::Pause`, `spec::validate`, `spec::keys`:** `onResume` (D4).
- **`worker::claim`:**
  - `pause_by_hand(pool, ws, id)`, hàm mới (D3);
  - `resume_by_hand(pool, id, inputs)`, nhận thêm input, ghi output và trả về trường hợp nào đã áp dụng (D5).
- **`routes.rs`:**
  - route `pause`;
  - body của `resume` (JSON hoặc multipart);
  - route `resume/validate`;
  - cổng người tạo `egret.sql` (D7);
  - các mã lỗi `not_running`, `not_paused`, `resume_inputs_unexpected`;
  - sự kiện audit;
  - OpenAPI.
- **`ExecutionDto.paused_by_hand`.**
- **Frontend:** `actions.ts` (`showsPause`, `canPause`), `ExecutionDetail.tsx` (nút và hộp thoại Pause, một `ResumeDialog` với form input) và client được sinh ra.

## 4. Test

- **Unit test executor:**
  - `paused_by_hand` với từng kiểu `PAUSED` (bằng tay, run `Pause`, `LoopUntil` đang chờ, nhiều pause song song);
  - `drives_forward` trả false với pause bằng tay;
  - một run kết thúc trong lúc pause không tạo run kế tiếp và không hoàn tất run cha;
  - resume bằng tay cho `RUNNING`, và lần `advance` kế tiếp tạo run kế tiếp;
  - giải phóng một `Pause` kèm input ghi `{onResume, resumed}` và execution chuyển `RESTARTED`.
- **Kiểm tra spec:**
  - `onResume` với từng kiểu input publish được mà không có cảnh báo;
  - kiểu sai, id trùng hoặc giá trị mặc định sai bị từ chối kèm đường dẫn `onResume[i]`;
  - test của cảnh báo bị bỏ được thay thế.
- **Test worker (DB):**
  - pause dưới khoá, và các cuộc đua: pause với kill, pause với một run đang hoàn tất;
  - một run hoàn tất trong lúc pause được ghi lại, và không có gì mới được tạo;
  - resume bằng tay;
  - resume kèm input ghi output;
  - kill trên pause bằng tay;
  - restart của 7a trên pause bằng tay (cả hai trường hợp ở D10).
- **Test route:**
  - 403 với chỉ `workflow:read`, cho pause, resume và validate;
  - 409 `not_running` (`PAUSED`, `KILLING`, đã kết thúc) và 409 `not_paused`;
  - 400 `resume_inputs_unexpected`;
  - 400 cho input bị từ chối, có nêu tên input;
  - resume multipart với FILE lưu dưới `resume/{run}/…`, và resume bị từ chối thì xoá file;
  - cổng D7: người không phải người tạo bị từ chối khi có `onResume` và được phép khi không có;
  - 404 cho flow trong thùng rác;
  - iteration của loop trả 400 nêu loop gốc;
  - pause khi một `Pause` hay `LoopUntil` đang chờ trả 409 `not_running` (Q5);
  - payload audit;
  - validate với body rỗng trả về giá trị mặc định;
  - validate với giá trị trả về lỗi từng input;
  - validate không lưu file nào.
- **Một test executor đầu-cuối:** flow `Pause` (`onResume`: STRING, INT, FILE) → `Log` đọc `outputs.p.onResume.*`. Resume gửi các giá trị, và dòng log mang đúng các giá trị đó.
- **Frontend (vitest):**
  - `showsPause` và `canPause`;
  - hộp thoại Resume không có input (xác nhận) và có input (form, validate khi mở, hiện lỗi);
  - hộp thoại Pause.
- **Live check**, với `egret serve` chạy native trên store dùng một lần: pause bằng tay khi một task chậm đang chạy (task chạy xong, không có gì mới bắt đầu), rồi resume (flow chạy tiếp); resume một `Pause` kèm input, gồm cả FILE; một input bị từ chối; chế độ sáng và tối.

## 5. Các điểm khác Kestra đã ghi nhận

1. **Không thể pause một execution đang `KILLING`** (D3). Kestra cho phép vì `isRunning()` bao gồm `KILLING`. Trong egret điều đó sẽ chặn thao tác kill.
2. **Bảo mật: cổng người tạo `egret.sql` cho input resume** (D7). Kestra OSS không có danh tính theo từng flow. egret chạy `egret.sql` dưới danh tính người tạo flow, nên input đi vào đó chỉ người tạo được gửi.
3. **Flow trong thùng rác hoặc đã xoá bị từ chối với 404** (D9). Đây là cổng mà 7a và 7b dùng. Flow bị tắt không bị từ chối, như Kestra.
4. **Pause và resume cần `workflow:manage`.** Kestra OSS không kiểm tra quyền nào ở server; giao diện của nó kiểm tra `EXECUTION:UPDATE`.
5. **Execution bị pause bằng tay trả 400 khi resume kèm input** (`resume_inputs_unexpected`). Kestra lặng lẽ bỏ qua chúng, vì không có `Pause` nào để kiểm tra theo. egret từ chối thay vì làm rơi dữ liệu (quy tắc không làm rơi dữ liệu âm thầm, `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md`).
6. **Input `onResume` kiểu FILE được lưu dưới run `Pause`, tại `tasks/{pause}/{run}/resume/{request_id}/{input}/{file}` (Q4).** Kestra lưu chúng cùng chỗ với input lúc bắt đầu chạy (`storageInterface.from(execution, inputId, …)`), nên một input resume có thể ghi đè input lúc bắt đầu chạy có cùng id.
7. **Giao diện đọc danh sách input từ `resume/validate`.** Giao diện Kestra đọc `task.inputs` của task đang dừng (`Resume.vue:72-81`), không phải `onResume`, và không bao giờ gọi `validateResume`. egret dùng câu trả lời của server, nên form khớp với những gì server kiểm tra.
8. **Pause bằng tay bị từ chối khi còn run nào đang chờ (Q5).** Kestra pause một execution mà `Pause` hay `LoopUntil` của nó đã đang chờ (`ES.pause` chỉ kiểm tra `isRunning()`), và bộ hẹn giờ của run đang chờ khi đó có thể kết thúc pause bằng tay. egret trả 409 `not_running`, nên D1 luôn đúng và một pause bằng tay không thể bị bộ hẹn giờ huỷ.

9. **Pause bằng tay được nhận từ `RESTARTED`** (D3; thêm 2026-11-16). `ES.pause` của Kestra cần `isRunning()`, vốn chỉ là `RUNNING` hoặc `KILLING` (`core/src/main/java/io/kestra/core/models/flows/State.java:304-306`). `RESTARTED` của egret là trạng thái đang chạy mà lần `advance` kế tiếp chuyển sang `RUNNING`, nên từ chối nó sẽ từ chối một pause trong khoảng ngay sau restart hay resume; hiệu ứng giống như pause muộn hơn một chút.

Ghi chú tương đương (không phải điểm khác), 2026-11-15:

- **Các iteration của loop vẫn tiếp tục bắt đầu dưới một loop gốc bị pause (Q3)**, như ở Kestra (`LoopExecutionEventMessageHandler.java:94-182`; egret `worker/spawn.rs:735-746`).
- **Giá trị `onResume` kiểu SECRET không bao giờ được lưu dạng rõ (Q6).** Kestra cũng mã hoá giá trị input SECRET.

## 6. Ngoài phạm vi

- Pause và resume theo danh sách id hoặc theo truy vấn: gói con 7f.
- Breakpoint và resume từ breakpoint: gói riêng (7b Q3).
- `behavior`, `onPause`, `errors` và `finally` của `Pause`: dòng #9.
- Resume một `Pause` khác run `PAUSED` đầu tiên (Kestra cũng không có tham số này).

## 7. Kết quả thực hiện

Xây dựng ngày 2026-11-15/16 trên `feat/workflow-pause-resume`, từ origin/main `7b19481b1`. Không có migration (D12 đúng; migration cao nhất vẫn là 0228).

- **Engine:** `executor::manual_pause` giữ các quy tắc thuần: điều kiện "pause bằng tay", các lệnh pause và resume, `Pause` đầu tiên đang chờ và payload output. `drives_forward` trả false với execution bị pause bằng tay. Worker chặn các bước render runIf/biểu thức (S4) và loại execution đó khỏi `due_retries` (S5).
- **Pause bằng tay:** `pause_by_hand` của worker chạy dưới khoá execution. Nó từ chối `TaskWaiting` khi còn run nào `PAUSED` (Q5).
- **Resume:** `resume_with` của worker nhận input `onResume`, ghi output `{onResume, resumed}`, niêm phong input SECRET, và lưu input FILE dưới run `Pause` kèm index `execution_files`. Nó trả `Moved` khi những gì nó thấy dưới khoá khác với những gì route đã đọc, kể cả mọi trường hợp `Pause` đang chờ có khai báo `onResume` mà route không đọc thấy `Pause` nào (review cuối, I1).
- **Route:**
  - `POST …/pause`, body của `resume` (JSON hoặc multipart) và `POST …/resume/validate`;
  - các mã `not_running`, `not_paused` và `resume_inputs_unexpected`;
  - sự kiện audit `workflow_execution:pause` (`{from_state}`) và `:resume` (`{from_state, by_hand, task_run_id, inputs_given}`); `inputs_given` là bool, nên không bao giờ ghi giá trị input nào;
  - `ExecutionDto.paused_by_hand`.
- **Frontend:**
  - nút và hộp thoại Pause;
  - một `ResumeDialog` đọc danh sách input một lần từ `resume/validate` và hiện form input của "Run flow";
  - nhãn "Paused by hand".
- **Cổng kiểm tra (2026-11-16):**
  - `cargo fmt --check` và clippy `-D warnings` sạch, `cargo check -p egret-bin` pass.
  - `cargo test -p egret-workflow`: mọi target pass trừ 7 test `sql_starrocks_live` (máy test không có engine StarRocks, như trên main).
  - egret-config, egret-server và egret-metastore: 179 pass.
  - Ratchet giữ 301 / 25 / 25 / 0 và census 295 / 25 / 21. Bảng phân loại thuộc tính đổi từ 1210 / 4 / 362 / 126 sang 1211 / 4 / 361 / 126 (`Pause.onResume` chuyển từ bị bỏ qua sang được hỗ trợ).
  - Frontend: lint 0 lỗi; typecheck, build và `test:bundle:workflows` pass. Vitest pass, chỉ trừ các lần hết thời gian chờ 5 s khi máy bận ở những file gói này không đụng tới (ScheduleDetail, TriggersList); cả hai file đều pass khi chạy riêng.
- **Kiểm tra thực tế (2026-11-16)**, thao tác bằng tay trên trình duyệt với `egret serve` chạy native, trên database và bucket RustFS dùng một lần, ở chế độ sáng và tối:
  - Pause bằng tay khi `Sleep PT25S` đang chạy: Sleep chạy xong, task kế tiếp không được tạo, execution giữ `PAUSED` kèm nhãn "Paused by hand".
  - Resume: task kế tiếp chạy và execution kết thúc `SUCCESS`. Validate được gửi với body `{}` và trả 200.
  - Resume một `Pause` có `onResume` (STRING bắt buộc, INT có mặc định, FILE): để trống giá trị bắt buộc thì form hiện lỗi. Sau đó output của `Pause` có `onResume` và `resumed`, file FILE được lưu dưới `tasks/approve/{run}/resume/{request}/note/note.txt`, và task `Log` kế tiếp in ra đúng các giá trị.
  - Các trường hợp từ chối:
    - INT sai kiểu và key không khai báo trả 400, nêu tên input;
    - pause một execution đang `PAUSED` trả 409 `not_running`;
    - validate trả lỗi theo từng input.
  - Các dòng audit không chứa giá trị input nào.

### Điểm lệch và lý do

- Các ghi chú có ngày "plan investigation" và "As built" ở trên ghi lại từng điểm: S1–S9, mã 400 của D5 khi chỉ có `LoopUntil`, quy tắc "moved" cho race, I1, và các điểm khác Kestra số 8 và 9 ở §5.

### Các vòng review

- **Review theo task:** một lần review sau mỗi task T1–T7.
  - T5 cần hai vòng sửa. Vòng đầu đóng lỗ hổng vượt D7 qua một resume JSON được đọc là pause bằng tay. Vòng hai sửa một test bị cũ đi do vòng đầu.
  - T4 cũng sửa một test thư viện về cổng render mà T1 đã làm đỏ.
- **Review cuối:** review toàn branch tìm ra 0 lỗi Critical và 2 lỗi Important (I1, "trường hợp giữ lại" của race D7; I2, chú thích FE-DATA-2) cùng các mục khuyên sửa. Tiếp theo là một đợt sửa gồm 4 commit, và lần review lại phần sửa không còn lỗi.

### Các phát hiện để sau

Đây là các lỗi nhỏ mà phần phân loại của review cuối cho phép để lại:
- `from_state` trong audit của pause được đọc trước khi lấy khoá;
- validate và resume trả lời một số body khác nhau (validate bỏ qua key JSON không khai báo);
- `due_retries` viết lại quy tắc "pause bằng tay" bằng SQL;
- chưa có test trên trang render cho `PAUSE_TASK_WAITING_REASON`;
- một test worker phụ thuộc đồng hồ;
- ba kiểm tra nhỏ trong `worker/pause.rs`: danh sách file rỗng ở nhánh pause bằng tay, run được chọn có nằm trong các run được giải phóng, và số dòng mà lệnh UPDATE output đã đổi.
