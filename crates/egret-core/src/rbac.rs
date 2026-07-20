//! Code-defined RBAC permission catalog — the single source of truth for every
//! control-plane permission. DB `role_permissions` rows must reference these keys.
//!
//! `workspace:*` keys are enforced by `egret_workspace::rbac::Authorizer::require_platform`
//! (not `egret_controlplane::auth::require_permission`) but are real, live gates on
//! workspace admin routes (`egret-workspace/src/routes.rs`) — keep them here.
//! `database:*` was removed from this catalog: it had zero enforcement call sites
//! anywhere in the codebase (genuinely dead, unlike `workspace:*`).
//!
//! `role:create`/`role:update`/`role:delete`/`role:assign_permission`,
//! `user:assign_role`, and `group:manage_members` were removed: they used to
//! be delegatable permissions guarding writes to the authorization graph
//! itself (role definitions, role-permission sets, role/group-role
//! assignments, group membership), which let a non-admin holding one of them
//! grant themselves any permission. Those routes now gate on
//! `require_platform_admin` only (see `egret-controlplane/src/access/mod.rs`),
//! so the keys are no longer enforced anywhere and are dropped from the
//! catalog to keep the drift-lint honest.

/// Where a permission is enforced: platform-wide (control-plane routes) or
/// per-workspace (workspace-feature routes gated by the workspace authorizer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Platform,
    Workspace,
}

/// One catalog entry. `key` is the `resource:action` string checked in code.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Permission {
    pub key: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub scope: Scope,
}

macro_rules! perm {
    ($key:literal, $group:literal, $desc:literal, $scope:expr) => {
        Permission {
            key: $key,
            group: $group,
            description: $desc,
            scope: $scope,
        }
    };
}

/// The complete permission catalog. Add a row here when guarding a new route.
pub static PERMISSIONS: &[Permission] = &[
    perm!("engine:read", "Engines", "View engines", Scope::Platform),
    perm!(
        "engine:create",
        "Engines",
        "Create engines",
        Scope::Platform
    ),
    perm!(
        "engine:update",
        "Engines",
        "Update engine config",
        Scope::Platform
    ),
    perm!(
        "engine:delete",
        "Engines",
        "Delete engines",
        Scope::Platform
    ),
    perm!(
        "engine:provision",
        "Engines",
        "Provision / restart engines on the cluster",
        Scope::Platform
    ),
    perm!(
        "engine:kill",
        "Engines",
        "Kill running queries",
        Scope::Platform
    ),
    perm!(
        "warehouse:read",
        "Warehouses",
        "View catalog warehouses",
        Scope::Platform
    ),
    perm!(
        "warehouse:create",
        "Warehouses",
        "Create warehouses",
        Scope::Platform
    ),
    perm!(
        "warehouse:update",
        "Warehouses",
        "Update warehouses",
        Scope::Platform
    ),
    perm!(
        "warehouse:delete",
        "Warehouses",
        "Delete warehouses",
        Scope::Platform
    ),
    perm!(
        "storage:read",
        "Storage",
        "View storage registrations",
        Scope::Platform
    ),
    perm!(
        "storage:create",
        "Storage",
        "Create storage registrations",
        Scope::Platform
    ),
    perm!(
        "storage:update",
        "Storage",
        "Update storage registrations",
        Scope::Platform
    ),
    perm!(
        "storage:delete",
        "Storage",
        "Delete storage registrations",
        Scope::Platform
    ),
    perm!(
        "secret:read",
        "Secrets",
        "View secrets (metadata)",
        Scope::Platform
    ),
    perm!(
        "secret:create",
        "Secrets",
        "Create secrets",
        Scope::Platform
    ),
    perm!(
        "secret:update",
        "Secrets",
        "Update secrets",
        Scope::Platform
    ),
    perm!(
        "secret:delete",
        "Secrets",
        "Delete secrets",
        Scope::Platform
    ),
    perm!(
        "policy:read",
        "Policies",
        "View Ranger policies",
        Scope::Platform
    ),
    perm!(
        "policy:create",
        "Policies",
        "Create policies",
        Scope::Platform
    ),
    perm!(
        "policy:update",
        "Policies",
        "Update policies",
        Scope::Platform
    ),
    perm!(
        "policy:delete",
        "Policies",
        "Delete policies",
        Scope::Platform
    ),
    perm!(
        "policy:seed",
        "Policies",
        "Seed default policies for a profile",
        Scope::Platform
    ),
    perm!(
        "tag:read",
        "Classifications",
        "View classification tags",
        Scope::Platform
    ),
    perm!(
        "tag:manage",
        "Classifications",
        "Create, edit, and delete classification tags",
        Scope::Platform
    ),
    perm!(
        "workspace:read",
        "Workspaces",
        "View workspaces",
        Scope::Platform
    ),
    perm!(
        "workspace:create",
        "Workspaces",
        "Create workspaces",
        Scope::Platform
    ),
    perm!(
        "workspace:update",
        "Workspaces",
        "Update workspaces",
        Scope::Platform
    ),
    perm!(
        "workspace:archive",
        "Workspaces",
        "Archive workspaces",
        Scope::Platform
    ),
    perm!("user:read", "Users", "View users", Scope::Platform),
    perm!(
        "user:create",
        "Users",
        "Create local users",
        Scope::Platform
    ),
    perm!(
        "user:update",
        "Users",
        "Update users (name/status)",
        Scope::Platform
    ),
    perm!("user:disable", "Users", "Disable users", Scope::Platform),
    perm!("user:delete", "Users", "Delete users", Scope::Platform),
    perm!("group:read", "Groups", "View groups", Scope::Platform),
    perm!(
        "group:create",
        "Groups",
        "Create local groups",
        Scope::Platform
    ),
    perm!(
        "group:update",
        "Groups",
        "Update local groups",
        Scope::Platform
    ),
    perm!(
        "group:delete",
        "Groups",
        "Delete local groups",
        Scope::Platform
    ),
    perm!("role:read", "Roles", "View roles", Scope::Platform),
    // ---- Workspace-feature actions (enforced per-workspace by the workspace
    // authorizer via `require_workspace`/`require_platform`). Keys must match
    // the exact string literals at those call sites. ----
    perm!(
        "query:run",
        "SQL Lab",
        "Run SQL queries in a workspace",
        Scope::Workspace
    ),
    perm!(
        "dataset:read",
        "Datasets",
        "View datasets",
        Scope::Workspace
    ),
    perm!(
        "dataset:manage",
        "Datasets",
        "Create, update, and delete datasets",
        Scope::Workspace
    ),
    perm!("chart:read", "Charts", "View charts", Scope::Workspace),
    perm!(
        "chart:manage",
        "Charts",
        "Create, update, and delete charts",
        Scope::Workspace
    ),
    perm!(
        "dashboard:read",
        "Dashboards",
        "View dashboards",
        Scope::Workspace
    ),
    perm!(
        "dashboard:manage",
        "Dashboards",
        "Create, update, and delete dashboards",
        Scope::Workspace
    ),
    perm!(
        "notebook:read",
        "Notebooks",
        "View notebooks",
        Scope::Workspace
    ),
    perm!(
        "notebook:write",
        "Notebooks",
        "Create and edit notebooks",
        Scope::Workspace
    ),
    perm!(
        "saved_query:read",
        "SQL Lab",
        "View saved queries",
        Scope::Workspace
    ),
    perm!(
        "saved_query:manage",
        "SQL Lab",
        "Create, edit, and delete saved queries",
        Scope::Workspace
    ),
    perm!(
        "workspace:manage_members",
        "Workspaces",
        "Manage workspace members and workspace-scoped resources",
        Scope::Workspace
    ),
];

/// True if `key` is a known catalog permission.
pub fn is_known(key: &str) -> bool {
    PERMISSIONS.iter().any(|p| p.key == key)
}

/// Admin-only, NON-delegatable actions: the PDP `forbid`s them for everyone but
/// `platform_admin`. Mostly authorization-graph mutations (roles, role-permission
/// sets, role/group assignments); also `audit:read` (the security audit trail is
/// admin-only — B4). Deliberately NOT in `PERMISSIONS` (not grantable to any role).
/// See the B1b-2 design.
pub static ADMIN_GRAPH_ACTIONS: &[&str] = &[
    "role:create",
    "role:update",
    "role:delete",
    "role:assign_permission",
    "user:assign_role",
    "group:manage_members",
    "user:assign_platform_role",
    "audit:read",
];

/// True if `key` is an admin-graph (non-delegatable) action.
pub fn is_admin_graph(key: &str) -> bool {
    ADMIN_GRAPH_ACTIONS.contains(&key)
}

/// Permissions at a given scope, catalog order preserved.
pub fn at_scope(scope: Scope) -> Vec<&'static Permission> {
    PERMISSIONS.iter().filter(|p| p.scope == scope).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_has_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for p in PERMISSIONS {
            assert!(seen.insert(p.key), "duplicate permission key: {}", p.key);
        }
        assert!(!PERMISSIONS.is_empty());
    }
    #[test]
    fn known_and_groups() {
        assert!(is_known("engine:create"));
        assert!(!is_known("engine:frobnicate"));
        assert!(groups().contains(&"Engines"));
    }
    #[test]
    fn admin_graph_actions_are_non_grantable() {
        assert!(is_admin_graph("role:create"));
        assert!(!is_admin_graph("engine:read"));
        for a in ADMIN_GRAPH_ACTIONS {
            assert!(!is_known(a), "{a} must not be grantable");
        }
    }
    #[test]
    fn saved_query_perms_are_known() {
        assert!(is_known("saved_query:read"));
        assert!(is_known("saved_query:manage"));
    }
    #[test]
    fn tag_perms_are_known_and_delegatable() {
        // The classification perms are grantable governance perms (data_steward
        // holds tag:read/tag:manage), NOT admin-graph actions. Object tag
        // assignment reuses object-manage (`<kind>:manage`), so there is no
        // dedicated `tag:assign` perm.
        assert!(is_known("tag:read"));
        assert!(is_known("tag:manage"));
        assert!(!is_known("tag:assign"));
        assert!(!is_admin_graph("tag:manage"));
        assert!(groups().contains(&"Classifications"));
        // tag:read/manage are platform-scoped catalog perms.
        assert!(PERMISSIONS
            .iter()
            .any(|p| p.key == "tag:read" && p.scope == Scope::Platform));
        assert!(PERMISSIONS
            .iter()
            .any(|p| p.key == "tag:manage" && p.scope == Scope::Platform));
    }
    #[test]
    fn workspace_actions_present_and_scoped() {
        assert!(is_known("dashboard:read"));
        assert!(PERMISSIONS
            .iter()
            .any(|p| p.key == "dashboard:read" && p.scope == Scope::Workspace));
        assert!(!at_scope(Scope::Workspace).is_empty());
        assert!(at_scope(Scope::Platform)
            .iter()
            .any(|p| p.key == "engine:create"));
    }
}
