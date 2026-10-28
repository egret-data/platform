//! An HTTP executor for user-chosen URLs.
//!
//! Every property below closes a way around [`crate::guard::vet`]:
//!
//! - **Pinning.** The addresses `vet` approved are handed to the client with
//!   `ClientBuilder::resolve_to_addrs`, so the connection goes to an approved
//!   address, not to whatever a second DNS lookup returns (DNS rebinding).
//!   A pinned client is per host, so one client is built per hop.
//! - **No proxy.** reqwest reads `HTTP(S)_PROXY` from the environment by
//!   default; through a proxy, DNS happens at the proxy and pinning means
//!   nothing. `.no_proxy()` on every client.
//! - **Manual redirects.** `redirect::Policy::none()`, then each `Location` is
//!   resolved against the current URL and goes through `vet` again, scheme
//!   included, up to [`EgressOptions::max_redirects`] hops. A public URL that
//!   answers `302 Location: http://169.254.169.254/` is refused, never dialled.
//! - **Credentials stay with their origin.** When a hop changes scheme, host
//!   or port, `Authorization`, `Proxy-Authorization`, `Cookie`, every header
//!   whose name contains `key`, `token`, `auth`, `secret` or `cookie`
//!   (case-insensitive: `X-Api-Key`, `X-Auth-Token`, ...) and `Host` are
//!   removed and stay removed for the rest of the chain. A custom credential
//!   header with another name (`X-Signature`, say) still follows the
//!   redirect.
//! - **The caller cannot set the framing or the target.** `Host`,
//!   `Content-Length`, `Transfer-Encoding`, `Connection`, `Upgrade`, `TE`,
//!   `Keep-Alive`, `Trailer` and every `Proxy-*` header are dropped from the
//!   request ([`FORBIDDEN_REQUEST_HEADERS`]): hyper writes `Host` from the
//!   vetted URL and the framing from the body. A caller-supplied `Host` would
//!   reach other virtual hosts behind an allowlisted address, and the
//!   framing headers are request-smuggling primitives.
//! - **Error texts carry no query.** A refused destination is quoted as
//!   scheme, host, port and path ([`redact`]); an unparseable URL or
//!   `Location` is not quoted at all.
//! - **Method rules** follow the Fetch standard, as Apache HttpClient (Kestra)
//!   does: 303 turns any method but HEAD into GET; 301/302 turn POST into GET;
//!   307/308 keep the method and the body. A dropped body takes its
//!   `Content-*`/`Transfer-Encoding` headers with it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::header::{
    HeaderMap, HeaderName, AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE,
    HOST, LOCATION, PROXY_AUTHORIZATION, TRANSFER_ENCODING,
};
use reqwest::{Method, StatusCode};
use url::Url;

use crate::guard::{vet_url, DestinationError, Resolver, SsrfPolicy};

/// Reqwest's own default, and Kestra's (Apache HttpClient 5: 50, but egret
/// keeps reqwest's 10 — D1).
pub const DEFAULT_MAX_REDIRECTS: u32 = 10;

/// Headers that carry credentials and must not follow a request to another
/// origin. [`is_credential_like`] adds the custom ones.
const CREDENTIAL_HEADERS: [HeaderName; 3] = [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE];

/// Header names a caller may not send (hop-by-hop, framing, and the target
/// host); every `Proxy-*` name is refused too ([`is_forbidden_request_header`]).
pub const FORBIDDEN_REQUEST_HEADERS: [&str; 8] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "keep-alive",
    "trailer",
];

/// Whether a caller-supplied header `name` must not reach the wire
/// ([`FORBIDDEN_REQUEST_HEADERS`] or a `Proxy-*` name), case-insensitive.
pub fn is_forbidden_request_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FORBIDDEN_REQUEST_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-")
}

/// Whether a header name looks like it carries a credential: it contains
/// `key`, `token`, `auth`, `secret` or `cookie`, case-insensitive. Such a
/// header is dropped on a cross-origin redirect (final review M7).
pub fn is_credential_like(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["key", "token", "auth", "secret", "cookie"]
        .iter()
        .any(|w| lower.contains(w))
}

#[derive(Debug, Clone)]
pub struct EgressOptions {
    /// Cap on establishing one TCP (+TLS) connection.
    pub connect_timeout: Duration,
    /// Cap on the whole exchange: every hop, and reading the final body. `None`
    /// leaves only the connect timeout (an SSE stream has no natural end).
    pub timeout: Option<Duration>,
    /// Cap on the time between two reads of the response (Kestra's
    /// `options.timeout.readIdleTimeout`). `None` leaves only `timeout`.
    pub read_timeout: Option<Duration>,
    /// Redirect hops followed before giving up.
    pub max_redirects: u32,
    /// `false` returns a 3xx response as is (Kestra's `followRedirects`).
    pub follow_redirects: bool,
}

impl Default for EgressOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            timeout: Some(Duration::from_secs(60)),
            read_timeout: None,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            follow_redirects: true,
        }
    }
}

/// What to send. Kept minimal; the caller builds headers (auth included).
#[derive(Debug, Clone)]
pub struct RequestSpec {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
}

impl RequestSpec {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    /// The initial URL, or a redirect target, was refused by the guard.
    #[error("{url}: {source}")]
    Destination {
        url: String,
        #[source]
        source: DestinationError,
    },
    /// A `Location` header that is not a URL, even relative to the current
    /// one. Not quoted: it may carry a token.
    #[error("redirect location is not a valid url")]
    BadRedirect,
    #[error("too many redirects (more than {0})")]
    TooManyRedirects(u32),
    #[error("request timed out")]
    Timeout,
    #[error("request failed: {0}")]
    Transport(String),
}

impl EgressError {
    fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            EgressError::Timeout
        } else {
            // `without_url`: the URL may carry userinfo or a token in its query;
            // the caller knows the URL and decides what to show.
            EgressError::Transport(e.without_url().to_string())
        }
    }
}

/// Sends [`RequestSpec`]s under an [`SsrfPolicy`].
#[derive(Clone)]
pub struct EgressClient {
    policy: SsrfPolicy,
    resolver: Arc<dyn Resolver>,
    options: EgressOptions,
}

impl EgressClient {
    pub fn new(policy: SsrfPolicy, resolver: Arc<dyn Resolver>, options: EgressOptions) -> Self {
        Self {
            policy,
            resolver,
            options,
        }
    }

    pub fn options(&self) -> &EgressOptions {
        &self.options
    }

    /// Send `spec`, following redirects by hand. Returns the final response
    /// with its body unread, so the caller can stream it (`chunk()`); the
    /// total timeout, if any, still bounds that read. `Response::url()` is the
    /// URL of the last hop.
    pub async fn send(&self, spec: RequestSpec) -> Result<reqwest::Response, EgressError> {
        send(&self.policy, self.resolver.as_ref(), &self.options, spec).await
    }
}

/// [`EgressClient::send`] for a caller that holds its resolver by reference
/// (alert webhook delivery). Same guarantees, hop for hop.
pub async fn send(
    policy: &SsrfPolicy,
    resolver: &dyn Resolver,
    options: &EgressOptions,
    spec: RequestSpec,
) -> Result<reqwest::Response, EgressError> {
    Hops {
        policy,
        resolver,
        options,
    }
    .send(spec)
    .await
}

struct Hops<'a> {
    policy: &'a SsrfPolicy,
    resolver: &'a dyn Resolver,
    options: &'a EgressOptions,
}

impl Hops<'_> {
    async fn send(&self, spec: RequestSpec) -> Result<reqwest::Response, EgressError> {
        let deadline = self.options.timeout.map(|t| Instant::now() + t);
        let RequestSpec {
            mut method,
            url,
            mut headers,
            mut body,
        } = spec;
        // Not quoted: a URL that does not parse cannot be redacted, and it
        // may carry a password or a token (final review M3).
        let mut current = Url::parse(&url).map_err(|_| EgressError::Destination {
            url: "the request URL".into(),
            source: DestinationError::BadUrl,
        })?;
        drop_forbidden(&mut headers);
        let mut hops = 0u32;

        loop {
            let res = self
                .send_one(&current, &method, &headers, body.clone(), deadline)
                .await?;

            let status = res.status();
            let rule = redirect_rule(status);
            if rule == Redirect::No || !self.options.follow_redirects {
                return Ok(res);
            }
            let Some(location) = res.headers().get(LOCATION) else {
                return Ok(res);
            };
            let location = location.to_str().map_err(|_| EgressError::BadRedirect)?;
            let next = current
                .join(location)
                .map_err(|_| EgressError::BadRedirect)?;

            hops += 1;
            if hops > self.options.max_redirects {
                return Err(EgressError::TooManyRedirects(self.options.max_redirects));
            }

            if rule == Redirect::ToGet && method_becomes_get(status, &method) {
                method = Method::GET;
                body = None;
                for h in [
                    CONTENT_TYPE,
                    CONTENT_LENGTH,
                    CONTENT_ENCODING,
                    TRANSFER_ENCODING,
                ] {
                    headers.remove(h);
                }
            }
            if next.origin() != current.origin() {
                for h in CREDENTIAL_HEADERS {
                    headers.remove(h);
                }
                let custom: Vec<HeaderName> = headers
                    .keys()
                    .filter(|n| is_credential_like(n.as_str()))
                    .cloned()
                    .collect();
                for h in custom {
                    headers.remove(h);
                }
                headers.remove(HOST);
            }
            current = next;
        }
    }

    /// One hop: vet, pin, send. No redirect handling here.
    async fn send_one(
        &self,
        url: &Url,
        method: &Method,
        headers: &HeaderMap,
        body: Option<Bytes>,
        deadline: Option<Instant>,
    ) -> Result<reqwest::Response, EgressError> {
        let addrs = vet_url(url, self.policy, self.resolver)
            .await
            .map_err(|source| EgressError::Destination {
                url: redact(url),
                source,
            })?;
        // `vet_url` refused a URL without a host, so this is present.
        let host = url.host_str().unwrap_or_default();

        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(self.options.connect_timeout)
            .resolve_to_addrs(host, &addrs);
        if let Some(t) = self.options.read_timeout {
            builder = builder.read_timeout(t);
        }
        let client = builder
            .build()
            .map_err(|e| EgressError::Transport(e.to_string()))?;

        let mut req = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if let Some(b) = body {
            req = req.body(b);
        }
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(EgressError::Timeout);
            }
            req = req.timeout(left);
        }
        req.send().await.map_err(EgressError::from_reqwest)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Redirect {
    No,
    /// 301/302/303: the method may change to GET.
    ToGet,
    /// 307/308: method and body are kept.
    Keep,
}

fn redirect_rule(status: StatusCode) -> Redirect {
    match status {
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER => {
            Redirect::ToGet
        }
        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => Redirect::Keep,
        _ => Redirect::No,
    }
}

fn method_becomes_get(status: StatusCode, method: &Method) -> bool {
    if status == StatusCode::SEE_OTHER {
        *method != Method::GET && *method != Method::HEAD
    } else {
        *method == Method::POST
    }
}

/// The URL without userinfo, query or fragment, for error texts (final
/// review I5: a token in the query must not reach a message).
fn redact(url: &Url) -> String {
    let mut u = url.clone();
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

/// Drop every [`is_forbidden_request_header`] name from `headers`.
fn drop_forbidden(headers: &mut HeaderMap) {
    let names: Vec<HeaderName> = headers
        .keys()
        .filter(|n| is_forbidden_request_header(n.as_str()))
        .cloned()
        .collect();
    for n in names {
        headers.remove(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_rules_follow_fetch() {
        use StatusCode as S;
        assert!(method_becomes_get(S::SEE_OTHER, &Method::POST));
        assert!(method_becomes_get(S::SEE_OTHER, &Method::PUT));
        assert!(method_becomes_get(S::SEE_OTHER, &Method::DELETE));
        assert!(!method_becomes_get(S::SEE_OTHER, &Method::HEAD));
        assert!(!method_becomes_get(S::SEE_OTHER, &Method::GET));
        for s in [S::MOVED_PERMANENTLY, S::FOUND] {
            assert!(method_becomes_get(s, &Method::POST));
            assert!(!method_becomes_get(s, &Method::PUT));
            assert!(!method_becomes_get(s, &Method::DELETE));
        }
        assert_eq!(redirect_rule(S::TEMPORARY_REDIRECT), Redirect::Keep);
        assert_eq!(redirect_rule(S::PERMANENT_REDIRECT), Redirect::Keep);
        for s in [S::MULTIPLE_CHOICES, S::NOT_MODIFIED, S::USE_PROXY, S::OK] {
            assert_eq!(redirect_rule(s), Redirect::No, "{s}");
        }
    }

    #[test]
    fn redact_drops_userinfo_query_and_fragment() {
        let u = Url::parse("https://user:pw@api.example.com:8443/p?q=1#f").unwrap();
        assert_eq!(redact(&u), "https://api.example.com:8443/p");
    }

    #[test]
    fn forbidden_and_credential_like_names() {
        for n in [
            "Host",
            "content-length",
            "Transfer-Encoding",
            "Connection",
            "Upgrade",
            "TE",
            "Keep-Alive",
            "Trailer",
            "Proxy-Authorization",
            "proxy-connection",
        ] {
            assert!(is_forbidden_request_header(n), "{n}");
        }
        for n in ["Accept", "X-Host", "Content-Type", "Authorization"] {
            assert!(!is_forbidden_request_header(n), "{n}");
        }
        for n in [
            "X-Api-Key",
            "x-auth-token",
            "X-SECRET",
            "Set-Cookie",
            "Authorization",
        ] {
            assert!(is_credential_like(n), "{n}");
        }
        for n in ["Accept", "X-Request-Id", "Content-Type"] {
            assert!(!is_credential_like(n), "{n}");
        }
    }
}
