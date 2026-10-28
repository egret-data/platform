//! The response side: Kestra's status rule (`HttpClient.isAllowedStatusCode`),
//! its failure message (`HttpResponseFailure`), the capped body read, the
//! `headers` output, and transport errors as task-run messages.
//!
//! **The status rule is Kestra's, not "2xx".** `allowedResponseCodes`, when
//! set and non-empty, is the whole rule (2xx included only if listed); else
//! `allowFailed` passes everything; else a code below 400 passes -- so a 3xx
//! returned with `followRedirects: false` succeeds, as in Kestra.
//!
//! **The failure message carries at most [`FAILURE_BODY_BYTES`] of the body,
//! masked** (spec D6; Kestra puts the whole body in it, and that message is
//! the task run's message and a log line).

use bytes::{Bytes, BytesMut};
use egret_egress::{DestinationError, EgressError};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value};

use super::mask::MaskSet;
use super::spec::ResolvedOptions;

/// The most response-body bytes a failure message quotes.
pub const FAILURE_BODY_BYTES: usize = 4096;

/// `HttpClient.isAllowedStatusCode`.
pub fn check_status(code: u16, opts: &ResolvedOptions) -> bool {
    if let Some(codes) = &opts.allowed_response_codes {
        return codes.contains(&code);
    }
    opts.allow_failed || code < 400
}

/// Kestra's `"Failed http request with response code '<code>'"`, plus
/// `" and body:\n<body>"` when there is one: here at most
/// [`FAILURE_BODY_BYTES`] of it, masked, with a note when it was cut.
/// `body_cut`: the caller stopped reading before the body's end.
pub fn failure_message(code: u16, body: &[u8], body_cut: bool, masks: &MaskSet) -> String {
    let mut msg = format!("Failed http request with response code '{code}'");
    if !body.is_empty() {
        let (text, cut) = masks.truncate_masked(body, FAILURE_BODY_BYTES);
        msg.push_str(" and body:\n");
        msg.push_str(&text);
        if cut || body_cut {
            msg.push_str(&format!(
                "\n[body truncated to its first {FAILURE_BODY_BYTES} bytes]"
            ));
        }
    }
    msg
}

/// `Ok(res)` when [`check_status`] passes; otherwise reads the head of the
/// body and returns [`failure_message`].
pub async fn ensure_allowed(
    res: reqwest::Response,
    opts: &ResolvedOptions,
    masks: &MaskSet,
) -> Result<reqwest::Response, String> {
    let code = res.status().as_u16();
    if check_status(code, opts) {
        return Ok(res);
    }
    // Enough past the limit that a value straddling it is read whole.
    let want = FAILURE_BODY_BYTES + masks.longest() + 1;
    let (body, cut) = read_prefix(res, want).await;
    Err(failure_message(code, &body, cut, masks))
}

/// At most `n` bytes of the body, best effort (a read error ends it). `true`
/// when the body went on past `n`.
pub async fn read_prefix(mut res: reqwest::Response, n: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    while out.len() <= n {
        match res.chunk().await {
            Ok(Some(c)) => out.extend_from_slice(&c),
            _ => break,
        }
    }
    let cut = out.len() > n;
    out.truncate(n);
    (out, cut)
}

/// The body was over the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyTooLarge {
    pub max: u64,
    /// `Content-Length`, when the server sent one.
    pub length: Option<u64>,
}

impl std::fmt::Display for BodyTooLarge {
    /// `Request.java:377-382`, adapted: egret stops reading at the cap, so the
    /// size is `Content-Length` or "more than" the cap.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = match self.length {
            Some(l) => format!("{l} bytes"),
            None => format!("more than {} bytes", self.max),
        };
        write!(
            f,
            "Response body is too large to store in task outputs ({n} > max {} bytes). Use \
             io.kestra.plugin.core.http.Download to fetch large payloads as files instead.",
            self.max
        )
    }
}

/// What reading a body can fail with.
#[derive(Debug)]
pub enum ReadError {
    TooLarge(BodyTooLarge),
    /// The connection failed or timed out mid-body (no URL in the text).
    Transport(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::TooLarge(t) => t.fmt(f),
            ReadError::Transport(t) => f.write_str(t),
        }
    }
}

/// The whole body, failing as soon as it passes `max` bytes (never buffering
/// more than `max` plus one chunk). A `Content-Length` over `max` fails before
/// any read.
pub async fn read_capped(mut res: reqwest::Response, max: u64) -> Result<Bytes, ReadError> {
    let length = res.content_length();
    if length.is_some_and(|l| l > max) {
        return Err(ReadError::TooLarge(BodyTooLarge { max, length }));
    }
    let mut out = BytesMut::new();
    loop {
        match res.chunk().await {
            Ok(Some(c)) => {
                if (out.len() + c.len()) as u64 > max {
                    return Err(ReadError::TooLarge(BodyTooLarge { max, length }));
                }
                out.extend_from_slice(&c);
            }
            Ok(None) => return Ok(out.freeze()),
            Err(e) if e.is_timeout() => {
                return Err(ReadError::Transport("request timed out".into()))
            }
            Err(e) => return Err(ReadError::Transport(e.without_url().to_string())),
        }
    }
}

/// The `headers` output: `Map<String, List<String>>`, as Kestra's
/// `HttpHeaders.map()`. Names are lower-case (HTTP/2 and hyper normalise them).
pub fn headers_output(headers: &HeaderMap) -> Value {
    let mut out = Map::new();
    for name in headers.keys() {
        let values: Vec<Value> = headers
            .get_all(name)
            .iter()
            .map(|v| Value::String(String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        out.insert(name.as_str().to_string(), Value::Array(values));
    }
    Value::Object(out)
}

/// An egress failure as a task-run message, masked. A refused destination
/// says how an operator opens a private host.
pub fn transport_message(err: &EgressError, masks: &MaskSet) -> String {
    let text = match err {
        EgressError::Destination {
            source: DestinationError::Blocked(..),
            ..
        } => format!(
            "{err}. egret refuses private, loopback, link-local and metadata addresses; an \
             operator can allow a host with workflow.http.allow_hosts"
        ),
        other => other.to_string(),
    };
    masks.mask(&text)
}
