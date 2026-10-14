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

/// A namespace no other test in this binary uses.
///
/// **Fix round 1, Important 1.** Every test used the literal `fx.hooks`, and
/// all of them share one per-process database. Under a mutation that removes
/// the `workspace_id` predicate the flow lookup becomes global, so one test's
/// fixture answers another's request — and the sixteen failures that produced
/// were **fixture collision, not the guard**. A suite whose failures have the
/// wrong cause cannot be read; the tests that genuinely cover a property have
/// to be the ones that fail.
///
/// Each test calls this once and threads the result through, so the namespaces
/// are distinct between tests and identical within one.
fn fresh_namespace() -> String {
    format!("fx.{}", Uuid::new_v4().simple())
}

/// The shared fixture. Four pairwise-distinct strings — the namespace (unique
/// per test), the flow id `fx-flow`, the trigger id `fx-trigger` and the key
/// `fx-key` — so a handler that read the wrong one of the four cannot pass by
/// coincidence.
///
/// `marker` goes in the task's `message:`, which is what makes *which source
/// ran* observable: two workspaces can then hold flows that agree on every
/// column the lookup touches and still be told apart afterwards.
fn hook_flow_in(ns: &str, marker: &str) -> String {
    format!(
        "\
id: fx-flow
namespace: {ns}
tasks:
  - id: fx-task
    type: core.log
    message: {marker}
triggers:
  - id: fx-trigger
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-key
"
    )
}

fn hook_flow(ns: &str) -> String {
    hook_flow_in(ns, "hello")
}

fn url(ws: Uuid, ns: &str, flow: &str, key: &str) -> String {
    format!("/workflows/v1/{ws}/executions/webhook/{ns}/{flow}/{key}")
}

fn hook_url(ws: Uuid, ns: &str, key: &str) -> String {
    url(ws, ns, "fx-flow", key)
}

/// The `source` of the one execution in `ws` — i.e. **which flow actually
/// ran**, which is the fact a cross-workspace resolve gets wrong and which an
/// execution count cannot see (the execution row carries the path's workspace
/// either way).
async fn only_execution_source(db: &egret_metastore::Db, ws: Uuid) -> String {
    sqlx::query_scalar("SELECT source FROM workflow.executions WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .expect("read execution source")
}

/// The `flow_id` FK of the one execution in `ws`.
async fn only_execution_flow_id(db: &egret_metastore::Db, ws: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT flow_id FROM workflow.executions WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .expect("read execution flow_id")
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "wrong-key"),
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
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &format!("/workflows/v1/{ws}/executions/webhook/{ns}/fx-flow"),
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
        &hook_url(ws, &ns, ""),
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
    let ns = fresh_namespace();

    let flow_a = hook_flow(&ns).replace("key: fx-key", "key: key-of-a");
    let flow_b = hook_flow(&ns).replace("key: fx-key", "key: key-of-b");
    common::seed_flow(&db, ws_a, user_a, &flow_a, "draft").await;
    common::seed_flow(&db, ws_b, user_b, &flow_b, "draft").await;

    let app = common::webhook_app(&db).await;

    // B's key at A's URL.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, &ns, "key-of-b"),
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
        &hook_url(ws_b, &ns, "key-of-a"),
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
        &hook_url(ws_a, &ns, "key-of-a"),
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
        &hook_url(ws_b, &ns, "key-of-b"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_a).await, 1);
    assert_eq!(common::execution_count(&db, ws_b).await, 1);
}

/// The harder half of the same class: two workspaces holding flows that agree
/// on **every column the lookup touches**, key included.
///
/// # This test's docstring used to be false, and that is the lesson
///
/// It claimed that the previous test "can be passed by a handler that resolves
/// without a workspace predicate" while this one "cannot". Fix round 1,
/// Important 1 measured it: run **alone**, this test **passed under M6** (the
/// mutation that deletes `workspace_id = $1`). The sixteen failures the file
/// produced under M6 were *fixture collision* between tests sharing one
/// per-process database — the wrong cause entirely.
///
/// The reason it passed is worth writing down, because it is a whole class of
/// useless assertion: it asserted `execution_count(ws_a)` and
/// `execution_count(ws_b)`, and **the execution row carries the workspace from
/// the PATH either way**. A cross-workspace resolve does not move the execution;
/// it makes the execution run *someone else's source*. A count cannot see that.
///
/// So the two flows now differ in one observable — the task's `message:` — and
/// the assertions are on **which source ran** and **which `flows` row the
/// execution points at**. Namespaces are per-test (see [`fresh_namespace`]), so the only
/// thing that can select the right row is the predicate.
#[tokio::test]
async fn two_workspaces_holding_the_same_flow_and_the_same_key_do_not_cross() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();

    let flow_a =
        common::seed_flow(&db, ws_a, user_a, &hook_flow_in(&ns, "marker-a"), "draft").await;
    let flow_b =
        common::seed_flow(&db, ws_b, user_b, &hook_flow_in(&ns, "marker-b"), "draft").await;
    assert_ne!(flow_a, flow_b);

    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_a).await, 1);
    assert_eq!(common::execution_count(&db, ws_b).await, 0);

    // **The assertions that count.** Which source ran, and which row it came
    // from — neither of which a workspace-scoped count can answer.
    let ran = only_execution_source(&db, ws_a).await;
    assert!(
        ran.contains("marker-a"),
        "A's URL ran a source that is not A's: {ran}"
    );
    assert!(
        !ran.contains("marker-b"),
        "A's URL ran B's source -- this is the cross-workspace read: {ran}"
    );
    assert_eq!(
        only_execution_flow_id(&db, ws_a).await,
        Some(flow_a),
        "the execution must point at A's `flows` row"
    );

    // The symmetric direction, on the same pair.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_b, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let ran = only_execution_source(&db, ws_b).await;
    assert!(ran.contains("marker-b"), "B's URL ran: {ran}");
    assert_eq!(only_execution_flow_id(&db, ws_b).await, Some(flow_b));
}

/// A workspace that is not `active` answers the same 404 as one that does not
/// exist — a key minted before the workspace was archived must stop working.
#[tokio::test]
async fn an_archived_workspace_refuses() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    // The control FIRST, so that the refusal below is attributable to the
    // archive and to nothing else.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    let flow = common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .expect("soft delete");

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    // A second flow beside it, switched off at runtime: a caller without its
    // key must still see the same 404.
    let ns_off = fresh_namespace();
    let off = common::seed_flow(&db, ws, user, &hook_flow(&ns_off), "draft").await;
    switch_off(&db, ws, off, "fx-trigger").await;
    let app = common::webhook_app(&db).await;

    let mut seen = Vec::new();
    for (what, path) in [
        (
            "no such workspace",
            url(Uuid::new_v4(), &ns, "fx-flow", "fx-key"),
        ),
        (
            "no such namespace",
            url(ws, "fx.other", "fx-flow", "fx-key"),
        ),
        ("no such flow", url(ws, &ns, "fx-other", "fx-key")),
        ("no such key", hook_url(ws, &ns, "fx-other-key")),
        (
            "wrong key on a switched-off trigger",
            hook_url(ws, &ns_off, "fx-other-key"),
        ),
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

    // The controls: the responses they must all differ from. The right key on
    // the enabled flow fires; on the switched-off one it explains itself.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns_off, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    for (what, path) in [
        (
            "the trigger id used as the key",
            hook_url(ws, &ns, "fx-trigger"),
        ),
        ("the flow id used as the key", hook_url(ws, &ns, "fx-flow")),
        ("the namespace used as the key", hook_url(ws, &ns, &ns)),
        (
            "the key used as the flow id",
            url(ws, &ns, "fx-key", "fx-key"),
        ),
        (
            "namespace and flow swapped",
            url(ws, "fx-flow", &ns, "fx-key"),
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
    let ns = fresh_namespace();
    let flow = hook_flow(&ns).replace("key: fx-key", "key: \"  fx-spaced  \"");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    // The declared key trims, so the bare form matches.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-spaced"),
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
        &hook_url(ws, &ns, "%20%20fx-spaced%20%20"),
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
    let ns = fresh_namespace();
    let flow = hook_flow(&ns).replace("key: fx-key", "key: \"dyn-{{ flow.id }}\"");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    // The SOURCE text must not match.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "dyn-%7B%7B%20flow.id%20%7D%7D"),
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
        &hook_url(ws, &ns, "dyn-fx-flow"),
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    let cap = egret_workflow::MAX_WEBHOOK_BODY_BYTES;
    let over = vec![b'x'; cap + 1];
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    let flow = hook_flow(&ns).replace("    key: fx-key\n", "    key: fx-key\n    disabled: true\n");
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
        &hook_url(ws, &ns, "wrong"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Stage the runtime switch (spec 2026-10-14 §3.2): a row in
/// `workflow.disabled_triggers`, which a webhook has no `triggers` row to carry.
async fn switch_off(db: &egret_metastore::Db, ws: Uuid, flow: Uuid, trigger: &str) {
    sqlx::query(
        "INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(flow)
    .bind(trigger)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("switch off");
}

/// W7e Task 3: the runtime switch answers the YAML switch's 409, byte for byte,
/// starts nothing, and switching back on restores the 200.
#[tokio::test]
async fn a_switched_off_webhook_trigger_answers_409_like_the_yaml_one() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow = common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;

    // The reference: the same trigger, disabled in its YAML.
    let (ws_y, user_y) = common::seed_workspace(&db).await;
    let yaml = hook_flow(&ns).replace("    key: fx-key\n", "    key: fx-key\n    disabled: true\n");
    common::seed_flow(&db, ws_y, user_y, &yaml, "draft").await;

    let app = common::webhook_app(&db).await;
    let fire = |w: Uuid| {
        let (app, ns) = (&app, &ns);
        async move {
            let res = common::anon(
                app,
                "POST",
                &hook_url(w, ns, "fx-key"),
                Some("application/json"),
                b"{}",
            )
            .await;
            let status = res.status();
            (status, common::body_text(res).await)
        }
    };
    let (y_status, y_body) = fire(ws_y).await;
    assert_eq!(y_status, StatusCode::CONFLICT);

    switch_off(&db, ws, flow, "fx-trigger").await;
    let (status, body) = fire(ws).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, y_body, "the runtime 409 must read as the YAML one");
    assert_eq!(common::execution_count(&db, ws).await, 0);

    sqlx::query("DELETE FROM workflow.disabled_triggers WHERE flow_id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, _) = fire(ws).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// A switch in workspace A does not touch the byte-identical flow in B.
#[tokio::test]
async fn a_switched_off_trigger_does_not_reach_the_same_flow_in_another_workspace() {
    let Some(db) = common::db().await else { return };
    let (ws_a, user_a) = common::seed_workspace(&db).await;
    let (ws_b, user_b) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow_a = common::seed_flow(&db, ws_a, user_a, &hook_flow(&ns), "draft").await;
    common::seed_flow(&db, ws_b, user_b, &hook_flow(&ns), "draft").await;
    switch_off(&db, ws_a, flow_a, "fx-trigger").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_a, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(common::execution_count(&db, ws_a).await, 0);

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws_b, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws_b).await, 1);
}

/// A disabled flow is 409 for a key-holder, and the control is the same flow
/// enabled.
#[tokio::test]
async fn a_disabled_flow_is_refused_after_the_key_matches() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow = common::seed_flow(&db, ws, user, &hook_flow(&ns), "disabled").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
        &hook_url(ws, &ns, "wrong"),
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
        &hook_url(ws, &ns, "fx-key"),
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
///
/// **Six rows, not one**, and four of them are fix round 1's Important 3:
/// `returnOutputs`, `responseContentType` and `responseCode` were parsed into
/// `models::Webhook` and then silently dropped — an author could write them,
/// they validated, and they changed nothing. That is the `lateMaximumDelay`
/// shape this branch has now hit three times.
#[tokio::test]
async fn the_unsupported_declarations_are_refused_by_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let base = fresh_namespace();
    let app = common::webhook_app(&db).await;

    for (suffix, needle) in [
        ("    wait: true\n", "wait: true"),
        ("    returnOutputs: true\n", "returnOutputs: true"),
        (
            "    responseContentType: \"text/plain\"\n",
            "responseContentType",
        ),
        ("    responseCode: 202\n", "responseCode"),
        ("    fetchType: STORE\n", "fetchType: STORE"),
    ] {
        let ns = format!(
            "{base}.{}",
            needle.replace([':', ' ', '/'], "-").to_lowercase()
        );
        let flow =
            hook_flow(&ns).replace("    key: fx-key\n", &format!("    key: fx-key\n{suffix}"));
        common::seed_flow(&db, ws, user, &flow, "draft").await;
        let res = common::anon(
            &app,
            "POST",
            &url(ws, &ns, "fx-flow", "fx-key"),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(res.status(), StatusCode::CONFLICT, "{needle} was ACCEPTED");
        let body = common::body_text(res).await;
        assert!(
            body.contains(needle),
            "the refusal must name {needle}; got {body}"
        );
        assert!(body.contains("fx-trigger"), "and the trigger; got {body}");
    }

    // multipart is refused on the REQUEST rather than on the declaration.
    let multi = format!("{base}.multi");
    common::seed_flow(&db, ws, user, &hook_flow(&multi), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &url(ws, &multi, "fx-flow", "fx-key"),
        Some("multipart/form-data; boundary=b"),
        b"--b--",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(common::body_text(res).await.contains("multipart/form-data"));

    // The control: the SAME fixture with none of the six declared does fire.
    // Without it every row above is satisfied by a handler that refuses
    // everything.
    let plain = format!("{base}.plain");
    common::seed_flow(&db, ws, user, &hook_flow(&plain), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &url(ws, &plain, "fx-flow", "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "exactly the control -- not one of the six refusals started anything"
    );
}

/// `inputs:` that read `trigger.*` are refused by name rather than letting the
/// renderer blame the author's template for a root egret does not publish.
#[tokio::test]
async fn inputs_reading_the_trigger_root_are_refused_by_name() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow = "\
id: fx-flow
namespace: __NS__
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
"
    .replace("__NS__", &ns);
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = common::body_text(res).await;
    assert!(
        body.contains("`trigger` render root")
            && body.contains("input 'payload'")
            && body.contains("fx-trigger"),
        "the refusal must name the root, the input and the trigger; got {body}"
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &format!(
            "{}?validationToken=abc&validationToken=def&x=1",
            hook_url(ws, &ns, "fx-key")
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
    // W13a Task 3: every caller that is not the Flow-trigger processor starts
    // at the bottom of a chain -- no source link, depth 0, no outbox event.
    let chain: (Option<Uuid>, i32, Option<i64>) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_depth, flow_trigger_event_id \
           FROM workflow.executions WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("read the chain columns");
    assert_eq!(
        chain,
        (None, 0, None),
        "a webhook start is not a Flow-trigger fire"
    );

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
    assert_eq!(body["namespace"], json!(&ns));
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
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "GET",
        &format!("{}?q=7", hook_url(ws, &ns, "fx-key")),
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
        &hook_url(ws, &ns, "fx-key"),
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
    let ns = fresh_namespace();
    let flow = "\
id: fx-flow
namespace: __NS__
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
"
    .replace("__NS__", &ns);
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    for (key, expect) in [("key-one", "door-one"), ("key-two", "door-two")] {
        let res = common::anon(
            &app,
            "POST",
            &hook_url(ws, &ns, key),
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
    let ns = fresh_namespace();
    let flow = "\
id: fx-flow
namespace: __NS__
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
"
    .replace("__NS__", &ns);
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "reachable-key"),
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
        &hook_url(ws, &ns, "%7B%7B%20nosuchroot.nothing%20%7D%7D"),
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
    let ns = fresh_namespace();
    let flow = "\
id: fx-flow
namespace: __NS__
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
"
    .replace("__NS__", &ns);
    common::seed_flow(&db, ws, user, &flow, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
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
        json!({"who": format!("from-{ns}")}),
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
        // Fix round 1's sweep: the CHECK has three `jsonb_typeof` clauses and
        // only two of them had a row. A clause no row reaches is a clause that
        // could be deleted with the suite still green.
        (
            "type is not a string",
            json!({"id": "i", "type": 1, "variables": {}}),
        ),
        (
            "variables is not an object",
            json!({"id": "i", "type": "t", "variables": []}),
        ),
        (
            "variables is a non-empty array",
            json!({"id": "i", "type": "t", "variables": [1, 2]}),
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

// ===========================================================================
// Fix round 1 — the defects that shipped, and the decisions that arrived
// ===========================================================================

/// **Critical 1's regression test.** A flow whose YAML says `disabled: true`
/// must not fire.
///
/// egret has two ways a flow is switched off and the handler read only one of
/// them: the `flows.state` column, which the management UI writes, and the
/// YAML's own `disabled:`, which is Kestra's `Flow.isDisabled()` — the field
/// the artifact answers **409** for and the one `trigger::flow::evaluate`
/// already checked on the sibling seam. The first version of this route
/// answered **200** and created an execution.
///
/// Four rows, because the two switches are independent and the fixture has to
/// say which one is doing the work.
#[tokio::test]
async fn a_flow_disabled_in_its_yaml_does_not_fire() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let app = common::webhook_app(&db).await;

    let disabled_yaml = hook_flow(&ns).replace(
        &format!("namespace: {ns}"),
        &format!("namespace: {ns}\ndisabled: true"),
    );
    let flow = common::seed_flow(&db, ws, user, &disabled_yaml, "draft").await;

    // 1. YAML says disabled, column says draft -> 409, nothing started.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "a `disabled: true` flow FIRED -- this is Critical 1"
    );
    assert!(common::body_text(res).await.contains("flow is disabled"));
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // 2. Without the key it is still just a 404 -- the 409 is only ever shown
    //    to a caller that has proved it holds the credential.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "wrong"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 3. The control: the SAME flow with only that one line removed fires.
    //    Without it, row 1 is satisfied by a handler that refuses everything.
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(hook_flow(&ns))
        .execute(&db.pool)
        .await
        .expect("re-enable in the YAML");
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);

    // 4. And the OTHER switch still refuses on its own, so the fix did not
    //    replace one check with the other.
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(flow)
        .execute(&db.pool)
        .await
        .expect("disable the column");
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// A `draft: true` flow **does** fire — measured on the artifact (200), and
/// asserted here so that Critical 1's fix cannot quietly grow into refusing it.
///
/// This is the discriminating neighbour of the test above: `disabled` and
/// `draft` are adjacent boolean fields on the same struct and the wrong one is
/// an easy reach.
#[tokio::test]
async fn a_draft_flow_does_fire() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let yaml = hook_flow(&ns).replace(
        &format!("namespace: {ns}"),
        &format!("namespace: {ns}\ndraft: true"),
    );
    common::seed_flow(&db, ws, user, &yaml, "draft").await;
    let app = common::webhook_app(&db).await;

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the artifact answers 200 for a draft; refusing it would be under-acceptance \
         invented here"
    );
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// **Critical 2's regression test, end to end.**
///
/// `inputs: {x: "{{ trigger }}"}` answered **200** and fed the execution the
/// key-comparison context's private `{id, type}` object. Five spellings, and
/// the assertion is on the **stored `inputs` column** as well as the status —
/// a 200 is the visible half, but the leak is what reached the database.
#[tokio::test]
async fn no_input_spelling_can_reach_the_trigger_root_through_the_route() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let base = fresh_namespace();
    let app = common::webhook_app(&db).await;

    for (n, expr) in [
        "{{ trigger }}",
        "{{ trigger | json }}",
        "{{ trigger.body }}",
        "{{ trigger['body'] }}",
        "{% set t = trigger %}{{ t }}",
        // Fix round 2, Important 1: these five RENDERED against a
        // trigger-free context -- to `[null]`, `{}`, the fallback, the
        // fallback and `false` -- so closing the leak left them answering
        // **200** with a value the author never asked for.
        "{{ [trigger] }}",
        "{{ {'a': trigger} }}",
        "{{ trigger ?? 'fallback' }}",
        "{{ trigger | default('fallback') }}",
        "{{ trigger is defined }}",
    ]
    .into_iter()
    .enumerate()
    {
        let ns = format!("{base}.s{n}");
        let yaml = format!(
            "\
id: fx-flow
namespace: {ns}
inputs:
  - id: x
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
      x: \"{expr}\"
"
        );
        common::seed_flow(&db, ws, user, &yaml, "draft").await;
        let res = common::anon(
            &app,
            "POST",
            &url(ws, &ns, "fx-flow", "fx-key"),
            Some("application/json"),
            br#"{"hello":"world"}"#,
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::CONFLICT,
            "`{expr}` was ACCEPTED -- Critical 2 if it leaked, fix round 2's Important 1 if \
             it merely rendered to something the author did not ask for"
        );
    }

    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "not one spelling may reach an execution, and the stored `inputs` column is where \
         the leak landed"
    );

    // The control: an input that reads `flow.*` still works, on the same
    // route, so the five assertions above are not satisfied by a handler that
    // refuses every `inputs:`.
    let ok_ns = format!("{base}.ok");
    let yaml = format!(
        "\
id: fx-flow
namespace: {ok_ns}
inputs:
  - id: x
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
      x: \"from-{{{{ flow.id }}}}\"
"
    );
    common::seed_flow(&db, ws, user, &yaml, "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &url(ws, &ok_ns, "fx-flow", "fx-key"),
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
    assert_eq!(inputs, json!({"x": "from-fx-flow"}));
}

/// The attempt limiter refuses past its quota, **through the route**, and it
/// counts refusals — which is the whole point, because a key guess is a
/// refusal.
#[tokio::test]
async fn the_attempt_limiter_bounds_key_guessing_through_the_route() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    // Spend the whole window on WRONG keys.
    for i in 0..egret_workflow::WEBHOOK_ATTEMPTS_PER_WINDOW {
        let res = common::anon(
            &app,
            "POST",
            &hook_url(ws, &ns, &format!("guess-{i}")),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "guess {i}");
    }

    // The next one is refused as a rate limit, and so is the one carrying the
    // RIGHT key -- the budget is the target's, not the guess's.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "guess-more"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "refusals are counted, so a valid key does not escape the window"
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // A DIFFERENT target is untouched -- without this the assertions above are
    // satisfied by a limiter that stops the whole process.
    let other = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&other), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &other, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// **The decision `subflow.rs` deferred, asserted.** A webhook runs the DRAFT
/// row's `source`, so editing the draft changes what a live webhook URL does
/// with no publish step in between.
///
/// That is the cost of the decision, written down in two module docs; this is
/// the running program that says the cost is real. If someone makes the route
/// execute a `flow_versions` snapshot instead, this is what fails — and it
/// should, in the same change that revisits `worker::subflow::resolve`.
#[tokio::test]
async fn a_webhook_runs_the_current_draft_source() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow = common::seed_flow(&db, ws, user, &hook_flow_in(&ns, "before-edit"), "draft").await;
    let app = common::webhook_app(&db).await;

    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(hook_flow_in(&ns, "after-edit"))
        .execute(&db.pool)
        .await
        .expect("edit the draft");

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let ran = only_execution_source(&db, ws).await;
    assert!(
        ran.contains("after-edit") && !ran.contains("before-edit"),
        "the webhook ran a source that is not the current draft: {ran}"
    );
}

/// A flow whose stored `source` does not parse answers the same 404 as an
/// absent one — it has no triggers, so no key can match it, and telling a
/// caller "this exists but is broken" is the existence oracle again.
///
/// Fix round 1's sweep: the branch was in the handler from the first commit and
/// **no test reached it**.
#[tokio::test]
async fn an_unparseable_flow_answers_the_same_404() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    let flow = common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;
    let app = common::webhook_app(&db).await;

    // The control FIRST, so the refusal below is attributable to the edit.
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);

    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind("id: fx-flow\nnamespace: x\ntasks: [ this is not: yaml: at all")
        .execute(&db.pool)
        .await
        .expect("break the source");

    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = common::body_text(res).await;
    assert_eq!(
        body, r#"{"error":"not found: webhook"}"#,
        "and it must be the SAME body as every other absence, not a parse error"
    );
    assert_eq!(
        common::execution_count(&db, ws).await,
        1,
        "nothing new started"
    );
}

/// The bulkhead gates the **handler**, not only the semaphore.
///
/// Fix round 1's sweep, mutation M24: deleting
/// `guard.bulkhead.try_acquire()...?` from `handle` left every test in this
/// crate green, because the only bulkhead test drove the semaphore directly.
/// That is a claim surviving its implementation being removed.
///
/// A guard with **zero** permits makes the bound reachable without any
/// concurrency at all, which is what lets this be deterministic rather than a
/// race.
#[tokio::test]
async fn the_bulkhead_gates_the_handler() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &hook_flow(&ns), "draft").await;

    let closed = common::webhook_app_with_guard(
        &db,
        std::sync::Arc::new(egret_workflow::WebhookGuard::new(0)),
    )
    .await;
    let res = common::anon(
        &closed,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a bulkhead with no permits must refuse before the handler does any work"
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);

    // The control: the same request against a guard with one permit fires.
    // Without it this test passes against a route that refuses everything.
    let open = common::webhook_app_with_guard(
        &db,
        std::sync::Arc::new(egret_workflow::WebhookGuard::new(1)),
    )
    .await;
    let res = common::anon(
        &open,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 1);

    // And the permit COMES BACK: a second request through the same one-permit
    // router succeeds, so the refusal above is about permits in flight rather
    // than a permit spent for ever.
    let res = common::anon(
        &open,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(common::execution_count(&db, ws).await, 2);
}

/// The two input refusals carry **two different statuses**, and that split had
/// no test.
///
/// Fix round 1's sweep, mutation M29: collapsing `InvalidInput` into `Conflict`
/// in the handler left every test green, because only the 409 arm was ever
/// asserted. A capability gap egret owes (**409**) and an author's broken
/// template (**400**) are different facts and a client that retries on one must
/// not retry on the other.
#[tokio::test]
async fn a_broken_input_is_400_and_a_missing_root_is_409() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let base = fresh_namespace();
    let app = common::webhook_app(&db).await;

    for (suffix, expr, expect) in [
        (
            "broken",
            "{{ nosuchroot.nothing }}",
            StatusCode::BAD_REQUEST,
        ),
        ("root", "{{ trigger.body }}", StatusCode::CONFLICT),
    ] {
        let ns = format!("{base}.{suffix}");
        let yaml = format!(
            "\
id: fx-flow
namespace: {ns}
inputs:
  - id: x
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
      x: \"{expr}\"
"
        );
        common::seed_flow(&db, ws, user, &yaml, "draft").await;
        let res = common::anon(
            &app,
            "POST",
            &url(ws, &ns, "fx-flow", "fx-key"),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(res.status(), expect, "`{expr}` answered the wrong status");
    }
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// **Fix round 3, Importants 5 and 6, through HTTP.**
///
/// A `trigger` reference in a nested value, and one reached by indirection
/// through `vars`, both used to answer **200**. The first stored the template
/// text verbatim (the check was asked only for a top-level `Value::String`);
/// the second rendered to `"[null]"` (the check was asked of pass 1 while the
/// renderer is a fixpoint loop).
#[tokio::test]
async fn nested_and_indirect_trigger_references_are_refused_through_the_route() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let base = fresh_namespace();
    let app = common::webhook_app(&db).await;

    for (n, decl) in [
        // Important 5 -- nested, and neither rendered nor refused before.
        "      x: [\"{{ trigger }}\"]",
        "      x:\n        b: \"{{ trigger }}\"",
        "      x:\n        \"{{ trigger }}\": 1",
        // Important 6 -- indirection through `vars`, which is ordinary
        // authoring.
        "      x: \"{{ vars.tpl }}\"",
        "      x: [\"{{ vars.tpl }}\"]",
    ]
    .into_iter()
    .enumerate()
    {
        let ns = format!("{base}.n{n}");
        let yaml = format!(
            "\
id: fx-flow
namespace: {ns}
variables:
  tpl: \"{{{{ [trigger] }}}}\"
inputs:
  - id: x
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
{decl}
"
        );
        common::seed_flow(&db, ws, user, &yaml, "draft").await;
        let res = common::anon(
            &app,
            "POST",
            &url(ws, &ns, "fx-flow", "fx-key"),
            Some("application/json"),
            b"{}",
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::CONFLICT,
            "declaration {n} was ACCEPTED:\n{decl}"
        );
    }

    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "not one of the five started anything"
    );

    // The control: nested values and `vars` indirection that do NOT reach the
    // root are rendered and stored -- so the five refusals above are not
    // satisfied by a handler that refuses every nested input.
    let ok_ns = format!("{base}.ok");
    let yaml = format!(
        "\
id: fx-flow
namespace: {ok_ns}
variables:
  tpl: \"from-{{{{ flow.id }}}}\"
inputs:
  - id: x
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
      x: \"{{{{ vars.tpl }}}}\"
      nested:
        deep: [\"{{{{ flow.namespace }}}}\", 7]
"
    );
    common::seed_flow(&db, ws, user, &yaml, "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &url(ws, &ok_ns, "fx-flow", "fx-key"),
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
        json!({"x": "from-fx-flow", "nested": {"deep": [ok_ns, 7]}}),
        "nested strings render, non-strings pass through, and `vars` indirection \
         reaches its fixpoint"
    );
}

/// **The 2026-10-11 ruling, on the webhook.** A flow whose `Pause` declares
/// `behavior:` (one of the four properties egret does not run) is refused when
/// the webhook fires, with a 400 naming the property, and no execution is
/// created. Before the ruling the webhook answered 200 and the execution
/// stalled in `RUNNING` for ever. The control is the identical flow with a
/// plain `Pause`, which fires: without it a handler that refuses every
/// `Pause` would pass.
#[tokio::test]
async fn a_pause_egret_does_not_run_is_refused_and_starts_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let app = common::webhook_app(&db).await;
    let flow = |ns: &str, extra: &str| {
        format!(
            "\
id: fx-flow
namespace: {ns}
tasks:
  - id: gate
    type: io.kestra.plugin.core.flow.Pause
    pauseDuration: PT1S
{extra}triggers:
  - id: fx-trigger
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-key
"
        )
    };

    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &flow(&ns, "    behavior: RESUME\n"), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains("unsupported_pause_property") && body.contains("tasks.gate.behavior"),
        "{body}"
    );
    assert_eq!(
        common::execution_count(&db, ws).await,
        0,
        "a refused webhook must leave no execution behind"
    );

    // The control.
    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &flow(&ns, ""), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "a plain Pause fires");
    assert_eq!(common::execution_count(&db, ws).await, 1);
}

/// **The 2026-10-12 ruling, on the webhook.** A flow holding a `ForEach` is
/// refused when its webhook fires, with a 400 naming `unsupported_foreach`,
/// and no execution is created. The control is the same flow with `Loop`.
#[tokio::test]
async fn a_foreach_is_refused_and_starts_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let app = common::webhook_app(&db).await;
    let flow = |ns: &str, task: &str| {
        format!(
            "id: fx-flow\nnamespace: {ns}\ntasks:\n{task}triggers:\n  - id: fx-trigger\n    \
             type: io.kestra.plugin.core.trigger.Webhook\n    key: fx-key\n"
        )
    };

    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &flow(&ns, "  - id: fan\n    type: io.kestra.plugin.core.flow.ForEach\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n"), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::body_text(res).await;
    assert!(
        body.contains("unsupported_foreach") && body.contains("tasks.fan"),
        "{body}"
    );
    assert_eq!(common::execution_count(&db, ws).await, 0);

    let ns = fresh_namespace();
    common::seed_flow(&db, ws, user, &flow(&ns, "  - id: fan\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1, 2]\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n"), "draft").await;
    let res = common::anon(
        &app,
        "POST",
        &hook_url(ws, &ns, "fx-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "a Loop fires");
    // Roots only: the Loop's iterations are child executions.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.executions \
              WHERE workspace_id = $1 AND parent_execution_id IS NULL",
        )
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
}
