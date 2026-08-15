//! Engine-gated StarRocks parameter-binding test. Self-skips (like
//! `connector_starrocks.rs`) unless `EGRET_TEST_STARROCKS_URL` is set — in
//! that case it reports `ok` while asserting nothing, so the test's DURATION
//! is what distinguishes a real pass from a self-skip.

use egret_query::execution::connector::build_connector;
use egret_query::execution::model::Limits;
use egret_query::registry::model::{ConnOptions, Engine};
use egret_query_api::Param;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn spec(host: &str, port: i32) -> egret_query::execution::connector::ConnSpec {
    egret_query::execution::connector::ConnSpec {
        engine: Engine::StarRocks,
        host: host.into(),
        port,
        options: ConnOptions {
            user: Some("root".into()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
        bearer_token: None,
    }
}

fn parse() -> Option<(String, i32)> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

fn limits() -> Limits {
    Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    }
}

/// This is §9's headline criterion. `"007"` catches a string being coerced to
/// a number; the backslash and quote are the P1 injection vector; `NULL` and
/// the decimal catch type collapse; the date is the binary-protocol decode
/// regression this task must not ship.
#[tokio::test]
async fn bound_values_round_trip_exactly_on_starrocks() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    let c = build_connector(&spec(&h, p));

    let out = c
        .execute_with_params(
            "SELECT CAST(? AS DATE) d, CAST(? AS DECIMAL(10,4)) m, ? s, ? n, ? bs, ? q",
            &[
                Param::Date("2020-02-29".into()),
                Param::Decimal("12345.6789".into()),
                Param::Str("007".into()),
                Param::Null,
                Param::Str(r"a\' OR 1=1 -- ".into()),
                Param::Str("it's".into()),
            ],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("bind");

    assert_eq!(
        serde_json::to_string(&out.rows[0]).unwrap(),
        r#"["2020-02-29","12345.6789","007",null,"a\\' OR 1=1 -- ","it's"]"#,
        "exact JSON: a DATE arriving as Value::Date must not serialize as its Debug repr"
    );
}

/// A throwaway database this test owns outright, distinct from
/// `connector_starrocks.rs`'s `egret_p3fix_probe` so the two files' teardowns
/// can never race each other.
const CANARY_DB: &str = "egret_p45_params_probe";

async fn admin_conn(host: &str, port: i32) -> mysql_async::Conn {
    let opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(host.to_string())
        .tcp_port(u16::try_from(port).unwrap_or(9030))
        .user(Some("root".to_string()))
        .prefer_socket(false);
    mysql_async::Conn::new(mysql_async::Opts::from(opts))
        .await
        .expect("admin connection to StarRocks")
}

async fn canary_db_exists(host: &str, port: i32) -> bool {
    use mysql_async::prelude::Queryable;
    let mut c = admin_conn(host, port).await;
    let rows: Vec<String> = c
        .query(format!(
            "SELECT SCHEMA_NAME FROM information_schema.schemata \
             WHERE SCHEMA_NAME = '{CANARY_DB}'"
        ))
        .await
        .expect("information_schema probe");
    let _ = c.disconnect().await;
    !rows.is_empty()
}

/// A hostile bound value — a would-be `DROP DATABASE` — must come back as
/// inert data of its exact length, never as an executed statement. Proven on
/// the real engine: a canary database is created first, bound as a *value*
/// (not interpolated SQL) through `execute_with_params`, and checked to still
/// exist afterward.
#[tokio::test]
async fn hostile_bound_value_is_inert_data_and_canary_database_survives() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    use mysql_async::prelude::Queryable;
    let mut admin = admin_conn(&h, p).await;
    admin
        .query_drop(format!("CREATE DATABASE IF NOT EXISTS {CANARY_DB}"))
        .await
        .expect("create canary database");
    let _ = admin.disconnect().await;
    assert!(
        canary_db_exists(&h, p).await,
        "setup: canary database must exist first"
    );

    let payload = format!("'; DROP DATABASE {CANARY_DB}; --");
    let c = build_connector(&spec(&h, p));
    let out = c
        .execute_with_params(
            "SELECT ? v",
            &[Param::Str(payload.clone())],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("a bound value, however hostile-looking, must round-trip as data");

    assert_eq!(
        out.rows[0][0],
        serde_json::json!(payload),
        "the bound value must come back byte-for-byte, not truncated at the quote"
    );
    assert_eq!(
        out.rows[0][0].as_str().unwrap().len(),
        payload.len(),
        "exact length: no part of the payload was interpreted as SQL"
    );

    assert!(
        canary_db_exists(&h, p).await,
        "the bound `DROP DATABASE` payload executed — parameters were interpolated, not bound"
    );

    let mut admin = admin_conn(&h, p).await;
    let _ = admin
        .query_drop(format!("DROP DATABASE IF EXISTS {CANARY_DB}"))
        .await;
    let _ = admin.disconnect().await;
}
