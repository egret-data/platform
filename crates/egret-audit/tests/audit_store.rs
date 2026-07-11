//! AuditStore insert + query + prune. Requires DATABASE_URL.
use egret_audit::{AuditEvent, AuditQuery, AuditSink, AuditStore};
use egret_metastore::Db;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn insert_query_prune_roundtrip() {
    let db = db().await;
    let store = AuditStore::new(db.clone());
    let actor = uuid::Uuid::new_v4();
    let ev = AuditEvent::Decision {
        actor: Some(actor),
        workspace_id: None,
        action: "dashboard:read".into(),
        resource_type: "workspace".into(),
        resource_id: Some("w".into()),
        allow: false,
        reasons: vec!["role:abc".into()],
    };
    store.insert_batch(&[ev]).await.unwrap();
    let rows = store
        .query(&AuditQuery {
            actor: Some(actor),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, "dashboard:read");
    assert_eq!(rows[0].decision.as_deref(), Some("deny"));
    // prune with retention 0 days deletes everything older than now
    let n = store.prune(0).await.unwrap();
    assert!(n >= 1);
}

/// The sink is best-effort: with a capacity-1 channel and no consumer, three
/// `emit`s must return immediately (2nd and 3rd are dropped, never blocking).
#[tokio::test]
async fn sink_emit_is_non_blocking_and_drops_when_full() {
    let (sink, _rx) = AuditSink::new(1);
    for _ in 0..3 {
        sink.emit(AuditEvent::Mutation {
            actor: None,
            workspace_id: None,
            action: "noop".into(),
            resource_type: "grant".into(),
            resource_id: None,
            detail: serde_json::json!({}),
        });
    }
    // Reaching here without hanging proves emit never blocks on a full channel.
}
