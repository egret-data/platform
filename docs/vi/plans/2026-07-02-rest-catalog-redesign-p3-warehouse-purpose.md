# Redesign REST Catalog — Plan 3: warehouse purpose (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-rest-catalog-redesign-p3-warehouse-purpose.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstream **E**). Plan 3/6. Thực thi qua subagent-driven-development.

**Mục tiêu:** thay chọn warehouse theo tên (`"notebook"` + fallback-first) bằng **`purpose` tường minh** → chọn catalog notebook xác định, admin chỉ định lúc tạo.

## Bối cảnh
- Migration ở `migrations/` (root), embed bởi `egret-metastore`; mới nhất `0047` → tiếp `0048`. **Thêm migration xong phải `touch crates/egret-metastore/src/lib.rs`** (re-embed).
- `workspace_warehouses(id, workspace_id, warehouse_id, name, storage_conn_id, created_at)` UNIQUE(ws,name). `insert_warehouse(ws, wh_id, name, storage_conn_id)` — **~25 caller, KHÔNG đổi signature**. `service/storage_conn.rs` đã có cột `purpose` (mẫu để bắt chước).
- `runtime.rs`: `select_notebook_warehouse` khớp tên + `.unwrap_or(0)` fallback; `build_notebook_runtime` dùng `|w| w.name`.
- `browse.rs`: `CreateWarehouseRequest { name, storage_conn_id }` + handler `create_warehouse`.
- UI: `frontend/src/routes/explore/catalog/WarehousesList.tsx`.

## Task
- **T1 (migration 0048):** `ADD COLUMN purpose text NOT NULL DEFAULT 'shared' CHECK (in shared|notebook)`; backfill `name='notebook'→'notebook'`; partial unique index ≤1 notebook/ws. `touch metastore/src/lib.rs`; `migrate` + verify `\d`.
- **T2 (enum + store):** `WarehousePurpose {Shared,Notebook}` (as_str/parse); thêm field `purpose` vào `WorkspaceWarehouse`+`WarehouseRow`+`From`; `insert_warehouse_with_purpose(...)` (INSERT+RETURNING purpose); `insert_warehouse` **delegate** `Shared` (giữ 25 caller); thêm `purpose` vào SELECT của list/get. Test roundtrip.
- **T3 (chọn theo purpose):** đổi `select_notebook_warehouse` khớp `purpose=="notebook"`, **bỏ fallback-first** → None; `build_notebook_runtime` dùng `|w| w.purpose.as_str()` + lỗi rõ khi None; sửa 3 unit test.
- **T4 (API + guard):** `CreateWarehouseRequest` thêm `purpose: Option<String>`; handler parse + **pre-check ≤1 notebook** (409) trước khi gọi lakekeeper; dùng `insert_warehouse_with_purpose`; thêm `purpose` vào DTO list. Test tạo notebook thứ 2 bị từ chối.
- **T5 (UI):** thêm `purpose` vào type + payload; `<Select>` Shared/Notebook trong dialog "Add warehouse" + cột Purpose trong bảng; typecheck/lint/build.
- **T6 (verify):** fmt + clippy + test backend (`egret-catalog`, `egret-notebook`, `warehouse_api`) + frontend.

## Ghi chú
- Đăng ký 2 catalog trong Trino/StarRocks = ops, ngoài phạm vi (spec ghi).
- Kế tiếp: Plan 4 (C — notebook→JWT, lớn nhất).
