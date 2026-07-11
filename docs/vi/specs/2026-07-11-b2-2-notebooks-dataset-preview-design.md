# B2-2 — Object-scope cho notebooks + dataset data-preview

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b2-2-notebooks-dataset-preview-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** B2-1a (object-scope backend), B2-1b (sharing UI + data path).
>
> Increment B2 thứ ba. Mở rộng object-level sharing cho **notebooks** (document + live kernel) + thêm **dataset data-preview** object-scoped. Áp pattern B2-1a/B2-1b cho type mới.

---

## 1. Mục tiêu & scope

Cho notebook share được mức object như BI content — user/group được grant Viewer/Editor trên notebook (hoặc folder) đọc/sửa + chạy cell — và cho dataset một endpoint data-preview object-scoped (dataset đã object-grantable từ B2-1a nhưng chưa xem được data).

**Trong scope:** migrate notebook per-object handler (`egret-notebook`) + kernel per-notebook handler (`egret-kernel`) sang `require_object`; notebook object grants + object-guest admission cho notebook; mở rộng sharing modal + Share entry cho notebook; `POST /ws/{ws}/datasets/{id}/data` dataset preview object-scoped.
**Ngoài scope:** warehouses (model data-plane `catalog_roles`/`catalog_grants` là lãnh địa B3 — KHÔNG object-scope qua access_grants); saved-queries (cần folder/model uplift — riêng); data-model flip; role hierarchy/deny (B2-3); per-user data-plane delegation (B3).

**Roles:** reuse `ws_viewer` (Viewer → `notebook:read`/`dataset:read`) và `ws_editor` (Editor → `notebook:write`/`dataset:manage`). ws_viewer đã có `notebook:read`; migration 0070 (B2-1a) cho `dataset:read`.

## 2. Notebooks — object-scope document + kernel

### 2.1 Migrate per-object handler
Handler `{id}` trong `egret-notebook/src/routes.rs` gate `require(&ctx, READ|WRITE)` workspace-scope: `get_one`(READ), `update`/`remove`/`trash`/`restore`/`set_folder`/`add_cell`/`update_cell`/`delete_cell`(WRITE), `bookmark_*`(READ). Migrate mỗi cái → `require_object(&*st.pdp, &ctx, READ|WRITE, notebook_obj)`. `list`(READ)+`create`(WRITE) giữ workspace-scope. Theo quyết định role-based B2-1a, bỏ owner-gate (`read_scoped`/`write_scoped` phần owner) ưu tiên `require_object`; nếu chúng làm thêm existence/workspace guard thì giữ phần đó.

### 2.2 Object-scope kernel (chạy cell) — analog "data-gap" của notebook
`egret-kernel/src/routes.rs` `start`/`status`/`stop`/`connect`/`channels` gate `require(&ctx, "notebook:read")` workspace-scope. Shared reader phải chạy/tương tác kernel được. Migrate → `require_object(&*st.pdp, &ctx, "notebook:read", notebook_obj)` (giữ action `notebook:read` — chạy trong kernel là read-level hôm nay). Viewer-shared user mở + chạy cell được.

### 2.3 Resolver `ObjectRef` (không dep egret-bi)
`shares::resolve` ở `egret-bi`; `egret-notebook`/`egret-kernel` không dep nó. Thêm resolver nhỏ mỗi seam: `notebook_object_ref(db, id)` = `SELECT folder_id, workspace_id FROM analytics.notebooks WHERE id=$1` → `ObjectRef{kind:"notebook",id,folder_id,workspace_id}`; `NotFound` nếu vắng; guard `workspace_id==ctx.workspace_id` (→NotFound) như `shares::resolve`. (Dup nhỏ pattern resolve; refactor sau có thể lift generic resolver.)

### 2.4 Object-guest admission cho notebook (then chốt)
`Authorizer::has_object_grant_in_workspace` (B2-1a) hiện join object row tới `folders/dashboards/charts/datasets`. **Thêm nhánh `notebook`** (`object_type='notebook' AND scope_id IN (SELECT id FROM analytics.notebooks WHERE workspace_id=$2)`). Không có → user chỉ được share notebook bị chặn ở `WsContext` trước khi handler chạy (đúng gap B2-1a).

## 3. Notebook sharing — share API + modal

### 3.1 Share gate cần "manage-action" per-kind
Share API B2-1a gate `require_object(&ctx, "{kind}:manage", obj)`. Notebook **không có** `notebook:manage` (chỉ read/write). Thêm map `manage_action(kind)`: BI kind → `"{kind}:manage"`; **`notebook` → `"notebook:write"`** (Editor share được, Viewer không). Áp ở `grant_put`/`grant_delete`/`grant_list`. Mở rộng `object_ref`/`object_table` trong `shares.rs` biết `notebook` (`analytics.notebooks`).

### 3.2 Frontend
`ShareDialog` đã nhận `kind`/`id` tuỳ ý — thêm **"Share…"** entry trên notebook list/detail (mirror action menu chart/dashboard). Inherited-from-folder qua `folder_id` của notebook.

## 4. Dataset data-preview

Thêm `POST /ws/{ws}/datasets/{id}/data` (object-scoped) trong `egret-bi`:
1. `resolve(&st,&ctx,"dataset",id)` → `require_object(&ctx, "dataset:read", obj)`.
2. Load source dataset (physical `catalog.schema.table` HOẶC `sql` virtual) + `database_id`. Build query preview chặn: `SELECT * FROM <source> LIMIT n` (physical: quote qua `schema.rs`/`chartquery`; virtual: wrap `sql` làm subquery + LIMIT). Reuse logic dataset→FROM của `ch_data`/`compile_chart_sql` để quoting/dialect nhất quán.
3. `run_query(database_id, sql, Some(limit), …)` → trả `ChartRunResponse{columns,rows}` (reuse B2-1b).
- **Không client SQL** — chỉ nhận `{id}`, build `SELECT * FROM source của dataset`; shared reader chỉ preview dataset được grant. Chặn limit (100–1000 rows).

## 5. Security

Cùng đảm bảo B2-1a/B2-1b: `require_object` gate trước load/run; notebook/kernel handler authorize trên notebook object; dataset preview chỉ chạy source của dataset (không client SQL). Kernel object-scope → shared reader tới kernel chỉ cho notebook được share (resolver + require_object bind notebook id). Thêm object-guest admission scoped cho notebook TRONG workspace (cùng join B2-1a). Share gate notebook (`notebook:write`) → chỉ Editor (hoặc admin) share; Viewer không. Regen FE client cho dataset-preview.

## 6. Testing

- **Notebook object-scope:** outside `ws_viewer` grant notebook → GET/run + kernel start = 200; non-granted → 403; `ws_editor` grant → edit/add-cell; non-owner ws_editor member sửa notebook người khác (role-based). Non-member chỉ-share-notebook được WsContext admit (regression §2.4). Viewer không share (403 PUT); Editor share được.
- **Kernel:** outside Viewer-shared reaches `start`/`connect` cho notebook được share (200/authz-reached) nhưng 403 notebook không grant.
- **Dataset preview:** outside `ws_viewer` grant dataset → `POST /datasets/{id}/data` trả rows (200/502-no-engine); non-granted → 403; không nhận client SQL.
- Full gate: BE `cargo test --workspace` (pdp parity không đổi), FE `lint/typecheck/test/build`, openapi + client current.

## 7. Câu hỏi mở (cho plan)

- Lift generic `object_ref` resolver vào shared crate (egret-core?) thay vì dup — mặc định: dup nhỏ per-seam giờ, refactor sau.
- Kernel `start`/`connect` đang `notebook:read`: xác nhận Viewer *nên* chạy cell (read-level) hay yêu cầu Editor (`notebook:write`). Mặc định: giữ `notebook:read` (không đổi).
- `read_scoped`/`write_scoped`: xác nhận owner-gate (bỏ, role-based) vs existence/workspace guard (giữ) — inspect lúc plan.
- Limit preview (100–1000) + FE có "Preview data" affordance ở increment này hay chỉ endpoint (mặc định: endpoint + viewer tối thiểu, full UI sau).

---

*Design v1.0 (2026-07-11). Mở rộng object-level sharing cho notebooks (document + kernel) + dataset data-preview object-scoped, reuse pattern B2-1a/B2-1b. Warehouses (data-plane) là B3; saved-queries cần folder uplift.*
