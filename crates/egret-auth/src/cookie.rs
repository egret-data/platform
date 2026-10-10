//! Session + CSRF cookie construction and the CSRF token generator.
//!
//! # Names depend on `session.secure`
//!
//! When cookies are `Secure` they are named `__Host-egret_session` / `__Host-egret_csrf`.
//! The `__Host-` prefix makes a browser refuse the cookie unless it is `Secure`, has
//! `Path=/` and carries **no `Domain`**, so a page on a sibling host (the apps origin
//! runs arbitrary author JavaScript, spec §6.1) cannot set a cookie of that name for
//! egret by scoping it to the parent domain ("cookie tossing"). Over plain http, where
//! `Secure` cookies do not exist, the plain names stay.
//!
//! The reader is as strict as the writer: in a secure deployment ONLY the `__Host-`
//! names count, so a tossed plain `egret_csrf` can neither satisfy the double-submit
//! check nor can a tossed `egret_session` log the victim in as someone else.

use std::sync::atomic::{AtomicBool, Ordering};

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

pub const SESSION_COOKIE: &str = "egret_session";
pub const CSRF_COOKIE: &str = "egret_csrf";
pub const HOST_SESSION_COOKIE: &str = "__Host-egret_session";
pub const HOST_CSRF_COOKIE: &str = "__Host-egret_csrf";
pub const CSRF_HEADER: &str = "x-egret-csrf";

/// Whether this process issues and reads the `__Host-` names. Set once at boot from
/// `session.secure`; the extractors and the CSRF layer consult it so they need no
/// state plumbed through every router. Everything that DECIDES takes `secure`
/// explicitly ([`session_cookie_name`], [`read_session`], …), which is what the unit
/// tests exercise.
static SECURE_COOKIES: AtomicBool = AtomicBool::new(false);

pub fn set_secure_cookies(secure: bool) {
    SECURE_COOKIES.store(secure, Ordering::Relaxed);
}

pub fn secure_cookies() -> bool {
    SECURE_COOKIES.load(Ordering::Relaxed)
}

pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        HOST_SESSION_COOKIE
    } else {
        SESSION_COOKIE
    }
}

pub fn csrf_cookie_name(secure: bool) -> &'static str {
    if secure {
        HOST_CSRF_COOKIE
    } else {
        CSRF_COOKIE
    }
}

/// The session cookie as this deployment names it, and no other spelling.
pub fn read_session(jar: &CookieJar, secure: bool) -> Option<&Cookie<'static>> {
    jar.get(session_cookie_name(secure))
}

/// The CSRF cookie as this deployment names it, and no other spelling.
pub fn read_csrf(jar: &CookieJar, secure: bool) -> Option<&Cookie<'static>> {
    jar.get(csrf_cookie_name(secure))
}

/// Build the session cookie (HttpOnly).
pub fn session_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((session_cookie_name(secure), value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// Build the CSRF cookie (NOT HttpOnly — the SPA reads it to echo in a header).
pub fn csrf_cookie(value: String, secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((csrf_cookie_name(secure), value))
        .http_only(false)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// Build an expired cookie to clear one on logout.
///
/// A browser ignores a `__Host-` `Set-Cookie` that lacks `Secure`, so clearing one
/// without it would leave the cookie in place.
pub fn clear_cookie(name: &'static str) -> Cookie<'static> {
    let mut b = Cookie::build((name, "")).path("/");
    if name.starts_with("__Host-") {
        b = b.secure(true);
    }
    b.max_age(time::Duration::seconds(0)).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_is_http_only_lax() {
        let c = session_cookie("tok".into(), true, 3600);
        assert_eq!(c.name(), HOST_SESSION_COOKIE);
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
        assert_eq!(c.secure(), Some(true));
    }

    #[test]
    fn a_secure_deployment_uses_host_prefixed_names_that_cannot_carry_a_domain() {
        let s = session_cookie("tok".into(), true, 3600);
        let c = csrf_cookie("csrf".into(), true, 3600);
        assert_eq!(s.name(), "__Host-egret_session");
        assert_eq!(c.name(), "__Host-egret_csrf");
        for cookie in [&s, &c] {
            // The `__Host-` rules: Secure, Path=/, and no Domain attribute at all.
            assert_eq!(cookie.secure(), Some(true));
            assert_eq!(cookie.path(), Some("/"));
            assert_eq!(cookie.domain(), None);
        }
    }

    #[test]
    fn a_plain_http_deployment_keeps_the_plain_names() {
        assert_eq!(
            session_cookie("t".into(), false, 60).name(),
            "egret_session"
        );
        assert_eq!(csrf_cookie("t".into(), false, 60).name(), "egret_csrf");
    }

    #[test]
    fn a_secure_deployment_reads_only_host_prefixed_cookies() {
        // A tossed plain-name cookie (set with `Domain=` from a sibling host) must
        // not authenticate or satisfy CSRF once cookies are Secure.
        let jar = CookieJar::new()
            .add(Cookie::new("egret_session", "tossed"))
            .add(Cookie::new("egret_csrf", "tossed"));
        assert!(read_session(&jar, true).is_none());
        assert!(read_csrf(&jar, true).is_none());
        let jar = jar
            .add(Cookie::new("__Host-egret_session", "real"))
            .add(Cookie::new("__Host-egret_csrf", "real"));
        assert_eq!(read_session(&jar, true).unwrap().value(), "real");
        assert_eq!(read_csrf(&jar, true).unwrap().value(), "real");
    }

    #[test]
    fn a_plain_deployment_reads_only_plain_cookies() {
        let jar = CookieJar::new()
            .add(Cookie::new("__Host-egret_session", "other"))
            .add(Cookie::new("egret_session", "mine"));
        assert_eq!(read_session(&jar, false).unwrap().value(), "mine");
    }

    #[test]
    fn clearing_a_host_prefixed_cookie_is_itself_secure() {
        // A browser ignores a `__Host-` Set-Cookie that lacks Secure, so a clear
        // without it would leave the cookie in place.
        assert_eq!(clear_cookie(HOST_SESSION_COOKIE).secure(), Some(true));
        assert_eq!(clear_cookie(SESSION_COOKIE).secure(), None);
    }

    #[test]
    fn csrf_cookie_is_readable_by_js() {
        let c = csrf_cookie("csrf".into(), false, 3600);
        assert_eq!(c.http_only(), Some(false));
    }
}
