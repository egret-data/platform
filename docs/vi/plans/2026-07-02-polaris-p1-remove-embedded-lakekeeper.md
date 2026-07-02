# Control-Plane Polaris — Plan 1: xoá embedded-lakekeeper catalog (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p1-remove-embedded-lakekeeper.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P1/6** (P1 xoá → P2 storage/secrets lên control plane → P3 `egret-polaris` client → P4 routes catalog+RBAC → P5 UI → P6 Polaris compose+e2e). Thực thi qua subagent-driven-development.

**Mục tiêu:** xoá toàn bộ catalog embedded-lakekeeper (engine, token/warehouse, provisioning notebook, kernel Daft-direct helpers, DB tables, UI catalog workspace) — greenfield — nhưng **GIỮ** `egret-catalog::storage_conn` cho P2.

**Cách làm:** xoá theo compiler — bỏ consumer trước (notebook provisioning + kernel helpers, rồi engine wiring + mint-token CLI), sau đó xoá crate (`egret-catalog-engine`, `egret-iceberg-ext`) + module chết của `egret-catalog` (`tokens.rs`,`warehouse.rs`), rồi DB tables + UI. `cargo build --workspace` + `codegraph_impact` làm lưới an toàn.

## Bối cảnh
- `egret-catalog` = `tokens.rs`+`warehouse.rs` (XOÁ) + `storage_conn.rs` (GIỮ) + store/mod/lib/error. Dependents: egret-bin/kernel/notebook/server/workspace. egret-kernel không dùng egret_catalog (bỏ dep được); egret-workspace chỉ có comment + dùng storage (giữ).
- Server wiring: `AppState.catalog_engine`/`catalog_browse` + nest `/catalog/v1` (egret-server); assembly + mint-token CLI (egret-bin).
- Notebook: `provisioning.rs` + block catalog trong `runtime.rs` + `NotebookState.{provisioner,catalog}`.
- Kernel: `iceberg`/`write_iceberg`/`catalog`/`RuntimeCatalog` (bỏ); giữ trino/starrocks/write_table.
- Migration: 0047 → **0048**; nhớ `touch metastore/src/lib.rs`.
- Frontend: route `catalog` (bỏ); `storage`/`secrets` (giữ tới P2).

## Task
- **T1 (notebook dep):** xoá kernel `iceberg`/`write_iceberg`/`catalog.py`/`RuntimeCatalog` + sửa export/startup/test; `git rm provisioning.rs`; gọn `runtime.rs`/`lib.rs`/`routes.rs` (bỏ provisioner+catalog+select_notebook_warehouse); build notebook+bin.
- **T2 (engine wiring + CLI):** bỏ `catalog_engine`/`catalog_browse` + nest khỏi egret-server; bỏ assembly block + mint-token CLI + `fmt_scope` khỏi egret-bin (giữ EngineConnResolver/kernel engine-token). build.
- **T3 (xoá crate):** `rg`+`codegraph_impact` xác nhận sạch → `git rm -r egret-catalog-engine egret-iceberg-ext`; bỏ khỏi workspace members + `[patch]` lakekeeper + deps dependents. `cargo build --workspace`.
- **T4 (gọn egret-catalog):** `git rm tokens.rs warehouse.rs` + test; bỏ `pub mod`/re-export trong mod.rs/lib.rs (giữ storage_conn/store/error). build.
- **T5 (DB):** migration `0048_drop_embedded_catalog.sql` (DROP catalog_tokens, workspace_warehouses, schema lakekeeper CASCADE); `touch metastore` + migrate + verify.
- **T6 (UI):** bỏ route `catalog`/`catalog/:wid` + import WarehousesList/WarehouseDetail + nav; `git rm -r routes/explore/catalog` (giữ storage/secrets). typecheck+build.
- **T7 (verify):** fmt + clippy --workspace + test --workspace; frontend typecheck/lint/build; kernel pytest; `rg` xác nhận hết surface catalog + `/catalog/v1` 404.

## Ghi chú
- Giữ cho P2: `egret-catalog::storage_conn` + trang Storage/Secrets workspace (chuyển ở P2).
- Xoá theo compiler: mỗi lần xoá → `cargo build` để lộ dependent; `rg`/`codegraph_impact` trước khi xoá crate.
