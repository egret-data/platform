//! egret's lakekeeper [`Authorizer`] (vended-credentials spec §4).
//!
//! The catalog guard (egret-bin) decides every Iceberg request before it
//! reaches lakekeeper. What it cannot decide is the **scope of the S3
//! credentials** lakekeeper vends on a table load or a `/credentials` call:
//! lakekeeper picks read-write-delete when its authorizer allows
//! `WriteData` on the table, read when it allows only `ReadData`. Under
//! lakekeeper's `AllowAllAuthorizer` every caller got read-write-delete,
//! a reader included (finding F4).
//!
//! So this authorizer answers exactly one question for real: may this
//! catalog client read, or write, this table's data? It answers it from
//! `catalog.catalog_grants` by the same rule the guard uses
//! ([`crate::grant_match`]). Every other question lakekeeper asks it is
//! answered `true`, as `AllowAllAuthorizer` answered it, because the guard
//! has already decided that request (spec §4.3 lists each method).
//!
//! Who is asking comes from the request's [`RequestMetadata`]:
//!
//! * a catalog client, which the mount names after the guard verified its
//!   token ([`catalog_client_metadata`]): the grants of its role decide;
//! * egret's own system principal (browse, recovery, warehouse admin, the
//!   lineage sweep): allowed, as before;
//! * lakekeeper's internal actor never reaches an authorizer (lakekeeper
//!   bypasses it);
//! * anyone else: refused data access (fail closed).

use std::collections::HashMap;

use lakekeeper::api::iceberg::v1::Result;
use lakekeeper::api::{ApiContext, RequestMetadata};
use lakekeeper::limes::Authentication;
use lakekeeper::service::authz::{
    ActionOnGenericTable, ActionOnTable, ActionOnView, AuthorizationBackendUnavailable, Authorizer,
    AuthzBackendErrorOrBadRequest, CatalogGenericTableAction, CatalogNamespaceAction,
    CatalogProjectAction, CatalogRoleAction, CatalogServerAction, CatalogTableAction,
    CatalogUserAction, CatalogViewAction, CatalogWarehouseAction, IsAllowedActionError,
    ListProjectsResponse, NamespaceParent, UserOrRole,
};
use lakekeeper::service::health::{Health, HealthExt};
use lakekeeper::service::{
    Actor, ArcProjectId, AuthZGenericTableInfo, AuthZNamespaceInfo, AuthZTableInfo, AuthZViewInfo,
    CatalogStore as LakekeeperStore, GenericTableId, NamespaceId, NamespaceWithParent, ProjectId,
    ResolvedWarehouse, Role, RoleId, SecretStore, ServerId, State, TableId, UserId, ViewId,
    WarehouseId,
};

use crate::grant_match::{catalog_grant_covers_by, CatalogNameEq};
use crate::store::CatalogStore;

/// The claim under which the mount records which catalog client sent a
/// request: `{"warehouse", "role", "subject"}` from its verified token.
pub const CATALOG_CLIENT_CLAIM: &str = "egret_catalog_client";

/// The IdP id of a catalog client's lakekeeper principal
/// (`egret-catalog~<client>`).
const CATALOG_CLIENT_IDP: &str = "egret-catalog";

/// A catalog client, as the guard verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogClientIdentity {
    /// The egret warehouse its token is scoped to (by name).
    pub warehouse: String,
    /// The catalog role whose grants it holds.
    pub role: String,
    /// The token's `sub` (the client id).
    pub subject: String,
}

/// The request metadata lakekeeper's handlers get for a request the guard
/// allowed: a principal that names the catalog client, so lakekeeper asks
/// [`EgretAuthorizer`] (the internal actor would skip it).
pub fn catalog_client_metadata(client: &CatalogClientIdentity) -> RequestMetadata {
    // A client id is egret-minted and short; a subject lakekeeper would
    // refuse as a user id (too long, control characters) is named by a
    // stable hash instead. Nothing decides on the user id: the claim does.
    let user_id = UserId::try_from(format!("{CATALOG_CLIENT_IDP}~{}", client.subject).as_str())
        .or_else(|_| {
            let hashed = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, client.subject.as_bytes());
            UserId::try_from(format!("{CATALOG_CLIENT_IDP}~{hashed}").as_str())
        })
        .expect("a UUID subject is a valid lakekeeper user id");
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({
            CATALOG_CLIENT_CLAIM: {
                "warehouse": client.warehouse,
                "role": client.role,
                "subject": client.subject,
            }
        }))
        .subject(user_id.clone().into())
        .name(Some(client.subject.clone()))
        .email(None)
        .principal_type(None)
        .build();
    let mut metadata = RequestMetadata::new_lakekeeper_internal(uuid::Uuid::now_v7());
    metadata.set_authentication(Actor::Principal(user_id), authentication);
    metadata
}

/// The catalog client a request's metadata names, if it names one.
fn catalog_client_of(metadata: &RequestMetadata) -> Option<CatalogClientIdentity> {
    let claim = metadata.authentication()?.claims(CATALOG_CLIENT_CLAIM)?;
    let field = |k: &str| claim.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some(CatalogClientIdentity {
        warehouse: field("warehouse")?,
        role: field("role")?,
        subject: field("subject")?,
    })
}

/// What a table action asks of the data decision.
#[derive(Debug, PartialEq, Eq)]
enum DataAsk {
    /// Not a data-plane action: the guard has decided it.
    NotData,
    /// May the caller read (`write: false`) or write the table's data?
    Ask { write: bool },
    /// A data question on behalf of another user: refused (fail closed).
    Refuse,
}

/// Classify one table action lakekeeper asks about.
///
/// `user` is set (lakekeeper has already folded "the caller itself" into
/// `None`) only when lakekeeper asks on behalf of another principal: a
/// DEFINER view chain (trusted engines, which egret does not configure) or a
/// for-user permission check (the management API, which egret does not
/// mount). egret's grants belong to the caller, so a data question about
/// another user is refused rather than answered with the caller's grants.
fn data_ask(action: &CatalogTableAction, user: Option<&UserOrRole>) -> DataAsk {
    match action {
        CatalogTableAction::ReadData | CatalogTableAction::WriteData if user.is_some() => {
            DataAsk::Refuse
        }
        CatalogTableAction::ReadData => DataAsk::Ask { write: false },
        CatalogTableAction::WriteData => DataAsk::Ask { write: true },
        _ => DataAsk::NotData,
    }
}

/// egret's lakekeeper authorizer. See the module docs.
#[derive(Clone)]
pub struct EgretAuthorizer {
    server_id: ServerId,
    /// egret's own principal ([`crate::bootstrap::system_user_id`]).
    system: UserId,
    /// The catalog RBAC store the grants are read from.
    grants: CatalogStore,
}

impl std::fmt::Debug for EgretAuthorizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgretAuthorizer")
            .field("server_id", &self.server_id)
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl EgretAuthorizer {
    pub fn new(server_id: ServerId, system: UserId, grants: CatalogStore) -> Self {
        Self {
            server_id,
            system,
            grants,
        }
    }

    /// For each `(namespace, table, write)`, may `client` read (or, with
    /// `write`, write) that table's data? The guard's rule
    /// ([`catalog_grant_covers_by`]), on the client's role's grants, in the
    /// warehouse its token names, which must be the one lakekeeper resolved.
    async fn client_may(
        &self,
        client: &CatalogClientIdentity,
        lakekeeper_warehouse_id: &str,
        asks: &[(Vec<String>, String, bool)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        let unavailable = |e: crate::store::CatalogError| {
            IsAllowedActionError::from(AuthorizationBackendUnavailable::new(e))
        };
        let scoped = self
            .grants
            .get_warehouse_by_name(&client.warehouse)
            .await
            .map_err(unavailable)?
            .and_then(|w| w.lakekeeper_warehouse_id)
            .is_some_and(|id| id == lakekeeper_warehouse_id);
        if !scoped {
            return Ok(vec![false; asks.len()]);
        }
        let grants = self
            .grants
            .grants_for_role(&client.warehouse, &client.role)
            .await
            .map_err(unavailable)?;
        let names: Vec<&str> = asks
            .iter()
            .flat_map(|(ns, table, _)| ns.iter().map(String::as_str).chain([table.as_str()]))
            .collect();
        let eq = CatalogNameEq::load(&self.grants, &grants, &names)
            .await
            .map_err(unavailable)?;
        Ok(asks
            .iter()
            .map(|(ns, table, write)| {
                grants.iter().any(|g| {
                    catalog_grant_covers_by(g, Some(ns), Some(table), *write, &|a, b| eq.eq(a, b))
                })
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl HealthExt for EgretAuthorizer {
    async fn health(&self) -> Vec<Health> {
        vec![]
    }
    async fn update_health(&self) {}
}

/// Who a request is, for the one decision this authorizer makes.
#[derive(Debug, PartialEq, Eq)]
enum Caller {
    Client(CatalogClientIdentity),
    System,
    Other,
}

impl Caller {
    /// Who `metadata` names, given egret's own principal `system`.
    fn of(system: &UserId, metadata: &RequestMetadata) -> Self {
        if let Some(client) = catalog_client_of(metadata) {
            return Self::Client(client);
        }
        match metadata.actor() {
            Actor::Principal(id) if id == system => Self::System,
            _ => Self::Other,
        }
    }

    /// The data-access answer that needs no grants: egret's own principal
    /// reads and writes every table, as it did under the allow-all
    /// authorizer; an unknown caller none (fail closed). A catalog client's
    /// grants decide (`None`).
    fn fixed_answer(&self) -> Option<bool> {
        match self {
            Self::System => Some(true),
            Self::Other => Some(false),
            Self::Client(_) => None,
        }
    }
}

#[async_trait::async_trait]
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

    fn new_router<C: LakekeeperStore, S: SecretStore>(
        &self,
    ) -> axum::Router<ApiContext<State<Self, C, S>>> {
        axum::Router::new()
    }

    // The management API (users, roles, projects, assume-role) is not mounted
    // for catalog clients; only egret's in-process system principal calls it
    // (spec §4.3). These answer as `AllowAllAuthorizer` does.

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

    // Warehouse and namespace actions (`/config`, list/create/drop
    // namespaces, create/register a table in one): the guard decides them
    // on the path, and the token's warehouse is the path's prefix.

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
        _metadata: &RequestMetadata,
        _for_user: Option<&UserOrRole>,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(&impl AuthZNamespaceInfo, Self::NamespaceAction)],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; actions.len()])
    }

    /// The one real decision. `ReadData` and `WriteData` (asked by a table
    /// load and `/credentials`, to pick the vended credentials' scope) follow
    /// the catalog client's grants. Every other table action (`GetMetadata`,
    /// `Commit`, `Drop`, `Rename`, `IncludeInList`, …) the guard has decided.
    async fn are_allowed_table_actions_impl<A: Into<Self::TableAction> + Send + Clone + Sync>(
        &self,
        metadata: &RequestMetadata,
        warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnTable<'_, '_, impl AuthZTableInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        // For each action: `Some(write)` for a data-plane action, `None` for
        // any other. Owned, so nothing borrowed from `actions` is held across
        // the store's awaits.
        // Per action: `None` for a control-plane action (allowed), `Some(None)`
        // for a refused data question, `Some(Some(write))` for one the grants
        // decide.
        let mut data_plane: Vec<Option<Option<bool>>> = Vec::with_capacity(actions.len());
        let mut asks: Vec<(Vec<String>, String, bool)> = Vec::new();
        for (_, a) in actions {
            let ask = match data_ask(&a.action.clone().into(), a.user) {
                DataAsk::NotData => None,
                DataAsk::Refuse => Some(None),
                DataAsk::Ask { write } => {
                    asks.push((
                        a.info.namespace_ident().clone().inner(),
                        a.info.table_ident().name().to_string(),
                        write,
                    ));
                    Some(Some(write))
                }
            };
            data_plane.push(ask);
        }
        if asks.is_empty() {
            return Ok(data_plane.iter().map(|d| d.is_none()).collect());
        }
        let caller = Caller::of(&self.system, metadata);
        let decided = match (caller.fixed_answer(), caller) {
            (Some(answer), _) => vec![answer; asks.len()],
            (None, Caller::Client(client)) => {
                self.client_may(&client, &warehouse.warehouse_id.to_string(), &asks)
                    .await?
            }
            (None, _) => vec![false; asks.len()],
        };
        let mut decided = decided.into_iter();
        let mut out = Vec::with_capacity(data_plane.len());
        for d in data_plane {
            out.push(match d {
                Some(Some(_)) => decided.next().unwrap_or(false),
                Some(None) => false,
                None => true,
            });
        }
        Ok(out)
    }

    // Views: a view load vends read credentials on the view's metadata
    // location, a view commit the guard requires write for. `Select` is
    // consulted only on DEFINER chains, which egret does not configure
    // (no trusted engines).

    async fn are_allowed_view_actions_impl<A: Into<Self::ViewAction> + Send + Clone + Sync>(
        &self,
        _metadata: &RequestMetadata,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnView<'_, '_, impl AuthZViewInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; actions.len()])
    }

    // Generic tables are a management-API feature egret does not mount.

    async fn are_allowed_generic_table_actions_impl<
        A: Into<Self::GenericTableAction> + Send + Clone + Sync,
    >(
        &self,
        _metadata: &RequestMetadata,
        _warehouse: &ResolvedWarehouse,
        _parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>,
        actions: &[(
            &NamespaceWithParent,
            ActionOnGenericTable<'_, '_, impl AuthZGenericTableInfo, A>,
        )],
    ) -> Result<Vec<bool>, IsAllowedActionError> {
        Ok(vec![true; actions.len()])
    }

    // Lifecycle hooks: an authorizer that keeps its own relation store
    // (OpenFGA) records objects here. egret's grants live in
    // `catalog.catalog_grants`, so there is nothing to record.

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
    use super::*;

    fn principal(id: &str) -> RequestMetadata {
        let user_id = UserId::try_from(id).expect("a user id");
        let authentication = Authentication::builder()
            .token_header(None)
            .claims(serde_json::json!({}))
            .subject(user_id.clone().into())
            .name(None)
            .email(None)
            .principal_type(None)
            .build();
        let mut m = RequestMetadata::new_lakekeeper_internal(uuid::Uuid::now_v7());
        m.set_authentication(Actor::Principal(user_id), authentication);
        m
    }

    fn system() -> UserId {
        crate::bootstrap::system_user_id("egret-system").expect("system id")
    }

    #[test]
    fn a_catalog_client_is_named_by_its_claim() {
        let client = CatalogClientIdentity {
            warehouse: "wh".into(),
            role: "reader".into(),
            subject: "client-1".into(),
        };
        let caller = Caller::of(&system(), &catalog_client_metadata(&client));
        assert_eq!(caller, Caller::Client(client));
        assert_eq!(caller.fixed_answer(), None, "its grants decide");
    }

    #[test]
    fn egret_s_own_principal_reads_and_writes() {
        let id = system().to_string();
        let caller = Caller::of(&system(), &principal(&id));
        assert_eq!(caller, Caller::System);
        assert_eq!(caller.fixed_answer(), Some(true));
    }

    #[test]
    fn any_other_principal_gets_no_data_access() {
        let caller = Caller::of(&system(), &principal("oidc~stranger"));
        assert_eq!(caller, Caller::Other);
        assert_eq!(caller.fixed_answer(), Some(false));
    }

    /// Finding 3 of the review: lakekeeper asks "may user X read this table"
    /// for a DEFINER view chain or a for-user check. egret's grants are the
    /// caller's, so such a question is refused, never answered with the
    /// caller's grants.
    #[test]
    fn a_data_question_on_behalf_of_another_user_is_refused() {
        let other = UserOrRole::User(UserId::try_from("oidc~someone-else").expect("a user id"));
        for action in [CatalogTableAction::ReadData, CatalogTableAction::WriteData] {
            assert_eq!(
                data_ask(&action, Some(&other)),
                DataAsk::Refuse,
                "{action:?}"
            );
        }
        assert_eq!(
            data_ask(&CatalogTableAction::ReadData, None),
            DataAsk::Ask { write: false }
        );
        assert_eq!(
            data_ask(&CatalogTableAction::WriteData, None),
            DataAsk::Ask { write: true }
        );
        assert_eq!(
            data_ask(&CatalogTableAction::GetMetadata, Some(&other)),
            DataAsk::NotData,
            "a control-plane question stays the guard's"
        );
    }

    /// C12: a grant store that cannot answer fails the decision; it never
    /// picks a scope.
    #[tokio::test]
    async fn a_grant_store_error_fails_the_decision() {
        let unreachable = egret_metastore::Db {
            pool: sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(std::time::Duration::from_millis(500))
                .connect_lazy("postgres://egret:egret@127.0.0.1:1/none")
                .expect("a lazy pool"),
        };
        let authorizer = EgretAuthorizer::new(
            ServerId::new_random(),
            system(),
            CatalogStore::new(unreachable),
        );
        let client = CatalogClientIdentity {
            warehouse: "wh".into(),
            role: "reader".into(),
            subject: "client-1".into(),
        };
        let asked = authorizer
            .client_may(
                &client,
                "lk-wh",
                &[(vec!["sales".into()], "orders".into(), false)],
            )
            .await;
        assert!(
            matches!(
                asked,
                Err(IsAllowedActionError::AuthorizationBackendUnavailable(_))
            ),
            "{asked:?}"
        );
    }

    /// A subject lakekeeper would refuse as a user id still names the client;
    /// only the principal's id is hashed.
    #[test]
    fn an_overlong_subject_still_names_the_client() {
        let client = CatalogClientIdentity {
            warehouse: "wh".into(),
            role: "r".into(),
            subject: "x".repeat(200),
        };
        let metadata = catalog_client_metadata(&client);
        assert_eq!(Caller::of(&system(), &metadata), Caller::Client(client));
    }
}
