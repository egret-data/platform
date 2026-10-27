> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-26-workflow-end-hooks-design.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi và câu thông báo người dùng thấy được giữ nguyên tiếng Anh.

# Gói workflow #2 "End hooks": `finally:` ở mọi nơi, `afterExecution:`, và các `errors:` mà các vòng lặp đang bỏ rơi

**Trạng thái:** thiết kế ngày 2026-10-26. Chưa xây. Dòng #2 của lộ trình `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, kèm các mục bổ sung ở mục 3 của §9 trong spec gói #1. Kế hoạch: `docs/en/plans/2026-10-26-workflow-end-hooks.md`. Nhánh `feat/workflow-end-hooks`, xếp chồng lên `feat/workflow-no-silent-drops` (PR nháp #756). Spec này đề xuất hai PR, #2a và #2b (D1, Q1).

**Trạng thái, 2026-10-27: #2a đã xây** (Phần A của kế hoạch, Task A1 đến A8, nhánh `feat/workflow-end-hooks`; PR #2a, đang review). `finally:` chạy trên mọi flowable trừ `Pause`, `finally:` của chính flow chạy, còn `Loop`, `LoopUntil` và `WorkingDirectory` chạy `errors:` của chúng. Ratchet đo được **301 / 60 / 60 / 0** (từ 301 / 72 / 72 / 0): 12 mục được banked, không phải 13 như §7.1 kỳ vọng (xem ghi chú "như đã xây" dưới §7.1). `LIVE` 295, `VALIDATE_REFUSED` 25 và `UNKNOWN_TYPE_REFUSED` 21 không đổi. #2b (`afterExecution:` cấp flow, pha sau terminal, D8 đến D15 và chỗ khác Kestra về kill) chưa xây.

**Trạng thái, 2026-10-28: #2b đã xây** (Phần B của kế hoạch, Task B1 đến B7, nhánh `feat/workflow-end-hooks-b`, xếp chồng lên PR #759 = #2a; PR #2b, đang review; bản VI là Task B8). `afterExecution:` của flow chạy trong một pha sau terminal không bao giờ ghi vào execution (D8); execution cha nghe được execution con khi con "terminated" (D10); `Pause`, `Exit` và `LoopUntil`/`WaitFor` bị từ chối bên trong `afterExecution:` và `retry:` ở đó bị cảnh báo (D11, D12); các sweep, pruner và route log đọc `workflow.execution_is_idle` (migration 0210, D13); các trang execution tiếp tục theo dõi khi còn một after-run đang chạy, tối đa 30 phút sau khi kết thúc (D15). **D9 bị Q3 thay thế:** một lệnh kill dừng các run `afterExecution:` còn sống và để nguyên trạng thái của chính execution (xem các ghi chú như-đã-xây dưới D9, §5 và §9). Ratchet đo được **301 / 52 / 52 / 0** (từ 301 / 60 / 60 / 0): cả 8 mục §7.1 kỳ vọng cho #2b đều banked, trong Task B1. `LIVE` 295, `VALIDATE_REFUSED` 25 và `UNKNOWN_TYPE_REFUSED` 21 không đổi. Mọi câu dưới đây nói rằng kill một execution terminal "vẫn 409" hoặc `afterExecution:` "không kill được" là lịch sử; các ghi chú đề ngày 2026-10-28 nói điều đã được xây.

**Các khẳng định lấy từ đâu.** Mọi khẳng định về egret được kiểm trong worktree `egret-endhooks` tại `68c9f4de`. Đường dẫn egret tương đối với `crates/egret-workflow/src/` trừ khi ghi gốc khác. Kestra là bản checkout `data-portal/kestra`, ghim tại `96893b589`. Đường dẫn Kestra dùng các dạng viết tắt sau:
- `ES` là `executor/src/main/java/io/kestra/executor/ExecutorService.java`.
- `DE` là `…/executor/DefaultExecutor.java`.
- `k/` là `core/src/main/java/io/kestra/`. Dưới nó, `Execution.java` là `k/core/models/executions/Execution.java`, `FU` là `k/core/runners/FlowableUtils.java` và `ExS` là `k/core/services/ExecutionService.java`.

Những gì oracle ghi lại lấy từ `oracle-exec/golden.json`, chuẩn hoá như `tests/executor.rs` so sánh (`EX` bên dưới). Spec này chưa build hay chạy gì. Khẳng định nào không kiểm tĩnh được thì ghi "đo ở Task N".

**Mục tiêu (người dùng, roadmap §2).** egret là engine của Kestra OSS, trừ chỗ egret khác đi vì bảo mật. Gói #2 làm egret chạy mọi danh sách task mà Kestra chạy ở cuối một khối hay cuối một execution:
- `finally:` và `afterExecution:` của flow;
- `finally:` trên mọi flowable;
- `errors:` trên `Loop`, `LoopUntil` và `WorkingDirectory`.

Gói này cũng kiểm lại tiền đề "một execution terminal không còn việc đang chạy" (Quy tắc 2 của lộ trình).

## 0. Kestra làm gì (ngữ nghĩa mà spec này chép theo)

### 0.1 Một quy tắc chọn danh sách: `Execution.findTaskDependingFlowState`

`Execution.java:579-624` là nơi duy nhất chọn danh sách nào chạy tiếp. Nó phục vụ mọi flowable và chính flow (`ES:504-512` cho task kế tiếp, `ES:1025-1050` cho phần kết thúc). Nó kiểm các trường hợp theo thứ tự:
1. **Đã có một run `finally`**: tiếp tục chạy `finally` (`:589-593`).
2. **Danh sách `errors` chưa bắt đầu và `terminalState` của bên gọi là `FAILED`**: bắt đầu `errors`, hoặc `finally` khi không có `errors` (`:595-599`). Chỉ `Pause` truyền `terminalState`.
3. **`errors` đã bắt đầu, hoặc một task đã fail**:
   - Nếu mọi task fail đều sẽ được retry, trả về không gì cả. Cả `errors` lẫn `finally` đều không bắt đầu (`:602-606`).
   - Nếu `finally` có và `errors` có nhưng chưa terminated, trả về `errors` (`:608-609`).
   - Nếu không có `finally`, trả về `errors` (`:610-612`).
4. **`finally` có và các task đều terminated, hoặc một task đã fail mà hết lượt retry**: trả về `finally` (`:615-620`).
5. **Còn lại**: trả về `tasks`.

Hệ quả của quy tắc này:
- Thứ tự là `tasks`, rồi `errors` (chỉ khi fail), rồi `finally`. `finally` chạy cả khi thành công lẫn khi fail.
- `errors` và `finally` luôn chạy **từng cái một**, kể cả dưới `Parallel` hay `Dag` (`FU:350-358`: "errors & finally must be run as sequential tasks").
- Một con của `finally` bị fail không chặn con `finally` kế tiếp. `innerResolveSequentialNexts` nhảy tới `lastIndex + 1` bất kể trạng thái của con cuối (`FU:79-109`). `sequential::walk` của egret đã đọc theo cách này (doc `executor/flowable/sequential.rs:68-84`, hàm `:85`).

### 0.2 Trạng thái của một khối, hay của flow, chỉ đến từ `tasks`

- **Một flowable.** `FU.resolveState` đợi tới khi danh sách hiện tại đã terminated (`finally`, nếu có). Rồi nó trả về `guessFinalState(tasks, …)` chỉ trên `tasks` chính (`FU:225-246`; `Execution.java:769-801`). Một con của `finally` (hay của `errors`) bị fail không đổi trạng thái của khối. Một khối có tasks fail vẫn `FAILED` dù `finally` của nó thành công.
- **Flow.** `onEnd` dùng `guessFinalState(flow)` trên `flow.getTasks()` (`ES:455`; `Execution.java:760-761`).
- **Kết thúc của chính khối.** Khối chỉ terminated sau `finally` của nó. Golden đồng ý: trong `valids/finally-sequential`, mỗi run `finally` đều `overlapping` với `Sequential` của nó (§7.1).
- **Hai trường hợp bỏ qua `finally`:**
  - một `If` có nhánh rỗng kết thúc ngay, từ trạng thái của chính nó (`k/plugin/core/flow/If.java:188-198`);
  - một khối kiểu `Sequential` có mọi con bị disabled là `SUCCESS` ngay (`FU:163-170`).

### 0.3 `afterExecution:` của flow chạy sau khi execution đã terminal

- **Khi nào bắt đầu.** `handleAfterExecution` (`ES:1005-1023`) không làm gì cho tới khi `execution.getState().isTerminated()`. Rồi nó tạo các run của danh sách từng cái một, mỗi cái `withForceExecution(true)`: "so it would be executed even if the execution is killed" (`ES:1015`). Nó được gọi sau `handleNext` và cả khi execution đang `KILLING` ("but keep listeners on killing", `ES:192-193`).
- **Trạng thái của execution không đổi.** Execution giữ trạng thái terminal của nó (`SUCCESS`, `FAILED`, `KILLED`, …), và `endDate` đã được đặt (`k/core/models/flows/State.java:141-150`). `handleEnd` return ngay khi execution đã terminal (`ES:1026`). Nên một task `afterExecution` fail không đổi gì. Các run của nó là các mục bình thường trong cùng `taskRunList` (`ES:1018`).
- **"Terminated" là một vị từ riêng.** `ExS.isTerminated(flow, execution)` (`ExS:1325-1338`) nghĩa là: terminal, và mọi run `afterExecution` đã terminated. Một lần lặp `LOOP` bị loại khỏi nửa sau (`:1331`). `process` dừng ở vị từ đó (`ES:171`).
- **Cái gì bắn trước `afterExecution`.** Flow trigger bắn ở mọi lần đổi trạng thái của một chu kỳ (`DE:722-726`). Trạng thái terminal được vào trước khi `afterExecution` chạy, nên các flow trigger đã bắn rồi.
- **Cái gì đợi tới khi `afterExecution` đã terminated.** Tất cả những việc này đợi "terminated" (`DE:714,729-817`):
  - slot concurrency được nhả ("the cycle completing them is the one that really terminates the execution", `DE:737-742`);
  - việc subflow kết thúc được báo cho cha (`DE:752-759`);
  - sự kiện loop được gửi (`DE:762-791`);
  - SLA monitor bị xoá (`DE:793-796`);
  - trigger được reset (`DE:798-801`);
  - sự kiện `TERMINATED` được gửi (`DE:803-811`).
- **Output của flow** được tính trong `onEnd` (`ES:461-479`), trước `afterExecution`.
- **Kill.** Kestra trả 409 cho kill trên execution đã terminated (`webserver/…/ExecutionController.java:1757`). Vì vậy một `afterExecution` đang chạy không kill được. Worker của nó bỏ qua phép kiểm kill với run được force-execute (`worker/…/WorkerTaskProcessor.java:201-204`).
  - *egret, 2026-10-28 (#2b, Q3):* egret khác ở đây có chủ đích. Kill một execution terminal có run `afterExecution:` còn sống được chấp nhận và dừng run đó; xem ghi chú như-đã-xây của D9.

### 0.4 Kill, `Exit`, retry, restart

- **Kill.** Không có gì mới được tạo khi execution đang `KILLING` (`FU:83`, `FU:337-339`; `ES:185-190`). Vì vậy `finally:` của flowable và `finally:` của flow không chạy khi kill. `afterExecution:` chạy khi execution đã `KILLED` (§0.3).
- **`Exit`.** Một `Exit` với state khác `KILLED` đặt execution terminal trực tiếp (`k/plugin/core/execution/Exit.java:81-109`). `errors`/`finally` của flow không bắt đầu sau đó. `afterExecution` vẫn chạy (`ES:1005-1018`).
- **Retry.** Một task fail sẽ được retry không bắt đầu `errors` cũng không `finally` (`Execution.java:603-605`). Retry của chính một flowable đợi `errors`/`finally` của nó (`ES:587-590`, `isErrorOrFinallyHandlingPending` `ES:860-905`). Hiện egret không retry flowable nào (`executor/retry.rs:48-56`).
- **Restart** (`ExS:339-351`, `:535-552`) xoá các run `errors`/`finally`/`afterExecution` trước khi chạy lại. egret không có restart: replay là một execution mới (§9).

### 0.5 Các vòng lặp

- **`Loop`.** Mỗi lần lặp là một sub-execution `LOOP`. `handleNext`/`handleEnd` của nó chạy `tasks`, `errors` và `finally` của `Loop` như ba danh sách của sub-execution đó (`ES:513-526`, `ES:1037-1043`). State của lần lặp chỉ đến từ `tasks` của `Loop` (`ES:446-450`). `Loop` ở execution cha không làm gì với `errors`/`finally` (`k/plugin/core/flow/Loop.java:282-286,303-309`).
- **`LoopUntil`.** `resolveWaitForNext` chạy `tasks`, rồi `errors`, rồi `finally`, tất cả trong mỗi lần lặp, qua cùng quy tắc (`k/plugin/core/flow/LoopUntil.java:107-116`; `FU:113-151`). `resolveState` chỉ hỏi điều kiện sau đó (`LoopUntil.java:177-201`).
- **`WorkingDirectory`** kế thừa `Sequential` (`WorkingDirectory.java:207`). Worker chạy `tasks`. Executor chỉ resolve các task kế tiếp khi một con fail (`WorkingDirectory.java:234-243`), và khi đó chạy `errors`, rồi `finally`, như các task thường bên ngoài thư mục của worker. Khi thành công nó không resolve gì, nhưng `resolveState` vẫn đợi `finally` (`Execution.java:615-620`). **Đọc từ code: một `WorkingDirectory` thành công có `finally:` không bao giờ bắt đầu nó, và khối không bao giờ kết thúc.** Điều này chưa chạy thử, và không có test Kestra nào phủ (Q4).

### 0.6 Golden ghim gì và không ghim gì

Golden ghim: thứ tự `tasks` → `errors` → `finally` → `afterExecution`, ở cả hai cấp; mỗi danh sách chạy tuần tự; một khối kết thúc sau `finally` của nó; các run `afterExecution` là mục bình thường với parent `None`, được tạo sau state cuối của execution; và lịch sử execution không bao giờ vào lại `RUNNING` vì chúng. Điều này trải trên 23 mục (§7.1).

Golden không ghim:
- một task `finally` hay `afterExecution` bị fail (không flow nào trong corpus fail);
- `afterExecution` sau một lệnh kill (`flow-concurrency-queue-killed-after-execution` chạy tới `SUCCESS`, không có gì bị kill);
- `errors`/`finally` của `WorkingDirectory` (`valids/working-directory` bị Kestra từ chối bằng 422);
- `finally` của `Pause` khi không có `errors`;
- `retry:` bên trong các danh sách này.

Mỗi điều trên được quyết từ source Kestra ở trên, hoặc được nêu là lựa chọn riêng của egret.

## Quyết định

Mọi quyết định là mặc định của spec này, ghi ngày 2026-10-26, theo quy tắc "theo Kestra trừ khi egret có lý do khác, và giữ an toàn chặt". D1, D7, D9 và D15 mang một câu hỏi cho controller (§12).

- **D1. Hai PR: #2a rồi #2b, từ một spec và một kế hoạch.**
  - **#2a, "finally và errors bên trong một execution đang sống":**
    - `finally:` trên `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil` và `WorkingDirectory`, cộng `finally:` của flow;
    - `errors:` trên `Loop`, `LoopUntil` và `WorkingDirectory`;
    - plan ổn định chỉ số (D3);
    - bộ duyệt khoá (§8);
    - đồ thị (§10).

    Mọi thứ trong #2a xảy ra trước khi execution terminal, nên không tiền đề nào của worker bị dịch chuyển.
  - **#2b, "afterExecution: việc sau khi kết thúc":**
    - pha sau terminal (D8–D13);
    - kiểm toán Quy tắc 2 và cách sửa (§6);
    - "finished" của frontend (§10).

  Roadmap ước lượng #2 là M. Đo được, nó là hai M: #2a chạm model, plan, executor và bộ duyệt; #2b chạm settle của worker, ba sweep, một migration và frontend. #2b phụ thuộc #2a ở `finally:` của flow (`after-execution-finally`) và ở bố cục plan (D3). Ranh giới PR rơi sau Task A8 của kế hoạch (Q1).
- **D2. Một hàm chọn danh sách, của Kestra.** `flowable::current_list` (`executor/flowable/mod.rs:199-233`) là nửa của egret trong `findTaskDependingFlowState`. Nó đã chọn `Tasks`, `Errors`, `RetryPending` hoặc `Ended`. Nó thêm đối số `finally: &[TaskIx]` và đáp án `Current::Finally(list)`, và các trường hợp của nó theo đúng thứ tự §0.1. Mọi bên gọi truyền `finally` của khối mình: `sequential`, `parallel`, `dag`, `branch`, `allow_failure`, `working_directory`, `loop_until`, và lời gọi gốc của flow (`executor/mod.rs:225`). Không flowable nào có quy tắc `finally` riêng. Một danh sách `Current::Finally` được `sequential::walk` duyệt bất kể cách lập lịch của khối (§0.1). `Done` vẫn mang `aggregate(state, tasks, parent)` trên `tasks` chính (`flowable/mod.rs:244-282`), chính là `guessFinalState` của Kestra (§0.2).
- **D3. Plan giữ ổn định chỉ số cho các execution đã tồn tại.** Điều này thay chỉ dẫn ở §10 của gói #1 (xem "Tiền đề sai 1" bên dưới).
  - **Vì sao.** egret lưu task của một run dưới dạng chỉ số vị trí. `task_runs.task_ix` được ghi ở `worker/claim.rs:538` và đọc lại thành `TaskRun::task` ở `:4393`. Plan được dựng lại từ `executions.source` ở mỗi lần claim (`plan_stored`, `claim.rs:4349`; `executor/plan.rs:1149`). `flatten_one` là pre-order (`plan.rs:1255-1258`). Nếu #2 làm phẳng một `finally:` tại chỗ, mọi task sau nó sẽ dịch xuống, và một execution đang chạy lúc #2 được deploy sẽ đọc các run đã lưu của nó với sai task: `type`, spec và secret của một task khác.
  - **Quy tắc.**
    - `flatten_flow` trước hết tạo đúng bố cục hiện nay. Gồm cả `finally` của `AllowFailure`, vốn đã nằm inline.
    - Các danh sách #2 thêm vào được hoãn vào một hàng đợi. Chúng được làm phẳng sau `errors` của flow, theo pre-order của model: `finally` của flowable, rồi `errors` của `LoopUntil`/`WorkingDirectory`. Sau đó đến `finally` của flow và `afterExecution` của flow (#2b), mỗi cái theo sau bằng một lần xả hàng đợi.
    - Chỉ số của con luôn lớn hơn chỉ số của cha (bất biến mà `plan.rs:1255-1256` nêu).
  - **Test.** Một fixture được commit về bố cục hiện nay, lấy trước khi đổi bất kỳ code nào (Task A1 của kế hoạch), được khẳng định là tiền tố của plan mới với mọi flow trong corpus.
- **D4. Các execution đang chạy lúc #2 được deploy bắt đầu chạy end hook ở vòng kế tiếp.** Chúng được lập plan lại từ cùng source, và source luôn chứa các danh sách này. Không cột nào ghi "các quy tắc nó bắt đầu dưới" (việc đó cần một migration và sẽ tiếp tục bỏ qua phần dọn dẹp của tác giả). Một khối đã terminal không quay lại: `finally` của nó không bao giờ chạy. Rolling deploy không được trộn binary. Một binary cũ nạp một run do binary mới tạo cho chỉ số được nối thêm sẽ đọc `plan.tasks.get(ix) == None` và từ chối nó như dữ liệu hỏng. Ghi chú phát hành dặn dừng worker cũ trước. (Q2)
  - **Như đã xây, 2026-10-27 (D4, Pause `finally:`).** Một ngoại lệ với "bắt đầu chạy end hook": `Pause` có `finally:`. Một flow publish trước #2a có thể mang nó (serde bỏ danh sách và task vẫn pause). Một execution của flow đó đang chạy lúc deploy tiếp tục chạy `Pause` đó như trước: nó pause, và `finally:` của nó không chạy. Planner hỏi `pause_plan_gap`, hàm bỏ qua `finally:`, nên `plan_stored` vẫn lập plan task đó là `TaskKind::Pause` ở cùng chỉ số (các lần lặp Loop, replay và retry dùng cùng đường này). Publish và mọi lần start mới vẫn từ chối nó qua `pause_gap` (D14). Được ghim bởi `a_stored_pause_with_finally_still_plans_as_a_pause_and_still_validates_as_refused`.
- **D5. `errors:` và `finally:` của `Loop` chạy bên trong mỗi lần lặp**, như Kestra chạy (§0.5).
  - `spawn::sub_flow_source` (`worker/spawn.rs:823-883`, các lệnh xoá ở `:836-839`) hiện xoá `errors`, `finally` và `afterExecution` cấp flow của cha. Nay nó đặt `errors` của lần lặp bằng `errors` của `Loop` và `finally` của lần lặp bằng `finally` của `Loop`. `afterExecution` vẫn bị xoá, vì `isTerminated` của Kestra loại `LOOP` (`ExS:1331`).
  - Plan của cha không làm phẳng `errors`/`finally` của `Loop`: chúng không tạo run nào ở cha (`Loop.java:282-286`).
  - `spawn::find_task` (`spawn.rs:891-902`) cũng duyệt `after_execution`, nên một `Loop` trong `afterExecution:` được tìm thấy (#2b).
- **D6. `errors:` và `finally:` của `LoopUntil` chạy trong mỗi lần lặp, trước khi hỏi điều kiện.** "Thân đã xong" đổi từ "`tasks` đã hết" thành "`current_list` đã duyệt `tasks`, `errors` (khi fail) và `finally`". Điều kiện, giới hạn và quy tắc fail của `loop_until::drive` (`executor/flowable/loop_until.rs:224-279`) không đổi. `Command::RestartLoop` đã bỏ mọi run hậu duệ, và `Plan::children_of` được thêm `finally`, nên các run `errors`/`finally` của một lần lặp bị bỏ cùng thân của nó.
- **D7. `errors:` và `finally:` của `WorkingDirectory` chạy sau `FinishWorkingDirectory` của khối, bên ngoài thư mục, như các run thường.** `errors` chạy khi fail. `finally` chạy cả khi thành công. Code của Kestra không bao giờ bắt đầu một `finally` sau một `WorkingDirectory` thành công (§0.5). Spec này đọc đó là một bug của Kestra (một execution không bao giờ kết thúc). Quy tắc #20 của lộ trình cho phép egret khác đi ("unless the spec shows Kestra's behaviour is a bug"). (Q4)
- **D8. `afterExecution:` chạy trong một pha sau terminal, và state, history và `ended_at` của execution không đổi (#2b).** Đây là mô hình của Kestra, và golden ghim nó: `after-execution-error` đọc `CREATED RUNNING FAILED` với run `afterExecution` được tạo sau `FAILED`. `advance` được thêm pha này. Một execution terminal, hiện nhận `Vec::new()` (`executor/mod.rs:179-189`), nay nhận `after_execution::drive(plan, state, now)`. Hàm đó duyệt `plan.after_execution` bằng `sequential::walk` (parent `None`). Nó không bao giờ phát `SetExecutionState`, `ScheduleRetry` hay `PauseRun`. `KILLING` giữ `advance_killing`. Nên `afterExecution` bắt đầu khi execution đã `KILLED`, không bao giờ khi còn `KILLING`, như Kestra.
- **D9. Kill trong lúc `afterExecution` vẫn là 409, như Kestra.** Hiện nay route kill của egret trả `Terminated` (409) trên một execution terminal (`worker/kill.rs:186`). Kestra cũng vậy (§0.3). Một task `afterExecution` do đó chỉ bị chặn bởi `timeout:` của chính nó. Một chỗ khác đi vì an toàn (để route kill dừng các run after-execution còn sống và để nguyên state của execution) được đưa ra là Q3. Mặc định: theo Kestra.
  - **Bị thay thế, 2026-10-28 (#2b như đã xây; câu trả lời của người dùng cho Q3, 2026-10-27; Task B3 và vòng sửa của nó).** Route kill `POST …/executions/{id}/kill` (`worker/kill.rs`, `routes.rs` `kill_execution`) nay trả:
    - **Một execution terminal có một run còn sống của `afterExecution:` của nó** (`executor::after_execution_live`: terminal, và một run chưa terminal mà tổ tiên gốc là một after-task): **202**. `executor::kill::kill_phase` ghi các lá còn sống của pha là `KILLING` (một `Runnable` đang chạy) hoặc `KILLED` (mọi lá khác) và các flowable còn sống của pha là `KILLING` ngay; rồi `advance` đóng pha (`kill::close_phase`) và không tạo thêm after-task nào. **State, history và `ended_at` của execution không bao giờ bị ghi**, nên body 202 vẫn đọc ra như `SUCCESS`. `executor::terminated` tính một pha đã bị kill (`phase_killed`: một run của pha là `KILLING`/`KILLED`) là xong khi các run của nó đã settle, nên cha khi đó nghe được con. Cascade chỉ chạm các con của chính run của pha: một con `wait: false` của `tasks:` không bị chạm.
    - **Một execution terminal không có after-run còn sống:** **409 `execution_terminated`**, như trước. Gồm cả một execution mà run còn sống duy nhất là một `Exit` anh em trong `tasks:` (không phải run của pha).
    - **Một phase kill chưa áp dụng được:** **409 `kill_not_applied`** (mã mới, `routes::KILL_NOT_APPLIED_CODE`), có thể retry. Nó được trả khi việc còn sống duy nhất của pha là một `WorkingDirectory` after-task gốc ở `PREPARING`/`FINISHING` (khối phải giữ `RUNNING` cho hàng rào claim của nó) và chưa có gì của pha là `KILLING`/`KILLED`. Không ghi gì và không audit gì. Retry sau khi claim đáp xuống thì được chấp nhận.
    - **Bản ghi audit** (`workflow_execution:kill`) của một phase kill có `outcome` = state **của chính** execution (nên có thể là một state không phải kill như `SUCCESS`) cộng `"after_execution": "KILLING"|"KILLED"`. Mọi bản ghi kill khác giữ nguyên hình dạng.
    - **Cascade kill của cha chạm một con terminal có pha after còn sống** (quyết định của controller dưới Q3, 2026-10-27, vòng sửa B3 I2). `DESCENDANTS` giữ một hậu duệ terminal khi nó có run còn sống, và `descendants` cho nó nhánh pha (`kill_phase`) chỉ khi plan của nó nói run còn sống nằm trong pha; state, history và `ended_at` của nó không bị ghi. **Đây là một chỗ khác Kestra được ghi nhận**, vì cascade của Kestra bỏ qua hậu duệ terminal (`ExS:998-1000`). Một hậu duệ terminal mà run còn sống duy nhất là một `Exit` anh em, hoặc phase kill của nó chưa áp dụng được (dạng `kill_not_applied`), hoặc không nạp được, bị bỏ qua lặng lẽ trong một cascade.
    - Route mất bước kiểm terminal trước với scoped-read: chỉ worker, dưới khoá của execution và có plan, mới phân biệt được. Một id lạ vẫn là 404 và người gọi chỉ có `workflow:read` vẫn là 403 (`a_phase_kill_is_scoped_and_needs_manage`).
    - Test: mười test kill của `tests/end_hooks.rs` (`a_kill_stops_a_running_after_execution_task` đến `a_phase_kill_is_scoped_and_needs_manage`); `tests/kill_route.rs` không đổi và xanh.
- **D10. "Terminated" là một vị từ thuần, và cha nghe về con lúc đó.** Hàm mới là `executor::terminated(plan, state) -> bool`: execution terminal, và `after_execution::drive` không còn gì để làm (`ExS:1325-1338` của Kestra).
  - Đuôi của `worker::claim` chỉ báo một terminal cho cha `Subflow` hoặc cha `Loop` khi `terminated` lần đầu thành true. Hiện nay nó báo khi history lần đầu thành terminal (`claim.rs:3487-3497`, `notifies_parent` `:2930`).
  - Sự kiện outbox của flow-trigger vẫn theo từng lần đổi state (`claim.rs:3608-3672`), nên chúng bắn trước `afterExecution`, như của Kestra (§0.3).
  - Một lần lặp `Loop` không có `afterExecution` (D5), nên với nó "terminated" là "terminal".
  - **Như đã xây, 2026-10-28 (D10, B3).** Đuôi settle (`worker/claim.rs`, `now_terminated`) báo khi `terminated(after) && (!terminated(before) || was != is)`: một terminal đổi thành terminal khác vẫn được báo, như trước, nên một flow không có `afterExecution:` báo đúng như nó từng báo. Một báo cáo ép buộc (`Job::report`, không có plan) giữ quy tắc cũ. `render_updates` được chặn bởi `drives_forward || drives_after`, nên một `SetVariables`/`Labels` trong `afterExecution:` chạy; không lượt render nào khác chặn theo state của execution (bảng cổng của B3). Được ghim bởi `a_subflow_parent_hears_the_child_only_after_its_after_execution` và `a_set_variables_inside_after_execution_runs` (`tests/end_hooks.rs`).
- **D11. Trong pha sau terminal một task không được retry, và tác giả được báo lúc lưu.** Retry của một task `afterExecution` chưa được đo ở Kestra, và bộ máy retry của egret mở lại execution (`retry::scan` phát `SetExecutionState { Retrying }`, `executor/retry.rs:348-361`). Truy vấn due-retry cũng bỏ qua các run của một execution terminal (`claim.rs:799-807`). `after_execution::drive` không bao giờ gọi `retry::scan`: một run fail vẫn `FAILED`, và phép duyệt chuyển sang run kế. Lúc lưu, một `retry:` trên task trong `afterExecution:`, hoặc một `retry:` cấp flow khi `afterExecution:` tồn tại, nhận cảnh báo `not_honoured` nêu #13 (ghi chú lộ trình §11).
  - **Như đã xây, 2026-10-28 (D11, B1 và B2).** `retry::policy_for` trả `None` cho một run dưới `afterExecution:` (tìm ra bằng test ở B1: không có nó, một flowable trong pha có con fail dưới bất kỳ chính sách retry nào, của nó, của tổ tiên hay của flow, sẽ đọc ra `RetryPending` và đợi mãi, vì `retry::scan` không bao giờ chạy trong pha). Cảnh báo là `spec::keys::after_execution_retry`, chỉ lúc lưu và publish (không phải `validate_for_run`, nên một version đã lưu vẫn start được), mã `not_honoured`, thông báo "an afterExecution task is not retried on egret: the execution has already ended. Roadmap package #13 lifts it".
- **D12. Hai loại bị từ chối bên trong `afterExecution:`, lúc lưu và lúc start, nêu gói của chúng.**
  - **`Pause`**, mã `unsupported_after_execution_task`, #9. Một pause sẽ đánh dấu một execution đã kết thúc là `PAUSED` (`pause_mark`, `executor/mod.rs:269`).
  - **`Exit`**, cùng mã, #20. `exit::drive` đặt state của execution (`executor/exit.rs:212-219`).

  Cả hai bị `validate_for_run` từ chối, nên `build_plan` từ chối start một flow như vậy. Corpus: 0 flow (§7.2).

  Mọi loại khác đều chạy. Gồm cả `If` mà `after-execution-flowable` ghim và các cập nhật mức executor. Cổng của lượt cập nhật `drives_forward` (`worker/render.rs:1504`) mở rộng thành "đang đi tiếp, hoặc đang ở pha sau terminal". Với `Subflow`, `Loop`, `LoopUntil` và `WorkingDirectory` bên trong `afterExecution:`, Task B4 của kế hoạch dựng mỗi loại một test DB. Nếu một loại bị treo, task thêm loại đó vào lời từ chối này và báo cáo; nó không vá vòng quanh.

  **Như đã xây, 2026-10-28 (D12, B2 và B4).** `spec::validate::validate_after_execution`, được gọi từ `validate_for_run` (nên `validate` lúc lưu và publish, và `build_plan` lúc start, đều từ chối), duyệt mọi task dưới `afterExecution:` ở mọi độ lồng (`children()` và `error_children()`, gồm các danh sách `finally:`). Mã `unsupported_after_execution_task`, thông báo "task '<id>' is <kind> inside afterExecution, which egret does not run: <why>. Roadmap package #<n> lifts it". Ba loại bị từ chối:
  - `Pause`, #9;
  - `Exit`, #20;
  - **`LoopUntil` (và `WaitFor`, bí danh của nó), #9, do B4 thêm.** Test DB của B4 bị treo: một `LoopUntil` đợi giữa các lần lặp bằng `PauseRun`, thứ mà pha không được phát (`debug_assert` trong `after_execution::drive` bắn kể cả ở `checkFrequency` `PT0S`).

  `Subflow`, `Loop` và `WorkingDirectory` bên trong `afterExecution:` chạy (B4, mỗi loại một test DB trong `tests/end_hooks.rs`). Một cuộc kiểm toán mọi nơi phát `PauseRun`/`ScheduleRetry`/`SetExecutionState` không thấy loại nào khác chạm tới một cái từ pha (báo cáo của Task B4).
- **D13. Quy tắc 2 (§6): mọi quy tắc SQL đọc "execution terminal" là "không còn việc đang chạy" nay đọc một vị từ mới, `workflow.execution_is_idle(id)`.** Nó đúng khi execution terminal và không run task nào của nó còn sống. Nó được định nghĩa một lần, trong migration **0210** (số trống đầu tiên trên `0209_username_ledger.sql`; kiểm lại main và các PR đang mở trước khi merge). Các nơi đọc nó là quy tắc superseded của orphan sweep, `_wd` crash sweep và storage pruner.
- **D14. `finally:` của Pause chuyển sang #9, cùng `errors:` của `Pause`.** Cả hai cần cùng một cơ chế mới: một `Pause` được resume phải vào lại `RUNNING` để chạy một danh sách con. Kestra đặt nó `RUNNING` khi resume nếu có `errors` hoặc `finally` (`ExS:630-640`), và golden cho thấy `CREATED RUNNING PAUSED RUNNING FAILED` (`pause-errors-finally-after-execution`). Resume của egret phát thẳng terminal (`flowable/mod.rs` `resume_paused`, `Resume::Terminal`), và CHECK của migration 0142 buộc `PAUSED` với `resume_with`.
  - **Như đã xây, 2026-10-27 (D14, các execution đã lưu).** Lời từ chối chỉ ở publish và start (`spec::validate` hỏi `pause_gap`, mã `unsupported_pause_property`, nêu #9). Planner hỏi `pause_plan_gap`, hàm bỏ `finally:`, nên một execution lưu trước #2a vẫn chạy `Pause` của nó như một `Pause` với `finally:` không chạy, đúng như trước (D4).
  - Model được thêm `Pause.finally` để `executor::pause_gap` (`plan.rs:862-919`) từ chối nó: mã `unsupported_pause_property`, thông báo kết thúc bằng "Roadmap package #9 lifts it". Mã được giữ.
  - Hiện nay `finally:` của Pause bị bỏ lặng lẽ. Từ #2a nó bị từ chối ở publish và ở start. Corpus: 1 flow, đã bị từ chối vì `errors:` của nó trước, nên census không đổi.
  - Điều này thu hẹp dòng #2 đã được người dùng duyệt (Q5).
- **D15. "Finished" của frontend thành "terminal, và không run task nào còn sống" (#2b).** Hiện nay các trang execution ngừng poll ở state terminal (`frontend/src/components/workflows/executions/polling.ts:43-45`, và các tab topology và Gantt). Một run `afterExecution` khi đó chỉ hiện sau khi reload thủ công. DTO chi tiết đã mang `runs` (`routes.rs:1793-1817`), nên không cần đổi API. Đây là phiên bản của egret cho việc follow của Kestra, kết thúc ở sự kiện `TERMINATED` (§0.3).
  - **Như đã xây, 2026-10-28 (D15, B6).** `executionLive(execution, now)` (`frontend/src/components/workflows/executionState.ts`) đúng khi execution chưa terminal, và, một khi đã terminal, khi còn run nào chưa terminal **tối đa `AFTER_RUN_FOLLOW_MS` (30 phút) sau `ended_at`** (nếu không có thì `updated_at`). Giới hạn này có vì một execution terminal có thể giữ một run chưa terminal mãi mãi (rò rỉ `Exit`-anh-em, §6 như đã xây). Polling, danh sách execution con, tab topology và đồng hồ Gantt đọc nó; `timing.ts` thì không (thời lượng execution kết thúc ở `ended_at`). Nút Kill (`actions.ts` `showsKill`/`canKill`) cũng đọc nó, nên nó hiện trên một execution đã kết thúc có after-run còn sống; sau một lần kill như vậy trang nói "Stopping the afterExecution tasks", quyết từ state lúc bấm (body 202 giữ state của chính execution). `ExecutionDetail` xử lý `kill_not_applied`.
- **D16. Bảo mật** (§11). Không có bề mặt mới. Không endpoint mới, không chỗ render mới, không quyền mới.

**Các tiền đề sai tìm thấy khi kiểm** (mỗi cái được sửa bằng một ghi chú có ngày trong tài liệu của nó, Task A8 của kế hoạch):
1. **D8 và §10 của spec #1, và ghi chú 3 của roadmap §2**, nói "một version publish trước #2 chỉ giữ `finally:` của flowable trong `snapshot.source`… #2 phải dựng lại plan từ `snapshot.source`". Không run nào đọc snapshot:
   - một lần start chạy `flows.source` của **dòng draft** (`routes.rs:2621-2625`, `&flow.source`; doc của DTO `routes.rs:352`: "executions run the saved draft");
   - nó chép source đó vào `executions.source`;
   - mỗi lần claim lập plan lại từ bản chép đó (`claim.rs:4344-4349`);
   - `"plan"` của snapshot (`publish.rs:347-351`) là `models::Flow` được serialize lại, và chỉ test đọc nó (`tests/publish.rs:288,958,1084`).

   Nên không cần dựng lại gì: một khi model có trường đó, mọi execution đều thấy nó. Mối nguy thật là `task_ix` theo vị trí, và D3 trả lời nó.
2. **Quy tắc 2 của roadmap** gán tiền đề cho "debt spec D15". D15 là quy tắc container bị thay thế của `LoopUntil`. Tiền đề được viết trong doc của `sweep_decision` (`worker/commands.rs:2669-2671`). Và tiền đề đã sai trước #2: một `Exit` với `SUCCESS`, `WARNING`, `FAILED` hoặc `CANCELLED` để các anh em của nó chạy tiếp trong một execution terminal (`executor/exit.rs:66-73`). §6 cho thấy vì sao điều đó không gây hại, và vì sao `afterExecution` đổi câu trả lời cho `_wd` sweep và pruner.
3. **Dòng #2 của roadmap ước lượng "M", "~20 upper, 14–15 strict".** Đo được, nó là hai M (D1), với 23 cận trên và 21 kỳ vọng (§7.1).
4. **Phụ lục A của roadmap** gộp `restart-with-finally` và `restart-with-after-execution` thành các flow restart. Harness không restart được (`oracle-exec/driver.rs:1160-1241` start một lần và tick). Golden của Kestra cho cả hai chỉ là execution đầu tiên, không có restart nào được ghi. Cả hai là các mục `finally`/`afterExecution` thường (§7.1).

**Như đã xây, 2026-10-27 (các tiền đề sai).** Được xác nhận khi xây #2a:
- Tiền đề 1: không run nào đọc snapshot. Plan được dựng lại từ bản chép source của chính execution, nên một version publish trước #2 không cần xử lý đặc biệt. Mối nguy thật là `task_ix` theo vị trí: plan ổn định chỉ số (Task A3) và fixture A1 ghim nó.
- Tiền đề 2: tiền đề của sweep là doc comment của `sweep_decision`, không phải debt D15, và nó đã sai trước #2 vì `Exit`. Nó quan trọng với #2b (một khối `afterExecution:` còn sống quá 24 giờ bị `_wd` sweep chạm tới; chuyển sang Task B5).
- Tiền đề 3 (ước lượng): #2a banked 12 trong 13 kỳ vọng, xem §7.1.

## 1. #2 bao phủ gì

| construct | hiện nay | #2 | PR |
|---|---|---|---|
| `finally:` của flow | được parse (`models/flow.rs:81-82`), không bao giờ lập plan (`plan.rs:1173-1183`) | được lập plan (nối thêm), chạy sau `tasks`/`errors` khi execution đang `RUNNING` | 2a |
| `finally:` trên `Sequential`, `Parallel`, `If`, `Switch`, `Dag` | bị serde bỏ (`models/task.rs:309-409`), lặng lẽ (`spec/keys.rs:561-569`) | được model hoá, lập plan (nối thêm), chạy bởi `current_list` | 2a |
| `errors:`/`finally:` của `Loop` | bị bỏ | chạy trong mỗi lần lặp (D5) | 2a |
| `errors:`/`finally:` của `LoopUntil` | bị bỏ | chạy trong mỗi lần lặp (D6) | 2a |
| `errors:`/`finally:` của `WorkingDirectory` | bị bỏ | chạy sau khi khối kết thúc (D7) | 2a |
| `finally:` của `AllowFailure` | chạy (`flowable/allow_failure.rs:23-48`) | không đổi, chuyển sang quy tắc của D2 | 2a |
| `finally:` của `Pause` | bị bỏ | bị từ chối, nêu #9 (D14) | 2a |
| `afterExecution:` của flow | được parse (`models/flow.rs:91-92`), không bao giờ lập plan | pha sau terminal (D8) | 2b |
| `Pause`/`Exit` bên trong `afterExecution:` | không bao giờ chạy | bị từ chối (D12) | 2b |
| `retry:` chạm tới một task `afterExecution:` | không bao giờ chạy | không áp dụng; `not_honoured` #13 (D11) | 2b |
| "terminal nghĩa là không còn việc đang chạy" | tiền đề của ba quy tắc SQL | `workflow.execution_is_idle` (D13) | 2b |

## 2. Model (`models/task.rs`, `models/flow.rs`)

- `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil`, `WorkingDirectory` và `Pause` mỗi loại được thêm `#[serde(rename = "finally", default, skip_serializing_if = "Vec::is_empty")] pub finally: Vec<Task>`. Đây là thuộc tính `AllowFailure` dùng (`task.rs:621-632`), cộng thêm skip, để source được serialize lại cho một lần lặp `Loop` (`spawn.rs:878`) vẫn sát byte.
- `Loop`, `LoopUntil` và `WorkingDirectory` được thêm `errors: Vec<Task>`, viết như của `Sequential`.
- `Task::children()` (`task.rs:122-160`) liệt kê `finally` của mọi loại sau các con control-flow của nó, như `AllowFailure` đã làm (`:144`). `error_children()` (`:182-204`) được thêm `Loop`, `LoopUntil` và `WorkingDirectory`. Các bản song sinh `_mut` (`:212`, `:235`) đổi tương tự.
  - Mọi phép duyệt dựng trên các hàm này khi đó thấy các danh sách mới: `spec::validate` (`duplicate_task_id`, `validate.rs:725`), `spec::path::every_task`, `publish::resolve_all`, `graph/` và `spawn::find_task`. Đó là chủ ý.
  - Một hệ quả: một id trùng giữa `finally` của một flowable và task khác, hiện được chấp nhận vì danh sách bị bỏ, thành `duplicate_task_id`. Corpus: đo ở Task A2; kỳ vọng 0.

## 3. Plan (`executor/plan.rs`)

- `Plan` được thêm `pub finally: Vec<TaskIx>` (2a) và `pub after_execution: Vec<TaskIx>` (2b).
- `PlanTask` được thêm `pub finally: Vec<TaskIx>`. `TaskKind::AllowFailure { tasks, finally }` (`plan.rs:384-387`) thành `AllowFailure { tasks }`, và `finally` của nó chuyển sang `PlanTask::finally`, được làm phẳng inline đúng chỗ hiện nay (D3: bố cục không dịch).
- `Plan::children_of` / `direct_task_children` (`plan.rs:108-149`) trả `errors`, các con control-flow, rồi `finally`, cho mọi loại.
- `flatten_flow` (`plan.rs:1173-1183`) dựng bố cục D3. `roots` và `errors` của flow đứng đầu, không đổi. Hàng đợi `(owner, list)` được hoãn đến sau: mỗi mục được làm phẳng bằng `flatten_list`, và các danh sách mới lồng nhau lại được hoãn tiếp. Rồi `flow.finally_`, một lần xả, `flow.after_execution` (2b), và lần xả cuối. `Loop` không hoãn gì (D5). `Pause` không lập plan gì mới (D14).
- `build_plan` và `plan_stored` giữ chữ ký. `validate_for_run` được thêm lời từ chối của D12 ở #2b.

## 4. Executor (`advance` vẫn thuần)

### 4.1 #2a: một quy tắc, mọi bên gọi

- `current_list(plan, state, tasks, errors, finally, parent, now) -> Current` theo §0.1 từng trường hợp:
  - `finally_started` → `Finally`;
  - (không có đối số `terminalState`: `Pause` của egret chưa dùng nó tới #9);
  - (`errors_started` hoặc `failed`):
    - mọi lỗi sẽ retry → `RetryPending`;
    - `finally` và `errors` đều có và `errors` chưa hết → `Errors`;
    - không có `finally` → `Errors`, hoặc `Ended` khi `errors` rỗng;
  - `finally` có và (`tasks` đã hết hoặc fail mà hết retry) → `Finally`;
  - còn lại → `Tasks`.

  "Hết" nghĩa là mọi con đã lập plan có một run terminal (`isTerminated(list)` của Kestra). Nhánh `RetryPending` sẵn có cho danh sách của chính flow (`sequential.rs:55`) được giữ.
- `sequential::drive` nhận `finally`, duyệt `Current::Finally(list)` bằng `walk`, và trả `Done(aggregate(tasks))` khi nó hết.
- `parallel::drive` / `dag::drive` tới `Current::Finally` và duyệt nó tuần tự (§0.1). Trên đường vào `Finally` từ một lỗi, chúng áp `fail_the_rest` như khi vào `Errors` (`parallel.rs:33-66`). Nửa `Errors` đã được đo (`finally-parallel`: `p1..p3` vẫn `SUCCESS`, README:3036-3043). Khi không có danh sách `errors` mà các anh em còn sống, quy tắc `fail_the_rest` sẵn có của egret chi phối. Kestra để chúng chạy tới khi cha kết thúc rồi ép chúng (`ES:793-806`); corpus không chạm trường hợp này.
- `branch::drive_if` / `drive_switch` truyền `tasks` của nhánh được chọn và `errors`/`finally` của khối. Một `If` có nhánh được chọn rỗng là `Done` ngay, không có `finally` (`If.java:188-198`). Một danh sách mà mọi con bị disabled (plan rỗng) là `Done(Success)` không có `finally` (`FU:163-170`).
- `allow_failure::drive` mất vòng lặp `finally` riêng và gọi `sequential::drive` với `finally`. `block_state` của nó không đổi. Ba test `allow_failure_drive_tests` (`executor/mod.rs:10033-10170`) ghim rằng không có gì dịch.
- `working_directory::drive` (`working_directory.rs:27-49`): Begin → Prepared chạy `tasks` (với `errors` là `&[]`, như hiện nay) → Finish. Rồi, sau khi claim của chính khối đã xong, `current_list` trên (`tasks`, `errors`, `finally`) chạy `errors`/`finally` như các run con thường (D7). Run của khối vẫn `RUNNING` tới lúc đó. **Đo ở Task A5:** liệu run của khối có thể giữ `RUNNING` sau `FinishWorkingDirectory` mà worker không thấy một khối đã xong là còn sống (`worker/working_dir.rs`). Nếu không, task dừng và báo cáo, và D7 chuyển sang một việc theo sau với một lời từ chối nêu #17.
- `loop_until::drive`: bước thân dùng `sequential::drive(tasks, errors, finally)` (D6).
- Lời gọi gốc của flow (`executor/mod.rs:225`) truyền `&plan.finally`.

**Như đã xây, 2026-10-27 (§4.1, #2a).**
- `current_list` theo `Execution.java:579-624` từng nhánh; mỗi nhánh dẫn các dòng Java. Nhánh `terminalState` không được port (#9).
- **Đổi quy tắc dùng chung.** Phép thử "đã bắt đầu" của `current_list` bỏ qua một run mà `RestartLoop` đã bỏ (history rỗng): run như vậy không "đã bắt đầu". Điều này dùng chung cho mọi bên gọi.
- **Một `Dag` có `errors:`** từng được giao cho `parallel::drive`, có vị từ là `true`, nên nó bỏ qua `dependsOn`. Nay nó giữ cổng của mình (`drive_lists` với vị từ `dependsOn`). Không `Dag` corpus nào có `errors:` trước `finally-dag`.
- **Tra cứu của worker.** `spec::path::find_task` dùng chung (phép tra lá của worker, dùng bởi claim và spawn) nay duyệt `finally` và `errors` của các vòng lặp. Nó đóng ba khe có từ trước #2: một `Loop` trong danh sách `errors:` làm tick bị huỷ; một `WorkingDirectory` trong danh sách `errors:` fail; và (tìm ra ở vòng sửa A4) các task `finally:` cấp flow ban đầu fail với "not a leaf" cho tới khi phép duyệt của worker đọc `flow.finally_`.
- **`WorkingDirectory` (Q4: mặc định).** `errors:` và `finally:` chạy như run con sau claim kết thúc của chính khối, nên `finally:` chạy sau thành công (code Kestra dường như không bao giờ bắt đầu nó; coi là bug của Kestra). Các con hook nằm **ngoài thư mục**: không `block_id`, không snapshot, không claim theo thư mục. Run của khối giữ `RUNNING` tới khi các hook kết thúc.
- **Các vòng lặp (A6).** Một lần lặp `Loop` chạy `errors:` và `finally:` của chính `Loop` (D5) thay vì xoá chúng; thân `LoopUntil` truyền `errors`/`finally` cho `sequential::drive` (D6).
- `finally:` của `Pause` vẫn bị từ chối và chuyển sang #9 (Q5).
- **Các chỗ khác biệt và mục chưa đo, được ghi là như vậy:**
  1. Một khối mà mọi con bị disabled: egret bỏ `finally` của nó (`FU:163-170`); code Kestra có thể tạo con `finally` đầu tiên trong cùng lượt. **Chưa đo**; không flow corpus nào có dạng này.
  2. Một `Parallel`/`Dag` fail khi không có `errors:` làm fail các anh em còn sống của nó trước `finally`; Kestra giữ chúng chạy tới khi cha kết thúc. **Chưa đo**; không có trong corpus.
  3. Một khối `WorkingDirectory` mà claim prepare hay finish fail kết thúc `FAILED` không chạy `errors`/`finally` (quy tắc V3). Được ghim bởi `a_blocks_failed_finish_claim_runs_neither_errors_nor_finally` trong `tests/working_dir_claims.rs`; một prepare hay finish fail là việc #17 xét lại.
  4. `run_of` lấy run đầu tiên của một task. Còn tiềm ẩn với một restart cấp execution: #7 phải bỏ các run end-hook khi restart (§9).
  5. Tiền đề của `_wd` sweep cắn một khối `afterExecution:` còn sống quá 24 giờ. Chuyển sang Task B5 của #2b.

### 4.2 #2b: pha sau terminal

```rust
pub fn advance(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    if !drives_forward(state) {
        return match state.history.last() {
            Some(RunState::Killing) => kill::advance_killing(plan, state),
            Some(s) if s.is_terminal() => after_execution::drive(plan, state, now),
            _ => Vec::new(),
        };
    }
    // … unchanged …
}
```

- `executor/after_execution.rs` (mới): `drive` trả các lệnh của phép duyệt cho `plan.after_execution` (parent `None`). Nó không lọc gì, vì `walk` chỉ phát `CreateRun`, `StartRun`, `SkipRun`, `RecreateRun` và đi xuống. Một `debug_assert!` và một unit test ghim rằng không có `SetExecutionState`, `ScheduleRetry` hay `PauseRun` nào đi ra (D8, D11, D12).
- `pub fn terminated(plan, state) -> bool` (D10) là `last.is_terminal() && after_execution::drive(…)` tại điểm cố định của nó: mọi after-run terminal và không còn gì để tạo.
- `pub fn drives_after(plan, state) -> bool` đúng khi execution terminal và chưa `terminated`. Nó là cổng đã mở rộng của `render_updates` (D12).
- Một vòng `RETRYING` không thể bắt đầu pha. Vòng ghi `FAILED` rồi `RETRYING` (`retry.rs:329-365`) để `RETRYING` ở cuối, nên history không terminal, và pha bắt đầu một vòng sau khi terminal là cuối.

**Như đã xây, 2026-10-28 (§4.2, B1 và B3).**
- `executor/after_execution.rs`: `drive` duyệt `plan.after_execution` bằng `sequential::drive_list` dưới parent `None` và không lọc gì; một `debug_assert!` kiểm nó không phát `SetExecutionState`, `ScheduleRetry` hay `PauseRun`. `Plan::after_execution` được làm phẳng cuối cùng, sau `finally` của flow và lần xả của nó, rồi tự được xả (D3); test tiền tố A1 vẫn xanh.
- `terminated(plan, state)` = terminal VÀ (mọi after-task có một run gốc terminal, HOẶC (`phase_killed` VÀ mọi run của pha đã settle)). Nhánh thứ hai là của Q3 (D9 như đã xây). `drives_after` = terminal và chưa `terminated`. `after_execution_live` = terminal và một run còn sống mà tổ tiên gốc là một after-task: vị từ chấp nhận của route kill.
- Nếu `phase_killed`, `drive` trả `kill::close_phase`: các quy tắc theo từng run của `advance_killing` giới hạn trong các run của pha (`advance_killing` được tái cấu trúc thành `close_runs(plan, state, only)`, hành vi không đổi), không bao giờ có `SetExecutionState`.
- `phase_killed` cũng đọc một `Subflow` của pha có `transmitFailed: true` mà con bị người khác kill: điều đó cũng dừng pha, cùng cách đọc mà một con `KILLED` nhận trong `tasks:` (được ghim bởi `a_phase_subflow_whose_child_is_killed_elsewhere_stops_the_phase`).
- Test: `executor::end_hooks_drive_tests` (B1, mọi flow corpus có `afterExecution:` từ `SUCCESS`, `FAILED`, `KILLED` và `WARNING`; history không bao giờ dịch) và `a_killed_phase_closes_its_flowables_and_creates_nothing_more` (B3).

## 5. Worker (#2b)

| vị trí | hiện nay | #2b |
|---|---|---|
| claim một lá (`claim.rs:519-560`) | không lọc theo state execution | không đổi: một after-run được claim như mọi run |
| `apply_outcome_on` (`claim.rs:2822-2838`) | chỉ rào theo run | không đổi |
| đuôi settle, báo cha (`claim.rs:3487-3497`) | khi history lần đầu thành terminal | khi `terminated` lần đầu thành true (D10) |
| cổng `render_updates` (`render.rs:1504`) | `drives_forward` | `drives_forward \|\| drives_after` |
| các lượt render khác (`render_run_if`, render `If`/`Switch`) | đo ở Task B3 xem có cái nào chặn theo execution terminal | cổng nào tìm thấy được mở rộng tương tự, kèm một test |
| due retries (`claim.rs:799-807`) | bỏ qua các run của execution terminal | không đổi (D11: không retry sau khi kết thúc) |
| `persist` / `stamp_set!` (`stamps.rs:27-36`) | `ended_at` được đặt khi vào terminal | không đổi, và không có gì vào lại terminal |
| outbox (`claim.rs:3608-3672`) | một sự kiện cho mỗi state execution được nối thêm | không đổi: không state nào được nối thêm trong pha |
| `force_close` (`worker/force_close.rs`, quy tắc 6) | không plan, không `errors`/`finally` | không đổi: một execution bị force-close cũng không chạy `afterExecution` (nó không có plan để chạy từ đó) |
| route kill (`worker/kill.rs:186`) | 409 khi terminal | không đổi (D9) |
| cascade kill (`worker/kill.rs:358`) | bỏ qua các con terminal | không đổi: như Kestra, `afterExecution` của một con terminal không bị kill (`ExS:998-1000`) |
| driver oracle (`oracle-exec/driver.rs:1160-1241`) | tick tới khi hết việc | đo ở Task B6 rằng nó tiếp tục tick qua pha |

**Như đã xây, 2026-10-28 (§5, B3).** Hai dòng ở trên bị Q3 thay thế (D9 như đã xây): **route kill** chấp nhận một execution terminal có after-run còn sống (202, state của execution không đụng tới), trả 409 `execution_terminated` khi không có và 409 `kill_not_applied` khi phase kill chưa áp dụng được; **cascade kill** chạm một con terminal có pha after còn sống (khác `ExS:998-1000` của Kestra). Các dòng còn lại được đo đúng như viết: claim lá, `renew_lease`, claim chỉ-kill và `killing_under` không mang bộ lọc state execution nào, nên một after-run được claim, gia hạn và kill như mọi run; `due_retries`, vị từ state của `notifies_parent` và `sla_terminal` không đổi. Driver oracle không cần đổi: worker của nó đã chạy `advance` sau một state terminal, đó là lý do B1 banked cả tám mục.

## 6. Quy tắc 2: cái gì đọc "execution terminal" là "không còn việc đang chạy"

`grep -rn "state_is_terminal(e.state)\|state_is_terminal(state)" src` liệt kê mọi nơi đọc trong SQL. Mỗi cái được xét:

| nơi đọc | làm gì trên một execution terminal | có việc còn sống trước #2? | sau #2b | quyết định |
|---|---|---|---|---|
| truy vấn superseded của `run_liveness` (`worker/commands.rs:2591-2597`), dùng bởi `sweep_decision` (`:2672`) và `pod_sweep_deletes` (`k8s/runner.rs:1592`) | coi một container mà run của nó không có dòng là superseded: gỡ ngay (không giữ) hoặc sau thời hạn lưu (giữ), tính từ lúc execution kết thúc | có, một `LoopUntil` cạnh một `Exit`. Vô hại: thân của lần lặp bị thay thế đã kết thúc trước khi restart (debt §4.7) | một `LoopUntil` trong `afterExecution:` là cùng trường hợp. Nhưng thời hạn lưu được tính từ `ended_at`, nay đứng trước việc after | đọc `execution_is_idle(e.id)` thay cho `state_is_terminal(state)`; đo lúc kết thúc là `GREATEST(e.ended_at, max(task_runs.ended_at))`; viết lại doc ở `:2669-2671` |
| `_wd` crash sweep (`storage/wd_sweep.rs:163-169`) | xoá snapshot `_wd/` của một khối 24 giờ sau `COALESCE(t.ended_at, t.updated_at)` khi execution terminal | có, một khối `Exit` anh em còn sống 24 giờ | một `WorkingDirectory` trong `afterExecution:` còn sống khi execution terminal | thêm `AND workflow.state_is_terminal(t.state)`: chính khối đã kết thúc. `KILLED` là terminal, nên trường hợp force-close của debt §3.1 (`the_wd_sweep_removes_a_force_closed_blocks_snapshots`) được giữ |
| storage pruner (`storage/prune.rs:10-30`, `:375-381`) | dọn một family khi gốc của nó terminal và `updated_at` cũ hơn thời hạn lưu; "an execution that has not ended is never pruned" | chỉ khi một anh em còn sống sống lâu hơn thời hạn lưu | tương tự, cho việc after | "has not ended" thành "not idle" (`execution_is_idle`) |
| cascade kill (`worker/kill.rs:358`) | bỏ qua các con terminal | – | – | không đổi (Kestra) |
| `due_retries` (`claim.rs:799-807`) | bỏ qua retry của execution terminal | – | – | không đổi (D11) |

Migration 0210 (`migrations/0210_workflow_execution_is_idle.sql`):

```sql
-- A terminal execution can still have live work: an `Exit` sibling (since
-- kill-and-exit) and `afterExecution:` (since package #2b). "Idle" is what the
-- sweeps mean when they ask whether an execution is over.
CREATE OR REPLACE FUNCTION workflow.execution_is_idle(execution uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT workflow.state_is_terminal(e.state)
     AND NOT EXISTS (
       SELECT 1 FROM workflow.task_runs r
        WHERE r.execution_id = e.id
          AND NOT workflow.state_is_terminal(r.state))
    FROM workflow.executions e
   WHERE e.id = execution
$$;
```

"Idle" (không run nào còn sống) rộng hơn "terminated" (D10: danh sách after đã xong). Các sweep lấy cái bảo thủ, nên một `Exit` anh em cũng giữ được việc của nó.

**Như đã xây, 2026-10-28 (§6, B5 và vòng sửa của nó).** Migration `0210_workflow_execution_is_idle.sql` là đoạn SQL trên, nguyên văn (được phục vụ bởi `task_runs_execution_id_run_ix_key`). Các nơi đọc:
- truy vấn superseded của `run_liveness` (`worker/commands.rs`, nên `sweep_decision` và `pod_sweep_deletes`): `execution_is_idle(e.id)`, lúc kết thúc = `GREATEST(COALESCE(e.ended_at, e.updated_at), max(COALESCE(r.ended_at, r.updated_at)))`. Doc của `sweep_decision` nêu vị từ đó.
- `_wd` sweep (`storage/wd_sweep.rs`): `AND workflow.state_is_terminal(t.state)`, state của chính khối, như đã quyết ở trên. Không phải `execution_is_idle`: một anh em còn sống bị kẹt khi đó sẽ giữ snapshot của một khối đã chết mãi mãi.
- Pruner (`storage/prune.rs`): "has not ended" = `NOT execution_is_idle` ở cả `protected_set` và phép kiểm late-dependent của `prune_family`. Đồng hồ thời hạn lưu của `prune_candidates` là `GREATEST(updated_at, max(COALESCE(task_runs.ended_at, task_runs.updated_at)))`, vì một after-run không dịch `executions.updated_at`.
- **Do cuộc kiểm toán thêm (ngoài bảng này):** `ExecutionRepo::log_status` (`repo.rs`, `complete` của route log) là idle VÀ đồng hồ run cuối cũ hơn settle lag. Không có nó, bộ theo dõi log dừng ở `complete` trong khi một after-run vẫn đang ghi log. **`complete` do đó nghĩa là "idle"** (terminal VÀ mọi run terminal), không phải "terminal".
- Giữ như cũ (bảng kiểm toán trong báo cáo Task B5): `due_retries`, `parent_is_terminal`, `force_close`, `spawn::terminated_count`, reaper, bộ lọc danh sách, và `sweep_candidates` (orphan sweep FAILED/KILLED), vốn giữ `updated_at` làm đồng hồ: nó được bảo vệ bởi `protected_set` đã mở rộng và xét object theo tuổi của chính chúng, nên một upload chưa được index của after-run trẻ hơn 24 giờ lúc family bị sweep thì được để lại, là rò rỉ chứ không phải mất mát, giống một upload muộn bình thường trước #2.
- Test: `tests/migration_0210_execution_is_idle.rs`, `tests/storage_prune.rs`, `tests/working_dir_claims.rs` (`the_wd_sweep_leaves_a_live_block_of_a_terminal_execution`), `tests/task_logs.rs` (`logs_route_is_not_complete_while_an_after_execution_run_is_live`), mỗi cái có một RED được ghi và một mutation (`B5-mutations.txt` trong hồ sơ build).

**Nợ hoãn lại (B5, 2026-10-28): một `Exit` để lại một flowable anh em khiến execution của nó không bao giờ idle.** Một `Exit` không phải `KILLED` kết thúc execution và để các anh em của nó yên (kill-and-exit D5). Từ đó `advance()` chỉ chạy `afterExecution:`, như trước #2 nó chạy không gì cả. Nên một lá `CREATED` của danh sách chính không bao giờ được start và một flowable của danh sách chính không bao giờ được đóng (đo được: `tests/exit_sibling_idle.rs`, dạng `Parallel[Sequential[b1,b2], Exit]` với `Exit` `SUCCESS`, `FAILED` hoặc `WARNING`; cùng các dòng đó ở `1e0a3cf8`, trước #2). Một lá đã start được worker của nó làm xong, và một `Exit` không có flowable anh em không để lại gì. Execution không bao giờ idle (`workflow.execution_is_idle`, migration 0210), và do đó, mãi mãi:
- storage pruner không bao giờ dọn family của execution đó;
- `protected_set` ghim tập đọc được của execution đó: loop family, chuỗi replay và retry, execution đã trigger nó và loop family của execution đó;
- một container `delete: false` bị thay thế của nó không bao giờ được xét (một cái không-giữ cũng không bị sweep gỡ; việc release của chính nó thường gỡ nó);
- route log không bao giờ trả `complete`, nên bộ theo dõi log của frontend không bao giờ dừng trên execution như vậy; các trang execution ngừng theo dõi 30 phút sau khi kết thúc (`AFTER_RUN_FOLLOW_MS`, D15 như đã xây).

`_wd` sweep không bị ảnh hưởng: nó đọc state của chính khối. Trước 0210 pruner và các sweep đọc "terminal", nên chúng lấy file và container của các execution này đúng giờ; đúng một cách tình cờ, vì chúng cũng lấy chúng từ một anh em còn đang chạy. #2b đổi việc xoá sớm đó lấy rò rỉ này: giữ dữ liệu mãi trên một dạng hiếm là phía an toàn. **Cách sửa khuyến nghị (một gói sau, dòng #20 của lộ trình):** một execution terminal tiếp tục đóng các run danh sách chính mà không tạo hay start gì (một flowable được đóng bằng `aggregate` khi các con của nó terminal, một run danh sách chính `CREATED` được đóng). Nó cần một phép đo Kestra trước về state kết thúc của một anh em chưa bao giờ start (`SKIPPED`? `KILLED`? để `CREATED`?), và có thể làm dịch ratchet oracle. Bị loại bây giờ: đóng mọi run danh sách chính chưa start tại settle của `Exit` (nó kết thúc một flowable mà lá đã start vẫn chạy, ngược D5); một vị từ chỉ đếm lá đã start và run của pha (cần cột `task_runs.after_execution`, một migration, và viết lại hàm của D13); một ngưỡng tuổi (nghiêng về phía mất dữ liệu với một `Subflow` của pha đang đợi một con chạy lâu).

**Nợ hoãn lại (review B5, 2026-10-28): chi phí của `protected_set`.** Hàm `STABLE` có sublink, nên không bao giờ được inline và một bộ lọc toàn bảng trả một lần gọi cho mỗi dòng. Đo bằng `EXPLAIN ANALYZE` trên 100 000 execution và 300 000 run: `protected_set` với `NOT execution_is_idle(id)` mất **3,5 đến 5,2 giây** (quét tuần tự với lời gọi từng dòng), so với **0,12 giây** của `NOT state_is_terminal(state)` cũ. Nó chạy một lần mỗi lượt pruner có ứng viên (mỗi giờ) trên một kết nối trong pool, nên được chấp nhận. Các nơi đọc khác bị chặn bởi các qual khác và vẫn rẻ. Một dạng rẻ hơn khi cần: gieo tập từ `NOT state_is_terminal(state)` UNION các execution có một `task_run` chưa terminal (một lượt có index trên các run còn sống), trong một CTE `MATERIALIZED`; cùng nghĩa, không có lời gọi từng dòng. Một prefilter trong cùng `WHERE` không giúp (planner đánh giá hàm trước, 2,4 giây).

## 7. Oracle: census và ratchet

### 7.1 Các bank kỳ vọng (đo; mỗi dòng chỉ rời `baseline.txt` khi ratchet nói "banked")

Ratchet so sánh toàn bộ trace: history của execution, các run theo thứ tự tạo với parent, giá trị, history và số lần thử, các cặp có thứ tự của golden, và `spawned` (`EX:199-215`, `:2048-2063`, `:305-315`).

| mục | baseline.txt | golden (rút gọn) | cần | PR |
|---|---|---|---|---|
| `valids/finally-flow` | 583 | `FAILED`: if, ko, a1, a2 | `finally` của flow | 2a |
| `valids/finally-flow-error` | 584 | if, ko, e1, e2, a1, a2 | `finally` của flow | 2a |
| `valids/finally-flow-error-first` | 585 | ko, a1 (`ok` không bao giờ được tạo) | `finally` của flow | 2a |
| `valids/finally-sequential` | 589 | seq `FAILED`; a1, a2 dưới seq, `overlapping` với nó | `finally` của `Sequential` | 2a |
| `valids/finally-sequential-error` | 590 | seq: if, ko, e1, e2, a1, a2 | `errors`+`finally` của `Sequential` | 2a |
| `valids/finally-sequential-error-first` | 591 | seq, ko, a1 (cấp flow) | `finally` của flow | 2a |
| `valids/finally-dag` | 582 | dag, t1, t2, t3, ko, e1, e2, a1, a2 | `finally` của `Dag` | 2a |
| `valids/finally-parallel` | 588 | p1..p3 `SUCCESS`, if, ko, e1, e2, a1, a2 | `finally` của `Parallel`; một cặp bị bỏ (`EX:2263`) | 2a |
| `valids/restart-with-finally` | 625 | hello, fail_randomly `FAILED`, end (`finally`) | `finally` của flow; `runIf` trên `taskrun.attemptsCount` (README:4140-4150 ghi một chi phí render có thể đã cũ) | 2a |
| `valids/task-runif-taskswithstate` | 648 | `SetVariables` trong `finally` của flow `CREATED RUNNING SUCCESS`; Log `CREATED SKIPPED` | `finally` của flow; `runIf` với `tasksWithState` (`runners/functions.rs:177`) | 2a |
| `valids/finally-loop` | 586 | mỗi cái `FAILED`; mọi sub-execution: if, ko, e1, e2, a1, a2 ở cấp trên cùng | `errors`+`finally` của `Loop` (D5) | 2a |
| `valids/finally-loop-parallel` | 587 | tương tự | tương tự | 2a |
| `invalids/loop-switch-failed` | 570 | sub-execution 2: switch, fail, errorforeach | `errors` của `Loop` | 2a |
| `valids/after-execution-error` | 574 | `CREATED RUNNING FAILED`; fail, hello, end | `afterExecution` | 2b |
| `valids/after-execution-finally` | 576 | hello, finally, end | `finally` của flow + `afterExecution` | 2b |
| `valids/after-execution-flowable` | 577 | mytask, after_if (`CREATED RUNNING SUCCESS`), after_if_log, after_end | `afterExecution` với một `If` | 2b |
| `valids/restart-with-after-execution` | 624 | như `restart-with-finally` | `afterExecution` | 2b |
| `valids/flow-concurrency-queue-after-execution` | 593 | sleep, afterExecution | `afterExecution` (QUEUE không bao giờ bắn với một execution) | 2b |
| `valids/flow-concurrency-queue-killed-after-execution` | 595 | `SUCCESS` (không có gì bị kill) | `afterExecution`; "id có baseline duy nhất với `kill`" ở `EX:7527` thành danh sách rỗng | 2b |
| `valids/sleep-long` | 642 | sleep-long, output | `afterExecution` (`Sleep` bị chặn ở 0, `driver.rs:219`) | 2b |
| `valids/sleep-long-after-execution-flowable` | 643 | sleep-long, after-if, after-if-log, after-end | `afterExecution` với một `If` | 2b |

Kỳ vọng: #2a bank 13, từ 72 baselined xuống **59**, và `(301, 229, 72)` của `the_sixteen_banked_entries…` (`EX:813`) thành `(301, 242, 59)`. #2b bank 8, xuống **51**, `(301, 250, 51)`. Các mục rủi ro: `finally-parallel` (các cặp), `restart-with-*` (render `runIf`) và `task-runif-taskswithstate` (`SetVariables` trong `finally`). Một mục không bank vẫn baselined với nguyên nhân được viết lại. Kế hoạch không ép nó.

Không được #2 bank, kèm gói của chúng:
- `valids/after-execution`: `afterExecution` của nó render `{{ execution.outputs.final }}`, và egret không bao giờ tính `outputs:` của flow (`worker/render.rs:279-284`). Hiện không dòng nào của lộ trình sở hữu điều này; Task B7 của kế hoạch thêm nó vào phạm vi #6 bằng một ghi chú có ngày ("flow-level `outputs:`").
- `valids/pause-errors-finally-after-execution`: #9 (D14).
- `valids/all-flowable`: #5 (renderer, `EX:5086-5089`).

Cận trên là 23 (21 + `after-execution` + `pause-errors-…`), so với "~20 upper, 14–15 strict" của lộ trình.

**Như đã xây, 2026-10-27 (§7.1, #2a).** Đo từ một lần chạy mới `cargo test -p egret-workflow --test executor -- --nocapture`: **301 / 60 / 60 / 0**, `(301, 241, 60)` trong `the_sixteen_banked_entries…`. 12 trong 13 kỳ vọng được bank, không phải 13 (nên 60, không phải 59):
- A4: `valids/finally-dag`, `finally-parallel`, `finally-sequential`, `finally-sequential-error` (72 xuống 68).
- Vòng sửa A4 (worker tìm thấy một lá `finally:` cấp flow): `valids/finally-flow`, `finally-flow-error`, `finally-flow-error-first`, `finally-sequential-error-first`, `restart-with-finally` (68 xuống 63).
- A6: `valids/finally-loop`, `finally-loop-parallel`, `invalids/loop-switch-failed` (63 xuống 60).
- **Không bank: `valids/task-runif-taskswithstate`.** `finally:` cấp flow của nó chạy và hai task được tạo, nhưng cả hai kết thúc `CREATED FAILED`: `runIf` của chúng (`tasksWithState(...) is [not] empty`) không render được, phép thử `empty` của Pebble. Nguyên nhân ở renderer, thuộc gói #5. Comment baseline của nó được viết lại.
- Các mục rủi ro đều bank trừ cái đó: `finally-parallel` (một cặp bị bỏ) và `restart-with-finally` (`runIf` trên `taskrun.attemptsCount` đã render).
- Census (`LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21) không dịch. Các hằng dẫn xuất dịch trong các task đã dịch chúng, mỗi cái có một comment có ngày trong `tests/executor.rs`. Ở A8 mọi hằng được đọc lại với bản in; không cái nào lệch (61 trên 61 test pass).

**Như đã xây, 2026-10-28 (§7.1, #2b).** Đo ở Task B7 (HEAD `cf7e7c5c`) từ một lần chạy mới `cargo test -p egret-workflow --test executor -- --nocapture`: **301 / 52 / 52 / 0**, nên **(301, 249, 52)**, không phải `(301, 250, 51)` ở trên: con số đó tính từ 59 kỳ vọng của #2a, và #2a kết thúc ở 60 (`task-runif-taskswithstate`, #5). Từ 60, cả **8** mục #2b trong bảng đều bank, tất cả ở Task B1 (worker của oracle đã chạy `advance` sau một state terminal, nên pha trong `advance` là đủ): `valids/after-execution-error`, `after-execution-finally`, `after-execution-flowable`, `flow-concurrency-queue-after-execution`, `flow-concurrency-queue-killed-after-execution`, `restart-with-after-execution`, `sleep-long`, `sleep-long-after-execution-flowable`. Các Task B2 đến B6 không dịch hằng nào (mỗi cái đo 301 / 52 / 52 / 0).
- **Không bank, như liệt kê ở trên:** `valids/after-execution` (after-run của nó nay tồn tại, nhưng `end` render `{{ execution.outputs.final }}` và egret không bao giờ tính `outputs:` của flow; gói #6, ghi chú 6 của lộ trình) và `valids/pause-errors-finally-after-execution` (#9). `valids/all-flowable` vẫn ở #5.
- **Các hằng dịch ở B1** (mỗi cái có một comment có ngày trong `tests/executor.rs`): chia bank (301,241,60) thành (301,249,52); `EGRET_TOP_LEVEL_PAIRS` 852 thành 874; `EGRET_PAIRS_BEFORE_RESTRICTION` 1160 thành 1182; cặp giữ được khẳng định (586,197,783) thành (610,197,807), baselined (303,30,333) thành (279,30,309); terminal baselined 210 thành 187, execution 88 thành 80, token run-history 842 thành 752, token execution-history 329 thành 305; `MISMATCH_ON_RUNS` 46 thành 38, `MISMATCH_ON_ORDER` 17 thành 8, `MISMATCHING_RUNS_ENTRIES` 57 thành 49; chia timeout (2,58,60) thành (2,50,52); các dòng residual `finally:` 3 thành 2 và `afterExecution:` 10 thành 2; số declarer concurrency khớp 10 thành 12; danh sách tên kill thành rỗng; dựng lại plan 456/109/293/106 thành 467/111/280/106; guard của `handler_surface.py` 60 thành 52 và trần 273 thành 281. Ở B7 mọi cái được đọc lại với bản in: không cái nào lệch (61 trên 61 pass).
- **`MISMATCH_ON_ORDER` giảm 9 trong khi 8 mục bank.** B1 không liệt kê được cái thứ chín. B7 in các mục khác nhau ở `order` (một lệnh in tạm, đã gỡ): `valids/dag`, `full`, `if`, `loop-object`, `loop-object-in-list`, `parallel-disabled-tasks`, `task-runif-workingdirectory`, `variables`. `valids/after-execution` không nằm trong đó: run `end` của nó nay tồn tại, nên nó khớp ở `order` và vẫn khác ở `runs`. Nó là cái thứ chín.

### 7.2 Census và các hằng được ghim

- `LIVE` 295, `VALIDATE_REFUSED` 25 và `UNKNOWN_TYPE_REFUSED` 21 được kỳ vọng không đổi. Không flow corpus nào có `Pause`/`Exit` trong `afterExecution:` (quét cả 351 tệp, 2026-10-26). `Pause` duy nhất của corpus có `finally:` đã bị từ chối vì `errors:` của nó. Đo ở Task A2 và B5. Mọi dịch chuyển là một phát hiện phải báo cáo.
- Các hằng dịch theo các bank, mỗi cái chỉ đổi ở task làm nó dịch, kèm một comment có ngày nêu các mục:
  - `the_residual_baseline_is_attributed…` các dòng `finally:` 14 (`EX:6999`) và `afterExecution:` 10 (`EX:7044`);
  - `MISMATCH_ON_RUNS` 55 (`EX:4402`), `MISMATCH_ON_ORDER` 27 (`EX:4426`), `MISMATCH_ON_EXECUTION` 32 (`EX:4377`);
  - `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` 4 (`EX:4826`);
  - các danh sách `RESTRICTION_REMOVES*` (`EX:3098-3150`);
  - danh sách tên kill (`EX:7527`);
  - guard của `handler_surface.py` và trần của `handler-surface.txt`;
  - các số được khẳng định của `oracle-exec/README.md`.
- `spec/schema/property-treatment.json`, sinh lại bằng `EGRET_WRITE_PROPERTY_TREATMENT=1`:
  - `finally` trên `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `Loop`, `LoopUntil` và `WorkingDirectory` đi từ `ignored` sang `honoured`;
  - `errors` trên `Loop`, `LoopUntil` và `WorkingDirectory` đi từ `ignored` sang `honoured`;
  - `finally` của `Pause` đi từ `ignored` sang `refused`.

  Không dòng nào khác dịch. Các dòng `honoured` được phục vụ trong schema của trình soạn, đúng chủ ý: trình soạn bắt đầu gợi ý chúng.

## 8. Bộ duyệt khoá (`spec/keys.rs`)

- `silent()` (`keys.rs:561-569`) mất `"finally" | "errors"` cho flowable. Chúng thành khoá của model (`flowable_keys` đọc `wire_fields_of`), nên không phát hiện nào được nêu. `finally` của `Pause` chỉ được `pause_gap` báo (D14); bộ duyệt không lặp lại.
- `child_lists` (`keys.rs:507-522`) thêm `finally` cho mọi loại flowable và `errors` cho `Loop`, `LoopUntil` và `WorkingDirectory`, theo thứ tự `children()` rồi `error_children()`. Một task bên trong các danh sách này nay được duyệt tìm khoá lạ.
- Không có dòng `lift_of` nào cho các khoá này (`keys.rs:1280-1286` khẳng định `None`), nên không xoá dòng nào. Ngoại lệ `TWO` của phép đối chiếu (`keys.rs:1351`) bị xoá. `finally_and_the_errors_two_takes_draw_nothing` (`keys.rs:1636-1650`) được thay bằng `a_typo_inside_a_flowable_finally_is_refused_at_its_path` và `a_typo_inside_a_loop_errors_is_refused_at_its_path`.
- #2b thêm cảnh báo D11 (`not_honoured`, #13) và `unsupported_after_execution_task` của D12 (trong `validate_for_run`, nên `build_plan` thấy nó).
- §10 của spec #1 được đóng bằng một ghi chú có ngày: chỉ dẫn `snapshot.source` là một tiền đề sai (ở trên), và thay đổi bộ duyệt là mục này.

**Như đã xây, 2026-10-27 (§8, A2 và A7).** Việc đi xuống của `child_lists` và việc sinh lại `property-treatment.json` được gộp vào Task A2 (quyết định của controller: nếu không A2 sẽ làm hỏng test ghép cặp corpus); A7 gỡ `finally`/`errors` khỏi `silent()` và xoá ngoại lệ `TWO`. `property-treatment.json` không có diff sau A7. `finally` của `Pause` chỉ được `pause_gap` báo: thêm nó vào `silent()` là code chết (mutation giữ mọi test xanh), nên không thêm.

**Như đã xây, 2026-10-28 (§8, B2 và B4).** Cảnh báo D11 là `keys::after_execution_retry`, được nêu từ `findings` (chỉ lúc lưu và publish) ở path `…retry` của task, hoặc ở `retry` của flow khi `afterExecution:` tồn tại. Lời từ chối của D12 nằm trong `validate_for_run` (`validate_after_execution`), không phải trong bộ duyệt, và cũng từ chối `LoopUntil`/`WaitFor` (#9; D12 như đã xây). Corpus: không flow nào có cái nào trong số đó, và `LIVE`, `VALIDATE_REFUSED` và `UNKNOWN_TYPE_REFUSED` không dịch.

## 9. Replay, restart, `Exit`, kill, SLA

- **Replay** (`worker/replay.rs`) tạo một execution mới chép source. Execution mới chạy end hook của nó như mọi cái khác. Một retry `CREATE_NEW_EXECUTION` kết thúc execution cũ `RETRIED`, vốn là terminal, nên `afterExecution` của nó cũng chạy. `handleAfterExecution` của Kestra chỉ kiểm `isTerminated`.
- **Restart tại chỗ** thuộc #7. Khi #7 xây nó, nó phải gỡ các run `errors`/`finally`/`afterExecution`, như `ExS:339-351` làm. Một ghi chú có ngày được đặt vào dòng của #7.
- **`Exit`.** `finally` của flow không chạy sau một `Exit` kết thúc execution. `drives_forward` là false, như Kestra (`ES:171`). `afterExecution` chạy.
- **Kill.** Không `finally` nào chạy ở cả hai cấp. `afterExecution` chạy sau `KILLED` (D8). Kill trên một execution terminal vẫn 409 (D9).
  - *Bị thay thế, 2026-10-28 (#2b, Q3):* kill một execution terminal có after-run còn sống được chấp nhận và dừng các after-run mà không ghi vào execution; không có after-run còn sống thì vẫn là 409 `execution_terminated`; một phase kill chưa áp dụng được là 409 `kill_not_applied`. Xem ghi chú như-đã-xây của D9.
- **SLA.** `claim::sla_terminal` bỏ qua một execution terminal (`claim.rs:4652-4691`). Một SLA không bắn trong `afterExecution`. Vòng hết hạn của Kestra tính execution còn sống tới khi "terminated" (`DE:622-626`), nhưng `handleExecutionChangedSLA` dừng ở state terminal (`ES:1662`). Một SLA của Kestra làm gì với một execution đã terminal thì chưa đo. Để cho #16, kèm một ghi chú có ngày.

## 10. Đồ thị và frontend

- `graph/mod.rs:831-841` chỉ vẽ `finally:` cho `AllowFailure`. Nó vẽ cho mọi loại mà model nay có (`Branch::Finally` và `Relation::Finally` đã tồn tại, `graph/mod.rs:229-251`). Các danh sách cấp flow đã được vẽ (`:489-553`). `graph/tests.rs` được thêm một test cho mỗi họ loại. Topology của frontend đã render `finally` và `after_execution` (`frontend/src/components/workflows/topology/topologyModel.ts:111-121`). Không đổi frontend cho đồ thị.
- #2b, D15: `executionFinished` (`polling.ts:43-45`) thành `isTerminalState(state) && runs.every(r => isTerminalState(r.state))`. `ExecutionTopologyTab.tsx:67-79`, `ExecutionGanttTab.tsx:91` và `timing.ts:50` đọc cùng helper (`executionLive`, mới trong `executionState.ts`). `actions.ts:53,74` (các nút kill và resume) tiếp tục đọc state execution: kill là 409 trên execution terminal (D9). Test đặt ở `polling.test.ts` và các test tab, theo các quy tắc của `docs/en/frontend-rules.md` cho data hook và polling. Không component mới, không chữ nào ngoài các nhãn sẵn có, không sinh lại API.
  - *Bị thay thế một phần, 2026-10-28 (#2b, Q3):* nút kill của `actions.ts` **không** tiếp tục chỉ đọc state execution: vì kill dừng after-run, nó đọc `executionLive`, và trang có một thông báo mới ("Stopping the afterExecution tasks") và xử lý mã 409 mới `kill_not_applied` (D15 như đã xây). API chỉ đổi ở phần chữ (mô tả 202/409 của route kill và `complete` của trang log); xem báo cáo của Task B7 về việc sinh lại `openapi.json`.

## 11. Bảo mật (D16)

- **Không có bề mặt mới.** Không endpoint, quyền, chỗ render hay đường secret nào được thêm. Các task `afterExecution` và `finally` render qua cùng `Surface` như mọi task, trong phạm vi workspace của execution.
- **Việc sau một lệnh kill.** Như Kestra, `afterExecution` chạy sau một lệnh kill và không tự kill được (D9). Danh sách after của tác giả tiếp tục chạy tới `timeout:` của các task của nó. Đây là Q3.
  - *Bị thay thế, 2026-10-28 (#2b, Q3 đã xây):* `afterExecution:` vẫn chạy sau một lệnh kill, nhưng một after-run còn sống **có thể** bị kill (D9 như đã xây), và cascade kill của cha cũng chạm tới nó. Route kill cần `workflow:manage` và vẫn trong phạm vi workspace (404 cho id lạ), như trước. Đây là một chỗ khác Kestra vì bảo mật (roadmap §3.1).
- **Storage và container không bị lấy khỏi việc còn sống** (D13). Các sweep và pruner đọc `execution_is_idle`, nên file, snapshot `_wd` và container của một after-run ở lại tới khi nó kết thúc. Điều này đóng cùng lỗ hổng cho các `Exit` anh em, vốn đã mở trước #2 (§6).
- **Plan không thể trỏ một run đã lưu sang task khác** (D3). Không có D3, lần deploy sẽ trao cho các run đang chạy spec của task khác, có thể chứa secret của task khác.
- **Các lời từ chối chỉ đọc draft.** Lời từ chối của D12 và D14 đọc flow đang được lưu hay start: không repository, workspace hay flow nào khác. Thông báo của chúng mang id task và tên loại, không bao giờ mang giá trị.

## 12. Các câu hỏi cho controller

Mỗi câu hỏi thuộc về người dùng trả lời: đổi phạm vi, đổi hành vi với việc đang chạy, hoặc xung đột giữa hành vi của Kestra và an toàn của egret. Mọi thứ khác ở trên được quyết theo mặc định và ghi ngày.

- **Q1. Tách #2 thành #2a và #2b, hai PR từ một spec và một kế hoạch (D1)?** **Mặc định: có.** #2a ship `finally:` của flow và flowable và `errors:` của các vòng lặp (13 bank). #2b ship `afterExecution:` và cách sửa Quy tắc 2 (8 bank). Phương án khác là một PR khoảng 16 task, chạm model, plan, executor, worker, ba sweep, một migration và frontend.
- **Q2. Các execution đang chạy lúc deploy bắt đầu chạy end hook của chúng (D4)?** **Mặc định: có.** Chúng được lập plan lại từ source của chính chúng ở vòng kế tiếp, với bố cục ổn định chỉ số (D3), nên không run đã lưu nào bị đọc sai. Một khối đã kết thúc không quay lại. Phương án khác là một cột quy tắc theo từng execution (migration) để các execution trước #2 tiếp tục bỏ qua `finally:`/`afterExecution:` của chúng. Cách đó giữ "chạy theo quy tắc nó bắt đầu dưới", nhưng bỏ phần dọn dẹp của tác giả cho các execution đó. Dù cách nào, rolling deploy không được trộn binary.
- **Q3. Một lệnh kill không dừng được `afterExecution:` (D9).** Kestra trả 409 và force-execute danh sách sau một lệnh kill. **Mặc định: theo Kestra.** Một after-task chạy lạc chỉ kết thúc ở `timeout:` của nó. Phương án khác là một chỗ khác đi vì an toàn (kiểu roadmap §3.1): kill trên một execution terminal có after-run còn sống thì kill các run đó và để nguyên state của execution. Nó tốn khoảng một task trong #2b và lệch mục tiêu của #20 là căn chỉnh 409.
- **Q4. `finally:` của `WorkingDirectory` sau thành công (D7).** Code của Kestra đọc ra là không bao giờ bắt đầu nó, và khối không bao giờ kết thúc (chưa chạy thử). **Mặc định: egret chạy nó, coi hành vi của Kestra là bug.** Phương án khác là từ chối `finally:` của `WorkingDirectory` bằng một lời từ chối có tên cho tới khi một phép thử Kestra thật chốt được.
- **Q5. `finally:` của `Pause` chuyển từ #2 sang #9 (D14)**, nơi `errors:` và `onPause:` của `Pause` đã ở, vì cả ba cần cùng cơ chế "một `Pause` được resume vào lại `RUNNING`". **Mặc định: có.** Cho tới lúc đó nó bị từ chối ở publish và ở start, nêu #9 (hiện nó bị bỏ lặng lẽ). Corpus: flow duy nhất có nó đã bị từ chối.

### Câu trả lời (người dùng, 2026-10-27)

- **Q1: có.** Hai PR: #2a (Phần A của kế hoạch), rồi #2b (Phần B).
- **Q2: có** (lấy mặc định).
- **Q3: người dùng chọn chỗ khác đi vì an toàn, KHÔNG phải hành vi của Kestra.** Một lệnh kill dừng các run `afterExecution:`. Điều này khác Kestra (trả 409 và force-execute danh sách sau một lệnh kill) để một tenant không thể tạo một task mà không ai kill được; nó vào danh sách §3.1 của roadmap như một chỗ khác vì bảo mật trong #2b. Task B3 của kế hoạch xây nó: một lệnh kill trên execution terminal vẫn còn after-task sống được chấp nhận (không 409), chuyển các run đó sang KILLING/KILLED qua đường kill sẵn có, và không bao giờ đổi state cuối của chính execution. Brief của Task B3 phải mang điều này; chữ trong kế hoạch dưới đây nói "follow Kestra" cho D9 bị thay thế.
- **Q4: mặc định** (egret chạy `finally:` của `WorkingDirectory`; hành vi của Kestra được coi là bug).
- **Q5: có.** `finally:` của `Pause` chuyển sang #9 và bị từ chối nêu #9 cho tới lúc đó.

**Như đã xây, 2026-10-27 (các câu trả lời).** Q1 được xây dưới dạng tách: #2a là Phần A (PR của ghi chú này), #2b là Phần B. Q5: `finally:` của `Pause` bị từ chối nêu #9 (A2). Q4: `finally:` của `WorkingDirectory` chạy sau thành công (A5). Q2: mặc định. Q3 **không xây trong #2a**: lệnh kill dừng các run `afterExecution:` là Task B3 của Phần B; §3.1 của roadmap liệt kê nó là "planned in #2b".

**Như đã xây, 2026-10-28 (các câu trả lời, #2b).** Q3 đã xây (Task B3 và vòng sửa của nó): một lệnh kill trên execution terminal có after-run còn sống được chấp nhận, dừng chúng qua đường kill sẵn có và không bao giờ đổi state, history hay `ended_at` của chính execution; không có after-run còn sống thì vẫn là 409 `execution_terminated`; một phase kill chưa áp dụng được là 409 `kill_not_applied` có thể retry; và cascade của cha dừng pha after của một con terminal (chỗ khác Kestra thứ hai, do controller quyết dưới Q3 ngày 2026-10-27). Câu của Q3 ở trên, "Default: follow Kestra", và D9 là lịch sử. §3.1 của roadmap liệt kê cả hai chỗ khác như đã xây.

**Ghi chú deploy, 2026-10-28 (#2b).** Migration 0210 (`workflow.execution_is_idle`) phải được áp dụng trước khi binary mới chạy: các sweep, pruner và route log gọi nó. Kiểm lại số so với main và các PR đang mở trước khi merge (số migration là một không gian tên dùng chung). Ghi chú #2a ở trên (dừng worker cũ trước) vẫn áp dụng.

**Ghi chú deploy, 2026-10-27 (#2a).** Dừng worker cũ trước khi deploy (D4): một binary cũ nạp một run do binary mới tạo cho chỉ số được nối thêm sẽ từ chối nó như dữ liệu hỏng. Còn hai trường hợp biên cho các execution đã đang chạy lúc deploy. Cả hai được ghi lại, không sửa:
- **(M1) Danh sách sai dạng lưu trước #2a.** Một `finally:` hay một `errors:` của `Loop`/`LoopUntil`/`WorkingDirectory` sai dạng (ví dụ `finally: "x"`, hoặc một con không có `type`) từng publish được trước #2a, vì serde bỏ qua khoá. Nay nó fail `spec::parse` trong `plan_stored`, nên một execution đang chạy của flow như vậy bị treo (Unplannable) cho tới khi bị kill. Các lần start mới từ chối flow gọn gàng, và Kestra cũng từ chối các flow như vậy.
- **(M2) Id trùng trong các danh sách mới.** `duplicate_task_id` không duyệt một `finally:` lồng hay `errors:` của một vòng lặp trước #2a. Nếu một danh sách như vậy dùng chung id với task khác, worker resolve kết quả khớp đầu tiên cho run `finally` trong một execution đã đang chạy lúc deploy. Các lần start mới từ chối flow.
