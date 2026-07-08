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

/// One catalog entry. `key` is the `resource:action` string checked in code.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Permission {
    pub key: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

macro_rules! perm {
    ($key:literal, $group:literal, $desc:literal) => {
        Permission {
            key: $key,
            group: $group,
            description: $desc,
        }
    };
}

/// The complete permission catalog. Add a row here when guarding a new route.
pub static PERMISSIONS: &[Permission] = &[
    perm!("engine:read", "Engines", "View engines"),
    perm!("engine:create", "Engines", "Create engines"),
    perm!("engine:update", "Engines", "Update engine config"),
    perm!("engine:delete", "Engines", "Delete engines"),
    perm!(
        "engine:provision",
        "Engines",
        "Provision / restart engines on the cluster"
    ),
    perm!("engine:kill", "Engines", "Kill running queries"),
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
    perm!("policy:read", "Policies", "View Ranger policies"),
    perm!("policy:create", "Policies", "Create policies"),
    perm!("policy:update", "Policies", "Update policies"),
    perm!("policy:delete", "Policies", "Delete policies"),
    perm!(
        "policy:seed",
        "Policies",
        "Seed default policies for a profile"
    ),
    perm!("workspace:read", "Workspaces", "View workspaces"),
    perm!("workspace:create", "Workspaces", "Create workspaces"),
    perm!("workspace:update", "Workspaces", "Update workspaces"),
    perm!("workspace:archive", "Workspaces", "Archive workspaces"),
    perm!("user:read", "Users", "View users"),
    perm!("user:create", "Users", "Create local users"),
    perm!("user:update", "Users", "Update users (name/status)"),
    perm!("user:disable", "Users", "Disable users"),
    perm!("group:read", "Groups", "View groups"),
    perm!("group:create", "Groups", "Create local groups"),
    perm!("group:update", "Groups", "Update local groups"),
    perm!("group:delete", "Groups", "Delete local groups"),
    perm!("role:read", "Roles", "View roles"),
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
}
