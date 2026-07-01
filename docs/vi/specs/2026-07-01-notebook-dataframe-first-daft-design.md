# Notebook: dataframe-first với Daft làm engine — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md`. File này là bản tham chiếu rút gọn.
> Trạng thái: đã duyệt (brainstorming) · 2026-07-01.

## Mục tiêu
Xoay notebook quanh **Daft dataframe**. Daft nối **thẳng** Iceberg / Trino / StarRocks as-user (JWT ngắn hạn per-user cắm sẵn trong kernel), trả **Daft DataFrame lazy** để compose bằng Python. **Bỏ** cell type SQL + magic `%%egret_sql`. Plane BI (SQL Lab, dataset/chart/dashboard, Rust patched-`mysql_async`) **giữ nguyên**.

## Hiện trạng
- Kernel Python/Daft, 1 gateway per (user×workspace). SQL trong notebook hiện là cell type + dropdown + magic `%%egret_sql --target daft|<engine>`:
  - `daft` → `daft.sql(cell)` nhưng **không attach catalog Iceberg** → chỉ resolve DataFrame trong scope (docstring "attached Iceberg catalog" là ảo).
  - `trino/starrocks` → `query()` → `POST /kernel/query` → materialize `daft.from_pydict` (eager, cap 100k, không pushdown).
- Flow "chọn engine, gõ SQL, xem grid" trùng SQL Lab.
- F2 đã có JWT delegation per-user (RFC 8693, `EngineTokenMinter`), runtime manifest (`GET …/notebook-runtime`: username, catalog token, engines/catalogs, `engine_jwt`), `catalog()` (pyiceberg RestCatalog), callback auth bằng `EGRET_KERNEL_TOKEN`.

## Feasibility (đã research)
- **Iceberg**: native — `daft.read_iceberg(pyiceberg_table)` + token per-user qua `catalog()`. Lazy, pushdown.
- **Trino**: được — `daft.read_sql(sql, factory)` với SQLAlchemy `trino://` + `JWTAuthentication(jwt)`. Pushdown as-user.
- **StarRocks**: chưa chắc (spike Phase 0) — JWT qua MySQL protocol cần plugin `authentication_openid-connect_client` (MySQL client 9.2+), driver Python không hỗ trợ sẵn (Rust phải patch `mysql_async`).

## Quyết định (brainstorming)
1. **Helper mỏng nạp sẵn**, trả Daft DF lazy: `iceberg(table)`, `trino(sql)`, `starrocks(sql)`.
2. **StarRocks spike trước (Phase 0)** — chứng minh Python→StarRocks bằng JWT trước; được → direct, không → `starrocks()` giữ signature nhưng route qua callback governed (hybrid).
3. **Bỏ SQL khỏi notebook** — bỏ cell type SQL + dropdown + magic. Cell còn Code + Markdown.
4. **JWT nằm trong kernel** — mỗi helper lấy JWT tươi per-user để Daft nối thẳng. Đánh đổi có chủ đích: token của chính user, downscoped, sống vài phút; user vốn chạy SQL as-self được nên không tăng quyền.
5. **BI plane giữ nguyên** — chỉ chạm kernel + frontend notebook.

## Kiến trúc
- **A. Backend token+coords**: callback mới `POST /api/kernel/engine-token {engine}` → `{jwt, expires_at, kind, host, port, catalog, scheme}` (re-validate `notebook_engine_access`, mint qua `EngineTokenMinter`). Manifest `engines[]` thêm toạ độ kết nối. Iceberg dùng token catalog sẵn có.
- **B. Helper** (`kernel-image/egret_kernel_helpers/`): `iceberg()` = `daft.read_iceberg(catalog().load_table(...))`; `trino()` = `daft.read_sql` + factory SQLAlchemy `trino://` + `JWTAuthentication`; `starrocks()` = direct/hybrid theo Phase 0. `_engine_token(engine)` cache + refresh gần hết hạn. Lỗi rõ ràng, không silent-empty.
- **C. Frontend**: bỏ cell type SQL + dropdown target trong `NotebookPage.tsx`. Cell type = Code | Markdown.
- **D. Kernel image + data model**: bỏ `magic.py` + đăng ký; `sql_target` thành vô dụng (drop sau); **giữ** `notebook_engine_access`/`notebook_catalog_access`; `POST /kernel/query` gỡ khi StarRocks-direct xong, không thì giữ làm hybrid.

## Phasing
- **P0** — spike StarRocks-JWT/Python (exit: recipe chạy được hoặc lý do block rõ).
- **P1** — `/kernel/engine-token` + coords trong manifest + reuse `EngineTokenMinter` (test DB-gated).
- **P2** — 3 helper + `_engine_token` cache/refresh + pytest (mock HTTP + fake daft).
- **P3** — gỡ SQL surface (FE + magic + dọn data model) + vitest/tsc/lint.
- **P4** — verify live: cả 3 nguồn Daft-direct as-user + full CI local.

## Ngoài phạm vi
- Plane BI (SQL Lab, dataset/chart/dashboard, Rust governed SQL). Completion/hover, export `.ipynb`, multi-kernel. Isolation data-file Iceberg per-user (F2-P6).

## Rủi ro
- StarRocks-direct có thể fail (P0) → fallback callback, epic vẫn ship Iceberg+Trino direct.
- JWT lộ trong kernel — chấp nhận (token của chính user, ngắn hạn, refresh).
- Kernel phải tới được host:port engine (đúng topology hiện tại).
- Kernel sống lâu vs JWT ngắn — `_engine_token` cache+refresh; factory luôn dùng token tươi lúc execute.
- Pushdown Iceberg phụ thuộc Daft reader — dạy idiom filter sớm.
