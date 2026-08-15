//! P4 — the Data API **serving** surface: the public, token-authenticated path
//! that runs a published endpoint. Everything under `/api` is session- and
//! CSRF-protected and belongs to a human; this surface belongs to a machine and
//! is authenticated by a bearer token alone.
//!
//! Task 2 lands [`auth`]: bearer token → [`ServingPrincipal`]. Routing, the
//! response cache, the rate limiter and the access log arrive in Tasks 3-5.

pub mod auth;

pub use auth::{verify_serving_token, ServingPrincipal, DATA_API_AUDIENCE};

/// A serving-path failure.
///
/// Every authentication failure — wrong audience, expired, missing `exp`, bad
/// signature, unparseable identity claim — is the SAME [`Self::Unauthorized`].
/// Distinguishing them would hand anyone probing the surface a free oracle:
/// "expired" tells them the signature was good, "wrong audience" tells them the
/// token is a real egret token. The caller learns only that authentication
/// failed; the reason goes to the log.
///
/// Task 5 adds the remaining variants (`NotFound`, `ParamInvalid`,
/// `RateLimited`, …) and the wire mapping in `serving/error.rs`.
#[derive(Debug, thiserror::Error)]
pub enum ServingError {
    #[error("unauthorized")]
    Unauthorized,
    /// Something on egret's side failed — the IdP key store was unreachable, say.
    /// Deliberately NOT folded into [`Self::Unauthorized`]: a 401 tells an
    /// operator their credential is bad and sends them to rotate it, while the
    /// actual fault is ours. Task 5 maps this to a 500 with a fixed message.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}
