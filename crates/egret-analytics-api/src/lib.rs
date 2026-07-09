//! `egret-analytics-api` — leaf contract crate for the Analytics/Notebook seam.
//!
//! Holds the notebook-docs vocabulary that `egret-kernel` needs from the
//! Analytics seam without pulling in `egret-notebook`'s sqlx/axum machinery:
//!
//! - Pure-serde DTOs — [`NotebookSummary`], [`EngineOption`],
//!   [`NotebookRuntime`], [`CellOutput`].
//! - The engine-token seam types — [`EngineToken`], [`EngineTokenMinter`],
//!   [`BrokerError`] (see [`engine_token`]).
//! - The [`NotebookDocsPort`] reader/writer trait: `egret-kernel` depends on
//!   `Arc<dyn NotebookDocsPort>`, and `egret-notebook` implements it over its
//!   concrete `NotebookState`.
//!
//! No sqlx, no axum: this crate stays a light leaf so the stateful Kernel
//! runtime seam is decoupled from the concrete Analytics seam. Mirrors
//! `egret-query-api` (E4d) / `egret-catalog-api` (E3b) / `egret-policy-api`.

pub mod dto;
pub mod engine_token;
pub mod port;

pub use dto::{CellOutput, EngineOption, NotebookRuntime, NotebookSummary};
pub use engine_token::{BrokerError, EngineToken, EngineTokenMinter};
pub use port::NotebookDocsPort;
