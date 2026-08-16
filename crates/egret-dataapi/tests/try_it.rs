//! Data API P5 §3 — the Try-it proxy: running a published endpoint as a service
//! account, from the console.
//!
//! Every test here drives the REAL management route, which in turn drives the
//! REAL serving router in process. That is the whole point of the feature: a
//! Try-it that answers OK while a consumer gets 403 converts the one tool that
//! could have found the problem into the reason nobody looked. The load-bearing
//! test is [`a_trial_without_a_grant_gets_the_real_403`] — mutate
//! `serving::router::serve` to return any other status and it must go red.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::{fixture, Fixture};

/// `POST …/endpoints/{id}/try` for one service account, with no parameters.
async fn try_it(f: &Fixture, sa: uuid::Uuid) -> (axum::http::StatusCode, serde_json::Value) {
    f.mgmt_post(
        &format!(
            "/api/ws/{}/data-api/endpoints/{}/try",
            f.ws_slug, f.endpoint_id
        ),
        Some(serde_json::json!({ "service_account_id": sa, "params": {} })),
    )
    .await
}

#[tokio::test]
async fn a_trial_without_a_grant_gets_the_real_403() {
    let Some(f) = fixture().await else { return };
    // A real service account holding no invoke grant on this endpoint.
    let sa = f.create_service_account("ungranted").await;

    let (status, body) = try_it(&f, sa).await;

    assert_eq!(status, 200, "the trial itself succeeded; the CALL did not");
    assert_eq!(body["status"], 403, "body: {body}");
    // The serving surface's OWN envelope, verbatim: `{"error": {code, message,
    // request_id}}` (spec §7), not a shape this route invented. A trial that
    // repackaged the body would be showing the author something no consumer
    // ever receives.
    assert_eq!(
        body["body"]["error"]["code"], "not_granted",
        "Try-it must not bypass a single gate — this is the most common first failure"
    );
    assert!(
        body["body"]["error"]["request_id"].is_string(),
        "the trial must carry the serving call's own correlation id: {body}"
    );
}

#[tokio::test]
async fn a_trial_is_recorded_as_a_trial() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("granted").await;
    // Through the REAL grants route, so the trial's 200 depends on the same
    // `access_grants` row a consumer's call would.
    let (granted, gbody) = f
        .mgmt_post(
            &format!(
                "/api/ws/{}/data-api/endpoints/{}/grants",
                f.ws_slug, f.endpoint_id
            ),
            Some(serde_json::json!({ "service_account_id": sa })),
        )
        .await;
    assert_eq!(granted, 200, "grant failed: {gbody}");

    let (status, body) = try_it(&f, sa).await;
    assert_eq!(status, 200);
    assert_eq!(
        body["status"], 200,
        "a granted trial must serve rows: {body}"
    );

    let row: (Option<uuid::Uuid>, Option<uuid::Uuid>, Option<String>, i32) = sqlx::query_as(
        "SELECT actor_user_id, service_account_id, client_id, status_code \
         FROM dataapi.access_log WHERE workspace_id = $1 ORDER BY ts DESC LIMIT 1",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("access_log row");

    assert_eq!(
        row.0,
        Some(f.author),
        "a trial writes to the access log and must be identifiable as one"
    );
    assert_eq!(
        row.1,
        Some(sa),
        "the call ran AS the service account, not as the author"
    );
    assert_eq!(row.3, 200);
    // The reserved marker, not a real client id: no `dataapi.clients` row made
    // this call, and the log must not claim one did.
    assert!(
        row.2.as_deref().is_some_and(|c| c == format!("trial:{sa}")),
        "client_id must name the trial, not a credential: {:?}",
        row.2
    );
}

#[tokio::test]
async fn the_token_never_reaches_the_caller() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("echo").await;

    let (_, body) = try_it(&f, sa).await;
    let text = body.to_string();

    assert!(
        text.contains("Bearer \\u2022") || text.contains("Bearer •"),
        "the echo must show a masked header: {text}"
    );
    assert!(
        !text.contains("eyJ"),
        "no JWT may appear anywhere in the response body: {text}"
    );
    // The reproduction is copyable and carries a placeholder, never the token.
    assert!(
        body["request"]["curl"]
            .as_str()
            .expect("curl")
            .contains("$TOKEN"),
        "body: {body}"
    );
    assert_eq!(
        body["request"]["url"],
        format!("https://egret.test/data-api/v1/{}/probe", f.ws_slug)
    );
}

#[tokio::test]
async fn an_id_that_is_not_a_service_account_is_not_found() {
    let Some(f) = fixture().await else { return };
    // `f.admin` is a real principal and a `ws_admin` of this workspace, but NOT
    // a row in `dataapi.service_accounts` — the shape `scoped_account` must
    // refuse with 404 BEFORE anything is minted. Without it this route would
    // mint a `data-api` token for an arbitrary user id, which is the whole
    // credential system bypassed by one request body.
    let (status, _) = try_it(&f, f.admin).await;
    assert_eq!(status, 404);
}
