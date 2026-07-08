//! `egret-auth` — authentication (local), sessions, and the request `Principal`.

pub mod authz;
pub mod cookie;
pub mod csrf;
pub mod groups;
pub mod model;
pub mod oidc;
pub mod oidc_cookie;
pub mod oidc_refresh;
pub mod oidc_routes;
pub mod password;
pub mod principal;
pub mod ratelimit;
pub mod roles;
pub mod routes;
pub mod session;
pub mod token;
pub mod users;

pub use authz::{require_permission, require_platform_admin};
