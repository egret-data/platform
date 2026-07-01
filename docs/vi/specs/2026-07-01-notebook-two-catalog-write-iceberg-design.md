# Notebook: mô hình 2-catalog + ghi Iceberg + e2e dataframe — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-notebook-two-catalog-write-iceberg-design.md`. File này rút gọn.
> Trạng thái: đã duyệt (brainstorming) · 2026-07-01.

## Mục tiêu
Cho notebook một catalog Iceberg **đọc-ghi riêng** (tách khỏi catalog shared mà Trino/StarRocks query), thêm helper `write_iceberg()`, và viết e2e Playwright opt-in cho path dataframe-first (đọc engine + roundtrip write→read Iceberg).

## Hiện trạng
- Dataframe-first đã ship (#110/#111/#112): `iceberg()`/`trino()`/`starrocks()` trả Daft DF lazy; `/kernel/engine-token` mint JWT.
- Workspace đã hỗ trợ nhiều warehouse. `build_notebook_runtime` lấy warehouse **đầu tiên**, provision namespace `notebook_{user}` + token rw, trả `RuntimeCatalog`. `catalog()` → RestCatalog rw đó; `iceberg()` đọc qua nó. **Chưa có write helper.**

## Quyết định (brainstorming)
1. **Routing:** shared đọc CHỈ qua `trino()`/`starrocks()` (SQL). `iceberg()` + `write_iceberg()` chỉ thao tác catalog **B** (của notebook). Không iceberg-direct-read shared.
2. **B theo convention tên:** warehouse tên **`notebook`** là catalog B; còn lại là shared. Không có `notebook` → fallback warehouse đầu tiên (back-compat).
3. **`write_iceberg(df, table, mode="append")`** — auto-create bảng từ schema df nếu chưa có, rồi ghi; mode append (default) | overwrite; vào namespace `notebook_{user}` (rw) trong B.
4. **e2e opt-in** (như `notebook.spec.ts`) — cần live stack + engine đăng ký + warehouse `notebook`; KHÔNG vào CI.

## Kiến trúc
- **A. Backend** (`runtime.rs`): `build_notebook_runtime` chọn warehouse `name=="notebook"` nếu có, không thì warehouse đầu tiên (fallback). Thêm const `NOTEBOOK_WAREHOUSE_NAME="notebook"`. Downstream (provision namespace/token/RuntimeCatalog) không đổi — chỉ trỏ vào B. Đây là thay đổi backend DUY NHẤT.
- **B. Helper** (`engines.py` hoặc `iceberg_io.py`): `write_iceberg(df, table, *, mode="append")` — dùng `catalog()` (rw B); resolve `(namespace,name)` (default namespace = `notebook_{user}`); auto-create bảng từ schema Arrow của df nếu chưa có; `df.write_iceberg(pyiceberg_table, mode)`; lỗi rõ ràng `egret:`. Export + auto-load cạnh iceberg/trino/starrocks.
- **C. e2e** (`frontend/tests/e2e/notebook-dataframe.spec.ts`, opt-in): SSO login → tạo notebook → (1) `trino("SELECT 1").show()` assert; (2) roundtrip: `write_iceberg(df,"e2e_roundtrip",mode="overwrite")` → `iceberg("e2e_roundtrip").show()` assert rows. Cập nhật README (workspace cần warehouse `notebook` + storage + ≥1 engine đăng ký).

## Test
- Backend (Rust, DB-gated): test chọn warehouse — 2 warehouse (shared+notebook) → chọn `notebook`; 1 warehouse → chọn nó (back-compat).
- Helper (pytest): `write_iceberg` (mock daft+catalog): create-if-missing, mode append/overwrite, default namespace. `test_init` thêm `write_iceberg`.
- e2e opt-in.

## Ngoài phạm vi
- Iceberg-direct read shared từ notebook (chốt: shared chỉ SQL). Đổi config catalog A của Trino/StarRocks. Auto-provision warehouse `notebook` (admin tự tạo, chỉ document). Schema evolution/partition khi ghi (MVP: bảng phẳng append/overwrite).

## Rủi ro
- **Daft→pyiceberg schema derivation** (auto-create) là chỗ khó nhất — chốt API cụ thể theo version cài (Daft 0.7.x, pyiceberg): `df.write_iceberg` create-on-write hay tự `catalog.create_table(identifier, schema)` từ Arrow schema.
- Fallback theo tên: workspace không có `notebook` warehouse vẫn chạy (dùng warehouse đầu) nhưng write rơi vào đó — document nên tạo `notebook` để tách rw khỏi BI.
- e2e write dùng `overwrite` bảng tên cố định → rerun idempotent.
