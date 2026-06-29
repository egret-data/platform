# Catalog — tab Tasks + Statistics của warehouse — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-catalog-tasks-statistics-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `feat/catalog-tasks-stats` (off main). **Sub-epic 4** của Catalog-console-parity (warehouse ✅ → namespace ✅ → table detail ✅ → **tasks + statistics** → permissions). Tham chiếu: tab tasks/statistics console lakekeeper.

## Mục tiêu
Thêm 2 tab **read-only** vào warehouse detail — **Statistics** (đếm tables/views theo giờ) + **Tasks** (list GC/maintenance task) — backed bởi lakekeeper management API.

## Hiện trạng
Warehouse detail có tabs Browse·Access·Details (scope {wid} → lakekeeper id qua `lakekeeper_id`, ownership). egret chạy GC task queues CE-6 (tabular_expiration/purge/task_log_cleanup). lakekeeper management: `get_warehouse_statistics → WarehouseStatisticsResponse{stats: Vec<WarehouseStatistics{timestamp, number_of_tables, number_of_views, updated_at}>}` (lazy hourly, KHÔNG có size bytes — chỉ đếm); `list_tasks(warehouse_id) → ListTasksResponse` (GC tasks + status).

## Quyết định
1. **Backend** (browse.rs, dưới {wid}, gate `catalog:read` như browse, resolve ownership):
   - `GET …/{wid}/statistics` → `get_warehouse_statistics` → `{ entries: [{timestamp, number_of_tables, number_of_views, updated_at}] }` (trang đầu, newest first; rỗng → []).
   - `GET …/{wid}/tasks` → `list_tasks(default)` → `[{task_id, task_type, status, entity_name, created_at, scheduled_for, attempt, last_error}]` (map từ Task fields lakekeeper, đọc shape khi implement). Read-only.
2. **FE** (api/catalog.ts + WarehouseDetail.tsx): hooks `useWarehouseStatistics`/`useWarehouseTasks`; thêm tab **Statistics** (latest: # tables · # views · as of timestamp + history nhỏ; empty "No statistics yet — hourly") + **Tasks** (bảng Type·Status·Entity·Created·Attempt·Error + status badge; empty "No tasks.").

## Test
BE (DB+MinIO+lakekeeper): statistics 200 + entries (rỗng hợp lệ cho warehouse mới, hoặc ≥1); tasks 200 + list (thường []); cross-ws {wid} → 404; gate. FE (mock): Statistics render counts + empty; Tasks render rows + badge + empty. Manual seed table + xem live.

## Rủi ro
stats lazy/hourly (mới → empty, không phải lỗi); shape Task lakekeeper (đọc kỹ map); {wid} ownership IDOR; cả 2 read-only low-risk.

## Ngoài phạm vi
control task (run/cancel `control_tasks`); endpoint statistics; size bytes (lakekeeper không có); permissions (sub-epic 5); chart time-series (bảng/số đủ).
