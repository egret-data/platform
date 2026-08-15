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

pub mod clients;
pub mod endpoints;
pub mod lifecycle;
pub mod routes;
pub mod serving;

pub use clients::{ClientStore, NewClient, DATA_API_TTL_SECS};
pub use endpoints::{
    CreateEndpointInput, DataApiEndpointDto, DeletedEndpoint, EndpointStore, UpdateEndpointInput,
    ALLOWED_METHODS,
};
pub use lifecycle::{
    check_cache_scope, check_enum_params, check_params, check_single_select, validate,
    DataApiEndpointVersionDto, DataApiPublishedDto, ValidationError, VersionRef,
};
pub use routes::{
    management_router, openapi, CreateServiceAccountInput, DataApiAdminApi, DataApiClientDto,
    DataApiError, DataApiNewClientDto, DataApiServiceAccountDto, DataApiState, IssueClientInput,
    PublishEndpointInput, RollbackEndpointInput,
};
pub use serving::{
    serving_openapi, serving_router, verify_serving_token, ServingError, ServingMeta,
    ServingPrincipal, ServingResponse, DATA_API_AUDIENCE,
};
