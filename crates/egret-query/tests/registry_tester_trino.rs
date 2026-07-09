//! Engine-gated Trino tester test. Self-skip unless EGRET_TEST_TRINO_URL is set.
//! EGRET_TEST_TRINO_URL example: http://localhost:8090 (dev compose maps Trino there).
use egret_query::registry::model::{ConnOptions, Engine};
use egret_query::registry::tester::test_connection;

#[tokio::test]
async fn trino_select_1_ok() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let scheme = url.scheme().to_string();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(8080) as i32;
    let opts = ConnOptions {
        user: Some("admin".into()),
        http_scheme: Some(scheme),
        ..Default::default()
    };
    let outcome = test_connection(Engine::Trino, &host, port, &opts, None).await;
    assert!(outcome.ok, "detail: {}", outcome.detail);
}
