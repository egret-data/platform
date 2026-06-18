# Phase 3b-1 — Query Engine (thực thi bất đồng bộ, REST polling) Design Spec

> **Trạng thái:** Thiết kế đã duyệt · **Ngày:** 2026-06-18 · **Phiên bản:** 0.1
> Bản tham chiếu (Tiếng Việt). Bản chuẩn (English): [`docs/en/specs/2026-06-18-phase-3b-1-query-engine-design.md`](../../en/specs/2026-06-18-phase-3b-1-query-engine-design.md).
> Hiện thực nửa đầu của [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.2 (`egret-engine`). Dùng database registry từ [Phase 3a](2026-06-18-phase-3a-database-registration-design.md). Đã đối chiếu với Apache Superset (async SQL Lab) và Metabase (query processor) — xem §12.

---

## 1. Mục tiêu & phạm vi

Thực thi một câu SQL trên database đã được grant cho workspace **theo cơ chế bất đồng bộ**, theo dõi job, lưu kết quả, và cho client poll trạng thái + lấy kết quả qua REST. Đây là engine mà SQL Lab UI (Phase 3c), chart, và Data API sau này sẽ gọi.

Đây là **Phase 3b được tách đôi**: **3b-1 (spec này)** = engine core + REST polling; **3b-2** sẽ thêm WebSocket hub (`egret-realtime`) + streaming trạng thái/kết quả trực tiếp, lý tưởng là làm cùng SQL Lab UI tiêu thụ nó.

**Trong phạm vi**
- Một crate mới `egret-engine`: trait `Connector` + connector Trino (REST statement protocol) và StarRocks (`mysql_async`); `QueryStore` (bảng Postgres `queries`) + I/O kết quả ra object store; `JobManager` (mỗi query một tokio task, lifecycle được persist, hỗ trợ cancel); các route REST (submit / status / results phân trang / cancel / history).
- **Model job bất đồng bộ + REST polling.** `POST` trả về job id ngay; query chạy trong background task; client `GET` trạng thái và kết quả. Không có WebSocket ở 3b-1.
- **Lưu kết quả:** kết quả đầy đủ ghi ra object store dạng **JSONL** (mỗi dòng một row) tại `<results_prefix>/<id>.jsonl`; **preview** giới hạn số row + schema cột + thống kê nằm trong `queries` (Postgres).
- **Guard:** row cap cấu hình được (fetch `max_rows + 1`, cắt, đánh cờ), timeout mỗi query (ưu tiên engine-native + tokio timeout lớp ngoài), và cancel theo yêu cầu người dùng (engine-native + cancel token).
- **RBAC:** permission workspace `query:run` seed cho `ws_editor` + `ws_admin`; cổng chặn của engine là thành viên-workspace-có-`query:run` **và** database đích đã được grant cho workspace đó.

**Ngoài phạm vi (hoãn)**
- WebSocket hub + streaming trực tiếp → **Phase 3b-2** (`egret-realtime`).
- Hash cache kết quả (`query_results_cache`) — chạy lại query y hệt mà không thực thi lại. Để sau.
- **Enforce** bytes-scanned (phase này chỉ ghi lại để hiển thị).
- Phân trang range-read thật (MVP nạp object JSONL rồi slice server-side; bị chặn bởi `max_rows`).
- Chèn `LIMIT` ở tầng SQL / parse SQL (ta cap ở tầng fetch; xem §6, cố ý khác Superset).
- Hòa giải cancel khi multi-instance / restart (registry cancel nằm in-memory; MVP một process — xem §7).
- Định dạng kết quả khác JSONL (Parquet/Arrow/columnar) và guard max-bytes payload.
- SQL Lab UI, dataset, chart, saved query — Phase 3c (`egret-bi`).

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Tách phase | 3b-1 = engine core + REST polling; 3b-2 = WS hub | Làm core đã hiểu rõ trước; hợp đồng WS được kiểm chứng bởi SQL Lab UI tiêu thụ nó (3c) |
| 2 | Model thực thi | Job bất đồng bộ (tokio task nền) + **REST polling** | Status persist trong `queries`; client poll. Superset xác nhận model poll buộc phải persist status |
| 3 | Lưu kết quả | Object store **JSONL** + preview/metadata Postgres | Ghi row theo stream (bộ nhớ bị chặn); tránh OOM blob-đơn mà cả Superset & Metabase đều chỉ ra |
| 4 | Guard | Row cap (`max_rows+1`) + timeout (native + tokio) + cancel | Bao các case nguy hiểm; timeout/cancel native đáng tin hơn (Metabase/Superset) |
| 5 | RBAC | `query:run` → `ws_editor` + `ws_admin`; cổng member + DB-granted | Default thận trọng; engine + Ranger mới quyết định data thật sự thấy được |
| A | Định dạng kết quả | **JSONL** (không phải một mảng JSON đơn) | Ghi stream bộ nhớ bị chặn + phân trang cắt theo dòng |
| B | Cắt (truncation) | Fetch `max_rows + 1`; nếu có row dư ⇒ `truncated = true`, bỏ row dư | Mẹo phát hiện tràn gọn của Superset |
| C | Áp dụng limit | Ở **tầng fetch**, không rewrite SQL | MVP không có SQL parser; rewrite SQL người dùng làm vỡ multi-statement/DDL/SQL-đã-có-LIMIT |

## 3. Kiến trúc / bố cục crate

```text
crates/
└── egret-engine/
    └── src/
        ├── lib.rs
        ├── model.rs        # QueryResult, Column, Row, QueryStatus, Limits, DTOs
        ├── error.rs        # EngineError + EngineApiError (IntoResponse)
        ├── connector/
        │   ├── mod.rs      # trait Connector + Limits/Progress + dispatch
        │   ├── trino.rs    # Trino REST statement protocol (mở rộng tester 3a)
        │   └── starrocks.rs# StarRocks mysql_async (mở rộng tester 3a)
        ├── store.rs        # QueryStore (CRUD bảng `queries`) + I/O kết quả JSONL qua egret-storage
        ├── job.rs          # JobManager: spawn task, registry cancel, persist lifecycle
        └── routes.rs       # route REST + EngineState
```

**Layering / deps:** `egret-engine` phụ thuộc `egret-{core,config,metastore,auth,workspace,database,secrets,storage}`. Dùng `egret-database` (`DatabaseRepo`) để resolve `database_id → (engine, host, port, ConnOptions)` và `open_secret`; `egret-storage` (`ObjectStore`) để ghi/đọc kết quả JSONL. Được `egret-server` mount; `EngineState` build trong `egret-bin`. Không tạo dependency cycle mới (server → engine → {database, storage, …}).

**Tái dùng từ Phase 3a:** slice Trino statement-protocol và slice StarRocks `mysql_async` (với `prefer_socket(false)`) trong `egret-database::tester` là hạt giống cho hai connector — connector mở rộng chúng với row-cap, timeout, cancel, và thu thập đầy đủ cột + row.

## 4. Mô hình Connector

```rust
pub struct Limits { pub max_rows: usize, pub timeout: Duration }

pub struct QueryResult {
    pub columns: Vec<Column>,   // name + nhãn type
    pub rows: Vec<Vec<serde_json::Value>>,  // được store ghi ra dạng JSONL
    pub row_count: u64,         // số row trả về (sau khi cap)
    pub bytes_scanned: Option<u64>, // thống kê Trino; chỉ hiển thị
    pub truncated: bool,
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Chạy `sql`, tôn trọng `limits` và `cancel`. Connector fetch tối đa
    /// `limits.max_rows + 1` row; nếu chạm row dư thì set `truncated` và bỏ row đó.
    /// Trả về cột + row đã thu, hoặc EngineError.
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: &CancellationToken,
    ) -> Result<QueryResult, EngineError>;

    /// Cancel best-effort một query đang chạy (engine-native).
    async fn cancel(&self, handle: &EngineHandle) -> Result<(), EngineError>;
}
```

- **Trino** (`connector/trino.rs`): `POST {scheme}://{host}:{port}/v1/statement` với body SQL + `X-Trino-User` (+ basic auth từ secret); poll `nextUri` (gắn lại auth ở MỖI GET — theo fix của 3a), thu `columns` + các page `data`. Dừng khi đủ `max_rows + 1` row (rồi cancel/bỏ). Timeout qua session header `X-Trino-Session: query_max_run_time=<secs>s` (engine-native) cộng tokio timeout lớp ngoài. Query id từ response đầu là `EngineHandle`; cancel qua `DELETE {nextUri}` / `DELETE /v1/query/{queryId}`.
- **StarRocks** (`connector/starrocks.rs`): kết nối `mysql_async` (`prefer_socket(false)`), chạy SQL, tiêu thụ row stream dừng ở `max_rows + 1`. Set query timeout ở tầng statement/connection (engine-native) + tokio timeout lớp ngoài. Connection/query id là `EngineHandle`; cancel qua `KILL QUERY <id>` trên một connection phụ.
- Ánh xạ giá trị MySQL/Trino → `serde_json::Value` xử lý các kiểu phổ biến (int/float/bool/string/null/decimal-as-string/timestamp-as-string); kiểu lạ fallback về dạng chuỗi.

## 5. Mô hình dữ liệu — `migrations/0009_engine.sql`

```sql
CREATE TABLE queries (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id   uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  user_id       uuid REFERENCES users(id),
  sql           text NOT NULL,
  status        text NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued','running','finished','failed','cancelled')),
  columns_jsonb jsonb,            -- [{name, type}] khi đã biết schema
  preview_jsonb jsonb,            -- ≤ preview_rows row đầu
  row_count     bigint,
  bytes_scanned bigint,
  truncated     boolean NOT NULL DEFAULT false,
  result_ref    text,             -- key object-store cho kết quả JSONL đầy đủ
  error         text,
  submitted_at  timestamptz NOT NULL DEFAULT now(),
  started_at    timestamptz,
  finished_at   timestamptz
);
CREATE INDEX queries_ws_user_idx ON queries (workspace_id, user_id, submitted_at DESC);

-- RBAC: chỉ editor + admin được submit query.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'query:run' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

`query_results_cache` để sau.

## 6. Lưu kết quả & giới hạn

- **Định dạng (Quyết định A):** kết quả đầy đủ ghi ra object store dạng **JSONL** — mỗi dòng một mảng JSON (một dòng một row), tại key `<results_prefix>/<id>.jsonl`. Schema cột KHÔNG lặp mỗi dòng; nó nằm một lần trong `queries.columns_jsonb`. Ghi từng dòng khi row về giữ bộ nhớ server **bị chặn** (không buffer cả kết quả) — đúng điểm yếu Superset (blob zlib đơn) và Metabase đã chỉ ra.
- **Preview:** `preview_rows` row đầu (mặc định 100) lưu inline trong `queries.preview_jsonb` để `GET /queries/{id}` trả preview tức thì không cần đọc object store.
- **Cắt (Quyết định B):** connector fetch `max_rows + 1` row. Nếu row dư xuất hiện, set `truncated = true` và bỏ nó; `row_count` là số đã cap. Client đọc `truncated` để báo "kết quả đã bị giới hạn".
- **Áp dụng limit (Quyết định C):** cap được enforce ở **tầng fetch** (dừng tiêu thụ page Trino / row stream StarRocks khi vượt `max_rows + 1`), **không** chèn `LIMIT` vào SQL người dùng. Tránh thêm SQL parser và tránh làm hỏng SQL multi-statement / DDL / đã-có-LIMIT. (Superset rewrite SQL vì có `sqlparse`; ta cố ý không.)
- **Phân trang:** `GET /queries/{id}/results?page=&page_size=` đọc object JSONL và trả slice yêu cầu. MVP nạp object rồi slice trong RAM (bị chặn bởi `max_rows`); đọc byte-range/line-index để sau.

## 7. Vòng đời job & cancel

1. **submit** → authorize (member + `query:run` + database granted cho workspace) → insert row `queries` (`status='queued'`) → đăng ký một `CancellationToken` vào map in-memory key theo query id → spawn tokio task → trả `202 { id, status }`.
2. **task** → set `running` + `started_at` (persist) → resolve database qua `DatabaseRepo` (`ConnOptions` + `open_secret`) → dựng `Connector` → `tokio::time::timeout(limits.timeout, connector.execute(sql, limits, &token))`:
   - **finished:** ghi row ra object store dạng JSONL (`result_ref`), persist `columns_jsonb` / `preview_jsonb` / `row_count` / `bytes_scanned` / `truncated` / `finished_at`, `status='finished'`.
   - **error / engine error:** `status='failed'`, `error=<message>`, `finished_at`.
   - **timeout lớp ngoài:** kích token, best-effort engine cancel, `status='failed'`, `error='timeout'`.
3. **cancel** (`POST …/cancel`) → tra token, kích nó, gọi `connector.cancel(handle)` (Trino `DELETE`, StarRocks `KILL QUERY`) → `status='cancelled'`. Cancel đến khi còn `queued`/vừa bắt đầu vẫn được xử lý vì connector check token **trước khi** execute (bao race early-cancel của Superset).
4. **Giới hạn (một process):** registry cancel + task đang chạy nằm trong một process. Nếu server restart giữa chừng, row của job đó bị kẹt `running` không có task hoàn tất. MVP một instance; reconcile lúc boot (đánh `running` mồ côi → `failed`) để sau 3b-2/về sau. Spec ghi rõ thay vì giả vờ không có.

## 8. RBAC + bề mặt REST

Phạm vi workspace, mount dưới `/api/ws`, tái dùng extractor `WsContext` (resolve slug `{workspace}` → membership + role). Mọi route cần `require_workspace(ctx, "query:run")` **và** xác minh database đích đã được grant cho workspace đó (qua `workspace_database_grant` của `egret-database`).

| Method + path | Mục đích |
|---|---|
| `POST /api/ws/{workspace}/queries` | Submit `{ database_id, sql }` → `202 { id, status }`. |
| `GET /api/ws/{workspace}/queries/{id}` | Status + `columns` + `preview` + `row_count`/`bytes_scanned`/`truncated` + thời gian. Chỉ owner hoặc platform_admin. |
| `GET /api/ws/{workspace}/queries/{id}/results?page=&page_size=` | Một page của kết quả đầy đủ (đọc từ object JSONL). |
| `POST /api/ws/{workspace}/queries/{id}/cancel` | Cancel một query đang chạy. |
| `GET /api/ws/{workspace}/queries` | Các query gần đây của người gọi trong workspace này (history; không kèm body kết quả). |

**Quyền sở hữu:** status/results/cancel chỉ giới hạn cho `user_id` đã submit (hoặc `platform_admin`). Một thành viên workspace không đọc được kết quả query của thành viên khác bằng id.

## 9. Config + wiring

- **`egret-config`:** thêm `[engine]` → `EngineConfig { max_rows: usize (mặc định 100_000), query_timeout_secs: u64 (60), preview_rows: usize (100), results_prefix: String ("query-results") }`. `validate()`: `max_rows ≥ 1`, `query_timeout_secs ≥ 1`, `preview_rows ≤ max_rows`.
- **`egret-server`:** `AppState.engine: Option<egret_engine::EngineState>`; mount engine router dưới `/api/ws` sau stack session + CSRF (các route mutating; tái dùng pattern hiện có). `AppState::without_db()` đặt `engine: None`.
- **`egret-bin`:** build `EngineState { store: QueryStore, jobs: JobManager, databases: DatabaseRepo, storage: Arc<dyn ObjectStore>, sessions, workspaces, authz, limits: Limits, preview_rows, results_prefix }` từ config + DB + `S3Store` dùng chung. Engine tái dùng cấu hình storage của catalog (cùng bucket, khác key prefix).

## 10. Xử lý lỗi

`EngineError`: `NotFound(String)`, `Forbidden`, `BadRequest(String)`, `Connect(String)`, `Engine(String)` (message từ engine), `Timeout`, `Cancelled`, `Limit(String)`, `Storage(egret_storage::StorageError)`, `Db(sqlx::Error)`. Lỗi job kết thúc được persist vào `queries.error` (message đã làm sạch). `EngineApiError` (`IntoResponse`, kiểu `DbApiError`) map lỗi authz/validation sang HTTP cho tầng REST; lỗi DB/storage nội bộ gộp về `500 "internal error"` và không lộ nội tại driver. Message `EngineError::Engine` (vốn không chứa secret — secret đi trong header/opts, không bao giờ trong SQL hay URL) có thể trả cho owner làm `error` của query.

## 11. Kiểm thử

- **`egret-engine` unit:** serde `QueryResult`/DTO; round-trip (de)serialize JSONL; logic cắt (`max_rows + 1` → `truncated`); default `Limits`.
- **DB-gated** (`DATABASE_URL`): vòng đời `QueryStore` — insert (queued) → running → finished với `columns/preview/result_ref/row_count` được persist; liệt kê history; cancel set `cancelled`.
- **Engine-gated** (`EGRET_TEST_TRINO_URL` / `EGRET_TEST_STARROCKS_URL`, Trino :8090 + StarRocks :9030 dev thật): submit `SELECT 1` → `finished` + 1 row + cột đúng; một `SELECT` vượt `max_rows` (vd generated series) → `truncated = true` và `row_count == max_rows`; cancel một query chạy lâu → `cancelled`.
- **HTTP** (`oneshot`, DB-gated): chưa auth → 401; không phải member → 403/404; member không có `query:run` (viewer) → 403; happy path submit→poll→results với engine thật khi có.
- **Config validate** (figment::Jail): `preview_rows ≤ max_rows`; biên dương.

## 12. Tham chiếu

- **Kiến trúc:** [Master design](../architecture/2026-06-17-egret-architecture-design.md) §10.2 (`egret-engine`), §9.5 (mô hình dữ liệu `queries`), §10.1 (`egret-database` cấp connection — tiêu thụ ở đây).
- **Apache Superset** (đã đối chiếu): async SQL Lab qua Celery — `sql_lab.py::get_sql_results` / `execute_sql_statements`, enum `QueryStatus` (PENDING/RUNNING/SUCCESS/FAILED/TIMED_OUT/STOPPED), results backend key theo `results_key` (blob out-of-band), `SQL_MAX_ROW = 100000`, cờ cắt `limiting_factor`, `apply_limit` (`limit + 1`), `cancel_query`/`get_cancel_query_id` theo engine (MySQL `KILL`, Postgres `pg_terminate_backend`). **Tiếp thu:** state machine status persist, key kết quả out-of-band, mẹo tràn `max_rows + 1`, cờ cắt rõ ràng, cancel engine-native, async-native thay Celery. **Khác đi:** ta cap ở tầng fetch thay vì rewrite SQL (không có `sqlparse`).
- **Metabase** (đã đối chiếu): query processor đồng-bộ-có-streaming — `qp.streaming/streaming-response`, JDBC `setQueryTimeout` (timeout engine-native, mặc định ~60s), `*canceled-chan*` → `Statement.cancel()` (cancel theo client-disconnect), constraint `max-results`/`absolute-max-results` + `rows_truncated`, log `query_execution` + `query_cache` key theo hash. **Tiếp thu:** ghi kết quả JSONL/stream (bộ nhớ bị chặn), timeout engine-native, đếm row khi stream, cờ cắt. **Khác:** Metabase stream ra client và cancel khi disconnect; model job bất đồng bộ của ta persist status trong Postgres và cancel qua API tường minh — đúng cho query bền, fetch nhiều lần, chạy lâu.
- **Pattern hiện có:** `egret-database` (Phase 3a — `Option<_>` sub-state, test DB-/engine-gated, map `DbApiError`, slice tester Trino/StarRocks), `egret-workspace::{context::WsContext, rbac::Authorizer}` (RBAC phạm vi workspace), `egret-storage::ObjectStore` (I/O kết quả).

## 13. Phân rã cho plan

Các task đề xuất (~10): (1) scaffold crate `egret-engine` + workspace deps; (2) `model` + `EngineError`/`EngineApiError`; (3) trait `Connector` + Trino connector (engine-gated); (4) StarRocks connector (engine-gated); (5) `migrations/0009_engine.sql` + `QueryStore` + I/O kết quả JSONL object-store (DB-gated); (6) `egret-config` `[engine]` + validate; (7) `JobManager` submit/run/cancel + persist lifecycle (DB-gated); (8) route REST + RBAC + ownership + HTTP test (DB-gated); (9) wiring `egret-server`/`egret-bin`; (10) verify cuối.
