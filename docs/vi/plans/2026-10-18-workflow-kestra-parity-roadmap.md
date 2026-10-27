> Bản tham khảo tiếng Việt. Bản gốc (chuẩn) là [docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md](../../en/plans/2026-10-18-workflow-kestra-parity-roadmap.md); khi hai bản khác nhau, bản tiếng Anh thắng.

# Workflow engine: lộ trình đạt parity với Kestra

**Mục tiêu (người dùng, 2026-10-19):** workflow engine của egret làm được những gì engine của Kestra OSS làm, trừ những chỗ egret khác đi vì bảo mật (§3.1). Mỗi khoảng trống hoặc được xếp thành một gói ở §2, hoặc được liệt kê ở §3 kèm lý do giữ lại. Không còn nhóm "về sau" nào chưa xếp lịch, và không lỗi từ chối nào là vĩnh viễn nếu §3 không nói vì sao.

**Trạng thái:**
- **Sửa ngày 2026-10-19.** Người dùng đặt mục tiêu trên và mở lại các từ chối đã chốt: `tasks:`/`errors:`/`behavior:`/`onPause:` của `Pause`, `retry:` trong block `AllowFailure`, trigger dạng plugin (W5b §11), và `outputs:` STORE/AUTO của `Loop` (phán quyết 2026-10-03).
- **Duyệt lần đầu ngày 2026-10-18.** Đo ngày đó trên `feat/workflow-kestra-executor-tasks` (main `a2850644` + nhóm C2 đang làm) so với bản checkout Kestra `data-portal/kestra` tại `96893b589` (2.0.0-SNAPSHOT, bản pin của chương trình). Cả hai phép đo chỉ đọc code và file oracle (không cargo, không docker). Bằng chứng, kèm `file:line` cho từng dòng, nằm ở Phụ lục A (oracle corpus) và Phụ lục B (bề mặt tính năng) của [bản tiếng Anh](../../en/plans/2026-10-18-workflow-kestra-parity-roadmap.md).

**Mốc thời điểm (2026-10-18, final review C2):** các phụ lục là phép đo ngày 2026-10-18, thực hiện khi C2 còn đang làm.

**Spec của chương trình:** `docs/en/specs/2026-09-01-workflow-programme-design.md`. Mọi gói W trong §9 của spec đó đã merge. Lộ trình này sắp xếp những gì đến sau nhóm C (`docs/en/specs/2026-10-18-workflow-core-tasks-design.md`) và gói nợ (`docs/en/specs/2026-10-19-workflow-debt-design.md`).

## 1. Hiện trạng của egret

**Oracle corpus** (các flow test của chính Kestra, `crates/egret-workflow/oracle-exec/`), trên main sau gói nợ (#698). Nguồn: `cargo test -p egret-workflow --test executor` (dòng ratchet in ra và các hằng census trong `oracle-exec/driver.rs`), cùng `PUBLISHED_BUT_REFUSED` trong `src/executor/mod.rs`:

| | 2026-10-18 | 2026-10-19 |
|---|---|---|
| flow trong corpus | 351 | 351 |
| đã publish và chạy thật trên egret (`LIVE`) | 281 | **295** |
| được so với golden của Kestra | 301 | 301 |
| khớp golden | 217 | **228** |
| lệch, đã đưa vào baseline kèm nguyên nhân | 84 | **73** |
| bị validation từ chối lúc publish (`VALIDATE_REFUSED`) | 27 | **25** |
| Kestra chạy được, egret từ chối (`PUBLISHED_BUT_REFUSED`) | 18 | **6**: 3 thuộc tính của `Pause`, 1 `retry:` trong block `AllowFailure`, 1 `namespace:` dạng template của Subflow, 1 mục `dependsOn` của Flow trigger chỉ có một số key |

**Ghi chú, 2026-10-26 (gói #1 "Không bỏ qua im lặng", đang review: PR #756, dòng #1 chỉ đánh dấu xong sau khi merge).** Đo trên nhánh của gói, đã rebase lên main `28fb8693`. Nguồn: kết quả in ra của ratchet từ `cargo test -p egret-workflow --test executor -- --nocapture` và các hằng số trong `oracle-exec/driver.rs`. Lệch, đã đưa vào baseline kèm nguyên nhân: **72** (cột 2026-10-19 ghi 73; `invalids/invalid-property` nay được tính là bị từ chối, qua một nhóm lúc lưu, spec Q1 (b)). Ratchet: **301 / 72 / 72 / 0** (golden / lệch / baselined / hỏng), nên 229 khớp golden (`sixteen_banked` assert (301, 229, 72)). `LIVE` vẫn **295** (`driver.rs:853`) và `VALIDATE_REFUSED` vẫn **25** (`driver.rs:979`): spec của gói dự đoán 294 và 26, nhưng `invalids/invalid-property` là bên thua trong cuộc tranh tên (dùng chung `id: duplicate` với `invalids/duplicate`) và `publish_corpus` không bao giờ lưu nó. `UNKNOWN_TYPE_REFUSED` 21 (`driver.rs:931`) và `PUBLISHED_BUT_REFUSED` 6 không đổi. Các cột ở trên vẫn là bản ghi của 2026-10-18 và 2026-10-19.

**Ghi chú, 2026-10-27 (gói #2a "Hook kết thúc, phần A", xây trên `feat/workflow-end-hooks`, PR #2a đang review).** Ratchet đo bằng cùng lệnh: **301 / 60 / 60 / 0**, nên 241 khớp (`sixteen_banked` assert (301, 241, 60)). `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21 không đổi. Mười hai mục được bank: chín mục `finally:`, `valids/finally-loop` và `-loop-parallel`, và `invalids/loop-switch-failed` (danh sách nằm trong ghi chú như-đã-xây §7.1 của spec end-hooks). `valids/task-runif-taskswithstate` vẫn ở baseline: `runIf` của nó dùng test `empty` của Pebble (renderer, gói #5). Phần chia theo nguyên nhân ở trên không được suy lại.

Trong 84 trường hợp lệch đo ngày 2026-10-18, C2 chiếm 12. 72 trường hợp còn lại theo nguyên nhân: `finally:` 11, `afterExecution:` 9, renderer lệch 8, `sla: MAX_DURATION` 5, `outputs:` riêng của `Loop` 5, độ trễ timing/poller 4, các cặp thứ tự mà driver không có đồng hồ không thể đạt tới 4, flow invalid bị chấp nhận thừa 4, các lệch `Loop` chưa quy được nguyên nhân 4, `Pause` 3, flow `outputs:`/SECRET 3, `WorkingDirectory`+`runIf` 3, retry của Subflow 2, thiếu hàm Pebble 2, nguyên nhân đơn lẻ 4, cố ý 1.

**Bề mặt tính năng** (Kestra 2.0 OSS, ngoài corpus; bằng chứng ở Phụ lục B):
- **Core task:** 49. C2 đã xong, nên 41 được hỗ trợ, trong đó 11 chỉ hỗ trợ một phần. 8 còn thiếu: `http.Request`, `http.Download`, `http.SseRequest`, `PurgeExecutions`, `PurgeFlows`, `PurgeStorage`, `metric.Publish`, `TemplatedTask`.
- **Bỏ qua im lặng, nhóm tệ nhất:**
  - `finally:` và `afterExecution:` ở cấp flow được parse nhưng không bao giờ được lập kế hoạch;
  - `finally:` bị bỏ trên mọi flowable trừ `AllowFailure`;
  - `stopAfter` và `retry.warningOnRetry` được parse nhưng không có tác dụng;
  - các key của Subflow và của input mà egret không mô hình hoá bị bỏ;
  - property lạ trên core task bị bỏ qua.
  Không có cảnh báo nào nêu tên chúng.
- **Đã đăng ký nhưng không có ai phục vụ:** `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack`.
- **Inputs:** không cái nào trong `required`, type, `values`, `validator`, min/max hay giới hạn ngày được kiểm khi start thủ công, qua API, webhook hay flow-trigger.
- **Trigger:**
  - thiếu: `ScheduleOnDates`, `http.Trigger`, trigger dạng plugin;
  - bị từ chối: Schedule `when:`, webhook `wait`/`returnOutputs`.
- **Thao tác trên execution:**
  - đã có: start, list, get, graph, logs, files, kill, resume (không kèm inputs) và replay toàn bộ execution;
  - còn thiếu: restart tại chỗ, replay từ một task, change-status, pause, delete, API labels, thao tác hàng loạt, streaming trực tiếp, endpoint eval biểu thức.
- **Biểu thức:**
  - bị từ chối: `render`/`renderOnce`/`errorLogs`/`subflow`;
  - còn thiếu: `max`/`min`, mười filter Pebble, các test `null`/`empty`/`map`/`json`;
  - `secret()` chỉ phân giải trong task plugin `kestra.*`.
- **Plugin:** chỉ chạy các `RunnableTask` từ 3 jar (9 class JDBC).
- **Runner:** `WorkingDirectory` chỉ chạy trên Docker.

## 2. Thứ tự

Mỗi gói đi qua quy trình thường lệ: spec (hoặc một mục spec) → plan → build bằng subagent → review cuối → gate → PR → merge.
- **Kích cỡ:** S ≈ một task, M ≈ một gói, L ≈ hơn một gói.
- **"Banks":** cận trên của oracle lấy từ Phụ lục A. Ratchet đo con số thật.
- **"Gỡ":** những lỗi từ chối và chỗ bỏ qua im lặng mà một gói biến thành hành vi của Kestra.

**Quy tắc 1: lỗi từ chối chỉ là chỗ giữ tạm.** Mỗi lỗi từ chối hay cảnh báo mà một gói thêm vào đều nêu tên gói trong bảng này sẽ gỡ nó. Gói #1 chỉ thêm chúng cho những key mà gói gỡ không phải là gói kế tiếp. `finally:`/`afterExecution:` của flow đi thẳng vào #2, để một flow đang publish được hôm nay không bị từ chối trong một gói rồi lại được chấp nhận ở gói sau.

**Quy tắc 2: kiểm lại sau #2.** Tiền đề của việc dọn container, "execution đã terminal thì không còn việc nào đang chạy", chỉ đúng khi egret chưa chạy `afterExecution:` (D15 của spec nợ). #2 phải kiểm lại nó.

*Đính chính, 2026-10-27 (spec `2026-10-26-workflow-end-hooks-design.md`, tiền đề sai 2).* Tiền đề này nằm trong doc comment của `sweep_decision` (`worker/commands.rs`), không phải D15 của spec nợ (quy tắc container bị thay thế của `LoopUntil`). Nó đã sai trước #2: một `Exit` với `SUCCESS`, `WARNING`, `FAILED` hoặc `CANCELLED` để các task anh em tiếp tục chạy trong một execution đã terminal. #2a không chạy việc gì sau terminal, nên không đổi câu trả lời; Task B5 của #2b kiểm lại việc dọn `_wd` và pruner cho `afterExecution:`.

| # | Gói | Phạm vi (những gì nó gỡ) | Banks | Cỡ |
|---|---|---|---|---|
| 0 | **Nhóm C2** — xong, #693 (`78c182ab`) | `SetVariables`, `UnsetVariables`, `Labels` | 11 trên 12 | M |
| — | **Gói nợ** — xong, #696 (`92c0a66a`) và #698 (`071973c9`) | Phần theo sau của kill và `WorkingDirectory`; giới hạn kích thước và thứ tự của C2; task-run id của `LoopUntil`; publish phân giải tham chiếu trong workspace (gỡ 2 từ chối thừa) | 0 (LIVE +2) | M |
| 1 | **Không bỏ qua im lặng** | Một lỗi từ chối hoặc cảnh báo có nêu tên, kèm tên gói sẽ gỡ nó, cho mọi key được parse mà bị bỏ qua, trừ `finally:`/`afterExecution:` của flow. Gồm: `finally:` của flowable nằm ngoài tầm #2, `stopAfter` (#11), `retry.warningOnRetry` (#13), các key của Subflow (#12), các key của input (#3), và property lạ trên core task, thứ mà schema của Kestra từ chối (căn theo mục 15 của Phụ lục B §8). Đồng thời sửa các con số "345" cũ trong `driver.rs` | 0 (đúng đắn) | S |
| 2 | **Hook kết thúc** | `finally:` và `afterExecution:` ở cấp flow. `finally:` trên mọi flowable: Sequential, Parallel, If, Switch, Dag, Loop, LoopUntil, Pause. `errors:` của `Loop`. Kiểm lại tiền đề của việc dọn container (Quy tắc 2) | ~20 cận trên, 14–15 chặt | M |
| 3 | **Kiểm inputs khi start** | `required`, lỗi ép kiểu, `values`/`allowCustomValue` của SELECT/MULTISELECT, `validator` của STRING, min/max của INT/FLOAT, giới hạn ngày/giờ, items của ARRAY, kiểm kiểu SECRET/URI/EMAIL/YAML/ION, khi start thủ công, qua API, webhook và flow-trigger. Endpoint `validate inputs` | một vài invalid | M |
| 4 | **HTTP và các task nền tảng chưa được phục vụ** | `http.Request`, `http.Download`, `http.SseRequest` làm worker handler có egress policy. Quyết định và nối `egret.sql` (cần một `QueryExecPort`) cùng các dòng `egret.*`/`core.*` khác, hoặc gỡ các dòng đó | 0 | M |
| 5 | **Parity của renderer** | Chia cho 0 (trường hợp Infinity của `restart-*`), `taskrun.value` không xác định, so sánh số với chuỗi, `??` trên root không tồn tại. `render`/`renderOnce`, `errorLogs`, `subflow`, `max`/`min`. Mười filter và bốn test còn thiếu. Các strategy `js`/`css`/`url_param` của `escape`. So sánh chuỗi tính lười, số học BigDecimal, `{{ task.type }}` trên flowable | ~8–10 | M |
| 6 | **`outputs:` riêng của `Loop`** | `outputs:` của `Loop` được đánh giá khi nó kết thúc, kể cả STORE/AUTO (mở lại). Trước hết xác định nguyên nhân của 4 mục `Loop` chưa quy được | 3 (+ tối đa 4) | M |
| 7 | **Thao tác trên execution** | Restart task lỗi tại chỗ, replay từ một task được chọn (kèm inputs và chọn revision), change-status và trạng thái task-run, pause, delete (kèm logs và storage), API labels. Resume kèm inputs `onResume`, và resume-validate. Logs follow (SSE), tải về và xoá. Execution follow (SSE), endpoint eval biểu thức, export/distinct/average-duration/latest. Sau đó là mọi thao tác hàng loạt `by-ids`/`by-query` | 0 (vận hành) | L |
| 8 | **`concurrency:` của flow** | `limit` với QUEUE/CANCEL/FAIL, unqueue và force-run | ít | M |
| 9 | **`Pause` và `AllowFailure` đầy đủ** (mở lại) | `tasks:`, `errors:`, `behavior:`, `onPause:`, `onResume:` của `Pause`. `retry:` trong block `AllowFailure` | 4 từ chối → chạy được | M |
| 10 | **`secret()` ở mọi nơi** | `secret()` trong env của script, Log, inputs, `runIf`, trigger và các task http, bằng cách inject giá trị (env từ Kubernetes Secret, hoặc mount file) thay vì render nó vào pod spec. Thay giới hạn ở mục 3 của Phụ lục B §8 bằng phạm vi của Kestra | 0 | M–L |
| 11 | **Trigger đầy đủ** | Schedule `when:` với các hàm ngày, `stopAfter`, `ScheduleOnDates`, `http.Trigger`. Webhook đồng bộ: `wait`, `returnOutputs`, `responseContentType`, `responseCode`. `labels` của trigger trên Schedule và Webhook. Mục `dependsOn` của Flow trigger chỉ có một số key (lỗi từ chối `listener-namespace-condition`). Unlock/restart trigger ở những chỗ mô hình claim của egret cần | 1 từ chối → chạy được | M |
| 12 | **Subflow đầy đủ** | `revision`, `labels`, `inheritLabels`, `scheduleDate`, `restartBehavior`. `namespace:`/`flowId:` dạng template được phân giải lúc chạy (lỗi từ chối `task-flow-dynamic`). Retry của Subflow (2 baselined) | 1 từ chối + 2 | M |
| 13 | **Property của task** | `retry.warningOnRetry`, `logLevel`, `logToFile`, `taskCache` trên mọi loại task | 0 | S–M |
| 14 | **Core task còn thiếu** | `PurgeExecutions`, `PurgeFlows`, `PurgeStorage`, `TemplatedTask`. `PurgeLogs.purgeNonExecutionLogs` | 0 | S mỗi cái |
| 15 | **Metric** | Kho metric, `metric.Publish`, metric của `Assert`, API metric, và `metric()` của plugin host ghi metric thật | 0 | L |
| 16 | **`sla: MAX_DURATION` và `checks:`** | Cả hai, kèm mô hình thời gian của harness mà chúng cần | 5 | M–L |
| 17 | **Runner và ngôn ngữ script** | `WorkingDirectory` trên Kubernetes. `dependencies:`/uv/`packageManager`/`pythonVersion` của Python. node, R, Julia, Ruby, Go và các plugin script khác của Kestra. Các tuỳ chọn Docker và `targetOS` đang bị từ chối | 0 | M + S–M mỗi cái |
| 18 | **Plugin host đầy đủ** (mở lại trigger dạng plugin) | Trigger plugin dạng polling và realtime. Task plugin dạng flowable và dynamic-result. Các chỗ thiếu của `RunContext` (exit code 4). Render trì hoãn và render theo từng dòng. Thêm jar plugin | 0 | L |
| 19 | **Flow API** | Import/export, bật/tắt/xoá hàng loạt, tìm-trong-source/thay thế, validate task/trigger, xoá revision | 0 | M |
| 20 | **Các khác biệt còn lại không vì bảo mật** | Các mục 8 (lịch sử namespace file đã xoá mềm), 9 (window của Flow trigger bị xoá theo listener, window hết hạn bị thay) và 12 (kill execution đã terminal trả 409) của Phụ lục B §8: làm giống Kestra, trừ khi spec chỉ ra hành vi của Kestra là lỗi. Mọi mục khác của §8 đều đã có chỗ: 1, 2 và 4–7 ở §3.1; 10, 11, 14 và 16 ở §3.2; 3 ở #10 (nửa về KV ở §3.1); 13 ở #14 và #15; 15 ở #1 | 0 | S mỗi cái |

**Kỳ vọng sau #2:** baseline giảm từ 73 xuống khoảng 52.

**Tách đôi, 2026-10-27 (spec `2026-10-26-workflow-end-hooks-design.md`, D1 và Q1: có).** Dòng #2 thành hai PR và dòng ở trên không bị sửa.
- **#2a (đã xây, PR đang review):** `finally:` trên mọi flowable trừ `Pause`, `finally:` của chính flow, và `errors:`/`finally:` trên `Loop`, `LoopUntil` và `WorkingDirectory`. Đo được: 12 mục bank, baseline từ 72 xuống **60** (bảng trên kỳ vọng 13 và khoảng 14 đến 15 chặt; `task-runif-taskswithstate` không bank, nguyên nhân renderer, #5).
- **#2b (chưa xây):** `afterExecution:` của flow, pha sau terminal và khác biệt về kill (§3.1). Kỳ vọng 8 mục bank, xuống 52 (§7.1 của spec kỳ vọng 51 từ 59; tính từ 60 là 52).
- **`finally:` của `Pause` chuyển sang #9** (spec D14, Q5: có). Đến lúc đó nó bị từ chối và nêu #9.

**Dòng #2 xong, 2026-10-28 (#2b đã xây trên `feat/workflow-end-hooks-b`, xếp chồng lên PR #759 = #2a; PR #2b đang review; dòng chỉ được đánh dấu merged sau khi cả hai PR merge).** Đo bằng `cargo test -p egret-workflow --test executor -- --nocapture`: ratchet **301 / 52 / 52 / 0**, `(301, 249, 52)`. `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21 không đổi.
- **Cả dòng #2:** 20 mục bank, baseline từ 72 xuống **52** (#2a 12, #2b 8), so với "~20 upper, 14–15 strict" ở dòng và "khoảng 52" dưới bảng.
- **#2b bank đủ 8 mục kỳ vọng:** `valids/after-execution-error`, `-finally`, `-flowable`, `flow-concurrency-queue-after-execution`, `flow-concurrency-queue-killed-after-execution`, `restart-with-after-execution`, `sleep-long`, `sleep-long-after-execution-flowable`.
- **Không bank, thuộc về gói khác:** `valids/after-execution` (`outputs:` cấp flow, #6, ghi chú 6), `valids/pause-errors-finally-after-execution` (#9), `valids/task-runif-taskswithstate` (#5, ghi chú 10).
- **Xây thêm ngoài các bank:** pha sau terminal không bao giờ ghi vào execution; cha nghe được con khi `afterExecution:` của con xong; `Pause` và `LoopUntil`/`WaitFor` bên trong `afterExecution:` bị từ chối nêu #9, `Exit` nêu #20, một `retry:` ở đó bị cảnh báo nêu #13; Quy tắc 2 (bên dưới); các chỗ khác Kestra về kill (§3.1); các trang execution theo dõi after-run (giới hạn 30 phút sau khi kết thúc).

*Quy tắc 2 như đã xây, 2026-10-28 (#2b Task B5).* Quy tắc 2 đã đóng. Quy tắc superseded của orphan sweep, storage pruner và `complete` của route log đọc `workflow.execution_is_idle` (migration 0210: terminal và không run nào còn sống); `_wd` sweep đọc state của chính block. Hai khoản nợ được ghi ở spec end-hooks §6 như-đã-xây: rò rỉ `Exit`-anh-em (dòng #20 bên dưới) và chi phí của `protected_set` (0,12 giây lên khoảng 4 giây trên 100 000 execution, mỗi giờ; một dạng rẻ hơn được nêu ở đó).

**Ghi chú, 2026-10-28 (gói #2b, D9, D12 và §6 của spec end-hooks như đã xây; các dòng ở trên không bị sửa).**
11. **#20 thêm:**
    - (a) **`Exit` bên trong `afterExecution:`** bị từ chối lúc lưu và lúc start (`unsupported_after_execution_task`, nêu #20): một `Exit` sẽ đặt state của một execution đã kết thúc. Kestra chạy nó; nó làm gì ở đó thì chưa đo.
    - (b) **Run kẹt của `Exit`-anh-em.** Một `Exit` không phải `KILLED` cạnh một flowable để một flowable của danh sách chính `RUNNING` (và một lá chưa bao giờ start `CREATED`) mãi mãi, nên execution không bao giờ idle. Đo trong `tests/exit_sibling_idle.rs`; giống hệt trước #2 (`1e0a3cf8`). Hệ quả: pruner không bao giờ dọn family đó; `protected_set` ghim tập đọc được của nó; một container được giữ bị thay thế của nó không bao giờ được xét; `complete` của route log không bao giờ true; frontend ngừng theo dõi nó 30 phút sau khi kết thúc (`AFTER_RUN_FOLLOW_MS`). Không mất gì; dữ liệu được giữ. Cách sửa khuyến nghị (phương án 1 của ghi chú §6 trong spec): một execution terminal tiếp tục đóng các run danh sách chính, không tạo hay start gì. Cần một phép đo Kestra trước về state kết thúc của một anh em chưa bao giờ start.
    - (c) Mục 12 của dòng #20 ("kill trên một execution terminal trả 409: căn chỉnh với Kestra") nay chỉ áp dụng cho một execution terminal không có run `afterExecution:` còn sống; khi có, kill được chấp nhận có chủ ý (§3.1).
12. Quy tắc V3 của `WorkingDirectory` (claim prepare hay finish fail kết thúc block `FAILED` mà không có `errors`/`finally`) vốn thuộc #17, ghi chú 9; #2b không đổi nó.

**Ghi chú, 2026-10-26 (§9 của spec gói #1, `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md`; các dòng ở trên không bị sửa).** Đính chính 6 và 8 nằm dưới Phụ lục B §4 và §8 (bản EN), và 7 ở §4 bên dưới.
1. **#9:** `tasks:` của `Pause` không phải property của Kestra (`Pause.java:158`: field của nó là `pauseDuration`, `behavior`, `onPause`, `onResume`, `errors`, `finally`). Nó bị bỏ khỏi phạm vi của #9 và vẫn bị từ chối vĩnh viễn, như một property không xác định (mã `unsupported_pause_property` được giữ). #9 còn thêm `checkFrequency`/`failOnMaxReached` không phải literal của `LoopUntil` (`unsupported_loop_until_property`; spec Q3).
2. **#11:** thêm `allowConcurrent` của Schedule (mặc định của Kestra là `false` và khoá trigger đến khi execution của nó xong, `TriggerScheduler.java:345-349`; egret không bao giờ khoá; gói #1 chỉ cảnh báo khi viết `false` tường minh), `failOnTriggerError`, và `when` của Webhook.
3. **#2:** thêm `errors:` của `LoopUntil`, `errors:` của `WorkingDirectory` và `finally:` của `WorkingDirectory` (đều là `AbstractBranch`). Một version publish trước #2 giữ chúng chỉ trong `snapshot.source`, nên #2 phải suy lại plan từ source (spec §10).
   - *Đính chính, 2026-10-27: tiền đề `snapshot.source` là sai* (spec end-hooks, tiền đề sai 1). Không run nào đọc snapshot: một lần start chạy `flows.source` của dòng draft, chép nó vào `executions.source`, và mỗi claim lập lại plan từ bản chép đó. Không có gì cần suy lại. Rủi ro thật là `task_ix` theo vị trí, được xử lý bằng plan ổn định chỉ số (A3) và fixture A1. #2a cũng xây `errors:`/`finally:` của `WorkingDirectory` (Q4: mặc định; code của Kestra có vẻ không bao giờ khởi chạy `finally:` của WD, coi là lỗi).
4. **#3:** thêm `isRadio` và `autoSelectFirst` của SELECT/MULTISELECT như gợi ý cho form start, và `expression` của SELECT/MULTISELECT.
5. **#17:** thêm các pattern `**` và `[...]` của `outputFiles` trong script (`worker/commands.rs:3108-3124`).

**Ghi chú, 2026-10-27 (gói #2, như-đã-xây §7.1, §9 và §4.1 của spec end-hooks).**
6. **#6:** thêm việc tính `outputs:` cấp flow. `valids/after-execution` vẫn ở baseline vì `afterExecution:` của nó render `{{ execution.outputs.final }}` mà egret không bao giờ tính `outputs:` của flow (`worker/render.rs:279-284`).
7. **#7:** khi xây restart tại chỗ, nó phải xoá các run `errors`, `finally` và `afterExecution`, như `ExecutionService` của Kestra làm (`ExS:339-351`). Hiện đang tiềm ẩn: `run_of` lấy run đầu tiên của một task, nên một restart cấp execution sẽ đọc một run end-hook cũ.
8. **#9:** sở hữu `finally:` của `Pause` (bị từ chối và nêu #9 cho đến lúc đó).
9. **#17:** một block `WorkingDirectory` có claim prepare hoặc finish thất bại kết thúc `FAILED` mà không chạy `errors`/`finally` của nó (quy tắc V3, ghim bằng một test trong `tests/working_dir_claims.rs`); xem lại khi có runner Kubernetes.
10. **#5:** sở hữu `valids/task-runif-taskswithstate` (test `empty` của Pebble trong `runIf`).

## 3. Những gì vẫn khác, và vì sao

### 3.1 Khác biệt vì bảo mật (giữ lại)

Những điểm này được giữ vì làm giống Kestra sẽ mở lỗ hổng xuyên workspace, mở SSRF, hoặc để tài nguyên không giới hạn trong một hệ thống nhiều tenant. Kestra OSS là single-tenant.
- **Feature của workspace và allowlist:** container và plugin nằm sau feature của workspace, mặc định tắt, cộng thêm allowlist toàn cục cho image và type (mục 1 của §8).
- **Runner:** runner `Process` bị từ chối, và chart từ chối runner Docker trong production (mục 2 của §8). `WorkingDirectory` sẽ có Kubernetes ở #17 thay thế.
- **`secret()` trong giá trị KV:** `secret()` bên trong giá trị của KV `Set`/`Put` vẫn bị từ chối lúc publish, để secret không bao giờ được lưu dạng rõ. Phần còn lại của mục 3 của §8 được #10 gỡ.
- **`http()` trong biểu thức:** bị từ chối, vì nó render trên control plane (mục 4 của §8). Các task http ở #4 chạy trên worker, sau một egress policy.
- **`allowedFileExtensions`:** allowlist của input FILE được thực thi (mục 5 của §8).
- **Giới hạn trong namespace:** KV, ghi namespace file, `PurgeKV`, `PurgeFiles` và `PurgeLogs` chỉ tác động lên namespace của chính flow và các namespace con (mục 6 của §8).
- **Giới hạn kích thước:** kích thước và mật độ giá trị KV, số chữ số (mục 7 của §8), và `executions.variables`/`Labels` (D8, D9 của gói nợ).
- **Kill dừng các run `afterExecution:` (dự kiến trong #2b, quyết định Q3 của người dùng, 2026-10-27).** Kestra trả 409 cho lệnh kill một execution đã terminal và vẫn ép chạy danh sách `afterExecution:` sau một lần kill. egret sẽ chấp nhận kill một execution đã terminal mà còn after-task đang chạy, chuyển các run đó sang KILLING/KILLED qua đường kill hiện có, và không bao giờ đổi trạng thái cuối của chính execution, để một tenant không thể tạo ra task mà không ai kill được. Chưa xây trong #2a: Task B3 của Phần B.
  - *Đã xây, 2026-10-28 (#2b, Task B3 và vòng sửa của nó).* Lệnh kill một execution terminal có một run còn sống của `afterExecution:` được chấp nhận (202); các run chuyển `KILLING`/`KILLED` và state, history và `ended_at` của execution không bị ghi; không after-task nào được tạo thêm. Không có after-run còn sống thì vẫn là `409 execution_terminated`; một phase kill chưa áp dụng được (một `WorkingDirectory` after-task gốc đang giữa claim) là `409 kill_not_applied` có thể retry. `outcome` của audit là state của chính execution cộng `after_execution`.
- **Cascade kill của cha dừng `afterExecution:` của một con terminal (đã xây trong #2b, quyết định của controller dưới Q3, 2026-10-27).** Cascade của Kestra bỏ qua hậu duệ terminal (`ExecutionService.java:998-1000`). Cascade của egret giữ một hậu duệ terminal có run còn sống và chỉ kill các run pha-after của nó, không bao giờ ghi state của con. Không có nó, kill một cha sẽ để task after chạy lạc của con chạy tiếp. Một con mà run còn sống duy nhất là một `Exit` anh em vẫn bị bỏ qua.
- **Tham chiếu theo workspace:** publish chỉ phân giải đích của Subflow/`dependsOn` trong workspace của người publish (§5.1 của spec nợ).

### 3.2 Thiết kế riêng của egret được giữ, không mất hành vi nào của Kestra

- **Tách draft/publish** (mục 11 của §8).
- **Dòng log** nằm trong Postgres có retention thay vì object storage (mục 16 của §8). Đây là lựa chọn lưu trữ, không phải hành vi.
- **Node `Dag` bị disable** bị bỏ cùng các cạnh của nó, trong khi Kestra bị treo (mục 14 của §8). Đây là lỗi của Kestra mà egret không chép theo.
- **Publish đóng băng tham chiếu** (mục 10 của §8). Spec của #12 phải nêu lại phần nào của nó còn lại sau §5.1 của spec nợ, khi một đích vắng mặt lúc publish không bị đóng băng gì và lúc chạy egret phân giải theo tên.

### 3.3 Ngoài phạm vi engine, hoặc không thuộc OSS

- **Kestra Enterprise:** `quotas`, `workerSelector` (cấp flow và task), `policyRefs`, input FORM và REUSABLE_INPUTS, API namespace secret.
- **Các bề mặt sản phẩm bên cạnh engine:** dashboard và preview, blueprint, AI, MCP (kể cả `McpToolTrigger`), Apps.
- **Kiểm ở spec của #13:** nếu Kestra OSS có `assets` (lineage) và `version` plugin theo task, #13 nhận làm; nếu chúng chỉ có ở Enterprise, chúng chuyển sang đây.

### 3.4 Giới hạn của harness

8 mục baselined: độ trễ timing và poller, cùng các cặp thứ tự mà driver không có đồng hồ không thể đạt tới. Đây là giới hạn của oracle driver, không phải tính năng thiếu. Chúng vẫn nằm trong baseline trừ khi driver có mô hình thời gian; #16 xây mô hình đó và đo lại chúng.

## 4. Giữ cho tài liệu luôn cập nhật

Đo lại mỗi khi một gói merge: kết quả in ra của ratchet cho số liệu corpus, và `python3 handler_surface.py` cho bề mặt handler. Cập nhật §1, đánh dấu gói đã xong ở §2 kèm PR của nó, và giữ nguyên các phụ lục như bản ghi có ngày của 2026-10-18. Khi spec của một gói tìm ra một khoảng trống mà không dòng nào nêu tên, thêm nó vào một dòng hoặc vào §3 kèm lý do. Đừng bao giờ để nó nằm ngoài danh sách.

**Ghi chú, 2026-10-26 (gói #1, đính chính 7): bản pin quyết định.** Bản xuất schema đã commit (rc13, dựng 2026-09-01) mới hơn bản pin của source (`96893b589`, 2026-08-17). Chỗ nào hai bản khác nhau, bản pin quyết định: rc13 có `onChildFailure` trên `Parallel` và `Dag`, bản pin không có (`git log -S onChildFailure` rỗng ở bản pin), nên nó bị từ chối như property không xác định và hai dòng của nó trong `property-treatment.json` là `refused`. Hãy đo lại với một bản pin mới hơn trước khi coi rc13 là thẩm quyền.

**Ghi chú, 2026-10-26 (gói #1, đính chính 6 và 8, vốn sửa Phụ lục B; bản VI không có phụ lục, nên ghi ở đây):**
- **Đính chính 6 (Phụ lục B §4):** dòng "Schedule.lateMaximumDelay | supported | trigger/schedule.rs:1368" là sai. Nó bị từ chối khi scheduler đồng bộ trigger (`trigger/schedule.rs:1368-1371`, hiện ở `last_error` của dòng trigger), nên trigger không bao giờ chạy; từ gói #1 nó còn được cảnh báo lúc publish (`schedule_not_resolvable`, nêu #11). Bản EN giữ dòng cũ làm bản ghi 2026-10-18.
- **Đính chính 8 (Phụ lục B §8, mục 15):** mục 15 ("Các property không xác định và không được hỗ trợ bị từ chối theo tên trên script task và plugin task, nhưng bị bỏ qua trên core task") được gói #1 đóng. Một khoá không xác định trên core task, flowable, flow, trigger, input, output hay bean lồng nay là lỗi `unknown_property` lúc publish, như Kestra từ chối; một khoá Kestra chấp nhận mà egret không hành động theo là cảnh báo `not_honoured` nêu gói sẽ gỡ nó.
