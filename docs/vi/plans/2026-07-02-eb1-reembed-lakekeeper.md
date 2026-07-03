# EB-1: nhúng lại lakekeeper (AllowAll) (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb1-reembed-lakekeeper.md`. Spec Component 1. subagent-driven.

**Mục tiêu:** đưa lakekeeper nhúng trở lại egret (in-process, mount `/catalog/v1/*`) với **AllowAll authorizer**, **không auth cũ** (JWT auth + Authorizer thật ở EB-2/EB-3). Bỏ Polaris khỏi compose.

**Kiến trúc:** khôi phục `egret-catalog-engine` (+ `egret-iceberg-ext`) từ `26a13f4~1` (main cũ trước epic; PR #115=`26a13f4` đã xoá), strip auth cũ (front_auth opaque-token, browse, provisioning, mint-token), giữ mount lakekeeper + migrations lakekeeper + seam `Authorizer` gắn AllowAll; nest `/catalog/v1` trong egret-server. lakekeeper dùng Postgres + MinIO có sẵn.

## Task
- **T1 (recover):** `git checkout 26a13f4~1 -- crates/egret-catalog-engine crates/egret-iceberg-ext`; khôi phục dep + `[patch]` lakekeeper/iceberg trong root Cargo.toml (`git show 26a13f4~1:Cargo.toml | grep lakekeeper`); build 2 crate độc lập. Commit recovery.
- **T2 (strip auth):** `git rm` front_auth/browse/provisioning + test; lib.rs bỏ mod/re-export; giữ assembly lakekeeper (`build_state`/`mount_catalog_engine`/`run_lakekeeper_migrations`/`ensure_bootstrapped`/gc); bỏ hack `catalog_public_url`/rewrite/`warehouse_prefix`; gắn `AllowAllAuthorizer` (hoặc của lakekeeper). Build.
- **T3 (mount):** `egret-bin` khôi phục assembly catalog-engine (từ `26a13f4~1:.../main.rs`) → `catalog_engine: Option<Router>` (bỏ catalog_browse); `egret-server` add lại `AppState.catalog_engine` + nest `/catalog/v1` khi Some (**không CSRF**). build workspace.
- **T4 (compose):** xoá service `polaris` khỏi `docker-compose.dev.yml` (lakekeeper nhúng, dùng postgres+minio sẵn).
- **T5 (verify):** serve up → `curl localhost:8080/catalog/v1/config` trả JSON (lakekeeper in-process); log migration+bootstrap OK. fmt+clippy+build.

## Ghi chú
- KHÔNG add lại `catalog_tokens`/`workspace_warehouses` (thay bằng bảng EB-3). lakekeeper tự tạo schema `lakekeeper` khi boot.
- Ngoài EB-1: mini-IdP+clients (EB-2), Authorizer thật+warehouses/roles/grants+authN JWT (EB-3), mgmt+revert egret-polaris (EB-4), UI (EB-5), e2e (EB-6).
