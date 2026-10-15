//! View governance B1: the lineage EXPLAIN, against a stub Trino coordinator.
//!
//! What goes over the socket is the claim: the session catalog and schema the
//! view's unqualified names resolve against, the principal, and the statement
//! itself. A refused statement must not reach the coordinator at all.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;

use egret_query::execution::connector::{run_lineage_explain, ConnSpec, EXPLAIN_IO_PREFIX};
use egret_query::registry::model::{ConnOptions, Engine};

#[derive(Debug, Clone)]
struct Seen {
    user: Option<String>,
    catalog: Option<String>,
    schema: Option<String>,
    body: String,
}

type Recording = Arc<Mutex<Vec<Seen>>>;

/// One varchar column holding the plan, as Trino answers an EXPLAIN.
const PLAN_BODY: &str = r#"{"columns":[{"name":"Query Plan","type":"varchar"}],"data":[["{\"inputTableColumnInfos\":[],\"estimate\":{}}"]],"stats":{}}"#;

async fn statement(
    State(seen): State<Recording>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let get = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    seen.lock().unwrap().push(Seen {
        user: get("x-trino-user"),
        catalog: get("x-trino-catalog"),
        schema: get("x-trino-schema"),
        body,
    });
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        PLAN_BODY,
    )
}

async fn start() -> (u16, Recording) {
    let seen: Recording = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/statement", post(statement))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, seen)
}

/// No bearer: a bearer over the stub's plain HTTP is refused by the transport
/// guard, which is a different test's subject.
fn spec(port: u16, engine: Engine) -> ConnSpec {
    ConnSpec {
        engine,
        host: "127.0.0.1".into(),
        port: port as i32,
        options: ConnOptions::default(),
        secret: None,
        session_user: Some("egret-lineage".into()),
        bearer_token: None,
    }
}

#[tokio::test]
async fn the_explain_carries_the_views_default_catalog_and_schema() {
    let (port, seen) = start().await;
    let sql = format!("{EXPLAIN_IO_PREFIX}SELECT id, ssn FROM pii");

    let got = run_lineage_explain(
        &spec(port, Engine::Trino),
        "ice",
        "s",
        &sql,
        Duration::from_secs(10),
    )
    .await
    .expect("explain");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].catalog.as_deref(), Some("ice"));
    assert_eq!(seen[0].schema.as_deref(), Some("s"));
    assert_eq!(seen[0].body, sql);
    // Without a bearer and without `impersonate`, X-Trino-User is the
    // connection's service user; with the lineage runner's bearer it is the
    // principal (asserted in the connector's unit tests).
    assert!(seen[0].user.is_some());
    assert_eq!(got.rows.len(), 1);
    assert_eq!(
        got.rows[0][0],
        serde_json::json!("{\"inputTableColumnInfos\":[],\"estimate\":{}}")
    );
}

#[tokio::test]
async fn anything_but_the_explain_shape_never_reaches_the_engine() {
    let (port, seen) = start().await;
    for sql in [
        "SELECT 1",
        "EXPLAIN ANALYZE SELECT 1",
        "explain (type io, format json) select 1",
        "",
    ] {
        let err = run_lineage_explain(
            &spec(port, Engine::Trino),
            "ice",
            "s",
            sql,
            Duration::from_secs(10),
        )
        .await
        .expect_err(sql);
        assert!(err.to_string().contains("EXPLAIN (TYPE IO"), "{sql}: {err}");
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn starrocks_is_refused() {
    let (port, seen) = start().await;
    let sql = format!("{EXPLAIN_IO_PREFIX}SELECT 1");
    run_lineage_explain(
        &spec(port, Engine::StarRocks),
        "ice",
        "s",
        &sql,
        Duration::from_secs(10),
    )
    .await
    .expect_err("starrocks");
    assert!(seen.lock().unwrap().is_empty());
}
