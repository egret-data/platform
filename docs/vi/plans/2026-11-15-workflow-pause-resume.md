# Workflow package 7c: Pause thủ công, resume với input onResume: kế hoạch triển khai

> Bản dịch tiếng Việt của docs/en/plans/2026-11-15-workflow-pause-resume.md. Khi hai bản khác nhau, bản tiếng Anh là chuẩn.

> **Dành cho agentic worker:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để thực hiện plan này từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** một người có `workflow:manage` có thể Pause thủ công một execution đang chạy (các run đang chạy thì chạy xong, không có gì mới bắt đầu), resume nó, và resume một task `Pause` với input `onResume` có kiểu mà flow đọc là `outputs.<pause>.onResume.<id>`. Một route `resume/validate` kiểm tra các input đó mà không resume. Ngữ nghĩa là của Kestra (`ES.pause` `ES:907-916`, `ES.resume` `ES:880-897`, `EC:1854-1906`, `Pause.java:193`, `:305-330`), trừ những chỗ §5 của spec và §"Spec corrections" bên dưới ghi nhận là khác biệt.

**Kiến trúc:**
- Một module thuần, `executor::manual_pause`, nắm quy tắc của D1 (`paused_by_hand`), các state được chấp nhận của D3, lệnh resume-by-hand, việc chọn `Pause` đang chờ đầu tiên, và payload `{onResume, resumed}`. `executor::drives_forward` là false với một execution bị Pause thủ công, nên `advance` và lượt update của worker dừng cùng nhau (D2).
- `models::Pause` nhận thêm `on_resume`. Publish kiểm tra từng entry bằng các kiểm tra `inputs:` của flow, dưới path `tasks.<pause>.onResume.<id>`. Warning `not_honoured` bị bỏ, và bảng property-treatment được đo lại.
- Một module worker mới, `worker/pause.rs`, giữ `pause_by_hand` và `resume_with`. Cả hai chạy bên trong lock của execution (`claim::load`) và kết thúc bằng `claim::settle`. `resume_with` resolve các giá trị được post bằng kiểm tra input của start đối với `onResume` của `Pause` được chọn. Nó ghi `{onResume, resumed}` vào `outputs` của run đó (giá trị SECRET được seal vào `secret_outputs`), index các file đã upload dưới run `Pause`, giải phóng các pause như hiện nay, và settle.
- `claim::drive` ngừng render khi execution bị Pause thủ công, và waker ngừng đánh thức nó.
- Route: `POST …/pause`; `POST …/resume` với body JSON hoặc multipart tuỳ chọn; `POST …/resume/validate`. Cũng có các code từ chối, cổng creator D7, cổng trash D9, audit, OpenAPI, và `ExecutionDto.paused_by_hand`.
- Frontend: một nút và dialog Pause, một `ResumeDialog` với form input Run-flow, và một ghi chú "Paused by hand" ở header.

**Tech stack:** Rust (axum, sqlx 0.9, Postgres, utoipa) trong `crates/egret-workflow`; React 18 + TypeScript + client orval + vitest trong `frontend/`.

Spec: `docs/en/specs/2026-11-15-workflow-pause-resume-design.md`. Q1, Q2 và D1–D13 của nó có tính ràng buộc. Chỗ nào plan này thấy một giả định của spec là sai, nó nói rõ trong "Spec corrections" và **không** âm thầm đổi quyết định. Các mục S1, S2, S3 và S7 phải được trình người dùng trước khi T3 merge. Nhánh `feat/workflow-pause-resume`, một PR, base `origin/main` `7b19481b1` (commit spec `940b0f5fb`).

Các task: T0 (baseline), T1–T5 (engine và API), T6 (client), T7 (frontend), T8 (docs trong repo này). Repo docs người dùng và kiểm tra trực tiếp thuộc về controller (§"Sau các task").

**Review không phải là một task của plan này.** Controller review từng task giữa các task và chạy một lần review toàn nhánh sau T8. Các bản sửa được ghi trong §7 "As built" của spec.

---

## Spec corrections

Mỗi mục nói giả định của spec sai hoặc thiếu ở đâu, đưa bằng chứng, và nêu cách sửa hẹp nhất. Plan thực hiện dòng được đánh dấu **Plan**. Các mục đánh dấu **USER** cần người dùng xác nhận; các mục còn lại chỉ để thông báo.

- **S1 (D3: "Pausing the loop root stops it starting new iterations") — USER.**
  - Trong egret, một `Loop` bắt đầu iteration kế tiếp của nó từ worker, không phải từ `advance`. Khi một iteration kết thúc, `spawn::on_execution_terminal_inner` nạp loop root dưới lock của nó và, với `Next::Start`, gọi thẳng `spawn_one` (`worker/spawn.rs:735-746`). Nó không bao giờ hỏi `drives_forward`. Chỉ batch đầu tiên đi qua `advance` (`executor/flowable/loop_.rs:21-36`, "only the FIRST batch is spawned").
  - Kestra làm như vậy. `LoopExecutionEventMessageHandler.handleTerminated` (`executor/.../handler/LoopExecutionEventMessageHandler.java:94-182`) phát ra `loopExecution` kế tiếp dưới lock của parent, bất kể state của parent. Chỉ `ExecutorService.process` kiểm tra `canBeProcessed()` (`ExecutorService.java:171`, `ExecutorContext.java:67-70`), và handler này không đi qua nó.
  - **Plan:** theo Kestra. Khi loop root đang bị Pause thủ công, mỗi iteration kết thúc sẽ bắt đầu iteration kế tiếp, cho tới hết các giá trị của loop. Run `Loop` hoàn thành khi iteration cuối kết thúc. Không có gì sau `Loop` bắt đầu cho tới khi resume. Test 6 của T3 ghim điều này.
  - Nếu người dùng muốn đúng câu của spec: chặn `Next::Start` trong `on_execution_terminal_inner` bằng `executor::paused_by_hand(&state)`, và cho nhánh by-hand của `resume_with` bắt đầu `min(concurrencyLimit − running, remaining)` iteration cho mỗi run `Loop` đang `RUNNING`. Đó là thêm một task với test riêng, và là một điểm khác biệt được ghi nhận so với Kestra.
- **S2 (D6: nơi lưu các input FILE của `onResume`) — USER.**
  - `resume/{task_run_id}/{input}/{file}` không phải là URI file của execution. `storage::uri::execution_file` chỉ nhận `inputs/…`, `tasks/…`, `trigger/…` hoặc một tên file đơn (`storage/uri.rs:291-317`); bất kỳ thứ gì khác là "`'resume' is not an execution file tail`". Một URI không parse được là `Unreadable::Malformed` trong `storage::index::resolve_readable_many` (`storage/index.rs:228-250`). Vì vậy không task nào về sau và không lượt tải xuống nào từ UI đọc được file.
  - Một file đọc được còn phải có một dòng trong `workflow.execution_files` (`index.rs:258-283`), mà spec không lên kế hoạch.
  - **Plan:**
    - Lưu mỗi file tại `tasks/{pause-task-slug}/{pause_run_id}/resume/{request_id}/{input}/{file}`. Đó là một URI `Tail::Task` dưới task prefix riêng của run `Pause` (`uri::task_prefix`, `uri.rs:393-418`). Nó không thể ghi đè một input của start, đúng như D6 muốn.
    - Index các file bằng `storage::index_task_files(conn, ws, pause_run_id, files)` bên trong transaction của resume (`storage/mod.rs:390-435`).
    - Đoạn `{request_id}` (một UUID mới cho mỗi request) nghĩa là một resume bị từ chối hoặc thua race chỉ xoá file của chính nó, không bao giờ xoá file mà một resume thắng đồng thời đã lưu.
    - Điểm khác biệt số 6 ở §5 của spec được viết lại ở T8.
- **S3 (D10: "a `Pause` run's timer cannot fire on a paused-by-hand execution, because … no run is then `PAUSED`") — USER.**
  - Điều này sai với hai `Pause` kết thúc vào các thời điểm khác nhau. Trong `two_pauses_in_one_parallel_resume_independently` (`executor/mod.rs:8365-8445`), sau khi `quick` kết thúc, execution đọc là `… PAUSED RESTARTED RUNNING` trong khi `slow` vẫn `PAUSED` với một deadline. Khi đó một Pause thủ công được chấp nhận (state là `RUNNING`), và D1 phân loại execution là **không** bị Pause thủ công, vì có một run `PAUSED`.
  - Điều xảy ra tiếp theo: timer của `slow` nổ, `pause_mark` chuyển execution sang `RESTARTED` rồi `RUNNING` (`executor/mod.rs:387-424`), và Pause thủ công kết thúc. Một resume trước đó sẽ giải phóng `slow`, cùng với `onResume` của `slow`.
  - Kestra cũng vậy: `ES.resume` tìm run `PAUSED` và resume nó.
  - **Plan:** không đổi code. Test 9 của T3 ghim hành vi này, và docs người dùng nêu nó. Phương án thay thế, nếu người dùng thích: từ chối Pause thủ công bằng 409 `not_running` khi bất kỳ run nào đang `PAUSED`.
- **S4 (D2: còn gì khác làm một execution đang Pause chuyển động) — inform.**
  - `claim::drive` (`worker/claim.rs:5234-5310`) chạy `render::render_run_if` (`render.rs:1718-1751`) và `render::render_expressions` (`render.rs:1973-…`) giữa các vòng `advance`, và cả hai đều không bị chặn. Chỉ `render_updates` bị chặn (`render.rs:1853`).
  - Dưới một Pause thủ công, một `LoopUntil` mà body đã kết thúc sẽ bị render điều kiện ngay lúc Pause. Một lỗi render sẽ làm nó fail (`render.rs` `fail`), điều này làm một run chuyển state.
  - **Plan:** `drive` return trước ba lượt render khi `executor::paused_by_hand(state)` là true (T3). Điều này nằm trong D2: executor và render surface dừng cùng nhau.
- **S5 (D2: waker) — inform.**
  - `claim::due_retries` (`claim.rs:790-840`) đánh thức một execution có một run `RETRYING` đến hạn trừ khi nó terminal hoặc `KILLING`. Một execution bị Pause thủ công có thể giữ một run như vậy: một run fail trong lúc Pause, hoặc retry của một sibling song song. Khi đó `advance` không trả lời gì (D2), và waker sẽ đánh thức execution mỗi tick một cách vô ích.
  - **Plan:** `due_retries` loại trừ một execution `PAUSED` không có run `PAUSED` (T3, kèm một test).
- **S6 (D5: "Undeclared keys give 400, as on start") — inform.**
  - Start **không** từ chối một key JSON không khai báo; nó bỏ đi (`inputs_check.rs:175-195`, test `undeclared_keys_are_dropped_and_order_decides_the_first_error`, `inputs_check.rs:1541`). Chỉ một part multipart không nêu input nào đã khai báo mới bị từ chối (`routes.rs:3099-3103`).
  - **Plan:** giữ 400 của D5 cho resume, cho cả JSON và multipart, với message nêu tên key. Cụm "as on start" là sai. Đây là quy tắc không bỏ âm thầm.
- **S7 (input SECRET của `onResume`; spec không nói) — USER (xác nhận).**
  - Corpus khai báo một input như vậy (`oracle-exec/corpus/valids/pause_on_resume.yaml`, `secret_pause: SECRET`). Ghi nó vào `task_runs.outputs` sẽ lưu một secret dạng văn bản rõ và hiển thị nó trong mọi `ExecutionDto`.
  - **Plan:**
    - Che nó thành `******` trong `outputs`.
    - Seal các giá trị rõ vào `task_runs.secret_outputs` bằng `secret_inputs::split_outputs` (`secret_inputs.rs:337-363`), cùng cách seal mà output SECRET đã dùng (migration 0214).
    - Reveal chúng trong render surface bằng một nhánh `onResume` mới trong `render::reveal_run_outputs` (`render.rs:2149-2167`), để một task về sau đọc được giá trị rõ và log của nó che đi.
    - Khi không có `secrets.master_key`, resume bị từ chối bằng 400 nêu tên input, như một input start kiểu SECRET.

    Phương án thay thế là từ chối SECRET trong `onResume` lúc publish. Điều đó sẽ làm dịch chuyển census corpus (`pause_on_resume` thôi publish) và khác với Kestra.
- **S8 (D4: đường dẫn `tasks[..].onResume[i]`) — inform.** Các path finding của egret nêu tên input theo id: `inputs.<id>.<field>` (`spec/validate.rs:1418-1531`). Vì vậy các finding của `onResume` đọc là `tasks.<pause>.onResume.<id>.<field>` (`<pause>` là path có dấu chấm của `spec::path::task_path`). Cùng ý định, cách viết của egret.
- **S9 (§3: "`release_now` gains the outputs payload") — inform.** `ExecutionState` không giữ outputs, và writer không bao giờ đụng `outputs` (`claim.rs` `persist`). `release_now` (`executor/flowable/pause.rs:94-109`) giữ nguyên. Payload là hàm thuần `executor::resumed_outputs`, do worker ghi bằng một `UPDATE` trong transaction của resume, trước settle.

---

## Investigation record (những sự kiện mà implementer không được tự suy lại)

### Kestra (`/Users/1pro-l4r8/Documents/github/data-portal/kestra`, chỉ đọc)

- `ExecutorContext.canBeProcessed()` là false với `isPaused()` (`executor/src/main/java/io/kestra/executor/ExecutorContext.java:67-70`), được kiểm tra trong `ExecutorService.process` (`ExecutorService.java:171`).
- Các iteration của loop được bắt đầu bởi `LoopExecutionEventMessageHandler.handleTerminated` (`:94-182`), không bao giờ hỏi state của parent (S1).
- Phần còn lại được trích dẫn ở spec §1 và §2.

### egret

- **Cổng chặn của executor.**
  - `advance` (`executor/mod.rs:191-213`): khi `!drives_forward(state)` nó trả `advance_killing` cho `KILLING`, `after_execution::drive` cho state terminal, và `Vec::new()` cho mọi thứ khác. Một `PAUSED` thủ công rơi vào nhánh cuối. Không gì bên dưới chạy: không `reopen_due`, không walk, không `retry::scan`, không `pause_mark`.
  - `drives_forward` ở `executor/mod.rs:366-371`. Nơi gọi nó là `advance` và `render::render_updates` (`render.rs:1853`, cùng với `drives_after`).
- **Worker vẫn làm gì dưới một Pause thủ công (D2).**
  - Một leaf chỉ được claim khi nó `RUNNING` (`claim::claim`, `claim.rs:515-555`), nên một run đã `RUNNING` vẫn được claim và chạy.
  - Kết quả của nó được ghi bởi `apply_outcome_on` (`claim.rs:2918-3035`): `UPDATE … SET outputs, message`, rồi `SetRunState`/`CompleteRun`, rồi `settle`, mà `advance` của nó không tạo gì.
  - Một child `Subflow` kết thúc hoàn thành run của parent qua `subflow::on_child_terminal` → `Job::Complete` → `subflow::open_completion`, ghi terminal của run và settle. Không gì khác chuyển động.
  - Một iteration `Loop` kết thúc sẽ bắt đầu iteration kế tiếp (S1).
  - Kill: `worker/kill.rs:191-245` nạp bất kỳ execution non-terminal nào và áp dụng `executor::kill::kill_leaves` (`executor/kill.rs:226-247`); rồi `advance_killing` (`:79-87`) đóng các flowable. Điều này hoạt động trên `PAUSED` như trên `RUNNING`.
- **D1 đứng vững.**
  - `Command::PauseRun` là nguồn duy nhất tạo run `PAUSED`, và `pause_mark` đưa execution vào `PAUSED` chỉ trong batch có một `PauseRun` (`executor/mod.rs:387-424`).
  - Execution rời `PAUSED` trong batch kết thúc run đang chờ cuối cùng: `SetRunState` trên run `PAUSED` cho `RESTARTED`, và `RestartLoop` cho `RUNNING`.
  - Restart của 7a trên một execution `PAUSED` reset các run `PAUSED` và ghi `RESTARTED` (`executor/restart.rs:68-116`).
  - Vậy một execution `PAUSED` không có run `PAUSED` chỉ đạt được bằng Pause thủ công mới. Chiều ngược lại, một execution `RUNNING` có run `PAUSED`, là S3.
- **Resume hiện nay.**
  - `claim::resume_by_hand` (`claim.rs:4535-4552`) nạp dưới lock và trả `NotPaused` trừ khi state cuối là `PAUSED`. Nó gọi `executor::release_paused_pauses` (= `flowable::pause::release_now`, re-export `executor/mod.rs:100`), trả `NoPauseWaiting` khi không giải phóng gì, và nếu không thì settle.
  - `release_now` đánh dấu mọi run `PAUSED` có `resume_with` là `Never` hoặc `Terminal` (một `Pause`, không bao giờ là `Iterate` của `LoopUntil`) kết thúc `SUCCESS` ngay.
  - Route là `routes.rs:3198-3276`. Dòng router của nó là `routes.rs:4887-4890`, path OpenAPI là `routes.rs:4943`, và audit của nó là `workflow_execution:resume` `{from_state}` (`routes.rs:3261-3268`).
- **Output của run.**
  - `workflow.task_runs.outputs` (jsonb), `secret_outputs` (bytea, migration 0214), `outputs_bytes` (bigint, chỉ `Loop`).
  - Render surface đọc `outputs` và `secret_outputs` của mọi run theo thứ tự run (`render.rs:594-620`), bất kể state của run, qua `reveal_run_outputs` (`render.rs:2149-2167`). Hàm đó biết shape `Subflow` (`outputs.outputs`) và shape `Loop`.
- **Kiểm tra input.**
  - `claim::checked_inputs` (`claim.rs:4243-4249`) là `inputs_check::resolve(&flow.inputs, posted, &start_renderer(flow))`.
  - `claim::input_outcomes` (`:4255-4260`) là `resolve_all`.
  - `start_renderer` (`:4263-4295`) bind `flow`, `vars` và `inputs`, với `null` cho mỗi input có default của `flow.inputs`.
  - Re-export `pub(crate)` ở `worker/mod.rs:118`.
- **Input FILE.**
  - `storage::inputs::upload_input` (`storage/inputs.rs:97-167`) kiểm tra kiểu, tên và phần mở rộng, dựng `uri::input_uri`, stream, và head object.
  - `check_file_inputs` (`:240-…`) chỉ đọc `flow.inputs`.
  - `FileStore::execution_prefix` là `storage/mod.rs:255-270`.
  - Các route: `receive_parts` (`routes.rs:3006-3051`), `discard_uploads` (`:3054-3066`, xoá toàn bộ execution prefix), `read_parts` (`:3069-3137`, "the flow declares no input" ở `:3099-3103`).
- **Body JSON.** `read_json_body` (`routes.rs:2593-2640`: 2 MiB, một body không có header được đọc như JSON) và `start_inputs` (`:2507-2524`).
- **Validate.**
  - `validate_execution_inputs` (`routes.rs:2905-2930`) trả `ValidateInputsDto` (`:2792-2803`) do `validate_inputs_answer` (`:2828-2880`) dựng.
  - `declared_inputs(source)` (`:2807-2824`) echo các object input của chính YAML.
- **Các gate.**
  - `startable_flow` (`routes.rs:2431-2458`): 404 trash, 400 bản nháp không parse được, 409 disabled.
  - `rerun_gate` (`:2476-2504`) chạy nó cộng `sql_principal::check_start` (`worker/sql_principal.rs:90-113`, cửa 2: với `started_by = Some(caller)`, chỉ creator qua được trên một flow chạy `egret.sql`; `runs_sql` quyết định điều đó).
  - Một iteration loop: `ExecOpError::loop_iteration(id, root, what)` (`routes.rs:3873-3878`), được restart của 7a dùng (`routes.rs:4196-4201`).
  - `ExecOpError` là `routes.rs:3854-4033`.
- **Model `Pause` và chỗ thiếu của nó.**
  - `models::Pause` là `models/task.rs:739-761`.
  - Warning đến từ `spec/keys.rs:171` (`lift_of` → `Package(9)`). Các test của nó: `keys.rs:1344-1347`, `:1539-1556`, `:1895-1914`.
  - Block `Loop.outputs` của key walker (`keys.rs:984-997`) là mẫu để đi các entry `onResume` bằng `input(...)` (`keys.rs:897-935`, có path hard-code `inputs.{id}`).
  - `executor::pause_gap`/`pause_plan_gap` (`executor/plan.rs:920-990`) chỉ nêu `tasks`, `errors`, `finally`, `behavior` và `onPause`. `onResume` chưa bao giờ là một gap, chỉ là một warning.
  - Nhánh `Pause` của `spec::validate` là `validate.rs:2302-2338`. Các kiểm tra input của flow là `validate_inputs` (`validate.rs:1381-1470`, run gate, gọi ở `:791`), `input_save_findings` (`:1499-1541`, chỉ lúc save, gọi ở `:398`) và `input_dependency_findings` (`:1543-1583`). Cả ba ghi path `inputs.{id}`.
  - `spec::path::every_task` (`spec/path.rs:69`) và `find_task` (`:49`) đi cả bốn list gốc ở mọi độ sâu.
- **Property treatment.**
  - `spec/schema/property-treatment.json:648` đọc là `"onResume": "ignored"`. File này được đo bởi `spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today` và được ghi lại bằng `EGRET_WRITE_PROPERTY_TREATMENT=1` (`measure.rs:1-7`).
  - Một property của struct model có kiểu của một flowable nằm trong read set, nên thêm field làm nó thành `honoured` (`measure.rs:19-27`).
  - `spec/schema/README.md:214` (số đếm) và `:230` (dòng `Pause`) nhắc lại điều đó.
- **Census và ratchet.**
  - `oracle-exec/driver.rs:854` `const LIVE: usize = 295`.
  - Ratchet `(301, 276, 25)` at `tests/executor.rs:889`.
  - Ba flow corpus `onResume` (`valids/pause_on_resume.yaml`, `pause_on_resume_optional.yaml`, `resume-validate.yaml`) publish hôm nay, kèm một warning. Entry của chúng là các pause không giới hạn không bao giờ tới golden. Vì vậy không số đếm nào dự kiến thay đổi, nhưng T0 và T2 **đo**; chúng không giả định.
- **Migration.**
  - Không cần migration. `PAUSED` là một state execution hợp lệ. CHECK của 0142 (`state <> 'PAUSED' OR resume_with IS NOT NULL`) chỉ ràng buộc các **run** `PAUSED`, và một Pause thủ công không ghi run. Output dùng các cột sẵn có.
  - Migration trên cùng là `0228_compile_run_migration_complete.sql`. Nếu một migration trở nên cần thiết, hãy báo và đánh số trên main và mọi PR đang mở (memory: số migration là một namespace dùng chung).
- **OpenAPI.**
  - `crates/egret-server/src/lib.rs:855-866` `committed_openapi_json_is_current`, có message là "run `egret openapi > openapi.json`": `cargo run -p egret-bin -- openapi > openapi.json`.
  - `generated_client_was_emitted_from_the_committed_spec` (`lib.rs:~940-990`) đọc `frontend/src/api/generated.digest.json`, do `npm run gen:api` ghi.
  - Operation id là tên handler (`openapi.json:33819` `"resume_execution"`). orval thêm tag làm tiền tố: `useWorkflowResumeExecution`, `getWorkflowResumeExecutionUrl`.
- **Frontend.**
  - `components/workflows/executions/actions.ts`: `showsResume`/`canResume` `:19-44`, `isLoopIteration` `:193-195`.
  - `routes/workflows/ExecutionDetail.tsx`:
    - `ExecutionActions` `:366-542`;
    - `ConfirmAction` của Resume `:489-497`;
    - `runResume` `:391-400`;
    - slot `status` của header `:284`.
  - `ReplayDialog.tsx` post JSON qua hook và multipart qua `customInstance(getWorkflowReplayExecutionUrl(...), { method: "POST", body: FormData })` (`:369-386`).
  - Form Run-flow là `components/workflows/run/inputModel.ts`:
    - `inputsFromSource` `:120-164`;
    - `formFromInputs`, `validateForm`, `requestFromForm`;
    - `validationRequestFromForm` `:500-502`;
    - `validationView` `:518-534`;
    - `serverErrorsByInput` `:550`.

    Các control là `inputControls.tsx` (`InputField`).
  - Test harness: `components/workflows/executions/__tests__/ReplayDialog.test.tsx:1-80` (một class `Server` stub `fetch`) và `routes/workflows/__tests__/ExecutionDetail.test.tsx:92-122` (reply `resume`), với các test resume ở `:384-482`.

---

## Global Constraints

- Q1, Q2 và D1–D13 của spec có tính ràng buộc, chỉ được sửa bởi các Spec corrections ở trên sau khi người dùng đã xác nhận S1, S2, S3 và S7. Khi spec và code của egret không khớp theo cách không được liệt kê ở trên, **dừng và báo cáo**; không tự ứng biến. Source của Kestra là chỉ đọc.
- **Tìm code:** CodeGraph trước: `codegraph_explore` với `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Chỉ dùng grep cho chuỗi literal. `rg` chưa được cài.
- **Worktree:** `/Users/1pro-l4r8/Documents/github/data-portal/egret-execops`, nhánh `feat/workflow-pause-resume`.
  - Không bao giờ chuyển nhánh. Không bao giờ dùng `git stash`.
  - Không có mạng: không `git push`/`fetch`/`pull`, không `gh`, không `npm ci`/`npm install`.
- **Môi trường (mọi lệnh cargo):**
  ```bash
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target
  export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465
  ```
  - **Chạy từng lệnh cargo một**; không bao giờ chạy hai test suite cùng lúc.
  - Chạy `df -h` trước một lần build đầy đủ. Dọn `target/debug/incremental` thay vì tạo target dir thứ hai.
  - Không bao giờ đụng port 5432 hay các container `egret-dev-*` / `egret-demo-*`.
- **`node_modules` của frontend:** nếu `frontend/node_modules` không có, symlink nó, không bao giờ cài:
  ```bash
  ln -s ../../egret-sp2b/frontend/node_modules /Users/1pro-l4r8/Documents/github/data-portal/egret-execops/frontend/node_modules
  ```
  Xoá symlink ở cuối T7 (nó bị gitignore; không bao giờ commit). `npm run gen:api`, không bao giờ `orval` trần.
- **Gate** (mỗi task chạy những gate nó đụng tới; T8 chạy tất cả, từng cái một):
  - `cargo fmt --check`
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`
  - `cargo check -p egret-bin`
  - `cargo test -p egret-workflow --no-fail-fast` (7 lỗi `sql_starrocks_live` là dự kiến trên host này; bất kỳ lỗi nào khác là một failure)
  - `cargo test -p egret-config -p egret-server -p egret-metastore`
  - `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows` (in `frontend/`)
- **Commit:** Conventional Commits, mỗi message kết thúc bằng
  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
  Không có `Closes #` trong commit. Không bao giờ commit file scratch hay `.superpowers/`. Output scratch đi vào scratchpad của session (`$SCRATCH` bên dưới), không bao giờ vào repo.
- **Binary test:** mỗi binary `#[sqlx::test]` mới gọi `egret_test_db::assert_database_url_is_exported()` trong phần setup **và** có
  ```rust
  #[test]
  fn database_url_is_exported_rather_than_read_from_dotenv() {
      egret_test_db::assert_database_url_is_exported();
  }
  ```
  Các binary mới của plan này là `tests/pause_resume.rs` và `tests/pause_resume_routes.rs`.
- **Red trước:** mỗi task viết các test fail trước code, và chạy để thấy chúng fail.
- **Ratchet và census:** **301 / 25 / 25 / 0** và census **295 / 25 / 21**, đo lại ở T0. T2 đo lại chúng. Một thay đổi được đo và ghi lại cùng lý do, không bao giờ đoán. Bất kỳ thay đổi nào ngoài T2 là một regression cần báo cáo.
- Không thêm `std::env::var` mới. Nếu cần, thêm nó vào `ENV_OUTSIDE_CONFIG` của `egret-config` và chạy `cargo test -p egret-config`.
- Không migration (D12). Nếu cần, dừng và báo cáo.
- **Từ ngữ trong code và message:**
  - Code từ chối mới, chính xác: `not_running` (409), `not_paused` (409), `resume_inputs_unexpected` (400).
  - Audit action: `workflow_execution:pause` `{from_state}`; `workflow_execution:resume` `{from_state, by_hand, task_run_id, inputs_given}`. Không bao giờ ghi giá trị input.
  - Key output: `onResume`, `resumed` (`by`, `on`, `to`).
- Code, comment và docs bằng tiếng Anh.

---

## Cấu trúc file

| File | Thay đổi | Task |
|---|---|---|
| `crates/egret-workflow/src/executor/manual_pause.rs` | **mới**: `paused_by_hand`, `paused_by_hand_of`, `NotRunning`, `pause_by_hand_commands`, `resume_by_hand_commands`, `first_waiting_pause`, `resumed_outputs`; test | T1 |
| `crates/egret-workflow/src/executor/mod.rs` | `pub mod manual_pause;`, re-export; `drives_forward` học D1 | T1 |
| `crates/egret-workflow/src/models/task.rs` | `Pause.on_resume` | T2 |
| `crates/egret-workflow/src/spec/validate.rs` | các kiểm tra input nhận một path prefix; `onResume` được kiểm tra trong lượt run và save | T2 |
| `crates/egret-workflow/src/spec/keys.rs` | bỏ dòng `lift_of`; `input` nhận path list của nó; walk các entry `Pause.onResume`; test | T2 |
| `crates/egret-workflow/src/spec/schema/property-treatment.json`, `spec/schema/README.md` | đo lại | T2 |
| `crates/egret-workflow/src/worker/pause.rs` | **mới**: `Paused`, `pause_by_hand`; rồi `ResumeRequest`, `ResumeRefused`, `resume_with` | T3, T4 |
| `crates/egret-workflow/src/worker/claim.rs` | cổng render của `drive`; `due_retries` loại trừ; `Resumed` nhận thêm `ByHand`, `Refused`; `resume_by_hand` uỷ quyền; `checked_inputs_for`, `input_outcomes_for`, `input_renderer` | T3, T4 |
| `crates/egret-workflow/src/worker/mod.rs` | `mod pause;` và re-export | T3, T4 |
| `crates/egret-workflow/src/worker/render.rs` | nhánh `onResume` của `reveal_run_outputs` | T4 |
| `crates/egret-workflow/src/worker/sql_principal.rs` | `check_resume_inputs` | T5 |
| `crates/egret-workflow/src/secret_inputs.rs` | `reveal_keyed_outputs` (tổng quát hoá `reveal_subflow_outputs`) | T4 |
| `crates/egret-workflow/src/storage/uri.rs`, `storage/mod.rs`, `storage/inputs.rs` | `resume_input_uri`; `FileStore::resume_root`/`resume_prefix`; `upload_resume_input`; `check_file_values` | T4 |
| `crates/egret-workflow/tests/pause_resume.rs` | **mới**: test cấp worker | T3, T4 |
| `crates/egret-workflow/src/routes.rs` | `pause_execution`, `resume_execution` (body), `validate_resume_inputs`, `Waiting`, `live_flow_of`, `resume_gate`, `PartSink`, `receive_parts`/`read_parts`/`discard_*` tổng quát hoá, code, `ExecutionDto.paused_by_hand`, OpenAPI, router | T5 |
| `crates/egret-workflow/tests/pause_resume_routes.rs` | **mới**: test route | T5 |
| `openapi.json` | tạo lại | T5 |
| `frontend/src/api/generated/**`, `frontend/src/api/generated.digest.json` | `npm run gen:api` | T6 |
| `frontend/src/components/workflows/executions/actions.ts` (+ test) | `showsPause`, `canPause`, `canResume`, `pauseRefusal`, `resumeRefusal` | T7 |
| `frontend/src/components/workflows/run/inputModel.ts` | `runInputFrom` được export từ `inputsFromSource` | T7 |
| `frontend/src/components/workflows/executions/ResumeDialog.tsx` (+ test) | **mới** | T7 |
| `frontend/src/routes/workflows/ExecutionDetail.tsx` (+ test), `routes/workflows/queryKeys.ts` | nút và dialog Pause, ResumeDialog, ghi chú ở header, key `resumeInputs` | T7 |
| `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, `docs/en/specs/2026-10-28-workflow-inputs-design.md`, spec §5, `docs/vi/...` | các bản sửa có ghi ngày | T8 |

---

## T0. Môi trường và baseline

- [ ] Kiểm tra các container: `docker ps --format '{{.Names}} {{.Ports}}' | grep egret-execops` liệt kê `egret-execops-postgres` ở 5465 và `egret-execops-valkey` ở 6465. Nếu thiếu cái nào, khởi động nó như T0 của 7b đã làm (image `postgres:16`, `egret`/`egret`/`egret_test`, host port 5465; Valkey ở 6465), và báo cáo.
- [ ] Export ba biến (Global Constraints).
- [ ] Ratchet và census: `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t0-executor.txt"`. Đọc tuple và dòng census từ file, không từ `tail` (memory: `tail` cắt báo cáo).
- [ ] Property treatment: `cargo test -p egret-workflow --lib spec::schema::measure 2>&1 | tee "$SCRATCH/t0-measure.txt"` (dự kiến xanh). Đếm các treatment bằng `python3 -c "import json,collections;d=json.load(open('crates/egret-workflow/src/spec/schema/property-treatment.json'));c=collections.Counter(v for r in d.values() for v in (r.values() if isinstance(r,dict) else []) if isinstance(v,str));print(c)"`. Ghi lại số đếm.
- [ ] Migration: `ls migrations | tail -1` (dự kiến `0228_compile_run_migration_complete.sql`).
- **Chấp nhận:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; đã ghi số đếm treatment; migration trên cùng 0228. Báo cả bốn như đã đo. Không commit.

---

## T1. Các quy tắc thuần: `executor::manual_pause` (spec D1, D2, D3, D5)

**File:**
- Tạo: `crates/egret-workflow/src/executor/manual_pause.rs`.
- Sửa: `crates/egret-workflow/src/executor/mod.rs:46-56` (danh sách module), `:59-140` (re-export), `:359-371` (`drives_forward` và doc của nó).

**Interface.**

Dùng: `executor::{Command, ExecutionState, RunIx, RunState, Resume, apply, advance, build_plan}`; `executor::test_support::{flow, leaf, driven, drive, last, run, T0}` (test); `executor::release_paused_pauses` (test).

Sinh ra (`executor/manual_pause.rs`, re-export từ `executor`):
```rust
pub fn paused_by_hand(state: &ExecutionState) -> bool;
pub fn paused_by_hand_of(
    execution: Option<RunState>,
    runs: impl Iterator<Item = Option<RunState>>,
) -> bool;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotRunning(pub Option<RunState>);
pub fn pause_by_hand_commands(state: &ExecutionState) -> Result<Vec<Command>, NotRunning>;
pub fn resume_by_hand_commands(state: &ExecutionState) -> Vec<Command>;
pub fn first_waiting_pause(state: &ExecutionState) -> Option<RunIx>;
pub fn resumed_outputs(
    on_resume: serde_json::Map<String, serde_json::Value>,
    on: &str,
) -> serde_json::Value;
```
`drives_forward(state)` giữ chữ ký và cũng trả false khi `paused_by_hand(state)`.

**Các bước.**

- [ ] **Viết các test fail trước** (`#[cfg(test)] mod tests` bên dưới), với module chỉ có các dòng `use` và doc. Thêm `pub mod manual_pause;` sau `pub mod exit;` trong `executor/mod.rs`. Chạy `cargo test -p egret-workflow --lib executor::manual_pause`: nó không compile. Đó là red.
- [ ] **Triển khai** (code bên dưới). Thêm vào `executor/mod.rs`, cạnh các dòng `pub use` khác:
  ```rust
  pub use manual_pause::{
      first_waiting_pause, pause_by_hand_commands, paused_by_hand, paused_by_hand_of,
      resume_by_hand_commands, resumed_outputs, NotRunning,
  };
  ```
- [ ] **`drives_forward`** (`executor/mod.rs:359-371`) trở thành:
  ```rust
  /// **Whether [`advance`] still drives this execution's runs forward**: it
  /// is neither terminal nor `KILLING`, nor paused by hand (package 7c D2,
  /// [`paused_by_hand`]). Every other state -- `RUNNING`, a `PAUSED` that a
  /// `Pause` or a waiting `LoopUntil` holds, `RETRYING`, `CREATED`,
  /// `RESTARTED` -- reaches the walk, which creates, starts and completes
  /// runs. `advance` branches on exactly this, and the worker's update pass
  /// (`worker::render::render_updates`, group C2 ruling D9) gates on it, so
  /// the two cannot disagree about when a sibling may still move. A manual
  /// pause reaches `advance`'s `_ => Vec::new()` arm: Kestra's
  /// `ExecutorContext.canBeProcessed()` is false for `PAUSED`.
  pub fn drives_forward(state: &ExecutionState) -> bool {
      !state
          .history
          .last()
          .is_some_and(|s| s.is_terminal() || *s == RunState::Killing)
          && !paused_by_hand(state)
  }
  ```
  Trong comment guard của `advance` (`executor/mod.rs:193-200`) thêm một dòng: "A manual pause (package 7c) returns `Vec::new()` here, and `kill` and 7a's restart still reach it."
- [ ] Chạy `cargo test -p egret-workflow --lib executor::manual_pause` (tất cả pass), `cargo test -p egret-workflow --lib executor::` (không regression), và ratchet `cargo test -p egret-workflow --test executor` (301/25/25/0 không đổi).
- [ ] Commit: `feat(workflow): pure rules for pause by hand and resume (package 7c)`.

**Triển khai** (`crates/egret-workflow/src/executor/manual_pause.rs`):

```rust
//! Pause by hand and resume by hand -- workflow package 7c (spec
//! `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D1-D3, D5;
//! Kestra `ExecutionService.pause`, `ES:907-916`, and `.resume`,
//! `ES:880-897`).
//!
//! Pure, like `executor::restart`: these functions say what a manual pause
//! and a manual resume write. The worker (`worker::pause`) takes the
//! execution's lock, applies them and settles.

use super::command::Command;
use super::state::{ExecutionState, Resume, RunIx, RunState};

/// **D1: paused by hand** -- the execution's last state is `PAUSED` and no
/// run's last state is (Kestra `ES.resume`'s `findFirstByState(PAUSED)`
/// empty). A `Pause` task or a waiting `LoopUntil` always leaves a `PAUSED`
/// run (`Command::PauseRun` is the only producer of one, and `pause_mark`
/// pauses the execution only in a batch holding one), so the two cannot be
/// confused.
pub fn paused_by_hand(state: &ExecutionState) -> bool {
    paused_by_hand_of(
        state.history.last().copied(),
        state.runs.iter().map(|r| r.history.last().copied()),
    )
}

/// [`paused_by_hand`]'s rule over the last states alone, for a reader that
/// holds rows rather than an `ExecutionState` (`routes::ExecutionDto`). One
/// rule, two shapes of input.
pub fn paused_by_hand_of(
    execution: Option<RunState>,
    mut runs: impl Iterator<Item = Option<RunState>>,
) -> bool {
    execution == Some(RunState::Paused) && !runs.any(|r| r == Some(RunState::Paused))
}

/// Why an execution cannot be paused by hand (D3): its last state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotRunning(pub Option<RunState>);

/// **D3**: a `RUNNING` or `RESTARTED` execution takes `PAUSED`; no task run
/// changes. Every other state is refused with itself -- `KILLING` too,
/// which Kestra's `isRunning()` accepts (spec §5 divergence 1): `PAUSED`
/// after `KILLING` would stop `advance_killing`.
pub fn pause_by_hand_commands(state: &ExecutionState) -> Result<Vec<Command>, NotRunning> {
    match state.history.last().copied() {
        Some(RunState::Running | RunState::Restarted) => Ok(vec![Command::SetExecutionState {
            state: RunState::Paused,
        }]),
        other => Err(NotRunning(other)),
    }
}

/// **D5, paused by hand**: the execution goes back to `RUNNING` (Kestra
/// `ES:880-897`), and the next `advance` drives it. Empty when the
/// execution is not paused by hand.
pub fn resume_by_hand_commands(state: &ExecutionState) -> Vec<Command> {
    if paused_by_hand(state) {
        vec![Command::SetExecutionState {
            state: RunState::Running,
        }]
    } else {
        Vec::new()
    }
}

/// **D5**: the first `PAUSED` `Pause` run in run order -- the one whose
/// `onResume` a resume reads (Kestra `findFirstByState(PAUSED)`). A
/// `LoopUntil`'s wait (`Resume::Iterate`) is skipped: a resume does not end
/// it. The same predicate as `flowable::pause::release_now`.
pub fn first_waiting_pause(state: &ExecutionState) -> Option<RunIx> {
    state.runs.iter().position(|r| {
        r.history.last() == Some(&RunState::Paused)
            && matches!(r.resume_with, Some(Resume::Never | Resume::Terminal { .. }))
    })
}

/// **D5**: the outputs of a `Pause` run released by hand (`Pause.java:305-330`):
/// `{"onResume": {...}, "resumed": {"by": null, "on": <ISO-8601>, "to":
/// "SUCCESS"}}`. `by` stays null, as in Kestra OSS (`Pause.Resumed.now()`);
/// who resumed is in the audit row. A run with no `onResume` gets `{}` so a
/// template reading it does not fail.
pub fn resumed_outputs(
    on_resume: serde_json::Map<String, serde_json::Value>,
    on: &str,
) -> serde_json::Value {
    serde_json::json!({
        "onResume": on_resume,
        "resumed": { "by": null, "on": on, "to": "SUCCESS" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_support::{driven, flow, last, leaf, run, T0};
    use crate::executor::{advance, apply, build_plan, release_paused_pauses, Plan, Tick};

    /// `advance` alone to a fixed point: no leaf ends (a worker would).
    fn rest(plan: &Plan, state: &mut ExecutionState, now: Tick) {
        for _ in 0..100 {
            let cmds = advance(plan, state, now);
            if cmds.is_empty() {
                return;
            }
            for c in &cmds {
                apply(state, c);
            }
        }
        panic!("advance did not come to rest");
    }

    fn started(src: &str) -> (Plan, ExecutionState) {
        let plan = build_plan(src).expect("the fixture flow plans");
        let mut state = ExecutionState::new(&plan);
        rest(&plan, &mut state, T0);
        (plan, state)
    }

    fn paused(state: &mut ExecutionState) {
        for c in pause_by_hand_commands(state).expect("running") {
            apply(state, &c);
        }
    }

    const PAUSE: &str = "io.kestra.plugin.core.flow.Pause";

    // 1. D1 against each kind of PAUSED.
    #[test]
    fn paused_by_hand_tells_a_manual_pause_from_every_pause_that_leaves_a_paused_run() {
        // Manual.
        let (plan, mut state) = started(&flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2))));
        paused(&mut state);
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(paused_by_hand(&state));

        // A `Pause` task.
        let (_, state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n{}", leaf("after", 2))),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // A waiting `LoopUntil` (the drive answers its condition `false`).
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    tasks:\n{}",
                leaf("body", 6)
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // Two `Pause`s in a `Parallel`.
        let (_, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n"
            )),
            &[],
        );
        assert_eq!(state.history.last(), Some(&RunState::Paused));
        assert!(!paused_by_hand(&state));

        // The row form agrees.
        assert!(paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Running), Some(RunState::Success)].into_iter()
        ));
        assert!(!paused_by_hand_of(
            Some(RunState::Paused),
            [Some(RunState::Paused)].into_iter()
        ));
        assert!(!paused_by_hand_of(Some(RunState::Running), std::iter::empty()));
    }

    // 2. D2.
    #[test]
    fn drives_forward_is_false_for_a_manual_pause_and_true_for_a_pause_task() {
        let (plan, mut state) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        assert!(crate::executor::drives_forward(&state));
        paused(&mut state);
        assert!(!crate::executor::drives_forward(&state));

        let (_, state) = driven(&flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")), &[]);
        assert!(crate::executor::drives_forward(&state), "a Pause task's PAUSED still drives");
    }

    // 3. D2: a run ending during the pause creates nothing and completes nothing.
    #[test]
    fn a_run_ending_during_a_manual_pause_creates_and_completes_nothing() {
        let src = flow(&format!(
            "tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n{}{}{}",
            leaf("a", 6),
            leaf("b", 6),
            leaf("c", 2)
        ));
        let (plan, mut state) = started(&src);
        let a = run(&plan, &state, "a");
        assert_eq!(last(&state, a), Some(RunState::Running));
        paused(&mut state);

        apply(&mut state, &Command::SetRunState { run: a, state: RunState::Success });
        let before = state.runs.len();
        assert!(advance(&plan, &state, Tick(10)).is_empty(), "nothing is created or started");
        assert_eq!(state.runs.len(), before, "b has no run");
        let seq = run(&plan, &state, "seq");
        assert_eq!(last(&state, seq), Some(RunState::Running), "the parent is not completed");

        // A run that FAILS during the pause is not retried, failed upward or handled either.
        let src = flow(&format!("tasks:\n{}{}", leaf("x", 2), leaf("y", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let x = run(&plan, &state, "x");
        apply(&mut state, &Command::SetRunState { run: x, state: RunState::Failed });
        assert!(advance(&plan, &state, Tick(10)).is_empty());
        assert_eq!(state.history.last(), Some(&RunState::Paused));
    }

    // 4. D5: resume by hand gives RUNNING, and the next advance creates the next run.
    #[test]
    fn resume_by_hand_gives_running_and_the_next_advance_creates_the_next_run() {
        let src = flow(&format!("tasks:\n{}{}", leaf("a", 2), leaf("b", 2)));
        let (plan, mut state) = started(&src);
        paused(&mut state);
        let a = run(&plan, &state, "a");
        apply(&mut state, &Command::SetRunState { run: a, state: RunState::Success });

        let cmds = resume_by_hand_commands(&state);
        assert_eq!(cmds, vec![Command::SetExecutionState { state: RunState::Running }]);
        for c in &cmds {
            apply(&mut state, c);
        }
        rest(&plan, &mut state, Tick(20));
        let b = run(&plan, &state, "b");
        assert_eq!(last(&state, b), Some(RunState::Running));
        assert_eq!(
            &state.history[state.history.len() - 3..],
            &[RunState::Running, RunState::Paused, RunState::Running],
            "Kestra's ES.resume: PAUSED back to RUNNING, no RESTARTED"
        );

        // Not paused by hand: nothing.
        let (_, state) = driven(&flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n")), &[]);
        assert!(resume_by_hand_commands(&state).is_empty());
    }

    // 5. D3: only RUNNING and RESTARTED are paused.
    #[test]
    fn pause_by_hand_refuses_every_state_but_running_and_restarted() {
        let (_, base) = started(&flow(&format!("tasks:\n{}", leaf("a", 2))));
        for s in RunState::ALL {
            let mut state = base.clone();
            state.history.push(*s);
            let got = pause_by_hand_commands(&state);
            match s {
                RunState::Running | RunState::Restarted => assert!(got.is_ok(), "{s:?}"),
                other => assert_eq!(got, Err(NotRunning(Some(*other))), "{s:?}"),
            }
        }
    }

    // 6. D5: the first PAUSED Pause in run order, never a LoopUntil; and the payload.
    #[test]
    fn the_first_waiting_pause_is_a_pause_in_run_order_and_its_payload_is_kestras() {
        let (plan, state) = driven(
            &flow(&format!(
                "tasks:\n  - id: both\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: lu\n        type: io.kestra.plugin.core.flow.LoopUntil\n        condition: \"{{{{ false }}}}\"\n        checkFrequency:\n          interval: PT1H\n          maxIterations: 3\n        tasks:\n{}      - id: p1\n        type: {PAUSE}\n      - id: p2\n        type: {PAUSE}\n",
                leaf("body", 10)
            )),
            &[],
        );
        let p1 = run(&plan, &state, "p1");
        assert_eq!(first_waiting_pause(&state), Some(p1));

        let mut map = serde_json::Map::new();
        map.insert("ok".into(), serde_json::json!(true));
        assert_eq!(
            resumed_outputs(map, "2026-11-15T10:00:00.000Z"),
            serde_json::json!({
                "onResume": {"ok": true},
                "resumed": {"by": null, "on": "2026-11-15T10:00:00.000Z", "to": "SUCCESS"}
            })
        );
    }

    // 7. D5: a released Pause still ends SUCCESS and the execution goes RESTARTED.
    #[test]
    fn releasing_a_pause_still_restarts_the_execution() {
        let (plan, mut state) = driven(
            &flow(&format!("tasks:\n  - id: p\n    type: {PAUSE}\n{}", leaf("after", 2))),
            &[],
        );
        let released = release_paused_pauses(&mut state, Tick(10));
        assert_eq!(released, vec![run(&plan, &state, "p")]);
        let cmds = advance(&plan, &state, Tick(10));
        assert!(
            cmds.contains(&Command::SetExecutionState { state: RunState::Restarted }),
            "{cmds:?}"
        );
    }
}
```

`RunState::ALL` là một slice của mọi variant (`executor/state.rs`, dùng bởi `due_retries`). Nếu nó là một array, lặp `RunState::ALL.iter()`. Kiểm tra `last`/`run` trong `test_support` trước khi dùng; chúng là `pub(crate)`.

---

## T2. `Pause.onResume`: model, validate lúc publish, bỏ warning, đo lại (spec D4; S8)

**File:**
- Sửa:
  - `crates/egret-workflow/src/models/task.rs:739-761`;
  - `src/spec/validate.rs` (`:384-400`, `:721-800`, `:1381-1583`);
  - `src/spec/keys.rs` (`:171`, `:897-935`, `:984-997`, tests `:1344-1347`, `:1539-1556`, `:1895-1914`);
  - `src/spec/schema/property-treatment.json` (đo lại);
  - `src/spec/schema/README.md` (`:214`, `:230`).
- Nếu phép đo của T2 làm chúng dịch chuyển: `oracle-exec/driver.rs:854`, `tests/executor.rs:889`.

**Interface.**

Dùng: `models::Input`, `spec::path::every_task`, `spec::path::task_path`.

Sinh ra:
```rust
// models/task.rs, in `pub struct Pause`
#[serde(default, skip_serializing_if = "Option::is_none")]
pub on_resume: Option<Vec<crate::models::Input>>,

// spec/validate.rs (private)
fn validate_inputs_at(inputs: &[Input], list: &str, report: &mut ValidationReport);
fn input_save_findings_at(inputs: &[Input], list: &str, report: &mut ValidationReport);
fn input_dependency_findings_at(inputs: &[Input], list: &str, report: &mut ValidationReport);

// spec/keys.rs (private, on the walker)
fn input(&mut self, list: &str, id: &str, input: &serde_json::Map<String, Value>);
```

**Các bước.**

- [ ] **Test fail trước.** Trong test module của `spec/validate.rs` thêm bốn test bên dưới. Trong `spec/keys.rs`:
  - thay `pause_on_resume_warns_and_pause_tasks_is_refused_once` (`:1895-1914`) bằng `pause_on_resume_is_honoured_and_pause_tasks_is_refused_once`;
  - trong `a_key_kestra_accepts_and_egret_ignores_is_not_honoured_and_names_its_package` (`:1539-1556`), bỏ assertion `tasks.gate.onResume` và mong đợi `got.len() == 2`;
  - trong `lift_of_names_the_package_the_roadmap_gives` (`:1344-1347`), mong đợi `None`;
  - thêm `an_unknown_key_in_an_on_resume_input_is_refused_at_its_path`.

  Chạy `cargo test -p egret-workflow --lib spec::`: red (chưa có field, warning vẫn được phát).
- [ ] **Model.** Thêm field trên vào `models::Pause`, sau `on_pause`, với doc: "`onResume` (`Pause.java:193`): inputs a resume collects, written to this run's outputs as `onResume` (package 7c D4, D5). `None` and `Some([])` are the same: no inputs."
- [ ] **`spec/validate.rs`.**
  - Đổi tên `validate_inputs` thành `validate_inputs_at(inputs, list, report)`. Thay mỗi path `format!("inputs.{}…", input.id)` trong đó bằng `format!("{list}.{}…", input.id)` (`:1418`, `:1449`, `:1464`). Giữ `fn validate_inputs(inputs: &[Input], report: &mut ValidationReport) { validate_inputs_at(inputs, "inputs", report) }`.
  - Làm tương tự cho `input_save_findings` → `input_save_findings_at` (`:1508`, `:1515`, `:1522`, `:1534`) và `input_dependency_findings` → `input_dependency_findings_at` (`:1561`, `:1577`), với các wrapper `"inputs"`.
  - Trong `validate_for_run`, ngay sau `validate_inputs(&flow.inputs, &mut report);` (`:791`):
    ```rust
    // Package 7c D4: a `Pause`'s `onResume` is an input list, checked as the
    // flow's `inputs:` are, under the task's path (`tasks.<id>.onResume`).
    for (path, task) in crate::spec::path::every_task(flow) {
        if let TaskKind::Pause(p) = &task.kind {
            if let Some(list) = &p.on_resume {
                validate_inputs_at(list, &format!("{path}.onResume"), &mut report);
            }
        }
    }
    ```
  - Trong `save_only_findings`, sau `input_save_findings(&flow.inputs, report);` (`:398`):
    ```rust
    for (path, task) in crate::spec::path::every_task(flow) {
        if let TaskKind::Pause(p) = &task.kind {
            if let Some(list) = &p.on_resume {
                input_save_findings_at(list, &format!("{path}.onResume"), report);
            }
        }
    }
    ```
  - `input_save_findings_at` gọi `input_dependency_findings_at(inputs, list, report)`.
  - `every_task` trả `Vec<(String, &Task)>` với path có dấu chấm của task (`tasks.p`), như `path` của nhánh `Pause` đọc. Xác nhận bằng cách đọc `spec/path.rs:69`. Nếu phần tử đầu không phải path đó, dựng nó bằng `spec::path::task_path`.
- [ ] **`spec/keys.rs`.**
  - Xoá `("io.kestra.plugin.core.flow.Pause", "onResume") => Some(Package(9)),` (`:171`).
  - Đổi `fn input(&mut self, id, input)` thành `fn input(&mut self, list: &str, id: &str, input: &serde_json::Map<String, Value>)`. Hai path của nó trở thành `format!("{list}.{id}.dependsOn")` và `format!("{list}.{id}.{key}")`. Flow walker (`:694-697`) gọi `self.input("inputs", id, o)`.
  - Sau block `Loop` (`:984-997`) thêm:
    ```rust
    // Package 7c D4: `Pause.onResume` is a list of the flow's `Input` bean,
    // checked key by key as `inputs:` is.
    if row == "io.kestra.plugin.core.flow.Pause" {
        if let Some(Value::Array(items)) = t.get("onResume") {
            let list = format!("{path}.onResume");
            for item in items {
                let Some(o) = item.as_object() else { continue };
                let Some(iid) = o.get("id").and_then(Value::as_str) else {
                    continue;
                };
                self.seen.inputs += 1;
                self.input(&list, iid, o);
            }
        }
    }
    ```
    Nếu `self.seen.inputs` nuôi một số đếm mà một test ghim, bỏ nó ra và nói điều đó trong báo cáo.
- [ ] **Đo lại property treatment:**
  ```bash
  EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today
  cargo test -p egret-workflow --lib spec::schema::measure
  git diff --stat crates/egret-workflow/src/spec/schema/property-treatment.json
  ```
  Dự kiến đúng một dòng thay đổi, `io.kestra.plugin.core.flow.Pause.onResume: "ignored"` → `"honoured"`. Bất kỳ dòng nào khác dịch chuyển: dừng và báo cáo.

  Cập nhật `spec/schema/README.md`:
  - `:214` nhận một câu dẫn có ghi ngày với số đếm đo bằng one-liner Python của T0, chạy lại bây giờ ("…, 2026-11-15, after package 7c modelled `Pause.onResume` and moved it from `ignored` to `honoured`; before that, …");
  - `:230`, dòng `Pause`: `honoured` 10 → 11, và `onResume` rời ô `ignored`.
- [ ] **Test** (test module của `spec/validate.rs`; `report(src)` là helper sẵn có của module trên `validate_source`, nếu không dùng `crate::spec::validate_source(src, &crate::spec::parse(src).unwrap())`):
  ```rust
  const ON_RESUME_ALL_TYPES: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: s\n        type: STRING\n      - id: n\n        type: INT\n        min: 1\n      - id: x\n        type: FLOAT\n      - id: b\n        type: BOOL\n        defaults: true\n      - id: d\n        type: DATE\n      - id: dt\n        type: DATETIME\n      - id: t\n        type: TIME\n      - id: du\n        type: DURATION\n      - id: f\n        type: FILE\n        required: false\n      - id: sec\n        type: SECRET\n        required: false\n      - id: sel\n        type: SELECT\n        values: [a, b]\n      - id: ms\n        type: MULTISELECT\n        values: [a, b]\n      - id: arr\n        type: ARRAY\n        itemType: INT\n      - id: j\n        type: JSON\n      - id: u\n        type: URI\n      - id: e\n        type: EMAIL\n      - id: y\n        type: YAML\n        required: false\n  - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{ outputs.p.onResume.s }}\"\n";

  #[test]
  fn on_resume_with_every_input_type_publishes_without_a_finding() {
      let flow = crate::spec::parse(ON_RESUME_ALL_TYPES).expect("parses");
      let r = crate::spec::validate_source(ON_RESUME_ALL_TYPES, &flow);
      assert!(r.errors.is_empty(), "{r:?}");
      assert!(r.warnings.iter().all(|w| !w.path.contains("onResume")), "{r:?}");
      assert!(crate::executor::build_plan(ON_RESUME_ALL_TYPES).is_ok());
  }

  #[test]
  fn a_bad_on_resume_input_is_refused_at_its_path() {
      let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: a\n        type: ARRAY\n        itemType: SECRET\n      - id: f\n        type: FILE\n        allowedFileExtensions: [csv]\n      - id: dup\n        type: STRING\n      - id: dup\n        type: STRING\n";
      let flow = crate::spec::parse(src).expect("parses");
      let r = crate::spec::validate_source(src, &flow);
      let at = |code: &str| r.errors.iter().find(|e| e.code == code).map(|e| e.path.clone());
      assert_eq!(at("input_item_type_invalid").as_deref(), Some("tasks.p.onResume.a.itemType"));
      assert_eq!(at("input_file_extension_invalid").as_deref(), Some("tasks.p.onResume.f.allowedFileExtensions"));
      assert_eq!(at("duplicate_input_id").as_deref(), Some("tasks.p.onResume.dup"));
  }

  #[test]
  fn a_bad_on_resume_default_or_dependency_is_refused_at_save_at_its_path() {
      let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: n\n        type: INT\n        min: abc\n      - id: m\n        type: STRING\n        dependsOn:\n          inputs: [nope]\n";
      let flow = crate::spec::parse(src).expect("parses");
      let r = crate::spec::validate_source(src, &flow);
      let paths: Vec<_> = r.errors.iter().map(|e| (e.code, e.path.as_str())).collect();
      assert!(paths.contains(&("input_bound_invalid", "tasks.p.onResume.n.min")), "{paths:?}");
      assert!(paths.contains(&("input_depends_on_unknown", "tasks.p.onResume.m.dependsOn.inputs")), "{paths:?}");
      // A save-only finding: the run gate does not raise it.
      assert!(crate::spec::validate_for_run(&flow).errors.iter().all(|e| e.code != "input_depends_on_unknown"));
  }

  #[test]
  fn on_resume_ids_may_repeat_a_flow_input_id_and_the_other_pause_gaps_stay() {
      let src = "id: f\nnamespace: a.b\ninputs:\n  - id: ok\n    type: STRING\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n";
      let flow = crate::spec::parse(src).expect("parses");
      assert!(crate::spec::validate_source(src, &flow).errors.is_empty());
      let gap = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n    behavior: RESUME\n";
      let flow = crate::spec::parse(gap).expect("parses");
      assert!(crate::spec::validate_source(gap, &flow)
          .errors
          .iter()
          .any(|e| e.code == "unsupported_pause_property" && e.path == "tasks.p.behavior"));
  }
  ```
  Trong `spec/keys.rs`:
  ```rust
  #[test]
  fn pause_on_resume_is_honoured_and_pause_tasks_is_refused_once() {
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n");
      assert!(r.errors.is_empty(), "{r:?}");
      assert!(r.warnings.iter().all(|w| !w.path.starts_with("tasks.p.onResume")), "{r:?}");
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
      assert_eq!(r.errors.iter().filter(|e| e.path == "tasks.p.tasks").count(), 1, "{r:?}");
  }

  #[test]
  fn an_unknown_key_in_an_on_resume_input_is_refused_at_its_path() {
      let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n        colour: red\n");
      assert!(
          r.errors.iter().any(|e| e.code == "unknown_property" && e.path == "tasks.p.onResume.ok.colour"),
          "{r:?}"
      );
  }
  ```
- [ ] Chạy, từng cái một:
  - `cargo test -p egret-workflow --lib spec::`
  - `cargo test -p egret-workflow --lib executor::`
  - `cargo test -p egret-workflow --test flow_schema --test publish`
- [ ] **Ratchet và census:** `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t2-executor.txt"`. Đọc tuple và dòng census `LIVE` từ file.
  - Dự kiến không đổi: 301/25/25/0 và 295/25/21. Ba flow corpus `onResume` đã publish trước đó, và validate onResume không từ chối cái nào.
  - Nếu một trong hai dịch chuyển, số mới là số **đã đo**. Cập nhật `oracle-exec/driver.rs:854` (`LIVE`) và doc block của nó, hoặc `tests/executor.rs:889` và câu phía trên nó, trong cùng commit này, với một dòng có ghi ngày nêu entry corpus nào đã dịch chuyển và vì sao. Báo cáo nó như một phát hiện.
- [ ] Commit: `feat(workflow): model Pause onResume and check it at publish (package 7c)`.

---

## T3. Worker: Pause thủ công dưới lock, và những gì vẫn chuyển động (spec D2, D3, D10; S1, S3, S4, S5)

**File:**
- Tạo: `crates/egret-workflow/src/worker/pause.rs`, `crates/egret-workflow/tests/pause_resume.rs`.
- Sửa: `src/worker/mod.rs` (`mod pause;` sau `mod kill;`, re-export), `src/worker/claim.rs:5234-5310` (`drive`), `src/worker/claim.rs:790-840` (`due_retries`).

**Interface.**

Dùng: `claim::{load, settle, Stack}`, `txn::transaction`, `executor::{pause_by_hand_commands, NotRunning, paused_by_hand, apply}`.

Sinh ra:
```rust
// worker/pause.rs, re-exported from `worker`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paused {
    Done,
    /// D3: not `RUNNING` or `RESTARTED`; the state's wire name.
    NotRunning(String),
}
/// D3. In the execution's lock; scoped by `workspace` (`NotFound` otherwise).
pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused>;
```

**Các bước.**

- [ ] **Viết các test fail trước** trong `tests/pause_resume.rs` (danh sách bên dưới). Chạy `cargo test -p egret-workflow --test pause_resume`: nó không compile.
- [ ] **Triển khai `worker/pause.rs`:**
  ```rust
  //! Pause by hand and resume with `onResume` inputs -- workflow package 7c
  //! (spec `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D3, D5,
  //! D6). Both run inside the execution's lock (`claim::load`) and end in
  //! `claim::settle`, as every writer does.

  use egret_core::{Error, Result};
  use sqlx::PgPool;
  use uuid::Uuid;

  use super::claim::{self, Stack};
  use super::txn::transaction;
  use crate::executor::apply;

  /// What [`pause_by_hand`] did.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Paused {
      /// The execution is `PAUSED`; no task run changed.
      Done,
      /// Nothing written: the execution is not `RUNNING` or `RESTARTED`; its
      /// state's wire name (D3).
      NotRunning(String),
  }

  /// **Pause by hand** (D3; Kestra `ES.pause`, `ES:907-916`): one
  /// `SetExecutionState { PAUSED }` under the execution's lock, then the
  /// settle, whose `advance` answers nothing (D2). Runs already `RUNNING`
  /// are left to finish. The state is re-checked under the lock, so a pause
  /// that loses the race to a kill or to the execution's end writes nothing.
  pub async fn pause_by_hand(pool: &PgPool, workspace: Uuid, id: Uuid) -> Result<Paused> {
      transaction!(pool, |at| {
          let (conn, gate) = at.parts();
          let (ctx, mut state, now) = claim::load(conn, id, Stack::Fresh(gate.clone())).await?;
          if ctx.workspace_id != workspace {
              return Err(Error::NotFound("execution".into()));
          }
          let cmds = match crate::executor::pause_by_hand_commands(&state) {
              Ok(cmds) => cmds,
              Err(crate::executor::NotRunning(s)) => {
                  return Ok(Paused::NotRunning(
                      s.map_or("UNKNOWN", |s| s.wire()).to_string(),
                  ));
              }
          };
          let before = state.clone();
          for c in &cmds {
              apply(&mut state, c);
          }
          claim::settle(conn, &ctx, &before, state, now, None).await?;
          Ok(Paused::Done)
      })
  }
  ```
  Trong `worker/mod.rs` thêm `mod pause;` và `pub use pause::{pause_by_hand, Paused};`.

  `claim::load` và `claim::settle` là `pub(super)`. `ExecutionCtx::workspace_id` là `pub`. Nếu `load` trả về shape tuple khác, hãy theo `claim::resume_by_hand` (`claim.rs:4535-4552`), là mẫu.
- [ ] **S4, `claim::drive`** (`claim.rs:5234-5310`). Sau block `if !cmds.is_empty() { … continue; }` và trước lượt render đầu tiên, chèn:
  ```rust
  // Package 7c D2 (S4): a manual pause stops the render passes too. A
  // `LoopUntil` whose body ended during the pause is asked its condition
  // at resume, not now, and no `runIf:` or expression render can fail a
  // run while nothing drives the execution (Kestra: `canBeProcessed()`).
  if crate::executor::paused_by_hand(state) {
      return Ok((spawns, subflows, exit_killed));
  }
  ```
- [ ] **S5, `due_retries`** (`claim.rs:790-840`). Trong `WHERE`, sau `AND e.state <> $3`, thêm:
  ```sql
  AND NOT (e.state = 'PAUSED' \
           AND NOT EXISTS (SELECT 1 FROM workflow.task_runs p \
                            WHERE p.execution_id = e.id AND p.state = 'PAUSED')) \
  ```
  Điều này giữ khoảng cách của string literal xung quanh. Thêm một đoạn doc phía trên hàm: "**Package 7c (S5):** an execution paused by hand (`executor::paused_by_hand`: `PAUSED` with no `PAUSED` run) is not woken. `advance` answers nothing for it until resume, so a due `RETRYING` run in it would wake it every tick for nothing. `a_manual_pause_with_a_due_retry_is_not_woken` is the test."
- [ ] Chạy, từng cái một:
  - `cargo test -p egret-workflow --test pause_resume`
  - `cargo test -p egret-workflow --test worker --test restart --test kill_route --test end_hooks`
  - ratchet `--test executor` (không đổi)
- [ ] Commit: `feat(workflow): pause an execution by hand; nothing moves while it waits (package 7c)`.

**Test** (`tests/pause_resume.rs`). Doc của module: package, path spec và "Run with: export DATABASE_URL=… ; cargo test -p egret-workflow --test pause_resume".

Fixture:
- copy `one_task`, `two_tasks`, `file_flow`, `settle_everything` và `subflow_parent` nguyên văn từ `tests/worker.rs:69-81`, `:4330-4344`, `:1910-1920`, `:4408-4415`;
- registry là `egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current())` (`tests/worker.rs:852`);
- thêm các helper bên dưới.

```rust
async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    (db, ws, user)
}

async fn start(pool: &sqlx::PgPool, db: &Db, ws: Uuid, user: egret_core::UserId, src: &str) -> Uuid {
    let flow = file_flow(db, ws, user, src).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), src, serde_json::json!({}))
        .await
        .expect("start")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id).fetch_one(pool).await.expect("state")
}

/// `task_id -> state` of every run, in run order.
async fn runs(pool: &sqlx::PgPool, id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as("SELECT task_id, state FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix")
        .bind(id).fetch_all(pool).await.expect("runs")
}

fn handlers() -> egret_workflow::worker::Handlers {
    egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current())
}
```

Mỗi test là `#[sqlx::test(migrations = "../../migrations")]`. Arrange / Act / Assert:

1. `a_running_execution_pauses_and_no_run_changes`:
   - Arrange: `two_tasks("p")` đã start, chưa tick; `a` là `RUNNING`.
   - Act: `pause_by_hand(&pool, ws, ex)` cho `Paused::Done`.
   - Assert: execution là `PAUSED`; history của nó kết thúc bằng `["RUNNING", "PAUSED"]`; `runs` vẫn là `[("a","RUNNING")]`.
2. `a_run_completing_during_the_pause_is_recorded_and_nothing_new_is_created`:
   - Arrange: như 1; pause.
   - Act: `settle_everything(&pool, &handlers())`.
   - Assert: `runs` là `[("a","SUCCESS")]` (không có `b`); execution là `PAUSED`; `outputs`/`ended_at` của `a` đã được ghi (`ended_at IS NOT NULL`).
3. `a_run_ending_inside_a_flowable_does_not_complete_its_parent`:
   - Arrange: một `Sequential` `seq` chứa `a`, `b`, rồi một `c` top-level; start; pause.
   - Act: settle.
   - Assert: `seq` là `RUNNING`, `a` là `SUCCESS`, không có run `b` và `c`.
4. `pause_against_kill_and_against_the_end`:
   - (a) kill trước (`egret_workflow::worker::kill_execution(&pool, ws, ex, true)`), rồi pause: `NotRunning("KILLING")` hoặc `NotRunning("KILLED")`, và state không đổi.
   - (b) pause trước, rồi kill: execution chuyển `KILLING`; sau `settle_everything` nó là `KILLED`, và không có run `b`.
   - (c) một execution đã xong (`one_task`, settle tới `SUCCESS`): `NotRunning("SUCCESS")`.
   - (d) đồng thời: `tokio::join!(pause_by_hand(..), kill_execution(..))` trên một `two_tasks` mới. Cả hai trả `Ok`. State cuối sau `settle_everything` là `KILLED`, và `a` không bao giờ được theo sau bởi `b`.
5. `a_foreign_workspace_cannot_pause`: `pause_by_hand(&pool, other_ws, ex)` cho `Err(NotFound)`; state không đổi.
6. `s1_a_loop_iteration_ending_during_the_pause_starts_the_next_and_the_flow_waits`:
   - Arrange: flow
     ```yaml
     id: loopy
     namespace: t
     tasks:
       - id: each
         type: io.kestra.plugin.core.flow.Loop
         values: ["1", "2"]
         concurrencyLimit: 1
         tasks:
           - id: a
             type: io.kestra.plugin.core.log.Log
             message: "{{ taskrun.value }}"
       - id: after
         type: io.kestra.plugin.core.log.Log
         message: done
     ```
     Start nó và pause root.
   - Act: `settle_everything`.
   - Assert (S1, Kestra):
     - có hai iteration (`SELECT count(*) FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index IS NOT NULL` = 2), cả hai `SUCCESS`;
     - run `each` là `SUCCESS`;
     - không có run `after`; root là `PAUSED`.
   - Sau đó `resume_with` (T4) hoặc, ở T3, resume thủ công qua `egret_workflow::worker::resume_by_hand(&pool, ex)`, mà T4 dạy trường hợp by-hand. **Ở T3 nửa này là `#[ignore = "T4"]`; T4 bỏ ignore.** Sau đó và `settle_everything`, root là `SUCCESS` và `after` đã chạy.
   - Nếu người dùng bác bỏ S1, test này lật: một iteration, và iteration thứ hai chỉ bắt đầu sau resume.
7. `a_subflow_child_ending_during_the_pause_completes_its_run_and_nothing_after`:
   - Arrange: `file_flow(one_task("child"))`; parent `subflow_parent("parent", "child", "")`; start parent và pause nó trước bất kỳ tick nào.
   - Act: `settle_everything`.
   - Assert: child là `SUCCESS`; run `sub` của parent là `SUCCESS`; không có run `after`; parent là `PAUSED`.
8. `s4_a_loop_until_condition_is_not_rendered_during_the_pause`:
   - Arrange: một `LoopUntil` `lu` với `condition: "{{ false }}"`, `checkFrequency: {interval: PT1H, maxIterations: 2}`, body `a` (Log). Start; pause.
   - Act: `settle_everything`.
   - Assert: `a` là `SUCCESS`; `SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'lu'` là `NULL`; execution là `PAUSED`.
   - Red trước cổng `drive`: `rendered` là `"false"`.
9. `s3_a_manual_pause_beside_a_waiting_pause_is_not_by_hand_and_its_timer_ends_it`:
   - Arrange: một `Parallel` gồm `quick` (`Pause`, `pauseDuration: PT0S`) và `slow` (`Pause`, `pauseDuration: PT1H`), rồi `after` (Log). Start, `settle_everything` và `egret_workflow::worker::wake(&pool, 16)` cho tới khi `quick` là `SUCCESS` và execution là `RUNNING`.
   - Act: pause (được chấp nhận, `Done`).
   - Assert: execution là `PAUSED`, và `executor::paused_by_hand_of(...)` trên các row là false (`slow` là `PAUSED`).
   - Sau đó `UPDATE workflow.task_runs SET not_before_ms = 0 WHERE task_id = 'slow'`, `wake`, `settle_everything`: execution rời `PAUSED` (`RESTARTED` rồi `RUNNING`, rồi `SUCCESS` sau `after`). Điều này ghim S3.
10. `s5_a_manual_pause_with_a_due_retry_is_not_woken`:
    - Arrange: `one_task_with_retry("r", "PT0S")` (copy từ `tests/worker.rs:2768-2774`) cạnh một task top-level thứ hai `b` trong một `Parallel`, để execution vẫn `RUNNING` trong khi `a` là `RETRYING`. Hoặc đơn giản hơn: start `two_tasks`, pause, rồi ghi tay một run `RETRYING` đến hạn:
      ```sql
      UPDATE workflow.task_runs
         SET state = 'RETRYING', history = history || '["RETRYING"]', not_before_ms = 0
       WHERE execution_id = $1 AND task_id = 'a'
      ```
    - Act/Assert: `egret_workflow::worker::due_retries(&pool, 10)` không chứa `ex`. Đối chứng: `UPDATE workflow.executions SET state = 'RUNNING'` → có chứa.
11. `kill_on_a_manual_pause_with_a_finished_child_ends_killed`:
    - Arrange: flow của test 3; pause; settle (`a` `SUCCESS`, `seq` `RUNNING`).
    - Act: `kill_execution(&pool, ws, ex, true)`.
    - Assert: `KILLING` rồi, sau settle, `KILLED`; `seq` là `KILLED`; không có `b`.
12. `restart_of_a_manual_pause_follows_7a`:
    - (a) `two_tasks`; pause; settle (`a` `SUCCESS`). `egret_workflow::worker::restart_execution(&pool, ex)` cho `Restarted::Refused(RestartRefused::NothingToRestart)`, và state vẫn `PAUSED`.
    - (b) một `two_tasks` mà `a` fail (một registry làm `a` fail; copy `Flaky` từ `tests/restart_routes.rs:33-67`); pause trước tick; tick (`a` `FAILED`, execution vẫn `PAUSED`); restart cho `Done`, execution chuyển `RESTARTED`, và sau một lần settle với `a` thành công, `b` chạy và execution là `SUCCESS`.
13. `database_url_is_exported_rather_than_read_from_dotenv` (test của Global Constraints).

---

## T4. Worker: resume, với input `onResume` (spec D5, D6; S2, S6, S7, S9)

**File:**
- Sửa:
  - `src/worker/pause.rs`;
  - `src/worker/claim.rs` (`Resumed` `:4499-4513`, `resume_by_hand` `:4535-4552`, `checked_inputs`/`input_outcomes`/`start_renderer` `:4243-4295`);
  - `src/worker/mod.rs` (re-exports, `:118-122`);
  - `src/worker/render.rs:2149-2167`;
  - `src/secret_inputs.rs:468-481`;
  - `src/storage/uri.rs` (sau `task_prefix`, `:393-420`);
  - `src/storage/mod.rs` (sau `execution_prefix`, `:255-270`);
  - `src/storage/inputs.rs` (`:97-167`, `:240-…`);
  - `tests/pause_resume.rs`.

**Interface.**

Dùng: T1 (`paused_by_hand`, `resume_by_hand_commands`, `first_waiting_pause`, `resumed_outputs`, `release_paused_pauses`); `spec::path::find_task`; `inputs_check::resolve`; `secret_inputs::{split_outputs, can_seal}`; `storage::index_task_files`.

Sinh ra:
```rust
// worker/pause.rs, re-exported from `worker`
pub struct ResumeRequest<'a> {
    pub id: Uuid,
    /// The posted values; `None` = no body or `"inputs": null`.
    pub inputs: Option<&'a serde_json::Map<String, serde_json::Value>>,
    /// The files the route stored for this resume (S2), all under `expected`.
    pub files: &'a [crate::storage::StoredFile],
    /// The `Pause` run the route read as waiting and stored files under.
    pub expected: Option<Uuid>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeRefused {
    /// D5: inputs where no `onResume` applies (paused by hand, or a `Pause`
    /// with none).
    InputsUnexpected,
    /// A value refused, a key not declared (S6), a FILE not readable, or a
    /// SECRET with no master key (S7): the message names the input.
    Invalid(String),
    /// The waiting `Pause` run is not the one the route read (a lost race).
    Moved,
}
pub async fn resume_with(pool: &PgPool, req: &ResumeRequest<'_>) -> Result<Resumed>;

// worker/claim.rs
pub enum Resumed {
    Released(usize),
    /// D5: the execution was paused by hand and is `RUNNING` again.
    ByHand,
    NoPauseWaiting,
    NotPaused,
    /// Nothing written; why.
    Refused(ResumeRefused),
}
/// Unchanged signature: a resume with no body (tests, W7b callers).
pub async fn resume_by_hand(pool: &PgPool, execution_id: Uuid) -> Result<Resumed>;
pub(crate) fn checked_inputs_for(flow: &Flow, declared: &[Input], posted: &Value)
    -> std::result::Result<Value, crate::inputs_check::InputError>;
pub(crate) fn input_outcomes_for(flow: &Flow, declared: &[Input], posted: &Value)
    -> Vec<crate::inputs_check::InputOutcome>;

// storage/uri.rs
pub fn resume_input_uri(namespace: &str, flow_id: &str, execution: Uuid, task_id: &str,
    task_run: Uuid, request: Uuid, input: &str, file: &str) -> Result<String, String>;
// storage/mod.rs, on FileStore
pub fn resume_root(&self, workspace: Uuid, namespace: &str, flow_id: &str, execution: Uuid,
    task_id: &str, task_run: Uuid) -> String;
pub fn resume_prefix(&self, workspace: Uuid, namespace: &str, flow_id: &str, execution: Uuid,
    task_id: &str, task_run: Uuid, request: Uuid) -> String;
// storage/inputs.rs
#[allow(clippy::too_many_arguments)]
pub async fn upload_resume_input(fs: &FileStore, workspace: Uuid, namespace: &str, flow_id: &str,
    execution: Uuid, task_id: &str, task_run: Uuid, request: Uuid, input: &Input,
    file_name: &str, body: &mut (dyn tokio::io::AsyncRead + Send + Unpin))
    -> std::result::Result<StoredFile, UploadError>;
pub async fn check_file_values(conn: &mut PgConnection, workspace: Uuid, execution: Uuid,
    declared: &[Input], values: &serde_json::Value, uploaded: &[StoredFile])
    -> Result<std::result::Result<(), String>>;
// secret_inputs.rs
pub(crate) fn reveal_keyed_outputs(outputs: Value, key: &str, sealed: Option<&[u8]>)
    -> Result<(Value, Vec<String>)>;
```

**Các bước.**

- [ ] **Test fail** (danh sách bên dưới) trong `tests/pause_resume.rs`. Bỏ `#[ignore]` của test 6 ở T3. Chạy `cargo test -p egret-workflow --test pause_resume`: red.
- [ ] **Tổng quát hoá kiểm tra input** (`claim.rs:4243-4295`):
  ```rust
  pub(crate) fn checked_inputs(flow: &crate::models::Flow, posted: &serde_json::Value)
      -> std::result::Result<serde_json::Value, crate::inputs_check::InputError> {
      checked_inputs_for(flow, &flow.inputs, posted)
  }
  /// [`checked_inputs`] against another declared list -- a `Pause`'s
  /// `onResume` (package 7c D5), rendered as a start's are.
  pub(crate) fn checked_inputs_for(
      flow: &crate::models::Flow,
      declared: &[crate::models::Input],
      posted: &serde_json::Value,
  ) -> std::result::Result<serde_json::Value, crate::inputs_check::InputError> {
      crate::inputs_check::resolve(declared, posted, &input_renderer(flow, declared))
          .map(serde_json::Value::Object)
  }
  pub(crate) fn input_outcomes(flow: &crate::models::Flow, posted: &serde_json::Value)
      -> Vec<crate::inputs_check::InputOutcome> {
      input_outcomes_for(flow, &flow.inputs, posted)
  }
  pub(crate) fn input_outcomes_for(
      flow: &crate::models::Flow,
      declared: &[crate::models::Input],
      posted: &serde_json::Value,
  ) -> Vec<crate::inputs_check::InputOutcome> {
      crate::inputs_check::resolve_all(declared, posted, &input_renderer(flow, declared))
  }
  ```
  Đổi tên `start_renderer(flow)` thành `input_renderer(flow, declared: &[crate::models::Input])`. Map `defaulted` của nó lặp trên `declared` thay vì `flow.inputs`. Thêm `checked_inputs_for, input_outcomes_for` vào `pub(crate) use claim::{…}` ở `worker/mod.rs:118`.
- [ ] **Storage.**
  - `uri.rs`, sau `task_prefix`:
    ```rust
    /// A FILE `onResume` input's URI (package 7c S2): under the `Pause` run's
    /// own task prefix, `tasks/{task-slug}/{task_run}/resume/{request}/{input}/{file}`,
    /// so it parses as [`Tail::Task`], is indexed under that run, and can
    /// never overwrite a start input (`inputs/{input}/{file}`).
    #[allow(clippy::too_many_arguments)]
    pub fn resume_input_uri(
        namespace: &str,
        flow_id: &str,
        execution: Uuid,
        task_id: &str,
        task_run: Uuid,
        request: Uuid,
        input: &str,
        file: &str,
    ) -> Result<String, String> {
        let prefix = task_prefix(namespace, flow_id, execution, task_id, task_run)?;
        let tail = format!("resume/{}/{input}/{file}", request.hyphenated());
        let uri = format!("{prefix}{tail}");
        checked(
            &uri,
            execution,
            &Tail::Task {
                task_slug: slugify_str(task_id),
                task_run: task_run.hyphenated().to_string(),
                file: tail,
            },
            &format!("namespace {namespace:?}, flow {flow_id:?}, task {task_id:?}, input {input:?} and file {file:?}"),
        )?;
        Ok(uri)
    }
    ```
    `checked` là helper private mà `input_uri` dùng (`uri.rs:438-446`). Đọc chữ ký của nó trước khi viết đoạn này.
  - `storage/mod.rs`, trên `FileStore`, sau `execution_prefix`:
    ```rust
    /// Every resume's files of `task_run` (package 7c S2), ending with `/`.
    pub fn resume_root(&self, workspace: Uuid, namespace: &str, flow_id: &str,
                       execution: Uuid, task_id: &str, task_run: Uuid) -> String {
        format!(
            "{}tasks/{}/{}/resume/",
            self.execution_prefix(workspace, namespace, flow_id, execution),
            uri::slugify_str(task_id),
            task_run.hyphenated()
        )
    }
    /// One resume request's files: what a refused resume deletes.
    #[allow(clippy::too_many_arguments)]
    pub fn resume_prefix(&self, workspace: Uuid, namespace: &str, flow_id: &str,
                         execution: Uuid, task_id: &str, task_run: Uuid, request: Uuid) -> String {
        format!(
            "{}{}/",
            self.resume_root(workspace, namespace, flow_id, execution, task_id, task_run),
            request.hyphenated()
        )
    }
    ```
  - `storage/inputs.rs`. Tách `upload_input` thành:
    - `fn admit(input: &Input, file_name: &str) -> std::result::Result<(), UploadError>`: các kiểm tra kiểu, tên rỗng và phần mở rộng, `:105-118`;
    - `async fn store(fs, workspace, uri: String, input: &Input, file_name: &str, body) -> std::result::Result<StoredFile, UploadError>`: mọi thứ từ `let object = fs.object_uri(…)` tới hết.

    `upload_input` trở thành `admit` + `uri::input_uri` + `store`. Thêm `upload_resume_input`, tức là `admit` + `uri::resume_input_uri(…)` (ánh xạ `Err(e)` thành `UploadError::Author(format!("Invalid value for input `{}`: {e}", input.id))`) + `store`.

    Đổi tên body của `check_file_inputs` thành `check_file_values(conn, workspace, execution, declared, values, uploaded)`, đọc `declared` ở chỗ nó đọc `flow.inputs` (chỗ duy nhất nó dùng `flow`). `check_file_inputs` trở thành một wrapper một dòng truyền `&flow.inputs`.
- [ ] **Secret.**
  - `secret_inputs.rs`: thay body của `reveal_subflow_outputs` bằng một lời gọi tới `reveal_keyed_outputs(outputs, "outputs", sealed)` mới. Hàm đó là body cũ với `"outputs"` được thay bằng `key`.
  - `render.rs:2154-2165`, `match` trong `reveal_run_outputs`:
    ```rust
    (Some(o), Some(sealed)) => {
        let (o, values) = if matches!(o.get("outputs"), Some(Value::Object(_))) {
            crate::secret_inputs::reveal_subflow_outputs(o, Some(sealed))?
        } else if matches!(o.get("onResume"), Some(Value::Object(_))) {
            // Package 7c (S7): a `Pause` run's SECRET `onResume` values.
            crate::secret_inputs::reveal_keyed_outputs(o, "onResume", Some(sealed))?
        } else {
            crate::secret_inputs::reveal_loop_outputs(o, Some(sealed))?
        };
        secrets.extend(values);
        Ok(Some(o))
    }
    ```
- [ ] **`worker/pause.rs`: `resume_with`.**
  ```rust
  // `use super::claim::Resumed;` and `use crate::executor::RunState;` join the imports.

  /// **Resume** (D5): in the execution's lock. Paused by hand: back to
  /// `RUNNING` (no inputs allowed). Otherwise the first `PAUSED` `Pause`
  /// run's `onResume` is checked and written into its outputs, every
  /// waiting `Pause` is released as today, and the execution settles on
  /// (`RESTARTED`, `pause_mark`). Every refusal comes before the first
  /// write: `transaction!` commits an `Ok`.
  pub async fn resume_with(pool: &PgPool, req: &ResumeRequest<'_>) -> Result<Resumed> {
      transaction!(pool, |at| {
          let (conn, gate) = at.parts();
          let (ctx, mut state, now) =
              claim::load(conn, req.id, Stack::Fresh(gate.clone())).await?;
          if state.history.last() != Some(&RunState::Paused) {
              return Ok(Resumed::NotPaused);
          }
          let posted = req.inputs.cloned().unwrap_or_default();
          let before = state.clone();
          if crate::executor::paused_by_hand(&state) {
              if !posted.is_empty() {
                  return Ok(Resumed::Refused(ResumeRefused::InputsUnexpected));
              }
              for c in crate::executor::resume_by_hand_commands(&state) {
                  apply(&mut state, &c);
              }
              claim::settle(conn, &ctx, &before, state, now, None).await?;
              return Ok(Resumed::ByHand);
          }
          let Some(chosen) = crate::executor::first_waiting_pause(&state) else {
              return Ok(Resumed::NoPauseWaiting);
          };
          let chosen_ix = i32::try_from(chosen).expect("a run index fits the column");
          let chosen_id: Uuid = sqlx::query_scalar(
              "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
          )
          .bind(req.id)
          .bind(chosen_ix)
          .fetch_one(&mut *conn)
          .await
          .map_err(db_err)?;
          if req.expected.is_some_and(|e| e != chosen_id) {
              return Ok(Resumed::Refused(ResumeRefused::Moved));
          }
          let flow = crate::spec::parse(&ctx.source).map_err(|e| {
              Error::Internal(anyhow::anyhow!(
                  "execution {}: its frozen source does not parse: {}",
                  req.id,
                  e.message
              ))
          })?;
          let task_id = ctx.plan.tasks[state.runs[chosen].task].id.clone();
          let declared: Vec<crate::models::Input> = crate::spec::path::find_task(&flow, &task_id)
              .and_then(|t| match &t.kind {
                  crate::models::TaskKind::Pause(p) => p.on_resume.clone(),
                  _ => None,
              })
              .unwrap_or_default();
          let (values, sealed) = match checked_on_resume(conn, &ctx, &flow, &task_id, &declared, &posted, req.files).await? {
              Ok(v) => v,
              Err(r) => return Ok(Resumed::Refused(r)),
          };

          let released = crate::executor::release_paused_pauses(&mut state, now);
          let on = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
          for &ix in &released {
              let (outputs, secret) = if ix == chosen {
                  (crate::executor::resumed_outputs(values.clone(), &on), sealed.clone())
              } else {
                  (crate::executor::resumed_outputs(serde_json::Map::new(), &on), None)
              };
              sqlx::query(
                  "UPDATE workflow.task_runs SET outputs = $3, secret_outputs = $4 \
                    WHERE execution_id = $1 AND run_ix = $2 AND state = 'PAUSED'",
              )
              .bind(req.id)
              .bind(i32::try_from(ix).expect("a run index fits the column"))
              .bind(&outputs)
              .bind(secret.as_deref())
              .execute(&mut *conn)
              .await
              .map_err(db_err)?;
          }
          crate::storage::index_task_files(conn, ctx.workspace_id, chosen_id, req.files).await?;
          claim::settle(conn, &ctx, &before, state, now, None).await?;
          Ok(Resumed::Released(released.len()))
      })
  }

  /// D5, D6, S6, S7: the posted values against `declared`. `Ok(Err(_))` is
  /// the caller's refusal; `Err` is the database's. Answers the masked
  /// values and, when a SECRET was given, their seal.
  #[allow(clippy::too_many_arguments)]
  async fn checked_on_resume(
      conn: &mut sqlx::PgConnection,
      ctx: &claim::ExecutionCtx,
      flow: &crate::models::Flow,
      task_id: &str,
      declared: &[crate::models::Input],
      posted: &serde_json::Map<String, serde_json::Value>,
      files: &[crate::storage::StoredFile],
  ) -> Result<std::result::Result<(serde_json::Map<String, serde_json::Value>, Option<Vec<u8>>), ResumeRefused>> {
      if declared.is_empty() {
          return Ok(if posted.is_empty() {
              Ok((serde_json::Map::new(), None))
          } else {
              Err(ResumeRefused::InputsUnexpected)
          });
      }
      // S6: refused, not dropped (the no-silent-drops rule).
      if let Some(k) = posted.keys().find(|k| !declared.iter().any(|i| &i.id == *k)) {
          return Ok(Err(ResumeRefused::Invalid(format!(
              "Invalid value for input `{k}`: the Pause `{task_id}` declares no such onResume input"
          ))));
      }
      let posted = serde_json::Value::Object(posted.clone());
      let values = match claim::checked_inputs_for(flow, declared, &posted) {
          Ok(serde_json::Value::Object(m)) => m,
          Ok(_) => unreachable!("checked_inputs_for answers an object"),
          Err(e) => return Ok(Err(ResumeRefused::Invalid(e.message))),
      };
      if let Err(why) = crate::storage::inputs::check_file_values(
          conn,
          ctx.workspace_id,
          ctx.id,
          declared,
          &serde_json::Value::Object(values.clone()),
          files,
      )
      .await?
      {
          return Ok(Err(ResumeRefused::Invalid(why)));
      }
      let secret_ids: Vec<String> = declared
          .iter()
          .filter(|i| i.r#type == crate::models::InputType::Secret)
          .filter(|i| values.get(&i.id).is_some_and(|v| !v.is_null()))
          .map(|i| i.id.clone())
          .collect();
      if secret_ids.is_empty() {
          return Ok(Ok((values, None)));
      }
      if !crate::secret_inputs::can_seal() {
          return Ok(Err(ResumeRefused::Invalid(format!(
              "egret refuses input {}: a SECRET value cannot be stored on this deployment: set secrets.master_key",
              secret_ids.join(", ")
          ))));
      }
      let (masked, sealed) = crate::secret_inputs::split_outputs(&values, &secret_ids)?;
      Ok(Ok((masked, sealed)))
  }

  fn db_err(e: sqlx::Error) -> Error {
      Error::Internal(anyhow::Error::new(e))
  }
  ```
  - Làm cho `ExecutionCtx` truy cập được dưới dạng `claim::ExecutionCtx` (nó là `pub(super) struct`, nên đã vậy rồi).
  - Thêm `use crate::executor::RunState;` và `use super::claim::Resumed;` vào các import của `pause.rs`.
  - Không chuyển gì khác ra khỏi `claim.rs`.
- [ ] **`claim.rs`.**
  - `Resumed` nhận thêm `ByHand` và `Refused(super::pause::ResumeRefused)` kèm doc, và thôi derive `Copy` (giờ nó mang một `String`).
  - `resume_by_hand(pool, id)` trở thành:
    ```rust
    pub async fn resume_by_hand(pool: &PgPool, execution_id: Uuid) -> Result<Resumed> {
        super::pause::resume_with(pool, &super::pause::ResumeRequest {
            id: execution_id,
            inputs: None,
            files: &[],
            expected: None,
        })
        .await
    }
    ```
    Giữ doc của nó và thêm một dòng: "Since package 7c it is [`super::pause::resume_with`] with no body: a manual pause goes back to `RUNNING`, a `Pause` run ends `SUCCESS` with `{onResume: {}, resumed}`."
  - Re-export `pub use pause::{pause_by_hand, resume_with, Paused, ResumeRefused, ResumeRequest};` trong `worker/mod.rs`.
- [ ] Chạy, từng cái một:
  - `cargo test -p egret-workflow --test pause_resume`
  - `cargo test -p egret-workflow --test executions_routes --test file_inputs --test secret_inputs --test inputs_start --test worker --test storage_index`
  - `cargo test -p egret-workflow --lib storage:: secret_inputs:: worker::render`
  - ratchet.

  Các test resume hiện có (`executions_routes.rs:2445-2760`) phải giữ xanh, trừ test nào khẳng định `outputs` của một run `Pause` là `NULL` sau resume. Test như vậy giờ thấy `{onResume: {}, resumed}` theo thiết kế; cập nhật nó và nói điều đó trong báo cáo.
- [ ] Commit: `feat(workflow): resume a Pause with onResume inputs written to its outputs (package 7c)`.

**Test** (`tests/pause_resume.rs`, bổ sung). Chạy `egret_workflow::storage::install_mem_for_tests()` và `egret_workflow::secret_inputs::install_for_tests()` ở đầu mỗi test cần chúng.

14. `resume_by_hand_runs_on_and_the_flow_finishes`:
    - Arrange: `two_tasks`; pause; settle (`a` `SUCCESS`).
    - Act: `resume_with` không có input cho `Resumed::ByHand`; rồi settle.
    - Assert: `b` đã chạy; execution là `SUCCESS`; history của nó có `PAUSED` rồi `RUNNING`, không có `RESTARTED` ở giữa.
15. `resume_by_hand_with_inputs_is_refused_and_writes_nothing`: input `{"x": 1}` cho `Refused(InputsUnexpected)`; state là `PAUSED`.
16. `resume_with_inputs_writes_on_resume_and_resumed_and_the_next_task_reads_them`. Test end-to-end của spec §4:
    - Arrange: flow
      ```yaml
      id: approve
      namespace: t
      tasks:
        - id: p
          type: io.kestra.plugin.core.flow.Pause
          onResume:
            - id: who
              type: STRING
            - id: n
              type: INT
            - id: doc
              type: FILE
              required: false
        - id: log
          type: io.kestra.plugin.core.log.Log
          message: "who={{ outputs.p.onResume.who }} n={{ outputs.p.onResume.n }} by={{ outputs.p.resumed.to }}"
      ```
      Start; `settle_everything` (execution là `PAUSED`).
    - Act: `resume_with(inputs {"who":"ada","n":"3"})` cho `Released(1)`; rồi settle.
    - Assert:
      - `outputs` của `p` = `{"onResume":{"who":"ada","n":3},"resumed":{"by":null,"on":<string>,"to":"SUCCESS"}}`; giá trị `on` parse được như RFC 3339;
      - `SELECT message FROM workflow.task_logs WHERE execution_id = $1` chứa `who=ada n=3 by=SUCCESS`;
      - execution là `SUCCESS`, và history của nó có `RESTARTED`.
17. `a_refused_value_names_the_input_and_writes_nothing`:
    - `{"who":"ada","n":"x"}` cho `Refused(Invalid(m))`, với `m` chứa `` `n` ``; state là `PAUSED`; outputs của `p` là `NULL`.
    - Thiếu `who` bắt buộc cho `Invalid` chứa `Missing required input:who`.
18. `s6_an_undeclared_key_is_refused`: `{"who":"ada","n":"3","zzz":1}` cho `Invalid` chứa `` `zzz` ``.
19. `a_pause_without_on_resume_accepts_no_body_and_refuses_inputs`: một `Pause` không giới hạn và `after`. `{"a":1}` cho `InputsUnexpected`; `{}` cho `Released(1)`, và outputs của `p` là `{"onResume":{},"resumed":{…}}`.
20. `parallel_pauses_the_first_gets_the_inputs_and_the_others_on_resume_empty`: một `Parallel` gồm `p1` (`onResume: [{id: a, type: STRING}]`) và `p2` (`onResume: [{id: b, type: STRING}]`). `{"a":"x"}` cho `Released(2)`; `p1.onResume == {"a":"x"}`; `p2.onResume == {}`.
21. `only_a_loop_until_waiting_is_no_pause_waiting`: fixture `LoopUntil` của test 1 ở T1, settle tới `PAUSED`. `resume_with` cho `NoPauseWaiting`.
22. `a_lost_race_is_moved`: `expected: Some(Uuid::new_v4())` cho `Refused(Moved)`; không gì được ghi.
23. `s2_a_file_stored_under_the_pause_run_is_indexed_and_readable`:
    - Arrange: flow của test 16; `let fs = install_mem_for_tests();`; đọc id run của `p`.
    - Act: lưu `b"hello"` bằng `upload_resume_input(&fs, ws, "t", "approve", ex, "p", run, req_id, &doc_input, "a.txt", &mut &b"hello"[..])`. Rồi `resume_with` với `{"who":"ada","n":"1","doc": <uri>}` và `files: &[stored]`.
    - Assert:
      - `Released(1)`;
      - URI khớp `…/executions/{ex}/tasks/p/{run}/resume/{req_id}/doc/a.txt`;
      - `egret_workflow::storage::resolve_readable(&mut conn, ws, ex, &uri)` cho `Ok(Ok(_))` (đọc được bởi các task sau của chính execution và bởi file API, dùng cùng quy tắc);
      - `workflow.execution_files` chứa URI với `task_run_id = run`.
24. `s7_a_secret_on_resume_value_is_sealed_and_a_later_task_reads_it_masked_in_logs`:
    - Arrange: `install_for_tests()`; `onResume: [{id: tok, type: SECRET}]`; message của `log` `"tok={{ outputs.p.onResume.tok }}"`.
    - Act: resume với `{"tok":"s3cr3t"}`; settle.
    - Assert: `p.outputs.onResume.tok == "******"`; `secret_outputs IS NOT NULL`; dòng log không chứa `s3cr3t` (nó bị che); execution là `SUCCESS`.
    - Khi không có cipher: cần một binary riêng vì cipher là toàn process. **Assert việc từ chối không có key trong `tests/secret_inputs_no_key.rs`** (một binary sẵn có không có key): một resume với một SECRET cho `Invalid` chứa `secrets.master_key`.
25. Bỏ ignore của test 6 ở T3.

---

## T5. Route, gate, audit, OpenAPI, `ExecutionDto.paused_by_hand` (spec D3, D5–D9, field DTO của D11)

**File:**
- Sửa:
  - `crates/egret-workflow/src/routes.rs` (các phần nêu tên bên dưới);
  - `src/worker/sql_principal.rs` (sau `check_start`, `:90-113`);
  - `src/repo.rs` (dự kiến không có gì);
  - `openapi.json` (tạo lại).
- Tạo: `crates/egret-workflow/tests/pause_resume_routes.rs`.

**Interface.**

Dùng: worker API của T3/T4; `ExecOpError`; `read_json_body`, `start_inputs`, `startable_flow`; `crate::spec::path::find_task`.

Sinh ra:
```rust
// routes.rs
pub const NOT_RUNNING_CODE: &str = "not_running";
pub const NOT_PAUSED_CODE: &str = "not_paused";
pub const RESUME_INPUTS_UNEXPECTED_CODE: &str = "resume_inputs_unexpected";

async fn pause_execution(State<WorkflowState>, WsContext, Path<(String, Uuid)>)
    -> Result<Json<ExecutionDto>, ExecOpError>;
async fn resume_execution(State<WorkflowState>, WsContext, Path<(String, Uuid)>, Request)
    -> Result<Json<ExecutionDto>, Response>;
async fn validate_resume_inputs(State<WorkflowState>, WsContext, Path<(String, Uuid)>, Request)
    -> Result<Json<ValidateInputsDto>, Response>;

/// D9: 404 for a flow in the trash or deleted for good; disabled does not matter.
async fn live_flow_of(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail) -> EgretResult<()>;
/// What a resume finds waiting, from the scoped read (before any body is read).
enum Waiting { ByHand, Pause { run: Uuid, task_id: String, on_resume: Vec<crate::models::Input> }, LoopOnly }
fn waiting_of(detail: &crate::repo::ExecutionDetail) -> EgretResult<Waiting>;
/// Where a multipart part's file goes.
enum PartSink<'a> { Start { execution: Uuid }, Resume { execution: Uuid, task_id: &'a str, task_run: Uuid, request: Uuid }, Discard }
async fn receive_parts(workspace: Uuid, parsed: &crate::models::Flow, declared: &[crate::models::Input],
    owner: &str, sink: &PartSink<'_>, req: Request) -> Result<Uploaded, Response>;
async fn discard_prefix(prefix: String);
fn sink_prefix(fs: &crate::storage::FileStore, workspace: Uuid, parsed: &crate::models::Flow, sink: &PartSink<'_>) -> Option<String>;

// ExecutionDto
/// Package 7c D1/D11: `PAUSED` and no task run waits -- paused by hand.
pub paused_by_hand: bool,

// worker/sql_principal.rs
pub(crate) async fn check_resume_inputs(conn: &mut PgConnection, flow: &Flow,
    flow_id: Option<Uuid>, caller: Uuid) -> Result<()>;
```

**Hình dạng handler.**

- **`live_flow_of`** (D9). Tách nửa đầu của `startable_flow` ra thành `live_flow(st, ws, flow_id) -> EgretResult<FlowRow>` (`repo.get` và kiểm tra `deleted_at`, `routes.rs:2439-2447`). `startable_flow` gọi nó, rồi parse và kiểm tra disabled. Sau đó:
  ```rust
  async fn live_flow_of(st: &WorkflowState, ws: Uuid, detail: &crate::repo::ExecutionDetail) -> EgretResult<()> {
      let flow_id = detail.execution.flow_id.ok_or_else(|| Error::NotFound("flow".into()))?;
      live_flow(st, ws, flow_id).await.map(|_| ())
  }
  ```
- **`check_resume_inputs`** (D7):
  ```rust
  const NOT_THE_CREATOR_RESUME: &str = "the flow contains an `egret.sql` task, which runs as the \
       flow's creator, and the waiting Pause takes onResume inputs that task can read, so only \
       the flow's creator can resume it";

  /// Package 7c D7 (7a/7b's D24, door 2): a resume of a `Pause` with a non-empty
  /// `onResume` on a flow that runs `egret.sql` is its creator's alone.
  pub(crate) async fn check_resume_inputs(conn: &mut PgConnection, flow: &Flow,
      flow_id: Option<Uuid>, caller: Uuid) -> Result<()> {
      let Some(flow_id) = flow_id else { return Ok(()) };
      if !runs_sql(flow) {
          return Ok(());
      }
      let creator: Option<Uuid> =
          sqlx::query_scalar("SELECT created_by FROM workflow.flows WHERE id = $1")
              .bind(flow_id)
              .fetch_optional(&mut *conn)
              .await
              .map_err(|e| Error::Internal(anyhow::Error::new(e)))?
              .flatten();
      match creator {
          Some(c) if c != caller => Err(Error::InvalidInput(NOT_THE_CREATOR_RESUME.into())),
          _ => Ok(()),
      }
  }
  ```
- **`waiting_of`**:
  ```rust
  fn waiting_of(detail: &crate::repo::ExecutionDetail) -> EgretResult<Waiting> {
      let by_hand = crate::executor::paused_by_hand_of(
          crate::worker::from_wire(&detail.execution.state),
          detail.runs.iter().map(|r| crate::worker::from_wire(&r.state)),
      );
      if by_hand {
          return Ok(Waiting::ByHand);
      }
      // The engine's own spelling (`executor::Resume::wire`): a `Pause` waits
      // with `NEVER` or `TERMINAL:*`; a `LoopUntil` with `ITERATE`.
      let Some(run) = detail.runs.iter().find(|r| {
          r.state == "PAUSED"
              && r.resume_with.as_deref().is_some_and(|w| w == "NEVER" || w.starts_with("TERMINAL:"))
      }) else {
          return Ok(Waiting::LoopOnly);
      };
      let flow = parse(&detail.source).map_err(|e| {
          Error::Internal(anyhow::anyhow!("its frozen source does not parse: {}", e.message))
      })?;
      let on_resume = crate::spec::path::find_task(&flow, &run.task_id)
          .and_then(|t| match &t.kind {
              crate::models::TaskKind::Pause(p) => p.on_resume.clone(),
              _ => None,
          })
          .unwrap_or_default();
      Ok(Waiting::Pause { run: run.id, task_id: run.task_id.clone(), on_resume })
  }
  ```
  `detail.runs` theo thứ tự `run_ix` (doc của `ExecutionDto`), khớp với `first_waiting_pause`. Worker quyết định lại dưới lock và trả `Moved` nếu chúng không khớp.
- **Multipart, tổng quát hoá.**
  - `receive_parts` nhận thêm `declared`, `owner` và `sink`, và giữ `parsed` (cho namespace và id của flow). `read_parts` nhận như vậy.
  - Một part được tra trong `declared`. "No input" đọc là ``"{owner} declares no input `{name}`: each multipart part is named by an input id"``, và start truyền `owner = "the flow"`, nên text của nó không đổi.
  - Một file part được lưu theo sink:
    - `Start { execution }`: `upload_input(fs, ws, &parsed.namespace, &parsed.id, execution, input, &file_name, &mut body)`, như hiện nay;
    - `Resume { execution, task_id, task_run, request }`: `upload_resume_input(...)`;
    - `Discard`: xả field bằng `while field.chunk().await…?.is_some() {}` và trả **tên file** làm giá trị (D8: "the would-be value, not a URI"). Không lưu gì và `uploading` vẫn false.
  - Khi chính nó thất bại, `receive_parts` xoá `sink_prefix(...)` khi `uploading`.
  - `sink_prefix`:
    - `Start` cho `fs.execution_prefix(ws, ns, flow, execution)`;
    - `Resume` cho `fs.resume_prefix(ws, ns, flow, execution, task_id, task_run, request)`;
    - `Discard` cho `None`.
  - `discard_uploads(workspace, parsed, execution)` trở thành `discard_prefix(prefix)` (cùng `delete_prefix` và warning). Start và replay tính prefix bằng `sink_prefix(.., &PartSink::Start { execution })`.
  - **Một resume không bao giờ được xoá prefix của chính execution.** Đó là lý do sink tồn tại. Test 9 kiểm tra điều này.
  - `multipart_first_part(req) -> Result<Option<String>, Response>`: dựng `axum::extract::Multipart::from_request(req, &())` và trả tên field đầu tiên, hoặc `None`. Nó được dùng khi `onResume` rỗng, để trả `resume_inputs_unexpected` mà không lưu gì.
- **`pause_execution`** (D3):
  ```rust
  async fn pause_execution(
      State(st): State<WorkflowState>,
      WsContext(ctx): WsContext,
      Path((_ws, id)): Path<(String, Uuid)>,
  ) -> std::result::Result<Json<ExecutionDto>, ExecOpError> {
      egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
      let ws = ctx.workspace_id.0;
      let before = st.executions.get(ws, id).await?;
      if let (Some(root), Some(_)) = (before.execution.parent_execution_id, before.execution.loop_index) {
          return Err(ExecOpError::loop_iteration(id, root, "cannot be paused"));
      }
      live_flow_of(&st, ws, &before).await?;
      match crate::worker::pause_by_hand(&st.db.pool, ws, id).await? {
          crate::worker::Paused::Done => {}
          crate::worker::Paused::NotRunning(state) => {
              return Err(ExecOpError::conflict(
                  NOT_RUNNING_CODE,
                  format!("execution {id} is {state}; only a RUNNING or RESTARTED execution can be paused"),
              ));
          }
      }
      st.audit.emit(egret_audit::mutation_for(
          &ctx.principal,
          Some(ws),
          "workflow_execution:pause",
          "workflow_execution",
          Some(id.to_string()),
          serde_json::json!({ "from_state": before.execution.state }),
      ));
      let detail = st.executions.get(ws, id).await?;
      Ok(Json(ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)))
  }
  ```
- **`resume_execution`** (D5–D7, D9). Các bước của nó:
  1. `require(WF_MANAGE)`.
  2. `before = executions.get` (404).
  3. `live_flow_of` (404).
  4. Không `PAUSED`: 409 `not_paused`, "execution {id} is {state}, not PAUSED; only a paused execution can be resumed".
  5. `waiting = waiting_of(&before)`.
  6. D7, trước khi đọc bất kỳ body nào: `if let Waiting::Pause { on_resume, .. } = &waiting && !on_resume.is_empty()`, rồi `check_resume_inputs(&mut conn, &parse(&before.source)?, before.execution.flow_id, caller)` (400, không có code).
  7. Body. `let request = Uuid::new_v4();`
     - multipart, với `Waiting::Pause { on_resume }` không rỗng: `receive_parts(ws, &parsed, on_resume, &format!("the Pause `{task_id}`"), &PartSink::Resume { execution: id, task_id, task_run: run, request }, req)` → `Some(inputs)`, `files`, và `uploaded_prefix = sink_prefix(..)` khi files không rỗng;
     - multipart, trường hợp khác: `multipart_first_part(req)`. `Some(_)` cho 400 `resume_inputs_unexpected`; `None` cho không có input;
     - JSON (`read_json_body`): `inputs` vắng hoặc `null` cho `None`; một object cho `Some(map)`; bất kỳ thứ gì khác cho 400 với message của `start_inputs`.
  8. `crate::worker::resume_with(&st.db.pool, &ResumeRequest { id, inputs: inputs.as_ref(), files: &files, expected })`, với `expected = Some(run)` cho `Waiting::Pause` và `None` trong các trường hợp còn lại.
  9. Ánh xạ câu trả lời:
     - `Released(_)` và `ByHand`: OK;
     - `NotPaused`: 409 `not_paused` ("… its pause ended before the resume reached it");
     - `NoPauseWaiting`: text 409 không code hiện nay;
     - `Refused(InputsUnexpected)`: 400 `resume_inputs_unexpected`, "execution {id} takes no inputs to resume: it is paused by hand, or the waiting Pause declares no onResume";
     - `Refused(Invalid(m))`: 400 không code `m`;
     - `Refused(Moved)`: 409 không code, "the Pause execution {id} waited on was resumed by another request; refresh and resume again".

     Với bất kỳ `Err` hoặc từ chối nào mà `uploaded_prefix` được đặt: `discard_prefix(prefix)`.
  10. Audit `workflow_execution:resume` với
      ```json
      {"from_state": "...", "by_hand": true|false, "task_run_id": "<uuid>|null", "inputs_given": true|false}
      ```
      `by_hand` = `matches!(outcome, Resumed::ByHand)`; `task_run_id` = run của `Waiting::Pause`; `inputs_given` = input không rỗng.
  11. Đọc lại, 200.
- **`validate_resume_inputs`** (D8). Các bước của nó:
  1. `require(WF_MANAGE)`.
  2. `get` (404).
  3. `live_flow_of` (404).
  4. Không `PAUSED`: 409 `not_paused`.
  5. `waiting_of`.
  6. `declared` = danh sách `onResume` (rỗng cho `ByHand`/`LoopOnly`).
  7. Các giá trị được post:
     - multipart: `receive_parts(.., declared, owner, &PartSink::Discard, req)`;
     - nếu không: `read_json_body` + `start_inputs`.
  8. `Ok(Json(inputs_answer(declared_resume_inputs(&before.source, task_id), &flow, &declared, &posted)))`. Không audit.

  Refactor `validate_inputs_answer(row, flow, posted)` thành `inputs_answer(declared_yaml: HashMap<String, Value>, flow: &Flow, inputs: &[Input], posted: &Value) -> ValidateInputsDto`, cùng body, với `crate::worker::input_outcomes_for(flow, inputs, posted)`, và `types` dựng từ `inputs`. `validate_execution_inputs` gọi `inputs_answer(declared_inputs(&row.source), &flow, &flow.inputs, &posted)`.

  `declared_resume_inputs(source, task_id: Option<&str>)` đi YAML như JSON và trả, theo id, các entry của mảng `onResume` của object đầu tiên có `id` bằng `task_id` và giữ một mảng `onResume`. Viết nó thành một `fn find<'a>(v: &'a Value, id: &str) -> Option<&'a Vec<Value>>` đệ quy trên object và array. `None` cho một map rỗng.
- **`ExecutionDto::new`** tính, trước khi `d.execution` bị move:
  ```rust
  let paused_by_hand = crate::executor::paused_by_hand_of(
      crate::worker::from_wire(&d.execution.state),
      d.runs.iter().map(|r| crate::worker::from_wire(&r.state)),
  );
  ```
  Doc của field: "`true` when the execution is `PAUSED` and no task run waits: it was paused by hand (`POST …/pause`). A resume sends it on; it takes no inputs. `false` for a `Pause` task or a `LoopUntil` waiting, and for every other state."
- **`ExecOpError`** nhận thêm `fn not_paused(id: Uuid, state: &str) -> Self` = `Self::conflict(NOT_PAUSED_CODE, format!("execution {id} is {state}, not PAUSED; only a paused execution can be resumed"))`.
- **Router** (`routes.rs:4887-4890`):
  ```rust
  .route(
      "/ws/{workspace}/workflows/executions/{id}/pause",
      post(pause_execution),
  )
  .route(
      "/ws/{workspace}/workflows/executions/{id}/resume",
      // As the start route: the handler limits each content type itself.
      post(resume_execution).layer(axum::extract::DefaultBodyLimit::disable()),
  )
  .route(
      "/ws/{workspace}/workflows/executions/{id}/resume/validate",
      post(validate_resume_inputs).layer(axum::extract::DefaultBodyLimit::disable()),
  )
  ```
- **OpenAPI.** Thêm `pause_execution` và `validate_resume_inputs` vào `paths(...)` cạnh `resume_execution` (`routes.rs:4943`).
  - **Pause:** 200 `ExecutionDto` ("The execution is `PAUSED`. Runs already running finish and record their results; no run is created or started until it is resumed."); 400 (iteration loop, nêu tên root của nó); 401; 403; 404 (không có execution như vậy; flow của nó ở trash hoặc đã xoá); 409 `not_running` (không phải `RUNNING` hay `RESTARTED`, kể cả `KILLING`; message nêu tên state); 500.
  - **Resume:**
    - `request_body(description = "Optional. No body, `{}`, or `\"inputs\": null` resumes with no inputs. With `inputs` (JSON, `StartExecutionRequest`, at most 2 MiB; no `Content-Type` is read as JSON) or `multipart/form-data` (`StartExecutionMultipart`; one part per input id, a file part stored under the Pause run, at most `workflow.storage.max_file_bytes` + 1 MiB), the values are checked against the first waiting Pause's `onResume` as a start checks the flow's `inputs:`, and written to its outputs as `onResume`.", content((StartExecutionRequest = "application/json"), (StartExecutionMultipart = "multipart/form-data")))`;
    - 200 (bị Pause thủ công: về lại `RUNNING`; một `Pause`: nó kết thúc `SUCCESS` với `outputs.onResume` và `outputs.resumed`, mọi `Pause` đang chờ khác kết thúc `SUCCESS` với `onResume: {}`; body là detail sau đó);
    - 400 (`resume_inputs_unexpected`; không code: một input bị từ chối hoặc không khai báo, được nêu tên; flow chạy `egret.sql`, Pause khai báo `onResume`, và caller không phải creator của flow; không có master key cho một input SECRET);
    - 401; 403; 404 (execution; flow ở trash hoặc đã xoá);
    - 409 (`not_paused`; không code: chỉ một `LoopUntil` đang chờ, hoặc một resume khác thắng);
    - 413; 415; 500; 503.
  - **Validate:** 200 `ValidateInputsDto` ("Every `onResume` input of the first waiting Pause, with the start check's errors; an empty body answers each with its default; an empty list when the execution is paused by hand or the Pause has no `onResume`. Nothing is stored: a file part is read and dropped, and its value is its file name. No audit."); 400; 401; 403; 404; 409 `not_paused`; 413; 415; 500.
- [ ] **Route test fail** (danh sách bên dưới) trong `tests/pause_resume_routes.rs`. Copy `setup`, `flow_of`, `start`, `exec_state`, `expect_error` từ `tests/restart_routes.rs:77-183`, và `multipart_body`/`Part`/`BOUNDARY` từ `tests/file_inputs.rs:135-166`. Chạy `cargo test -p egret-workflow --test pause_resume_routes`: red.
- [ ] Triển khai.
- [ ] Chạy, từng cái một:
  - `cargo test -p egret-workflow --test pause_resume_routes --test pause_resume --test executions_routes --test restart_routes --test replay_routes --test routes --test inputs_start --test file_inputs --test secret_inputs --test validate_inputs_route`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] **OpenAPI:**
  ```bash
  cargo run -p egret-bin -- openapi > openapi.json
  cargo test -p egret-server committed_openapi_json_is_current
  ```
  `generated_client_was_emitted_from_the_committed_spec` giờ fail cho tới T6; điều đó là dự kiến, và controller chạy T6 tiếp theo.
- [ ] Commit: `feat(workflow): pause, resume with inputs and validate resume inputs routes (package 7c)`.

**Route test** (`tests/pause_resume_routes.rs`). Mỗi lần từ chối đi cặp với một đối chứng được chấp nhận.

1. `pause_answers_200_paused_and_is_audited`:
   - `one_task` đã start (`a` `RUNNING`). `POST …/pause` cho 200, `state == "PAUSED"`, `paused_by_hand == true`.
   - Audit là `("workflow_execution:pause", Some(ex), {"from_state":"RUNNING"})`.
   - `GET` execution: `paused_by_hand == true`. Một execution của task `Pause`: `paused_by_hand == false`.
2. `pause_resume_and_validate_need_workflow_manage`: một user chỉ có `workflow:read` nhận 403 trên cả ba; state không đổi.
3. `pause_refuses_what_is_not_running`:
   - `PAUSED` (pause hai lần): 409 `not_running`;
   - `KILLING`: kill một `one_task` mà run của nó `RUNNING` (một lần kill một run đã claim giữ `KILLING`; claim nó trước bằng `egret_workflow::worker::claim(&pool, 10)`), rồi 409 `not_running`, với message chứa `KILLING`;
   - terminal: 409 `not_running`.
4. `resume_of_a_running_execution_is_409_not_paused` (và validate tương tự).
5. `resume_by_hand_runs_on_and_inputs_are_refused`:
   - pause; `POST …/resume` với `{"inputs":{"x":1}}` cho 400 `resume_inputs_unexpected`, và state vẫn `PAUSED`;
   - multipart với một text part cho 400 `resume_inputs_unexpected`;
   - không body cho 200, và state không phải `PAUSED`;
   - audit `{"from_state":"PAUSED","by_hand":true,"task_run_id":null,"inputs_given":false}`.
6. `resume_with_inputs_writes_them_and_a_refused_one_is_named`:
   - flow của test 16 ở T4 (không có `egret.sql`), được đẩy tới `PAUSED` bằng `settle_everything`;
   - `{"inputs":{"who":"ada","n":"x"}}` cho 400, và lỗi chứa `` `n` ``;
   - `{"inputs":{"who":"ada","n":"3","zz":1}}` cho 400 chứa `` `zz` ``;
   - `{"inputs":{"who":"ada","n":"3"}}` cho 200; `outputs.onResume == {"who":"ada","n":3}` của run `p`;
   - audit `{"by_hand":false,"task_run_id":<p's id>,"inputs_given":true}`.
7. `a_pause_without_on_resume_keeps_todays_resume`: một `Pause` không giới hạn. Không body cho 200; `{"inputs":{"a":1}}` cho 400 `resume_inputs_unexpected` (trên một execution mới).
8. `only_a_loop_until_waiting_is_still_409`: text hiện nay "Nothing to resume", không code.
9. `a_multipart_file_resume_stores_under_the_pause_run_and_a_refused_one_deletes_only_its_own`:
   - Arrange: `install_mem_for_tests()`; flow của test 6 với `doc: FILE`, `PAUSED`; cũng start một execution **thứ hai** của một flow có input FILE ở start được upload bằng multipart, để có một object `inputs/` trong store.
   - Bị từ chối: multipart với `who=ada`, `n=x` và `doc` = `a.txt` cho 400. Rồi `fs.store.list(&fs.resume_root(ws, "t", "approve", ex, "p", run), None)` không cho object nào, và object `inputs/` của execution kia vẫn tồn tại (`head` là `Some`).
   - Được chấp nhận: multipart với `who=ada`, `n=3` và `doc` cho 200. `outputs.onResume.doc` là một URI chứa `/tasks/p/{run}/resume/`. `GET /api/ws/{ws}/workflows/executions/{ex}/files?path=<uri>` (route file mà `tests/storage_files_api.rs` dùng; copy lời gọi của nó) trả 200 với `hello`.
10. `the_egret_sql_gate_applies_only_with_on_resume`:
   - một flow có task `egret.sql` sau một `Pause` có `onResume`, do user A (chủ workspace, `flow_of`) nộp. User B với `workflow:manage` resume: 400, và message chứa "only the flow's creator can resume it"; state vẫn `PAUSED`. User A resume: 200.
   - Cùng flow với một `Pause` không có `onResume`: user B nhận 200.
   - Một Pause thủ công của flow `egret.sql`: user B nhận 200.

   `egret.sql` cần một entry registry được phục vụ. Copy cách nộp flow của test `the_egret_sql_gate…` trong `tests/restart_routes.rs` (grep `egret.sql` ở đó). Nó không bao giờ chạy SQL: execution chỉ được đẩy tới `Pause`.
11. `a_trashed_flow_is_404_and_a_disabled_one_is_not`:
   - `UPDATE workflow.flows SET deleted_at = now()`: pause, resume và validate cho 404;
   - khôi phục nó, rồi đặt `disabled: true` trong YAML của flow ở `workflow.flows.source`: pause cho 200 và resume cho 200.
12. `a_loop_iteration_is_400_naming_its_root`: flow `Loop` của test 6 ở T3. Pause id execution của iteration đầu: 400, và lỗi chứa id root; không gì thay đổi.
13. `validate_with_an_empty_body_answers_the_defaults` (hình dạng của `resume-validate.yaml`: `approved: BOOL, defaults: true`): 200; `inputs[0].value == true`, `isDefault == true`, `errors == []`. Một execution bị Pause thủ công: `inputs == []`.
14. `validate_with_values_answers_per_input_errors_and_stores_nothing`:
   - JSON `{"inputs":{"n":"x"}}`: 200, và entry `n` có một lỗi chứa `` `n` ``;
   - multipart với một file part `doc`: 200; `fs.store.list(resume_root…)` rỗng; giá trị `doc` là `"a.txt"`; không có dòng audit.
15. `the_workflows_entitlement_gates_the_three_routes` (như `restart_routes.rs:222-232`).
16. `database_url_is_exported_rather_than_read_from_dotenv`.

---

## T6. Client được sinh ra (controller)

- [ ] Trong `frontend/`, sau kiểm tra `node_modules` ở Global Constraints, chạy `npm run gen:api` (không bao giờ `orval` trần).
- [ ] Kiểm tra `frontend/src/api/generated/workflow/workflow.ts` có:
  - `useWorkflowPauseExecution`;
  - `useWorkflowValidateResumeInputs` và hàm raw của nó `workflowValidateResumeInputs`;
  - `useWorkflowResumeExecution`, mà biến của nó giờ mang `data`;
  - `getWorkflowResumeExecutionUrl`;
  - `ExecutionDto.paused_by_hand: boolean`.

  Ghi lại tên chính xác cho T7.
- [ ] Chạy `cargo test -p egret-server generated_client_was_emitted_from_the_committed_spec` (xanh).
- [ ] Commit: `chore(frontend): regenerate the API client (package 7c)`.

---

## T7. Frontend: Pause, ResumeDialog với input, "Paused by hand" (spec D11)

Đọc `docs/en/frontend-rules.md` trước; mọi quy tắc MUST đều áp dụng. Những quy tắc task này gặp:
- FE-ACTION-1 (confirm);
- FE-ACTION-2 (chỉ toast khi ở lại trang);
- FE-ACTION-3 (lỗi hiện message của server và không xoá gì);
- FE-ACTION-4 (một control bị disable nói lý do);
- FE-STATE-2 (skeleton);
- FE-FORM-2 và FE-FORM-4 (label, lỗi cạnh field);
- FE-COPY-1 đến FE-COPY-3;
- FE-TEST-1.

**File:**
- Sửa:
  - `frontend/src/components/workflows/executions/actions.ts` (+ `__tests__/actions.ops.test.ts`);
  - `frontend/src/components/workflows/run/inputModel.ts:120-164`;
  - `frontend/src/routes/workflows/ExecutionDetail.tsx` (`:4`, `:42-57`, `:269-353`, `:355-542`);
  - `frontend/src/routes/workflows/queryKeys.ts`;
  - `routes/workflows/__tests__/ExecutionDetail.test.tsx`;
  - mọi test fixture mà `npm run typecheck` báo thiếu `paused_by_hand` (dự kiến: `ReplayDialog.test.tsx:25-46`, `ExecutionOutputsTab.test.tsx`, `ExecutionDetail.test.tsx`; thêm `paused_by_hand: false`).
- Tạo: `frontend/src/components/workflows/executions/ResumeDialog.tsx` và `__tests__/ResumeDialog.test.tsx`.

**Interface.**

Dùng: các hook của T6; `inputModel` (`formFromInputs`, `validateForm`, `requestFromForm`, `validationRequestFromForm`, `validationView`, `serverErrorsByInput`, `unsupportedInputs`); `InputField`; `useRefreshExecution`; `runExecutionAction`.

Sinh ra:
```ts
// actions.ts
export function showsPause(execution: Pick<ExecutionDto, "state">): boolean; // RUNNING | RESTARTED | KILLING
export const PAUSE_KILLING_REASON: string;
export function canPause(execution: Pick<ExecutionDto, "state" | "parent_execution_id" | "loop_index">): Enablement;
export function canResume(execution: Pick<ExecutionDto, "runs" | "paused_by_hand">): Enablement; // changed
export function pauseRefusal(code: string | undefined): string | null;
export function resumeRefusal(code: string | undefined): string | null;
// inputModel.ts
export function runInputFrom(item: unknown): RunInput | null;
// ResumeDialog.tsx
export function ResumeDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (open: boolean) => void }): JSX.Element;
// queryKeys.ts
resumeInputs: (id: string) => ["workflows", "execution", id, "resume-inputs"] as const,
```

**Code.**

Phần bổ sung cho `actions.ts` (sau `canResume`):
```ts
/**
 * Pause by hand (package 7c, D3, D11): shown while the execution runs, and while it is being
 * killed, where it is disabled with the reason (the server refuses `KILLING`).
 */
export function showsPause(execution: Pick<ExecutionDto, "state">): boolean {
  return execution.state === "RUNNING" || execution.state === "RESTARTED" || execution.state === "KILLING";
}

export const PAUSE_KILLING_REASON = "The execution is being killed, so it cannot be paused";

/** Pause, for an execution that `showsPause`: not while it is killed, and not a loop iteration. */
export function canPause(execution: Pick<ExecutionDto, "state"> & LoopFields): Enablement {
  if (execution.state === "KILLING") return { ok: false, reason: PAUSE_KILLING_REASON };
  return isLoopIteration(execution) ? { ok: false, reason: iterationReason(execution, "Pause") } : { ok: true };
}

/** One sentence for the pause route's coded refusal; `null` otherwise. */
export function pauseRefusal(code: string | undefined): string | null {
  return code === "not_running" ? "The execution is no longer running, so it cannot be paused. Refresh to see its latest state." : null;
}

/** One sentence for each coded refusal of the resume and resume-validate routes; `null` otherwise. */
export function resumeRefusal(code: string | undefined): string | null {
  switch (code) {
    case "not_paused":
      return "The execution is no longer paused. Refresh to see its latest state.";
    case "resume_inputs_unexpected":
      return "This execution takes no inputs to resume. Resume it without inputs.";
    default:
      return null;
  }
}
```
`LoopFields` và `iterationReason` được khai báo ở phía dưới file (`:190-199`). Chuyển cả hai lên trên `showsPause`. `canResume` trở thành:
```ts
/** Resume, for an execution that `showsResume`: enabled when it was paused by hand or a `Pause` task waits (spec §5.2; 7c D1). */
export function canResume(execution: Pick<ExecutionDto, "runs" | "paused_by_hand">): Enablement {
  return execution.paused_by_hand || aPauseIsWaiting(execution) ? { ok: true } : { ok: false, reason: LOOP_WAIT_REASON };
}
```

`inputModel.ts`: chuyển thân vòng lặp của `inputsFromSource` (`:133-162`) vào
```ts
/** One `inputs:` (or `onResume:`) entry of a flow's YAML as the form's model; `null` for one with no string `id` or `type`. */
export function runInputFrom(item: unknown): RunInput | null {
  if (!isRecord(item) || typeof item.id !== "string" || typeof item.type !== "string") return null;
  // ... the existing object literal, unchanged ...
}
```
`inputsFromSource` khi đó làm `const inputs = raw.map(runInputFrom).filter((i): i is RunInput => i !== null);`.

`ResumeDialog.tsx`:
```tsx
import { useId, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { customInstance } from "@/api/mutator";
import {
  getWorkflowResumeExecutionUrl,
  useWorkflowResumeExecution,
  useWorkflowValidateResumeInputs,
  workflowValidateResumeInputs,
  type workflowResumeExecutionResponse,
} from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { resumeFeedback, resumeRefusal } from "@/components/workflows/executions/actions";
import { useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  formFromInputs,
  requestFromForm,
  runInputFrom,
  serverErrorsByInput,
  unsupportedInputs,
  validateForm,
  validationRequestFromForm,
  validationView,
  type FieldValue,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const BY_HAND = "The execution continues: the tasks that were waiting start now.";
const NO_INPUTS =
  "Every paused task ends now as a success, whatever its duration or timeout, and the execution continues with the tasks after it.";
const WITH_INPUTS = "Fill in what the paused task asks for. It then ends as a success and the execution continues.";

/**
 * Resume (package 7c, D11): asks `resume/validate` with an empty body when it opens, which answers
 * the waiting Pause's `onResume` inputs with their defaults. With none it is today's confirm; with
 * some it shows the Run-flow inputs form, checks the values on the server first, then resumes with
 * JSON (multipart when a file is chosen). A refusal stays in the dialog (FE-ACTION-3).
 */
export function ResumeDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <AlertDialog open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto">{props.open ? <ResumeForm {...props} /> : null}</AlertDialogContent>
    </AlertDialog>
  );
}

function ResumeForm({ ws, execution, onOpenChange }: { ws: string; execution: ExecutionDto; onOpenChange: (open: boolean) => void }) {
  const idPrefix = useId();
  const refresh = useRefreshExecution(ws, execution.id);
  const resume = useWorkflowResumeExecution();
  const validate = useWorkflowValidateResumeInputs();
  const asked = useQuery({
    queryKey: workflowsKeys.resumeInputs(execution.id),
    queryFn: () => workflowValidateResumeInputs(ws, execution.id, {}),
    staleTime: 0,
    gcTime: 0,
    retry: false,
  });
  const inputs: RunInput[] = useMemo(
    () =>
      asked.data?.status === 200
        ? asked.data.data.inputs.map((o) => runInputFrom(o.input)).filter((i): i is RunInput => i !== null)
        : [],
    [asked.data],
  );
  const [values, setValues] = useState<Record<string, FieldValue>>({});
  const form: FormState = useMemo(() => {
    const base = formFromInputs(inputs);
    return { ...base, values: { ...base.values, ...values } };
  }, [inputs, values]);
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const loading = asked.isPending;
  const loadError =
    asked.error instanceof ApiError
      ? (resumeRefusal(asked.error.code) ?? asked.error.message)
      : asked.isError
        ? "The inputs could not be loaded. Close the dialog and try again."
        : null;
  const unsupported = unsupportedInputs(inputs);
  const description = execution.paused_by_hand ? BY_HAND : inputs.length > 0 ? WITH_INPUTS : NO_INPUTS;

  const setValue = (id: string, v: FieldValue) => {
    setValues((s) => ({ ...s, [id]: v }));
    setClientErrors((e) => without(e, id));
    setServerErrors((e) => without(e, id));
  };

  const confirm = async () => {
    setPending(true);
    setError(null);
    setServerErrors({});
    try {
      let res: workflowResumeExecutionResponse;
      if (inputs.length === 0) {
        res = await resume.mutateAsync({ workspace: ws, id: execution.id, data: {} });
      } else {
        const errors = validateForm(form);
        setClientErrors(errors);
        if (Object.keys(errors).length > 0) return;
        // The server's own check first (D11): its per-input errors sit beside their fields.
        const checked = await validate.mutateAsync({ workspace: ws, id: execution.id, data: validationRequestFromForm(form) });
        if (checked.status === 200) {
          const view = validationView(checked.data, inputs);
          if (Object.keys(view.errors).length > 0) {
            setServerErrors(view.errors);
            return;
          }
        }
        const req = requestFromForm(form);
        res =
          "json" in req
            ? await resume.mutateAsync({ workspace: ws, id: execution.id, data: req.json })
            : // The generated function JSON.stringifies its body: a multipart resume goes through the mutator with the FormData (as RunFlow does).
              await customInstance<workflowResumeExecutionResponse>(getWorkflowResumeExecutionUrl(ws, execution.id), {
                method: "POST",
                body: req.multipart,
              });
      }
      if (res.status !== 200) throw new Error("The execution could not be resumed. Try again.");
      refresh();
      // The page stays, so a toast confirms it (FE-ACTION-2), in terms of the answered state.
      const feedback = resumeFeedback(res.data.state);
      if (feedback.tone === "success") toast.success(feedback.message);
      else toast.warning(feedback.message);
      onOpenChange(false);
    } catch (e) {
      const sentence = e instanceof ApiError ? resumeRefusal(e.code) : null;
      if (sentence) {
        setError(sentence);
        refresh();
      } else {
        const message = e instanceof Error && e.message ? e.message : "The execution could not be resumed. Try again.";
        const { fields, rest } = serverErrorsByInput(message, inputs);
        if (Object.keys(fields).length > 0) {
          setServerErrors(fields);
          if (rest.length > 0) setError(rest.join(" "));
        } else setError(message);
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>Resume this execution?</AlertDialogTitle>
        <AlertDialogDescription>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      {loading ? (
        <div aria-busy="true" className="grid gap-2">
          <Skeleton className="h-4 w-48" />
          <span className="sr-only">Reading the inputs</span>
        </div>
      ) : loadError ? (
        <Alert variant="destructive">
          <AlertDescription>{loadError}</AlertDescription>
        </Alert>
      ) : inputs.length > 0 ? (
        <div className="grid gap-3">
          {unsupported.length > 0 ? (
            <p id={`${idPrefix}-unsupported`} className="text-sm text-muted-foreground">
              {`${unsupported.map((u) => `${u.id} (${u.type})`).join(", ")} cannot be entered in this form`}
            </p>
          ) : null}
          {inputs.map((input) => (
            <InputField
              key={input.id}
              input={input}
              value={form.values[input.id] ?? null}
              onChange={(v) => setValue(input.id, v)}
              error={serverErrors[input.id] ?? clientErrors[input.id]}
              idPrefix={idPrefix}
            />
          ))}
        </div>
      ) : null}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
        <Button
          type="button"
          disabled={pending || loading || loadError !== null || unsupported.length > 0}
          aria-describedby={unsupported.length > 0 ? `${idPrefix}-unsupported` : undefined}
          onClick={() => void confirm()}
        >
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Resume
        </Button>
      </AlertDialogFooter>
    </>
  );
}

function without(errors: Record<string, string>, id: string): Record<string, string> {
  if (!(id in errors)) return errors;
  const next = { ...errors };
  delete next[id];
  return next;
}
```
Dùng đúng tên mà T6 đã ghi. Nếu orval đặt tên hàm raw khác (ví dụ có hậu tố `Response`), import tên đó. Nếu shape kết quả `queryFn` của `useQuery` khác với shape của các hook (`{ status, data }`), theo cách `useWorkflowGetExecution` được dùng ở `ExecutionDetail.tsx:107-114`.

`ExecutionDetail.tsx`:
- **Import:** `Pause as PauseIcon` từ lucide; `useWorkflowPauseExecution`; `canPause`, `showsPause` và `pauseRefusal` từ actions; `ResumeDialog`. Xoá `resumeFeedback` và `useWorkflowResumeExecution` khỏi import của file này khi `runResume` đã mất.
- **`type Pending`** nhận thêm `"pause"`.
- **Trong `ExecutionActions`:**
  ```tsx
  const pause = useWorkflowPauseExecution();
  const pauseShown = showsPause(execution);
  const pauseState = canPause(execution);
  const runPause = () =>
    runExecutionAction({
      call: () => pause.mutateAsync({ workspace: ws, id: execution.id }),
      expected: 200,
      failure: "The execution could not be paused. Try again.",
      refresh,
      refusal: pauseRefusal,
      onSuccess: () => toast.success("Paused the execution"),
    });
  ```
  Xoá `runResume` và hook `resume`.
- **Nút:** trước nút Kill, thêm
  ```tsx
  {pauseShown ? (
    <ReasonButton enablement={pauseState} reasonId={`${reasonsId}-pause`} variant="outline" onClick={() => setOpen("pause")}>
      <PauseIcon aria-hidden />
      Pause
    </ReasonButton>
  ) : null}
  ```
  và entry `DisabledReasons` tương ứng `{ id: `${reasonsId}-pause`, label: "Pause", enablement: pauseState }`.
- **Thay `ConfirmAction` của Resume** (`:489-497`) bằng
  ```tsx
  <ResumeDialog ws={ws} execution={execution} open={open === "resume"} onOpenChange={(o) => setOpen(o ? "resume" : null)} />
  <ConfirmAction
    open={open === "pause"}
    onOpenChange={(o) => setOpen(o ? "pause" : null)}
    title="Pause this execution?"
    description="Runs that are already running finish; nothing new starts until you resume."
    confirmLabel="Pause"
    onConfirm={runPause}
    fallbackError="The execution could not be paused. Try again."
  />
  ```
  `runExecutionAction` phải throw khi lỗi để `ConfirmAction` giữ dialog mở. Nếu nó return thay vì throw, theo cách `runSetStatus` được nối với `ChangeStateDialog` và làm `onConfirm` throw câu từ chối.
- **Header** (`:284`): `status={<span className="flex items-center gap-2"><StateChip state={execution.state} kind="execution" />{execution.paused_by_hand ? <span className="text-sm text-muted-foreground">Paused by hand</span> : null}</span>}`.

`queryKeys.ts`: thêm `resumeInputs: (id: string) => ["workflows", "execution", id, "resume-inputs"] as const,` cạnh `execution`.

**Test** (vitest; Arrange / Act / Assert; mỗi test đều có thể fail):
- `actions.ops.test.ts`:
  - `showsPause` là true với `RUNNING`, `RESTARTED` và `KILLING`, false với `PAUSED`, `SUCCESS` và `CREATED`;
  - `canPause` bị disable với `PAUSE_KILLING_REASON` trên `KILLING`, bị disable với một lý do nêu tên parent trên một iteration loop, và ok trên `RUNNING`;
  - `canResume` ok với `{paused_by_hand: true, runs: []}`, ok với một run `NEVER`, và cho `LOOP_WAIT_REASON` với chỉ một run `ITERATE`;
  - `pauseRefusal("not_running")` và cả hai code của `resumeRefusal` đều khác nhau và không null; một code lạ cho `null`.
- `ResumeDialog.test.tsx`: copy mẫu class `Server` của `ReplayDialog.test.tsx:61-80`, với các route `POST …/resume/validate` → `this.validate` và `POST …/resume` → `this.resume`. Các test:
  1. không input (`validate` trả `{id, namespace, inputs: [], checks: []}`): mô tả là `NO_INPUTS`; Resume post `{}` tới `/resume` một lần; toast `"Resumed the execution"`;
  2. bị Pause thủ công (`paused_by_hand: true`): mô tả là `BY_HAND`;
  3. có input: `validate` trả một INT `n` (`input: {id:"n", type:"INT"}`, `value: null`). Field `n` có label. Gõ `x` và bấm Resume hiện lỗi client cạnh `n`, và `/resume` không được gọi. Gõ `3`: `validate` được gọi với `{inputs:{n:"3"}}`, rồi `/resume` với `{inputs:{n:"3"}}`;
  4. một lỗi field từ server: lời gọi `validate` thứ hai trả `errors: [{message: "Invalid value for input `n`. Cause: it must be more than `5`", renderError: false}]`. Message hiện cạnh `n`, dialog vẫn mở, và `/resume` không được gọi;
  5. một từ chối có code: `/resume` trả 400 `{code: "resume_inputs_unexpected"}`. Câu hiện ra và dialog vẫn mở;
  6. đang tải: trong khi `validate` pending, một skeleton với "Reading the inputs" hiện và Resume bị disable;
  7. một input FILE: chọn file gửi một body `FormData` (`body` là một instance của `FormData`).
- `ExecutionDetail.test.tsx`:
  - mở rộng fake server (`:92-122`) với `pause: Reply` (`POST ${EXEC_PATH}/pause`) và `resumeValidate: Reply` (`POST ${EXEC_PATH}/resume/validate`, mặc định `{status: 200, body: {id: "f", namespace: "n", inputs: [], checks: []}}`);
  - cập nhật các test Resume (`:384-482`): tiêu đề dialog vẫn là "Resume this execution?" và test từ chối vẫn tìm thấy message của server;
  - thêm: một execution `RUNNING` hiện **Pause**; xác nhận post `/pause` và toast "Paused the execution"; một 409 `not_running` giữ dialog mở với câu của `pauseRefusal`; `KILLING` hiện Pause bị disable với `PAUSE_KILLING_REASON`; `paused_by_hand: true` hiện "Paused by hand" ở header và một Resume được enable.
- [ ] Chạy, trong `frontend/`:
  - `npm run typecheck`;
  - `npm run lint`;
  - `npx vitest run src/components/workflows/executions src/routes/workflows/__tests__/ExecutionDetail.test.tsx src/components/workflows/run`;
  - `npm run build`;
  - `npm run test:bundle:workflows`.
- [ ] Xoá symlink `node_modules` nếu T7 đã tạo nó.
- [ ] Commit: `feat(workflows): pause by hand, resume with inputs, paused-by-hand note (package 7c)`.

---

## T8. Docs trong repo này, và các gate (spec D13)

- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Các dòng không được sửa; thêm một block ghi chú có ghi ngày theo kiểu `:162-183`:
  ```markdown
  **Notes, 2026-11-15 (package 7c, `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` Q1; the rows above are not edited).**
  13. **#7:** 7c builds pause by hand (`POST …/pause`), resume with `onResume` inputs and resume-validate (`POST …/resume/validate`). Row #7's "pause" and "Resume with `onResume` inputs, and resume-validate" are 7c's.
  14. **#9:** `Pause` `onResume` moved to 7c (user decision Q1, 2026-11-15). #9 keeps `Pause` `behavior:`, `onPause:`, `errors:` and `finally:` (still refused, `unsupported_pause_property`) and `AllowFailure` block `retry:`.
  ```
  Phụ lục B `:425` (`flow.Pause`) và `:556` (`POST /{id}/actions/resume`) mỗi chỗ nhận thêm một câu cuối *Corrected 2026-11-15 (7c)*: `onResume` và resume với input được hỗ trợ; resume-validate tồn tại; resume-from-breakpoint vẫn chưa. Đánh số các ghi chú sau số ghi chú hiện có cao nhất (grep `^[0-9]*\. \*\*#` trong phần đó).
- [ ] **Spec inputs** `docs/en/specs/2026-10-28-workflow-inputs-design.md:43`: thêm vào cuối *Corrected 2026-11-15: resume with `onResume` inputs and resume-validate moved to package 7c (`docs/en/specs/2026-11-15-workflow-pause-resume-design.md` Q1).*
- [ ] **Spec của package này:**
  - §5 nhận thêm các điểm khác biệt mà người dùng đã xác nhận từ S1, S2, S3 và S7 (mỗi cái là một mục đánh số);
  - điểm khác biệt số 6 được viết lại cho path của S2;
  - §7 "As built" giữ là `(Filled in at the end of the implementation.)`; controller điền nó sau review.
- [ ] **Bản tiếng Việt:** phản chiếu mọi đoạn đã đổi ở trên trong `docs/vi/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, `docs/vi/specs/2026-10-28-workflow-inputs-design.md` và `docs/vi/specs/2026-11-15-workflow-pause-resume-design.md` (code, identifier và lệnh không dịch).
- [ ] **Gate**, từng cái một, mỗi cái đọc từ một file trong `$SCRATCH`:
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow --no-fail-fast 2>&1 | tee "$SCRATCH/t8-workflow.txt"`: báo cáo passed, failed và ignored. Chỉ 7 test `sql_starrocks_live` được phép fail;
  - `cargo test -p egret-config -p egret-server -p egret-metastore` (bao gồm các test OpenAPI và client-digest);
  - ratchet và census, phải bằng các số đã đo của T2;
  - trong `frontend/` (symlink như ở Global Constraints, xoá sau đó): `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`.
- [ ] Commit: `docs(workflow): record package 7c in the roadmap and the inputs spec`.

---

## Sau các task (controller, không phải implementer)

- **Docs người dùng** (`egret-data/docs`, Platform → Workflows):
  - Executions → Run actions: Pause, Resume với input, các code `not_running`, `not_paused`, `resume_inputs_unexpected`, hành vi loop của S1 và ghi chú của S3;
  - trang task `Pause`: `onResume`, `outputs.<id>.onResume` và `outputs.<id>.resumed`, và việc che SECRET;
  - screenshot mới.

  Một PR riêng; PR docs merge ngay.
- **Kiểm tra trực tiếp** trên một `egret serve` native với các container và store throwaway của chính worktree (không bao giờ dùng demo stack dùng chung). Trước đó, grep binary tìm `resume_inputs_unexpected` (memory: build cache ship một binary cũ). Trong trình duyệt với tư cách `admin@egret.local`, sáng và tối:
  1. Pause thủ công trong khi một task chậm chạy: nó chạy xong, không có gì mới bắt đầu; resume: flow tiếp tục;
  2. resume một `Pause` với input `onResume` STRING, INT và FILE; một Log về sau hiện chúng; file tải xuống được từ tab outputs;
  3. một input bị từ chối hiện cạnh field của nó;
  4. ghi chú "Paused by hand".

  Chụp screenshot docs.
- **Review toàn nhánh**, rồi spec §7 "As built" (commit theo task, số đếm đã đo, các điểm lệch, ratchet ở T0 và ở cuối), bản tiếng Việt của nó, và PR (body kết thúc bằng dòng attribution).

---

## Tự review so với spec

| Mục spec | Ở đâu |
|---|---|
| Q1 `onResume` ở 7c; các gap `Pause` khác ở lại #9 | T2 (model, validate, keys; `pause_gap` không đụng, test `on_resume_ids_may_repeat_a_flow_input_id_and_the_other_pause_gaps_stay`), ghi chú roadmap/inputs-spec ở T8 |
| Q2 pause chỉ lật state; các run đang chạy kết thúc | T1 `pause_by_hand_commands`, test 1–3 của T3 |
| D1 suy ra, một helper, dùng bởi executor, route, DTO | T1 `paused_by_hand`/`_of`; T4 `resume_with`; T5 `waiting_of`, `ExecutionDto::new` |
| D2 không drive forward; run kết thúc được ghi; parent không hoàn thành; sibling không được tạo | T1 `drives_forward` + test 3; T3 test 2, 3, 7; cổng render S4 test 8; waker S5 test 10; S1 test 6 |
| D3 route, quyền, state, `KILLING` 409, iteration loop 400 nêu tên root, lock, audit | T1 test 5; T3 `pause_by_hand` + test 4; T5 `pause_execution`, route test 1–3, 12 |
| D4 model, cùng validator, path, id duy nhất, bỏ warning | T2 (cách viết path S8) |
| D5 hình dạng body; by hand; `Pause` `PAUSED` đầu tiên; output; các cái khác `onResume: {}`; `RESTARTED`; `by` null; không `onResume`; `LoopUntil` 409 | T4 `resume_with` + test 14–22; T5 `resume_execution` + route test 5–8 |
| D6 lưu FILE, giá trị URI, giới hạn kích thước, xoá khi từ chối, quy tắc đọc + test | T4 storage S2 + test 23; T5 `PartSink` + route test 9 |
| D7 gate creator chỉ khi `onResume` không rỗng, source đã đóng băng | T5 `check_resume_inputs` + route test 10 |
| D8 route validate, quyền, `not_paused`, DTO, list rỗng, không lưu file, không audit | T5 `validate_resume_inputs` + route test 13, 14 |
| D9 trash/đã xoá 404, disabled được phép | T5 `live_flow_of` + route test 11 |
| D10 kill; restart 7a cả hai trường hợp; giả định timer (S3); SLA như hiện nay | T3 test 11, 12, 9; SLA không đụng (`claim::sla_terminal` đã tác động lên bất kỳ execution non-terminal, không `KILLING` nào) |
| D11 nút Pause, text confirm, lý do `KILLING`; dialog Resume validate-khi-mở, form, JSON/multipart, lỗi theo input; `paused_by_hand`; `gen:api` | T5 field DTO; T6; T7 |
| D12 không migration | Investigation record; Global Constraints |
| D13 docs | T8; "Sau các task" |
| Danh sách test §4 | executor T1; spec T2; worker T3/T4; route T5; e2e test 16 ở T4; frontend T7; kiểm tra trực tiếp sau đó |
| §5 điểm khác biệt 1–7 | 1: T1 test 5, T5 route test 3; 2: T5 route test 10; 3: route test 11; 4: route test 2; 5: route test 5; 6: S2; 7: T7 ResumeDialog |

**Các điểm mở mà controller phải chốt trước khi T3 merge:** S1 (iteration loop dưới một root bị Pause), S2 (path FILE), S3 (một Pause thủ công cạnh một `Pause` đang chờ), S7 (`onResume` SECRET được seal).
