# Pause thành flowable đầy đủ (gói 9a)

**Trạng thái:** thiết kế
**Ngày:** 2026-11-24
**Phụ thuộc:** 8c (concurrency gate), 7c (Pause onResume + pause/resume thủ công, #868), #2a/#2b (finally/afterExecution).
**Roadmap:** gói #9 tách thành **9a (bản này: Pause flowable đầy đủ)** và **9b (AllowFailure block retry)** — user chốt 2026-11-24.

## 1. Mục tiêu

Biến `io.kestra.plugin.core.flow.Pause` thành flowable đầy đủ như Kestra. Hiện egret chỉ hỗ trợ pause **trần**: vào `PAUSED`, chờ `pauseDuration:`/`timeout:`/resume thủ công, kết thúc run; cộng `onResume:` inputs (7c). Một Pause khai báo `tasks:`, `errors:`, `finally:`, `onPause:` hay `behavior:` bị **từ chối** khi publish/start với `unsupported_pause_property` (`executor::plan::pause_gap`). 9a mở cả năm.

## 2. Semantics Kestra (đo từ Pause.java vendored)

`Pause extends Task implements FlowableTask<Output>`:

- **`tasks:`** — thân flowable. Chạy **sau khi pause được resume** (như thân `Sequential` bị chặn sau pause). Terminal của Pause run = aggregate của `tasks:`.
- **`errors:`** / **`finally:`** — handler lỗi/finally chuẩn (sequential driver đã xử lý cả hai).
- **`onPause:`** — một **task đơn** chạy **khi pause bắt đầu**.
- **`behavior:`** (mặc định `RESUME`) — điều gì xảy ra khi pause **chạm deadline `pauseDuration:`/`timeout:`** (KHÔNG phải resume thủ công; resume thủ công trước deadline luôn success và bỏ qua behavior). Enum → state:
  - `RESUME` → chạy tiếp (chạy thân `tasks:`);
  - `WARN` → Pause run kết thúc `WARNING`, execution tiếp tục;
  - `CANCEL` → execution `CANCELLED`;
  - `FAIL` → Pause run kết thúc `FAILED`.
- **`onResume:`** — inputs thu khi resume (ĐÃ xong 7c; output feed vào thân `tasks:`).
- Bound: `pauseDuration:` (field riêng) thắng `timeout:` (kế thừa). Pause không bound chờ resume thủ công mãi.

## 3. Quyết định

**D1 — Tách (user 2026-11-24).** 9a = Pause flowable đầy đủ (tasks/errors/finally/onPause/behavior). 9b = AllowFailure block retry (gói riêng).

**D2 — Giá trị `behavior:` không hợp lệ (user 2026-11-24).** Một behavior **literal** ngoài `{RESUME, WARN, CANCEL, FAIL}` bị **từ chối ở validate** (`unsupported_pause_behavior`) khi publish/start — biết được lúc plan. Một **expression** render ra giá trị ngoài enum lúc runtime → **FAIL task Pause** (không treo). Khác Kestra (để execution `RUNNING` mãi) — footgun treo tài nguyên worker mà 9a không tái tạo. Ghi divergence trong spec.

**D3 — behavior chỉ áp lúc deadline.** Resume thủ công (route resume 7c) trước deadline luôn success và chạy thân, bỏ qua `behavior`. `behavior` chỉ map terminal trên đường hết hạn `pauseDuration:`/`timeout:`.

**D4 — `tasks:` chạy sau resume.** Khi RESUME (thủ công, hoặc deadline-với-RESUME), `tasks:` của Pause chạy như thân sequential với Pause run là parent, kèm `errors:`/`finally:` qua sequential driver sẵn có. Terminal Pause run = `aggregate(tasks)`.

**D5 — `onPause:` chạy một lần, lúc vào pause**, trước khi run thành `PAUSED` (Kestra `resolveTasks(List.of(onPause))`). Một task đơn.

**D6 — `finally:` (từ 7c Q5).** Pause `finally:` thuộc 9a (sequential driver đã chạy `finally:` của một list).

**D7 — Không migration.** Plan node Pause thêm field nhưng schema row/task_run không đổi (tasks/errors/finally là cấu trúc plan; state PAUSED và machinery resume đã có).

**D8 — Execution đã lưu (kiểu D4, plan_stored).** Một Pause lưu trước 9a (trần, hoặc một Pause pre-#2a đã rớt `finally:`) re-plan theo luật lúc nó start: `pause_plan_gap`/`plan_stored` phải giữ Pause trần cũ là Pause trần. Chỉ start/publish MỚI mới được flowable đầy đủ. (Soi gương cách tách `pause_gap` vs `pause_plan_gap` đã có cho `finally:`.)

## 4. Kiến trúc / công việc nằm đâu

### 4.1 Plan (`executor/plan.rs`)
- `TaskKind::Pause` hiện chỉ mang `{ duration }`. Mở rộng mang thân: `tasks`, `errors`, `finally`, `on_pause: Option<TaskIx>`, `behavior: Option<String>` (literal hoặc expression; None = RESUME), cạnh bound hiện có.
- `TaskKind::children()` phải trả `tasks` của Pause (+ errors/finally/on_pause như các flowable khác) để plan đệ quy. Hiện trả `Vec::new()`.
- Planner dựng các child list (soi gương Sequential/AllowFailure ~plan.rs:1454-1468).
- `pause_gap`/`pause_plan_gap`: ngừng từ chối tasks/errors/behavior/onPause/finally. GIỮ từ chối một **behavior literal không hợp lệ** (D2). Giữ tách stored-vs-new để `plan_stored` không retro-flowable Pause trần cũ (D8).

### 4.2 Validate (`spec/validate.rs`)
- Gỡ từ chối `unsupported_pause_property` cho tasks/errors/finally/behavior/onPause.
- Thêm `unsupported_pause_behavior` (D2) cho behavior literal ngoài enum.
- Đệ quy validate vào các child list của Pause.
- Giữ check `onResume` (7c) + từ chối Pause trong afterExecution (validate.rs:861).

### 4.3 Driver (`executor/flowable/pause.rs` + `flowable/sequential.rs`)
- **onPause (D5):** khi Pause run lần đầu RUNNING và sắp pause, nếu có `on_pause` thì chạy một lần trước khi vào PAUSED. Tôn trọng luật một-lần-mỗi-batch (Parallel hai pause vào cùng một batch — dùng lại guard sẵn có).
- **Pause rồi thân (D4):** Pause run vào PAUSED (như nay). Khi resume (`release_now` thủ công, hoặc deadline với behavior RESUME), thay vì chỉ kết thúc run `SUCCESS`, giờ DRIVE thân `tasks:` qua sequential machinery (Pause run là parent), kèm errors/finally. Terminal Pause run = aggregate(tasks).
- **behavior lúc deadline (D2/D3):** `deadline()` hiện trả `Resume::Terminal{SUCCESS}` cho pauseDuration và state map-theo-allow-flag cho timeout. Mở rộng để đường deadline áp `behavior`: RESUME→chạy thân; WARN→WARNING (không thân); CANCEL→huỷ execution; FAIL→FAILED. Expression behavior render bởi caller (như pauseDuration); render ra ngoài enum → FAIL (D2).
- Dùng lại `sequential::drive(children, errors, finally, parent=pause_run)` cho thân — KHÔNG lặp lại sequential walk.

### 4.4 Replay / restart (`executor/replay.rs`, `executor/restart.rs`)
- replay.rs đã đặc biệt-hoá Pause (replay.rs:347). Cập nhật các arm Pause cho shape flowable mới để replay/restart một execution có Pause-có-thân mở lại đúng (thân chạy lại/resume theo luật replay, như thân Sequential).

### 4.5 Outputs
- `Output` của Pause mang `onResume` (7c). Xác nhận thân đọc được `{{ outputs.<pause>.onResume.* }}` như Kestra.

## 5. Ngoài phạm vi (9a)
- AllowFailure block retry → 9b.
- Thay đổi API pause/resume thủ công (7c đã ship; 9a dùng lại).
- Bất kỳ thứ gì namespace/tenant.

## 6. Kiểm thử (phác, nở ra trong plan)
- Pause có `tasks:` → PAUSED, resume thủ công → thân chạy, terminal Pause = aggregate(thân).
- `onPause:` chạy lúc vào pause (một lần; Parallel hai pause mỗi cái chạy onPause một lần).
- `behavior: WARN|CANCEL|FAIL` lúc deadline → terminal map; `RESUME` → thân chạy. Literal sai → refuse@validate; expression→sai → Pause FAIL.
- `errors:`/`finally:` trên thân Pause hành xử như Sequential.
- Resume thủ công trước deadline bỏ qua behavior (luôn chạy thân / SUCCESS).
- plan_stored: Pause trần cũ (và Pause pre-#2a rớt finally) re-plan như trước (D8).
- Replay/restart execution có Pause-có-thân.
- Scoped runs PHẢI gồm `--test routes --test execution_timestamps`.

## 7. Rủi ro
- Tích hợp driver (pause → thân qua sequential) là thay đổi sâu nhất (như 8c Task 4). Machinery đã có, nhưng nối chuyển PAUSED→thân qua sequential walk với Pause run là parent cần cẩn thận. Implementer tìm seam ở flowable/pause.rs + cửa PAUSED của sequential::walk; fallback KHÔNG phải ship trace sai — escalate nếu bất khả thi.
- Thứ tự onPause (trước PAUSED) và luật RESTARTED một-lần-mỗi-batch.
