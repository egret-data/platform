//! Spec §7's wire envelope: the fixed code table, and the one function that
//! puts a [`ServingError`] on the wire.
//!
//! ```json
//! { "error": { "code": "row_limit_exceeded", "message": "...", "request_id": "..." } }
//! ```
//!
//! # The rule this module exists to enforce
//!
//! **Nothing from the engine reaches the body.** Trino and StarRocks errors
//! carry table names, column names and sometimes the coordinator's host:port. On
//! a surface authenticated by a machine credential alone, that is a metadata
//! leak — a caller who cannot read a table can still learn it exists, and a
//! caller outside the network can still learn where the engine lives. So every
//! answer here is a FIXED string chosen per variant, and the detail goes to
//! `tracing` against the same `request_id` the caller was handed.
//!
//! `egret-core`'s `IntoResponse for Error` already redacts `Error::Internal` to
//! the literal `"internal error"`
//! (`egret-core/src/http.rs:54`, pinned by
//! `connect_failures_are_internal_and_do_not_leak_the_host`). This module is not
//! a second copy of that guard; it is the guard for a DIFFERENT type, and its
//! job is to not undo the first one — `ServingError` wraps `egret_core::Error`
//! by `Display` and would happily carry the message back out if any arm below
//! rendered its source.
//!
//! # Why the mapper has no wildcard arm
//!
//! [`to_wire`] matches every [`ServingError`] variant by name. A `_ => (500,
//! "internal", …)` arm would be the comfortable thing to write and the exact
//! shape of the defect this surface cannot afford: a future variant added for a
//! specific, actionable condition would silently become a 500, and — worse — a
//! future variant that carried an engine message would inherit whatever the
//! wildcard did with it. Exhaustiveness makes a new variant a compile error at
//! this file, which is where the decision belongs.
//!
//! # Why no parameter name appears in a 400
//!
//! `ParamInvalid` and `ParamMissing` arrived with spec §6.2 step 7, and the rule
//! written here before they existed is what they were built to: the parameter
//! *name* is the endpoint author's, not the caller's. Naming it in a 400 turns
//! the error surface into a directory of which parameters an endpoint declares,
//! which is the same class of oracle the single `Unauthorized` and the single
//! `NotFound` exist to close.
//!
//! It is enforced by the SHAPE of the two variants and not only by the strings
//! below: both are unit variants carrying no parameter, so there is nothing here
//! for a later edit to interpolate out of habit. Whoever decides otherwise has
//! to widen the type first, which makes it a decision rather than a slip.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use super::ratelimit::retry_after_secs;
use super::router::ServingStatus;
use super::ServingError;

/// How long a caller is asked to wait before retrying a 503 caused by published
/// configuration having drifted.
///
/// # This is a floor, not a prediction
///
/// The four 503s — [`ServingError::SchemaDrift`],
/// [`ServingError::OutputNamingCollision`], [`ServingError::CacheScopeStale`]
/// and [`ServingError::ParamGateStale`] — do not self-heal. Each is cleared by a
/// human re-publishing the endpoint, and no duration egret can name is a
/// forecast of when that happens. So the obvious reading is that they should
/// carry no `Retry-After` at all.
///
/// **That reading is wrong, and the reason is what a bare 503 actually does.**
/// Omitting the header does not stop retries; it leaves the interval to the
/// client's default, which for every mainstream retry library means an
/// exponential backoff starting around a second. A fleet of callers on a
/// permanently-drifted endpoint therefore hammers hardest in exactly the state a
/// human has to notice and fix. RFC 9110 §10.2.3 defines `Retry-After` on a 503
/// as "the minimum time the user agent is asked to wait" — a floor, explicitly
/// not a promise that the condition will have cleared. Stating five minutes is
/// therefore both true and strictly quieter than saying nothing.
///
/// # And it is a different thing from the 429's header
///
/// [`ServingError::RateLimited`] carries a `Retry-After` computed from its
/// bucket's refill, and that one IS a prediction: at that instant the refusal
/// genuinely lifts. Same header, same units, two different meanings — which is
/// why this one is a named constant with this comment rather than a number
/// sitting next to `retry_after_secs`.
///
/// Five minutes: long enough that a stuck endpoint is not a load source, short
/// enough that a re-publish is picked up without an operator wondering why
/// clients have not come back.
const CONFIG_DRIFT_RETRY_AFTER_SECS: u64 = 300;

/// How long a caller is asked to wait after being SHED.
///
/// A third meaning for the same header, and the shortest: unlike the
/// config-drift floor above (which waits for a human) and unlike the 429's
/// prediction (which is a bucket refilling at a known rate), a shed clears when
/// whatever filled the surface's permits drains — usually within a request. One
/// second is what the bounds layer sends today; the value lives here now
/// because that layer's envelope is built from this table.
const SHED_RETRY_AFTER_SECS: u64 = 1;

/// Does this engine failure mean "the principal may not read this data"?
///
/// Matched on the message because this function only ever sees what
/// `router::engine_error` could NOT type. Trino's connector does return a
/// typed `egret_core::Error::Forbidden`, but only when the coordinator's own
/// HTTP response is 401/403 — `engine_error` peels that case off first, into
/// [`super::ServingError::DataAccessDenied`] with no `raw`, before this
/// function ever runs. A match here reaches the SAME variant, carrying the
/// message. Everything that reaches here is untyped: a Trino denial that instead
/// surfaces as a query-level failure inside a 200 (`Access Denied: …`), and
/// every StarRocks failure, typed or not — StarRocks' connector has no
/// `Forbidden` path at all. **A substring match is a heuristic and is
/// documented as one** — a false negative merely returns today's generic
/// `internal` error (safe: nothing leaks either way), which is why the match
/// is deliberately narrow rather than clever. A broader match would risk the
/// opposite failure, a false POSITIVE that sends an operator to Ranger for a
/// denial the endpoint's own SQL caused.
///
/// # This heuristic is UNVERIFIED against a live engine
///
/// The strings below were chosen by inspecting Trino's and StarRocks'
/// documented error wording, not by reading a real denial off either
/// connector. This codebase has been wrong three separate times about engine
/// behaviour asserted from the wrong instrument (see
/// `measure-with-the-right-instrument` in project memory), so that gap is
/// recorded here rather than implied by silence. Attempted verification:
/// the local dev Trino coordinator cannot serve ANY Data API request —
/// `deploy/trino/etc/config.properties` sets `required-audience=trino` while
/// egret's engine-token minter signs `aud: "engine"`, so nothing egret mints
/// authenticates to it at all, permission-denied or otherwise; this is a known
/// config defect, not something fixed here. No StarRocks-backed manual
/// integration test exists for the Data API path to fall back to. Whoever next
/// has a live-authenticating Trino or StarRocks handy should run a genuinely
/// unauthorized query through this path, quote the verbatim message, and
/// either confirm a string below matches it or extend the list — the
/// alternative is trusting a heuristic no one has watched fire.
pub(crate) fn is_permission_denied(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("access denied") || m.contains("permission denied") || m.contains("not authorized")
}

/// The serving surface's error envelope (spec §7).
///
/// Named `ServingErrorBody` and not `ErrorBody` on purpose. `egret_core::http`
/// already exports an `ErrorBody`, it is a DIFFERENT shape (`{"error":
/// "<string>"}`), and `egret_server::openapi_doc` merges this crate's serving
/// document with the rest of the API into one OpenAPI document — where two
/// schemas sharing a name silently overwrite each other. A generated client
/// would then be typed against whichever won.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingErrorBody {
    /// Always [`ServingStatus::Error`] here, and DERIVED from the status this
    /// response will actually carry rather than written as a constant — see
    /// [`ServingStatus`]. `status` mirrors the HTTP status; it never replaces
    /// it, and a body saying `"error"` under a `200` is the failure mode the
    /// mirror exists to make unrepresentable.
    pub status: ServingStatus,
    /// The same human-readable string as `error.message`, lifted to where the
    /// success envelope carries it (`null` there) so that `status` and
    /// `message` are readable on EVERY serving response without knowing first
    /// which of the two bodies arrived.
    ///
    /// `error.code` is what a client branches on and is untouched; this is the
    /// human half, and it may be reworded.
    pub message: String,
    pub error: ServingErrorDetail,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingErrorDetail {
    /// The stable, machine-readable code from spec §7. Clients branch on this;
    /// `message` is for humans and may be reworded.
    #[schema(example = "row_limit_exceeded")]
    pub code: String,
    /// A fixed string per code. Never rendered from an internal error — see the
    /// module comment.
    pub message: String,
    /// The correlation id, also written to `dataapi.access_log.request_id` and
    /// to every `tracing` line for this call. Quoting it is how a caller gets
    /// support to the detail they were deliberately not given.
    pub request_id: String,
}

/// One answer, decided but not yet rendered.
///
/// Split out from the response so the mapping is a pure function over
/// [`ServingError`]: the whole table is unit-testable with no router, no
/// database and no request id, and the handler can read `status` and `code` to
/// write the access-log row BEFORE the body is consumed.
pub(crate) struct Wire {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: Cow<'static, str>,
    /// Seconds, when this answer carries the header. Two sources with two
    /// meanings — see [`CONFIG_DRIFT_RETRY_AFTER_SECS`].
    pub retry_after: Option<u64>,
}

/// Spec §7's code table. Exhaustive over [`ServingError`] by design — see the
/// module comment.
///
/// Every `message` here is a literal or built only from values egret itself
/// chose. In particular `RowLimitExceeded` states the cap, which is the
/// endpoint author's published number and is already visible to any caller that
/// can trip it; nothing else interpolates anything.
pub(crate) fn to_wire(e: &ServingError) -> Wire {
    let (status, code, message): (StatusCode, &'static str, Cow<'static, str>) = match e {
        // Every authentication failure is one answer — see `ServingError`'s own
        // comment for why distinguishing them is a free oracle. `key_invalid` is
        // spec §7's name for it.
        ServingError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "key_invalid",
            "authentication failed".into(),
        ),
        // Absent, disabled, another workspace's, or an unparseable path. One
        // answer, so a caller cannot map another tenant's URLs.
        ServingError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "no such endpoint".into(),
        ),
        // ONE code for both recognitions — the typed `Error::Forbidden` and the
        // `is_permission_denied` message match — because a caller can act on
        // neither distinction, and because neither can name a single cause.
        // Trino's connector returns `Forbidden` for the coordinator's own HTTP
        // 401 AND 403 (`connector/trino.rs`'s `send()`), which conflates a
        // Ranger/file-ACL deny with a bad or expired engine credential; that is
        // why this code is not named for Ranger, and why the message names both
        // causes with the remedy for each rather than claiming one it does not
        // have. Reporting any of it as bad input would send the operator to
        // debug the query, which is the whole reason this is not a 400.
        //
        // `raw` is deliberately NOT read: the wire message is fixed regardless
        // of what the engine said, same as `Internal`'s source is never
        // rendered. It exists for the `error = ?e` log line in `serve`'s failure
        // branch, so a false-positive classification still leaves a trace of the
        // real engine text in production.
        ServingError::DataAccessDenied { .. } => (
            StatusCode::FORBIDDEN,
            "data_access_denied",
            "the service account running this endpoint was rejected by the engine; \
             check that it is granted the underlying data (in Ranger, for example) \
             and that its engine credential is valid"
                .into(),
        ),
        // Same status as the row above, a DIFFERENT code, because the fix is in
        // a different system: this one is cleared by granting the service
        // account invoke on the endpoint, that one by a Ranger grant on the
        // data. A client branches on `code`, and an operator reading
        // `access_log.error_code` needs the two separable — "403s on this
        // endpoint" is not an actionable statement if it means either.
        //
        // The message names the endpoint's OWNER as the person to ask rather
        // than describing the grant mechanics, because the caller is a machine
        // client in someone else's organisation: it cannot write the grant, and
        // spelling out where the row goes tells it about egret's authorization
        // model for nothing.
        ServingError::NotGranted => (
            StatusCode::FORBIDDEN,
            "not_granted",
            "this service account is not granted access to this endpoint; \
             ask the endpoint's owner to grant it"
                .into(),
        ),
        // The two step-7 codes. NEITHER names the parameter, and neither
        // variant carries one to name — see the module comment above and
        // `ServingError::ParamMissing`'s own. The messages differ only in which
        // fix they point at ("send it" vs "send something else"), which is the
        // whole distinction a client can act on without being told the
        // endpoint's parameter list.
        ServingError::ParamMissing => (
            StatusCode::BAD_REQUEST,
            "param_missing",
            "a required parameter is missing".into(),
        ),
        ServingError::ParamInvalid => (
            StatusCode::BAD_REQUEST,
            "param_invalid",
            "a parameter value is not valid for this endpoint".into(),
        ),
        // An error, never a truncated 200: a client that believes it has every
        // row produces wrong numbers with no signal.
        ServingError::RowLimitExceeded { cap } => (
            StatusCode::BAD_REQUEST,
            "row_limit_exceeded",
            format!("result exceeded the {cap}-row cap").into(),
        ),
        ServingError::RateLimited { .. } => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests".into(),
        ),
        // The three 503s. All three mean "the published configuration no longer
        // describes reality"; none is an egret fault, and each names the action
        // that clears it, because the person who reads this message is the only
        // one who can.
        ServingError::SchemaDrift => (
            StatusCode::SERVICE_UNAVAILABLE,
            "schema_drift",
            "this endpoint's published output schema no longer matches its data; \
             re-publish it"
                .into(),
        ),
        ServingError::OutputNamingCollision => (
            StatusCode::SERVICE_UNAVAILABLE,
            "output_naming_collision",
            "this endpoint's output naming maps two columns onto one key; \
             change it and re-publish"
                .into(),
        ),
        // BOTH fixes are named. Spec §7 says the operator can re-publish the
        // endpoint OR put the connection back, and the message used to offer
        // only the first — sending someone to re-publish when the real event was
        // a connection being changed, which re-publishing does not undo and may
        // not even be the change they want to keep.
        //
        // **And BOTH halves of the pair are named, which is the correction that
        // matters most on this line.** This error fires when EITHER the frozen
        // `delegated` or the frozen `options.impersonate` disagrees with the live
        // row. The message used to say only "delegation setting" — so an
        // operator hitting the impersonate case would go and look at
        // `delegated`, find it unchanged, and reasonably conclude the error was
        // wrong. That is the worst outcome available for a message whose only
        // job is to point at the thing that moved: it does not merely fail to
        // help, it actively sends them away from the setting they changed. The
        // `tracing::warn!` in `shared_cache_allowed` names which half moved; the
        // body cannot (it is a fixed string on a public surface), so it names
        // both.
        ServingError::CacheScopeStale => (
            StatusCode::SERVICE_UNAVAILABLE,
            "cache_scope_stale",
            "this endpoint's shared cache was published against different \
             connection settings; re-publish it, or restore the connection's \
             `delegated` and `options.impersonate` settings"
                .into(),
        ),
        // Names all three fixes, and deliberately does NOT say "re-publish".
        // Every other 503 on this list is cleared by a re-publish and this one
        // is not: `check_governed_connection` refuses the publish under exactly
        // the condition that produced this error, so an operator sent to
        // re-publish would be refused with the same sentence. The three actions
        // that do clear it are the three named here.
        ServingError::GovernanceStale => (
            StatusCode::SERVICE_UNAVAILABLE,
            "governance_stale",
            "this endpoint's database is marked `governed`, so its queries must run as the \
             caller; turn on the connection's `delegated` or `options.impersonate` setting, \
             move the endpoint to a connection that has one, or clear `governed` on the \
             database"
                .into(),
        ),
        // Names the fix and nothing else. The operator cannot act on WHICH rule
        // the old snapshot never met — the rules are the author's, and the list
        // changes — so the message states the one action that clears it.
        ServingError::ParamGateStale => (
            StatusCode::SERVICE_UNAVAILABLE,
            "param_gate_stale",
            "this endpoint's live version was published before its parameters were validated; \
             re-publish it"
                .into(),
        ),
        // The author's endpoint is broken, not egret and not the caller's
        // request. A 5xx because the caller did nothing wrong (spec §7's
        // requirement (c)), and its OWN code because the whole point is that
        // `access_log.error_code` can now separate "which endpoints are failing"
        // from "egret is failing".
        //
        // The message says which of the two it is and who fixes it, and it says
        // it WITHOUT the engine's words: `raw` is not read here, exactly as
        // `Internal`'s source is not, because the engine's text names tables and
        // columns and this surface is authenticated by a machine credential
        // alone. The endpoint's OWNER is named rather than the mechanics of a
        // fix, for the same reason `not_granted` names them: the caller is
        // somebody else's client and can act on none of it.
        ServingError::EndpointQueryFailed { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "endpoint_query_failed",
            "this endpoint's published statement was rejected by the engine; \
             the request was fine — ask the endpoint's owner to check it"
                .into(),
        ),
        // Spec §7's `timeout`. The endpoint works; its published budget did not
        // fit this query. A caller CAN act on this one — retrying a narrower
        // request may succeed — but no `Retry-After` is offered, because egret
        // has no idea when (or whether) the same query would fit.
        ServingError::EngineTimeout => (
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "this endpoint's query exceeded its published time budget".into(),
        ),
        // Spec §7's `engine_unavailable`. The one code here that is an
        // OPERATOR's, and the message says so without naming the engine, its
        // host or its port — that detail is precisely what a 502 body must not
        // teach a caller outside the network.
        ServingError::EngineUnavailable { .. } => (
            StatusCode::BAD_GATEWAY,
            "engine_unavailable",
            "the query engine behind this endpoint could not be reached".into(),
        ),
        // **The three the request-bounds layer answers**, decided here rather
        // than where they fire. `serving/guard.rs` (Package 3a) runs OUTSIDE the
        // handler — a deadline, a bulkhead and a body cap — so it builds its
        // envelope from a `ServingError` it never constructs a handler for. The
        // decision still belongs in this match: it is the only place in the
        // crate where a status and a code are chosen together, and three codes
        // built elsewhere are three codes the taxonomy's tests, the metrics
        // module's cardinality bound and the OpenAPI document cannot see.
        //
        // `deadline_exceeded`, NOT `timeout`. Both are 504s and the bounds layer
        // originally called this one `timeout` as well — one string, one status,
        // two different actors: egret's own serving clock here, the endpoint
        // author's published budget in `EngineTimeout`.
        //
        // **And the place they would have collided is the `code` metric label,
        // not `access_log.error_code`.** This comment used to name the access
        // log, which cannot be right: the bounds layer fires OUTSIDE the
        // handler, so a request it cuts off leaves no `dataapi.access_log` row
        // at all — the two 504s can never meet in that column, because only one
        // of them ever reaches it. Where they do meet is
        // `egret_dataapi_requests_total{code=…}` (`serving::metrics`), which is
        // the only instrument that sees a shed or a deadline at all, and the
        // `error.code` in the body a client branches on. One string there and an
        // operator could not have separated "we are cutting callers off" from
        // "their queries are too slow" — the exact class of question this
        // package split `internal` to make answerable.
        ServingError::ServingDeadline => (
            StatusCode::GATEWAY_TIMEOUT,
            "deadline_exceeded",
            "this request exceeded the serving deadline".into(),
        ),
        // egret refusing egret's own work, and the only 503 here that is not
        // configuration drift: it clears by itself, which is why it carries a
        // one-second `Retry-After` rather than the five-minute floor. Sheds
        // rather than queues, so a caller told this can back off; that argument
        // lives with the middleware.
        ServingError::Overloaded => (
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded",
            "too many requests are in flight; retry shortly".into(),
        ),
        // The one refusal in the table that really is the caller's own doing,
        // and the only 413. The cap itself is not named: it is egret's number,
        // not the endpoint author's, and a caller cannot act on it beyond
        // sending less.
        ServingError::PayloadTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "request body is larger than this surface accepts".into(),
        ),
        // The source is NOT rendered. It is the only thing in this file that
        // could carry a table name or a host, and it goes to `tracing` instead.
        ServingError::Internal(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error".into(),
        ),
    };

    let retry_after = match e {
        // A prediction: at this instant the bucket has refilled.
        ServingError::RateLimited { retry_after } => Some(retry_after_secs(*retry_after)),
        // A floor, not a prediction.
        ServingError::SchemaDrift
        | ServingError::OutputNamingCollision
        | ServingError::CacheScopeStale
        | ServingError::GovernanceStale
        | ServingError::ParamGateStale => Some(CONFIG_DRIFT_RETRY_AFTER_SECS),
        // A prediction again, and a short one: the permits drain on their own.
        ServingError::Overloaded => Some(SHED_RETRY_AFTER_SECS),
        // The three engine codes carry no `Retry-After`, and each for its own
        // reason. `endpoint_query_failed` may be permanent (the table is gone)
        // or transient (the cluster was busy) and egret cannot tell which, so
        // naming a floor would be a guess. `timeout` is the caller's own query
        // being too big — waiting changes nothing. `engine_unavailable` is the
        // one where a bare 5xx and a client's own backoff is exactly right: the
        // engine comes back when an operator brings it back, and unlike the
        // config-drift 503s there is no human who needs the fleet to go quiet in
        // order to notice.
        ServingError::Unauthorized
        | ServingError::NotFound
        | ServingError::DataAccessDenied { .. }
        | ServingError::NotGranted
        | ServingError::ParamMissing
        | ServingError::ParamInvalid
        | ServingError::RowLimitExceeded { .. }
        | ServingError::EndpointQueryFailed { .. }
        | ServingError::EngineTimeout
        | ServingError::EngineUnavailable { .. }
        // Neither of these two clears with time: the request outran a fixed
        // deadline, and the body will be the same size next time.
        | ServingError::ServingDeadline
        | ServingError::PayloadTooLarge
        | ServingError::Internal(_) => None,
    };

    Wire {
        status,
        code,
        message,
        retry_after,
    }
}

/// Every answer this taxonomy can put on the wire: HTTP status -> the
/// `error.code`s that produce it.
///
/// # Why this is a function and not a constant
///
/// It is DERIVED — [`to_wire`] applied to [`crate::serving::census`], which is
/// itself checked against `ServingError`'s own source. Nothing here is written
/// down twice, which is the point: the two OpenAPI documents this surface
/// publishes both carried a hand-maintained status list, and both went stale the
/// moment Package 3b made `502 engine_unavailable` and `504 timeout` live. One
/// list was corrected; the other — the per-service-account document a consumer
/// actually generates a client from (`openapi_gen::error_responses`) — was not,
/// and no gate could see it, because the byte gate compares the ROOT document to
/// itself.
///
/// A list a human maintains goes stale silently. A list the compiler and the
/// census maintain cannot: add a variant and [`to_wire`] fails to compile until
/// it has an arm, the census fails until it has a sample, and this map — and
/// therefore every document built from it — grows the new status without anyone
/// remembering to widen it.
///
/// `BTreeMap`/`BTreeSet` rather than hash sets, so a generated document is
/// byte-stable across runs: an OpenAPI file that reorders its own keys makes
/// every regeneration look like a change.
pub fn wire_answers() -> BTreeMap<u16, BTreeSet<&'static str>> {
    let mut out: BTreeMap<u16, BTreeSet<&'static str>> = BTreeMap::new();
    for (_, e) in crate::serving::census::every_variant() {
        let w = to_wire(&e);
        out.entry(w.status.as_u16()).or_default().insert(w.code);
    }
    out
}

impl Wire {
    /// Render the envelope. `request_id` is threaded in from the handler rather
    /// than minted here, because the same id must appear in the body, in the
    /// `tracing` line carrying the detail, and in `dataapi.access_log` — three
    /// places that are only useful if they agree.
    pub(crate) fn into_response(self, request_id: &str) -> Response {
        let message = self.message.into_owned();
        let body = Json(ServingErrorBody {
            // Read off `self.status`, the very value the tuple below turns into
            // the status line. The two spellings of one outcome are therefore
            // one decision, and a `200` carrying `"status":"error"` is not a
            // thing this function can produce.
            status: ServingStatus::mirroring(self.status),
            message: message.clone(),
            error: ServingErrorDetail {
                code: self.code.to_string(),
                message,
                request_id: request_id.to_string(),
            },
        });
        match self.retry_after {
            Some(secs) => (self.status, [("Retry-After", secs.to_string())], body).into_response(),
            None => (self.status, body).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Spec §7's table: every variant's status and code, spelled out.
    ///
    /// # What changed here, and why the old shape was worth replacing
    ///
    /// This used to be a hand-written list of variants whose comment claimed
    /// that "adding a variant to `ServingError` fails to compile in `to_wire`
    /// and then shows up as a missing line here". Only the first half was true.
    /// A new variant forces a `to_wire` arm and NOTHING forces a row here, so
    /// the list could silently describe a subset of the taxonomy — which is
    /// exactly what happened to its twin in `serving/metrics.rs`.
    ///
    /// Now the variants come from [`crate::serving::census::every_variant`],
    /// which is itself checked against the enum's own source, and this test owns
    /// only the EXPECTATIONS. A new variant therefore cannot reach the wire
    /// without someone writing down, here, what status and code it carries.
    #[test]
    fn the_code_table_is_spec_section_7() {
        use std::collections::{BTreeMap, BTreeSet};

        // variant name -> (status, code). The census guarantees the key set is
        // the whole enum; the assertions below guarantee this map is exactly it.
        let expected: BTreeMap<&str, (u16, &str)> = BTreeMap::from([
            ("Unauthorized", (401, "key_invalid")),
            ("NotFound", (404, "not_found")),
            // Both recognitions, one code — the two `raw` shapes are two
            // construction sites in `router::engine_error`.
            ("DataAccessDenied", (403, "data_access_denied")),
            ("NotGranted", (403, "not_granted")),
            ("ParamMissing", (400, "param_missing")),
            ("ParamInvalid", (400, "param_invalid")),
            ("RowLimitExceeded", (400, "row_limit_exceeded")),
            ("RateLimited", (429, "rate_limited")),
            ("SchemaDrift", (503, "schema_drift")),
            ("OutputNamingCollision", (503, "output_naming_collision")),
            ("CacheScopeStale", (503, "cache_scope_stale")),
            ("GovernanceStale", (503, "governance_stale")),
            ("ParamGateStale", (503, "param_gate_stale")),
            // The three Package 3b codes.
            ("EndpointQueryFailed", (500, "endpoint_query_failed")),
            ("EngineTimeout", (504, "timeout")),
            ("EngineUnavailable", (502, "engine_unavailable")),
            // The three the request-bounds layer answers. `deadline_exceeded`
            // and `timeout` are two 504s on purpose: two actors, two codes.
            ("ServingDeadline", (504, "deadline_exceeded")),
            ("Overloaded", (503, "overloaded")),
            ("PayloadTooLarge", (413, "payload_too_large")),
            ("Internal", (500, "internal")),
        ]);

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (name, e) in crate::serving::census::every_variant() {
            let Some(&(status, code)) = expected.get(name) else {
                panic!(
                    "`{name}` reaches the wire with no row in this table. Spec §7 \
                     is what a client branches on: write down its status and code \
                     here rather than letting `to_wire` decide unwatched."
                );
            };
            let w = to_wire(&e);
            assert_eq!(w.status.as_u16(), status, "status for {name} ({code})");
            assert_eq!(w.code, code, "code for {name} ({e:?})");
            seen.insert(name);
        }

        let unused: Vec<_> = expected.keys().filter(|k| !seen.contains(*k)).collect();
        assert!(
            unused.is_empty(),
            "rows for variants the census does not produce (renamed? removed?): \
             {unused:?}"
        );
    }

    /// `Retry-After` is present exactly where the code table says a wait is
    /// meaningful, and absent everywhere else.
    ///
    /// Driven off the census for the same reason as the table above: the header
    /// is a promise about a specific condition clearing, and a variant that
    /// acquired one by accident (or lost one) would otherwise be invisible.
    #[test]
    fn the_retry_after_header_is_carried_by_exactly_the_codes_that_mean_it() {
        use std::collections::BTreeMap;

        let expected: BTreeMap<&str, Option<u64>> = BTreeMap::from([
            // A prediction: the bucket has refilled.
            ("RateLimited", Some(2)),
            // A floor, not a forecast — a human clears these.
            ("SchemaDrift", Some(CONFIG_DRIFT_RETRY_AFTER_SECS)),
            ("OutputNamingCollision", Some(CONFIG_DRIFT_RETRY_AFTER_SECS)),
            ("CacheScopeStale", Some(CONFIG_DRIFT_RETRY_AFTER_SECS)),
            ("GovernanceStale", Some(CONFIG_DRIFT_RETRY_AFTER_SECS)),
            ("ParamGateStale", Some(CONFIG_DRIFT_RETRY_AFTER_SECS)),
            // Drains on its own, and fast.
            ("Overloaded", Some(SHED_RETRY_AFTER_SECS)),
        ]);

        for (name, e) in crate::serving::census::every_variant() {
            let want = expected.get(name).copied().unwrap_or(None);
            assert_eq!(
                to_wire(&e).retry_after,
                want,
                "{name} carries the wrong `Retry-After`; every value here is a \
                 claim about when the condition clears"
            );
        }

        // The 429's value is COMPUTED from the bucket, and rounded up: a client
        // that comes back at one second is still refused.
        assert_eq!(
            to_wire(&ServingError::RateLimited {
                retry_after: Duration::from_millis(1_500)
            })
            .retry_after,
            Some(2)
        );
    }

    /// The one property this whole module exists for. Asserted on the MESSAGE
    /// rather than on the rendered body so it holds no matter how the envelope
    /// is serialised.
    #[test]
    fn an_internal_error_never_renders_its_source() {
        let e = ServingError::internal(egret_core::Error::NotFound(
            "hive.analytics.does_not_exist".into(),
        ));
        let w = to_wire(&e);
        assert_eq!(w.message, "internal error");
        assert!(
            !w.message.contains("does_not_exist"),
            "leaked a relation name: {}",
            w.message
        );

        let e = ServingError::internal(egret_core::Error::Internal(anyhow::anyhow!(
            "query port: connect trino-coordinator.internal:8080: connection refused"
        )));
        let w = to_wire(&e);
        assert!(
            !w.message.contains("trino-coordinator.internal"),
            "leaked the engine host: {}",
            w.message
        );
    }

    /// `DataAccessDenied::raw` exists ONLY for the server-side `error = ?e` log
    /// line — see its doc comment. This is the wire half of that promise:
    /// whatever the engine actually said, `to_wire` must render the same
    /// fixed message every time, never the raw text.
    #[test]
    fn a_data_access_denial_never_renders_the_engines_raw_message() {
        let e = ServingError::DataAccessDenied {
            raw: Some(anyhow::anyhow!(
                "Access Denied: Cannot select from table sales"
            )),
        };
        let w = to_wire(&e);
        assert!(
            !w.message.contains("Cannot select from table"),
            "leaked the engine's raw denial text: {}",
            w.message
        );
        assert!(
            !w.message.contains("sales"),
            "leaked a table name from the engine's message: {}",
            w.message
        );
        assert!(
            w.message.contains("service account"),
            "must still point at the fix: {}",
            w.message
        );

        // The other half of the promise: the text the wire refused to render is
        // still recoverable server-side, through the `#[source]` chain the
        // `error = ?e` log line in `serve`'s failure branch prints. Without this
        // assertion the field could be dropped from the attribute and only the
        // leak test above would still pass.
        let src =
            std::error::Error::source(&e).expect("the engine's words must survive as a source");
        assert!(
            src.to_string().contains("Cannot select from table sales"),
            "the raw message must reach the log: {src}"
        );

        // And the typed arm carries nothing, deliberately — `Error::Forbidden`
        // is a unit variant, so there is no engine text to invent.
        assert!(
            std::error::Error::source(&ServingError::DataAccessDenied { raw: None }).is_none(),
            "the typed arm has no engine message and must not fabricate one"
        );
    }

    /// **The leak-proofness rule, applied to the three codes Package 3b added.**
    ///
    /// Splitting `internal` into named causes is exactly the kind of change that
    /// takes a message from "internal error" to something that quotes the
    /// engine, because the whole point of the split is to be more specific. It
    /// is more specific about the CAUSE and not one word more specific about the
    /// data: `EndpointQueryFailed` and `EngineUnavailable` both carry a `raw`,
    /// and neither may render it.
    ///
    /// Mutating either arm to interpolate its source fails here.
    #[test]
    fn the_engine_codes_never_render_what_the_engine_said() {
        let failed = ServingError::EndpointQueryFailed {
            raw: Some(anyhow::anyhow!(
                "line 1:15: Table 'hive.analytics.salaries' does not exist"
            )),
        };
        let w = to_wire(&failed);
        for leak in ["hive.analytics.salaries", "line 1:15", "Table"] {
            assert!(
                !w.message.contains(leak),
                "leaked {leak:?} from the engine: {}",
                w.message
            );
        }
        // The other half: the words still reach the log, through the same
        // `#[source]` chain `serve`'s `error = ?e` line prints.
        let src = std::error::Error::source(&failed).expect("the engine's words reach the log");
        assert!(src.to_string().contains("hive.analytics.salaries"));

        let unavailable = ServingError::EngineUnavailable {
            raw: Some(anyhow::anyhow!(
                "engine unavailable: connect: error sending request for url \
                 (https://trino-coordinator.internal:8443/v1/statement)"
            )),
        };
        let w = to_wire(&unavailable);
        for leak in ["trino-coordinator.internal", "8443", "v1/statement"] {
            assert!(
                !w.message.contains(leak),
                "a 502 taught a caller outside the network where the engine lives: {leak:?} in {}",
                w.message
            );
        }
        assert!(std::error::Error::source(&unavailable)
            .expect("the address reaches the log")
            .to_string()
            .contains("trino-coordinator.internal"));

        // `EngineTimeout` carries nothing to leak, and that is a statement: the
        // port's timeout has no engine text behind it, only egret's own note of
        // which budget expired.
        assert!(std::error::Error::source(&ServingError::EngineTimeout).is_none());
    }

    /// The split is only worth anything if the codes are DISTINCT — an operator
    /// aggregating `access_log.error_code` to answer "which endpoints are
    /// failing" needs the author's broken endpoint separable from egret's own
    /// fault and from the engine being down.
    ///
    /// Collapsing any two of them back onto one string fails here.
    #[test]
    fn the_four_5xx_causes_have_four_different_codes() {
        let codes = [
            to_wire(&ServingError::EndpointQueryFailed { raw: None }).code,
            to_wire(&ServingError::EngineTimeout).code,
            to_wire(&ServingError::EngineUnavailable { raw: None }).code,
            to_wire(&ServingError::Internal(anyhow::anyhow!("boom"))).code,
        ];
        let mut seen: Vec<&str> = codes.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            codes.len(),
            "two 5xx causes share one error_code: {codes:?}"
        );
        // And none of them is a 4xx — spec §7's requirement (c). The caller did
        // nothing wrong in any of the four.
        for e in [
            ServingError::EndpointQueryFailed { raw: None },
            ServingError::EngineTimeout,
            ServingError::EngineUnavailable { raw: None },
        ] {
            let w = to_wire(&e);
            assert!(
                w.status.is_server_error(),
                "{} must not be a client error, got {}",
                w.code,
                w.status
            );
        }
    }

    /// The rule the module comment states, asserted on the only two answers
    /// that could break it. A parameter name is the author's, so neither body
    /// may contain one — and since the variants carry nothing, the strongest
    /// available assertion is that the messages are constants that mention no
    /// specific parameter at all.
    #[test]
    fn a_parameter_error_names_no_parameter() {
        assert_eq!(
            to_wire(&ServingError::ParamMissing).message,
            "a required parameter is missing"
        );
        assert_eq!(
            to_wire(&ServingError::ParamInvalid).message,
            "a parameter value is not valid for this endpoint"
        );
        // The names an endpoint in the suites actually declares. If either
        // message ever started interpolating, this is what would appear.
        for name in ["region", "day", "from", "sort"] {
            for e in [ServingError::ParamMissing, ServingError::ParamInvalid] {
                assert!(
                    !to_wire(&e).message.contains(name),
                    "a 400 named the parameter `{name}`, which turns the error \
                     surface into a directory of what the endpoint declares"
                );
            }
        }
    }

    /// The cap is egret-published author config, not caller input, and spec §7
    /// asks for it explicitly. Pinned so a future "no interpolation anywhere"
    /// sweep has to argue with a test rather than delete it silently.
    #[test]
    fn the_row_cap_is_the_one_number_a_message_states() {
        let w = to_wire(&ServingError::RowLimitExceeded { cap: 5_000 });
        assert_eq!(w.message, "result exceeded the 5000-row cap");
    }

    /// Codes are the client's branching key, so two variants sharing one would
    /// make them indistinguishable on the wire — and indistinguishable in
    /// `access_log.error_code`, which is the column an operator aggregates.
    ///
    /// Over the census rather than over a list, for the reason the code table
    /// above gives: a hand-written list can only find a collision between
    /// variants somebody remembered to list, and the collision this repo
    /// actually met was between a variant on this branch (`EngineTimeout` →
    /// `timeout`) and one on a sibling branch that no list here mentioned.
    ///
    /// One variant may appear more than once in the census (two `raw` shapes are
    /// two construction sites), so the claim is per NAME: two different variants
    /// may not answer with one code.
    #[test]
    fn no_two_variants_share_a_code() {
        use std::collections::BTreeMap;

        let mut owner: BTreeMap<&'static str, &'static str> = BTreeMap::new();
        for (name, e) in crate::serving::census::every_variant() {
            let code = to_wire(&e).code;
            match owner.insert(code, name) {
                Some(other) if other != name => panic!(
                    "`{name}` and `{other}` both answer `{code}`: a client \
                     branching on the code, and an operator aggregating \
                     `access_log.error_code`, cannot tell them apart"
                ),
                _ => {}
            }
        }
    }
}
