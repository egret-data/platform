# Control-Plane Polaris — Plan 2: chuyển Storage + Secrets lên control plane (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p2-storage-secrets-control-plane.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P2/6** (sau P1). Thực thi qua subagent-driven-development.

**Mục tiêu:** Storage connection thành **global** + rebuild quản Storage/Secrets ở **control plane** (chỉ `platform_admin`) — vì P1 đã xoá route cũ (nằm trong `browse.rs` đã bị xoá). Workspace không quản Storage/Secrets nữa.

**Cách làm:** storage → bảng `storage_connections` global (bỏ `workspace_id`); `workspace_secrets` **hỗ trợ global** (nullable `workspace_id`, NULL = secret control-plane) để storage tham chiếu secret global còn `egret-database` giữ secret workspace như cũ. Thêm router control-plane gate `platform_admin` + UI; bỏ trang Storage/Secrets workspace.

## Bối cảnh
- P1 đã xoá route storage+secrets (trong `browse.rs`). **Store còn**: `egret-catalog::storage_conn` (bảng `workspace_storages`, có `workspace_id`, secret_ref…) + `egret-secrets::WorkspaceSecretStore` (bảng `workspace_secrets`, `workspace_id NOT NULL`).
- `workspace_secrets` **dùng chung** với `egret-database` (workspace-scoped) → không được phá; databases giữ nguyên.
- Gate: control-plane không có workspace context → dùng `egret_auth::Auth(principal)` + check `platform_roles` chứa `"platform_admin"`. Frontend `RequireGlobalAdmin`.
- Migration: P1 thêm 0048 → tiếp **0049**; `touch metastore`.

## Task
- **T1 (migration 0049):** storage: `DROP COLUMN workspace_id` + rename `workspace_storages`→`storage_connections` + unique(name). secrets: `ALTER workspace_id DROP NOT NULL` + partial unique index tên cho global. Migrate + verify.
- **T2 (storage store global):** bỏ `workspace_id` khỏi struct + create/list/get/update/delete (bảng `storage_connections`), map dup-name → AlreadyExists. Test global.
- **T3 (secrets có scope global):** `create/list/get_meta/update/delete` nhận `Option<Uuid>` (None=global, bind NULL, `WHERE workspace_id IS NOT DISTINCT FROM $1`); sửa caller `egret-database` → `Some(ws)`. Test global + workspace tách biệt.
- **T4 (router control-plane):** crate `egret-controlplane` (`storage.rs`/`secrets.rs`/`auth.rs`): `require_platform_admin(principal)`; route `/api/control-plane/storage` + `/secrets` (CRUD, gate Auth+platform_admin, secret global scope None); wire state trong main.rs + nest ở egret-server (sau CSRF). Test: non-admin → 403.
- **T5 (UI):** `controlPlane.ts` client; `ControlPlaneStorage.tsx` + `ControlPlaneSecrets.tsx` (RequireGlobalAdmin, mirror UX cũ); thêm route control-plane + bỏ route workspace storage/secrets + `git rm StoragePage/SecretsPage`; đổi nav. typecheck/lint/build.
- **T6 (verify):** fmt + clippy --workspace + test --workspace; frontend; manual: admin tạo secret+storage global, non-admin 403.

## Ghi chú
- Databases không ảnh hưởng (secret workspace-scoped `Some(ws)` giữ; chỉ thêm scope global `None`).
- P1 đã xoá route storage/secrets cũ → P2 viết mới ở control-plane (handler mỏng), không "port".
- Ngoài P2: `egret-polaris` (P3), routes catalog+RBAC+assignment (P4), UI catalog control-plane (P5), Polaris compose/e2e (P6).
