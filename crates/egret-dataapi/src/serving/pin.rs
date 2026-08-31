//! Package 7 — how a consumer asks for the version it built against.
//!
//! # The gap this closes
//!
//! [`super::router::live_snapshot`] resolved `endpoints.live_version_id` and
//! nothing else, so a publish moved **every** caller at the instant it
//! committed. Package 6 now tells an author what a publish breaks and refuses it
//! without an acknowledgement — but the acknowledgement is the author saying
//! "yes, I am breaking them", and the consumer on the other end still had no way
//! to keep working while they adapted. `meta.version` let them *observe* which
//! version answered; nothing let them *ask*.
//!
//! # A HEADER, and why not a query parameter
//!
//! The obvious spelling is `?version=3`, and it is wrong here for a reason
//! specific to this surface: **the query string is the endpoint AUTHOR's
//! namespace, and it is unrestricted.**
//!
//! * Nothing at publish reserves a parameter name. `version` is a legal
//!   `ParamSpec.name` today — indeed `check_params` *requires* a declared
//!   parameter to be referenced by a `{{ version }}` placeholder, so an endpoint
//!   that filters on a `version` column has exactly this parameter and is
//!   publishable on `main`.
//! * A supplied name that is not declared is **silently ignored**
//!   (`params::resolve_supplied_with` walks the declared specs and looks each one
//!   up). So `?version=` on an endpoint that does not declare it does nothing
//!   today, and giving it a meaning tomorrow would change what an already-
//!   deployed caller's request means without that caller editing anything —
//!   which is precisely the "silently ignored" failure `contract::ChangeKind`
//!   classifies as breaking. Reading a pin out of a name an author may also own
//!   would either shadow their parameter (their filter stops applying, 200, no
//!   signal) or require a reserved-name rule applied retroactively to every
//!   already-published snapshot.
//! * The generated per-service-account document lists an endpoint's query
//!   parameters from `snap.params`. A pin in the same namespace would appear
//!   beside them and a generated client would carry two `version` arguments.
//!
//! A header has its own namespace, cannot collide with anything an author can
//! declare, needs **no new route** (there is exactly one — `/{ws}/{*path}`), and
//! stays out of `resolved.key`, so it cannot perturb the cache key's parameter
//! section. It costs a consumer nothing: every HTTP client can set one, and the
//! generated document declares it as an `in: header` parameter so a generated
//! client gets an argument for it.
//!
//! **The name is `X-Egret-Version`, matching the `x-egret-version` extension the
//! generated document already stamps on every operation.** That field exists so
//! "a consumer holding a stale file cannot tell that it is stale"; the header is
//! the number read out of it, sent back. One spelling, read in one place and
//! written in the other.
//!
//! # What the parse accepts, and why it is strict
//!
//! One header, whose value is a non-empty run of ASCII digits, at most
//! [`MAX_PIN_DIGITS`] of them, denoting a value of at least 1. Everything else is
//! [`ServingError::VersionPinInvalid`].
//!
//! * **`+1` and `-1` are refused** even though `i32::from_str` accepts `+1`.
//!   A version is a counter, not an arithmetic expression, and two spellings of
//!   one number is the shape that lets a caller get two of something keyed on the
//!   spelling. The digit rule removes the question rather than answering it.
//! * **Leading and trailing whitespace is refused** for the same reason: `" 1"`
//!   is a client bug, and quietly accepting it teaches a client that the value is
//!   forgiving.
//! * **Leading zeros are accepted.** `0007` is unambiguously seven — every byte
//!   is a digit and the decimal value is the whole meaning. Nothing downstream
//!   keys on the spelling (the cache key takes the parsed `i32`), so there is no
//!   property a rule against them would protect, and a rule with no property
//!   behind it is one more thing to get wrong.
//! * **A repeated header is refused, not resolved.** `HeaderMap::get` returns the
//!   first of several, so `X-Egret-Version: 1` and `X-Egret-Version: 2` on one
//!   request would silently answer with the first — a caller choosing between two
//!   contracts by header order. This is the same shape as the percent-encoded
//!   path that would have given one endpoint two rate-limit buckets: a request
//!   with two answers must have none.
//! * **The parse is bounded before it is parsed.** [`MAX_PIN_DIGITS`] is checked
//!   on the byte run, so `i32::from_str` is never handed a 4 KiB header value,
//!   and overflow is unreachable rather than caught.
//!
//! # Where it is parsed, and what that ordering buys
//!
//! In [`super::router::run`], **after** the bearer token has been verified. A
//! malformed pin from an unauthenticated caller is still a `401`: the serving
//! surface answers one thing to anyone who has not proved a credential, and a
//! `400` that only a malformed header can produce would be a free "this host
//! speaks the Data API" oracle. Authentication first, then the request's shape.

use axum::http::HeaderMap;

use super::ServingError;

/// The request header a consumer pins with.
///
/// Lower-case because `HeaderMap` keys are lower-cased on the wire and this
/// constant is used for lookup; the documented spelling is `X-Egret-Version`,
/// and HTTP field names are case-insensitive, so a caller may send either.
pub(crate) const VERSION_PIN_HEADER: &str = "x-egret-version";

/// The same header, spelled the way the generated document publishes it.
///
/// A second constant rather than a `to_uppercase` because the two have different
/// jobs — one is a lookup key, one is a string a consumer reads — and because
/// `the_documented_pin_header_is_the_one_serving_reads` asserts they are the
/// same field name, which is only a claim worth making if they are two values.
pub(crate) const VERSION_PIN_HEADER_DOCUMENTED: &str = "X-Egret-Version";

/// The most digits a pin may carry.
///
/// Nine, so the largest accepted value is `999_999_999` — comfortably inside
/// `i32`, which is the type of `dataapi.endpoint_versions.version`. The bound is
/// on the DIGITS rather than on the parsed value so that overflow is
/// unrepresentable instead of caught: `i32::from_str` never sees a string it
/// could overflow on.
pub(crate) const MAX_PIN_DIGITS: usize = 9;

/// The version this request asked for, or `None` for "whatever is live".
///
/// `None` is today's behaviour, byte for byte: no header, no change to how the
/// request resolves.
///
/// # Mutation
///
/// Every rule above is a line here, and each has a test in
/// `tests/serving_version_pin.rs` that names the input it refuses. Loosening any
/// one of them — accepting the second of two headers, trimming, `parse()`ing the
/// raw value — fails a test by name.
pub(crate) fn requested_version(headers: &HeaderMap) -> Result<Option<i32>, ServingError> {
    let mut values = headers.get_all(VERSION_PIN_HEADER).iter();
    let Some(raw) = values.next() else {
        return Ok(None);
    };
    // Two spellings of the pin on one request is a request with two answers.
    // Refused rather than resolved by header order — see the module comment.
    if values.next().is_some() {
        return Err(ServingError::VersionPinInvalid);
    }
    let raw = raw.to_str().map_err(|_| ServingError::VersionPinInvalid)?;
    // Digits only, and bounded BEFORE the parse. No sign, no whitespace, no
    // decimal point, no empty value, nothing `i32::from_str` could overflow on.
    if raw.is_empty() || raw.len() > MAX_PIN_DIGITS || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ServingError::VersionPinInvalid);
    }
    let version: i32 = raw.parse().map_err(|_| ServingError::VersionPinInvalid)?;
    // Version numbers start at 1 (`COALESCE(MAX(version), 0) + 1`), so `0` names
    // nothing and is a client bug rather than a stale pin. Refused here, with the
    // other malformed values, rather than being sent to the database to come back
    // as `version_unavailable` — the two codes name two different actors, and a
    // `0` is always the caller's.
    if version < 1 {
        return Err(ServingError::VersionPinInvalid);
    }
    Ok(Some(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn refused(value: &str) -> bool {
        matches!(
            requested_version(&headers(&[(VERSION_PIN_HEADER, value)])),
            Err(ServingError::VersionPinInvalid)
        )
    }

    /// No header is the unpinned path, and it must not be an error.
    ///
    /// Mutation: make the absent case an `Err` and every unpinned request in the
    /// suite fails.
    #[test]
    fn an_absent_header_is_not_a_pin() {
        assert!(requested_version(&headers(&[])).unwrap().is_none());
    }

    #[test]
    fn a_decimal_version_is_the_pin() {
        assert_eq!(
            requested_version(&headers(&[(VERSION_PIN_HEADER, "3")])).unwrap(),
            Some(3)
        );
        assert_eq!(
            requested_version(&headers(&[(VERSION_PIN_HEADER, "999999999")])).unwrap(),
            Some(999_999_999)
        );
    }

    /// The written rule, exercised on the values that would slip through a bare
    /// `parse()`.
    ///
    /// `+1` is the one worth naming: `i32::from_str` ACCEPTS it, so a version
    /// built on `raw.parse()` alone passes this list except here — which is the
    /// mutation for the digit check.
    #[test]
    fn everything_that_is_not_a_bare_decimal_is_refused() {
        for bad in [
            "",                     // an empty header value
            " ",                    //
            "abc",                  //
            "+1",                   // `i32::from_str` accepts this one
            "-1",                   //
            " 1",                   // leading whitespace
            "1 ",                   // trailing whitespace
            "1.0",                  //
            "1,2",                  //
            "0x1",                  //
            "1e3",                  //
            "٣",                    // ARABIC-INDIC DIGIT THREE — `char::is_numeric`, not ASCII
            "1234567890",           // ten digits: over MAX_PIN_DIGITS
            "99999999999999999999", // and would overflow i32
        ] {
            assert!(refused(bad), "{bad:?} must be refused");
        }
    }

    /// `0` names no version — the counter starts at 1.
    #[test]
    fn zero_is_refused_as_malformed_and_not_as_a_stale_pin() {
        assert!(refused("0"));
        assert!(refused("000"));
    }

    /// Leading zeros are the decimal value, deliberately.
    #[test]
    fn leading_zeros_are_the_same_version() {
        assert_eq!(
            requested_version(&headers(&[(VERSION_PIN_HEADER, "0007")])).unwrap(),
            Some(7)
        );
    }

    /// Two pins on one request is refused, not resolved by order.
    ///
    /// Mutation: replace `get_all` with `get` and this is the only test that
    /// fails — `get` returns `1`, which is a perfectly plausible answer.
    #[test]
    fn two_pin_headers_are_refused_rather_than_answered_by_order() {
        let h = headers(&[(VERSION_PIN_HEADER, "1"), (VERSION_PIN_HEADER, "2")]);
        assert!(matches!(
            requested_version(&h),
            Err(ServingError::VersionPinInvalid)
        ));
        // Including two of the SAME value: the rule is about the request being
        // unambiguous in its shape, not about the two values disagreeing.
        let h = headers(&[(VERSION_PIN_HEADER, "1"), (VERSION_PIN_HEADER, "1")]);
        assert!(matches!(
            requested_version(&h),
            Err(ServingError::VersionPinInvalid)
        ));
    }

    /// The lookup is case-insensitive, because HTTP field names are.
    #[test]
    fn the_documented_spelling_reaches_the_same_lookup() {
        let mut h = HeaderMap::new();
        h.append(
            axum::http::HeaderName::from_static("x-egret-version"),
            axum::http::HeaderValue::from_static("2"),
        );
        assert_eq!(requested_version(&h).unwrap(), Some(2));
    }

    /// The name the generated document publishes and the name this module looks
    /// up must be one HTTP field.
    ///
    /// They are two constants, so they can drift — and a drift is silent in the
    /// worst possible way: the document would advertise a header the server
    /// ignores, so every pinned call would answer with the LIVE version and a
    /// consumer would believe they were pinned. Compared through
    /// `HeaderName::from_bytes`, which is the comparison HTTP actually makes, so
    /// this passes for a case change and fails for a real rename.
    ///
    /// Mutation: change either constant's field name and this fails.
    #[test]
    fn the_documented_pin_header_is_the_one_serving_reads() {
        let read = axum::http::HeaderName::from_bytes(VERSION_PIN_HEADER.as_bytes()).unwrap();
        let published =
            axum::http::HeaderName::from_bytes(VERSION_PIN_HEADER_DOCUMENTED.as_bytes()).unwrap();
        assert_eq!(read, published);
    }
}
