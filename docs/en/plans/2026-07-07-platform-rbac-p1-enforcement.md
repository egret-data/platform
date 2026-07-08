# Platform RBAC P1 — Enforcement Core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development to execute this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Turn egret's existing RBAC schema into enforced, fine-grained authorization: a code-defined permission catalog, per-user resolved permissions on `Principal`, a `require_permission` guard with a `platform_admin` superuser bypass, and migration of every control-plane route from `require_platform_admin` to a specific permission.

**Architecture:** Catalog lives in `egret-core::rbac` (single source of truth). `session.rs` resolves each user's effective permissions (role_permissions ∪ over direct + group role assignments) onto `Principal`. `egret-controlplane::auth::require_permission` checks membership, bypassing for `platform_admin`. Routes swap their guard mechanically per a resource:action mapping.

**Tech stack:** Rust (axum, sqlx/Postgres), existing `roles`/`role_permissions`/`platform_role_assignments` tables (migration 0003).

**Spec:** `docs/en/specs/2026-07-07-platform-rbac-admin-design.md`.

**Constraints:** Do NOT run DB tests against the demo DB on :5432 — use a throwaway `egret_test`. Compile-only for backend subagents where possible (`cargo test -p <crate> --no-run`). Never `cargo build -p egret-controlplane` alone. Touch `crates/egret-metastore/src/lib.rs` after adding a migration. `cargo fmt --all` before committing.

---

### Task 1: Permission catalog (`egret-core::rbac`)

**Files:**
- Create: `crates/egret-core/src/rbac.rs`
- Modify: `crates/egret-core/src/lib.rs` (add `pub mod rbac;`)

- [ ] **Step 1: Write the failing test** (append to `rbac.rs` once created, or write first in a `#[cfg(test)]` block)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_has_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for p in PERMISSIONS {
            assert!(seen.insert(p.key), "duplicate permission key: {}", p.key);
        }
        assert!(PERMISSIONS.len() >= 40);
    }
    #[test]
    fn known_and_groups() {
        assert!(is_known("engine:create"));
        assert!(!is_known("engine:frobnicate"));
        assert!(groups().contains(&"Engines"));
    }
}
```

- [ ] **Step 2: Implement `rbac.rs`**

```rust
//! Code-defined RBAC permission catalog — the single source of truth for every
//! control-plane permission. DB `role_permissions` rows must reference these keys.

/// One catalog entry. `key` is the `resource:action` string checked in code.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Permission {
    pub key: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

macro_rules! perm {
    ($key:literal, $group:literal, $desc:literal) => {
        Permission { key: $key, group: $group, description: $desc }
    };
}

/// The complete permission catalog. Add a row here when guarding a new route.
pub static PERMISSIONS: &[Permission] = &[
    perm!("engine:read", "Engines", "View engines"),
    perm!("engine:create", "Engines", "Create engines"),
    perm!("engine:update", "Engines", "Update engine config"),
    perm!("engine:delete", "Engines", "Delete engines"),
    perm!("engine:provision", "Engines", "Provision / restart engines on the cluster"),
    perm!("warehouse:read", "Warehouses", "View catalog warehouses"),
    perm!("warehouse:create", "Warehouses", "Create warehouses"),
    perm!("warehouse:update", "Warehouses", "Update warehouses"),
    perm!("warehouse:delete", "Warehouses", "Delete warehouses"),
    perm!("storage:read", "Storage", "View storage registrations"),
    perm!("storage:create", "Storage", "Create storage registrations"),
    perm!("storage:update", "Storage", "Update storage registrations"),
    perm!("storage:delete", "Storage", "Delete storage registrations"),
    perm!("secret:read", "Secrets", "View secrets (metadata)"),
    perm!("secret:create", "Secrets", "Create secrets"),
    perm!("secret:update", "Secrets", "Update secrets"),
    perm!("secret:delete", "Secrets", "Delete secrets"),
    perm!("database:read", "Databases", "View database connections"),
    perm!("database:create", "Databases", "Create database connections"),
    perm!("database:update", "Databases", "Update database connections"),
    perm!("database:delete", "Databases", "Delete database connections"),
    perm!("policy:read", "Policies", "View Ranger policies"),
    perm!("policy:create", "Policies", "Create policies"),
    perm!("policy:update", "Policies", "Update policies"),
    perm!("policy:delete", "Policies", "Delete policies"),
    perm!("policy:seed", "Policies", "Seed default policies for a profile"),
    perm!("workspace:read", "Workspaces", "View workspaces"),
    perm!("workspace:create", "Workspaces", "Create workspaces"),
    perm!("workspace:update", "Workspaces", "Update workspaces"),
    perm!("workspace:archive", "Workspaces", "Archive workspaces"),
    perm!("user:read", "Users", "View users"),
    perm!("user:create", "Users", "Create local users"),
    perm!("user:update", "Users", "Update users (name/status)"),
    perm!("user:disable", "Users", "Disable users"),
    perm!("user:assign_role", "Users", "Assign/unassign platform roles to users"),
    perm!("group:read", "Groups", "View groups"),
    perm!("group:create", "Groups", "Create local groups"),
    perm!("group:update", "Groups", "Update local groups"),
    perm!("group:delete", "Groups", "Delete local groups"),
    perm!("group:manage_members", "Groups", "Add/remove group members"),
    perm!("role:read", "Roles", "View roles"),
    perm!("role:create", "Roles", "Create custom roles"),
    perm!("role:update", "Roles", "Update roles"),
    perm!("role:delete", "Roles", "Delete custom roles"),
    perm!("role:assign_permission", "Roles", "Set a role's permissions"),
];

/// True if `key` is a known catalog permission.
pub fn is_known(key: &str) -> bool {
    PERMISSIONS.iter().any(|p| p.key == key)
}

/// Distinct group labels, catalog order preserved.
pub fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for p in PERMISSIONS {
        if !out.contains(&p.group) {
            out.push(p.group);
        }
    }
    out
}
```

- [ ] **Step 3: Register module** — add `pub mod rbac;` to `crates/egret-core/src/lib.rs`.
- [ ] **Step 4: Run** `cargo test -p egret-core rbac` → PASS.
- [ ] **Step 5: Commit** `feat(core): code-defined RBAC permission catalog (egret-core::rbac)`.

---

### Task 2: `Principal.permissions` field

**Files:**
- Modify: `crates/egret-core/src/context.rs` (the `Principal` struct)
- Modify: call-sites that construct `Principal` (compiler will point them out): `crates/egret-auth/src/session.rs`, `crates/egret-auth/src/routes.rs`, `crates/egret-controlplane/src/auth.rs` test helper.

- [ ] **Step 1: Add the field**

```rust
pub struct Principal {
    pub user_id: UserId,
    pub email: String,
    /// Platform-level roles (e.g. `platform_admin`, `data_steward`).
    pub platform_roles: Vec<String>,
    /// Effective permission keys (resolved from role_permissions at session build).
    pub permissions: Vec<String>,
}
```

- [ ] **Step 2: Fix constructors to compile** — every `Principal { … }` literal gains `permissions: vec![]` (or the resolved value in session.rs, done in Task 3). Update the `auth.rs` test helper `principal(roles)` to add `permissions: vec![]`, and add a second helper `principal_with(roles, perms)` for Task 4 tests.
- [ ] **Step 3: Run** `cargo build -p egret-bin` (NOT `-p egret-controlplane` alone) → compiles.
- [ ] **Step 4: Commit** `feat(core): Principal carries resolved permissions`.

---

### Task 3: Resolve permissions at session build

**Files:**
- Modify: `crates/egret-auth/src/session.rs` (add `permissions(user_id)`, call it where `platform_roles` is resolved, set on `Principal`)
- Test: `crates/egret-auth/tests/` (DB-gated, reuse the existing skip-without-DATABASE_URL helper)

- [ ] **Step 1: Add the resolver** (mirrors the existing `platform_roles` query)

```rust
/// Effective permission keys for the user: role_permissions over every role
/// assigned directly or via the user's group memberships.
async fn permissions(&self, user_id: UserId) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT rp.permission \
         FROM platform_role_assignments pra \
         JOIN role_permissions rp ON rp.role_id = pra.role_id \
         WHERE (pra.principal_type = 'user'  AND pra.principal_id = $1) \
            OR (pra.principal_type = 'group' AND pra.principal_id IN \
                   (SELECT group_id FROM user_groups WHERE user_id = $1))",
    )
    .bind(user_id.0)
    .fetch_all(&self.db.pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|(p,)| p).collect())
}
```

- [ ] **Step 2: Wire it** — where the `Principal` is built (near `let platform_roles = self.platform_roles(...)`), add `let permissions = self.permissions(UserId(row.user_id)).await?;` and set `permissions` on the constructed `Principal`.
- [ ] **Step 3: DB-gated test** — seed a role with a permission, assign it to a user (direct) and another via a group; assert `permissions(user)` contains the permission for both paths; a user with no roles → empty. Self-skips without `DATABASE_URL`; run against `egret_test` only.
- [ ] **Step 4: Run** `cargo test -p egret-auth --no-run` (compile) + (locally, against egret_test) the DB test. Commit `feat(auth): resolve effective permissions in session`.

---

### Task 4: `require_permission` guard + `platform_admin` bypass

**Files:**
- Modify: `crates/egret-controlplane/src/auth.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn permission_allows_when_held() {
    let p = principal_with(&[], &["engine:create"]);
    assert!(require_permission(&p, "engine:create").is_ok());
}
#[test]
fn permission_denies_when_missing() {
    let p = principal_with(&[], &["engine:read"]);
    assert!(require_permission(&p, "engine:create").is_err());
}
#[test]
fn platform_admin_bypasses_any_permission() {
    let p = principal_with(&["platform_admin"], &[]);
    assert!(require_permission(&p, "engine:create").is_ok());
    assert!(require_permission(&p, "role:delete").is_ok());
}
```

- [ ] **Step 2: Implement**

```rust
/// The platform role that bypasses all permission checks (superuser).
const PLATFORM_ADMIN: &str = "platform_admin";

/// Gate a control-plane action on a specific permission key.
/// `platform_admin` always passes (superuser). Others must hold the key.
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), CpError> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN)
        || principal.permissions.iter().any(|p| p == permission)
    {
        Ok(())
    } else {
        Err(CpError::Forbidden)
    }
}
```

Keep `require_platform_admin` (used by Task 5 for admin-only routes + any not-yet-migrated caller).

- [ ] **Step 3: Run** `cargo test -p egret-controlplane auth` → PASS. Commit `feat(controlplane): require_permission guard with platform_admin bypass`.

---

### Task 5: Migrate control-plane routes to `require_permission`

**Files (one commit per module):** `engines.rs`, `warehouses.rs`, `storage.rs`, `secrets.rs`, `policies/mod.rs`, `monitor.rs`.

**Mechanical rule** — replace each `require_platform_admin(&p)?;` with `require_permission(&p, "<area>:<action>")?;` where:

- `<area>` by module: `engines.rs`→`engine`, `warehouses.rs`→`warehouse`, `storage.rs`→`storage`, `secrets.rs`→`secret`, `policies/mod.rs`→`policy`, `monitor.rs`→`engine` (monitoring reads engine state → `engine:read`).
- `<action>` by handler name: `list_*`/`get_*`/`*_status`/`*_logs`/`resource-lookup`/`trino-config`/`service-def`/`principals` → `read`; `create_*`/`add_*` → `create`; `update_*`/`set_*`/`*_enabled`/`toggle_*` → `update`; `delete_*`/`remove_*` → `delete`; engine `provision`/`restart_*` → `provision`; policies `seed_defaults` → `seed`.
- `policies/mod.rs` specifics: `list_policies`/`get_policy`/`list_profiles`/`trino_config`/`get_service_def`/`resource_lookup`/`list_principals` → `policy:read`; `create_policy` → `policy:create`; `update_policy`/`set_policy_enabled` → `policy:update`; `delete_policy_handler` → `policy:delete`; `seed_defaults` → `policy:seed`.

- [ ] **Per module — Step 1:** apply the rule to every `require_platform_admin(&p)?` in the file (leave the `use` import; add `require_permission` to it).
- [ ] **Step 2:** add/adjust a route test proving enforcement for ≥1 read and ≥1 mutating endpoint in that module: a principal WITHOUT the permission → 403; WITH it (or `platform_admin`) → 200. (Reuse the module's existing route-test harness; the session/`Auth` test helper must set `permissions`.)
- [ ] **Step 3:** `cargo test -p egret-controlplane <module>` (compile + the route tests, against egret_test locally) → PASS. `cargo fmt --all`.
- [ ] **Step 4:** Commit `feat(controlplane): enforce <area> permissions on <module> routes`.

Repeat for all six modules (six commits).

---

### Task 6: Default role → permission seed + startup validation

**Files:**
- Create: `migrations/0059_rbac_permission_seed.sql` (confirm the next number with `ls migrations | tail`)
- Modify: `crates/egret-metastore/src/lib.rs` (touch to force migrator re-embed)
- Modify: wherever the app boots the DB/migrator (e.g. `egret-bin` startup or `egret-metastore`) to add the catalog drift warning.

- [ ] **Step 1: Seed migration** — grant `data_steward` its read-heavy set; `platform_admin` needs NONE (bypass). Idempotent.

```sql
-- data_steward: read across the platform + full policy control.
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, perm FROM roles r, unnest(ARRAY[
    'policy:read','policy:create','policy:update','policy:delete','policy:seed',
    'warehouse:read','database:read','storage:read','engine:read',
    'user:read','group:read','role:read'
]) AS perm
WHERE r.scope='platform' AND r.name='data_steward'
ON CONFLICT (role_id, permission) DO NOTHING;
```

- [ ] **Step 2: Startup validation** — after migrations run at boot, query `SELECT DISTINCT permission FROM role_permissions` and `tracing::warn!` for any not in `egret_core::rbac::is_known(...)`. Do not fail startup.
- [ ] **Step 3:** Touch `crates/egret-metastore/src/lib.rs`; `cargo build -p egret-bin`. Commit `feat(rbac): seed data_steward permissions + startup catalog validation`.

---

### Task 7: Phase gate

- [ ] Backend static: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` (0), `cargo test --workspace` on a fresh `egret_test` → all pass (2 pre-existing egret-engine job tests may self-skip/pass; no NEW failures).
- [ ] Manual smoke (optional): a non-admin user with only `policy:read` can GET policies but gets 403 on POST; `platform_admin` unaffected.
- [ ] Commit any fmt fixes. P1 ready; proceed to P2 plan (access APIs).

## Notes for the implementer

- The `Auth` extractor's test helper must populate `Principal.permissions` so route tests can exercise `require_permission`. If the harness hard-codes a `platform_admin` principal, add a variant that yields a scoped principal (roles empty, specific permissions) to prove 403/200.
- Keep `require_platform_admin` for `role:*`/`user:assign_role` style actions in P2; P1 only migrates the six existing modules.
- No frontend changes in P1.
