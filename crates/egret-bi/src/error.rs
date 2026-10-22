//! BI domain error + its HTTP mapping. The auth/validation arm now lives in
//! `egret_core::ApiError` (shared); this file keeps only the BI-domain arm.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum BiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("engine: {0}")]
    Engine(String),
    /// Two mask (or row-filter) policies would decide one request differently
    /// — a tag write's recompile refused, or the write created such a pair
    /// among the stored policies (SP4d-4 Task 3). 409 with
    /// `egret_perm_compile::overlap::refusal_body`, served as is, so the tag
    /// UI gets the same `{error, code: "rule_overlap", rules, coordinate,
    /// kind}` the rule routes answer.
    #[error("rule overlap: {body}")]
    Overlap { body: serde_json::Value },
    /// A refusal with a stable `code` for the UI (classification UX spec
    /// C5/C10/C11). `body` is `{error, code, ...fields}`, served as is with
    /// `status`; `error` is always a full sentence, so a client that reads only
    /// `error` keeps working.
    #[error("{}", .body["error"].as_str().unwrap_or("refused"))]
    Coded {
        status: StatusCode,
        body: serde_json::Value,
    },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl BiError {
    /// `{error, code}` merged with `fields` (an object; anything else is ignored).
    pub fn coded(
        status: StatusCode,
        code: &str,
        error: impl Into<String>,
        fields: serde_json::Value,
    ) -> Self {
        let mut body = serde_json::json!({ "error": error.into(), "code": code });
        if let (Some(b), serde_json::Value::Object(extra)) = (body.as_object_mut(), fields) {
            b.extend(extra);
        }
        BiError::Coded { status, body }
    }

    /// The stable code of a coded or overlap refusal.
    pub fn code(&self) -> Option<&str> {
        match self {
            BiError::Coded { body, .. } | BiError::Overlap { body } => body["code"].as_str(),
            _ => None,
        }
    }
}

impl From<egret_query::DatabaseError> for BiError {
    fn from(e: egret_query::DatabaseError) -> Self {
        match e {
            egret_query::DatabaseError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}
impl From<egret_query::EngineError> for BiError {
    fn from(e: egret_query::EngineError) -> Self {
        match e {
            egret_query::EngineError::NotFound(s) => BiError::NotFound(s),
            // The engine's own words. `EngineError::Engine` already reads
            // "engine: …", and `BiError::Engine` adds that prefix again (KI-087).
            egret_query::EngineError::Engine(s) => BiError::Engine(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}

/// SP4a Task 7: a catalog-tag write recompiles every StarRocks profile it can
/// stale (D5), and a genuine `CompileError` (an authoring/resolution bug —
/// `WorkspaceMembers` is skipped, not surfaced as an error; see
/// `egret-perm-compile::trigger`'s module doc) must not disappear into an
/// opaque 500. `Engine` already means "a downstream policy/engine operation
/// failed" and renders the message, not just a generic label.
impl From<egret_perm::CompileError> for BiError {
    fn from(e: egret_perm::CompileError) -> Self {
        match egret_perm_compile::overlap::refusal_body(&e) {
            Some(body) => BiError::Overlap { body },
            None => BiError::Engine(format!("permission recompile failed: {e}")),
        }
    }
}

impl BiError {
    /// The refusal of a TAG ASSIGNMENT whose overlap check found a pair the
    /// assignment creates. The 409 body is the compiler's, but its `error` is
    /// rewritten: the compiler's own sentence leads with "recompiling profile X
    /// would emit", which describes a recompile the steward did not ask for,
    /// where this write is an assignment. Any other failure is mapped as ever.
    pub(crate) fn from_assignment_check(e: egret_perm::CompileError, assignment: &str) -> Self {
        let refusal = match &e {
            egret_perm::CompileError::AmbiguousMaskOrFilterArbitration { refusal, .. } => {
                refusal.clone()
            }
            _ => return e.into(),
        };
        match egret_perm_compile::overlap::refusal_body(&e) {
            Some(mut body) => {
                body["error"] = serde_json::Value::String(format!(
                    "assigning {assignment} would create an order-ambiguous pair: {refusal}"
                ));
                BiError::Overlap { body }
            }
            None => e.into(),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for BiError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        let error = |m: &str| serde_json::json!({ "error": m });
        let (status, body) = match self {
            BiError::NotFound(_) => (StatusCode::NOT_FOUND, error(&msg)),
            BiError::Forbidden => (StatusCode::FORBIDDEN, error("forbidden")),
            BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, error(&msg)),
            BiError::Conflict(_) => (StatusCode::CONFLICT, error(&msg)),
            BiError::Engine(_) => (StatusCode::BAD_GATEWAY, error(&msg)),
            // The structured body, served as is: its `error` is the sentence.
            BiError::Overlap { body } => (StatusCode::CONFLICT, body),
            BiError::Coded { status, body } => (status, body),
            BiError::Db(_) => (StatusCode::INTERNAL_SERVER_ERROR, error("internal error")),
        };
        (status, Json(body)).into_response()
    }
}

/// The BI handler error type. `From<egret_core::Error>` comes from core;
/// this adds the domain-side `From` so `?` works on `BiError` too.
pub type BiApiError = ApiError<BiError>;

impl From<BiError> for BiApiError {
    fn from(e: BiError) -> Self {
        ApiError::Domain(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::{CompileError, OverlapKind, OverlapRule};

    fn overlap() -> CompileError {
        CompileError::AmbiguousMaskOrFilterArbitration {
            profile: "trino-main".into(),
            refusal: "two mask policies would be served".into(),
            kind: OverlapKind::Mask,
            coordinate: "wh.sales.orders.email".into(),
            rules: vec![
                OverlapRule {
                    id: None,
                    name: "a".into(),
                },
                OverlapRule {
                    id: None,
                    name: "b".into(),
                },
            ],
        }
    }

    #[test]
    fn an_assignment_refusal_describes_the_assignment_and_keeps_the_structured_body() {
        let BiError::Overlap { body } =
            BiError::from_assignment_check(overlap(), "this tag to wh.sales.orders.email")
        else {
            panic!("expected an overlap");
        };
        let error = body["error"].as_str().unwrap();
        assert!(
            error.starts_with("assigning this tag to wh.sales.orders.email would create"),
            "{error}"
        );
        assert!(
            error.ends_with("two mask policies would be served"),
            "{error}"
        );
        assert!(!error.contains("recompiling profile"), "{error}");
        assert_eq!(body["code"], "rule_overlap");
        assert_eq!(body["coordinate"], "wh.sales.orders.email");
        assert_eq!(body["kind"], "mask");
        assert_eq!(body["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_recompile_refusal_keeps_the_compilers_own_sentence() {
        let BiError::Overlap { body } = BiError::from(overlap()) else {
            panic!("expected an overlap");
        };
        assert!(body["error"]
            .as_str()
            .unwrap()
            .starts_with("recompiling profile"));
    }

    #[test]
    fn an_engine_error_carries_one_engine_prefix() {
        let e = BiError::from(egret_query::EngineError::Engine(
            "line 1:14: mismatched input".into(),
        ));
        assert_eq!(e.to_string(), "engine: line 1:14: mismatched input");
    }

    #[test]
    fn a_non_overlap_failure_is_not_an_overlap() {
        assert!(matches!(
            BiError::from_assignment_check(CompileError::Db("x".into()), "a tag"),
            BiError::Engine(_)
        ));
    }
}
