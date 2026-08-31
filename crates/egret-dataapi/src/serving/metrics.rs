//! The serving path's Prometheus metrics — the counters an operator can scrape
//! without holding a workspace.
//!
//! # The one thing here that is not ordinary good practice
//!
//! [`PREAUTH_REJECTIONS_TOTAL`] counts the calls `dataapi.access_log`
//! **structurally cannot record**. `access_log.workspace_id` is `NOT NULL` and
//! is only known once the bearer token verifies, so
//! [`super::access_log::AccessLogStore::record`] drops a row it has no workspace
//! for — deliberately, and for a good reason spelled out in that module's
//! comment (writing the URL's unverified slug would let an unauthenticated
//! caller forge rows against any tenant whose slug they can guess).
//!
//! The consequence is an outage nothing in this system can see. If the token
//! endpoint breaks or the signing key is rotated out from under every issued
//! token, **every** call is refused before a workspace is resolved. No
//! `access_log` row is written, so the per-workspace `error_rate` the activity
//! statistics compute is `None` rather than 1.0 and no alert fires; the refusals
//! log at `debug` on purpose, to keep an unauthenticated flood from flooding the
//! log with it. The system goes completely dark and `/healthz` still answers
//! `ok`.
//!
//! A counter is the right instrument for that specific hole and a row is not:
//! it needs no workspace, it costs nothing per call, and it cannot be forged by
//! a caller because it carries no caller-supplied value at all. It has no
//! labels for exactly that reason — see the cardinality section.
//!
//! It does NOT replace the access log. The log is the per-workspace,
//! per-endpoint, per-request record; these are process-wide rates. The one
//! question only the counter answers is "is anything getting through at all",
//! and the one question only the log answers is "whose, and which endpoint".
//!
//! ## What "the counter is the table's complement" is, exactly
//!
//! It is a property of ONE expression, not of two that happen to agree.
//! [`super::access_log::AccessLogStore::record`] returns
//! [`super::access_log::Recorded`], and the serving path counts whichever
//! no-row variant it was handed. There is no second copy of the "does this call
//! produce a row" predicate to drift out of step with the first — which is what
//! there was, and what the comment beside it overstated.
//!
//! What that does and does not guarantee, stated plainly because this is the
//! kind of claim that ages badly:
//!
//! * **Guaranteed by the types.** A new early return added to `record` cannot
//!   compile without naming a [`super::access_log::Recorded`] variant, so it
//!   cannot silently leave a call uncounted. A caller cannot ignore the answer
//!   either — the type is `#[must_use]`.
//! * **NOT guaranteed by the types.** Someone could return
//!   `Recorded::Row` from a path that wrote nothing. Nothing in Rust catches
//!   that, so `record_reports_a_row_exactly_when_one_exists` in
//!   `tests/serving_metrics.rs` asserts the biconditional against the real
//!   table, over a table of entry shapes rather than for one case.
//!
//! ## The second no-row path, and why it gets its own counter
//!
//! A pre-authentication refusal is not the only way a call leaves no row: the
//! INSERT itself can fail, which `record` logs at `warn` and swallows. That hole
//! has the same shape — no row, so `error_rate` stays `None` — but a different
//! cause and a different fix, so it is
//! [`ACCESS_LOG_WRITE_FAILURES_TOTAL`] and not a second reason for the pre-auth
//! counter to move. Folding the two would make an alert on
//! `egret_dataapi_preauth_rejections_total` fire for a metastore problem while
//! saying "authentication is refusing everything", which is worse than not
//! alerting: it is alerting with the wrong noun.
//!
//! # Cardinality is a correctness property, not a style note
//!
//! A Prometheus series exists for every distinct combination of label values,
//! forever (until the retention window rolls). A label whose value a CALLER
//! chooses — a request path, a parameter value, a client id, a token subject —
//! is unbounded by construction, and one hostile or merely careless client turns
//! the scrape into an out-of-memory. That is a worse failure than the blindness
//! this module exists to fix, because it takes the metrics of every other
//! subsystem down with it.
//!
//! So the whole label vocabulary is:
//!
//! | metric | labels | why the value set is bounded |
//! |---|---|---|
//! | `egret_dataapi_requests_total` | `code`, `status` | `code` is [`super::error::Wire::code`], typed `&'static str` and written as a literal in every arm of `to_wire`'s exhaustive match over `ServingError` — a runtime string cannot reach it without leaking one. `status` is mapped through [`status_label`], whose fallback arm is `"other"`. |
//! | `egret_dataapi_request_duration_seconds` | `status` | as above. |
//! | `egret_dataapi_cache_lookups_total` | `result` | `"hit"` or `"miss"`; nothing else is constructible. |
//! | `egret_dataapi_preauth_rejections_total` | *(none)* | it is reached before anything about the caller has been verified, so there is no value here that is not the caller's own word. |
//! | `egret_dataapi_access_log_write_failures_total` | *(none)* | the failing thing is the metastore, not the call; the sqlx error's text is a runtime string and would be unbounded. |
//! | `egret_dataapi_store_calls_total` | `store`, `op`, `result` | all three come from [`StoreOp::labels`] and a `bool` — three combinations of the first two, two of the third, and nothing else is constructible. The redis error's text is deliberately NOT a label: it is a runtime string, and it is in the log line instead. |
//! | `egret_dataapi_store_duration_seconds` | `store`, `op` | as above. |
//!
//! `code` and `status` are both present and that costs nothing: `code`
//! determines `status` (each arm of `to_wire` pairs one with one), so the two
//! together produce the same number of series as `code` alone rather than their
//! product. `status` earns its place by making `rate(...{status="500"})` a query
//! an operator can write without memorising egret's code table.
//!
//! ## What is deliberately NOT a label
//!
//! - **`endpoint_id` / `workspace_id`.** Bounded per deployment, but bounded by
//!   *what tenants publish*, which is not a number this crate controls and which
//!   only grows. Per-endpoint call counts, error rates, p50/p95 and cache hit
//!   rate already exist, computed over `dataapi.access_log` by the management UI
//!   — so the label would buy a duplicate of a capability the system has, at a
//!   cost that scales with tenant count.
//! - **The request path.** Caller-supplied and unbounded; a 404-scanner would
//!   mint a series per URL it guessed. `request_path` is recorded in the access
//!   log, which is a table and can hold it.
//! - **`client_id`, the token subject, parameter values.** Caller-supplied.
//! - **The error MESSAGE, or any parameter name.** [`super::error`] is
//!   deliberately leak-proof: a serving 400 names no parameter, no bound and no
//!   constraint kind, because the parameter list is the endpoint author's and
//!   naming it turns the error surface into a directory of what an endpoint
//!   declares. A metric is a *more* public surface than an error body — one
//!   scrape returns every label combination the process has ever seen — so
//!   labelling with anything but the stable code would make the metric the leak
//!   the error body was careful to avoid.
//!
//! # Cost on the request path
//!
//! Every label value handed to the macros below is `&'static str`, so no label
//! allocates. `metrics`' registry is sharded and read-mostly (a hit is a shared
//! read, and only the first sighting of a combination takes a write), so this
//! adds no process-wide mutex to a path that already holds two. The recorded
//! work is a hash and an atomic add, against a request that has already made a
//! network round trip to an engine.

use std::time::Duration;

use axum::http::StatusCode;

use super::access_log::Recorded;

/// One increment per served call, whatever the answer.
///
/// Package 4's refusal is a NEW code on this counter
/// (`code="rate_limiter_unavailable"`) and deliberately not a second reason for
/// `code="rate_limited"` to move: an alert or a dashboard panel written against
/// `rate_limited` means "callers are exceeding their published allowance", and
/// a store outage moving it would report every consuming team as over their
/// limit during a failure of egret's own dependency.
pub const REQUESTS_TOTAL: &str = "egret_dataapi_requests_total";
/// End-to-end latency as the CALLER experienced it, in seconds because that is
/// Prometheus' base unit.
///
/// Literally the same measurement `access_log.duration_ms` records: `serve`
/// takes ONE `started.elapsed()` and feeds both, so the row and the histogram
/// cannot disagree by the cost of whatever ran between them. It used to be two
/// separate readings with this comment claiming otherwise.
pub const REQUEST_DURATION_SECONDS: &str = "egret_dataapi_request_duration_seconds";
/// Hit/miss, counted only where a lookup actually happened.
///
/// **Unchanged in meaning by Package 4**, which is worth saying because the
/// shared store gives a lookup a second way to fail. A store error is counted
/// here as a `miss` — which is what it IS from the endpoint's point of view: the
/// request re-executed. What it is *not* is a cache that was consulted and had
/// nothing; that distinction lives in [`STORE_CALLS_TOTAL`]'s `result="error"`,
/// so an operator seeing the hit rate collapse can tell "the working set turned
/// over" from "the store stopped answering" by looking at exactly one other
/// series.
pub const CACHE_LOOKUPS_TOTAL: &str = "egret_dataapi_cache_lookups_total";
/// The calls refused before a workspace was known, which is the larger of the
/// two ways a call leaves no `access_log` row — see the module comment.
pub const PREAUTH_REJECTIONS_TOTAL: &str = "egret_dataapi_preauth_rejections_total";
/// The other way a call leaves no `access_log` row: the INSERT failed.
///
/// Its own counter rather than a second reason for [`PREAUTH_REJECTIONS_TOTAL`]
/// to move, because the two name different faults — see the module comment.
pub const ACCESS_LOG_WRITE_FAILURES_TOTAL: &str = "egret_dataapi_access_log_write_failures_total";
/// Round trips to the SHARED limiter/cache store (Package 4), by operation and
/// outcome.
///
/// Only [`super::shared::SharedStore`] moves it: an in-process store makes no
/// round trip, so a deployment that has not configured one reports nothing here
/// rather than reporting zeros — the absence of the series is itself the answer
/// to "is this deployment sharing state?".
///
/// The `result="error"` stream is the one an operator alerts on, and it means
/// two different things depending on `store`, which is exactly why the label
/// exists: on `ratelimit` it counts requests that were **refused** because the
/// allowance could not be consulted (fail closed), and on `cache_get`/
/// `cache_put` it counts lookups that silently became misses (fail open). See
/// [`super::shared`] for why the two policies are opposite.
pub const STORE_CALLS_TOTAL: &str = "egret_dataapi_store_calls_total";
/// How long a shared-store round trip took, in seconds.
///
/// Timed around the timeout, so a call that gave up at
/// `shared::STORE_TIMEOUT` records that duration rather than vanishing — a
/// store that has stopped answering must be visible as latency and not only as
/// a missing observation.
pub const STORE_DURATION_SECONDS: &str = "egret_dataapi_store_duration_seconds";

/// Which shared-store round trip this was.
///
/// An enum rather than two `&str` arguments, so the label pair cannot be
/// assembled wrongly at a call site and the value set is closed by the type —
/// the same reason [`super::error::Wire::code`] is `&'static str`. See the
/// cardinality note in this module's comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreOp {
    /// The token-bucket script.
    RateLimit,
    /// A response-cache read.
    CacheGet,
    /// A response-cache write.
    CachePut,
}

impl StoreOp {
    /// `(store, op)` — two labels rather than one, so `sum by (store)` answers
    /// "is the cache healthy" without an operator having to know that a cache
    /// has two operations and the limiter one.
    pub(crate) fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::RateLimit => ("ratelimit", "check"),
            Self::CacheGet => ("cache", "get"),
            Self::CachePut => ("cache", "put"),
        }
    }
}

/// The `code` label's value on a success, where there is no [`super::error::Wire`]
/// to read one off.
///
/// A value rather than an absent label: a series that sometimes carries `code`
/// and sometimes does not cannot be summed with `sum by (code)` without the
/// successes vanishing from the total.
pub(crate) const OK_CODE: &str = "ok";

/// Register names, units and help text with whatever recorder is installed.
///
/// Called once at startup, from the composition root that installs the recorder.
/// It is separate from the increments because `describe_*` overwrites the
/// registered description each call, and calling it per request would be paying
/// for a string write on the hot path to store the same string.
///
/// Without it a scrape carries no `# HELP` line, and the histogram carries no
/// unit — which is how a dashboard ends up plotting seconds as if they were
/// milliseconds.
pub fn describe() {
    ::metrics::describe_counter!(
        REQUESTS_TOTAL,
        "Data API serving requests, by outcome code and HTTP status"
    );
    ::metrics::describe_histogram!(
        REQUEST_DURATION_SECONDS,
        ::metrics::Unit::Seconds,
        "Data API serving request latency as the caller experienced it"
    );
    ::metrics::describe_counter!(
        CACHE_LOOKUPS_TOTAL,
        "Data API serving response-cache lookups, counted only where the endpoint has caching enabled"
    );
    ::metrics::describe_counter!(
        PREAUTH_REJECTIONS_TOTAL,
        "Data API serving calls refused before a workspace was resolved, which dataapi.access_log \
         cannot record because workspace_id is NOT NULL"
    );
    ::metrics::describe_counter!(
        ACCESS_LOG_WRITE_FAILURES_TOTAL,
        "Data API serving calls whose dataapi.access_log INSERT failed, so no row exists for them \
         either"
    );
    ::metrics::describe_counter!(
        STORE_CALLS_TOTAL,
        "Data API serving round trips to the shared limiter/cache store; result=error means a \
         rate-limit refusal (fail closed) or a cache miss (fail open) depending on store"
    );
    ::metrics::describe_histogram!(
        STORE_DURATION_SECONDS,
        ::metrics::Unit::Seconds,
        "Data API serving shared-store round-trip latency, including calls that timed out"
    );
}

/// The HTTP status as a label, from a closed set.
///
/// The listed codes are exactly the ones `to_wire` can produce plus the `200`
/// of a served call. The `_ => "other"` arm is what makes the label BOUNDED
/// rather than merely small: a status added to the taxonomy later shows up as
/// `other` and gets fixed, instead of quietly growing the series count.
///
/// Written as a match returning `&'static str` rather than
/// `StatusCode::as_str()` because that method borrows from its receiver, so
/// using it would force an allocation per request to get a value the label can
/// hold.
pub(crate) fn status_label(status: StatusCode) -> &'static str {
    match status.as_u16() {
        200 => "200",
        400 => "400",
        401 => "401",
        403 => "403",
        404 => "404",
        // The request-bounds layer's body cap.
        413 => "413",
        // Package 7's `version_unavailable`: the pinned version is not one this
        // endpoint can serve right now. A status of its own so an operator can
        // see "consumers are pinned to versions that have gone" without it
        // hiding inside the 400s a malformed header also produces.
        409 => "409",
        429 => "429",
        500 => "500",
        // The engine could not be reached.
        502 => "502",
        503 => "503",
        // Two of them: the endpoint's published budget (`timeout`) and egret's
        // own serving deadline (`deadline_exceeded`). The `code` label is what
        // separates those two; this one only has to stop being `other`.
        504 => "504",
        _ => "other",
    }
}

/// One served call: the counter and the latency histogram, together, so the two
/// cannot disagree about how many requests there were.
///
/// `code` is `&'static str` on purpose — it is the type of
/// [`super::error::Wire::code`], and taking `&str` here would let a future
/// caller pass an interpolated message and blow the series count up.
pub(crate) fn record_request(status: StatusCode, code: &'static str, duration: Duration) {
    let status = status_label(status);
    ::metrics::counter!(REQUESTS_TOTAL, "code" => code, "status" => status).increment(1);
    ::metrics::histogram!(REQUEST_DURATION_SECONDS, "status" => status)
        .record(duration.as_secs_f64());
}

/// A response-cache lookup that happened.
///
/// Recorded at the lookup site rather than from `meta.cached` in the handler,
/// because those two are not the same question: an endpoint with caching
/// disabled consults nothing, and counting its every success as a "miss" would
/// put a hit rate on a dashboard for an endpoint that has no cache.
pub(crate) fn record_cache_lookup(hit: bool) {
    ::metrics::counter!(CACHE_LOOKUPS_TOTAL, "result" => if hit { "hit" } else { "miss" })
        .increment(1);
}

/// One call that left no `access_log` row, counted under the reason there is
/// none.
///
/// Takes the [`Recorded`] the write itself returned rather than re-deciding
/// from the entry, so the counters are the table's complement by construction
/// rather than by two call sites agreeing — see the module comment for what that
/// does and does not guarantee.
///
/// Neither counter has labels and neither may gain one: at a pre-auth refusal
/// everything known is the caller's unverified word, and the write failure's
/// only distinguishing value is a sqlx error string.
/// One shared-store round trip: the counter and the latency histogram, together.
///
/// `ok` rather than a `Result` because the two failure shapes a store call has —
/// an error reply and a timeout — are the SAME outcome to both failure policies,
/// and giving them two label values would invite one of them to be treated as
/// admissible later. The distinction is in the log line, where it belongs.
///
/// The duration is recorded on both arms. A store that has stopped answering
/// shows up as `STORE_TIMEOUT`-long observations on the histogram, so a
/// dashboard sees the outage as latency and not as a gap.
pub(crate) fn record_store_call(op: StoreOp, ok: bool, duration: Duration) {
    let (store, operation) = op.labels();
    ::metrics::counter!(
        STORE_CALLS_TOTAL,
        "store" => store,
        "op" => operation,
        "result" => if ok { "ok" } else { "error" },
    )
    .increment(1);
    ::metrics::histogram!(STORE_DURATION_SECONDS, "store" => store, "op" => operation)
        .record(duration.as_secs_f64());
}

pub(crate) fn record_access_log_outcome(recorded: Recorded) {
    match recorded {
        Recorded::Row => {}
        Recorded::NoWorkspace => ::metrics::counter!(PREAUTH_REJECTIONS_TOTAL).increment(1),
        Recorded::InsertFailed => ::metrics::counter!(ACCESS_LOG_WRITE_FAILURES_TOTAL).increment(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each way a call can leave no `access_log` row moves its OWN counter, and
    /// a row moves neither.
    ///
    /// Driven over every [`Recorded`] variant rather than over the one that
    /// matters, so a variant added later has to be given a home here — a `_ =>`
    /// arm silently mapping a new no-row reason onto the pre-auth counter is the
    /// drift this whole change is about, one level up.
    ///
    /// A real recorder, local to this thread, rendering real exposition text: a
    /// double would assert that a function was called, which is not the claim.
    #[test]
    fn each_no_row_outcome_moves_its_own_counter_and_a_row_moves_neither() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        ::metrics::with_local_recorder(&recorder, || {
            record_access_log_outcome(Recorded::Row);
            let after_row = handle.render();
            assert!(
                !after_row.contains(PREAUTH_REJECTIONS_TOTAL)
                    && !after_row.contains(ACCESS_LOG_WRITE_FAILURES_TOTAL),
                "a written row is not a gap and must move nothing:\n{after_row}"
            );

            record_access_log_outcome(Recorded::NoWorkspace);
            record_access_log_outcome(Recorded::InsertFailed);
            record_access_log_outcome(Recorded::InsertFailed);
            let text = handle.render();

            // Different totals on purpose: equal ones would pass just as well if
            // both names resolved to one counter.
            assert!(
                text.contains(&format!("{PREAUTH_REJECTIONS_TOTAL} 1")),
                "the pre-auth refusal did not move its own counter exactly \
                 once:\n{text}"
            );
            assert!(
                text.contains(&format!("{ACCESS_LOG_WRITE_FAILURES_TOTAL} 2")),
                "the two failed INSERTs are not counted apart from the pre-auth \
                 refusal; an alert on the pre-auth counter would now fire for a \
                 metastore problem while naming authentication:\n{text}"
            );
        });
    }

    /// The status label is a closed set, and the fallback is what closes it.
    ///
    /// The `418` line is the one that matters: it asserts that an unforeseen
    /// status becomes a FIXED string. Deleting the `_ => "other"` arm makes this
    /// file stop compiling (the match is not exhaustive over `u16`), which is
    /// the strongest form this guarantee can take; replacing it with something
    /// that renders the number is what this line catches.
    #[test]
    fn an_unforeseen_status_does_not_become_a_new_label_value() {
        assert_eq!(status_label(StatusCode::IM_A_TEAPOT), "other");
        assert_eq!(status_label(StatusCode::from_u16(599).unwrap()), "other");
        assert_eq!(status_label(StatusCode::PERMANENT_REDIRECT), "other");
    }

    /// Every status the serving taxonomy can produce has its own label, so
    /// `status="500"` means what an operator's alert rule assumes it means.
    ///
    /// **Looped over the taxonomy itself, not over a written-down list.** The
    /// written-down list is what this used to be, and it is why `502` and `504`
    /// — both live since Package 3b — were both silently labelled `other` with
    /// this test green: a spelled-out list can only assert about statuses
    /// somebody remembered to spell out, and the failure it must catch is
    /// exactly the one nobody remembered.
    #[test]
    fn every_status_the_taxonomy_produces_has_its_own_label() {
        use crate::serving::error::to_wire;

        // The success path, which no `ServingError` produces.
        assert_eq!(status_label(StatusCode::OK), "200");

        for (name, e) in crate::serving::census::every_variant() {
            let status = to_wire(&e).status;
            assert_eq!(
                status_label(status),
                status.as_str(),
                "{name} answers {status}, which falls into `other` — an alert \
                 rule on `status` cannot see it, and every such status shares \
                 one series with every other unforeseen one"
            );
        }
    }

    /// Every code `to_wire` can return is reachable as a label, and the whole
    /// set is small, fixed, and **exactly** what is written here.
    ///
    /// This is the cardinality claim stated as an assertion rather than as
    /// prose: the product of the label sets is what bounds the series count, and
    /// `code` is the larger of the two factors. If someone adds a variant whose
    /// wire code is built with `format!`, `Wire::code`'s `&'static str` stops
    /// compiling first.
    ///
    /// # Why this is a set and no longer a count over a hand-written array
    ///
    /// It was `codes.len() == 14` over thirteen hand-listed variants plus
    /// `OK_CODE`, and it was **already stale on `main`**: `governance_stale` was
    /// missing from the list, so the count described a taxonomy the code had
    /// outgrown and the assertion passed anyway. Three more codes would have
    /// slipped past it the same way. A count cannot notice a value it was never
    /// shown; the variants now come from
    /// [`crate::serving::census::every_variant`], which is checked against the
    /// enum itself, and the expectation is the set of strings rather than its
    /// size.
    #[test]
    fn the_code_label_set_is_the_error_taxonomy_and_nothing_else() {
        use crate::serving::error::to_wire;
        use std::collections::BTreeSet;

        let expected: BTreeSet<&'static str> = BTreeSet::from([
            OK_CODE,
            "key_invalid",
            "not_found",
            "data_access_denied",
            "not_granted",
            "param_missing",
            "param_invalid",
            "version_pin_invalid",
            "version_unavailable",
            "row_limit_exceeded",
            "rate_limited",
            "schema_drift",
            "output_naming_collision",
            "cache_scope_stale",
            "governance_stale",
            "param_gate_stale",
            "endpoint_query_failed",
            "timeout",
            "engine_unavailable",
            "deadline_exceeded",
            "overloaded",
            "payload_too_large",
            "rate_limiter_unavailable",
            "internal",
        ]);

        let mut codes: BTreeSet<&'static str> = crate::serving::census::every_variant()
            .iter()
            .map(|(_, e)| to_wire(e).code)
            .collect();
        codes.insert(OK_CODE);

        assert_eq!(
            codes, expected,
            "the `code` label's value set changed; re-read the cardinality note \
             in this module before accepting it, and remember every value here \
             is a Prometheus series that lives until the retention window rolls"
        );
    }
}
