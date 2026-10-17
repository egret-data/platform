# Follow-up workflow, nhóm B: kill một execution, và task `Exit`

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã thiết kế 2026-10-17, chưa xây. Mọi khẳng định về code của egret đã được kiểm tra trong worktree `egret-kill` tại `6e995c1f` (main, sau #686) và được trích dẫn `path:line` từ `crates/egret-workflow/src/` trừ khi ghi gốc khác. Kestra là checkout tại `data-portal/kestra`; đường dẫn của nó tính từ gốc đó.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 ("kill/restart" của W7 bị bỏ khỏi W7; kill ở mức execution của W8 chưa bao giờ được ship).

**Quyết định cùng user, 2026-10-17:**
- **Kill đi qua `KILLING`, rồi `KILLED`, như Kestra.** `KILLED` chỉ được ghi khi công việc đã dừng. Worker chết giữa lúc kill sẽ được thay bằng worker khác hoàn tất việc kill, nên không container hay pod nào bị bỏ lại đang chạy.
- **Thiết kế dưới đây (§3-§7) đã được duyệt như đã trình bày.**

**Quyết định mặc định (2026-10-17),** theo quy tắc "theo Kestra trừ khi egret có lý do; giữ tính đúng đắn chặt":
- D1. `cascade` mặc định là `true` và kill các child execution (child của subflow và của loop), như `isOnKillCascade` của Kestra.
- D2. Execution đang paused được kill mà không cần worker: không có gì đang chạy, nên các run đang chờ của nó đi thẳng sang `KILLED` (Kestra resume nó vào `KILLING`, `core/…/services/ExecutionService.java:1007-1013`; kết quả như nhau).
- D3. Kill một execution đã terminal là `409 execution_terminated`. Kestra chỉ trả 409 khi `isOnKillCascade` là false (`webserver/…/ExecutionController.java:1757`); egret không có gì để phát ra cho execution terminal, nên từ chối ở cả hai trường hợp.
- D4. Route kill cần `WF_MANAGE`, cùng quyền với resume và replay (`routes.rs:2833`, `:2939`), và ghi một audit record.
- D5. `Exit` với `SUCCESS`, `WARNING`, `FAILED` hoặc `CANCELLED` để các nhánh sibling đang chạy tiếp tục chạy, như Kestra ghi trong tài liệu (`core/…/plugin/core/execution/Exit.java`, *"only `KILLED` stops sibling tasks"*).
- D6. UI có nút Kill trên trang chi tiết execution (W7b), hộp thoại xác nhận với tuỳ chọn cascade, và badge `KILLING`. Không kill theo từng task run, không kill hàng loạt.
- D7. Phát hiện kill bằng cách poll state của run mỗi `workflow.kill_poll_ms` (mặc định 5000). Không chờ lease renewal, vốn chạy mỗi `working_timeout / 3` (`worker/claim.rs:1179-1181`), tức 20 phút với `working_timeout` mặc định 3600 s (`worker/mod.rs:153`).

## 1. Vì sao kill chưa được ship, và điều gì đã đổi

`routes.rs:2176-2236` ghi bảy trở ngại. Đọc lại theo main hiện tại:

| # | Trở ngại (routes.rs) | Trạng thái |
|---|---|---|
| 1 | Không lệnh nào tạo ra `KILLED`; `advance()` không nhận input từ operator | `advance()` đọc state của execution. Kill ghi `KILLING` vào state đó; `advance()` biến nó thành các command `KILLED` (§5). Không thêm argument. |
| 2 | Bên kill không giữ lease; outcome fence của worker vẫn khớp | Fence thêm `AND state = 'RUNNING'` (§4.1), nên run đã chuyển sang `KILLING` từ chối lần ghi của worker. |
| 3 | Không có kênh từ HTTP tới `TaskHandler` của worker | Worker poll state của run (§4.2). |
| 4 | "Đã yêu cầu kill" không biểu diễn được: không có `Killing` trong `executor::RunState` | Đã thêm (§2). |
| 5 | Xoá `claimed_at` mở lại run | Kill không xoá nó trên run đang có lease; nó đổi `state` (§3.2). |
| 6 | Bên kill là một process khác | Tín hiệu nằm trong database và được poll (§4.2). |
| 7 | `egret.sql` không cancel được | Drop future của nó kết thúc statement trên StarRocks 4.1.6 (đã đo, #686, `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §1.2) và trên Trino qua `query_max_run_time`. Kill thực hiện drop đó. |

## 2. State và dữ liệu

- `executor::RunState` thêm `Killing`, **không terminal**, tại `executor/state.rs:447` cạnh `Killed`. Wire name của nó là `KILLING`, mà `models/state.rs` đã có. Bài học của W8 áp dụng: mọi predicate liệt kê một tập con của `RunState::ALL` phải được đọc lại, và variant được thêm vào round-trip test của `RunState::ALL` với `slot()`.
- `workflow.task_runs.state` và `workflow.executions.state` là `text` thuần, không có danh sách giá trị; `workflow.state_is_terminal` (`migrations/0183_workflow_execution_timestamps.sql:52`) chỉ liệt kê các state terminal, và `KILLING` không phải một trong số đó. **Dự kiến không cần migration.** Task đầu tiên của plan xác nhận điều này với mọi constraint và index trên cả hai bảng; nếu có cái cần `KILLING`, migration lấy số trống kế tiếp trên main vào lúc đó (số migration là namespace dùng chung).

## 3. Route kill

### 3.1 Hợp đồng

`POST /api/v1/workspaces/{ws}/workflows/executions/{id}/kill?cascade=true|false` (mặc định `true`).
- `202` kèm execution DTO: lúc này nó là `KILLING`, hoặc `KILLED` nếu không có gì đang chạy.
- `404` nếu execution không thuộc workspace; `409 execution_terminated` nếu nó đã terminal (D3).
- `WF_MANAGE` (D4). Có tài liệu utoipa, được sinh lại vào client frontend bằng `npm run gen:api`.

### 3.2 Một transaction

Khoá dòng execution (`FOR UPDATE`), rồi, với execution đó và, khi có `cascade`, mọi descendant execution đi qua `workflow.executions.parent_execution_id` (`migrations/0135_workflow_loop_sub_executions.sql:21`) và liên kết parent của subflow (plan đặt tên cột sau khi đọc `worker/subflow.rs`):
1. Run leaf không terminal đang **có lease** (`claimed_at IS NOT NULL`, state `RUNNING`) → `KILLING`. `claimed_at` được giữ nguyên.
2. Run leaf không terminal **không có lease** (đang chờ được claim, `RETRYING`, `PAUSED`, `CREATED`) → `KILLED`, kèm `ended_at`.
3. Run flowable không terminal được để cho `advance()` (§5).
4. Execution → `KILLING`. Nếu bước 1 không đổi gì, `advance()` đóng nó ở tick kế tiếp; route cũng chạy một `advance()` trong cùng transaction để kill khi không có gì đang chạy trả về `KILLED` ngay.

Các execution được khoá parent trước, rồi child theo thứ tự id, để tránh deadlock với một lần kill đồng thời của child.

## 4. Worker dừng handler

### 4.1 Fence

- `renew_lease` (`worker/claim.rs:1845-1861`): `WHERE id = $1 AND claimed_at = $2` thêm `AND state = 'RUNNING'`.
- Fence của `apply_outcome_with` (`worker/claim.rs:2031`): `WHERE id = $2 AND claimed_at = $3` thêm `AND state = 'RUNNING'`. Outcome cho một run `KILLING` bị từ chối là `LeaseLost` và không ghi gì.

### 4.2 Phát hiện và cancel

`under_lease_with` (`worker/claim.rs:1769`) có thêm lần wake-up thứ ba bên cạnh renewal và deadline: mỗi `kill_poll_ms`, `SELECT state, claimed_at FROM workflow.task_runs WHERE id = $1`. Nếu dòng là `KILLING` với `claimed_at` của worker này:
1. future của handler bị drop;
2. `handler.cancel(task, grace)` được gọi, cùng lời gọi mà đường timeout dùng (`worker/cancel.rs`, `worker/timeout.rs`), với grace đã cấu hình (`workflow.plugin_host.kill_grace_s` cho plugin task, grace của container cho container);
3. run được ghi `KILLED` (kèm `ended_at`, attempt được đóng) `WHERE id = $1 AND claimed_at = $held AND state = 'KILLING'`, và `advance()` chạy trong cùng transaction, như `apply_outcome` làm.

Handler có `cancel` trả `Err` (`handlers/sql.rs`) vẫn được ghi `KILLED` sau khi drop; lỗi đi vào `tracing`, như đường timeout làm (`worker/cancel.rs`, fact 2).

Renewal thất bại vì state không còn là `RUNNING` không phải mất lease: worker đọc state và đi theo đường kill ở trên thay vì `Leased::Lost` (cái drop handler mà không cancel, có chủ đích, `worker/claim.rs:1715-1721`).

### 4.3 Worker chết giữa lúc kill

- Reaper (`worker/reaper.rs:42-44`) xoá lease hết hạn trên run `KILLING` y như trên run `RUNNING`.
- `claim` (`worker/claim.rs:477-485`) cũng nhận `state = 'KILLING' AND claimed_at IS NULL`. Một run `KILLING` đã được claim không chạy handler của nó: worker dựng handler, gọi `cancel` (container được đặt tên theo run và attempt từ W5a, nên holder mới với tới được container cũ), rồi ghi `KILLED` như §4.2 bước 3.

## 5. `advance()` khi execution đang `KILLING`

`advance(plan, state, now)` (`executor/mod.rs:175`) vẫn thuần. Khi state cuối của execution là `Killing`:
- nó không tạo run nào và không bắt đầu retry nào;
- run flowable chưa terminal trở thành `KILLING`, rồi `KILLED` khi mọi child run đã terminal (Kestra `executor/…/ExecutorService.java:313-335`);
- execution trở thành `KILLED` khi mọi run đã terminal.

## 6. Task `Exit`

`io.kestra.plugin.core.execution.Exit`, property `state` (mặc định `SUCCESS`; một trong `SUCCESS`, `WARNING`, `KILLED`, `FAILED`, `CANCELLED`). Trong Kestra nó là `ExecutionUpdatableTask` do executor chạy (`core/…/plugin/core/execution/Exit.java`). Trong egret nó cũng ở mức executor: đăng ký trong `spec::registry` không có worker handler, và `advance()` xử lý run của nó khi nó được tạo:
- `KILLED`: execution trở thành `KILLING` và §5 tiếp quản.
- state khác: run `Exit`, từng run tổ tiên của nó, và execution được đặt sang state đó (`update` của Kestra: run chưa terminal cuối cùng, rồi các parent của nó, rồi execution). Các nhánh sibling đang chạy tiếp tục chạy (D5).
- `state` là property có thể templating; nó được render với render context của run. Giá trị ngoài năm giá trị trên là run failure nêu tên giá trị đó.

Việc từ chối `Exit` (`executor/mod.rs:4235-4250`, bốn mục `valids/exit*`) được gỡ bỏ.

## 7. Frontend (trang execution của W7b)

- Nút Kill trên trang chi tiết execution, hiện khi execution chưa terminal, sau một hộp thoại xác nhận có checkbox "also kill subflow executions" (bật mặc định). Theo `docs/en/frontend-rules.md`.
- `KILLING` có badge và màu trạng thái như các state khác; danh sách và trang chi tiết refresh như hiện nay.

## 8. Test và nghiệm thu

- **Oracle:** `valids/exit`, `valids/exit-cancelled`, `valids/exit-killed`, `valids/exit-nested` hiện đang baselined. Chúng phải bank theo `oracle-exec/golden.json`; số đếm trước và sau của ratchet được ghi lại. `golden.json` không bao giờ bị sửa cho khớp egret.
- **Unit (thuần):** table test của `advance()` cho execution ở `Killing`: run leaf `Killing` / terminal, một flowable parent với các child lẫn lộn, flowable lồng nhau, một run `Pause`d. Table test của `Exit` cho từng trong năm state, lồng trong `If` và trong `Parallel`.
- **DB test** (database riêng): transaction của route (§3.2 bước 1-4, cascade tới một subflow child và một loop child, 409, 404, quyền); cả hai fence từ chối run `KILLING`; reaper và `claim` nhận run `KILLING`; worker chỉ ghi `KILLED` dưới lease của nó.
- **Live, Docker** (gate theo engine như `container_docker_live` hiện có): một container task chạy `sleep 600` bị kill; run là `KILLED` trong vòng `kill_poll_ms + grace + margin`, và không còn container nào đặt tên theo run và attempt đó. Test live thứ hai kill khi worker đang dừng, rồi khởi động worker, và kiểm tra điều tương tự.
- **Frontend:** vitest cho nút, hộp thoại và badge; kiểm tra live trong trình duyệt trên stack riêng.

## 9. Phạm vi

Trong: §2-§7. Ngoài: kill một task run, kill hàng loạt, restart, đo live kill trên Kubernetes (k8s runner nhận cùng lời gọi `cancel`; test live chạy trên Docker), các task Kestra khác của nhóm C.
