//! Masking for everything an http task writes: its log lines, its failure
//! messages, its transport errors (spec D5).
//!
//! Two sources of values: the secrets `secret()` resolved
//! ([`crate::runners::secrets::Resolved::values`]), and the credentials the
//! task wrote itself ([`super::BuiltRequest::credentials`]: `options.auth`, a
//! credential header, a URL password), masked whether or not they came from
//! `secret()`. Each value is also masked in its percent-encoded forms: the
//! form encoding, and the `url` crate's path, query and fragment encodings,
//! the forms it takes in a URL a message may quote.
//!
//! Credential headers (`Authorization`, `Proxy-Authorization`, `Cookie`,
//! `Set-Cookie`) are never printed: their value is replaced by [`MASK`]
//! before any value masking, so a credential no list knows about is still not
//! written.

use reqwest::header::{
    HeaderMap, HeaderName, AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION, SET_COOKIE,
};
use reqwest::{Method, StatusCode};

use super::spec::LogKind;
use crate::logs::{mask, LogLevel, MASK};

/// The most body bytes an `options.logs` payload line carries.
pub const LOG_BODY_BYTES: usize = 64 * 1024;

/// The values to mask in anything an http task writes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MaskSet {
    values: Vec<String>,
}

impl std::fmt::Debug for MaskSet {
    /// A count: a value must never reach a `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MaskSet(<{} masked>)", self.values.len())
    }
}

impl MaskSet {
    /// `values`: the resolved secrets and the task's credentials. Empty values
    /// are dropped; each value's percent-encoded form is added when it differs.
    pub fn new<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut out = Self::default();
        out.extend(values);
        out
    }

    pub fn extend<I, S>(&mut self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for v in values {
            let v: String = v.into();
            if v.is_empty() {
                continue;
            }
            let encoded: String = url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
            let pct = encoded.replace('+', "%20");
            let mut forms = vec![encoded, pct];
            forms.extend(url_forms(&v));
            for form in forms {
                if form != v && !self.values.contains(&form) {
                    self.values.push(form);
                }
            }
            if !self.values.contains(&v) {
                self.values.push(v);
            }
        }
    }

    /// Every value, for `RunLog::masked`.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// The longest value's length in bytes (0 when empty).
    pub fn longest(&self) -> usize {
        self.values.iter().map(String::len).max().unwrap_or(0)
    }

    pub fn mask(&self, text: &str) -> String {
        mask(text, &self.values).into_owned()
    }

    /// `bytes` decoded as UTF-8 (lossily), cut to about `limit` bytes, and
    /// masked. `true` when anything was cut.
    ///
    /// **Masked before it is cut, and never cut inside a value.** Cutting
    /// first could leave the head of a secret readable at the end; so the cut
    /// moves past any value that straddles it, and the caller passes at least
    /// `limit + longest()` bytes so such a value is whole. The result may be
    /// longer than `limit` by at most one value, which is then masked.
    pub fn truncate_masked(&self, bytes: &[u8], limit: usize) -> (String, bool) {
        let text = String::from_utf8_lossy(bytes);
        if text.len() <= limit {
            return (self.mask(&text), false);
        }
        let mut cut = limit;
        // Until no value straddles the cut: moving it can make another one.
        loop {
            let before = cut;
            for v in self.values.iter().filter(|v| !v.is_empty()) {
                for (start, _) in text.match_indices(v.as_str()) {
                    if start >= cut {
                        break;
                    }
                    cut = cut.max(start + v.len());
                }
            }
            if cut == before {
                break;
            }
        }
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let truncated = cut < text.len();
        (self.mask(&text[..cut]), truncated)
    }
}

/// `v` as the `url` crate writes it in a path, a query and a fragment (final
/// review I5). A secret rendered into the `uri` is encoded by `Url::parse`
/// with these component sets, which leave `@`, `:`, `/` and friends alone
/// but encode a space, `"`, `<`, `>` and non-ASCII -- neither the raw value
/// nor its form encoding. The forms are produced by the same `Url` setters,
/// so they cannot drift from what a parsed `uri` holds.
fn url_forms(v: &str) -> Vec<String> {
    let Ok(mut u) = url::Url::parse("http://h/") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    u.set_query(Some(v));
    out.extend(u.query().map(str::to_string));
    u.set_query(None);
    u.set_fragment(Some(v));
    out.extend(u.fragment().map(str::to_string));
    u.set_fragment(None);
    u.set_path(&format!("/{v}"));
    if let Some(path) = u.path().strip_prefix('/') {
        out.push(path.to_string());
    }
    out.retain(|f| !f.is_empty());
    out
}

/// Credential headers, whose value is never printed.
fn is_secret_header(name: &HeaderName) -> bool {
    *name == AUTHORIZATION || *name == PROXY_AUTHORIZATION || *name == COOKIE || *name == SET_COOKIE
}

/// Kestra's `buildHeadersEntry`, with credential headers' values replaced by
/// [`MASK`] and everything else masked.
pub fn headers_entry(kind: &str, headers: &HeaderMap, masks: &MaskSet) -> String {
    if headers.is_empty() {
        return format!("{kind} headers: null");
    }
    let lines: Vec<String> = headers
        .iter()
        .map(|(name, value)| {
            let value = if is_secret_header(name) {
                MASK.to_string()
            } else {
                String::from_utf8_lossy(value.as_bytes()).into_owned()
            };
            format!("{name}: {value}")
        })
        .collect();
    masks.mask(&format!("{kind} headers:\n    {}", lines.join("\n    ")))
}

/// The bytes of `body` a payload line can need: [`LOG_BODY_BYTES`] plus the
/// longest masked value (so a value straddling the cut is whole), and whether
/// anything was left out. Cut **before** the lossy UTF-8 decode, so a large
/// binary body is never decoded (and tripled by U+FFFD) whole only to print
/// 64 KiB of it (final review I4).
pub(crate) fn log_head<'a>(body: &'a [u8], masks: &MaskSet) -> (&'a [u8], bool) {
    let keep = LOG_BODY_BYTES.saturating_add(masks.longest());
    if body.len() > keep {
        (&body[..keep], true)
    } else {
        (body, false)
    }
}

fn payload_entry(kind: &str, body: Option<&[u8]>, masks: &MaskSet) -> String {
    match body {
        None => format!("{kind} payload: null"),
        Some(b) => {
            let (head, cut_early) = log_head(b, masks);
            let (text, cut) = masks.truncate_masked(head, LOG_BODY_BYTES);
            let more = if cut || cut_early {
                "\n    [truncated]"
            } else {
                ""
            };
            format!("{kind} payload:\n    {text}{more}")
        }
    }
}

/// The `options.logs` lines for the request (`LoggingRequestInterceptor`):
/// `REQUEST_HEADERS` at DEBUG, `REQUEST_BODY` at TRACE.
pub fn log_request_lines(
    method: &Method,
    uri: &str,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    logs: &[LogKind],
    masks: &MaskSet,
) -> Vec<(LogLevel, String)> {
    let mut out = Vec::new();
    if logs.contains(&LogKind::RequestHeaders) {
        out.push((
            LogLevel::Debug,
            masks.mask(&format!("request:\n    method: {method}\n    uri: {uri}")),
        ));
        out.push((LogLevel::Debug, headers_entry("request", headers, masks)));
    }
    if logs.contains(&LogKind::RequestBody) {
        out.push((LogLevel::Trace, payload_entry("request", body, masks)));
    }
    out
}

/// The `options.logs` lines for the response (`LoggingResponseInterceptor`):
/// `RESPONSE_HEADERS` at DEBUG, `RESPONSE_BODY` at TRACE.
pub fn log_response_lines(
    status: StatusCode,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    logs: &[LogKind],
    masks: &MaskSet,
) -> Vec<(LogLevel, String)> {
    let mut out = Vec::new();
    if logs.contains(&LogKind::ResponseHeaders) {
        out.push((
            LogLevel::Debug,
            format!(
                "response:\n    reason: {}",
                status.canonical_reason().unwrap_or("")
            ),
        ));
        out.push((LogLevel::Debug, headers_entry("response", headers, masks)));
    }
    if logs.contains(&LogKind::ResponseBody) {
        out.push((LogLevel::Trace, payload_entry("response", body, masks)));
    }
    out
}
