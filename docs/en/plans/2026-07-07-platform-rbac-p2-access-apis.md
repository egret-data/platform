# Platform RBAC P2 — Access APIs — Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Backend CRUD + assignment APIs for Users, Groups, Roles, and the Permission catalog, on the existing schema, each guarded by its P1 permission, with lock-out and seeded-role safety guards.

**Architecture:** New `RoleRepo` (egret-auth) for roles/role_permissions/platform_role_assignments; small additions to `UserRepo`/`GroupRepo`; a new `access` module in egret-controlplane exposing the endpoints, mounted in `control_plane_router`.

**Tech:** Rust (axum, sqlx). Follow existing patterns: `crates/egret-auth/src/users.rs` / `groups.rs` (repo style), `crates/egret-controlplane/src/secrets.rs` (module/router/handler style, `require_permission(&p, "…")?`). `CpState` (lib.rs) exposes `users`, `groups`, `authz`; add `roles: RoleRepo`.

**Spec:** `docs/en/specs/2026-07-07-platform-rbac-admin-design.md`. **Constraints:** DB tests on `egret_test` only; `cargo fmt --all`; touch metastore lib.rs if a migration is added (none expected in P2 — schema exists); no `cargo build -p egret-controlplane` alone.

---

### Task 1: `RoleRepo` (egret-auth)

**Files:** Create `crates/egret-auth/src/roles.rs`; register in `crates/egret-auth/src/lib.rs`.

Struct + methods (follow `GroupRepo` style; `db: Db`):
```rust
pub struct RoleRow { pub id: Uuid, pub scope: String, pub name: String, pub description: String }
pub struct RoleDetail { pub role: RoleRow, pub permissions: Vec<String>, pub assignee_count: i64 }

impl RoleRepo {
    // reads
    async fn list(&self) -> Result<Vec<RoleRow>>;
    async fn get(&self, id: Uuid) -> Result<Option<RoleRow>>;
    async fn permissions_of(&self, id: Uuid) -> Result<Vec<String>>;
    async fn assignees_of(&self, id: Uuid) -> Result<Vec<(String, Uuid)>>; // (principal_type, principal_id)
    async fn roles_of(&self, ptype: &str, pid: Uuid) -> Result<Vec<RoleRow>>; // for user/group detail
    async fn count_platform_admins(&self) -> Result<i64>; // distinct principal assignments to platform_admin
    // writes
    async fn create(&self, scope: &str, name: &str, description: &str) -> Result<Uuid>;
    async fn update_description(&self, id: Uuid, description: &str) -> Result<()>;
    async fn delete(&self, id: Uuid) -> Result<()>;
    async fn set_permissions(&self, id: Uuid, perms: &[String]) -> Result<()>; // replace-all in a tx
    async fn assign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()>; // ON CONFLICT DO NOTHING
    async fn unassign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()>;
}
```
- Seeded role names: `["platform_admin","data_steward","ws_admin","ws_editor","ws_viewer"]` — expose `pub fn is_seeded(name: &str) -> bool`.
- `count_platform_admins`: `SELECT count(*) FROM platform_role_assignments pra JOIN roles r ON r.id=pra.role_id WHERE r.name='platform_admin'`.
- Add `pub roles: RoleRepo` to `CpState` (lib.rs) + construct it where `users`/`groups` are built (egret-bin state assembly).
- **Test (DB-gated):** create role → set_permissions (validated) → assign to a user + group → assignees_of/roles_of/permissions_of/count_platform_admins reflect it → unassign → delete. Self-skip without DATABASE_URL.
- Commit `feat(auth): RoleRepo (roles, role_permissions, platform_role_assignments)`.

### Task 2: `UserRepo` + `GroupRepo` additions

**Files:** `crates/egret-auth/src/users.rs`, `crates/egret-auth/src/groups.rs`.
- `UserRepo::set_status(id, status: &str)` (`active`/`disabled`); `UserRepo` already has `list`, `insert_local`, `update_profile`.
- `GroupRepo`: `create_local(name) -> Uuid` (source `'local'`), `rename(id, name)`, `delete_local(id)` (only where `source='local'` — guard in SQL: `DELETE … WHERE id=$1 AND source='local'`, return rows-affected so the handler can 409 on oidc), `add_member(group_id, user_id)`, `remove_member(group_id, user_id)`, `members_of(group_id) -> Vec<Uuid>`, and a `list_with_meta` returning `(Group, member_count)`.
- **Tests:** extend the egret-auth DB tests for the new methods (create local group + member add/remove; set_status). DB-gated.
- Commit `feat(auth): UserRepo.set_status + GroupRepo local CRUD/membership`.

### Task 3: `access` module — Users + Groups endpoints

**Files:** Create `crates/egret-controlplane/src/access/mod.rs` (+ submodules if it grows); register `mod access;` + `.merge(access::router())` in `lib.rs`.

Endpoints (all under `/control-plane`, `Auth(p)` + `require_permission`):
- `GET /users` → `user:read` — list users, each with `roles: [name]` + `groups: [name]` (join via RoleRepo.roles_of + user_groups).
- `POST /users` → `user:create` — `{email, name, password}` → `UserRepo::insert_local` (hash the password with the same hasher the login path uses — find it in egret-auth).
- `PATCH /users/{id}` → `user:update` — `{name?}`; a `status` change to `disabled` requires `user:disable` (check that permission specifically when disabling).
- `POST /users/{id}/roles` → `user:assign_role` — `{roleId}` → `RoleRepo::assign("user", id, roleId)`.
- `DELETE /users/{id}/roles/{roleId}` → `user:assign_role` — **last-admin guard**: if the role is `platform_admin` and `count_platform_admins()==1`, return `CpError::Conflict` ("cannot remove the last platform admin"); else `unassign`.
- `GET /groups` → `group:read` — list groups (name, source, member_count, roles).
- `POST /groups` → `group:create` — `{name}` local.
- `PATCH /groups/{id}` → `group:update` — `{name}` (local only; 409 if oidc).
- `DELETE /groups/{id}` → `group:delete` — local only (409 if oidc / rows-affected 0).
- `POST /groups/{id}/members` → `group:manage_members` — `{userId}` (local only).
- `DELETE /groups/{id}/members/{userId}` → `group:manage_members`.
- `POST /groups/{id}/roles` / `DELETE /groups/{id}/roles/{roleId}` → `user:assign_role` — assign/unassign a platform role to a group (same last-admin guard).

Add a `CpError::Conflict` variant if missing (409). DTOs as needed. Commit `feat(controlplane): access module — users & groups management endpoints`.

### Task 4: `access` module — Roles + Permissions endpoints

**Files:** same module.
- `GET /access/roles` → `role:read` — list roles with `#permissions` + `#assignees` (RoleDetail). (Mount under `/access/roles` to avoid any collision with existing `/roles` routes.)
- `POST /access/roles` → `role:create` — `{scope, name, description}` (scope `platform`|`workspace`).
- `PATCH /access/roles/{id}` → `role:update` — `{description}` (name/scope immutable; if `is_seeded(name)` you may still edit description).
- `DELETE /access/roles/{id}` → `role:delete` — **seeded guard**: 409 if `is_seeded(name)`.
- `PUT /access/roles/{id}/permissions` → `role:assign_permission` — `{permissions: [key]}` → validate every key with `egret_core::rbac::is_known` (400 on unknown); `RoleRepo::set_permissions`. (For `platform_admin`, permissions are irrelevant due to bypass — still allow editing but it's a no-op functionally; optionally 409 to avoid confusion. Choose: allow but note.)
- `GET /permissions` → `role:read` — return `egret_core::rbac::PERMISSIONS` (key/group/description) + for each, the role names that grant it (from role_permissions). Shape: `[{group, permissions:[{key, description, roles:[name]}]}]`.
- Commit `feat(controlplane): access module — roles & permission-catalog endpoints`.

### Task 5: Route tests + phase gate

- Route tests (DB-gated, reuse `routes_api.rs` harness + the `session_for_with_permission` helper from P1): create user; assign/unassign role; **last-admin guard** returns 409; create/delete local group + members; oidc group delete → 409; create role + set invalid permission → 400; delete seeded role → 409; `GET /permissions` returns the catalog; a scoped principal without `role:read` → 403 on `GET /access/roles`.
- **Phase gate:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test -p egret-auth -p egret-controlplane` on `egret_test` → all pass.
- Commit any fmt fixes. P2 ready.

## Notes
- Password hashing: reuse the exact hasher used by local login (search egret-auth for `argon2`/`bcrypt`/`hash_password`); do not roll your own.
- Assigning a platform role to a group is gated by `user:assign_role` (same "assign platform roles" capability) — no separate group perm exists.
- Keep handlers thin; put SQL in the repos.
