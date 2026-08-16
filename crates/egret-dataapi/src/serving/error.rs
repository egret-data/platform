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

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use super::ratelimit::retry_after_secs;
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

/// Does this engine failure mean "the principal may not read this data"?
///
/// Matched on the message because neither connector surfaces a typed
/// authorization error: Trino answers `Access Denied: …` and StarRocks answers
/// with its own permission wording. **A substring match is a heuristic and is
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
        // The author's SQL is fine and the engine's policy denied the DATA.
        // Reporting this as bad input sends the operator to debug the query
        // instead of the grant, which is the whole reason it is not folded into
        // a 400.
        ServingError::DataAccessDenied => (
            StatusCode::FORBIDDEN,
            "data_access_denied",
            "the service account is not granted access to this data".into(),
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
        // Recognised from the engine's own message by `is_permission_denied`
        // rather than from a typed signal — see that function's doc and
        // `ServingError::RangerDenied`'s own. Names the missing STEP, not the
        // engine's text: an author who reads a raw "Access Denied: Cannot
        // select from table sales" debugs their SQL, which is the one thing
        // that is not wrong here.
        ServingError::RangerDenied => (
            StatusCode::FORBIDDEN,
            "ranger_denied",
            "The service account running this endpoint is not permitted to read the \
             underlying data. Publishing an endpoint does not grant data access — \
             grant the service account in Ranger as well."
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
        | ServingError::ParamGateStale => Some(CONFIG_DRIFT_RETRY_AFTER_SECS),
        ServingError::Unauthorized
        | ServingError::NotFound
        | ServingError::DataAccessDenied
        | ServingError::NotGranted
        | ServingError::RangerDenied
        | ServingError::ParamMissing
        | ServingError::ParamInvalid
        | ServingError::RowLimitExceeded { .. }
        | ServingError::Internal(_) => None,
    };

    Wire {
        status,
        code,
        message,
        retry_after,
    }
}

impl Wire {
    /// Render the envelope. `request_id` is threaded in from the handler rather
    /// than minted here, because the same id must appear in the body, in the
    /// `tracing` line carrying the detail, and in `dataapi.access_log` — three
    /// places that are only useful if they agree.
    pub(crate) fn into_response(self, request_id: &str) -> Response {
        let body = Json(ServingErrorBody {
            error: ServingErrorDetail {
                code: self.code.to_string(),
                message: self.message.into_owned(),
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

    /// Every variant, spelled out. Written as a list rather than as a loop over
    /// something clever so that adding a variant to `ServingError` fails to
    /// compile in `to_wire` and then shows up as a missing line here.
    #[test]
    fn the_code_table_is_spec_section_7() {
        let cases: Vec<(ServingError, u16, &str)> = vec![
            (ServingError::Unauthorized, 401, "key_invalid"),
            (ServingError::NotFound, 404, "not_found"),
            (ServingError::DataAccessDenied, 403, "data_access_denied"),
            (ServingError::NotGranted, 403, "not_granted"),
            (ServingError::RangerDenied, 403, "ranger_denied"),
            (ServingError::ParamMissing, 400, "param_missing"),
            (ServingError::ParamInvalid, 400, "param_invalid"),
            (
                ServingError::RowLimitExceeded { cap: 100 },
                400,
                "row_limit_exceeded",
            ),
            (
                ServingError::RateLimited {
                    retry_after: Duration::from_secs(2),
                },
                429,
                "rate_limited",
            ),
            (ServingError::SchemaDrift, 503, "schema_drift"),
            (
                ServingError::OutputNamingCollision,
                503,
                "output_naming_collision",
            ),
            (ServingError::CacheScopeStale, 503, "cache_scope_stale"),
            (
                ServingError::Internal(anyhow::anyhow!("boom")),
                500,
                "internal",
            ),
        ];
        for (e, status, code) in cases {
            let w = to_wire(&e);
            assert_eq!(w.status.as_u16(), status, "status for {code}");
            assert_eq!(w.code, code, "code for {e:?}");
        }
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

    /// Two `Retry-After` sources with two meanings. The 429's is computed from
    /// the bucket; the 503s' is a fixed floor.
    #[test]
    fn retry_after_is_present_exactly_where_it_means_something() {
        assert_eq!(
            to_wire(&ServingError::RateLimited {
                retry_after: Duration::from_millis(1_500)
            })
            .retry_after,
            // Rounded UP by `retry_after_secs`: a client that comes back at 1s
            // is still refused.
            Some(2),
            "the 429's wait is computed, not fixed"
        );
        for e in [
            ServingError::SchemaDrift,
            ServingError::OutputNamingCollision,
            ServingError::CacheScopeStale,
            ServingError::ParamGateStale,
        ] {
            assert_eq!(
                to_wire(&e).retry_after,
                Some(CONFIG_DRIFT_RETRY_AFTER_SECS),
                "a config-drift 503 states a floor so clients do not default to \
                 a one-second backoff against a state only a human clears: {e:?}"
            );
        }
        for e in [
            ServingError::Unauthorized,
            ServingError::NotFound,
            ServingError::DataAccessDenied,
            ServingError::NotGranted,
            ServingError::RangerDenied,
            ServingError::ParamMissing,
            ServingError::ParamInvalid,
            ServingError::RowLimitExceeded { cap: 1 },
            ServingError::Internal(anyhow::anyhow!("boom")),
        ] {
            assert_eq!(
                to_wire(&e).retry_after,
                None,
                "waiting does not help with {e:?}, so promising a wait would be a lie"
            );
        }
    }

    /// Codes are the client's branching key, so two variants sharing one would
    /// make them indistinguishable on the wire. The four 503s in particular
    /// have four different fixes.
    #[test]
    fn no_two_variants_share_a_code() {
        let codes = [
            to_wire(&ServingError::Unauthorized).code,
            to_wire(&ServingError::NotFound).code,
            to_wire(&ServingError::DataAccessDenied).code,
            // The pair this test exists for most: two variants that share a
            // STATUS must not share a code, or the wire cannot tell "ask egret
            // for a grant" from "ask Ranger for a grant".
            to_wire(&ServingError::NotGranted).code,
            // A third 403. Recognised from an untyped engine message rather
            // than a typed signal, but the client-facing distinction from the
            // two rows above is the same one that matters: which system holds
            // the fix.
            to_wire(&ServingError::RangerDenied).code,
            // Three 400s now. `param_missing` and `param_invalid` point at two
            // different client-side fixes, and both are different from
            // `row_limit_exceeded`, which is not the caller's fault at all.
            to_wire(&ServingError::ParamMissing).code,
            to_wire(&ServingError::ParamInvalid).code,
            to_wire(&ServingError::RowLimitExceeded { cap: 1 }).code,
            to_wire(&ServingError::RateLimited {
                retry_after: Duration::from_secs(1),
            })
            .code,
            to_wire(&ServingError::SchemaDrift).code,
            to_wire(&ServingError::OutputNamingCollision).code,
            to_wire(&ServingError::CacheScopeStale).code,
            // Four 503s now, and this is the one most easily confused with
            // `cache_scope_stale`: both mean "this version predates something",
            // and only one of them is cleared by restoring a connection setting.
            to_wire(&ServingError::ParamGateStale).code,
            to_wire(&ServingError::Internal(anyhow::anyhow!("boom"))).code,
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len(), "duplicate code in {codes:?}");
    }
}
