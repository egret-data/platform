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

/// `true` for the one `Err` shape 8-way contention on `insert_version` is
/// expected to produce: the `UNIQUE (flow_id, version)` violation from two
/// racers computing the same next number. `egret_core::Error::Internal` wraps
/// the original `sqlx::Error` through `anyhow::anyhow!("{e}")`
/// (`repo.rs`'s `db_err`), which loses the structured error and leaves only
/// its `Display` text -- so this matches on the constraint's own name rather
/// than downcasting.
fn is_version_conflict(e: &egret_core::Error) -> bool {
    e.to_string().contains("flow_versions_flow_id_version_key")
}

/// Fix round 1 rewrite. The original shape of this test (`if let Ok(Ok(v)) =
/// res { versions.push(v.version) }`) silently dropped every `Err` — so it
/// stayed green even when most racers failed on the `UNIQUE (flow_id,
/// version)` constraint and only a couple of calls ever actually succeeded.
///
/// That silent drop was hiding a real fact, established empirically as part
/// of this fix (both directions recorded in `task-5-report.md`, "Fix round
/// 1"): the `UNIQUE` constraint alone already guarantees no two COMMITTED
/// rows can ever share a version number, or leave a gap in front of one that
/// did commit -- regardless of whether the next number is computed inside the
/// `INSERT` statement or in a separate, prior `SELECT`. Both shapes are
/// correct in that sense; there is no version-race bug to catch by outcome
/// alone. What differs, sharply and reproducibly against this crate's 5
/// -connection pool, is HOW MANY of the 8 concurrent callers win: rewritten as
/// a genuine two-round-trip (`SELECT` on one pooled connection, released,
/// then a separate `INSERT ... VALUES` on whichever connection is next
/// available), most of the 8 read the same stale `MAX` before any of them
/// commit and lose to the constraint. Computing it inside one statement holds
/// a single connection for the whole read-then-write, which naturally
/// staggers the racers through the pool instead -- far fewer collide, though
/// not necessarily zero.
///
/// So this test asserts the property that is both TRUE and MEANINGFUL: every
/// one of the 8 calls is accounted for as either a success or exactly this
/// expected conflict (nothing else, and nothing silently dropped), the
/// successful versions are a gapless, duplicate-free run starting at 1, and
/// -- the one number that empirically separates the two implementations here
/// -- at least 3 of the 8 callers succeed. Measured directly as part of this
/// fix (`task-5-report.md`, "Fix round 1"), 5 and 8 runs respectively, both
/// against this crate's 5-connection test pool: the shipped, single-statement
/// `insert_version` lands 4 successes / 4 conflicts every run; a genuine
/// two-round-trip rewrite lands 2 successes / 6 conflicts every run. The `3`
/// threshold sits with margin above the latter and below the former, rather
/// than pinning the exact observed count, since the split is a real
/// contention outcome and not a guaranteed constant.
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
    let mut successes = Vec::new();
    let mut conflicts = 0usize;
    while let Some(res) = set.join_next().await {
        match res.expect("task panicked") {
            Ok(v) => successes.push(v.version),
            Err(e) if is_version_conflict(&e) => conflicts += 1,
            Err(e) => panic!("unexpected error, not the version-conflict this test expects: {e}"),
        }
    }
    assert_eq!(
        successes.len() + conflicts,
        8,
        "every one of the 8 concurrent calls must be accounted for as a success or an \
         expected version conflict, with none dropped: {} successes, {conflicts} conflicts",
        successes.len()
    );
    successes.sort_unstable();
    assert_eq!(
        successes,
        (1..=successes.len() as i32).collect::<Vec<_>>(),
        "successful versions must be a gapless, duplicate-free run starting at 1: {successes:?}"
    );
    assert!(
        successes.len() >= 3,
        "the single-statement allocation should let a healthy majority of 8 concurrent callers \
         through this pool succeed (measured: 4/8 for this shape, 2/8 for a two-round-trip \
         rewrite); only {} did ({conflicts} lost to the constraint) -- see task-5-report.md \
         \"Fix round 1\"",
        successes.len()
    );
}
