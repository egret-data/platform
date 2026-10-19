> Bản tham khảo tiếng Việt. Bản gốc (chuẩn) là [docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md](../../en/plans/2026-10-18-workflow-kestra-parity-roadmap.md); khi hai bản khác nhau, bản tiếng Anh thắng.

# Workflow engine: lộ trình đạt parity với Kestra

**Trạng thái:** thứ tự đã được người dùng duyệt ngày 2026-10-18. Đo ngày 2026-10-18 trên `feat/workflow-kestra-executor-tasks` (main `a2850644` + nhóm C2 đang làm) so với bản checkout Kestra `data-portal/kestra` tại `96893b589` (2.0.0-SNAPSHOT, bản pin của chương trình). Cả hai phép đo chỉ đọc code và file oracle (không cargo, không docker); bằng chứng, kèm `file:line` cho từng dòng, nằm ở Phụ lục A (oracle corpus) và Phụ lục B (bề mặt tính năng) của bản tiếng Anh, tại [bản tiếng Anh](../../en/plans/2026-10-18-workflow-kestra-parity-roadmap.md).

**Mốc thời điểm (2026-10-18, final review C2):** §1 và các phụ lục là phép đo ngày 2026-10-18 khi C2 còn đang làm; C2 đã được xây xong sau đó (73 baselined, LIVE 293) — xem §2 dòng 0.

**Spec của chương trình:** `docs/en/specs/2026-09-01-workflow-programme-design.md`. Mọi gói W trong §9 của spec đó đã merge; lộ trình này sắp xếp những gì đến sau nhóm C (`docs/en/specs/2026-10-18-workflow-core-tasks-design.md`).

## 1. Hiện trạng của egret

**Oracle corpus** (các flow test của chính Kestra, `crates/egret-workflow/oracle-exec/`):

| | số lượng | nguồn |
|---|---|---|
| flow trong corpus | 351 (một số comment trong `driver.rs` vẫn ghi 345, đã cũ) | `corpus.json` |
| đã publish và chạy thật trên egret | 281 | `driver.rs` `LIVE` |
| được so với golden của Kestra | 301 | `golden.json` |
| khớp golden | 217 | 301 − 84 |
| lệch, đã đưa vào baseline kèm nguyên nhân | 84 | `baseline.txt` |
| Kestra chạy được, egret từ chối | 18: 12 thuộc nhóm C2, 3 thuộc tính của `Pause` và 1 `retry:` trong block `AllowFailure` (cả hai đã quyết định), 2 từ chối thừa | `executor/mod.rs` `PUBLISHED_BUT_REFUSED` |

84 trường hợp lệch theo nguyên nhân: C2 12, `finally:` 11, `afterExecution:` 9, renderer lệch 8, `sla: MAX_DURATION` 5, `outputs:` riêng của `Loop` 5, độ trễ timing/poller 4 và các cặp thứ tự mà driver không có đồng hồ không thể đạt tới 4 (cả hai nhóm này phần lớn là giới hạn vĩnh viễn của harness), flow invalid bị chấp nhận thừa 4, các lệch `Loop` chưa quy được nguyên nhân 4, `Pause` 3, flow `outputs:`/SECRET 3, `WorkingDirectory`+`runIf` 3, retry của Subflow 2, thiếu hàm Pebble 2, nguyên nhân đơn lẻ 4, cố ý 1.

**Bề mặt tính năng** (Kestra 2.0 OSS, ngoài corpus):
- **Core task:** 49. 38 đã hỗ trợ (11 trong số đó hỗ trợ một phần), 3 đang làm (C2), 8 còn thiếu: `http.Request`, `http.Download`, `http.SseRequest`, `PurgeExecutions`, `PurgeFlows`, `PurgeStorage`, `metric.Publish`, `TemplatedTask`.
- **Bỏ qua im lặng, nhóm tệ nhất:** `finally:` và `afterExecution:` ở cấp flow được parse nhưng không bao giờ được lập kế hoạch (đã kiểm: `build_plan` không đọc cái nào; chỉ có một test về độ sâu), và `finally:` trên mọi flowable trừ `AllowFailure` cũng bị bỏ. `stopAfter` và `retry.warningOnRetry` được parse nhưng không có tác dụng. Không có cảnh báo nào nêu tên chúng.
- **Đã đăng ký nhưng không có ai phục vụ:** `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack` có dòng trong `TASK_TYPES` nhưng không deployment nào nối handler (đã kiểm: `runtime.rs` không nối cái nào), nên publish từ chối chúng với `task_type_not_served`.
- **Inputs:** `required`, type, `values`, `validator`, min/max và giới hạn ngày không được kiểm khi start thủ công, qua API, webhook hay flow-trigger; chỉ backfill kiểm `required`.
- **Trigger:** thiếu `ScheduleOnDates` và `http.Trigger`; Schedule `when:` và webhook `wait`/`returnOutputs` bị từ chối.
- **Thao tác trên execution:** đã có start, list, get, graph, logs, files, kill, resume (không kèm inputs) và replay toàn bộ execution. Còn thiếu restart tại chỗ, replay từ một task, change-status, pause, delete, API labels, thao tác hàng loạt, streaming trực tiếp và endpoint eval biểu thức.
- **Biểu thức:** `render`/`renderOnce`/`errorLogs`/`subflow` bị từ chối; thiếu `max`/`min`, mười filter Pebble và các test `null`/`empty`/`map`/`json`; `secret()` chỉ phân giải trong task plugin `kestra.*`.
- **Plugin:** plugin host chạy các `RunnableTask` từ 3 jar (9 class JDBC), mỗi lần thử mất 1,6–3,6 s để khởi động JVM.
- **Runner:** `WorkingDirectory` chỉ chạy trên Docker, thứ mà chart từ chối trong production.

## 2. Thứ tự đã duyệt

Mỗi gói đi qua quy trình thường lệ: spec (hoặc một mục spec) → plan → build bằng subagent → review cuối → gate → PR → merge. Kích cỡ: S ≈ một task, M ≈ một gói, L ≈ hơn một gói. "Banks" là cận trên của oracle lấy từ Phụ lục A của bản tiếng Anh; ratchet đo con số thật.

| # | Gói | Phạm vi | Banks | Cỡ |
|---|---|---|---|---|
| 0 | **Nhóm C2** (đã xây 2026-10-18, PR chưa có: 11 trên 12 entry đã bank, 73 baselined, LIVE 293) | `SetVariables`, `UnsetVariables`, `Labels`; migration 0196 | 11 trên tối đa 12 | M |
| 1 | **Không bỏ qua im lặng** | Biến mọi key được parse mà bị bỏ qua thành một lỗi từ chối hoặc cảnh báo có nêu tên: `finally:`/`afterExecution:` ở cấp flow và flowable (cho đến khi #2 chạy được chúng), `stopAfter` của trigger, `retry.warningOnRetry`, các key của Subflow mà egret không mô hình hoá (`revision`, `labels`, `inheritLabels`, `scheduleDate`, `restartBehavior`), các key của input mà egret không mô hình hoá. Sửa các con số "345" cũ trong comment của `driver.rs` | 0 (đúng đắn) | S |
| 2 | **Hook kết thúc: `finally:` và `afterExecution:`** | `finally:` và `afterExecution:` cấp flow, và `finally:` trên mọi flowable (`AllowFailure` đã lập kế hoạch một cái nên mẫu đã có); `Loop errors:` nếu làm luôn được với chi phí thấp | ~20 cận trên, 14–15 chặt | M |
| 3 | **Kiểm inputs khi start** | `required`, lỗi ép kiểu, `values`/`allowCustomValue` của SELECT/MULTISELECT, `validator` của STRING, min/max của INT/FLOAT, giới hạn ngày, items của ARRAY, khi start thủ công, qua API, webhook và flow-trigger; endpoint `validate inputs` | một vài invalid | M |
| 4 | **HTTP và các task nền tảng chưa được phục vụ** | `http.Request` và `http.Download` làm worker handler có egress policy (cùng lập trường SSRF với `http()`); quyết định và nối `egret.sql` (cần một `QueryExecPort`) cùng các dòng `egret.*` khác, hoặc gỡ các dòng đó | 0 | M |
| 5 | **Parity của renderer** | Chia cho 0 (trường hợp Infinity của `restart-*`), `taskrun.value` không xác định, so sánh số với chuỗi, `??` trên root không tồn tại; `render`/`renderOnce`, `errorLogs`, `max`/`min`, các filter và test còn thiếu | ~8–10 | S–M |
| 6 | **`outputs:` riêng của `Loop`** | Đánh giá `outputs:` của `Loop` khi nó kết thúc (STORE/AUTO vẫn hoãn theo phán quyết 2026-10-03); trước hết xác định nguyên nhân của 4 mục `Loop` chưa quy được | 3 (+ tối đa 4) | M |
| 7 | **Thao tác trên execution** | Restart task lỗi tại chỗ, replay từ một task được chọn (kèm inputs), change-status, pause, delete, API labels; sau đó là thao tác hàng loạt | 0 (vận hành) | M–L |
| 8 | **`concurrency:` của flow** | `limit` với QUEUE/CANCEL/FAIL, unqueue và force-run | ít | M |
| 9 | **Về sau, theo thứ tự này trừ khi xếp hạng lại** | Schedule `when:` + `stopAfter` + `ScheduleOnDates` (S–M); webhook đồng bộ `wait`/`returnOutputs` (M); `secret()` ngoài task plugin, bằng cách inject thay vì render (M–L); `WorkingDirectory` trên Kubernetes (M); `sla: MAX_DURATION` kèm thay đổi mô hình thời gian của harness (M–L); `dependencies:`/uv cho Python và thêm ngôn ngữ script (S–M mỗi cái); `labels`/`inheritLabels`/`revision`/`scheduleDate` của Subflow, `onResume` của Pause và resume kèm inputs (S–M mỗi cái); `checks:`, `metric.Publish` và metric store (L), `http.Trigger`, `PurgeExecutions`/`PurgeFlows`/`PurgeStorage`, `TemplatedTask`; SSE follow và endpoint eval biểu thức | — | — |

**Kỳ vọng sau #0 và #2:** baseline giảm từ 84 xuống khoảng 52.

## 3. Không nằm trong kế hoạch

- **Giới hạn của harness** (8 mục): độ trễ timing và poller, cùng các cặp thứ tự mà driver không có đồng hồ không thể đạt tới. Chúng vẫn nằm trong baseline trừ khi driver có thêm mô hình thời gian.
- **Khác biệt cố ý** (16, liệt kê ở Phụ lục B §8 của bản tiếng Anh), trong đó có: ghi và purge giới hạn trong namespace, `http()` bị từ chối, runner `Process` bị từ chối, phạm vi `PurgeLogs`, `Assert` không phát metric.
- **Từ chối đã quyết định:** `behavior:`/`errors:`/`onPause:` của `Pause` (phán quyết 2026-10-11) và `retry:` trong block `AllowFailure`, cho đến khi có gói nhận làm; trigger plugin (W5b §11).
- **Ngoài phạm vi workflow engine:** dashboard, preview, blueprint, AI, MCP, Apps và các tính năng EE của Kestra.

## 4. Giữ cho tài liệu luôn cập nhật

Đo lại mỗi khi một gói merge: kết quả in ra của ratchet cho số liệu corpus, và `python3 handler_surface.py` cho bề mặt handler. Cập nhật §1, đánh dấu gói đã xong ở §2 kèm PR của nó, và giữ nguyên các phụ lục như bản ghi có ngày của 2026-10-18.
