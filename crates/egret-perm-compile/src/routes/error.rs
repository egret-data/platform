//! HTTP error for the `perm` router. Same status codes and `{"error": msg}`
//! body shape as the Policy/Ranger seam's `PolicyHttpError`, so the two
//! consoles under `/api/control-plane` behave identically to a client.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use egret_perm::{CompileError, PermError};
use egret_policy::PolicyStoreError;

#[derive(Debug, thiserror::Error)]
pub enum PermHttpError {
    /// Authenticated but lacks the required `policy:*` permission.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    /// Not found, with the sentence that says what was looked for (People &
    /// groups Task 3: "No group is named x.").
    #[error("{0}")]
    Missing(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// The row a PATCH read changed under it before it held the profile locks,
    /// and the profiles it locked no longer cover the ones the row now affects
    /// — see `routes::write_grant`. A 409 rather than a 400 because the request
    /// was well-formed and nothing about it needs fixing: it needs retrying
    /// against the state that won.
    #[error("conflict: {0}")]
    Conflict(String),
    /// **Two mask (or row-filter) rules would decide one request differently**
    /// — `CompileError::AmbiguousMaskOrFilterArbitration` (SP4d-4 Task 3). A
    /// 409, not a 400: the write is well-formed and conflicts with a rule (or
    /// policy, or tag assignment) already stored. `body` is
    /// `crate::overlap::refusal_body` — `{error, code: "rule_overlap", rules,
    /// coordinate, kind}` — served as is.
    #[error("rule overlap: {body}")]
    Overlap { body: serde_json::Value },
    /// **Allocating a username would make a stale stored ref apply to this
    /// person** (People & groups Task 5, spec §4.4). A 409: the request is
    /// well-formed and conflicts with grant principals or rule exemptions
    /// already stored. `body` is `{error, refs}`, served as is.
    #[error("stale user refs: {body}")]
    StaleUserRefs { body: serde_json::Value },
    #[error("internal error")]
    Internal,
}

impl PermHttpError {
    /// A database failure the handler itself hit (`begin`, `commit`,
    /// `acquire`). Logged, not echoed: the message can name schema internals.
    pub(crate) fn db<E: std::fmt::Display>(e: E) -> Self {
        tracing::error!(error = %e, "perm route database error");
        PermHttpError::Internal
    }
}

impl From<PermError> for PermHttpError {
    fn from(e: PermError) -> Self {
        match e {
            PermError::NotFound(_) => PermHttpError::NotFound,
            PermError::Invalid(s) => {
                tracing::error!(error = %s, "perm store returned data it cannot parse");
                PermHttpError::Internal
            }
            PermError::Db(e) => PermHttpError::db(e),
        }
    }
}

impl From<PolicyStoreError> for PermHttpError {
    fn from(e: PolicyStoreError) -> Self {
        match e {
            PolicyStoreError::NotFound(_) => PermHttpError::NotFound,
            PolicyStoreError::BadRequest(s) => PermHttpError::BadRequest(s),
            other => PermHttpError::db(other),
        }
    }
}

/// **The mapping that carries the task's point.** A recompile failure is not
/// an internal error to be swallowed — it is the reason the request fails and
/// the write rolls back, so the response has to say which of the two kinds it
/// was:
///
/// - Every variant below except `Db` describes something about the grant or
///   rule THE CALLER JUST WROTE that cannot be expressed against a profile it
///   affects (an undeclared mask type, an unparseable custom mask, a `Role` in
///   an exemption, an expansion over the ceiling). That is a 400 naming the
///   refusal, because the author can fix it — and because a 500 would read as
///   "egret is broken" for an input egret correctly refused.
/// - `Db` is the genuine internal failure: 500, message withheld.
/// - `AmbiguousMaskOrFilterArbitration` is a 409 with a structured body
///   (SP4d-4 Task 3): the write conflicts with a rule, policy or tag
///   assignment already stored, and the body names both rules and the
///   coordinate so a client can link to them.
///
/// `UnexpandedPrincipalKind` never reaches here from the recompile driver — it
/// is recorded as a skip and surfaced in the response's `skipped` list instead
/// — but it is mapped as a 400 for exhaustiveness, since a direct caller of
/// the pure compiler would get it for an input the author wrote.
impl From<CompileError> for PermHttpError {
    fn from(e: CompileError) -> Self {
        match e {
            CompileError::Db(detail) => PermHttpError::db(detail),
            ref overlap @ CompileError::AmbiguousMaskOrFilterArbitration { .. } => {
                PermHttpError::Overlap {
                    body: crate::overlap::refusal_body(overlap)
                        .expect("refusal_body answers every AmbiguousMaskOrFilterArbitration"),
                }
            }
            CompileError::ServiceDefUnreadable { .. } => {
                tracing::error!(error = %e, "embedded service-def failed to parse");
                PermHttpError::Internal
            }
            other => PermHttpError::BadRequest(other.to_string()),
        }
    }
}

/// `egret_core::Error` is what `egret_core::authz::require` returns.
impl From<egret_core::Error> for PermHttpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => PermHttpError::NotFound,
            egret_core::Error::InvalidInput(s) | egret_core::Error::Conflict(s) => {
                PermHttpError::BadRequest(s)
            }
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => {
                PermHttpError::Forbidden
            }
            egret_core::Error::FeatureDisabled(_) => PermHttpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                PermHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(s) => PermHttpError::db(s),
        }
    }
}

fn error_body(msg: &str) -> serde_json::Value {
    serde_json::json!({ "error": msg })
}

impl IntoResponse for PermHttpError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        let (status, body) = match self {
            PermHttpError::Forbidden => (StatusCode::FORBIDDEN, error_body("forbidden")),
            PermHttpError::NotFound => (StatusCode::NOT_FOUND, error_body("not found")),
            PermHttpError::Missing(_) => (StatusCode::NOT_FOUND, error_body(&msg)),
            PermHttpError::BadRequest(_) => (StatusCode::BAD_REQUEST, error_body(&msg)),
            PermHttpError::Conflict(_) => (StatusCode::CONFLICT, error_body(&msg)),
            // The structured body, served as is: its `error` is the sentence.
            PermHttpError::Overlap { body } => (StatusCode::CONFLICT, body),
            PermHttpError::StaleUserRefs { body } => (StatusCode::CONFLICT, body),
            PermHttpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body("internal error"),
            ),
        };
        (status, Json(body)).into_response()
    }
}
