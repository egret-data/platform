//! `egret-dataapi` — the Data API seam (P2): OAuth2 `client_credentials`
//! clients minted for service accounts, and the [`egret_machine_idp::ClientVerifier`]
//! that lets them exchange `client_id`/`client_secret` at the existing
//! `POST /oauth/token` for a JWT with `aud: "data-api"`.
//!
//! Registers a SECOND verifier on the shared machine-IdP rather than standing
//! up a parallel credential system — see `egret-catalog-engine`'s `CatalogStore`
//! for the first.
//!
//! Task 5 adds the workspace-scoped management surface ([`routes`]): create and
//! delete service accounts, list and issue their clients. Issuing gates on
//! `service_account:manage`, never `:read`.
//!
//! P3 Task 3 adds endpoint DRAFTS ([`endpoints`]) on the same surface, gated on
//! the separate `data_api_endpoint:read`/`:manage` pair. Draft CRUD only —
//! nothing in that module writes `state` or `live_version_id`, so editing cannot
//! publish an endpoint, and a published version's frozen contents are beyond its
//! reach.
//!
//! P3 Task 4 adds the publish [`lifecycle`]: validate, publish, roll back,
//! disable and re-enable. Publishing freezes the whole definition — including the
//! output schema resolved through [`egret_query_api::QueryExecPort::describe_as`]
//! — into an immutable `dataapi.endpoint_versions` snapshot. **That snapshot is
//! the only thing P4's serving path may read to decide what a request returns**;
//! `lifecycle`'s module comment carries the full contract, including why routing
//! is the one thing that still comes from the endpoint row.
//!
//! P4 Task 3 adds [`serving::serving_router`]: `/data-api/v1/{ws}/{*path}` →
//! the live snapshot → rows. It is mounted as its OWN group with no CSRF layer
//! (nothing is ambient on a bearer-authenticated surface) and outside `/api`.
//!
//! P4 Task 2 opens the [`serving`] surface with its authentication half:
//! [`serving::verify_serving_token`] turns a bearer token into a principal and a
//! workspace. It asserts `aud` and `exp` ITSELF, because
//! `egret_machine_idp::verify_with_jwk` checks NEITHER and accepts a token
//! carrying no `exp` at all — that module's comment carries the reasoning.

//! P5 §3 adds [`try_it`]: `POST …/endpoints/{id}/try` runs a published endpoint
//! AS one of the workspace's service accounts, from the console. It drives the
//! REAL [`serving_router`] in process through its tower `Service` — not a
//! re-implemented equivalent and not an HTTP loopback — so a trial cannot
//! report OK on a path a consumer is refused. See its module comment.

//! P4.5 adds [`render`]: an endpoint's SQL and a caller's values become
//! `(sql, Vec<Param>)` — a statement whose every `{{ … }}` has become a bind
//! marker, and the values to bind to it. It is the only SQL-building surface in
//! egret that is safe by construction rather than by escaper; its module comment
//! carries the mechanism and the rules that keep it that way.

pub mod activity;
pub mod clients;
mod endpoint_port;
pub mod endpoints;
mod invoke;
pub mod lifecycle;
pub mod openapi_gen;
/// P6 §2: preview an author's UNSAVED draft. Private because the route in
/// [`routes`] is its only caller — nothing outside this crate constructs a
/// preview.
mod preview;
pub mod render;
pub mod routes;
pub mod serving;
/// C2 Task 1: the Starlark evaluator a response transform runs in, and the
/// budgets it runs under. Private for now — Task 3 gives it a caller and decides
/// what, if anything, the rest of the crate needs to see.
mod transform;
pub mod try_it;

pub use activity::{
    ActivityCallDto, ActivityRange, ActivityRow, ActivityStore, ActivitySummaryDto,
};
pub use clients::{ClientStore, NewClient, DATA_API_TTL_SECS};
pub use endpoints::{
    CreateEndpointInput, DataApiEndpointDto, DataApiEndpointListItemDto, DeletedEndpoint,
    EndpointStore, UpdateEndpointInput, ALLOWED_METHODS, BOOKMARK_ENTITY, OBJECT_KIND,
};
pub use lifecycle::{
    check_cache_scope, check_enum_params, check_governed_connection, check_params,
    check_single_select, validate, ConnectionSafety, DataApiEndpointVersionDto,
    DataApiPublishedDto, ValidationError, VersionRef,
};
pub use render::{check_dynamic, check_publish, check_static, render, ParamValues, RenderError};
pub use routes::{
    management_router, openapi, CreateServiceAccountInput, DataApiAdminApi, DataApiClientDto,
    DataApiEndpointGrantDto, DataApiError, DataApiNewClientDto, DataApiServiceAccountDto,
    DataApiState, GrantInvokeInput, IssueClientInput, PublishEndpointInput, RollbackEndpointInput,
};
pub use serving::{
    serving_openapi, serving_router, verify_serving_token, MetaContract, ServingError, ServingMeta,
    ServingPrincipal, ServingResponse, ServingStatus, StatementMeta, DATA_API_AUDIENCE,
};
pub use try_it::{
    trial_serving_router, TrialActor, TrialResultTooLarge, TryItError, TryRequest, TryRequestEcho,
    TryResponse, TRIAL_MAX_CONCURRENCY, TRIAL_REQUEST_DEADLINE,
};

/// Test-only access to the trial-token minter.
///
/// The trial token is minted, used in-process against the serving router, and
/// discarded — `tests/try_it.rs`'s `the_token_never_reaches_the_caller` pins
/// that it is never echoed to the author. So the ONLY way an integration test
/// can assert what claims it carries is a shim like this one. The claim that
/// matters is `iss`: it is the machine-IdP issuer and must NOT follow the
/// consumer-facing base URL when a deployment sets the two apart.
///
/// Same shape as [`can_invoke_for_test`] below: the real function stays
/// crate-private, so no production caller can drift onto a second copy.
#[doc(hidden)]
pub async fn mint_trial_token_for_test(
    st: &crate::routes::DataApiState,
    workspace_id: uuid::Uuid,
    service_account_id: uuid::Uuid,
) -> Result<String, crate::routes::DataApiError> {
    crate::try_it::mint_trial_token(st, workspace_id, service_account_id).await
}

/// Test-only access to the shared invoke expression (P5 §5.2). Integration
/// tests live outside the crate and cannot see `pub(crate)`; exposing it through
/// a named shim keeps the real function crate-private, so no production caller
/// can drift onto a second copy of this decision.
#[doc(hidden)]
pub async fn can_invoke_for_test(
    st: &crate::routes::DataApiState,
    user_id: egret_core::ids::UserId,
    ws: egret_core::WorkspaceId,
    endpoint_id: uuid::Uuid,
) -> Result<bool, egret_core::Error> {
    crate::invoke::can_invoke(st, user_id, ws, endpoint_id).await
}
