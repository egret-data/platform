# Catalog Engine = nhúng lakekeeper (thin embed) — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`.
> File này là bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-26 · **Trạng thái:** Thiết kế. Thay phần *catalog store/REST engine* của spec A (A-1), giữ lại auth/RBAC/UI/token/provisioning của A-2/A-3 (đấu lại dây).

## Bối cảnh & quyết định
Mỗi workspace là một data platform tự chứa; pillar Catalog phải production-grade + dùng được độc lập với mọi engine. Thay vì tự viết lại GC/soft-delete/multi-storage/full-REST của lakekeeper, **nhúng lakekeeper core làm thư viện (thin embed)**.

**Khả thi (đã kiểm chứng):** egret và lakekeeper pin **cùng rev fork `iceberg-rust` `bb70173`** → một version `iceberg` duy nhất. lakekeeper license Apache-2.0, có chủ đích nhúng (`service` = các trait, `api` = `ApiContext<S>`, `serve` feature `router`), có sẵn `AllowAllAuthorizer` + `lakekeeper-storage-postgres`.

**Thin embed:** lakekeeper = *catalog engine* (store + REST handler + GC tasks + storage I/O). egret giữ **identity**: token authn per-workspace, RBAC, Catalog UI, provisioning warehouse, và một bridge `Authorizer` mang **scope per-token**. Tenancy: 1 **default project** lakekeeper, **1 warehouse / workspace** (`{prefix}` = slug → warehouse).

## Kiến trúc đích
egret-server mount router lakekeeper tại `/catalog`; state cung cấp: `CatalogStore` (lakekeeper-storage-postgres, schema Postgres riêng), `Authorizer` (bridge → RBAC workspace + token scope — CE-4), `SecretStore` (CE-5), Storage (lakekeeper-io, CE-5), task queues (GC/expire/soft-delete — CE-6). Trước router: middleware egret `require_bearer` (token → principal + scope — CE-3). Provisioning: tạo workspace → tạo warehouse lakekeeper (CE-2).

**Số phận spec A:** *Thay thế* schema `catalog_*` + store/handlers + migration 0023 (~40%). *Giữ + đấu lại* token auth, RBAC perms, internal API + UI, provisioning hook (~60%).

## Phân rã CE-1..8
- **CE-1** (spec này) — Spike + nền tảng: lakekeeper build được, migrator chạy schema riêng, mount router AllowAll, 1 REST call round-trip.
- **CE-2** Tenancy + provisioning (project mặc định, warehouse per-workspace).
- **CE-3** Authn bridge + per-token scope (`permissions[]`, `namespace_scope[]`).
- **CE-4** Authz bridge (`Authorizer` = RBAC + token scope, chặn trước khi sign).
- **CE-5** Storage profiles + secrets.
- **CE-6** GC/task workers.
- **CE-7** UI + internal API trỏ sang dữ liệu lakekeeper.
- **CE-8** Bỏ MVP catalog cũ.

## CE-1 chi tiết (rút gọn)
Deps git-pin cùng iceberg rev; chạy migrator lakekeeper trong **Postgres schema riêng** (`search_path`) tránh đụng `_sqlx_migrations` (fallback: DB riêng); dựng state + mount `new_v1_full_router` với `AllowAllAuthorizer`; provision 1 default project + 1 warehouse tĩnh. **Acceptance:** chạy 2 migrator + provision + REST round-trip (config → create namespace → create table → load) qua store lakekeeper + check `cargo tree -d` chỉ một `iceberg`.

## Rủi ro / ngoài phạm vi
Migrator coexistence (CE-1 chốt); per-token scope phải chặn trước S3 sign (CE-4); pin lakekeeper rev (nâng = bước có chủ đích). **Bỏ:** projects/OpenFGA/events/contract-verify của lakekeeper (egret đã có tương đương hoặc chưa cần). Notebook/storage/Engineering là epic riêng.
