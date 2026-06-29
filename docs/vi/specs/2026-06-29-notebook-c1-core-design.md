# Notebook C1 — Lõi Notebook (thiết kế)

**Trạng thái:** nháp · **Ngày:** 2026-06-29 · **Epic:** Notebooks (mô phỏng sparklabx, engine Daft+Spark-Connect)
> Bản tiếng Anh là bản chuẩn (canonical): `docs/en/specs/2026-06-29-notebook-c1-core-design.md`. Bản này là tham chiếu.

## 1. Bối cảnh & phạm vi

egret thêm notebook kiểu Jupyter **theo từng workspace**, mô phỏng **sparklabx**
nhưng theo stack Rust + mô hình per-workspace của egret. Cả epic được phân rã
thành các sub-spec độc lập, mỗi cái 1 chu trình spec → plan → build (như part A
của catalog):

- **C1 — Lõi notebook** *(spec này)*: lưu trữ + API CRUD + UI editor. **Chưa execute.**
- **F1 — Nền tảng truy cập**: impersonation Trino `X-Trino-User` (đọc governed
  qua Ranger, theo từng end-user) + auto-provision namespace `notebook_{user}` +
  catalog token 1-grant.
- **D1 — Daft kernel runtime**: trait `KernelLauncher` + impl subprocess, registry
  kernel, idle reaper, WS proxy theo Jupyter protocol.
- **E1 — Execution**: nối cell run → kernel; Python (Daft) · SQL (Trino) ·
  Markdown; stream output, render DataFrame, lưu `last_output`.
- **Sau**: picker Spark Connect · StarRocks per-user · launcher docker/k8s ·
  per-user MinIO IAM / vended credentials.

Mô hình truy cập đã khóa (xem memory `egret-notebook-access-model`) là **2 plane**:
data governed **chỉ-đọc** với notebook qua Trino+Ranger (per-user); mỗi user có
vùng đọc/ghi riêng `notebook_{user}` trên REST catalog. **C1 không xây phần đó** —
C1 là bản thân tài liệu notebook: tạo/sửa/lưu notebook + cell với editor Monaco.
Dùng độc lập được (soạn & lưu notebook; chỉ chưa chạy được tới khi có D1/E1).

### Trong phạm vi (C1)
- Lưu trữ Postgres cho notebook + cell, scoped theo workspace.
- API REST CRUD trên session plane (`/api/ws/{workspace}/notebooks…`).
- Mô hình sở hữu: đọc = chủ **hoặc** public; ghi/xóa = chỉ chủ.
- Frontend: trang danh sách notebook (tạo/đổi tên/xóa/mở) + trang editor (cell có
  thứ tự, Monaco cho code, render Markdown, thêm/sắp xếp/xóa cell, autosave source
  cell + metadata notebook).
- Trường `engine` (`daft` mặc định, `spark_connect`) được **lưu & sửa được** nhưng
  chưa dùng tới khi có D1/E1.

### Ngoài phạm vi (C1 — để các sub-spec sau)
- Mọi việc execute, kernel, stream output (D1/E1).
- Namespace `notebook_{user}`, mint catalog token, impersonation Trino (F1).
- Chia sẻ ngoài cờ `is_public` (không có ACL per-user).
- Realtime collaboration, lịch sử version, import/export `.ipynb`.

## 2. Mô hình dữ liệu

Migration mới `migrations/0037_notebooks.sql`. Sau khi thêm, **touch
`crates/egret-metastore/src/lib.rs`** để migrator nhúng lại (memory
`egret-sqlx-migration-staleness`).

Hai bảng (chi tiết DDL ở bản tiếng Anh):
- `notebooks` (id, `workspace_id` FK cascade, `owner_user_id`, name, description,
  `engine` default `'daft'`, `is_public`, timestamps) + index `(workspace_id, owner_user_id)`.
- `notebook_cells` (id, `notebook_id` FK cascade, `cell_type` `code|markdown|sql`,
  source, `cell_order`, + cột execution `execution_count`/`last_output`/
  `last_execution_time_ms` chỉ E1 mới ghi) + index `(notebook_id, cell_order)`.

`engine`/`cell_type` validate ở tầng API (giá trị lạ → 400), không CHECK ở DB, để
thêm engine/cell-type chỉ là sửa code.

## 3. Backend — crate mới `egret-notebook`

Sao theo layout + `Cargo.toml` của `egret-engine`. Module: `model.rs` (types +
DTO), `store.rs` (`NotebookStore`, sqlx tham số hóa, không nội suy chuỗi),
`routes.rs` (`notebook_router() -> Router<NotebookState>` + handler), `error.rs`
(`NotebookApiError`), `lib.rs`. `NotebookState` giữ `store/sessions/workspaces/authz`
+ `FromRef` (như `EngineState`).

### API (session plane)
Dưới `/ws/{workspace}/notebooks`, egret-server mount
`.nest("/api", egret_notebook::notebook_router())` + `csrf_layer` + `.with_state(notebook)`
(như block engine/bi trong `egret-server/src/lib.rs`). Mọi handler dùng
`WsContext(ctx)` + `require_workspace(ctx, PERM)`.

| Method & path | Perm | Hành vi |
|---|---|---|
| `GET    /ws/{ws}/notebooks` | `notebook:read` | List: own + public trong ws. |
| `POST   /ws/{ws}/notebooks` | `notebook:write` | Tạo (owner = caller). |
| `GET    /ws/{ws}/notebooks/{id}` | `notebook:read` | Lấy notebook **kèm cell theo thứ tự**. 404 nếu khác ws; 403 nếu không phải own & không public. |
| `PATCH  /ws/{ws}/notebooks/{id}` | `notebook:write` | Sửa name/description/engine/is_public. Chỉ chủ. |
| `DELETE /ws/{ws}/notebooks/{id}` | `notebook:write` | Xóa (cascade cell). Chỉ chủ. |
| `POST   /ws/{ws}/notebooks/{id}/cells` | `notebook:write` | Thêm cell. Chỉ chủ. |
| `PATCH  /ws/{ws}/notebooks/{id}/cells/{cid}` | `notebook:write` | Sửa source/type cell. Chỉ chủ. |
| `DELETE /ws/{ws}/notebooks/{id}/cells/{cid}` | `notebook:write` | Xóa cell. Chỉ chủ. |
| `PUT    /ws/{ws}/notebooks/{id}/cells/order` | `notebook:write` | Sắp xếp: body là danh sách id cell theo thứ tự. Chỉ chủ. |

### Sở hữu / phân quyền
2 perm RBAC seed trong migration + cấp cho role workspace (như `catalog:read|write`
ở migration 0024): `notebook:read`, `notebook:write`. Gate thô
`require_workspace(ctx, PERM)`; sở hữu per-record qua helper `owned` (như
`egret-engine::routes::owned`) với policy:
- **Đọc** 1 cái: cho phép nếu cùng ws **và** (chủ **hoặc** public). Khác → 404/403.
- **Ghi**: chỉ khi `owner_user_id == caller`. **Không có override platform-admin**
  — notebook là workspace riêng tư, theo đúng chủ ý sparklabx. (Khác `egret-engine`
  vốn cho platform_admin đọc mọi query — notebook **không** mở rộng kiểu đó.)
- List: lọc store `WHERE workspace_id=$1 AND (owner_user_id=$2 OR is_public)`.

Mọi route cell phải resolve notebook cha trước + verify thuộc đúng ws của path +
caller được ghi — bịt IDOR nested-resource (memory `api-nested-resource-ownership-scoping`).

## 4. Frontend
Tận dụng vị trí Explore sẵn có — **route + item sidebar đã tồn tại** (`App.tsx`
`notebooks`; `Sidebar.tsx` nhóm Explore "Notebooks").

- `src/api/notebooks.ts` — client react-query như `api/sqllab.ts` (`qk`,
  `useNotebooks`, `useNotebook`, `useCreate/Update/DeleteNotebook`,
  `useCreate/Update/DeleteCell`, `useReorderCells`).
- `src/routes/explore/Notebooks.tsx` — thay placeholder bằng **trang list**: lưới
  notebook của caller (+ public, có badge), nút New (name + engine), đổi tên, xóa
  (typed-confirm như DatabasesPage), click → mở, empty state.
- `src/routes/explore/NotebookPage.tsx` *(mới)* — **editor**, route `notebooks/:id`
  (thêm vào `App.tsx`). Render cell theo thứ tự: code/SQL → Monaco (đã có dep, theo
  `sqllab/SqlEditor.tsx`), Markdown → textarea + preview render bằng
  `react-markdown` + `remark-gfm` đã có sẵn (xem `components/chart/dashboard/TextCard.tsx`).
  Toolbar mỗi cell (đổi
  type, lên/xuống, xóa); header (đổi tên, chọn engine, toggle public). **Autosave**
  debounce ~600ms source cell; metadata lưu on blur. **Không có nút Run ở C1** (Run
  thêm ở E1; có thể render disabled + title "coming soon").

## 5. Kiểm thử
- **Backend** (`#[sqlx::test]` như egret-engine): create → get-kèm-cell → list (own
  vs public) → update → delete; sở hữu (non-owner đọc private=403, đọc public=ok,
  ghi=403, **platform_admin ghi notebook người khác = 403**); cross-ws=404; CRUD +
  reorder cell; IDOR nested; validate engine/cell_type lạ=400.
- **Frontend** (vitest, như `DatabasesPage.test.tsx`): list + empty; create/rename/
  delete (mock api); editor render cell; autosave gọi `useUpdateCell`; add/reorder/
  delete cell; preview markdown.
- **Cổng CI local trước merge** (memory `egret-run-full-ci-local-before-merge`): full
  `fmt`+`clippy`+`test` workspace + frontend `lint`/`typecheck`/`test`/`build`.
  GitHub Actions hết quota.

## 6. Câu hỏi mở (chốt khi làm plan)
1. Khi tạo: trả cell rỗng hay seed **1 code cell rỗng**? *Đề xuất: seed 1 code cell.*
2. Map role→perm cho `notebook:read|write` — theo các row role của `catalog:read|write`
   ở migration 0024.

## 7. Không-mục-tiêu / rủi ro
- C1 cố ý ship notebook chưa chạy được; đó là mục đích chia lát. Hiển thị trạng thái
  Run disabled để không bị hiểu là lỗi.
- Giữ `egret-notebook` không phụ thuộc engine/kernel/storage — các thứ đó tới ở D1/E1
  dạng crate riêng, giữ ranh giới C1 sạch.
