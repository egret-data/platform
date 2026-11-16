# Gói workflow 7d: xoá một execution, sửa label, tải và xoá log

> Bản dịch tiếng Việt của `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md`. Khi hai bản khác nhau, bản tiếng Anh là chuẩn.

**Trạng thái:** thiết kế ngày 2026-11-16. Branch `feat/workflow-delete-labels-logs`, một PR. Dòng #7 của roadmap (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), gói con thứ tư trong sáu gói 7a–7f. 7a (#860), 7b (#866) và 7c (#868) đã merge; spec của chúng là `2026-11-13-workflow-execution-ops-restart-design.md`, `2026-11-14-workflow-replay-from-task-design.md` và `2026-11-15-workflow-pause-resume-design.md`. Kestra là bản checkout tại `data-portal/kestra`. `EC` là `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`, `ES` là `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `LC` là `webserver/src/main/java/io/kestra/webserver/controllers/api/LogController.java`.

## 1. Vấn đề

Kestra có bốn thao tác trên execution mà egret còn thiếu:

1. **Xoá một execution** (`DELETE /{id}`, `EC:478-494`; `ES.delete`, `ES:747`). Nó xoá cứng execution và, theo các cờ đều mặc định là `true`, cả log (`deleteLogs`), metric (`deleteMetrics`) và internal storage tại `kestra:///{ns path}/{flow slug}/executions/{id}` (`deleteStorage`). 404 nếu id không tồn tại, ngược lại 204. Nó không kiểm tra trạng thái, không kill gì, và để yên các subflow con.
2. **Đặt label** (`POST /{id}/actions/labels`, `EC:2352-2380`). Body, một danh sách `{key, value}`, THAY THẾ label của execution; các label `system.*` hiện có được nối lại, và một label `system.*` trong body khác với label hiện có thì bị từ chối (`mergeSystemLabels`). Chỉ execution đã kết thúc mới được sửa (400 nếu không). Key khớp `^\p{Ll}[\p{L}0-9._-]*$`, value không rỗng. 200 kèm execution.
3. **Tải log** (`GET /logs/{id}/download`, `LC:114-137`). `text/plain`, attachment `<id>.log`, mỗi entry một dòng theo `LogEntry.toPrettyString` (`<instant ISO> <LEVEL> <message>`), nối bằng `\n`. Cùng bộ lọc với danh sách log. `Cache-Control: no-cache` khi execution chưa kết thúc.
4. **Xoá log** (`DELETE /logs/{id}`, `LC:192-201`; `logRepository.deleteByQuery`). `taskId`, `taskRunId`, `attempt`, `minLevel` là tuỳ chọn, kết hợp bằng AND; `minLevel` xoá level đó và các level cao hơn. Không kiểm tra trạng thái.

egret hiện tại (khảo sát, 2026-11-16):

- Không có gì xoá được execution. Flow purge giữ lại execution (`flow_id` thành NULL, `repo.rs:757`). `task_runs`, `task_logs`, `execution_files`, `loop_file_values`, `flow_trigger_events` và các loop/retry con (`parent_execution_id`) cascade khi xoá; `trigger_execution_id` (migration 0138) và `replayed_from_execution_id` (0166) là `NO ACTION`. Object đã lưu thì không bao giờ cascade: `storage/prune.rs` xoá các prefix của một family (`own_prefix`, `prune.rs:524`) trước các row của nó.
- `workflow.executions.labels` chỉ chứa các label ngoài label của chính flow (migration 0192, `labels.rs`). Ba nơi đọc gộp nó với `labels:` của source đã đóng băng qua `labels::execution_labels` (`routes.rs:1854` detail, `worker/render.rs:309` render, `flow_trigger/process.rs:808`). Task `Labels` ghi cả danh sách đã gộp trở lại (`worker/claim.rs:5242`). Vì vậy không thể bỏ một label của flow: nó quay lại ở mọi lần đọc.
- `GET …/executions/{id}/logs` phân trang `workflow.task_logs` (`routes.rs:2254`). Nút Download của frontend phân trang API đó ngay trong trình duyệt (`LogView.tsx`, `LOG_DOWNLOAD_MAX_LINES = 100_000`). Không có route nào xoá log; `LogRepo::purge` có sẵn cho retention (`logs/repo.rs:413`).

## 2. Quyết định

### 2.1 Quyết định của người dùng, 2026-11-16 (đã hỏi, đã trả lời)

- **Q1. Chỉ execution đang rảnh (idle) mới xoá được.** Execution và các run `afterExecution:` của nó phải đã kết thúc (`workflow.execution_is_idle`, migration 0210). Nếu không, 409 `not_terminated`. Kill trước, rồi xoá. Kestra xoá ở mọi trạng thái; egret từ chối vì một worker còn sống sẽ ghi vào các row đã mất. §5 điểm khác 1.
- **Q2. Execution có phụ thuộc thì bị từ chối.** Nếu một subflow con (`trigger_execution_id`) hoặc một replay hay restart (`replayed_from_execution_id`) trỏ tới nó, 409 `has_dependents`, nêu id của chúng. Xoá những cái đó trước. Kestra để lại tham chiếu treo. §5 điểm khác 2.
- **Q3. Label làm theo Kestra:** body thay thế toàn bộ danh sách hiển thị, các label `system.*` đã có được giữ, một `system.*` mới hoặc bị đổi thì bị từ chối, và chỉ execution đã kết thúc mới sửa được.
- **Q4. Log: theo từng execution với các bộ lọc của Kestra, cộng thêm tải về từ server.** Xoá theo flow nằm ngoài phạm vi.
- **Q5. Cờ xoá: chỉ `deleteStorage` thay đổi được điều gì.** Log luôn đi cùng execution (`task_logs` cascade), nên `deleteLogs=false` bị từ chối với 400. egret không có metric; `deleteMetrics` được chấp nhận và không làm gì. §5 điểm khác 3.
- **Q6. Cho phép xoá log của execution đang chạy**, như ở Kestra. Các dòng được ghi sau lúc xoá vẫn còn.
- **Q7. Giao diện chỉ ở trang execution.** Header: Delete và Edit labels. Tab Logs: Download từ API server, Delete logs cho execution và cho một task run. Không có thao tác trên dòng của danh sách; bulk thuộc 7f.
- **Q8. Bỏ được một label của flow (migration).** Một cột mới `workflow.executions.labels_replaced boolean NOT NULL DEFAULT false`. Khi nó là true, `labels` giữ TOÀN BỘ danh sách và `labels:` của flow không được gộp vào. Mọi nơi đọc đều đi qua một helper (D6).
- **Duyệt thiết kế, 2026-11-16:** người dùng đã duyệt thiết kế (§2.2–§2.3) bằng câu "ok tiếp tục đi".

### 2.2 Quyết định thiết kế

- **D1. Route xoá.** `DELETE /api/ws/{ws}/workflows/executions/{id}?deleteStorage=&deleteLogs=&deleteMetrics=`, đều là boolean tuỳ chọn mặc định `true`. `workflow:manage`. 204 khi thành công. Id của workspace khác hoặc không tồn tại là 404. Flow trong trash hay đã purge KHÔNG chặn việc xoá: xoá chính là cách dọn execution cũ.
- **D2. Chỉ xoá root.** Một loop iteration hoặc retry con (`parent_execution_id IS NOT NULL`) được xoá cùng root của nó, không xoá riêng: 409 `not_a_root`, nêu root. §5 điểm khác 4.
- **D3. Thứ tự: object trước, row sau, dưới khoá execution.** Worker lấy khoá execution, kiểm tra lại idle và phụ thuộc (lần đọc của route có thể đã cũ), rồi, với `deleteStorage`, xoá prefix của từng thành viên trong family (root và mọi hậu duệ `parent_execution_id`) bằng `own_prefix`. Thành viên có prefix không chứng minh được là của chính nó (`own_prefix` → `None`) thì bị bỏ qua và ghi log, như pruner làm. Lỗi store trả 500 và không xoá row nào, nên có thể gọi lại. Sau đó `DELETE FROM workflow.executions WHERE id = $root` trong cùng transaction với lần kiểm tra lại; cascade lo phần còn lại.
- **D4. Audit.** `workflow_execution:delete` với `{namespace, flow_key, state, delete_storage, executions, files}`: sau đó row đã mất, nên chi tiết nêu nó từng là gì. Không có giá trị input hay output nào.
- **D5. Route label.** `POST /api/ws/{ws}/workflows/executions/{id}/labels`, body `[{"key","value"}]`, `workflow:manage`. Dưới khoá execution: không idle → 409 `not_terminated`. Kiểm tra hợp lệ, mỗi lỗi là 400 `invalid_labels` nêu label đó: các quy tắc của `backfill_labels_refusal` (≤ 50 label, key không trống, không trùng, value không rỗng, key ≤ 128 và value ≤ 1024 ký tự) trừ quy tắc cú pháp template, vốn chỉ riêng backfill, cộng với pattern key của Kestra `^\p{Ll}[\p{L}0-9._-]*$` cho key chưa có trong danh sách hiển thị (một flow có thể mang các key cũ không khớp, và gửi lại chúng không được thất bại); một key `system.*` trong body chỉ qua nếu cùng key và value đã tồn tại. Danh sách được lưu là body cộng mọi label `system.*` hiện có chưa nằm trong đó; `labels_replaced` được đặt true. 200 kèm detail của execution. Audit `workflow_execution:labels` với `{keys_before, keys_after}`; không có value.
- **D6. Một helper cho danh sách hiển thị.** `labels::visible_labels(flow: &[Label], own: &[Label], replaced: bool) -> Vec<Label>`: `replaced` → `own` đã khử trùng (bỏ key hoặc value rỗng, value cuối cùng đặt ở vị trí đầu tiên); nếu không thì `execution_labels(flow, own)`. Ba nơi đọc (detail, render, flow trigger) gọi nó và đọc `labels_replaced` cạnh `labels`. Task `Labels` trên một execution đã replaced gộp vào danh sách hiển thị và ghi lại với `labels_replaced` được giữ nguyên. Replay và restart trên một source mới sao chép `labels_replaced` cùng với `labels` (Kestra sao chép toàn bộ danh sách). Trường `labels` của DTO danh sách (chỉ phần own) hiển thị toàn bộ danh sách khi `labels_replaced`.
- **D7. Route tải log.** `GET /api/ws/{ws}/workflows/executions/{id}/logs/download?task_id=&task_run_id=&min_level=`, `workflow:read`, cùng bộ lọc và cùng phân quyền với `GET …/logs`. `text/plain; charset=utf-8`, `Content-Disposition: attachment; filename="<id>.log"`, mỗi entry một dòng `<ts RFC 3339 UTC, millis> <LEVEL> <message>`, nối bằng `\n`. Tối đa 100 000 dòng (giới hạn sẵn có của frontend); vượt quá thì dòng cuối là `… truncated at 100000 lines`. `Cache-Control: no-cache` khi execution chưa idle. Không có độ trễ settle: bản tải đọc những gì đã commit.
- **D8. Route xoá log.** `DELETE /api/ws/{ws}/workflows/executions/{id}/logs?task_id=&task_run_id=&attempt=&min_level=`, `workflow:manage`, 204. Các bộ lọc kết hợp bằng AND; `min_level` xoá level đó và các level cao hơn. Không kiểm tra trạng thái. Một câu `DELETE … WHERE execution_id = $1 AND …` (execution đã được lần đọc của route giới hạn trong workspace). Audit `workflow_execution:logs_delete` với các bộ lọc và số row.
- **D9. Mã lỗi.** Mới: `not_terminated` (409), `has_dependents` (409, `details.executions`), `not_a_root` (409, `details.root`), `invalid_labels` (400), `delete_logs_required` (400, cho `deleteLogs=false`).

### 2.3 Giao diện, test, tài liệu

- **D10. Header trang execution.** Nút **Delete** mở hộp thoại xác nhận với checkbox "Delete stored files" (được chọn) và dòng chữ nói rằng log và các loop iteration đi cùng; khi thành công, điều hướng về danh sách execution kèm toast. Nút **Edit labels** mở hộp thoại với các dòng key/value (label của flow và của execution, `system.*` chỉ đọc), thêm và xoá. Cả hai chỉ bật khi execution idle (trường `idle` của detail, thêm vào nếu detail chưa có), nếu không thì có tooltip. Thông báo 409 hiển thị trong hộp thoại.
- **D11. Tab Logs.** Download gọi route server với các bộ lọc hiện tại (một anchor tới URL). Mục menu **Delete logs** xoá theo các bộ lọc hiện tại sau khi xác nhận; menu của dòng task run có thêm **Delete logs** cho run đó. Cả hai cần `workflow:manage` và làm mới view.
- **D12. Test.** Thuần: `visible_labels`, kiểm tra hợp lệ label, dòng pretty. Worker: xoá dưới khoá (idle, phụ thuộc, root, lỗi store giữ nguyên row, prefix của family), thay label với system được giữ. Route: mọi mã ở trên, audit row không có value, quyền (`workflow:read` không xoá hay sửa được), workspace khác 404. Render: danh sách đã replaced tới được `{{ labels }}`; một task `Labels` sau khi sửa giữ nguyên việc đã bỏ.
- **D13. Tài liệu.** `egret-data/docs`: Executions → Run actions (Delete, Edit labels), Logs (download, delete), REST reference, ảnh chụp màn hình của hai hộp thoại và menu Logs.

## 3. Hình dạng của thay đổi

- Migration `0230_execution_labels_replaced.sql`: cột mới.
- `labels.rs`: `visible_labels`, `edit_labels_refusal`, `merge_system_labels`.
- `worker/delete.rs`: `delete_execution` dưới khoá; `worker/labels.rs` (hoặc cạnh `delete.rs`): `set_labels`.
- `logs/repo.rs`: `delete_for_execution(filters)`, `download_lines(filters, cap)`.
- `routes.rs`: bốn route, OpenAPI, mã lỗi; ba nơi đọc label chuyển sang `visible_labels`; restart/replay sao chép cờ.
- Frontend: client được sinh lại (`npm run gen:api`), `DeleteExecutionDialog`, `EditLabelsDialog`, menu tab Logs, nút trên header.

## 4. Test

Xem D12. Các cổng như ở 7c: `egret-workflow` mọi target (7 test `sql_starrocks_live` cần StarRocks), srv/config/metastore, ratchet và census không đổi, frontend lint/typecheck/build/bundle test, kiểm tra live trên DB và bucket dùng một lần.

## 5. Điểm khác với Kestra đã ghi nhận

Do bảo mật hoặc toàn vẹn dữ liệu:

1. Execution đang chạy hoặc đang pause không xoá được (409 `not_terminated`); Kestra xoá nó và một worker còn sống có thể vẫn tiếp tục ghi.
2. Execution có subflow con hoặc replay trỏ tới nó không xoá được (409 `has_dependents`); Kestra để lại tham chiếu treo.
3. `deleteLogs=false` bị từ chối (log cascade cùng row); `deleteMetrics` không làm gì (egret không có metric).
4. Một loop iteration hoặc retry con không xoá riêng được (409 `not_a_root`).
5. `workflow:manage` cho xoá, label và xoá log; `workflow:read` cho tải. Kestra OSS không kiểm tra gì.

Khác:

6. Sửa label chờ đến khi execution idle (gồm cả các run afterExecution); Kestra chỉ kiểm tra `isTerminated`.
7. Tải về bị giới hạn 100 000 dòng kèm một dòng báo cắt; Kestra stream toàn bộ.
8. Kiểm tra label còn giới hạn số lượng và độ dài (quy tắc backfill của egret).

## 6. Ngoài phạm vi

Xoá log theo flow (`LC:204`), bulk theo ids hoặc query (7f), bộ lọc label trên danh sách execution, metric.
