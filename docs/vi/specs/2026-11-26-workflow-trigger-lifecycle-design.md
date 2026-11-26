# Trigger lifecycle parity cho workflow (gói 11d) — thiết kế

**Trạng thái:** đã duyệt (brainstorm 2026-11-26)
**Roadmap:** egret = Kestra OSS engine; gói #11 (triggers) phần 4/4.
**Tiền nhiệm:** 11a trigger.* binding (#915), 11b Schedule parity (#916),
11c Webhook parity (#917). Gói này đóng 3 key trigger còn lại mà
`lift_of → Package(11)`.

## 1. Mục tiêu

Honour 3 lifecycle key cross-cutting mà `models::Trigger` đã parse nhưng
`spec::keys::lift_of` vẫn báo là cảnh báo `not_honoured` trỏ về gói 11:

- `allowConcurrent` — tuần tự hoá các execution của một trigger (không fire
  execution mới khi execution trước của trigger còn chạy), **và** giải phóng
  hold treo khi engine khởi động lại (Kestra "unlock on restart").
- `failOnTriggerError` — phơi lỗi phát sinh khi eval trigger thành một
  execution FAILED, thay vì chỉ log.
- `stopAfter` trên trigger **Flow** — mở rộng hook
  `worker/stop_after.rs::on_terminal` sẵn có (Schedule 11b, Webhook 11c) sang
  loại trigger Flow.

Ngoài phạm vi, theo roadmap và theo đo đạc:

- **Flow-trigger listener conditions / `dependsOn` / MultipleCondition** **đã
  được implement** (W13a window-free, W13b `dependsOn` với bảng
  `workflow.flow_trigger_windows`, modes `ALL`/`ANY`/`AT_LEAST`, `minSatisfied`
  partial, window `fireOnce`, `sweep_expired_windows`). 11d không đụng tới.
  Task 0 xác nhận không còn gap ẩn trước khi tuyên bố đã xong.
- `logToFile`, `logLevel`, `version`, `assets` → gói 13 (giữ warned).
- `workerSelector`, `policyRefs` → Enterprise (giữ warned).

## 2. Kiến trúc

Cả 3 key đều đã là field trên `models::Trigger`
(`fail_on_trigger_error: bool`, `allow_concurrent: bool`, `stop_after`), hiện
chỉ *warned* (`lift_of → Package 11`). Gói này = **honour mỗi key tại seam có
sẵn, gỡ warning, đo chính xác semantics Kestra trước.** Không shape model mới.

| Key | Seam | Thay đổi |
|---|---|---|
| `allowConcurrent` | `scheduler/fire.rs` (đường fire Schedule) | Trước khi fire một tick đến hạn, nếu trigger khai báo `allowConcurrent: false` và đã có execution sống (non-terminal) cho `(flow_id, trigger_id)` này, **giữ** tick (không fire, không advance qua nó) cho tới khi execution đó kết thúc. Suy từ trạng thái execution sống — **không lưu lock**. |
| `failOnTriggerError` | đường lỗi `scheduler/fire.rs` / `flow_trigger/process.rs` | Khi eval/fire trigger ném lỗi và `failOnTriggerError: true`, tạo một execution FAILED mang lỗi, ngoài việc ghi `last_error`. Mặc định `false` giữ hành vi log-vào-`last_error` hiện tại. |
| `stopAfter` (Flow) | `worker/stop_after.rs::on_terminal` | Mở type-gate từ `{Schedule, Webhook}` thêm `Flow`; disable qua bảng `workflow.disabled_triggers` (Flow trigger không có hàng `workflow.triggers`, giống Webhook ở 11c). |

### 2.1 allowConcurrent: derived lock, migration-free (D1)

Kestra lưu hold trong `TriggerState.executionId`/`locked` nên cần một pass
**unlock-on-restart**: khi scheduler khởi động phải xoá các lock mà execution
không còn tồn tại. egret bỏ các cột đó (xem doc `TriggerState` trong
`trigger/mod.rs`) và thay vào đó **suy** hold:

> "Đã có execution non-terminal nào cho `(flow_id, trigger_id)` này chưa?" —
> một query vào `workflow.executions` lọc theo trigger record, dưới cùng cái
> per-flow advisory lock mà đường fire đã nắm.

Hệ quả: **không có lock lưu trữ nào để rò, nên unlock-on-restart là non-issue**
— restart suy lại hold từ trạng thái execution sống, luôn hiện thời. Bỏ được
migration và pass reconcile mà thiết kế stored-lock cần. Khớp phong cách egret
(gói 8a suy "limit = live-non-QUEUED" dưới `pg_advisory_xact_lock`).

**Fallback:** nếu Task 0 đo ra một semantic Kestra không thể suy — ví dụ hold
phải nhớ một `executionId` *cụ thể* trước đó và bỏ qua execution sống khác của
cùng flow — ta fallback sang cột `locked_execution_id` trên `workflow.triggers`
cộng pass unlock-on-restart tường minh. Spec mang đây làm dự phòng; mặc định là
migration-free.

### 2.2 Hold vs skip-and-advance

Hỗ trợ `when` của 11b **skip và advance** (tick không khớp bị tiêu thụ).
`allowConcurrent: false` thì ngược lại: tick phải được **giữ** (không advance)
để nó fire khi lock rảnh — nếu không run đã lịch sẽ mất. Task 0 đo hành vi chính
xác của Kestra: tick bị giữ có fire hồi tố khi lock rảnh không, tương tác với
`recoverMissedSchedules`, và một execution chạy dài trải nhiều tick thì gộp lại
thành một hay xếp hàng. Triển khai ghim theo hành vi đo được, không đoán.

## 3. Task 0 — đo load-bearing (trên pinned Kestra artifact)

Không key nào được honour cho tới khi Task 0 ghim semantics. Đo và ghi:

1. **allowConcurrent**
   - a. **Default** (comment keys.rs:733 ghi `false`; xác nhận — nếu default là
     `false` thì egret over-fire ngay cả khi không khai báo, và default honoured
     đổi, không chỉ trường hợp tường minh).
   - b. Kestra áp cho **trigger type** nào (chỉ Schedule/polling, hay cả Flow và
     Webhook). Flow event-driven và Webhook synchronous có thể inert.
   - c. **Hold semantics**: khi locked, tick đến hạn bị giữ hay bỏ? Khi unlock,
     tick bị giữ có fire không, hay chỉ tick đến hạn kế tiếp? Tương tác với
     `recoverMissedSchedules` và backfill.
   - d. **Lock key** (per-trigger, per-flow, hay per-execution identity) —
     quyết định query suy có đủ không hay cần fallback lưu lock.
2. **failOnTriggerError**
   - a. **Default** và áp cho trigger type nào.
   - b. Khi lỗi với `true`: Kestra tạo execution FAILED, chỉ log, hay cả hai?
     Execution failed mang gì (message lỗi, trigger context)?
   - c. Thế nào là "trigger error" (eval throw, render fail, fire fail) vs
     no-fire bình thường.
3. **stopAfter (Flow)**: xác nhận Kestra disable Flow trigger khi gặp terminal
   state trong `stopAfter` y như disable Schedule, và disable là per-trigger
   (không per-flow).
4. **xác nhận dependsOn**: kiểm nhanh census + fixture rằng máy móc W13a/W13b
   không để lại gap `dependsOn`/condition-type trước khi tuyên bố đã xong.
5. **Census baseline + lift map**: counts `property-treatment.json` hiện tại
   (1224/3/358/117 sau 11c) và tập chính xác các cặp `(trigger-type, key)` có
   warning được gỡ, cộng **lift-refusal grep** (code + message + fixture YAML
   khắp `tests/`) cho mọi key sắp đổi, theo bài học 9a/9b.

## 4. Phạm vi type (D2)

**Chỉ honour mỗi key trên các type Kestra honour** (Task 0, §3.1b/§3.2a). Type
mà Kestra không act lên key giữ cảnh báo `not_honoured` với câu chữ đúng (hoặc
ghi là inert nơi Kestra cũng inert). Ta không implement một key cho type Kestra
bỏ qua nó — như thế lệch Kestra và over-engineer.

Dự kiến (chờ Task 0): `allowConcurrent` và `failOnTriggerError` là khái niệm
scheduler-evaluated → chủ yếu **Schedule**; Flow (event-driven) và Webhook
(synchronous HTTP) có thể giữ warned/inert. `stopAfter` áp cả ba
(Schedule/Webhook đã xong; Flow thêm ở đây).

## 5. Xử lý lỗi

- Hold `allowConcurrent`: nắm dưới per-flow advisory lock sẵn có; thua race hay
  deadlock dùng lại retry discipline của đường fire. Tick bị giữ không bao giờ
  bị âm thầm bỏ — nếu không giữ được thì để nguyên đến hạn (fire pass sau).
- `failOnTriggerError` không bao giờ được tự ném: lỗi khi *tạo* execution failed
  fallback về ghi `last_error` (hành vi hôm nay), nên key chỉ thêm phơi lỗi,
  không bao giờ bỏ log sẵn có.
- `stopAfter` dùng lại cơ chế webhook của 11c nguyên văn (INSERT
  `disabled_triggers`); guard `became_terminal` sẵn có áp dụng.

## 6. Kiểm thử

- Scoped egret-workflow runs **phải** gồm `--test routes --test
  execution_timestamps` và, cho stopAfter, `--test disabled_triggers --test
  stop_after`, cộng các suite trigger (`--test schedule` / test flow-trigger
  process). Theo [[egret-workflow-scoped-tests-miss-routes-and-timestamps]].
- `allowConcurrent`: test fire tick 1, giữ tick 2 khi execution 1 còn sống, rồi
  fire tick bị giữ một lần khi execution 1 kết thúc. Dùng mẫu mutex tuần-tự-tick
  file-local (worker `tick` claim toàn cục — bài học `TICK` mutex 11c).
- `failOnTriggerError`: trigger mà fire ném lỗi → với `false`, chỉ `last_error`;
  với `true`, một execution FAILED mang lỗi.
- `stopAfter` (Flow): Flow trigger fire một execution kết thúc ở state
  `stopAfter` → trigger bị disable trong `disabled_triggers` và không fire nữa.
- Census golden regenerate (`EGRET_WRITE_PROPERTY_TREATMENT=1`); các warning
  được gỡ chuyển warned→honoured, tổng 1702 không đổi.

## 7. Divergences (ghi vào egret-data/docs)

- allowConcurrent dùng hold **derived** (query execution sống) thay vì lock
  `executionId` lưu của Kestra; unlock-on-restart do đó ẩn, không phải pass
  tường minh. Mọi khác biệt hành vi Task 0 phát hiện (held-tick fire hồi tố,
  gộp multi-tick) ghi ở đây.
- Mọi trigger type mà Kestra áp key nhưng egret giữ warned (nếu Task 0 thấy một
  type ta không honour được nếu không thêm máy móc) được liệt kê là divergence
  mang theo với gói sẽ gỡ nó.

## 8. Quyết định

- **D1** allowConcurrent = derived lock, migration-free; stored-lock + pass
  unlock-on-restart tường minh là dự phòng chỉ khi Task 0 chứng minh một
  semantic không suy được.
- **D2** Chỉ honour mỗi key trên các type Kestra honour; nơi khác giữ cảnh báo
  `not_honoured` đúng. Không đoán — Task 0 đo.
- **D3** All-in-one, một PR, subagent-driven (như 11c). Task 0 đo cả ba key
  trước khi build key nào.
- **D4** dependsOn/listener conditions đã xong (W13a/W13b); 11d không đụng; Task
  0 xác nhận không gap ẩn.
- **D5** Lift-refusal discipline: grep code + message + **fixture YAML** khắp
  `tests/` trước khi gỡ warning nào (bài học 9a×2 / 9b×1).
