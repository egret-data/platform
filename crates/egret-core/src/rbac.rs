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
    perm!(
        "database:create",
        "Databases",
        "Create database connections"
    ),
    perm!(
        "database:update",
        "Databases",
        "Update database connections"
    ),
    perm!(
        "database:delete",
        "Databases",
        "Delete database connections"
    ),
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
    perm!(
        "user:assign_role",
        "Users",
        "Assign/unassign platform roles to users"
    ),
    perm!("group:read", "Groups", "View groups"),
    perm!("group:create", "Groups", "Create local groups"),
    perm!("group:update", "Groups", "Update local groups"),
    perm!("group:delete", "Groups", "Delete local groups"),
    perm!("group:manage_members", "Groups", "Add/remove group members"),
    perm!("role:read", "Roles", "View roles"),
    perm!("role:create", "Roles", "Create custom roles"),
    perm!("role:update", "Roles", "Update roles"),
    perm!("role:delete", "Roles", "Delete custom roles"),
    perm!(
        "role:assign_permission",
        "Roles",
        "Set a role's permissions"
    ),
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
        assert!(PERMISSIONS.len() >= 40);
    }
    #[test]
    fn known_and_groups() {
        assert!(is_known("engine:create"));
        assert!(!is_known("engine:frobnicate"));
        assert!(groups().contains(&"Engines"));
    }
}
