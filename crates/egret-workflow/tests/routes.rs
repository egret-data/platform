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

/// **Drafts still save** (no-silent-stall §3): a `PUT` of a flow holding an
/// `AllowFailure` with a `retry:` on the block -- which egret refuses (spec §2,
/// a non-goal), so publish and every start path refuse it -- answers 200 and
/// stores the draft, and the report it carries holds the ERROR
/// `unsupported_allow_failure_retry` at the block's `retry` path.
#[tokio::test]
async fn a_draft_with_an_allow_failure_saves_and_its_report_carries_the_error() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.allowfailure") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let source = "id: f\nnamespace: a.allowfailure\ntasks:\n  - id: risky\n    \
                  type: io.kestra.plugin.core.flow.AllowFailure\n    retry:\n      \
                  type: constant\n      interval: PT1S\n      maxAttempts: 3\n    tasks:\n      \
                  - id: t\n        type: core.log\n        message: hi\n";
    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": source, "folder_id": null }),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a draft saves whatever its report says"
    );
    let updated = common::json(res).await;
    let errors = updated["validation"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "unsupported_allow_failure_retry"
                && e["path"] == "tasks.risky.retry"),
        "{errors:?}"
    );
    assert_eq!(updated["source"], source, "and the draft is the one sent");
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

// ---------- W13b Task 2: a flow trigger's `window`, refused at save ----------

/// A flow listening on two flows of its own namespace, with `window_yaml`
/// (already indented for the trigger's mapping) appended to its trigger.
fn flow_with_window(ns: &str, window_yaml: &str) -> String {
    format!(
        "id: f\nnamespace: {ns}\ntasks:\n  - id: t\n    type: core.log\n\
         triggers:\n  - id: listen\n    type: io.kestra.plugin.core.trigger.Flow\n\
         \x20   dependsOn:\n\
         \x20     - namespace: {ns}\n        flowId: p1\n\
         \x20     - namespace: {ns}\n        flowId: p2\n\
         {window_yaml}"
    )
}

/// `fireOnce`, which the artifact refuses with 422 `Unrecognized field
/// "fireOnce" (class io.kestra.core.models.triggers.Window)`, cannot be stored
/// at all -- not by create and not by update -- because `models::Window`
/// denies unknown fields. A draft that is never stored never reaches
/// `scheduler::sync`, so no listener row can carry it either.
#[tokio::test]
async fn a_window_with_fire_once_cannot_be_saved() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    let fire_once = flow_with_window(
        "w13b.fireonce",
        "    window:\n      every: PT1H\n      fireOnce: true\n",
    );
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": fire_once }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains("fireOnce"),
        "the refusal names the field: {body}"
    );
    let listed =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await).await;
    assert!(
        !listed.to_string().contains("w13b.fireonce"),
        "a refused create stores nothing: {listed}"
    );

    // Control, and the update arm: the same flow without `fireOnce` is
    // created; updating it to carry `fireOnce` is refused and leaves the
    // stored source as it was.
    let good = flow_with_window("w13b.fireonce", "    window:\n      every: PT1H\n");
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": good }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let id = common::json(res).await["id"].as_str().unwrap().to_string();
    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": fire_once }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let stored =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await)
            .await;
    assert_eq!(
        stored["source"],
        good.as_str(),
        "the refused update wrote nothing"
    );
}

/// A window shape egret does not implement -- here the artifact's own
/// measured 422, `every` + `lookback` -- is reported by the draft's
/// validation and blocks publish, so no live version can carry it.
#[tokio::test]
async fn a_window_egret_does_not_implement_cannot_be_published() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    let mixed = flow_with_window(
        "w13b.mixed",
        "    window:\n      every: PT1H\n      lookback: PT2M\n",
    );
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": mixed }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();
    let errors = created["validation"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "window_conflicting_fields"
                && e["path"] == "triggers.listen.window.every"),
        "the draft reports the conflict: {created}"
    );

    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/publish"),
        user,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains("Window of type `SLIDING_WINDOW` cannot have an every duration."),
        "publish is refused for the window: {body}"
    );
    let after =
        common::json(common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await)
            .await;
    assert!(
        after["live_version_id"].is_null(),
        "nothing went live: {after}"
    );
}

// ---------- W7a Task 1: the flow schema, id-less validation, if_updated_at ----------

async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(res.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

fn header(res: &axum::response::Response, name: &str) -> String {
    res.headers()
        .get(name)
        .unwrap_or_else(|| panic!("no {name} header"))
        .to_str()
        .unwrap()
        .to_string()
}

async fn flow_count(db: &egret_metastore::Db, ws: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.flows WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A reader gets the schema with a strong `ETag` that is the same on a
/// second call, `Cache-Control: private, no-cache`, and a 304 when it sends
/// the tag back.
#[tokio::test]
async fn the_schema_route_serves_a_reader_with_a_stable_etag_and_answers_304() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let path = format!("/api/ws/{ws}/workflows/schema");

    let first = common::get(&app, &path, user).await;
    assert_eq!(first.status(), StatusCode::OK);
    let etag = header(&first, "etag");
    assert!(
        etag.starts_with("\"sha256-") && etag.ends_with('"'),
        "{etag}"
    );
    assert_eq!(header(&first, "cache-control"), "private, no-cache");
    let schema: serde_json::Value = serde_json::from_slice(&body_bytes(first).await).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );

    let second = common::get(&app, &path, user).await;
    assert_eq!(
        header(&second, "etag"),
        etag,
        "the same workspace, the same tag"
    );

    let res = common::send_with(
        &app,
        "GET",
        &path,
        user,
        &[("If-None-Match", etag.as_bytes())],
        axum::body::Body::empty(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header(&res, "etag"), etag);
    assert!(body_bytes(res).await.is_empty(), "a 304 has no body");
}

/// The schema follows the workspace's features, so two workspaces that
/// differ in `workflow_containers` get different documents and tags.
#[tokio::test]
async fn the_schema_etag_differs_between_workspaces_with_different_features() {
    let Some(db) = common::db().await else { return };
    let (plain, user_a) = common::seed_workspace(&db).await;
    let (with_containers, user_b) = common::seed_workspace(&db).await;
    common::grant(&db, plain, user_a, &["workflow:read"]).await;
    common::grant(&db, with_containers, user_b, &["workflow:read"]).await;
    common::enable_feature(&db, with_containers, "workflow_containers").await;
    // A deployment that can run a script task at all: a runner, and an
    // `allowed-images` entry for it (publish refuses scripts without both).
    let app =
        common::app_with_scripts(&db, egret_workflow::spec::schema::permissive_deployment()).await;

    let a = common::get(&app, &format!("/api/ws/{plain}/workflows/schema"), user_a).await;
    let b = common::get(
        &app,
        &format!("/api/ws/{with_containers}/workflows/schema"),
        user_b,
    )
    .await;
    assert_eq!((a.status(), b.status()), (StatusCode::OK, StatusCode::OK));
    assert_ne!(header(&a, "etag"), header(&b, "etag"));
    let a: serde_json::Value = serde_json::from_slice(&body_bytes(a).await).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&body_bytes(b).await).unwrap();
    let shell = "io.kestra.plugin.scripts.shell.Commands";
    assert!(
        a["$defs"].get(shell).is_none(),
        "containers off: no script type"
    );
    assert!(
        b["$defs"].get(shell).is_some(),
        "containers on: script types"
    );
}

/// The schema needs `workflow:read`, and nothing else does it.
#[tokio::test]
async fn the_schema_route_requires_workflow_read() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let app = common::app(&db).await;
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/schema"), user).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// `POST /flows/validate` answers, for the same source, exactly what the
/// by-id route answers, and writes nothing.
#[tokio::test]
async fn validate_without_an_id_answers_the_by_id_report_and_writes_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let created = common::json(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow("a.idless") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();
    let before = flow_count(&db, ws).await;

    // Errors AND warnings, so both halves of the report are compared. The
    // warning is a flow-level `concurrency:` (`not_yet_enforced`): the typo'd
    // type that supplied it until no-silent-stall Task 1 is an error now.
    let source = "id: f\nnamespace: BAD NAMESPACE\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: egret.sqll\n";
    let by_id = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}/validate"),
        user,
        serde_json::json!({ "source": source }),
    )
    .await;
    let idless = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/validate"),
        user,
        serde_json::json!({ "source": source }),
    )
    .await;
    assert_eq!(
        (by_id.status(), idless.status()),
        (StatusCode::OK, StatusCode::OK)
    );
    let (by_id, idless) = (common::json(by_id).await, common::json(idless).await);
    assert_eq!(idless, by_id);
    assert!(!idless["errors"].as_array().unwrap().is_empty(), "{idless}");
    assert!(
        !idless["warnings"].as_array().unwrap().is_empty(),
        "{idless}"
    );

    assert_eq!(flow_count(&db, ws).await, before, "validate wrote a row");
}

/// A source that does not parse is a 400 whose message carries the line and
/// column, as on the by-id route.
#[tokio::test]
async fn validate_without_an_id_refuses_unparsable_yaml_with_its_position() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/validate"),
        user,
        serde_json::json!({ "source": "id: f\nnamespace: a.b\ntasks: [\n" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::json(res).await;
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("line") && msg.contains("column"), "{msg}");
}

/// Validating is an authoring action: `workflow:read` alone is refused.
#[tokio::test]
async fn validate_without_an_id_requires_workflow_manage() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/validate"),
        user,
        serde_json::json!({ "source": minimal_flow("a.ro") }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// Create a flow and answer `(id, updated_at as the server sent it)`.
async fn created_flow(
    app: &common::App,
    ws: Uuid,
    user: egret_core::UserId,
    ns: &str,
) -> (String, String) {
    let created = common::json(
        common::post(
            app,
            &format!("/api/ws/{ws}/workflows/flows"),
            user,
            serde_json::json!({ "source": minimal_flow(ns) }),
        )
        .await,
    )
    .await;
    (
        created["id"].as_str().unwrap().to_string(),
        created["updated_at"].as_str().unwrap().to_string(),
    )
}

/// `if_updated_at` equal to the row's: the save goes through.
#[tokio::test]
async fn update_with_a_matching_if_updated_at_saves() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let (id, at) = created_flow(&app, ws, user, "a.match").await;

    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": minimal_flow("a.match2"), "folder_id": null, "if_updated_at": at }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = common::json(res).await;
    assert_eq!(body["namespace"], "a.match2");
    assert_ne!(
        body["updated_at"].as_str().unwrap(),
        at,
        "the save moves it"
    );
}

/// A stale `if_updated_at`: 409 with the spec's sentence, and the row is
/// what the other save left.
#[tokio::test]
async fn update_with_a_stale_if_updated_at_is_refused_and_writes_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let (id, opened_at) = created_flow(&app, ws, user, "a.stale").await;
    let path = format!("/api/ws/{ws}/workflows/flows/{id}");

    // Someone else saves first, unconditionally.
    let other = common::put(
        &app,
        &path,
        user,
        serde_json::json!({ "source": minimal_flow("a.other"), "folder_id": null }),
    )
    .await;
    assert_eq!(other.status(), StatusCode::OK);
    let other = common::json(other).await;

    let res = common::put(
        &app,
        &path,
        user,
        serde_json::json!({ "source": minimal_flow("a.mine"), "folder_id": null, "if_updated_at": opened_at }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let refusal = common::json(res).await;
    let msg = refusal["error"].as_str().unwrap().to_string();
    assert!(
        msg.contains("the draft was changed since you opened it"),
        "{msg}"
    );
    // The client branches on the code, never on the sentence.
    assert_eq!(refusal["code"], "draft_changed", "{refusal}");

    let row = common::json(common::get(&app, &path, user).await).await;
    assert_eq!(
        row["namespace"], "a.other",
        "the refused save wrote nothing"
    );
    assert_eq!(row["source"], other["source"]);
    assert_eq!(row["updated_at"], other["updated_at"]);
}

/// A precondition on a flow that does not exist is still a 404, not a 409.
#[tokio::test]
async fn update_with_if_updated_at_on_a_missing_flow_is_404() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{}", Uuid::new_v4()),
        user,
        serde_json::json!({ "source": minimal_flow("a.none"), "folder_id": null, "if_updated_at": "2026-10-10T00:00:00Z" }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// A precondition on ANOTHER workspace's flow is a 404, never a 409: the
/// 409/404 split would otherwise tell a caller that a flow with that id
/// exists in a workspace they cannot see. Both a stale `if_updated_at` and
/// the flow's exact current one are refused the same way, and neither
/// writes. (W7a final review, M3.)
#[tokio::test]
async fn update_with_if_updated_at_on_another_workspaces_flow_is_404() {
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
    let (id, current) = created_flow(&app, owner_ws, owner, "a.theirs").await;
    let owner_path = format!("/api/ws/{owner_ws}/workflows/flows/{id}");
    let foreign_path = format!("/api/ws/{other_ws}/workflows/flows/{id}");

    for if_updated_at in [current.as_str(), "2000-01-01T00:00:00Z"] {
        let res = common::put(
            &app,
            &foreign_path,
            other_user,
            serde_json::json!({ "source": minimal_flow("a.mine"), "folder_id": null, "if_updated_at": if_updated_at }),
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::NOT_FOUND,
            "if_updated_at = {if_updated_at}"
        );
        let body = common::json(res).await;
        assert_ne!(body["code"], "draft_changed", "{body}");
    }

    let row = common::json(common::get(&app, &owner_path, owner).await).await;
    assert_eq!(
        row["namespace"], "a.theirs",
        "the refused saves wrote nothing"
    );
    assert_eq!(row["updated_at"].as_str(), Some(current.as_str()));
}

/// Without `if_updated_at` a save is unconditional, as before W7a: an old
/// draft is overwritten.
#[tokio::test]
async fn update_without_if_updated_at_behaves_as_before() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let (id, _) = created_flow(&app, ws, user, "a.absent").await;
    let path = format!("/api/ws/{ws}/workflows/flows/{id}");
    for ns in ["a.absent1", "a.absent2"] {
        let res = common::put(
            &app,
            &path,
            user,
            serde_json::json!({ "source": minimal_flow(ns), "folder_id": null }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }
    let row = common::json(common::get(&app, &path, user).await).await;
    assert_eq!(row["namespace"], "a.absent2");
}

/// Two saves with the same `if_updated_at`: exactly one wins, every time.
///
/// Deterministic, after `tests/runtime.rs`'s
/// `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`: a third
/// connection holds the flow's row lock, both saves are started and observed
/// **blocked on that lock** in `pg_stat_activity` (never inferred from
/// timing), and only then is the lock released. Both saves have done every
/// read they will do before the release, so an implementation that reads
/// `updated_at` and then writes passes both; the comparison inside the
/// `UPDATE` re-evaluates after the winner commits and refuses the loser.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_saves_with_the_same_if_updated_at_one_wins() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = std::sync::Arc::new(common::app(&db).await);
    let (id, at) = created_flow(&app, ws, user, "a.race").await;
    let path = format!("/api/ws/{ws}/workflows/flows/{id}");

    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow.flows WHERE id = $1 FOR UPDATE")
        .bind(Uuid::parse_str(&id).unwrap())
        .execute(&mut *holder)
        .await
        .unwrap();
    // The save's statement starts `WITH r AS (UPDATE workflow.flows SET ...`
    // since W7a's creator-name join (spec §7.4), hence the leading `%`. This
    // probe's own text matches too, but it is never waiting on a lock.
    let blocked_saves = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock' \
                AND query LIKE '%UPDATE workflow.flows SET%'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };

    let mut handles = Vec::new();
    for (n, ns) in ["a.race1", "a.race2"].into_iter().enumerate() {
        let (app, path, at) = (app.clone(), path.clone(), at.clone());
        handles.push(tokio::spawn(async move {
            common::put(
                &app,
                &path,
                user,
                serde_json::json!({ "source": minimal_flow(ns), "folder_id": null, "if_updated_at": at }),
            )
            .await
            .status()
        }));
        // Wait for THIS save to be blocked before starting the next one.
        let start = std::time::Instant::now();
        while blocked_saves().await < (n as i64) + 1 {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "save {n} never blocked on the held row lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    assert!(handles.iter().all(|h| !h.is_finished()), "both saves wait");
    holder.commit().await.unwrap();

    let mut statuses = Vec::new();
    for h in handles {
        statuses.push(h.await.unwrap());
    }
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::OK, StatusCode::CONFLICT]);
}

// ---------------------------------------------------------------------------
// W7a spec §7.4: creator names, joined server-side.
// ---------------------------------------------------------------------------

/// Sets `user`'s profile name, so an assertion on it cannot be satisfied by
/// the seed's shared `'t'`.
async fn name_user(db: &egret_metastore::Db, user: egret_core::UserId, name: &str) {
    sqlx::query("UPDATE users SET name = $2 WHERE id = $1")
        .bind(user.0)
        .bind(name)
        .execute(&db.pool)
        .await
        .expect("name user");
}

/// Every DTO that carries `created_by` carries its name too, from every route
/// that answers one: create, get, list, put, publish, versions, version.
#[tokio::test]
async fn every_flow_route_names_the_creator() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let name = format!("Ada {}", Uuid::new_v4().simple());
    name_user(&db, user, &name).await;

    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/flows");
    let created = common::json(
        common::post(
            &app,
            &base,
            user,
            serde_json::json!({ "source": minimal_flow("a.named") }),
        )
        .await,
    )
    .await;
    assert_eq!(created["created_by_name"], name, "create: {created}");
    let id = created["id"].as_str().unwrap();

    let got = common::json(common::get(&app, &format!("{base}/{id}"), user).await).await;
    assert_eq!(got["created_by_name"], name, "get: {got}");

    let list = common::json(common::get(&app, &base, user).await).await;
    assert_eq!(list[0]["created_by_name"], name, "list: {list}");

    let put = common::json(
        common::put(
            &app,
            &format!("{base}/{id}"),
            user,
            serde_json::json!({ "source": minimal_flow("a.named2") }),
        )
        .await,
    )
    .await;
    assert_eq!(put["created_by_name"], name, "put: {put}");

    let published = common::json(
        common::post(
            &app,
            &format!("{base}/{id}/publish"),
            user,
            serde_json::json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(published["created_by_name"], name, "publish: {published}");
    let versions =
        common::json(common::get(&app, &format!("{base}/{id}/versions"), user).await).await;
    assert_eq!(versions[0]["created_by_name"], name, "versions: {versions}");
    let one = common::json(common::get(&app, &format!("{base}/{id}/versions/1"), user).await).await;
    assert_eq!(one["created_by_name"], name, "version: {one}");
}

/// No profile name: the email stands in, as for dashboards.
#[tokio::test]
async fn a_creator_without_a_profile_name_is_named_by_email() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    name_user(&db, user, "").await;
    let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(user.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/flows");
    common::post(
        &app,
        &base,
        user,
        serde_json::json!({ "source": minimal_flow("a.email") }),
    )
    .await;
    let list = common::json(common::get(&app, &base, user).await).await;
    assert_eq!(list[0]["created_by_name"], email, "{list}");
}

/// A row with no creator answers a null name -- and is still listed: the
/// join is a LEFT JOIN, so the row is not dropped.
#[tokio::test]
async fn a_flow_without_a_creator_has_a_null_name_and_is_still_listed() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/flows");
    let created = common::json(
        common::post(
            &app,
            &base,
            user,
            serde_json::json!({ "source": minimal_flow("a.nobody") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();
    sqlx::query("UPDATE workflow.flows SET created_by = NULL WHERE id = $1::uuid")
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();

    let list = common::json(common::get(&app, &base, user).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert!(list[0]["created_by"].is_null());
    assert!(list[0]["created_by_name"].is_null(), "{list}");
    let got = common::json(common::get(&app, &format!("{base}/{id}"), user).await).await;
    assert!(got["created_by_name"].is_null(), "{got}");
}

/// The creator's account is deleted: the name goes with it (null), and the
/// flow and its version stay listed.
#[tokio::test]
async fn a_deleted_creator_leaves_a_null_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/flows");
    let created = common::json(
        common::post(
            &app,
            &base,
            user,
            serde_json::json!({ "source": minimal_flow("a.gone") }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap();
    common::post(
        &app,
        &format!("{base}/{id}/publish"),
        user,
        serde_json::json!({}),
    )
    .await;

    // A second account owns the flow and its version, and is then deleted.
    // (The caller cannot be the one deleted: it must still be able to read.)
    let leaver: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 'Leaver', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("leaver_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow.flows SET created_by = $2 WHERE id = $1::uuid")
        .bind(id)
        .bind(leaver)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow.flow_versions SET created_by = $2 WHERE flow_id = $1::uuid")
        .bind(id)
        .bind(leaver)
        .execute(&db.pool)
        .await
        .unwrap();
    // Control: the name is there while the account exists.
    let before = common::json(common::get(&app, &base, user).await).await;
    assert_eq!(before[0]["created_by_name"], "Leaver", "{before}");

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(leaver)
        .execute(&db.pool)
        .await
        .unwrap();

    let list = common::json(common::get(&app, &base, user).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert!(list[0]["created_by_name"].is_null(), "{list}");
    let versions =
        common::json(common::get(&app, &format!("{base}/{id}/versions"), user).await).await;
    assert_eq!(versions.as_array().unwrap().len(), 1, "{versions}");
    assert!(versions[0]["created_by_name"].is_null(), "{versions}");
}

/// `can_manage` on `GET /flows/{id}` is the PDP's `workflow:manage` answer
/// (W7a Task 5): the editor opens read-only on `false`. A reader gets
/// `false`, and the control below, identical but for the grant, gets `true`.
#[tokio::test]
async fn get_flow_says_whether_the_caller_may_manage_it() {
    let Some(db) = common::db().await else { return };
    for (perms, expected) in [
        (&["workflow:read"][..], false),
        (&["workflow:read", "workflow:manage"][..], true),
    ] {
        let (ws, user) = common::seed_workspace(&db).await;
        common::grant(&db, ws, user, perms).await;
        // Built after the grant, as every other test in this file builds it.
        let app = common::app(&db).await;
        let id = common::seed_flow(&db, ws, user, &minimal_flow("a.can"), "draft").await;
        let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
        assert_eq!(res.status(), StatusCode::OK, "{perms:?}");
        let body = common::json(res).await;
        assert_eq!(body["can_manage"], expected, "{perms:?}: {body}");
    }
}

/// The other 409 a `PUT` can answer -- another flow holds the namespace and
/// id -- carries no `code`, even with a matching `if_updated_at`, so a client
/// branching on `draft_changed` cannot mistake it for a stale save.
#[tokio::test]
async fn an_identity_collision_on_update_is_a_409_without_the_draft_changed_code() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let (_taken, _) = created_flow(&app, ws, user, "a.taken").await;
    let (id, at) = created_flow(&app, ws, user, "a.mover").await;

    let res = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": minimal_flow("a.taken"), "folder_id": null, "if_updated_at": at }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let refusal = common::json(res).await;
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{refusal}"
    );
    assert!(refusal.get("code").is_none(), "{refusal}");
}

/// Create and update answer `can_manage: true`: the caller just passed the
/// `workflow:manage` check those routes make.
#[tokio::test]
async fn create_and_update_answer_can_manage_true() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let created = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": minimal_flow("a.cm") }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = common::json(created).await;
    assert_eq!(created["can_manage"], true, "{created}");

    let id = created["id"].as_str().unwrap();
    let updated = common::put(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{id}"),
        user,
        serde_json::json!({ "source": minimal_flow("a.cm2"), "folder_id": null }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = common::json(updated).await;
    assert_eq!(updated["can_manage"], true, "{updated}");
}

/// A PDP that fails while deciding `workflow:manage` is an error (500), not
/// a "no": `get_flow` must not open the editor read-only on an outage. The
/// read check (`workflow:read`) goes to the real PDP and passes, so the 500
/// can only come from the `can_manage` decision.
#[tokio::test]
async fn get_flow_answers_an_error_when_the_can_manage_decision_fails() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let id = common::seed_flow(&db, ws, user, &minimal_flow("a.pdp"), "draft").await;

    // Control: the same request through an app whose PDP fails on nothing.
    let ok = common::app_with_failing_pdp_action(&db, "no-such-action").await;
    let res = common::get(&ok, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
    assert_eq!(res.status(), StatusCode::OK);

    let app = common::app_with_failing_pdp_action(&db, "workflow:manage").await;
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{id}"), user).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// `latest_version` (W7a spec §4, §5.3): the flow's highest version number,
/// null with none, from every route that answers a `FlowDto` or a
/// `FlowListItemDto`. Three flows in one workspace -- none, one and three
/// versions -- so a number taken from the wrong flow's versions, or a count
/// standing in for the maximum, reads differently from the right one.
#[tokio::test]
async fn every_flow_route_carries_the_latest_version_number() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/flows");

    let mut ids = Vec::new();
    for ns in ["v.none", "v.one", "v.three"] {
        let created = common::json(
            common::post(
                &app,
                &base,
                user,
                serde_json::json!({ "source": minimal_flow(ns) }),
            )
            .await,
        )
        .await;
        assert!(created["latest_version"].is_null(), "create: {created}");
        ids.push(created["id"].as_str().unwrap().to_string());
    }
    for (id, times) in [(&ids[1], 1), (&ids[2], 3)] {
        for _ in 0..times {
            let url = format!("{base}/{id}/publish");
            let res = common::post(&app, &url, user, serde_json::json!({})).await;
            assert_eq!(res.status(), StatusCode::OK);
        }
    }

    let expected = [serde_json::Value::Null, 1.into(), 3.into()];
    for (id, want) in ids.iter().zip(&expected) {
        let got = common::json(common::get(&app, &format!("{base}/{id}"), user).await).await;
        assert_eq!(&got["latest_version"], want, "get {id}: {got}");
    }
    let list = common::json(common::get(&app, &base, user).await).await;
    for (id, want) in ids.iter().zip(&expected) {
        let item = list
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id.as_str())
            .unwrap_or_else(|| panic!("{id} not listed: {list}"));
        assert_eq!(&item["latest_version"], want, "list {id}: {item}");
    }
    // A save after versions keeps the number: the PUT answers it too.
    let put = common::json(
        common::put(
            &app,
            &format!("{base}/{}", ids[2]),
            user,
            serde_json::json!({ "source": minimal_flow("v.three") + "description: edited\n" }),
        )
        .await,
    )
    .await;
    assert_eq!(put["latest_version"], 3, "put: {put}");
}
