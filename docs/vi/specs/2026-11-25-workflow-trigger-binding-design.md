# Gói 11a — bind `trigger.*` trong task render — Thiết kế

**Trạng thái:** thiết kế đã duyệt, đã formalize trên worktree #11a
**Ngày:** 2026-11-25
**Roadmap:** egret-workflow parity Kestra, gói **#11 (triggers)**, phần **11a**. #11 là một chương trình nhiều gói (decomposition user duyệt): **11a** = bind `trigger.*` (tài liệu này); 11b Schedule parity (`when`/`lateMaximumDelay`/`recoverMissedSchedules`/labels/stopAfter); 11c Webhook parity (synchronous webhooks, webhook `when`, labels, stopAfter); 11d Flow-trigger listener conditions + lifecycle cross-cutting. 11a là keystone, làm trước.

> **Hiệu chỉnh Task 0 (2026-11-25):** egret ĐÃ persist trigger record trong cột `workflow.executions.execution_trigger` sẵn có (migration 0140), shape `{id,type,variables:{...}}`; Schedule + Flow đã populate và **replay copy verbatim**. Nên "cột mới" ở §3.1 là không cần — 11a tái dùng `execution_trigger`. Chỉ thiếu: `build_context` không đọc nó, và route Webhook set `trigger: None`. Plan phản ánh điều này (không migration; chỉ bind + webhook-capture).


## 1. Mục tiêu

Khi một flow được khởi bởi trigger, bind biến của trigger vào render context của **mọi task** trong flow được khởi, để biểu thức `{{ trigger.body }}`, `{{ trigger.date }}`, `{{ trigger.outputs.x }}` render được. Hiện tại không: `build_context` (worker/render.rs) bind `inputs/vars/flow/labels/execution/outputs/tasks/task/taskrun` và **không có root `trigger`**, nên `{{ trigger.* }}` trong task render fail ("Unable to find `trigger`"). Đây là roadmap note 13.

**Chỉ bind — 11a KHÔNG gỡ refusal nào.** Schedule `when:`, webhook `wait`/`returnOutputs`/`responseContentType`/`responseCode`, partial `dependsOn`, `lateMaximumDelay`… vẫn refuse (11b/11c/11d). 11a chỉ surface các biến mà 3 trigger đang hỗ trợ đã tạo ra.

## 2. Bối cảnh đã đo

- egret model 3 trigger có kiểu (`TriggerKind::{Schedule, Webhook, Flow}`); còn lại là `Leaf` mờ.
- Mỗi trigger có kiểu đã **tính** output lúc fire — Webhook build `trigger.variables` (`Output` có `body`/`uri`/`headers`/`parameters`; `trigger/webhook.rs`); Flow build Output (`executionId`/.../`outputs`); Schedule có `schedule_date`. Nhưng chỉ dùng để render `inputs:` CỦA CHÍNH trigger — **không persist trên execution được tạo**, task của flow không thấy.
- `Execution` (`models/execution.rs`) **không có field `trigger`**. Row executions đã có cột `variables` jsonb persist (→ key render `vars`) và cột `trigger_execution_id` — nhưng không mang output của trigger.
- Kestra bind `trigger` từ `Execution.getTrigger().getVariables()` vào mọi task qua `RunVariables` (`RunVariables.java:492-515`), với `trigger._context={id,type}` + map Output per-type. 11a mirror điều này.

## 3. Thiết kế

### 3.1 Persist: cột `executions.trigger` jsonb mới (D1, user duyệt)
Migration thêm `trigger jsonb NULL` vào `workflow.executions`. NULL khi execution không do trigger khởi (manual/API, subflow child). Cột `variables` đã là key `vars`, nên output trigger có cột riêng — tách bạch, mirror `Execution.trigger` của Kestra.

Shape lưu (một object): map biến per-type **cộng** `_context:{id,type}`, đúng shape Kestra:
- **Schedule** → `{date,next,previous}` (`date`=ngày fire=`schedule_date`; `next`/`previous` tính từ cron lúc fire bằng cron machinery sẵn có).
- **Webhook** → `{body?,uri?,headers,parameters}` — các field `Output` webhook của egret đã mang. `body` vắng với GET/empty body. `parts`/`formFields` **bỏ** (multipart đang refuse). `uri` chỉ dưới `fetchType: STORE` (cũng refuse) → thực tế `{body?,headers,parameters}`.
- **Flow** → `{executionId,namespace,flowId,flowRevision,state,startDate,endDate,firstFailedTaskId,lastTaskId,executionLabels,outputs}`.

### 3.2 Capture tại 3 fire path (D2)
Mỗi fire path ghi Output đã tính (+`_context`) vào cột `trigger` lúc INSERT execution: Webhook (route handler), Schedule (scheduler fire), Flow trigger (`flow_trigger::process`). Các site INSERT là `worker/{subflow,replay,spawn}.rs` + fire path; 11a luồn cột qua seam tạo execution mỗi path dùng.

### 3.3 Replay / restart / subflow (D3)
- **Replay/restart** execution do trigger khởi → **copy** cột `trigger` của execution gốc sang execution mới/reset (để `{{ trigger.* }}` vẫn render khi replay — Kestra parity).
- **Subflow child** + **manual/API** → `trigger` NULL.

### 3.4 Bind trong `build_context` (D4)
`build_context` (worker/render.rs ~805) đọc cột `trigger` (đã select cạnh `variables` ở render.rs:220). Khi non-null, chèn key render top-level `trigger`. Khi null (hoặc Leaf không lưu gì) → không chèn — `{{ trigger.* }}` fail như hôm nay cho execution không-trigger (hành vi không đổi).

### 3.5 Leaf / trigger mờ
`ScheduleOnDates`, `http.Trigger`, plugin triggers là `Leaf` mờ — 11a không model output, nên `trigger` NULL cho execution chúng khởi. (Binding của chúng đến ở gói riêng, #18 / phần #11 sau.)

### 3.6 Không tương tác secret
Output trigger của egret không secret-typed (khác Kestra EE), nên không decrypt/mask. 11a không đụng machinery 10a. (Body webhook là dữ liệu caller, không phải giá trị `secret()`.)

## 4. Xử lý lỗi & ca biên
- **Trigger null:** không key `trigger` (hành vi hôm nay) — không lỗi.
- **Schedule `next`/`previous`:** tính từ cron lúc fire; nếu cron không cho (vd ScheduleOnDates, vốn là Leaf) chỉ lưu `date`.
- **Webhook GET / empty body:** không key `body` (Kestra parity).
- **Body webhook lớn:** lưu trong cột jsonb như hiện tại (`fetchType: STORE` vẫn refuse ở 11a).
- **Task đọc sub-field thiếu** (`{{ trigger.outputs.nope }}`): render miss bình thường.

## 5. Kiểm thử
- **Task 0 (đo trước, như 10a/10b):** grep corpus/oracle (`tests/executor.rs`, `oracle-exec/`) xem flow nào đọc `{{ trigger.* }}` trong TASK và đang ở bucket fail/refused. Bind làm nó render — nếu tuple census/oracle dịch, chốt (giữ census ổn định hoặc ghi ratchet) trước khi code. Xác nhận không test nào assert sự VẮNG của bind `trigger`.
- **Integration (mỗi loại trigger):** webhook→`{{ trigger.body }}`; schedule→`{{ trigger.date }}`; flow→`{{ trigger.outputs.x }}`/`{{ trigger.executionId }}`.
- **Replay:** execution do trigger khởi, replay vẫn render `{{ trigger.* }}` (cột copy).
- **Null:** execution khởi thủ công, task đọc `{{ trigger.x }}` fail (không đổi), không có key `trigger`.
- **Unit:** `build_context` chèn `trigger` iff cột non-null; `trigger._context`={id,type}.
- **Migration** apply sạch; row cũ = NULL.
- **Chạy scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps`.**

## 6. Tài liệu
- `platform/workflows/flows/expressions.md` (hoặc doc triggers) — ghi `{{ trigger.* }}` khả dụng per-type (Schedule `date`/`next`/`previous`, Webhook `body`/`headers`/`parameters`, Flow `outputs`/`executionId`/…), và null cho execution không-trigger.

## 7. Ràng buộc toàn cục
- Chat tiếng Việt; code/commit/docs tiếng Anh. Conventional Commits; `Closes #N` chỉ trong body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết bằng dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build Rust nặng/lần, foreground trong subagent. Merge khi CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- Số migration là namespace chung — renumber trên main + PR mở lúc plan.
- Chạy scoped `egret-workflow` kèm `--test routes --test execution_timestamps`.
