//! The shared properties of the three http tasks, as Kestra names them
//! (`AbstractHttp.java`, `HttpConfiguration.java` and the auth, proxy, ssl and
//! timeout configurations), and the D4 refusals.
//!
//! **One model for both stages.** Publish has the unrendered task (a
//! `Property<Boolean>` may still be `"{{ inputs.x }}"`); the handler has it
//! rendered. Every templatable scalar is therefore read leniently here (a
//! [`Flag`] is a bool or a string) and converted, with an error naming the
//! property, only when the run asks for it ([`HttpSpec::options`],
//! `request::build_request`). [`refusals`] reads either stage.
//!
//! `proxy`, `ssl`, `DIGEST` and `encryptBody` are modelled **only to refuse
//! them** (spec D4), under Kestra's own keys so a flow written for Kestra gets
//! a refusal that names the property it wrote.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

/// `options.timeout.readIdleTimeout`'s default (`TimeoutConfiguration.java:18-19`).
pub const DEFAULT_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The task's properties. Unknown keys (`id`, `type`, `retry`, a task's own
/// properties such as `saveAs`) are ignored: this is the shared part only.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpSpec {
    /// `@NotNull`. Rendered; a space becomes `%20` (`AbstractHttp.java:75`).
    pub uri: Option<Value>,
    /// Default `GET`. A free string, not an enum.
    pub method: Option<Value>,
    /// Sent only when `formData` is empty. A non-string value is sent as its
    /// JSON text, as Kestra's `Property<String>` renders one.
    pub body: Option<Value>,
    /// Non-empty wins over `body`: multipart when `contentType` is exactly
    /// `multipart/form-data`, else `application/x-www-form-urlencoded`.
    pub form_data: Option<Map<String, Value>>,
    /// Query parameters: a string, or a list for a repeated key.
    pub params: Option<Map<String, Value>>,
    /// Default `application/json`, sent even with no body.
    pub content_type: Option<Value>,
    pub headers: Option<Map<String, Value>>,
    pub options: Option<HttpOptions>,
    /// `Request` only. Refused (D4).
    pub encrypt_body: Option<Value>,
}

/// `HttpConfiguration`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpOptions {
    pub timeout: Option<TimeoutOptions>,
    /// Refused when it names an address (D4).
    pub proxy: Option<ProxyOptions>,
    pub auth: Option<AuthOptions>,
    /// `insecureTrustAllCertificates` refused unless absent or false (D4).
    pub ssl: Option<SslOptions>,
    /// Default true.
    pub follow_redirects: Option<Value>,
    /// Default false.
    pub allow_failed: Option<Value>,
    /// When set and non-empty, the ONLY codes that pass, 2xx included.
    pub allowed_response_codes: Option<Value>,
    /// Kestra's Windows workaround. Accepted and ignored.
    pub enabled_tcp_extended_keep_alive: Option<Value>,
    /// The request body's charset. Only UTF-8 (the default) is supported.
    pub default_charset: Option<Value>,
    /// `REQUEST_HEADERS`, `REQUEST_BODY`, `RESPONSE_HEADERS`, `RESPONSE_BODY`.
    pub logs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimeoutOptions {
    pub connect_timeout: Option<Value>,
    pub read_idle_timeout: Option<Value>,
}

/// `ProxyConfiguration`, modelled to be refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxyOptions {
    #[serde(rename = "type")]
    pub kind: Option<Value>,
    pub address: Option<Value>,
    pub port: Option<Value>,
    pub username: Option<Value>,
    pub password: Option<Value>,
}

/// `SslOptions`, modelled to be refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SslOptions {
    pub insecure_trust_all_certificates: Option<Value>,
}

/// `AbstractAuthConfiguration`, polymorphic on `type` (not a `Property`, so
/// never rendered).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthOptions {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// BASIC (and DIGEST).
    pub username: Option<Value>,
    /// BASIC (and DIGEST). `@PluginProperty(secret = true)`.
    pub password: Option<Value>,
    /// BEARER. `@PluginProperty(secret = true)`.
    pub token: Option<Value>,
}

/// `HttpConfiguration.LoggingType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    RequestHeaders,
    RequestBody,
    ResponseHeaders,
    ResponseBody,
}

impl LogKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "REQUEST_HEADERS" => Self::RequestHeaders,
            "REQUEST_BODY" => Self::RequestBody,
            "RESPONSE_HEADERS" => Self::ResponseHeaders,
            "RESPONSE_BODY" => Self::ResponseBody,
            _ => return None,
        })
    }
}

/// The run-time reading of `options`, every value converted.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedOptions {
    pub follow_redirects: bool,
    pub allow_failed: bool,
    /// `None` when absent or empty (Kestra: `!= null && !isEmpty()`).
    pub allowed_response_codes: Option<Vec<u16>>,
    /// `None`: the deployment's `workflow.http.connect_timeout_secs`.
    pub connect_timeout: Option<Duration>,
    pub read_idle_timeout: Duration,
    pub logs: Vec<LogKind>,
}

impl HttpSpec {
    /// Read the task's (rendered or unrendered) properties. Fails only on a
    /// shape no value could fix (`options` that is not an object, say); a
    /// wrong scalar is reported when it is used.
    pub fn from_value(props: &Value) -> Result<Self, String> {
        serde_json::from_value(props.clone()).map_err(|e| format!("invalid http task: {e}"))
    }

    /// `options`, converted. Call on the rendered spec.
    pub fn options(&self) -> Result<ResolvedOptions, String> {
        let o = self.options.clone().unwrap_or_default();
        let follow_redirects =
            flag(o.follow_redirects.as_ref(), "options.followRedirects")?.unwrap_or(true);
        let allow_failed = flag(o.allow_failed.as_ref(), "options.allowFailed")?.unwrap_or(false);
        let allowed_response_codes = codes(o.allowed_response_codes.as_ref())?;
        let t = o.timeout.unwrap_or_default();
        let connect_timeout =
            duration(t.connect_timeout.as_ref(), "options.timeout.connectTimeout")?;
        let read_idle_timeout = duration(
            t.read_idle_timeout.as_ref(),
            "options.timeout.readIdleTimeout",
        )?
        .unwrap_or(DEFAULT_READ_IDLE_TIMEOUT);
        if let Some(c) = o.default_charset.as_ref().filter(|v| !v.is_null()) {
            let name = text(c, "options.defaultCharset")?;
            let n = name.to_ascii_lowercase().replace(['-', '_'], "");
            if n != "utf8" {
                return Err(format!(
                    "options.defaultCharset: {name:?} is not supported; egret sends a request \
                     body as UTF-8 only"
                ));
            }
        }
        let mut logs = Vec::new();
        for l in o.logs.unwrap_or_default() {
            match LogKind::parse(&l) {
                Some(k) => logs.push(k),
                None => {
                    return Err(format!(
                        "options.logs: {l:?} is not one of REQUEST_HEADERS, REQUEST_BODY, \
                         RESPONSE_HEADERS, RESPONSE_BODY"
                    ))
                }
            }
        }
        Ok(ResolvedOptions {
            follow_redirects,
            allow_failed,
            allowed_response_codes,
            connect_timeout,
            read_idle_timeout,
            logs,
        })
    }
}

/// The D4 refusals: properties egret will not honour because each would let a
/// flow author step around the egress guard or a stated guarantee. Each entry
/// names the property under Kestra's key and the reason. Empty: none applies.
///
/// Accepts the unrendered spec (publish) and the rendered one (run): a value
/// that is still a template where only "absent or false" is allowed is
/// refused, because what it renders to cannot be known at publish.
pub fn refusals(spec: &HttpSpec) -> Vec<String> {
    let mut out = Vec::new();
    let o = spec.options.clone().unwrap_or_default();
    if let Some(p) = &o.proxy {
        if p.address.as_ref().is_some_and(|a| !is_blank(a)) {
            out.push(
                "options.proxy: a proxy chosen by the flow is not supported: the proxy, not \
                 egret, would resolve and reach the host, past egret's egress check. Remove \
                 options.proxy"
                    .to_string(),
            );
        }
    }
    if let Some(s) = &o.ssl {
        if !absent_or_false(s.insecure_trust_all_certificates.as_ref()) {
            out.push(
                "options.ssl.insecureTrustAllCertificates: not supported: egret always \
                 verifies the server's TLS certificate. Remove it or set it to false"
                    .to_string(),
            );
        }
    }
    if let Some(a) = &o.auth {
        if a.kind.as_deref() == Some("DIGEST") {
            out.push(
                "options.auth.type: DIGEST is not supported by egret; use BASIC or BEARER, \
                 or send the credentials in headers"
                    .to_string(),
            );
        }
    }
    if uri_has_userinfo(spec.uri.as_ref()) {
        out.push(
            "uri: credentials in the URI (user:password@host) are not supported: the URI is \
             stored as the task's `uri` output. Use options.auth or an Authorization header"
                .to_string(),
        );
    }
    if !absent_or_false(spec.encrypt_body.as_ref()) {
        out.push(
            "encryptBody: not supported by egret: the response body is stored as the `body` \
             output. Remove it or set it to false"
                .to_string(),
        );
    }
    out
}

/// Whether `uri` parses (as the request builder parses it) with a user name
/// or a password. Unrendered, a URI whose host is still a template does not
/// parse and is judged at run time, once rendered.
fn uri_has_userinfo(uri: Option<&Value>) -> bool {
    let Some(Value::String(raw)) = uri else {
        return false;
    };
    url::Url::parse(&raw.replace(' ', "%20"))
        .is_ok_and(|u| !u.username().is_empty() || u.password().is_some())
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

pub(crate) fn absent_or_false(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => true,
        Some(Value::String(s)) => s.trim() == "false",
        Some(_) => false,
    }
}

/// A scalar as text: a string as is, a number or bool as written. Anything
/// else is an error naming `name`.
pub(crate) fn text(v: &Value, name: &str) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(format!("{name}: expected a string, got {}", kind(other))),
    }
}

/// A scalar as text, or JSON text for a list or an object (Kestra's
/// `Property<String>` of a non-string value).
pub(crate) fn text_or_json(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

pub(crate) fn flag(v: Option<&Value>, name: &str) -> Result<Option<bool>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::String(s)) => match s.trim() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            other => Err(format!("{name}: {other:?} is not a boolean")),
        },
        Some(other) => Err(format!("{name}: expected a boolean, got {}", kind(other))),
    }
}

fn codes(v: Option<&Value>) -> Result<Option<Vec<u16>>, String> {
    const NAME: &str = "options.allowedResponseCodes";
    let list = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a.clone(),
        // A rendered template: its text is the list.
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => a,
            _ => return Err(format!("{NAME}: {s:?} is not a list of status codes")),
        },
        Some(other) => {
            return Err(format!(
                "{NAME}: expected a list of status codes, got {}",
                kind(other)
            ))
        }
    };
    let mut out = Vec::with_capacity(list.len());
    for c in &list {
        let n = match c {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.trim().parse::<u64>().ok(),
            _ => None,
        };
        match n.and_then(|n| u16::try_from(n).ok()) {
            Some(n) => out.push(n),
            None => return Err(format!("{NAME}: {c} is not a status code")),
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

/// An ISO-8601 duration (`PT30S`, Java's `Duration.parse`), or a number of
/// seconds.
/// The longest `options.timeout.*` egret accepts: one day. A larger number
/// once reached `Duration::from_secs_f64`, which panics above `u64::MAX`
/// seconds, and a panicking handler is re-claimed and panics again on every
/// lease (final review I2). A day is far beyond any useful connect or
/// read-idle timeout; the task's own `timeout:` bounds the whole run.
pub(crate) const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

fn duration(v: Option<&Value>, name: &str) -> Result<Option<Duration>, String> {
    let d = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => {
            let secs = n
                .as_f64()
                .ok_or_else(|| format!("{name}: {n} is not a duration"))?;
            seconds(secs, name)?
        }
        Some(Value::String(s)) => crate::executor::parse_duration(s)
            .ok_or_else(|| format!("{name}: {s:?} is not an ISO-8601 duration (e.g. PT30S)"))?,
        Some(other) => return Err(format!("{name}: expected a duration, got {}", kind(other))),
    };
    if d > MAX_TIMEOUT {
        return Err(format!(
            "{name}: {} seconds is too large (at most {} seconds)",
            d.as_secs(),
            MAX_TIMEOUT.as_secs()
        ));
    }
    Ok(Some(d))
}

/// A number of seconds as a `Duration`, never panicking: negative, NaN,
/// infinite and out-of-range values are refused naming `name`.
pub(crate) fn seconds(secs: f64, name: &str) -> Result<Duration, String> {
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("{name}: {secs} is not a duration"));
    }
    Duration::try_from_secs_f64(secs).map_err(|_| {
        format!(
            "{name}: {secs} seconds is too large (at most {} seconds)",
            MAX_TIMEOUT.as_secs()
        )
    })
}
