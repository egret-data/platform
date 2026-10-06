//! The bounded multipart body of egret's upload routes: W9a's FILE-input
//! start (`POST …/flows/{id}/executions`, `routes::start_multipart`) and W9b's
//! namespace file upload (`POST …/namespaces/{ns}/files`,
//! [`super::nsfiles::routes`]).
//!
//! Moved here unchanged from `routes.rs` (W9b Task 8): the body limit
//! ([`multipart_body_limit`], `http_body_util::Limited`), the header budget
//! ([`HeaderBudget`], [`MultipartGate`]) and the refusal that reads what the
//! gate saw ([`multipart_refusal`]). [`IdleTimeout`] is W9b's: a namespace
//! file upload holds the path's advisory lock and a pool connection for as
//! long as its body takes to arrive, so a body that stops arriving ends it.

use std::future::Future as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;

use crate::routes::refuse;

/// Does `e`, or anything in its `source()` chain, say a body limit was
/// crossed? (`http_body_util::LengthLimitError`, however many boxes deep.)
pub(crate) fn is_length_limit(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(e);
    while let Some(err) = cur {
        if err.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        cur = err.source();
    }
    false
}

/// What [`HeaderBudget`] observed, shared with the handler that drives the
/// multipart parser.
#[derive(Default)]
pub(crate) struct MultipartGate {
    /// Set by the handler while it awaits the next part's headers.
    pub(crate) in_headers: std::sync::atomic::AtomicBool,
    /// Bytes pulled since `in_headers` was last set.
    pub(crate) pulled: std::sync::atomic::AtomicUsize,
    /// The header budget was crossed.
    pub(crate) header_overflow: std::sync::atomic::AtomicBool,
    /// The route's body limit was crossed.
    pub(crate) body_limit: std::sync::atomic::AtomicBool,
}

impl MultipartGate {
    pub(crate) fn start_headers(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.pulled.store(0, SeqCst);
        self.in_headers.store(true, SeqCst);
    }
    pub(crate) fn end_headers(&self) {
        self.in_headers
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The body multer reads, in pieces of at most [`HEADER_BUDGET_PIECE_BYTES`]:
/// while the handler awaits the next part's headers, more than
/// [`MAX_MULTIPART_HEADER_BYTES`] pulled is an error (fix round 1, I1).
///
/// multer keeps the preamble and a part's headers in memory until it finds
/// the boundary or `CRLFCRLF`, re-scanning its whole buffer on each pull, and
/// axum's `Multipart` exposes none of multer's `Constraints` -- none of which
/// bounds the headers anyway (they are per field and per stream). Without
/// this, an unterminated header is held up to the whole body limit
/// (`max_file_bytes` + 1 MiB, 1 GiB by default) per request. The pieces are
/// what make the count tight: a part's headers can be followed in the same
/// network chunk by its data, so at most one piece of data is counted with
/// them.
///
/// **One piece per poll.** multer's `StreamBuffer::poll_stream` pulls every
/// chunk the body has READY into its buffer before it parses anything -- a
/// fast client (or an in-memory body) is drained whole into multer's memory
/// on the first `next_field`. So after each piece this answers `Pending` once,
/// waking itself: multer then parses what it holds, and its buffer grows by
/// at most one piece per poll, while looking for headers and while a part's
/// data is read alike.
pub(crate) struct HeaderBudget {
    pub(crate) inner: std::pin::Pin<
        Box<
            dyn futures_util::Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>>
                + Send,
        >,
    >,
    pub(crate) pending: axum::body::Bytes,
    pub(crate) gate: Arc<MultipartGate>,
    /// A piece was just handed out: the next poll answers `Pending` (and wakes
    /// itself) once. See the type's doc.
    pub(crate) yielded: bool,
}

/// See [`HeaderBudget`].
pub(crate) const HEADER_BUDGET_PIECE_BYTES: usize = 8 * 1024;

impl futures_util::Stream for HeaderBudget {
    type Item = std::result::Result<axum::body::Bytes, axum::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::sync::atomic::Ordering::SeqCst;
        use std::task::Poll;
        let this = self.get_mut();
        if this.yielded {
            this.yielded = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        while this.pending.is_empty() {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(e))) => {
                    if is_length_limit(&e) {
                        this.gate.body_limit.store(true, SeqCst);
                    }
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(Some(Ok(b))) => this.pending = b,
            }
        }
        let n = this.pending.len().min(HEADER_BUDGET_PIECE_BYTES);
        let piece = this.pending.split_to(n);
        if this.gate.in_headers.load(SeqCst)
            && this.gate.pulled.fetch_add(n, SeqCst) + n > MAX_MULTIPART_HEADER_BYTES
        {
            this.gate.header_overflow.store(true, SeqCst);
            return Poll::Ready(Some(Err(axum::Error::new(std::io::Error::other(
                "a multipart part's headers are over the route's budget",
            )))));
        }
        this.yielded = true;
        Poll::Ready(Some(Ok(piece)))
    }
}

/// A multipart error as the response, read through what the gate saw first:
/// the header budget (400), the body limit (413, naming the limit -- M1),
/// else the parser's own status, where a read failure it cannot classify
/// (500 in axum) is the request's (400).
pub(crate) fn multipart_refusal(
    gate: &MultipartGate,
    limit: usize,
    status: StatusCode,
    text: String,
) -> axum::response::Response {
    use std::sync::atomic::Ordering::SeqCst;
    if gate.header_overflow.load(SeqCst) {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!(
                "a multipart part's headers (or the preamble before the first boundary) are over \
                 {MAX_MULTIPART_HEADER_BYTES} bytes; a part needs its headers ended by a blank \
                 line and the body its boundary"
            ),
        );
    }
    if gate.body_limit.load(SeqCst) {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, body_too_large(limit));
    }
    if status.is_server_error() {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("the multipart body could not be read: {text}"),
        );
    }
    refuse(status, text)
}

/// M1: the refusal for a body over the route's limit -- about the REQUEST,
/// not one file.
pub(crate) fn body_too_large(limit: usize) -> String {
    format!(
        "the request body is over {limit} bytes: one start carries at most \
         workflow.storage.max_file_bytes + 1 MiB in total"
    )
}

/// The room a multipart body has beyond one file of `max_file_bytes`: its
/// text parts, boundaries and headers (W9a spec §7.1, "the route's body limit
/// follows `max_file_bytes`").
pub const MULTIPART_OVERHEAD_BYTES: u64 = 1024 * 1024;

/// What the route lets multer pull while it looks for the next part's
/// headers (and, for the first part, the preamble before the first
/// boundary): multer keeps those bytes in memory until it finds `CRLFCRLF` or
/// the boundary, so without a cap an unterminated header is buffered up to
/// the whole body limit (W9a Task 7 fix round 1, I1).
pub const MAX_MULTIPART_HEADER_BYTES: usize = 64 * 1024;

/// `max_file_bytes` + [`MULTIPART_OVERHEAD_BYTES`], read from the installed
/// store when the request arrives.
pub(crate) fn multipart_body_limit(caps: crate::storage::Caps) -> usize {
    usize::try_from(caps.max_file_bytes.saturating_add(MULTIPART_OVERHEAD_BYTES))
        .unwrap_or(usize::MAX)
}

pub(crate) fn is_multipart(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("multipart/form-data"))
}
/// The longest a namespace file upload's body may go without a byte arriving
/// (W9b Task 8). `put` holds the path's advisory lock and a pool connection
/// across the upload, so a client that stops sending must not hold them: the
/// body ends with an error, `put` rolls back, the lock and the connection are
/// released, and the answer is a 408. The total time is bounded by the body
/// limit at this rate, not by a clock: a 1 GiB file on a slow link is legal.
pub const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// A body stream that fails once `idle` passes with no chunk arriving, and
/// says so in `timed_out` (so the route can answer 408, not the store's 503).
pub(crate) struct IdleTimeout<S> {
    inner: std::pin::Pin<Box<S>>,
    idle: Duration,
    sleep: std::pin::Pin<Box<tokio::time::Sleep>>,
    timed_out: Arc<AtomicBool>,
}

impl<S> IdleTimeout<S> {
    pub(crate) fn new(inner: S, idle: Duration, timed_out: Arc<AtomicBool>) -> Self {
        Self {
            inner: Box::pin(inner),
            idle,
            sleep: Box::pin(tokio::time::sleep(idle)),
            timed_out,
        }
    }
}

impl<S> futures_util::Stream for IdleTimeout<S>
where
    S: futures_util::Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>>,
{
    type Item = std::result::Result<axum::body::Bytes, axum::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        let this = self.get_mut();
        if this.timed_out.load(Ordering::SeqCst) {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(item) => {
                let deadline = tokio::time::Instant::now() + this.idle;
                this.sleep.as_mut().reset(deadline);
                Poll::Ready(item)
            }
            Poll::Pending => match this.sleep.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.timed_out.store(true, Ordering::SeqCst);
                    Poll::Ready(Some(Err(axum::Error::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "no byte of the request body arrived for {} seconds",
                            this.idle.as_secs()
                        ),
                    )))))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt as _;

    /// A body that sends one chunk and then nothing ends with an error after
    /// the idle timeout, flagged; one that keeps sending within it does not.
    /// Real time, short durations (the crate's tokio has no `test-util`).
    #[tokio::test]
    async fn a_body_that_stops_arriving_times_out_and_one_that_keeps_arriving_does_not() {
        let idle = Duration::from_millis(300);
        let flag = Arc::new(AtomicBool::new(false));
        let stalled = futures_util::stream::once(async {
            Ok::<_, axum::Error>(axum::body::Bytes::from_static(b"a"))
        })
        .chain(futures_util::stream::pending());
        let mut s = IdleTimeout::new(stalled, idle, flag.clone());
        assert_eq!(s.next().await.unwrap().unwrap(), "a");
        let started = std::time::Instant::now();
        let err = tokio::time::timeout(Duration::from_secs(10), s.next())
            .await
            .expect("the idle timeout fired")
            .unwrap()
            .unwrap_err();
        assert!(
            err.to_string().contains("no byte of the request body"),
            "{err}"
        );
        assert!(started.elapsed() >= idle);
        assert!(flag.load(Ordering::SeqCst));
        assert!(s.next().await.is_none());

        // Eight chunks 100 ms apart: 800 ms in all, never 300 ms idle.
        let flag = Arc::new(AtomicBool::new(false));
        let slow = futures_util::stream::iter(0..8).then(|_| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, axum::Error>(axum::body::Bytes::from_static(b"b"))
        });
        let got: Vec<_> = IdleTimeout::new(slow, idle, flag.clone()).collect().await;
        assert_eq!(got.len(), 8);
        assert!(got.iter().all(Result::is_ok));
        assert!(!flag.load(Ordering::SeqCst));
    }
}
