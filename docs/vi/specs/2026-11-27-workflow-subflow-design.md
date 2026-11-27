# Workflow Subflow parity (gói #12) — Thiết kế

**Trạng thái:** đã brainstorm + chốt quyết định 2026-11-26 (formalize trên worktree #12 tách từ main).
Đo đối chiếu với source Kestra local `96893b589`.
**Roadmap:** egret = engine Kestra OSS; roadmap 20 gói, #12 (Subflow), sau #11.

## 1. Mục tiêu

egret đã chạy task `io.kestra.plugin.core.flow.Subflow` (spawn execution của flow khác; tôn trọng
namespace/flowId/inputs/wait/transmitFailed/retry/timeout/allowFailure/allowWarning). Gói #12 ban
đầu nhắm **năm key Subflow mà egret hiện bỏ qua** (census: cả năm `ignored`), `lift_of` trong
`spec/keys.rs` map về `Package(12)`:

- **labels** — label trên execution con được spawn (có render).
- **inheritLabels** — khi true, con kế thừa thêm label của execution cha.
- **revision** — chạy một revision cụ thể của subflow (mặc định: mới nhất).
- **scheduleDate** — trì hoãn việc start của con tới một thời điểm. **Tách sang #12b** (xem "Kết quả Task 0").
- **restartBehavior** — `RETRY_FAILED` (restart con hiện có tại chỗ — hành vi của egret từ 7a) so với
  `NEW_EXECUTION` (khi restart cha, spawn con mới).

Kèm theo follow-up roadmap: **"LoopUntil Subflow child matched by position only"** (ghi chú 7a) —
khớp con được restart/lặp với nguồn của nó đúng cách, không chỉ theo vị trí trong list.

Ngoài phạm vi: `logLevel`/`logToFile`/`version`/`assets`/`taskCache` (→ #13),
`workerSelector`/`policyRefs` (Enterprise) — vẫn ignored/warned.

## 2. Đo đạc (Kestra `96893b589`, `core/.../flow/Subflow.java`)

`class Subflow extends Task implements ExecutableTask<Output>, ChildFlowInterface`.
- `revision: Integer` (tùy chọn; mặc định = revision mới nhất).
- `labels: List<Label>`; `inheritLabels: Property<Boolean> = false`.
- `scheduleDate: Property<ZonedDateTime>` (trì hoãn start của con).
- `restartBehavior: RestartBehavior = RETRY_FAILED` (enum: RETRY_FAILED | NEW_EXECUTION).
- `wait = true`, `transmitFailed = true` (đã tôn trọng). `Output{executionId, state, outputs}`.
- Class doc: "Nếu cha restart, các subflow đã start trước đó cũng được restart" — do restartBehavior chi phối.

egret hiện tại: `TaskKind::Subflow(Subflow)` (models/task.rs:97/687); năm key parse vào model nhưng
không có gì đọc (census `ignored`). `visible_labels` (labels.rs) đã tính thứ tự flow-rồi-own label
(dùng cho label trigger ở 11b/11c). Revision flow đã có (replay 7b `source=version:N`).
`workflow.executions.scheduled_date` + due-gate của scheduler đã có (Schedule). Cơ chế restart tại chỗ
đã có (7a/7b `restart::open`, restart con Subflow).

## 3. Kiến trúc

Mỗi key được tôn trọng tại seam spawn/restart của Subflow (`worker/handlers/subflow.rs` + phần build
`NewExecution` của con + `restart`). Không thêm cơ chế trigger/flowable mới.

### 3.1 labels + inheritLabels (nhẹ)
Render `labels` của Subflow (giá trị render theo context task) và đưa vào `NewExecution.labels` của
con. Khi `inheritLabels` render ra true, đặt label của execution cha lên trước (thứ tự của Kestra —
xác nhận ở Task 0). Dùng lại `visible_labels`/luồng label 11c.

### 3.2 revision (dùng lại flow-version của 7b)
Khi `revision` được đặt, resolve source flow của subflow tại revision đó qua cùng cơ chế version mà 7b
dùng (`source=version:N`); mặc định = mới nhất (hành vi hiện tại). Revision không tồn tại → đường
Subflow-not-found/refusal hiện có (Task 0 chốt lỗi chính xác).

### 3.3 scheduleDate (dùng lại scheduled_date + due-gate) — SPLIT sang #12b
Thiết kế ban đầu: tạo execution con với `scheduled_date = scheduleDate` và để due-gate start khi đến
hạn. Task 0 phát hiện không khả thi trong #12 (xem "Kết quả Task 0").

### 3.4 restartBehavior (nặng — dùng lại restart 7a/7b)
- `RETRY_FAILED` (mặc định): hành vi hiện có (7a restart con lỗi tại chỗ).
- `NEW_EXECUTION`: khi restart cha chạy lại task Subflow, spawn execution con MỚI thay vì restart con
  cũ. Dùng lại seam Subflow của `restart::open` (7a/7b). Task 0 chốt ngữ nghĩa NEW_EXECUTION chính
  xác của Kestra (con cũ còn chạy không? bị kill? liên kết gì với cha?).
- Follow-up: sửa "LoopUntil Subflow child matched by position only" để con lặp/restart khớp với nguồn
  theo identity, không theo vị trí list (ghi chú 7a / hàng #12).

### 3.5 Reconcile
Thêm các key được hiện thực vào read-set Subflow (`measure.rs`), bỏ cảnh báo `lift_of` Package(12) của
chúng, sinh lại census, cập nhật README. Kỷ luật lift-refusal trước khi lift.

## 4. Quyết định

- **D1** #12 = các key trong một gói (user duyệt); sau Task 0, `scheduleDate` tách sang #12b.
- **D2 (scheduleDate)** ban đầu dùng lại `executions.scheduled_date` + due-gate; Task 0 bác bỏ → #12b.
- **D3 (revision)** dùng lại flow-version 7b; mặc định = mới nhất.
- **D4 (restartBehavior)** RETRY_FAILED vẫn là mặc định (7a); thêm NEW_EXECUTION qua seam restart
  7a/7b; gồm follow-up LoopUntil-position.
- **D5** migration: dự kiến không cần (dùng lại labels, version, restart); Task 0 xác nhận
  (dự phòng: một cột ở số tiếp theo còn trống).
- **D6** census: các ô chuyển ignored→honoured trong MỘT commit reconcile (cuối); các task hành vi
  không đụng keys.rs/measure.rs/property-treatment.json.

## 5. Task 0 — đo đạc có tính quyết định

1. labels/inheritLabels: cách Kestra ghép label con + thứ tự (own, inherited, system) và context render.
2. revision: cách version của 7b resolve flow tại revision N; lỗi not-found; xác nhận egret lưu lịch sử
   revision dùng được.
3. scheduleDate: seam của egret gate việc start con theo `scheduled_date`; `wait` tương tác thế nào
   với con bị trì hoãn; xử lý scheduleDate quá khứ/không hợp lệ trong Kestra.
4. restartBehavior: ngữ nghĩa NEW_EXECUTION của Kestra (số phận con cũ, liên kết cha↔con, outputs);
   seam Subflow của `restart::open` cần mở rộng; vị trí chính xác của bug LoopUntil-position.
5. Baseline census (sau #11) + read-set và vị trí lift của các key + grep lift-refusal + kết luận migration.

## 6. Kiểm thử

- Các lần chạy scoped gồm bộ test Subflow/executor/restart/routes/execution_timestamps.
- labels: con mang label đã render; inheritLabels gộp label cha.
- revision: subflow ghim revision cũ chạy revision đó; mặc định chạy mới nhất.
- restartBehavior: RETRY_FAILED restart tại chỗ; NEW_EXECUTION spawn con mới khi restart cha; con
  LoopUntil khớp theo identity.
- Golden census sinh lại; bốn ô honoured.

## 7. Divergence (cần ghi tài liệu)

- Mọi chi tiết NEW_EXECUTION hoặc scheduleDate mà mô hình restart/scheduler của egret khác Kestra
  (Task 0 ghi lại khác biệt chính xác).
- **LoopUntil lồng trên Subflow (xấp xỉ):** identity iteration của con (`child_iteration`) dùng
  iteration của vòng lặp bao GẦN NHẤT, khớp `taskRunIteration` đơn của Kestra; iteration của vòng
  ngoài không thuộc identity của con.
- `scheduleDate` không được tôn trọng (gói #12b); vẫn được chấp nhận và cảnh báo.

## Kết quả Task 0 (2026-11-27) — phạm vi được chỉnh

Đo trên Kestra `96893b589` + egret:

- **labels/inheritLabels, revision, restartBehavior**: khả thi, không cần migration — tôn trọng trong #12.
  Cách ghép label (own của con + [inheritLabels ? label execution cha đã lọc : `system.*` của cha] +
  label task + correlationId, khử trùng; label task render trong runContext của cha). revision qua
  `FlowRepo::get_version`. restartBehavior NEW_EXECUTION gate `restart_earlier_child`.
- **LoopUntil child-by-identity** (bug hàng #12): egret khớp con Subflow theo
  `(trigger_execution_id, trigger_run_ix)`; Kestra theo `taskRunId`+value+iteration. egret không bao giờ
  đặt `TaskRun::value` trên run Subflow → có thể cần một cột child-identity (migration 0245) — quyết
  định trong task restartBehavior.
- **scheduleDate → tách sang #12b.** egret KHÔNG có seam để trì hoãn start của execution đã tạo, và
  CHECK `workflow_executions_scheduled_date_iff_schedule` (0156/0244) cấm con Subflow (trigger NULL)
  ghi `scheduled_date`. Hiện thực cần một hệ thống deferred-start mới (mô phỏng retry `ExecutionDelay`)
  + migration — lớn hơn nhiều so với nối một key, nên là gói riêng #12b (user duyệt tách 2026-11-27).
  Trong #12, `scheduleDate` vẫn ignored/warned, trỏ về #12b.

#12 đã chỉnh: Task 1 labels+inheritLabels, Task 2 revision, Task 3 restartBehavior+LoopUntil-identity,
Task 4 reconcile (bốn key). Census 1257 → honoured +4.
