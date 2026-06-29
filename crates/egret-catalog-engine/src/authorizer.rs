//! `EgretAuthorizer` — a minimal [`Authorizer`] for the embedded lakekeeper
//! Iceberg REST engine (CE-4).
//!
//! It replaces lakekeeper's inert [`AllowAllAuthorizer`] for *data* actions
//! (namespace / table / view) while leaving every control-plane action
//! permissive. Data actions are gated by a per-token **egret scope** carried in
//! the request's authentication claims under the `"egret"` key:
//!
//! ```json
//! { "perms": ["catalog:read", "catalog:write"], "ns": ["marts", "staging.raw"] }
//! ```
//!
//! Decision (per `(target, action)` tuple):
//! * The configured `system_principal` bypasses scope entirely (egret's
//!   bootstrap + in-process browse principal).
//! * Otherwise the `"egret"` claim must be present (fail-closed if absent),
//!   carry the required permission (`catalog:write` for writes, else
//!   `catalog:read`), and — if `ns` is non-empty — list a subtree-prefix of the
//!   target namespace.
//!
//! This module only defines the type; wiring it into lakekeeper's router
//! `State` is a separate task. The whole [`Authorizer`] impl below is copied
//! verbatim from lakekeeper's `AllowAllAuthorizer`, changing only the struct,
//! `implementation_name`, and the three data-action methods.
//!
//! [`AllowAllAuthorizer`]: lakekeeper::service::authz::AllowAllAuthorizer

use std::collections::HashMap;

use async_trait::async_trait;
use axum::Router;
use egret_catalog::service::tokens::Grant;
use lakekeeper::{
    api::{iceberg::v1::Result, ApiContext, RequestMetadata},
    service::{
        authn::UserId,
        authz::{
            ActionOnGenericTable, ActionOnTable, ActionOnView, Authorizer,
            AuthzBackendErrorOrBadRequest, CatalogGenericTableAction, CatalogNamespaceAction,
            CatalogProjectAction, CatalogRoleAction, CatalogServerAction, CatalogTableAction,
            CatalogUserAction, CatalogViewAction, CatalogWarehouseAction, IsAllowedActionError,
            ListProjectsResponse, NamespaceParent, UserOrRole,
        },
        health::{Health, HealthExt},
        ArcProjectId, AuthZGenericTableInfo, AuthZNamespaceInfo, AuthZTableInfo, AuthZViewInfo,
        CatalogStore, GenericTableId, NamespaceId, NamespaceWithParent, ProjectId,
        ResolvedWarehouse, Role, RoleId, SecretStore, ServerId, State, TableId, ViewId,
        WarehouseId,
    },
};

/// Permission string required for *read* data actions.
const PERM_READ: &str = "catalog:read";
/// Permission string required for *write* data actions.
const PERM_WRITE: &str = "catalog:write";

/// Minimal scope-based authorizer for the embedded lakekeeper catalog engine.
///
/// * `server_id` — the lakekeeper server id (as for `AllowAllAuthorizer`).
/// * `system_principal` — egret's bootstrap + in-process browse principal; it
///   bypasses the scope gate so internal operations and the browse API are
///   never blocked by per-token claims.
#[derive(Clone, Debug)]
pub struct EgretAuthorizer {
    pub server_id: ServerId,
    pub system_principal: UserId,
}

// --- pure scope helpers (unit-tested below; no lakekeeper State needed) -----

/// Classify a table action as a *write* (mutating / privileged) action.
///
/// Reads (return `false`): `ReadData`, `GetMetadata`, `IncludeInList`,
/// `GetTasks`. Everything else is a write. The match is exhaustive on purpose
/// so a newly added variant fails to compile rather than silently defaulting to
/// read (default-deny is safer).
fn table_action_is_write(a: &CatalogTableAction) -> bool {
    match a {
        CatalogTableAction::ReadData
        | CatalogTableAction::GetMetadata
        | CatalogTableAction::IncludeInList
        | CatalogTableAction::GetTasks => false,
        CatalogTableAction::Drop
        | CatalogTableAction::WriteData
        | CatalogTableAction::Commit { .. }
        | CatalogTableAction::Rename
        | CatalogTableAction::Undrop
        | CatalogTableAction::ControlTasks
        | CatalogTableAction::SetProtection => true,
    }
}

/// Classify a namespace action as a *write* action.
///
/// Reads (return `false`): `GetMetadata`, the `List*` actions, and
/// `IncludeInList`. Create/Delete/UpdateProperties/SetProtection are writes.
fn namespace_action_is_write(a: &CatalogNamespaceAction) -> bool {
    match a {
        CatalogNamespaceAction::GetMetadata
        | CatalogNamespaceAction::ListTables
        | CatalogNamespaceAction::ListViews
        | CatalogNamespaceAction::ListNamespaces
        | CatalogNamespaceAction::ListEverything
        | CatalogNamespaceAction::ListGenericTables
        | CatalogNamespaceAction::IncludeInList => false,
        CatalogNamespaceAction::CreateTable { .. }
        | CatalogNamespaceAction::CreateView { .. }
        | CatalogNamespaceAction::CreateNamespace { .. }
        | CatalogNamespaceAction::CreateGenericTable { .. }
        | CatalogNamespaceAction::Delete
        | CatalogNamespaceAction::UpdateProperties { .. }
        | CatalogNamespaceAction::SetProtection => true,
    }
}

/// Classify a view action as a *write* action.
///
/// Reads (return `false`): `GetMetadata`, `Select`, `IncludeInList`,
/// `GetTasks`. Drop/Commit/Rename/Undrop/ControlTasks/SetProtection are writes.
fn view_action_is_write(a: &CatalogViewAction) -> bool {
    match a {
        CatalogViewAction::GetMetadata
        | CatalogViewAction::Select
        | CatalogViewAction::IncludeInList
        | CatalogViewAction::GetTasks => false,
        CatalogViewAction::Drop
        | CatalogViewAction::Commit { .. }
        | CatalogViewAction::Rename
        | CatalogViewAction::Undrop
        | CatalogViewAction::ControlTasks
        | CatalogViewAction::SetProtection => true,
    }
}

/// The all-namespaces sentinel for a grant's `namespace`.
const ALL_NS: &str = "*";

/// The permission string required for an action of the given write-ness.
fn need(is_write: bool) -> &'static str {
    if is_write {
        PERM_WRITE
    } else {
        PERM_READ
    }
}

/// True if `scope_entry` (a dot-joined namespace prefix, e.g. `"staging.raw"`)
/// is a subtree-prefix of `target` (the target namespace's dot-split levels) —
/// i.e. the grant's namespace is an ancestor-or-self of the target, so the grant
/// covers the target.
fn ns_prefix_matches(scope_entry: &str, target: &[String]) -> bool {
    let want: Vec<&str> = scope_entry.split('.').collect();
    want.len() <= target.len() && want.iter().zip(target).all(|(a, b)| a == b)
}

/// The reverse of [`ns_prefix_matches`]: true if `target` is an ancestor-or-self
/// of `grant_ns` (the grant lives at-or-below `target`). Used for navigation:
/// a `B.t2`-only grant must let the client read/list `B` (and the root) to
/// reach the granted table. `"*"` is handled by the caller (covers everything).
fn ns_prefix_matches_levels(target: &[String], grant_ns: &str) -> bool {
    let grant_levels: Vec<&str> = grant_ns.split('.').collect();
    target.len() <= grant_levels.len() && target.iter().zip(&grant_levels).all(|(a, b)| a == b)
}

/// True if grant `g`'s namespace covers `target` (the grant is at-or-above the
/// target). `"*"` covers every namespace.
fn grant_covers_ns(g: &Grant, target: &[String]) -> bool {
    g.namespace == ALL_NS || ns_prefix_matches(&g.namespace, target)
}

/// Table-action decision (OR over grants): allowed if ANY grant carries the
/// needed perm, covers `target_ns`, and either has no `table` (whole-namespace)
/// or names exactly `name`.
fn decide_table(grants: &[Grant], target_ns: &[String], name: &str, is_write: bool) -> bool {
    let need = need(is_write);
    grants.iter().any(|g| {
        g.permissions.iter().any(|p| p == need)
            && grant_covers_ns(g, target_ns)
            && g.table.as_deref().is_none_or(|t| t == name)
    })
}

/// Namespace READ decision (GetMetadata / List*): allowed if ANY grant with
/// `read` *relates* to `target_ns` — it either covers the target, OR the target
/// is an ancestor of the grant's namespace (navigation down to a granted child
/// table). A table-only grant therefore lets the client list its namespace.
fn decide_ns_read(grants: &[Grant], target_ns: &[String]) -> bool {
    grants.iter().any(|g| {
        g.permissions.iter().any(|p| p == PERM_READ)
            && (grant_covers_ns(g, target_ns)
                || (g.namespace != ALL_NS && ns_prefix_matches_levels(target_ns, &g.namespace)))
    })
}

/// Namespace WRITE decision (Create*/Delete/UpdateProperties/SetProtection):
/// allowed only via a WHOLE-namespace grant (`table is None`) with `write` that
/// covers `target_ns`. A table-only grant never permits namespace writes.
fn decide_ns_write(grants: &[Grant], target_ns: &[String]) -> bool {
    grants.iter().any(|g| {
        g.table.is_none()
            && g.permissions.iter().any(|p| p == PERM_WRITE)
            && grant_covers_ns(g, target_ns)
    })
}

/// Object (view / generic-table) decision: allowed if ANY whole-namespace grant
/// (`table is None`) with the needed perm covers `ns`. Views/generic tables are
/// not individually pickable, so only whole-namespace grants cover them.
fn decide_object_ns(grants: &[Grant], ns: &[String], is_write: bool) -> bool {
    let need = need(is_write);
    grants.iter().any(|g| {
        g.table.is_none() && g.permissions.iter().any(|p| p == need) && grant_covers_ns(g, ns)
    })
}

/// Extract the resource `grants` from the request's `"egret"` authentication
/// claim (`{ "egret": { "grants": [ { "ns", "table", "perms" } ] } }`). Returns
/// `None` (fail-closed) when there is no authentication or no `"egret"` claim;
/// returns `Some(vec![])` (deny-all) when the claim is present but carries no
/// grants. Malformed grant entries are skipped defensively.
fn egret_grants(metadata: &RequestMetadata) -> Option<Vec<Grant>> {
    let v = metadata.authentication()?.claims("egret")?;
    let arr = v.get("grants")?.as_array()?;
    let grants = arr
        .iter()
        .filter_map(|g| {
            let namespace = g.get("ns")?.as_str()?.to_string();
            // `table` may be absent or JSON null (whole-namespace grant).
            let table = g.get("table").and_then(|t| t.as_str()).map(String::from);
            let permissions = g
                .get("perms")?
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(String::from))
                .collect();
            Some(Grant {
                namespace,
                table,
                permissions,
            })
        })
        .collect();
    Some(grants)
}

impl EgretAuthorizer {
    /// Whether `metadata`'s principal is the system principal (the bootstrap /
    /// in-process browse bypass). `user_id()` (not `actor()`) is the right bypass
    /// identity: the system principal has a fixed `UserId` and never assumes a
    /// role, so the underlying user id is always the in-process principal.
    fn is_system(&self, metadata: &RequestMetadata) -> bool {
        metadata.user_id() == Some(&self.system_principal)
    }

    /// Table-action gate: system bypass, else OR the token's grants over the
    /// target table (namespace coverage + optional exact table + needed perm).
    /// An absent `"egret"` claim fails closed.
    fn gate_table(
        &self,
        metadata: &RequestMetadata,
        target_ns: &[String],
        table_name: &str,
        is_write: bool,
    ) -> bool {
        if self.is_system(metadata) {
            return true;
        }
        match egret_grants(metadata) {
            Some(grants) => decide_table(&grants, target_ns, table_name, is_write),
            None => false,
        }
    }

    /// Namespace-action gate: system bypass, else read uses the navigation-aware
    /// [`decide_ns_read`] and write uses the whole-namespace [`decide_ns_write`].
    /// An absent `"egret"` claim fails closed.
    fn gate_namespace(
        &self,
        metadata: &RequestMetadata,
        target_ns: &[String],
        is_write: bool,
    ) -> bool {
        if self.is_system(metadata) {
            return true;
        }
        match egret_grants(metadata) {
            Some(grants) => {
                if is_write {
                    decide_ns_write(&grants, target_ns)
                } else {
                    decide_ns_read(&grants, target_ns)
                }
            }
            None => false,
        }
    }

    /// Object (view / generic-table) gate: system bypass, else OR whole-namespace
    /// grants over `ns`. An absent `"egret"` claim fails closed.
    fn gate_object(&self, metadata: &RequestMetadata, ns: &[String], is_write: bool) -> bool {
        if self.is_system(metadata) {
            return true;
        }
        match egret_grants(metadata) {
            Some(grants) => decide_object_ns(&grants, ns, is_write),
            None => false,
        }
    }
}

#[async_trait]
impl HealthExt for EgretAuthorizer {
    async fn health(&self) -> Vec<Health> {
        vec![]
    }
    async fn update_health(&self) {
        // Do nothing
    }
}

#[async_trait]
impl Authorizer for EgretAuthorizer {
    type ServerAction = CatalogServerAction;
    type ProjectAction = CatalogProjectAction;
    type WarehouseAction = CatalogWarehouseAction;
    type NamespaceAction = CatalogNamespaceAction;
    type TableAction = CatalogTableAction;
    type ViewAction = CatalogViewAction;
    type GenericTableAction = CatalogGenericTableAction;
    type UserAction = CatalogUserAction;
    type RoleAction = CatalogRoleAction;

    fn implementation_name() -> &'static str {
        "egret"
    }

    fn server_id(&self) -> ServerId {
        self.server_id
    }

    fn new_router<C: CatalogStore, S: SecretStore>(&self) -> Router<ApiContext<State<Self, C, S>>> {
        Router::new()
    }

    async fn check_assume_role_impl(
        &self,
        _principal: &UserId,
        _assumed_role: &Role,
        _request_metadata: &RequestMetadata,
    ) -> Result<bool, AuthzBackendErrorOrBadRequest> {
        Ok(true)
    }

    async fn can_bootstrap(&self, _metadata: &RequestMetadata) -> Result<()> {
        Ok(())
    }

    async fn bootstrap(&self, _metadata: &RequestMetadata, _is_operator: bool) -> Result<()> {
        Ok(())
    }

    async fn list_projects_impl(
        &self,
        _metadata: &RequestMetadata,
    ) -> Result<ListProjectsResponse, AuthzBackendErrorOrBadRequest> {
        Ok(ListProjectsResponse::All)
    }

    async fn can_search_users_impl(
        &self,
        _metadata: &RequestMetadata,
    ) -> Result<bool, AuthzBackendErrorOrBadRequest> {
        Ok(true)
    }

    async fn are_allowed_user_actions_impl(
        &self,
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        users_with_actions: &[(&UserId, Self::UserAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; users_with_actions.len()])
    }

    async fn are_allowed_role_actions_impl(
        &self,
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        roles_with_actions: &[(&Role, Self::RoleAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; roles_with_actions.len()])
    }

    async fn are_allowed_server_actions_impl(
        &self,
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        actions: &[Self::ServerAction],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; actions.len()])
    }

    async fn are_allowed_project_actions_impl(
        &self,
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        projects_with_actions: &[(&ArcProjectId, Self::ProjectAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; projects_with_actions.len()])
    }

    async fn are_allowed_warehouse_actions_impl(
        &self,
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        warehouses_with_actions: &[(&ResolvedWarehouse, Self::WarehouseAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; warehouses_with_actions.len()])
    }

    async fn are_allowed_namespace_actions_impl(
        &self,
        metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        _warehouse: &ResolvedWarehouse,
        parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(&impl AuthZNamespaceInfo, Self::NamespaceAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(actions
            .iter()
            .map(|(info, action)| {
                // Prefer the caller-cased ident from the parent-namespaces map;
                // fall back to the info's own (canonical) namespace ident.
                let levels: &[String] = parent_namespaces.get(&info.namespace_id()).map_or_else(
                    || info.namespace().namespace_ident.as_ref().as_slice(),
                    |nwp| nwp.namespace_ident().as_ref().as_slice(),
                );
                self.gate_namespace(metadata, levels, namespace_action_is_write(action))
            })
            .collect())
    }

    async fn are_allowed_table_actions_impl<A: Into<Self::TableAction> + Send + Clone + Sync>(
        &self,
        metadata: &RequestMetadata,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnTable<'_, '_, impl AuthZTableInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(actions
            .iter()
            .map(|(ns, action_on_table)| {
                let action: CatalogTableAction = action_on_table.action.clone().into();
                // Namespace levels come from the caller-resolved tuple element
                // (`ns`); the table NAME comes from the action's own table ident.
                let table_ident = action_on_table.info.table_ident();
                self.gate_table(
                    metadata,
                    ns.namespace_ident().as_ref(),
                    &table_ident.name,
                    table_action_is_write(&action),
                )
            })
            .collect())
    }

    async fn are_allowed_view_actions_impl<A: Into<Self::ViewAction> + Send + Clone + Sync>(
        &self,
        metadata: &RequestMetadata,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnView<'_, '_, impl AuthZViewInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(actions
            .iter()
            .map(|(ns, action_on_view)| {
                let action: CatalogViewAction = action_on_view.action.clone().into();
                self.gate_object(
                    metadata,
                    ns.namespace_ident().as_ref(),
                    view_action_is_write(&action),
                )
            })
            .collect())
    }

    async fn are_allowed_generic_table_actions_impl<
        A: Into<Self::GenericTableAction> + Send + Clone + Sync,
    >(
        &self,
        metadata: &RequestMetadata,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnGenericTable<'_, '_, impl AuthZGenericTableInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        // TODO(CE-4): egret doesn't expose generic tables yet; gate conservatively
        // as write (the system principal still bypasses via gate_object(); token
        // principals fail-closed — never the old unconditional AllowAll).
        Ok(actions
            .iter()
            .map(|(ns, _)| self.gate_object(metadata, ns.namespace_ident().as_ref(), true))
            .collect())
    }

    async fn delete_user(&self, _metadata: &RequestMetadata, _user_id: UserId) -> Result<()> {
        Ok(())
    }

    async fn create_role(
        &self,
        _metadata: &RequestMetadata,
        _role_id: RoleId,
        _parent_project_id: ArcProjectId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_role(&self, _metadata: &RequestMetadata, _role_id: RoleId) -> Result<()> {
        Ok(())
    }

    async fn create_project(
        &self,
        _metadata: &RequestMetadata,
        _project_id: &ProjectId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_project(
        &self,
        _metadata: &RequestMetadata,
        _project_id: &ProjectId,
    ) -> Result<()> {
        Ok(())
    }

    async fn create_warehouse(
        &self,
        _metadata: &RequestMetadata,
        _warehouse_id: WarehouseId,
        _parent_project_id: &ProjectId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_warehouse(
        &self,
        _metadata: &RequestMetadata,
        _warehouse_id: WarehouseId,
    ) -> Result<()> {
        Ok(())
    }

    async fn create_namespace(
        &self,
        _metadata: &RequestMetadata,
        _namespace_id: NamespaceId,
        _parent: NamespaceParent,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_namespace(
        &self,
        _metadata: &RequestMetadata,
        _namespace_id: NamespaceId,
    ) -> Result<()> {
        Ok(())
    }

    async fn create_table(
        &self,
        _metadata: &RequestMetadata,
        _warehouse_id: WarehouseId,
        _table_id: TableId,
        _parent: NamespaceId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_table(&self, _warehouse_id: WarehouseId, _table_id: TableId) -> Result<()> {
        Ok(())
    }

    async fn create_view(
        &self,
        _metadata: &RequestMetadata,
        _warehouse_id: WarehouseId,
        _view_id: ViewId,
        _parent: NamespaceId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_view(&self, _warehouse_id: WarehouseId, _view_id: ViewId) -> Result<()> {
        Ok(())
    }

    async fn create_generic_table(
        &self,
        _metadata: &RequestMetadata,
        _warehouse_id: WarehouseId,
        _generic_table_id: GenericTableId,
        _parent: NamespaceId,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_generic_table(
        &self,
        _warehouse_id: WarehouseId,
        _generic_table_id: GenericTableId,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    fn target(levels: &[&str]) -> Vec<String> {
        levels.iter().map(|x| s(x)).collect()
    }

    // --- *_action_is_write -------------------------------------------------

    #[test]
    fn table_action_classification() {
        assert!(table_action_is_write(&CatalogTableAction::Drop));
        assert!(table_action_is_write(&CatalogTableAction::Commit {
            updated_properties: Arc::new(Default::default()),
            removed_properties: Arc::new(Vec::new()),
        }));
        assert!(table_action_is_write(&CatalogTableAction::WriteData));
        assert!(!table_action_is_write(&CatalogTableAction::ReadData));
        assert!(!table_action_is_write(&CatalogTableAction::GetMetadata));
        assert!(!table_action_is_write(&CatalogTableAction::IncludeInList));
    }

    #[test]
    fn namespace_action_classification() {
        assert!(namespace_action_is_write(&CatalogNamespaceAction::Delete));
        assert!(namespace_action_is_write(
            &CatalogNamespaceAction::SetProtection
        ));
        assert!(!namespace_action_is_write(
            &CatalogNamespaceAction::GetMetadata
        ));
        assert!(!namespace_action_is_write(
            &CatalogNamespaceAction::ListTables
        ));
        assert!(!namespace_action_is_write(
            &CatalogNamespaceAction::IncludeInList
        ));
    }

    #[test]
    fn view_action_classification() {
        assert!(view_action_is_write(&CatalogViewAction::Drop));
        assert!(view_action_is_write(&CatalogViewAction::Commit {
            updated_properties: Arc::new(Default::default()),
            removed_properties: Arc::new(Vec::new()),
        }));
        assert!(!view_action_is_write(&CatalogViewAction::GetMetadata));
        assert!(!view_action_is_write(&CatalogViewAction::Select));
        assert!(!view_action_is_write(&CatalogViewAction::IncludeInList));
    }

    // --- ns_prefix_matches / ns_prefix_matches_levels ----------------------

    #[test]
    fn ns_prefix_matching() {
        assert!(ns_prefix_matches("marts", &target(&["marts"])));
        assert!(ns_prefix_matches("marts", &target(&["marts", "sales"])));
        assert!(!ns_prefix_matches("marts", &target(&["marketing"])));
        assert!(ns_prefix_matches(
            "staging.raw",
            &target(&["staging", "raw", "t"])
        ));
        assert!(!ns_prefix_matches("staging.raw", &target(&["staging"])));
    }

    #[test]
    fn ns_prefix_matches_levels_is_ancestor_or_self() {
        // target is an ancestor-or-self of the grant ns → true (navigation).
        assert!(ns_prefix_matches_levels(&[], "marketing")); // root
        assert!(ns_prefix_matches_levels(
            &target(&["marketing"]),
            "marketing"
        ));
        assert!(ns_prefix_matches_levels(
            &target(&["staging"]),
            "staging.raw"
        ));
        // target deeper than the grant ns → false.
        assert!(!ns_prefix_matches_levels(
            &target(&["staging", "raw", "x"]),
            "staging.raw"
        ));
        // unrelated sibling → false.
        assert!(!ns_prefix_matches_levels(&target(&["sales"]), "marketing"));
    }

    // --- grant decisions: the OR matrix ------------------------------------

    fn g(ns: &str, table: Option<&str>, perms: &[&str]) -> Grant {
        Grant {
            namespace: ns.into(),
            table: table.map(|t| t.into()),
            permissions: perms.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn mixed_grants_or_semantics() {
        // `sale` whole-namespace read-only + `marketing.campaigns` read+write.
        let grants = vec![
            g("sale", None, &["catalog:read"]),
            g(
                "marketing",
                Some("campaigns"),
                &["catalog:read", "catalog:write"],
            ),
        ];
        // table actions
        assert!(decide_table(&grants, &target(&["sale"]), "orders", false)); // sale read
        assert!(!decide_table(&grants, &target(&["sale"]), "orders", true)); // sale write -> no
        assert!(decide_table(
            &grants,
            &target(&["marketing"]),
            "campaigns",
            true
        )); // mkt.campaigns write
        assert!(!decide_table(
            &grants,
            &target(&["marketing"]),
            "leads",
            false
        )); // other table
        assert!(!decide_table(
            &grants,
            &target(&["finance"]),
            "ledger",
            false
        )); // other ns
            // namespace navigation: a table-only grant lets you read its namespace + root
        assert!(decide_ns_read(&grants, &target(&["marketing"])));
        assert!(decide_ns_read(&grants, &[])); // root list
        assert!(!decide_ns_write(&grants, &target(&["marketing"]))); // no whole-ns write grant
        assert!(!decide_ns_write(&grants, &target(&["sale"]))); // sale is read-only
    }

    #[test]
    fn table_only_grant_navigation_and_write_boundary() {
        // A SINGLE table-only grant `marketing.campaigns` (read+write).
        let grants = vec![g(
            "marketing",
            Some("campaigns"),
            &["catalog:read", "catalog:write"],
        )];
        // Listing/reading namespace `marketing` (and root) is allowed to navigate.
        assert!(decide_ns_read(&grants, &target(&["marketing"])));
        assert!(decide_ns_read(&grants, &[]));
        // But creating a table in `marketing` is denied (no whole-ns write grant).
        assert!(!decide_ns_write(&grants, &target(&["marketing"])));
        // The granted table reads + writes; a sibling does neither.
        assert!(decide_table(
            &grants,
            &target(&["marketing"]),
            "campaigns",
            true
        ));
        assert!(!decide_table(
            &grants,
            &target(&["marketing"]),
            "leads",
            false
        ));
        // Reading an UNRELATED namespace is denied (grant doesn't relate).
        assert!(!decide_ns_read(&grants, &target(&["sales"])));
        // Views/generic tables need a whole-namespace grant → a table-only grant denies.
        assert!(!decide_object_ns(&grants, &target(&["marketing"]), false));
    }

    #[test]
    fn whole_namespace_grant_covers_views_and_subtree() {
        let grants = vec![g("staging", None, &["catalog:read", "catalog:write"])];
        // Object (view/generic) action covered by the whole-ns grant.
        assert!(decide_object_ns(&grants, &target(&["staging"]), false));
        assert!(decide_object_ns(&grants, &target(&["staging"]), true));
        // Subtree coverage for namespace write + tables.
        assert!(decide_ns_write(&grants, &target(&["staging", "raw"])));
        assert!(decide_table(
            &grants,
            &target(&["staging", "raw"]),
            "events",
            true
        ));
        // Outside the subtree → denied.
        assert!(!decide_ns_read(&grants, &target(&["prod"])));
    }

    #[test]
    fn star_grant_covers_all() {
        let grants = vec![g("*", None, &["catalog:read"])];
        assert!(decide_table(&grants, &target(&["anything"]), "t", false));
        assert!(!decide_table(&grants, &target(&["anything"]), "t", true));
        // `*` read covers namespace read + object read anywhere.
        assert!(decide_ns_read(&grants, &target(&["deep", "tree"])));
        assert!(decide_object_ns(&grants, &target(&["x"]), false));
        // but not writes (read-only).
        assert!(!decide_ns_write(&grants, &target(&["x"])));
        assert!(!decide_object_ns(&grants, &target(&["x"]), true));
    }

    #[test]
    fn empty_scope_denies() {
        assert!(!decide_table(&[], &target(&["sale"]), "orders", false));
        assert!(!decide_ns_read(&[], &target(&["sale"])));
        assert!(!decide_ns_read(&[], &[]));
        assert!(!decide_ns_write(&[], &target(&["sale"])));
        assert!(!decide_object_ns(&[], &target(&["sale"]), false));
    }

    // --- gate_*: the in-engine enforcers (defense in depth) ----------------
    //
    // These exercise `EgretAuthorizer::gate_table` / `gate_namespace` /
    // `gate_object` directly — the methods ALL data actions funnel through,
    // independent of egret's front-auth middleware. If the authorizer ever stops
    // enforcing (system bypass widens, claim path fails open, grant OR matrix
    // breaks), these fail. This is the CE-4 guarantee that the engine itself —
    // not just the front layer — denies out-of-scope requests.

    use lakekeeper::api::RequestMetadata;
    use lakekeeper::limes::Authentication;
    use lakekeeper::service::authn::Actor;
    use uuid::Uuid;

    /// Build a fixed, valid `UserId` for tests (distinct per `seed`).
    fn user_id(seed: &str) -> UserId {
        let subject = Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes());
        UserId::try_from(format!("oidc~{subject}").as_str()).expect("valid user id")
    }

    /// `RequestMetadata` for `principal` carrying an optional `egret` claim.
    fn metadata_with(principal: &UserId, claim: serde_json::Value) -> RequestMetadata {
        let authentication = Authentication::builder()
            .token_header(None)
            .claims(claim)
            .subject(principal.clone().into())
            .name(Some("test".to_string()))
            .email(None)
            .principal_type(None)
            .build();
        let mut md = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());
        md.set_authentication(Actor::Principal(principal.clone()), authentication);
        md
    }

    /// A `"egret"` claim carrying the given grants in the wire shape
    /// (`{ ns, table, perms }`), as `front_auth::token_request_metadata` builds.
    fn grants_claim(grants: &[Grant]) -> serde_json::Value {
        let arr: Vec<_> = grants
            .iter()
            .map(|g| {
                serde_json::json!({ "ns": g.namespace, "table": g.table, "perms": g.permissions })
            })
            .collect();
        serde_json::json!({ "egret": { "grants": arr } })
    }

    /// An `EgretAuthorizer` whose system principal is `sys`.
    fn authorizer(sys: UserId) -> EgretAuthorizer {
        EgretAuthorizer {
            server_id: ServerId::new(Uuid::nil()),
            system_principal: sys,
        }
    }

    #[test]
    fn egret_grants_parses_wire_claim() {
        let token = user_id("egret-catalog-token");
        let grants = vec![
            g("sale", None, &["catalog:read"]),
            g(
                "marketing",
                Some("campaigns"),
                &["catalog:read", "catalog:write"],
            ),
        ];
        let md = metadata_with(&token, grants_claim(&grants));
        assert_eq!(egret_grants(&md), Some(grants));
    }

    #[test]
    fn gate_allows_system_principal_unconditionally() {
        let sys = user_id("egret-system");
        let az = authorizer(sys.clone());
        // System principal, NO egret claim: still allowed for read AND write,
        // any namespace, for tables, namespaces, and objects.
        let md = metadata_with(&sys, serde_json::json!({}));
        assert!(az.gate_namespace(&md, &target(&["marketing"]), false));
        assert!(az.gate_namespace(&md, &target(&["anything", "deep"]), true));
        assert!(az.gate_table(&md, &target(&["sale"]), "customers", true));
        assert!(az.gate_object(&md, &target(&["sale"]), true));
    }

    #[test]
    fn gate_namespace_enforces_grants() {
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        let token = user_id("egret-catalog-token");
        // `marts` whole-namespace, read-only.
        let md = metadata_with(&token, grants_claim(&[g("marts", None, &["catalog:read"])]));
        // Allowed: read inside the `marts` subtree.
        assert!(az.gate_namespace(&md, &target(&["marts"]), false));
        assert!(az.gate_namespace(&md, &target(&["marts", "sales"]), false));
        // Denied: a different namespace.
        assert!(!az.gate_namespace(&md, &target(&["marketing"]), false));
        // Denied: a write, even within scope (read-only grant).
        assert!(!az.gate_namespace(&md, &target(&["marts"]), true));
    }

    #[test]
    fn gate_fails_closed_without_egret_claim() {
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        // A non-system principal with NO egret claim is denied everything.
        let stranger = user_id("stranger");
        let md = metadata_with(&stranger, serde_json::json!({}));
        assert!(!az.gate_namespace(&md, &target(&["marts"]), false));
        assert!(!az.gate_table(&md, &target(&["marts"]), "t", false));
        assert!(!az.gate_object(&md, &target(&["marts"]), false));
    }

    #[test]
    fn gate_table_enforces_grants_via_claim() {
        let sys = user_id("egret-system");
        let az = authorizer(sys.clone());
        // Token scoped to a single table grant in namespace `sale`.
        let token = user_id("egret-catalog-token");
        let md = metadata_with(
            &token,
            grants_claim(&[g("sale", Some("orders"), &["catalog:read"])]),
        );
        // The granted table is allowed for read.
        assert!(az.gate_table(&md, &target(&["sale"]), "orders", false));
        // A sibling table in the same namespace is denied.
        assert!(!az.gate_table(&md, &target(&["sale"]), "customers", false));
        // Write is denied even on the granted table (read-only).
        assert!(!az.gate_table(&md, &target(&["sale"]), "orders", true));
        // The system principal bypasses entirely.
        let sys_md = metadata_with(&sys, serde_json::json!({}));
        assert!(az.gate_table(&sys_md, &target(&["sale"]), "customers", true));
    }

    #[test]
    fn gate_mixed_grants_end_to_end() {
        // Through the real claim → egret_grants → decide_* path.
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        let token = user_id("egret-catalog-token");
        let md = metadata_with(
            &token,
            grants_claim(&[
                g("sale", None, &["catalog:read"]),
                g(
                    "marketing",
                    Some("campaigns"),
                    &["catalog:read", "catalog:write"],
                ),
            ]),
        );
        // sale: read table yes, write table no.
        assert!(az.gate_table(&md, &target(&["sale"]), "orders", false));
        assert!(!az.gate_table(&md, &target(&["sale"]), "orders", true));
        // marketing.campaigns: write yes; sibling no.
        assert!(az.gate_table(&md, &target(&["marketing"]), "campaigns", true));
        assert!(!az.gate_table(&md, &target(&["marketing"]), "leads", false));
        // Navigation: listing `marketing` allowed (table-only grant); create denied.
        assert!(az.gate_namespace(&md, &target(&["marketing"]), false));
        assert!(!az.gate_namespace(&md, &target(&["marketing"]), true));
        // Unrelated namespace fully denied.
        assert!(!az.gate_namespace(&md, &target(&["finance"]), false));
    }
}
