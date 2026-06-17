use crate::routes::RangerState;
use axum::Router;

/// CRUD routes (service-defs read, services, policies) — implemented in Task 7.
pub fn crud_routes() -> Router<RangerState> {
    Router::new()
}
