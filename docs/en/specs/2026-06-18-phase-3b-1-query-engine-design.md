# Phase 3b-1 — Query Engine (async execution, REST polling) Design Spec

> **Status:** Approved design · **Date:** 2026-06-18 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-18-phase-3b-1-query-engine-design.md`](../../vi/specs/2026-06-18-phase-3b-1-query-engine-design.md).
> Implements the first half of [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.2 (`egret-engine`). Consumes the database registry from [Phase 3a](2026-06-18-phase-3a-database-registration-design.md). Validated against Apache Superset (async SQL Lab) and Metabase (query processor) — see §12.

---

## 1. Goal & scope

Execute a SQL string against a workspace's granted database **asynchronously**, track the job, store the result, and let clients poll for status + fetch results over REST. This is the engine the later SQL Lab UI (Phase 3c), charts, and Data API will call.

This is **Phase 3b split into two**: **3b-1 (this spec)** is the engine core + REST polling; **3b-2** adds the WebSocket hub (`egret-realtime`) + live status/result streaming on top, ideally alongside the SQL Lab UI that consumes it.

**In scope**
- One new crate `egret-engine`: a `Connector` trait + Trino (REST statement protocol) and StarRocks (`mysql_async`) connectors; a `QueryStore` (Postgres `queries` table) + result I/O to object store; a `JobManager` (async tokio task per query, lifecycle persisted, cancellation); REST routes (submit / status / paginated results / cancel / history).
- **Async job model with REST polling.** `POST` returns a job id immediately; the query runs in a background task; clients `GET` status and results. No WebSocket in 3b-1.
- **Result storage:** the full result is written to object store as **JSONL** (one JSON row per line) at `<results_prefix>/<id>.jsonl`; a row-capped **preview** + column schema + stats live in `queries` (Postgres).
- **Guards:** configurable row cap (fetch `max_rows + 1`, truncate, flag), per-query timeout (engine-native where available + an outer tokio timeout), and user cancellation (engine-native + a cancel token).
- **RBAC:** a `query:run` workspace permission seeded to `ws_editor` + `ws_admin`; the engine gate is workspace-membership-with-`query:run` **and** the target database granted to that workspace.

**Out of scope (deferred)**
- WebSocket hub + live streaming → **Phase 3b-2** (`egret-realtime`).
- Result hash cache (`query_results_cache`) — re-run identical queries without re-executing. Later.
- Byte-scanned **enforcement** (recorded for display only this phase).
- True range-read pagination (MVP loads the JSONL object and slices server-side; bounded by `max_rows`).
- SQL-level `LIMIT` injection / SQL parsing (we cap at the fetch layer; see §6, deliberate deviation from Superset).
- Multi-instance / restart cancel reconciliation (the cancel registry is in-memory; MVP is single-process — see §7).
- Result format beyond JSONL (Parquet/Arrow/columnar) and a payload max-bytes guard.
- SQL Lab UI, datasets, charts, saved queries — Phase 3c (`egret-bi`).

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Phase split | 3b-1 = engine core + REST polling; 3b-2 = WS hub | Build the well-understood core first; the WS contract is validated by the SQL Lab UI that consumes it (3c) |
| 2 | Execution model | Async job (background tokio task) + **REST polling** | Status persisted in `queries`; clients poll. Superset confirms a poll model must persist status |
| 3 | Result storage | Object store **JSONL** + Postgres preview/metadata | Stream-write rows (bounded memory); avoids the single-blob OOM both Superset & Metabase flag |
| 4 | Guards | Row cap (`max_rows+1`) + timeout (native + tokio) + cancel | Covers the dangerous cases; native timeout/cancel are more reliable (Metabase/Superset) |
| 5 | RBAC | `query:run` → `ws_editor` + `ws_admin`; member + DB-granted gate | Conservative default; engine + Ranger govern actual data visibility |
| A | Result format | **JSONL** (not a single JSON array) | Bounded-memory stream-write + line-sliceable pagination |
| B | Truncation | Fetch `max_rows + 1`; if the extra row exists ⇒ `truncated = true`, drop it | Superset's clean overflow-detection trick |
| C | Limit application | At the **fetch layer**, not by rewriting SQL | No SQL parser in MVP; rewriting user SQL breaks multi-statement/DDL/existing-LIMIT |

## 3. Architecture / crate layout

```text
crates/
└── egret-engine/
    └── src/
        ├── lib.rs
        ├── model.rs        # QueryResult, Column, Row, QueryStatus, Limits, DTOs
        ├── error.rs        # EngineError + EngineApiError (IntoResponse)
        ├── connector/
        │   ├── mod.rs      # Connector trait + Limits/Progress + dispatch
        │   ├── trino.rs    # Trino REST statement protocol (extends 3a tester)
        │   └── starrocks.rs# StarRocks mysql_async (extends 3a tester)
        ├── store.rs        # QueryStore (Postgres `queries` CRUD) + JSONL result I/O via egret-storage
        ├── job.rs          # JobManager: spawn task, cancel registry, persist lifecycle
        └── routes.rs       # REST routes + EngineState
```

**Layering / deps:** `egret-engine` depends on `egret-{core,config,metastore,auth,workspace,database,secrets,storage}`. It uses `egret-database` (`DatabaseRepo`) to resolve `database_id → (engine, host, port, ConnOptions)` and to `open_secret`; `egret-storage` (`ObjectStore`) to write/read the JSONL result. Mounted by `egret-server`; `EngineState` built in `egret-bin`. No new dependency cycles (server → engine → {database, storage, …}).

**Reuse from Phase 3a:** the Trino statement-protocol slice and the StarRocks `mysql_async` slice (with `prefer_socket(false)`) in `egret-database::tester` are the seeds for the two connectors — the connectors extend them with row-cap, timeout, cancellation, and full column+row collection.

## 4. Connector model

```rust
pub struct Limits { pub max_rows: usize, pub timeout: Duration }

pub struct QueryResult {
    pub columns: Vec<Column>,   // name + type label
    pub rows: Vec<Vec<serde_json::Value>>,  // streamed out as JSONL by the store
    pub row_count: u64,         // rows returned (after cap)
    pub bytes_scanned: Option<u64>, // Trino stats; display only
    pub truncated: bool,
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Run `sql`, honoring `limits` and `cancel`. The connector fetches up to
    /// `limits.max_rows + 1` rows; if it reaches the extra row it sets `truncated`
    /// and drops it. Returns the collected columns + rows, or an EngineError.
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: &CancellationToken,
    ) -> Result<QueryResult, EngineError>;

    /// Best-effort cancel of an in-flight query (engine-native).
    async fn cancel(&self, handle: &EngineHandle) -> Result<(), EngineError>;
}
```

- **Trino** (`connector/trino.rs`): `POST {scheme}://{host}:{port}/v1/statement` with the SQL body + `X-Trino-User` (+ basic auth from the secret); poll `nextUri` (re-attaching auth on every GET — see the 3a fix), collecting `columns` + `data` pages. Stop once `max_rows + 1` rows are accumulated (then call cancel/abandon). Honor the timeout via the session header `X-Trino-Session: query_max_run_time=<secs>s` (engine-native) plus the outer tokio timeout. The query id from the first response is the `EngineHandle`; cancel via `DELETE {nextUri}` / `DELETE /v1/query/{queryId}`.
- **StarRocks** (`connector/starrocks.rs`): `mysql_async` connection (`prefer_socket(false)`), run the SQL, consume the row stream stopping at `max_rows + 1`. Set the statement/connection query timeout (engine-native) + outer tokio timeout. The connection/query id is the `EngineHandle`; cancel via `KILL QUERY <id>` on a side connection.
- The MySQL/Trino value→`serde_json::Value` mapping handles the common types (int/float/bool/string/null/decimal-as-string/timestamp-as-string); exotic types fall back to their string form.

## 5. Data model — `migrations/0009_engine.sql`

```sql
CREATE TABLE queries (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id   uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  user_id       uuid REFERENCES users(id),
  sql           text NOT NULL,
  status        text NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued','running','finished','failed','cancelled')),
  columns_jsonb jsonb,            -- [{name, type}] once the schema is known
  preview_jsonb jsonb,            -- first ≤ preview_rows rows
  row_count     bigint,
  bytes_scanned bigint,
  truncated     boolean NOT NULL DEFAULT false,
  result_ref    text,             -- object-store key for the full JSONL result
  error         text,
  submitted_at  timestamptz NOT NULL DEFAULT now(),
  started_at    timestamptz,
  finished_at   timestamptz
);
CREATE INDEX queries_ws_user_idx ON queries (workspace_id, user_id, submitted_at DESC);

-- RBAC: only editors + admins may submit queries.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'query:run' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

`query_results_cache` is deferred.

## 6. Result storage & limits

- **Format (Decision A):** the full result is written to the object store as **JSONL** — one JSON array per line (one line per row), at key `<results_prefix>/<id>.jsonl`. The column schema is NOT repeated per line; it lives once in `queries.columns_jsonb`. Writing line-by-line as rows arrive keeps server memory **bounded** (no full-result buffering), the weak point both Superset (single zlib blob) and Metabase flagged.
- **Preview:** the first `preview_rows` (default 100) rows are also stored inline in `queries.preview_jsonb` so `GET /queries/{id}` returns an instant preview without an object-store read.
- **Truncation (Decision B):** the connector fetches `max_rows + 1` rows. If the extra row materializes, it sets `truncated = true` and drops it; `row_count` is the capped count. Clients read `truncated` to show a "results were limited" notice.
- **Limit application (Decision C):** the cap is enforced at the **fetch layer** (stop consuming Trino pages / the StarRocks row stream past `max_rows + 1`), **not** by injecting a `LIMIT` into the user's SQL. This avoids adding a SQL parser and avoids corrupting multi-statement / DDL / already-limited SQL. (Superset rewrites SQL because it ships `sqlparse`; we deliberately don't.)
- **Pagination:** `GET /queries/{id}/results?page=&page_size=` reads the JSONL object and returns the requested slice. MVP loads the object and slices in memory (bounded by `max_rows`); byte-range/line-index reads are deferred.

## 7. Job lifecycle & cancellation

1. **submit** → authorize (member + `query:run` + database granted to the workspace) → insert a `queries` row (`status='queued'`) → register a `CancellationToken` in an in-memory map keyed by query id → spawn a tokio task → return `202 { id, status }`.
2. **task** → set `running` + `started_at` (persist) → resolve the database via `DatabaseRepo` (`ConnOptions` + `open_secret`) → build the `Connector` → `tokio::time::timeout(limits.timeout, connector.execute(sql, limits, &token))`:
   - **finished:** stream the rows to the object store as JSONL (`result_ref`), persist `columns_jsonb` / `preview_jsonb` / `row_count` / `bytes_scanned` / `truncated` / `finished_at`, `status='finished'`.
   - **error / engine error:** `status='failed'`, `error=<message>`, `finished_at`.
   - **outer timeout:** trigger the token, best-effort engine cancel, `status='failed'`, `error='timeout'`.
3. **cancel** (`POST …/cancel`) → look up the token, trigger it, call `connector.cancel(handle)` (Trino `DELETE`, StarRocks `KILL QUERY`) → `status='cancelled'`. A cancel that arrives while still `queued`/just starting is handled because the connector checks the token **before** executing (covers Superset's early-cancel race).
4. **Limitation (single-process):** the cancel registry + the running task live in one process. If the server restarts mid-query, that job's row is left `running` with no task to finish it. MVP is single-instance; a boot-time reconcile (mark orphaned `running` → `failed`) is deferred to 3b-2/later. The spec records this explicitly rather than pretending otherwise.

## 8. RBAC + REST surface

Workspace-scoped, mounted under `/api/ws`, reusing the existing `WsContext` extractor (resolves the `{workspace}` slug → membership + role). Every route requires `require_workspace(ctx, "query:run")` **and** verifies the target database is granted to that workspace (via `egret-database`'s `workspace_database_grant`).

| Method + path | Purpose |
|---|---|
| `POST /api/ws/{workspace}/queries` | Submit `{ database_id, sql }` → `202 { id, status }`. |
| `GET /api/ws/{workspace}/queries/{id}` | Status + `columns` + `preview` + `row_count`/`bytes_scanned`/`truncated` + timing. Owner or platform_admin only. |
| `GET /api/ws/{workspace}/queries/{id}/results?page=&page_size=` | A page of the full result (read from the JSONL object). |
| `POST /api/ws/{workspace}/queries/{id}/cancel` | Cancel a running query. |
| `GET /api/ws/{workspace}/queries` | The caller's recent queries in this workspace (history; no result bodies). |

**Ownership:** status/results/cancel are restricted to the submitting `user_id` (or a `platform_admin`). A workspace member cannot read another member's query result by id.

## 9. Config + wiring

- **`egret-config`:** add `[engine]` → `EngineConfig { max_rows: usize (default 100_000), query_timeout_secs: u64 (60), preview_rows: usize (100), results_prefix: String ("query-results") }`. `validate()`: `max_rows ≥ 1`, `query_timeout_secs ≥ 1`, `preview_rows ≤ max_rows`.
- **`egret-server`:** `AppState.engine: Option<egret_engine::EngineState>`; mount the engine router under `/api/ws` behind the session + CSRF stack (the mutating routes; reuse the existing pattern). `AppState::without_db()` sets `engine: None`.
- **`egret-bin`:** build `EngineState { store: QueryStore, jobs: JobManager, databases: DatabaseRepo, storage: Arc<dyn ObjectStore>, sessions, workspaces, authz, limits: Limits, preview_rows, results_prefix }` from config + DB + the shared `S3Store`. The engine reuses the catalog's storage config (same bucket, different key prefix).

## 10. Error handling

`EngineError`: `NotFound(String)`, `Forbidden`, `BadRequest(String)`, `Connect(String)`, `Engine(String)` (message surfaced from the engine), `Timeout`, `Cancelled`, `Limit(String)`, `Storage(egret_storage::StorageError)`, `Db(sqlx::Error)`. Terminal job errors are persisted to `queries.error` (a sanitized message). `EngineApiError` (`IntoResponse`, `DbApiError`-style) maps authz/validation errors to HTTP for the REST layer; internal DB/storage errors collapse to `500 "internal error"` and never leak driver internals. `EngineError::Engine` messages (already non-secret — the secret travels in headers/opts, never in SQL or URLs) may be returned to the owner as the query `error`.

## 11. Testing

- **`egret-engine` unit:** `QueryResult`/DTO serde; JSONL (de)serialization round-trip; truncation logic (`max_rows + 1` → `truncated`); `Limits` defaults.
- **DB-gated** (`DATABASE_URL`): `QueryStore` lifecycle — insert (queued) → running → finished with `columns/preview/result_ref/row_count` persisted; history listing; cancel sets `cancelled`.
- **Engine-gated** (`EGRET_TEST_TRINO_URL` / `EGRET_TEST_STARROCKS_URL`, live dev Trino :8090 + StarRocks :9030): submit `SELECT 1` → `finished` + 1 row + correct columns; a `SELECT` exceeding `max_rows` (e.g. a generated series) → `truncated = true` and `row_count == max_rows`; cancel a long-running query → `cancelled`.
- **HTTP** (`oneshot`, DB-gated): unauthenticated → 401; non-member → 403/404; a member without `query:run` (a viewer) → 403; submit→poll→results happy path against a live engine where available.
- **Config validate** (figment::Jail): `preview_rows ≤ max_rows`; positive bounds.

## 12. References

- **Architecture:** [Master design](../architecture/2026-06-17-egret-architecture-design.md) §10.2 (`egret-engine`), §9.5 (`queries` data model), §10.1 (`egret-database` provides connections — consumed here).
- **Apache Superset** (validated): async SQL Lab via Celery — `sql_lab.py::get_sql_results` / `execute_sql_statements`, `QueryStatus` enum (PENDING/RUNNING/SUCCESS/FAILED/TIMED_OUT/STOPPED), results backend keyed by `results_key` (out-of-band blob), `SQL_MAX_ROW = 100000`, `limiting_factor` truncation signal, `apply_limit` (`limit + 1`), per-engine `cancel_query`/`get_cancel_query_id` (MySQL `KILL`, Postgres `pg_terminate_backend`). **Adopted:** persisted status state machine, out-of-band result key, `max_rows + 1` overflow trick, explicit truncation flag, engine-native cancel, native-async-over-Celery. **Deviated:** we cap at the fetch layer rather than rewriting SQL (no `sqlparse` equivalent).
- **Metabase** (validated): synchronous-with-streaming query processor — `qp.streaming/streaming-response`, JDBC `setQueryTimeout` (engine-native timeout, ~60s default), `*canceled-chan*` → `Statement.cancel()` (client-disconnect cancel), `max-results`/`absolute-max-results` constraints + `rows_truncated`, `query_execution` log + hash-keyed `query_cache`. **Adopted:** JSONL/streamed result write (bounded memory), engine-native timeout, streaming row-count counter, truncation flag. **Diverged:** Metabase streams to the client and cancels on disconnect; our async job model persists status in Postgres and cancels via an explicit API — the right model for durable, multi-fetch, long-running queries.
- **Existing patterns:** `egret-database` (Phase 3a — `Option<_>` sub-state, DB-/engine-gated tests, `DbApiError` mapping, the Trino/StarRocks tester slices), `egret-workspace::{context::WsContext, rbac::Authorizer}` (workspace-scoped RBAC), `egret-storage::ObjectStore` (result I/O).

## 13. Decomposition for the plan

Suggested tasks (~10): (1) scaffold `egret-engine` crate + workspace deps; (2) `model` + `EngineError`/`EngineApiError`; (3) `Connector` trait + Trino connector (engine-gated); (4) StarRocks connector (engine-gated); (5) `migrations/0009_engine.sql` + `QueryStore` + JSONL object-store I/O (DB-gated); (6) `egret-config` `[engine]` + validate; (7) `JobManager` submit/run/cancel + lifecycle persistence (DB-gated); (8) REST routes + RBAC + ownership + HTTP tests (DB-gated); (9) `egret-server`/`egret-bin` wiring; (10) final verification.
