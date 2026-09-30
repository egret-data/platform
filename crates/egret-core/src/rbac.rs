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
    // Registry catalogs (SP3). Deliberately NOT `catalog:*`: `catalog:read` is
    // the workspace-scope key every workspace role holds (0024) for
    // egret-query's workspace engine view. Cedar itself cannot confuse the two
    // (a platform grant is `resource == Platform::"root"`, which never matches
    // a Workspace entity even for the SAME action name — B1b-2's anti-leak; see
    // `platform_grant_never_satisfies_workspace_scope_check`). The reason to
    // keep them distinct is that permission KEYS are scope-blind strings
    // everywhere else they are read: `rbac::is_known`, the effective-permission
    // lists the frontend's admin permission UI renders, and the single Cedar
    // action namespace a policy names by string — one key meaning two
    // different things there is ambiguous even though Cedar's own enforcement
    // is safe.
    // SP3c-2 widened these four beyond catalog DEFINITIONS: the 27
    // `warehouse_admin` routes and the 3 resource-tag routes moved onto them
    // from the retired `warehouse:*`. So they now also carry the egret-warehouse
    // admin surface — browsing lakehouse namespaces and tables, creating and
    // deleting warehouses, and minting and revoking Catalog Clients, which are
    // real data-lake credentials.
    //
    // D3 claimed "nobody gains access, nobody loses it". The second half is
    // guaranteed by 0153's re-grant. The FIRST half is not: a platform role
    // created after 0148 holding `catalog_registry:*` and never `warehouse:*`
    // gains all of the above. That is accepted (user decision, 2026-09-23) —
    // the surface belongs to the catalog now, since its tabs live on the
    // catalog page — but the descriptions below must say so, because they are
    // what an admin reads in the Permissions UI before granting.
    perm!(
        "catalog_registry:read",
        "Catalogs",
        "View registry catalogs and their engine attachments, and browse an egret warehouse's namespaces, tables and Catalog Clients",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:create",
        "Catalogs",
        "Create registry catalogs and egret warehouses, and mint Catalog Clients (data-lake credentials) and catalog roles",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:update",
        "Catalogs",
        "Edit registry catalogs, re-apply them to engines, assign them to workspaces, and change an egret warehouse's settings, protection and format policy",
        Scope::Platform
    ),
    perm!(
        "catalog_registry:delete",
        "Catalogs",
        "Delete registry catalogs and egret warehouses, and revoke Catalog Clients and catalog roles",
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
        "service_account:read",
        "Data API",
        "View service accounts and their OAuth2 clients (never the secret)",
        Scope::Workspace
    ),
    perm!(
        "service_account:manage",
        "Data API",
        "Create and delete service accounts and issue their OAuth2 clients",
        Scope::Workspace
    ),
    perm!(
        "data_api_endpoint:read",
        "Data API",
        "View Data API endpoints and their versions",
        Scope::Workspace
    ),
    perm!(
        "data_api_endpoint:manage",
        "Data API",
        "Create, edit, publish and roll back Data API endpoints",
        Scope::Workspace
    ),
    // Spec §6.2 step 4. Unlike its two siblings this one is meant to be held
    // OBJECT-scoped by a service account (one `access_grants` row per endpoint),
    // not workspace-wide by a human — see migration 0124.
    perm!(
        "data_api_endpoint:invoke",
        "Data API",
        "Call a published Data API endpoint",
        Scope::Workspace
    ),
    // Workflow W1 (Task 7). `:manage` was seeded to `ws_editor`/`ws_admin` in
    // migration 0132 ("managing flows is an editor act", following
    // `alert:manage`/`chart:manage`); `:read` is seeded alongside it in 0133,
    // for the same reason `data_api_endpoint:read` is seeded alongside
    // `:manage` in 0122 -- permissions are flat, so a role that may edit a
    // flow must be granted the ability to list/view it explicitly.
    perm!(
        "workflow:read",
        "Workflows",
        "View flow drafts, their validation report, and their published versions",
        Scope::Workspace
    ),
    perm!(
        "workflow:manage",
        "Workflows",
        "Create, edit, publish and delete flow drafts",
        Scope::Workspace
    ),
    // Workflow W5b fix round 1 (user decision 2026-09-30). `secret()` reads
    // ANY secret of the workspace -- including engine credentials created
    // under `workspace:manage_members` -- and a flow's author decides where
    // the value goes (a query's outputs, a JDBC url of their choosing). So
    // writing a flow that can call it is its own permission, seeded to
    // `ws_admin` only (migration 0160), not implied by `workflow:manage`.
    // Checked on every write of a flow's source (create, update, publish):
    // executions run the draft, so a publish-only gate would be walked round.
    perm!(
        "workflow:use_secrets",
        "Workflows",
        "Save and publish flows that call secret(), which reads any secret of the workspace",
        Scope::Workspace
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
    // Platform, for the same reason `dashboard:certify` is, and covering METRICS as well as
    // datasets: a metric belongs to a dataset, and "vouching for this data" is one act. A
    // separate `metric:certify` would be a second grant to remember and to forget.
    perm!(
        "dataset:certify",
        "Datasets",
        "Mark datasets and their metrics as certified, and withdraw certification",
        Scope::Platform
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
    // Platform, not Workspace, and deliberately NOT folded into `dashboard:manage`.
    //
    // Certification is a trust signal. If everyone who can edit a dashboard can also mark it
    // trusted, the mark says only "its author liked it" and a reader learns nothing from it.
    // Platform scope is what makes it a stewardship act rather than an editing one: it sits
    // beside `policy:*` (0059) and `tag:*` (0080), which is the set `data_steward` already holds.
    perm!(
        "dashboard:certify",
        "Dashboards",
        "Mark dashboards as certified, and withdraw certification",
        Scope::Platform
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
        "app:read",
        "Apps",
        "Open published data apps",
        Scope::Workspace
    ),
    perm!(
        "app:write",
        "Apps",
        "Author, publish and restore data apps",
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
    // One permission covers alerts AND reports: they are one table, one editor
    // and one list page, and splitting the grant would mean an editor who can
    // schedule an email of a dashboard but not an email of a query — a
    // distinction with no security content, since both deliver data the owner
    // can already read.
    perm!(
        "alert:manage",
        "Alerts & Reports",
        "Create, edit, and delete scheduled alerts and reports",
        Scope::Workspace
    ),
    // Reading the list is otherwise implied by workspace membership plus the
    // `alerts` entitlement. This exists because the generic folder router keys
    // off a per-kind READ permission (`kind_perms`), and without one a viewer
    // would be denied the folder list and lose the grouping the page is built
    // around. Every other foldered entity has the same pair.
    perm!(
        "alert:read",
        "Alerts & Reports",
        "View scheduled alerts and reports",
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
    fn certifying_a_dashboard_is_a_platform_stewardship_perm_not_an_editing_one() {
        assert!(is_known("dashboard:certify"));
        // Grantable governance, like tag:manage — not an admin-graph action.
        assert!(!is_admin_graph("dashboard:certify"));
        // PLATFORM, not Workspace. This is the whole point: a workspace editor holds
        // dashboard:manage and must NOT thereby be able to certify their own work, or the
        // badge degrades to "the author liked it".
        assert!(PERMISSIONS
            .iter()
            .any(|p| p.key == "dashboard:certify" && p.scope == Scope::Platform));
        assert!(PERMISSIONS
            .iter()
            .any(|p| p.key == "dashboard:manage" && p.scope == Scope::Workspace));
    }
    #[test]
    fn certifying_a_dataset_is_the_same_kind_of_act_as_certifying_a_dashboard() {
        // The asymmetry this closes: `dataset:certify` used to not exist, so certification rode
        // on `dataset:manage` and an editor could vouch for their own work — and name someone
        // else as the voucher. Asserted as a PAIR against the dashboard perms so the two cannot
        // drift apart again without a test noticing.
        assert!(is_known("dataset:certify"));
        assert!(!is_admin_graph("dataset:certify"));
        let scope_of = |k: &str| PERMISSIONS.iter().find(|p| p.key == k).map(|p| p.scope);
        assert_eq!(scope_of("dataset:certify"), scope_of("dashboard:certify"));
        assert_eq!(scope_of("dataset:certify"), Some(Scope::Platform));
        assert_eq!(scope_of("dataset:manage"), Some(Scope::Workspace));
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
    fn warehouse_permissions_are_retired_sp3c2() {
        // SP3c-2 Task 7: `warehouse_admin` re-gated onto `catalog_registry:*`
        // (migration 0153 re-ran 0148's grant, then deleted the old keys), so
        // none of the four `warehouse:*` keys are known any more, and the
        // "Warehouses" group they were the only members of disappears with
        // them rather than surviving as an empty group.
        for k in [
            "warehouse:read",
            "warehouse:create",
            "warehouse:update",
            "warehouse:delete",
        ] {
            assert!(!is_known(k), "{k} must be retired");
            assert!(
                !PERMISSIONS.iter().any(|p| p.key == k),
                "{k} must not appear in the catalog"
            );
        }
        assert!(
            !groups().contains(&"Warehouses"),
            "the Warehouses group must disappear with its only members"
        );
    }
    #[test]
    fn catalog_registry_perms_are_platform_scoped_and_distinct() {
        for k in [
            "catalog_registry:read",
            "catalog_registry:create",
            "catalog_registry:update",
            "catalog_registry:delete",
        ] {
            assert!(
                PERMISSIONS
                    .iter()
                    .any(|p| p.key == k && p.scope == Scope::Platform),
                "{k}"
            );
        }
        assert!(groups().contains(&"Catalogs"));
        // The workspace-scope `catalog:read` (0024) is NOT a platform key and
        // must never become one. This is NOT because Cedar would confuse the
        // two scopes (it can't — see `platform_grant_never_satisfies_workspace_scope_check`
        // in egret-query's registry_workspace_routes_api tests): it's because
        // `is_known`/the effective-permission lists/the admin permission UI all
        // read permission keys as plain scope-blind strings, so one key with two
        // meanings would be ambiguous there.
        assert!(!is_known("catalog:read"));
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
