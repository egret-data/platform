//! The anonymous webhook surface — W6 Task 7.
//!
//! DB-gated; self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set, per
//! `egret_test_db::url()`'s contract. Run with:
//!
//! ```text
//! export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//! cargo test -p egret-workflow --test webhook_route
//! ```
//!
//! # The negative tests come first, and that is the file's order on purpose
//!
//! A webhook is an unauthenticated public surface by construction: the key in
//! the path is the whole credential. The interesting question is therefore not
//! "does the right key work" but **"what does the wrong one do, and what does a
//! key from another workspace do"** — and this repository has already paid for
//! the second one once, on `chart_id`, which allowed a cross-workspace read
//! before anybody noticed the column had no scope.
//!
//! So every refusal in this file asserts **two** things: the status code, and
//! `execution_count`. A 404 that nevertheless started an execution is the
//! failure that matters, and a status assertion alone cannot see it.
//!
//! # Every fixture's fields are pairwise distinct, and that is not decoration
//!
//! W6 Task 6 shipped a `max_depth` that survived being replaced by the literal
//! `100`, because every fixture passed the default — the mutation had nothing
//! to disagree with. Swept properly, nine fields had that shape. The fixtures
//! here therefore never let the namespace, the flow id, the trigger id and the
//! key be the same string, so a handler that read the wrong one of the four
//! cannot pass by coincidence. `the_four_path_segments_are_not_interchangeable`
//! is the test that holds that directly.

mod common;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// The shared fixture. Four pairwise-distinct strings:
///
/// * namespace `fx.hooks`
/// * flow id    `fx-flow`
/// * trigger id `fx-trigger`
/// * key        `fx-key`
fn hook_flow() -> String {
    "\
id: fx-flow
namespace: fx.hooks
tasks:
  - id: fx-task
    type: core.log
    message: hello
triggers:
  - id: fx-trigger
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-key
"
    .to_string()
}

fn url(ws: Uuid, ns: &str, flow: &str, key: &str) -> String {
    format!("/workflows/v1/{ws}/executions/webhook/{ns}/{flow}/{key}")
}

fn hook_url(ws: Uuid, key: &str) -> String {
    url(ws, "fx.hooks", "fx-flow", key)
}

// ===========================================================================
// NEGATIVE — the wrong key, the missing key, the key from another workspace
// ===========================================================================

/// The plainest refusal, and the one every other negative test is measured
/// against.
#[tokio::test]
async fn a_wrong_key_refuses_and_starts_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "wrong-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "a refused webhook must leave no execution behind"
    );

    // The control. Without it the assertion above is satisfied by a handler
    // that refuses EVERYTHING, which is the most likely way this file could be
    // green and worthless.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "the right key does fire");
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// A URL with no key segment is not this route at all. It must not fall
/// through to a handler that treats the flow id as the key.
#[tokio::test]
async fn a_missing_key_segment_is_not_this_route() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &format!("/workflows/v1/{ws}/executions/webhook/fx.hooks/fx-flow"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // An EMPTY key segment is a different URL from a missing one, and it must
    // also refuse -- a trigger with no `key:` would otherwise be reachable.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, ""),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert!(
        res.status() == StatusCode::NOT_FOUND,
        "an empty key segment answered {}",
        res.status()
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// **The cross-workspace leak test.**
///
/// Two workspaces, each holding a flow with the SAME namespace and the SAME
/// flow id — which `UNIQUE (workspace_id, namespace, flow_id)` permits and
/// which is exactly what two people importing the same corpus produces — and
/// **different keys**.
///
/// Four assertions, and the counts are the load-bearing ones:
///
/// * B's key at A's URL → 404, and **neither** workspace gains an execution;
/// * A's key at B's URL → 404, and neither gains one (the symmetric direction,
///   because a handler that resolved "the first flow by id" would leak in
///   exactly one direction and a one-sided test would miss half of it);
/// * A's key at A's URL → 200, with the execution in **A only**;
/// * B's key at B's URL → 200, with the execution in **B only**.
///
/// **The mutation this test exists for** is deleting `workspace_id = $1` from
/// the flow lookup in `webhook_routes::handle`. `worker::subflow::resolve`
/// already recorded that nothing in the schema objects to the resulting row, so
/// this test is the only thing that does.
#[tokio::test]
async fn a_key_from_another_workspace_fires_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;

    let flow_a = hook_flow().replace("key: fx-key", "key: key-of-a");
    let flow_b = hook_flow().replace("key: fx-key", "key: key-of-b");
    common::seed_flow(&db, ws_a, user_a, &flow_a, "draft").await;
    common::seed_flow(&db, ws_b, user_b, &flow_b, "draft").await;

    let app = common::webhook_app(&db).await;

    // B's key at A's URL.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, "key-of-b"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "B's key must not open A"
    );
    assert_eq!(common::execution_count(&db, ws_a).await, 0);
    assert_eq!(
        common::execution_count(&db, ws_b).await,
        0,
        "and it must not have fired B's flow through A's URL either"
    );

    // A's key at B's URL -- the symmetric direction.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_b, "key-of-a"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "A's key must not open B"
    );
    assert_eq!(common::execution_count(&db, ws_a).await, 0);
    assert_eq!(common::execution_count(&db, ws_b).await, 0);

    // Each key at its own URL -- the controls, without which both assertions
    // above are satisfied by a handler that never fires anything.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, "key-of-a"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_a).await, 1);
    assert_eq!(
        common::execution_count(&db, ws_b).await,
        0,
        "A's call is A's alone"
    );

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_b, "key-of-b"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_a).await, 1);
    assert_eq!(common::execution_count(&db, ws_b).await, 1);
}

/// The harder half of the same class: two workspaces holding flows that are
/// **byte-identical**, key included.
///
/// The previous test can be passed by a handler that resolves the flow without
/// a workspace predicate and then happens to pick the right row, because the
/// keys differ. This one cannot: both rows match on every column the lookup
/// touches except `workspace_id`, so the ONLY thing that can select the right
/// one is the predicate this test exists to protect.
#[tokio::test]
async fn two_workspaces_holding_the_same_flow_and_the_same_key_do_not_cross() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws_a, user_a, &hook_flow(), "draft").await;
    common::seed_flow(&db, ws_b, user_b, &hook_flow(), "draft").await;

    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        common::execution_count(&db, ws_a).await,
        1,
        "A was addressed"
    );
    assert_eq!(
        common::execution_count(&db, ws_b).await,
        0,
        "B holds an identical flow with an identical key and must not have run"
    );

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_b, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_a).await, 1);
    assert_eq!(common::execution_count(&db, ws_b).await, 1);
}

/// A workspace that is not `active` answers the same 404 as one that does not
/// exist — a key minted before the workspace was archived must stop working.
#[tokio::test]
async fn an_archived_workspace_refuses() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    // The control FIRST, so that the refusal below is attributable to the
    // archive and to nothing else.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "it works while active");
    assert_eq!(common::execution_count(&db, ws).await, 1);

    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(ws)
        .execute(&db.pool)
        .await
        .expect("archive");

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "still the one from before the archive"
    );
}

/// A soft-deleted flow answers the same 404.
#[tokio::test]
async fn a_deleted_flow_refuses() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .expect("soft delete");

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// **The non-disclosure property**, and it compares bodies rather than statuses.
///
/// Kestra answers `Flow not found` for one of these and `Webhook not found` for
/// another, which lets an anonymous caller enumerate which flows a namespace
/// holds. All four must be one response here, byte for byte.
#[tokio::test]
async fn the_four_absences_are_indistinguishable() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let mut seen = Vec::new();
    for (what, path) in [
        (
            "no such workspace",
            url(Uuid::new_v4(), "fx.hooks", "fx-flow", "fx-key"),
        ),
        (
            "no such namespace",
            url(ws, "fx.other", "fx-flow", "fx-key"),
        ),
        ("no such flow", url(ws, "fx.hooks", "fx-other", "fx-key")),
        ("no such key", hook_url(ws, "fx-other-key")),
    ] {
        let res = common::anon(&app, "POST", &path, Some("application/json"), b"{}").await;
        let status = res.status();
        let body = common::body_text(res).await;
        seen.push((what, status, body));
    }

    let (_, first_status, first_body) = seen[0].clone();
    assert_eq!(first_status, StatusCode::NOT_FOUND);
    for (what, status, body) in &seen {
        assert_eq!(*status, first_status, "{what} answered a different status");
        assert_eq!(
            *body, first_body,
            "{what} answered a different BODY -- that is an existence oracle"
        );
    }
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // The control: the response they must all differ from.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

/// **The operand test.** The four path segments carry four different facts, and
/// swapping any two of them must refuse.
///
/// Without this, a handler that (say) compared the key against the trigger's
/// `id` rather than its `key` would pass every other test in a file whose
/// fixture happened to name them the same.
#[tokio::test]
async fn the_four_path_segments_are_not_interchangeable() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    for (what, path) in [
        ("the trigger id used as the key", hook_url(ws, "fx-trigger")),
        ("the flow id used as the key", hook_url(ws, "fx-flow")),
        ("the namespace used as the key", hook_url(ws, "fx.hooks")),
        (
            "the key used as the flow id",
            url(ws, "fx.hooks", "fx-key", "fx-key"),
        ),
        (
            "namespace and flow swapped",
            url(ws, "fx-flow", "fx.hooks", "fx-key"),
        ),
        (
            "the workspace slug used as the namespace",
            url(ws, &ws.to_string(), "fx-flow", "fx-key"),
        ),
    ] {
        let res = common::anon(&app, "POST", &path, Some("application/json"), b"{}").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{what} was accepted");
    }
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// The URL key is **not** trimmed, and the declared one is. Both directions,
/// measured on the artifact (200 / 404) and reproduced end to end here.
#[tokio::test]
async fn the_url_key_is_not_trimmed_and_the_declared_key_is() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = hook_flow().replace("key: fx-key", "key: \"  fx-spaced  \"");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    // The declared key trims, so the bare form matches.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-spaced"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);

    // The URL key does NOT trim, so the padded form does not.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "%20%20fx-spaced%20%20"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "no second execution"
    );
}

/// A key template is compared **rendered**; its source text is not a key.
#[tokio::test]
async fn a_dynamic_key_is_compared_rendered() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = hook_flow().replace("key: fx-key", "key: \"dyn-{{ flow.id }}\"");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    // The SOURCE text must not match.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "dyn-%7B%7B%20flow.id%20%7D%7D"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // The RENDERED value must.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "dyn-fx-flow"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// A body larger than [`egret_workflow::MAX_WEBHOOK_BODY_BYTES`] is refused by
/// the layer, before the handler and before any database work.
///
/// The only bound this anonymous surface has, so it is asserted rather than
/// assumed — and with a control one byte under the cap, because a test that
/// only sends an oversized body passes against a router that refuses every
/// body.
#[tokio::test]
async fn an_oversized_body_is_refused_by_the_cap() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let cap = egret_workflow::MAX_WEBHOOK_BODY_BYTES;
    let over = vec![b'x'; cap + 1];
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("text/plain"),
        &over,
    )
    .await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(common::execution_count(&db, ws).await, 0);

    let under = vec![b'x'; cap];
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("text/plain"),
        &under,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "exactly at the cap is accepted"
    );
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

// ===========================================================================
// REFUSALS a key-holder is entitled to an explanation for
// ===========================================================================

/// A disabled trigger is matched and then refused **by name**, with Kestra's
/// own sentence. Filtering it out at the match would have made it a 404 and
/// lost the operator's only clue.
#[tokio::test]
async fn a_disabled_trigger_is_refused_by_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = hook_flow().replace("    key: fx-key\n", "    key: fx-key\n    disabled: true\n");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = common::body_text(res).await;
    assert!(
        body.contains("the trigger 'fx-trigger' is disabled"),
        "the refusal must name the trigger; got {body}"
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // And a caller WITHOUT the key learns nothing about it -- the 409 is only
    // ever visible to someone who already proved they hold the credential.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "wrong"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// A disabled flow is 409 for a key-holder, and the control is the same flow
/// enabled.
#[tokio::test]
async fn a_disabled_flow_is_refused_after_the_key_matches() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = common::seed_flow(&db, ws, user, &hook_flow(), "disabled").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(common::body_text(res).await.contains("flow is disabled"));
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // Without the key: 404, not 409. Kestra answers 409 here; this is the
    // deliberate divergence.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "wrong"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // The control: `state` is the only thing that changed.
    sqlx::query("UPDATE workflow.flows SET state = 'draft' WHERE id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .expect("enable");
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// The three things this port cannot do are refused **by name**, never
/// approximated. An approximated `fetchType: STORE` would hand the flow a
/// trigger with the body silently missing.
#[tokio::test]
async fn the_unsupported_declarations_are_refused_by_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let app = common::webhook_app(&db).await;

    for (suffix, needle, ns) in [
        ("    wait: true\n", "wait: true", "fx.wait"),
        ("    fetchType: STORE\n", "fetchType: STORE", "fx.store"),
    ] {
        let flow = hook_flow()
            .replace("namespace: fx.hooks", &format!("namespace: {ns}"))
            .replace("    key: fx-key\n", &format!("    key: fx-key\n{suffix}"));
        common::seed_flow(&db, ws, user, &flow, "draft").await;
        let res = common::anon(
            &app,
            "POST",
            &url(ws, ns, "fx-flow", "fx-key"),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(res.status(), StatusCode::CONFLICT, "{needle}");
        let body = common::body_text(res).await;
        assert!(
            body.contains(needle),
            "the refusal must name {needle}; got {body}"
        );
        assert!(body.contains("fx-trigger"), "and the trigger; got {body}");
    }

    // multipart is refused on the REQUEST rather than on the declaration.
    let flow = hook_flow().replace("namespace: fx.hooks", "namespace: fx.multi");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &url(ws, "fx.multi", "fx-flow", "fx-key"),
        Some("multipart/form-data; boundary=b"),
        b"--b--",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(common::body_text(res).await.contains("multipart/form-data"));

    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "not one of the three refusals started anything"
    );
}

/// `inputs:` that read `trigger.*` are refused by name rather than letting the
/// renderer blame the author's template for a root egret does not publish.
#[tokio::test]
async fn inputs_reading_the_trigger_root_are_refused_by_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = "\
id: fx-flow
namespace: fx.hooks
inputs:
  - id: payload
    type: STRING
tasks:
  - id: fx-task
    type: core.log
    message: hello
triggers:
  - id: fx-trigger
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-key
    inputs:
      payload: \"{{ trigger.body }}\"
";
    common::seed_flow(&db, ws, user, flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = common::body_text(res).await;
    assert!(
        body.contains("trigger.*") && body.contains("fx-trigger"),
        "the refusal must say what it cannot do and which trigger declared it; got {body}"
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

// ===========================================================================
// POSITIVE — what actually reaches the execution
// ===========================================================================

/// The whole of what a fired webhook stores, compared field by field against
/// the shape measured on the artifact.
#[tokio::test]
async fn a_fired_webhook_stores_the_trigger_record() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &format!(
            "{}?validationToken=abc&validationToken=def&x=1",
            hook_url(ws, "fx-key")
        ),
        Some("application/json"),
        br#"{"hello":"world","n":3}"#,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let stored = common::only_execution_trigger(&db, ws)
        .await
        .expect("execution_trigger is not null for a webhook-started execution");

    assert_eq!(stored["id"], json!("fx-trigger"));
    assert_eq!(
        stored["type"],
        json!("io.kestra.plugin.core.trigger.Webhook")
    );
    assert_eq!(
        stored["variables"]["body"],
        json!({"hello": "world", "n": 3}),
        "a JSON object body arrives as an OBJECT, not as a string"
    );
    assert_eq!(
        stored["variables"]["parameters"]["validationToken"],
        json!(["abc", "def"]),
        "a repeated query parameter keeps both values, in URL order"
    );
    assert_eq!(stored["variables"]["parameters"]["x"], json!(["1"]));
    assert_eq!(
        stored["variables"]["headers"]["content-type"],
        json!(["application/json"])
    );
    assert!(
        stored["variables"].get("uri").is_none(),
        "no uri without fetchType: STORE"
    );

    // And the response echoes the same record.
    let body: serde_json::Value = serde_json::from_str(&common::body_text(res).await).unwrap();
    assert_eq!(body["trigger"], stored);
    assert_eq!(body["state"], json!("CREATED"));
    assert_eq!(body["namespace"], json!("fx.hooks"));
    assert_eq!(body["flow_id"], json!("fx-flow"));
    assert_eq!(body["workspace_id"], json!(ws.to_string()));
}

/// A `GET` carries no body, and `body` is then absent entirely — which is a
/// different fact from an empty one. The control is the same URL under `POST`
/// with a body, on the same flow.
#[tokio::test]
async fn a_get_carries_parameters_and_no_body() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::seed_flow(&db, ws, user, &hook_flow(), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "GET",
        &format!("{}?q=7", hook_url(ws, "fx-key")),
        None,
        b"",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&common::body_text(res).await).unwrap();
    assert!(
        body["trigger"]["variables"].get("body").is_none(),
        "a GET has no body key at all"
    );
    assert_eq!(
        body["trigger"]["variables"]["parameters"]["q"],
        json!(["7"])
    );

    // PUT reaches the same handler.
    let res = common::anon(
        &app,
        "PUT",
        &hook_url(ws, "fx-key"),
        Some("text/plain"),
        b"put-body",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&common::body_text(res).await).unwrap();
    assert_eq!(
        body["trigger"]["variables"]["body"],
        json!("put-body"),
        "the control: a method that DOES carry a body still does"
    );
}

/// Two webhook triggers on one flow are two different doors, and each opens
/// only its own.
#[tokio::test]
async fn two_triggers_on_one_flow_are_two_doors() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = "\
id: fx-flow
namespace: fx.hooks
tasks:
  - id: fx-task
    type: core.log
    message: hello
triggers:
  - id: door-one
    type: io.kestra.plugin.core.trigger.Webhook
    key: key-one
  - id: door-two
    type: io.kestra.plugin.core.trigger.Webhook
    key: key-two
";
    common::seed_flow(&db, ws, user, flow, "draft").await;
    let app = common::webhook_app(&db).await;

    for (key, expect) in [("key-one", "door-one"), ("key-two", "door-two")] {
        let res = common::anon(
            &app,
            "POST",
            &hook_url(ws, key),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&common::body_text(res).await).unwrap();
        assert_eq!(
            body["trigger"]["id"],
            json!(expect),
            "{key} must open {expect} and nothing else"
        );
    }
    assert_eq!(common::execution_count(&db, ws).await, 2);
}

/// A trigger whose key cannot render hides itself without hiding the ones
/// after it — the artifact's behaviour, and the reason the fixture puts the
/// broken one FIRST.
#[tokio::test]
async fn an_unrenderable_key_does_not_hide_the_triggers_after_it() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = "\
id: fx-flow
namespace: fx.hooks
tasks:
  - id: fx-task
    type: core.log
    message: hello
triggers:
  - id: broken
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"{{ nosuchroot.nothing }}\"
  - id: reachable
    type: io.kestra.plugin.core.trigger.Webhook
    key: reachable-key
";
    common::seed_flow(&db, ws, user, flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "reachable-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&common::body_text(res).await).unwrap();
    assert_eq!(body["trigger"]["id"], json!("reachable"));

    // And the broken one is not reachable by its own source text.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "%7B%7B%20nosuchroot.nothing%20%7D%7D"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// `inputs:` that do NOT read `trigger.*` are rendered and stored on the
/// execution.
#[tokio::test]
async fn renderable_trigger_inputs_reach_the_execution() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = "\
id: fx-flow
namespace: fx.hooks
inputs:
  - id: who
    type: STRING
tasks:
  - id: fx-task
    type: core.log
    message: hello
triggers:
  - id: fx-trigger
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-key
    inputs:
      who: \"from-{{ flow.namespace }}\"
";
    common::seed_flow(&db, ws, user, flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let inputs: serde_json::Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .expect("read inputs");
    assert_eq!(
        inputs,
        json!({"who": "from-fx.hooks"}),
        "the value is RENDERED, not the source text"
    );
}

/// An execution nobody triggered carries a NULL `execution_trigger` — the
/// column's own invariant, and the reason it is nullable rather than defaulted
/// to `{}`.
#[tokio::test]
async fn an_untriggered_execution_carries_no_trigger_record() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;
    let source =
        "id: plain\nnamespace: fx.plain\ntasks:\n  - id: t\n    type: core.log\n    message: x\n";
    egret_workflow::worker::start_execution(&db.pool, ws, None, source, json!({}))
        .await
        .expect("start");
    assert_eq!(
        common::only_execution_trigger(&db, ws).await,
        None,
        "no trigger means NULL, which `{{}}` could not have distinguished"
    );
}

/// The CHECK that makes a half-written trigger record unrepresentable.
///
/// A guard nobody has seen fail is a guard nobody has seen, so this drives it
/// directly — and includes the row that must be ACCEPTED, without which the
/// assertion is satisfied by a CHECK that rejects everything.
#[tokio::test]
async fn a_half_written_trigger_record_is_unrepresentable() {
    let Some(db) = common::db().await else { return };
    let (ws, _user) = common::seed_workspace(&db).await;

    let insert = |value: serde_json::Value| {
        let pool = db.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO workflow.executions \
                   (workspace_id, namespace, flow_key, source, state, history, execution_trigger) \
                 VALUES ($1, 'n', 'f', 's', 'CREATED', '[\"CREATED\"]'::jsonb, $2)",
            )
            .bind(ws)
            .bind(value)
            .execute(&pool)
            .await
        }
    };

    for (what, bad) in [
        ("no id", json!({"type": "t", "variables": {}})),
        ("no type", json!({"id": "i", "variables": {}})),
        ("no variables", json!({"id": "i", "type": "t"})),
        (
            "id is not a string",
            json!({"id": 1, "type": "t", "variables": {}}),
        ),
        (
            "variables is not an object",
            json!({"id": "i", "type": "t", "variables": []}),
        ),
        ("not an object at all", json!("i")),
    ] {
        let err = insert(bad)
            .await
            .expect_err(&format!("{what} was accepted"));
        assert!(
            err.to_string()
                .contains("workflow_executions_trigger_record_is_whole"),
            "{what} was refused by the wrong constraint: {err}"
        );
    }

    insert(json!({"id": "i", "type": "t", "variables": {}}))
        .await
        .expect("the whole record IS accepted");
}
