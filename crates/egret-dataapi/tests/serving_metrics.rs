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
//! # Two gates, on purpose
//!
//! Most of this file self-skips when `DATABASE_URL` is unset ([`skip_without_db`])
//! — check the DURATION, a 0.00s binary asserted nothing.
//!
//! [`no_caller_controlled_value_reaches_a_label`] does NOT. It uses
//! [`require_db`] and panics instead, because it is not a test of a feature: it
//! is the only automated thing standing between an unauthenticated `/metrics`
//! (mounted beside the health routes, in neither `deploy/helm/egret/templates/
//! ingress.yaml` nor `frontend/nginx-prod.conf`, so cluster-internal but
//! unguarded) and a label naming a tenant. A guard that reports success in the
//! environment where it ran nothing is not a guard, and this one would have
//! self-skipped green in exactly the CI shape most likely to lack a database.

mod serving_common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_dataapi::serving::metrics::{
    ACCESS_LOG_WRITE_FAILURES_TOTAL, CACHE_LOOKUPS_TOTAL, PREAUTH_REJECTIONS_TOTAL, REQUESTS_TOTAL,
    REQUEST_DURATION_SECONDS,
};
use egret_dataapi::serving::{AccessLogEntry, AccessLogStore, Recorded};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serving_common::{fixture, Fixture};
use tower::ServiceExt;
use uuid::Uuid;

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

/// The DB gate for a test that must not pass by not running.
///
/// A sibling package is turning the self-skips in this tree into failures
/// wholesale; this does not depend on that landing and does not conflict with it
/// either — if the shared `fixture()` starts panicking on its own, this stops
/// being reachable and nothing here has to change.
macro_rules! require_db {
    () => {
        fixture().await.expect(
            "DATABASE_URL must be set for this test. It is the disclosure guard on an \
             unauthenticated /metrics endpoint, so 'it did not run' must never render as \
             'it passed'.",
        )
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

        // And it is counted under the right one of the two no-row reasons. A
        // single counter for both would satisfy every assertion above.
        assert_eq!(
            m.series(ACCESS_LOG_WRITE_FAILURES_TOTAL, &[]),
            0.0,
            "a pre-authentication refusal is not a failed INSERT; counting it as \
             one would make an alert name the metastore for an authentication \
             outage"
        );
    });
}

/// `record` says `Row` for exactly the entries that end up in the table.
///
/// The counters above are only worth anything if they are the table's
/// complement, and since this change that complement is one expression:
/// `AccessLogStore::record` returns [`Recorded`], and the serving path counts
/// whichever no-row variant it was handed. No type can stop someone returning
/// `Recorded::Row` from a path that wrote nothing, so this asserts the
/// biconditional against the real table.
///
/// It is written as a TABLE of entry shapes with one shared assertion, rather
/// than as one test per case, because the property is general: *whatever* a
/// future `record` decides about *whatever* entry, its answer and the table must
/// agree. A new early return added to `record` — filtering trial rows the way
/// `activity` does elsewhere, which is the drift the reviewer named — only has
/// to be given a row here to be checked, and gets checked against the same
/// assertion as everything else. The trial-actor case is present already, so
/// that particular one is covered before it is written.
///
/// The `no such workspace` case is the one that exercises the failed INSERT:
/// `access_log.workspace_id` carries a foreign key to `workspaces`, and
/// `endpoint_id` deliberately does not, so this is the way to make the statement
/// fail without breaking the pool.
/// No recorder: `record` touches no metric, and pretending otherwise would hide
/// that the mapping from its answer to a counter lives elsewhere (asserted by
/// `each_no_row_outcome_moves_its_own_counter_and_a_row_moves_neither`, a unit
/// test in `serving/metrics.rs`).
#[tokio::test]
async fn record_reports_a_row_exactly_when_one_exists() {
    let f = skip_without_db!();
    let store = AccessLogStore::new(f.db.clone());

    type Shape = (&'static str, Box<dyn Fn(&mut AccessLogEntry)>);
    let ws = f.ws_id;
    let shapes: Vec<Shape> = vec![
        (
            "a resolved workspace",
            Box::new(move |e: &mut AccessLogEntry| e.workspace_id = Some(ws)),
        ),
        (
            "no workspace, as after a pre-authentication refusal",
            Box::new(|_e: &mut AccessLogEntry| {}),
        ),
        (
            "a resolved workspace and a trial actor",
            Box::new(move |e: &mut AccessLogEntry| {
                e.workspace_id = Some(ws);
                e.actor_user_id = Some(Uuid::new_v4());
            }),
        ),
        (
            "a workspace id no row in `workspaces` carries, so the INSERT fails",
            Box::new(|e: &mut AccessLogEntry| e.workspace_id = Some(Uuid::new_v4())),
        ),
    ];

    let mut seen_row = false;
    let mut seen_gap = false;
    for (what, shape) in shapes {
        let request_id = Uuid::new_v4().to_string();
        let mut entry = AccessLogEntry::new(&request_id, None);
        entry.status_code = 200;
        shape(&mut entry);

        let recorded = store.record(&entry).await;
        let rows = logged_rows(&f, &request_id).await;

        // The invariant, stated once. Not "this shape returns that variant" —
        // that would be four expectations to keep in step with `record`, which
        // is the shape of the problem rather than of the fix.
        assert_eq!(
            recorded == Recorded::Row,
            rows == 1,
            "record() answered {recorded:?} for an entry with {what}, but \
             dataapi.access_log holds {rows} row(s) for its request_id. The \
             counters that measure the gap are defined as this table's \
             complement, so an answer the table disagrees with makes them \
             measure something else."
        );
        seen_row |= recorded == Recorded::Row;
        seen_gap |= recorded != Recorded::Row;
    }

    // Not vacuous: a biconditional is satisfied by every case landing on the
    // same side, and `false == false` four times would say nothing at all.
    assert!(
        seen_row,
        "no shape produced a row; the assertion above was trivial"
    );
    assert!(
        seen_gap,
        "no shape produced a gap; the assertion above was trivial"
    );
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

/// Nothing a caller controls, and nothing that names a tenant, reaches a label.
///
/// Two families, both unbounded and each for its own reason:
///
/// * **Caller-controlled** — the request path (a scanner mints a series per URL
///   it guesses), the client id, and, through the 400, the parameter surface the
///   error taxonomy is careful never to name.
/// * **Tenant-scoped** — the workspace slug and id, the ENDPOINT id, the
///   database id, and the invoking principal. These are bounded per deployment
///   but bounded by what tenants create, and `/metrics` is unauthenticated, so a
///   label carrying one is a cross-tenant disclosure and not merely a
///   cardinality problem. The endpoint id is the one this test used to discard:
///   `publish_with_params` returns it, and an `endpoint_id` label — forbidden by
///   name in `serving/metrics.rs` — would have passed here.
///
/// The scrape is searched for each as a raw substring, which is the only check
/// that does not depend on guessing which label someone might have added.
///
/// `require_db!`, not `skip_without_db!` — see this file's header.
#[test]
fn no_caller_controlled_value_reaches_a_label() {
    with_isolated_recorder(|m| async move {
        let f = require_db!();
        let endpoint_id = f.publish_with_params(
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
        // Every identifier in scope at any point on the serving path, not a
        // sample of them: the module doc forbids `endpoint_id` and
        // `workspace_id` by name, and a list that omits one of the names it
        // forbids is a guard with a hole in exactly the place the prose says
        // matters.
        for forbidden in [
            "cardinality-probe",
            "no-such-endpoint",
            "client-cardinality-probe",
            "probeparam",
            f.ws_slug.as_str(),
            &f.ws_id.to_string(),
            &endpoint_id.to_string(),
            &f.database_id.to_string(),
            &f.principal_a.0.to_string(),
            &f.admin.to_string(),
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

// ---------------------------------------------------------------------------
// The mount's SECOND route, and telling it apart from the first.
// ---------------------------------------------------------------------------

/// A contract fetch moves the request counter, under a route label that is not
/// the call path's.
///
/// # Why both halves are one test
///
/// The defect was not "the contract route had the wrong label" — it had no
/// increment at all, so `/metrics` showed a workspace's contract traffic as if
/// it did not exist, including the 429s from the bucket that route has to
/// itself. The obvious repair, folding it into the existing series, would be
/// just as wrong in the other direction: an operator reading
/// `egret_dataapi_requests_total` as endpoint traffic would find contract
/// fetches in the endpoint error rate, and a contract 503 would page someone
/// about queries.
///
/// So the claim is "counted AND distinguishable", and it needs one call and one
/// contract fetch in the same registry. Each assertion below fails under a
/// different mutation:
///
/// * delete `metrics::record_request(...)` from `contract::serve_contract` and
///   the `route="contract"` readings are `0`;
/// * pass `Route::Call` there instead and `route="call"` reads `2` while
///   `route="contract"` reads `0`;
/// * drop the `route` label from `record_request` and BOTH selectors read `0`,
///   because a series that carries no `route` at all matches neither.
#[test]
fn a_contract_fetch_is_counted_and_told_apart_from_a_call() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();
        f.publish("/counted-apart", f.database_id, no_cache(), no_rate_limit())
            .await;

        let called = f.call_as(f.principal_a, "counted-apart").await;
        assert_eq!(called.status, StatusCode::OK, "{}", called.body);

        let fetched = f.contract_as(f.principal_a).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.body);

        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "ok"), ("status", "200"), ("route", "contract")]
            ),
            1.0,
            "the contract fetch must be counted. It is the most expensive request \
             this surface serves — one PDP decision per live endpoint — and it \
             was invisible to the only instrument that can see it:\n{}",
            m.text()
        );
        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "ok"), ("status", "200"), ("route", "call")]
            ),
            1.0,
            "and the endpoint call must still be counted as exactly one call — \
             not two, which is what folding the two routes into one series \
             looks like:\n{}",
            m.text()
        );
        assert_eq!(
            m.series(
                &format!("{REQUEST_DURATION_SECONDS}_count"),
                &[("route", "contract")]
            ),
            1.0,
            "the latency of a contract fetch is observed too — it is the request \
             whose cost grows with the workspace, so a p95 that excludes it \
             describes the cheap half of the surface:\n{}",
            m.text()
        );
    });
}

/// The contract route's OWN rate-limit refusal is counted, under the contract
/// route.
///
/// This is the refusal the design doc's §5 shape is about: measured on the
/// cluster, twenty concurrent fetches produced one `429` that appeared in no
/// series anywhere. A `dataapi.access_log` row cannot be the backstop for it —
/// the table is per endpoint and a contract fetch names none — so if the counter
/// does not move, nothing in the system knows the bucket ever fired.
///
/// The burst is spent through the real route with the fixture's frozen clock, so
/// the refusal is the shipped limiter's and not a constant this test asserts
/// against itself.
#[test]
fn a_refused_contract_fetch_is_counted_under_the_contract_route() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();

        let burst = egret_dataapi::serving::ratelimit::CONTRACT_BURST;
        for i in 0..burst {
            let res = f.contract_as(f.principal_a).await;
            assert_eq!(res.status, StatusCode::OK, "fetch {i}: {}", res.body);
        }
        let refused = f.contract_as(f.principal_a).await;
        assert_eq!(
            refused.status,
            StatusCode::TOO_MANY_REQUESTS,
            "the burst is spent: {}",
            refused.body
        );

        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[
                    ("code", "rate_limited"),
                    ("status", "429"),
                    ("route", "contract")
                ]
            ),
            1.0,
            "the contract bucket's refusal must be visible. Nothing else in the \
             system records it: there is no access_log row for a contract \
             fetch:\n{}",
            m.text()
        );
        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "rate_limited"), ("route", "call")]
            ),
            0.0,
            "and it must not read as an endpoint being rate limited — that is an \
             alert about a consuming team exceeding a published allowance, fired \
             for a discovery loop:\n{}",
            m.text()
        );
        assert_eq!(
            m.series(REQUESTS_TOTAL, &[("code", "ok"), ("route", "contract")]),
            burst as f64,
            "the admitted fetches are counted too:\n{}",
            m.text()
        );
    });
}

/// A bound that fires is counted under the route it refused — for BOTH routes.
///
/// The three bounds wrap the whole mount and fire before any handler, so unlike
/// the two handlers they cannot name their own route: `metrics::Route::of_path`
/// reads it off the request path, which is a second reading of the routing
/// table. This drives the real router at both real URLs so that second reading
/// is checked against the first, rather than by calling the classifier with
/// strings — the unit test in `serving/metrics.rs` does that, and it would agree
/// with a routing table that had changed underneath it.
///
/// The body cap is the bound used because it is the one a test can trip with a
/// header and no waiting. Its label comes from the same expression the other two
/// use.
#[test]
fn a_bound_that_fires_is_counted_under_the_route_it_refused() {
    with_isolated_recorder(|m| async move {
        let f = skip_without_db!();

        // The real router, with only the body cap changed — four bytes, so any
        // declared body trips it. The other two bounds stay at production
        // values: this test is about the label, not about the bound.
        let app = axum::Router::new()
            .nest(
                "/data-api/v1",
                egret_dataapi::serving::router::serving_router_with_bounds(
                    egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
                    egret_dataapi::serving::SERVING_MAX_CONCURRENCY,
                    4,
                ),
            )
            .with_state(f.state.clone());

        let over_the_cap = |uri: String| {
            let app = app.clone();
            async move {
                let res = app
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(uri)
                            .header("content-length", "1024")
                            .body(Body::from(vec![b'x'; 1024]))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                res.status()
            }
        };

        // The contract URL. No token: the cap fires before routing, so it never
        // reaches authentication — which is exactly why the metric is the only
        // instrument that sees this at all.
        assert_eq!(
            over_the_cap(format!("/data-api/v1/{}", f.ws_slug)).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // And a call URL.
        assert_eq!(
            over_the_cap(format!("/data-api/v1/{}/probe", f.ws_slug)).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );

        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "payload_too_large"), ("route", "contract")]
            ),
            1.0,
            "a bound firing on the contract URL must be filed under the contract \
             route:\n{}",
            m.text()
        );
        assert_eq!(
            m.series(
                REQUESTS_TOTAL,
                &[("code", "payload_too_large"), ("route", "call")]
            ),
            1.0,
            "and one firing on an endpoint URL under the call route. A constant \
             here would make every shed and every deadline on a contract fetch \
             read as endpoint-call traffic:\n{}",
            m.text()
        );
    });
}
