//! AuditingPdp emits one Decision event per authorize (allow + deny).
use egret_audit::{AuditEvent, AuditSink};
use egret_auth::pdp::AuditingPdp;
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use std::sync::Arc;

struct FixedPdp(bool);
#[async_trait::async_trait]
impl Pdp for FixedPdp {
    async fn authorize(
        &self,
        _s: &Subject,
        _a: &str,
        _r: &Resource,
    ) -> egret_core::Result<Decision> {
        Ok(Decision {
            allow: self.0,
            reasons: vec!["p1".into()],
        })
    }
}

#[tokio::test]
async fn emits_a_decision_event() {
    let (sink, mut rx) = AuditSink::new(16);
    let pdp = AuditingPdp::new(Arc::new(FixedPdp(false)), sink);
    let subject = Subject {
        user_id: UserId::new(),
        group_ids: vec![],
    };
    let ws = WorkspaceId::new();
    let d = pdp
        .authorize(&subject, "dashboard:read", &Resource::Workspace(ws))
        .await
        .unwrap();
    assert!(!d.allow);
    let ev = rx.try_recv().expect("one event emitted");
    match ev {
        AuditEvent::Decision {
            action,
            allow,
            resource_type,
            ..
        } => {
            assert_eq!(action, "dashboard:read");
            assert!(!allow);
            assert_eq!(resource_type, "workspace");
        }
        _ => panic!("expected Decision"),
    }
}

#[tokio::test]
async fn a_probe_returns_the_decision_without_emitting_one() {
    let (sink, mut rx) = AuditSink::new(16);
    let pdp = AuditingPdp::new(Arc::new(FixedPdp(false)), sink);
    let subject = Subject {
        user_id: UserId::new(),
        group_ids: vec![],
    };
    let d = pdp
        .probe(
            &subject,
            "workspace:manage_members",
            &Resource::Workspace(WorkspaceId::new()),
        )
        .await
        .unwrap();
    assert!(!d.allow, "the probe answers as authorize does");
    assert!(rx.try_recv().is_err(), "a probe writes no audit decision");
}
