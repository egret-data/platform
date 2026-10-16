# AllowFailure flowable: block biến lỗi của con thành cảnh báo

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-16-workflow-allow-failure-flowable-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã xây và kiểm chứng, 2026-10-16. Đọc mã nguồn từ `origin/main` tại `a39b36f7` và từ checkout Kestra `data-portal/kestra` tại `96893b589`. Image Kestra ghim `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` là artifact mà oracle được ghi lại; mọi khẳng định Java mà checkout và image có thể khác nhau được đánh dấu **kiểm trên image** và đã đo ở Task 1 của plan.

**Phụ thuộc:**
- Gói no-silent-stall (`docs/en/plans/2026-10-12-workflow-no-silent-stall.md`), refuse `AllowFailure` lúc validate bằng `unsupported_allow_failure` để một flow chứa nó dừng thay vì kẹt ở `RUNNING`.
- W8 (`docs/en/plans/2026-09-12-workflow-w8-advanced-semantics.md`), sở hữu **flag** `allowFailure:` trên một task (`executor/allow_failure.rs`), một cấu trúc khác với cấu trúc này (§2).
- Oracle (`crates/egret-workflow/oracle-exec/README.md`) và ratchet của nó (`oracle-exec/baseline.txt`).

## 1. Vì sao

`io.kestra.plugin.core.flow.AllowFailure` là một block gồm các task: chạy chúng theo thứ tự, nếu một task lỗi thì phần còn lại của block bị bỏ qua, block kết thúc ở trạng thái **cảnh báo** (`WARNING`), và flow đi tiếp sang task sau block. Người dùng viết nó cho các đoạn làm-được-thì-làm. Hiện tại egret từ chối cấu trúc này lúc validate, nên người dùng viết nó sẽ nhận lỗi validate khi lưu, và một run không thể đi tới nó.

Bốn entry của oracle bị baseline vì cấu trúc này: `valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure` và `valids/retry-with-flowable-errors`. Entry cuối còn cần retry **trên chính block** (§2, ngoài phạm vi). Ba entry đầu là mục tiêu của gói này.

## 2. Hai cấu trúc cùng một tên

| | flag `allowFailure:` | flowable `AllowFailure` |
|---|---|---|
| Đó là gì | một thuộc tính trong `Task.java` mà mọi task đều có thể viết | một class `extends Sequential` |
| egret có ở đâu | `executor/allow_failure.rs`, ship trong W8 Task 5 | chưa triển khai; bị refuse lúc validate |
| Làm gì | ánh xạ trạng thái cuối `FAILED` của chính task thành `WARNING` (và `WARNING` thành `SUCCESS` dưới `allowWarning:`) | chạy một danh sách con và ánh xạ kết quả của **block** |

Gói này xây cấu trúc thứ hai. Nó dùng lại hàm ánh xạ của cấu trúc đầu chỉ ở chỗ Java code làm vậy (§3.2); nó không đổi hành vi của flag.

**Ngoài phạm vi:** retry trên block (`retry:` trên một `AllowFailure`, như trong `valids/retry-with-flowable-errors`). Egret retry một task con (`RetryPending`, `executor/flowable/mod.rs`) nhưng chưa có retry đã đo cho một flowable. Đó là một dòng riêng và được giữ baseline; §7 nói plan làm gì với nó.

## 3. Kestra làm gì

### 3.1 Trạng thái của block

`AllowFailure.resolveState` (`kestra/core/src/main/java/io/kestra/plugin/core/flow/AllowFailure.java`) gọi `FlowableUtils.resolveState` hai lần, cả hai với các flag `allowFailure` và `allowWarning` của block (các trường `allowFailure:`/`allowWarning:` của chính flowable, mặc định `false`):

1. **Có** danh sách `errors:`: trạng thái của block nhìn từ phía errors. Nếu chưa có thì block chưa terminal và method trả về rỗng.
2. **Không có** danh sách `errors:` (`normalState`), vẫn có `finally:`: trạng thái của riêng các task.

Nếu lần gọi thứ nhất trả về một trạng thái, và lần thứ hai là `FAILED`, block là **`WARNING`**, bất kể lần thứ nhất nói gì. Ngược lại block lấy trạng thái của lần thứ nhất.

Hệ quả, đọc từ code: khi một task con lỗi, block là `WARNING` dù một task trong `errors:` có thành công hay không. Danh sách errors đã xử lý lỗi nhưng không làm block thành `SUCCESS`. `valids/allow-failure` và `valids/finally-allowfailure` là các trường hợp kiểm chứng điều này.

### 3.2 `guessFinalState` và các flag

`FlowableUtils.resolveState` kết thúc bằng `Execution.guessFinalState(tasks, parent, allowFailure, allowWarning, terminal)` (`kestra/core/src/main/java/io/kestra/core/models/executions/Execution.java`, từ dòng 760). Hàm này chọn trạng thái terminal có độ ưu tiên cao nhất trong các task run được liệt kê (`KILLED` > `FAILED` > `CANCELLED` > `WARNING` > `PAUSED`), rồi, khi các flag được bật, ánh xạ `FAILED` thành `WARNING` (hoặc `SUCCESS` khi `allowWarning` cũng bật) và `WARNING` thành `SUCCESS` dưới `allowWarning`.

`WARNING` mà `AllowFailure.resolveState` trả về cho một block lỗi là **giá trị cố định**, không đi qua bước ánh xạ thứ hai. Vì vậy `allowWarning:` của chính block không biến nó thành `SUCCESS`. Nó chỉ đổi kết quả khi một task con đã là `WARNING` (ví dụ một task có `allowFailure:` lồng bên trong), vì `guessFinalState` chạy trên đường đó. Plan đo tổ hợp này trên image thay vì tin vào cách đọc.

### 3.3 Anh em, errors và finally

- Một task con lỗi làm dừng các anh em của nó trong block, như trong `Sequential`: các task còn lại của block không chạy.
- `errors:` chạy sau khi block đã lỗi, như trong `Sequential`. Chúng không đổi `WARNING` của block (§3.1).
- `finally:` chạy sau khi block đã terminal, bất kể trạng thái nào.
- Flow sau block vẫn chạy. Trạng thái của nó không kế thừa `WARNING` của block như một lỗi.

## 4. Egret hiện làm gì

- `spec::validate` báo một node `AllowFailure` là `unsupported_allow_failure` (no-silent-stall Task 1). Một draft vẫn lưu được với lỗi đó trong `validation.errors` (check ở Task 4 của plan no-silent-stall). Việc `build_plan` trả `Err` cho flow như vậy được ghi tại `executor/plan.rs:527-537`; plan xác nhận đường publish và run theo code, vì ghi chú này chưa được truy theo qua các đường đó.
- `executor::plan` làm phẳng node thành `TaskKind::Runnable` (opaque), và đây là thứ ratchet đếm (ghi chú tại `executor/mod.rs:3331-3345`).
- Chưa có `TaskKind` nào cho nó. Corpus có sáu flow `AllowFailure` (`executor/mod.rs:4140`); bốn cái bị baseline (§1).

## 5. Thiết kế

### 5.1 Một biến thể `TaskKind` mới, chạy như `Sequential`

Thêm `TaskKind::AllowFailure { tasks: Vec<TaskIx> }`, được `flatten_flow` lập plan từ node `AllowFailure` giống `Sequential`, với danh sách `errors:` và `finally:` được mang theo như mọi flowable. Handler là `executor/flowable/allow_failure.rs`, gọi `sequential::drive` cho các task con, `errors` và `finally`, và chỉ thay đổi trạng thái **terminal** mà nó trả về:

| Task con (`tasks`) | Errors đã chạy | Trạng thái block | Đo trên image (`28f59bf175e1`) |
|---|---|---|---|
| tất cả `SUCCESS` | không | `SUCCESS` | `probe-allsuccess`: block `SUCCESS` |
| một task con `FAILED`, không có retry đang chờ | cả hai | `WARNING` (§3.1) | errors đã chạy: `allow-failure`, `finally-allowfailure` (đều `WARNING`); không có errors: `probe-noerrors` `WARNING`; errors thành công: `probe-errors-ok` `WARNING`; block `allowWarning: true`, task con `FAILED`: `probe-failwarn` `WARNING` |
| một task con `WARNING` | cả hai | `WARNING`, hoặc `SUCCESS` nếu `allowWarning:` của block được bật (§3.2) | không có `allowWarning`: `probe-combo-nowarn` `WARNING`; `allowWarning: true`: `probe-combo-warn` `SUCCESS` |
| `KILLED` hoặc `CANCELLED` | cả hai | giống `Sequential` | **chưa đo**: không quan sát được kill (xem ghi chú đo) |

Đo ngày 2026-10-16 trong một Kestra tạm khởi động từ `docker-compose.yml` của oracle (project `allowfail-probe`, container `allowfail-probe-*`, đã xóa sau đó). Trạng thái terminal của mọi task run và của execution khớp với `oracle-exec/golden.json` cho `valids/allow-failure`, `valids/allow-failure-with-retry` và `valids/finally-allowfailure` (các trạng thái trung gian của golden không được so từng cái). Anh em sau một task con lỗi không chạy trong mọi block đã đo.

Ghi chú đo:

- **Retry.** `valids/allow-failure-with-retry` đặt `retry:` lên task con lỗi, không lên block: task con chạy ba lần (`FAILED`, `RETRYING` hai lần, rồi `FAILED`), block là `WARNING`, và `error_handler` cùng `finally_task` chạy. Retry task con của egret (`RetryPending`) đã bao phủ dạng này.
- **Retry trên block (ngoài phạm vi, §2).** `valids/retry-with-flowable-errors` chạy một lần trên image: bản thân block không có lần thử nào; retry rơi xuống task con `run_script` (hai lần thử, `FAILED` rồi `RETRYING` rồi `SUCCESS`), block là `SUCCESS`, và `errors:` không chạy. Golden ghi cùng dạng. Vậy image áp `retry:` của block lên task con lỗi; khẳng định ở §2 rằng egret "chưa có retry đã đo cho một flowable" nói về phía egret, không phải image. Dòng này vẫn bị baseline và việc refuse vẫn giữ (plan Task 3); phép đo này là bằng chứng cho một gói sau.
- **KILLED / CANCELLED.** Route kill đã thử (`POST` và `DELETE` trên `/api/v1/main/executions/{id}/kill`) trả về 405 và 404, và một `Sleep` 60 giây kết thúc `SUCCESS` trước khi kill kịp tới. Dòng này chỉ là thiết kế và không được gói này banked.

### 5.2 Thay đổi nằm ở đâu

- `models/task.rs`: **có thay đổi, không phải "không đổi"** (phát hiện ở Task 3). `models::AllowFailure` chỉ giữ `tasks`, nên một `errors:` trên block bị bỏ lúc parse, và `finally:` bị bỏ ở mọi nơi (không kind nào triển khai nó). Nó có thêm các trường `errors` và `finally`; `finally` là một control-flow child (`children()`), `errors` là `error_children()`, nên mọi bước duyệt tìm child đều thấy cả hai.
- `executor/plan.rs`: biến thể mới `TaskKind::AllowFailure { tasks, finally }`. `errors` nằm trên `PlanTask::errors` như mọi flowable khác; `finally` nằm trên biến thể vì không kind nào khác có nó. `flatten_flow` ánh xạ node.
- `executor/flowable/allow_failure.rs`: mới. Hàm ánh xạ trạng thái terminal, `block_state(children, allow_warning)`, là một hàm thuần có unit test dạng bảng. Handler phát trạng thái terminal của block như một `SetRunState` và không trả `Step::Done`, vì `descend` sẽ áp `allowWarning:` của block lên `Done` đó một lần nữa và biến `WARNING` đã đo thành `SUCCESS`.
- `executor/flowable/mod.rs`: gửi biến thể này (một nhánh), cạnh `Sequential`.
- `spec/validate.rs`: bỏ refuse `unsupported_allow_failure`. Một `retry:` trên block bị refuse bằng `unsupported_allow_failure_retry` (spec §2, §5.3), và `errors:` cùng `finally:` lồng nhau được duyệt.
- `graph/mod.rs`: vẽ `finally:` của block cạnh `errors:`, để mọi task đã lập plan đều là một node của graph.
- `executor/mod.rs`: `build_plan` lập plan cho nó; các test đếm nó là opaque (`executor/mod.rs:3390-3410`, `3960-3970`, `4140`, `4235`) đi cùng.

### 5.3 Những gì không đổi

- Flag `allowFailure:` và `executor/allow_failure.rs`: không đụng tới.
- Retry trên block (§2). Một flow có `retry:` trên `AllowFailure` vẫn bị refuse ở thuộc tính đó, nên không kẹt và cũng không bị chạy âm thầm mà thiếu retry. Mã của lỗi refuse được ghi trong plan, không bịa ở đây.

## 6. Nghiệm thu

1. Ba entry mục tiêu được banked: `valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure` khớp với golden trace, và các dòng của chúng rời khỏi `oracle-exec/baseline.txt`.
2. `valids/retry-with-flowable-errors` vẫn bị baseline, với lý do được cập nhật để nêu đúng ngoài phạm vi retry ở §2.
3. Mọi dòng bảng ở §5.1 có một unit test, và mỗi dòng bị giết bởi một mutation có chủ đích (task con lỗi cho `SUCCESS`; `finally` bị bỏ qua; errors đặt block về `SUCCESS`).
4. Các test validation khẳng định `unsupported_allow_failure` (`executor/mod.rs:3405`, `3968`) được cập nhật để khẳng định plan thay vì refuse; một draft có `retry:` trên block vẫn nhận refuse của nó.
5. Ratchet giảm đúng số entry được banked. Không entry nào được thêm.
6. Gate: `cargo fmt --check`, `cargo clippy -p egret-workflow --all-targets -- -D warnings`, `cargo test -p egret-workflow`, và lần chạy oracle nêu trong `oracle-exec/README.md`. Các test cần DB chạy trên DB riêng, không bao giờ trên dev stack dùng chung.

## 7. Các điểm mở mà plan giải quyết trước

- **Kiểm từng dòng §5.1 trên image.** Cách đọc ở §3 lấy từ source. Một dòng chưa được triển khai cho đến khi image khớp.
- **Retry trên một flowable.** egret chưa có retry đã đo cho block. Plan đo xem `retry:` trên `AllowFailure` có tới được task con không; nếu không, dòng đó vẫn bị baseline kèm lý do (§6.2) thay vì tự bịa một retry.
- **`allowWarning:` trên block.** Tổ hợp ở §3.2 được đo, không được giả định.

## 8. Quy tắc gói này phải đáp ứng

- Oracle là bài kiểm tra nghiệm thu. Một trace khác với image không phải "gần đủ"; sự khác biệt được ghi vào `baseline.txt` kèm nguyên nhân, hoặc code phải đổi.
- Không đổi `oracle-exec/golden.json` để khớp với đầu ra của egret.
- Tài liệu song ngữ: spec này và plan có bản tham khảo tiếng Việt trong `docs/vi/` (quy ước, `docs-bilingual-convention`), viết sau khi bản tiếng Anh xong.

## 9. Không xây ở đây

- Retry trên block (§2).
- Bất kỳ thay đổi nào của flag `allowFailure:` (W8).
- Kế thừa label của subflow con hoặc của flowable này (không liên quan đến trạng thái của flowable).
