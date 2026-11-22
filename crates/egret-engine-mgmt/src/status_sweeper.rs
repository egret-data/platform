//! Keeps the stored `engine_mgmt.engines.status` of a provisioned engine
//! content-derived: `provisioning` becomes `running` once the provisioner
//! reports every pod ready, whether or not anyone opens the engine's detail
//! page. The list and the governance browse read the stored column, so a
//! convergence that only happened on the detail read left them on a stale
//! `provisioning`.

use crate::{engine_status, EngineMgmtState as CpState};

/// How often [`spawn_engine_status_sweeper`] looks at the `provisioning`
/// engines. Provisioning is transient (minutes) and the pass is one cheap
/// provisioner status call per engine still provisioning, so a short period
/// keeps the label fresh without cost once every engine is `running`.
pub const ENGINE_STATUS_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a live provisioner `phase` for an engine stored as `stored_status`
/// is the first all-pods-ready observation, to be recorded as `running`.
/// Shared by the detail read and the sweep so the two never diverge. Only
/// `provisioning` converges: `deleting` and `failed` stay put.
pub fn records_running(stored_status: &str, live_phase: &str) -> bool {
    live_phase == engine_status::RUNNING && stored_status == engine_status::PROVISIONING
}

/// One pass: every provisioned engine stored as `provisioning` whose live
/// phase says running is advanced with the same compare-and-set the detail
/// read uses. Returns how many rows this pass advanced.
pub async fn converge_provisioning_engines(st: &CpState) -> usize {
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return 0;
    };
    let engines = match st.engine_mgmt.list_provisioning_engines().await {
        Ok(engines) => engines,
        Err(err) => {
            tracing::warn!(error = %err, "could not list the provisioning engines");
            return 0;
        }
    };
    let mut advanced = 0;
    for engine in engines {
        let live = match prov
            .status(&engine.r#type, &engine.namespace, &engine.name)
            .await
        {
            Ok(live) => live,
            Err(err) => {
                tracing::debug!(engine = %engine.name, error = %err,
                    "engine status not available; left as provisioning");
                continue;
            }
        };
        if !records_running(&engine.status, &live.phase) {
            continue;
        }
        match st
            .engine_mgmt
            .transition_engine_status(
                engine.id,
                engine_status::PROVISIONING,
                engine_status::RUNNING,
            )
            .await
        {
            Ok(true) => advanced += 1,
            Ok(false) => {}
            Err(err) => tracing::warn!(engine = %engine.name, error = %err,
                "could not record running status"),
        }
    }
    advanced
}

/// Run [`converge_provisioning_engines`] every `period` until `shutdown`
/// resolves. A pass longer than the period does not cause a burst.
pub fn spawn_engine_status_sweeper(
    st: CpState,
    period: std::time::Duration,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::pin!(shutdown);
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tick.tick() => {
                    converge_provisioning_engines(&st).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_provisioning_engine_with_a_running_phase_converges() {
        assert!(records_running("provisioning", "running"));
        assert!(!records_running("provisioning", "provisioning"));
        assert!(!records_running("running", "running"));
        assert!(!records_running("deleting", "running"));
        assert!(!records_running("failed", "running"));
    }
}
