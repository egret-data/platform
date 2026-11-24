# Workflow gói 8b — concurrency behaviors + unqueue/force-run (thiết kế)

**Trạng thái:** thiết kế (user duyệt auto-implement 2026-11-24; scope đã chốt). **Gói:** #8 concurrency, sub-package **8b** (sau 8a lõi). Gate replay/restart/subflow-child = follow-up RIÊNG (8c), KHÔNG thuộc 8b.
**Branch:** `feat/workflow-concurrency-behaviors` off main (sau khi 8a merge).
**Migration:** không (state CANCELLED/FAILED đã có; không thêm state).

## Mục tiêu
Hoàn thiện flow concurrency của Kestra trên nền 8a: enforce các behavior non-QUEUE (`CANCEL`/`FAIL`), và thêm op quản lý hàng đợi `unqueue` + `force-run` (single + by-ids/by-query), cùng FE actions + hiện flow concurrency.limit.

## Phạm vi (8b)
1. **Enforce CANCEL / FAIL ở admission gate.** 8a `concurrency_admit` trả `Run|Queue` cho `QUEUE`. Mở rộng theo behavior khi vượt limit:
   - `QUEUE` → QUEUED (8a, giữ nguyên).
   - `CANCEL` → execution mới tạo **CANCELLED** (terminal ngay; history `["CREATED","CANCELLED"]`, `ended_at` set, không task run, KHÔNG chiếm slot).
   - `FAIL` → **FAILED** (tương tự).
   **Bỏ warning `NOT_YET_ENFORCED`** cho CANCEL/FAIL (giờ đã enforce); cập nhật pin test + property-treatment.json.
2. **unqueue** — `POST .../executions/{id}/unqueue?state=` : chỉ execution **QUEUED** (409 `not_queued` nếu không); chuyển sang state **terminal** (`?state=`, mặc định **CANCELLED**; state non-terminal → 400). Gate `workflow:manage`. + `.../unqueue/by-ids` + `.../unqueue/by-query` (bulk all-or-nothing qua `bulk_apply` của 7f, cap MAX_BULK).
3. **force-run** — `POST .../executions/{id}/force-run` : chỉ **QUEUED** (409 `not_queued` nếu không; egret chỉ có QUEUED là state "giữ"); promote **chạy ngay, BỎ QUA limit** (qua `promote_queued` của 8a, KHÔNG check under-limit). Gate `workflow:manage`. + by-ids + by-query (bulk).
4. **Hiện flow concurrency.limit (FE).** 8a thấy `concurrency` không có trong flow response. Thêm `concurrency` (limit + behavior) vào flow GET DTO (nhỏ, không migration), regen openapi + client, hiện trên flow detail.
5. **FE actions.** Trên execution QUEUED detail: nút **Unqueue** + **Force run**. Trên bulk toolbar: **Unqueue** + **Force run** cho selection QUEUED (by-ids + select-all-filter → by-query), theo mẫu 7f + confirm/surfacing 400.

## Divergences so Kestra
1. unqueue đặt state TERMINAL (mặc định CANCELLED); Kestra unqueue nhận cả RUNNING — egret giữ unqueue=terminal + force-run=promote (2 op rõ ràng).
2. force-run chỉ QUEUED trong egret (Kestra: mọi non-terminated).
3. Sync (200) thay vì 202 + command queue; bulk all-or-nothing + cap (như 7f).
4. Gate replay/restart/subflow-child vẫn hoãn (8c).

## Bảo mật
unqueue/force-run gate `workflow:manage`; bulk 404-not-403; per-object PDP. force-run bỏ qua limit là hành động privileged (manage).

## Quyết định (chốt)
- Scope = CANCEL/FAIL + unqueue/force-run + FE; replay/restart/subflow = 8c ✅ (user).
- unqueue mặc định CANCELLED, chỉ terminal ✅; force-run QUEUED-only, bỏ limit ✅; thêm concurrency vào flow DTO ✅.
