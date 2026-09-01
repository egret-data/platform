//! FlowRepo insert + read + version. Requires DATABASE_URL.

use uuid::Uuid;

mod common;

#[tokio::test]
async fn a_draft_round_trips_and_versions_are_ordered() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let created = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "f".into(),
                source: "id: f\nnamespace: a.b\ntasks: []\n".into(),
                plan: serde_json::json!({"id": "f"}),
                folder_id: None,
            },
        )
        .await
        .expect("create");

    assert_eq!(created.namespace, "a.b");
    assert_eq!(created.state, "draft");
    assert!(created.live_version_id.is_none());

    let v1 = repo
        .insert_version(created.id, user, &serde_json::json!({"v": 1}), "first")
        .await
        .expect("v1");
    let v2 = repo
        .insert_version(created.id, user, &serde_json::json!({"v": 2}), "second")
        .await
        .expect("v2");
    assert_eq!(v1.version, 1);
    assert_eq!(
        v2.version, 2,
        "versions must increment per flow, not globally"
    );

    let versions = repo.list_versions(created.id).await.expect("list");
    assert_eq!(versions[0].version, 2, "newest first");
}

/// The scoping obligation every port in this repo carries: answer with the
/// object's OWN workspace so the caller can compare, and let the caller refuse a
/// mismatch as NotFound. A never-existed id and a foreign id must be
/// indistinguishable to the caller.
#[tokio::test]
async fn workspace_of_answers_with_the_flows_own_workspace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "g".into(),
                source: "id: g\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create");

    assert_eq!(repo.workspace_of(f.id).await.unwrap(), Some(ws));
    assert_eq!(repo.workspace_of(Uuid::new_v4()).await.unwrap(), None);
}

/// (workspace_id, namespace, flow_id) is the Kestra identity. Two flows with the
/// same id in different namespaces must both exist.
#[tokio::test]
async fn the_same_flow_id_is_allowed_in_two_namespaces() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for ns in ["a.b", "a.c"] {
        repo.create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: ns.into(),
                flow_id: "same".into(),
                source: "id: same\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{ns}: {e}"));
    }
}

#[tokio::test]
async fn concurrent_publishes_do_not_collide_on_a_version_number() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "race".into(),
                source: "id: race\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create");

    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let r = repo.clone();
        set.spawn(async move {
            r.insert_version(f.id, user, &serde_json::json!({ "i": i }), "")
                .await
        });
    }
    let mut versions = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Ok(v)) = res {
            versions.push(v.version);
        }
    }
    versions.sort_unstable();
    let before = versions.len();
    versions.dedup();
    assert_eq!(
        versions.len(),
        before,
        "two versions got the same number: {versions:?}"
    );
}
