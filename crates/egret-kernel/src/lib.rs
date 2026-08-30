//! `egret-kernel` — per-(user×workspace) Jupyter Kernel Gateway lifecycle.

pub mod engine_conn;
pub mod env;
pub mod error;
pub mod gateway;
pub mod jmsg;
pub mod kernel_token;
pub mod launcher;
pub mod manager;
pub mod notebook_kernels;
pub mod proxy;
pub mod reaper;
pub mod recorder;
pub mod registry;
pub mod routes;
pub mod subprocess;

pub use engine_conn::{EngineConn, EngineConnError, EngineConnResolver};
pub use launcher::KernelLauncher;
pub use manager::{KernelManager, KernelStatus};
pub use notebook_kernels::NotebookKernelStore;
pub use proxy::{BackendWsProxy, KernelProxy};
pub use reaper::{reap_once, spawn_reaper};
pub use registry::KernelRegistry;
pub use routes::{kernel_callback_router, kernel_router, KernelState};
pub use subprocess::SubprocessLauncher;

/// The Kernel seam's OpenAPI document (E0·P4b-8): the session-cookie kernel
/// lifecycle/channels router (`KernelApi`) merged with the kernel-token bearer
/// callback router (`KernelCallbackApi`). `egret-server` calls this and merges it
/// into the top-level API doc — the per-crate `openapi()` convention every
/// annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = routes::KernelApi::openapi();
    doc.merge(routes::KernelCallbackApi::openapi());
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
