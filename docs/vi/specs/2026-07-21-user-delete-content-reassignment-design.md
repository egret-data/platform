# User-Delete Content Reassignment — Thiết kế

> **Trạng thái:** Đã duyệt thiết kế. Bước tiếp: viết plan.
> **Xây trên:** #305 (`UserRepo::hard_delete` + `DELETE /users/{id}` + safety-net `23503`→409 đã có).
> *(Bản tiếng Anh là chuẩn: `docs/en/specs/2026-07-21-user-delete-content-reassignment-design.md`.)*

**Mục tiêu:** Cho hard-delete thành công với user có sở hữu content. Hiện `DELETE /users/{id}` trả 409 khi user sở hữu dashboard/notebook/dataset/… (FK `REFERENCES users(id)` chặn xóa row). Xử lý từng FK để delete thành công mà giữ content.

## 1. Bức tranh FK (25 cột tham chiếu `users(id)`, 3 nhóm)
- **Nhóm A — đã `ON DELETE CASCADE` (0 việc):** sessions, oidc_identities, bookmarks, kernels, notebook_kernels, access_requests.requester_id, user_groups. Tự dọn khi xóa user. (`oidc_refresh_tokens` đã drop ở #305.)
- **Nhóm B — cột attribution nullable → `ON DELETE SET NULL` (~14):** dashboards/charts/datasets/folders/sqllab `.created_by`, databases `.created_by`/`.granted_by`, catalog `.granted_by`, tags `.created_by`/`.assigned_by`, catalog_tag_targets `.assigned_by`, workspace_rbac `.created_by`/`.added_by`, classification_policies `.created_by`, engine_workspaces `.assigned_by`, engine `.user_id`, access_requests `.decided_by`. Chỉ ghi "ai làm"; null đi mất attribution nhưng giữ content. (`policy_audit.actor` KHÔNG trong scope — bảng đã drop ở mig 0063.)
- **Nhóm C — cột sở hữu thật:** `notebooks.owner_user_id` là `NOT NULL` — notebook phải có owner, không SET NULL được.

## 2. Quyết định (đã chốt)
| Quyết định | Lựa chọn |
| --- | --- |
| Cơ chế Nhóm B | **Migration `ON DELETE SET NULL`** — DB tự null cột attribution khi xóa user (nguồn sự thật duy nhất); `hard_delete` gọn. |
| Nhóm C (notebooks) | **Reassign cho admin xóa** — notebook của target → `owner_user_id = <caller admin>`; content sống, delete thành công. |

## 3. Thành phần
### 3.1 Migration `0093_user_delete_set_null.sql`
Với mỗi cột Nhóm B, drop-và-add lại FK với `ON DELETE SET NULL`:
```sql
ALTER TABLE <schema.table>
  DROP CONSTRAINT <table>_<col>_fkey,
  ADD CONSTRAINT <table>_<col>_fkey
    FOREIGN KEY (<col>) REFERENCES users(id) ON DELETE SET NULL;
```
Impl liệt kê từng cột với **tên constraint thật** (Postgres mặc định `<table>_<col>_fkey`) + **schema qualifier thật** (vài bảng vào schema ở mig 0067, vd `analytics.dashboards`), verify với schema live trước khi viết `ALTER`. `touch crates/egret-metastore/src/lib.rs` để re-embed.

### 3.2 `UserRepo::hard_delete` — reassign notebooks
Signature mới `hard_delete(target: UserId, reassign_notebooks_to: UserId)`. Trong tx sẵn có, trước khi xóa user:
```rust
sqlx::query("UPDATE notebooks SET owner_user_id = $2 WHERE owner_user_id = $1")
    .bind(target.0).bind(reassign_to.0).execute(&mut *tx).await?;
```
Còn lại giữ nguyên: `access_grants` xóa tường minh (polymorphic, không FK); `DELETE FROM users` giờ kích hoạt cascade Nhóm A + set-null Nhóm B tự động. Xóa `sessions`/`user_groups` tường minh giữ lại (belt-and-suspenders, vốn CASCADE).

### 3.3 Handler `delete_user`
Truyền caller làm reassignee: `st.users.hard_delete(user_id, p.user_id)`. Self-guard sẵn có đảm bảo caller ≠ target.

## 4. Safety-net `23503`→409
Catch FK-violation giữ lại nhưng thành **guard hiếm khi bắn**, không phải đường thường: chỉ bắn khi một bảng TƯƠNG LAI thêm FK NOT-NULL vào `users(id)` mà không có cascade/set-null/reassign. Đổi message: *"cannot delete: this user owns content in a table that isn't auto-handled on delete — a new FK to users(id) needs an ON DELETE rule."*

## 5. Xử lý lỗi
Mọi statement trong 1 tx; fail bất kỳ (vd admin reassignee bị xóa đồng thời) → rollback toàn bộ. Migration drop-add từng bảng atomic; tên constraint sai → migration fail rõ, nên verify tên trước.

## 6. Kiểm thử
- **DB integration (`hard_delete`):** xóa user tạo **dashboard** → 204, dashboard sống với `created_by IS NULL`; xóa user sở hữu **notebook** → 204, notebook sống với `owner_user_id = <admin reassignee>`; assertion dọn cũ (sessions/user_groups/access_grants/user gone) vẫn đúng. **Đổi test `deleting_a_content_owner_conflicts_not_500`** — dashboard-owner giờ trả 204 chứ không 409; repurpose thành `deleting_a_content_owner_reassigns_and_succeeds`.
- **Live e2e (tùy chọn):** user tạo notebook qua API → admin xóa họ → 204 + notebook còn (owner=admin). Tùy chọn vì DB tests là coverage chính.

## 7. Phân rã task (cho plan)
1. Migration `0093` — `ON DELETE SET NULL` cho ~14 FK Nhóm B (tên constraint + schema thật).
2. `hard_delete` reassign notebook (signature + `UPDATE` + handler truyền caller) + đổi tests + đổi message 409.
3. (Tùy chọn) Live e2e: content-owner delete → 204 + notebook sống.

## 8. Ngoài phạm vi
- Flow self-service "chuyển hết content của tôi cho X" (chỉ làm admin-driven delete cleanup).
- Đổi hành vi `disabled` (giữ nguyên).
- STS follow-up khác (IdP revocation / back-channel logout).
