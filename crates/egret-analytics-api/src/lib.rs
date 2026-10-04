//! `egret-analytics-api` — leaf contract crate for the Analytics seam.
//!
//! Holds:
//!
//! - The engine-token seam types — [`EngineToken`], [`EngineTokenMinter`],
//!   [`BrokerError`] (see [`engine_token`]), shared by `EngineState` (SQL Lab
//!   delegation) and `BiState` (per-user delegation).
//! - The pure-serde BI wire DTOs (see [`bi`]) — dataset/chart/dashboard/SQL-Lab
//!   request & response shapes moved verbatim from `egret-bi::model`; `egret-bi`
//!   re-exports them, keeping its domain→wire conversions and `From<Row>` impls.
//!
//! No sqlx, no axum: this crate stays a light leaf. Mirrors `egret-query-api`
//! (E4d) / `egret-catalog-api` (E3b) / `egret-policy-api`.
//!
//! Data Apps P6 removed this crate's notebook-docs vocabulary (the
//! `NotebookSummary`/`EngineOption`/`NotebookRuntime`/`CellOutput` DTOs and the
//! `NotebookDocsPort` trait `egret-kernel` depended on) once `egret-kernel` and
//! `egret-notebook`, its only consumers, were deleted.

pub mod bi;
pub mod engine_token;

// The BI wire DTOs stay namespaced under `egret_analytics_api::bi::…` (29 types
// with generic names like `CreateChart`/`CardInput` — a root glob would pollute
// the crate root).
pub use engine_token::{BrokerError, EngineToken, EngineTokenMinter};
