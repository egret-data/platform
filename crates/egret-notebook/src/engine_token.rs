//! The engine-token seam. The types moved verbatim to the leaf contract crate
//! `egret-analytics-api` (E5c-1) so `egret-kernel` can consume them without
//! depending on this crate; re-exported here so existing
//! `egret_notebook::engine_token::{EngineToken, EngineTokenMinter, BrokerError}`
//! paths (this crate's routes, egret-bin) keep working unchanged.
pub use egret_analytics_api::engine_token::{BrokerError, EngineToken, EngineTokenMinter};
