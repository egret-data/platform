//! The management router, driven over real HTTP against a real `CedarPdp` --
//! following `crates/egret-dataapi/tests/endpoints_routes.rs`'s convention of
//! never mocking the decision point for an authorization-shaped test.
//! DB-gated; self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-workflow --test routes

mod common;

use axum::http::StatusCode;
use uuid::Uuid;

/// A flow with no external references at all -- `core.log` (`registry.rs`
/// lists it with `references: &[]`) -- so a publish test never has to stand
/// up a real `WorkflowRefPort` answer. `ns` is a parameter so several tests
/// in this file can each mint a flow with a fresh, non-colliding namespace
/// (`UNIQUE (workspace_id, namespace, flow_id)`).
fn minimal_flow(ns: &str) -> String {
    format!("id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n")
}

/// The gate W0 registered must actually bite now that routes exist. Before this
/// task the rule was inert because nothing matched it; a route landing before its
/// rule is what fails OPEN.
#[tokio::test]
async fn the_workflows_entitlement_gates_these_routes() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::disable_feature(&db, ws, "workflows").await;

    let app = common::app(&db).await;
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await;
    assert_eq!(
        res.status(),
        403,
        "a disabled module must not serve its routes"
    );
}

/// Control for the test above: with the module ON and the caller granted
/// `workflow:read`, the identical request succeeds. Without this, the gate
/// test alone could not tell "the gate correctly refused" from "this route
/// 403s for some unrelated reason" (e.g. a routing typo).
#[tokio::test]
async fn a_workflow_reader_reaches_the_list_route_while_the_module_is_on() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let app = common::app(&db).await;
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await;
    assert_eq!(res.status(), 200, "workflows is on and the caller can read");
}

/// `workflow:read` cannot author -- egret's permissions are flat, so `:read`
/// implies none of `:manage`.
#[tokio::test]
async fn read_permission_cannot_create_a_flow() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("a.reader") }),
    )
    .await;
    assert_eq!(res.status(), 403);
}

/// Create returns 201 with the validation report inline -- Task 7 brief,
/// Step 1's first requirement.
#[tokio::test]
async fn create_returns_201_with_the_validation_report() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("a.create") }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = common::json(res).await;
    assert_eq!(body["namespace"], "a.create");
    assert_eq!(body["flow_id"], "f");
    assert_eq!(body["state"], "draft");
    assert!(body["live_version_id"].is_null());
    // The validation report is PRESENT and empty -- `core.log` with no
    // references and a well-formed namespace has nothing to say.
    assert_eq!(
        body["validation"]["errors"],
        serde_json::json!([]),
        "{body}"
    );
    assert_eq!(
        body["validation"]["warnings"],
        serde_json::json!([]),
        "{body}"
    );
}

/// A syntactically broken YAML cannot even be stored (namespace/flow_id are
/// derived from a successful parse), so create refuses it -- 400, not a
/// silently-created row with garbage identity columns.
#[tokio::test]
async fn create_refuses_yaml_that_does_not_parse() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": "id: [this is not valid yaml" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// `POST .../validate` checks the REQUEST BODY, not the stored draft, and
/// never persists -- spec §10's whole reason for the route: the editor calls
/// it on every keystroke-pause and must not require a saved draft. This test
/// sends a source that DIFFERS from the stored one and confirms (a) the
/// report reflects the body, and (b) the stored draft is untouched.
#[tokio::test]
async fn validate_checks_the_unsaved_body_and_never_persists() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.validate") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    // An UNSAVED edit that introduces a semantic error: `namespace` no
    // longer matches `is_valid_namespace`'s pattern.
    let broken_unsaved = "id: f\nnamespace: BAD NAMESPACE\ntasks:\n  - id: t\n    type: core.log\n";
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/validate"),
        user,
        serde_json::json!({ "source": broken_unsaved }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a validation finding is content, not a 4xx"
    );
    let report = common::json(res).await;
    assert!(
        !report["errors"].as_array().unwrap().is_empty(),
        "the bad namespace in the BODY must be reported: {report}"
    );

    // The STORED draft must be exactly what create wrote -- validate must
    // not have saved the broken source over it.
    let still =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await)
            .await;
    assert_eq!(
        still["namespace"], "a.validate",
        "validate must not persist: {still}"
    );
    assert_eq!(
        still["validation"]["errors"],
        serde_json::json!([]),
        "the STORED draft is still clean: {still}"
    );
}

/// Publish resolves references, freezes a version, and returns it.
#[tokio::test]
async fn publish_returns_the_version() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.publish") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/publish"),
        user,
        serde_json::json!({ "note": "first" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let version = common::json(res).await;
    assert_eq!(version["version"], 1);
    assert_eq!(version["note"], "first");
    assert_eq!(version["flow_id"], id);

    // The draft itself is now live and points at this version.
    let after =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await)
            .await;
    assert_eq!(after["state"], "live");
    assert_eq!(after["live_version_id"], version["id"]);

    // And it shows up in the version list/get routes.
    let versions = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{id}/versions"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(versions.as_array().unwrap().len(), 1);
    let one = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{id}/versions/1"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(one["id"], version["id"]);
}

/// A flow in another workspace returns 404, never 403 or a foreign body --
/// absence and denial must be indistinguishable from outside. Covers
/// `GET`, `PUT`, `DELETE`, `/validate`, `/publish`, `/versions`, and
/// `/versions/{v}` -- one flow, one foreign caller, every route that takes
/// `{id}`.
#[tokio::test]
async fn a_flow_in_another_workspace_is_404_from_every_id_scoped_route() {
    let Some(db) = common::db().await else { return };
    let (owner_ws, owner) = common::seed_workspace(&db).await;
    common::grant(&db, owner_ws, owner, &["workflow:read", "workflow:manage"]).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    common::grant(
        &db,
        other_ws,
        other_user,
        &["workflow:read", "workflow:manage"],
    )
    .await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{owner_ws}/workflows/flows"),
            owner,
            serde_json::json!({ "source": minimal_flow("a.foreign") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let cases: Vec<(&str, &str, Option<serde_json::Value>)> = vec![
        ("GET", "", None),
        (
            "PUT",
            "",
            Some(serde_json::json!({ "source": minimal_flow("a.foreign") })),
        ),
        ("DELETE", "", None),
        (
            "POST",
            "/validate",
            Some(serde_json::json!({ "source": minimal_flow("a.foreign") })),
        ),
        ("POST", "/publish", Some(serde_json::json!({}))),
        ("GET", "/versions", None),
        ("GET", "/versions/1", None),
    ];
    for (method, suffix, body) in cases {
        let path = format!("/api/ws/{other_ws}/workflows/flows/{id}{suffix}");
        let res = match (method, body) {
            ("GET", _) => common::get(&app, &path, other_user).await,
            ("DELETE", _) => common::delete(&app, &path, other_user).await,
            ("PUT", Some(b)) => common::put(&app, &path, other_user, b).await,
            ("POST", Some(b)) => common::post(&app, &path, other_user, b).await,
            _ => unreachable!(),
        };
        assert_eq!(
            res.status(),
            StatusCode::NOT_FOUND,
            "{method} {path} must be 404, got {}",
            res.status()
        );
    }
}

/// `PUT` replaces the draft (source, and its derived namespace/flow_id/plan),
/// and `folder_id` is set to exactly what is sent.
#[tokio::test]
async fn update_replaces_the_draft() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.before") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": minimal_flow("a.after"), "folder_id": null }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let updated = common::json(res).await;
    assert_eq!(updated["namespace"], "a.after");
}

/// Delete soft-deletes: `FlowRepo::get` (and therefore `GET /flows/{id}`)
/// does not filter on `deleted_at`, matching `EndpointStore::get`'s
/// identical choice in `egret-dataapi` -- a trashed row is still viewable,
/// only `GET /flows` (which DOES filter, via `ListFilter::trashed`
/// defaulting false) drops it. What DOES change is idempotency: `soft_delete`
/// only updates a row where `deleted_at IS NULL`, so deleting an
/// already-trashed flow is refused the same way deleting one that never
/// existed is -- `repo.rs`'s own doc comment on `soft_delete` states this.
#[tokio::test]
async fn delete_soft_deletes_drops_it_from_the_list_but_not_from_a_direct_get() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.delete") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let res = common::delete(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Still directly gettable.
    let after = common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
    assert_eq!(after.status(), StatusCode::OK);

    // But gone from the (untrashed) list.
    let list =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await).await;
    assert!(
        list.as_array().unwrap().is_empty(),
        "a trashed flow must not appear in the default list: {list}"
    );

    // A second delete is refused, the same way deleting one that never
    // existed is -- idempotent-in-effect, never silently a no-op.
    let again = common::delete(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

// ---------- Whole-branch review, CRITICAL finding 1: `folder_id` scoping ----------
//
// Measured live by the reviewer against the pre-fix branch:
//   create with a FOREIGN folder_id      -> 201 Created, folder_id stored
//   create with a NONEXISTENT folder_id  -> 500
// So a `workflow:manage` holder in workspace A could file a flow into
// workspace B's folder, and 201-vs-500 was a free existence oracle for
// folder ids in every other workspace. The folder used was
// `entity_type = 'chart'` -- the polymorphic-type half was unchecked too.
// All four cases below must now be a single, indistinguishable 404.

#[tokio::test]
async fn create_refuses_a_foreign_folder_id() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let foreign_folder = common::create_folder(&db, other_ws, other_user, "flow").await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.foreign"), "folder_id": foreign_folder }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a folder in ANOTHER workspace must be refused, not silently accepted"
    );
}

#[tokio::test]
async fn create_refuses_a_nonexistent_folder_id() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.nowhere"), "folder_id": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a folder id that exists nowhere must be a 404, not a 500 quoting a constraint name"
    );
}

/// A foreign folder and a nonexistent one must be INDISTINGUISHABLE, or the
/// distinction itself becomes the oracle -- both bodies, not just both
/// statuses.
#[tokio::test]
async fn a_foreign_folder_and_a_nonexistent_one_answer_identically() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let foreign_folder = common::create_folder(&db, other_ws, other_user, "flow").await;

    let app = common::app(&db).await;
    let foreign = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.a"), "folder_id": foreign_folder }),
    )
    .await;
    let nonexistent = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.b"), "folder_id": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(foreign.status(), nonexistent.status());
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let (foreign_body, nonexistent_body) =
        (common::json(foreign).await, common::json(nonexistent).await);
    assert_eq!(
        foreign_body, nonexistent_body,
        "a distinguishable answer would let a caller probe which folder ids exist elsewhere"
    );
}

/// The polymorphic-type half: a folder in the CALLER's own workspace, but of
/// the wrong `entity_type` (a chart folder, not a flow one), must refuse the
/// same way -- to a flow author, a folder that cannot hold flows is a folder
/// that does not exist.
#[tokio::test]
async fn create_refuses_a_folder_of_the_wrong_entity_type() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let chart_folder = common::create_folder(&db, ws, user, "chart").await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.wrongtype"), "folder_id": chart_folder }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "a same-workspace CHART folder must not silently hold a flow"
    );
}

/// The positive control: a real, same-workspace, `flow`-typed folder is
/// accepted and stored -- without this, the three refusals above could not
/// distinguish "the guard works" from "the guard rejects everything".
#[tokio::test]
async fn create_accepts_a_legitimate_folder() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let folder = common::create_folder(&db, ws, user, "flow").await;

    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.ok"), "folder_id": folder }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::CREATED,
        "{}",
        common::json(res).await
    );
}

/// `update` repoints a flow's folder just as `create` sets one -- the
/// identical hazard, one route over.
#[tokio::test]
async fn update_refuses_a_foreign_folder_id() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let foreign_folder = common::create_folder(&db, other_ws, other_user, "flow").await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("folder.update") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": minimal_flow("folder.update"), "folder_id": foreign_folder }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------- `list()`'s filters (Task 5 flagged these as untested) ----------

/// Folder, namespace, and state filters each narrow the list, and a filter
/// can never be used to reach another workspace's flows -- the cross-
/// workspace half of this test is the one Task 7's brief calls out
/// specifically ("confirm a filter cannot be used to reach another
/// workspace's flows").
#[tokio::test]
async fn list_filters_narrow_within_the_workspace_and_never_leak_across_it() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    common::grant(
        &db,
        other_ws,
        other_user,
        &["workflow:read", "workflow:manage"],
    )
    .await;

    let app = common::app(&db).await;

    // Two flows in `ws`, two different namespaces.
    let a = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("filters.a") }),
        )
        .await,
    )
    .await;
    let _b = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("filters.b") }),
        )
        .await,
    )
    .await;
    // One flow in `other_ws`, SAME namespace as `a` -- so a namespace filter
    // that leaked across workspaces would return it too.
    let _foreign = common::json(
        common::post(
            &app,
            &format!("/api/ws/{other_ws}/workflows/flows"),
            other_user,
            serde_json::json!({ "source": minimal_flow("filters.a") }),
        )
        .await,
    )
    .await;

    // Unfiltered: both of `ws`'s own flows, never the foreign one.
    let all =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await).await;
    assert_eq!(all.as_array().unwrap().len(), 2, "{all}");

    // `namespace` filter narrows to exactly the matching one, and does NOT
    // pick up the foreign workspace's flow of the identical namespace.
    let by_ns = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows?namespace=filters.a"),
            user,
        )
        .await,
    )
    .await;
    let by_ns = by_ns.as_array().unwrap();
    assert_eq!(by_ns.len(), 1, "{by_ns:?}");
    assert_eq!(by_ns[0]["id"], a["id"]);

    // `state` filter: both flows are still drafts.
    let by_state = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows?state=draft"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(by_state.as_array().unwrap().len(), 2);
    let by_wrong_state = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows?state=live"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(by_wrong_state.as_array().unwrap().len(), 0);

    // `folder_id` filter: neither flow has one, so a random folder id
    // narrows to nothing, and an absent filter (already covered by `all`
    // above) does not exclude either.
    let by_folder = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/flows?folder_id={}", Uuid::new_v4()),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(by_folder.as_array().unwrap().len(), 0);

    // The workspace's own list, from the OTHER workspace's caller, sees only
    // its own single flow -- the direct cross-workspace check, independent
    // of any filter.
    let foreign_list = common::json(
        common::get(
            &app,
            &format!("/api/ws/{other_ws}/workflows/flows"),
            other_user,
        )
        .await,
    )
    .await;
    assert_eq!(foreign_list.as_array().unwrap().len(), 1);
}
