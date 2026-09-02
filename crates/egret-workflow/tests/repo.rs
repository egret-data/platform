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

    let versions = repo.list_versions(ws, created.id).await.expect("list");
    assert_eq!(versions[0].version, 2, "newest first");
}

/// Task 6 fix round 2, Finding 2: `list_versions`/`get_version` used to be
/// unscoped reads -- a foreign flow id handed back that flow's version
/// snapshots and notes with no error, the exact shape migration 0128 is this
/// programme's recorded case of. Both now take `ws` and join
/// `workflow.flows` to check it, so a foreign flow id and a never-existed one
/// must be indistinguishable: an empty `Vec` from `list_versions`, `NotFound`
/// from `get_version`.
#[tokio::test]
async fn list_versions_and_get_version_are_scoped_to_the_callers_workspace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let f = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "scoped".into(),
                source: "id: scoped\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create");
    repo.insert_version(f.id, user, &serde_json::json!({"v": 1}), "first")
        .await
        .expect("insert_version");

    // The owning workspace sees it, same as before.
    let owned = repo.list_versions(ws, f.id).await.expect("list_versions");
    assert_eq!(
        owned.len(),
        1,
        "the owning workspace must see its own version"
    );
    let owned_v1 = repo.get_version(ws, f.id, 1).await.expect("get_version");
    assert_eq!(owned_v1.version, 1);

    // A DIFFERENT, real workspace asking about the SAME flow id sees nothing --
    // not an error naming the flow, an empty answer, indistinguishable from a
    // flow id nobody has ever created.
    let foreign = repo
        .list_versions(other_ws, f.id)
        .await
        .expect("list_versions must not error, just answer empty");
    assert!(
        foreign.is_empty(),
        "a foreign workspace must not see another workspace's versions: {foreign:?}"
    );
    let foreign_err = repo
        .get_version(other_ws, f.id, 1)
        .await
        .expect_err("a foreign workspace must not be able to read a specific version either");
    assert!(matches!(foreign_err, egret_core::Error::NotFound(_)));

    // A never-existed flow id, from either workspace, answers exactly the same
    // way as the foreign one above -- the indistinguishability this fix exists
    // to guarantee.
    let nope = Uuid::new_v4();
    let never_existed = repo
        .list_versions(other_ws, nope)
        .await
        .expect("list_versions must not error on an unknown id either");
    assert!(never_existed.is_empty());
    let never_existed_err = repo
        .get_version(other_ws, nope, 1)
        .await
        .expect_err("must fail the same way as the foreign flow above");
    assert!(matches!(never_existed_err, egret_core::Error::NotFound(_)));

    let _ = other_user; // seeded only to get a distinct, real workspace id
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

/// `FlowRepo::list`'s filters, flagged by Task 5 as implemented-to-its-own-
/// design and never tested. Folder, namespace, and state each narrow the
/// list, ANDed with the mandatory `workspace_id` predicate -- and a filter
/// can never be the thing that reaches another workspace's flows, since
/// nothing here relaxes that predicate.
#[tokio::test]
async fn list_filters_narrow_and_cannot_cross_a_workspace_boundary() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let a = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "list.a".into(),
                flow_id: "one".into(),
                source: "id: one\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create a");
    let _b = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "list.b".into(),
                flow_id: "two".into(),
                source: "id: two\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create b (present only so the unfiltered list must see BOTH of ws's flows)");
    // A flow in a DIFFERENT workspace, using the SAME namespace as `a` -- so
    // a namespace filter that leaked across workspaces would return it too.
    repo.create(
        other_ws,
        other_user,
        &egret_workflow::CreateFlowInput {
            namespace: "list.a".into(),
            flow_id: "foreign".into(),
            source: "id: foreign\n".into(),
            plan: serde_json::json!({}),
            folder_id: None,
        },
    )
    .await
    .expect("create foreign");

    // No filter: both of `ws`'s own flows, never the foreign one.
    let all = repo
        .list(ws, &egret_workflow::ListFilter::default())
        .await
        .expect("list all");
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all.iter().all(|f| f.workspace_id == ws));

    // `namespace` narrows to exactly the match IN `ws`, never the foreign
    // workspace's flow of the identical namespace.
    let by_ns = repo
        .list(
            ws,
            &egret_workflow::ListFilter {
                namespace: Some("list.a".into()),
                ..Default::default()
            },
        )
        .await
        .expect("list by namespace");
    assert_eq!(by_ns.len(), 1, "{by_ns:?}");
    assert_eq!(by_ns[0].id, a.id);

    // `state`: both are still drafts.
    let by_state = repo
        .list(
            ws,
            &egret_workflow::ListFilter {
                state: Some("draft".into()),
                ..Default::default()
            },
        )
        .await
        .expect("list by state");
    assert_eq!(by_state.len(), 2);
    let by_wrong_state = repo
        .list(
            ws,
            &egret_workflow::ListFilter {
                state: Some("live".into()),
                ..Default::default()
            },
        )
        .await
        .expect("list by wrong state");
    assert!(by_wrong_state.is_empty());

    // `folder_id`: neither flow has one, so a real, unrelated folder id
    // narrows to nothing.
    let by_folder = repo
        .list(
            ws,
            &egret_workflow::ListFilter {
                folder_id: Some(Uuid::new_v4()),
                ..Default::default()
            },
        )
        .await
        .expect("list by folder");
    assert!(by_folder.is_empty());

    // The direct cross-workspace check, independent of any filter: asking
    // for `other_ws`'s list (even from that workspace's own filter set)
    // returns only its own single flow.
    let foreign_list = repo
        .list(other_ws, &egret_workflow::ListFilter::default())
        .await
        .expect("list other_ws");
    assert_eq!(foreign_list.len(), 1);
    assert_eq!(foreign_list[0].namespace, "list.a");
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

// ---------- Whole-branch review, IMPORTANT finding 3 ----------
//
// Before this fix round, EVERY `UNIQUE (workspace_id, namespace, flow_id)`
// violation on `create`/`update` mapped through the generic `db_err` to
// `Error::Internal` -- an opaque 500, even though `openapi.json` documents a
// 409 on both routes. Measured live by the reviewer against the pre-fix
// branch: a duplicate `create` was 500 `{"error":"internal error"}`, and
// recreating a flow after soft-deleting the original (which still holds the
// name) was ALSO 500, with no way for an author to see why. Both are 409s
// below, and the trashed case names the cause.

#[tokio::test]
async fn a_duplicate_create_is_a_conflict_not_an_internal_error() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let input = egret_workflow::CreateFlowInput {
        namespace: "conflict.a".into(),
        flow_id: "dup".into(),
        source: "id: dup\n".into(),
        plan: serde_json::json!({}),
        folder_id: None,
    };
    repo.create(ws, user, &input).await.expect("first create");

    let err = repo
        .create(ws, user, &input)
        .await
        .expect_err("a second create with the same identity must be refused");
    assert!(
        matches!(err, egret_core::Error::Conflict(_)),
        "expected Conflict (409), got {err:?} -- a bare 500 is exactly the defect this test \
         exists to catch"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("conflict.a") && msg.contains("dup"),
        "the message must name the colliding identity, not just say \"conflict\": {msg}"
    );
}

#[tokio::test]
async fn recreating_a_soft_deleted_flow_is_a_conflict_that_says_so() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let input = egret_workflow::CreateFlowInput {
        namespace: "conflict.b".into(),
        flow_id: "trashed".into(),
        source: "id: trashed\n".into(),
        plan: serde_json::json!({}),
        folder_id: None,
    };
    let created = repo.create(ws, user, &input).await.expect("create");
    repo.soft_delete(ws, created.id).await.expect("soft delete");

    // The name is still burned -- `UNIQUE` has no `WHERE deleted_at IS
    // NULL`, matching `dataapi.endpoints`' identical constraint.
    let err = repo
        .create(ws, user, &input)
        .await
        .expect_err("the trashed row still holds this identity");
    assert!(
        matches!(err, egret_core::Error::Conflict(_)),
        "expected Conflict (409), got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("trash") || msg.to_lowercase().contains("deleted"),
        "the message must say the row is TRASHED, not just that a conflict happened \
         (an author cannot see why otherwise): {msg}"
    );
}

/// A visible (never-trashed) collision must NOT claim to be about the trash
/// -- the two messages are meaningfully different and a caller reading one
/// must be able to trust it.
#[tokio::test]
async fn a_visible_collision_does_not_mention_the_trash() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let input = egret_workflow::CreateFlowInput {
        namespace: "conflict.c".into(),
        flow_id: "visible".into(),
        source: "id: visible\n".into(),
        plan: serde_json::json!({}),
        folder_id: None,
    };
    repo.create(ws, user, &input).await.expect("first create");

    let err = repo.create(ws, user, &input).await.expect_err("collision");
    let msg = err.to_string().to_lowercase();
    assert!(
        !msg.contains("trash") && !msg.contains("deleted"),
        "a LIVE collision must not read as a trash message: {msg}"
    );
}

/// `update` can hit the same constraint by repointing a flow onto a name
/// another (visible) row already holds -- the identical 500-vs-409 defect,
/// one statement over.
#[tokio::test]
async fn update_onto_an_existing_name_is_a_conflict_not_an_internal_error() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    repo.create(
        ws,
        user,
        &egret_workflow::CreateFlowInput {
            namespace: "conflict.d".into(),
            flow_id: "taken".into(),
            source: "id: taken\n".into(),
            plan: serde_json::json!({}),
            folder_id: None,
        },
    )
    .await
    .expect("create taken");
    let mover = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "conflict.d".into(),
                flow_id: "mover".into(),
                source: "id: mover\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create mover");

    let err = repo
        .update(
            ws,
            mover.id,
            user,
            &egret_workflow::UpdateFlowInput {
                flow_id: Some("taken".into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("renaming onto an existing identity must be refused");
    assert!(
        matches!(err, egret_core::Error::Conflict(_)),
        "expected Conflict (409), got {err:?}"
    );
}
