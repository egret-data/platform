//! Why a [`crate::QueryExecPort`] **execution** failed, as a value rather than
//! as a message.
//!
//! # The problem this closes
//!
//! `QueryExecPort` answers with `egret_core::Error`, whose variants are HTTP
//! shapes (`NotFound`, `InvalidInput`, `Forbidden`, `Internal`) rather than
//! causes. Two of those shapes are ambiguous at the seam, and the Data API's
//! serving surface has carried the consequence since P4:
//!
//! * **`Internal` is produced for at least six unrelated causes** — an
//!   unreachable coordinator, a metastore round-trip, an unopenable secret, a
//!   failed delegation resolve, a corrupt snapshot, a cache failure. A serving
//!   response can therefore only ever say `internal`, and an operator reading
//!   `dataapi.access_log.error_code` cannot separate "the engine is down" from
//!   "egret is broken".
//! * **A timeout arrives as `InvalidInput("query timed out")`**, a fixed string
//!   egret itself wrote. Recognising it downstream would mean matching another
//!   crate's message text — a check that keeps compiling and silently stops
//!   working the day someone rewords the string.
//!
//! Both are recorded in `docs/vi/specs/2026-08-14-data-api-design.md` §7 as
//! blocking `engine_unavailable` (502) and `timeout` (504), with the note that
//! closing them means widening the port's error contract.
//!
//! # Why this widening is additive rather than a new error type
//!
//! Changing `QueryExecPort`'s `Result<_, egret_core::Error>` would touch every
//! implementation — ten of them, nine hand-written test fakes — and every
//! caller, for the benefit of exactly one consumer. Instead the *existing*
//! `egret_core::Error::Internal(anyhow::Error)` carries a typed
//! [`ExecFailure`], recoverable by [`ExecFailure::kind_of`].
//!
//! What that buys, and what it deliberately does not disturb:
//!
//! * A consumer that does not look stays on today's behaviour exactly — still
//!   `Error::Internal`, still a 500, still redacted to `"internal error"` by
//!   `egret_core`'s `IntoResponse`. **No existing surface changes shape.**
//! * A consumer that does look gets the cause as a value, with no message
//!   matching anywhere.
//! * The detail (a coordinator's `host:port`, an engine's SQLSTATE) still rides
//!   along in `Display` for the log line `egret_core::http` already writes for
//!   every `Internal`, and still never reaches a body.
//!
//! # What is NOT routed through here, and why
//!
//! The engine's own words about a **statement** — `EngineError::Engine`,
//! `BadRequest`, `Limit` — stay `egret_core::Error::InvalidInput(msg)`. That is
//! not an oversight: the Data API's authoring surfaces (publish-time validate,
//! and the preview/"Try it" loop) show that message to the endpoint author, who
//! is the person who can act on `line 1:8: mismatched input 'FROM'`. Folding it
//! into a redacted `Internal` would take the one useful error message in the
//! authoring loop away to gain a classification the serving router can already
//! make — `InvalidInput` is, at that seam, provably the only variant whose text
//! came from the engine.

use std::fmt;

/// The cause, as a closed set.
///
/// Kept small on purpose: a kind exists here only when some consumer answers
/// differently because of it. `EngineUnavailable` becomes a 502 and `Timeout` a
/// 504 on the Data API's serving surface; nothing else is distinguishable, so
/// nothing else is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecFailureKind {
    /// The engine could not be reached, or the transport to it failed: a refused
    /// connection, a TLS failure, a dropped socket. An **operator's** problem,
    /// and specifically not egret's own — a metastore failure or an unopenable
    /// secret must never reach here, because answering 502 for one of those
    /// would page someone about Trino when the database is what broke.
    EngineUnavailable,
    /// The statement outran the budget its caller passed in `Limits::timeout`.
    ///
    /// **Execution only.** `describe_as` maps a timeout to
    /// `InvalidInput("query timed out")` and keeps doing so, because for a
    /// `LIMIT 0` describe a timeout really does mean the query is too expensive
    /// to plan — the author's problem, and a 400 they can act on. For an
    /// execution it means the caller's own published budget was exceeded, which
    /// is a different sentence to a different person.
    Timeout,
}

/// A typed cause, carried inside `egret_core::Error::Internal`'s `anyhow::Error`.
///
/// `detail` is for the log, exactly like every other `Internal` source: it may
/// name a coordinator's `host:port`, and `egret_core`'s `IntoResponse` redacts
/// the whole thing to `"internal error"` before any client sees it.
#[derive(Debug)]
pub struct ExecFailure {
    kind: ExecFailureKind,
    detail: String,
}

impl ExecFailure {
    pub fn new(kind: ExecFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> ExecFailureKind {
        self.kind
    }

    /// Build the `egret_core::Error` a port method returns for this cause.
    ///
    /// One constructor so the wrapping is spelled once: a caller that built the
    /// `anyhow::Error` itself could add a context layer, and `downcast_ref`
    /// would still find the `ExecFailure` — but it could equally forget to wrap
    /// it at all, and the only symptom would be a serving response quietly
    /// falling back to `internal`. Nothing about that failure is visible in a
    /// test that does not look for it, which is why there is exactly one way in.
    pub fn into_error(self) -> egret_core::Error {
        egret_core::Error::Internal(anyhow::Error::new(self))
    }

    /// Recover the cause from a port error, if it carries one.
    ///
    /// Returns `None` for every other error — including an `Error::Internal`
    /// that came from egret's own machinery rather than from the engine, which
    /// is the distinction the whole module exists to make.
    pub fn kind_of(e: &egret_core::Error) -> Option<ExecFailureKind> {
        match e {
            egret_core::Error::Internal(src) => {
                src.downcast_ref::<ExecFailure>().map(ExecFailure::kind)
            }
            _ => None,
        }
    }
}

impl fmt::Display for ExecFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ExecFailureKind::EngineUnavailable => write!(f, "engine unavailable: {}", self.detail),
            ExecFailureKind::Timeout => write!(f, "query timed out: {}", self.detail),
        }
    }
}

impl std::error::Error for ExecFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip the whole design rests on: what a port method builds is
    /// what a consumer recovers.
    #[test]
    fn a_kind_survives_the_trip_through_an_internal_error() {
        for kind in [ExecFailureKind::EngineUnavailable, ExecFailureKind::Timeout] {
            let e = ExecFailure::new(kind, "trino.internal:8443: connection refused").into_error();
            assert!(
                matches!(e, egret_core::Error::Internal(_)),
                "the carrier must stay Internal so no existing consumer changes shape"
            );
            assert_eq!(ExecFailure::kind_of(&e), Some(kind));
        }
    }

    /// **The distinction this module exists to make.** An `Internal` from
    /// egret's own machinery — a metastore round-trip, an unopenable secret —
    /// must not be readable as an engine failure, or a 502 would page an
    /// operator about the engine when the database is what broke.
    #[test]
    fn an_ordinary_internal_error_carries_no_kind() {
        let e =
            egret_core::Error::Internal(anyhow::anyhow!("open secret: connection pool timeout"));
        assert_eq!(ExecFailure::kind_of(&e), None);
    }

    /// Every other variant is a shape the port already answers precisely; none
    /// of them may be mistaken for an engine failure.
    #[test]
    fn no_other_variant_carries_a_kind() {
        for e in [
            egret_core::Error::NotFound("tpch.tiny.nope".into()),
            egret_core::Error::InvalidInput("line 1:8: mismatched input 'FROM'".into()),
            egret_core::Error::Forbidden,
            egret_core::Error::Unauthorized,
            egret_core::Error::Conflict("x".into()),
            egret_core::Error::TooManyRequests,
        ] {
            assert_eq!(ExecFailure::kind_of(&e), None, "{e:?}");
        }
    }

    /// The detail is kept for the log — `egret_core::http::log_internal` renders
    /// the `anyhow` chain with `{:#}` — and it is the client-facing redaction,
    /// not this string, that keeps a coordinator's address out of a response.
    #[test]
    fn the_detail_survives_for_the_log() {
        let e = ExecFailure::new(
            ExecFailureKind::EngineUnavailable,
            "error sending request for url (https://trino.internal:8443/v1/statement)",
        )
        .into_error();
        assert!(e.to_string().contains("trino.internal:8443"));
    }
}
