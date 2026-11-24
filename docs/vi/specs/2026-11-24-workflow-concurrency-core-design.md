# Workflow gói 8a — concurrency lõi của flow (thiết kế)

**Trạng thái:** thiết kế (user duyệt trước cho auto-implement 2026-11-24; quyết định đã khoá khi brainstorm). **Gói:** #8 concurrency, sub-package **8a** (lõi); **8b** sau (CANCEL/FAIL + unqueue/force-run + FE actions).
**Branch:** `feat/workflow-concurrency` (worktree `egret-concurrency`, off main `faaf51f15`).
**Migration:** có — state execution `QUEUED` mới (số tiếp theo 0242; renumber khi merge).

## Mục tiêu
Port concurrency cấp-flow của Kestra: flow khai `concurrency: { limit: ≥1, behavior: QUEUE }`; khi flow đã có `limit` execution non-terminal, execution tạo mới bị giữ ở state **`QUEUED`** thay vì chạy, và được promote khi có slot trống. 8a chỉ ship behavior `QUEUE`.

## Phạm vi (8a)
1. **Flow YAML** `concurrency: { limit, behavior }` — parse + validate (`limit ≥ 1`; `behavior` mặc định `QUEUE`; `CANCEL`/`FAIL` **từ chối khi publish** với "supported in a later release").
2. **State `QUEUED`** (migration) — không terminal, không idle, không "đang chạy task". QUEUED KHÔNG tính vào limit; execution live non-terminal non-QUEUED thì tính.
3. **Gate admission race-safe lúc tạo execution** — trong transaction tạo, với flow có limit, lấy `pg_advisory_xact_lock(hashtext(namespace||'/'||flow_key))` theo flow, đếm execution live của flow, quyết RUN (< limit) hay QUEUE (≥ limit). Lock serialize create cùng flow → 2 start đồng thời không cùng vượt limit.
4. **Drain khi slot trống** — khi một execution của flow terminal (trong settle terminalize nó), dưới cùng lock, promote execution QUEUED **cũ nhất** của flow (`ORDER BY created_at, id`) vào chạy qua start path thường. Một slot/một slot trống. + **safety sweep** (định kỳ) drain flow bị under-limit mà còn QUEUED chờ (phòng drain lỡ).
5. **Children** tính theo flow của chúng (subflow child theo limit của subflow flow; retry `CREATE_NEW_EXECUTION` theo flow của nó) — đúng Kestra.
6. **FE** — chip + filter state `QUEUED`; hiện `concurrency.limit` của flow. Không có action quản lý hàng đợi (8b).

**Hoãn sang 8b:** `behavior: CANCEL` (→ CANCELLED), `FAIL` (→ FAILED); `unqueue` và `force-run` (single + bulk, 2 op hoãn từ 7f) + FE actions.

## Kiến trúc
- **Flow model:** `Concurrency { limit: u32, behavior: Behavior { Queue, Cancel, Fail } }` trên `Flow`; publish từ chối Cancel/Fail ở 8a.
- **State:** thêm `QUEUED` vào biểu diễn state + migration; rà `workflow.execution_is_idle`, predicate terminal, pruner, logs settle xử lý QUEUED đúng (QUEUED ≠ terminal, ≠ idle).
- **Gate:** `worker::concurrency::concurrency_admit(tx, flow) -> Run|Queue` wire vào create path (`claim::insert_execution`/`start_flow_execution`); flow không có concurrency bỏ qua gate.
- **Drain:** `worker::concurrency::concurrency_drain(tx, flow)` trong settle terminalize + safety sweep; execution promote chạy qua start path thường (y hệt start mới).

## Divergences so với Kestra
1. 8a chỉ QUEUE; CANCEL/FAIL từ chối publish tới 8b.
2. Enforcement = advisory lock + count (không phải `ExecutionRunning` in-memory của Kestra; egret lấy DB làm nguồn sự thật).
3. unqueue/force-run hoãn 8b.

## Bảo mật / đúng đắn
- Admission + drain dưới per-flow advisory lock trong transaction → không double-admit, không over-promote quá limit.
- QUEUED chịu cùng workspace scoping + per-object PDP.
- Drain oldest-first (không starve); sweep phòng drain lỡ.

## Kiểm thử
parse/validate (limit≥1; QUEUE; reject CANCEL/FAIL publish); admission (limit N, N live → cái N+1 QUEUED; dưới N chạy; flow không-concurrency bỏ gate); **race** (2 create đồng thời ở biên → đúng 1 run 1 queue); drain (terminalize promote QUEUED cũ nhất; oldest-first; không over-promote; promote chạy như start mới); QUEUED không terminal/idle; sweep recover; FE chip+filter+limit.

## Quyết định (khoá 2026-11-24)
- Tách 8a/8b ✅; reject CANCEL/FAIL publish ✅; gate = pg_advisory_xact_lock per-flow ✅; limit đếm live non-terminal non-QUEUED ✅; drain oldest-first + safety sweep ✅.
