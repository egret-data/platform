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

/// True if `scope_entry` (a dot-joined namespace prefix, e.g. `"staging.raw"`)
/// is a subtree-prefix of `target` (the target namespace's dot-split levels).
fn ns_prefix_matches(scope_entry: &str, target: &[String]) -> bool {
    let want: Vec<&str> = scope_entry.split('.').collect();
    want.len() <= target.len() && want.iter().zip(target).all(|(a, b)| a == b)
}

/// Core scope decision: the principal needs the right permission and (when
/// `ns_scope` is non-empty) a subtree-prefix match against `target_ns`.
fn decide_scope(
    perms: &[String],
    ns_scope: &[String],
    target_ns: &[String],
    is_write: bool,
) -> bool {
    let need = if is_write { PERM_WRITE } else { PERM_READ };
    if !perms.iter().any(|p| p == need) {
        return false;
    }
    if ns_scope.is_empty() {
        return true;
    }
    ns_scope.iter().any(|e| ns_prefix_matches(e, target_ns))
}

/// Table-action decision: the namespace-level `decide_scope` must pass AND, when
/// `table_scope` is non-empty, the target table's FQN (`target_ns.join(".") +
/// "." + table_name`) must be an exact member of `table_scope`.
///
/// An empty `table_scope` falls back to the namespace decision unchanged, so a
/// token with no table scope keeps its exact prior behaviour.
fn decide_table_scope(
    perms: &[String],
    ns_scope: &[String],
    table_scope: &[String],
    target_ns: &[String],
    table_name: &str,
    is_write: bool,
) -> bool {
    if !decide_scope(perms, ns_scope, target_ns, is_write) {
        return false;
    }
    if table_scope.is_empty() {
        return true;
    }
    let mut fqn = target_ns.join(".");
    fqn.push('.');
    fqn.push_str(table_name);
    table_scope.iter().any(|t| t == &fqn)
}

/// Read a `"egret"` claim array field (`ns` / `tables`) into a `Vec<String>`,
/// defaulting to empty when the field is absent or not an array.
fn claim_string_array(claim: &serde_json::Value, field: &str) -> Vec<String> {
    claim
        .get(field)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract `(perms, ns, tables)` from the request's `"egret"` authentication
/// claim. Returns `None` (fail-closed) when there is no authentication or no
/// claim. `ns` / `tables` default to empty when absent.
fn egret_claim(metadata: &RequestMetadata) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
    let v = metadata.authentication()?.claims("egret")?;
    let perms = v
        .get("perms")?
        .as_array()?
        .iter()
        .filter_map(|x| x.as_str().map(String::from))
        .collect();
    let ns = claim_string_array(v, "ns");
    let tables = claim_string_array(v, "tables");
    Some((perms, ns, tables))
}

impl EgretAuthorizer {
    /// Shared data-action gate: bypass for the system principal, else evaluate
    /// the egret scope claim against `target_ns` and the action's write-ness.
    fn gate(&self, metadata: &RequestMetadata, target_ns: &[String], is_write: bool) -> bool {
        // `user_id()` (not `actor()`) is the right bypass identity here: the
        // system principal has a fixed `UserId` and never assumes a role, so the
        // underlying user id is always the in-process principal we want to match.
        if metadata.user_id() == Some(&self.system_principal) {
            return true;
        }
        match egret_claim(metadata) {
            Some((perms, ns, _tables)) => decide_scope(&perms, &ns, target_ns, is_write),
            None => false,
        }
    }

    /// Table-action gate: like [`EgretAuthorizer::gate`] but also enforces the
    /// token's `table_scope` against the target table's namespace + name. The
    /// system principal still bypasses entirely; an absent claim fails closed.
    fn gate_table(
        &self,
        metadata: &RequestMetadata,
        target_ns: &[String],
        table_name: &str,
        is_write: bool,
    ) -> bool {
        if metadata.user_id() == Some(&self.system_principal) {
            return true;
        }
        match egret_claim(metadata) {
            Some((perms, ns, tables)) => {
                decide_table_scope(&perms, &ns, &tables, target_ns, table_name, is_write)
            }
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
                self.gate(metadata, levels, namespace_action_is_write(action))
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
                self.gate(
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
        // as write (the system principal still bypasses via gate(); token
        // principals fail-closed — never the old unconditional AllowAll).
        Ok(actions
            .iter()
            .map(|(ns, _)| self.gate(metadata, ns.namespace_ident().as_ref(), true))
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

    // --- ns_prefix_matches -------------------------------------------------

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

    // --- decide_scope ------------------------------------------------------

    #[test]
    fn decide_scope_perms() {
        // read-only perm cannot write
        assert!(!decide_scope(
            &[s("catalog:read")],
            &[],
            &target(&["x"]),
            true
        ));
        // read+write perm can write
        assert!(decide_scope(
            &[s("catalog:read"), s("catalog:write")],
            &[],
            &target(&["x"]),
            true
        ));
        // read-only perm can read
        assert!(decide_scope(
            &[s("catalog:read")],
            &[],
            &target(&["x"]),
            false
        ));
    }

    #[test]
    fn decide_scope_namespace() {
        // empty ns scope => unrestricted
        assert!(decide_scope(
            &[s("catalog:read")],
            &[],
            &target(&["anything", "goes"]),
            false
        ));
        // ns scope ["marts"] does not match target ["marketing"]
        assert!(!decide_scope(
            &[s("catalog:read")],
            &[s("marts")],
            &target(&["marketing"]),
            false
        ));
        // ns scope ["marts"] matches target ["marts", "x"]
        assert!(decide_scope(
            &[s("catalog:read")],
            &[s("marts")],
            &target(&["marts", "x"]),
            false
        ));
    }

    // --- decide_table_scope ------------------------------------------------

    #[test]
    fn table_scope_allows_listed_table_and_denies_others() {
        // perms=read, ns empty, tables=["sale.orders"]
        let perms = vec![s("catalog:read")];
        let ns: Vec<String> = vec![];
        let tables = vec![s("sale.orders")];
        // target table sale.orders → allowed (read)
        assert!(decide_table_scope(
            &perms,
            &ns,
            &tables,
            &target(&["sale"]),
            "orders",
            false
        ));
        // sibling sale.customers → denied
        assert!(!decide_table_scope(
            &perms,
            &ns,
            &tables,
            &target(&["sale"]),
            "customers",
            false
        ));
    }

    #[test]
    fn empty_table_scope_falls_back_to_namespace_decision() {
        let perms = vec![s("catalog:read")];
        // No table scope → unrestricted at table level (ns decision governs).
        assert!(decide_table_scope(
            &perms,
            &[],
            &[],
            &target(&["sale"]),
            "anything",
            false
        ));
        // Even with a matching table scope, a read-only perm cannot write.
        assert!(!decide_table_scope(
            &perms,
            &[],
            &[s("sale.orders")],
            &target(&["sale"]),
            "orders",
            true
        ));
    }

    #[test]
    fn table_scope_respects_namespace_boundary() {
        // table FQN listed but namespace scope excludes the target → denied,
        // because the ns gate must pass first.
        let perms = vec![s("catalog:read")];
        assert!(!decide_table_scope(
            &perms,
            &[s("marts")],
            &[s("sale.orders")],
            &target(&["sale"]),
            "orders",
            false
        ));
    }

    #[test]
    fn table_scope_matches_nested_namespace_fqn() {
        // Nested namespace: the FQN is the dotted ns levels + table name.
        let perms = vec![s("catalog:read")];
        let tables = vec![s("staging.raw.events")];
        assert!(decide_table_scope(
            &perms,
            &[],
            &tables,
            &target(&["staging", "raw"]),
            "events",
            false
        ));
        // Same name in a different namespace is NOT a match.
        assert!(!decide_table_scope(
            &perms,
            &[],
            &tables,
            &target(&["staging"]),
            "events",
            false
        ));
    }

    // --- gate_table: the in-engine table-FQN enforcer ----------------------

    // --- gate: the in-engine enforcer (defense in depth) -------------------
    //
    // These exercise `EgretAuthorizer::gate` directly — the method ALL data
    // actions funnel through, independent of egret's front-auth middleware. If
    // the authorizer ever stops enforcing (system bypass widens, claim path
    // fails open, ns boundary drops), these fail. This is the CE-4 guarantee
    // that the engine itself — not just the front layer — denies out-of-scope
    // requests.

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

    /// An `EgretAuthorizer` whose system principal is `sys`.
    fn authorizer(sys: UserId) -> EgretAuthorizer {
        EgretAuthorizer {
            server_id: ServerId::new(Uuid::nil()),
            system_principal: sys,
        }
    }

    #[test]
    fn gate_allows_system_principal_unconditionally() {
        let sys = user_id("egret-system");
        let az = authorizer(sys.clone());
        // System principal, NO egret claim: still allowed for read AND write,
        // any namespace. This is the bootstrap/browse bypass.
        let md = metadata_with(&sys, serde_json::json!({}));
        assert!(az.gate(&md, &target(&["marketing"]), false));
        assert!(az.gate(&md, &target(&["anything", "deep"]), true));
    }

    #[test]
    fn gate_denies_token_principal_out_of_namespace_scope() {
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        // A token principal (NOT the system one) scoped to `marts`, read-only.
        let token = user_id("egret-catalog-token");
        let md = metadata_with(
            &token,
            serde_json::json!({ "egret": { "perms": ["catalog:read"], "ns": ["marts"] } }),
        );
        // Allowed: read inside the `marts` subtree.
        assert!(az.gate(&md, &target(&["marts"]), false));
        assert!(az.gate(&md, &target(&["marts", "sales"]), false));
        // Denied: a different namespace (the namespace_scope boundary).
        assert!(!az.gate(&md, &target(&["marketing"]), false));
        // Denied: a write, even within scope (read-only perm).
        assert!(!az.gate(&md, &target(&["marts"]), true));
    }

    #[test]
    fn gate_fails_closed_without_egret_claim() {
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        // A non-system principal with NO egret claim is denied everything.
        let stranger = user_id("stranger");
        let md = metadata_with(&stranger, serde_json::json!({}));
        assert!(!az.gate(&md, &target(&["marts"]), false));
        assert!(!az.gate(&md, &target(&["marts"]), true));
    }

    #[test]
    fn gate_table_enforces_table_scope_via_claim() {
        let sys = user_id("egret-system");
        let az = authorizer(sys.clone());
        // Token scoped to a single table FQN in namespace `sale`.
        let token = user_id("egret-catalog-token");
        let md = metadata_with(
            &token,
            serde_json::json!({
                "egret": { "perms": ["catalog:read"], "ns": [], "tables": ["sale.orders"] }
            }),
        );
        // The listed table is allowed for read.
        assert!(az.gate_table(&md, &target(&["sale"]), "orders", false));
        // A sibling table in the same namespace is denied.
        assert!(!az.gate_table(&md, &target(&["sale"]), "customers", false));
        // Write is denied even on the listed table (read-only perm).
        assert!(!az.gate_table(&md, &target(&["sale"]), "orders", true));
        // The system principal bypasses table scope entirely.
        let sys_md = metadata_with(&sys, serde_json::json!({}));
        assert!(az.gate_table(&sys_md, &target(&["sale"]), "customers", true));
    }

    #[test]
    fn gate_table_without_table_scope_matches_namespace_gate() {
        // An egret claim with no `tables` field must behave exactly like the
        // namespace-only `gate` — no new restriction is introduced.
        let sys = user_id("egret-system");
        let az = authorizer(sys);
        let token = user_id("egret-catalog-token");
        let md = metadata_with(
            &token,
            serde_json::json!({ "egret": { "perms": ["catalog:read"], "ns": ["sale"] } }),
        );
        // In scope → allowed for any table name.
        assert!(az.gate_table(&md, &target(&["sale"]), "orders", false));
        assert!(az.gate_table(&md, &target(&["sale"]), "customers", false));
        // Out of namespace scope → denied.
        assert!(!az.gate_table(&md, &target(&["marketing"]), "orders", false));
    }
}
