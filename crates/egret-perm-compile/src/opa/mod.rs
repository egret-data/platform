//! Governance SP2b: egret answers Trino's OPA plugin from the lowered
//! permission IR. `build` assembles one profile's evaluator snapshot from the
//! same lowering `crate::trigger::compile_desired` renders for Ranger.

pub mod build;
pub mod cache;
pub mod surface;
pub mod wire;

use std::sync::Arc;

use egret_engine_mgmt_api::credential::EngineCredentialVerifier;
use sqlx::PgPool;

use cache::{spawn_snapshot_refresher, SnapshotCache, REFRESH_PERIOD};
use surface::OpaState;

/// The OPA surface's state, with its snapshot refresher running, when
/// `enabled` (`[governance.opa] enabled`) and the Ranger surface's engine
/// credential `verifier` exists; otherwise `None`, and nothing is spawned and
/// no connection is opened (the gate off means the routes are not mounted and
/// no refresher runs). `batch_body_limit` is `[governance.opa]
/// batch_body_limit_bytes`, the `/batch` route's body limit;
/// `body_read_timeout` is `[governance.opa] body_read_timeout_ms`.
///
/// The verifier is the one the Ranger surface checks engine credentials with:
/// an engine authenticates to both surfaces with the same secret. The
/// refresher runs every [`REFRESH_PERIOD`] until `shutdown` resolves; a
/// refresh error is logged and retried next period, never fatal.
pub fn start_opa(
    enabled: bool,
    batch_body_limit: usize,
    body_read_timeout: std::time::Duration,
    verifier: Option<Arc<dyn EngineCredentialVerifier>>,
    pool: PgPool,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Option<(OpaState, tokio::task::JoinHandle<()>)> {
    if !enabled {
        return None;
    }
    let Some(verifier) = verifier else {
        // Not an error: with `governance.opa.enabled` on by default this is the
        // state of every install that has not turned the Ranger surface on.
        tracing::info!(
            "OPA surface idle: ranger.enabled is off, so no OPA listener runs (the surface \
             checks engines with the Ranger surface's credentials); a switch to OPA is \
             refused until ranger.enabled is on"
        );
        return None;
    };
    let cache = Arc::new(SnapshotCache::default());
    let refresher = spawn_snapshot_refresher(Arc::clone(&cache), pool, REFRESH_PERIOD, shutdown);
    Some((
        OpaState::new(cache, verifier)
            .with_batch_body_limit(batch_body_limit)
            .with_body_read_timeout(body_read_timeout),
        refresher,
    ))
}
