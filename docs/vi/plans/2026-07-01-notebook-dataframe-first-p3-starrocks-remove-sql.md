# Notebook dataframe-first — follow-up: starrocks() + gỡ SQL surface (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-01-notebook-dataframe-first-p3-starrocks-remove-sql.md`. Rút gọn — code/step đầy đủ xem bản Anh.
> Spec: `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md`. Spike: `.../notebook-starrocks-jwt-spike-findings.md` (§4 recipe, §5 DIRECT). Thực thi qua subagent-driven-development.

**Mục tiêu:** thêm `starrocks(sql)` Daft-direct (DIRECT theo P0), verify live cả 3 engine, rồi **gỡ SQL surface** (magic `%%egret_sql`, SQL cell type + dropdown, path `query()`/`/kernel/query`).

**Thứ tự an toàn:** gỡ SQL surface (Phase C) **chỉ sau khi** verify live (Phase B) xác nhận 3 helper chạy — không gỡ path cũ trước khi thay thế được chứng minh.

## Phase A — starrocks() helper
- **A1** — thêm `pymysql` + `sqlglot` vào `kernel-image/Dockerfile`.
- **A2** — `starrocks(sql)` (TDD): mirror `trino()` nhưng dùng **PyMySQL** + custom auth-plugin `authentication_openid_connect_client` (reply `[0x01][lenenc(JWT)][JWT]`, recipe spike §4); `daft.read_sql(sql, factory)` với `create_engine("mysql+pymysql://", creator=...)`. Export + auto-load `starrocks`. Tests mock daft/pymysql/sqlalchemy.

## Phase B — verify live (checkpoint, gate Phase C)
- **B1** — rebuild kernel image (có pymysql/sqlglot), login user có refresh token, engine Trino/StarRocks enabled + warehouse. Chạy `iceberg().show()`, `trino("SELECT 1").show()`, `starrocks("SELECT current_user()").show()` — mỗi cái ra Daft DF as-user. Pass hết → Phase C; starrocks fail → STOP.

## Phase C — gỡ SQL surface
- **C1 Frontend** — `CellEditor.tsx`: bỏ SelectItem "SQL" + dropdown target + prop `onChangeSqlTarget`, `monacoLanguage`→"python". `NotebookPage.tsx`: `handleRun` gửi thẳng `source` (bỏ wrap `%%egret_sql`), bỏ `sql_target`. `api/notebooks.ts`: bỏ field/param `sql_target`. Cập nhật tests. `tsc` bắt reference thừa.
- **C2 Kernel image** — `git rm magic.py query.py test_magic.py test_query.py`; dọn `__init__.py` (bỏ export magic/query) + `startup/00-egret.py` (bỏ import query + block load magic).
- **C3 Backend** — bỏ route `/kernel/query` + handler `kernel_query` + field `query_runner`; `git rm query.rs` (seam `NotebookQueryRunner`) + dọn `lib.rs`; bỏ `EngineQueryRunner` + wiring trong egret-bin; sửa `tests/http.rs`. Giữ `notebook_runtime_token`, `engine-token`, `notebook_engine_access`.
- **C4 Migration** — `ALTER TABLE notebook_cells DROP COLUMN sql_target`; bỏ `sql_target` khỏi `model.rs`/`store.rs`; `touch egret-metastore/src/lib.rs` (re-embed migrator) → migrate + test.

## D1 — verify đóng phase
- Full CI local (fmt/clippy/test workspace + FE lint/tsc/vitest/build + pytest). Final review → finish branch.

## Rủi ro
- starrocks TLS: handler PyMySQL chạy cả plaintext lẫn TLS (spike §5); không đổi code cho prod TLS.
- Gỡ path cũ chỉ sau verify live (Phase B).
- Giữ `notebook_runtime_token` + `notebook_engine_access` (vẫn dùng).
- Drop `sql_target` là destructive (cột unused; ok), migration một chiều.
