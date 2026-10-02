//! `egret-auth` — authentication (local), sessions, and the request `Principal`.

pub mod cookie;
pub mod csrf;
pub mod effective;
pub mod events;
pub mod fetch_meta;
pub mod grants;
pub mod groups;
pub mod impersonation;
pub mod model;
pub mod oidc;
pub mod oidc_cookie;
pub mod oidc_routes;
pub mod password;
pub mod pdp;
pub mod principal;
pub mod roles;
pub mod routes;
pub mod session;
pub mod token;
pub mod users;

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

/// The `--lib` binary of this crate runs `#[sqlx::test]` cases too, and they are the ones
/// this crate's `tests/` guards do not cover: a `tests/<name>.rs` guard lives in a
/// different binary and therefore a different process.
///
/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported variable does
/// not skip those tests — it silently points them at the database `.env` names, which is
/// the DEV one, and they report `ok` having written scratch databases into it. See
/// `egret_test_db::assert_database_url_is_exported`, and
/// `egret-test-db/tests/every_sqlx_test_binary_guards_the_dotenv_fallback.rs` for the rule
/// that keeps this from being forgotten in the next binary.
#[cfg(test)]
mod dotenv_fallback_guard {
    #[test]
    fn database_url_is_exported_rather_than_read_from_dotenv() {
        egret_test_db::assert_database_url_is_exported();
    }
}
