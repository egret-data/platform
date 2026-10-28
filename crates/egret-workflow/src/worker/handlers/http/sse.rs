//! `io.kestra.plugin.core.http.SseRequest` (package #4 Task C4, spec D8):
//! send one request through the egress guard, read the response as a
//! Server-Sent Events stream, and return the events or a JQ extraction.
//!
//! The request side is [`Request`](super::Request)'s ([`super::prepare`]).
//! Then, as `SseRequest.java:131-203` and `HttpClient.sseRequest`:
//!
//! - the stream is parsed by [`crate::http::sse`] (Kestra's parser, line by
//!   line) until the server closes it;
//! - each event's `data` is read as JSON when it parses (the first value, as
//!   Jackson's `readValue` does), else kept as a string;
//! - with `concatJqExpression`, the program runs over each event as
//!   `{data, id, name, comment, retry}` (null fields left out, Jackson's
//!   `NON_NULL`); a text, number or boolean result is kept as text, the LAST
//!   one per event; an event whose program fails or yields anything else
//!   fails the task with `Failed to resolve JQ expression '<q>' and value
//!   '<event>'` when `failedOnMissingJq` (default true), else is skipped;
//! - outputs: `uri` (the request URI), `size` (the events received),
//!   `events` (every event, only when no JQ result was kept) and `result`
//!   (the kept results joined, only when there is one). An absent output is
//!   left out, as Kestra's `NON_NULL` output map does.
//!
//! # Where egret differs, on purpose (spec D8, and the C4 decisions)
//!
//! - **The status rule applies** (Kestra never checks): a 400 or above fails
//!   with Request's message, unless `options.allowFailed` or
//!   `options.allowedResponseCodes` passes it.
//! - **`workflow.http.max_sse_events`** (default 10 000): at that many events
//!   the stream is closed and the task SUCCEEDS with them, logging a warning.
//!   The task's `timeout:` also stops a stream (as a failure, the budget's).
//! - **Memory bounds**: the stream may carry at most [`MAX_STREAM_BYTES`]
//!   and the outputs may be at most [`MAX_OUTPUT_BYTES`] of JSON (egret's
//!   render cap); crossing either fails, naming it.
//! - **JQ is `jaq`** through the `jq` filter's sandbox, not jackson-jq 1.6:
//!   a program jaq does not parse (jackson-jq's `$.field` path form, say)
//!   fails at the start with egret's message.
//! - An event whose `data` lines are all empty has `data` null (Kestra fails
//!   the task there, on `JacksonMapper.toObject(null)`).
//! - `retry` is a number of seconds (Jackson writes the `Duration` that way).

use std::sync::Arc;

use egret_core::Result;
use serde_json::{json, Map, Value};

use super::super::super::{Outcome, TaskHandler, TaskInput};
use super::{prepare, Prepared, MULTIPART_CAP_WITHOUT_STORAGE, SSE_REQUEST};
use crate::http::spec::{flag, kind};
use crate::http::sse::{SseEvent, SseParser};
use crate::http::{ensure_allowed, log_response_lines, transport_message, HttpEgress};
use crate::logs::LogLevel;
use crate::runners::secrets::SecretSource;
use crate::runners::{jq_each, JqScalar};
use crate::storage::FileStore;

/// The most stream bytes one `SseRequest` holds (16 MiB).
pub const MAX_STREAM_BYTES: u64 = 16 << 20;

/// The largest output map, as JSON (egret's render cap, 1 MiB).
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// The properties `SseRequest` reads besides the shared ones, for the
/// save-time key check (`spec::keys`).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct Spec {
    uri: Option<Value>,
    method: Option<Value>,
    body: Option<Value>,
    form_data: Option<Value>,
    params: Option<Value>,
    content_type: Option<Value>,
    headers: Option<Value>,
    options: Option<Value>,
    concat_jq_expression: Option<Value>,
    failed_on_missing_jq: Option<Value>,
}

/// `io.kestra.plugin.core.http.SseRequest`.
pub struct SseRequest {
    egress: HttpEgress,
    secrets: Arc<dyn SecretSource>,
    files: Option<Arc<FileStore>>,
    max_events: u64,
}

impl SseRequest {
    pub fn new(
        egress: HttpEgress,
        secrets: Arc<dyn SecretSource>,
        files: Option<Arc<FileStore>>,
        max_events: u64,
    ) -> Self {
        Self {
            egress,
            secrets,
            files,
            max_events,
        }
    }
}

#[async_trait::async_trait]
impl TaskHandler for SseRequest {
    fn type_id(&self) -> &'static str {
        SSE_REQUEST
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let multipart_cap = Some(
            self.files
                .as_ref()
                .map_or(MULTIPART_CAP_WITHOUT_STORAGE, |f| f.caps.max_file_bytes),
        );
        let Prepared {
            rendered,
            opts,
            built,
            masks,
            log,
        } = match prepare(
            SSE_REQUEST,
            task,
            self.secrets.as_ref(),
            self.files.as_ref(),
            multipart_cap,
        )
        .await?
        {
            Ok(p) => p,
            Err(failed) => return Ok(failed),
        };
        let failed = |m: &str| Ok(Outcome::failed_with(masks.mask(m)));

        let jq = match rendered.get("concatJqExpression") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => {
                return failed(&format!(
                    "concatJqExpression: expected a string, got {}",
                    kind(other)
                ))
            }
        };
        let fail_on_missing = match flag(rendered.get("failedOnMissingJq"), "failedOnMissingJq") {
            Ok(f) => f.unwrap_or(true),
            Err(m) => return failed(&m),
        };
        // Kestra compiles before the request: a program that cannot run
        // sends nothing.
        if let Some(q) = &jq {
            if let Err(m) = jq_each(q, Vec::new()) {
                return failed(&format!("concatJqExpression: {m}"));
            }
        }

        let res = match self.egress.client(&opts, None).send(built.request).await {
            Ok(r) => r,
            Err(e) => return failed(&transport_message(&e, &masks)),
        };
        let status = res.status();
        let headers = res.headers().clone();
        let mut res = match ensure_allowed(res, &opts, &masks).await {
            Ok(r) => r,
            Err(m) => return failed(&m),
        };
        for (level, line) in log_response_lines(status, &headers, None, &opts.logs, &masks) {
            log.line(level, &line).await;
        }

        // Read until the server closes the stream or the event cap is met.
        let mut parser = SseParser::new();
        let mut events: Vec<SseEvent> = Vec::new();
        let mut read: u64 = 0;
        let mut capped = false;
        loop {
            match res.chunk().await {
                Ok(Some(c)) => {
                    read += c.len() as u64;
                    if read > MAX_STREAM_BYTES {
                        return failed(&format!(
                            "The event stream is too large (more than {MAX_STREAM_BYTES} bytes); \
                             egret holds at most that much of one stream"
                        ));
                    }
                    parser.feed(&c, &mut events);
                    if events.len() as u64 >= self.max_events {
                        events.truncate(self.max_events as usize);
                        capped = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) if e.is_timeout() => return failed("request timed out"),
                Err(e) => return failed(&e.without_url().to_string()),
            }
        }
        drop(res);
        if capped {
            log.line(
                LogLevel::Warn,
                &format!(
                    "Stopped reading the event stream after {} events \
                     (workflow.http.max_sse_events)",
                    self.max_events
                ),
            )
            .await;
        }

        let values: Vec<Value> = events.iter().map(event_value).collect();
        let mut results: Vec<String> = Vec::new();
        if let Some(q) = &jq {
            let inputs = values.iter().map(Value::to_string).collect();
            let each = match jq_each(q, inputs) {
                Ok(e) => e,
                Err(m) => return failed(&format!("concatJqExpression: {m}")),
            };
            for (one, value) in each.into_iter().zip(&values) {
                if let Some(m) = one.refused {
                    return failed(&format!("concatJqExpression: {m}"));
                }
                let mut kept = None;
                let mut missing = one.failed;
                for out in one.outputs {
                    match out {
                        JqScalar::Text(t) => kept = Some(t),
                        JqScalar::NotText(_) => missing = true,
                    }
                }
                if missing {
                    let error =
                        format!("Failed to resolve JQ expression '{q}' and value '{value}'");
                    if fail_on_missing {
                        return failed(&error);
                    }
                    log.line(LogLevel::Debug, &format!("{error}. Skipping this event."))
                        .await;
                }
                if let Some(t) = kept {
                    results.push(t);
                }
            }
        }

        let mut out = Map::new();
        out.insert("uri".into(), json!(built.uri));
        if results.is_empty() {
            out.insert("events".into(), Value::Array(values));
        }
        out.insert("size".into(), json!(events.len()));
        if !results.is_empty() {
            out.insert("result".into(), Value::String(results.concat()));
        }
        let out = Value::Object(out);
        let bytes = out.to_string().len();
        if bytes > MAX_OUTPUT_BYTES {
            return failed(&format!(
                "The task outputs are too large ({bytes} bytes > max {MAX_OUTPUT_BYTES} bytes). \
                 Use concatJqExpression to keep only what is needed"
            ));
        }
        Ok(Outcome::succeeded_with(out))
    }

    /// Nothing to cancel: the stream is dropped with the future.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// One event as Kestra's `HttpSseEvent` serialises (`NON_NULL`), its data
/// read as JSON when it parses.
pub(crate) fn event_value(e: &SseEvent) -> Value {
    let mut m = Map::new();
    if let Some(d) = &e.data {
        m.insert("data".into(), data_value(d));
    }
    if let Some(v) = &e.id {
        m.insert("id".into(), json!(v));
    }
    if let Some(v) = &e.name {
        m.insert("name".into(), json!(v));
    }
    if let Some(v) = &e.comment {
        m.insert("comment".into(), json!(v));
    }
    if let Some(ms) = e.retry_ms {
        m.insert("retry".into(), json!(ms as f64 / 1000.0));
    }
    Value::Object(m)
}

/// `JacksonMapper.toObject`: the first JSON value of the text (trailing
/// content ignored, as Jackson's default), or the text itself when it does
/// not parse.
pub(crate) fn data_value(text: &str) -> Value {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    match stream.next() {
        Some(Ok(v)) => v,
        _ => Value::String(text.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_json_when_it_parses_else_a_string() {
        assert_eq!(data_value(r#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(data_value("42"), json!(42));
        assert_eq!(data_value(r#""quoted""#), json!("quoted"));
        assert_eq!(data_value("plain text"), json!("plain text"));
        assert_eq!(data_value("  "), json!("  "));
        assert_eq!(data_value(r#"{"a":1} tail"#), json!({"a": 1}));
    }

    #[test]
    fn an_event_leaves_out_its_null_fields() {
        let e = SseEvent {
            data: Some("{\"m\":\"hi\"}".into()),
            id: Some("1".into()),
            retry_ms: Some(1500),
            ..SseEvent::default()
        };
        assert_eq!(
            event_value(&e),
            json!({"data": {"m": "hi"}, "id": "1", "retry": 1.5})
        );
        assert_eq!(event_value(&SseEvent::default()), json!({}));
    }

    #[test]
    fn jq_keeps_scalars_as_text_and_stops_at_anything_else() {
        let inputs = vec![
            json!({"data": {"m": "a"}}).to_string(),
            json!({"data": {"m": 2}}).to_string(),
            json!({"data": {"m": 1.5}}).to_string(),
            json!({"data": {"m": true}}).to_string(),
            json!({"data": {}}).to_string(),
            json!({"data": {"m": [1]}}).to_string(),
            json!({"data": "text"}).to_string(),
        ];
        let got = jq_each(".data.m", inputs).unwrap();
        let text = |s: &str| vec![JqScalar::Text(s.into())];
        assert_eq!(got[0].outputs, text("a"));
        assert_eq!(got[1].outputs, text("2"));
        assert_eq!(got[2].outputs, text("1.5"));
        assert_eq!(got[3].outputs, text("true"));
        assert_eq!(got[4].outputs, vec![JqScalar::NotText("NULL")]);
        assert_eq!(got[5].outputs, vec![JqScalar::NotText("ARRAY")]);
        assert!(got[6].failed, "indexing a string is a jq error");
        assert!(got.iter().all(|e| e.refused.is_none()));
        assert!(
            jq_each("$.message", Vec::new()).is_err(),
            "jackson-jq's path form"
        );
        assert!(jq_each(".[", Vec::new()).is_err());
    }
}
