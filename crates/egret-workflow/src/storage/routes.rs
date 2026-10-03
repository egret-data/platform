//! The execution file API (W9a spec §8): download, metas and preview of a
//! file an execution wrote or was given.
//!
//! ```text
//! GET /api/ws/{workspace}/workflows/executions/{id}/file?path=kestra:///…
//! GET /api/ws/{workspace}/workflows/executions/{id}/file/metas?path=…
//! GET /api/ws/{workspace}/workflows/executions/{id}/file/preview?path=…&maxRows=100
//! ```
//!
//! # Which files `{id}` serves
//!
//! Authorisation is `GET …/executions/{id}/logs`'s: `WsContext`, then
//! `workflow:read`, then the execution's existence in `{workspace}` (a foreign
//! id is a 404, never a 403). The file is then decided by
//! [`super::resolve_readable_many`] -- **the one readability function** --
//! with the readable set narrowed to `{id}`'s **loop family**
//! ([`super::loop_family`]: its loop root and every iteration under it).
//! Every task of a family reads every file of it, so any of its ids serves
//! them (Kestra reaches the same files by a 301 to the owning iteration).
//! Trigger and replay links are readable while a flow renders but are NOT
//! served here: a viewer asks those executions by their own id. Anything
//! `resolve_readable_many` does not answer `Ok` is one 404 with one message,
//! whatever the reason (malformed, another execution, missing): no redirect,
//! and nothing that tells whether a file exists outside the family.
//!
//! # Streaming
//!
//! A download is the object's stream, chunk by chunk; a preview reads at most
//! `workflow.storage.max_read_bytes` and stops as soon as the page is full. No
//! file is ever held whole.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::Json;
use egret_core::{Error, WorkspacePath};
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncRead, ReadBuf};
use uuid::Uuid;

use super::{ion, loop_family, resolve_readable_many, FileStore, IndexedFile};
use crate::routes::{WorkflowState, WF_READ};

/// `maxRows` when the request gives none.
pub const DEFAULT_PREVIEW_ROWS: u32 = 100;
/// The most rows one preview answers; a larger `maxRows` is clamped to it.
pub const MAX_PREVIEW_ROWS: u32 = 5000;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FileQuery {
    /// The file's `kestra:///…` URI, as the execution's outputs or inputs name it.
    pub path: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PreviewQuery {
    /// The file's `kestra:///…` URI, as the execution's outputs or inputs name it.
    pub path: String,
    /// At most this many rows (default 100; anything above 5000 is 5000, below 1 is 1).
    #[serde(rename = "maxRows")]
    pub max_rows: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FileMetasDto {
    /// The file's size in bytes, as indexed when it was stored.
    pub size: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FilePreviewDto {
    /// `false` (with no rows) for a file that is neither Ion nor UTF-8 text,
    /// Ion that does not parse, or Ion nested deeper than 100 levels.
    pub previewable: bool,
    /// `.ion`: one JSON value per top-level Ion value (a struct's null fields
    /// omitted, a timestamp as its Ion text, a decimal as a number). Anything
    /// else: one string per line.
    #[schema(value_type = Vec<serde_json::Value>)]
    pub rows: Vec<serde_json::Value>,
    /// More rows follow than `maxRows`, or the preview's byte budget
    /// (`workflow.storage.max_read_bytes`) ended the read.
    pub truncated: bool,
}

fn core_refusal(e: Error) -> Response {
    e.into_response()
}

fn unavailable(message: String) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(egret_core::http::ErrorBody {
            error: message,
            code: None,
        }),
    )
        .into_response()
}

/// `path` as a file of `{id}`'s loop family in `workspace`, or the one 404.
async fn family_file(
    st: &WorkflowState,
    workspace: Uuid,
    id: Uuid,
    path: &str,
) -> egret_core::Result<IndexedFile> {
    // Existence and ownership first: a foreign id is a 404 before anything
    // else is looked at (`log_status` is `ExecutionRepo::get`'s contract on
    // one row, without the source and task runs).
    st.executions.log_status(workspace, id).await?;
    let mut conn = st
        .db
        .pool
        .acquire()
        .await
        .map_err(|e| Error::Internal(anyhow::Error::new(e)))?;
    let family = loop_family(&mut conn, workspace, id).await?;
    let path = path.to_string();
    let mut answers =
        resolve_readable_many(&mut conn, workspace, &family, std::slice::from_ref(&path)).await?;
    match answers.remove(&path) {
        Some(Ok(file)) => Ok(file),
        _ => Err(Error::NotFound(format!(
            "{path}: no such file of execution {id}"
        ))),
    }
}

/// The answer when egret has no store installed (`storage::current()` is `None`).
fn no_store() -> Response {
    unavailable(
        "egret has no internal storage configured (the `[storage]` section of its \
         configuration), so no execution file can be read"
            .into(),
    )
}

/// The object's stream. A missing object behind an index row is a 404 (the
/// file is gone, e.g. pruned); any other store failure is a 503 naming the
/// `kestra://` URI and never the bucket, key or the store's own message.
async fn open(
    fs: &FileStore,
    workspace: Uuid,
    file: &IndexedFile,
) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, Response> {
    let object = fs
        .object_uri(workspace, &file.uri)
        .map_err(|e| core_refusal(Error::Internal(anyhow::anyhow!(e))))?;
    match fs.store.get_stream(&object).await {
        Ok(r) => Ok(r),
        Err(egret_storage::StorageError::NotFound(_)) => {
            tracing::warn!(uri = %file.uri, "an indexed execution file has no object");
            Err(core_refusal(Error::NotFound(format!(
                "{}: the file is indexed but its contents are gone",
                file.uri
            ))))
        }
        Err(e) => {
            tracing::error!(uri = %file.uri, error = %e, "internal storage failed to open an execution file");
            Err(unavailable(format!(
                "{}: internal storage is unavailable; retry later",
                file.uri
            )))
        }
    }
}

/// The last segment of the URI's path: the name a browser saves it under.
fn file_name(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

/// `attachment; filename="<ascii>"; filename*=utf-8''<pct-encoded>` (RFC 6266,
/// Kestra's shape). The quoted name keeps `[A-Za-z0-9._-]` and makes every
/// other character `_`, so it can hold no quote, `;`, separator or line
/// break; the extended name percent-encodes every byte outside RFC 8187's
/// `attr-char`.
pub(crate) fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ascii = if ascii.is_empty() {
        "file".to_string()
    } else {
        ascii
    };
    let mut ext = String::new();
    for b in name.bytes() {
        let attr_char = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if attr_char {
            ext.push(b as char);
        } else {
            ext.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=utf-8''{ext}")
}

/// The stream's io errors name the `kestra://` URI (the store's own error
/// does not) and are logged: the status line has already gone, so the client
/// only sees the body end early.
struct NamedErrors<R> {
    inner: R,
    uri: Arc<str>,
}

impl<R: AsyncRead + Unpin> AsyncRead for NamedErrors<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Err(e)) => {
                tracing::error!(uri = %self.uri, error = %e, "an execution file download failed mid-body");
                Poll::Ready(Err(std::io::Error::new(
                    e.kind(),
                    format!("{}: reading from internal storage failed: {e}", self.uri),
                )))
            }
            other => other,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}/file",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id: any id of the file's loop family (its loop root or any iteration under it)"),
        FileQuery,
    ),
    responses(
        (status = 200, description = "The file's bytes, streamed. `Content-Type: application/octet-stream`, `Content-Disposition: attachment` with the file's name (sanitised, and percent-encoded in `filename*`), `X-Content-Type-Options: nosniff`.", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, or `path` is not a file of its loop family: malformed, another execution's (a trigger's or a replay source's too -- ask that execution by its own id), never stored, or its contents are gone. **One answer for all of them**, and never a redirect.", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is unavailable or not configured; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn download_execution_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<FileQuery>,
) -> Result<Response, Response> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ)
        .await
        .map_err(core_refusal)?;
    let workspace = ctx.workspace_id.0;
    let file = family_file(&st, workspace, id, &q.path)
        .await
        .map_err(core_refusal)?;
    let fs = super::current().ok_or_else(no_store)?;
    let reader = open(&fs, workspace, &file).await?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(NamedErrors {
        inner: reader,
        uri: Arc::from(file.uri.as_str()),
    }));
    let disposition = HeaderValue::from_str(&content_disposition(file_name(&file.uri)))
        .map_err(|e| core_refusal(Error::Internal(anyhow::anyhow!("{}: {e}", file.uri))))?;
    let mut res = Response::new(body);
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_DISPOSITION, disposition);
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(res)
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}/file/metas",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id: any id of the file's loop family"),
        FileQuery,
    ),
    responses(
        (status = 200, description = "The file's size, from the index (the store is not asked).", body = FileMetasDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, or `path` is not a file of its loop family (one answer, never a redirect).", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn execution_file_metas(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<FileQuery>,
) -> egret_core::Result<Json<FileMetasDto>> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?;
    let file = family_file(&st, ctx.workspace_id.0, id, &q.path).await?;
    Ok(Json(FileMetasDto { size: file.size }))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/executions/{id}/file/preview",
    tag = "workflow",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Execution id: any id of the file's loop family"),
        PreviewQuery,
    ),
    responses(
        (status = 200, description = "Up to `maxRows` rows (default 100, at most 5000), read from the start of the file and never more than `workflow.storage.max_read_bytes` of it. A `.ion` file (text or binary Ion) gives one JSON value per top-level value; any other file whose bytes read are UTF-8 text gives one string per line; anything else, or Ion that does not parse, is `previewable: false` with no rows.", body = FilePreviewDto),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such execution in this workspace, or `path` is not a file of its loop family, or its contents are gone (one answer, never a redirect).", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is unavailable or not configured; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn preview_execution_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<FilePreviewDto>, Response> {
    egret_core::authz::require(&*st.pdp, &ctx, WF_READ)
        .await
        .map_err(core_refusal)?;
    let workspace = ctx.workspace_id.0;
    let file = family_file(&st, workspace, id, &q.path)
        .await
        .map_err(core_refusal)?;
    let max_rows = q
        .max_rows
        .unwrap_or(DEFAULT_PREVIEW_ROWS)
        .clamp(1, MAX_PREVIEW_ROWS) as usize;
    let fs = super::current().ok_or_else(no_store)?;
    let reader = open(&fs, workspace, &file).await?;
    let budget = Budget::new(reader, fs.caps.max_read_bytes, file.size);
    let state = budget.state.clone();
    let is_ion = file_name(&file.uri)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("ion"));
    let preview = if is_ion {
        preview_ion(budget, max_rows, &state, &file.uri).await
    } else {
        preview_lines(budget, max_rows, &state).await
    };
    // A full page stands even if the store failed on the one-past peek.
    let page_full = preview.rows.len() == max_rows;
    if let Some(e) = state.store_error().filter(|_| !page_full) {
        tracing::error!(uri = %file.uri, error = %e, "internal storage failed during a preview");
        return Err(unavailable(format!(
            "{}: internal storage is unavailable; retry later",
            file.uri
        )));
    }
    Ok(Json(preview))
}

fn not_previewable() -> FilePreviewDto {
    FilePreviewDto {
        previewable: false,
        rows: Vec::new(),
        truncated: false,
    }
}

async fn preview_ion(
    budget: Budget,
    max_rows: usize,
    state: &BudgetState,
    uri: &str,
) -> FilePreviewDto {
    // The Ion reader is synchronous: `ion::read_values` runs it on its own
    // thread, and this blocking task waits for it while the bridge pulls the
    // store's stream as the reader needs bytes.
    let bridge = tokio_util::io::SyncIoBridge::new(budget);
    let read = tokio::task::spawn_blocking(move || ion::read_values(bridge, max_rows)).await;
    let (rows, stop) = match read {
        Ok(read) => read,
        Err(e) => {
            tracing::error!(uri = %uri, error = %e, "the Ion preview's task failed");
            return not_previewable();
        }
    };
    match stop {
        ion::Stop::Full => FilePreviewDto {
            previewable: true,
            rows,
            truncated: true,
        },
        ion::Stop::End => FilePreviewDto {
            previewable: true,
            rows,
            truncated: false,
        },
        // The budget ended the read inside a value: the rows before it stand.
        ion::Stop::Failed(_) if state.exhausted() => FilePreviewDto {
            previewable: true,
            rows,
            truncated: true,
        },
        ion::Stop::Failed(_) => not_previewable(),
        ion::Stop::TooDeep(why) | ion::Stop::TooLarge(why) => {
            tracing::warn!(uri = %uri, "{why}");
            not_previewable()
        }
        ion::Stop::Crashed(why) => {
            tracing::error!(uri = %uri, error = %why, "the Ion reader failed");
            not_previewable()
        }
    }
}

async fn preview_lines(budget: Budget, max_rows: usize, state: &BudgetState) -> FilePreviewDto {
    let mut reader = tokio::io::BufReader::new(budget);
    let mut rows = Vec::new();
    let mut line = Vec::new();
    loop {
        if rows.len() == max_rows {
            let more = match reader.fill_buf().await {
                Ok(rest) => !rest.is_empty(),
                // The budget, or a store error, after a full page: more was there.
                Err(_) => true,
            };
            return FilePreviewDto {
                previewable: true,
                rows,
                truncated: more,
            };
        }
        line.clear();
        let ended = match reader.read_until(b'\n', &mut line).await {
            Ok(0) => true,
            Ok(_) => false,
            // The budget ended the read: a partial line is not a row.
            Err(_) if state.exhausted() => {
                return FilePreviewDto {
                    previewable: true,
                    rows,
                    truncated: true,
                }
            }
            Err(_) => return not_previewable(),
        };
        if ended {
            return FilePreviewDto {
                previewable: true,
                rows,
                truncated: false,
            };
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        // Text is UTF-8 with no NUL; a file that is not is not previewed at all.
        match std::str::from_utf8(&line) {
            Ok(s) if !s.contains('\0') => rows.push(serde_json::Value::String(s.to_string())),
            _ => return not_previewable(),
        }
    }
}

/// What a [`Budget`] saw, shared with the handler (the Ion read moves the
/// reader onto another thread).
#[derive(Clone, Default)]
struct BudgetState {
    exhausted: Arc<AtomicBool>,
    store_error: Arc<Mutex<Option<String>>>,
}

impl BudgetState {
    fn exhausted(&self) -> bool {
        self.exhausted.load(Ordering::SeqCst)
    }
    fn store_error(&self) -> Option<String> {
        self.store_error.lock().ok().and_then(|g| g.clone())
    }
}

/// The store's stream, cut at the preview's byte budget with an ERROR, not an
/// end: a reader that saw an end could take a value cut in two (`123456`
/// after `12`) for a whole one. A file that ends within the budget ends
/// normally. An error of the store itself is recorded, so the handler answers
/// 503 rather than "not previewable".
struct Budget {
    inner: Box<dyn AsyncBufRead + Send + Unpin>,
    left: u64,
    read: u64,
    /// The indexed size: at the budget, a file read whole ends normally.
    size: u64,
    state: BudgetState,
}

impl Budget {
    fn new(inner: Box<dyn AsyncBufRead + Send + Unpin>, budget: u64, size: u64) -> Self {
        Self {
            inner,
            left: budget,
            read: 0,
            size,
            state: BudgetState::default(),
        }
    }
}

impl AsyncRead for Budget {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.left == 0 {
            if self.read >= self.size {
                return Poll::Ready(Ok(()));
            }
            self.state.exhausted.store(true, Ordering::SeqCst);
            return Poll::Ready(Err(std::io::Error::other(
                "the preview's byte budget (workflow.storage.max_read_bytes) is spent",
            )));
        }
        let room = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.remaining());
        let this = &mut *self;
        let n = match Pin::new(&mut this.inner).poll_fill_buf(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => {
                if let Ok(mut g) = this.state.store_error.lock() {
                    *g = Some(e.to_string());
                }
                return Poll::Ready(Err(e));
            }
            Poll::Ready(Ok(available)) => {
                let n = available.len().min(room);
                buf.put_slice(&available[..n]);
                n
            }
        };
        Pin::new(&mut this.inner).consume(n);
        this.left -= n as u64;
        this.read += n as u64;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quoted_name_holds_no_quote_semicolon_separator_or_line_break() {
        let d = content_disposition("a\"b;c/d\r\ne é.txt");
        assert_eq!(
            d,
            "attachment; filename=\"a_b_c_d__e__.txt\"; \
             filename*=utf-8''a%22b%3Bc%2Fd%0D%0Ae%20%C3%A9.txt"
        );
        assert!(HeaderValue::from_str(&d).is_ok());
    }

    /// Fix round 1, I3: a row is a string, a number, null, an array or an
    /// object, so its schema is any value -- never `type: object`.
    #[test]
    fn preview_rows_are_documented_as_any_value() {
        let doc = serde_json::to_value(crate::routes::openapi()).unwrap();
        let rows = &doc["components"]["schemas"]["FilePreviewDto"]["properties"]["rows"];
        assert_eq!(rows["type"], "array", "{rows}");
        let items = rows["items"].as_object().expect("an items schema");
        assert!(
            !items.contains_key("type") && !items.contains_key("$ref"),
            "rows' items must accept any value: {rows}"
        );
    }

    #[test]
    fn kestras_names_come_out_as_kestra_wrote_them() {
        // `w9a-storage.json` `file_api.files.jsonl.file.content_disposition`.
        assert_eq!(
            content_disposition("2T7PTJb8vvcXwymU2Y73BK-data.jsonl"),
            "attachment; filename=\"2T7PTJb8vvcXwymU2Y73BK-data.jsonl\"; \
             filename*=utf-8''2T7PTJb8vvcXwymU2Y73BK-data.jsonl"
        );
        assert_eq!(
            content_disposition(""),
            "attachment; filename=\"file\"; filename*=utf-8''"
        );
    }
}
