//! `EgressClient` against two real local servers on 127.0.0.1.
//!
//! The servers are loopback, which the guard refuses. Positive tests reach them
//! through invented names (`a.test`, `b.test`) that only the injected resolver
//! knows and that the allowlist names — so a success also proves the vetted
//! address was pinned (no real DNS could answer for `a.test`). Every refusal is
//! proved with the target NOT on the allowlist, and where the target is one of
//! the live servers, by showing it received nothing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use egret_egress::{
    DestinationError, EgressClient, EgressError, EgressOptions, RequestSpec, Resolver, SsrfPolicy,
};

/// What a server saw: path, method, headers, body.
type Seen = Arc<Mutex<Vec<(String, Method, HeaderMap, String)>>>;

struct Server {
    port: u16,
    seen: Seen,
}

impl Server {
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    fn last(&self) -> (String, Method, HeaderMap, String) {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request")
    }
}

async fn record(
    State(seen): State<Seen>,
    uri: axum::http::Uri,
    method: Method,
    headers: HeaderMap,
    body: String,
) {
    seen.lock()
        .unwrap()
        .push((uri.path().to_string(), method, headers, body));
}

async fn start() -> Server {
    let seen: Seen = Arc::default();
    let app = Router::new()
        .route(
            "/echo",
            any(
                |s: State<Seen>, uri, m: Method, h: HeaderMap, b: String| async move {
                    record(s, uri, m.clone(), h, b.clone()).await;
                    format!("{m} {b}")
                },
            ),
        )
        // `/redirect?status=302&to=<location>`
        .route(
            "/redirect",
            any(
                |s: State<Seen>,
                 uri,
                 m: Method,
                 h: HeaderMap,
                 Query(q): Query<HashMap<String, String>>,
                 b: String| async move {
                    record(s, uri, m, h, b).await;
                    let status: u16 = q.get("status").map_or(302, |s| s.parse().unwrap());
                    Response::builder()
                        .status(status)
                        .header("location", q.get("to").cloned().unwrap_or_default())
                        .body(axum::body::Body::empty())
                        .unwrap()
                },
            ),
        )
        .route(
            "/loop",
            any(
                |s: State<Seen>, uri, m: Method, h: HeaderMap, b: String| async move {
                    record(s, uri, m, h, b).await;
                    (StatusCode::FOUND, [("location", "/loop")]).into_response()
                },
            ),
        )
        .route(
            "/slow",
            any(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { port, seen }
}

/// Fixed answers; anything else is NXDOMAIN.
struct FakeDns(HashMap<&'static str, IpAddr>);

#[async_trait::async_trait]
impl Resolver for FakeDns {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, std::io::Error> {
        self.0
            .get(host)
            .map(|ip| vec![SocketAddr::new(*ip, port)])
            .ok_or_else(|| std::io::Error::other("NXDOMAIN"))
    }
}

fn dns() -> Arc<FakeDns> {
    Arc::new(FakeDns(HashMap::from([
        ("a.test", "127.0.0.1".parse().unwrap()),
        ("b.test", "127.0.0.1".parse().unwrap()),
        ("internal.test", "10.0.0.5".parse().unwrap()),
        ("sneaky.test", "127.0.0.1".parse().unwrap()),
    ])))
}

fn client(allow: &[&str], options: EgressOptions) -> EgressClient {
    EgressClient::new(
        SsrfPolicy {
            allow_hosts: allow.iter().map(|s| s.to_string()).collect(),
        },
        dns(),
        options,
    )
}

fn allowed() -> EgressClient {
    client(&["a.test", "b.test"], EgressOptions::default())
}

fn get(url: impl Into<String>) -> RequestSpec {
    RequestSpec::new(Method::GET, url)
}

fn blocked(err: &EgressError) -> Option<&'static str> {
    match err {
        EgressError::Destination {
            source: DestinationError::Blocked(_, reason),
            ..
        } => Some(reason),
        _ => None,
    }
}

#[tokio::test]
async fn an_allowlisted_name_reaches_the_server_through_the_pinned_address() {
    let a = start().await;
    let res = allowed()
        .send(get(format!("http://a.test:{}/echo", a.port)))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "GET ");
    assert_eq!(a.count(), 1);
}

#[tokio::test]
async fn loopback_is_refused_without_the_allowlist_and_nothing_is_sent() {
    let a = start().await;
    for url in [
        format!("http://127.0.0.1:{}/echo", a.port),
        format!("http://sneaky.test:{}/echo", a.port),
        format!("http://[::1]:{}/echo", a.port),
    ] {
        let err = client(&[], EgressOptions::default())
            .send(get(&url))
            .await
            .unwrap_err();
        assert_eq!(blocked(&err), Some("loopback"), "{url}: {err:?}");
    }
    assert_eq!(
        a.count(),
        0,
        "a refused request must never leave the process"
    );
}

#[tokio::test]
async fn the_metadata_address_is_refused() {
    let err = client(&[], EgressOptions::default())
        .send(get("http://169.254.169.254/latest/meta-data/"))
        .await
        .unwrap_err();
    assert_eq!(blocked(&err), Some("link-local"), "{err:?}");
}

#[tokio::test]
async fn a_name_resolving_to_a_private_address_is_refused() {
    let err = client(&[], EgressOptions::default())
        .send(get("https://internal.test/api"))
        .await
        .unwrap_err();
    assert_eq!(blocked(&err), Some("private"), "{err:?}");
}

#[tokio::test]
async fn a_redirect_into_blocked_space_is_refused_before_it_is_dialled() {
    // Only the first hop's name is allowlisted. Server B is live on loopback:
    // the redirect names it by literal address, and it must see nothing.
    let a = start().await;
    let b = start().await;
    let only_a = client(&["a.test"], EgressOptions::default());
    for (to, reason) in [
        (format!("http://127.0.0.1:{}/echo", b.port), "loopback"),
        ("http://169.254.169.254/latest/".to_string(), "link-local"),
        ("http://internal.test/x".to_string(), "private"),
        (format!("http://b.test:{}/echo", b.port), "loopback"),
    ] {
        let err = only_a
            .send(get(format!(
                "http://a.test:{}/redirect?to={}",
                a.port,
                urlencode(&to)
            )))
            .await
            .unwrap_err();
        assert_eq!(blocked(&err), Some(reason), "{to}: {err:?}");
    }
    assert_eq!(a.count(), 4, "each first hop was sent");
    assert_eq!(b.count(), 0, "no redirect target was dialled");
}

#[tokio::test]
async fn a_redirect_to_a_non_http_scheme_is_refused() {
    let a = start().await;
    for to in ["ftp://a.test/x", "file:///etc/passwd", "gopher://a.test/"] {
        let err = allowed()
            .send(get(format!(
                "http://a.test:{}/redirect?to={}",
                a.port,
                urlencode(to)
            )))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EgressError::Destination {
                    source: DestinationError::BadScheme,
                    ..
                }
            ),
            "{to}: {err:?}"
        );
    }
}

#[tokio::test]
async fn a_non_http_url_is_refused() {
    for url in ["file:///etc/passwd", "ftp://a.test/", "javascript:alert(1)"] {
        let err = allowed().send(get(url)).await.unwrap_err();
        assert!(
            matches!(
                err,
                EgressError::Destination {
                    source: DestinationError::BadScheme,
                    ..
                }
            ),
            "{url}: {err:?}"
        );
    }
}

#[tokio::test]
async fn userinfo_cannot_borrow_an_allowlisted_name() {
    // `a.test` is allowlisted, but here it is the USERINFO; the host is the
    // metadata address, or loopback.
    let a = start().await;
    let only_a = client(&["a.test"], EgressOptions::default());
    for (url, reason) in [
        (
            "http://a.test@169.254.169.254/latest/".to_string(),
            "link-local",
        ),
        (
            format!("http://a.test:{0}@127.0.0.1:{0}/echo", a.port),
            "loopback",
        ),
    ] {
        let err = only_a.send(get(&url)).await.unwrap_err();
        assert_eq!(blocked(&err), Some(reason), "{url}: {err:?}");
        assert!(
            !err.to_string().contains("a.test"),
            "the error names the real host, without userinfo: {err}"
        );
    }
    assert_eq!(a.count(), 0);
}

#[tokio::test]
async fn a_cross_origin_redirect_drops_credentials_and_keeps_other_headers() {
    let a = start().await;
    let b = start().await;
    let mut spec = get(format!(
        "http://a.test:{}/redirect?to={}",
        a.port,
        urlencode(&format!("http://b.test:{}/echo", b.port))
    ));
    spec.headers
        .insert("authorization", "Bearer s3cret".parse().unwrap());
    spec.headers
        .insert("proxy-authorization", "Basic eDp5".parse().unwrap());
    spec.headers.insert("cookie", "sid=1".parse().unwrap());
    spec.headers.insert("x-keep", "yes".parse().unwrap());
    spec.headers.insert("x-api-key", "k3y".parse().unwrap());
    spec.headers.insert("X-Auth-Token", "t0k".parse().unwrap());
    spec.headers.insert("x-client-secret", "s".parse().unwrap());

    let res = allowed().send(spec).await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.url().host_str(), Some("b.test"));

    let (_, _, first, _) = a.last();
    assert_eq!(
        first["authorization"], "Bearer s3cret",
        "origin hop keeps it"
    );
    let (_, _, second, _) = b.last();
    for h in [
        "authorization",
        "proxy-authorization",
        "cookie",
        // Final review M7: credential-looking custom headers too.
        "x-api-key",
        "x-auth-token",
        "x-client-secret",
    ] {
        assert!(!second.contains_key(h), "{h} crossed origins");
    }
    assert_eq!(first["x-api-key"], "k3y", "origin hop keeps it");
    assert_eq!(second["x-keep"], "yes");
    assert_eq!(
        second["host"],
        format!("b.test:{}", b.port).as_str(),
        "the next origin's own Host"
    );
}

/// Final review I6: a caller-supplied `Host` would reach other virtual hosts
/// behind the vetted address, and `Content-Length`/`Transfer-Encoding` are
/// request-smuggling primitives. They never reach the wire; hyper writes
/// them from the URL and the body.
#[tokio::test]
async fn a_caller_cannot_set_the_host_or_the_framing() {
    let a = start().await;
    let mut spec = RequestSpec::new(Method::POST, format!("http://a.test:{}/echo", a.port));
    spec.body = Some("hi".into());
    for (name, value) in [
        ("host", "grafana.monitoring.svc"),
        ("content-length", "999"),
        ("transfer-encoding", "chunked"),
        ("connection", "close"),
        ("upgrade", "websocket"),
        ("te", "trailers"),
        ("proxy-x", "1"),
    ] {
        spec.headers.insert(name, value.parse().unwrap());
    }
    let res = allowed().send(spec).await.unwrap();
    assert_eq!(res.status(), 200);
    let (_, _, h, body) = a.last();
    assert_eq!(h["host"], format!("a.test:{}", a.port).as_str());
    assert_eq!(h["content-length"], "2");
    assert_eq!(body, "hi");
    for name in ["transfer-encoding", "upgrade", "te", "proxy-x"] {
        assert!(!h.contains_key(name), "{name} reached the wire");
    }
}

/// Final review I5/M3: an error quotes no query, fragment or userinfo, and
/// an unparseable URL is not quoted at all.
#[tokio::test]
async fn an_error_quotes_no_query_and_no_unparseable_url() {
    let a = start().await;
    let err = client(&[], EgressOptions::default())
        .send(get(format!(
            "http://u:pw1@a.test:{}/p?token=s3cret#frag-s3cret",
            a.port
        )))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains(&format!("http://a.test:{}/p", a.port)),
        "{text}"
    );
    assert!(!text.contains("s3cret") && !text.contains("pw1"), "{text}");
    assert_eq!(a.count(), 0);

    let err = allowed()
        .send(get("http://u:s3cret@[::1"))
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("s3cret"), "{err}");
}

#[tokio::test]
async fn a_port_change_is_an_origin_change() {
    // Same name, different port: still another origin.
    let a = start().await;
    let b = start().await;
    let mut spec = get(format!(
        "http://a.test:{}/redirect?to={}",
        a.port,
        urlencode(&format!("http://a.test:{}/echo", b.port))
    ));
    spec.headers
        .insert("authorization", "Bearer s3cret".parse().unwrap());
    allowed().send(spec).await.unwrap();
    assert!(!b.last().2.contains_key("authorization"));
}

#[tokio::test]
async fn a_same_origin_redirect_keeps_credentials() {
    let a = start().await;
    let mut spec = get(format!("http://a.test:{}/redirect?to=/echo", a.port));
    spec.headers
        .insert("authorization", "Bearer s3cret".parse().unwrap());
    let res = allowed().send(spec).await.unwrap();
    assert_eq!(res.status(), 200);
    let (path, _, h, _) = a.last();
    assert_eq!(path, "/echo", "relative Location resolved against the URL");
    assert_eq!(h["authorization"], "Bearer s3cret");
}

#[tokio::test]
async fn redirect_method_rules() {
    let a = start().await;
    // (status, method in, method out, body kept)
    for (status, m_in, m_out, keeps_body) in [
        (301, Method::POST, Method::GET, false),
        (302, Method::POST, Method::GET, false),
        (303, Method::POST, Method::GET, false),
        (303, Method::PUT, Method::GET, false),
        (302, Method::PUT, Method::PUT, true),
        (307, Method::POST, Method::POST, true),
        (308, Method::PUT, Method::PUT, true),
    ] {
        let mut spec = RequestSpec::new(
            m_in.clone(),
            format!("http://a.test:{}/redirect?status={status}&to=/echo", a.port),
        );
        spec.headers
            .insert("content-type", "text/plain".parse().unwrap());
        spec.body = Some("payload".into());
        let res = allowed().send(spec).await.unwrap();
        let expected_body = if keeps_body { "payload" } else { "" };
        assert_eq!(
            res.text().await.unwrap(),
            format!("{m_out} {expected_body}"),
            "{status} {m_in}"
        );
        let (_, _, h, _) = a.last();
        assert_eq!(
            h.contains_key("content-type"),
            keeps_body,
            "{status} {m_in}: Content-Type follows the body"
        );
    }
}

#[tokio::test]
async fn the_redirect_limit_stops_a_loop() {
    let a = start().await;
    let c = client(
        &["a.test"],
        EgressOptions {
            max_redirects: 3,
            ..EgressOptions::default()
        },
    );
    let err = c
        .send(get(format!("http://a.test:{}/loop", a.port)))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::TooManyRedirects(3)), "{err:?}");
    assert_eq!(a.count(), 4, "the first request plus three followed hops");
}

#[tokio::test]
async fn the_default_limit_is_ten() {
    let a = start().await;
    let err = allowed()
        .send(get(format!("http://a.test:{}/loop", a.port)))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::TooManyRedirects(10)), "{err:?}");
    assert_eq!(a.count(), 11);
}

#[tokio::test]
async fn follow_redirects_false_returns_the_redirect() {
    let a = start().await;
    let c = client(
        &["a.test"],
        EgressOptions {
            follow_redirects: false,
            ..EgressOptions::default()
        },
    );
    let res = c
        .send(get(format!(
            "http://a.test:{}/redirect?to={}",
            a.port,
            urlencode("http://169.254.169.254/")
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), 302);
    assert_eq!(a.count(), 1);
}

#[tokio::test]
async fn the_total_timeout_is_enforced() {
    let a = start().await;
    let c = client(
        &["a.test"],
        EgressOptions {
            timeout: Some(Duration::from_millis(200)),
            ..EgressOptions::default()
        },
    );
    let err = c
        .send(get(format!("http://a.test:{}/slow", a.port)))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::Timeout), "{err:?}");
}

#[tokio::test]
async fn the_read_timeout_is_enforced_without_a_total_timeout() {
    // Kestra's `readIdleTimeout`: no total cap, but a silent server is cut.
    let a = start().await;
    let c = client(
        &["a.test"],
        EgressOptions {
            timeout: None,
            read_timeout: Some(Duration::from_millis(200)),
            ..EgressOptions::default()
        },
    );
    let started = std::time::Instant::now();
    let err = c
        .send(get(format!("http://a.test:{}/slow", a.port)))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::Timeout), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "cut by the read timeout"
    );
}

#[tokio::test]
async fn the_environment_proxy_is_ignored() {
    // With a proxy honoured, the request would go to the (dead) proxy and
    // fail; `.no_proxy()` means it goes to the pinned address. Set for this
    // process only; no other test in this binary reads it.
    std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    std::env::set_var("http_proxy", "http://127.0.0.1:9");
    let a = start().await;
    let res = allowed()
        .send(get(format!("http://a.test:{}/echo", a.port)))
        .await;
    std::env::remove_var("HTTP_PROXY");
    std::env::remove_var("http_proxy");
    assert_eq!(res.unwrap().status(), 200);
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
