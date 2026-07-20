# Local-User Management gaps + STS cleanup — Thiết kế

> **Trạng thái:** Đã duyệt thiết kế. Bước tiếp: viết plan.
> **Phạm vi:** Sub-project 2/2 của effort STS. Xây trên sub-project 1 (STS broker, #304 `045d0cd`).
> *(Bản tiếng Anh là chuẩn: `docs/en/specs/2026-07-21-egret-local-user-mgmt-design.md`.)*

**Mục tiêu:** Lấp các gap còn lại của quản trị user local + dọn đường Keycloak refresh-token đã chết. Phần lớn local-user CRUD ĐÃ có (#174 + access-control): Access console (`UsersPage`/`GroupsPage`) đã tạo user local, sửa tên, enable/disable (`status`), gán role, quản group membership; và sub-project 1 đã làm `SessionService::validate` reject user bị disable. SP2 lấp 4 gap hẹp.

## 1. Phạm vi (4 gap)
1. **Reset password** cho user local — chưa có cách đổi/reset pw sau khi tạo.
2. **Delete user (hard)** — chưa có `DELETE /users/{id}`; mới chỉ có enable/disable.
3. **Lockout tức thì** — disable hiện chỉ áp ở request KẾ; làm disable/reset/delete revoke session NGAY.
4. **STS cleanup** — bỏ ghi `oidc_refresh_tokens` + scope `offline_access` không còn dùng sau khi xóa Keycloak broker (#304).

## 2. Quyết định approach (đã chốt)
| Quyết định | Lựa chọn |
| --- | --- |
| Delete | **Hard-delete** (khác `disabled`), tx cleanup dependents, + guard self + last-admin. |
| Revoke session | **Active** — seam chung `SessionRepo::revoke_all_for_user` gọi khi disable/reset/delete (lockout tức thì). |

## 3. Thành phần
### 3.1 Seam revoke chung
`SessionRepo::revoke_all_for_user(user_id) -> Result<u64>` = `DELETE FROM sessions WHERE user_id=$1`. Choke point duy nhất cho 3 trigger lockout → "lockout tức thì" là 1 primitive test được.

### 3.2 Reset password
- **Backend:** `POST /api/control-plane/users/{id}/password` `{password}` — gate `user:update`; `404` id lạ; `400` nếu `provider != "local"` (tài khoản SSO không có pw local); `hash_password` → `UserRepo::set_password` → `revoke_all_for_user` (bắt login lại).
- **FE:** action "Reset password" ở `UsersPage` (dialog nhập pw), chỉ hiện cho user `provider=="local"`.

### 3.3 Delete user (hard)
- **Backend:** `DELETE /api/control-plane/users/{id}` — permission mới `user:delete` (`platform_admin` bypass). Guard: `404` id lạ; `400` nếu target là **chính caller**; `400` nếu target là `platform_admin` mà không còn admin active nào khác (last-admin). Thành công → **1 transaction**: `revoke_all_for_user` + xóa `user_groups` + xóa role grants (`access_grants` principal=user) + xóa `oidc_refresh_tokens` + xóa row `users`. Audit-log tham chiếu actor — xác nhận lúc impl là ref denormalized/nullable; nếu FK cứng chặn delete, thêm `ON DELETE SET NULL` cho ref đó (KHÔNG cascade-xóa lịch sử audit).
- **FE:** action "Delete" (destructive, confirm dialog) ở `UsersPage`.

### 3.4 Lockout tức thì (nối seam)
Gọi `revoke_all_for_user` từ `update_user` khi `status` chuyển `disabled`, và từ reset-pw + delete. Guard last-admin áp cho cả **disable một admin** (cùng rủi ro lockout).

### 3.5 STS cleanup
Bỏ đường Keycloak refresh đã chết (không ai đọc sau #304): bỏ block ghi refresh-token trong `oidc_routes.rs` (~209-237) + field `refresh_repo` (~31); bỏ `offline_access` khỏi scope OIDC (`oidc.rs`) + phần capture refresh token; bỏ dựng `OidcRefreshRepo` (`main.rs:~1044`); xóa `oidc_refresh.rs`; migration `DROP TABLE oidc_refresh_tokens` (+ touch `metastore/src/lib.rs`).

## 4. Permission
Thêm `user:delete` vào catalog permission code-defined (nơi `user:create/update/disable` sống). `platform_admin` bypass; route delete gate theo nó.

## 5. Xử lý lỗi
`404` user lạ; `400` self/last-admin/reset-pw-non-local/input sai; `403` thiếu permission. Delete transactional — fail giữa chừng rollback, không bao giờ nửa-xóa.

## 6. Kiểm thử
- **Unit/DB:** `revoke_all_for_user` xóa đúng session của target; `set_password`+revoke; `update_user` disable revoke session; delete xóa user + mọi dependents + rollback khi fail giữa tx; self-guard + last-admin-guard reject `400`.
- **STS cleanup:** OIDC login vẫn OK **không** `offline_access` (tạo session, không ghi `oidc_refresh_tokens`); sửa/bỏ test cũ assert lưu refresh token.
- **Live e2e (kind) — disable→lockout (đinh của SP2):** tạo user local qua console → login as họ → admin `PATCH status=disabled` → assert request kế của user **401 / bị logout** ngay. + reset-pw vô hiệu session cũ; delete → login fail. `local-user-mgmt-kind.spec.ts` mới + recipe just.

## 7. Phân rã task (cho plan)
1. Seam `revoke_all_for_user` + nối vào `update_user` disable.
2. Route reset-pw (`set_password`+revoke) + UI UsersPage.
3. Route delete (`user:delete` + guard self/last-admin + tx cleanup) + UI.
4. STS cleanup (bỏ refresh write + `offline_access` + `OidcRefreshRepo`; drop table).
5. Live e2e disable→lockout (+ reset-pw re-login + delete).

## 8. Ngoài phạm vi
- Revoke từ IdP / SSO back-channel logout (tương lai).
- Soft-delete/archival (đã có `disabled`); delete là xóa thật.
- Bulk import / provisioning user ngoài.
