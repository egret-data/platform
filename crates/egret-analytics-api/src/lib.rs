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
//! - The pure-serde BI wire DTOs (see [`bi`]) — dataset/chart/dashboard/SQL-Lab
//!   request & response shapes moved verbatim from `egret-bi::model`; `egret-bi`
//!   re-exports them, keeping its domain→wire conversions and `From<Row>` impls.
//!
//! No sqlx, no axum: this crate stays a light leaf so the stateful Kernel
//! runtime seam is decoupled from the concrete Analytics seam. Mirrors
//! `egret-query-api` (E4d) / `egret-catalog-api` (E3b) / `egret-policy-api`.

pub mod bi;
pub mod dto;
pub mod engine_token;
pub mod port;

// The BI wire DTOs stay namespaced under `egret_analytics_api::bi::…` (29 types
// with generic names like `CreateChart`/`CardInput` — a root glob would pollute
// the crate root and risk colliding with `dto::*`); `egret-bi` imports them via
// the `bi` module path. The notebook-docs types below are few and distinctive,
// so they're re-exported flat for `egret-kernel`/`egret-notebook` ergonomics.
pub use dto::{CellOutput, EngineOption, NotebookRuntime, NotebookSummary};
pub use engine_token::{BrokerError, EngineToken, EngineTokenMinter};
pub use port::NotebookDocsPort;
