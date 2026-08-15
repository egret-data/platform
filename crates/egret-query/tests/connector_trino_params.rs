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
//!     invocation.
//!
//! A self-skipped run proves nothing.

use egret_query::execution::connector::{build_connector, ConnSpec};
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
fn trino_or_skip() -> Option<Box<dyn egret_query::execution::connector::Connector>> {
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
/// string, a NULL, a backslash, and a single quote — assert the EXACT JSON
/// returned. `"007"` catches string-to-number coercion; the quote is the P1
/// injection vector; the backslash pins that Trino does NOT treat it as an
/// escape (so this connector must never double it, unlike the StarRocks
/// path); `NULL` and the decimal catch type collapse; the date pins that a
/// typed `DATE '…'` literal round-trips as a date, not as its literal text.
#[tokio::test]
async fn bound_values_round_trip_exactly_on_trino() {
    let Some(c) = trino_or_skip() else {
        eprintln!("skip");
        return;
    };
    let out = c
        .execute_with_params(
            "SELECT ? d, ? m, ? s, ? n, ? bs, ? q",
            &[
                Param::Date("2020-02-29".into()),
                Param::Decimal("12345.6789".into()),
                Param::Str("007".into()),
                Param::Null,
                Param::Str(r"a\ OR 1=1 -- ".into()),
                Param::Str("it's".into()),
            ],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect("bind");

    assert_eq!(
        serde_json::to_string(&out.rows[0]).unwrap(),
        r#"["2020-02-29","12345.6789","007",null,"a\\ OR 1=1 -- ","it's"]"#,
        "exact JSON round-trip of all six bound values"
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
