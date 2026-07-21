# User-Delete Content Reassignment — Kế hoạch (tóm tắt)

> **Bản chi tiết (chuẩn, code từng bước):** `docs/en/plans/2026-07-21-user-delete-content-reassignment.md`
> **Spec:** `docs/vi/specs/2026-07-21-user-delete-content-reassignment-design.md`
> **Thực thi:** subagent-driven-development.

**Mục tiêu:** `DELETE /users/{id}` thành công cho user có content — auto-null 14 FK attribution nullable + reassign `notebooks.owner_user_id` (NOT NULL) cho admin xóa.

**Danh sách FK (verify từ schema live — tên + schema chính xác):**
- Group B → SET NULL (14, nullable): `analytics.{charts,dashboards,datasets,folders,saved_queries}.created_by`, `engine_mgmt.engine_workspaces.assigned_by`, `public.access_requests.decided_by`, `public.catalog_tag_assignments.assigned_by`, `public.classification_policies.created_by`, `public.tag_assignments.assigned_by`, `public.tags.created_by`, `public.workspaces.created_by`, `query.databases.created_by`, `query.queries.user_id`. Tên constraint = `<table>_<col>_fkey`.
- Group C → reassign (1, NOT NULL): `analytics.notebooks.owner_user_id`.

## Task
1. **Migration `0093_user_delete_set_null.sql`** — 14 `ALTER … DROP CONSTRAINT … ADD … ON DELETE SET NULL`. Verify lại tên constraint từ `pg_constraint` trước khi viết. `touch metastore/src/lib.rs`. Test: seed user + dashboard(created_by=user) → `DELETE FROM users` → assert `dashboards.created_by IS NULL` (`#[sqlx::test(migrations="../../migrations")]`; điền đủ cột NOT NULL — soi `\d`).
2. **`hard_delete` reassign notebooks** — đổi signature `hard_delete(target, reassign_notebooks_to)`, thêm `UPDATE analytics.notebooks SET owner_user_id=$2 WHERE owner_user_id=$1` đầu tx (chú ý schema `analytics.`); handler truyền `p.user_id`; giữ catch `23503`→409 làm safety-net (đổi message). Sửa unit test `hard_delete_removes_user_and_dependents` (thêm arg reassignee). **Đổi** test `deleting_a_content_owner_conflicts_not_500` → `deleting_a_content_owner_reassigns_and_succeeds`: user sở hữu dashboard+notebook → DELETE → **204**, dashboard `created_by IS NULL`, notebook `owner_user_id=<admin>`, user gone.
3. **(Tùy chọn) Live e2e** — `local-user-mgmt-kind.spec.ts`: user tạo notebook qua API → admin xóa → 204 + notebook còn (owner=admin).

## Verify cuối
fmt + clippy(`-D warnings`) + `cargo test -p egret-auth -p egret-controlplane` + pg_constraint query (14 cột giờ SET NULL, chỉ notebooks còn plain FK) + (tùy chọn) `just e2e-kind-local-user` xanh.
