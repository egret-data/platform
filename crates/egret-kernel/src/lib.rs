//! `egret-kernel` — per-(user×workspace) Jupyter Kernel Gateway lifecycle.

pub mod env;
pub mod error;
pub mod launcher;
pub mod manager;
pub mod notebook_kernels;
pub mod reaper;
pub mod registry;
pub mod routes;
pub mod subprocess;

pub use launcher::KernelLauncher;
pub use manager::{KernelManager, KernelStatus};
pub use notebook_kernels::NotebookKernelStore;
pub use reaper::{reap_once, spawn_reaper};
pub use registry::KernelRegistry;
pub use routes::{kernel_router, KernelState};
pub use subprocess::SubprocessLauncher;
