# Không kẹt im lặng: từ chối thứ egret không chạy được, và cho thất bại thứ lọt qua

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-12-workflow-no-silent-stall-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã cài đặt, 2026-10-12. Đo sống trên một stack riêng (`container_runner = none`, `working_timeout_s = 20`, `reaper_interval_secs = 5`), trình duyệt được lái bằng tay qua Playwright, không phải test tự động:
- Một draft có `type: egret.sql` được start dưới main `ee8194af` đứng ở `RUNNING` với log rỗng, và server ghi `workflow tick failed error=no handler is registered for task type "egret.sql"` mỗi chu kỳ một lần (16:46:03, 16:46:25, 16:46:50 UTC). Sau khi đổi sang binary của nhánh này trên cùng database, lần claim kế tiếp cho nó thất bại: `FAILED` lúc 16:47:12, khoảng 5 giây sau khi khởi động, và tab Logs của W7b hiện dòng `ERROR` của §5 trên run `q #0`.
- Một flow mới dùng `egret.sql` liệt kê `task_type_not_served` là lỗi trong Problems, và Save trả 201 với lỗi đó trong báo cáo. Nút Run của editor bị vô hiệu ("Fix 1 error to run"), nên editor không bao giờ gửi lệnh start. Route start trả 400 "the flow cannot run on this deployment: …" và không ghi execution nào; publish trả 400; Replay execution cũ hiện cùng thông điệp trong hộp thoại của nó.
- Một flow có `AllowFailure` liệt kê `unsupported_allow_failure` trong Problems.
- Schema không có type nào trong sáu type chưa nối, và gợi ý `type:` của editor không đưa ra `egret.sql`.

**Phụ thuộc:** W7b (#662), đã từ chối `Pause` bị kẹt (`unsupported_pause_property`) và `ForEach` (`unsupported_foreach`), và ghi phần còn lại của lớp lỗi này ở §15 của nó.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`.

**Đã quyết với người dùng, 2026-10-12 (review cuối W7b):**
- Lớp lỗi kẹt im lặng là một gói engine riêng, ngay sau W7b.
- Quy tắc chung: một task được lập kế hoạch là opaque mà không handler nào chạy được sẽ bị từ chối ở validate và ở start.

**Đã quyết với người dùng, 2026-10-12 (brainstorm của gói này):**
- **Việc từ chối tính theo từng deployment.** Sáu type publish được mà không deployment production nào nối dây, cùng các type script và plugin khi chưa cấu hình runner, bị từ chối như các trường hợp tĩnh. Một deployment có phục vụ một type hay không được quyết từ cấu hình của nó bằng cùng logic dựng handler của worker (§4).
- **Worker cho thất bại thứ lọt qua.** Một run được claim mà không có handler (`NoHandler`) hoặc không có leaf (`NotALeaf`) kết thúc `FAILED`, kèm một dòng log hệ thống nói egret không chạy được nó. Nó không còn giữ lease và lặp mãi. Retry và `errors:` áp dụng như với mọi `FAILED` khác (§5). Điều này đảo ngược lựa chọn thiết kế trước đó, ghi ở `WorkerError::NoHandler`, rằng một thất bại "trông như flow của tác giả sai". Chính dòng log là thứ nói rằng không phải vậy.
- **Tập được phục vụ được cài một lần lúc khởi động** (`Served`, như `storage::current()`), và một hàm thuần đánh giá flow dựa trên nó. `build_plan` và `validate_for_run` vẫn thuần và không phụ thuộc deployment (§4.2).

**Đã quyết với người dùng, 2026-10-12 (review Task 4):**
- **Execution đã lưu được lập kế hoạch không qua validation; validation chỉ chặn ở start.** Một execution đã start chạy theo luật lúc nó start. Mọi lần lập kế hoạch lại một execution đã có (`claim::load_with`, sub-source của một vòng lặp `Loop` trong `spawn::insert_iteration`, một retry `CREATE_NEW_EXECUTION` trong `replay`) dùng `executor::plan_stored`: parse và flatten, không `validate_for_run`. Start mới (`start_execution_in_on`, source hiện tại của đích `Subflow`) vẫn dùng `build_plan`. Nếu không, mọi cấu trúc §3 từ chối biến các execution đang chạy thành abort `Unplannable` vòng qua reaper mãi mãi (§5).

## 1. Vì sao

Một execution có thể kẹt ở `RUNNING` mãi mãi, không có gì trong log. Cơ chế giống nhau cho mọi trường hợp:
1. worker claim run;
2. nó lỗi trước khi ghi nhận kết quả (`NotALeaf` hoặc `NoHandler`);
3. lỗi thoát khỏi `run_one` và tick, nên run giữ nguyên lease;
4. reaper xoá lease sau một `working_timeout`;
5. worker lại claim run, và vòng lặp lặp lại.

Dấu vết duy nhất là một `warn` mỗi chu kỳ trong log server. UI của W7b hiển thị execution như vậy là đang chạy, không có kết thúc.

Đo trên main `ee8194af`, các trường hợp này rơi vào vòng lặp:

| Trường hợp | Từ chối lúc chạy | Lúc publish hiện nay |
|---|---|---|
| `AllowFailure`, `WorkingDirectory` (được lập kế hoạch là opaque) | `NotALeaf` | sạch, không cảnh báo |
| `LoopUntil`/`WaitFor` có cận là biểu thức (`loop_until_gap`) | `NotALeaf` | sạch, không cảnh báo (đo ở Task 1; bảng này lúc đầu ghi một cảnh báo `not_yet_enforced`, mà `validate` chưa từng phát cho nó) |
| Một type viết giống control flow của Kestra mà egret không cài | `NoHandler` | cảnh báo `not_yet_enforced`; bị từ chối là unknown lúc publish, nhưng draft vẫn start được |
| Một type không xác định | `NoHandler` | cảnh báo `unknown_task_type`; bị từ chối lúc publish, nhưng draft vẫn start được |
| `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack` | `NoHandler` | **sạch**: chúng có dòng trong `TASK_TYPES` và không deployment nào đăng ký handler (`egret-bin/src/main.rs`, "Six of twenty-three publishable types hang in production") |
| Task script khi `workflow.container_runner = none` | `NoHandler` | bị từ chối lúc publish (`check_script_tasks`), nhưng draft vẫn start được |

## 2. Gói này không làm gì

- Không chạy `AllowFailure`, `WorkingDirectory`, `LoopUntil` có cận là biểu thức, hay sáu type chưa nối dây. Mỗi cái vẫn là một quyết định riêng: `AllowFailure` cần một state `WARNING` mà `RunState` không chứa được, và `WorkingDirectory` giao cả một cây con cho một worker.
- Không thêm route kill. §5 kết thúc các execution đang kẹt hiện có; route kill vẫn là việc nối tiếp. *(Ghi chú, 2026-10-18: route kill sau đó đã được ship; xem `docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`.)*
- Không thêm code frontend. Các code mới đến editor qua báo cáo validation và tab Problems hiện có, và schema mất các type không được phục vụ qua bộ lọc hiện có.

## 3. Từ chối tĩnh (không phụ thuộc deployment, trong `validate_for_run`)

Mọi construct mà `build_plan` lập kế hoạch là một `Runnable` opaque vì egret không có executor cho nó trở thành **lỗi** validation. Nó không còn là cảnh báo, cũng không còn im lặng. `validate_for_run` là cổng thứ ba của `build_plan`, nên mọi đường start đều từ chối các flow này, và publish cũng từ chối.

| Construct | Code | Thông báo nói |
|---|---|---|
| `io.kestra.plugin.core.flow.AllowFailure` (và `core.allowfailure`) | `unsupported_allow_failure` | egret không chạy nó; hãy đặt `allowFailure: true` trên từng task con |
| `io.kestra.plugin.core.flow.WorkingDirectory` (và `core.workingdirectory`) | `unsupported_working_directory` | egret không chạy nó; hãy chuyển file giữa các task script bằng `outputFiles` và `inputFiles` |
| `LoopUntil`/`WaitFor` có trường `loop_until_gap` | `unsupported_loop_until_property`; không có cảnh báo nào để thay: flow như vậy publish sạch (đo ở Task 1) | nêu tên trường (`checkFrequency.interval`, …) và nói nó phải là literal |
| Một type viết giống control flow của Kestra mà egret không cài (`canonical_flow_type(..).is_some()`, không phải `foreach`) | `unsupported_flow_task`, thay cho cảnh báo `not_yet_enforced` | nêu tên type |
| Một type không xác định | `unknown_task_type`, từ cảnh báo thành lỗi | không đổi: kiểm tra lỗi gõ |

**Các quy tắc giữ bảng này lại với nhau:**
- **Mỗi cái một vị từ.** `pause_gap` và `loop_until_gap` vẫn là các vị từ duy nhất mà cả `build_plan` và `validate` cùng hỏi. `unsupported_flow_task` và `unknown_task_type` được phát ở chỗ trước đây phát cảnh báo của chúng; `unsupported_loop_until_property` chưa từng có cảnh báo và được phát ở chỗ `validate` kiểm tra `loop_until_gap`.
- **Draft vẫn lưu được.** `update_flow` và `create_flow` không từ chối vì lỗi validation, nên draft có bất kỳ cái nào trong số này vẫn được lưu. Editor hiển thị phát hiện đó như một lỗi.
- **Các đường giữ nguyên lỗi của chúng.** `working_directory_tasks_required`, `working_directory_child_not_runnable`, `working_directory_namespace_files_invalid` và `allow_failure_tasks_required` vẫn giữ. Một `WorkingDirectory` giờ mang thêm `unsupported_working_directory`.
- **Cả `errors:` lồng nhau.** `walk_task` chỉ đi xuống qua `children()`. `validate_script_tasks` ghi lại sự khác biệt đó, còn `build_plan` thì làm phẳng `errors:` riêng của một flowable (`Sequential`, `Parallel`, `If`, `Switch`, `Dag`). Vì vậy các kiểm tra của bảng này cũng phải chạm tới các task dưới `error_children()`; nếu không, một `AllowFailure` trong `errors:` của một `Sequential` sẽ lọt qua. Kế hoạch chọn việc để chính `walk_task` đi xuống hay để một lượt đi thứ hai làm, và nêu điều đó làm đổi gì với các kiểm tra khác.
- **Một lưới chắn trên corpus: opaque kéo theo bị từ chối.** Với mọi flow trong `oracle-exec/corpus`, mọi task mà `build_plan` lập kế hoạch là `TaskKind::Runnable` trong khi `models::TaskKind` không phải `Leaf` phải mang một lỗi `validate_for_run` tại path của nó. Một flowable tương lai được lập kế hoạch là opaque mà không có từ chối sẽ làm một test fail đích danh.
- **Oracle.**
  - Các flow của corpus mà Kestra publish với 200 và giờ bị một từ chối trong bảng này bắt thì không lập kế hoạch được. Mỗi cái vào `PUBLISHED_BUT_REFUSED` với `cause` riêng, viết như các dòng `Pause` của W7b.
  - Đo ở Task 1: **52 dòng mới**. 4 `unsupported_allow_failure`, 4 `unsupported_working_directory`, và 44 `unknown_task_type` cho các task Kestra mà egret không có (`OutputValues`, `Labels`, `SetVariables`, `UnsetVariables`, `Exit`, `Assert`, `PurgeLogs`). Corpus không có `LoopUntil` nào có gap, nên không dòng nào mang `unsupported_loop_until_property`. Bảng có tổng cộng 57 dòng.
  - Grep `valids` thấy 6 file nhắc `AllowFailure` và 13 file nhắc `WorkingDirectory`; chỉ 4 và 4 ở trên được publish với 200, số còn lại vốn đã bị trả 422 hoặc bị từ chối.
  - Ratchet (`baseline.txt`) được đo lại. Không entry nào được chuyển từ khớp sang lệch vì lý do khác.

## 4. Từ chối theo deployment

### 4.1 `Served`

`Served` là tập các id task type mà worker của process này sẽ chạy: chuỗi chính xác, khớp như `Handlers::get` khớp chúng, gồm mọi alias và cách viết canonical mà nó đăng ký.

- **Một nguồn, hai nơi dùng.**
  - `runtime::served_for(&WorkflowConfig) -> Served` thuần, không I/O và không dựng runner.
  - `runtime::handlers_for` vẫn dựng các handler.
  - Một test khẳng định `served_for(cfg) == handlers_for(cfg).type_ids()` cho từng `container_runner`: `none`, `docker`, và `kubernetes` với phần dựng client được stub hoặc bỏ qua tuỳ kế hoạch thấy khả thi. Thêm một handler mà không thêm `served_for`, hoặc ngược lại, sẽ làm nó fail.
- **Hiện nay nó chứa:**
  - với `none`: mười một task core, các handler storage, namespace và KV;
  - với một runner: thêm bốn type script cùng alias của chúng, và mọi type catalog `kestra.`.

  Kế hoạch đo danh sách chính xác từ code và không chép từ câu này.
- **Cài đặt.**
  - `main.rs` của `egret-bin` cài `Served` một lần lúc khởi động, trước khi router và worker chạy, như `storage::install` làm.
  - `runtime::served()` đọc nó.
  - Một process chưa từng cài là process test, và nó nhận một giá trị do test chọn. **Không bao giờ mặc định "phục vụ tất cả":** thiếu cài đặt ở production không được mở lại lỗ hổng. Kế hoạch quyết định test cài nó thế nào, và mọi đường start phải có một cái.

### 4.2 `unrunnable`

`spec::unrunnable(flow: &Flow, served: &Served) -> Vec<Finding>` là hàm thuần.

- Nó duyệt mọi task trong `tasks`, `errors`, `finally` và `afterExecution`, qua `children()` và `error_children()`.
- Với mỗi task mà kế hoạch sẽ biến thành một `Runnable`, tức một `models::TaskKind::Leaf` có type đã biết, nó phát lỗi `task_type_not_served` tại path của task khi `served` thiếu type đó.
- Các type không xác định được để cho `unknown_task_type` (§3), để lỗi gõ không bị báo hai lần. Tương tự với các flowable ở §3.
- **Thông báo nêu type và lý do, theo lớp:**
  - các type script và `kestra.`: "this egret deployment has no container runner (`workflow.container_runner`), so it cannot run `X`";
  - mọi type khác: "this egret deployment has no handler for `X`, so it cannot run it".
  - Cả hai đều thêm rằng flow không thể start ở đây, và không cái nào đổ lỗi cho flow.

### 4.3 Được gọi ở đâu

Mọi nơi dựng một báo cáo validation, cộng hai nơi start công việc:

1. `routes::draft_dto`, báo cáo trên `GET` và `PUT` của một flow;
2. `routes::validate_flow_source`, mà cả hai route validate đều dùng;
3. `publish`, từ chối bằng dạng 400 hiện có cho lỗi validation;
4. `worker::claim::start_execution_in_on`, điểm nghẽn duy nhất cho start qua API, webhook, một lần fire của schedule, một Flow trigger và replay, trả `Error::InvalidInput("the flow cannot run on this deployment: …")`. Fire của schedule và Flow trigger giữ cách xử lý hiện có khi một start thất bại; kế hoạch ghi lại mỗi cái làm gì và không thêm gì mới;
5. đích của `worker::subflow`, nơi run thất bại với thông báo hiện có "cannot execute an invalid flow '{ns}'.'{id}': …", như khi đích không lập kế hoạch được.

Các vòng lặp lập kế hoạch lại source đã lưu của chính parent, bằng `executor::plan_stored` (không validation; quyết định ở review Task 4). Source đó đã qua điểm 4 khi execution start, nên chúng không cần kiểm tra riêng; một execution start trước các lệnh từ chối của gói này không bị chúng xét lại, và run của nó thất bại trong worker thay vào đó (§5).

### 4.4 Schema của flow

`Accepted` của `spec::schema` vốn đã chỉ giữ một type script hoặc plugin khi các cổng của nó qua. Giờ nó còn yêu cầu thuộc `served`, nên `egret.sql`, `core.http` và các type chưa nối dây khác không còn được editor đề xuất. ETag của schema đổi theo tập được phục vụ.

## 5. Worker cho thất bại thứ lọt qua

**Hành vi mới.** Trong `worker::run_one`, `WorkerError::NoHandler` và `WorkerError::NotALeaf` không còn thoát khỏi tick:
- Attempt của run được ghi `FAILED` qua cùng đường commit mà kết quả thất bại của một handler đi: cùng lease fence, cùng dấu `ended_at`, cùng cách đánh giá retry.
- Một dòng `ERROR` được ghi vào log của run:
  - `NoHandler`: "egret cannot run task type `X` on this deployment: no handler is registered for it. This is not an error in the flow."
  - `NotALeaf`: "egret cannot run task `id`: it is a control-flow task (`X`) that egret does not execute. This is not an error in the flow."
- `warn` trong log server vẫn giữ.
- **Retry và `errors:` áp dụng** như với mọi `FAILED` khác. Retry kết thúc sau `maxAttempts`, vì mỗi attempt đều thất bại theo cùng một cách.

**Các execution đang kẹt hiện có tự kết thúc.** Khi reaper lần tới nhả lease, lần claim kế tiếp cho run thất bại và execution ổn định. Không cần migration và không cần script dọn dẹp.

**Không đổi:**
- `Unplannable` và `Corrupt` vẫn là abort, vì đó là dữ liệu hỏng, không phải thiếu handler. `Unplannable` chỉ có nghĩa là source đã lưu không parse hoặc không flatten được: một execution đã lưu được lập kế hoạch bằng `executor::plan_stored`, không bao giờ validate lại (quyết định ở review Task 4), nên một source mà validation từ chối kể từ §3 đi tới `NoHandler` hoặc `NotALeaf` và thất bại như mọi run khác.
- `Registry::cancel` và `Registry::release` vẫn từ chối khi trượt.

**Tài liệu phải đổi trong cùng commit với hành vi:**
- `WorkerError::NoHandler` và `NotALeaf`;
- đoạn "Not guarding it is still the right call" của `spec::registry`;
- "A flow whose task type is unregistered HANGS" của `egret-bin/src/main.rs`;
- mục 19 của `oracle-exec/README.md`.

**Driver của oracle.** Comment của nó nói nó "carries on" qua một chỗ kẹt. Giờ nó thấy một run `FAILED`. Trace của các entry đó đổi từ kẹt sang `FAILED`; trước đây chúng là lệch và vẫn là lệch, và ratchet ghi lại điều này.

## 6. Kiểm thử

**Từ chối tĩnh:**
- Một unit test cho mỗi code mới, trong `validate` và trong `build_plan`, phủ mọi cách viết: FQCN và alias `core.`, mọi kiểu chữ hoa thường.
- Lưới chắn corpus của §3, cộng một mutation bỏ đi một lỗi và cho thấy lưới chắn fail đích danh.
- `PUBLISHED_BUT_REFUSED` được cập nhật và ratchet được đo lại.

**`Served` và `unrunnable`:**
- Unit test cho `unrunnable`:
  - một task lồng trong `errors:`, `finally:` và bên trong một flowable;
  - một type không xác định chỉ được báo một lần;
  - một type được phục vụ thì qua.
- Test drift `served_for` so với `handlers_for`, cho từng thiết lập runner.
- Test database trên container Postgres riêng của nhánh, với `container_runner = none`, một flow dùng `egret.sql`:
  - các route validate và `GET` flow mang `task_type_not_served`;
  - publish trả 400;
  - start qua API trả 400 và không ghi execution nào;
  - một lần fire của schedule hành xử như đường start thất bại hiện có;
  - một `Subflow` tới nó làm run thất bại với thông báo;
  - schema không có `egret.sql`.
- Một mutation cho mỗi call site ở §4.3.

**Worker:**
- Test database với một `Handlers` thiếu một type:
  - execution kết thúc `FAILED` và log của run có dòng `ERROR`;
  - với `retry`, các attempt dừng ở `maxAttempts`;
  - với `errors:`, nhánh handler chạy;
  - một run có lease bị reap khi đang kẹt sẽ bị tick kế tiếp cho thất bại.
- Tương tự cho `NotALeaf`, dùng một flow chèn trực tiếp, vì validation giờ từ chối nó.

**Chạy thật**, trên một stack riêng, không bao giờ trên stack dev dùng chung hay một cụm kind tên `egret`:
1. Trong trình duyệt, một flow dùng `egret.sql` hiện lỗi đỏ, và Run bị từ chối kèm thông báo.
2. Một execution kẹt dưới binary cũ (start trước khi nâng cấp, trên cùng database) kết thúc `FAILED` sau khi nâng cấp, và trang chi tiết của W7b hiện dòng log.

## 7. Giới hạn đã biết

- Một deployment mà process API và process worker chạy với các cấu hình `workflow` khác nhau vẫn có thể nhận một start mà worker của nó không phục vụ được. §5 biến điều đó thành một run `FAILED` thay vì một chỗ kẹt.
- Một flow đã publish với một construct của §3 hoặc §4 vẫn ở trạng thái đã publish và không start được. Editor hiện lý do.
- Replay một execution mà source của nó nay bị từ chối thì chính nó bị từ chối, như với `ForEach` (W7b §15). Đây là replay do người dùng yêu cầu, tức là một start; một retry `CREATE_NEW_EXECUTION` của một execution đã lưu không phải là start và replay source đã đóng băng bằng `executor::plan_stored` (§5).
- `Unplannable` (một source đã lưu không parse hoặc không flatten được) và `Corrupt` vẫn là abort của tick: run bị reap rồi claim lại, kèm một `warn` mỗi vòng. Gói này không thêm cách mới nào để đi tới chúng.
