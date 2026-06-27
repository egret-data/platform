# DB connections → per-workspace — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-27-db-connections-per-workspace-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-27 · **Branch:** `feat/db-per-workspace` (off main). **Epic A** của chương trình "DB-per-workspace + Secrets"; Epic B (Secrets) làm sau và phụ thuộc epic này.

## Mục tiêu
Thay model global **register-then-grant** (ADR-12) bằng **DB connection per-workspace self-service** (ADR-19), mirror Storage registry. Mỗi workspace sở hữu DB connection của mình, do workspace admin quản lý ở zone **MANAGE**. **Không đụng secret** (DB giữ `secret_id → secrets` inline) — named secret để Epic B.

## Hiện trạng
`databases` là bảng **global** (không `workspace_id`); platform admin đăng ký + grant cho workspace qua `workspace_database_grant`; workspace thấy qua `/api/me/databases`. Query path: `egret-engine authorize()` check grant-join; `job.rs` resolve qua `get`+`open_secret`. Dev DB: **587 databases nhưng chỉ 2 có grant** (mỗi cái 1 ws, không fan-out); ~585 là rác test (0 grant).

## Quyết định
1. **Thêm `workspace_id` vào `databases`** (không tạo bảng mới → giữ `databases.id` PK nên `queries.database_id` FK + engine/bi resolve-by-id chạy nguyên). Index `databases(workspace_id)`.
2. **Backfill** từ grant (mỗi DB có grant → đúng 1 ws). **Orphan để NULL = legacy/unreachable, lọc khỏi mọi query workspace** (non-destructive). Cleanup destructive tuỳ chọn, không tự chạy.
3. **Drop** `workspace_database_grant` + admin/grant routes.
4. **RBAC** `workspace:manage_members` (như Storage); bỏ platform `database:manage` cho routes mới.
5. **Routes mới** `/ws/{ws}/databases` (mirror storages): list / create / test(unsaved) / {id} detail-update-delete / {id}/test(saved) — `require_workspace_admin`, scope ws (cross-ws id → 404 IDOR). `DatabaseRepo` workspace-scoped; `grants.rs` xoá. `egret-engine authorize()` → check ownership `DB.workspace_id == ctx.workspace_id`.
6. **FE**: `DatabasesPage` mới ở MANAGE (mirror StoragePage: bảng + dialog engine/host/port/options/password + Test connection + Delete + 403). Sidebar thêm "Databases" dưới MANAGE. `src/api/databases.ts` hooks mirror storage.ts. SQL Lab database-picker đọc `/ws/{ws}/databases`.
7. **Migration**: ALTER + index + backfill + drop grant; touch `egret-metastore/src/lib.rs` (re-embed).

## Test
BE (DB+Trino): CRUD scope-ws, cross-ws 404 IDOR, non-admin 403, test-conn ok/error, query authorize theo ownership, migration backfill. FE (mock) DatabasesPage. Manual Trino integration test + full local CI trước admin-merge.

## Rủi ro
Luồng query (`authorize`/SQL Lab picker) phải chuyển ownership sạch; orphan non-destructive; chạy manual Trino + full CI local trước merge.

## Ngoài phạm vi
Named-secret registry + rewire form (Epic B); chia sẻ DB cross-workspace (bỏ grant); Ranger; GCS/Azure.
