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
