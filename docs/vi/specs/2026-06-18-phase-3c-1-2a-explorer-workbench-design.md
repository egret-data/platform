# Phase 3c-1.2a — Explorer Workbench + SQL Lab v2 Design Spec

> **Trạng thái:** Thiết kế đã duyệt · **Ngày:** 2026-06-18 · **Phiên bản:** 0.1
> Bản tham chiếu (Tiếng Việt). Bản chuẩn (English): [`docs/en/specs/2026-06-18-phase-3c-1-2a-explorer-workbench-design.md`](../../en/specs/2026-06-18-phase-3c-1-2a-explorer-workbench-design.md).
> Tinh chỉnh SQL Lab từ [Phase 3c-1](2026-06-18-phase-3c-1-sql-lab-design.md) sau manual test. Dựa trên feedback + đọc kỹ frontend SQL Lab của Apache Superset (xem §12). Increment anh em **3c-1.2b** thêm streaming export gated-permission (hoãn ở đây).

---

## 1. Mục tiêu & phạm vi

Dựng lại SQL Lab thành **Explorer Workbench đa-tab** với tab persist ở backend, + polish UX kiểu Superset (cascading data selectors, autocomplete giàu hơn, SQL formatter, row-limit control, save dialog đẹp). Thêm surface **Home** landing (chỉ shell) để sau này user ghim/bookmark.

Shell Workbench **generic** (mỗi tab có `kind`), nhưng giờ chỉ implement `kind = "sql"`; dataset/chart/dashboard cắm thêm ở 3c-2/3/4.

**Trong phạm vi**
- **Backend:** bảng `workbench_tabs` (theo workspace+user) + CRUD với auto-save debounce; tham số **row limit** mỗi query ở endpoint submit của engine; cột `description` cho `saved_queries`.
- **Frontend:**
  - **Home** (`/explore/:ws`) — chỉ shell/empty-state ("bookmark của bạn sẽ hiện ở đây") + nav.
  - **Workbench** (`/explore/:ws/workbench`) — tab bar (thêm/đóng/đổi tên), state mỗi tab persist xuống backend (debounce), restore khi mở.
  - **SQL tab** — editor + run/poll/results, results key theo `queryId` (vá triệt để bug stale-results).
  - **DataPanel** (trái) — DB select + **cascading Catalog → Schema → Table selects** → panel **Columns** (name + type, click-chèn); **collapse/expand**.
  - **Toolbar** — Run · Cancel · **Format** (sql-formatter) · **LIMIT** dropdown · Save.
  - **Autocomplete** — SQL keywords + catalogs/schemas/tables/columns đã load.
  - **Save dialog** — shadcn `Dialog` (name + description) thay `window.prompt`; list saved-query mở vào tab mới + xoá.

**Ngoài phạm vi (hoãn)**
- **Bookmark/pin CRUD** — Home increment này chỉ shell; lưu/mở item ghim làm increment sau.
- **Streaming export gated-permission / "no-limit"** → Phase 3c-1.2b (permission `query:export` + per-user grant + streaming connector).
- **Tab type dataset/chart/dashboard** → 3c-2/3/4 (shell dựng generic để chứa chúng).
- Tính năng tab ngoài persist+restore (vd "go back", drag-reorder cầu kỳ), multi-statement, templating/parameters, schema-tree (đã thay bằng selects).

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Persist tab | **Backend-persisted** `workbench_tabs` (theo workspace+user) + auto-save debounce + restore | User chọn cross-device; giống Superset `tab_state` |
| 2 | Shell Workbench | Cột `kind` generic; giờ chỉ `sql` | Sau này dataset/chart/dashboard chung một workbench |
| 3 | Chọn data | **Cascading selects** (Catalog → Schema → Table) + panel Columns; bỏ tree | Superset `DatabaseSelector`/`TableSelector`; yêu cầu user |
| 4 | Row limit | Dropdown LIMIT (10/100/1k/10k/100k); gửi mỗi lần run, cap ở `engine.max_rows` | Superset `QueryLimitSelect`; "no-limit" thuộc 3c-1.2b |
| 5 | Formatter | `sql-formatter` client-side, dialect theo engine (`trino`/`mysql`) | Nhẹ nhất; không round-trip backend |
| 6 | Stale results | Fetch results qua react-query key có `queryId` | Query mới → key mới → result cũ không bao giờ hiện |
| 7 | Home/Bookmarks | **Shell** landing giờ; bookmark CRUD hoãn | User muốn surface vào; nội dung sau |
| 8 | RBAC | Mọi route Workbench/SQL gate `require_workspace("query:run")`; tabs + saved-queries owner-scoped | Tái dùng permission của engine |

## 3. Kiến trúc

```text
crates/egret-bi/src/
  workbench.rs   # MỚI: WorkbenchTabRepo (CRUD workbench_tabs)
  saved.rs       # SỬA: + description
  model.rs       # SỬA: DTO WorkbenchTab, + description cho SaveQuery/SavedQueryDto
  routes.rs      # SỬA: route workbench tab; plumbing description
crates/egret-engine/src/
  model.rs       # SỬA: SubmitQuery { …, limit: Option<usize> }
  routes.rs/job.rs  # SỬA: đưa limit mỗi query vào Limits
migrations/0011_workbench.sql   # MỚI: workbench_tabs + saved_queries.description

frontend/src/
  routes/explore/Home.tsx          # MỚI: shell bookmarks
  routes/explore/Workbench.tsx      # MỚI: tab bar + host tab active (thay trang SqlLab)
  components/sqllab/SqlTab.tsx       # MỚI: một SQL tab (editor + run + results)
  components/sqllab/DataPanel.tsx    # MỚI: cascading selects + columns + collapse
  components/sqllab/SaveQueryDialog.tsx  # MỚI: shadcn dialog
  components/sqllab/SqlEditor.tsx    # SỬA: keywords + completion schema-object
  components/sqllab/ResultsGrid.tsx  # giữ
  api/workbench.ts                   # MỚI: hook tab + limit
  api/sqllab.ts                      # SỬA: limit param, description, results-by-queryId
  App.tsx + sidebar nav              # SỬA: route/nav Home + Workbench
```

`egret-bi` giữ deps cũ. Không crate mới. Frontend thêm `sql-formatter`.

## 4. Mô hình dữ liệu — `migrations/0011_workbench.sql`

```sql
CREATE TABLE workbench_tabs (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind         text NOT NULL DEFAULT 'sql' CHECK (kind IN ('sql')),
  title        text NOT NULL DEFAULT 'Untitled',
  state        jsonb NOT NULL DEFAULT '{}',   -- { sql, database_id, limit, catalog?, schema? }
  position     int  NOT NULL DEFAULT 0,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX workbench_tabs_owner_idx ON workbench_tabs (workspace_id, user_id, position);

ALTER TABLE saved_queries ADD COLUMN description text NOT NULL DEFAULT '';
```

Tab active theo dõi phía client (localStorage `sqllab:active:{ws}`); chỉ thứ tự + nội dung được persist. CHECK `kind` nới rộng ở các phase sau.

## 5. Backend — workbench tabs + limit

### 5.1 `WorkbenchTabRepo` (`egret-bi/src/workbench.rs`)
`list(ws, user)` (theo position), `create(ws, user, kind, title, state) -> Tab`, `update(id, title?, state?, position?) -> Tab`, `delete(id)`, `owner(id) -> Option<Uuid>`. Owner-scoped: route xác minh `tab.user_id == caller` (hoặc `platform_admin`).

### 5.2 Routes (`egret-bi`, dưới `/api/ws/{ws}/workbench`, gate `require_workspace("query:run")`)
| Method + path | Mục đích |
|---|---|
| `GET /workbench/tabs` | List tab của người gọi (theo thứ tự). |
| `POST /workbench/tabs` `{kind, title, state}` | Tạo → trả tab (có id). |
| `PUT /workbench/tabs/{id}` `{title?, state?, position?}` | Auto-save (debounce phía client). Chỉ owner. |
| `DELETE /workbench/tabs/{id}` | Đóng tab. Chỉ owner. |

Route saved-query thêm `description` (trong `SaveQuery` + `SavedQueryDto`).

### 5.3 Row limit của engine
`egret_engine::model::SubmitQuery` thêm `pub limit: Option<usize>` (serde `#[serde(default)]`). Trong `JobManager::submit`, cap hiệu lực = `limit.map(|l| l.clamp(1, cfg_max_rows)).unwrap_or(cfg_max_rows)` và dùng làm `Limits.max_rows` cho query đó (thay vì global). `truncated` vẫn báo khi result chạm cap. Không thêm config.

## 6. Frontend

### 6.1 IA / routing
- `/explore/:ws` → **Home** (shell). `HomeRedirect` của Explorer trỏ vào Home của workspace.
- `/explore/:ws/workbench` → **Workbench**. `/explore/:ws/sql-lab` → redirect sang `workbench` (back-compat).
- Sidebar nav: **Home**, **Workbench** (đổi từ "SQL Lab"), rồi placeholder Datasets/Charts/Dashboards cũ.

### 6.2 Home (shell)
Trang đơn giản: heading + card empty-state ("Bookmark của bạn sẽ hiện ở đây") + nút chính "Open Workbench". Không fetch data. Bookmark CRUD hoãn.

### 6.3 Workbench shell (`Workbench.tsx`)
- Load tabs qua `useWorkbenchTabs(ws)`; nếu chưa có thì tạo một SQL tab mặc định.
- **Tab bar**: mỗi tab hiện title; `＋` thêm (`POST`), `✕` đóng (`DELETE`), double-click đổi tên (`PUT title`). Tab active ở client state (+ localStorage).
- Render nội dung tab active theo `kind` (giờ chỉ `SqlTab`).
- **Auto-save:** thay đổi state mỗi tab (sql, dbId, limit) debounce (~800ms) → `PUT /workbench/tabs/{id}`. Hook `useDebouncedSave`.

### 6.4 SQL tab (`SqlTab.tsx`)
State mỗi tab: `{ sql, dbId, limit, queryId }`. Editor + toolbar + DataPanel + results. Run flow:
- `run()` → clear `queryId`, `POST /queries {database_id, sql, limit}` → set `queryId`.
- Status poll bởi `useQueryStatus(ws, queryId)` (`refetchInterval` khi queued/running).
- **Results bind theo queryId:** `useResults(ws, queryId, page)` là react-query có key chứa `queryId` — đổi/chạy lại query là đổi key, nên result cũ không bao giờ render (Quyết định 6). Thay effect `setResults` thủ công.
- Cancel → `POST /queries/{id}/cancel`. Lỗi → hiện `error`. Banner truncation từ status.

### 6.5 DataPanel (`DataPanel.tsx`) — cascading selects + columns + collapse
- **DB select** (databases đã grant) → đổi thì reset catalog/schema/table.
- **Catalog select** (`useCatalogs`; chỉ Trino — StarRocks ẩn select này, schemas load thẳng).
- **Schema select** (`useSchemas`).
- **Table select** (`useTables`).
- **Columns panel** — chọn table thì `useColumns` list `name : type`; click một column (hoặc table) → chèn tên vào editor tại con trỏ.
- **Collapse/expand** — nút ẩn panel (editor pane mở rộng); trạng thái collapsed ở client state.
- Khi schemas/tables/columns load thì feed vào autocomplete hint (bị chặn theo catalog/schema đang chọn).

### 6.6 Toolbar
- **Run** / **Cancel** (disable hợp lý).
- **Format** — `sql-formatter` `format(sql, { language })` với `language` map từ engine (`trino` → `"trino"`, `starrocks` → `"mysql"`); thay nội dung editor.
- **LIMIT** dropdown — `10 / 100 / 1.000 / 10.000 / 100.000`; mặc định `1.000`; lưu theo tab; gửi khi run.
- **Save** — mở `SaveQueryDialog`.

### 6.7 Autocomplete (`SqlEditor.tsx`)
Completion provider của Monaco gộp: list **SQL keyword/snippet** + **catalogs, schemas, tables, columns** đã load từ DataPanel. Item kind keyword/struct/field. Bị chặn theo object của catalog/schema đang chọn (tránh nạp cả DB). Provider dispose khi unmount (đã vá ở 3c-1).

### 6.8 Save dialog + saved/history
- `SaveQueryDialog` (shadcn `Dialog`): **Name** (bắt buộc) + **Description** (textarea) → `POST /sqllab/queries {database_id, name, sql, description}`. Toast khi xong.
- **Saved** list: mỗi item hiện name (+ tooltip description); click → mở trong **tab workbench mới** (tạo tab với sql/db đó); xoá (của mình).
- **History** list: các lần chạy gần đây (`GET /queries`); click → nạp sql vào tab hiện tại.

## 7. Xử lý lỗi
Tái dùng `BiApiError` / `EngineApiError`. Route tab: 401 chưa-auth, 403 không-phải-member-hoặc-không-phải-owner (PUT/DELETE tab người khác), 404 thiếu. Lỗi auto-save không chí mạng (toast cảnh báo nhẹ; client giữ edit local). Lỗi run/format/save phía FE hiện qua `sonner` toast.

## 8. Config + wiring
- **`egret-server`:** route workbench ship trong mount `BiState` sẵn có (không sub-state mới).
- **`egret-bin`:** `BiState` thêm `tabs: WorkbenchTabRepo` (build từ `db`).
- **Frontend:** thêm `sql-formatter` (`npm i sql-formatter`). Route mới trong `App.tsx`; nav mới.

## 9. Kiểm thử
- **Backend:** CRUD `WorkbenchTabRepo` + thứ tự (DB-gated); route tab auth/ownership (HTTP, DB-gated — 401, và PUT của non-owner → 403); round-trip `saved_queries.description`; **limit param** của engine (engine-gated: submit `SELECT` trên generated series với `limit=5` → `row_count==5`, `truncated`).
- **Frontend (Vitest):** tab bar thêm/đổi/đóng/đổi-tên (mock API); run-flow theo tab với results bind `queryId` (chạy query A rồi B → ra B, không bao giờ A — đúng bug đã report); DataPanel cascading select load khi đổi; nút Format biến đổi SQL; autocomplete provider trả keyword + một table; SaveQueryDialog post name+description.

## 10. Phân rã cho plan
Các task đề xuất (~10): (1) `migrations/0011` + `WorkbenchTabRepo` + `saved_queries.description` (DB-gated); (2) route workbench tab + ownership + plumbing `description` + wiring `BiState`/bin (HTTP test); (3) engine `SubmitQuery.limit` + clamp JobManager (engine-gated); (4) frontend `api/workbench.ts` + cập nhật `api/sqllab.ts` (limit, description, results-by-queryId) + test hook; (5) shell `Home` + routing/nav (Home + Workbench, redirect sql-lab); (6) shell `Workbench` + tab bar + auto-save debounce; (7) `SqlTab` (run/poll, results bind queryId, cancel); (8) `DataPanel` cascading select + columns + collapse + feed autocomplete; (9) toolbar Format (`sql-formatter`) + LIMIT + `SaveQueryDialog` + saved/history-mở-trong-tab; (10) component test + verify cuối.

## 11. Tham chiếu
- **Apache Superset SQL Lab frontend** (đã đối chiếu): `TabbedSqlEditors` (state `queryEditor` mỗi tab, `tabHistory`, thêm/xoá/đổi-tên), persist backend `tab_state`; `DatabaseSelector`/`TableSelector` cascading selects + metadata cột; `QueryLimitSelect` (luỹ thừa 10 tới max server, gửi mỗi run); `SqlEditorLeftBar` collapse (`hideLeftBar` + splitter); `SaveQuery` modal (name + description); autocomplete từ `useKeywords` + object schema; results bind theo `latestQueryId` của query active (cách vá stale-results). **Tiếp thu:** tab persist backend, cascading selects, limit-mỗi-run, formatter, results-by-query-id, save modal. **Hoãn:** streaming export (3c-1.2b), nội dung bookmark/pin Home.
- **Pattern hiện có:** `egret-bi`/`egret-engine` (Phase 3c-1/3b-1 — `WsContext` + `require_workspace`, `BiApiError`/`EngineApiError`, test DB-/engine-gated), `frontend/src/api/{client,hooks}.ts`, bộ shadcn (`dialog`, `select`, `tabs`, `resizable`), `react-resizable-panels`, Vitest `__tests__` (Phase 0b).
