# Phase 3c-1.2a — SQL Lab v2 (đa-tab + polish) Design Spec

> **Trạng thái:** Thiết kế đã duyệt · **Ngày:** 2026-06-18 · **Phiên bản:** 0.2
> Bản tham chiếu (Tiếng Việt). Bản chuẩn (English): [`docs/en/specs/2026-06-18-phase-3c-1-2a-sql-lab-v2-design.md`](../../en/specs/2026-06-18-phase-3c-1-2a-sql-lab-v2-design.md).
> Tinh chỉnh SQL Lab từ [Phase 3c-1](2026-06-18-phase-3c-1-sql-lab-design.md) sau manual test. Dựa trên feedback + đọc kỹ frontend SQL Lab của Superset (§11). Cố ý tách 2 increment anh em: **3c-1.2b** (streaming export gated-permission) và một **Explorer Workbench** tương lai (tab persist backend + Home/bookmarks + shell tab đa-artifact) — xem §1 / §12.

---

## 1. Mục tiêu & phạm vi

Biến SQL Lab thành công cụ thật: **nhiều SQL tab** + polish kiểu Superset lộ ra khi test tay — cascading data selectors, autocomplete giàu hơn, SQL formatter, row-limit control, save dialog đẹp. Vá triệt để bug stale-results.

**Cố ý giữ nhẹ (theo rà soát scope):** tab persist bằng **localStorage** (F5 còn nguyên), KHÔNG backend. Một "Explorer Workbench" nặng hơn — tab persist backend (cross-device), Home/bookmarks landing, shell generic chứa tab dataset/chart/dashboard — là **increment tương lai riêng**, thiết kế khi thật sự có tab type thứ 2 để generalize. Dựng giàn giáo đó bây giờ là premature (đoán tab state model trước khi biết chart/dashboard cần gì) và chatty (auto-save mỗi phím gõ, conflict ghi đa-tab).

**Trong phạm vi**
- **Frontend (phần chính):**
  - **Multi-tab SQL** — tab bar (thêm / đóng / đổi tên / chuyển); state mỗi tab `{ sql, dbId, limit }` persist vào **localStorage** (`sqllab:tabs:{workspace}` + tab active); restore khi reload.
  - **DataPanel** (trái) — DB select + **cascading Catalog → Schema → Table selects** → panel **Columns** (name + type, click-chèn); **collapse/expand**.
  - **Toolbar** — Run · Cancel · **Format** (sql-formatter) · **LIMIT** dropdown · Save.
  - **Autocomplete** — SQL keywords + catalogs/schemas/tables/columns đã load.
  - **Save dialog** — shadcn `Dialog` (name + description) thay `window.prompt`; list saved-query mở item vào tab mới + xoá.
  - **Results bind theo `queryId`** — vá bug "query 2 hiện kết quả query 1".
- **Backend (2 thay đổi nhỏ):**
  - **Row limit** mỗi query ở endpoint submit của engine (`SubmitQuery.limit`).
  - Cột `description` cho `saved_queries` (cho save dialog).

**Ngoài phạm vi (hoãn)**
- **Explorer Workbench** = tab persist backend (`workbench_tabs`, auto-save, cross-device restore), **Home/bookmarks** landing, shell generic theo `kind` chứa dataset/chart/dashboard. Increment tương lai khi có ≥2 tab type.
- **Streaming "no-limit" export gated-permission** → Phase 3c-1.2b.
- Multi-statement, query parameters/templating, drag-reorder tab cầu kỳ, schema-tree (đã thay bằng selects).

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Persist tab | **localStorage** (theo workspace), restore khi reload | 90% giá trị, 10% chi phí; không bảng/CRUD/auto-save/conflict; persist backend thuộc Workbench tương lai |
| 2 | Chọn data | **Cascading selects** (Catalog → Schema → Table) + panel Columns; bỏ tree | Superset `DatabaseSelector`/`TableSelector`; yêu cầu user |
| 3 | Row limit | Dropdown LIMIT (10/100/1k/10k/100k); gửi mỗi run; cap ở `engine.max_rows` | Superset `QueryLimitSelect`; "no-limit" export thuộc 3c-1.2b |
| 4 | Formatter | `sql-formatter` client-side, dialect theo engine (`trino`/`mysql`) | Nhẹ nhất; không round-trip backend |
| 5 | Stale results | Fetch results qua react-query key chứa `queryId` | Query mới → key mới → result cũ không render |
| 6 | Save | shadcn `Dialog` (name + description); thêm `saved_queries.description` | Superset `SaveQuery` modal |
| 7 | Route / IA | Giữ `/explore/:ws/sql-lab`; không Home, không đổi tên, không xáo IA | "Explorer Workbench" + Home thuộc increment tương lai |
| 8 | RBAC | Mọi route gate `require_workspace("query:run")`; saved-queries owner-scoped (như cũ) | Tái dùng permission engine |

## 3. Kiến trúc

```text
crates/egret-bi/src/
  saved.rs   # SỬA: + description
  model.rs   # SỬA: + description trên SaveQuery/SavedQueryDto
  routes.rs  # SỬA: plumbing description
crates/egret-engine/src/
  model.rs   # SỬA: SubmitQuery { …, limit: Option<usize> }
  job.rs     # SỬA: clamp limit mỗi query vào Limits.max_rows
migrations/0011_sqllab_v2.sql   # MỚI: saved_queries.description

frontend/src/
  routes/explore/SqlLab.tsx          # VIẾT LẠI: tab bar + host SqlTab active
  components/sqllab/SqlTab.tsx        # MỚI: một SQL tab (DataPanel + editor + toolbar + results)
  components/sqllab/DataPanel.tsx     # MỚI: cascading selects + columns + collapse
  components/sqllab/SaveQueryDialog.tsx # MỚI: shadcn dialog (name + description)
  components/sqllab/SqlEditor.tsx     # SỬA: keywords + completion schema-object
  components/sqllab/ResultsGrid.tsx   # giữ
  hooks/useTabs.ts                    # MỚI: mảng tab lưu localStorage
  api/sqllab.ts                       # SỬA: limit param, description, hook results-by-queryId
```

Không crate mới. Frontend thêm `sql-formatter`. Không route/nav mới.

## 4. Mô hình dữ liệu — `migrations/0011_sqllab_v2.sql`

```sql
ALTER TABLE saved_queries ADD COLUMN description text NOT NULL DEFAULT '';
```

Đó là toàn bộ migration. State tab nằm ở browser (localStorage), không phải Postgres.

## 5. Backend

### 5.1 `saved_queries.description`
DTO `SaveQuery` thêm `#[serde(default)] description: String`; `SavedQueryDto` trả `description`; `SavedQueryRepo::{create,update}` nhận + persist; struct `FromRow` + `COLS` gồm `description`.

### 5.2 Row limit engine
`egret_engine::model::SubmitQuery` thêm `pub limit: Option<usize>` (`#[serde(default)]`). Ở đường submit (`JobManager`/handler submit), cap hiệu lực = `limit.map(|l| l.clamp(1, cfg.max_rows)).unwrap_or(cfg.max_rows)`, dùng làm `Limits.max_rows` của query đó (thay global). `truncated` vẫn báo khi result chạm cap. Không thêm config. Caller cũ (không gửi `limit`) giữ nguyên (`None` → global max).

## 6. Frontend

### 6.1 Tabs (`hooks/useTabs.ts`, localStorage)
```ts
type SqlTabState = { id: string; title: string; sql: string; dbId: string | null; limit: number };
```
`useTabs(ws)` quản lý mảng + active id, persist vào `localStorage["sqllab:tabs:" + ws]` và `localStorage["sqllab:active:" + ws]`. API: `tabs`, `activeId`, `setActive`, `addTab()`, `closeTab(id)` (không dưới một tab), `renameTab(id, title)`, `patchTab(id, partial)` (cập nhật sql/dbId/limit). Load khi mount (hoặc seed một tab `Untitled`). State runtime (`queryId`, page hiện tại) giữ in-memory theo tab ở parent (KHÔNG persist) để chuyển tab vẫn giữ queryId của query đang chạy; results hiện lại từ cache react-query.

### 6.2 Trang SqlLab (viết lại)
**Tab bar** (dùng shadcn `tabs` hoặc bar tự làm): mỗi tab hiện title; `＋` thêm, `✕` đóng, double-click đổi tên. Render **`SqlTab`** active (key theo `id`, runtime `queryId` truyền từ parent để không mất khi chuyển). Edit tab gọi `patchTab`.

### 6.3 SqlTab (`SqlTab.tsx`)
Giữ editor + DataPanel + toolbar + results của một tab. Run flow:
- `run()` → clear `queryId` của tab, `POST /queries { database_id, sql, limit }` → set `queryId` (runtime state ở parent).
- Status: `useQueryStatus(ws, queryId)` (`refetchInterval` khi queued/running).
- **Results bind theo `queryId`:** `useResults(ws, queryId, page)` — react-query key `["sqllab", ws, "results", queryId, page]`. Đổi/chạy lại query là đổi key, nên result cũ không render (Quyết định 5). Thay effect `setResults` thủ công.
- Cancel → `POST /queries/{id}/cancel`. Lỗi → hiện `error`. Banner truncation từ status.

### 6.4 DataPanel (`DataPanel.tsx`) — cascading selects + columns + collapse
- **DB select** (databases đã grant) → đổi thì reset catalog/schema/table.
- **Catalog select** (`useCatalogs`; chỉ Trino — ẩn cho StarRocks, schemas load thẳng).
- **Schema select** (`useSchemas`).
- **Table select** (`useTables`).
- **Columns panel** — chọn table thì `useColumns` list `name : type`; click một column (hoặc table) → chèn tên vào editor tại con trỏ.
- **Collapse/expand** — nút ẩn panel (editor pane mở rộng); trạng thái collapsed ở client state.
- catalogs/schemas/tables/columns đã load feed vào autocomplete hint (bị chặn theo catalog/schema đang chọn).

### 6.5 Toolbar
- **Run** / **Cancel**.
- **Format** — `sql-formatter` `format(sql, { language })`, `language` theo engine (`trino` → `"trino"`, `starrocks` → `"mysql"`); thay nội dung editor.
- **LIMIT** dropdown — `10 / 100 / 1.000 / 10.000 / 100.000`; mặc định `1.000`; theo tab; gửi khi run.
- **Save** — mở `SaveQueryDialog`.

### 6.6 Autocomplete (`SqlEditor.tsx`)
Completion provider Monaco gộp list **SQL keyword/snippet** + **catalogs, schemas, tables, columns** đã load từ DataPanel (kind keyword/struct/field), bị chặn theo catalog/schema đang chọn. Provider dispose khi unmount (đã vá ở 3c-1).

### 6.7 Save dialog + saved/history
- `SaveQueryDialog` (shadcn `Dialog`): **Name** (bắt buộc) + **Description** (textarea) → `POST /sqllab/queries { database_id, name, sql, description }`. Toast khi xong.
- **Saved** list: name (+ tooltip description); click → mở trong **tab mới**; xoá (của mình).
- **History** list: lần chạy gần đây (`GET /queries`); click → nạp sql vào tab hiện tại.

## 7. Xử lý lỗi
Tái dùng `BiApiError` / `EngineApiError`. Lỗi run/format/save phía FE qua `sonner` toast. State tab localStorage best-effort (parse lỗi → fallback một tab mới).

## 8. Config + wiring
- `egret-bi` `SaveQueryRepo`/routes mang `description` (không sub-state mới). `egret-engine` submit nhận `limit`. Không đổi wiring `egret-server`/`egret-bin`.
- Frontend: thêm `sql-formatter` (`npm i sql-formatter`). Không route/nav mới.

## 9. Kiểm thử
- **Backend:** round-trip `saved_queries.description` (DB-gated, mở rộng test saved repo); **limit param** engine (engine-gated: submit `SELECT` generated-series với `limit=5` → `row_count==5`, `truncated`).
- **Frontend (Vitest):** `useTabs` thêm/đóng/đổi-tên/chuyển + round-trip localStorage; run-flow với results bind `queryId` (**chạy query A rồi query B → ra B, không bao giờ A** — đúng bug đã report); DataPanel cascading select load khi đổi; Format biến đổi SQL; autocomplete trả keyword + một table; `SaveQueryDialog` post name + description; "mở saved query trong tab mới".

## 10. Phân rã cho plan
Các task đề xuất (~7): (1) `migrations/0011` + `saved_queries.description` (repo/model/routes + test DB-gated); (2) engine `SubmitQuery.limit` + clamp (engine-gated); (3) frontend `api/sqllab.ts` (limit, description, `useResults`-by-queryId) + hook `useTabs` + tests; (4) `DataPanel` (cascading selects + columns + collapse + feed autocomplete); (5) `SqlEditor` autocomplete (keywords + schema objects); (6) `SqlTab` (run/poll, results-by-queryId, cancel) + tab bar `SqlLab` (thêm/đóng/đổi-tên) + `SaveQueryDialog` + saved/history-mở-trong-tab + toolbar `sql-formatter`; (7) component tests + verify cuối.

## 11. Tham chiếu
- **Apache Superset SQL Lab frontend** (đã đối chiếu): `TabbedSqlEditors` (state editor mỗi tab, thêm/xoá/đổi-tên), `DatabaseSelector`/`TableSelector` cascading selects + metadata cột, `QueryLimitSelect` (luỹ thừa 10, gửi mỗi run), `SqlEditorLeftBar` collapse, `SaveQuery` modal (name + description), autocomplete từ keywords + object schema, results bind theo id query active. **Tiếp thu:** multi-tab, cascading selects, limit-mỗi-run, formatter, results-by-query-id, save modal. **Khác:** tabs ở localStorage (Superset persist `tab_state` server-side — đó là Workbench tương lai, không phải giờ).
- **Pattern hiện có:** `egret-bi`/`egret-engine` (3c-1/3b-1 — `WsContext` + `require_workspace`, `BiApiError`/`EngineApiError`, test DB-/engine-gated), `frontend/src/api/{client,hooks}.ts`, bộ shadcn (`dialog`, `select`, `tabs`, `resizable`), `react-resizable-panels`, Vitest `__tests__`.

## 12. "Explorer Workbench" tương lai (ghi nhận, không build ở đây)
Khi có tab type thứ 2 (sau dataset/chart), generalize tab của SQL Lab thành Explorer Workbench: shell generic theo `kind`, tuỳ chọn persist tab backend (cross-device restore) với auto-save debounce + xử lý conflict, và Home/bookmarks landing để user ghim artifact. Thiết kế lúc đó — với requirement đa-artifact thật — tránh giàn giáo premature dễ phải đập đi mà increment này cố ý bỏ qua.
