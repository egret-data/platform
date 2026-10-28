//! The http client core against a real local server on 127.0.0.1.
//!
//! Loopback is refused by the guard, so positive tests reach the server
//! through invented names (`a.test`, `b.test`) that only the injected resolver
//! knows and that the allowlist names. Refusals run with the allowlist unset.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use bytes::Bytes;
use egret_egress::Resolver;
use serde_json::{json, Value};

use super::*;
use crate::logs::{LogLevel, MASK};

/// A secret the `/leak` route puts in its body, straddling the 4 KiB mark.
const SECRET: &str = "s3cr3t-T0KEN-value-0123456789";

/// What the server saw: path and query, method, headers, body.
type Seen = Arc<Mutex<Vec<(String, Method, HeaderMap, Bytes)>>>;
/// Multipart parts: (name, file name, content type, content).
type Parts = Arc<Mutex<Vec<(String, Option<String>, Option<String>, String)>>>;

struct Server {
    port: u16,
    seen: Seen,
    parts: Parts,
}

impl Server {
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    fn last(&self) -> (String, Method, HeaderMap, Bytes) {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request")
    }
    fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

#[derive(Clone)]
struct St {
    seen: Seen,
    parts: Parts,
}

fn record(st: &St, uri: &axum::http::Uri, m: &Method, h: &HeaderMap, b: Bytes) {
    let pq = uri.path_and_query().map_or("", |p| p.as_str()).to_string();
    st.seen.lock().unwrap().push((pq, m.clone(), h.clone(), b));
}

async fn start() -> Server {
    let st = St {
        seen: Arc::default(),
        parts: Arc::default(),
    };
    let app = Router::new()
        .route(
            "/echo",
            any(
                |State(st): State<St>, uri: axum::http::Uri, m: Method, h: HeaderMap, b: Bytes| async move {
                    record(&st, &uri, &m, &h, b.clone());
                    (
                        [("set-cookie", "sid=abc"), ("x-one", "1")],
                        b,
                    )
                },
            ),
        )
        .route(
            "/status/{code}",
            any(
                |State(st): State<St>,
                 Path(code): Path<u16>,
                 uri: axum::http::Uri,
                 m: Method,
                 h: HeaderMap,
                 b: Bytes| async move {
                    record(&st, &uri, &m, &h, b);
                    (StatusCode::from_u16(code).unwrap(), format!("status {code}"))
                },
            ),
        )
        // 500 with a 10 KiB body whose SECRET straddles byte 4096.
        .route(
            "/leak",
            any(|State(st): State<St>, uri: axum::http::Uri, m: Method, h: HeaderMap| async move {
                record(&st, &uri, &m, &h, Bytes::new());
                let mut body = "x".repeat(4090);
                body.push_str(SECRET);
                body.push_str(&"y".repeat(6000));
                (StatusCode::INTERNAL_SERVER_ERROR, body)
            }),
        )
        .route(
            "/redirect",
            any(
                |State(st): State<St>,
                 uri: axum::http::Uri,
                 m: Method,
                 h: HeaderMap,
                 Query(q): Query<HashMap<String, String>>| async move {
                    record(&st, &uri, &m, &h, Bytes::new());
                    Response::builder()
                        .status(302)
                        .header("location", q.get("to").cloned().unwrap_or_default())
                        .body(axum::body::Body::empty())
                        .unwrap()
                },
            ),
        )
        .route(
            "/multipart",
            any(
                |State(st): State<St>, uri: axum::http::Uri, m: Method, h: HeaderMap, mut mp: Multipart| async move {
                    record(&st, &uri, &m, &h, Bytes::new());
                    while let Some(f) = mp.next_field().await.unwrap() {
                        let name = f.name().unwrap_or_default().to_string();
                        let file = f.file_name().map(str::to_string);
                        let ct = f.content_type().map(str::to_string);
                        let text = f.text().await.unwrap();
                        st.parts.lock().unwrap().push((name, file, ct, text));
                    }
                    "ok".into_response()
                },
            ),
        )
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        port,
        seen: st.seen,
        parts: st.parts,
    }
}

struct FakeDns;

#[async_trait::async_trait]
impl Resolver for FakeDns {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, std::io::Error> {
        match host {
            "a.test" | "b.test" => Ok(vec![SocketAddr::new(
                "127.0.0.1".parse::<IpAddr>().unwrap(),
                port,
            )]),
            "private.test" => Ok(vec![SocketAddr::new("10.0.0.5".parse().unwrap(), port)]),
            _ => Err(std::io::Error::other("NXDOMAIN")),
        }
    }
}

fn egress(allow: &[&str]) -> HttpEgress {
    let cfg = egret_config::WorkflowHttpConfig {
        allow_hosts: allow.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    HttpEgress::from_config(&cfg, Arc::new(FakeDns))
}

fn spec(v: Value) -> HttpSpec {
    HttpSpec::from_value(&v).unwrap()
}

/// Build, send, and check the status: what a handler does, minus outputs.
async fn run(
    eg: &HttpEgress,
    s: &HttpSpec,
    files: Option<&dyn FormFiles>,
    secrets: &[&str],
) -> Result<(reqwest::Response, BuiltRequest), String> {
    let opts = s.options()?;
    let built = build_request(s, files).await?;
    let mut masks = MaskSet::new(secrets.iter().copied());
    masks.extend(built.credentials.clone());
    let res = eg
        .client(&opts, None)
        .send(built.request.clone())
        .await
        .map_err(|e| transport_message(&e, &masks))?;
    let res = response::ensure_allowed(res, &opts, &masks).await?;
    Ok((res, built))
}

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).map(|v| v.to_str().unwrap().to_string())
}

// ---- auth -----------------------------------------------------------------

#[tokio::test]
async fn http_auth_basic_sets_a_preemptive_header_over_a_written_one() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"),
        "headers": {"Authorization": "Bearer written"},
        "options": {"auth": {"type": "BASIC", "username": "ann", "password": "pa:ss"}}
    }));
    let (res, built) = run(&egress(&["a.test"]), &s, None, &[]).await.unwrap();
    assert_eq!(res.status(), 200);
    let (_, _, h, _) = a.last();
    // base64("ann:pa:ss")
    assert_eq!(header(&h, "authorization").unwrap(), "Basic YW5uOnBhOnNz");
    for c in ["pa:ss", "YW5uOnBhOnNz", "Bearer written"] {
        assert!(built.credentials.iter().any(|v| v == c), "{c} is masked");
    }
}

#[tokio::test]
async fn http_auth_bearer_sets_the_token() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"),
        "options": {"auth": {"type": "BEARER", "token": "tok-1"}}
    }));
    run(&egress(&["a.test"]), &s, None, &[]).await.unwrap();
    assert_eq!(
        header(&a.last().2, "authorization").unwrap(),
        "Bearer tok-1"
    );
}

#[tokio::test]
async fn http_auth_without_a_type_or_with_an_unknown_one_fails() {
    for (auth, want) in [
        (json!({"token": "t"}), "options.auth.type: must not be null"),
        (
            json!({"type": "NTLM"}),
            "\"NTLM\" is not one of BASIC, BEARER, DIGEST",
        ),
    ] {
        let s = spec(json!({"uri": "http://a.test/", "options": {"auth": auth}}));
        let err = build_request(&s, None).await.unwrap_err();
        assert!(err.contains(want), "{err}");
    }
}

// ---- body, contentType, headers -------------------------------------------

#[tokio::test]
async fn http_body_is_sent_with_its_content_type_and_utf8() {
    let a = start().await;
    let eg = egress(&["a.test"]);
    // Default contentType.
    let s = spec(json!({"uri": a.url("a.test", "/echo"), "method": "POST", "body": "{\"é\":1}"}));
    let (res, _) = run(&eg, &s, None, &[]).await.unwrap();
    assert_eq!(res.bytes().await.unwrap(), "{\"é\":1}".as_bytes());
    let (_, m, h, b) = a.last();
    assert_eq!(m, Method::POST);
    assert_eq!(
        header(&h, "content-type").unwrap(),
        "application/json; charset=UTF-8"
    );
    assert_eq!(&b[..], "{\"é\":1}".as_bytes());
    assert_eq!(header(&h, "user-agent").unwrap(), "egret");

    // Explicit contentType; a non-string body is sent as its JSON.
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"), "method": "PUT",
        "contentType": "text/plain", "body": {"k": [1, 2]}
    }));
    run(&eg, &s, None, &[]).await.unwrap();
    let (_, _, h, b) = a.last();
    assert_eq!(
        header(&h, "content-type").unwrap(),
        "text/plain; charset=UTF-8"
    );
    assert_eq!(&b[..], br#"{"k":[1,2]}"#);

    // A Content-Type in headers wins over the body's.
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"), "method": "POST", "body": "x",
        "headers": {"Content-Type": "application/xml", "X-Num": 5}
    }));
    run(&eg, &s, None, &[]).await.unwrap();
    let (_, _, h, _) = a.last();
    assert_eq!(header(&h, "content-type").unwrap(), "application/xml");
    assert_eq!(header(&h, "x-num").unwrap(), "5");
}

#[tokio::test]
async fn http_without_a_body_content_type_is_a_header_unless_headers_replace_them() {
    let a = start().await;
    let eg = egress(&["a.test"]);
    run(
        &eg,
        &spec(json!({"uri": a.url("a.test", "/echo")})),
        None,
        &[],
    )
    .await
    .unwrap();
    let (_, m, h, _) = a.last();
    assert_eq!(m, Method::GET, "method defaults to GET");
    assert_eq!(header(&h, "content-type").unwrap(), "application/json");

    // Kestra's `request.headers(...)` replaces the map, dropping it.
    let s = spec(json!({"uri": a.url("a.test", "/echo"), "headers": {"X-A": "1"}}));
    run(&eg, &s, None, &[]).await.unwrap();
    let (_, _, h, _) = a.last();
    assert_eq!(header(&h, "content-type"), None);
    assert_eq!(header(&h, "x-a").unwrap(), "1");
}

#[tokio::test]
async fn http_params_become_query_parameters_and_spaces_in_the_uri_are_encoded() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/echo?x=1 2"),
        "params": {"k": "v w", "r": ["1", 2]}
    }));
    let (_, built) = run(&egress(&["a.test"]), &s, None, &[]).await.unwrap();
    let (pq, ..) = a.last();
    assert_eq!(pq, "/echo?x=1%202&k=v+w&r=1&r=2");
    assert_eq!(built.uri, a.url("a.test", "/echo?x=1%202&k=v+w&r=1&r=2"));

    let s = spec(json!({"uri": "http://a.test/", "params": {"k": 1}}));
    let err = build_request(&s, None).await.unwrap_err();
    assert_eq!(err, "Unsupported param type: a number");
}

#[tokio::test]
async fn http_uri_and_method_errors_name_the_property() {
    let err = build_request(&spec(json!({})), None).await.unwrap_err();
    assert_eq!(err, "uri: must not be null");
    let err = build_request(&spec(json!({"uri": "not a url"})), None)
        .await
        .unwrap_err();
    assert!(err.starts_with("uri: "), "{err}");
    let err = build_request(
        &spec(json!({"uri": "http://a.test/", "method": "GE T"})),
        None,
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("method: "), "{err}");
    let err = build_request(
        &spec(json!({"uri": "http://a.test/", "headers": {"X": "a\nb"}})),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.starts_with("headers.X: ") && !err.contains("a\nb"),
        "{err}"
    );
}

// ---- formData -------------------------------------------------------------

#[tokio::test]
async fn http_form_data_is_url_encoded_by_default_and_wins_over_body() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"), "method": "POST", "body": "ignored",
        "formData": {"a": "1 2", "b": "x&y", "n": 3}
    }));
    run(&egress(&["a.test"]), &s, None, &[]).await.unwrap();
    let (_, _, h, b) = a.last();
    assert_eq!(
        header(&h, "content-type").unwrap(),
        "application/x-www-form-urlencoded; charset=UTF-8"
    );
    assert_eq!(&b[..], b"a=1+2&b=x%26y&n=3");
}

struct Files;

#[async_trait::async_trait]
impl FormFiles for Files {
    async fn read(&self, uri: &str) -> Result<Bytes, String> {
        match uri {
            "kestra:///ns/f/executions/e/tasks/t/r/data.csv" => {
                Ok(Bytes::from_static(b"a,b\n1,2\n"))
            }
            _ => Err(format!("{uri} is not readable from this execution")),
        }
    }
}

#[tokio::test]
async fn http_form_data_multipart_has_text_json_and_file_parts() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/multipart"), "method": "POST",
        "contentType": "multipart/form-data",
        "formData": {
            "text": "hello",
            "json": {"k": 1},
            "file": "kestra:///ns/f/executions/e/tasks/t/r/data.csv",
            "named": {"name": "report.csv", "content": "kestra:///ns/f/executions/e/tasks/t/r/data.csv"}
        }
    }));
    run(&egress(&["a.test"]), &s, Some(&Files), &[])
        .await
        .unwrap();
    let (_, _, h, _) = a.last();
    assert!(header(&h, "content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
    let mut parts = a.parts.lock().unwrap().clone();
    parts.sort();
    let csv = "a,b\n1,2\n".to_string();
    let octet = Some("application/octet-stream".to_string());
    // multer lower-cases the charset it reports; the bytes sent say `UTF-8`.
    let text = Some("text/plain; charset=utf-8".to_string());
    assert_eq!(
        parts,
        vec![
            (
                "file".into(),
                Some("data.csv".into()),
                octet.clone(),
                csv.clone()
            ),
            ("json".into(), None, text.clone(), r#"{"k":1}"#.into()),
            ("named".into(), Some("report.csv".into()), octet, csv),
            ("text".into(), None, text, "hello".into()),
        ]
    );
}

#[tokio::test]
async fn http_form_data_file_parts_need_storage_and_a_storage_uri() {
    let form = |v: Value| {
        spec(json!({
            "uri": "http://a.test/", "contentType": "multipart/form-data", "formData": {"f": v}
        }))
    };
    let err = build_request(&form(json!("kestra:///x/y.csv")), None)
        .await
        .unwrap_err();
    assert!(
        err.contains("formData.f") && err.contains("[storage]"),
        "{err}"
    );
    let err = build_request(
        &form(json!({"name": "a", "content": "/etc/passwd"})),
        Some(&Files),
    )
    .await
    .unwrap_err();
    assert!(err.contains("must be a kestra:// URI"), "{err}");
    let err = build_request(&form(json!("kestra:///other")), Some(&Files))
        .await
        .unwrap_err();
    assert!(
        err.starts_with("formData.f: ") && err.contains("not readable"),
        "{err}"
    );
}

// ---- status rules ---------------------------------------------------------

#[tokio::test]
async fn http_status_rules_follow_kestra() {
    let a = start().await;
    let eg = egress(&["a.test"]);
    let go = |path: &str, options: Value| {
        spec(json!({"uri": a.url("a.test", path), "options": options}))
    };

    // Default: below 400 passes.
    assert!(run(&eg, &go("/status/204", json!({})), None, &[])
        .await
        .is_ok());
    let err = run(&eg, &go("/status/404", json!({})), None, &[])
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Failed http request with response code '404' and body:\nstatus 404"
    );
    // A 3xx not followed passes, as in Kestra (`code < 400`).
    let r = go(
        &format!("/redirect?to={}", "http://a.test/echo"),
        json!({"followRedirects": false}),
    );
    assert_eq!(run(&eg, &r, None, &[]).await.unwrap().0.status(), 302);

    // allowFailed passes anything.
    let (res, _) = run(
        &eg,
        &go("/status/503", json!({"allowFailed": true})),
        None,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(res.status(), 503);

    // allowedResponseCodes REPLACES the rule: 2xx only if listed, and it wins
    // over allowFailed.
    let only_404 = json!({"allowedResponseCodes": [404], "allowFailed": true});
    assert!(run(&eg, &go("/status/404", only_404.clone()), None, &[])
        .await
        .is_ok());
    let err = run(&eg, &go("/status/200", only_404), None, &[])
        .await
        .unwrap_err();
    assert!(
        err.starts_with("Failed http request with response code '200'"),
        "{err}"
    );
    // A rendered template: the list as text.
    let as_text = json!({"allowedResponseCodes": "[201, \"202\"]"});
    assert!(run(&eg, &go("/status/202", as_text), None, &[])
        .await
        .is_ok());
    // An empty list is no rule (Kestra: `!isEmpty()`).
    assert!(run(
        &eg,
        &go("/status/200", json!({"allowedResponseCodes": []})),
        None,
        &[]
    )
    .await
    .is_ok());
}

#[test]
fn http_options_convert_with_errors_naming_the_property() {
    let o = |v: Value| spec(json!({"options": v})).options();
    let d = o(json!({})).unwrap();
    assert_eq!(
        d,
        ResolvedOptions {
            follow_redirects: true,
            allow_failed: false,
            allowed_response_codes: None,
            connect_timeout: None,
            read_idle_timeout: std::time::Duration::from_secs(300),
            logs: vec![],
        }
    );
    let r = o(json!({
        "followRedirects": "false", "allowFailed": true,
        "timeout": {"connectTimeout": "PT2S", "readIdleTimeout": "PT0.5S"},
        "logs": ["REQUEST_HEADERS", "RESPONSE_BODY"], "defaultCharset": "utf-8"
    }))
    .unwrap();
    assert!(!r.follow_redirects && r.allow_failed);
    assert_eq!(r.connect_timeout, Some(std::time::Duration::from_secs(2)));
    assert_eq!(r.read_idle_timeout, std::time::Duration::from_millis(500));
    assert_eq!(r.logs, vec![LogKind::RequestHeaders, LogKind::ResponseBody]);
    for (v, want) in [
        (
            json!({"allowFailed": "yes"}),
            "options.allowFailed: \"yes\" is not a boolean",
        ),
        (
            json!({"allowedResponseCodes": [70000]}),
            "options.allowedResponseCodes: 70000",
        ),
        (
            json!({"timeout": {"connectTimeout": "soon"}}),
            "options.timeout.connectTimeout",
        ),
        (json!({"logs": ["ALL"]}), "options.logs: \"ALL\""),
        (
            json!({"defaultCharset": "ISO-8859-1"}),
            "options.defaultCharset",
        ),
    ] {
        let err = o(v.clone()).unwrap_err();
        assert!(err.contains(want), "{v}: {err}");
    }
}

// ---- refusals (D4) --------------------------------------------------------

#[tokio::test]
async fn http_d4_refusals_name_the_property_at_publish_and_at_run() {
    let cases = [
        (
            json!({"options": {"proxy": {"address": "proxy.corp", "port": 3128}}}),
            "options.proxy",
        ),
        (
            json!({"options": {"proxy": {"address": "{{ inputs.p }}"}}}),
            "options.proxy",
        ),
        (
            json!({"options": {"ssl": {"insecureTrustAllCertificates": true}}}),
            "options.ssl.insecureTrustAllCertificates",
        ),
        // Unrendered: what it renders to is unknown, so it is refused.
        (
            json!({"options": {"ssl": {"insecureTrustAllCertificates": "{{ inputs.x }}"}}}),
            "options.ssl.insecureTrustAllCertificates",
        ),
        (
            json!({"options": {"auth": {"type": "DIGEST", "username": "u", "password": "p"}}}),
            "options.auth.type: DIGEST",
        ),
        (json!({"encryptBody": true}), "encryptBody"),
        (json!({"encryptBody": "true"}), "encryptBody"),
        // Controller decision (C3): userinfo would reach the `uri` output.
        (json!({"uri": "http://u:p@a.test/"}), "uri: credentials"),
        (json!({"uri": "http://u@a.test/"}), "uri: credentials"),
        (json!({"uri": "http://:p@a.test/"}), "uri: credentials"),
        (
            json!({"uri": "http://{{ inputs.u }}@a.test/"}),
            "uri: credentials",
        ),
    ];
    for (props, key) in cases {
        let mut v = props.clone();
        if v.get("uri").is_none() {
            v["uri"] = json!("http://a.test/");
        }
        let s = spec(v);
        let r = refusals(&s);
        assert_eq!(r.len(), 1, "{props}: {r:?}");
        assert!(r[0].starts_with(key), "{props}: {}", r[0]);
        // The same refusal fails the run, before anything is sent.
        let err = build_request(&s, None).await.unwrap_err();
        assert!(err.starts_with(key), "{props}: {err}");
    }

    // What stays allowed: absent, false, "false", and a proxy with no address.
    let ok = spec(json!({
        "uri": "http://a.test/", "encryptBody": "false",
        "options": {
            "ssl": {"insecureTrustAllCertificates": false},
            "proxy": {"type": "DIRECT"},
            "auth": {"type": "BASIC", "username": "u", "password": "p"}
        }
    }));
    assert!(refusals(&ok).is_empty());
    assert!(refusals(&spec(json!({"uri": "http://a.test/"}))).is_empty());
    // A templated host cannot be judged unrendered; the run judges it.
    assert!(refusals(&spec(json!({"uri": "{{ inputs.base }}/x"}))).is_empty());
    assert!(refusals(&spec(json!({"uri": "http://{{ inputs.h }}/x"}))).is_empty());

    // All four at once: each named.
    let all = spec(json!({
        "encryptBody": true,
        "options": {
            "proxy": {"address": "p"}, "ssl": {"insecureTrustAllCertificates": "true"},
            "auth": {"type": "DIGEST"}
        }
    }));
    assert_eq!(refusals(&all).len(), 4);
}

// ---- masking and the failure body -----------------------------------------

#[tokio::test]
async fn http_failure_body_is_cut_to_4_kib_and_masked() {
    let a = start().await;
    let s = spec(json!({"uri": a.url("a.test", "/leak")}));
    let err = run(&egress(&["a.test"]), &s, None, &[SECRET])
        .await
        .unwrap_err();
    assert!(err.starts_with("Failed http request with response code '500' and body:\n"));
    assert!(!err.contains(SECRET), "the secret is masked");
    // No head of the secret is left readable at the cut either.
    assert!(!err.contains(&SECRET[..8]), "{}", &err[err.len() - 120..]);
    assert!(
        err.contains(&format!("{}{MASK}", "x".repeat(10))),
        "masked where it was"
    );
    assert!(
        err.ends_with("[body truncated to its first 4096 bytes]"),
        "{}",
        &err[err.len() - 80..]
    );
    assert!(err.len() < 4096 + 200, "{}", err.len());
    let body = err.split_once(" and body:\n").unwrap().1;
    let body = body.split_once("\n[body truncated").unwrap().0;
    assert!(!body.contains('y'), "nothing past the cut");
    assert_eq!(body, format!("{}{MASK}", "x".repeat(4090)));
}

#[test]
fn http_truncate_masked_moves_the_cut_past_a_straddling_value() {
    let m = MaskSet::new(["abcdef"]);
    let (t, cut) = m.truncate_masked(b"0123abcdef789", 6);
    assert_eq!((t.as_str(), cut), ("0123******", true));
    let (t, cut) = m.truncate_masked(b"0123", 6);
    assert_eq!((t.as_str(), cut), ("0123", false));
    // A cut inside a multi-byte character moves back to its start.
    let (t, _) = MaskSet::default().truncate_masked("aé".as_bytes(), 2);
    assert_eq!(t, "a");
    // Percent-encoded forms are masked too.
    let m = MaskSet::new(["a b&c"]);
    assert_eq!(
        m.mask("q=a+b%26c&r=a%20b%26c"),
        format!("q={MASK}&r={MASK}")
    );
}

#[tokio::test]
async fn http_log_lines_never_print_credentials() {
    let a = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", "/echo"), "method": "POST", "body": "token=tok-1",
        "headers": {"Cookie": "sid=zzz", "X-Ok": "visible"},
        "options": {
            "auth": {"type": "BEARER", "token": "tok-1"},
            "logs": ["REQUEST_HEADERS", "REQUEST_BODY", "RESPONSE_HEADERS", "RESPONSE_BODY"]
        }
    }));
    let opts = s.options().unwrap();
    let built = build_request(&s, None).await.unwrap();
    let masks = MaskSet::new(built.credentials.clone());
    let mut lines = log_request_lines(
        &built.request.method,
        &built.uri,
        &built.request.headers,
        built.request.body.as_deref(),
        &opts.logs,
        &masks,
    );
    let res = egress(&["a.test"])
        .client(&opts, None)
        .send(built.request.clone())
        .await
        .unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let body = res.bytes().await.unwrap();
    lines.extend(log_response_lines(
        status,
        &headers,
        Some(&body),
        &opts.logs,
        &masks,
    ));

    let levels: Vec<LogLevel> = lines.iter().map(|(l, _)| *l).collect();
    assert_eq!(
        levels,
        [
            LogLevel::Debug,
            LogLevel::Debug,
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Debug,
            LogLevel::Trace
        ]
    );
    let all: String = lines
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for secret in ["tok-1", "sid=zzz", "sid=abc"] {
        assert!(!all.contains(secret), "{secret} printed:\n{all}");
    }
    assert!(all.contains(&format!("authorization: {MASK}")), "{all}");
    assert!(all.contains(&format!("cookie: {MASK}")), "{all}");
    assert!(all.contains(&format!("set-cookie: {MASK}")), "{all}");
    assert!(all.contains("x-ok: visible"), "{all}");
    assert!(
        all.contains(&format!("request payload:\n    token={MASK}")),
        "{all}"
    );
}

// ---- egress through the builder -------------------------------------------

#[tokio::test]
async fn http_a_cross_origin_redirect_drops_the_auth_header_the_builder_set() {
    let a = start().await;
    let b = start().await;
    let s = spec(json!({
        "uri": a.url("a.test", &format!("/redirect?to=http://b.test:{}/echo", b.port)),
        "options": {"auth": {"type": "BASIC", "username": "u", "password": "p"}}
    }));
    let (res, built) = run(&egress(&["a.test", "b.test"]), &s, None, &[])
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        header(&a.last().2, "authorization").is_some(),
        "the origin hop has it"
    );
    assert_eq!(
        header(&b.last().2, "authorization"),
        None,
        "it did not cross"
    );
    assert!(
        built.uri.starts_with(&a.url("a.test", "/redirect")),
        "uri is the request URI"
    );
}

#[tokio::test]
async fn http_private_destinations_are_refused_with_the_allowlist_unset() {
    let a = start().await;
    let eg = egress(&[]);
    for (uri, reached) in [
        (format!("http://127.0.0.1:{}/echo", a.port), "loopback"),
        (a.url("a.test", "/echo"), "a name resolving to loopback"),
        (
            "http://private.test/".to_string(),
            "a name resolving to 10/8",
        ),
        (
            "http://169.254.169.254/latest/meta-data".to_string(),
            "metadata",
        ),
    ] {
        let err = run(&eg, &spec(json!({"uri": uri})), None, &[])
            .await
            .unwrap_err();
        assert!(
            err.contains("workflow.http.allow_hosts"),
            "{reached}: {err}"
        );
    }
    let err = run(&eg, &spec(json!({"uri": "file:///etc/passwd"})), None, &[])
        .await
        .unwrap_err();
    assert!(err.contains("http or https"), "{err}");
    assert_eq!(a.count(), 0, "nothing reached the server");

    // A redirect to a private address, from an allowed host.
    let r = spec(json!({
        "uri": a.url("a.test", &format!("/redirect?to=http://127.0.0.1:{}/echo", a.port))
    }));
    let err = run(&egress(&["a.test"]), &r, None, &[]).await.unwrap_err();
    assert!(err.contains("loopback"), "{err}");
    assert_eq!(a.count(), 1, "only the redirecting hop");

    // An allowlisted IP literal is reached.
    let ok = spec(json!({"uri": format!("http://127.0.0.1:{}/echo", a.port)}));
    assert!(run(&egress(&["127.0.0.1"]), &ok, None, &[]).await.is_ok());
}

#[tokio::test]
async fn http_transport_errors_mask_secrets_in_the_url() {
    let eg = egress(&[]);
    let s = spec(json!({"uri": format!("http://nowhere.test/?key={SECRET}")}));
    let err = run(&eg, &s, None, &[SECRET]).await.unwrap_err();
    assert!(!err.contains(SECRET) && err.contains(MASK), "{err}");
}

// ---- reading the body -----------------------------------------------------

#[tokio::test]
async fn http_read_capped_stops_at_the_cap_pointing_at_download() {
    let a = start().await;
    let eg = egress(&["a.test"]);
    let s = spec(json!({"uri": a.url("a.test", "/echo"), "method": "POST", "body": "0123456789"}));
    let (res, _) = run(&eg, &s, None, &[]).await.unwrap();
    let err = read_capped(res, 4).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Response body is too large to store in task outputs (10 bytes > max 4 bytes). Use \
         io.kestra.plugin.core.http.Download to fetch large payloads as files instead."
    );
    let (res, _) = run(&eg, &s, None, &[]).await.unwrap();
    assert_eq!(&read_capped(res, 10).await.unwrap()[..], b"0123456789");
}

#[tokio::test]
async fn http_headers_output_is_a_map_of_lists() {
    let a = start().await;
    let (res, _) = run(
        &egress(&["a.test"]),
        &spec(json!({"uri": a.url("a.test", "/echo")})),
        None,
        &[],
    )
    .await
    .unwrap();
    let h = headers_output(res.headers());
    assert_eq!(h["x-one"], json!(["1"]));
    assert_eq!(h["set-cookie"], json!(["sid=abc"]));
}

#[test]
fn http_egress_takes_the_deployment_config() {
    let cfg = egret_config::WorkflowHttpConfig {
        allow_hosts: vec!["erp.internal".into()],
        max_redirects: 3,
        connect_timeout_secs: 7,
        ..Default::default()
    };
    let eg = HttpEgress::from_config(&cfg, Arc::new(FakeDns));
    assert_eq!(eg.policy.allow_hosts, vec!["erp.internal".to_string()]);
    let opts = spec(json!({})).options().unwrap();
    let c = eg.client(&opts, None);
    assert_eq!(c.options().max_redirects, 3);
    assert_eq!(
        c.options().connect_timeout,
        std::time::Duration::from_secs(7)
    );
    assert_eq!(
        c.options().read_timeout,
        Some(std::time::Duration::from_secs(300))
    );
    assert_eq!(c.options().timeout, None);
    let fast = spec(json!({"options": {"timeout": {"connectTimeout": "PT1S"}}}))
        .options()
        .unwrap();
    assert_eq!(
        eg.client(&fast, None).options().connect_timeout,
        std::time::Duration::from_secs(1)
    );
}

/// Final review I2: a huge, negative or NaN number of seconds is refused
/// with a message, never a panic (`Duration::from_secs_f64` panics above
/// `u64::MAX` seconds, and a panicking handler is re-claimed for ever).
#[test]
fn http_timeouts_out_of_range_are_refused_not_panicked() {
    let o = |v: Value| spec(json!({"options": v})).options();
    for (v, want) in [
        (json!({"timeout": {"connectTimeout": 1e20}}), "too large"),
        (json!({"timeout": {"readIdleTimeout": 1e20}}), "too large"),
        (json!({"timeout": {"connectTimeout": 86401}}), "too large"),
        (json!({"timeout": {"connectTimeout": "P2D"}}), "too large"),
        (
            json!({"timeout": {"connectTimeout": -1}}),
            "is not a duration",
        ),
    ] {
        let err = o(v.clone()).unwrap_err();
        assert!(err.contains(want), "{v}: {err}");
    }
    let ok = o(json!({"timeout": {"connectTimeout": 86400}})).unwrap();
    assert_eq!(ok.connect_timeout, Some(spec::MAX_TIMEOUT));
    assert!(spec::seconds(f64::NAN, "t")
        .unwrap_err()
        .contains("not a duration"));
    assert!(spec::seconds(f64::INFINITY, "t").is_err());
    assert!(spec::seconds(1e20, "t").unwrap_err().contains("too large"));
}

/// Final review I4: a payload log line decodes only the head it can print
/// (64 KiB plus the longest masked value), never the whole body; and a
/// secret straddling the 64 KiB mark is still masked whole.
#[test]
fn http_payload_log_decodes_only_its_head() {
    let masks = MaskSet::new([SECRET]);
    let body = vec![0xFFu8; 4 << 20];
    let (head, cut) = mask::log_head(&body, &masks);
    assert!(cut);
    assert_eq!(head.len(), mask::LOG_BODY_BYTES + SECRET.len());
    let (head, cut) = mask::log_head(b"short", &masks);
    assert_eq!((head, cut), (&b"short"[..], false));

    let mut straddling = vec![b'a'; mask::LOG_BODY_BYTES - 5];
    straddling.extend_from_slice(SECRET.as_bytes());
    straddling.extend(vec![b'b'; 1 << 20]);
    let lines = log_request_lines(
        &reqwest::Method::POST,
        "http://a.test/",
        &HeaderMap::new(),
        Some(&straddling),
        &[LogKind::RequestBody],
        &masks,
    );
    let line = &lines[0].1;
    assert!(!line.contains(&SECRET[..5]), "no head of the secret");
    assert!(
        line.contains(MASK) && line.ends_with("[truncated]"),
        "{}",
        &line[line.len() - 40..]
    );
}
