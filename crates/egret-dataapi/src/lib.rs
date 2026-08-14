//! `egret-dataapi` — the Data API seam (P2): OAuth2 `client_credentials`
//! clients minted for service accounts, and the [`egret_machine_idp::ClientVerifier`]
//! that lets them exchange `client_id`/`client_secret` at the existing
//! `POST /oauth/token` for a JWT with `aud: "data-api"`.
//!
//! Registers a SECOND verifier on the shared machine-IdP rather than standing
//! up a parallel credential system — see `egret-catalog-engine`'s `CatalogStore`
//! for the first. Management routes (create/revoke over HTTP) land in Task 5.

pub mod clients;

pub use clients::{ClientStore, NewClient, DATA_API_TTL_SECS};
