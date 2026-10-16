//! Request-scoped bounds on the serving surface: a deadline, a bulkhead, and a
//! body cap.
//!
//! # Why these live here and not at the mount site
//!
//! [`crate::serving_router`] applies them, so every caller of that function gets
//! them: `egret-server`'s composed router and all twenty of the
//! `tests/serving_*.rs` suites. A bound applied in `egret-server` instead would
//! be a bound that the suites which actually exercise this surface never see —
//! which is the shape of failure this repo keeps repeating. It also cannot be
//! forgotten at a second mount site.
//!
//! `try_it`'s in-process trial router gets the same three layers through
//! [`super::router::serving_router_sharing_bulkhead`], with exactly **one of the
//! three numbers changed**: the bulkhead, because a router built per trial
//! cannot own its own semaphore. The deadline and the body cap are the
//! consumer's, deliberately — Try it answers "can a consumer call this?", so a
//! trial that outlived what a consumer gets would answer the wrong question.
//! See [`crate::TRIAL_MAX_CONCURRENCY`] and `try_it`'s own block above it.
//!
//! # Why the answers are the serving envelope and not tower-http's
//!
//! `tower-http` ships a `TimeoutLayer` and a `RequestBodyLimitLayer` and either
//! would be fewer lines. Both answer with a **bare status and an empty body**.
//! The serving contract is that *every* failure is
//! `{status, message, error:{code, message, request_id}}` — that is what spec §7
//! documents and what consumers branch on. Three of the surface's answers
//! arriving with no envelope would make the bounds the one part of the API a
//! client cannot handle uniformly, so the middleware is written out.
//!
//! # These three codes ARE the taxonomy's, and nothing here builds one
//!
//! They used to be built here: three `&'static str` literals and three
//! hand-assembled [`super::error::ServingErrorBody`] values, sitting beside the
//! exhaustive `to_wire` match rather than inside it. That is the arrangement
//! that makes a new variant a compile error *everywhere except the place it was
//! bypassed*, and it is why the same 504 could be spelled `timeout` by this
//! module and `timeout` by `EngineTimeout` at the same time without anything
//! noticing.
//!
//! [`super::ServingError::ServingDeadline`], [`super::ServingError::Overloaded`]
//! and [`super::ServingError::PayloadTooLarge`] now carry them, and
//! [`bound_response`] takes a variant and renders it with
//! [`super::error::to_wire`]. So the status, the `error.code`, the message and
//! the `Retry-After` of a bound refusal are decided in exactly the same match as
//! every other serving answer — which is what puts them in
//! [`super::error::wire_answers`], and therefore in the consumer's OpenAPI
//! document and in the metrics module's cardinality bound, by construction
//! rather than by anyone remembering.
//!
//! # What a `request_id` from here means
//!
//! Minted here, not threaded from the handler: at the moment a bound fires there
//! either is no handler yet (`overloaded`, `payload_too_large`) or its id died
//! with the cancelled future (`deadline_exceeded`). It is still the id in the `tracing`
//! line for the same event, so "quote your request_id" remains actionable —
//! but there is **no `dataapi.access_log` row** behind it, because that insert
//! happens at the end of the handler. See [`SERVING_REQUEST_DEADLINE`].

use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::error::to_wire;
use super::ServingError;

/// What the deployed ingress will wait for a Data API call, in seconds.
///
/// egret's `egret` Ingress object sets
/// `nginx.ingress.kubernetes.io/proxy-read-timeout: "60"` for `/data-api` and
/// every other path but `/catalog`, which has its own object at 120 s
/// (`deploy/helm/egret/templates/ingress.yaml`). That annotation was added
/// *with* this constant: before it, 60 was ingress-nginx's **default**, read out
/// of the running controller's generated config rather than chosen — a number
/// this side depends on and the chart did not own.
/// `ingress_read_timeout_is_the_number_this_module_derives_from` is the test
/// that keeps the two spellings equal.
pub const INGRESS_READ_TIMEOUT_SECS: u64 = 60;

/// How long a served request may take before egret answers it `504`.
///
/// # It is derived from the ingress, and it has to be
///
/// A request timeout **longer** than [`INGRESS_READ_TIMEOUT_SECS`] is
/// decoration: nginx has already given the caller its own `504` HTML page, the
/// connection egret is still writing to is gone, and the operator sees an edge
/// error with no matching egret log line. So the only question this number
/// answers is *how much less than 60*.
///
/// `60 − 50 = 10 s` of margin, and each second of it is spent on something:
///
/// * **nginx's clock starts earlier than this middleware's.** It begins when the
///   request is proxied upstream; this layer's begins after the connection is
///   accepted, TLS is negotiated and a tokio worker picks the task up — all of
///   which stretch under exactly the load that makes a request slow. The
///   bulkhead is nested *inside* this deadline (see [`SERVING_MAX_CONCURRENCY`])
///   so that admission time is charged to the caller's budget rather than added
///   to it; today it sheds instantly, but that ordering is what keeps the
///   arithmetic true if it is ever changed to queue;
/// * **the answer still has to be written after the deadline fires.** A `504`
///   envelope is small, but the deadline can also fire while a large success
///   body is being serialised, and that body is fully materialised in memory
///   before the first byte goes out.
///
/// # What it does NOT do
///
/// It does not make an endpoint's worst case fit. `MAX_STATEMENTS` is 16 and
/// `TIMEOUT_CEILING_SECS` is 300, so the published worst case is 80 minutes and
/// no request-level number reconciles that — keeping a real endpoint inside 60 s
/// is `limits.timeout_seconds`'s job, per statement, exactly as
/// `egret_dataapi_api::MAX_STATEMENTS`'s own doc says. What this does is
/// guarantee that when an endpoint overruns, **egret is the thing that answers**,
/// in the serving envelope, with a code, rather than the edge answering with
/// HTML egret never sees.
///
/// # Two consequences, stated rather than discovered
///
/// * **The handler future is dropped.** Cancellation is how the deadline is
///   enforced, so the in-flight query's HTTP connection to Trino/StarRocks
///   closes; the *engine-side* query is not cancelled by that and keeps running
///   until its own `timeout_seconds` expires.
/// * **It bounds the request only up to the response HEAD.** `tokio::time::timeout`
///   completes as soon as `next.run(req)` resolves, and that is the moment the
///   `Response` value exists — not the moment its body has been written. Today
///   the whole body is already materialised by then (`Json(...)` serialises into
///   one buffer before the first byte goes out), so head and completion are the
///   same instant and the bound is total. **That stops being true the day the
///   response streams**, which is the fix the memory limit's comment points at:
///   a streaming body would leave the slow part of a slow request entirely
///   outside this layer. Whoever implements streaming has to move the bound onto
///   the body, not merely keep this constant.
/// * **No `dataapi.access_log` row is written for a timed-out call.** That insert
///   is the last thing `serving::router::serve` does, and it dies with the
///   cancelled future. A timed-out request is therefore visible in `tracing` and
///   invisible in activity — the same blind spot §5 of the design doc describes
///   for pre-authentication failures. Closing it needs the row written from a
///   place that survives cancellation, which belongs with the metrics package
///   that owns instrumentation.
pub const SERVING_REQUEST_DEADLINE: Duration = Duration::from_secs(50);

/// How many served requests may be in flight at once, per process.
///
/// # A bulkhead, not a throughput limit
///
/// The point is that the Data API cannot consume the process. Serving traffic
/// and the browser console share one tokio runtime, one metastore pool and one
/// memory limit; before this, a burst of published-endpoint calls could take all
/// three and the console would go down with them. This cap is the wall between
/// them, and it is why it is applied to `serving_router()` and not to the whole
/// composed router — a cap on everything would isolate nothing from anything.
///
/// # Why 64
///
/// It is **8× the default metastore pool** (`egret_config`'s `db.max_conns`,
/// 10), which the operations audit named the first ceiling this path will hit —
/// roughly 13 round-trips per served request. Setting the bulkhead *at* the pool
/// size would make this layer, rather than the pool or the engine, the thing
/// that limits ordinary throughput, and it would shed requests the system could
/// actually have served. Setting it much higher stops it being a wall.
///
/// # Why it is NOT derived from the worst case
///
/// A single request may materialise `MAX_ROWS_CEILING` = 1 000 000 rows, on the
/// order of gigabytes. No concurrency ≥ 1 survives several of those, so no value
/// here is the answer to that problem; the container memory limit added in the
/// same change bounds its blast radius to one pod, and actually fixing it means
/// streaming the response. Stating this because a cap that *looks* like it
/// bounds memory, and does not, is worse than none.
///
/// # Global, not per-workspace — and that is a choice
///
/// One tenant can still occupy all 64 slots and starve another. Per-workspace
/// isolation was considered and deferred for three reasons, in order of weight:
///
/// 1. **Admission happens before authentication.** The workspace on this path is
///    a path segment until `verify_serving_token` has run, so a per-workspace
///    semaphore keyed here is keyed on attacker-controlled text — an unbounded
///    map, i.e. a memory exhaustion dressed as a fairness feature. Keying it
///    *after* auth means the request has already done the metastore work the
///    bulkhead exists to protect.
/// 2. **It would be per-replica anyway.** Every shared limit on this surface is
///    process-local today (design doc §4 decision B); per-workspace fairness
///    computed per replica is fairness multiplied by the replica count, which is
///    the exact defect Package 4 exists to fix. It should be built once, there,
///    on the shared store.
/// 3. **The per-tenant admission control already exists.** `limits.rate_limit`
///    is published per endpoint and enforced by `super::ratelimit`. The bulkhead
///    is the last resort *behind* it, not the primary fairness mechanism.
pub const SERVING_MAX_CONCURRENCY: usize = 64;

/// Largest `Content-Length` a served request may declare, in bytes.
///
/// # The serving path never reads a request body
///
/// `serve_post` takes `RawQuery`, not a body extractor: parameters arrive in the
/// query string on both `GET` and `POST` (spec §5.2). So axum's
/// `DefaultBodyLimit` — which acts inside body *extractors* — has never applied
/// to this surface at all, and a `POST` carrying any number of megabytes was
/// accepted, streamed across the network and discarded. That is the gap; 64 KiB
/// is a bound generous enough that nothing legitimate is near it, since
/// legitimate is zero.
///
/// # It is enforced on the declared length
///
/// A request with no `Content-Length` (`Transfer-Encoding: chunked`) is not
/// bounded here, because bounding it means *reading* the body to find out how
/// big it is — spending the bandwidth the cap exists to refuse, on a body the
/// handler will discard. Every ordinary client sends a length. Recorded as a
/// limitation rather than left to be rediscovered.
pub const SERVING_MAX_BODY_BYTES: u64 = 64 * 1024;

/// Build the serving envelope for a bound that fired, with a fresh correlation
/// id and an optional `Retry-After`.
///
/// # It records the metric too, and that is the point of doing it here
///
/// A bound fires OUTSIDE `serve`, so none of the three refusals reaches
/// `AccessLogStore::record` — see the module comment. Left there, each new bound
/// would add a fresh instance of the blind spot the design doc's §5 describes:
/// an answer no instrument can see. `super::metrics::record_request` is the same
/// function the handler calls with the same two labels, so
/// `egret_dataapi_requests_total{code="overloaded"}` sits beside
/// `{code="rate_limited"}` on one dashboard and needs no second panel.
///
/// # The answer comes from [`to_wire`], and there is no second way to build one
///
/// This function used to take a status, a code literal, a message and a
/// `Retry-After` and assemble the envelope itself. Four of the five things that
/// distinguish one serving refusal from another were therefore chosen here, in
/// argument position, next to an exhaustive match whose whole purpose is that
/// they cannot be. Deleting an arm of `to_wire` did not break this module, and
/// that is precisely how `deadline_exceeded` spent its first life sharing the
/// string `timeout` with [`ServingError::EngineTimeout`].
///
/// Now the only argument is which bound fired. `to_wire` decides the status, the
/// code, the message and the header; [`super::error::Wire::into_response`]
/// renders it, the same call `serve` makes. Remove the `ServingDeadline` arm and
/// this file stops compiling.
///
/// # The `route` argument, and why a bound is the one place it is derived
///
/// These three layers wrap the WHOLE mount, so a bound can refuse either of its
/// two routes — an endpoint call or a contract fetch — and neither handler has
/// run to say which. The two handlers name their own
/// [`super::metrics::Route`]; here it is read off the request path by
/// [`super::metrics::Route::of_path`], whose rule is the routing table's own.
/// Left to a constant, every shed and every deadline on a contract fetch would
/// be counted as endpoint-call traffic, which is the conflation the label was
/// added to end.
fn bound_response(route: super::metrics::Route, e: ServingError, elapsed: Duration) -> Response {
    let wire = to_wire(&e);
    super::metrics::record_request(route, wire.status, wire.code, elapsed);
    // Minted here rather than threaded in — see the module comment on what a
    // `request_id` from a bound means.
    wire.into_response(&Uuid::new_v4().to_string())
}

/// What a bound may put in a log line, and it is the path — never the URI.
///
/// # A parameter value must not escape here
///
/// `Uri`'s `Display` writes the query string, and on this surface the query
/// string **is the caller's parameter values**: `?customer_email=…&region=…`.
/// Every other instrument on the serving path is already path-only —
/// `dataapi.access_log.request_path` stores `path.1`, and no other serving log
/// line records a URI at all — and the error taxonomy in [`super::error`] is
/// deliberately built so that no parameter name, bound or constraint kind
/// reaches a caller. A WARN line that prints the whole query undoes that
/// property from the side an operator, a log shipper and a support ticket all
/// read.
///
/// The path alone identifies the endpoint, which is the only thing any of these
/// three lines is diagnosing.
fn logged_path(req: &Request) -> String {
    req.uri().path().to_string()
}

/// Answer `504` when a request outruns `deadline`.
///
/// `deadline` is a parameter rather than a read of [`SERVING_REQUEST_DEADLINE`]
/// so a test can drive a bound it genuinely exceeds. A test that waited out the
/// production 50 s would be a 50-second test; one that asserted the layer was
/// *installed* would pass with the layer doing nothing, which is the failure
/// mode this repo has shipped repeatedly.
///
/// # This one stays at WARN
///
/// The other two bounds refuse before doing any work and were dropped to DEBUG
/// (see [`serving_admit`]). A deadline that fires has already spent the whole
/// budget, so one line per event costs nothing next to what produced it, and it
/// is the only instrument that sees the event at all — the `access_log` insert
/// dies with the cancelled future.
pub async fn serving_deadline(deadline: Duration, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = logged_path(&req);
    let started = std::time::Instant::now();
    match tokio::time::timeout(deadline, next.run(req)).await {
        Ok(resp) => resp,
        Err(_) => {
            tracing::warn!(
                %method, %path, deadline_secs = deadline.as_secs(),
                "data-api serving request exceeded its deadline; answering 504 \
                 (no access_log row is written for a cancelled request)"
            );
            bound_response(
                super::metrics::Route::of_path(&path),
                ServingError::ServingDeadline,
                started.elapsed(),
            )
        }
    }
}

/// The bulkhead's semaphore. Constructed **once, with the router**: built per
/// request it would be a limit nobody could ever reach, which is the same
/// mistake `DataApiState`'s cache and token buckets are constructed in
/// `egret-bin` to avoid.
pub fn serving_bulkhead(permits: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(permits))
}

/// Answer `503 overloaded` when `sem` has no permit left.
///
/// **Sheds; it does not queue.** `tower`'s `ConcurrencyLimitLayer` would make
/// the 65th request *wait*, which under sustained overload converts a fast
/// refusal into an unbounded queue whose members all time out at
/// [`SERVING_REQUEST_DEADLINE`] having consumed a connection each. A caller told
/// `503, Retry-After: 1` can back off; a caller left in a queue cannot.
///
/// # The shed logs at DEBUG, and that is a decision
///
/// It was a WARN. A shed costs one failed `try_acquire` — it is the cheapest
/// thing this process can do — and it happens precisely when the process has no
/// capacity to spare, so one formatted, allocated, shipped log line per refusal
/// made the cheapest path the most expensive one exactly under overload, and
/// turned a burst into a second burst on the log pipeline. Sampling was the
/// other candidate and needs state (a counter, a window) for an event that
/// **already has a counter**: `egret_dataapi_requests_total{code="overloaded"}`
/// is incremented by [`bound_response`] on every shed, it is a rate an alert can
/// read, and it does not grow with volume. So the metric is the instrument and
/// the line is for someone who has turned `debug` on for this target.
pub async fn serving_admit(sem: Arc<Semaphore>, req: Request, next: Next) -> Response {
    let Ok(_permit) = sem.try_acquire_owned() else {
        tracing::debug!(
            method = %req.method(), path = %logged_path(&req),
            "data-api serving bulkhead full; shedding with 503 overloaded"
        );
        // A shed costs one failed `try_acquire`. The elapsed time is recorded as
        // zero rather than measured, because a latency histogram that mixes
        // refusals in with served calls reports a system getting FASTER as it
        // falls over.
        return bound_response(
            super::metrics::Route::of_path(req.uri().path()),
            ServingError::Overloaded,
            Duration::ZERO,
        );
    };
    next.run(req).await
}

/// Answer `413` when a request declares a body larger than `max_bytes`.
///
/// DEBUG for the same reason as [`serving_admit`]: this refusal is a header read
/// and nothing else, it is trivially repeatable by anyone who can reach the
/// surface, and it is counted as
/// `egret_dataapi_requests_total{code="payload_too_large"}`.
pub async fn serving_body_limit(max_bytes: u64, req: Request, next: Next) -> Response {
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max_bytes) {
        tracing::debug!(
            method = %req.method(), path = %logged_path(&req), declared,
            "data-api serving request declared a body over the cap; answering 413"
        );
        // Same reasoning as the shed above: a header read, not a served request,
        // so it must not pull the latency histogram down.
        return bound_response(
            super::metrics::Route::of_path(req.uri().path()),
            ServingError::PayloadTooLarge,
            Duration::ZERO,
        );
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    /// The documented limitation, pinned so it cannot become a silent one.
    ///
    /// A request with no `Content-Length` passes this layer. That is deliberate
    /// — see [`SERVING_MAX_BODY_BYTES`] — and it is the sort of gap that gets
    /// rediscovered as a surprise years later. If someone closes it, this test
    /// fails and they update the constant's doc with it.
    #[tokio::test]
    async fn a_request_with_no_declared_length_is_not_bounded_by_this_layer() {
        let app = Router::new()
            .route("/x", post(|| async { "reached" }))
            .layer(axum::middleware::from_fn(move |req, next| {
                serving_body_limit(4, req, next)
            }));

        // No `Content-Length` header — the shape a chunked sender produces, and
        // the one this layer cannot see.
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/x")
                    .body(Body::from(vec![b'x'; 1024]))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            res.status(),
            StatusCode::OK,
            "the cap is enforced on the DECLARED length; if this now refuses, \
             the limitation in SERVING_MAX_BODY_BYTES's doc is stale"
        );
    }

    /// A bound that fires leaves no `access_log` row, so the metric is the ONLY
    /// instrument that can see it. Asserted on the rendered scrape rather than
    /// on the call, and with a control: a request that is *not* refused must not
    /// produce the same series, or a counter that incremented for everything
    /// would read identically here.
    ///
    /// The recorder is thread-local for the duration of the body, the same
    /// device `tests/serving_metrics.rs` uses and for the same reason —
    /// `metrics`' global recorder can be installed once per process, so a
    /// process-wide one would let this test and its neighbours see each other's
    /// increments.
    #[test]
    fn a_shed_request_is_counted_even_though_it_leaves_no_access_log_row() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        ::metrics::with_local_recorder(&recorder, || {
            let full = serving_bulkhead(0);
            let shedding = Router::new()
                .route("/x", post(|| async { "unreachable" }))
                .layer(axum::middleware::from_fn(move |req, next| {
                    serving_admit(full.clone(), req, next)
                }));
            let open = serving_bulkhead(1);
            let serving = Router::new()
                .route("/x", post(|| async { "served" }))
                .layer(axum::middleware::from_fn(move |req, next| {
                    serving_admit(open.clone(), req, next)
                }));

            rt.block_on(async {
                let req = || {
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/x")
                        .body(Body::empty())
                        .unwrap()
                };
                let shed = shedding.oneshot(req()).await.unwrap();
                assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);
                let ok = serving.oneshot(req()).await.unwrap();
                assert_eq!(ok.status(), StatusCode::OK);
            });
        });

        let scrape = handle.render();
        assert!(
            scrape.contains(r#"code="overloaded""#) && scrape.contains(r#"status="503""#),
            "a shed request must be visible somewhere; it is not in access_log \
             and it is not here either:\n{scrape}"
        );
        // The control. Without it, a counter incremented on every request would
        // satisfy the assertion above.
        assert!(
            !scrape.contains(r#"code="ok""#),
            "the admitted request must not have been counted as an outcome by \
             this layer — `serve` owns that:\n{scrape}"
        );
    }

    /// **The two 504s are told apart at the one instrument that sees them
    /// both.**
    ///
    /// This surface answers `504` for two unrelated reasons: the endpoint
    /// author's published time budget was exceeded
    /// ([`ServingError::EngineTimeout`], code `timeout`) and the request outran
    /// egret's own serving deadline ([`ServingError::ServingDeadline`], code
    /// `deadline_exceeded`). They used to share the string `timeout`.
    ///
    /// `dataapi.access_log` cannot separate them, and never could: the deadline
    /// fires OUTSIDE the handler, so the row is never written — the insert dies
    /// with the cancelled future. `egret_dataapi_requests_total{code=…}` is
    /// therefore the only place both can appear, which makes it the place the
    /// rename has to hold. One string here and an operator cannot separate "we
    /// are cutting callers off" from "their queries are too slow".
    ///
    /// The control is the second assertion: `timeout` must be ABSENT from a
    /// scrape produced by the deadline alone. Without it, a `code` label that
    /// had regressed to `timeout` would still satisfy a test that only looked
    /// for `status="504"`.
    #[test]
    fn the_deadlines_metric_code_is_not_the_engine_timeouts() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        ::metrics::with_local_recorder(&recorder, || {
            let slow = Router::new()
                .route(
                    "/x",
                    post(|| async {
                        std::future::pending::<()>().await;
                        "unreachable"
                    }),
                )
                .layer(axum::middleware::from_fn(move |req, next| {
                    serving_deadline(Duration::ZERO, req, next)
                }));
            rt.block_on(async {
                let res = slow.oneshot(firing_request()).await.unwrap();
                assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
            });
        });

        let scrape = handle.render();
        let want = to_wire(&ServingError::ServingDeadline).code;
        assert!(
            scrape.contains(&format!(r#"code="{want}""#)) && scrape.contains(r#"status="504""#),
            "the serving deadline must be counted under its own code `{want}`:\n{scrape}"
        );
        assert!(
            !scrape.contains(r#"code="timeout""#),
            "`timeout` belongs to the endpoint author's published budget \
             (`EngineTimeout`). The serving deadline appearing under it is the \
             collision this rename removed, and it is invisible in the access \
             log because a cut-off request writes no row:\n{scrape}"
        );
    }

    /// The one relationship that makes [`SERVING_REQUEST_DEADLINE`] a bound
    /// rather than decoration.
    #[test]
    fn request_deadline_is_shorter_than_what_the_ingress_will_wait() {
        assert!(
            SERVING_REQUEST_DEADLINE.as_secs() < INGRESS_READ_TIMEOUT_SECS,
            "a request deadline at or above the ingress's {INGRESS_READ_TIMEOUT_SECS}s is \
             decoration: the edge answers the caller first and egret's 504 is written to a \
             connection nobody is reading"
        );
    }

    /// Build the three bounds, each wired so that driving it once makes it
    /// fire, paired with the [`ServingError`] variant it is supposed to answer
    /// with.
    ///
    /// Shared by the two tests below so they cannot drift into testing two
    /// different sets of bounds.
    fn each_bound_firing() -> Vec<(&'static str, ServingError, Router)> {
        let full = serving_bulkhead(0);
        let shedding = Router::new()
            .route("/x", post(|| async { "unreachable" }))
            .layer(axum::middleware::from_fn(move |req, next| {
                serving_admit(full.clone(), req, next)
            }));
        let capped = Router::new()
            .route("/x", post(|| async { "unreachable" }))
            .layer(axum::middleware::from_fn(move |req, next| {
                serving_body_limit(4, req, next)
            }));
        let slow = Router::new()
            .route(
                "/x",
                post(|| async {
                    std::future::pending::<()>().await;
                    "unreachable"
                }),
            )
            .layer(axum::middleware::from_fn(move |req, next| {
                serving_deadline(Duration::ZERO, req, next)
            }));
        vec![
            ("the bulkhead shed", ServingError::Overloaded, shedding),
            ("the body cap", ServingError::PayloadTooLarge, capped),
            ("the deadline", ServingError::ServingDeadline, slow),
        ]
    }

    /// One request that makes any of the three bounds fire, with a declared
    /// length over the body cap so the same request drives all of them.
    fn firing_request() -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/x")
            .header(header::CONTENT_LENGTH, "1048576")
            .body(Body::empty())
            .unwrap()
    }

    /// Read a refusal's rendered envelope.
    async fn error_body(res: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// **A bound refusal is rendered by the taxonomy, not by this module.**
    ///
    /// # The defect this exists for
    ///
    /// [`bound_response`] used to take a status, a code literal, a message and a
    /// `Retry-After` and assemble the envelope itself, so three of the surface's
    /// answers were decided beside [`to_wire`]'s exhaustive match instead of
    /// inside it. The visible consequence was that this module's 504 and
    /// [`ServingError::EngineTimeout`]'s 504 were both spelled `timeout` — two
    /// different actors, one string, and nothing in the crate could notice,
    /// because only one of them went through the match.
    ///
    /// # What it asserts, and why it is not a string comparison
    ///
    /// The expectation is not written down here. It is `to_wire` applied to the
    /// variant each bound names, compared against the response the bound
    /// actually produced — status line, `error.code`, message and `Retry-After`
    /// together. Change what `to_wire` says about `Overloaded` and this test
    /// follows it; make this module disagree with `to_wire` and it fails.
    ///
    /// The structural half of the claim is not testable from here and does not
    /// need to be: `bound_response` takes a `ServingError`, so deleting an arm
    /// of `to_wire` makes that match non-exhaustive and **the crate stops
    /// compiling**. That is the property the literals in this file destroyed.
    #[tokio::test]
    async fn each_bound_answers_exactly_what_the_taxonomy_says_for_its_variant() {
        for (what, variant, app) in each_bound_firing() {
            let want = to_wire(&variant);
            let res = app.oneshot(firing_request()).await.unwrap();

            assert_eq!(
                res.status(),
                want.status,
                "{what} must answer the status `to_wire` gives its variant"
            );
            assert_eq!(
                res.headers().get(header::RETRY_AFTER).map(|v| v
                    .to_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()),
                want.retry_after,
                "{what} must carry the `Retry-After` the taxonomy decided, or \
                 none — the three meanings of that header are reconciled in \
                 `serving::error` and nowhere else"
            );

            let body = error_body(res).await;
            assert_eq!(
                body["error"]["code"], want.code,
                "{what} must answer with the code `to_wire` gives its variant. A \
                 literal here is how the serving deadline and the endpoint's \
                 published budget came to share the string `timeout`"
            );
            assert_eq!(
                body["error"]["message"],
                want.message.as_ref(),
                "{what}'s message is the taxonomy's too — this surface's \
                 messages are deliberately leak-proof, and a second wording here \
                 is a second thing to audit"
            );
            assert!(
                body["error"]["request_id"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
                "{what} must still carry a correlation id: {body}"
            );
        }
    }

    /// The seam between what a bound answers and what the per-endpoint OpenAPI
    /// document promises.
    ///
    /// `openapi_gen` builds the document a consuming team generates a client
    /// from, and it is NOT the repo's root `openapi.json` — the root document is
    /// built from `#[utoipa::path]` on the handlers and cannot see a status
    /// produced by a layer wrapped around them. So `413` and `504` were
    /// reachable and undeclared while the root file's diff stayed at zero lines.
    ///
    /// The declared set is no longer a hand-written const in `openapi_gen`. It
    /// is [`crate::serving::error::wire_answers`] — `to_wire` over the taxonomy
    /// census — which is the same derivation `openapi_gen::error_responses`
    /// renders from. This drives each bound for real and requires the **status
    /// and the code it actually answered with** to be in it: a status alone
    /// would pass for a bound that answered 504 with the wrong one of the two
    /// codes that status carries.
    #[tokio::test]
    async fn every_answer_a_bound_gives_is_in_the_derived_contract() {
        let declared = crate::serving::error::wire_answers();

        for (what, _, app) in each_bound_firing() {
            let res = app.oneshot(firing_request()).await.unwrap();
            let status = res.status().as_u16();
            let body = error_body(res).await;
            let code = body["error"]["code"].as_str().unwrap().to_string();

            let codes = declared.get(&status).unwrap_or_else(|| {
                panic!(
                    "{what} answers {status}, which the taxonomy derivation does \
                     not produce — so `openapi_gen` cannot declare it and a \
                     generated client meets a status its contract never named. \
                     Declared: {declared:?}"
                )
            });
            assert!(
                codes.contains(code.as_str()),
                "{what} answers {status} with `{code}`, which is not one of the \
                 codes the derivation says that status carries ({codes:?}). The \
                 consumer document names its codes per status, so a client told \
                 to branch on `error.code` would meet a value its contract never \
                 listed"
            );
        }
    }

    /// The console warns an author when their endpoint's worst case exceeds
    /// this deadline, and it has to be THIS deadline it warns about.
    ///
    /// # Why the number is duplicated at all
    ///
    /// Nothing serves `SERVING_REQUEST_DEADLINE` to the browser: it is a
    /// process constant, not part of any contract a client fetches, and adding
    /// an endpoint for one integer would be a larger change than the warning it
    /// feeds. So `Config.tsx` holds a copy — and a copy nobody checks is how the
    /// two sides of a seam drift, which is the failure mode this repo keeps
    /// shipping. Same device as
    /// `ingress_read_timeout_is_the_number_this_module_derives_from` directly
    /// below, pointed at the other side of the stack.
    ///
    /// If someone changes the deadline, this fails and they update the console
    /// copy with it. If someone changes the console copy, this fails and they
    /// find out why the number is what it is.
    #[test]
    fn the_config_surface_warns_against_this_modules_deadline() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/src/routes/explore/dataapi/detail/Config.tsx"
        );
        let source = std::fs::read_to_string(path).expect("Config.tsx missing");
        let needle = format!(
            "const SERVING_REQUEST_DEADLINE_SECONDS = {};",
            SERVING_REQUEST_DEADLINE.as_secs()
        );
        assert!(
            source.contains(&needle),
            "the Data API Config surface must warn against {}s (looked for \
             `{needle}`). An author is told there whether their endpoint's worst \
             case outruns a served call; a stale number there is a warning that \
             fires at the wrong threshold, which is worse than none",
            SERVING_REQUEST_DEADLINE.as_secs()
        );
    }

    /// The chart and this module must agree about the 60. Reads the template
    /// rather than trusting the comment, because the number used to be
    /// ingress-nginx's silent default and a chart edit could make it one again.
    #[test]
    fn ingress_read_timeout_is_the_number_this_module_derives_from() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/helm/egret/templates/ingress.yaml"
        );
        let chart = std::fs::read_to_string(path).expect("ingress.yaml missing");
        let needle = format!(
            "nginx.ingress.kubernetes.io/proxy-read-timeout: \"{INGRESS_READ_TIMEOUT_SECS}\""
        );
        assert!(
            chart.contains(&needle),
            "the chart must pin the ingress read timeout to {INGRESS_READ_TIMEOUT_SECS}s \
             (looked for `{needle}`); SERVING_REQUEST_DEADLINE is derived from it"
        );
    }
}
