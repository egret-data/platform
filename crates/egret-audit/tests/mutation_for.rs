//! `mutation_for` — builds a `Mutation` event from a request principal,
//! stamping the impersonator automatically when the request is an
//! impersonation. Pure constructor test; no DB needed.

#[test]
fn mutation_from_impersonated_principal_records_both_parties() {
    let admin = uuid::Uuid::new_v4();
    let target = uuid::Uuid::new_v4();
    let principal = egret_core::Principal {
        user_id: egret_core::UserId(target),
        email: "target@x".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: Some(egret_core::context::Impersonator {
            user_id: egret_core::UserId(admin),
            email: "admin@x".into(),
            workspace_scope: None,
            expires_at: chrono::Utc::now(),
        }),
    };
    let ev = egret_audit::mutation_for(
        &principal,
        None,
        "notebook.update",
        "notebook",
        Some("n1".into()),
        serde_json::json!({}),
    );
    match ev {
        egret_audit::AuditEvent::Mutation {
            actor,
            impersonator,
            action,
            resource_type,
            resource_id,
            ..
        } => {
            assert_eq!(actor, Some(target));
            assert_eq!(impersonator, Some(admin));
            assert_eq!(action, "notebook.update");
            assert_eq!(resource_type, "notebook");
            assert_eq!(resource_id, Some("n1".into()));
        }
        _ => panic!("expected Mutation"),
    }
}

#[test]
fn mutation_from_ordinary_principal_has_no_impersonator() {
    let uid = uuid::Uuid::new_v4();
    let principal = egret_core::Principal {
        user_id: egret_core::UserId(uid),
        email: "u@x".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    };
    let ev = egret_audit::mutation_for(&principal, None, "a", "t", None, serde_json::json!({}));
    match ev {
        egret_audit::AuditEvent::Mutation {
            actor,
            impersonator,
            ..
        } => {
            assert_eq!(actor, Some(uid));
            assert_eq!(impersonator, None);
        }
        _ => panic!(),
    }
}

#[test]
fn mutation_for_carries_the_workspace_id() {
    let uid = uuid::Uuid::new_v4();
    let ws = uuid::Uuid::new_v4();
    let principal = egret_core::Principal {
        user_id: egret_core::UserId(uid),
        email: "u@x".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    };
    let ev = egret_audit::mutation_for(&principal, Some(ws), "a", "t", None, serde_json::json!({}));
    match ev {
        egret_audit::AuditEvent::Mutation { workspace_id, .. } => {
            assert_eq!(workspace_id, Some(ws));
        }
        _ => panic!(),
    }
}
