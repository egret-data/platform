# Local-User Management gaps + STS cleanup — Kế hoạch (tóm tắt)

> **Bản chi tiết (chuẩn, code từng bước):** `docs/en/plans/2026-07-21-egret-local-user-mgmt.md`
> **Spec:** `docs/vi/specs/2026-07-21-egret-local-user-mgmt-design.md`
> **Thực thi:** subagent-driven-development.

**Mục tiêu:** lấp 4 gap quản trị user local + dọn đường Keycloak refresh đã chết (#304). Phần lớn CRUD/UI đã có (#174); `SessionService::revoke_all_for_user` + `CpState.sessions` cũng đã có → chỉ nối vào.

## Task
1. **Lockout tức thì.** Nối `revoke_all_for_user` (sẵn có) vào `update_user` khi disable + guard `guard_not_last_admin` (không tự disable/delete mình, không disable/delete admin cuối). Test DB.
2. **Reset password.** `UserRepo::set_password` mới + route `POST /users/{id}/password` (gate `user:update`, 400 nếu provider≠local, hash + revoke sessions) + action UI ở UsersPage (chỉ user local). Test DB + route.
3. **STS cleanup.** Bỏ ghi refresh trong `oidc_routes.rs` + field `refresh_repo`; bỏ `offline_access` + capture refresh trong `oidc.rs`; xóa `oidc_refresh.rs`; bỏ dựng `OidcRefreshRepo` (`main.rs`); migration `DROP TABLE oidc_refresh_tokens` (+ touch metastore/lib.rs). Grep xác nhận không còn ref sống. **Làm TRƯỚC task 4** để tx delete không đụng bảng đã drop.
4. **Hard-delete.** Perm `user:delete` (rbac.rs); `UserRepo::hard_delete` (1 tx: sessions + user_groups + access_grants + row users; audit KHÔNG xóa — kiểm FK `REFERENCES users`, nếu chặn thì `ON DELETE SET NULL`); route `DELETE /users/{id}` + `guard_not_last_admin`; nút Delete (confirm) UI. Test DB + guards.
5. **Live e2e (kind).** `local-user-mgmt-kind.spec.ts` + `just e2e-kind-local-user`: tạo user local → user login → admin PATCH status=disabled → request kế của user **401** (lockout tức thì).

## Verify cuối
fmt + clippy(`-D warnings`) + `cargo test --workspace` + FE lint/tsc/build + rebuild+redeploy + `just e2e-kind-local-user` xanh + grep không còn `oidc_refresh`/`offline_access` sống.
