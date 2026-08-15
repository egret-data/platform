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
    let existed_before = canary_db_exists(&h, p).await;

    let payload = format!("'; DROP DATABASE {CANARY_DB}; --");
    let c = build_connector(&spec(&h, p));
    let result = c
        .execute_with_params(
            "SELECT ? v",
            &[Param::Str(payload.clone())],
            &limits(),
            CancellationToken::new(),
        )
        .await;

    let survived = canary_db_exists(&h, p).await;

    // Teardown BEFORE any assertion below can panic — this is a shared dev
    // engine, and a failing assert leaking `CANARY_DB` behind is its own bug.
    let mut admin = admin_conn(&h, p).await;
    let _ = admin
        .query_drop(format!("DROP DATABASE IF EXISTS {CANARY_DB}"))
        .await;
    let _ = admin.disconnect().await;

    assert!(existed_before, "setup: canary database must exist first");
    let out = result.expect("a bound value, however hostile-looking, must round-trip as data");
    assert_eq!(
        out.rows[0][0],
        serde_json::json!(payload),
        "the bound value must come back byte-for-byte, not truncated at the quote \
         (this also proves the exact length: a truncated or SQL-interpreted value \
         could not equal the full payload string)"
    );
    assert!(
        survived,
        "the bound `DROP DATABASE` payload executed — parameters were interpolated, not bound"
    );
}

/// StarRocks' binary-protocol TIME payload does not decode as a standard
/// MySQL TIME value — measured live: `mysql_common`'s own `Debug` formatter
/// for `Value::Time` panics with "attempt to multiply with overflow" on
/// `CAST('10:20:30' AS TIME)` bound through `exec_iter`, because the day/hour
/// fields it reads back are garbage, not merely imprecise. There is no
/// faithful decode to fall back to, so `execute_with_params` must refuse a
/// TIME column with a clear error rather than return a plausible-looking
/// wrong string (or risk the same overflow reaching production).
///
/// The text protocol (`execute`, no bound params) is pinned alongside as the
/// control: it is unaffected, because it decodes TIME as `Value::Bytes`.
#[tokio::test]
async fn bound_query_returning_time_is_refused_not_garbled() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    let c = build_connector(&spec(&h, p));

    let err = c
        .execute_with_params(
            "SELECT ? x, CAST('10:20:30' AS TIME) t",
            &[Param::I64(1)],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect_err("a TIME column on the binary protocol must be refused, not decoded wrong");
    let msg = err.to_string();
    assert!(
        msg.to_ascii_uppercase().contains("TIME"),
        "the refusal must name the problem: {msg}"
    );

    // Control: the text protocol must still return TIME correctly — this is
    // not a blanket TIME failure, only the binary/bound-parameter path.
    let plain = c
        .execute(
            "SELECT CAST('10:20:30' AS TIME) t",
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("text-protocol TIME must still work");
    assert_eq!(plain.rows[0][0], serde_json::json!("10:20:30"));
}

/// Regression guard for a *reported* (not independently reproduced — see the
/// task report) precision loss: binding `F64` as an 8-byte binary
/// `Value::Double` was claimed to turn `1234567.891` into `1234567.875` —
/// which happens to be exactly the value you get by rounding `1234567.891`
/// down to `f32`. Three direct measurements against this live engine through
/// this connector's own `exec_iter` path (a bare `CAST(? AS DOUBLE)`
/// projection, an equality against the same literal, and a genuine backend
/// table-scan `WHERE v = ?` filter — the last one specifically to rule out
/// constant-folding in the FE analyzer masking a backend rounding bug) all
/// round-tripped exactly even with the old `Value::Double` binding, so this
/// test cannot demonstrate the claimed failure by reverting `param_to_value`.
/// The text-routed fix is kept anyway: it is zero-cost (JSON cannot express
/// NaN/Infinity, so there is no value this can fail to round-trip) and
/// consistent with `Decimal`'s already-established approach. This test
/// stands as a live regression guard for the fixed behavior, exercised
/// through the exact `WHERE price = ?` shape the report's failure scenario
/// named.
#[tokio::test]
async fn bound_f64_round_trips_exactly_through_a_backend_table_filter() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    use mysql_async::prelude::Queryable;
    const F64_DB: &str = "egret_p45_params_f64probe";
    let mut admin = admin_conn(&h, p).await;
    admin
        .query_drop(format!("CREATE DATABASE IF NOT EXISTS {F64_DB}"))
        .await
        .expect("create f64 probe database");
    admin
        .query_drop(format!(
            "CREATE TABLE {F64_DB}.t (id INT, v DOUBLE) DUPLICATE KEY(id) \
             DISTRIBUTED BY HASH(id) BUCKETS 1 PROPERTIES(\"replication_num\"=\"1\")"
        ))
        .await
        .expect("create f64 probe table");
    admin
        .query_drop(format!("INSERT INTO {F64_DB}.t VALUES (1, 1234567.891)"))
        .await
        .expect("seed f64 probe row");
    let _ = admin.disconnect().await;

    let c = build_connector(&spec(&h, p));
    let result = c
        .execute_with_params(
            &format!("SELECT v FROM {F64_DB}.t WHERE v = ?"),
            &[Param::F64(1234567.891)],
            &limits(),
            CancellationToken::new(),
        )
        .await;

    // Teardown before any assertion that could panic.
    let mut admin = admin_conn(&h, p).await;
    let _ = admin
        .query_drop(format!("DROP DATABASE IF EXISTS {F64_DB}"))
        .await;
    let _ = admin.disconnect().await;

    let out = result.expect("bind f64 as a table filter");
    assert_eq!(
        out.rows.len(),
        1,
        "WHERE v = ? must find the seeded row: a precision-lossy bind would \
         silently return zero rows here — exactly the `... WHERE price = ?` \
         failure scenario this guards against"
    );
    assert_eq!(out.rows[0][0], serde_json::json!(1234567.891));
}
