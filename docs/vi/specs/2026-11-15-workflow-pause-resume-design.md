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

### 2.2 Quyết định thiết kế (mặc định, 2026-11-15)

- **D1. "Pause bằng tay" là một sự kiện suy ra, không phải một cột mới.** Một execution bị pause bằng tay khi trạng thái cuối của nó là `PAUSED` và không run nào có trạng thái cuối là `PAUSED`. Đây chính là phép thử mà `ES.resume` của Kestra dùng (`findFirstByState(PAUSED)` rỗng, `ES:880-897`). Một task `Pause` hay một `LoopUntil` đang chờ luôn để lại một run `PAUSED`, nên hai trường hợp không thể lẫn nhau. Một hàm thuần duy nhất, `executor::paused_by_hand(&ExecutionState) -> bool`, giữ quy tắc này. Mọi nơi gọi (executor, route resume, `ExecutionDto`) đều dùng nó.
- **D2. Execution bị pause bằng tay không được đẩy tiếp.** `executor::drives_forward` trả false với nó, như với execution đã kết thúc hoặc đang `KILLING`. Bước cập nhật của worker (`worker::render::render_updates`) cũng gác theo cùng hàm đó, nên hai nơi luôn khớp nhau. Các run đang sống vẫn tiếp tục chạy. Worker vẫn ghi trạng thái kết thúc và output của chúng. Executor không dùng chúng để tạo, bắt đầu hay hoàn tất run nào khác cho đến khi resume. Plan phải chứng minh bằng test rằng một run kết thúc trong lúc pause vẫn được ghi lại, còn run cha của nó không được hoàn tất và run anh em kế tiếp không được tạo.
- **D3. Pause bằng tay.**
  - Route: `POST /api/ws/{ws}/workflows/executions/{id}/pause`, không có body.
  - Quyền: `workflow:manage`.
  - Trạng thái được nhận: `RUNNING` và `RESTARTED`. Mọi trạng thái khác trả 409 mã `not_running`.
  - `KILLING` cũng trả 409. Ở Kestra, `isRunning()` đúng với cả `KILLING` (`State.java:304`), nên Kestra cho pause một execution đang bị kill. Trong egret, `PAUSED` ghi sau `KILLING` sẽ chặn `advance_killing`. Xem §5 điểm khác 1.
  - Một iteration của loop bị từ chối với 400, không có mã; thông báo nêu loop gốc của nó để pause loop gốc thay thế, giống restart của 7a. Pause loop gốc sẽ ngừng tạo iteration mới. Các iteration đang chạy vẫn tiếp tục.
  - Thao tác chạy trong khoá execution (`claim::load`), có kiểm tra lại trạng thái. Nó ghi một `SetExecutionState { PAUSED }` rồi gọi `settle`.
  - Trả 200 kèm `ExecutionDto`.
  - Audit: `workflow_execution:pause`, entity `workflow_execution`, payload `{from_state}`.
- **D4. `Pause.onResume`.**
  - `models::task::Pause` có thêm `on_resume: Option<Vec<Input>>`.
  - Mỗi phần tử được kiểm tra lúc publish bằng chính đoạn code kiểm tra `inputs:` của flow (kiểu, id, giá trị mặc định, `dependsOn`), và lỗi mang đường dẫn `tasks[..].onResume[i]`.
  - `onResume` trong `afterExecution:` không thể xảy ra: `Pause` ở đó đã bị từ chối.
  - Các id phải khác nhau trong cùng một `onResume`. Chúng không cần khác id trong `inputs:` của flow, vì chúng nằm dưới `outputs.<pause>.onResume`.
  - Cảnh báo trong `keys.rs` và test của nó bị bỏ. Một test chứng minh `onResume` publish được mà không có cảnh báo.
- **D5. Resume nhận body tuỳ chọn.**
  - `POST …/resume` nhận: không có body, body JSON `{"inputs": {...}}`, hoặc `multipart/form-data` với mỗi input một part (tên part là id của input). Dạng này giống replay của 7b (`StartExecutionRequest`, `StartExecutionMultipart`).
  - **Pause bằng tay (D1):** gửi `inputs` khác rỗng thì trả 400 mã `resume_inputs_unexpected`. Ngược lại execution nhận `RUNNING`, lần `settle` kế tiếp đẩy nó tiếp, và payload audit ghi `by_hand: true`.
  - **Có một run `Pause` đang chờ:** run `Pause` `PAUSED` đầu tiên theo thứ tự run (không phải `LoopUntil`) là run mà `onResume` của nó được áp dụng, như `findFirstByState(PAUSED)` của Kestra.
    - Các giá trị gửi lên được giải quyết bằng `inputs_check::resolve` theo `onResume` của run đó, render theo execution như khi bắt đầu chạy. Khoá không khai báo trả 400, như lúc bắt đầu chạy. Giá trị bị từ chối trả 400, thông báo nêu tên input.
    - Output của run đó nhận `{"onResume": {...}, "resumed": {"by": null, "on": <now ISO-8601>, "to": "SUCCESS"}}`.
    - Mọi run `Pause` `PAUSED` khác được giải phóng như hiện nay, chỉ có `resumed`. Chúng nhận `onResume: {}` để template không lỗi.
    - Execution chuyển sang `RESTARTED` (quy tắc `pause_mark` hiện có).
    - `by` để null, như Kestra OSS (`Pause.Resumed.now()`). Ai đã resume được ghi trong dòng audit.
  - **`Pause` không có `onResume`** giữ hành vi hiện tại. Body rỗng hoặc không có body được nhận. `inputs` khác rỗng trả 400 `resume_inputs_unexpected`.
  - **Chỉ có `LoopUntil` đang chờ:** 409 "Nothing to resume" hiện nay giữ nguyên.
- **D6. Input `onResume` kiểu FILE.**
  - Mỗi part file được stream vào storage dưới prefix của execution, tại `resume/{task_run_id}/{input}/{file}`. Đường dẫn này khác `inputs/{input}/{file}`, nên không ghi đè lên input lúc bắt đầu chạy có cùng id.
  - Giá trị lưu trong output là URI storage, như input lúc bắt đầu chạy.
  - Giới hạn kích thước và ngân sách header là của cổng multipart (`workflow.storage.max_file_bytes` + 1 MiB).
  - Nếu sau đó resume bị từ chối (lỗi kiểm tra, cổng D7, thua cuộc đua giành khoá), các file đã upload bị xoá (`discard_uploads`, được tổng quát hoá để nhận một prefix).
  - Quy tắc đọc storage đã cho phép đọc dưới prefix riêng của execution. Plan xác nhận `resume/` được bao phủ và thêm test.
- **D7. Cổng người tạo `egret.sql` cho input (bảo mật).** Flow có thể chạy `egret.sql`, và task `Pause` đang chờ có thể khai báo `onResume` khác rỗng. Khi đó chỉ người tạo flow mới được resume (400, không có mã; thông báo nói rằng các task chạy dưới danh tính người tạo). Các giá trị gửi lên trở thành giá trị template mà câu SQL của người tạo đọc được; nếu không chặn, người có `workflow:manage` có thể định hình câu SQL chạy dưới danh tính của người khác. Đây là quy tắc D24 của 7a và 7b (`sql_principal::check_start`, door 2). Resume một `Pause` không có `onResume`, hay một execution bị pause bằng tay, không làm thay đổi những gì sẽ chạy, nên vẫn mở cho mọi người có `workflow:manage`, như hiện nay. Phép kiểm tra dùng source đã đóng băng của execution, vì đó là thứ sẽ chạy. Xem §5 điểm khác 2.
- **D8. Kiểm tra input resume.**
  - Route: `POST …/resume/validate`, cùng các dạng body như D5.
  - Quyền: `workflow:manage` (nó là một nửa của resume).
  - Execution phải ở `PAUSED` (nếu không thì 409 `not_paused`).
  - **Có một run `Pause` đang chờ:** trả về DTO kiểm tra input lúc bắt đầu chạy (response của `validate_execution_inputs`, `inputs_check::resolve_all`) cho `onResume` của run đó. Body rỗng trả về từng input với giá trị mặc định; đó là cách giao diện biết form gồm những ô nào (D11).
  - **Pause bằng tay, hoặc `Pause` không có `onResume`:** trả về danh sách input rỗng.
  - Các part file multipart được đọc rồi bỏ đi, không bao giờ được lưu, như `uploadFiles=false` của Kestra. Kết quả mang giá trị dự kiến, không phải URI.
  - Không ghi audit: nó không thay đổi gì.
- **D9. Cổng theo flow.** Pause, resume và validate từ chối flow trong thùng rác hoặc đã xoá hẳn với 404, như 7a. Flow bị tắt không chặn pause hay resume: Kestra không kiểm tra điều nào, và dừng hay cho chạy nốt một run của flow bị tắt là an toàn. Xem §5 điểm khác 3.
- **D10. Tương tác.**
  - Kill một execution bị pause bằng tay hoạt động như với một execution đang chạy (`kill_leaves`). Các lá đang sống chuyển `KILLING`, và execution chuyển `KILLING`. `drives_forward` vốn đã dừng ở `KILLING`.
  - Restart của 7a cho một execution bị pause bằng tay theo nguyên các quy tắc của 7a. Nếu không có run `FAILED` hoặc `PAUSED`, nó trả 409 `nothing_to_restart`. Nếu có run đã fail trong lúc pause, nó reset run đó và execution chuyển `RESTARTED`, trạng thái này được đẩy tiếp. Đây là `ES.restart` của Kestra trên một execution `PAUSED`.
  - Bộ hẹn giờ `pauseDuration` hoặc `timeout` của một run `Pause` không thể kích hoạt trên execution bị pause bằng tay, vì pause bằng tay đòi `RUNNING` hoặc `RESTARTED`, và khi đó không run nào ở `PAUSED`.
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

- **`executor`:** `paused_by_hand`; `drives_forward` hiểu nó; `resume_by_hand_commands` trả về `SetExecutionState { RUNNING }` cho trường hợp D1; và `release_now` nhận thêm payload output cho run được chọn.
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
6. **Input `onResume` kiểu FILE được lưu dưới `resume/{task_run_id}/`.** Kestra lưu chúng cùng chỗ với input lúc bắt đầu chạy (`storageInterface.from(execution, inputId, …)`), nên một input resume có thể ghi đè input lúc bắt đầu chạy có cùng id.
7. **Giao diện đọc danh sách input từ `resume/validate`.** Giao diện Kestra đọc `task.inputs` của task đang dừng (`Resume.vue:72-81`), không phải `onResume`, và không bao giờ gọi `validateResume`. egret dùng câu trả lời của server, nên form khớp với những gì server kiểm tra.

## 6. Ngoài phạm vi

- Pause và resume theo danh sách id hoặc theo truy vấn: gói con 7f.
- Breakpoint và resume từ breakpoint: gói riêng (7b Q3).
- `behavior`, `onPause`, `errors` và `finally` của `Pause`: dòng #9.
- Resume một `Pause` khác run `PAUSED` đầu tiên (Kestra cũng không có tham số này).

## 7. Kết quả thực hiện

(Điền vào khi hoàn tất phần cài đặt.)
