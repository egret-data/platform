//! Engine-gated describe tests. Self-skip unless the relevant env var is set.
//! EGRET_TEST_TRINO_URL    e.g. http://localhost:8090
//! EGRET_TEST_STARROCKS_URL e.g. mysql://root@localhost:9030

use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use std::time::Duration;

// ── Trino ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn trino_describe_returns_typed_columns() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip: EGRET_TEST_TRINO_URL not set");
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
    };
    let conn = build_connector(&spec);
    let sql = "SELECT CAST(1 AS bigint) AS n, CAST('x' AS varchar) AS s";
    let cols = conn
        .describe(sql, Duration::from_secs(15))
        .await
        .expect("describe failed");

    assert_eq!(cols.len(), 2, "expected 2 columns, got {:?}", cols);
    let n_col = cols.iter().find(|c| c.name == "n").expect("no column 'n'");
    let s_col = cols.iter().find(|c| c.name == "s").expect("no column 's'");
    assert!(
        n_col.ty.to_lowercase().contains("bigint"),
        "expected n.ty to contain 'bigint', got {:?}",
        n_col.ty
    );
    assert!(
        s_col.ty.to_lowercase().contains("varchar"),
        "expected s.ty to contain 'varchar', got {:?}",
        s_col.ty
    );
}

// ── StarRocks ─────────────────────────────────────────────────────────────────

fn starrocks_parse() -> Option<(String, i32)> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

fn starrocks_spec(host: &str, port: i32) -> ConnSpec {
    ConnSpec {
        engine: Engine::StarRocks,
        host: host.into(),
        port,
        options: ConnOptions {
            user: Some("root".into()),
            ..Default::default()
        },
        secret: None,
    }
}

#[tokio::test]
async fn starrocks_describe_returns_canonical_type_names() {
    let Some((h, p)) = starrocks_parse() else {
        eprintln!("skip: EGRET_TEST_STARROCKS_URL not set");
        return;
    };
    let conn = build_connector(&starrocks_spec(&h, p));
    // SELECT 1 AS n returns MYSQL_TYPE_TINY or MYSQL_TYPE_LONG depending on the engine.
    let sql = "SELECT 1 AS n, 'x' AS s";
    let cols = conn
        .describe(sql, Duration::from_secs(15))
        .await
        .expect("describe failed");

    assert_eq!(cols.len(), 2, "expected 2 columns, got {:?}", cols);
    let n_col = cols.iter().find(|c| c.name == "n").expect("no column 'n'");
    assert!(
        !n_col.ty.starts_with("MYSQL_TYPE"),
        "column 'n' ty must be canonical (not a debug protocol tag), got {:?}",
        n_col.ty
    );
    assert!(
        ["tinyint", "int", "bigint", "smallint"].contains(&n_col.ty.as_str()),
        "column 'n' ty expected an integer canonical name, got {:?}",
        n_col.ty
    );
}
