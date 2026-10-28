//! `io.kestra.plugin.core.http.Download` (alias
//! `io.kestra.plugin.fs.http.Download`; package #4 Task C4, spec D7): send one
//! request through the egress guard and stream the response body into the
//! run's internal storage.
//!
//! The request side is [`Request`](super::Request)'s ([`super::prepare`]:
//! `secret()` resolving, the D4 refusals, `options.logs`), and so is the
//! status rule. Then, as `Download.java:75-156`:
//!
//! - the body is streamed to the store (`put_stream`), never held whole;
//! - unless the response is content-encoded, a `Content-Length` that does not
//!   match the bytes stored fails with Kestra's `Invalid size, got <n>,
//!   expected <len>`;
//! - an empty body fails with `No response from server` when
//!   `failOnEmptyResponse` (default true) and not `options.allowFailed`; else
//!   it warns `File '<uri>' is empty`;
//! - the file name is `saveAs`, else the `Content-Disposition` file name
//!   (spaces as `+`), else a name ending in the URI's extension.
//!
//! Outputs: `uri` (the `kestra:///` file), `code`, `length` (bytes stored),
//! `headers`. The file reaches the index through [`Outcome::files`], in the
//! commit's transaction, as `egret.sql`'s `fetch: STORE` does.
//!
//! # Where egret differs, on purpose (spec D7)
//!
//! - **The name is stable per run**, so a re-claim overwrites its own object
//!   instead of leaving a second one: Kestra's fallback is a random temp-file
//!   name with the URI's extension; egret's is `download` + that extension.
//!   `saveAs` and the `Content-Disposition` name are cut to their last path
//!   segment (Kestra keeps a `saveAs` with `/` as a sub-path) and the
//!   characters no `kestra:///` URI may hold are percent-encoded, as Kestra's
//!   quoting `URI` constructor does for `[`, `]` and friends.
//! - **A cap**: the smallest of `workflow.http.max_download_bytes` (default
//!   1 GiB), `workflow.storage.max_file_bytes` and `max_task_bytes`, checked
//!   on `Content-Length` before reading and on the bytes as they stream.
//!   Kestra has none.
//! - **`filename*`** (RFC 5987) is percent-decoded after its `charset'lang'`
//!   prefix; Kestra's substring arithmetic drops the name's first and last
//!   characters there.
//! - Without `[storage]` the task fails naming the missing configuration,
//!   before anything is sent.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use egret_core::{Error, Result};
use serde_json::{json, Value};

use super::super::super::{Outcome, TaskHandler, TaskInput};
use super::{prepare, Prepared, DOWNLOAD};
use crate::http::spec::flag;
use crate::http::{
    ensure_allowed, headers_output, log_response_lines, transport_message, HttpEgress,
};
use crate::logs::LogLevel;
use crate::runners::secrets::SecretSource;
use crate::storage::{FileStore, StoredFile};

/// The file name stem when neither `saveAs` nor `Content-Disposition` names
/// the file (Kestra: a random temp-file name).
pub const FALLBACK_STEM: &str = "download";

/// The properties `Download` reads besides the shared ones, for the
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
    fail_on_empty_response: Option<Value>,
    save_as: Option<Value>,
}

/// `io.kestra.plugin.core.http.Download`.
pub struct Download {
    egress: HttpEgress,
    secrets: Arc<dyn SecretSource>,
    files: Option<Arc<FileStore>>,
    max_download_bytes: u64,
}

impl Download {
    pub fn new(
        egress: HttpEgress,
        secrets: Arc<dyn SecretSource>,
        files: Option<Arc<FileStore>>,
        max_download_bytes: u64,
    ) -> Self {
        Self {
            egress,
            secrets,
            files,
            max_download_bytes,
        }
    }
}

/// The cap one download is held to, and the key that sets it.
fn cap_of(max_download_bytes: u64, caps: crate::storage::Caps) -> (u64, &'static str) {
    [
        (max_download_bytes, "workflow.http.max_download_bytes"),
        (caps.max_file_bytes, "workflow.storage.max_file_bytes"),
        (caps.max_task_bytes, "workflow.storage.max_task_bytes"),
    ]
    .into_iter()
    .min_by_key(|(n, _)| *n)
    .expect("three caps")
}

fn over_cap(length: Option<u64>, cap: u64, key: &str) -> String {
    let n = match length {
        Some(l) => format!("{l} bytes"),
        None => format!("more than {cap} bytes"),
    };
    format!("The response body is too large to download ({n} > max {cap} bytes, {key}).")
}

#[async_trait::async_trait]
impl TaskHandler for Download {
    fn type_id(&self) -> &'static str {
        DOWNLOAD
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        // Before anything is sent: the body has nowhere else to go.
        let Some(fs) = self.files.clone() else {
            return Ok(Outcome::failed_with(format!(
                "{DOWNLOAD}: the file needs internal storage, and egret has none configured \
                 (the `[storage]` section of its configuration)"
            )));
        };
        let prefix = match super::super::task_storage_prefix(task, "`http.Download`") {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(format!("{DOWNLOAD}: {why}"))),
        };
        let multipart_cap = Some(super::multipart_cap(self.files.as_deref()));
        let Prepared {
            rendered,
            opts,
            built,
            masks,
            log,
        } = match prepare(
            DOWNLOAD,
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

        // The task's own properties, rendered with the rest.
        let fail_on_empty = match flag(rendered.get("failOnEmptyResponse"), "failOnEmptyResponse") {
            Ok(f) => f.unwrap_or(true),
            Err(m) => return failed(&m),
        };
        let save_as = match rendered.get("saveAs") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => match one_segment(s) {
                Some(name) => Some(name),
                None => {
                    return failed(&format!(
                        "saveAs: {s:?} is not a file name (it is empty, `.` or `..` once cut \
                         to its last path segment)"
                    ))
                }
            },
            Some(other) => {
                return failed(&format!(
                    "saveAs: expected a string, got {}",
                    crate::http::spec::kind(other)
                ))
            }
        };
        let (cap, cap_key) = cap_of(self.max_download_bytes, fs.caps);

        // Send, and the status rule.
        let res = match self.egress.client(&opts, None).send(built.request).await {
            Ok(r) => r,
            Err(e) => return failed(&transport_message(&e, &masks)),
        };
        let status = res.status();
        let headers = res.headers().clone();
        let res = match ensure_allowed(res, &opts, &masks).await {
            Ok(r) => r,
            Err(m) => return failed(&m),
        };
        for (level, line) in log_response_lines(status, &headers, None, &opts.logs, &masks) {
            log.line(level, &line).await;
        }
        let content_length = res.content_length();
        if content_length.is_some_and(|l| l > cap) {
            return failed(&over_cap(content_length, cap, cap_key));
        }

        // Where it goes.
        let name = save_as.unwrap_or_else(|| {
            headers
                .get(reqwest::header::CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .and_then(filename_from_header)
                .and_then(|n| one_segment(&n))
                .unwrap_or_else(|| fallback_name(&built.uri))
        });
        let file = match crate::storage::uri::verbatim_file_uri(&prefix, &name) {
            Ok(f) => f,
            Err(why) => return failed(&why),
        };
        let object = match fs.object_uri(task.workspace_id, &file) {
            Ok(o) => o,
            Err(why) => return failed(&why),
        };

        // Stream it. A crossed cap or a broken connection is the stream's
        // error, so the upload fails (and aborts) instead of storing a
        // truncated object; `why` says it was not the store.
        let why: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let read = Arc::new(AtomicU64::new(0));
        let body = futures_util::stream::unfold(
            (res, false, why.clone(), read.clone()),
            move |(mut res, done, why, read)| async move {
                if done {
                    return None;
                }
                let stop = |m: String| {
                    *why.lock().unwrap_or_else(|p| p.into_inner()) = Some(m.clone());
                    std::io::Error::other(m)
                };
                match res.chunk().await {
                    Ok(Some(c)) => {
                        let n = read.fetch_add(c.len() as u64, Ordering::SeqCst) + c.len() as u64;
                        if n > cap {
                            let e = stop(over_cap(content_length, cap, cap_key));
                            Some((Err(e), (res, true, why, read)))
                        } else {
                            Some((Ok(c), (res, false, why, read)))
                        }
                    }
                    Ok(None) => None,
                    Err(e) => {
                        let m = if e.is_timeout() {
                            "request timed out".to_string()
                        } else {
                            e.without_url().to_string()
                        };
                        let e = stop(m);
                        Some((Err(e), (res, true, why, read)))
                    }
                }
            },
        );
        let mut reader = tokio_util::io::StreamReader::new(Box::pin(body));
        let put = fs
            .store
            .put_stream(&object, &mut reader, content_length)
            .await;
        let author = why.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(m) = author {
            return failed(&m);
        }
        let outage = |what: String| -> Result<Outcome> {
            Err(Error::Internal(anyhow::anyhow!("{DOWNLOAD}: {what}")))
        };
        let size = match put {
            Ok(n) => n,
            Err(e) => {
                return outage(format!(
                    "{file} could not be stored: internal storage is unavailable: {e}"
                ))
            }
        };
        if size != read.load(Ordering::SeqCst) {
            return outage(format!(
                "{file}: {} bytes were read and {size} stored",
                read.load(Ordering::SeqCst)
            ));
        }
        // `head` for the size the index row will hold, as `fetch: STORE` does.
        match fs.store.head(&object).await {
            Ok(Some(held)) if held == size => {}
            Ok(Some(held)) => {
                return outage(format!(
                    "{file} holds {held} bytes in internal storage after {size} were written"
                ))
            }
            Ok(None) => {
                return outage(format!(
                    "{file} was written ({size} bytes) but internal storage does not have it"
                ))
            }
            Err(e) => {
                return outage(format!(
                    "{file} could not be checked: internal storage is unavailable: {e}"
                ))
            }
        }

        // Kestra's two checks on what arrived. A failed one leaves an
        // unindexed object, removed here (else the pruner's).
        let encoded = headers
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|e| !e.trim().is_empty() && !e.eq_ignore_ascii_case("identity"));
        let mut refused = None;
        if let (Some(len), false) = (content_length, encoded) {
            if len != size {
                refused = Some(format!("Invalid size, got {size}, expected {len}"));
            }
        }
        if refused.is_none() && size == 0 {
            if fail_on_empty && !opts.allow_failed {
                refused = Some("No response from server".to_string());
            } else {
                log.line(LogLevel::Warn, &format!("File '{}' is empty", built.uri))
                    .await;
            }
        }
        if let Some(m) = refused {
            let _ = fs.store.delete(&object).await;
            return failed(&m);
        }
        log.line(
            LogLevel::Debug,
            &format!("File '{}' downloaded with size '{size}'", built.uri),
        )
        .await;

        Ok(Outcome {
            files: vec![StoredFile {
                uri: file.clone(),
                size,
            }],
            ..Outcome::succeeded_with(json!({
                "uri": file,
                "code": status.as_u16(),
                "length": size,
                "headers": headers_output(&headers),
            }))
        })
    }

    /// Nothing to cancel: the request and the upload are dropped with the
    /// future (an S3 multipart upload aborts on drop).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// `Download.filenameFromHeader`: the last `filename` or `filename*` part
/// wins; a quoted value loses its quotes. `filename*` (RFC 5987
/// `charset'lang'value`) is percent-decoded as UTF-8.
pub(crate) fn filename_from_header(cd: &str) -> Option<String> {
    let mut name: Option<String> = None;
    for part in cd.split(';') {
        let p = part.trim();
        if p.starts_with("filename*") {
            let Some((_, v)) = p.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches('"');
            let encoded = v.rsplit_once('\'').map_or(v, |(_, n)| n);
            name = Some(percent_decode_lossy(encoded));
        } else if p.starts_with("filename") {
            if let Some(i) = p.rfind('=') {
                name = Some(p[i + 1..].to_string());
            }
        }
    }
    let name = name?;
    let name = match name.strip_prefix('"') {
        Some(rest) => rest.strip_suffix('"').unwrap_or(rest).to_string(),
        None => name,
    };
    (!name.is_empty()).then_some(name)
}

/// One path segment a `kestra:///` URI can carry: the part after the last
/// `/` or `\`, spaces as `+` (Kestra's `Content-Disposition` rule), and the
/// characters Java's quoting `URI` constructor escapes in a path
/// percent-encoded. `None` for an empty, `.` or `..` result.
pub(crate) fn one_segment(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = String::with_capacity(last.len());
    for c in last.chars() {
        match c {
            ' ' => out.push('+'),
            '"' | '<' | '>' | '[' | ']' | '^' | '`' | '{' | '|' | '}' | '?' | '#' => {
                out.push_str(&format!("%{:02X}", c as u32))
            }
            c if c.is_control() => {
                let mut b = [0u8; 4];
                for byte in c.encode_utf8(&mut b).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
            c => out.push(c),
        }
    }
    (!out.is_empty() && out != "." && out != "..").then_some(out)
}

/// `download` plus the URI path's extension (`Download.filenameFromURI`: the
/// last segment's text from its last `.`).
pub(crate) fn fallback_name(uri: &str) -> String {
    let path = url::Url::parse(uri)
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rfind('.') {
        Some(i) => match one_segment(&format!("{FALLBACK_STEM}{}", &last[i..])) {
            Some(n) => n,
            None => FALLBACK_STEM.to_string(),
        },
        None => FALLBACK_STEM.to_string(),
    }
}

fn percent_decode_lossy(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_names_follow_kestra() {
        for (cd, want) in [
            (
                "attachment; filename=\"filename.jpg\"",
                Some("filename.jpg"),
            ),
            ("attachment; filename=plain.csv", Some("plain.csv")),
            (
                "attachment; filename=\"/secure-path/filename.jpg\"",
                Some("/secure-path/filename.jpg"),
            ),
            (
                "attachment; filename=\"a.txt\"; filename*=UTF-8''r%C3%A9sum%C3%A9.txt",
                Some("résumé.txt"),
            ),
            ("attachment", None),
            ("attachment; filename=\"\"", None),
        ] {
            assert_eq!(filename_from_header(cd).as_deref(), want, "{cd}");
        }
    }

    #[test]
    fn a_name_is_cut_to_one_segment_a_uri_can_carry() {
        for (name, want) in [
            ("/secure-path/filename.jpg", Some("filename.jpg")),
            ("/secure-path/filename..jpg", Some("filename..jpg")),
            ("file.with spaces.txt", Some("file.with+spaces.txt")),
            (
                "file.with[]brackets.txt",
                Some("file.with%5B%5Dbrackets.txt"),
            ),
            ("a\\b?c#d.txt", Some("b%3Fc%23d.txt")),
            ("x/..", None),
            ("x/", None),
            (".", None),
        ] {
            assert_eq!(one_segment(name).as_deref(), want, "{name}");
        }
    }

    #[test]
    fn the_fallback_keeps_only_the_extension_and_is_stable() {
        assert_eq!(
            fallback_name("https://h.test/datasets/raw/main/csv/orders.csv"),
            "download.csv"
        );
        assert_eq!(fallback_name("https://h.test/a.b/file?x=1.2"), "download");
        assert_eq!(fallback_name("https://h.test/"), "download");
        assert_eq!(
            fallback_name("https://h.test/x.tar.gz"),
            fallback_name("https://h.test/x.tar.gz"),
            "a re-claim computes the same name"
        );
        assert_eq!(fallback_name("https://h.test/x.tar.gz"), "download.gz");
    }

    #[test]
    fn the_smallest_cap_wins_and_is_named() {
        let mut caps = crate::storage::Caps {
            max_file_bytes: 100,
            max_task_bytes: 200,
            ..crate::storage::Caps::default()
        };
        assert_eq!(cap_of(50, caps), (50, "workflow.http.max_download_bytes"));
        assert_eq!(cap_of(150, caps), (100, "workflow.storage.max_file_bytes"));
        caps.max_task_bytes = 80;
        assert_eq!(cap_of(150, caps), (80, "workflow.storage.max_task_bytes"));
    }
}
