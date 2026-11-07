//! Governance SP2b Task 15: the engine reconfigurer a profile's mechanism
//! switch calls, over the engine seam.
//!
//! The policy admin state is built before the engine seam's state, so the
//! reconfigurer holds a cell `main` fills right after that state is built.

use std::sync::Arc;

/// Why an engine was not re-applied while the engine seam is not wired.
pub const NOT_WIRED: &str = "engine management is not wired yet";

/// [`egret_policy::ProfileEngineReconfigurer`] over
/// [`egret_engine_mgmt::reapply_bound_engines`].
pub struct ReconfigurerImpl {
    /// The engine seam's state; empty until `main` builds it.
    pub cp: Arc<tokio::sync::OnceCell<egret_engine_mgmt::EngineMgmtState>>,
    /// Names the engines an empty cell cannot reach, so each one is reported.
    pub store: egret_engine_mgmt::EngineMgmtStore,
}

#[async_trait::async_trait]
impl egret_policy::ProfileEngineReconfigurer for ReconfigurerImpl {
    async fn reconfigure_bound_engines(&self, service_id: i64) -> egret_policy::ReconfigureReport {
        if let Some(cp) = self.cp.get() {
            return egret_engine_mgmt::reapply_bound_engines(cp, service_id).await;
        }
        let mut report = egret_policy::ReconfigureReport::default();
        match self
            .store
            .list_engines_bound_to_authz_service(service_id)
            .await
        {
            Ok(engines) => report.failed.extend(
                engines
                    .into_iter()
                    .filter(|e| {
                        !egret_engine_mgmt::external::is_external(e)
                            && e.status != egret_engine_mgmt::engine_status::DELETING
                    })
                    .map(|e| (e.name, NOT_WIRED.to_string())),
            ),
            Err(e) => report.failed.push((
                format!("policy profile {service_id}"),
                format!("{NOT_WIRED}; its engines could not be listed: {e}"),
            )),
        }
        report
    }
}
