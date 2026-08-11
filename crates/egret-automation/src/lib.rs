//! Alerts & Reports: scheduled SQL alerts and recurring report deliveries.
//!
//! P1 is the model and its CRUD — nothing in this crate runs a schedule yet.
//! The scheduler, the evaluator and the delivery transports arrive in P2/P3 and
//! live here too, beside the repo they claim rows from.

pub mod cron;
mod error;
pub mod model;
mod repo;
mod routes;

pub use error::{AutomationApiError, AutomationError};
pub use repo::{NewSchedule, Owner, Recipient, Schedule, ScheduleRepo, ScheduleRun};
pub use routes::{automation_router, AutomationState};

/// This seam's OpenAPI document, merged into the top-level doc by
/// `egret-server` — the per-crate `openapi()` convention every annotated seam
/// exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    routes::AutomationApi::openapi()
}
