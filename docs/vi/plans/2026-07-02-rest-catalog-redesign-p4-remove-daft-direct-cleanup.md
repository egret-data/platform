# Redesign REST Catalog — Plan 4: hoàn tất pivot (xoá Daft-direct + dọn) (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-rest-catalog-redesign-p4-remove-daft-direct-cleanup.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (đã sửa single-catalog). Plan gộp cho **R** (xoá Daft-direct) + **C** (dọn single-catalog) + **F** (dọn di sản). **A+B ở Plan 1**; **W (write_table) đã xong**. Plan 2 & 3 SUPERSEDED. Thực thi qua subagent-driven-development.

**Mục tiêu:** notebook engine-only — xoá mọi đường Daft-direct (`iceberg`/`write_iceberg`/`catalog`) + provisioning catalog per-user; rewrite e2e sang `write_table`; xoá catalog hand-rolled cũ. Còn một catalog chung, engine-facing.

## Bối cảnh
- Helper: `engines.py` có `iceberg()`/`write_iceberg()`/`_resolve_identifier`; `catalog()` ở `catalog.py` (chỉ 2 hàm kia dùng). `runtime.py` có `RuntimeCatalog`. Export ở `__init__.py`/`startup`. Test: `test_catalog.py`, iceberg tests trong `test_engines.py`, `test_runtime.py`, `test_init.py`.
- Backend: `build_notebook_runtime` gọi `select_notebook_warehouse` + `provision_notebook_access` (mint-and-revoke + `ensure_namespace`); manifest `NotebookRuntime.catalog: RuntimeCatalog`. Sau pivot notebook **không cần** credential catalog + namespace riêng → cả đường provisioning chết.
- e2e: `notebook-dataframe.spec.ts` dùng `write_iceberg`/`iceberg`.
- Di sản: `egret-catalog::api`/`egret-iceberg-ext` — vị trí **không chắc** (`ls src/api/` không thấy) → Task 5 = verify-rồi-xoá bằng `codegraph_impact`.

## Task
- **T1 (helper):** xoá `iceberg`/`write_iceberg`/`_resolve_identifier` khỏi `engines.py`; `git rm catalog.py test_catalog.py`; sửa export `__init__.py`/`startup` (còn `trino`/`starrocks`/`write_table`); xoá test iceberg trong `test_engines.py` + sửa `test_init.py`. pytest.
- **T2 (manifest reader):** xoá `RuntimeCatalog` + `self.catalog` trong `runtime.py`; sửa `test_runtime.py`. pytest.
- **T3 (backend):** xoá field `catalog: RuntimeCatalog` khỏi `NotebookRuntime` (+ struct `RuntimeCatalog`); trong `build_notebook_runtime` bỏ block `list_warehouses`+`select_notebook_warehouse` + call `provision_notebook_access` + field catalog trả về; xoá `select_notebook_warehouse`+3 test+const; xoá `provisioning.rs` (`provision_notebook_access`/`Provisioned`/`NamespaceProvisioner` nếu không còn caller — `rg` kiểm) + `NotebookState.provisioner` + wiring `egret-bin`; sửa re-export `lib.rs`. build+test.
- **T4 (e2e):** rewrite cell sang `write_table(df,"iceberg.e2e.roundtrip",engine="trino",mode="overwrite")` + `trino("SELECT ... ")` → assert `${marker} 42`; giữ guard reopen+rerun; README ghi prerequisite (Trino iceberg catalog→lakekeeper + group provider). tsc.
- **T5 (di sản):** `codegraph_impact` + `rg` xác nhận symbol chết (type `egret-iceberg-ext`, handler hand-rolled) → xoá cái zero-caller (giữ `CatalogStore`); `cargo build --workspace` (fail = còn dùng → thu hẹp).
- **T6 (verify):** fmt + clippy --workspace + test (`egret-notebook`/`egret-catalog`/`egret-catalog-engine`) + kernel pytest + frontend.

## Ghi chú
- Ngoài plan: **A+B (Plan 1)** + prerequisite ops (đăng ký Trino iceberg catalog + JWT→group provider) — spec ghi.
