# B2-flip — Xóa 2 bảng grant legacy, hợp nhất trên access_grants

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b2-flip-drop-legacy-grant-tables-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** B1a–B1b-final (PDP + PEP), B2-1a/1b/2/3 (object scope, sharing, notebooks, role hierarchy + deny).
>
> Data-model flip bị hoãn. `access_grants` là read-model từ B1a, được giữ đồng bộ với 2 bảng ghi legacy (`platform_role_assignments`, `workspace_members`) bằng trigger. B2-flip biến `access_grants` thành **single source of truth**: repoint mọi writer/reader sang nó, rồi drop trigger + 2 bảng legacy.

---

## 1. Mục tiêu & scope

`access_grants` (migration 0068) là read-model của PDP; 2 bảng legacy vẫn là đường ghi, mirror vào access_grants qua trigger 0068/0069. Dual-write này là mảnh cuối của data-model tiền-Effort-B. B2-flip bỏ nó: **access_grants là kho grant duy nhất.**

Đây là **refactor behavior-preserving**. Mọi public method giữ signature (`RoleRepo::assign/unassign`, `Authorizer::replace_platform_roles/workspace_roles_for`, `MemberRepo::add/remove/list`, derive của `SessionService`, `WorkspaceRepo::list_for_principal`); chỉ đổi SQL bên trong — từ bảng legacy sang access_grants. **Nội dung** access_grants không đổi (flip đổi *ai ghi*, không đổi *cái gì trong đó*) → **PDP decision matrix không đổi**.

**Trong scope:** repoint mọi writer/reader platform-grant và workspace-grant sang access_grants; migration (0073) re-backfill phòng vệ + drop 2 trigger + function + 2 bảng legacy; sửa test nào seed/đọc bảng legacy trực tiếp.

**Ngoài scope:** áp closure role-hierarchy (B2-3) vào `Principal.permissions` (FE gating giữ flat/own-perms/platform-only — behavior-preserving; việc riêng); B3 (per-user data-plane delegation); B4 (audit_log); mọi thay đổi đường object-grant/share (đã trên access_grants).

**Quyết định chốt khi brainstorm:** trọn 1 increment (platform + workspace cùng lúc); **giữ 1-role-ws-mỗi-member** (`MemberRepo::add` xóa ws grants cũ của principal ở ws đó trước khi insert — khớp PK cũ của workspace_members); **drop dứt điểm** 2 bảng legacy ở 0073 (sau re-backfill phòng vệ — data đã mirror đủ); giữ `Principal.permissions` flat.

## 2. Hiện trạng (cái gì di chuyển)

**`platform_role_assignments (principal_type, principal_id, role_id)`** — grants scope platform.
- Writers: `RoleRepo::assign`/`unassign` (`egret-auth/src/roles.rs`, Access console); `Authorizer::replace_platform_roles` (`egret-workspace/src/rbac.rs`); bootstrap admin (`egret-bin/src/main.rs`).
- Readers: `SessionService::platform_roles` + `permissions` (`egret-auth/src/session.rs` → `Principal`); `RoleRepo::assignees_of`/`assignees_named`/`roles_of`/`count_platform_admins`; check idempotency bootstrap.

**`workspace_members (workspace_id, principal_type, principal_id, role, added_by, added_at)`** — grants scope workspace; `role` là NAME; PK `(workspace_id, principal_type, principal_id)` → 1 role/member.
- Writers: `MemberRepo::add`/`remove` (`egret-workspace/src/members.rs`).
- Readers: `Authorizer::workspace_roles_for` (WsContext display role); `MemberRepo::list`; `WorkspaceRepo::list_for_principal` (`egret-workspace/src/workspaces.rs`, "my workspaces").

**Triggers (0068/0069):** `sync_platform_grant_trg` (AFTER INSERT/DELETE trên platform_role_assignments) + `sync_workspace_grant_trg` (AFTER INSERT/UPDATE/DELETE trên workspace_members) mirror vào access_grants. Drop bởi 0073.

**`access_grants`** là đích: `(principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, granted_by, created_at)`, UNIQUE `(principal_type, principal_id, role_id, scope_type, scope_id, effect)`, `role_id` FK `ON DELETE CASCADE`. Platform grant = `scope_type='platform'`, `scope_id=NULL`. Workspace grant = `scope_type='workspace'`, `scope_id=workspace_id`, `role_id` = id của ws role.

## 3. Repoint writers

**Platform:**
- `RoleRepo::assign(ptype, pid, role_id)` → INSERT access_grants (scope_type='platform', scope_id=NULL) ON CONFLICT DO NOTHING (effect default `allow`). `unassign` → DELETE khớp scope_type='platform'.
- `Authorizer::replace_platform_roles(user, names)` → trong tx: DELETE platform grants của user, rồi mỗi name INSERT…SELECT id FROM roles WHERE scope='platform' AND name=$2.
- Bootstrap: INSERT…SELECT id FROM roles WHERE scope='platform' AND name='platform_admin'.

**Workspace:**
- `MemberRepo::add(ws, ptype, pid, role_name, by)` → validate role_name (check `WORKSPACE_ROLES` sẵn); resolve role_id; trong tx: **DELETE mọi ws grant của (principal, ws)** [1-role/member] rồi INSERT (scope_type='workspace', scope_id=ws, granted_by=by) ON CONFLICT DO NOTHING.
- `MemberRepo::remove(ws, ptype, pid)` → DELETE ws grants của (principal, ws).

## 4. Repoint readers

Đều JOIN `roles` lấy NAME khi cần. Group union (`principal_id IN (SELECT group_id FROM user_groups WHERE user_id=$1)`) giữ y nguyên ở đâu đang có.

- `SessionService::platform_roles` → `access_grants ag JOIN roles r ... WHERE ag.scope_type='platform' AND (<user-or-group>)`.
- `SessionService::permissions` → `access_grants ag JOIN role_permissions rp ... WHERE ag.scope_type='platform' AND (<user-or-group>)`. **Flat/own-perms/platform-only, không đổi** — không áp closure hierarchy (ngoài scope).
- `RoleRepo::assignees_of`/`assignees_named`/`roles_of` → access_grants scope_type='platform' thay platform_role_assignments (cùng join users/groups/roles).
- `RoleRepo::count_platform_admins` → `count(*) FROM access_grants ag JOIN roles r ... WHERE ag.scope_type='platform' AND r.name='platform_admin'`. **Guard last-platform-admin giữ trên nguồn mới.**
- `Authorizer::workspace_roles_for(user, ws)` → `access_grants ag JOIN roles r ... WHERE ag.scope_type='workspace' AND ag.scope_id=$ws AND (<user-or-group>)`.
- `MemberRepo::list(ws)` → `SELECT ag.principal_type, ag.principal_id, r.name AS role FROM access_grants ag JOIN roles r ... WHERE ag.scope_type='workspace' AND ag.scope_id=$ws`.
- `WorkspaceRepo::list_for_principal(user)` (nhánh non-admin) → `JOIN access_grants ag ON ag.scope_id=w.id AND ag.scope_type='workspace'` thay `workspace_members wm`, cùng predicate user-or-group.

## 5. Migration 0073 (drop)

```
-- Re-backfill phòng vệ (self-heal drift lịch sử), rồi drop trigger + function, rồi 2 bảng legacy.
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT principal_type, principal_id, role_id, 'platform', NULL FROM platform_role_assignments
  ON CONFLICT DO NOTHING;
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
  FROM workspace_members wm JOIN roles r ON r.scope='workspace' AND r.name=wm.role
  ON CONFLICT DO NOTHING;
DROP TRIGGER sync_platform_grant_trg ON platform_role_assignments;
DROP TRIGGER sync_workspace_grant_trg ON workspace_members;
DROP FUNCTION sync_platform_grant();
DROP FUNCTION sync_workspace_grant();
DROP TABLE platform_role_assignments;
DROP TABLE workspace_members;
```

`touch crates/egret-metastore/src/lib.rs` sau khi thêm (gotcha sqlx re-embed). Không lo orphan: workspaces chỉ archive (không hard-delete), và `access_grants.role_id` đã cascade khi xóa role.

## 6. WsContext / object-guest

Extractor `WsContext` đã admit object-guest qua `has_object_grant_in_workspace` (đọc access_grants, lọc `effect='allow'`) + resolve display role member qua `workspace_roles_for`. Sau flip, `workspace_roles_for` đọc ws-grants access_grants: member resolve ws role; object-guest không có ws grant → empty → sentinel `object_guest` sẵn có. Phân biệt member-vs-object-guest không đổi.

## 7. Testing

- **Behavior-preserving:** suite `egret-auth` / `egret-workspace` / `egret-controlplane` hiện có pass. Test nào seed/đọc `platform_role_assignments` / `workspace_members` trực tiếp → repoint sang access_grants (hoặc dùng public method) — đây là phần lớn churn.
- **Writers:** assign/unassign ghi/xóa platform grant trong access_grants; replace_platform_roles thay tập; `MemberRepo::add` insert ws grant VÀ xóa ws grant cũ của principal ở ws đó (1-role/member — assert re-add role khác để lại đúng 1 dòng); remove xóa.
- **Readers:** platform_roles/permissions derive từ access_grants; count_platform_admins đếm platform_admin grants ở đó (assert last-admin guard vẫn nổ); workspace_roles_for + list đọc ws grants; list_for_principal qua access_grants.
- **Migration:** test assert `platform_role_assignments` + `workspace_members` không còn tồn tại sau `migrate()`, và grants có sẵn còn nguyên trong access_grants.
- **PDP parity:** matrix regression B1a + object-scope + B2-3 suites pass không đổi (flip không đổi nội dung access_grants).
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build` (FE không ảnh hưởng — DTO members/roles không đổi; regen chỉ khi openapi đổi, mà không nên đổi), openapi current, adversarial security review.

## 8. Bảo mật & an toàn

- **Không bề mặt quyền mới** — signatures + semantics giữ; flip bỏ rủi ro dual-write drift (single source of truth).
- **Không mất grant** — re-backfill phòng vệ ở 0073 chạy trước khi drop; data đã trigger-mirror từ 0068.
- **Giữ 1-role/member** — add xóa-rồi-insert trong tx, member không tích nhiều ws role ngoài ý (khớp PK cũ).
- **Giữ guard last-platform-admin** — count_platform_admins đếm platform_admin grants access_grants; guard chặn xóa admin cuối không đổi hành vi.
- **Giữ idempotency bootstrap** — check "đã có platform_admin" đọc access_grants.
- **Ghi transactional** — replace_platform_roles + MemberRepo::add delete+insert trong 1 tx nên reader đồng thời không thấy trạng thái rỗng trung gian.

---

*Design v1.0 (2026-07-11). Biến access_grants thành kho grant duy nhất: repoint mọi writer/reader grant platform/workspace khỏi 2 bảng legacy, re-backfill phòng vệ, drop trigger 0068/0069 + platform_role_assignments + workspace_members. Behavior-preserving — PDP parity không đổi; Principal.permissions giữ flat.*
