//! Engine-gated Trino connector test. Self-skip unless EGRET_TEST_TRINO_URL is set.
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn trino_select_1() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let spec = ConnSpec {
        engine: Engine::Trino,
        host: url.host_str().unwrap().to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some(url.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
    };
    let c = build_connector(&spec);
    let limits = Limits {
        max_rows: 100,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute("SELECT 1 AS one", &limits, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.row_count, 1);
    assert_eq!(r.columns.len(), 1);
    assert_eq!(r.rows[0][0], serde_json::json!(1));
    assert!(!r.truncated);
}

#[tokio::test]
async fn trino_row_cap_truncates() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let spec = ConnSpec {
        engine: Engine::Trino,
        host: url.host_str().unwrap().to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some(url.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
    };
    let c = build_connector(&spec);
    let limits = Limits {
        max_rows: 5,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute(
            "SELECT * FROM UNNEST(SEQUENCE(1,100)) AS t(n)",
            &limits,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.row_count, 5);
    assert!(r.truncated);
}
