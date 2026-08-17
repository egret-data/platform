//! Engine-gated StarRocks connector test. Self-skip unless EGRET_TEST_STARROCKS_URL is set.
use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query::execution::model::Limits;
use egret_query::registry::model::{ConnOptions, Engine};
use egret_query_api::Param;
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
        bearer_token: None,
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

// F2-P4b: StarRocks OIDC-JWT auth via the patched mysql_async openid plugin.
// Gated on EGRET_TEST_STARROCKS_URL + EGRET_TEST_SR_JWT (a fresh Keycloak access token).
#[tokio::test]
async fn starrocks_jwt_auth_current_user() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    let Ok(jwt) = std::env::var("EGRET_TEST_SR_JWT") else {
        eprintln!("skip: no EGRET_TEST_SR_JWT");
        return;
    };
    let mut s = spec(&h, p);
    s.options.user = Some("tester".into());
    s.session_user = Some("tester".into());
    s.bearer_token = Some(jwt);
    let c = build_connector(&s);
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute(
            "SELECT current_user() AS u",
            &limits,
            CancellationToken::new(),
        )
        .await
        .expect("starrocks jwt auth + query");
    eprintln!("current_user = {:?}", r.rows[0][0]);
    assert_eq!(r.row_count, 1);
}

/// The rendered literal must be inert on a real StarRocks, not merely
/// inert-looking as a Rust string. Injected, `OR 1=1` would make the count 1.
#[tokio::test]
async fn starrocks_rendered_param_cannot_inject() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };

    let sql = template_render_starrocks(
        "SELECT count(*) AS n FROM (SELECT 'a' AS c) t WHERE t.c = {{c}}",
        r"\' OR 1=1 -- ",
    );

    let r = c
        .execute(&sql, &limits, CancellationToken::new())
        .await
        .expect("query must parse and run — a parse error means the literal broke out");

    // 'a' never equals the attacker string, so the only correct answer is 0.
    // A 1 here means `OR 1=1` executed.
    assert_eq!(r.rows.len(), 1, "count() returns exactly one row");
    assert_eq!(
        r.rows[0][0],
        serde_json::json!(0),
        "injected predicate took effect: {sql}"
    );
}

fn template_render_starrocks(sql: &str, value: &str) -> String {
    use egret_query::execution::model::QueryParam;
    use egret_query::execution::template::render;
    use egret_query::registry::model::Engine;
    render(
        Engine::StarRocks,
        sql,
        &[QueryParam {
            name: "c".into(),
            ty: "string".into(),
            value: value.into(),
        }],
    )
    .expect("render must succeed")
}

// ---------------------------------------------------------------------------
// SEV-0 regression: no second statement, ever.
// ---------------------------------------------------------------------------

/// A throwaway database this test owns outright, so the canary table it drops
/// (or fails to drop) can never be someone else's data.
const PROBE_DB: &str = "egret_p3fix_probe";

/// Open a raw admin connection — deliberately NOT through `StarRocksConnector`,
/// because the setup/teardown here has to run DDL that the connector's whole
/// purpose is to make impossible.
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

/// Create `PROBE_DB.<name>` from scratch. Returns the fully-qualified name.
///
/// `name` is per-test, not a shared constant: cargo runs these tests on separate
/// threads against one engine, and a shared canary makes each test's teardown
/// race the other's setup. That showed up as a flaky `table already exists`,
/// which is a test defect that would otherwise get re-read as a product one.
async fn reset_canary(host: &str, port: i32, name: &str) -> String {
    use mysql_async::prelude::Queryable;
    let mut c = admin_conn(host, port).await;
    for stmt in [
        format!("CREATE DATABASE IF NOT EXISTS {PROBE_DB}"),
        format!("DROP TABLE IF EXISTS {PROBE_DB}.{name}"),
        format!(
            "CREATE TABLE {PROBE_DB}.{name} (id INT) DUPLICATE KEY(id) \
             DISTRIBUTED BY HASH(id) BUCKETS 1 PROPERTIES(\"replication_num\"=\"1\")"
        ),
    ] {
        c.query_drop(&stmt)
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
    }
    let _ = c.disconnect().await;
    format!("{PROBE_DB}.{name}")
}

/// `true` iff the canary table is still there — the only assertion in this file
/// that is a measurement of the ENGINE rather than of a Rust value.
async fn canary_exists(host: &str, port: i32, name: &str) -> bool {
    use mysql_async::prelude::Queryable;
    let mut c = admin_conn(host, port).await;
    let rows: Vec<String> = c
        .query(format!(
            "SELECT TABLE_NAME FROM information_schema.tables \
             WHERE TABLE_SCHEMA = '{PROBE_DB}' AND TABLE_NAME = '{name}'"
        ))
        .await
        .expect("information_schema probe");
    let _ = c.disconnect().await;
    !rows.is_empty()
}

/// Drop only THIS test's table. Dropping the shared database here is what made
/// the two tests race.
async fn drop_canary(host: &str, port: i32, name: &str) {
    use mysql_async::prelude::Queryable;
    let mut c = admin_conn(host, port).await;
    let _ = c
        .query_drop(format!("DROP TABLE IF EXISTS {PROBE_DB}.{name}"))
        .await;
    let _ = c.disconnect().await;
}

/// **The SEV-0 measurement.** `describe` wraps its argument as
/// `SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0`. A definition that closes
/// that wrapper itself — `#` is accepted and IGNORED by StarRocks, so `#)`
/// really does close the paren — turns the `;` that follows into a TOP-LEVEL
/// statement separator, and the trailing `SELECT * FROM (SELECT 1` re-opens a
/// subquery for the wrapper's own suffix to land in. Three valid statements,
/// the middle one a `DROP TABLE`.
///
/// This test FAILED before the fix — `describe` returned
/// `Ok([ColumnMeta { name: "a", ty: "tinyint" }])`, a perfectly clean one-column
/// answer, while the canary table was gone. That is the mutation proof, and it
/// is also why the bug was invisible: nothing in the response says a second
/// statement ran.
///
/// What stops it is [`reject_statement_separator`]'s byte test in
/// `connector/starrocks.rs`, applied to the RAW `sql` before the wrapper is
/// built. **It is not the capability flag** — see
/// [`starrocks_ignores_the_multi_statement_capability`] for why that idea was
/// measured and abandoned.
#[tokio::test]
async fn starrocks_describe_cannot_smuggle_a_second_statement() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    let name = "canary_describe";
    let canary = reset_canary(&h, p, name).await;
    assert!(
        canary_exists(&h, p, name).await,
        "setup: canary must exist first"
    );

    let payload = format!(
        "SELECT 1 AS a #) AS _egret_ds LIMIT 0 ; DROP TABLE {canary} ; SELECT * FROM (SELECT 1"
    );
    let c = build_connector(&spec(&h, p));
    let r = c.describe(&payload, Duration::from_secs(30)).await;
    eprintln!("describe(SEV-0 payload) -> {r:?}");
    assert!(
        r.is_err(),
        "the payload must be refused, not merely survived"
    );

    let survived = canary_exists(&h, p, name).await;

    // Not just the `#` trick: a bare two-statement submission is refused too, so
    // the guarantee does not rest on one lexer quirk.
    let plain = c
        .describe(
            &format!("SELECT 1 AS a; DROP TABLE {canary}"),
            Duration::from_secs(30),
        )
        .await;
    eprintln!("describe(plain two statements) -> {plain:?}");
    let survived_plain = canary_exists(&h, p, name).await;

    // The ordinary shape must still work, or the guard is a blanket ban.
    let ok = c
        .describe("SELECT 1 AS a, 'x' AS b", Duration::from_secs(30))
        .await;
    eprintln!("describe(ordinary) -> {ok:?}");

    // A TRAILING `;` passes the guard (it cannot separate anything) but still
    // fails on the engine, because `describe`'s wrapper puts it inside the
    // subquery. That is pre-existing behaviour, unchanged here, and pinned so
    // the guard is never blamed for it.
    let trailing = c.describe("SELECT 1 AS a;", Duration::from_secs(30)).await;
    eprintln!("describe(trailing `;`) -> {trailing:?}");
    let trailing_msg = format!("{:?}", trailing.as_ref().err());
    assert!(
        trailing_msg.contains("syntax error"),
        "a trailing `;` must fail as an ENGINE syntax error, not as the guard's \
         refusal — otherwise the guard has become a blanket ban: {trailing_msg}"
    );

    drop_canary(&h, p, name).await;
    assert!(
        survived,
        "the smuggled `DROP TABLE` executed: describe sent a multi-statement submission"
    );
    assert!(
        plain.is_err() && survived_plain,
        "plain `a; b` reached the engine"
    );
    let cols = ok.expect("a normal describe with a trailing `;` must still succeed");
    assert_eq!(cols.len(), 2, "columns still resolve: {cols:?}");
}

/// **The measurement that decided the shape of the fix, kept because it is the
/// only reason the byte test exists.**
///
/// The obvious fix was to stop negotiating `CLIENT_MULTI_STATEMENTS` in the
/// vendored `mysql_async` — MySQL documents that flag as the thing that permits
/// `a ; b` in one `COM_QUERY`. It was tried: the flag was cleared in
/// `Opts::get_capabilities`, and then `CLIENT_MULTI_RESULTS` as well. **Neither
/// changed anything on StarRocks 4.1.3.** The second statement still executed
/// and the table was still dropped, so the patch was reverted rather than
/// carried as protection that had been measured not to protect.
///
/// This test pins that behaviour through `execute`, which is deliberately NOT
/// guarded (SQL Lab is egret's arbitrary-SQL channel — a `query:run` holder can
/// send `DROP TABLE` as statement one, so batching is not an escalation there).
/// It therefore asserts CURRENT behaviour, not correct behaviour: if StarRocks
/// ever starts honouring the capability, this test fails and points at the
/// paragraph above, which would then be stale.
#[tokio::test]
async fn starrocks_ignores_the_multi_statement_capability() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: no EGRET_TEST_STARROCKS_URL");
        return;
    };
    let name = "canary_execute";
    let canary = reset_canary(&h, p, name).await;
    assert!(
        canary_exists(&h, p, name).await,
        "setup: canary must exist first"
    );

    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute(
            &format!("SELECT 1 AS a; DROP TABLE {canary}"),
            &limits,
            CancellationToken::new(),
        )
        .await;
    eprintln!("execute(two statements) -> {r:?}");

    let survived = canary_exists(&h, p, name).await;
    drop_canary(&h, p, name).await;
    assert!(
        !survived,
        "StarRocks now refuses batched statements — the byte test's justification \
         in `connector/starrocks.rs` needs re-measuring, and `execute` may be able \
         to rely on the capability flag after all"
    );
}

/// The execute path must report the same canonical type vocabulary as the
/// describe paths. Mirrors `describe.rs`'s
/// `starrocks_describe_returns_canonical_type_names` guard so BOTH paths carry
/// the guarantee: `drain_rows` used to build `ColumnMeta.ty` with
/// `format!("{:?}", c.column_type())`, which put raw protocol tags
/// (`MYSQL_TYPE_LONG`, `MYSQL_TYPE_VAR_STRING`, `MYSQL_TYPE_NEWDECIMAL`,
/// `MYSQL_TYPE_DATE`) in front of users instead of SQL type names.
///
/// The column set matches what was observed rendering wrongly in the browser:
/// an int, a varchar, a date, a decimal.
#[tokio::test]
async fn starrocks_execute_returns_canonical_type_names() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: EGRET_TEST_STARROCKS_URL not set");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute(
            "SELECT CAST(1 AS int) AS id, CAST('EU' AS varchar(8)) AS region, \
             CAST('2026-01-02' AS date) AS order_date, CAST(1.5 AS decimal(10,2)) AS amount",
            &limits,
            CancellationToken::new(),
        )
        .await
        .expect("execute failed");

    for col in &r.columns {
        assert!(
            !col.ty.starts_with("MYSQL_TYPE"),
            "column {:?} ty must be a SQL type name, not a debug protocol tag, got {:?}",
            col.name,
            col.ty
        );
    }

    let ty_of = |name: &str| {
        r.columns
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no column {name:?} in {:?}", r.columns))
            .ty
            .clone()
    };
    assert_eq!(ty_of("id"), "int");
    assert_eq!(ty_of("region"), "varchar");
    assert_eq!(ty_of("order_date"), "date");
    assert_eq!(ty_of("amount"), "decimal");
}

/// Same guarantee on the bound-parameter path. `execute` and
/// `execute_with_params` share `drain_rows`, but they reach it through
/// different protocols (text vs binary), and only a live check proves the
/// binary path's `columns_ref()` is typed the same way.
#[tokio::test]
async fn starrocks_execute_with_params_returns_canonical_type_names() {
    let Some((h, p)) = parse() else {
        eprintln!("skip: EGRET_TEST_STARROCKS_URL not set");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    let r = c
        .execute_with_params(
            "SELECT CAST(? AS int) AS id, CAST('EU' AS varchar(8)) AS region",
            &[Param::I64(7)],
            &limits,
            CancellationToken::new(),
        )
        .await
        .expect("execute_with_params failed");

    for col in &r.columns {
        assert!(
            !col.ty.starts_with("MYSQL_TYPE"),
            "column {:?} ty must be a SQL type name, not a debug protocol tag, got {:?}",
            col.name,
            col.ty
        );
    }
    assert_eq!(
        r.columns
            .iter()
            .find(|c| c.name == "id")
            .expect("no column 'id'")
            .ty,
        "int"
    );
}
