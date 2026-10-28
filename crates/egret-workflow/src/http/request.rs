//! The request builder: Kestra's `AbstractHttp.request` (`uri`, `params`,
//! `method`, `formData` / `body` / `contentType`, `headers`) and the auth
//! interceptors (`BasicAuthConfiguration`, `BearerAuthConfiguration`), plus
//! [`HttpEgress`], the deployment's egress policy from `workflow.http`.
//!
//! Kestra's ordering rules are kept, each measured from its source:
//!
//! - `formData`, when non-empty, wins over `body` (`AbstractHttp.java:104`).
//! - A body's `Content-Type` is `contentType` (default `application/json`)
//!   with `; charset=UTF-8` (`StringRequestBody`); a `Content-Type` in
//!   `headers` wins over it (Apache's `RequestContent` adds the entity's only
//!   when the request has none).
//! - With neither body nor `formData`, `contentType` is still sent as a header
//!   (`:157-158`) -- **unless `headers` is set**, which replaces the whole
//!   header map in Kestra (`request.headers(...)`, `:161-180`), dropping it.
//! - The auth header is set by an interceptor that runs first and uses
//!   `setHeader`, so it replaces an `Authorization` written in `headers`.
//!   A missing username, password or token is sent as `null`, as Java's string
//!   concatenation does.
//!
//! Divergences: the `User-Agent` default is `egret` (Kestra: `Kestra`); a
//! multipart file part from internal storage is named after the URI's last
//! segment (Kestra: a temp-file name); `Accept-Encoding` is not forced (the
//! response is not compressed, so nothing needs decoding).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use egret_egress::{EgressClient, EgressOptions, RequestSpec, Resolver, SsrfPolicy};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, PROXY_AUTHORIZATION,
    USER_AGENT,
};
use reqwest::Method;
use serde_json::{Map, Value};
use url::Url;

use super::spec::{kind, refusals, text, text_or_json, HttpSpec, ResolvedOptions};

/// `contentType`'s default (`AbstractHttp.java:57-58`).
pub const DEFAULT_CONTENT_TYPE: &str = "application/json";

/// What a `formData` file part is read from: internal storage. The handler
/// implements it over its run's readable files; `None` when the deployment
/// has no `[storage]`.
#[async_trait::async_trait]
pub trait FormFiles: Send + Sync {
    /// The whole file at `uri` (`kestra://...`). The error is a message for
    /// the task run.
    async fn read(&self, uri: &str) -> Result<Bytes, String>;
}

/// A built request and what the handler needs beside it.
#[derive(Debug, Clone)]
pub struct BuiltRequest {
    pub request: RequestSpec,
    /// The request URI (`params` included), Kestra's `uri` output, which is
    /// the URI asked for, not the one a redirect ended on.
    pub uri: String,
    /// Credential values written in the task (`options.auth`, credential
    /// headers, a URL password), to be masked with the resolved secrets
    /// whether or not they came from `secret()`.
    pub credentials: Vec<String>,
}

/// The deployment's egress policy for the http tasks (`workflow.http`).
#[derive(Clone)]
pub struct HttpEgress {
    pub policy: SsrfPolicy,
    pub resolver: Arc<dyn Resolver>,
    pub max_redirects: u32,
    /// Used when the task sets no `options.timeout.connectTimeout`.
    pub connect_timeout: Duration,
}

impl HttpEgress {
    pub fn from_config(
        cfg: &egret_config::WorkflowHttpConfig,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self {
            policy: SsrfPolicy {
                allow_hosts: cfg.allow_hosts.clone(),
            },
            resolver,
            max_redirects: cfg.max_redirects,
            connect_timeout: Duration::from_secs(cfg.connect_timeout_secs),
        }
    }

    /// The client for one run. There is no total timeout by default, as in
    /// Kestra: the read-idle timeout and the task's `timeout:` bound it.
    pub fn client(&self, opts: &ResolvedOptions, total: Option<Duration>) -> EgressClient {
        EgressClient::new(
            self.policy.clone(),
            self.resolver.clone(),
            EgressOptions {
                connect_timeout: opts.connect_timeout.unwrap_or(self.connect_timeout),
                timeout: total,
                read_timeout: Some(opts.read_idle_timeout),
                max_redirects: self.max_redirects,
                follow_redirects: opts.follow_redirects,
            },
        )
    }
}

/// Build the request from the RENDERED spec. Runs [`refusals`] first, so a
/// stored flow that slipped past publish still fails naming the property.
/// Every `Err` is a message for the task run; it may quote the URI, so the
/// caller masks it.
pub async fn build_request(
    spec: &HttpSpec,
    files: Option<&dyn FormFiles>,
) -> Result<BuiltRequest, String> {
    let refused = refusals(spec);
    if !refused.is_empty() {
        return Err(refused.join("; "));
    }
    let mut credentials = Vec::new();

    // uri + params
    let raw = match spec.uri.as_ref().filter(|v| !v.is_null()) {
        Some(v) => text(v, "uri")?,
        None => return Err("uri: must not be null".into()),
    };
    let raw = raw.replace(' ', "%20");
    // Not quoted (final review M3): a URI that does not parse cannot be
    // redacted, and a literal password in it is not masked yet.
    let mut url = Url::parse(&raw).map_err(|e| format!("uri: not a valid URI: {e}"))?;
    if let Some(p) = url.password() {
        credentials.push(p.to_string());
        if let Ok(decoded) = percent_decode(p) {
            credentials.push(decoded);
        }
    }
    if let Some(params) = spec.params.as_ref().filter(|p| !p.is_empty()) {
        let mut pairs = Vec::new();
        for (k, v) in params {
            match v {
                Value::Array(items) => {
                    for i in items {
                        pairs.push((k.clone(), text_or_json(i)));
                    }
                }
                Value::String(s) => pairs.push((k.clone(), s.clone())),
                other => return Err(format!("Unsupported param type: {}", kind(other))),
            }
        }
        let mut q = url.query_pairs_mut();
        for (k, v) in &pairs {
            q.append_pair(k, v);
        }
    }

    // method
    let method = match spec.method.as_ref().filter(|v| !v.is_null()) {
        Some(v) => text(v, "method")?,
        None => "GET".to_string(),
    };
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("method: {method:?} is not a valid HTTP method"))?;

    let content_type = match spec.content_type.as_ref() {
        None => Some(DEFAULT_CONTENT_TYPE.to_string()),
        Some(Value::Null) => None,
        Some(v) => Some(text(v, "contentType")?),
    };

    let mut headers = HeaderMap::new();
    let mut body = None;
    let user_headers = spec.headers.as_ref().filter(|h| !h.is_empty());

    // body
    match spec.form_data.as_ref().filter(|f| !f.is_empty()) {
        Some(form) if content_type.as_deref() == Some("multipart/form-data") => {
            let (ct, bytes) = multipart(form, files).await?;
            headers.insert(CONTENT_TYPE, header_value(&ct, "contentType")?);
            body = Some(bytes);
        }
        Some(form) => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in form {
                ser.append_pair(k, &text_or_json(v));
            }
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
            );
            body = Some(Bytes::from(ser.finish()));
        }
        None => match spec.body.as_ref().filter(|v| !v.is_null()) {
            Some(b) => {
                if let Some(ct) = content_type.as_deref() {
                    let ct = if ct.contains(';') {
                        ct.to_string()
                    } else {
                        format!("{ct}; charset=UTF-8")
                    };
                    headers.insert(CONTENT_TYPE, header_value(&ct, "contentType")?);
                }
                body = Some(Bytes::from(text_or_json(b)));
            }
            None => {
                if let (Some(ct), None) = (content_type.as_deref(), user_headers) {
                    headers.insert(CONTENT_TYPE, header_value(ct, "contentType")?);
                }
            }
        },
    }

    // headers (replace what the body set, as Apache keeps a header the
    // request already has)
    if let Some(h) = user_headers {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in h {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| format!("headers: {k:?} is not a valid header name"))?;
            let value = text(v, &format!("headers.{k}"))?;
            if is_credential(&name) {
                credentials.push(value.clone());
            }
            let value = header_value(&value, &format!("headers.{k}"))?;
            if seen.insert(name.clone()) {
                headers.insert(name, value);
            } else {
                headers.append(name, value);
            }
        }
    }

    // auth: set last, replacing a written Authorization
    if let Some(auth) = spec.options.as_ref().and_then(|o| o.auth.as_ref()) {
        let value = match auth.kind.as_deref() {
            Some("BASIC") => {
                let user = java_text(auth.username.as_ref(), "options.auth.username")?;
                let pass = java_text(auth.password.as_ref(), "options.auth.password")?;
                if auth.password.as_ref().is_some_and(|v| !v.is_null()) {
                    credentials.push(pass.clone());
                }
                let blob = base64::engine::general_purpose::STANDARD
                    .encode(format!("{user}:{pass}").as_bytes());
                credentials.push(blob.clone());
                format!("Basic {blob}")
            }
            Some("BEARER") => {
                let token = java_text(auth.token.as_ref(), "options.auth.token")?;
                if auth.token.as_ref().is_some_and(|v| !v.is_null()) {
                    credentials.push(token.clone());
                }
                format!("Bearer {token}")
            }
            Some(other) => {
                return Err(format!(
                    "options.auth.type: {other:?} is not one of BASIC, BEARER"
                ))
            }
            None => return Err("options.auth.type: must not be null (BASIC or BEARER)".into()),
        };
        credentials.push(value.clone());
        headers.insert(AUTHORIZATION, header_value(&value, "options.auth")?);
    }

    headers
        .entry(USER_AGENT)
        .or_insert(HeaderValue::from_static("egret"));

    let uri = url.to_string();
    let mut request = RequestSpec::new(method, uri.clone());
    request.headers = headers;
    request.body = body;
    Ok(BuiltRequest {
        request,
        uri,
        credentials,
    })
}

fn is_credential(name: &HeaderName) -> bool {
    *name == AUTHORIZATION || *name == PROXY_AUTHORIZATION || *name == COOKIE
}

/// A header value; the error never quotes the value (it may be a secret).
fn header_value(v: &str, name: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(v)
        .map_err(|_| format!("{name}: not a valid header value (it contains a control character)"))
}

/// Java's `"" + x` for an absent value: `null`.
fn java_text(v: Option<&Value>, name: &str) -> Result<String, String> {
    match v {
        None | Some(Value::Null) => Ok("null".into()),
        Some(v) => text(v, name),
    }
}

/// `%XX` decoded (a URL's userinfo); `Err` when the result is not UTF-8.
fn percent_decode(s: &str) -> Result<String, std::string::FromUtf8Error> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out)
}

/// `AbstractHttp.java:104-141` + `MultipartRequestBody`: a `kestra://` string
/// is a file part, a `{name, content}` object is a file part named `name`
/// read from the `content` URI, any other string is a text part, and any
/// other value is a text part holding its JSON.
async fn multipart(
    form: &Map<String, Value>,
    files: Option<&dyn FormFiles>,
) -> Result<(String, Bytes), String> {
    let boundary = boundary();
    let mut out = Vec::new();
    for (key, value) in form {
        let file = match value {
            Value::String(s) if s.starts_with("kestra://") => {
                let name = s.rsplit('/').next().unwrap_or("file").to_string();
                Some((name, read_file(files, key, s).await?))
            }
            Value::Object(o) if o.contains_key("name") && o.contains_key("content") => {
                let name = text(&o["name"], &format!("formData.{key}.name"))?;
                let content = text(&o["content"], &format!("formData.{key}.content"))?;
                Some((name, read_file(files, key, &content).await?))
            }
            _ => None,
        };
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some((name, bytes)) => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n\
                         Content-Type: application/octet-stream\r\n\r\n",
                        escape_quoted(key),
                        escape_quoted(&name)
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(&bytes);
            }
            None => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"\r\n\
                         Content-Type: text/plain; charset=UTF-8\r\n\r\n",
                        escape_quoted(key)
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(text_or_json(value).as_bytes());
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((
        format!("multipart/form-data; boundary={boundary}"),
        Bytes::from(out),
    ))
}

async fn read_file(files: Option<&dyn FormFiles>, key: &str, uri: &str) -> Result<Bytes, String> {
    if !uri.starts_with("kestra://") {
        return Err(format!(
            "formData.{key}: a file part's content must be a kestra:// URI of internal storage"
        ));
    }
    match files {
        Some(f) => f
            .read(uri)
            .await
            .map_err(|e| format!("formData.{key}: {e}")),
        None => Err(format!(
            "formData.{key}: a file part needs internal storage, and this deployment has no \
             [storage] configured"
        )),
    }
}

/// The HTML form-data escaping of a quoted parameter (WHATWG): `"` and line
/// breaks percent-encoded, so a key cannot end the header early.
fn escape_quoted(s: &str) -> String {
    s.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn boundary() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    format!(
        "egret-{}",
        Alphanumeric.sample_string(&mut rand::thread_rng(), 30)
    )
}
