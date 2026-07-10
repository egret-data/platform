//! `egret-auth` — authentication (local), sessions, and the request `Principal`.

pub mod authz;
pub mod cookie;
pub mod csrf;
pub mod events;
pub mod grants;
pub mod groups;
pub mod model;
pub mod oidc;
pub mod oidc_cookie;
pub mod oidc_refresh;
pub mod oidc_routes;
pub mod password;
pub mod pdp;
pub mod principal;
pub mod ratelimit;
pub mod roles;
pub mod routes;
pub mod session;
pub mod token;
pub mod users;

pub use authz::{require_permission, require_platform_admin};

/// The Auth seam's OpenAPI document (E0·P4b-1): the local-auth router
/// (`/api/auth`) and the OIDC router (`/api/auth/oidc`) merged into one
/// `OpenApi`. `egret-server` merges this into the top-level API doc. This is the
/// per-crate `openapi()` convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = routes::AuthApi::openapi();
    doc.merge(oidc_routes::OidcApi::openapi());
    doc
}
