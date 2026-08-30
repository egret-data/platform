//! HTTP mapping for the canonical `egret_core::Error` (feature = "http").
//!
//! This is the single source of truth for how core errors become HTTP
//! responses. Previously this match arm was copy-pasted in 7 crates.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::Error;

impl Error {
    /// The HTTP status this error maps to.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::FeatureDisabled(_) => StatusCode::FORBIDDEN,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The JSON body every error response carries: `{ "error": "<message>" }`.
///
/// This is the OpenAPI schema counterpart of the body produced by
/// `IntoResponse for Error` above — the single shape returned for every 4xx/5xx
/// across the API. Handlers reference it in their `#[utoipa::path]`
/// `responses(...)` for the error statuses. 5xx messages are redacted to
/// `"internal error"`; client-facing errors pass their `Display` message.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// Public-safe error message.
    #[schema(example = "not found: user")]
    pub error: String,
    /// Machine-readable discriminant, present only where clients must branch on
    /// the reason. Currently only `"feature_disabled"`. Omitted otherwise, so
    /// no existing response changes shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "feature_disabled")]
    pub code: Option<String>,
}

/// Record the detail that the 5xx body deliberately withholds.
///
/// # Why here, in a response conversion
///
/// [`IntoResponse for Error`] is the one funnel every core error passes through
/// on its way to a client, so it is the only place that sees an
/// `Error::Internal` **and** knows it is about to be redacted. Logging at the
/// hundreds of construction sites instead would be hundreds of chances to
/// forget, and the ones forgotten would be exactly the ones nobody notices —
/// which is the failure this exists to fix.
///
/// A conversion that writes to a global logger is a side effect in a
/// pure-looking function, and that cost is real: `into_response` is called
/// directly in unit tests all over this workspace. It is paid down to nothing by
/// how `tracing` works — with no subscriber installed the macro is a relaxed
/// atomic load and a branch, and writes nowhere. A test only observes this if it
/// installs a subscriber, which is precisely how the tests below observe it.
///
/// # Level
///
/// ERROR, and only for `Internal`. Every other variant is the system correctly
/// refusing a request: logging the whole enum at ERROR would bury the 500s this
/// exists to surface under every 404 and every failed login.
fn log_internal(source: &anyhow::Error) {
    // `{:#}` renders the whole `anyhow` context chain on one line
    // ("outer: inner: root"). `{}` would print only the outermost message, which
    // in practice names the wrapper ("query port") and not the cause — the
    // chain IS the useful part. Formatted into a `String` first: a `format_args!`
    // temporary cannot outlive the macro's argument position.
    let chain = format!("{source:#}");
    // The backtrace is a SEPARATE decision from the chain. It is large, and
    // `anyhow` only captures one when `RUST_BACKTRACE` is set, so asking for it
    // unconditionally would append "disabled backtrace" to every line in the
    // default configuration. Emitted as its own field only when one exists.
    let backtrace = source.backtrace();
    if matches!(
        backtrace.status(),
        std::backtrace::BacktraceStatus::Captured
    ) {
        let frames = backtrace.to_string();
        tracing::error!(error = %chain, backtrace = %frames, "internal error");
    } else {
        tracing::error!(error = %chain, "internal error");
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        // Public-safe message: expose the specific message for client-facing
        // errors; hide internals for 5xx.
        let error = match &self {
            // The body is two words BY DESIGN, so this is the last point at
            // which the wrapped detail still exists. Record it or lose it.
            Error::Internal(source) => {
                log_internal(source);
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        let code = match &self {
            Error::FeatureDisabled(_) => Some("feature_disabled".to_string()),
            _ => None,
        };
        // Built from `ErrorBody` rather than an ad-hoc `json!` so the declared
        // OpenAPI schema and the bytes actually sent cannot drift apart.
        (status, Json(ErrorBody { error, code })).into_response()
    }
}

/// The uniform API-error envelope every seam returns from its handlers.
///
/// `Auth` covers authn/authz/validation via the shared core mapping; `Domain`
/// is the seam's own error, which supplies its own `IntoResponse`. A handler
/// returns `Result<T, ApiError<MyDomainError>>` and `?` works on both a
/// `core::Error` and `MyDomainError` (given the seam adds `From<MyDomainError>`).
#[derive(Debug)]
pub enum ApiError<D> {
    Auth(Error),
    Domain(D),
}

impl<D> From<Error> for ApiError<D> {
    fn from(e: Error) -> Self {
        ApiError::Auth(e)
    }
}

impl<D: IntoResponse> IntoResponse for ApiError<D> {
    fn into_response(self) -> Response {
        match self {
            ApiError::Auth(e) => e.into_response(),
            ApiError::Domain(d) => d.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(
            Error::NotFound("x".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            Error::InvalidInput("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(Error::Unauthorized.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(Error::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(
            Error::Conflict("x".into()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            Error::TooManyRequests.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            Error::Internal(anyhow::anyhow!("boom")).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn error_into_response_carries_status() {
        let resp = Error::Forbidden.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = Error::NotFound("user".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn api_error_auth_arm_uses_core_mapping() {
        let e: ApiError<StatusCode> = Error::Forbidden.into();
        assert_eq!(e.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn api_error_domain_arm_delegates_to_d() {
        let e: ApiError<StatusCode> = ApiError::Domain(StatusCode::IM_A_TEAPOT);
        assert_eq!(e.into_response().status(), StatusCode::IM_A_TEAPOT);
    }

    #[tokio::test]
    async fn error_response_body_redacts_internal_and_passes_client_msg() {
        // 5xx must not leak the wrapped anyhow detail.
        let resp = Error::Internal(anyhow::anyhow!("db password = hunter2")).into_response();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "internal error");

        // Client-facing errors pass their Display message through.
        let resp = Error::NotFound("user".into()).into_response();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "not found: user");
    }
}

#[cfg(test)]
mod feature_disabled_tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn feature_disabled_is_403_with_a_machine_readable_code() {
        let resp = Error::FeatureDisabled("notebooks").into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "feature disabled: notebooks");
        assert_eq!(json["code"], "feature_disabled");
    }

    #[tokio::test]
    async fn other_errors_carry_no_code_field() {
        let resp = Error::Forbidden.into_response();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "forbidden");
        assert!(
            json.get("code").is_none(),
            "existing responses must not change shape"
        );
    }
}

/// The other half of the redaction contract.
///
/// [`IntoResponse for Error`] deliberately answers a 5xx with two words. That is
/// the right thing to send a client and the wrong thing to send *nowhere*: with
/// the `anyhow` source dropped on the floor, six live 500s left a pod log ending
/// at `egret listening`. These tests pin that the source is recorded, at what
/// level, and — the part that must never regress — that recording it does not
/// move it into the body.
#[cfg(test)]
mod internal_error_is_logged_tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A `MakeWriter` that appends into a shared buffer, so a test can read back
    /// exactly what a subscriber would have written to stderr.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).expect("log output is utf-8")
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Convert `e` with a subscriber installed; return the response and the log.
    ///
    /// `with_default` is thread-local and every caller below is a `current_thread`
    /// test, so the conversion and the capture are on one thread by construction.
    fn convert(e: Error) -> (Response, String) {
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(cap.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let resp = tracing::subscriber::with_default(sub, || e.into_response());
        (resp, cap.text())
    }

    async fn body_of(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The whole `anyhow` context chain is what an author needs — the outermost
    /// message alone names the wrapper, not the cause. `{:#}` is the rendering
    /// that carries all of it, and this asserts every link, not just the top.
    #[tokio::test]
    async fn an_internal_error_logs_its_whole_context_chain() {
        let source =
            anyhow::anyhow!("refusing to send a per-user engine JWT over a non-TLS connection")
                .context("query port")
                .context("previewing the draft");
        let (resp, log) = convert(Error::Internal(source));

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            log.contains("previewing the draft")
                && log.contains("query port")
                && log.contains("refusing to send a per-user engine JWT over a non-TLS connection"),
            "every link of the chain must be recorded, not just the outermost: {log}"
        );
    }

    /// An `Internal` is by definition unexpected, so it is the one variant that
    /// earns ERROR.
    #[tokio::test]
    async fn an_internal_error_is_recorded_at_error_level() {
        let (_, log) = convert(Error::Internal(anyhow::anyhow!("boom")));
        assert!(
            log.contains("ERROR"),
            "an unexpected 5xx is an ERROR, not a warning: {log}"
        );
    }

    /// A 4xx is the system working. Logging the whole enum at ERROR would bury
    /// the 500s this change exists to surface under every mistyped request.
    #[tokio::test]
    async fn client_errors_are_not_logged_at_error_level() {
        for e in [
            Error::NotFound("user".into()),
            Error::InvalidInput("bad".into()),
            Error::Unauthorized,
            Error::Forbidden,
            Error::FeatureDisabled("notebooks"),
            Error::Conflict("x".into()),
            Error::TooManyRequests,
        ] {
            let (_, log) = convert(e);
            assert!(
                !log.contains("ERROR"),
                "a 4xx is not an error condition of the server: {log}"
            );
        }
    }

    /// The backtrace is decided separately from the chain, and this pins both
    /// halves of that decision at once without depending on how the suite was
    /// invoked: a `backtrace` field appears exactly when `anyhow` actually
    /// captured one. In the default configuration (`RUST_BACKTRACE` unset) that
    /// means no field at all — not the string "disabled backtrace" on every
    /// line, which is what asking for it unconditionally would produce.
    #[tokio::test]
    async fn a_backtrace_field_appears_only_when_one_was_captured() {
        let source = anyhow::anyhow!("boom");
        let captured = matches!(
            source.backtrace().status(),
            std::backtrace::BacktraceStatus::Captured
        );
        let (_, log) = convert(Error::Internal(source));
        assert_eq!(
            log.contains("backtrace="),
            captured,
            "the field must track capture, not be emitted regardless: {log}"
        );
        assert!(
            !log.contains("disabled backtrace"),
            "an uncaptured backtrace is omitted, not rendered as noise: {log}"
        );
    }

    /// The point of the redaction, restated where the logging lives: adding the
    /// log must not have turned the body into a second copy of the source.
    #[tokio::test]
    async fn logging_the_source_does_not_move_it_into_the_body() {
        let (resp, log) = convert(Error::Internal(anyhow::anyhow!("db password = hunter2")));
        assert!(
            log.contains("db password = hunter2"),
            "the operator gets the detail: {log}"
        );
        let body = body_of(resp).await;
        assert_eq!(body["error"], "internal error");
        assert!(
            !serde_json::to_string(&body).unwrap().contains("hunter2"),
            "the client must not: {body}"
        );
    }
}
