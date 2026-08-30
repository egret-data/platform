//! The serving path's Prometheus metrics — the counters an operator can scrape
//! without holding a workspace.
//!
//! # The one thing here that is not ordinary good practice
//!
//! [`record_pre_auth_rejection`] counts the calls `dataapi.access_log`
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

/// One increment per served call, whatever the answer.
pub const REQUESTS_TOTAL: &str = "egret_dataapi_requests_total";
/// End-to-end latency as the CALLER experienced it — the same measurement
/// `access_log.duration_ms` records, in seconds because that is Prometheus'
/// base unit.
pub const REQUEST_DURATION_SECONDS: &str = "egret_dataapi_request_duration_seconds";
/// Hit/miss, counted only where a lookup actually happened.
pub const CACHE_LOOKUPS_TOTAL: &str = "egret_dataapi_cache_lookups_total";
/// The calls no `access_log` row exists for — see the module comment.
pub const PREAUTH_REJECTIONS_TOTAL: &str = "egret_dataapi_preauth_rejections_total";

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
        429 => "429",
        500 => "500",
        503 => "503",
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

/// A call refused before a workspace was resolved — see the module comment.
///
/// No labels, and it must stay that way: everything known at this point is the
/// caller's unverified word.
pub(crate) fn record_pre_auth_rejection() {
    ::metrics::counter!(PREAUTH_REJECTIONS_TOTAL).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// Spelled out rather than looped over `to_wire` so that a status added
    /// there shows up here as a missing line rather than as a silent `other`.
    #[test]
    fn every_status_the_taxonomy_produces_has_its_own_label() {
        let cases = [
            (StatusCode::OK, "200"),
            (StatusCode::BAD_REQUEST, "400"),
            (StatusCode::UNAUTHORIZED, "401"),
            (StatusCode::FORBIDDEN, "403"),
            (StatusCode::NOT_FOUND, "404"),
            (StatusCode::TOO_MANY_REQUESTS, "429"),
            (StatusCode::INTERNAL_SERVER_ERROR, "500"),
            (StatusCode::SERVICE_UNAVAILABLE, "503"),
        ];
        for (status, expected) in cases {
            assert_eq!(
                status_label(status),
                expected,
                "{status} must have its own label value, not fall into `other`"
            );
        }
    }

    /// Every code `to_wire` can return is reachable as a label, and the whole
    /// set is small and fixed.
    ///
    /// This is the cardinality claim stated as an assertion rather than as
    /// prose: the product of the label sets is what bounds the series count, and
    /// `code` is the larger of the two factors. If someone adds a variant whose
    /// wire code is built with `format!`, `Wire::code`'s `&'static str` stops
    /// compiling first — but if the type is ever widened, this is the line that
    /// notices the count moved.
    #[test]
    fn the_code_label_set_is_the_error_taxonomy_and_nothing_else() {
        use crate::serving::error::to_wire;
        use crate::serving::ServingError;
        use std::collections::BTreeSet;

        let every_variant = [
            ServingError::Unauthorized,
            ServingError::NotFound,
            ServingError::DataAccessDenied { raw: None },
            ServingError::NotGranted,
            ServingError::ParamMissing,
            ServingError::ParamInvalid,
            ServingError::RowLimitExceeded { cap: 1 },
            ServingError::RateLimited {
                retry_after: Duration::from_secs(1),
            },
            ServingError::SchemaDrift,
            ServingError::OutputNamingCollision,
            ServingError::CacheScopeStale,
            ServingError::ParamGateStale,
            ServingError::internal("boom"),
        ];
        let mut codes: BTreeSet<&'static str> =
            every_variant.iter().map(|e| to_wire(e).code).collect();
        codes.insert(OK_CODE);

        assert_eq!(
            codes.len(),
            14,
            "the `code` label's value set changed; re-read the cardinality note \
             in this module before accepting it: {codes:?}"
        );
        assert!(codes.contains("key_invalid"));
        assert!(codes.contains(OK_CODE));
    }
}
