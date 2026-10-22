//! Alerts & Reports: scheduled SQL alerts and recurring report deliveries.
//!
//! P1 is the model and its CRUD — nothing in this crate runs a schedule yet.
//! The scheduler, the evaluator and the delivery transports arrive in P2/P3 and
//! live here too, beside the repo they claim rows from.

pub mod cron;
pub mod dataapi_metric;
pub mod deliver;
mod error;
pub mod evaluator;
mod format;
pub mod model;
pub mod render;
mod repo;
mod routes;
pub mod runner;
pub mod scheduler;
pub mod subject;
pub mod validator;

pub use error::{AutomationApiError, AutomationError};
pub use repo::{
    NewSchedule, Owner, Recipient, Schedule, ScheduleRepo, ScheduleRun, WEBHOOK_SECRET_KIND,
};
pub use routes::{automation_router, render_router, AutomationState};

/// This seam's OpenAPI document, merged into the top-level doc by
/// `egret-server` — the per-crate `openapi()` convention every annotated seam
/// exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    routes::AutomationApi::openapi()
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
