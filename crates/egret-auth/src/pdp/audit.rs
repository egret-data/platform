//! A `Pdp` decorator that emits an audit Decision event per authorize, then
//! returns the inner decision verbatim (observation only — parity unchanged).
use std::sync::Arc;

use egret_audit::{AuditEvent, AuditSink};
use egret_core::authz::{Decision, Pdp, Resource, Subject};

pub struct AuditingPdp {
    inner: Arc<dyn Pdp>,
    sink: AuditSink,
}

impl AuditingPdp {
    pub fn new(inner: Arc<dyn Pdp>, sink: AuditSink) -> Self {
        Self { inner, sink }
    }
}

#[async_trait::async_trait]
impl Pdp for AuditingPdp {
    async fn authorize(
        &self,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> egret_core::Result<Decision> {
        let decision = self.inner.authorize(subject, action, resource).await?;
        self.sink.emit(AuditEvent::Decision {
            actor: Some(subject.user_id.0),
            workspace_id: resource.workspace_id().map(|w| w.0),
            action: action.to_string(),
            resource_type: resource.type_str().to_string(),
            resource_id: resource.id_str(),
            allow: decision.allow,
            reasons: decision.reasons.clone(),
        });
        Ok(decision)
    }
}
