//! Engine-gated StarRocks connector test. Self-skip unless EGRET_TEST_STARROCKS_URL is set.
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn spec(host: &str, port: i32) -> ConnSpec {
    ConnSpec {
        engine: Engine::StarRocks,
        host: host.into(),
        port,
        options: ConnOptions {
            user: Some("root".into()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
    }
}

fn parse() -> Option<(String, i32)> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

#[tokio::test]
async fn starrocks_select_1() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
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
}

#[tokio::test]
async fn starrocks_row_cap_truncates() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 3,
        timeout: Duration::from_secs(30),
    };
    // generate_series may not exist on all StarRocks images; use a safe UNION ALL of 5 literals.
    let r = c
        .execute(
            "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5",
            &limits,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.row_count, 3);
    assert!(r.truncated);
}

#[tokio::test]
async fn starrocks_varchar_not_coerced_to_number() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    // A string literal that looks numeric must stay a string (leading zeros preserved).
    let r = c
        .execute(
            "SELECT CAST('007' AS VARCHAR) AS code",
            &limits,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.rows[0][0], serde_json::json!("007"));
    // An integer column must be a JSON number.
    let r2 = c
        .execute("SELECT 42 AS n", &limits, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r2.rows[0][0], serde_json::json!(42));
}
