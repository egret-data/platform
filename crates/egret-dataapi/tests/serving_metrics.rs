//! Package 2 — the serving path's metrics, asserted by driving the REAL router
//! and watching the numbers move.
//!
//! # What this file refuses to do
//!
//! It never asserts that a counter *exists*. A rendered scrape contains a series
//! the moment something touches it, so "the metric is registered" passes whether
//! or not the increment is on the path a request takes — and a metric that is
//! registered but never incremented is exactly the failure this package exists
//! to prevent. Every assertion here is a DELTA across a real request through
//! `serving_router`.
//!
//! Each test gets its OWN recorder, installed as a thread-local for the
//! duration of its body ([`with_isolated_recorder`]). libtest runs these in
//! parallel, and against one shared registry a "did this counter move by one"
//! assertion is answering for every test that happened to overlap — which is
//! how `an_endpoint_with_caching_off_is_not_counted_as_a_miss` first went red
//! for a reason that had nothing to do with the endpoint it published. An
//! isolated registry makes every reading exact, so the assertions below are
//! equalities rather than inequalities, and the ones that say a counter did NOT
//! move mean it.
//!
//! # The headline assertion
//!
//! [`a_call_refused_before_a_workspace_is_known_is_counted_and_writes_no_row`]
//! is the whole point of the package: it drives a request with a bad token and
//! asserts BOTH halves of the pairing — the counter moved, and
//! `dataapi.access_log` holds no row for that exact `request_id`. Either half
//! alone is worthless. The counter alone does not show that the log was blind;
//! the missing row alone does not show that anything noticed.
//!
//! It carries its own control: the SAME queries run against a successful call,
//! which must produce a row. Without that, a `WHERE request_id = …` that matched
//! nothing for a silly reason (wrong column, wrong schema, a typo) would read as
//! proof of the property under test.
//!
//! DB-gated; self-skips when `DATABASE_URL` is unset — check the DURATION.

mod serving_common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_dataapi::serving::metrics::{
    CACHE_LOOKUPS_TOTAL, PREAUTH_REJECTIONS_TOTAL, REQUESTS_TOTAL, REQUEST_DURATION_SECONDS,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serving_common::{fixture, Fixture};
use tower::ServiceExt;

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

/// A registry only this test can touch, and the scrape helper reading it.
///
/// `metrics`' recorder is normally process-global, which is what lets
/// `egret-dataapi` instrument itself without being handed an exporter. For a
/// test that is the wrong scope: libtest runs these in parallel, so a global
/// registry answers for every test that overlapped and no exact reading is
/// possible. `with_local_recorder` installs one for the current thread only,
/// and a current-thread runtime driven inside it keeps every poll of the
/// request future on that thread — so the increments the shipped router makes
/// land here and nowhere else.
///
/// It is still the REAL `metrics-exporter-prometheus` recorder rendering real
/// exposition text. A test double would assert that the code calls a function,
/// which is not the same claim as "the number an operator scrapes moved".
struct Scrape(PrometheusHandle);

impl Scrape {
    fn text(&self) -> String {
        self.0.render()
    }

    /// The value of one series, summed over every line matching `name` that
    /// carries all of `labels`.
    ///
    /// `0.0` when the series is absent, because a counter nothing has touched
    /// has no line at all — and for a "before" reading that is exactly right.
    fn series(&self, name: &str, labels: &[(&str, &str)]) -> f64 {
        Self::series_in(&self.text(), name, labels)
    }

    fn series_in(text: &str, name: &str, labels: &[(&str, &str)]) -> f64 {
        text.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                // `strip_prefix` alone would match `…_seconds_bucket` when asked
                // for `…_seconds`; requiring `{` or ` ` immediately after the
                // name is what keeps a histogram's three derived series out of a
                // counter's reading.
                let rest = line.strip_prefix(name)?;
                let (label_part, value) = match rest.strip_prefix('{') {
                    Some(r) => r.split_once('}')?,
                    None => ("", rest.strip_prefix(' ')?),
                };
                labels
                    .iter()
                    .all(|(k, v)| label_part.contains(&format!("{k}=\"{v}\"")))
                    .then(|| value.trim().parse::<f64>().ok())
                    .flatten()
            })
            .sum()
    }
}

/// Run an async test body with a private recorder in scope.
///
/// `#[test]` rather than `#[tokio::test]` at every call site below, because the
/// runtime has to be built INSIDE the thread-local's scope: a runtime that
/// outlives it (or a multi-thread one that moves the future) would poll the
/// request on a thread where the local recorder is not installed, and every
/// increment would go to the global no-op recorder instead. The tests would then
/// read `0` everywhere and the "did not move" assertions would pass vacuously,
/// which is the failure mode this whole file is written against.
fn with_isolated_recorder<F, Fut>(body: F)
where
    F: FnOnce(Scrape) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let recorder = PrometheusBuilder::new().build_recorder();
    let scrape = Scrape(recorder.handle());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime");
    metrics::with_local_recorder(&recorder, || rt.block_on(body(scrape)));
}

/// How many `dataapi.access_log` rows carry this `request_id`.
///
/// Keyed on the id the caller was actually given, so the answer is about THIS
/// call and cannot be perturbed by tests running beside it.
async fn logged_rows(f: &Fixture, request_id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count access_log rows")
}

/// A raw serving call, with whatever `Authorization` header the caller names (or
/// none at all).
///
/// The shared fixture's `call_as*` helpers always mint a VALID token, which is
/// the one thing this file must be able not to do.
async fn call_with_authorization(
    f: &Fixture,
    authorization: Option<&str>,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/data-api/v1/{}/{path}", f.ws_slug));
    if let Some(value) = authorization {
        builder = builder.header("authorization", value);
    }
    let res = f
        .serving
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

fn no_rate_limit() -> serde_json::Value {
    serde_json::json!({})
}

// ---------------------------------------------------------------------------
// The one that matters.
// ---------------------------------------------------------------------------

/// A rejection before authentication resolves a workspace: counted, and absent
/// from `dataapi.access_log`.
///
/// This is the outage the package exists for. If the token endpoint breaks or
/// the signing key rotates, every call lands here. `access_log.workspace_id` is
/// `NOT NULL` and no workspace is known, so `AccessLogStore::record` writes
/// nothing; the activity statistics computed over that table report `None` for
/// the error rate rather than 1.0, so no alert fires; and the refusal logs at
/// `debug` deliberately. Nothing else in the system can see it.
///
/// Both halves are asserted together because neither is the property on its own.
#[test]
fn a_call_refused_before_a_workspace_is_known_is_counted_and_writes_no_row() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish("/counted", f.database_id, no_cache(), no_rate_limit())
            .await;

        // The control, FIRST: a good call must produce a row for its own
        // `request_id`. Without this, the `assert_eq!(…, 0)` below would pass
        // just as well if the query named a column that does not exist or looked
        // in an empty schema — "found nothing" would be an artefact of the
        // harness rather than evidence about the system.
        let served = f.call_as(f.principal_a, "counted").await;
        assert_eq!(served.status, StatusCode::OK, "{}", served.body);
        let served_id = served.body["meta"]["request_id"]
            .as_str()
            .expect("a served response carries meta.request_id")
            .to_string();
        assert_eq!(
            logged_rows(&f, &served_id).await,
            1,
            "an authenticated call must be recorded in dataapi.access_log — if \
             this fails, the zeroes asserted below prove nothing"
        );

        assert_eq!(
            m.series(PREAUTH_REJECTIONS_TOTAL, &[]),
            0.0,
            "the served call above must not have been counted as a \
             pre-authentication refusal"
        );

        // Two shapes of the same failure: a token that is not ours, and no token
        // at all. Both are `ServingError::Unauthorized`, and both are refused
        // before `entry.workspace_id` is ever set.
        let (status, body) =
            call_with_authorization(&f, Some("Bearer not.a.token"), "counted").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        let bad_token_id = body["error"]["request_id"]
            .as_str()
            .expect("a serving error body carries error.request_id")
            .to_string();

        let (status, body) = call_with_authorization(&f, None, "counted").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        let no_token_id = body["error"]["request_id"]
            .as_str()
            .expect("a serving error body carries error.request_id")
            .to_string();

        // Half one: the system noticed. Exactly two, from exactly two refusals.
        assert_eq!(
            m.series(PREAUTH_REJECTIONS_TOTAL, &[]),
            2.0,
            "both pre-authentication refusals must increment \
             {PREAUTH_REJECTIONS_TOTAL}"
        );

        // Half two: nothing else could have. This is why half one is the most
        // valuable metric in the package rather than one more counter.
        assert_eq!(
            logged_rows(&f, &bad_token_id).await,
            0,
            "a call refused before a workspace is known cannot be recorded in \
             dataapi.access_log — workspace_id is NOT NULL"
        );
        assert_eq!(logged_rows(&f, &no_token_id).await, 0);

        // And the refusals are ordinary requests too, so the request counter
        // carries them under the taxonomy's own code.
        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "key_invalid"), ("status", "401")]
            ),
            2.0,
            "a 401 is still a served request and must appear in {REQUESTS_TOTAL}"
        );
    });
}

// ---------------------------------------------------------------------------
// The ordinary good practice.
// ---------------------------------------------------------------------------

/// A served call moves the request counter under `code="ok"`, and moves the
/// latency histogram with it.
///
/// The histogram is checked through its `_count`, which is the only part of it
/// that is a claim about the request path rather than about bucket boundaries.
#[test]
fn a_served_call_moves_the_request_counter_and_the_latency_histogram() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish("/ok", f.database_id, no_cache(), no_rate_limit())
            .await;

        let res = f.call_as(f.principal_a, "ok").await;
        assert_eq!(res.status, StatusCode::OK, "{}", res.body);

        assert_eq!(
            m.series(REQUESTS_TOTAL, &[("code", "ok"), ("status", "200")]),
            1.0,
            "a served call must increment \
             {REQUESTS_TOTAL}{{code=\"ok\",status=\"200\"}}"
        );
        assert_eq!(
            m.series(
                &format!("{REQUEST_DURATION_SECONDS}_count"),
                &[("status", "200")]
            ),
            1.0,
            "a served call must be observed by {REQUEST_DURATION_SECONDS}"
        );
    });
}

/// A rate-limited call is counted under its own code, not folded into a generic
/// error bucket.
///
/// `rate_limited` is the code an operator alerts on to tell "a client is being
/// shed" from "the endpoint is broken", so the two must be separable in the
/// metric exactly as they are in `access_log.error_code`.
#[test]
fn a_rate_limited_call_is_counted_under_its_own_code() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish(
            "/limited",
            f.database_id,
            no_cache(),
            serde_json::json!({ "per_key_qps": 1, "burst": 1 }),
        )
        .await;

        let first = f.call_as(f.principal_a, "limited").await;
        assert_eq!(first.status, StatusCode::OK, "{}", first.body);
        let second = f.call_as(f.principal_a, "limited").await;
        assert_eq!(
            second.status,
            StatusCode::TOO_MANY_REQUESTS,
            "the fixture's clock is frozen, so the second call is inside the \
             same window: {}",
            second.body
        );

        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "rate_limited"), ("status", "429")]
            ),
            1.0,
            "a 429 must increment {REQUESTS_TOTAL}{{code=\"rate_limited\"}}"
        );
        assert_eq!(
            m.series(REQUESTS_TOTAL, &[("code", "ok")]),
            1.0,
            "and the admitted call must still be counted as one success"
        );
    });
}

/// A miss then a hit, counted apart.
///
/// Both are asserted from ONE endpoint in one test, because the property is that
/// the two are DISTINGUISHED — a single counter incremented twice would satisfy
/// either assertion made alone.
#[test]
fn a_cache_miss_and_a_cache_hit_are_counted_apart() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish(
            "/cached",
            f.database_id,
            serde_json::json!({ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }),
            no_rate_limit(),
        )
        .await;

        let cold = f.call_as(f.principal_a, "cached").await;
        assert_eq!(cold.status, StatusCode::OK, "{}", cold.body);
        assert!(!cold.cached(), "the first call cannot be a hit");

        assert_eq!(
            m.series(CACHE_LOOKUPS_TOTAL, &[("result", "miss")]),
            1.0,
            "the cold call must be counted as a miss"
        );
        assert_eq!(
            m.series(CACHE_LOOKUPS_TOTAL, &[("result", "hit")]),
            0.0,
            "the cold call must NOT also be counted as a hit"
        );

        let warm = f.call_as(f.principal_a, "cached").await;
        assert_eq!(warm.status, StatusCode::OK, "{}", warm.body);
        assert!(
            warm.cached(),
            "the second call must be served from the cache"
        );

        assert_eq!(
            m.series(CACHE_LOOKUPS_TOTAL, &[("result", "hit")]),
            1.0,
            "the warm call must be counted as a hit"
        );
        assert_eq!(
            m.series(CACHE_LOOKUPS_TOTAL, &[("result", "miss")]),
            1.0,
            "and it must not also have been counted as a second miss"
        );
    });
}

/// An endpoint with caching switched off contributes NEITHER a hit nor a miss.
///
/// The alternative — counting every uncached success as a miss — would put a 0%
/// hit rate on a dashboard for an endpoint that has no cache, which is a number
/// an operator would try to fix.
#[test]
fn an_endpoint_with_caching_off_is_not_counted_as_a_miss() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish("/uncached", f.database_id, no_cache(), no_rate_limit())
            .await;

        let res = f.call_as(f.principal_a, "uncached").await;
        assert_eq!(res.status, StatusCode::OK, "{}", res.body);

        assert_eq!(
            m.series(CACHE_LOOKUPS_TOTAL, &[]),
            0.0,
            "an endpoint with no cache consults nothing, so there is neither a \
             hit nor a miss to record"
        );
        // Not vacuous: the same call DID move the request counter, so the zero
        // above is about the cache and not about a recorder that saw nothing.
        assert_eq!(m.series(REQUESTS_TOTAL, &[("code", "ok")]), 1.0);
    });
}

// ---------------------------------------------------------------------------
// Cardinality, as an assertion rather than as a comment.
// ---------------------------------------------------------------------------

/// Nothing a caller controls reaches a label.
///
/// The values driven here are the ones it would be most tempting to label with
/// and are each unbounded: the request path (a scanner mints a series per URL it
/// guesses), the client id, and — through the 400 — the parameter surface the
/// error taxonomy is careful never to name. The scrape is searched for each as a
/// raw substring, which is the only check that does not depend on guessing which
/// label someone might have added.
#[test]
fn no_caller_controlled_value_reaches_a_label() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish_with_params(
            "/cardinality-probe",
            f.database_id,
            no_cache(),
            no_rate_limit(),
            "SELECT 1 AS n WHERE 1=1 {% if probeparam %} AND region = {{ probeparam }} {% endif %}",
            serde_json::json!([
                { "name": "probeparam", "type": "string", "location": "query", "required": true }
            ]),
        )
        .await;

        // A 404 on a path only this test uses.
        let missing = f
            .call_as(f.principal_a, "no-such-endpoint-cardinality-probe")
            .await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.body);

        // A 400 whose cause is a declared parameter — the error body names
        // neither the parameter nor the constraint, and neither may the metric.
        let bad = f.call_as(f.principal_a, "cardinality-probe").await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);

        // A 200 under a client id nobody else uses.
        let served = f
            .call_as_client(
                f.principal_a,
                "client-cardinality-probe",
                "cardinality-probe?probeparam=x",
            )
            .await;
        assert_eq!(served.status, StatusCode::OK, "{}", served.body);

        let text = m.text();
        for forbidden in [
            "cardinality-probe",
            "no-such-endpoint",
            "client-cardinality-probe",
            "probeparam",
            f.ws_slug.as_str(),
            &f.ws_id.to_string(),
        ] {
            assert!(
                !text.contains(forbidden),
                "{forbidden:?} reached the scrape. A caller-controlled label is \
                 unbounded by construction and takes the scrape down; a \
                 tenant-named one turns an unauthenticated endpoint into a \
                 disclosure. Read the cardinality section in \
                 serving/metrics.rs before changing this.\n{text}"
            );
        }

        // Guard against a vacuous pass: the three calls above must be IN this
        // scrape, or the loop proved only that the text was empty.
        assert_eq!(
            m.series(REQUESTS_TOTAL, &[("code", "not_found")]),
            1.0,
            "{text}"
        );
        assert_eq!(
            m.series(REQUESTS_TOTAL, &[("code", "param_missing")]),
            1.0,
            "{text}"
        );
        assert_eq!(m.series(REQUESTS_TOTAL, &[("code", "ok")]), 1.0, "{text}");
    });
}
