//! Poll→SSE follow (package 7e #2, #3): the shared pieces behind
//! `GET …/executions/{id}/follow` and `GET …/executions/{id}/logs/follow`.
//!
//! The follow streams are **not** a reactive bus: each re-reads the store on a
//! fixed interval and emits only what changed, so there is one code path for a
//! live execution and a historical one, and nothing to keep in sync with the
//! executor. The two handlers live in [`crate::routes`] (they need its private
//! `require_exec`, DTOs and `WorkflowState`); this module holds the interval
//! and the one helper that wraps a built stream as an SSE response, so the
//! wrapping (content type, keep-alive) is spelled once.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, KeepAliveStream, Sse};
use futures_util::Stream;

/// How often a follow stream re-polls the store in production (1 s). A test
/// drives the loop faster through [`crate::WorkflowState::follow_poll_interval`],
/// which this is the default for — the same seam `upload_idle_timeout` uses.
pub const FOLLOW_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Wrap a poll-driven event stream as an SSE response with a keep-alive.
///
/// The keep-alive comment holds the connection open across a quiet poll
/// interval (nothing changed), so a proxy between the browser and egret does
/// not reap an idle-looking stream. The stream itself decides when to end (a
/// terminal execution, a drained log); this only dresses it.
pub fn sse_from_poll<S>(stream: S) -> Sse<KeepAliveStream<S>>
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(KeepAlive::default())
}
