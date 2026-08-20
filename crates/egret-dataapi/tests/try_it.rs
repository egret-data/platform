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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::{ColumnMeta, Error, TabularResult, UserId};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

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

/// The URL and `curl` line Try-it echoes are shown to a HUMAN, so they must name
/// a host that human can reach.
///
/// With `data_api.public_url` set, the echo follows it. It used to print the
/// issuer, which in-cluster is `http://egret.<ns>.svc.cluster.local:8080` — a
/// copyable reproduction that resolves nowhere outside the cluster.
///
/// The fixture gives the consumer base and the issuer DIFFERENT hosts on
/// purpose: equal (the key unset) an echo still built from `issuer` would print
/// the same string and pass. `the_token_never_reaches_the_caller` above pins the
/// unset case, byte for byte.
#[tokio::test]
async fn the_echoed_url_names_the_host_the_author_can_reach() {
    let Some(f) = fixture().await else { return };
    const CONSUMER: &str = "https://api.egret.example";
    let f = f.with_consumer_base_url(CONSUMER);
    let sa = f.create_service_account("echo-origin").await;

    let (status, body) = try_it(&f, sa).await;
    assert_eq!(status, 200, "body: {body}");

    let url = body["request"]["url"].as_str().expect("url");
    assert_eq!(url, format!("{CONSUMER}/data-api/v1/{}/probe", f.ws_slug));
    // The `curl` line is a second rendering of the same URL and has its own
    // format string, so it can drift from the field above on its own.
    let curl = body["request"]["curl"].as_str().expect("curl");
    assert!(
        curl.contains(&format!("'{url}'")),
        "the pasteable reproduction must carry the same reachable URL: {curl}"
    );
    assert!(
        !curl.contains("egret.test"),
        "no trace of the in-cluster issuer may reach the author: {curl}"
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

// ---------------------------------------------------------------------------
// The audit event cannot be lost to a response the trial could not read.
// ---------------------------------------------------------------------------

/// One row, one column, and a value far past the trial's 4 MiB read cap.
///
/// The column NAME matches the frozen `n` the fixture published, so `shape_rows`
/// sees no drift and the serving path answers a perfectly ordinary 200 — the
/// overflow happens in the management handler afterwards, which is exactly the
/// window under test. A published `limits.max_rows` may be a million rows, so
/// this is a reachable shape rather than a contrived one; a single huge value is
/// just the cheapest way to reach it.
struct HugePort;

fn col_n() -> ColumnMeta {
    ColumnMeta {
        name: "n".into(),
        ty: "integer".into(),
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for HugePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![col_n()])
    }

    async fn describe_as_with_params(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _param_count: usize,
    ) -> Result<Vec<ColumnMeta>, Error> {
        Ok(vec![col_n()])
    }

    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        unreachable!("the serving path calls execute_as_with_params")
    }

    async fn execute_as_with_params(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: UserId,
        _sql: &str,
        _params: &[egret_query_api::Param],
        _limits: egret_query_api::Limits,
    ) -> Result<TabularResult, Error> {
        Ok(TabularResult {
            columns: vec![col_n()],
            rows: vec![vec![serde_json::json!("x".repeat(5 * 1024 * 1024))]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        })
    }
}

/// **A trial that RAN must be audited, even when its response is too large to
/// read back.**
///
/// By the time the body is read the impersonation is a fact: the query has
/// already executed as the service account. An audit event emitted after that
/// read is an event a large result silently deletes — and `access_log` is not a
/// substitute, because the two trails answer different people's questions.
///
/// The mutation that fails this test: move `st.audit.emit(…)` back below
/// `axum::body::to_bytes` in `try_it::try_endpoint`.
///
/// Built on a CLONE of the fixture's state with two substitutions — an audit
/// sink whose receiver this test keeps (the shared fixture drops its own), and a
/// port that answers larger than the cap. Nothing in `serving_common` changes, so
/// no sibling suite inherits either.
#[tokio::test]
async fn a_trial_too_large_to_read_is_still_audited() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("huge").await;
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

    let (sink, mut rx) = egret_audit::AuditSink::new(64);
    let mut st = f.state.clone();
    st.audit = sink;
    st.query = Arc::new(HugePort);
    let mgmt = axum::Router::new()
        .nest("/api", egret_dataapi::management_router())
        .with_state(st);

    let res = mgmt
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/ws/{}/data-api/endpoints/{}/try",
                    f.ws_slug, f.endpoint_id
                ))
                .header("Cookie", format!("egret_session={}", f.session))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "service_account_id": sa, "params": {} }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "the trial must fail on the cap — otherwise this test proves nothing. \
         413, not 409: nothing conflicts, and 409 on this surface already means \
         `openapi_gen`'s naming collision"
    );

    // `emit` is a synchronous `try_send`, so the event is already queued.
    let ev = rx.try_recv().expect(
        "a trial that executed as a service account emitted no audit event: the impersonation \
         happened before the body was read, so the record must not depend on reading it",
    );
    match ev {
        egret_audit::AuditEvent::Mutation {
            action,
            resource_id,
            detail,
            ..
        } => {
            assert_eq!(action, "data_api_endpoint:try");
            assert_eq!(
                resource_id.as_deref(),
                Some(f.endpoint_id.to_string().as_str())
            );
            // The status the SERVING path returned — 200. The cap is this
            // route's limit, not the endpoint's answer, and recording the 409
            // here would misreport what the service account was actually able
            // to do.
            assert_eq!(detail["status"], serde_json::json!(200), "detail: {detail}");
            assert_eq!(
                detail["service_account_id"].as_str(),
                Some(sa.to_string().as_str())
            );
            assert!(!detail.to_string().contains("eyJ"));
        }
        other => panic!("unexpected audit event: {other:?}"),
    }
    assert!(
        rx.try_recv().is_err(),
        "exactly one event: a trial is one act"
    );
}
