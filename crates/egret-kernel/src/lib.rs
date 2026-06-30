//! `egret-kernel` — per-(user×workspace) Jupyter Kernel Gateway lifecycle.

pub mod env;
pub mod error;
pub mod launcher;
pub mod manager;
pub mod registry;
pub mod routes;
pub mod subprocess;

pub use manager::{KernelManager, KernelStatus};
pub use routes::{kernel_router, KernelState};
