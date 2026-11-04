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
use surface::{OpaState, VerifiedCache, VERIFIED_TTL};

/// The OPA surface's state, with its snapshot refresher running, when
/// `enabled` (`[governance.opa] enabled`) and the Ranger surface's engine
/// credential `verifier` exists; otherwise `None`, and nothing is spawned and
/// no connection is opened (the gate off means the routes are not mounted and
/// no refresher runs).
///
/// The verifier is the one the Ranger surface checks engine credentials with:
/// an engine authenticates to both surfaces with the same secret. The
/// refresher runs every [`REFRESH_PERIOD`] until `shutdown` resolves; a
/// refresh error is logged and retried next period, never fatal.
pub fn start_opa(
    enabled: bool,
    verifier: Option<Arc<dyn EngineCredentialVerifier>>,
    pool: PgPool,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Option<(OpaState, tokio::task::JoinHandle<()>)> {
    if !enabled {
        return None;
    }
    let Some(verifier) = verifier else {
        tracing::warn!(
            "governance.opa.enabled is set but the Ranger surface (ranger.enabled) is off: \
             the OPA surface shares its engine credentials, so it is not mounted"
        );
        return None;
    };
    let cache = Arc::new(SnapshotCache::default());
    let refresher = spawn_snapshot_refresher(Arc::clone(&cache), pool, REFRESH_PERIOD, shutdown);
    let state = OpaState {
        cache,
        verifier,
        credentials: Arc::new(VerifiedCache::new(VERIFIED_TTL)),
    };
    Some((state, refresher))
}
