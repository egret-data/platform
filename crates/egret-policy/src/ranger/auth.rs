//! Authentication of the Ranger plugin surface (`/ranger/service/...`).
//!
//! Three kinds of caller:
//!
//! * **Operator** — `Authorization: Bearer <ranger.auth_token>` (constant-time
//!   compare; an empty configured token never matches). Reads everything.
//! * **Engine** — `Authorization: Basic engine-<uuid>:<secret>`, checked by the
//!   injected [`EngineCredentialVerifier`]. Reads only the policy profile its
//!   engine is bound to, plus that profile's tag service (enforced in the
//!   download handlers), and never the CRUD routes ([`require_operator`]).
//! * **Anonymous** — only in [`AuthMode::Anonymous`].
//!
//! In [`AuthMode::Required`] anything else is a `401` carrying
//! `WWW-Authenticate: Basic realm="egret-ranger"`, with one body for every
//! cause (absent, malformed, unknown user, wrong secret, wrong token), so a
//! refusal never says which part was wrong. Neither the header nor the secret
//! is ever logged.

use super::routes::RangerState;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use egret_engine_mgmt_api::credential::{engine_id_from_username, EngineIdentity};

/// How the surface treats a request without a valid credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuthMode {
    /// No valid credential ⇒ 401.
    #[default]
    Required,
    /// Anonymous mode serves everyone: a request without a credential, AND one
    /// with a bad credential, is served as [`Caller::Anonymous`]. Only a VALID
    /// credential changes the caller (an engine credential is still bound to
    /// its own profile).
    Anonymous,
}

/// Who the surface authenticated, inserted as a request extension.
#[derive(Clone, Debug)]
pub enum Caller {
    Operator,
    Engine(EngineIdentity),
    Anonymous,
}

/// Authenticate every request on the surface, matched route or not.
///
/// Returns the refusal as a plain `Response` (401 / 500) rather than
/// `Result<Response, Response>`: same wire behaviour, and clippy's
/// `result_large_err` refuses a `Response`-sized `Err`.
pub async fn require_surface_auth(
    State(state): State<RangerState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let caller = if let Some(t) = header.and_then(|v| v.strip_prefix("Bearer ")) {
        (!state.auth_token.is_empty() && ct_eq(t.as_bytes(), state.auth_token.as_bytes()))
            .then_some(Caller::Operator)
    } else if let Some((user, pw)) = header
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(decode_basic)
    {
        match (engine_id_from_username(&user), state.verifier.as_ref()) {
            (Some(id), Some(v)) => match v.verify(id, &pw).await {
                Ok(found) => found.map(Caller::Engine),
                Err(e) => {
                    tracing::error!(engine_id = %id, error = %e, "ranger: engine credential check failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            },
            _ => None,
        }
    } else {
        None
    };
    let caller = match (caller, state.mode) {
        (Some(c), _) => c,
        (None, AuthMode::Anonymous) => Caller::Anonymous,
        (None, AuthMode::Required) => return unauthorized(),
    };
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// Refuse engine credentials: the CRUD routes are for operators (and for
/// everyone in anonymous mode). Runs inside [`require_surface_auth`].
pub async fn require_operator(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    match req.extensions().get::<Caller>() {
        Some(Caller::Operator | Caller::Anonymous) => Ok(next.run(req).await),
        // `None` cannot happen behind `require_surface_auth`; refuse rather than serve.
        Some(Caller::Engine(_)) | None => Err(StatusCode::FORBIDDEN),
    }
}

fn unauthorized() -> Response {
    let mut r = StatusCode::UNAUTHORIZED.into_response();
    r.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"egret-ranger\""),
    );
    r
}

/// `base64(user:password)` → `(user, password)`, split at the FIRST `:` (a
/// password may contain `:`). Anything malformed is `None`, which the caller
/// turns into the same 401 as a wrong password — never a 400 that reveals parsing.
fn decode_basic(b64: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let s = String::from_utf8(raw).ok()?;
    let (user, pw) = s.split_once(':')?;
    Some((user.to_owned(), pw.to_owned()))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(s: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn decode_basic_splits_at_the_first_colon() {
        assert_eq!(
            decode_basic(&enc("engine-x:a:b")),
            Some(("engine-x".into(), "a:b".into()))
        );
        assert_eq!(
            decode_basic(&enc("user:")),
            Some(("user".into(), "".into()))
        );
    }

    #[test]
    fn decode_basic_refuses_malformed_input() {
        assert_eq!(decode_basic("!!!"), None);
        assert_eq!(decode_basic(&enc("no-colon")), None);
    }

    #[test]
    fn ct_eq_compares_whole_values() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
