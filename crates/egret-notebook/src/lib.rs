//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod docs_port;
pub mod engine_token;
pub mod error;
pub mod model;
pub mod output;
pub mod routes;
pub mod runtime;
pub mod store;

pub use routes::{notebook_router, NotebookState};
pub use runtime::{build_notebook_runtime, NotebookRuntime};
pub use store::NotebookStore;

/// The Notebook seam's OpenAPI document (E0·P4b-8): the workspace-scoped notebook
/// router (CRUD, cells, folders, trash, bookmarks, settings, runtime manifest).
/// `egret-server` calls this and merges it into the top-level API doc — the
/// per-crate `openapi()` convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    routes::NotebookApi::openapi()
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
