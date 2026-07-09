//! Engine-gated StarRocks tester test. Self-skip unless EGRET_TEST_STARROCKS_URL is set.
//! Example: EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030
use egret_query::registry::model::{ConnOptions, Engine};
use egret_query::registry::tester::test_connection;

#[tokio::test]
async fn starrocks_select_1_ok() {
    let Some(base) = std::env::var("EGRET_TEST_STARROCKS_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(9030) as i32;
    let user = if url.username().is_empty() {
        "root".to_string()
    } else {
        url.username().to_string()
    };
    let secret = url.password().map(|p| p.to_string());
    let opts = ConnOptions {
        user: Some(user),
        ..Default::default()
    };
    let outcome = test_connection(Engine::StarRocks, &host, port, &opts, secret.as_deref()).await;
    assert!(outcome.ok, "detail: {}", outcome.detail);
}
