//! Live-engine test for `TrinoConnector::execute_with_params` (Data API P4.5,
//! Task 3). Self-skips (reporting `ok` while asserting nothing) unless
//! `EGRET_TEST_TRINO_URL` is set — see each test's `trino_or_skip` call and
//! the task report for the env vars actually used and the run's duration,
//! which is the only signal separating a real pass from a skip.
//!
//! Required:
//!   * `EGRET_TEST_TRINO_URL`   — e.g. `http://localhost:8090`.
//!
//! The dev coordinator runs `http-server.authentication.type=JWT` (see
//! `deploy/trino/etc/config.properties`), so an unauthenticated request gets
//! a flat 403 regardless of SQL — these tests need a real bearer token:
//!   * `EGRET_TEST_TRINO_TOKEN` — a Keycloak access token, e.g.
//!     `curl -s -X POST 'http://localhost:8081/realms/egret/protocol/openid-connect/token' \
//!        -d grant_type=password -d client_id=egret -d client_secret=egret-secret \
//!        -d username=tester -d password=pw -d scope=openid | jq -r .access_token`
//!   * `EGRET_TEST_TRINO_USER`  — must equal the token's `preferred_username`
//!     (default `tester`); the connector sends this as `X-Trino-User` and a
//!     mismatch is refused as impersonation.
//!
//! JWT is enforced only over TLS, and this connector fails closed (refuses to
//! send a bearer) over a transport it cannot prove is TLS. Two live paths:
//!   * `https://localhost:8453` with `EGRET_TEST_TRINO_CERT` pointing at the
//!     coordinator's PEM (pinned as `ConnOptions.tls_cert`) — the "real"
//!     path, blocked in this environment because `deploy/trino/etc/gen-certs.sh`
//!     emits a `CA:TRUE` cert that rustls refuses as a leaf
//!     (`CaUsedAsEndEntity`) — a pre-existing dev-script defect, not a defect
//!     in this connector; recorded in `.superpowers/sdd/2026-08-15-data-api-p3-endpoints/task-1-report.md`.
//!   * `http://localhost:8090` (`X-Forwarded-Proto: https`, which the
//!     process-forwarded coordinator trusts as a secure transport claim) plus
//!     the documented operator opt-out `EGRET_ENGINE_TLS_INSECURE=1`. This is
//!     the path these tests use, matching `port_describe.rs`'s proven
//!     invocation. `EGRET_ENGINE_TLS_INSECURE` gates only whether the bearer
//!     is SENT (see `insecure_bearer_reason`) — it changes nothing about how
//!     `execute_with_params` renders or transmits the prepared statement
//!     itself, so it does not weaken anything this file is testing.
//!
//! A self-skipped run proves nothing.

use egret_query::execution::connector::{build_connector, ConnSpec, Connector};
use egret_query::execution::model::Limits;
use egret_query::registry::model::{ConnOptions, Engine};
use egret_query_api::Param;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Build a live `TrinoConnector` from the env vars documented above, or
/// `None` to self-skip. `EGRET_ENGINE_TLS_INSECURE=1` must also be set by the
/// caller's shell — it is the documented operator opt-out this file's
/// plain-HTTP-with-forwarded-proto invocation relies on, and is deliberately
/// NOT set here, so a run that forgets it fails loudly (`Connect` error
/// naming the reason) rather than this file silently working around it.
fn trino_or_skip() -> Option<Box<dyn Connector>> {
    let base = std::env::var("EGRET_TEST_TRINO_URL").ok()?;
    let url = url::Url::parse(&base).expect("EGRET_TEST_TRINO_URL is a URL");
    let token = std::env::var("EGRET_TEST_TRINO_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let user = std::env::var("EGRET_TEST_TRINO_USER").unwrap_or_else(|_| "tester".into());
    let spec = ConnSpec {
        engine: Engine::Trino,
        host: url.host_str().expect("host").to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some(user.clone()),
            http_scheme: Some(url.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
        session_user: token.as_ref().map(|_| user),
        bearer_token: token,
    };
    Some(build_connector(&spec))
}

fn limits() -> Limits {
    Limits {
        max_rows: 100,
        timeout: Duration::from_secs(30),
    }
}

/// §9's headline criterion, Trino side: bind a date, a decimal, a zero-padded
/// string, a NULL, a backslash, a single quote, a bool, a > 2^53 integer, and
/// a float — assert the EXACT JSON returned. `"007"` catches string-to-number
/// coercion; the quote is the P1 injection vector; the backslash pins that
/// Trino does NOT treat it as an escape (so this connector must never double
/// it, unlike the StarRocks path); `NULL` and the decimal catch type
/// collapse; `9007199254740993` catches silent `f64` precision loss on the
/// integer path; `1.5` pins the `DOUBLE '…'` typed-literal choice.
#[tokio::test]
async fn bound_values_round_trip_exactly_on_trino() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    let out = c
        .execute_with_params(
            "SELECT ? d, ? m, ? s, ? n, ? bs, ? q, ? bo, ? i, ? fl",
            &[
                Param::Date("2020-02-29".into()),
                Param::Decimal("12345.6789".into()),
                Param::Str("007".into()),
                Param::Null,
                Param::Str(r"a\ OR 1=1 -- ".into()),
                Param::Str("it's".into()),
                Param::Bool(true),
                Param::I64(9_007_199_254_740_993),
                Param::F64(1.5),
            ],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("bind");

    assert_eq!(
        serde_json::to_string(&out.rows[0]).unwrap(),
        r#"["2020-02-29","12345.6789","007",null,"a\\ OR 1=1 -- ","it's",true,9007199254740993,1.5]"#,
        "exact JSON round-trip of all nine bound values"
    );

    // The row-JSON assertion above can NOT by itself prove a typed
    // `DATE '…'` literal round-tripped AS A DATE rather than as its own
    // literal text: Trino serializes both `date` and `decimal` columns as
    // plain JSON STRINGS (measured), so `SELECT '2020-02-29' d` and
    // `SELECT DATE '2020-02-29' d` produce byte-identical row JSON. The
    // column metadata is what actually distinguishes a typed literal from a
    // bare string, so that is what this asserts.
    let ty = |name: &str| {
        out.columns
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no column named {name}"))
            .ty
            .clone()
    };
    assert_eq!(ty("d"), "date", "DATE '…' must type as date, not varchar");
    assert_eq!(
        ty("m"),
        "decimal(9, 4)",
        "DECIMAL '…' must type as decimal, not varchar"
    );
    assert_eq!(ty("bo"), "boolean");
    assert_eq!(
        ty("i"),
        "bigint",
        "a bare integer literal must type as bigint, never a lossy double"
    );
    assert_eq!(
        ty("fl"),
        "double",
        "DOUBLE '…' must type as double, not decimal — a bare `1.5` would be decimal(2,1)"
    );
}

/// The trap this test exists to avoid: asserting only "no error" would pass
/// for a defective renderer too, because a well-formed injected SCALAR
/// EXPRESSION parses fine on Trino and never surfaces as a syntax error —
/// measured live: `USING 'a' || 'b'` returns `"ab"`, not an error. So this
/// asserts the result IS DATA, and is the EXACT hostile string, proving the
/// quote was doubled rather than left to close the literal early.
#[tokio::test]
async fn hostile_bound_value_round_trips_as_inert_data() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    let payload = "'; DROP TABLE zz --";
    let out = c
        .execute_with_params(
            "SELECT ? AS s",
            &[Param::Str(payload.into())],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("a bound value, however hostile-looking, must round-trip as data, not error");

    assert_eq!(
        out.rows[0][0],
        serde_json::json!(payload),
        "the hostile string must come back byte-for-byte as DATA"
    );
}

/// The parser boundary itself, live: a `USING` payload attempting a second
/// statement is refused — proving the injection surface really is bounded to
/// scalar expressions and not full SQL, the property the whole module doc
/// comment is written around. Not a defense THIS code provides (Trino's own
/// parser is what would refuse a genuinely raw second statement); what this
/// test pins is that a value shaped like an attempted breakout — an embedded
/// quote immediately followed by `; DROP TABLE …; --` — still comes back as
/// one inert string rather than desyncing the literal, a second, distinct
/// hostile payload from `hostile_bound_value_round_trips_as_inert_data`
/// above for defense in depth.
#[tokio::test]
async fn a_second_statement_attempt_in_a_bound_value_stays_inert() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    // execute_with_params always renders exactly one literal per Param and
    // joins with ", " — there is no code path that lets a Param's *value*
    // reach the SQL unquoted, so this test drives the boundary the way a
    // hostile caller actually could: a Str value containing SQL-shaped text,
    // which `render_literal` must still quote as an inert string.
    let out = c
        .execute_with_params(
            "SELECT ? AS s",
            &[Param::Str("a'; DROP TABLE zz; --".into())],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("still just a quoted string value, not a second statement");
    assert_eq!(
        out.rows[0][0],
        serde_json::json!("a'; DROP TABLE zz; --"),
        "the quote must be doubled, not left to close the literal and open a new statement"
    );
}

/// Fix round 1, Important 1. The mandatory mutation on
/// `hostile_bound_value_round_trips_as_inert_data` and
/// `a_second_statement_attempt_in_a_bound_value_stays_inert` only ever
/// produced a Trino `SYNTAX_ERROR` — the failure mode the brief explicitly
/// says does NOT matter, because "the call returned `Err`" would already
/// catch it. The trap named in the brief is the opposite: a well-formed
/// injected expression PARSES FINE and returns a 200 with a wrong value.
///
/// With correct quote-doubling, `"a' || 'b"` renders as
/// `'a'' || ''b'` and round-trips as the literal 8-byte string `a' || 'b`.
/// With broken doubling it renders as `'a' || 'b'` — still perfectly valid
/// SQL — and Trino evaluates the `||` concatenation to `"ab"`. So a broken
/// escaper makes THIS assertion fail on a wrong VALUE, not on an error,
/// which is exactly the gap the mutation for the other two hostile tests
/// could not demonstrate.
#[tokio::test]
async fn hostile_value_that_would_silently_evaluate_under_broken_escaping() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    let payload = "a' || 'b";
    let out = c
        .execute_with_params(
            "SELECT ? AS s",
            &[Param::Str(payload.into())],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("a bound value, however hostile-looking, must round-trip as data, not error");

    assert_eq!(
        out.rows[0][0],
        serde_json::json!(payload),
        "must be the literal 8-byte string \"a' || 'b\" — NOT the evaluated concatenation \
         \"ab\" that broken quote-doubling would silently produce with a 200 and no error"
    );
}

/// Fix round 1, Important 2. `Decimal`/`Date`/`Timestamp` carry unvalidated
/// caller text through `format!("<TYPE> {}", quote_string(...))`, and
/// nothing upstream enforces that a `Date` actually looks like
/// `YYYY-MM-DD`. The reviewer's constructed failure: a later author
/// rewrites `Param::Date(s) => format!("DATE '{s}'")`, reasoning that "a
/// date can't contain a quote" — then a single `Param::Date` whose text
/// contains `', current_user, DATE '` expands into THREE `USING` items,
/// evaluating `current_user` and shifting every subsequent bind position.
///
/// Reproduced by hand against this exact live coordinator while designing
/// this test (not asserted here, since it requires bypassing
/// `render_literal` entirely — see the task report for the transcript):
/// with the naive form and a placeholder count that happens to match the
/// expanded item count, the call SUCCEEDS and leaks `current_user` as data;
/// with a mismatched count it fails with `Incorrect number of parameters`
/// — a DIFFERENT error from the one asserted below. Today's correct,
/// `quote_string`-routed code keeps the whole hostile string inside ONE
/// typed literal, so Trino's own DATE/DECIMAL/TIMESTAMP grammar rejects the
/// malformed *content* with a single-literal parse error — proven here with
/// a second, ordinary parameter present, so a regression that reintroduces
/// the item-count shift would surface as `Incorrect number of parameters`
/// (or a silent `current_user` leak) instead of the specific error asserted
/// below.
async fn assert_typed_literal_is_contained(c: &dyn Connector, param: Param, type_name: &str) {
    let err = c
        .execute_with_params(
            "SELECT ? a, ? b",
            &[param, Param::Str("second".into())],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect_err("malformed typed-literal content must be rejected, not evaluated");
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("not a valid {type_name} literal")),
        "must be the single-literal parse error naming {type_name}, got: {msg}"
    );
    assert!(
        !msg.contains("Incorrect number of parameters"),
        "must not indicate the value split into extra USING items (a bind-position shift): {msg}"
    );
    assert!(
        !msg.contains("tester"),
        "must not indicate `current_user` was evaluated: {msg}"
    );
}

#[tokio::test]
async fn hostile_decimal_content_is_contained_not_evaluated() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    // The reviewer independently verified the parenthesised form
    // `Decimal("1) OR (1=1")`, which contains no `'` and so cannot
    // distinguish escaping-correct from escaping-broken — this adds an
    // embedded quote (mirroring the DATE/TIMESTAMP payloads) so the
    // mutation actually exercises `quote_string`. See
    // `decimal_hostile_content_still_goes_through_quote_string` in
    // `trino.rs` for the exact escaped-string pin.
    assert_typed_literal_is_contained(c.as_ref(), Param::Decimal("1' OR '1'='1".into()), "DECIMAL")
        .await;
}

#[tokio::test]
async fn hostile_date_content_is_contained_not_evaluated() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    // Independently verified during review: `DATE '2020-01-01'' OR ''1''=''1'`.
    assert_typed_literal_is_contained(
        c.as_ref(),
        Param::Date("2020-01-01' OR '1'='1".into()),
        "DATE",
    )
    .await;
}

#[tokio::test]
async fn hostile_timestamp_content_is_contained_not_evaluated() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    // The exact `current_user`-injection shape the reviewer's failure
    // scenario describes, bound as ONE Timestamp param.
    assert_typed_literal_is_contained(
        c.as_ref(),
        Param::Timestamp(
            "2020-01-01 00:00:00', current_user, TIMESTAMP '2020-01-01 00:00:00".into(),
        ),
        "TIMESTAMP",
    )
    .await;
}
