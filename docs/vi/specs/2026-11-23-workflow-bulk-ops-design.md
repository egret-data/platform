# Workflow gói 7f — thao tác execution hàng loạt (thiết kế)

**Trạng thái:** thiết kế, chờ duyệt (brainstorm 2026-11-23).
**Gói:** #7 execution operations, sub-package **7f** (gói con cuối, sau 7a #860, 7b #866, 7c #868, 7d #872, 7e #904).
**Branch:** `feat/workflow-bulk-ops` (worktree `egret-7f`, off origin/main `93f76bd4`).
**Migration:** không — bulk tái dùng data path của single-op (đã có migration riêng).

## Mục tiêu

Port các thao tác **hàng loạt** `by-ids` / `by-query` của Kestra vào egret: áp dụng một thao tác lên nhiều execution trong một request, bằng danh sách id tường minh hoặc bằng chính filter mà danh sách execution dùng.

## Phạm vi

**Tám thao tác, mỗi cái có biến thể `by-ids` và `by-query` = 16 endpoint.** Đây đúng là các thao tác egret đã có route single-execution (7a–7e):

| Thao tác | Single-op (đã có) | Quyền |
|-----------|--------------------|------------|
| delete | `DELETE …/{id}` | `workflow:manage` |
| restart | `POST …/{id}/restart` | `workflow:manage` |
| change-status | `POST …/{id}/status` | `workflow:manage` |
| kill | `POST …/{id}/kill` | `workflow:manage` |
| resume | `POST …/{id}/resume` | `workflow:manage` |
| pause | `POST …/{id}/pause` | `workflow:manage` |
| replay | `POST …/{id}/replay` | `workflow:manage` |
| labels | `POST …/{id}/labels` | `workflow:manage` |

**Hoãn:** bulk `unqueue` và `force-run` của Kestra. Chúng tác động execution QUEUED, thuộc subsystem concurrency/hàng đợi (**package #8**, chưa làm); egret chưa có single-op cho chúng. Sẽ bổ sung bulk khi #8 xong.

Endpoint (đường dẫn egret — không có segment `/actions/` của Kestra; tất cả `POST` trừ delete/kill là `DELETE`, khớp Kestra):

```
{DELETE|POST} /api/ws/{ws}/workflows/executions/{op}/by-ids
{DELETE|POST} /api/ws/{ws}/workflows/executions/{op}/by-query
```
với `{op}` ∈ {delete, restart, change-status, kill, resume, pause, replay, labels}. (Riêng `delete` dùng path trần `.../executions/by-ids` và `.../executions/by-query`, khớp `DELETE /by-ids` của Kestra.)

## Kiến trúc

Tất cả handler nằm trong `crates/egret-workflow/src/routes.rs` cạnh route single-op. Mỗi bulk op:

1. Gate `workflow:manage` (mọi op đều là write).
2. Resolve tập id đích:
   - **by-ids:** body là JSON `["uuid", …]`.
   - **by-query:** tái dùng helper `ExecutionFilter` / `execution_filter` (cùng query params như `list_executions`) → resolve ra các id khớp trong workspace. **by-query delegate về path by-ids** sau khi resolve (đúng pattern Kestra).
3. Áp **cap** (`MAX_BULK`, mặc định 1000) lên số id resolve được; vượt cap → 400 `too_many` (chặn khối lượng đồng bộ).
4. **Validate TẤT CẢ id trước (all-or-nothing)** theo precondition của op — tái dùng validation + per-object PDP của từng single-op (tồn tại + sở hữu + precondition state, vd restart cần terminal/paused, kill cần running). Id ở workspace khác hoặc không tồn tại = "not found" (lớp 404, không bao giờ lộ 403). Nếu **bất kỳ** id nào invalid → trả **400 `BulkErrorResponse { message, invalids: [{ id, reason }] }` và KHÔNG áp dụng gì.**
5. Khi tất cả hợp lệ, áp op lên từng cái (đúng code path single-op dùng), một lượt, trả **200 `BulkResponse { count }`**.

**Đồng bộ, không async (divergence):** Kestra emit mỗi action vào `executionCommandQueue` và trả `202 Accepted` kèm async-operation id. egret không có command queue — single-op mutate state inline và worker nhặt việc — nên bulk của egret chạy **đồng bộ inline** và trả **200** kèm count. Cap giữ request bị chặn (mỗi op là mutate state / ghi row nhanh, không phải chạy task blocking — worker vẫn chạy execution thực sự bất đồng bộ).

### Máy móc dùng chung

Một helper nhỏ `bulk_apply` nhận tập id resolve được, một closure validator per-id, và một closure apply per-id; nó chạy protocol validate-all / rồi apply-all / all-or-nothing một lần, nên cả 8 op có ngữ nghĩa y hệt và chỉ cung cấp precondition + action của mình. Validator và action của mỗi op gọi ĐÚNG hàm mà route single-op đang dùng (không viết lại precondition).

## Bảo mật

- Mọi endpoint gate `workflow:manage`.
- by-query scope theo workspace qua `ExecutionFilter` (như `list_executions`); không bao giờ chạm execution của workspace khác.
- Per-id per-object PDP: mỗi execution là securable (EO programme). Caller có `workflow:manage` trên workspace nhưng bị policy per-object từ chối một execution cụ thể → id đó fail dạng 404-class invalid (không lộ 403), và dưới all-or-nothing → fail cả batch. Khớp quy tắc 404-not-403 của single-op.
- Bulk delete/kill phá hoại ngang N single-op; cap + validate all-or-nothing là lan can.

## Frontend

Trên danh sách execution (`ExecutionsList`):
- **Checkbox chọn dòng** + **thanh bulk-action** hiện khi chọn ≥1 dòng: Restart, Kill, Delete, Change status, Replay, Resume, Pause, Edit labels (mỗi nút chỉ bật khi lựa chọn đủ điều kiện, như nút single-op).
- Nút **"Chọn tất cả khớp filter hiện tại"** → biến thể `by-query` (để tác động mọi execution khớp filter, không chỉ trang hiện tại).
- Dialog xác nhận cho các op phá hoại (delete/kill), hiện số lượng. Khi 400 `BulkErrorResponse`, hiện id nào bị từ chối và vì sao.

## Kiểm thử

`crates/egret-workflow/tests/executions_routes.rs` (không migration):
- Cho một subset đại diện trên các loại op (ít nhất delete, restart, kill, labels, change-status): by-ids áp tất cả khi hợp lệ (200, count); một id invalid (không tồn tại / sai state / workspace khác) → 400 `BulkErrorResponse` liệt kê nó và **không áp dụng gì** (assert các cái hợp lệ KHÔNG bị mutate); by-query resolve filter và áp đúng tập khớp; cap được enforce (vượt `MAX_BULK` → 400); caller chỉ `workflow:read` → 403; danh sách id rỗng / không khớp → 200 count 0 (hoặc no-op rõ ràng).
- Một test chứng minh tính nguyên tử all-or-nothing: batch 3 cái mà #2 invalid để #1 và #3 nguyên vẹn.

## Divergences so với Kestra (ghi lại)

1. Đường dẫn bỏ `/actions/`.
2. **Đồng bộ (200 `BulkResponse`), không async (202 + command queue)** — egret không có `executionCommandQueue`.
3. **Cap** (`MAX_BULK`, mặc định 1000) chặn batch đồng bộ; Kestra không có cap tường minh (nó async).
4. Bulk `unqueue` và `force-run` **hoãn** tới package #8 (subsystem hàng đợi).
5. by-query resolve ra ids rồi chạy by-ids (như Kestra), all-or-nothing (như Kestra).

## Quyết định mở (đã chọn mặc định; xác nhận khi review)

- **Q1 phạm vi:** 8 op egret đã có single-op; hoãn unqueue/force-run. ✅ (user)
- **Q2 ngữ nghĩa lỗi:** all-or-nothing (Kestra parity). ✅ (user)
- **Q3 cap:** `MAX_BULK = 1000` cho độ dài by-ids và số match by-query. *(mặc định — xác nhận con số)*
- **Q4 shape response:** 200 `BulkResponse { count }`; 400 `BulkErrorResponse { message, invalids: [{id, reason}] }` (tên của Kestra). *(mặc định)*
- **Q5 phạm vi FE:** checkbox chọn + bulk toolbar + chọn-tất-cả-khớp-filter + xác nhận phá hoại. *(mặc định — xác nhận)*
