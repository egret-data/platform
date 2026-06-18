# Phase 3c-1 — SQL Lab Design Spec

> **Trạng thái:** Thiết kế đã duyệt · **Ngày:** 2026-06-18 · **Phiên bản:** 0.1
> Bản tham chiếu (Tiếng Việt). Bản chuẩn (English): [`docs/en/specs/2026-06-18-phase-3c-1-sql-lab-design.md`](../../en/specs/2026-06-18-phase-3c-1-sql-lab-design.md).
> Lát cắt đầu của [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.5 (`egret-bi`). Dùng query engine từ [Phase 3b-1](2026-06-18-phase-3b-1-query-engine-design.md). Đã đối chiếu Apache Superset (SQL Lab) và Metabase (native editor) — xem §13.

---

## 1. Mục tiêu & phạm vi

Một **SQL Lab** chạy được trong Explorer SPA: chọn database đã grant cho workspace → duyệt schema → viết SQL (Monaco, autocomplete theo schema) → chạy (async qua engine sẵn có, REST polling) → xem kết quả phân trang → export CSV → lưu/tải query có tên → xem lịch sử chạy.

Đây là **3c-1 của `egret-bi`**. Các phần BI còn lại — **dataset (3c-2), chart (3c-3), dashboard (3c-4)** — là spec riêng xây trên cái này. SQL Lab là lát cắt đầu hợp lý: chỉ cần query engine, chưa có gì phụ thuộc vào nó.

**Trong phạm vi**
- Crate backend mới **`egret-bi`**: CRUD saved-query (`saved_queries`) + **schema introspection** (liệt kê catalog/schema/table/column của database đã grant bằng cách chạy `SHOW`/`DESCRIBE` đồng bộ qua connector của engine).
- Một endpoint **CSV export server-side** thêm vào **`egret-engine`** (`GET …/queries/{id}/results.csv`) — stream kết quả JSONL đã lưu thành CSV (bộ nhớ bị chặn). [Quyết định ①]
- **Frontend SQL Lab** (`frontend/src/routes/explore/SqlLab.tsx` + components): Monaco editor autocomplete theo schema, run/cancel qua engine API với REST polling, results grid phân trang, tải CSV, cây schema lazy (cache + refresh tay), tab saved-query + history.
- Run/poll/results/cancel/history giữ trên engine API **sẵn có** (`/api/ws/{ws}/queries…`).

**Ngoài phạm vi (hoãn)**
- Dataset / chart / dashboard → 3c-2/3/4.
- WebSocket trạng thái trực tiếp → Phase 3b-2 (UI dùng REST polling).
- **Schema sync** kiểu Metabase vào bảng metadata riêng — overkill; live-introspect-có-cache là lựa chọn MVP (Superset làm đúng vậy). Chỉ dùng sync khi cần data-modeling/governance.
- Khôi phục phiên editor (`tab_state` của Superset), multi-tab editor, SQL snippets, lọc autocomplete theo per-table permission.
- Virtualize cây schema (react-arborist) — perf follow-up; lazy-load đủ cho MVP.
- Endpoint autocomplete prefix-search riêng — autocomplete v1 feed từ schema đã cache của catalog/schema đang chọn.

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Lát cắt BI đầu | SQL Lab (3c-1); dataset/chart/dashboard sau | Standalone — chỉ cần engine |
| 2 | Hình dạng backend | `egret-bi`: saved query + schema introspection; run/poll/results tái dùng engine | SQL Lab thật mà không nhân đôi engine |
| 3 | Nguồn schema | **Live introspection** qua `SHOW`/`DESCRIBE` của connector, lazy từng level | Superset làm đúng vậy; Iceberg catalog chỉ phủ table Iceberg |
| 4 | Giao trạng thái | **REST polling** (`refetchInterval`) trên endpoint engine sẵn có | Mô hình y hệt Superset; WS hub hoãn 3b-2 |
| 5 | Autocomplete | Theo schema, feed từ schema đã cache của catalog/schema đang chọn | Superset/Metabase feed completion từ metadata schema |
| ① | CSV export | **Server-side** streaming endpoint trong `egret-engine` (JSONL → CSV) | Superset + Metabase đều export server-side; client-side rủi ro browser OOM ở 100k row |
| ② | Cache schema | react-query cache phía FE (`staleTime ≈ 5 phút`) + nút **Refresh schema** | Superset = RTK-Query cache + `forceRefresh` |
| ③ | Phạm vi autocomplete | Bị chặn theo table/column đã cache của catalog/schema đang chọn | Tránh nạp cả DB; endpoint prefix-search để sau khi schema lớn |
| 4b | RBAC | Tái dùng `query:run` gate toàn bộ SQL Lab; sửa/xoá saved-query owner-or-`platform_admin` | Chạy được query thì duyệt schema + lưu được; theo quy tắc ownership của engine |

## 3. Kiến trúc

```text
crates/
├── egret-bi/                      # MỚI
│   └── src/
│       ├── lib.rs
│       ├── model.rs               # DTO SavedQuery, SchemaNode
│       ├── error.rs               # BiError + BiApiError (IntoResponse)
│       ├── saved.rs               # SavedQueryRepo (CRUD saved_queries)
│       ├── schema.rs              # SchemaService: SHOW/DESCRIBE theo engine qua connector
│       └── routes.rs              # BiState + router /api/ws/{ws}/sqllab
└── egret-engine/                  # SỬA: thêm route export CSV
    └── src/routes.rs              # GET /api/ws/{ws}/queries/{id}/results.csv

frontend/src/
├── routes/explore/SqlLab.tsx      # trang (thay placeholder)
├── components/sqllab/             # MỚI: SchemaTree, ResultsGrid, SavedQueries, History, editor wrapper
└── api/sqllab.ts (+ hooks)        # client API + hook react-query
```

**Deps backend:** `egret-bi` → `egret-{core,config,metastore,auth,workspace,database,engine}`. Dùng `egret_engine::connector::{build_connector, ConnSpec}` + `model::Limits` để chạy query introspection đồng bộ, và `egret_database::DatabaseRepo` để resolve + grant-check database. `egret-server` mount router của `egret-bi`; `egret-bin` build `BiState`.

**Deps frontend:** thêm `@monaco-editor/react` (Monaco). `@tanstack/react-query` đã có; bộ shadcn (`table`, `tabs`, `resizable`, `select`, `scroll-area`, `sonner`, `button`, `dialog`) đã có.

## 4. Backend — `egret-bi`

### 4.1 Saved queries (`saved.rs`)
`SavedQueryRepo` trên `saved_queries`: `create(ws, db_id, name, sql, user)`, `list(ws)`, `get(id)`, `update(id, name, sql, db_id)`, `delete(id)`. Sửa/xoá enforce owner-or-`platform_admin` ở tầng route (như ownership query của engine). List trả về mọi saved query trong workspace kèm owner (tài sản dùng chung của workspace).

### 4.2 Schema introspection (`schema.rs`)
`SchemaService` resolve database qua `DatabaseRepo` (có grant check), mở secret, dựng connector (`egret_engine::connector::build_connector(&ConnSpec)`), và chạy meta-query **nhỏ, đồng bộ** với `Limits` chặt (vd `max_rows = 10_000`, `timeout = 15s`) — đây là tương tác, nhanh, nên **bỏ qua `JobManager` async**. Logic theo engine (`SchemaIntrospector`):

| Level | Trino | StarRocks |
|---|---|---|
| catalogs | `SHOW CATALOGS` | *(không có — trả rỗng; FE nhảy sang schemas)* |
| schemas | `SHOW SCHEMAS FROM "{catalog}"` | `SHOW DATABASES` |
| tables | `SHOW TABLES FROM "{catalog}"."{schema}"` | `SHOW TABLES FROM `{schema}` ` |
| columns | `DESCRIBE "{catalog}"."{schema}"."{table}"` → (name, type) | `DESCRIBE `{schema}`.`{table}` ` → (Field, Type) |

Identifier được quote theo engine để chống injection qua tên schema/table. Mỗi call trả về một list phẳng (lazy — một level mỗi request). Frontend cache và ráp cây.

## 5. Mô hình dữ liệu — `migrations/0010_sqllab.sql`

```sql
CREATE TABLE saved_queries (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id  uuid REFERENCES databases(id) ON DELETE SET NULL,
  name         text NOT NULL,
  sql          text NOT NULL,
  created_by   uuid REFERENCES users(id),
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX saved_queries_ws_idx ON saved_queries (workspace_id, updated_at DESC);
```

Không thêm permission RBAC mới — SQL Lab tái dùng `query:run` (seed ở `0009`).

## 6. CSV export (Quyết định ①) — thêm vào `egret-engine`

Một route mới trên engine (nó sở hữu result): `GET /api/ws/{workspace}/queries/{id}/results.csv`, gate bằng cùng `require_workspace("query:run")` + check `owned()` sẵn có của engine. Nó nạp object JSONL `result_ref` của query từ object store (bị chặn bởi `max_rows`), ghi dòng header từ `columns_jsonb`, rồi mỗi dòng JSONL một dòng CSV, trả về `text/csv` với `Content-Disposition: attachment; filename="query-<id>.csv"`. Escape CSV (quote, dấu phẩy, newline) theo RFC 4180. Bộ nhớ bị chặn bởi `max_rows`; stream S3-range thật là tối ưu sau (giống note phân trang results ở 3b-1). Cái này thay export client-side — vốn rủi ro browser OOM ở 100k row (lý do Superset và Metabase đều export server-side).

## 7. Bề mặt REST

**SQL Lab (`egret-bi`, dưới `/api/ws/{workspace}/sqllab`)** — mọi route cần `require_workspace("query:run")` + database đã grant cho workspace (với route `databases/{id}/…`).

| Method + path | Mục đích |
|---|---|
| `POST /sqllab/queries` `{database_id, name, sql}` | Lưu query có tên. |
| `GET /sqllab/queries` | List saved query của workspace (kèm owner). |
| `GET /sqllab/queries/{id}` | Tải một query. |
| `PUT /sqllab/queries/{id}` `{name, sql, database_id}` | Cập nhật (owner hoặc `platform_admin`). |
| `DELETE /sqllab/queries/{id}` | Xoá (owner hoặc `platform_admin`). |
| `GET /sqllab/databases/{id}/catalogs` | Catalog Trino (StarRocks → list rỗng). |
| `GET /sqllab/databases/{id}/schemas?catalog=` | Schemas / databases. |
| `GET /sqllab/databases/{id}/tables?catalog=&schema=` | Tables. |
| `GET /sqllab/databases/{id}/columns?catalog=&schema=&table=` | `[{name, type}]`. |

**Engine (sẵn có + một CSV bổ sung):** `POST /queries`, `GET /queries/{id}`, `GET /queries/{id}/results`, `POST /queries/{id}/cancel`, `GET /queries`, và **mới** `GET /queries/{id}/results.csv`.

## 8. Frontend — SQL Lab

### 8.1 Layout (pane resizable; bộ shadcn đã có)
```
┌──────────────┬───────────────────────────────────────────┐
│ DB picker    │  Monaco SQL editor       [Run][Cancel][Save]│
│ cây schema   ├───────────────────────────────────────────┤
│ (lazy,cache; │  [Results] [History] [Saved]               │
│  Refresh ⟳;  │  results grid (phân trang) · truncated? ·  │
│  click→chèn) │  [Download CSV] · panel lỗi               │
└──────────────┴───────────────────────────────────────────┘
```

### 8.2 State machine khi run (react-query)
`POST /queries` → `{id}` → poll `GET /queries/{id}` với `refetchInterval: 1000` đến khi `status ∈ {finished, failed, cancelled}` → khi `finished`, `GET /queries/{id}/results?page=`. `running` → spinner + Cancel (`POST /queries/{id}/cancel`). `failed` → hiện `error`. Banner truncation khi `truncated`.

### 8.3 Cây schema + cache (Quyết định ②)
Lazy react-query từng level (`catalogs` → `schemas` → `tables` → `columns`), `staleTime ≈ 5 phút` để expand lặp lại không re-hit engine. Nút **Refresh schema** invalidate các query schema của DB (`forceRefresh` của Superset). Click table → chèn tên đầy đủ vào editor tại con trỏ.

### 8.4 Autocomplete (Quyết định 5, ③)
Một Monaco `registerCompletionItemProvider` cho SQL: keyword/snippet built-in + **gợi ý table/column từ schema đã cache của catalog/schema đang chọn** (bị chặn — không cả DB). Column của một table được kéo (và cache) khi table đó được tham chiếu/expand. Endpoint prefix-search để sau.

### 8.5 Results grid + CSV
`table` phân trang read-only (page qua endpoint results của engine); chọn page-size; banner truncation. **Download CSV** là `<a href>` tới endpoint `…/results.csv` của engine (server-side; browser chỉ tải file).

### 8.6 Tab Saved / History
- **Saved:** list `GET /sqllab/queries`; click để tải vào editor; xoá (của mình). Dialog Save (`name`) → `POST /sqllab/queries`.
- **History:** `GET /api/ws/{ws}/queries` (engine); click một dòng để tải lại SQL vào editor.

Một editor duy nhất; multi-tab + khôi phục phiên hoãn.

## 9. Config + wiring

- **`egret-server`:** `AppState.bi: Option<egret_bi::BiState>`; mount router `sqllab` dưới `/api/ws` sau stack session + CSRF. `AppState::without_db()` → `bi: None`.
- **`egret-bin`:** build `BiState { saved: SavedQueryRepo, schema: SchemaService, databases: DatabaseRepo, sessions, workspaces, authz }` từ config + DB. `SchemaService` giữ `Limits` nhỏ (10k row / 15s).
- Route CSV của engine ship trong mount `EngineState` sẵn có (không wiring mới).
- **Frontend:** thêm `@monaco-editor/react`; route đã tồn tại (`SqlLab.tsx` placeholder) dưới `/explore`.

## 10. Xử lý lỗi

`BiError`: `NotFound`, `Forbidden`, `BadRequest`, `Engine(String)` (lỗi introspection từ connector), `Db(sqlx::Error)`. `BiApiError` (`IntoResponse`, kiểu `DbApiError`) map authz/validation sang HTTP; lỗi DB nội bộ → 500. Lỗi schema-introspection (DB sai, engine không tới được) trả `502/400` với message engine (không chứa secret). Route CSV của engine tái dùng `EngineApiError`.

## 11. Bảo mật

- SQL Lab gate bằng `require_workspace("query:run")` (ws_editor/ws_admin) + database đã grant cho workspace. Engine + Ranger vẫn quyết định data thật thấy được lúc query.
- Sửa/xoá saved-query là owner-or-`platform_admin`. Route CSV tái dùng check `owned()` của engine (member không export được result của member khác).
- Schema introspection quote identifier theo engine; secret DB chỉ đi trong opts/header của connector, không bao giờ vào response, log, `saved_queries.sql`, hay CSV.

## 12. Kiểm thử

- **`egret-bi` unit:** serde DTO; parse `SHOW`/`DESCRIBE` → `SchemaNode` theo engine (table-driven, không DB).
- **DB-gated:** CRUD `SavedQueryRepo` + ownership (create/list/get/update/delete; sửa cross-user bị cấm).
- **Engine-gated** (Trino :8090 + StarRocks :9030 thật): `SchemaService` list catalog/schema/table/column cho database thật (tạo table Iceberg qua catalog hoặc dùng system schema); assert một table/column biết trước xuất hiện.
- **HTTP** (oneshot, DB-gated): chưa auth → 401; không phải member → 403/404; happy path CRUD saved-query; route `results.csv` của engine trả `text/csv` + dòng header đúng (DB-gated, dùng một query row đã finished).
- **Frontend (Vitest, theo `frontend/src/**/__tests__`):** state machine khi run (submit → poll → results, mock client); cây schema lazy + invalidate khi Refresh; provider autocomplete trả item table/column từ schema mock; load/delete saved-query; link CSV trỏ đúng URL.

## 13. Tham chiếu

- **Kiến trúc:** [Master design](../architecture/2026-06-17-egret-architecture-design.md) §10.5 (`egret-bi` — SQL Lab), §9.6 (`saved_queries`), §10.2 (engine — tiêu thụ ở đây).
- **Apache Superset SQL Lab** (đã đối chiếu, reference sát nhất — cũng là BI async-job + REST-polling): live schema introspection mỗi request (`/api/v1/database/{id}/{schemas,tables,table_metadata}`) với **cache RTK-Query phía FE + `forceRefresh`** (không sync metadata); autocomplete feed từ cùng data schema; model `saved_query`; **export CSV/streaming server-side** (ngưỡng ~1000 row → endpoint streaming); run-flow = POST execute → **poll** results by key; layout 3-pane (tree schema trái, editor giữa, results), tree virtualized (react-arborist). **Tiếp thu:** live-introspect + cache FE + refresh (②), polling (④), model saved_queries, CSV server-side (①), layout. **Hoãn:** khôi phục phiên `tab_state`, virtualize tree.
- **Metabase native editor** (đã đối chiếu, mô hình tương phản): **sync** schema vào bảng `metabase_table`/`metabase_field` riêng (Quartz, hằng giờ) → autocomplete tức thì từ metadata đã sync (endpoint cap 50, cache 60s); kết quả **stream một request** (`/api/dataset/:format`), không job/poll. **Tiếp thu:** ý tưởng cap autocomplete (③, dạng endpoint prefix-search sau), export server-side. **Cố ý KHÔNG theo:** hạ tầng schema sync (overkill cho MVP — chỉ cần khi data-modeling/governance); mô hình streaming một-request không chuyển sang engine async-job của ta (khác biệt nền tảng — job cancellable + polling của ta mới đúng, Superset xác nhận).
- **Pattern hiện có:** `egret-database`/`egret-engine` (Phase 3a/3b-1 — `Option<_>` sub-state, test DB-/engine-gated, `DbApiError`/`EngineApiError`, `WsContext` + `require_workspace`, API connector), `frontend/src/api/{client,hooks}.ts` + bộ shadcn + Vitest `__tests__` (Phase 0b).

## 14. Phân rã cho plan

Các task đề xuất (~9): (1) scaffold crate `egret-bi` + dep frontend `@monaco-editor/react`; (2) `model` + `BiError`/`BiApiError`; (3) `migrations/0010_sqllab.sql` + `SavedQueryRepo` (DB-gated); (4) `SchemaService` introspection theo engine (engine-gated); (5) route `egret-bi` + RBAC + ownership + HTTP test + wiring (`egret-server`/`egret-bin`); (6) route `results.csv` của `egret-engine` + test; (7) hook API frontend (`sqllab.ts`) + run state machine; (8) trang SQL Lab frontend — editor + run/cancel + results grid + CSV + cây schema (cache + refresh) + autocomplete; (9) tab saved/history frontend + component test + verify cuối.
