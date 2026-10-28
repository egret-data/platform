//! `io.kestra.plugin.core.http.Request` (package #4 Task C3, spec D5, D6, D9):
//! send one HTTP request through the egress guard and return the response.
//!
//! The request model, the D4 refusals, the status rule, masking and the
//! failure message are [`crate::http`]'s; this handler renders, sends and
//! shapes the outputs.
//!
//! # What it does, in order
//!
//! 1. Render the properties ONCE with `secret()` resolving
//!    ([`render_with_secrets`], D5), as the plugin tasks do. Every value it
//!    hands out is masked in every log line and in the failure message.
//! 2. Build the request ([`build_request`]), which runs the D4 refusals first
//!    (`options.proxy`, `options.ssl.insecureTrustAllCertificates`,
//!    `options.auth.type: DIGEST`, `encryptBody`, and a URI with userinfo), so
//!    a stored flow that slipped past publish fails here naming the property.
//!    A `kestra://` file part of a multipart `formData` is read from the run's
//!    readable files; a multipart body is capped at
//!    `workflow.storage.max_file_bytes` (1 MiB without `[storage]`).
//! 3. Send it with [`HttpEgress`]: every hop vetted and pinned, no proxy,
//!    redirects re-vetted, credentials dropped across origins.
//! 4. Apply the status rule ([`ensure_allowed`]: Kestra's, any code below 400
//!    passes unless `allowFailed` or `allowedResponseCodes` says otherwise).
//! 5. Read the body up to [`MAX_BODY_BYTES`] and decode it as Kestra does.
//!
//! Outputs: `uri` (the URI asked for, not where a redirect ended), `code`,
//! `headers` (`{name: [values]}`, as Kestra, `Set-Cookie` included; outputs
//! are never logged) and `body` (a string, never parsed as JSON; `null` when
//! the response has no entity: a HEAD, a 1xx, a 204 or a 304).
//!
//! # Where egret differs, on purpose
//!
//! - The body cap is 1 MiB (Kestra: 19 MiB), egret's render cap
//!   (`runners::env::MAX_OUTPUT_BYTES`): a larger body would break every later
//!   render that reads it. The message points at `Download`, as Kestra's does.
//! - The D4 refusals and the userinfo refusal (spec D4, controller decision
//!   C3).
//! - A failure message quotes at most 4 KiB of the body, masked.

pub mod download;
pub mod sse;

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use egret_core::{Error, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use unicode_general_category::{get_general_category, GeneralCategory};

use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use crate::http::{
    build_request, ensure_allowed, headers_output, log_request_lines, log_response_lines,
    read_capped, transport_message, BuiltRequest, FormFiles, HttpEgress, HttpSpec, MaskSet,
    ResolvedOptions,
};
use crate::logs::RunLog;
use crate::runners::secrets::{render_with_secrets, SecretSource};
use crate::runners::FileScope;
use crate::storage::FileStore;

/// `io.kestra.plugin.core.http.Request`.
pub const REQUEST: &str = "io.kestra.plugin.core.http.Request";
/// Kestra's alias for [`REQUEST`] (`@Plugin(aliases = …)`, `Request.java:333`).
pub const REQUEST_ALIAS: &str = "io.kestra.plugin.fs.http.Request";
/// `io.kestra.plugin.core.http.Download` (package #4 Task C4).
pub const DOWNLOAD: &str = "io.kestra.plugin.core.http.Download";
/// Kestra's alias for [`DOWNLOAD`].
pub const DOWNLOAD_ALIAS: &str = "io.kestra.plugin.fs.http.Download";
/// `io.kestra.plugin.core.http.SseRequest` (package #4 Task C4). No alias.
pub const SSE_REQUEST: &str = "io.kestra.plugin.core.http.SseRequest";

/// The task types on which `secret()` resolves besides the plugin tasks
/// (spec D5): the three http tasks under every spelling Kestra declares.
/// `spec::validate`'s `secret_outside_plugin_task` scan exempts their
/// properties.
pub const SECRET_TYPES: &[&str] = &[
    REQUEST,
    REQUEST_ALIAS,
    DOWNLOAD,
    DOWNLOAD_ALIAS,
    SSE_REQUEST,
];

/// Whether `secret()` resolves in the properties of a task of type `t`.
pub fn resolves_secrets(t: &str) -> bool {
    SECRET_TYPES.contains(&t)
}

/// The largest response body `Request` stores as its `body` output (spec D6).
pub const MAX_BODY_BYTES: u64 = 1 << 20;

/// The multipart body cap without `[storage]` (with it, the cap is
/// `workflow.storage.max_file_bytes`).
pub const MULTIPART_CAP_WITHOUT_STORAGE: u64 = 1 << 20;

/// The properties `Request` reads (`AbstractHttp.java`), for the save-time
/// key check (`spec::keys`). The handler itself reads them through
/// [`HttpSpec`]. `encryptBody` is not here: egret refuses it (D4), so it is
/// measured `refused`, never `honoured`.
#[derive(Debug, Default, Deserialize)]
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
}

/// Register the three http tasks ([`Request`], [`download::Download`],
/// [`sse::SseRequest`]): `runtime::handlers_for` calls this for every
/// deployment, with or without a container runner. `cfg` gives the caps
/// (`workflow.http.max_download_bytes`, `max_sse_events`).
pub fn register(
    handlers: &mut Handlers,
    cfg: &egret_config::WorkflowHttpConfig,
    egress: HttpEgress,
    secrets: Arc<dyn SecretSource>,
    files: Option<Arc<FileStore>>,
) {
    handlers.add_publishable(Arc::new(Request::new(
        egress.clone(),
        secrets.clone(),
        files.clone(),
    )));
    handlers.add_publishable(Arc::new(download::Download::new(
        egress.clone(),
        secrets.clone(),
        files.clone(),
        cfg.max_download_bytes,
    )));
    handlers.add_publishable(Arc::new(sse::SseRequest::new(
        egress,
        secrets,
        files,
        cfg.max_sse_events,
    )));
}

/// `io.kestra.plugin.core.http.Request`.
pub struct Request {
    egress: HttpEgress,
    secrets: Arc<dyn SecretSource>,
    files: Option<Arc<FileStore>>,
}

impl Request {
    pub fn new(
        egress: HttpEgress,
        secrets: Arc<dyn SecretSource>,
        files: Option<Arc<FileStore>>,
    ) -> Self {
        Self {
            egress,
            secrets,
            files,
        }
    }

    /// The multipart body cap: `workflow.storage.max_file_bytes`, or 1 MiB
    /// without `[storage]`.
    fn multipart_cap(&self) -> u64 {
        self.files
            .as_ref()
            .map_or(MULTIPART_CAP_WITHOUT_STORAGE, |fs| fs.caps.max_file_bytes)
    }
}

#[async_trait::async_trait]
impl TaskHandler for Request {
    fn type_id(&self) -> &'static str {
        REQUEST
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        // 1-2. Render (secret() resolving, D5) and build (D4 refusals first).
        let p = match prepare(
            REQUEST,
            task,
            self.secrets.as_ref(),
            self.files.as_ref(),
            Some(self.multipart_cap()),
        )
        .await?
        {
            Ok(p) => p,
            Err(failed) => return Ok(failed),
        };
        let Prepared {
            opts,
            built,
            masks,
            log,
            ..
        } = p;
        let failed = |masks: &MaskSet, m: &str| Ok(Outcome::failed_with(masks.mask(m)));

        // 3. Send.
        let head = built.request.method == reqwest::Method::HEAD;
        let res = match self.egress.client(&opts, None).send(built.request).await {
            Ok(r) => r,
            Err(e) => return failed(&masks, &transport_message(&e, &masks)),
        };
        let status = res.status();
        let headers = res.headers().clone();

        // 4. The status rule.
        let res = match ensure_allowed(res, &opts, &masks).await {
            Ok(r) => r,
            Err(m) => return failed(&masks, &m),
        };

        // 5. The body.
        let bytes = match read_capped(res, MAX_BODY_BYTES).await {
            Ok(b) => b,
            Err(e) => return failed(&masks, &e.to_string()),
        };
        for (level, line) in log_response_lines(status, &headers, Some(&bytes), &opts.logs, &masks)
        {
            log.line(level, &line).await;
        }
        let body = if has_no_entity(head, status) {
            Value::Null
        } else {
            match decode_body(&bytes) {
                Ok(s) => Value::String(s),
                Err(m) => return failed(&masks, &m),
            }
        };
        Ok(Outcome::succeeded_with(json!({
            "uri": built.uri,
            "code": status.as_u16(),
            "headers": headers_output(&headers),
            "body": body,
        })))
    }

    /// Nothing to cancel: the request is dropped with the future.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// What the three http tasks share before sending: the rendered
/// properties, the converted options, the built request, the values to mask
/// and the masked log.
pub(super) struct Prepared {
    /// The rendered properties, for a task's own keys (`saveAs`, ...).
    pub rendered: serde_json::Map<String, Value>,
    pub opts: ResolvedOptions,
    pub built: BuiltRequest,
    pub masks: MaskSet,
    pub log: RunLog,
}

/// Steps 1 and 2 of every http task: render once with `secret()` resolving
/// (D5), read the shared model, build the request (the D4 refusals first),
/// cap a multipart body at `multipart_cap` (when `Some`), and log the
/// `options.logs` request lines.
///
/// `Ok(Err(outcome))` is the task's failure, masked; `Err` a worker failure
/// (a store outage met while rendering or reading a file part).
pub(super) async fn prepare(
    type_id: &str,
    task: &TaskInput,
    secrets: &dyn SecretSource,
    files: Option<&Arc<FileStore>>,
    multipart_cap: Option<u64>,
) -> Result<std::result::Result<Prepared, Outcome>> {
    let serde_json::Value::Object(props) = &task.spec else {
        return Ok(Err(Outcome::failed_with(format!(
            "{type_id}: the task's properties are not a map"
        ))));
    };

    // 1. Render once, resolving secret() (D5). The error is masked already.
    let (rendered, resolved) =
        match render_with_secrets(props, &task.render, task.workspace_id, secrets).await {
            Ok(r) => r,
            Err(message) => {
                super::storage::outage(type_id, task)?;
                return Ok(Err(Outcome::failed_with(message)));
            }
        };
    super::storage::outage(type_id, task)?;
    let mut masks = MaskSet::new(resolved.values.iter());
    let failed = |masks: &MaskSet, m: &str| Ok(Err(Outcome::failed_with(masks.mask(m))));

    let mut spec = match HttpSpec::from_value(&Value::Object(rendered.clone())) {
        Ok(s) => s,
        Err(m) => return failed(&masks, &m),
    };
    // `encryptBody` is Request's property only. On Download and SseRequest a
    // stored flow that slipped past publish (where it is `unknown_property`)
    // fails naming the task type, not with the D4 text about Request's `body`
    // output, which [`build_request`] would give (package #4 Task C7, the C6
    // review's concern). `false` is harmless and dropped.
    if type_id != REQUEST {
        if !crate::http::spec::absent_or_false(spec.encrypt_body.as_ref()) {
            return failed(
                &masks,
                &format!(
                    "encryptBody: not a property of {type_id} (Kestra declares it on \
                     io.kestra.plugin.core.http.Request only). Remove it"
                ),
            );
        }
        spec.encrypt_body = None;
    }
    let opts = match spec.options() {
        Ok(o) => o,
        Err(m) => return failed(&masks, &m),
    };

    // 2. Build (the D4 refusals run first).
    let form_files = match (files, task.render.files()) {
        (Some(fs), Some(scope)) => Some(StoreFiles {
            fs: fs.clone(),
            scope: scope.clone(),
            workspace: task.workspace_id,
            cap: fs.caps.max_file_bytes,
            read: Mutex::new(0),
            outage: Mutex::new(None),
        }),
        _ => None,
    };
    let built = build_request(&spec, form_files.as_ref().map(|f| f as &dyn FormFiles)).await;
    if let Some(why) = form_files.as_ref().and_then(StoreFiles::outage) {
        // A store outage is egret's, not the author's (W9b spec §11).
        return Err(Error::Internal(anyhow::anyhow!("{type_id}: {why}")));
    }
    let built = match built {
        Ok(b) => b,
        Err(m) => return failed(&masks, &m),
    };
    masks.extend(built.credentials.iter());
    if let Some(cap) = multipart_cap {
        if let Some(message) = multipart_over_cap(&built.request, cap) {
            return failed(&masks, &message);
        }
    }
    let log = task.log.masked(masks.values());
    for (level, line) in log_request_lines(
        &built.request.method,
        &built.uri,
        &built.request.headers,
        built.request.body.as_deref(),
        &opts.logs,
        &masks,
    ) {
        log.line(level, &line).await;
    }
    Ok(Ok(Prepared {
        rendered,
        opts,
        built,
        masks,
        log,
    }))
}

/// A response Apache's client gives no entity, so Kestra's `body` is `null`:
/// a HEAD's, a 1xx, a 204 and a 304.
fn has_no_entity(head: bool, status: reqwest::StatusCode) -> bool {
    head || status.is_informational()
        || status == reqwest::StatusCode::NO_CONTENT
        || status == reqwest::StatusCode::NOT_MODIFIED
}

/// `Request.java:356-368`: the body as UTF-8 (a malformed sequence becomes
/// U+FFFD, as Java's decoder does), then refused when a UTF-16 unit is not
/// `Character.isDefined` -- an unassigned BMP code point or a noncharacter.
/// Java walks `chars()`, UTF-16 units, so a supplementary code point is two
/// surrogates, which are defined: only the BMP is ever refused.
pub(crate) fn decode_body(bytes: &[u8]) -> std::result::Result<String, String> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if let Some(c) = text
        .chars()
        .find(|c| (*c as u32) <= 0xFFFF && get_general_category(*c) == GeneralCategory::Unassigned)
    {
        return Err(format!(
            "Illegal unicode code point in request body: {}, the Request task only support \
             valid Unicode strings as body.\nYou can try using the Download task instead.",
            c as u32
        ));
    }
    Ok(text)
}

/// The refusal of a multipart body over `cap`, or `None`.
fn multipart_over_cap(req: &egret_egress::RequestSpec, cap: u64) -> Option<String> {
    let multipart = req
        .headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    let len = req.body.as_ref().map_or(0, |b| b.len() as u64);
    (multipart && len > cap).then(|| {
        format!(
            "formData: the multipart body is {len} bytes, over the {cap}-byte cap \
             (workflow.storage.max_file_bytes, or 1 MiB without [storage])"
        )
    })
}

/// `formData` file parts, read from the run's readable files: the same
/// readability decision as `read()` (the run's snapshot), and a cap on what
/// all parts together read.
struct StoreFiles {
    fs: Arc<FileStore>,
    scope: FileScope,
    workspace: uuid::Uuid,
    cap: u64,
    read: Mutex<u64>,
    outage: Mutex<Option<String>>,
}

impl StoreFiles {
    fn outage(&self) -> Option<String> {
        self.outage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[async_trait::async_trait]
impl FormFiles for StoreFiles {
    async fn read(&self, uri: &str) -> std::result::Result<Bytes, String> {
        let meta = self
            .scope
            .0
            .lookup(uri)
            .map_err(|why| why.message().to_string())?;
        {
            let mut read = self.read.lock().unwrap_or_else(|p| p.into_inner());
            if *read + meta.size > self.cap {
                return Err(format!(
                    "formData: {uri} would take the multipart body over the {}-byte cap \
                     (workflow.storage.max_file_bytes)",
                    self.cap
                ));
            }
            *read += meta.size;
        }
        let object = self.fs.object_uri(self.workspace, uri)?;
        match self.fs.store.get(&object).await {
            Ok(bytes) => Ok(Bytes::from(bytes)),
            Err(egret_storage::StorageError::NotFound(_)) => Err(format!(
                "{uri} is indexed but its object is missing from storage (it was pruned, or \
                 never finished uploading)"
            )),
            Err(e) => {
                let why = format!("{uri} could not be read: internal storage is unavailable: {e}");
                *self.outage.lock().unwrap_or_else(|p| p.into_inner()) = Some(why.clone());
                Err(why)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kestra's check walks UTF-16 units: an unassigned BMP code point and a
    /// noncharacter fail with its message; an unassigned supplementary one
    /// (two defined surrogates) and a malformed byte (U+FFFD) pass.
    #[test]
    fn the_body_is_refused_on_an_undefined_bmp_code_point_only() {
        assert_eq!(decode_body("héllo ✓".as_bytes()).unwrap(), "héllo ✓");
        // U+0378 is unassigned.
        let err = decode_body("a\u{0378}b".as_bytes()).unwrap_err();
        assert_eq!(
            err,
            "Illegal unicode code point in request body: 888, the Request task only support \
             valid Unicode strings as body.\nYou can try using the Download task instead."
        );
        // U+FFFF is a noncharacter: Java's isDefined is false.
        assert!(decode_body("\u{FFFF}".as_bytes())
            .unwrap_err()
            .contains(": 65535,"));
        // U+E0080 is unassigned but supplementary: Java sees two surrogates.
        assert!(decode_body("\u{E0080}".as_bytes()).is_ok());
        // A malformed byte becomes U+FFFD (defined).
        assert_eq!(decode_body(&[b'a', 0xFF, b'b']).unwrap(), "a\u{FFFD}b");
    }

    #[test]
    fn a_response_without_an_entity_has_a_null_body() {
        use reqwest::StatusCode as S;
        assert!(has_no_entity(true, S::OK));
        for s in [S::CONTINUE, S::NO_CONTENT, S::NOT_MODIFIED] {
            assert!(has_no_entity(false, s), "{s}");
        }
        for s in [S::OK, S::CREATED, S::NOT_FOUND, S::FOUND] {
            assert!(!has_no_entity(false, s), "{s}");
        }
    }

    #[test]
    fn only_a_multipart_body_is_capped() {
        let mut req = egret_egress::RequestSpec::new(reqwest::Method::POST, "http://a.test/");
        req.body = Some(Bytes::from(vec![b'x'; 11]));
        assert_eq!(multipart_over_cap(&req, 10), None, "no content type");
        req.headers.insert(
            reqwest::header::CONTENT_TYPE,
            "multipart/form-data; boundary=b".parse().unwrap(),
        );
        let m = multipart_over_cap(&req, 10).unwrap();
        assert!(
            m.contains("11 bytes") && m.contains("workflow.storage.max_file_bytes"),
            "{m}"
        );
        assert_eq!(multipart_over_cap(&req, 11), None, "at the cap");
    }

    #[test]
    fn secret_resolves_on_the_three_http_tasks_under_every_spelling() {
        for t in [
            "io.kestra.plugin.core.http.Request",
            "io.kestra.plugin.fs.http.Request",
            "io.kestra.plugin.core.http.Download",
            "io.kestra.plugin.fs.http.Download",
            "io.kestra.plugin.core.http.SseRequest",
        ] {
            assert!(resolves_secrets(t), "{t}");
        }
        for t in [
            "io.kestra.plugin.core.log.Log",
            "core.http",
            "io.kestra.plugin.core.http.Trigger",
        ] {
            assert!(!resolves_secrets(t), "{t}");
        }
    }
}
