# Notebook 2-catalog + ghi Iceberg + e2e dataframe — Kế hoạch (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-01-notebook-two-catalog-write-iceberg.md` (code/step đầy đủ ở đó).
> Spec: `docs/en/specs/2026-07-01-notebook-two-catalog-write-iceberg-design.md`. Thực thi qua subagent-driven-development.

**Mục tiêu:** notebook có catalog Iceberg đọc-ghi riêng (warehouse tên `notebook`), thêm helper `write_iceberg()`, và e2e opt-in (đọc engine + roundtrip write→read Iceberg).

**API đã pin (Daft 0.7.16 / pyiceberg 0.11.1):** `df.write_iceberg(pyiceberg_table, mode="append"|"overwrite")`; `pyiceberg Catalog.create_table(ident, schema)` nhận **PyArrow schema trực tiếp** → auto-create = `cat.create_table(ident, df.to_arrow().schema)`; bảng thiếu → `pyiceberg.exceptions.NoSuchTableError`.

## Task
- **T1 (backend, `runtime.rs`)** — thêm const `NOTEBOOK_WAREHOUSE_NAME="notebook"` + hàm thuần `select_notebook_warehouse(vec, name_of)` (generic theo accessor tên → unit-test bằng `Vec<&str>`, không dựng `Warehouse`): ưu tiên warehouse tên `notebook`, không có → cái đầu (back-compat). Dùng trong `build_notebook_runtime`. 3 unit test.
- **T2 (helper, `engines.py`)** — `write_iceberg(df, table, *, mode="append", env=None)`: `catalog()` (rw B) → `_resolve_identifier` (bare name → `notebook_{user}.name` qua `load_runtime`) → load_table, `NoSuchTableError` → `create_table(ident, df.to_arrow().schema)` → `df.write_iceberg(tbl, mode)`. Import `NoSuchTableError` lazy. pytest: create-if-missing + existing + default-namespace + mode.
- **T3** — export + auto-load `write_iceberg` (`__init__.py`, `startup/00-egret.py`, `test_init`).
- **T4 (e2e opt-in)** — `frontend/tests/e2e/notebook-dataframe.spec.ts`: SSO login → tạo notebook → cell `df=daft.from_pydict(...)`; `write_iceberg(df,"e2e_roundtrip",mode="overwrite")`; `iceberg("e2e_roundtrip").show()` → assert marker. README: workspace cần warehouse `notebook` + storage + (optional) engine đăng ký. `tsc`/`eslint`.
- **T5** — verify unit (cargo + pytest), optional live write_iceberg smoke, full CI + finish.

## Rủi ro
- `Warehouse.name` giả định `String` (`.as_str()`) — generic fn né trong test.
- Daft write API pin theo 0.7.16; bump Daft phải re-verify.
- Back-compat: workspace không có `notebook` warehouse vẫn chạy (dùng cái đầu); README khuyến nghị tạo `notebook` để tách rw.
- e2e opt-in (như `notebook.spec.ts`), không vào CI.
