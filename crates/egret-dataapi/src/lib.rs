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
//! nothing here writes `state` or `live_version_id`, so editing cannot publish
//! an endpoint, and a published version's frozen contents are beyond its reach.
//! A live endpoint's URL is not — `path`/`method` live on the draft row and have
//! no state guard yet; see `endpoints`' module comment.

pub mod clients;
pub mod endpoints;
pub mod routes;

pub use clients::{ClientStore, NewClient, DATA_API_TTL_SECS};
pub use endpoints::{
    CreateEndpointInput, DataApiEndpointDto, EndpointStore, UpdateEndpointInput, ALLOWED_METHODS,
};
pub use routes::{
    management_router, openapi, CreateServiceAccountInput, DataApiAdminApi, DataApiClientDto,
    DataApiError, DataApiNewClientDto, DataApiServiceAccountDto, DataApiState, IssueClientInput,
};
