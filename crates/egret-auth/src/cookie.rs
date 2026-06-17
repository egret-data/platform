//! Session + CSRF cookie construction and the CSRF token generator.

use axum_extra::extract::cookie::{Cookie, SameSite};

pub const SESSION_COOKIE: &str = "egret_session";
pub const CSRF_COOKIE: &str = "egret_csrf";
pub const CSRF_HEADER: &str = "x-egret-csrf";

/// Build the session cookie (HttpOnly).
pub fn session_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// Build the CSRF cookie (NOT HttpOnly — the SPA reads it to echo in a header).
pub fn csrf_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, value))
        .http_only(false)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// Build an expired cookie to clear one on logout.
pub fn clear_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .max_age(time::Duration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_is_http_only_lax() {
        let c = session_cookie("tok".into(), true, 3600);
        assert_eq!(c.name(), SESSION_COOKIE);
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
        assert_eq!(c.secure(), Some(true));
    }

    #[test]
    fn csrf_cookie_is_readable_by_js() {
        let c = csrf_cookie("csrf".into(), false, 3600);
        assert_eq!(c.http_only(), Some(false));
    }
}
