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
