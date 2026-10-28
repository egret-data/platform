//! `POST /api/ws/{workspace}/workflows/flows/{id}/executions/validate`
//! (package #3b, spec D14): the start's input check, every input listed,
//! nothing stored.
//!
//! DB-gated like its siblings: each test returns early without
//! `DATABASE_URL`.

use serde_json::{json, Value};
use uuid::Uuid;

mod common;

/// A flow whose inputs cover a bad value, a missing required one, a
/// `dependsOn` that disables, a SECRET with a `validator`, a FILE and an
/// optional one.
const FLOW: &str = "id: checked
namespace: a.validate
inputs:
  - id: n
    type: INT
    min: 1
  - id: name
    type: STRING
  - id: gate
    type: BOOL
    required: false
  - id: dep
    type: STRING
    dependsOn:
      inputs: [gate]
      condition: \"{{ inputs.gate }}\"
  - id: token
    type: SECRET
    validator: \"^s3cr[a-z0-9]+$\"
  - id: doc
    type: FILE
    required: false
  - id: level
    type: SELECT
    values: [LOW, HIGH]
    defaults: LOW
tasks:
  - id: a
    type: io.kestra.plugin.core.log.Log
    message: hi
";

fn path(ws: Uuid, flow: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/flows/{flow}/executions/validate")
}

async fn setup(perms: &[&str]) -> Option<(egret_metastore::Db, Uuid, egret_core::UserId, Uuid)> {
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, perms).await;
    let flow = common::seed_flow(&db, ws, user, FLOW, "draft").await;
    Some((db, ws, user, flow))
}

fn by_id<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["inputs"]
        .as_array()
        .expect("inputs is a list")
        .iter()
        .find(|i| i["input"]["id"] == id)
        .unwrap_or_else(|| panic!("input {id} is listed: {body}"))
}

#[tokio::test]
async fn invalid_inputs_answer_200_with_every_input_and_its_errors() {
    let Some((db, ws, user, flow)) = setup(&["workflow:manage"]).await else {
        return;
    };
    let app = common::app(&db).await;
    let before = common::execution_count(&db, ws).await;

    let res = common::post(
        &app,
        &path(ws, flow),
        user,
        json!({"inputs": {"n": "0", "gate": false, "dep": "x", "token": "s3cr-leaked!", "doc": "a.csv"}}),
    )
    .await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(body["id"], "checked");
    assert_eq!(body["namespace"], "a.validate");
    assert_eq!(body["checks"], json!([]));
    let ids: Vec<&str> = body["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["input"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["n", "name", "gate", "dep", "token", "doc", "level"]);

    let n = by_id(&body, "n");
    assert_eq!(n["input"], json!({"id": "n", "type": "INT", "min": 1}));
    assert_eq!(n["enabled"], true);
    assert_eq!(
        n["errors"],
        json!([{"message": "Invalid value for input `n`. Cause: it must be more than `1`", "renderError": false}])
    );
    assert_eq!(
        by_id(&body, "name")["errors"],
        json!([{"message": "Missing required input:name", "renderError": false}])
    );
    // Disabled by `dependsOn` (gate is false): listed, enabled=false, no error.
    let dep = by_id(&body, "dep");
    assert_eq!(dep["enabled"], false);
    assert_eq!(dep["errors"], json!([]));
    // SECRET (M6): the value is masked even on an error, and the error does
    // not carry the clear text.
    let token = by_id(&body, "token");
    assert_eq!(token["value"], "******");
    assert_eq!(token["errors"].as_array().unwrap().len(), 1);
    assert!(!body.to_string().contains("s3cr-leaked!"), "{body}");
    // FILE: the posted name, nothing uploaded.
    assert_eq!(by_id(&body, "doc")["value"], "a.csv");
    let level = by_id(&body, "level");
    assert_eq!(level["isDefault"], true);
    assert_eq!(level["value"], "LOW");

    assert_eq!(
        common::execution_count(&db, ws).await,
        before,
        "nothing stored"
    );
    assert!(
        common::audit_mutations(&app).is_empty(),
        "no audit record: {:?}",
        common::audit_mutations(&app)
    );
}

#[tokio::test]
async fn a_valid_object_has_no_errors_and_masks_the_secret() {
    let Some((db, ws, user, flow)) = setup(&["workflow:manage"]).await else {
        return;
    };
    let app = common::app(&db).await;
    let before = common::execution_count(&db, ws).await;

    let res = common::post(
        &app,
        &path(ws, flow),
        user,
        json!({"inputs": {"n": "5", "name": "x", "gate": "true", "dep": "y", "token": "s3cret9"}}),
    )
    .await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    for input in body["inputs"].as_array().unwrap() {
        assert_eq!(input["errors"], json!([]), "{input}");
        assert_eq!(input["enabled"], true, "{input}");
    }
    assert_eq!(by_id(&body, "n")["value"], 5);
    assert_eq!(by_id(&body, "gate")["value"], true);
    assert_eq!(by_id(&body, "token")["value"], "******");
    assert!(!body.to_string().contains("s3cret9"), "{body}");
    assert_eq!(common::execution_count(&db, ws).await, before);
}

#[tokio::test]
async fn a_failing_condition_is_a_render_error() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let source = "id: cond\nnamespace: a.validate\ninputs:\n  - id: a\n    type: STRING\n  \
                  - id: b\n    type: STRING\n    dependsOn:\n      inputs: [a]\n      \
                  condition: \"{{ inputs.a | nosuchfilter }}\"\n\
                  tasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let flow = common::seed_flow(&db, ws, user, source, "draft").await;
    let app = common::app(&db).await;

    let res = common::post(&app, &path(ws, flow), user, json!({"inputs": {"a": "x"}})).await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    let b = by_id(&body, "b");
    assert_eq!(b["enabled"], false);
    assert_eq!(b["errors"][0]["renderError"], true, "{body}");
    assert!(b["errors"][0]["message"]
        .as_str()
        .unwrap()
        .starts_with("Invalid condition: "));
}

#[tokio::test]
async fn no_body_is_an_empty_object() {
    let Some((db, ws, user, flow)) = setup(&["workflow:manage"]).await else {
        return;
    };
    let app = common::app(&db).await;

    let res = common::post_no_body(&app, &path(ws, flow), user).await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(
        by_id(&body, "n")["errors"][0]["message"],
        "Missing required input:n"
    );
}

#[tokio::test]
async fn a_body_or_inputs_that_is_not_an_object_is_400() {
    let Some((db, ws, user, flow)) = setup(&["workflow:manage"]).await else {
        return;
    };
    let app = common::app(&db).await;

    for body in [
        json!([1, 2]),
        json!({"inputs": [1]}),
        json!({"inputs": "x"}),
    ] {
        let res = common::post(&app, &path(ws, flow), user, body.clone()).await;
        assert_eq!(res.status(), 400, "{body}");
    }
    let res = common::send_raw(
        &app,
        "POST",
        &path(ws, flow),
        user,
        "application/json",
        b"{not json".to_vec(),
    )
    .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn a_missing_trashed_or_foreign_flow_is_404_and_a_disabled_one_409() {
    let Some((db, ws, user, _flow)) = setup(&["workflow:manage"]).await else {
        return;
    };
    let by_column = common::seed_flow(
        &db,
        ws,
        user,
        &FLOW.replace("id: checked", "id: off1"),
        "disabled",
    )
    .await;
    let by_yaml = common::seed_flow(
        &db,
        ws,
        user,
        &format!(
            "{}disabled: true\n",
            FLOW.replace("id: checked", "id: off2")
        ),
        "draft",
    )
    .await;
    let trashed = common::seed_flow(
        &db,
        ws,
        user,
        &FLOW.replace("id: checked", "id: gone"),
        "draft",
    )
    .await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&db.pool)
        .await
        .expect("trash the flow");
    let (ws2, other) = common::seed_workspace(&db).await;
    let foreign = common::seed_flow(&db, ws2, other, FLOW, "draft").await;
    let app = common::app(&db).await;

    for (flow, want) in [
        (Uuid::new_v4(), 404),
        (trashed, 404),
        (foreign, 404),
        (by_column, 409),
        (by_yaml, 409),
    ] {
        let res = common::post(&app, &path(ws, flow), user, json!({"inputs": {}})).await;
        assert_eq!(res.status(), want, "{flow}");
    }
}

#[tokio::test]
async fn the_gate_is_the_starts() {
    let Some((db, ws, reader, flow)) = setup(&["workflow:read"]).await else {
        return;
    };
    let (_ws2, manager) = common::seed_workspace(&db).await;
    common::grant(&db, ws, manager, &["workflow:manage"]).await;
    let app = common::app(&db).await;

    // `:read` alone cannot start, so it cannot validate either.
    let res = common::post(&app, &path(ws, flow), reader, json!({"inputs": {}})).await;
    assert_eq!(res.status(), 403);
    assert_eq!(
        common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
            reader,
            json!({"inputs": {}})
        )
        .await
        .status(),
        403,
        "the start refuses the same holder"
    );
    assert_eq!(common::anon_post(&app, &path(ws, flow)).await.status(), 401);

    // The control: `:manage` passes.
    let res = common::post(&app, &path(ws, flow), manager, json!({"inputs": {}})).await;
    assert_eq!(res.status(), 200);
}
