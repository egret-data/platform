//! URL → published snapshot → rows. The serving surface's request chain
//! (spec §6.2 steps 1-10, complete — step 7 landed in P4.5 and is described
//! under "Step 7, and the two halves it is split into" below).
//!
//! # The one invariant this module exists to hold
//!
//! **Routing comes from the endpoint row; content comes from the snapshot.**
//! `dataapi.endpoints (workspace_id, method, path)` is the pair carrying the
//! `UNIQUE` constraint, so it is the only place where "two endpoints claiming
//! one URL" is impossible — the snapshot's own `method`/`path` record where a
//! version *was* published and are not a routing key. Everything that decides
//! what the request RETURNS is then read from `v.snapshot` and from nowhere
//! else. If serving took its content from the draft table, an author saving an
//! edit would change behaviour under every already-deployed client and P3's
//! versioning would be decoration. See `crate::lifecycle`'s module comment.
//!
//! # Where 404 stops and 403 starts
//!
//! Unknown endpoint, disabled endpoint, an endpoint in another workspace and an
//! endpoint in an ARCHIVED workspace are all [`ServingError::NotFound`]. A
//! caller holding a token for workspace A must not be able to tell "no such
//! endpoint" from "exists, but not yours" — that difference is a free directory
//! of another tenant's URLs. The management API applies the same rule (spec
//! §6.1), and so does [`super::auth`] one layer up for authentication reasons.
//!
//! **Step 4 is where that reverses, and the reversal is the point rather than an
//! inconsistency.** Those 404s hide existence from a caller that has proved
//! nothing about itself. By the time [`invoke_allowed`] runs, the caller has
//! proved it holds a valid, unexpired credential for THIS workspace — so the
//! existence of an endpoint in it is not a secret the answer could keep, and
//! [`ServingError::NotGranted`] is a 403. Choosing 404 there would buy no
//! confidentiality and would cost an operator a real diagnosis: "the endpoint is
//! gone" instead of "the grant is missing".
//!
//! # Cancellation: decided, not deferred
//!
//! `QueryExecPort::execute_as_with_params` takes no `CancellationToken`
//! parameter, and for the ENGINE half of a request `limits.timeout` is this
//! handler's whole budget. **The decision is not to add one**, because the port
//! already owns the mechanism a caller-supplied token would duplicate:
//!
//! * `port_impl::run_bounded` — shared by `execute_as` and
//!   `execute_as_with_params`, which is why the wrapper cannot be present on one
//!   path and missing on the other — wraps the connector call in
//!   `tokio::time::timeout(limits.timeout, …)`. **Expiry releases egret by
//!   DROPPING the connector future**, which drops the connection with it. That
//!   wrapper is the whole wall clock on this path: neither
//!   `Connector::execute_with_params` implementation reads `limits.timeout`
//!   itself. Dropping the future closes the connection, which StarRocks 4.1.6
//!   answers by cancelling the statement (measured 2026-10-17).
//! * On client disconnect axum drops this handler's future, which drops the
//!   `execute_as` future, which drops the connector's — the same release path,
//!   with no token threaded anywhere.
//! * Trino additionally bounds the coordinator with `query_max_run_time`,
//!   derived from the same `limits.timeout`.
//!
//! # A transform is the ONE piece of this handler the paragraph above does not
//! # cover, and it is not covered by anything else either
//!
//! Two sentences up there were true of every request until C2 and are not true
//! of a request whose endpoint carries a response transform. Stated here rather
//! than left to be inferred from [`SCRIPT_REQUEST_DEADLINE`], because this is
//! the text a reader meets first:
//!
//! * **`limits.timeout` is not the whole budget.** A transformed request costs
//!   N statement timeouts *plus* up to [`SCRIPT_REQUEST_DEADLINE`].
//! * **Dropping the handler's future does not release the transform.** The
//!   evaluation runs on `tokio::task::spawn_blocking`; dropping the `JoinHandle`
//!   DETACHES the task, and `JoinHandle::abort` cannot stop a blocking task that
//!   has started either. So a client disconnect — and the deadline itself —
//!   stops the WAITING, never the work.
//!
//! **What a detached transform keeps holding**, which is the part that decides
//! how the 5 s should be read: one of the blocking pool's 512 default threads,
//! the statement rows MOVED into it (up to `limits.max_rows`, ceiling
//! [`MAX_ROWS_CEILING`]), and its starlark heap — until `transform::MAX_TICKS`
//! or `transform::MAX_WALL_CLOCK` fires, measured at **28.8 s** for a
//! tick-expensive script and not bounded at all for a tickless loop.
//!
//! **And the consequence nobody states when they pick a short deadline:
//! answering at 5 s invites the retry.** The caller gets a fast 500 and can
//! re-issue immediately, which re-runs the SQL and stacks a SECOND uncancellable
//! thread on top of the first; the endpoint's published rate limit is then the
//! only admission bound on how many pile up. The honest 28.8 s wait would have
//! self-throttled by simply not answering. The 5 s is still the right trade —
//! see [`SCRIPT_REQUEST_DEADLINE`] for why it is not 2 s — but it is a trade,
//! and this is the half of it that is invisible from the constant.
//!
//! **Note what is NOT the mechanism, because it reads like it is.**
//! `port_impl::execute_as` does create a `CancellationToken` and call
//! `cancel()`, and `StarRocksConnector::execute` does check
//! `cancel.is_cancelled()` and call `conn.disconnect()`
//! (`egret-query/src/execution/connector/starrocks.rs:238-242`) — but those two
//! never meet on this path: `tokio::time::timeout` drops the connector future
//! *before* `cancel()` runs, so the connector is gone before it could observe
//! the token. `port_impl`'s own comment says as much — that `cancel()` is "only
//! load-bearing if a future caller holds a clone of the token (as the JobManager
//! does)". Crediting it here would be crediting dead code.
//!
//! So a caller-supplied token adds nothing that dropping the future does not
//! already do, while costing 2 impls plus 4 test fakes now and a cross-crate
//! contract revision once the Data API moves to its own binary.
//!
//! What has NOT been measured, and should not be asserted either way until it
//! is: how completely each engine terminates a query server-side once its
//! connection goes away. If that turns out to need work, the change is in the
//! connector (a StarRocks `KILL QUERY <id>` on a second connection, say), not in
//! this port's signature — but do not treat that as the only thing worth doing
//! here.
//!
//! **This paragraph has now been wrong twice while its conclusion stayed
//! right** — first "StarRocks is unbounded server-side", then "`disconnect()`
//! is the reaction to the timeout", both asserted from reading and both false.
//! Across P3 and P4 together the count is five wrong mechanism claims in two
//! files (the other three were in `lifecycle.rs`, about `#` as a MySQL comment,
//! `#` as a syntax error, and a CLI-artifact measurement), and **at least two of
//! the five originated in review rather than in implementation** — so this is
//! not a discipline only implementers need.
//!
//! A conclusion that survives a wrong mechanism is a conclusion nobody has
//! checked the reasoning for; it is agreed with, not verified. If you are about
//! to restate any of this, open the two files first.
//!
//! # Step 7, and the two halves it is split into
//!
//! P4.5 landed it, and it is two lines far apart in [`run`] for a reason.
//!
//! * [`super::params::resolve`] runs BEFORE the cache lookup, because the key
//!   must describe what the request will RENDER. It percent-decodes names and
//!   values, drops undeclared parameters, applies defaults, type-checks against
//!   `ParamSpec.ty`, and produces both the renderer's context and the key's
//!   bytes from one pass — read its module comment before touching either half.
//! * [`crate::render::render`] runs after a cache MISS, because a hit needs no
//!   statement — once per statement, since M2. What goes to the engine is its
//!   output, never a statement's own definition: every `{{ … }}` has become a
//!   `?` and the values travel beside it as binds.
//!
//! **A `StatementSnapshot`'s definition must not be executed from this module.**
//! It is the template, not the statement; sending it would send `{{ region }}`
//! to the engine verbatim, which is the state P4 shipped in and P4.5 exists to
//! end.
//!
//! # Package S put a third thing between the cache and the render
//!
//! A statement's definition is now the author's TEXT or the author's SCRIPT
//! ([`StatementSnapshot::source`]), and `statement_texts` is what turns the
//! second into the first — per request, from this caller's resolved parameters.
//! Three properties of that placement, each measured rather than assumed, are
//! stated in that function: a cache hit generates nothing, the evaluation is one
//! `spawn_blocking` for the whole request rather than one per statement, and a
//! written statement pays nothing at all.
//!
//! **What it does NOT do is render.** The text a script returns still carries
//! its `{{ }}` and goes into the one `render` call above, so the path a caller's
//! value takes to the engine is the same for both kinds of statement. That is
//! the whole of package S's safety argument, and a second render call here would
//! end it.
//!
//! # What is deliberately NOT here yet
//!
//! Named so nothing reads as done that is not:
//!
//! * `ParamLocation::Body` parameters. Nothing on this surface parses a request
//!   body — [`serve_post`] takes no body extractor — so a declared `Body`
//!   parameter resolves through its `default`, or through `required` to a 400.
//!   It is a gap rather than a decision that body parameters are unsupported.
//!
//!   **Publish now refuses `location: body` outright**
//!   (`lifecycle::check_param_locations`), so no NEW version can reach this
//!   code with one. The handling above stays because a snapshot published
//!   before that rule can still be live — and the reason the rule exists is the
//!   OPTIONAL case, which is silent: the parameter resolves to absent, its
//!   `{% if %}` block is omitted, and the endpoint answers 200 with unfiltered
//!   rows. Such a snapshot also fails the parameter-gate stamp below, so in
//!   practice it is refused before it renders.
//!
//! # What Task 5 added: one id, and one place the request is described
//!
//! The `request_id` is minted ONCE, at the top of [`serve`], before anything can
//! fail — not at the two points where a response happened to be built, as it was
//! before. That is what lets the success body, the error body, every `tracing`
//! line for the call, and the `dataapi.access_log` row all carry the same id;
//! with three mints there were three ids and nothing correlated.
//!
//! [`serve`] therefore returns a `Response` rather than a
//! `Result<_, ServingError>`, and the whole request chain moved into [`run`].
//! `ServingError` no longer implements `IntoResponse` at all: the only route to
//! the wire is [`super::error::to_wire`], which needs a `request_id` it cannot
//! invent. Removing the impl is deliberate — leaving it would leave a second,
//! shorter path that compiles fine and produces an uncorrelatable body.
//!
//! # What Task 4 added, and what it keyed on
//!
//! Steps 5 and 6 are here now. **Both key on `endpoint_id`, never on the request
//! path** — axum percent-decodes the wildcard capture, so `reports%2Fdaily` and
//! `reports/daily` reach the same endpoint and a path-keyed bucket would be two
//! buckets a caller can alternate between, and a cache that never hits itself.
//! `endpoint_id` is already resolved by the time either is needed.
//!
//! The rate limit is applied BEFORE the cache lookup, per spec §6.2's order: a
//! published limit is the endpoint's admission control, and a client that
//! exhausts it should not be able to keep pulling cached rows out of it.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_dataapi_api::{
    compare_column_types, ColumnSpec, EndpointSnapshot, OutputSpec, StatementSnapshot,
    TransformSpec,
};
use egret_metastore::Db;
use serde::Serialize;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::access_log::{AccessLogEntry, AccessLogStore};
use super::cache::{cache_key, cache_ttl, shared_cache_allowed};
use super::error::{is_permission_denied, to_wire, ServingErrorBody};
use super::governance::{governance_allows_serving, LiveConnection};
use super::guard;
// Aliased: `metrics` unqualified would read as the FACADE crate at every
// call site in this file, and the two are one keystroke apart.
use super::metrics as serving_metrics;
use super::params;
use super::ratelimit::{check_request, retry_after_secs};
use super::{verify_serving_token, ServingError};
use crate::routes::DataApiState;
use crate::try_it::TrialActor;

/// Upper bound applied to a snapshot's `limits.max_rows` before it is acted on.
///
/// `limits` is frozen into the snapshot with NO validation rule anywhere (spec
/// §5.3 says so explicitly), so a published value is author input that has never
/// been checked. A `max_rows` of 0 would make every endpoint return nothing, and
/// an unbounded one would let a single call materialise an arbitrary result set
/// in memory. Clamping is the honest reading of spec §7's "the serving path must
/// sanity-check `limits.max_rows` and `limits.timeout_seconds` before acting on
/// them".
pub(crate) const MAX_ROWS_CEILING: u32 = 1_000_000;

/// Upper bound applied to a snapshot's `limits.timeout_seconds`. Same reasoning
/// as [`MAX_ROWS_CEILING`]; a 0 would make every call time out instantly and an
/// unbounded one would pin a connection for as long as the author felt like.
pub(crate) const TIMEOUT_CEILING_SECS: u64 = 300;

/// How long a REQUEST waits for author-supplied starlark before it is answered
/// 500 — a response transform (C2), or the SQL generation package S put on this
/// path.
///
/// # One constant for both phases, and that is a derivation rather than reuse
///
/// The number below is derived from `transform::MAX_WALL_CLOCK`, which is a
/// property of the EVALUATOR: it must not be tighter than the evaluator's own
/// honest budget. `crate::transform::build_sql` runs on the same `eval_inner`
/// under the same `Budgets::default()` as `run_transform`, so the same
/// derivation gives the same number, and two constants would be two things that
/// can drift apart while the budget they are both derived from does not.
///
/// **Measured for `build_sql` itself rather than inherited from C2's
/// transform**: a tick-expensive `build_sql` script ran **39.9 s against the 2 s
/// `MAX_WALL_CLOCK`** before the budget stopped it (debug build, package S Task
/// 6). The evaluator's budget bounds ticks, not time, and generation is on the
/// same side of that as the transform is.
///
/// # This is the serving path's own deadline, and it has to be
///
/// `transform::MAX_WALL_CLOCK` is 2 s and reads like a latency bound. It is not
/// one, and that function's own doc says so from measurement: the deadline is
/// polled every 1000 starlark ticks, so a script whose ticks are individually
/// expensive overruns it — **1000 sorts of a 200 000-element list ran 28.8 s
/// against the 2 s budget**, and a tickless string-doubler was still running at
/// 6.8 s. A serving path that treated the evaluator's budget as its own would
/// promise a 2 s worst case and deliver half a minute.
///
/// So the request gets a deadline of its own, here, denominated in the thing a
/// caller experiences. 5 s rather than 2 s because it must not be TIGHTER than
/// the evaluator's honest budget: a script that legitimately runs close to
/// `MAX_WALL_CLOCK` and finishes must be served, not cut off by a race between
/// two clocks measuring the same work.
///
/// # What this bounds, and what it does not
///
/// It bounds **the request**. It does not bound the CPU: a `spawn_blocking` task
/// cannot be cancelled, so an overrunning script keeps burning a blocking thread
/// until starlark's own tick check stops it, and the caller has already been
/// answered by then. Two consequences worth stating rather than discovering:
///
/// * the tokio blocking pool is bounded (512 threads by default), so a flood of
///   pathological scripts can saturate it, and the queue is what would then
///   slow unrelated blocking work. The bound is a real one; nothing here makes
///   it unbounded, and `MAX_TICKS`/`MAX_WALL_CLOCK` are what eventually free
///   each thread;
/// * the evaluation is on a blocking thread precisely BECAUSE of the 28.8 s
///   measurement. `run_transform` is synchronous and CPU-bound; calling it
///   inline would hold a tokio worker thread for that whole time, stalling every
///   unrelated request scheduled on it. The stack story is unchanged —
///   `transform::MAX_JSON_DEPTH` is calibrated against the 2 MiB stack tokio
///   gives worker AND blocking threads alike.
const SCRIPT_REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// The deadline one request gives its transform, as a value rather than as a
/// constant read at the call site.
///
/// # Why this exists at all
///
/// Exactly the reason `transform::Budgets` is a struct: **a limit with no
/// injection point cannot be tested.** With [`SCRIPT_REQUEST_DEADLINE`] read
/// directly inside [`transformed_data`], the only way to reach the timeout arm
/// was to wait five seconds of real time, so nothing did — and *deleting the
/// `tokio::time::timeout` wrapper entirely killed no test at all*. That is the
/// same shape as a budget that is never binding: present in the source, absent
/// from the guarantees.
///
/// # Non-test code cannot express a weakened deadline
///
/// The field is private and the only constructor that takes a value is
/// `#[cfg(test)]`, so the sole `ScriptDeadline` a shipped binary can build is
/// [`ScriptDeadline::default`] — the constant above. A relaxation that is
/// unrepresentable is better than one a reviewer has to remember to look for,
/// which is the argument `Budgets` already makes for itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScriptDeadline(Duration);

impl Default for ScriptDeadline {
    fn default() -> Self {
        Self(SCRIPT_REQUEST_DEADLINE)
    }
}

#[cfg(test)]
impl ScriptDeadline {
    /// A deadline a test can actually reach. Not available to a shipped build.
    fn of(d: Duration) -> Self {
        Self(d)
    }
}

/// `status` in the envelope (spec §4.4).
///
/// **It MIRRORS the HTTP status; it never replaces it.** A body saying
/// `"status": "error"` under a `200` is the failure mode this exists to avoid —
/// SQLREST answers `200` for everything and pushes the outcome into the body
/// (`HttpApiServlet.process` never calls `setStatus`), which breaks HTTP caches,
/// proxies and monitoring, and forces every client to parse a body to learn that
/// it failed. egret keeps returning real `400`/`403`/`429`/`503`.
///
/// So this value is DERIVED from the status line rather than written beside it —
/// see [`ServingStatus::mirroring`], which is the only constructor. Two
/// independent spellings of one outcome is exactly the shape that drifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServingStatus {
    Ok,
    Error,
}

impl ServingStatus {
    /// The only way to build one: read it off the status the response will
    /// actually carry.
    pub(crate) fn mirroring(http: StatusCode) -> Self {
        if http.is_success() {
            Self::Ok
        } else {
            Self::Error
        }
    }
}

/// The success body (spec §4.4, §5.1.2).
///
/// ```json
/// { "status": "ok", "message": null, "data": …, "meta": { … } }
/// ```
///
/// **This replaced the `{data, meta}` body P4 shipped, in one migration rather
/// than by carrying two shapes.** Endpoints published under the old envelope
/// have served real traffic since 2026-08-16; the decision to break rather than
/// version was taken deliberately (spec §4.4) because consumers are few and
/// internal, the API is pre-1.0, and two envelopes forever cost more than one
/// migration. It belongs in the deploy note and must not be made silent.
///
/// `meta.version` is the published version that served the call, so a
/// client-side bug is reproducible against the exact snapshot that produced it.
/// `meta.cached` is likewise not cosmetic: without it a stale-looking result is
/// indistinguishable from a fresh one while debugging.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingResponse {
    /// Always [`ServingStatus::Ok`] here — this type is only built on the
    /// success path — but read off `StatusCode::OK` rather than written as a
    /// constant, so the two cannot drift apart. See [`ServingStatus`].
    pub status: ServingStatus,
    /// `null` on success. It is the human-readable half of a REFUSAL
    /// ([`ServingErrorBody`](super::error::ServingErrorBody)), not a success
    /// banner, and a client that finds a string here is reading an error.
    pub message: Option<String>,
    /// The rows for a ONE-statement endpoint — an array of objects (default) or
    /// of arrays, per `output.shape` — and an object keyed by statement NAME for
    /// several.
    ///
    /// Not a union at the contract level: `openapi_gen` writes one document per
    /// endpoint and an endpoint's statement count is frozen in its snapshot, so
    /// each generated schema is unambiguous.
    ///
    /// The SHARED serving document below describes every endpoint at once, so it
    /// deliberately carries no `value_type`: utoipa then emits the unconstrained
    /// schema for `serde_json::Value`, which is the only honest answer there.
    /// `value_type = Vec<Object>` — what this field carried until M2 — asserted
    /// "array of objects" about a body that is an OBJECT for a several-statement
    /// endpoint.
    pub data: serde_json::Value,
    pub meta: ServingMeta,
}

/// The endpoint-wide half of `meta`, plus whichever contract shape the
/// endpoint's statement count calls for.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingMeta {
    pub version: i32,
    pub cached: bool,
    pub request_id: String,
    /// Flattened, so the wire carries `{version, cached, request_id, row_count,
    /// columns}` or `{version, cached, request_id, statements}` — never a
    /// `contract` key of its own.
    #[serde(flatten)]
    pub contract: MetaContract,
}

impl ServingMeta {
    /// The endpoint's total rows, for the access-log row and the "served" log
    /// line — the two places that want ONE number.
    ///
    /// Deliberately not a wire field for several statements: a sum across reads
    /// that share no transaction and no snapshot (spec §4.5) is not the row
    /// count of anything a caller can name.
    pub(crate) fn total_row_count(&self) -> u64 {
        match &self.contract {
            MetaContract::Single { row_count, .. } => *row_count,
            MetaContract::Several { statements } => statements
                .iter()
                .fold(0u64, |acc, (_, s)| acc.saturating_add(s.row_count)),
        }
    }
}

/// What `meta` reports about the published contract, keyed the only way the
/// endpoint's statement count permits.
///
/// An enum rather than four `Option` fields: `{row_count, columns}` and
/// `{statements}` are the two legal fillings, and a struct with all of them
/// optional can also represent both at once and neither at all — two states in
/// which a client would have two readings of one contract that can disagree.
///
/// **`Several` is what Task 7's `meta.columns` could not express.** Until this
/// existed the field held every statement's frozen columns CONCATENATED in
/// statement order: complete, but unkeyed, so a caller holding
/// `["id","name","total"]` could not learn which statement declared which — and
/// had no way to notice the question was unanswerable.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum MetaContract {
    /// One statement — every endpoint published before M2, and most after it.
    Single {
        row_count: u64,
        /// The FROZEN columns from the snapshot, not the ones this execution
        /// happened to return. The published contract is what a client codes
        /// against; if the underlying table has drifted, the difference is a
        /// defect to surface, not something to paper over by reporting today's
        /// shape.
        columns: Vec<ColumnSpec>,
    },
    /// Several statements, keyed by name — the same keys, in the same order, as
    /// `data`.
    Several {
        /// Serialized as a JSON OBJECT, from a `Vec` rather than a map, for one
        /// reason: the order is the author's, stated by the list they published,
        /// and it has to be the order `data` uses. A `BTreeMap` would sort the
        /// names and make `meta` disagree with `data` about the sequence for
        /// every endpoint whose statements are not alphabetical.
        ///
        /// No key can be lost to another: publish refuses two statements of one
        /// name (`validate_statements`), which is the same guarantee
        /// `statements_data` rests on one level up.
        #[serde(serialize_with = "serialize_statement_metas")]
        #[schema(value_type = std::collections::BTreeMap<String, StatementMeta>)]
        statements: Vec<(String, StatementMeta)>,
    },
}

/// One statement's half of `meta.statements`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StatementMeta {
    pub row_count: u64,
    /// This statement's own frozen columns — see [`MetaContract::Single`]'s
    /// `columns` for why they are the frozen ones.
    pub columns: Vec<ColumnSpec>,
}

fn serialize_statement_metas<S>(
    statements: &[(String, StatementMeta)],
    s: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.collect_map(statements.iter().map(|(name, meta)| (name.as_str(), meta)))
}

/// Which envelope a response is written in (spec §4.4).
///
/// `Bare` is SQLREST's `USE_SYSTEM_RESPONSE_FORMAT=false`, and the one part of
/// its response design worth copying: a consumer that already knows the shape it
/// asked for should not have to unwrap it on every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Envelope {
    Wrapped,
    Bare,
}

/// Read `output.envelope` off a frozen snapshot.
///
/// An unrecognised value serves the documented default and LOGS, exactly like
/// `output.shape` and `output.naming` — `output` is frozen at publish with no
/// validation rule anywhere (spec §5.3, and `lifecycle`'s module comment says so
/// explicitly), so an unrecognised value is reachable. Refusing would make a
/// published endpoint permanently dead over a cosmetic field, and unlike a drift
/// it cannot silently lose a column.
pub(crate) fn envelope_of(output: &OutputSpec) -> Envelope {
    match output.envelope.as_str() {
        "bare" => Envelope::Bare,
        "wrapped" => Envelope::Wrapped,
        other => {
            tracing::warn!(
                envelope = %other,
                "published snapshot carries an unrecognised output.envelope; serving the default"
            );
            Envelope::Wrapped
        }
    }
}

/// A decided success answer, and how to write it.
///
/// The envelope is a property of the SNAPSHOT, which only [`run`] has read, and
/// the rendering happens in [`serve`], which has the access-log row to fill. So
/// the choice travels with the body rather than being re-derived — re-reading
/// the snapshot in `serve` would be a second decision that can disagree with the
/// first.
pub(crate) struct Served {
    envelope: Envelope,
    response: ServingResponse,
}

impl Served {
    /// `bare` writes `data`'s VALUE — for several statements that is the object
    /// keyed by name, not "the rows". Nothing else is written: no `meta`, no
    /// `status`, no wrapper of any kind.
    fn into_response(self) -> Response {
        match self.envelope {
            Envelope::Wrapped => Json(self.response).into_response(),
            Envelope::Bare => Json(self.response.data).into_response(),
        }
    }
}

/// The serving surface. Mounted at `/data-api/v1` by `egret_server::build_router`
/// as its own group with **no CSRF layer** — see the comment at that call site.
///
/// `GET` and `POST` only, matching `endpoints.method`'s CHECK constraint. A
/// method outside the pair never reaches [`serve`]; axum answers 405, which is
/// correct and leaks nothing (it is the same answer for every path, existing or
/// not, under this router).
pub fn serving_router() -> Router<DataApiState> {
    serving_router_with_bounds(
        guard::SERVING_REQUEST_DEADLINE,
        guard::SERVING_MAX_CONCURRENCY,
        guard::SERVING_MAX_BODY_BYTES,
    )
}

/// [`serving_router`] with its three bounds supplied, so a test can drive a
/// limit it genuinely exceeds instead of asserting that a layer was installed.
///
/// # The nesting order is the design, not an accident of `.layer` calls
///
/// axum applies the LAST `.layer` outermost, so reading the calls bottom-up
/// gives the order a request meets them:
///
/// 1. **body cap** — the only refusal that costs nothing, so it is first;
/// 2. **deadline** — started before admission, so time spent being admitted is
///    charged against the caller's budget rather than added to it (see
///    [`guard::SERVING_REQUEST_DEADLINE`]);
/// 3. **bulkhead** — the last gate before the handler, holding its permit for
///    exactly the span of `serve` and released by the guard's drop even when the
///    deadline above cancels the future.
///
/// The semaphore is built HERE, once per router, rather than per request — the
/// same reason `DataApiState`'s cache and token buckets are constructed once in
/// `egret-bin`.
///
/// **That "once" is a property of the CALLER, not of this function.** A caller
/// that builds a router per request gets a fresh semaphore per request, which is
/// a limit nobody can ever reach — the mistake [`guard::serving_bulkhead`]'s own
/// doc warns about. `egret-server` builds its mount once at boot and is fine.
/// `try_it` builds a router per trial and is **not**, so it does not call this
/// function: it calls [`serving_router_sharing_bulkhead`] with a semaphore that
/// outlives the trial. An earlier version of this comment claimed the throwaway
/// router's private bulkhead was "the intended isolation"; it was not isolation,
/// it was an unreachable limit.
pub fn serving_router_with_bounds(
    deadline: Duration,
    permits: usize,
    max_body_bytes: u64,
) -> Router<DataApiState> {
    serving_router_sharing_bulkhead(deadline, guard::serving_bulkhead(permits), max_body_bytes)
}

/// [`serving_router_with_bounds`] with the bulkhead **supplied** rather than
/// built, for a caller whose router is constructed more than once.
///
/// The bulkhead is the one of the three bounds whose meaning depends on the
/// router outliving the request. A deadline and a body cap are per-request
/// numbers and stay correct however often the router is rebuilt; a semaphore
/// rebuilt per request has as many permits as there are requests. This entry
/// point exists so `try_it` can keep its permits somewhere that survives the
/// trial that borrowed them.
pub fn serving_router_sharing_bulkhead(
    deadline: Duration,
    bulkhead: Arc<Semaphore>,
    max_body_bytes: u64,
) -> Router<DataApiState> {
    Router::new()
        .route("/{ws}/{*path}", get(serve_get).post(serve_post))
        // Package 8. The contract for the calling service account, at the very
        // URL `servers[0].url` names — and the one URL under this mount a
        // published endpoint can never take, because `normalize_path` refuses an
        // empty path so every published URL has a segment after the slug. Inside
        // the three layers below on purpose: building a document costs one PDP
        // decision per live endpoint. See `serving::contract`.
        .route("/{ws}", get(super::contract::serve_contract))
        .layer(axum::middleware::from_fn(move |req, next| {
            guard::serving_admit(bulkhead.clone(), req, next)
        }))
        .layer(axum::middleware::from_fn(move |req, next| {
            guard::serving_deadline(deadline, req, next)
        }))
        .layer(axum::middleware::from_fn(move |req, next| {
            guard::serving_body_limit(max_body_bytes, req, next)
        }))
}

/// The serving surface's OpenAPI document, merged by `egret_server::openapi_doc`
/// beside [`crate::openapi`].
///
/// Kept as a SECOND document rather than folded into `DataApiAdminApi` because
/// the two describe different audiences: one is a browser session surface under
/// `/api/ws/{workspace}/…`, the other is a machine surface under
/// `/data-api/v1/{ws}/…`. The path parameter is `{ws}` and not `{workspace}` on
/// purpose — `egret-workspace`'s feature catalog records that this surface is
/// deliberately outside the `/api/ws/` feature gate (see `features.rs`'s
/// `data_api` entry), and `egret-server`'s `feature_route_coverage` suite reads
/// `{workspace}` as the marker of a gated route.
pub fn serving_openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    DataApiServingApi::openapi()
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(serve_get, serve_post, super::contract::serve_contract),
    components(schemas(
        ServingResponse,
        ServingStatus,
        ServingMeta,
        MetaContract,
        StatementMeta,
        ServingErrorBody,
        super::error::ServingErrorDetail,
        egret_dataapi_api::ColumnSpec
    )),
    modifiers(&BearerAddon, &ErrorCodeEnumAddon),
    tags((
        name = "data-api-serving",
        description = "Published Data API endpoints, called by service accounts with a bearer token"
    )),
)]
struct DataApiServingApi;

/// Declares the `data_api_bearer` security scheme the two operations require.
///
/// Without it the operations name a scheme that `components.securitySchemes`
/// never defines, which is an invalid document — and this is the only `security`
/// requirement in the whole merged doc, so nothing else would have declared it.
/// A `security` requirement pointing at nothing is worse than none: it reads as
/// documented auth while telling a generator nothing it can act on.
struct BearerAddon;

impl utoipa::Modify for BearerAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "data_api_bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "A `data-api` audience JWT from `POST /oauth/token` \
                         (OAuth2 client_credentials). TTL 600s.",
                    ))
                    .build(),
            ),
        );
    }
}

/// Puts the DERIVED `enum` of serving error codes onto
/// `ServingErrorDetail.code` in this document.
///
/// # Why a modifier and not `#[schema(...)]` on the field
///
/// The list is [`crate::serving::error::wire_codes`], computed from the variant
/// census at run time. `#[schema(...)]` takes literals, so declaring it there
/// would be a second hand-maintained copy of the taxonomy — the exact thing
/// `wire_codes`' comment records going stale twice already, once in this very
/// document. `openapi_gen`'s per-service-account document takes the same list
/// from the same function, so the two cannot disagree about what a client may
/// receive.
///
/// # It PANICS rather than silently doing nothing
///
/// A modifier that walks a path and returns quietly when it does not find it is
/// a guard that cannot fail: rename the schema or the field and the `enum`
/// vanishes from the published document with every test still green. The two
/// names it walks are owned by [`super::error::ServingErrorDetail`]'s `ToSchema`
/// derive, several files away, so nothing local would flag the drift. Both
/// lookups therefore `expect`, and `tests/contract_reachable.rs`'s
/// `the_published_root_document_names_every_code_too` is the test that executes
/// this path and reads the result back out of the document.
///
/// # What a panic here would actually take down — NOT the server
///
/// Worth stating, because "a `Modify` that panics" reads like a boot hazard and
/// is not one. Nothing in `build_router` builds this document:
/// `egret_server::openapi_doc()` has exactly two callers, the `/openapi.json`
/// handler (a dev doc route, mounted unauthenticated and outside every layer)
/// and `egret-bin`'s `openapi` subcommand, which prints the spec and exits
/// before it reads config or touches the database. So the blast radius of the
/// drift this guards against is a 500 on `/openapi.json` and a non-zero exit
/// from the code generator — loud, immediate, and caught in CI long before a
/// deployment, while served traffic is unaffected. That is the trade this
/// `expect` is making, and it is a cheap one.
struct ErrorCodeEnumAddon;

impl utoipa::Modify for ErrorCodeEnumAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{RefOr, Schema};

        let components = openapi.components.get_or_insert_with(Default::default);
        let detail = components
            .schemas
            .get_mut("ServingErrorDetail")
            .expect("ServingErrorDetail must be a component of the serving document");
        let RefOr::T(Schema::Object(detail)) = detail else {
            panic!("ServingErrorDetail must be an inline object schema");
        };
        let code = detail
            .properties
            .get_mut("code")
            .expect("ServingErrorDetail must carry a `code` property");
        let RefOr::T(Schema::Object(code)) = code else {
            panic!("ServingErrorDetail.code must be an inline object schema");
        };
        code.enum_values = Some(
            super::error::wire_codes()
                .into_iter()
                .map(serde_json::Value::from)
                .collect(),
        );
    }
}

// A `//` comment and NOT a doc comment: `utoipa` lifts a doc comment on a handler
// into the operation's `summary`/`description`, and this is a note to whoever
// edits the annotation, not something a machine consumer should read.
//
// **Every status and code below is pinned by
// `the_published_contract_names_every_answer_the_surface_can_give`**, which reads
// them out of the generated document and compares them against `to_wire`'s table
// rather than against a list. Before that test the responses declared
// `500 internal` and nothing else, so `502`, `504` and `endpoint_query_failed` —
// all of them live — were statuses a machine consumer branching on this document
// had no case for. The `openapi.json` byte gate did not notice, because nothing
// in the document had changed: that gate answers "is the committed file
// current", never "is the document true".
//
// `serve_post` below carries the same list, verbatim. `utoipa::path` takes
// literals, so the duplication cannot be factored out — the test asserts the two
// operations independently for that reason.
#[utoipa::path(
    get,
    path = "/data-api/v1/{ws}/{path}",
    tag = "data-api-serving",
    params(
        ("ws" = String, Path, description = "Workspace slug; must match the token's `ws` claim"),
        ("path" = String, Path, description = "The endpoint's published path, e.g. `reports/daily`"),
        ("X-Egret-Version" = Option<i32>, Header, description = "Pin the request to a published version of this endpoint, so a publish does not move this caller until they choose to move. Omit it to call whatever is live — the behaviour every caller had before this header existed. The number is the `version` in `meta`, and the `x-egret-version` stamped on this operation in the generated per-service-account document. A pin selects which frozen CONTRACT answers and never which live policy applies: the invoke grant, the governed-database check, the endpoint's state and the workspace's are all evaluated exactly as they are for an unpinned call, and the endpoint's rate limit is the LIVE version's in both cases. A version above the live one is refused — that can only mean a rollback moved consumers off it"),
    ),
    responses(
        (status = 200, description = "Rows from the published snapshot", body = ServingResponse),
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap; `param_missing` — a required parameter was not supplied; `param_invalid` — a supplied parameter failed its declared type, its `allowed` list, or was repeated where one value is required. Neither parameter error names the parameter: the name is the author's, and naming it turns the error surface into a directory of what an endpoint declares. `version_pin_invalid` — the `X-Egret-Version` header was not a single positive whole number; it names the caller's own header and no fact about the endpoint", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine refused this service account the underlying data, either because it lacks a data permission (a Ranger grant, say) or because its engine credential is bad or expired; one code, because the two recognitions available (Trino's typed 401/403 from the coordinator, and every other engine failure matched on its own message) cannot tell those causes apart. `not_granted` — egret refused the call because the service account holds no invoke grant on this endpoint", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 409, description = "`version_unavailable` — the version named by `X-Egret-Version` is not one this endpoint can serve: it was never published, or a rollback has moved consumers off it. One code for both, because a consumer's action is the same either way and two would enumerate the endpoint's history. A 409 rather than a 400 because the same request is correct or incorrect depending on which version is live at that instant; no `Retry-After`, because it clears only when a human republishes or the caller changes what it sends", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 413, description = "`payload_too_large` — the request declared a body larger than this surface accepts", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault (the metastore, the secret store, a corrupt snapshot); quote `request_id` to support. `endpoint_query_failed` — the engine ran the endpoint's published statement and refused it: the request was fine and the endpoint is what broke, so it is a 5xx rather than a 4xx, and its own code so that `which endpoints are failing` is separable from `egret is failing`", body = ServingErrorBody),
        (status = 502, description = "`engine_unavailable` — the query engine behind this endpoint could not be reached. An operator's problem, not the endpoint author's; the engine, its host and its port are deliberately not named", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` / `governance_stale` / `param_gate_stale` — the published configuration no longer describes the data, was frozen before the current parameter-safety rules existed, or points at a `governed` database through a connection that does not run queries as the caller. `Retry-After` is a floor, not a forecast: a re-publish clears the first four, and `governance_stale` is cleared only by restoring the connection or the database's `governed` flag. `overloaded` — egret shed the request because the serving surface was at its concurrency limit; that one clears by itself and its `Retry-After` is a prediction. `rate_limiter_unavailable` — the shared rate-limit store could not be consulted, so the request was refused rather than admitted without an allowance; it also clears by itself", body = ServingErrorBody),
        (status = 504, description = "`timeout` — the endpoint's statement exceeded the time budget its author published (`limits.timeout`); the fix is the endpoint's, not the caller's. `deadline_exceeded` — the request outran egret's own serving deadline. Two codes for one status on purpose: they name two different actors, and `code` is what tells them apart", body = ServingErrorBody),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_get(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
    trial: Option<Extension<TrialActor>>,
) -> Response {
    serve(state, path, query, method, headers, trial).await
}

#[utoipa::path(
    post,
    path = "/data-api/v1/{ws}/{path}",
    tag = "data-api-serving",
    params(
        ("ws" = String, Path, description = "Workspace slug; must match the token's `ws` claim"),
        ("path" = String, Path, description = "The endpoint's published path, e.g. `reports/daily`"),
        ("X-Egret-Version" = Option<i32>, Header, description = "Pin the request to a published version of this endpoint, so a publish does not move this caller until they choose to move. Omit it to call whatever is live — the behaviour every caller had before this header existed. The number is the `version` in `meta`, and the `x-egret-version` stamped on this operation in the generated per-service-account document. A pin selects which frozen CONTRACT answers and never which live policy applies: the invoke grant, the governed-database check, the endpoint's state and the workspace's are all evaluated exactly as they are for an unpinned call, and the endpoint's rate limit is the LIVE version's in both cases. A version above the live one is refused — that can only mean a rollback moved consumers off it"),
    ),
    responses(
        (status = 200, description = "Rows from the published snapshot", body = ServingResponse),
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap; `param_missing` — a required parameter was not supplied; `param_invalid` — a supplied parameter failed its declared type, its `allowed` list, or was repeated where one value is required. Neither parameter error names the parameter: the name is the author's, and naming it turns the error surface into a directory of what an endpoint declares. `version_pin_invalid` — the `X-Egret-Version` header was not a single positive whole number; it names the caller's own header and no fact about the endpoint", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine refused this service account the underlying data, either because it lacks a data permission (a Ranger grant, say) or because its engine credential is bad or expired; one code, because the two recognitions available (Trino's typed 401/403 from the coordinator, and every other engine failure matched on its own message) cannot tell those causes apart. `not_granted` — egret refused the call because the service account holds no invoke grant on this endpoint", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 409, description = "`version_unavailable` — the version named by `X-Egret-Version` is not one this endpoint can serve: it was never published, or a rollback has moved consumers off it. One code for both, because a consumer's action is the same either way and two would enumerate the endpoint's history. A 409 rather than a 400 because the same request is correct or incorrect depending on which version is live at that instant; no `Retry-After`, because it clears only when a human republishes or the caller changes what it sends", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 413, description = "`payload_too_large` — the request declared a body larger than this surface accepts", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault (the metastore, the secret store, a corrupt snapshot); quote `request_id` to support. `endpoint_query_failed` — the engine ran the endpoint's published statement and refused it: the request was fine and the endpoint is what broke, so it is a 5xx rather than a 4xx, and its own code so that `which endpoints are failing` is separable from `egret is failing`", body = ServingErrorBody),
        (status = 502, description = "`engine_unavailable` — the query engine behind this endpoint could not be reached. An operator's problem, not the endpoint author's; the engine, its host and its port are deliberately not named", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` / `governance_stale` / `param_gate_stale` — the published configuration no longer describes the data, was frozen before the current parameter-safety rules existed, or points at a `governed` database through a connection that does not run queries as the caller. `Retry-After` is a floor, not a forecast: a re-publish clears the first four, and `governance_stale` is cleared only by restoring the connection or the database's `governed` flag. `overloaded` — egret shed the request because the serving surface was at its concurrency limit; that one clears by itself and its `Retry-After` is a prediction. `rate_limiter_unavailable` — the shared rate-limit store could not be consulted, so the request was refused rather than admitted without an allowance; it also clears by itself", body = ServingErrorBody),
        (status = 504, description = "`timeout` — the endpoint's statement exceeded the time budget its author published (`limits.timeout`); the fix is the endpoint's, not the caller's. `deadline_exceeded` — the request outran egret's own serving deadline. Two codes for one status on purpose: they name two different actors, and `code` is what tells them apart", body = ServingErrorBody),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_post(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
    trial: Option<Extension<TrialActor>>,
) -> Response {
    serve(state, path, query, method, headers, trial).await
}

/// The outer half of a served call: mint the correlation id, run the chain,
/// render the answer, and record the row.
///
/// # Why the id is minted here and not where a response is built
///
/// `request_id` is the only thing tying four artefacts together: the body the
/// caller holds, the `tracing` line carrying the detail the body was denied, the
/// `dataapi.access_log` row, and — once `QueryExecPort` returns one — the
/// engine's own query id. Minting it inside the success paths (which is what
/// this used to do, twice, with a third id nowhere) made every one of those a
/// different id, so "quote your request_id" was advice that could not be acted
/// on.
///
/// # Why the access-log write is synchronous
///
/// `egret-audit` batches through a channel and a background writer, and that is
/// the pattern to reach for IF this shows up in a measurement. It has not been
/// measured, and until it is, the cost of the simple version is one insert on a
/// path that has already done a network round trip to an engine, while its
/// benefit is that a row cannot be lost to a shutdown between the response and
/// the flush. A dropped access-log row is the kind of gap nobody notices until
/// an incident needs it.
///
/// The insert happens AFTER `duration_ms` is computed, so the recorded duration
/// is what the caller experienced and not what the bookkeeping cost.
///
/// # The one thing that is not read off the wire
///
/// `trial` is an EXTENSION, so it is present only when an in-process management
/// handler ([`crate::try_it`]) built this request. A header could be sent by
/// anyone, and this path would then be trusting a caller's word about whether
/// their own call was a trial; nothing outside the process can insert an
/// extension. `Option<Extension<_>>` rather than a raw `parts.extensions.get`
/// so absence is a typed value rather than a lookup that reads the same whether
/// the extension is missing or the type has drifted.
async fn serve(
    State(st): State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
    trial: Option<Extension<TrialActor>>,
) -> Response {
    // One clock reading for the whole request, so the rate limit, the cache and
    // the recorded duration all agree about when "now" was. Monotonic, so an NTP
    // step cannot make a cache entry immortal, refill every bucket at once, or
    // record a negative duration.
    let started = Instant::now();
    let request_id = Uuid::new_v4().to_string();
    let mut entry = AccessLogStore::entry(&request_id, &headers);

    // Recorded BEFORE `run`, because `run` is what may fail to resolve an
    // endpoint — and a row with no endpoint is precisely the row these two
    // columns exist for. Populating them afterwards would mean the failure path
    // had to remember to do it.
    entry.request_method = Some(method.to_string());
    entry.request_path = Some(path.1.clone());

    // Present only when an in-process management handler built this request; a
    // real consumer cannot insert an extension, so this cannot be forged. Set
    // BEFORE `run`, like the two fields above, so a trial refused at step 1 is
    // still recorded as a trial rather than counted as consumer traffic.
    if let Some(Extension(TrialActor(actor))) = trial {
        entry.actor_user_id = Some(actor.0);
    }

    let outcome = run(
        &st,
        path,
        query,
        &method,
        &headers,
        started,
        &request_id,
        &mut entry,
    )
    .await;

    // ONE reading, feeding both the row and the histogram. They are documented
    // as the same measurement (`serving::metrics::REQUEST_DURATION_SECONDS`) and
    // were two separate `started.elapsed()` calls, which made that documentation
    // false by however long the rendering between them took.
    //
    // Measured before the log write, and saturating rather than wrapping: a
    // request that somehow ran for 25 days should record `i32::MAX`, not a
    // negative duration that poisons every average computed over the column.
    let elapsed = started.elapsed();
    entry.duration_ms = elapsed.as_millis().min(i32::MAX as u128) as i32;

    // The tuple carries what the metrics need OUT of the match rather than
    // re-deriving it below: `wire` is consumed by `into_response`, and reading
    // the code back off `entry.error_code` would mean allocating a `String` per
    // request and then labelling with a `&str` whose value set is no longer
    // pinned by the type — the exact thing `serving::metrics` forbids.
    let (response, metric_status, metric_code) = match outcome {
        Ok(served) => {
            entry.status_code = StatusCode::OK.as_u16() as i32;
            // The TOTAL across statements, which is what one integer column can
            // hold. `meta` reports the per-statement split; the access log and
            // the line below want one number, and say so.
            let total = served.response.meta.total_row_count();
            entry.row_count = Some(total as i64);
            entry.cache_hit = served.response.meta.cached;
            // Fires only once the response actually EXISTS. It used to fire
            // before `shape_rows`, so a drift or naming-collision 503 was logged
            // as served — the one line an operator would read to decide the call
            // was fine said so about a call that failed.
            tracing::debug!(
                %request_id,
                endpoint_id = ?entry.endpoint_id,
                version = ?entry.version,
                client_id = entry.client_id.as_deref().unwrap_or(""),
                row_count = total,
                cached = served.response.meta.cached,
                duration_ms = entry.duration_ms,
                "data-api request served"
            );
            (
                served.into_response(),
                StatusCode::OK,
                serving_metrics::OK_CODE,
            )
        }
        Err(e) => {
            let wire = to_wire(&e);
            entry.status_code = wire.status.as_u16() as i32;
            entry.error_code = Some(wire.code.to_string());
            // THE ONLY place the internal detail is allowed to appear. `?e`
            // renders the variant and, for `Internal`, its source — which is
            // exactly what the body must not carry.
            tracing::warn!(
                %request_id,
                code = wire.code,
                status = wire.status.as_u16(),
                error = ?e,
                endpoint_id = ?entry.endpoint_id,
                client_id = entry.client_id.as_deref().unwrap_or(""),
                "data-api request failed"
            );
            let (status, code) = (wire.status, wire.code);
            (wire.into_response(&request_id), status, code)
        }
    };

    // The same answer the access-log row records, counted where an operator can
    // see it without holding a workspace. `metric_status`/`metric_code` came out
    // of the match above rather than being re-derived, so one call is one
    // increment and the counter cannot disagree with the row about what happened.
    // `elapsed` is the reading the row's `duration_ms` was computed from.
    // `Route::Call` named here rather than derived from the URI: this IS the
    // call handler, so the label cannot drift from the route that produced it.
    // Only the bounds, which fire before any handler, have to read the path.
    serving_metrics::record_request(
        serving_metrics::Route::Call,
        metric_status,
        metric_code,
        elapsed,
    );

    // Unconditional, INCLUDING the 429 and the 404 — accepted deliberately, not
    // overlooked. The reasoning, because "the rate limiter protects the
    // database" is what a reader will otherwise assume:
    //
    // 1. **It never did.** The limiter is spec §6.2 step 5, and it is keyed on
    //    `endpoint_id` — which only exists once `live_snapshot` has resolved it.
    //    So a refused request has already paid an RSA signature verification and
    //    TWO SELECTs (`get_by_slug`, `live_snapshot`) by the time the bucket
    //    says no. The limiter sheds ENGINE load, which is the expensive thing it
    //    was built for; it has never shed database load and cannot be moved
    //    earlier without giving up the key it limits on.
    // 2. **These are the rows most worth having.** A client hammering an
    //    exhausted bucket and someone walking a workspace's URL space are the
    //    two things an operator most wants to see in this table. Dropping them
    //    to save a write would make the abuse invisible in the one place it is
    //    visible at all.
    // 3. **Sampling is the wrong bound.** Spec §8's statistics — call count,
    //    error rate — are counts. A sampled table answers them wrongly and
    //    silently, which is worse than answering them expensively.
    //
    // If this is ever MEASURED to matter, the fix with a precedent in-tree is
    // `egret-audit`'s `AuditSink` + `AuditWriter`: a bounded channel and a batch
    // writer, trading durability at shutdown for amortised writes. It is not
    // adopted now for the same reason partitioning is not — nothing has been
    // measured, and the simple version cannot lose a row.
    //
    // The write ANSWERS whether a row exists, and the answer is what the metrics
    // are told. That is the whole of the "the counter is the access log's
    // complement" property: there is no second copy of the predicate here to
    // drift from the one inside `record`. It used to be an
    // `if entry.workspace_id.is_none()` written out beside this line, held to
    // `record`'s own `let Some(ws) = … else` by adjacency and a comment — so a
    // third early return added there (skipping trial rows, say, the way
    // `activity` filters them elsewhere) would have left the counter quietly
    // measuring something other than the gap. See `serving::metrics` for what
    // this does and does not guarantee, and for why a failed INSERT gets its own
    // counter rather than moving the pre-auth one.
    let recorded = AccessLogStore::new(st.db.clone()).record(&entry).await;
    serving_metrics::record_access_log_outcome(recorded);
    response
}

/// The request chain proper (spec §6.2). Fills `entry` as each step resolves a
/// field, so a call refused at step 3 still records the workspace and the caller
/// that reached it.
#[allow(clippy::too_many_arguments)]
async fn run(
    st: &DataApiState,
    Path((ws_slug, raw_path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    method: &Method,
    headers: &HeaderMap,
    now: Instant,
    request_id: &str,
    entry: &mut AccessLogEntry,
) -> Result<Served, ServingError> {
    let principal = verify_serving_token(&*st.keys, bearer(headers)?).await?;
    // Package 7's version pin, parsed AFTER authentication. A malformed header
    // from a caller who has proved nothing is still a 401: a 400 that only this
    // header can produce would be a free "this host speaks the Data API" oracle,
    // which is the same reasoning behind the single `Unauthorized` above it. See
    // `super::pin` for what the parse accepts.
    let pin = super::pin::requested_version(headers)?;
    // Recorded the moment it is known, so every answer from here on — including
    // a 404 — is attributable to a workspace and a client.
    entry.workspace_id = Some(principal.workspace_id);
    entry.service_account_id = Some(principal.user_id.0);
    entry.client_id = Some(principal.client_id.clone());

    // axum percent-decodes a `{*path}` capture and hands it over WITHOUT a
    // leading slash; stored paths were never encoded and P3 stores them
    // canonical (one leading slash, no trailing one). Comparing the raw capture
    // to the stored column therefore misses every time, and the endpoint is
    // unreachable forever with nothing reporting why. `normalize_path` is the
    // one function that decides what a canonical path is, on both sides.
    //
    // A path that FAILS the rule is a 404, not a 400: it cannot match any stored
    // row, and a caller must not be able to tell "malformed" from "absent".
    let path = crate::endpoints::normalize_path(&raw_path).map_err(|_| ServingError::NotFound)?;

    // The workspace comes from the TOKEN; the slug in the path must agree.
    // Disagreement is 404, never 403 — see the module comment.
    //
    // `status` is checked here too, and it is NOT redundant with P2's
    // `ClientStore::verify`. That refuses to MINT a token for an archived
    // workspace; it cannot recall one already issued, and the design accepts up
    // to `exp` of exposure for a stateless token. The workspace row is already
    // in hand here, so closing the archive case on the serving path costs one
    // comparison — and archiving is the operator's most natural decommission
    // action, so it is the case most likely to be relied on.
    let ws = st
        .workspaces
        .get_by_slug(&ws_slug)
        .await
        .map_err(ServingError::internal)?
        .filter(|w| w.id.0 == principal.workspace_id && w.status == "active")
        .ok_or(ServingError::NotFound)?;

    // `live_conn` rides in on this same statement — see `live_snapshot`. It is
    // READ here, at step 2, and ACTED ON below, after the invoke grant and the
    // limiter: nothing between the two consults it.
    let routed = live_snapshot(&st.db, ws.id.0, method.as_str(), &path, pin)
        .await?
        .ok_or(ServingError::NotFound)?;
    let endpoint_id = routed.endpoint_id;
    // From here every failure is attributable to a specific published version,
    // which is what makes "this endpoint started 503ing at 14:02" answerable
    // from the log alone.
    //
    // The version recorded is the one that will ANSWER, so a pinned call records
    // the pin and not the live number — which is what makes
    // `access_log.version < live version` the query an operator runs to find who
    // is still on an old contract before deciding to retire it. A pin that could
    // not be served records no version at all: nothing answered.
    entry.endpoint_id = Some(endpoint_id);
    entry.version = routed.pinned.as_ref().map(|(v, _, _)| *v);

    // Step 4 — the authorization check. Authentication told us WHO; this is the
    // first thing on the chain that asks WHETHER. Without it a `data-api` token
    // is a workspace-wide read capability over every published endpoint's result
    // set, and service-account granularity collapses to workspace granularity —
    // see the module comment.
    //
    // **Denial is 403, not 404**, deliberately the opposite of steps 2 and 3
    // above. By this point the caller has proved it holds a valid, unexpired
    // credential for THIS workspace, so "this endpoint exists and you may not
    // call it" tells it nothing it could not already enumerate. Up there a 404
    // hides existence from a caller who has proved nothing; here there is
    // nothing left to hide, and a 404 would send the operator hunting for an
    // endpoint someone had supposedly deleted.
    //
    // **This runs BEFORE the limiter, so a `not_granted` refusal is unlimited**,
    // and that is spec §6.2's step order rather than an oversight. A valid token
    // hammering an endpoint it was never granted costs two SELECTs per refusal
    // (`get_by_slug`, `live_snapshot`) plus one PDP decision, with no bucket to
    // stop it. It is the same exposure the step-2 404s already carry for the same
    // structural reason: the limiter is keyed on `endpoint_id`, which only exists
    // once step 2 has resolved it, so nothing before step 2 can be limited at
    // all and steps 3 and 4 sit in that same window. **Do not reorder this to
    // fix that** — moving the limiter above step 4 would mean spending a
    // published per-endpoint allowance on callers who may not use the endpoint,
    // which lets one ungranted client exhaust a granted one's budget.
    //
    // If the 403 storm is ever the thing that matters, the fix is a limiter
    // keyed on the PRINCIPAL rather than on the endpoint, applied where step 1
    // finishes — a different bucket with a different key, not this one moved.
    // The access log records every refusal (see `serve`), so the storm is
    // visible before it is expensive.
    invoke_allowed(st, &principal, ws.id, endpoint_id).await?;

    // Step 5 — rate limit. BOTH published keys: the client's own allowance and
    // the endpoint-wide one. Keyed on `endpoint_id`, never on the path.
    //
    // The two refusals are answered with two different codes, which is the whole
    // reason `Refusal` has two variants. `Exhausted` is the caller's own doing
    // and logs at `debug`, because a flood of them is exactly what a working
    // limiter produces. `Unavailable` is egret's dependency failing and logs at
    // `warn`, because it means requests are being refused that nobody's
    // allowance had anything to do with — see `serving::shared` for why the
    // refusal is nevertheless the right answer.
    // `routed.live_rate_limit`, NOT the served snapshot's. One endpoint has one
    // bucket (the keys carry no version), so the rate that fills it must not be
    // one the caller selects with a header — see [`live_snapshot`]'s doc for the
    // arithmetic. Identical to `snap.rate_limit` on every unpinned request,
    // because there `lv` and `v` are the same row.
    //
    // Applied here, BEFORE the pin is resolved, so a caller hammering a stale pin
    // is limited by the endpoint's published allowance rather than getting an
    // unbounded refusal loop.
    check_request(
        &*st.rate_limits,
        endpoint_id,
        &principal.client_id,
        &routed.live_rate_limit,
        now,
    )
    .await
    .map_err(|refusal| match refusal {
        super::ratelimit::Refusal::Exhausted(retry_after) => {
            tracing::debug!(
                endpoint_id = %endpoint_id,
                client_id = %principal.client_id,
                retry_after_secs = retry_after_secs(retry_after),
                "data-api request rate limited"
            );
            ServingError::RateLimited { retry_after }
        }
        super::ratelimit::Refusal::Unavailable => {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                "data-api rate limit could not be checked; refusing the request rather than \
                 admitting it without an allowance"
            );
            ServingError::LimiterUnavailable
        }
    })?;

    // Package 7 — the pin, resolved. Placed HERE rather than at step 2, and the
    // position is two decisions:
    //
    // * **After the invoke grant.** Refusing earlier would tell a caller holding
    //   a valid workspace token but no grant on this endpoint that the endpoint
    //   exists, which steps 2 and 3 spend two 404s to avoid. By this line the
    //   caller has been granted invoke on this very endpoint, so the endpoint's
    //   existence is not a secret the answer could keep — the same reversal
    //   `NotGranted` documents one step up.
    // * **After the limiter**, so a client looping on a stale pin is bounded by
    //   the endpoint's published allowance. The limiter needed no snapshot to
    //   run, because its rate comes from the live version.
    //
    // The log line names the live version, which the wire body deliberately does
    // not: the operator reading it is the person who decides whether to keep the
    // old version around, and "someone is pinned to 3, live is 7" is the whole
    // of what they need.
    let Some((version, snap, live_conn)) = routed.pinned else {
        tracing::debug!(
            %endpoint_id,
            requested = ?pin,
            live_version = routed.live_version,
            client_id = %principal.client_id,
            "data-api call refused: the pinned version is not servable"
        );
        return Err(ServingError::VersionUnavailable);
    };

    // Step 6 — the cache.
    //
    // The `shared` re-check runs BEFORE the `enabled` test and regardless of it:
    // `check_cache_scope` refuses a shared scope at publish time whether or not
    // the cache is currently on, and a serving path that only noticed the drift
    // while caching would report it the moment someone flipped `enabled` rather
    // than the moment the configuration became unsafe.
    // Before anything is keyed, cached or rendered: a version that declares
    // parameters and predates the publish-time gate is refused outright. Every
    // rule spec §5 added lives at publish, so serving such a snapshot means
    // rendering a definition no rule ever looked at — see
    // [`ServingError::ParamGateStale`]. Placed above the cache lookup so a warm
    // entry cannot answer for it either.
    if !snap.params.is_empty() && snap.param_gate < egret_dataapi_api::PARAM_GATE {
        return Err(ServingError::ParamGateStale);
    }
    // Package 1. BEFORE the cache lookup, for the reason every check on this
    // path is before it: a warm entry would otherwise keep answering with
    // governed rows read under the service account for the whole of its TTL.
    // Evaluated against the LIVE row rather than compared with a frozen witness
    // — see `serving::governance` for why the shared-cache shape would stay
    // silent in the exact case that matters.
    governance_allows_serving(live_conn)?;
    let shared = shared_cache_allowed(live_conn, &snap)?;
    // The list is never empty — `EndpointSnapshotRepr` refuses a snapshot that
    // carries neither `statements` nor the pre-M2 `sql`+`columns` pair, and
    // `params::resolve` below refuses an empty list a second time. Checked here
    // anyway because everything after this point is a LOOP over the list, and a
    // loop over nothing does not fail: it produces a `200` carrying no statement
    // at all, which is the one answer this endpoint may never give.
    if snap.statements.is_empty() {
        return Err(ServingError::internal(
            "published snapshot carries no statements",
        ));
    }

    // Step 7's first half, and it runs BEFORE the cache lookup on purpose. The
    // key has to describe what this request will RENDER, not what it asked for
    // in the URL — see `params`' module comment. It also means a request that is
    // simply invalid (a missing required parameter) is a 400 whether or not an
    // entry happens to be warm, which is what makes the answer a property of the
    // request rather than of the cache's state.
    let resolved = params::resolve(raw_query.as_deref(), &snap)?;
    // The principal is in the key unless a re-checked `shared` says otherwise —
    // Ranger filters per principal, so this is the difference between a cache
    // and a cross-identity leak.
    let key = cache_key(
        endpoint_id,
        version,
        &resolved.key,
        (!shared).then_some(principal.user_id),
    );

    // `cache_ttl` is the single decision about whether this endpoint caches at
    // all: `enabled: false`, a `ttl_seconds` of 0, and a database row that is
    // GONE all answer `None`. The third is what keeps `governance`'s
    // missing-row reasoning honest — see that function's doc.
    let ttl = cache_ttl(live_conn, &snap);
    // Counted INSIDE this `if`, so an endpoint with caching off contributes
    // neither a hit nor a miss. Counting its every success as a miss would put a
    // 0% hit rate on a dashboard for an endpoint that has no cache to miss.
    if ttl.is_some() {
        if let Some(hit) = st.cache.get_at(&key, now).await {
            // The LENGTH is checked before the counts are zipped onto the
            // statement list. `version` is in the cache key, so an entry can only
            // belong to this frozen list and the two can only disagree if that
            // invariant is broken — but a `zip` that silently truncates would
            // then drop a statement from `meta` under a `200`, which is the one
            // answer this path may never give. A mismatch re-executes.
            if hit.row_counts.len() == snap.statements.len() {
                serving_metrics::record_cache_lookup(true);
                // The id comes from the caller, not from a fresh
                // `Uuid::new_v4()`. A hit that minted its own would be
                // uncorrelatable with the access-log row describing the very
                // same call.
                return Ok(Served {
                    envelope: envelope_of(&snap.output),
                    response: ServingResponse {
                        status: ServingStatus::mirroring(StatusCode::OK),
                        message: None,
                        // One clone per hit. The alternative is `Arc` in the
                        // response type, which needs serde's `rc` feature and
                        // leaks a caching detail into the public wire type.
                        data: (*hit.data).clone(),
                        meta: ServingMeta {
                            version,
                            cached: true,
                            request_id: request_id.to_string(),
                            contract: meta_contract(&snap, &hit.row_counts),
                        },
                    },
                });
            }
            tracing::warn!(
                cached = hit.row_counts.len(),
                published = snap.statements.len(),
                "a cache entry describes a different number of statements than \
                 the version it was keyed under; re-executing"
            );
        }
        // Every path that leaves this block without having RETURNED a cached
        // body re-executes, so all of them are misses — including the entry that
        // was found but unusable. An `else` on the `if let` would have counted
        // that one as neither.
        serving_metrics::record_cache_lookup(false);
    }

    // Step 7's second half, once per statement. What is sent to the engine is
    // NOT `stmt.sql`: every `{{ … }}` in the definition has become a `?`, and
    // `binds` carries the values in the order those markers appear. This is the
    // line that makes `?region=EU` and `?region=US` return different rows —
    // until it landed they returned the same rows, which is the reason P4.5
    // exists.
    //
    // Rendering happens after the cache lookup because a hit needs no statement,
    // and it is the only work on this path that a hit gets to skip.
    //
    // **EVERY snapshot goes through minijinja now, including one with no
    // parameters at all.** A P4 endpoint whose SQL happens to contain `{%` or
    // `{#` — a JSON path, a literal brace in a string — parsed as nothing before
    // this phase and is a template today, so it can now fail to render and
    // answer 500 where it used to answer 200. The `param_gate` stamp does NOT
    // cover this case: it is only consulted when the snapshot declares
    // parameters, and this one declares none. Rendering unconditionally is still
    // the right call — a `params.is_empty()` shortcut would mean the statement a
    // request executes depends on a field that is not the statement, which is
    // the shape of every bug this phase fixed — but the exposure is real, it is
    // a re-publish away from being visible, and it belongs in the release note
    // rather than in a comment nobody reads after the incident.
    let limits = execution_limits(&snap);
    let cap = limits.max_rows as u32;

    // **Sequentially, one `execute_as_with_params` each** (spec §4.3). Not
    // concurrently: N statements are N reads on ONE engine connection budget,
    // and the ordering is the author's, stated by the list they published.
    //
    // `limits` is CLONED per statement rather than shared, decremented, or
    // divided. `max_rows` is a cap on what one statement may return, so a
    // `total` statement answering one row must not spend the `rows` statement's
    // budget — and the direction that matters is the quiet one: a shared budget
    // does not fail, it TRUNCATES, and a truncated read is a `200` whose client
    // believes it has every row.
    //
    // §4.5 is the promise that goes with this loop and it is deliberately weak:
    // the statements share no transaction and no snapshot, so a total and the
    // rows it counts may disagree if the table moves between them. Task 9 puts
    // that sentence in the generated document, which is the whole of it.
    let mut shaped: Vec<(&str, serde_json::Value)> = Vec::with_capacity(snap.statements.len());
    // One count per statement, in the published order, because `meta` reports
    // them per statement (spec §4.4) and the cache stores them that way.
    let mut row_counts: Vec<u64> = Vec::with_capacity(snap.statements.len());
    // Computed ONCE, outside the loop, and identical to `&snap.output` unless
    // this endpoint has a transform — see [`shaping_output`].
    let shaping = shaping_output(&snap.output);
    // Package S: the SQL each statement will render, generated HERE for a
    // scripted statement and borrowed from the snapshot for a written one. After
    // the cache lookup, so a hit generates nothing; before the loop, so a
    // request spends one blocking thread rather than one per statement. See
    // [`statement_texts`] for both measurements.
    let texts = statement_texts(&snap.statements, &resolved, ScriptDeadline::default()).await?;
    for (stmt, text) in snap.statements.iter().zip(&texts) {
        // **The same call, for both kinds.** Generated text goes through the
        // render a hand-written statement goes through, which is the whole
        // safety argument of package S: `{{ region }}` is still `{{ region }}`
        // when the script hands it back, so the caller's value becomes a `?` and
        // travels beside the statement as a bind exactly as it always did. A
        // second render for generated SQL — or an interpolation instead of this
        // one — would be the defect the design exists to prevent.
        let (sql, binds) = crate::render::render(text, &resolved.values)
            .map_err(|e| render_error(e, resolved.has_repeats))?;

        let out = st
            .query
            .execute_as_with_params(
                ws.id.0,
                snap.database_id,
                principal.user_id,
                &sql,
                &binds,
                limits.clone(),
            )
            .await
            .map_err(engine_error)?;

        // `truncated`, never `row_count == cap`: the connector fetches `cap + 1`
        // and sets the flag, so a count equal to the cap cannot distinguish
        // "exactly at" from "over". Returning a truncated 200 is the worst
        // available failure mode for a data API — the client believes it has
        // every row.
        //
        // The caller is told the cap and nothing else, as before. The STATEMENT
        // is logged, because with several of them the cap alone does not say
        // which one to narrow.
        if out.truncated {
            tracing::warn!(
                statement = %stmt.name,
                cap,
                "a statement exceeded the endpoint's row cap"
            );
            return Err(ServingError::RowLimitExceeded { cap });
        }

        // Shaped against THIS statement's frozen columns — see `shape_rows`,
        // which is handed the statement rather than the snapshot precisely so
        // that it cannot reach a different one's list.
        //
        // A refusal here refuses the WHOLE call, including the statements that
        // already succeeded: their rows are discarded rather than served beside
        // a partial failure. An endpoint that answers with three of its four
        // result sets and no error is a client silently reading a hole.
        shaped.push((stmt.name.as_str(), shape_rows(&out, &shaping, stmt)?));
        row_counts.push(out.row_count);
    }

    // Shaped BEFORE it is cached, and the shaped form is what is stored: shaping
    // is where `output.shape`/`naming` and the drift refusal apply, so caching
    // the raw result would let a hit and a miss disagree about whether the
    // endpoint had drifted. The cached value is the WHOLE response body, so an
    // endpoint's several statements are cached and served together or not at
    // all.
    //
    // Nothing is logged as SERVED before this line. `shape_rows` is the last
    // thing that can refuse the call, and the "served" line used to sit above it
    // — so a drift or naming-collision 503 produced a log saying the request had
    // been served with N rows. That line now lives in `serve`, on the Ok arm.
    //
    // A transform replaces `data` here, and it is the LAST thing that can refuse
    // the call. Everything above still ran: the rows were shaped against each
    // statement's frozen contract, so a drifted endpoint is a 503 whether or not
    // it has a script, and a script never sees a row the drift check would have
    // refused.
    //
    // **The transformed value is what is cached**, for the same reason the
    // shaped value was: the alternative is a hit and a miss disagreeing about
    // what the endpoint returns. It is safe because everything the script's
    // output depends on is in the cache key or frozen beside it — the script and
    // its `returns` come from `version`, the parameters from `resolved.key` —
    // and because the value was checked against `returns` BEFORE it got here, so
    // a refused response returns above this line and is never stored.
    let data = match &snap.output.transform {
        None => statements_data(shaped),
        Some(spec) => transformed_data(spec, shaped, &resolved, ScriptDeadline::default()).await?,
    };
    if let Some(ttl) = ttl {
        st.cache
            .put_at(key, Arc::new(data.clone()), row_counts.clone(), ttl, now)
            .await;
    }

    Ok(Served {
        envelope: envelope_of(&snap.output),
        response: ServingResponse {
            status: ServingStatus::mirroring(StatusCode::OK),
            message: None,
            data,
            meta: ServingMeta {
                version,
                cached: false,
                request_id: request_id.to_string(),
                contract: meta_contract(&snap, &row_counts),
            },
        },
    })
}

/// What fills `data`: the rows themselves for a ONE-statement endpoint, an
/// object keyed by statement name for several (spec §4.4).
///
/// The asymmetry is deliberate and is why `LEGACY_STATEMENT_NAME` is invisible:
/// every endpoint published before M2 has exactly one statement, so its body is
/// the array its consumers already read, and the synthesized name `main` never
/// appears on the wire.
///
/// No key can be lost to another. Publish refuses two statements of one name
/// (`validate_statements`, re-run against the STORED row at publish for exactly
/// this reason), and a `serde_json::Map` keeps the LAST value for a duplicated
/// key — so a duplicate here would drop a whole result set with no error, the
/// same failure `shape_rows` refuses one level down for columns.
fn statements_data(mut shaped: Vec<(&str, serde_json::Value)>) -> serde_json::Value {
    if shaped.len() == 1 {
        return shaped.pop().expect("length just checked").1;
    }
    serde_json::Value::Object(
        shaped
            .into_iter()
            .map(|(name, data)| (name.to_string(), data))
            .collect(),
    )
}

/// The `output` a request's rows are SHAPED against.
///
/// `&snap.output` unchanged for every endpoint without a transform, which is the
/// whole corpus published before C2 — so nothing about an untransformed
/// response moves by one byte.
///
/// # With a transform, `shape` and `naming` are inert, and the script sees the
/// canonical rows
///
/// Those two knobs describe how **`data`** is written. On a transformed endpoint
/// `data` is whatever the script returned, described by `returns`; the rows never
/// reach the wire at all. What they reach is the script — and publish's probe
/// hands it `{column name: value}` objects built straight from the described
/// columns (`lifecycle::probe_row`). If serving applied `naming: "camel"` here,
/// a script probed against `total_revenue` would be called with `totalRevenue`
/// and fail on every real request having passed publish green. That is the exact
/// publish/serving disagreement `check_returns` is one function to avoid, one
/// level in.
///
/// So a transformed endpoint is shaped `objects`/`as_is`, and the rest of
/// `shape_rows` is untouched: containment, `required`, the type comparison, the
/// row-arity guard, the live-duplicate refusal and the zero-column refusal all
/// still run, because all six are properties of the frozen CONTRACT rather than
/// of the two presentation knobs. Two clauses do not run, and both are
/// presentation:
///
/// * the `arrays` path's ORDER-sensitive equality. Order matters to a positional
///   wire row; a script is handed named objects, so there is no position for a
///   reorder to move. Every NAME and TYPE clause still applies, via the
///   `objects` path;
/// * the `naming` collision refusal, which is vacuous under `as_is` —
///   `apply_naming` is the identity, and publish already refuses two frozen
///   columns of one name. Nothing can fold together, so there is nothing to
///   refuse.
fn shaping_output(output: &OutputSpec) -> Cow<'_, OutputSpec> {
    if output.transform.is_none() {
        return Cow::Borrowed(output);
    }
    Cow::Owned(OutputSpec {
        shape: "objects".into(),
        naming: "as_is".into(),
        // Carried rather than defaulted. `shape_rows` reads neither of the last
        // two fields, so this changes nothing today — and a value that is simply
        // WRONG in a struct nobody reads yet is how it becomes wrong in the one
        // that does.
        envelope: output.envelope.clone(),
        transform: None,
    })
}

/// The rows one statement contributes to a script's `statements` dict.
///
/// [`shape_rows`] returns a JSON array on every `Ok` path, so the other arm is
/// unreachable — and it is a `500` rather than an `expect`, because the cost of
/// being wrong about "unreachable" inside a request handler is a panicking
/// worker rather than a logged fault.
fn rows_of(name: &str, shaped: serde_json::Value) -> Result<Vec<serde_json::Value>, ServingError> {
    match shaped {
        serde_json::Value::Array(rows) => Ok(rows),
        other => Err(ServingError::internal(format!(
            "statement `{name}` shaped to {} rather than an array of rows",
            match other {
                serde_json::Value::Object(_) => "an object",
                serde_json::Value::Null => "null",
                _ => "a scalar",
            }
        ))),
    }
}

/// Run an endpoint's transform for THIS request, under the request's own
/// deadline, and answer with what it returned.
///
/// Off the async worker and onto a blocking thread, with
/// [`SCRIPT_REQUEST_DEADLINE`] over the join handle — see that constant for
/// the measurement both halves come from, and for what a deadline can and cannot
/// bound when the work it is waiting on is uncancellable.
///
/// The `statements` dict is keyed **even at N = 1**, matching `run_transform`'s
/// contract and publish's probe: a script that has to be rewritten when its
/// author adds a second statement is a trap, so the shape does not change with
/// the count. Note that this is the one place where `data` and the statement
/// keying come apart — `statements_data` unwraps a single statement's rows onto
/// the wire under no name, and the script never sees that unwrapping.
async fn transformed_data(
    spec: &TransformSpec,
    shaped: Vec<(&str, serde_json::Value)>,
    resolved: &params::Resolved,
    deadline: ScriptDeadline,
) -> Result<serde_json::Value, ServingError> {
    let mut statements: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for (name, value) in shaped {
        statements.insert(name.to_string(), rows_of(name, value)?);
    }

    // Cloned into the task because `spawn_blocking` needs `'static`, and the
    // rows are MOVED rather than cloned: `statements_data` is not called on this
    // path, so nothing above needs them again. The clone is the script and its
    // `returns` — bounded by what publish accepted, and once per request.
    let spec = spec.clone();
    let params = resolved.json.clone();
    let has_repeats = resolved.has_repeats;
    run_under_deadline(deadline, ScriptPhase::Transform, move || {
        transform_response(&spec, &params, &statements, has_repeats)
    })
    .await
}

/// The SQL every statement of this request will render — the author's own text,
/// or the text their script built from THIS caller's parameters (package S).
///
/// # Where generation sits, and what a cache hit pays
///
/// It is called from [`run`] AFTER the cache lookup and before the execution
/// loop, in the position `render` already occupied and for the same reason: a
/// hit needs no statement, so it needs no generation either. Generated SQL is a
/// pure function of the resolved parameters (`build_sql` sees `params` and
/// nothing else — [`crate::transform::build_sql`]) and the resolved parameters
/// are the cache key, so a hit could only ever regenerate what it already has.
/// **A hit runs no starlark at all**, which is what
/// `a_cache_hit_serves_without_running_the_script` measures by making the script
/// unrunnable between the two calls.
///
/// # On a blocking thread, and the case is stronger than C2's
///
/// `build_sql` is the same `eval_inner` under the same `Budgets::default()` as a
/// transform, so it carries the same measured tail: **39.9 s against a 2 s
/// budget** for a tick-expensive script (measured for `build_sql` itself, not
/// inherited), because `MAX_WALL_CLOCK` is polled every 1000 ticks rather than
/// being a latency bound. C2 moved the transform off the async worker for
/// exactly that, and generation runs on **every served miss** of a scripted
/// endpoint rather than only on endpoints that declare a transform. Inline, one
/// such script would hold a tokio worker thread — and every unrelated request
/// scheduled on it — for that whole time.
///
/// An ordinary script costs **542 µs** (debug build, same measurement), which is
/// what a miss actually pays; the number above is the tail the deadline exists
/// for, not the cost.
///
/// # ONE evaluation task for the request, not one per statement
///
/// Every scripted statement is generated inside a single `spawn_blocking`, so a
/// request holds at most one blocking thread for generation and a request that
/// times out detaches at most one. The alternative — a `spawn_blocking` per
/// statement, in the execution loop — is what makes the numbers ugly:
/// `MAX_STATEMENTS` is 16, so one request could stack 16 detached threads and
/// wait `16 × deadline` before the engine is touched at all. The bound the
/// deadline states is then a bound on the REQUEST, which is what it claims to
/// be.
///
/// The generations are sequential inside that task, in the published order, like
/// the executions that follow them.
///
/// # A written statement pays nothing
///
/// No script, no clone, no `spawn_blocking`, no await: the loop below hands back
/// borrowed slices of the snapshot and the caller renders exactly the `&str` it
/// rendered before package S existed.
async fn statement_texts<'a>(
    statements: &'a [StatementSnapshot],
    resolved: &params::Resolved,
    deadline: ScriptDeadline,
) -> Result<Vec<Cow<'a, str>>, ServingError> {
    // `(index, script)` rather than `(name, script)`: the answers are paired
    // back onto the statement list by position, so nothing here depends on the
    // names being unique. Publish does guarantee that, and this is one fewer
    // place that has to know.
    let scripts: Vec<(usize, String)> = statements
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.source.script().map(|script| (i, script.to_owned())))
        .collect();
    if scripts.is_empty() {
        return statements
            .iter()
            .map(|s| {
                s.source.sql().map(Cow::Borrowed).ok_or_else(|| {
                    // Unreachable: `scripts` is empty, so every statement's
                    // source is the `Sql` case. Answered rather than
                    // `expect`ed because a panic inside a request handler is a
                    // dropped connection rather than a 500.
                    ServingError::internal("a statement carries neither SQL nor a script")
                })
            })
            .collect();
    }

    // Cloned into the task because `spawn_blocking` needs `'static`. Both are
    // bounded by what publish accepted: the scripts by `MAX_STATEMENTS`, the
    // parameters by `MAX_DECLARED_PARAMS` and `params::MAX_PARAM_BYTES`.
    //
    // `generation`, not `json`: the dict a generating script sees is the one
    // publish probed it with, and the two are different projections for the
    // reason `crate::transform::GenerationParams` gives. `json` does not even
    // type-check here, which is the point of that type.
    let params = resolved.generation.clone();
    // The renderer's context too, because generation is followed by the
    // behavioural gate and that gate is about how these VALUES render — see
    // [`generate_one`]. One clone per request, of a map the request already
    // built.
    let values = resolved.values.clone();
    let mut generated: BTreeMap<usize, String> =
        run_under_deadline(deadline, ScriptPhase::Generation, move || {
            let mut out = BTreeMap::new();
            for (i, script) in &scripts {
                out.insert(*i, generate_one(script, &params, &values)?);
            }
            Ok(out)
        })
        .await?;

    statements
        .iter()
        .enumerate()
        .map(|(i, s)| match s.source.sql() {
            Some(sql) => Ok(Cow::Borrowed(sql)),
            None => generated.remove(&i).map(Cow::Owned).ok_or_else(|| {
                // Unreachable: `i` was put into `scripts` by the same
                // `script().is_some()` test that lands here.
                ServingError::internal("a scripted statement was not generated")
            }),
        })
        .collect()
}

/// Generate ONE statement's SQL and re-apply the two publish rules that nothing
/// downstream would re-apply for it.
///
/// # The asymmetry this exists for
///
/// Publish gates every template it can SEE, and for a script that is the
/// baseline plus whatever the sentinel probes emitted — two values per declared
/// parameter (`lifecycle::GeneratedSql::probe`). A script may branch on a value
/// no probe supplies:
///
/// ```text
/// def build_sql(params):
///     if params["region"] == "XX": return "DELETE FROM t"
///     return "SELECT * FROM t"
/// ```
///
/// publishes green — every template publish saw is a `SELECT` — and emits the
/// other arm to the first caller who asks for `region=XX`. **For a written
/// statement this cannot happen**: publish gated the exact bytes that will be
/// executed. So the rules re-applied here are exactly the ones `render` does not
/// re-apply, and they are applied to GENERATED text only. A written statement is
/// not re-scanned: its bytes are the bytes publish scanned, and re-scanning them
/// under today's rules could 500 an endpoint published green under yesterday's.
///
/// # What `render` DOES re-apply, so this does not
///
/// `check_static`, the placeholder==bind invariant, and `;`/`#` on the rendered
/// output (`check_rendered_statement`). Those hold for any text on any path.
///
/// # The two that it does not, and they fail in different directions
///
/// 1. **[`crate::lifecycle::check_single_select`] — the head word.** Nothing
///    re-tests it, which Task 3's Mutation A measured from the other side: a
///    generated `DELETE FROM t` validated. It is executed with the CALLER's
///    engine credentials on a delegated connection, so leaving it unscanned
///    turns "the author may write SQL" into "the author may run DML as any
///    caller". A miss here is a wrong ACTION.
///
/// 2. **[`crate::render::check_dynamic`] — the behavioural gate.** This is the
///    worse of the two and it is easy to miss, because `check_static` looks like
///    the whole of that rule. It is not: `check_dynamic` exists precisely
///    because that scanner has been bypassed five separate times by five
///    different spellings of one comparison, and `render` calls only the
///    scanner. A generated template carrying a spelling the scanner misses
///    renders a **silently false** comparison, drops the `{% if %}` it guards,
///    and answers `200` with over-broad rows — no error, nowhere. That is the
///    same silent-wrong-answer class this package refused to freeze a generated
///    template for, and it deserves the same treatment.
///
///    A sixth spelling was not hypothetical: `{% if [p, 'x'][0] == 'lit' %}`
///    passes `check_static` today and is caught here, pinned by
///    `render::tests::a_parameter_wrapped_in_a_list_literal_is_caught_only_by_the_gate`.
///
/// `check_dynamic` rather than `check_publish`, because `check_publish` is
/// `check_static` + `check_dynamic` and `render` runs `check_static` a few lines
/// later. Running it twice would report a scanner refusal through this message
/// instead of through `render_error`'s mapping.
///
/// # What it costs, measured, and why it is paid per request
///
/// `check_dynamic` renders the definition once per probe assignment, twice
/// each, capped at `render::MAX_PROBES`. Measured (debug build): **106 µs** for
/// a one-parameter statement with no literals (1 assignment), **9.9 ms** for a
/// four-parameter statement with fourteen harvested literals (57 assignments) —
/// the worst case inside the budget. Its own doc says this is "right at publish
/// and wrong per request", and that judgement is correct for a WRITTEN
/// statement, where publish gated the bytes that will run. It does not transfer
/// to a text no publish has ever seen: the alternative to paying it is serving
/// that text ungated.
///
/// It is bounded, it costs no engine connection, and it is dominated by the
/// query that follows. It runs on the blocking thread beside the evaluation that
/// produced the text, so it is inside the request deadline rather than on an
/// async worker.
///
/// # Every failure is a 500 and none is a 400 — on THIS path
///
/// The caller sent a valid request: authenticated, authorized, inside its rate
/// limit, parameters coerced. What failed is the ENDPOINT. A 400 would blame the
/// caller for the author's script and write `param_invalid` into `access_log`,
/// pointing every later investigation at the wrong half of the system.
///
/// **`crate::preview` is the second caller and inverts that**, because the
/// script is on screen and the author owns it: it reads the sentence out of
/// `Internal`'s source and answers 400 with it. That is why the sentences above
/// name "the validation probes" rather than publish's — a preview reaches this
/// on a draft that has never been published, and an author sent to look at a
/// publish that never happened is sent nowhere.
pub(crate) fn generate_one(
    script: &str,
    params: &crate::transform::GenerationParams,
    values: &crate::render::ParamValues,
) -> Result<String, ServingError> {
    let sql = crate::transform::build_sql(script, params).map_err(|e| {
        ServingError::internal(format!("a statement's script did not build its SQL: {e}"))
    })?;
    // The scan first: it is a byte test, and it refuses a text that is not a
    // read before the gate below spends up to `MAX_PROBES` renders on it.
    crate::lifecycle::check_single_select(&sql).map_err(|e| {
        ServingError::internal(format!(
            "a statement's script built SQL that is not a single read-only \
             statement, on a branch the validation probes never took: {e}"
        ))
    })?;
    crate::render::check_dynamic(&sql, values).map_err(|e| {
        ServingError::internal(format!(
            "a statement's script built SQL that INSPECTS a parameter, on a \
             branch the validation probes never took; running it would silently \
             drop the block that comparison guards and answer with over-broad \
             rows: {e}"
        ))
    })?;
    Ok(sql)
}

/// Run `work` on a blocking thread and stop WAITING for it at `deadline`.
///
/// Generic over the work for one reason, and it is a testing reason stated
/// plainly: with `transform_response` called inline here, the only way to reach
/// the `Elapsed` arm was to give a real starlark script real work to be too slow
/// at — and that work then ran, detached, on a core the rest of the test binary
/// was using. It took `transform::a_wide_shared_fan_out_is_stopped_by_the_output_deadline`
/// from a measured 1-in-12 flake to 3-in-12. A seam here lets the test park a
/// thread on a channel instead, which costs no CPU at all and is not a race.
///
/// Nothing else may use this seam. There are exactly TWO production callers —
/// [`transformed_data`] directly above and [`statement_texts`], package S's
/// generation — and the closures they pass are the only things this crate ever
/// evaluates on a blocking thread.
///
/// Generic over `T` because generation answers with texts rather than with a
/// JSON value. It is one function rather than two for the reason this file
/// gives everywhere else: "spawn it blocking, stop waiting at the deadline,
/// both failures are internal" is ONE rule, and a second copy is a second thing
/// that can be weakened alone. What differs between the two phases is a
/// sentence, and [`ScriptPhase`] is that sentence.
async fn run_under_deadline<T, F>(
    deadline: ScriptDeadline,
    phase: ScriptPhase,
    work: F,
) -> Result<T, ServingError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ServingError> + Send + 'static,
{
    let handle = tokio::task::spawn_blocking(work);
    blocking_outcome(
        tokio::time::timeout(deadline.0, handle).await,
        deadline,
        phase,
    )
}

/// Which author-supplied evaluation a refusal is about.
///
/// The two are indistinguishable to [`blocking_outcome`] except in the words it
/// writes, and those words are the only thing an operator has: both arrive as
/// `internal`, so the wire says nothing and the log line is the whole diagnosis.
/// A generation failure blamed on "the response transform" would send that
/// operator to a script the endpoint may not even have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptPhase {
    Transform,
    Generation,
}

impl ScriptPhase {
    /// The subject of the sentence a refusal writes.
    fn subject(self) -> &'static str {
        match self {
            Self::Transform => "the response transform",
            Self::Generation => "a statement's SQL-building script",
        }
    }
}

/// What [`tokio::time::timeout`] over the join handle means, decided apart from
/// the awaiting so that both refusal arms can be driven with REAL values.
///
/// The three nested `Result`s are exactly what `timeout(_, handle)` yields, and
/// they are deliberately not flattened: `Elapsed` and `JoinError` are different
/// facts about a request and the operator reads them differently, so collapsing
/// them into one would be the second spelling of one outcome this file argues
/// against everywhere else.
///
/// Both are [`ServingError::internal`]. A transform that did not finish is the
/// endpoint failing to answer, not the caller failing to ask — the same reading
/// [`transform_response`] argues for a script that raised, and the same reading
/// package S gives a script that could not build its SQL.
fn blocking_outcome<T>(
    joined: Result<
        Result<Result<T, ServingError>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
    deadline: ScriptDeadline,
    phase: ScriptPhase,
) -> Result<T, ServingError> {
    match joined {
        Ok(Ok(result)) => result,
        // `run_transform` and `build_sql` catch an evaluator unwind themselves
        // (`TransformError::Evaluator`), so this is a panic in the plumbing
        // around it, or a runtime shutting down. Neither is the caller's doing.
        Ok(Err(join)) => Err(ServingError::internal(format!(
            "{} did not complete: {join}",
            phase.subject()
        ))),
        // The WAITING stopped here; the work did not. See the module comment —
        // the blocking task is detached, keeps a pool thread and whatever it was
        // handed, and this answer is what invites the caller to stack a second
        // one on top of it.
        Err(_) => Err(ServingError::internal(format!(
            "{} exceeded the {} ms request deadline; the \
             evaluator's own budget bounds ticks rather than time, so a script \
             whose every tick is expensive can overrun it, and the run this \
             stopped waiting for is still holding a blocking thread",
            phase.subject(),
            deadline.0.as_millis()
        ))),
    }
}

/// Run the script and hold what it returned to what the endpoint DECLARED.
///
/// # This is the guarantee, and publish's probe is not a substitute for it
///
/// `lifecycle::probe_transform` runs the script against one synthetic row per
/// statement and against no rows, and says so. It cannot prove conformance for
/// every future call, because a script's output shape may depend on the DATA —
/// which is the whole reason `returns` is declared by the author rather than
/// inferred. Three ways a probed-green script reaches here with something else,
/// none of them hypothetical:
///
/// * **a branch on the data.** `if row["n"] > 0: return {...}` takes the other
///   arm on Tuesday, and the probe's stand-in value chose one of them;
/// * **a narrower row.** The probe row carries every column of the branch union;
///   `shape_rows` omits a `required: false` column the branch that ran did not
///   return, and a real cell can be `null` where a probe cell never is;
/// * **a parameter in a shape the probe never showed.** See below.
///
/// So every response is checked, by [`crate::transform::check_returns`] — the
/// SAME function publish probes with, deliberately, because a second reading of
/// "does this match `returns`" could accept at publish what serving refuses and
/// the endpoint would publish green and 500 in production.
///
/// # Every failure here is a 500, and none of them is a 400
///
/// The caller sent a valid request: it was authenticated, authorized, inside its
/// rate limit, and its parameters coerced and rendered. What failed is the
/// ENDPOINT — the author's script, or the declaration the author wrote about it.
/// A 400 would blame the caller for that and, worse, write `param_invalid` into
/// `access_log`, so the one table an operator reads to find a broken endpoint
/// would report a client-side mistake instead. `ServingError` gains no variant
/// for this: it is [`ServingError::internal`], exactly like a snapshot that
/// carries no statements or a `default` that does not satisfy its own type, and
/// the detail reaches the operator through `serve`'s `tracing::warn!` while the
/// body stays the fixed `"internal error"`.
///
/// # A repeated query parameter, and why it is not coerced away
///
/// `resolve_supplied_with` binds a parameter as a LIST when the caller supplied
/// it more than once, and as a scalar otherwise — unless a statement iterates it,
/// in which case it is always a list. Publish's probe knows the iterated half
/// exactly (`IteratedParams`, computed from the same SQL) and binds a scalar for
/// everything else. **The gap is a parameter no statement iterates that the
/// request repeated**: the probe showed the author a scalar, and the script is
/// called with a list.
///
/// The gap is narrow, because most of it is already closed one layer up: if any
/// statement uses such a parameter as a scalar (`{{ region }}`), the repeat
/// fails to render as `RenderError::SequenceBind` and the request is a 400
/// before a script runs at all (see [`render_error`]). What is left is a
/// parameter every statement either ignores or only tests for truthiness.
///
/// **Serving binds what the request resolved, unchanged, and does not collapse a
/// repeat to a scalar for the script's benefit.** The script's `params` and the
/// statements' binds are two readings of one request, and the whole point of
/// deriving both from one `as_list` decision in `resolve_supplied_with` is that
/// they cannot say different things about what the caller sent. Collapsing here
/// would hand the script `"EU"` while the SQL filtered on `EU` and `US` — a
/// disagreement that produces a plausible 200 and no error anywhere, which is
/// strictly worse than the fail-closed 500 it replaces.
///
/// So it fails closed, and `has_repeats` is folded into the LOG line rather than
/// into the status: the operator reading a 500 on an endpoint that works for
/// every single-valued call needs the one fact that distinguishes this call from
/// those, and that fact is caller-controlled, so it may inform a message and must
/// never choose a status (the defect [`render_error`] records at length).
fn transform_response(
    spec: &TransformSpec,
    params: &BTreeMap<String, serde_json::Value>,
    statements: &BTreeMap<String, Vec<serde_json::Value>>,
    has_repeats: bool,
) -> Result<serde_json::Value, ServingError> {
    /// The note appended when a repeat could be what the script tripped over.
    /// Author-facing detail for the log, never for a body.
    const REPEAT_NOTE: &str = "; this request supplied at least one parameter more than once, so \
                               a parameter publish probed as a scalar reached the script as a list";

    let note = if has_repeats { REPEAT_NOTE } else { "" };

    let value = crate::transform::run_transform(spec, params, statements)
        .map_err(|e| ServingError::internal(format!("the response transform failed: {e}{note}")))?;

    // The same rule publish probed with, run against the real response. A
    // mismatch is refused rather than served: an undeclared field is a response
    // the published contract and the generated document do not describe, and a
    // missing declared one is a client reading `body["total"]` on a body that
    // has no `total`.
    crate::transform::check_returns(&value, &spec.returns).map_err(|detail| {
        ServingError::internal(format!(
            "the response transform returned a value that does not match the endpoint's \
             declared `returns`: {detail}{note}"
        ))
    })?;

    Ok(value)
}

/// What `meta` reports about the contract, given the snapshot and one row count
/// per statement in the published order.
///
/// The frozen contract, never this execution's shape — see
/// [`MetaContract::Single`]'s `columns`.
///
/// **The two arms are chosen by the SNAPSHOT's statement count, not by the
/// counts handed in**, and that is what makes the shape a property of what was
/// published rather than of what this call happened to return: an endpoint does
/// not stop being a several-statement endpoint because one of its statements
/// returned nothing. `statements_data` branches on the same number one level up,
/// so `data` and `meta` cannot disagree about which of the two shapes this
/// endpoint has.
///
/// # A transform takes the keyed arm whatever its statement count, so that
/// `row_count` is OMITTED
///
/// `meta` describes the SQL. `data`, on a transformed endpoint, is whatever the
/// script returned — commonly a single object. `total_row_count()` still sums
/// the statements, so a top-level `row_count` would be *consistent* with
/// `meta.statements` and still be read by every consumer as "how many things are
/// in `data`", which after a transform it is not. M2 already changed that
/// field's meaning without changing its type; a field that is technically
/// truthful and reliably misread is that hazard a second time, so a transformed
/// endpoint does not publish one (spec's recommendation; the generated contract
/// says so in Task 7).
///
/// The keyed arm is how it is omitted rather than a third variant, because it is
/// exactly the right answer for the rest of `meta` too: a transformed endpoint's
/// SQL is still N statements with N row counts and N frozen column lists, and
/// keying them by name is the only shape that can say which is which. What it
/// gives up is the `data`/`meta` correspondence above — deliberately, and only
/// here: with a transform the two are two different truths, and pretending
/// otherwise is what a top-level `row_count` would do.
///
/// `zip` is safe here because the caller checked the length: the miss path
/// builds `row_counts` in the same loop that walks `snap.statements`, and the
/// hit path compares the lengths and re-executes rather than truncating.
fn meta_contract(snap: &EndpointSnapshot, row_counts: &[u64]) -> MetaContract {
    if snap.statements.len() == 1 && snap.output.transform.is_none() {
        return MetaContract::Single {
            row_count: row_counts.first().copied().unwrap_or(0),
            columns: snap.statements[0].columns.clone(),
        };
    }
    MetaContract::Several {
        statements: snap
            .statements
            .iter()
            .zip(row_counts)
            .map(|(stmt, &row_count)| {
                (
                    stmt.name.clone(),
                    StatementMeta {
                        row_count,
                        columns: stmt.columns.clone(),
                    },
                )
            })
            .collect(),
    }
}

/// Map a [`RenderError`] onto the serving vocabulary.
///
/// # Every one of these messages is AUTHOR-facing
///
/// `RenderError`'s own type comment says so: several of them quote the
/// definition back — a syntax error names the construct and the line — which is
/// the right thing at publish time and context disclosure on a public endpoint.
/// So the default is [`ServingError::Internal`], whose body is the fixed string
/// `"internal error"` and whose detail goes to `tracing` against this request's
/// id. If one ever did leak the exposure would be a construct name and a line
/// number (minijinja's `Display`, not its alternate form) rather than the
/// template source — but that is an argument for the mapping being cheap, not
/// for skipping it.
///
/// # The one caller-shaped failure, and why a heuristic is the honest answer
///
/// A render error is almost always the ENDPOINT's fault: a definition that
/// should never have published, caught here by the serve-time re-check. There is
/// one exception, and it is caller-triggered. A template that uses a parameter
/// as a scalar (`{{ region }}`) renders fine for `?region=EU` and fails for
/// `?region=EU&region=US`, because a repeat resolves to a LIST and the formatter
/// refuses to bind a sequence as one value (see `render::to_param`). The
/// endpoint is not broken — it works for every single-valued call — so a 500
/// would send an operator to debug a definition that is correct, while the
/// caller's fix is to stop sending the duplicate.
///
/// `has_repeats` is therefore consulted — but it is NOT sufficient on its own,
/// and treating it as sufficient was a real defect: it let any caller downgrade
/// every render failure to a 400 at `debug!` by appending one duplicate
/// parameter. `PlaceholderBindMismatch`, `RenderedSeparator`, `CaptureConstruct`
/// and `ParameterInspected` are statements about the DEFINITION; a caller must
/// not be able to decide how loudly they are reported.
///
/// So the 400 needs both halves: a repeat, and the one variant a repeat can
/// cause. That variant is [`crate::render::RenderError::SequenceBind`] — its own
/// variant precisely so this match is not on the text of a message, which is the
/// check that keeps compiling and silently stops working the day someone rewords
/// it (`port_impl`'s `engine_error` carries the same comment).
///
/// [`RenderError::Ident`] is the one variant safe to surface as-is, and it is
/// still mapped rather than echoed: [`super::params::resolve`] already checked
/// every enum value against `allowed` and refused an enum repeat, so a caller
/// cannot reach it. Reaching it means `| ident` was applied to something that is
/// not an enum parameter — an author fault, which publish (Task 7) refuses.
fn render_error(e: crate::render::RenderError, has_repeats: bool) -> ServingError {
    use crate::render::RenderError as R;
    // The 400 is for the ONE caller-triggered failure, and it is now identified
    // by its variant. Consulting `has_repeats` alone let any caller downgrade
    // EVERY render failure to a client error by appending one duplicate
    // parameter: `&region=x&region=x` turned a `PlaceholderBindMismatch` — the
    // backstop nothing has ever bypassed — into a `param_invalid` logged at
    // `debug!`, so the operator saw a client mistake and no warning at all.
    if matches!(e, R::SequenceBind) && has_repeats {
        tracing::debug!(
            error = %e,
            "data-api render failed on a request that repeated a declared parameter"
        );
        return ServingError::ParamInvalid;
    }
    // Everything else is the endpoint's fault or an attack on it, and both are
    // things an operator must see. `warn!` unconditionally — a caller cannot
    // choose the level, and these are exactly the events that must not be
    // silenced by one repeated query parameter.
    tracing::warn!(
        error = %e,
        has_repeats,
        "data-api render failed: the published definition is unsafe to serve"
    );
    ServingError::internal(format!("render endpoint definition: {e}"))
}

/// Spec §6.2 step 4: does this service account hold `data_api_endpoint:invoke`
/// on THIS endpoint?
///
/// # Why `Pdp::authorize` directly and not `egret_core::authz::require_object`
///
/// That helper takes a `WorkspaceContext` — a human's session principal, its
/// group memberships and its workspace — and the serving path has a
/// [`ServingPrincipal`], which is three fields read out of a JWT. Fabricating a
/// `WorkspaceContext` to satisfy the signature would mean inventing values for
/// everything the token does not carry, and the invented parts would be exactly
/// the parts an authorization decision reads. `Pdp::authorize` is the actual
/// decision point; `require_object` is a convenience over it for the surface
/// that has a `WorkspaceContext`.
///
/// # What the [`Subject`] carries, and what it deliberately does not
///
/// `group_ids` is EMPTY, and that is not a shortcut that happens to work — it is
/// what the token supports. A `data-api` JWT carries `egret_user_id` and `ws`
/// and nothing about groups, and resolving them here would be a database read on
/// the hot path that spec §6.2 step 1 exists to avoid.
///
/// It also costs nothing for the decision this function makes, for a reason
/// worth stating because it is not obvious: `AccessGrantStore::grants_for`
/// resolves group membership in SQL (`principal_id IN (SELECT group_id FROM
/// user_groups WHERE user_id = $1)`), so a grant held through a GROUP is still
/// loaded, and `link_grants` binds `?principal` to the user either way. The only
/// consumer of `Subject::group_ids` is the classification `forbid`'s
/// clearance-by-group `unless` clause — a DENY gate. An empty list there means
/// the `unless` cannot be satisfied by group membership, so the gate stays shut.
/// Fail-closed, which is the right direction for the field to be wrong in.
///
/// # `folder_id` is `None`, so a folder grant does not cascade to invoke
///
/// Endpoints carry a `folder_id` and the PDP would cascade a folder-scoped grant
/// to the items inside it, but this passes `None` — only a grant on the endpoint
/// ITSELF can allow the call.
///
/// Endpoint folders ARE creatable now: `egret-bi`'s `kind_perms` gained a
/// `"data_api_endpoints"` arm (see [`crate::endpoints::FOLDER_ENTITY_TYPE`]) and
/// the Endpoints page creates them and moves rows between them. `None` stays
/// anyway, and is now a live choice rather than an unreachable branch: passing
/// the real `folder_id` would let a folder-scoped grant authorize INVOKING every
/// endpoint filed into that folder, including ones moved in later by someone
/// who was only tidying a list. That widens access, so it is a decision for
/// whoever wants it — with a test that pins the cascade — not a fix to apply
/// while wiring the UI. Reading the folder would also cost a second query per
/// served request.
async fn invoke_allowed(
    st: &DataApiState,
    principal: &super::ServingPrincipal,
    ws: egret_core::WorkspaceId,
    endpoint_id: Uuid,
) -> Result<(), ServingError> {
    // The expression itself now lives in `crate::invoke` — the serving path is
    // one of three callers that must agree, see that module's comment.
    let allowed = crate::invoke::can_invoke(st, principal.user_id, ws, endpoint_id)
        .await
        // A PDP that could not answer is OUR fault, and it must not read as a
        // refusal: `Internal` (500) sends an operator to egret, `NotGranted`
        // (403) would send them to write a grant that already exists.
        .map_err(ServingError::internal)?;
    if allowed {
        Ok(())
    } else {
        tracing::debug!(
            %endpoint_id,
            service_account_id = %principal.user_id.0,
            client_id = %principal.client_id,
            "data-api call refused: no invoke grant on this endpoint"
        );
        Err(ServingError::NotGranted)
    }
}

/// Resolve an inbound request to a live snapshot.
///
/// Routing comes from `dataapi.endpoints (method, path)` — the pair carrying
/// `UNIQUE (workspace_id, method, path)` — restricted to `state = 'live'` and the
/// token's workspace. CONTENT comes only from `v.snapshot`.
///
/// The `state = 'live'` predicate is what makes a DISABLED endpoint a 404: the
/// row and its `live_version_id` both survive a disable (that is the point — an
/// enable must restore the same version), so the join alone would still find it.
/// # The `LEFT JOIN` carries the live connection, and costs nothing
///
/// `d.governed, d.delegated, d.options` are the three columns
/// [`governance_allows_serving`] and [`shared_cache_allowed`] decide from, and
/// they ride in here rather than in a `SELECT` of their own: same statement,
/// same round trip, and the snapshot and the connection are then read at ONE
/// moment. See [`super::governance`]'s Cost section for why the round trip
/// mattered enough to fold.
///
/// `LEFT`, so a DELETED database still routes: `governed` comes back `NULL`, the
/// column is `NOT NULL` in the schema, and [`LiveConnection::from_row`] reads
/// that as "the row is gone" — a state both readers answer deliberately, and
/// differently. An INNER join would instead turn a deleted connection into a
/// 404, hiding the one fact that explains everything the operator is seeing.
///
/// The join key is the SNAPSHOT's `database_id`, not `e.database_id`. They are
/// usually the same and are allowed to differ: `e.database_id` follows the
/// DRAFT, so an author who re-points the picker without publishing would
/// otherwise have the safety of a live version decided by a connection that
/// version never used. The `::uuid` cast can only fail on a snapshot `publish`
/// did not write, which is the same corruption the `from_value` below answers
/// with a 500 — the `map_err` on the query says so, because Postgres' own
/// `22P02` names neither the endpoint nor the field.
///
/// # `d.workspace_id = $1`, on a key that came out of a snapshot
///
/// It is the same predicate [`crate::lifecycle::EndpointStore::connection_safety_opt`]
/// carries, and it is here for the same reason: `database_id` is the worst
/// column in this crate to resolve unscoped, because what it opens is a
/// CREDENTIAL — a foreign row would hand this request another workspace's
/// engine connection to decide safety from. Nothing published can reach that
/// state today (publish and rollback both read the connection through
/// `connection_safety_opt`, which is scoped, so a foreign id never enters
/// `endpoint_versions`), so this is insurance rather than a fix — and this repo
/// has already shipped one cross-workspace `database_id` leak of exactly this
/// shape.
///
/// It does not fail open. A foreign — or legacy `workspace_id IS NULL` — row
/// simply does not match, so `governed` comes back `NULL` and both readers take
/// the row-is-gone branch: [`governance_allows_serving`] allows, and
/// [`crate::serving::cache_ttl`] answers `None` so nothing is served from or
/// written to the cache. What is left is execution, and that is where the
/// refusal lands: `execute_as_with_params` resolves through `connect_as`, which
/// calls `get_in_workspace` and reports a foreign id as `NotFound` before any
/// connection is opened. Strictly tighter than without the predicate, which
/// could serve such a request from a warm cache entry.
///
/// # The version pin changes ONE clause, and that is the whole security argument
///
/// Package 7 lets a consumer name the version it built against. The pin replaces
/// exactly one term of this statement — which `endpoint_versions` row `v` binds
/// to — and touches nothing else:
///
/// * `e.workspace_id = $1`, `e.method`, `e.path` and `e.state = 'live'` all
///   still decide whether there is a row at all. A pin therefore cannot reach
///   another workspace's endpoint, a **disabled** one, a **trashed** one
///   (migration 0126's CHECK makes `deleted_at IS NOT NULL` and `state = 'live'`
///   mutually exclusive), or one in an archived workspace — the caller's `ws`
///   filter one layer up in [`run`] having already refused that.
/// * `v.endpoint_id = e.id` is what makes a version number from ANOTHER endpoint
///   unreachable. Version numbers are per endpoint
///   (`UNIQUE (endpoint_id, version)`), so this is a structural property of the
///   join rather than a check someone has to remember.
/// * `v.version <= lv.version` refuses a pin ABOVE the live version. The only way
///   a version can be above live is that `rollback` moved consumers off it, and
///   that is the one act available to an operator during an outage — serving it
///   because a caller asked would make the rollback defeatable by a header.
/// * The `LEFT JOIN query.databases` keys on the **pinned** snapshot's
///   `database_id` and reads the **live** row, so [`governance_allows_serving`]
///   and [`shared_cache_allowed`] answer about the connection as it is right now.
///   Package 1 chose the live-predicate shape over a frozen witness precisely so
///   that marking a database `governed` takes effect on already-published
///   versions; a pin does not opt out of that, because there is nothing frozen
///   for it to opt out of.
///
/// Everything the pin does NOT change is therefore live by construction rather
/// than by a rule this function has to restate: the `data_api_endpoint:invoke`
/// check runs on `endpoint_id` in [`invoke_allowed`], after this and against the
/// PDP as it is now, and the endpoint's state and the workspace's state are the
/// predicates above.
///
/// # `lv` and the live rate limit
///
/// The statement joins the live version as `lv` **whether or not there is a
/// pin**, and takes `rate_limit` from it — never from `v`. The bucket
/// (`ratelimit::client_key`, `endpoint_key`) is keyed on `endpoint_id` with no
/// version in it, so one endpoint has one bucket; letting `v` supply the rate
/// would put a caller-selected refill rate on shared mutable state, which is the
/// Package 4 defect in a new dress. Concretely, `TokenBucket::take` tops the
/// stored tokens up to `rate.burst` on every call, so a caller alternating
/// `X-Egret-Version: 1` (an old, generous limit) with an unpinned call would
/// refill the endpoint's bucket at version 1's rate and spend it at version 2's
/// — the tightening an author published would not be a limit at all.
///
/// When there is no pin, `lv` and `v` are the same row, so this is not a second
/// source that could disagree with the first: it is the same value, read once,
/// on both paths. What it costs is one `->` on a jsonb already being read.
///
/// **`limits` is deliberately NOT treated this way and stays with `v`.**
/// `max_rows` and `timeout_seconds` bound ONE request, are already clamped by
/// [`MAX_ROWS_CEILING`] and [`TIMEOUT_CEILING_SECS`] before anything is sized
/// from them, and the row cap is quoted back to the caller in
/// `row_limit_exceeded` — it is part of what the frozen contract promises. A rate
/// limit is none of those things: it is a bound on state shared between requests,
/// and shared state driven by a caller-supplied parameter is the one shape this
/// programme has now been bitten by twice.
///
/// # What `Ok(Some(routed))` with `routed.pinned == None` means
///
/// The endpoint resolved and is live; the version the caller named is not one it
/// can serve. Kept apart from `Ok(None)` (no such endpoint) so that [`run`] can
/// answer the two differently — and so that the version refusal happens AFTER
/// the invoke grant and the rate limiter, which is where [`run`] puts it. A
/// refusal before the grant check would tell a caller holding a workspace token
/// but no grant that the endpoint exists.
///
/// An UNPINNED request can never produce it: with `$4` NULL the join condition
/// is `v.id = e.live_version_id`, the same row `lv` is joined on, which the INNER
/// join above has already proved exists.
async fn live_snapshot(
    db: &Db,
    ws: Uuid,
    method: &str,
    path: &str,
    pin: Option<i32>,
) -> Result<Option<Routed>, ServingError> {
    type Row = (
        Uuid,
        i32,
        // `->` yields SQL NULL for an absent key, so this is `Option` even
        // though publish always writes the field. Absent is handled below as
        // the corruption it is, rather than left to a driver decode error that
        // would name a type and no endpoint.
        Option<serde_json::Value>,
        Option<i32>,
        Option<serde_json::Value>,
        Option<bool>,
        Option<bool>,
        Option<serde_json::Value>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT e.id, lv.version, lv.snapshot->'rate_limit', \
                v.version, v.snapshot, d.governed, d.delegated, d.options \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions lv ON lv.id = e.live_version_id \
         LEFT JOIN dataapi.endpoint_versions v \
           ON ($4::int IS NULL AND v.id = e.live_version_id) \
           OR ($4::int IS NOT NULL AND v.endpoint_id = e.id \
               AND v.version = $4 AND v.version <= lv.version) \
         LEFT JOIN query.databases d \
           ON d.id = (v.snapshot->>'database_id')::uuid AND d.workspace_id = $1 \
         WHERE e.workspace_id = $1 AND e.method = $2 AND e.path = $3 \
           AND e.state = 'live'",
    )
    .bind(ws)
    .bind(method)
    .bind(path)
    .bind(pin)
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| {
        // The one expression here that can fail on DATA rather than on the
        // database being unreachable is the `::uuid` cast, and Postgres reports
        // it as `22P02 invalid input syntax for type uuid: "…"` — which names
        // the cast and nothing else. The `from_value` below, which this cast
        // used to be reached before, answered the same corruption with the field
        // name. Say the same thing here rather than leaving the operator with a
        // driver message about a type.
        if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("22P02") {
            return ServingError::internal(format!(
                "the live version of {method} {path} has a snapshot whose `database_id` is not a \
                 uuid; publish is the only writer of that field, so this row was not written by \
                 this server: {e}"
            ));
        }
        ServingError::internal(e)
    })?;

    let Some((id, live_version, raw_rate_limit, version, raw, governed, delegated, options)) = row
    else {
        return Ok(None);
    };
    // Read out of the LIVE version's snapshot with the same type and the same
    // strictness `EndpointSnapshotRepr` applies to it — `RateLimitSpec` carries
    // `deny_unknown_fields`, and the repr's own `rate_limit` field has no
    // `#[serde(default)]`, so a snapshot missing the key is a snapshot the
    // unpinned path has always answered 500 for. No second defaulting rule is
    // invented here: absent or unreadable is the same corruption, reported the
    // same way.
    let live_rate_limit: egret_dataapi_api::RateLimitSpec =
        serde_json::from_value(raw_rate_limit.unwrap_or(serde_json::Value::Null)).map_err(|e| {
            ServingError::internal(format!(
                "the live version of {method} {path} has a snapshot with no readable \
             `rate_limit`; publish is the only writer of that field: {e}"
            ))
        })?;
    // `v.version IS NULL` is the LEFT JOIN having matched nothing: the endpoint
    // is live and reachable, and the pinned version is not one it can serve.
    // Unreachable without a pin — see this function's doc.
    let pinned = match (version, raw) {
        (Some(version), Some(raw)) => {
            // A snapshot that will not deserialise is OUR fault, not the
            // caller's: the only writer is publish. 500, not 404 — a 404 would
            // quietly hide a corrupted or forward-incompatible version behind
            // "no such endpoint".
            let snapshot: EndpointSnapshot =
                serde_json::from_value(raw).map_err(ServingError::internal)?;
            Some((
                version,
                snapshot,
                LiveConnection::from_row(governed, delegated, options),
            ))
        }
        // Both columns come from one LEFT-joined row, so they are NULL together.
        // A half-matched pair would mean the join produced a version with no
        // snapshot, which the `NOT NULL` on the column makes impossible.
        _ => None,
    };
    Ok(Some(Routed {
        endpoint_id: id,
        live_version,
        live_rate_limit,
        pinned,
    }))
}

/// What [`live_snapshot`] resolved: the endpoint, the version that will answer,
/// and the two things read from the LIVE version regardless of any pin.
///
/// A struct rather than the four-tuple this used to be because the fields no
/// longer read off in an order anyone would remember, and because two of them
/// are deliberately about a DIFFERENT version than the one being served — which
/// a positional tuple would hide at every call site.
struct Routed {
    endpoint_id: Uuid,
    /// The version `endpoints.live_version_id` points at. Not necessarily the
    /// one that answers.
    ///
    /// Currently used only for the log line on a refused pin: the SQL already
    /// refuses a pin above it, so nothing compares it in Rust. It is here
    /// because "the pin was refused" is not an operator-actionable sentence
    /// without "and this is what is live".
    live_version: i32,
    /// The LIVE version's published rate limit, applied to every request on this
    /// endpoint whether or not it is pinned. See [`live_snapshot`]'s doc for why
    /// this one field does not follow the pin.
    live_rate_limit: egret_dataapi_api::RateLimitSpec,
    /// `(version, snapshot, live connection)` for the version that will answer,
    /// or `None` when a pin named one this endpoint cannot serve.
    pinned: Option<(i32, EndpointSnapshot, Option<LiveConnection>)>,
}

/// The `Authorization: Bearer …` value, or [`ServingError::Unauthorized`].
///
/// A missing header and a malformed one are the same answer for the same reason
/// every other authentication failure is: see [`super::auth`].
pub(super) fn bearer(headers: &HeaderMap) -> Result<&str, ServingError> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        // Case-insensitive: RFC 7235 makes the scheme token case-insensitive and
        // real clients send `bearer`.
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("Bearer").then(|| rest.trim())
        })
        .ok_or(ServingError::Unauthorized)
}

/// Turn the snapshot's frozen `limits` into the port's [`egret_query_api::Limits`],
/// clamped. See [`MAX_ROWS_CEILING`] for why a published value is not trusted.
///
/// # Why `limits` follows the pin while `rate_limit` does not
///
/// `max_rows` is quoted back to the caller in `row_limit_exceeded`, so it is part
/// of the contract a consumer pinned; the rate limit is not — it is one bucket per
/// endpoint with no version in its key, so taking it from the pin would let a
/// caller choose the refill rate with a header.
///
/// **The cost of that split is larger than "one bigger query", and is stated here
/// rather than discovered during an incident.** With [`MAX_ROWS_CEILING`] at 1M,
/// [`TIMEOUT_CEILING_SECS`] at 300 and up to `MAX_STATEMENTS` statements, an author
/// who tightens `max_rows` from 1M to 10k has tightened nothing at all for a caller
/// already pinned to the older version — and that caller sustains it at the *live*
/// rate limit, because the rate is the one thing the pin does not carry. Retiring a
/// version is the lever that would close it, and there is none below live today.
fn execution_limits(snap: &EndpointSnapshot) -> egret_query_api::Limits {
    egret_query_api::Limits {
        max_rows: snap.limits.max_rows.clamp(1, MAX_ROWS_CEILING) as usize,
        timeout: std::time::Duration::from_secs(
            (snap.limits.timeout_seconds as u64).clamp(1, TIMEOUT_CEILING_SECS),
        ),
    }
}

/// Map an engine failure onto the serving vocabulary.
///
/// The data denial is the one given its own answer, because it is the one the
/// operator can act on and the one that must never be reported as bad input —
/// recognised two ways, into ONE [`ServingError::DataAccessDenied`].
/// `egret_core::Error::Forbidden` is typed; Trino's connector produces it
/// whenever the COORDINATOR's own HTTP response is 401/403
/// (`connector/trino.rs`'s `send()`), which conflates a Ranger/file-ACL deny
/// with a bad or expired engine credential and cannot tell the two apart. Every
/// OTHER shape — StarRocks entirely, and a Trino denial that instead surfaces
/// as a query-level failure inside a 200 — arrives untyped **as
/// `Error::InvalidInput`**, and [`is_permission_denied`] recognises THOSE by
/// message. Both land on the same variant and the same wire code: see that
/// variant's doc for why the surviving code is the neutral `data_access_denied`
/// and not one named for Ranger.
///
/// # The heuristic sees ONE variant, not everything that is not `Forbidden`
///
/// `Error::InvalidInput` is the only variant `port_impl::engine_error` produces
/// from the engine's own message text (`EngineError::Engine`/`BadRequest`/
/// `Limit`). Matching more broadly — every non-`Forbidden` shape, as this
/// function once did — hands the substring test egret's OWN faults too, and
/// `"permission denied"` is a common OS/Kubernetes/vault string. See the match
/// arm below for what that cost.
///
/// # The widening the three notes below asked for, and what it brought
///
/// This function used to answer `500 internal` for everything that was not a
/// denial, and the doc comment here listed what a future widening would owe.
/// Package 3b did it; the list is kept, each item now naming what discharged it,
/// because the reasoning is what stops the next edit from collapsing them again.
///
/// 1. **`execute_as` needed its own timeout error, not `describe_as`'s.** They
///    shared one table, and that table mapped `EngineError::Timeout` to
///    `InvalidInput("query timed out")` for a reason true only of `describe_as`
///    — a `LIMIT 0` plan that times out really is a query problem. For
///    `execute_as` it means the caller's published budget was exceeded.
///    `port_impl` now has `describe_error` and `execute_error`, and only the
///    second types a timeout. Without that split, 504 was undecidable forever.
/// 2. **A distinct AUTHOR-FAULT code, not only 502/504.**
///    `EngineError::BadRequest`/`Engine`/`Limit` still become
///    `Error::InvalidInput` — deliberately, because that message is what the
///    authoring surfaces show the author — but they no longer land on `500
///    internal`. They are [`ServingError::EndpointQueryFailed`], so an operator
///    scanning `access_log.error_code` for "which endpoints are broken" can
///    separate that from an egret fault.
/// 3. **It must not be a 400.** It is a 500. The caller did nothing wrong; the
///    endpoint did.
///
/// # The two engine causes are recovered as VALUES, never from message text
///
/// The old note said 502 and 504 were undecidable here, and gave the right
/// reason: `Error::Internal` was produced both for the engine's
/// `Connect`/`Storage` failures and for EGRET's own — resolving the principal's
/// email, opening the database secret — so a 502 would have paged an operator
/// about Trino when the metastore was what failed; and a timeout arrived as a
/// message string, so recognising it would have meant matching another crate's
/// wording, "a check that keeps compiling and silently stops working the day
/// someone rewords it".
///
/// `egret_query_api::ExecFailure` closed both. `port_impl::execute_error` tags
/// ONLY the engine's own transport failures and ONLY an execution timeout;
/// egret's faults deliberately carry no tag and still fall through to
/// [`ServingError::Internal`]. So the 502 stays honest by construction: it is
/// not "an Internal we guessed was the engine", it is "the port said the engine
/// was unreachable".
fn engine_error(e: egret_core::Error) -> ServingError {
    // Read BEFORE the match, because recovering it needs the error by reference
    // and the arms below consume it. `None` for every error that carries no tag
    // — which is every error egret produced about itself.
    let exec_kind = egret_query_api::ExecFailure::kind_of(&e);
    match e {
        // `raw: None` is a statement, not an omission: `Error::Forbidden` is a
        // unit variant, so there is no engine text to keep. Naming the field
        // here forces any future edit to decide rather than default.
        egret_core::Error::Forbidden => ServingError::DataAccessDenied { raw: None },
        // **Only `InvalidInput` is message-matched, and the narrowing is the
        // point.** `port_impl::engine_error` folds every query-level engine
        // failure — `EngineError::Engine`, which is what a Trino denial inside a
        // 200 (`connector/trino.rs:334`) and every StarRocks failure arrive as,
        // plus `BadRequest` and `Limit` — into `Error::InvalidInput` carrying the
        // engine's own words. That is the ONLY variant whose message came from
        // the engine.
        //
        // `Internal` is deliberately excluded. It is produced for EGRET's own
        // faults as well as the engine's: resolving the principal's email,
        // opening the database secret, resolving delegation
        // (`port_impl.rs`'s `connect_as`). `"permission denied"` is one of the
        // most common OS, Kubernetes and vault error strings, so a secret this
        // process could not open used to be reported as `403 data_access_denied`
        // — sending an operator to audit engine grants that were fine while the
        // actual fault was ours. A false negative here costs a generic 500 with
        // the detail in the log; a false positive costs an operator's afternoon
        // and points them away from the failure.
        //
        // `NotFound` is excluded for the same reason: `connect_as` reports a
        // database outside the workspace as `NotFound` before it reaches an
        // engine at all.
        egret_core::Error::InvalidInput(msg) if is_permission_denied(&msg) => {
            // `raw` never reaches the wire — `to_wire` ignores it, same as
            // `Internal`'s source — but it does reach `serve`'s `error = ?e`
            // log line, so a misclassification still leaves the engine's actual
            // words in the server log. See `ServingError::DataAccessDenied`'s
            // doc for why that matters.
            ServingError::DataAccessDenied {
                raw: Some(anyhow::anyhow!("{msg}")),
            }
        }
        // Everything else `InvalidInput` can be at this seam is the engine
        // rejecting the endpoint's own statement — its SQL, a dropped relation,
        // a resource-group refusal. It is NOT the caller's input: caller values
        // never reach the SQL (they are bound), and a parameter the caller got
        // wrong was refused as `param_invalid` long before the engine was
        // contacted. So the author's endpoint is what is broken.
        egret_core::Error::InvalidInput(msg) => ServingError::EndpointQueryFailed {
            raw: Some(anyhow::anyhow!("{msg}")),
        },
        // The two the port tagged. Matched on the tag read above, never on the
        // variant alone — an untagged `Internal` is egret's own fault and must
        // keep falling through to `internal` below.
        _ if exec_kind == Some(egret_query_api::ExecFailureKind::Timeout) => {
            ServingError::EngineTimeout
        }
        other if exec_kind == Some(egret_query_api::ExecFailureKind::EngineUnavailable) => {
            ServingError::EngineUnavailable {
                raw: Some(anyhow::anyhow!("{other}")),
            }
        }
        other => ServingError::internal(other),
    }
}

/// Shape rows per the snapshot's frozen `output` spec, refusing anything that
/// would silently contradict the frozen contract.
///
/// # The contract is checked, not assumed
///
/// [`ServingMeta`] reports the FROZEN contract — since M1 the UNION of every
/// branch described at publish, and since M2 under this statement's own entry
/// ([`MetaContract`]) — so every key in an `objects` response and
/// every position in an `arrays` one must come from those frozen columns, or the
/// response contradicts its own advertised shape with no signal. Three ways that
/// happened before this function refused anything, each of which produced a
/// perfectly ordinary-looking 200:
///
/// * keys taken from the live `out.columns` with NOTHING checking that those
///   names were declared — a drifted endpoint returning rows whose keys are not
///   in its contract. **The keys do come from the live columns now**, and the
///   section below says why: what makes that safe is the containment check, not
///   where the names are read from. Reading them from the frozen list instead is
///   not the fix and is its own defect — it serves this execution's values under
///   another column's name;
/// * `names.zip(row)` truncating to the shorter side, so an extra live column
///   was dropped and a missing one shortened every object;
/// * `apply_naming` CREATING a collision — `total_revenue` and `totalRevenue`
///   both become `totalRevenue` under `camel`, so one column vanishes from
///   every row while `meta.columns` still advertises it.
///
/// So: drift is [`ServingError::SchemaDrift`] and a collision is
/// [`ServingError::OutputNamingCollision`], both before any row is shaped. The
/// operator's fix differs (re-publish vs. change `naming` or alias the column),
/// which is why they are two variants and two log lines rather than one.
///
/// # What is still a fallback rather than a refusal
///
/// `shape` and `naming` are unvalidated author input, exactly like `limits`
/// (spec §5.3 freezes them with no rule anywhere). An unrecognised value has no
/// meaning to honour, so it falls back to the documented default and logs —
/// refusing would make a published endpoint permanently dead over a cosmetic
/// field, and unlike the two cases above it cannot silently lose a column.
///
/// # Containment, not equality — and why types are compared now
///
/// A conditional definition describes EVERY branch at publish and freezes the
/// union of their columns, flagging `required` for the ones every branch
/// returns (spec §3.2). A branch that legitimately omits an optional column
/// must therefore be served rather than refused, and the `objects` predicate is:
/// every returned name is IN the frozen union, every `required` column is
/// present, and a returned column's type still HONOURS the declared one —
/// [`compare_column_types`], not string equality, for the one measured reason in
/// its doc comment: Trino spells a datetime column's precision differently
/// depending on which of its two typing paths answered, and both spellings are
/// already in the published corpus. Every other difference, including two
/// STATED precisions, is still drift. A column
/// declared `required: false` and not returned is simply ABSENT from the object,
/// never null — absent is what "this branch does not return it" means, and null
/// would be indistinguishable from a returned NULL value.
///
/// Types used to be deliberately ignored here, on the argument that a widened
/// `integer` → `bigint` changes no key and no position so refusing it would
/// break endpoints over a change their clients cannot observe. That argument
/// does not survive the union: once the NAME check is a subset check, the type
/// is the only thing left tying a returned column to the one that was reviewed
/// at publish — and publish already refuses a name that describes with two
/// types across branches (§3.2), because two types cannot be unioned into one
/// honest declaration. "Two types" is the same predicate there as here, and for
/// the same reason: one column whose precision one branch's typing path merely
/// suppressed is not two types, and refusing it made a definition nobody had
/// changed unpublishable. Without the comparison here, that same disagreement
/// arrives at serving time and is answered with a `200` carrying an integer
/// under a declared `varchar`.
///
/// **This is the backstop for every branch publish did not reach**, and it fails
/// closed for that reason. The probe budget is bounded (`MAX_PROBES`) and a
/// definition can branch on more combinations than it covers; the absent-
/// parameter contexts do not include pairs. Both gaps are measured and recorded
/// in the spec. An undeclared column reaching a caller is not a documentation
/// defect — it is a field nobody reviewed at publish being served.
///
/// A snapshot frozen before any of this carries `required: true` on every column
/// ([`ColumnSpec`]'s serde default), so an already-published endpoint keeps
/// exact-match semantics: subset plus all-required is set equality. What it does
/// lose is ORDER sensitivity, and only on this path — see below.
///
/// # Order: all of the `arrays` NAME predicate, none of the `objects` one
///
/// `arrays` is positional, so two identical name SETS in a different order are
/// still a different contract, and that path keeps the pre-M1 name-and-order
/// equality check unchanged. Publish refuses an `arrays` endpoint whose branches
/// describe different columns (`ValidationError::BranchingArraysShape`), so
/// there is no SUBSET to accept there.
///
/// **What that path does not keep is "names and arity are all that matter": it
/// compares TYPES too.** No branch is needed for `ALTER TABLE … TYPE`, and the
/// case for comparing them is stronger here than under `objects`, because a
/// positional row carries no names at all — a client reading `data[i][2]` as the
/// declared `varchar` has nothing in the body to notice an integer by. "There is
/// no subset to accept" is a statement about the NAME list; it is not a
/// statement that the positional path has nothing left to check.
///
/// `objects` no longer compares order, because this function no longer depends
/// on it: keys are taken from the LIVE columns, so a reordered result binds each
/// value to its own name by construction. The old code zipped the FROZEN names
/// onto the row, which is what made a reorder dangerous — and that mis-binding
/// is now unrepresentable rather than merely refused.
fn shape_rows(
    out: &egret_core::TabularResult,
    output: &OutputSpec,
    stmt: &StatementSnapshot,
) -> Result<serde_json::Value, ServingError> {
    // **The statement, never the snapshot.** All six clauses below — every live
    // name is contained, every `required` name is present, every declared type
    // still agrees, no live name repeats, every row's arity matches, and the
    // result carries columns at all — are properties of ONE statement's frozen
    // union, and they had to become per-statement TOGETHER: converting some and
    // leaving others reading `statements[0]` is a hole no single-statement test
    // can show, because at N=1 the two readings are the same list.
    //
    // Taking the statement rather than the snapshot is what makes that
    // impossible rather than merely fixed: there is no `snap` in scope here to
    // fetch another statement's columns from. `output` comes separately because
    // `shape` and `naming` are endpoint-wide — one contract, not one per
    // statement.
    let frozen = &stmt.columns;

    // Row arity, checked for BOTH shapes and BEFORE the branch on `shape`.
    //
    // The column LISTS agreeing does not imply each row has that many values —
    // a connector can return a row whose own length disagrees with the column
    // list it just reported. This guard used to live inside the `objects` loop
    // only, and the asymmetry was the bug: `arrays` cloned each row through
    // verbatim, so such a row was served as `[[v1,v2,v3]]` under a two-column
    // `meta.columns`, silently, while `objects` refused it. Hoisted above the
    // branch rather than duplicated, so the two shapes cannot drift apart again.
    //
    // Measured against the LIVE column list, not the frozen one: an `objects`
    // response may now carry a SUBSET of the frozen union, so the frozen count
    // is no longer the number of values a row is supposed to have. On the
    // `arrays` path the equality check below makes the two counts identical, so
    // nothing there changes.
    //
    // It is a connector bug, not a caller's or an author's problem, so it is
    // `Internal` rather than one of the contract variants — but it is refused
    // rather than zipped past, which is the whole point of this function.
    for row in &out.rows {
        if row.len() != out.columns.len() {
            return Err(ServingError::internal(format!(
                "connector returned a row of {} values for {} columns",
                row.len(),
                out.columns.len()
            )));
        }
    }

    // A result with NO columns at all, refused before either shape reads it.
    //
    // Containment cannot see this one. Against an all-optional union — which
    // `SELECT {% if x %}a{% else %}b{% endif %}` produces, since neither column
    // is in every branch — zero live columns satisfies every clause vacuously:
    // nothing is undeclared, no `required` column is missing, no name repeats.
    // The `objects` path then serves `200 [{}]`, one empty object per row.
    // MEASURED on the code as it stood one commit ago, not argued: `Ok("[{}]")`.
    // The pre-M1 count equality refused it by arithmetic, and this gate must not
    // be looser than what it replaced.
    //
    // Above the shape branch rather than inside `objects`, for the same reason
    // the arity guard is: a guard that lives on one path is a guard the other
    // path silently lacks. Publish refuses a describe that returns no columns
    // (`ValidationError::DescribeReturnedNoColumns`), so a frozen list is never
    // empty and `arrays` reaches this only with a result it would refuse anyway.
    if out.columns.is_empty() {
        return Err(drift(
            stmt,
            out,
            "the engine returned no columns at all",
            None,
        ));
    }

    if output.shape == "arrays" {
        // Order-sensitive equality: an array row is positional, so the same
        // names in a different order — or one name more, or one fewer — are a
        // different contract. Task 5 refuses to publish a branching definition
        // under this shape, so there is never a legitimate subset to accept
        // here, and this half of the predicate is the pre-M1 one unchanged.
        let drifted = out.columns.len() != frozen.len()
            || out
                .columns
                .iter()
                .zip(frozen.iter())
                .any(|(live, frozen)| live.name != frozen.name);
        if drifted {
            return Err(drift(stmt, out, "the positional column list changed", None));
        }
        // Types are compared HERE TOO, and this half is new.
        //
        // Nothing stops a table's type drifting under a positional endpoint:
        // `arrays` cannot branch, but `ALTER TABLE … TYPE` needs no branch. The
        // reason the `objects` path compares types applies here word for word —
        // the type is what ties a returned column to the one that was reviewed
        // at publish — and it applies with LESS warning, because a positional
        // row carries no names at all, so a client reading `data[i][2]` as the
        // declared `varchar` has nothing in the body to notice the integer by.
        // Kept as a second check rather than folded into the `any` above so the
        // log line names which clause refused; three rounds of this plan were
        // spent on refusals that reported the wrong reason.
        //
        // `compare_column_types` rather than `!=` — see its doc comment. Trino
        // spells one and the same datetime column two ways depending on which
        // of its two typing paths answered, and both spellings are already in
        // the published corpus, so the comparison is where they reconcile. The
        // strings LOGGED below stay verbatim: an operator must see what each
        // side actually said, not the canonical form they were judged on.
        if let Some((live, frozen)) = out
            .columns
            .iter()
            .zip(frozen.iter())
            .find(|(live, frozen)| !compare_column_types(&frozen.ty, &live.ty).agrees())
        {
            return Err(drift(
                stmt,
                out,
                "a declared column changed type",
                Some(TypeMismatch {
                    column: &live.name,
                    declared: &frozen.ty,
                    returned: &live.ty,
                }),
            ));
        }
        return Ok(serde_json::Value::Array(
            out.rows
                .iter()
                .map(|r| serde_json::Value::Array(r.clone()))
                .collect(),
        ));
    }
    if output.shape != "objects" {
        tracing::warn!(
            shape = %output.shape,
            "published snapshot carries an unrecognised output.shape; serving the default"
        );
    }

    // Containment, not equality. A branch may legitimately omit a column
    // declared `required: false`; it may never return one that was never
    // declared, and it may never change a declared column's type. See this
    // function's doc comment for why the type comparison is here now and why
    // this is the backstop that has to fail closed.
    //
    // "Change its type" is `compare_column_types`, not `!=`: a Trino datetime
    // column arrives with its precision SUPPRESSED on the client protocol and
    // STATED by `DESCRIBE OUTPUT`, so `timestamp(0)` and `timestamp` are one
    // column, not two types. Two stated precisions still disagree.
    for live in &out.columns {
        match frozen.iter().find(|f| f.name == live.name) {
            None => return Err(drift(stmt, out, "a column that was never declared", None)),
            Some(frozen) if !compare_column_types(&frozen.ty, &live.ty).agrees() => {
                return Err(drift(
                    stmt,
                    out,
                    "a declared column changed type",
                    Some(TypeMismatch {
                        column: &live.name,
                        declared: &frozen.ty,
                        returned: &live.ty,
                    }),
                ))
            }
            Some(_) => {}
        }
    }
    for required in frozen.iter().filter(|c| c.required) {
        if !out.columns.iter().any(|l| l.name == required.name) {
            return Err(drift(stmt, out, "a required column was not returned", None));
        }
    }
    // Two LIVE columns of ONE name — `SELECT *` over a join whose right side
    // gained a column the left already had. Equality refused this for free, by
    // arithmetic: a repeat made the counts disagree. Containment does not — both
    // are declared, both type-match, and every required column is present — and
    // the two would then land on one key with the `Map` keeping the last value,
    // which is a whole column gone with no error. So it is checked explicitly.
    //
    // `SchemaDrift` rather than `OutputNamingCollision` because `naming` is not
    // what did it: publish refuses two columns of one name outright
    // (`ValidationError::AmbiguousColumnName`), so re-publishing is what shows
    // the author the aliasing fix, and re-publishing is what `schema_drift`
    // tells the operator to do.
    let mut live_seen = std::collections::HashSet::with_capacity(out.columns.len());
    if out
        .columns
        .iter()
        .any(|c| !live_seen.insert(c.name.as_str()))
    {
        return Err(drift(stmt, out, "two live columns carry one name", None));
    }

    // The collision check runs over the FROZEN union, not over this call's
    // columns. `output.naming` collapsing two DECLARED names onto one key is a
    // property of the published contract, so it must refuse whichever branch is
    // being served: otherwise the generated document advertises two keys, and
    // the endpoint answers or refuses depending on which parameters it was
    // called with.
    let names: Vec<String> = frozen
        .iter()
        .map(|c| apply_naming(&c.name, &output.naming))
        .collect();
    // A `Map` silently keeps the LAST value for a duplicated key, which is how a
    // whole column disappears from every row without a single error.
    let mut seen = std::collections::HashSet::with_capacity(names.len());
    if let Some(dup) = names.iter().find(|n| !seen.insert(n.as_str())) {
        tracing::warn!(
            key = %dup,
            naming = %output.naming,
            frozen = ?frozen.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "output.naming maps two frozen columns onto one key"
        );
        return Err(ServingError::OutputNamingCollision);
    }

    // The LIVE names, because a branch's response carries the keys THAT BRANCH
    // returned. `meta.columns` still reports the frozen union — the contract,
    // not this execution — which is what tells a consumer the absent key can
    // appear at all, and the OpenAPI `required` list which ones always do.
    //
    // These cannot collide with one another: they are `apply_naming` over a
    // subset of the frozen names (checked above) that carries no repeats
    // (checked above) drawn from a frozen list whose mapped names are distinct
    // (checked just above). So no row can lose a column below.
    let live_names: Vec<String> = out
        .columns
        .iter()
        .map(|c| apply_naming(&c.name, &output.naming))
        .collect();

    // `live_names` is `out.columns` mapped one-to-one, so every row's arity was
    // already checked against it above and the zip below cannot truncate.
    let mut shaped = Vec::with_capacity(out.rows.len());
    for row in &out.rows {
        shaped.push(serde_json::Value::Object(
            live_names
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .collect::<serde_json::Map<_, _>>(),
        ));
    }
    Ok(serde_json::Value::Array(shaped))
}

/// The column a TYPE refusal is about, and the two spellings that disagree.
///
/// Named fields rather than a tuple because `declared` and `returned` are both
/// `&str` and both type names: positionally they are indistinguishable, so a
/// call site that swapped them would compile, log plausibly, and tell the
/// operator to fix the wrong end.
struct TypeMismatch<'a> {
    column: &'a str,
    declared: &'a str,
    returned: &'a str,
}

/// One log line for every way the frozen contract and the live columns can
/// disagree, so an operator sees both lists and the clause that refused.
///
/// The distinction from [`ServingError::OutputNamingCollision`] is the
/// operator's FIX: drift means re-publish (the definition and the table have
/// moved apart), a collision means change `naming` or alias the column. That is
/// why they stay two variants and two log lines rather than one.
///
/// # Why the type refusals carry three more fields
///
/// For every other clause the two name lists ARE the diagnosis: one list has a
/// name the other does not, or has it twice, or is empty. For a type refusal the
/// two lists are IDENTICAL, so an operator reading this line learns which clause
/// fired and nothing about which column or which types — and on the `arrays`
/// path there are no names in the response body either, which makes this line
/// the only instrument there is. `mismatch` is `None` for the clauses the lists
/// already explain, and the fields log empty there rather than absent, so one
/// log site serves every refusal and the two cannot drift apart.
fn drift(
    stmt: &StatementSnapshot,
    out: &egret_core::TabularResult,
    why: &str,
    mismatch: Option<TypeMismatch<'_>>,
) -> ServingError {
    let m = mismatch.unwrap_or(TypeMismatch {
        column: "",
        declared: "",
        returned: "",
    });
    tracing::warn!(
        // **The statement, handed in — never fetched.** This used to read the
        // snapshot's FIRST statement, which was the right list only while
        // serving executed only the first. Once execution became a loop, a drift
        // in the second statement would have logged the first's column names:
        // the operator reads one line to decide what moved, and it would have
        // sent them to a statement that was fine. Taking a `StatementSnapshot`
        // rather than the snapshot is what makes that unrepresentable — there is
        // no list here to pick the wrong element from.
        statement = %stmt.name,
        frozen = ?stmt.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
        live = ?out.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
        column = %m.column,
        declared = %m.declared,
        returned = %m.returned,
        why,
        "published output contract no longer matches the engine's columns"
    );
    ServingError::SchemaDrift
}

/// `as_is` (default) | `camel` | `snake`. Unrecognised falls back to `as_is`.
///
/// **`camel` lowercases only the FIRST character.** An earlier version
/// lowercased every character that did not follow a separator, which turned an
/// already-camel `totalRevenue` into `totalrevenue` — a name the author never
/// wrote, under a setting that should have left it alone. It also hid the
/// collision `camel` genuinely creates: `total_revenue` and `totalRevenue` must
/// both map to `totalRevenue`, which is what [`shape_rows`] refuses. The lossy
/// version made those two *different* keys and looked correct while quietly
/// mangling one of them.
pub(crate) fn apply_naming(name: &str, naming: &str) -> String {
    match naming {
        "camel" => {
            let mut out = String::with_capacity(name.len());
            let mut upper_next = false;
            for c in name.chars() {
                if c == '_' || c == '-' || c == ' ' {
                    upper_next = !out.is_empty();
                } else if upper_next {
                    out.extend(c.to_uppercase());
                    upper_next = false;
                } else if out.is_empty() {
                    out.extend(c.to_lowercase());
                } else {
                    out.push(c);
                }
            }
            out
        }
        "snake" => {
            let mut out = String::with_capacity(name.len() + 4);
            for (i, c) in name.chars().enumerate() {
                if c == '-' || c == ' ' {
                    out.push('_');
                } else if c.is_uppercase() {
                    if i > 0 && !out.ends_with('_') {
                        out.push('_');
                    }
                    out.extend(c.to_lowercase());
                } else {
                    out.push(c);
                }
            }
            out
        }
        _ => name.to_string(),
    }
}

// The wire mapping lives in `serving/error.rs` and NOWHERE else.
//
// `ServingError` deliberately does NOT implement `IntoResponse`. It used to, as
// a placeholder with a fixed body and no `request_id`, and leaving it would
// leave a second path to the wire that compiles fine, skips the access log, and
// hands the caller an error they cannot quote to anyone. `error::to_wire` takes
// a `request_id` it cannot invent, so the only way to answer is through `serve`,
// which has one.

#[cfg(test)]
mod tests {
    use super::*;

    // --- Package 3b: `internal` was one code for six causes -----------------
    //
    // Every case below builds its input with `egret_query_api::ExecFailure`,
    // which is the SAME constructor `egret_query::execution::port_impl`'s
    // `execute_error` uses — the two crates cannot see each other, and the
    // producer half is pinned by `a_timeout_means_different_things_to_describe_
    // and_to_execute` and `an_unreachable_engine_is_typed_on_the_execute_path`
    // over there. Asserting a code string against a hand-built `ServingError`
    // would prove nothing about what actually reaches the wire; these drive the
    // classifier itself, and `serving_errors.rs` drives the whole handler.

    /// The endpoint's own statement failed. Before this it was `500 internal`,
    /// indistinguishable in `access_log.error_code` from egret being broken.
    ///
    /// Deleting the `InvalidInput` arm from `engine_error` makes this fail with
    /// `internal`, which is exactly the defect it names.
    #[test]
    fn a_rejected_statement_is_the_authors_endpoint_not_egret() {
        let e = engine_error(egret_core::Error::InvalidInput(
            "line 1:15: Table 'hive.analytics.salaries' does not exist".into(),
        ));
        assert!(
            matches!(e, ServingError::EndpointQueryFailed { .. }),
            "expected EndpointQueryFailed, got {e:?}"
        );
        let w = to_wire(&e);
        assert_eq!(w.code, "endpoint_query_failed");
        assert_eq!(w.status.as_u16(), 500, "spec §7 (c): it must not be a 4xx");
    }

    /// The denial heuristic still wins over the new arm. Order matters: a
    /// permission denial arrives as `InvalidInput` too, and reporting it as a
    /// broken endpoint would send an admin to debug SQL that is fine.
    #[test]
    fn a_denial_still_beats_the_new_author_fault_arm() {
        let e = engine_error(egret_core::Error::InvalidInput(
            "Access Denied: Cannot select from table sales".into(),
        ));
        assert!(
            matches!(e, ServingError::DataAccessDenied { .. }),
            "the denial heuristic must be tried first, got {e:?}"
        );
    }

    /// Spec §7's `timeout`, decidable at last — and decidable **as a value**.
    /// The port no longer says "query timed out" in words this crate would have
    /// had to match.
    #[test]
    fn an_execution_timeout_is_a_504() {
        let from_port =
            egret_query_api::ExecFailure::new(egret_query_api::ExecFailureKind::Timeout, "timeout")
                .into_error();
        let e = engine_error(from_port);
        assert!(matches!(e, ServingError::EngineTimeout), "got {e:?}");
        let w = to_wire(&e);
        assert_eq!(w.code, "timeout");
        assert_eq!(w.status.as_u16(), 504);
    }

    /// Spec §7's `engine_unavailable`.
    #[test]
    fn an_unreachable_engine_is_a_502() {
        let from_port = egret_query_api::ExecFailure::new(
            egret_query_api::ExecFailureKind::EngineUnavailable,
            "connect: error sending request for url (https://trino.internal:8443/v1/statement)",
        )
        .into_error();
        let e = engine_error(from_port);
        assert!(
            matches!(e, ServingError::EngineUnavailable { .. }),
            "got {e:?}"
        );
        let w = to_wire(&e);
        assert_eq!(w.code, "engine_unavailable");
        assert_eq!(w.status.as_u16(), 502);
    }

    /// **The half that keeps the 502 honest, and the reason the old comment
    /// said it was undecidable.**
    ///
    /// `port_impl::connect_as` produces a plain `Error::Internal` for egret's
    /// OWN faults — resolving the principal's email, opening the database
    /// secret. Those must stay `internal`: answering 502 for one would page an
    /// operator about Trino when the metastore is what failed.
    ///
    /// Mutating the two arms above to match on the variant instead of on the
    /// tag — `Error::Internal(_) => EngineUnavailable` — fails here, and it is
    /// the mutation most likely to be written, because it looks simpler.
    #[test]
    fn an_untagged_internal_is_still_egrets_own_fault() {
        for detail in [
            "open secret: connection pool timed out",
            "resolve principal: no such user",
            "delegation: token minting failed",
        ] {
            let e = engine_error(egret_core::Error::Internal(anyhow::anyhow!("{detail}")));
            assert!(
                matches!(e, ServingError::Internal(_)),
                "{detail:?} is egret's fault and must stay `internal`, got {e:?}"
            );
            assert_eq!(to_wire(&e).code, "internal");
        }
    }

    /// `connect_as` reports a database outside the workspace as `NotFound`
    /// before an engine is contacted, so it is egret's answer and not the
    /// engine's — it must not become an endpoint fault.
    #[test]
    fn a_not_found_from_the_port_is_not_an_engine_failure() {
        let e = engine_error(egret_core::Error::NotFound("database".into()));
        assert!(matches!(e, ServingError::Internal(_)), "got {e:?}");
    }

    /// **A caller must not be able to choose how loudly a fault is reported.**
    ///
    /// `has_repeats` is caller-controlled — append `&region=x` twice — and on its
    /// own it turned EVERY render failure into a client-side 400 logged at
    /// `debug!`. A `PlaceholderBindMismatch`, the backstop nothing has ever
    /// bypassed, reached the operator as `param_invalid` with no warning line.
    #[test]
    fn only_the_caller_triggered_render_failure_becomes_a_400() {
        use crate::render::RenderError as R;

        // The one variant a repeat can actually cause, and only with a repeat.
        assert!(matches!(
            render_error(R::SequenceBind, true),
            ServingError::ParamInvalid
        ));
        assert!(matches!(
            render_error(R::SequenceBind, false),
            ServingError::Internal { .. }
        ));

        // Everything else is a statement about the DEFINITION, repeat or not.
        for e in [
            R::PlaceholderBindMismatch {
                placeholders: 1,
                binds: 2,
            },
            R::RenderedSeparator,
            R::RenderedHash,
            R::CaptureConstruct("filter"),
            R::ParameterInspected,
            R::Ident,
            R::Unscannable("x".into()),
            R::BindsTooLarge,
        ] {
            assert!(
                matches!(render_error(e, true), ServingError::Internal { .. }),
                "a repeated parameter must not downgrade this"
            );
        }
    }

    /// The naming transforms are pure and cheap to pin here; the routing
    /// properties they sit next to need a database and live in
    /// `tests/serving_routing.rs`.
    #[test]
    fn naming_transforms_are_applied_per_the_snapshot() {
        assert_eq!(apply_naming("total_revenue", "camel"), "totalRevenue");
        assert_eq!(apply_naming("totalRevenue", "snake"), "total_revenue");
        assert_eq!(apply_naming("total_revenue", "as_is"), "total_revenue");
        // An unrecognised value is `as_is`, not a panic and not an empty name.
        assert_eq!(apply_naming("total_revenue", "kebab"), "total_revenue");
        // `camel` must be IDEMPOTENT on an already-camel name. The earlier
        // version returned "totalrevenue" here — a name the author never wrote,
        // and the reason the collision below was invisible.
        assert_eq!(apply_naming("totalRevenue", "camel"), "totalRevenue");
        assert_eq!(apply_naming("TotalRevenue", "camel"), "totalRevenue");
    }

    /// The collision `shape_rows` refuses. Asserted on `apply_naming` too, so
    /// the refusal's premise is pinned independently of the serving path.
    #[test]
    fn camel_maps_two_distinct_frozen_names_onto_one_key() {
        assert_eq!(
            apply_naming("total_revenue", "camel"),
            apply_naming("totalRevenue", "camel"),
            "these two must collide — it is why shape_rows checks for duplicates"
        );
    }

    #[test]
    fn published_limits_are_clamped_before_they_are_acted_on() {
        let mut snap = snapshot_with_limits(0, 0);
        let l = execution_limits(&snap);
        assert_eq!(l.max_rows, 1, "a 0 cap must not mean 'return nothing'");
        assert_eq!(
            l.timeout,
            std::time::Duration::from_secs(1),
            "a 0 timeout must not mean 'time out instantly'"
        );

        snap = snapshot_with_limits(u32::MAX, u32::MAX);
        let l = execution_limits(&snap);
        assert_eq!(l.max_rows, MAX_ROWS_CEILING as usize);
        assert_eq!(
            l.timeout,
            std::time::Duration::from_secs(TIMEOUT_CEILING_SECS)
        );
    }

    // -----------------------------------------------------------------------
    // `shape_rows`: containment, `required`, and types.
    //
    // These drive the function directly rather than through the router because
    // the interesting inputs are results no well-behaved engine produces — a
    // branch that returns fewer columns than the union, a column that was never
    // declared, a declared column that came back with another type. The
    // end-to-end half (a real snapshot, a real 503 body) lives in
    // `tests/serving_routing.rs`.
    // -----------------------------------------------------------------------

    fn spec(name: &str, ty: &str, required: bool) -> ColumnSpec {
        ColumnSpec {
            name: name.into(),
            ty: ty.into(),
            required,
        }
    }

    fn live(name: &str, ty: &str) -> egret_core::ColumnMeta {
        egret_core::ColumnMeta {
            name: name.into(),
            ty: ty.into(),
        }
    }

    /// [`shape_rows`] against a single-statement snapshot's ONLY statement.
    ///
    /// Every test below pins one of the six clauses `shape_rows` judges, and
    /// each is a property of one statement's frozen columns — so they say
    /// "snapshot" and mean "the statement it carries". The destructuring is the
    /// point: a fixture that grew a second statement would otherwise be judged
    /// against its first silently, which is the very reading this task removed
    /// from the production path.
    fn shape_sole(
        out: &egret_core::TabularResult,
        snap: &EndpointSnapshot,
    ) -> Result<serde_json::Value, ServingError> {
        let [stmt] = &snap.statements[..] else {
            panic!("this helper is for single-statement fixtures");
        };
        shape_rows(out, &snap.output, stmt)
    }

    /// `snapshot_with_limits` with the frozen columns replaced. The limits are
    /// the clamped-to-1 pair, which `shape_rows` never reads.
    fn snapshot_with_columns(columns: Vec<ColumnSpec>) -> EndpointSnapshot {
        let base = snapshot_with_limits(1, 1);
        EndpointSnapshot {
            statements: vec![egret_dataapi_api::StatementSnapshot {
                columns,
                ..base.statements[0].clone()
            }],
            ..base
        }
    }

    fn result(
        columns: Vec<egret_core::ColumnMeta>,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> egret_core::TabularResult {
        egret_core::TabularResult {
            row_count: rows.len() as u64,
            columns,
            rows,
            ..Default::default()
        }
    }

    /// The whole point of the plan: the branch that returns fewer columns is the
    /// endpoint's OWN documented default call, and it used to 503.
    #[test]
    fn a_branch_that_omits_an_optional_column_is_served() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", false),
        ]);
        let out = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1)]],
        );

        let v = shape_sole(&out, &snap).expect("an optional column may be absent");
        assert_eq!(
            v,
            serde_json::json!([{"id": 1}]),
            "the key is ABSENT, not null: null is indistinguishable from a returned NULL"
        );
        // Asserted on the object rather than inferred from the equality above,
        // because a `null` under the key would still be a two-key object and the
        // reader of a failure needs to see which it was.
        let obj = v[0].as_object().unwrap();
        assert_eq!(
            obj.len(),
            1,
            "one key for the one column this branch returned"
        );
        assert!(!obj.contains_key("name"));
    }

    #[derive(Clone, Default)]
    struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);

    struct LogWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
        type Writer = LogWriter;
        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(self.0.clone())
        }
    }

    fn logs_from_shaping(out: &egret_core::TabularResult, snap: &EndpointSnapshot) -> String {
        let [stmt] = &snap.statements[..] else {
            panic!("this helper is for single-statement fixtures");
        };
        logs_from_statement_shaping(out, &snap.output, stmt)
    }

    /// The same capture, against an explicitly named statement — the only form
    /// that can ask which statement a refusal was logged AGAINST.
    fn logs_from_statement_shaping(
        out: &egret_core::TabularResult,
        output: &OutputSpec,
        stmt: &StatementSnapshot,
    ) -> String {
        let cap = LogCapture::default();
        let sub = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(cap.clone())
            .without_time()
            // Measured, not assumed: with ANSI on, the field NAME and its `=`
            // are separated by escape sequences, so `contains("declared=…")`
            // never matches and every assertion below would be vacuous.
            .with_ansi(false)
            .finish();
        {
            let _guard = tracing::subscriber::set_default(sub);
            assert!(
                matches!(
                    shape_rows(out, output, stmt),
                    Err(ServingError::SchemaDrift)
                ),
                "expected a refusal so there is a reason to log"
            );
        }
        let bytes = cap.0.lock().expect("log lock").clone();
        String::from_utf8(bytes).expect("log output is utf-8")
    }

    /// The log line IS the diagnosis for a type refusal, so it is asserted
    /// rather than promised.
    ///
    /// **This test exists because I first recorded the opposite as a
    /// limitation** — "verified by compilation and reading, because nothing in
    /// this crate captures `tracing` output". The premise was false:
    /// `tests/serving_auth.rs` has captured log CONTENT since
    /// `every_rejection_logs_a_distinguishable_reason`, and
    /// `tracing-subscriber` is a dev-dependency of this crate carrying a
    /// comment saying it is here so that a log promise does not stay prose.
    ///
    /// Note what `frozen` and `live` carry here: **identical lists**. That is
    /// the entire reason the three extra fields exist — for every other clause
    /// the two name lists are the diagnosis, and for this one they say nothing.
    ///
    /// The assertions are per FIELD rather than "both spellings appear
    /// somewhere", because a call site that transposed `declared` and
    /// `returned` emits both strings either way and would pass the loose form.
    /// `declared` must carry the FROZEN spelling and `returned` the LIVE one.
    #[test]
    fn a_type_refusal_logs_the_column_and_which_spelling_came_from_where() {
        let snap = snapshot_with_columns(vec![spec("v", "varchar", true)]);
        let out = result(vec![live("v", "integer")], vec![vec![serde_json::json!(1)]]);

        let logs = logs_from_shaping(&out, &snap);
        assert!(
            logs.contains(r#"why="a declared column changed type""#),
            "{logs}"
        );
        assert!(
            logs.contains(r#"frozen=["v"] live=["v"]"#),
            "the premise of this test: the name lists cannot diagnose a type refusal: {logs}"
        );
        assert!(logs.contains("column=v "), "{logs}");
        assert!(
            logs.contains("declared=varchar "),
            "`declared` must carry the FROZEN spelling, not the live one: {logs}"
        );
        assert!(
            logs.contains("returned=integer "),
            "`returned` must carry the LIVE spelling, not the frozen one: {logs}"
        );

        // The positional path, where the response body carries no names either,
        // so this line is the only instrument an operator has.
        let mut arrays = snapshot_with_columns(vec![spec("v", "varchar", true)]);
        arrays.output.shape = "arrays".into();
        let logs = logs_from_shaping(&out, &arrays);
        assert!(logs.contains("column=v "), "arrays: {logs}");
        assert!(logs.contains("declared=varchar "), "arrays: {logs}");
        assert!(logs.contains("returned=integer "), "arrays: {logs}");

        // A clause the name lists DO explain leaves the three fields empty —
        // one log site for every refusal, and nothing stale carried over from
        // the shape of another one.
        let undeclared = result(
            vec![live("v", "varchar"), live("secret", "varchar")],
            vec![vec![serde_json::json!("a"), serde_json::json!("b")]],
        );
        let logs = logs_from_shaping(&undeclared, &snap);
        assert!(
            logs.contains("column= declared= returned= "),
            "a non-type refusal must not invent a column or a type: {logs}"
        );
    }

    /// The merge blocker this comparison exists for, on BOTH shapes.
    ///
    /// Trino suppresses a datetime column's precision on the client protocol
    /// unless the caller sends `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME`,
    /// which `TrinoConnector` does not — while `DESCRIBE OUTPUT`, the publish
    /// path for every endpoint that declares a parameter, states it. Measured
    /// live on Trino 483: an endpoint whose SQL never changed froze
    /// `timestamp(0)` and served `timestamp`, and answered `503 schema_drift`
    /// on **every** call, on both shapes. Re-publishing re-ran `DESCRIBE
    /// OUTPUT` and re-froze `timestamp(0)`, so no author action cleared it.
    ///
    /// The fixture freezes the spelling a snapshot written BEFORE this branch
    /// carries, and never re-publishes: this is the upgrade path, not a fresh
    /// publish. Every other fixture in this module types its columns `integer`,
    /// which is why 432 green tests did not see this.
    #[test]
    fn a_datetime_column_whose_precision_the_client_protocol_suppressed_still_serves() {
        for (frozen_ty, live_ty) in [
            ("timestamp(0)", "timestamp"),
            ("timestamp(3)", "timestamp"),
            ("time(0)", "time"),
            ("timestamp(0) with time zone", "timestamp with time zone"),
            ("array(timestamp(0))", "array(timestamp)"),
            // The reverse pairing too: a snapshot frozen through the NO-param
            // path carries the bare spelling, and both are in the corpus.
            ("timestamp", "timestamp(0)"),
        ] {
            let out = result(
                vec![live("c", live_ty)],
                vec![vec![serde_json::json!("2020-01-01 10:20:30.000")]],
            );

            let objects = snapshot_with_columns(vec![spec("c", frozen_ty, true)]);
            assert_eq!(
                shape_sole(&out, &objects)
                    .unwrap_or_else(|e| panic!("objects: {frozen_ty} vs {live_ty} refused: {e:?}")),
                serde_json::json!([{"c": "2020-01-01 10:20:30.000"}]),
            );

            let mut arrays = snapshot_with_columns(vec![spec("c", frozen_ty, true)]);
            arrays.output.shape = "arrays".into();
            assert_eq!(
                shape_sole(&out, &arrays)
                    .unwrap_or_else(|e| panic!("arrays: {frozen_ty} vs {live_ty} refused: {e:?}")),
                serde_json::json!([["2020-01-01 10:20:30.000"]]),
            );
        }
    }

    /// The tolerance above is bounded, and this is where the boundary is.
    ///
    /// Asserted on the REASON, not on the status: `503` alone covers
    /// `output_naming_collision` too, so a test that only checked the code
    /// would pass on a refusal that fired for the wrong reason entirely.
    #[test]
    fn two_stated_datetime_precisions_are_still_drift() {
        let out = result(
            vec![live("c", "timestamp(6)")],
            vec![vec![serde_json::json!("2020-01-01 10:20:30.000000")]],
        );
        let snap = snapshot_with_columns(vec![spec("c", "timestamp(3)", true)]);
        assert!(
            logs_from_shaping(&out, &snap).contains(r#"why="a declared column changed type""#),
            "a precision BOTH sides stated is a real type change, not a suppressed one"
        );

        // And the ordinary case the whole gate is for.
        let out = result(vec![live("c", "integer")], vec![vec![serde_json::json!(1)]]);
        let snap = snapshot_with_columns(vec![spec("c", "varchar", true)]);
        assert!(
            logs_from_shaping(&out, &snap).contains(r#"why="a declared column changed type""#),
            "varchar -> integer must still be refused, and for THIS reason"
        );
    }

    /// **The plan's headline case, asserted positively.**
    /// `SELECT {% if x %}a{% else %}b{% endif %}` unions to two columns that are
    /// BOTH optional, since neither is in every branch — and either branch must
    /// serve. Every other all-optional fixture in this module expects an error,
    /// which is a gap with teeth:
    /// [`a_branch_that_omits_an_optional_column_is_served`] does not cover this,
    /// because its `id` is `required: true`, and the mutation that refuses every
    /// call on an all-optional union was caught only INCIDENTALLY, by a test
    /// asserting `OutputNamingCollision`. Give that fixture a required column and
    /// the mutant survives. This test is the one that cannot be dodged that way.
    ///
    /// Both branches are asserted, not one: a rule that serves `a` and refuses
    /// `b` is a rule about the first frozen column, not about optionality.
    #[test]
    fn an_all_optional_union_serves_either_branch() {
        let snap = snapshot_with_columns(vec![
            spec("a", "integer", false),
            spec("b", "integer", false),
        ]);

        let first = result(vec![live("a", "integer")], vec![vec![serde_json::json!(1)]]);
        assert_eq!(
            shape_sole(&first, &snap).expect("the `if` branch must serve"),
            serde_json::json!([{"a": 1}]),
            "exactly the key this branch returned, and no null for the other"
        );

        let second = result(vec![live("b", "integer")], vec![vec![serde_json::json!(2)]]);
        assert_eq!(
            shape_sole(&second, &snap).expect("the `else` branch must serve"),
            serde_json::json!([{"b": 2}]),
            "the other branch of the same union serves too"
        );
    }

    /// The other half of the same rule, and the one that catches a MIS-BINDING
    /// rather than a missing key: the omitted column is the FIRST declared one.
    /// Building the object from the FROZEN names instead produces `{"id": "x"}`
    /// — a varchar served under the integer column's key, with a 200.
    ///
    /// Split out from [`a_branch_that_omits_an_optional_column_is_served`]
    /// because that test **cannot** see it: truncating a two-name zip onto a
    /// one-value row happens to produce exactly the right object when the
    /// omitted column is the last one. Measured, not assumed — the frozen-keys
    /// mutation reddened one test with the two scenarios fused and two once they
    /// were separated.
    #[test]
    fn an_omitted_column_does_not_shift_the_others_onto_its_name() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", false),
            spec("name", "varchar", true),
        ]);
        let out = result(
            vec![live("name", "varchar")],
            vec![vec![serde_json::json!("x")]],
        );
        assert_eq!(
            shape_sole(&out, &snap).expect("the optional column is the FIRST one here"),
            serde_json::json!([{"name": "x"}]),
            "the value must be served under the name the ENGINE gave it"
        );
    }

    #[test]
    fn a_missing_required_column_is_still_drift() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", false),
        ]);
        let out = result(
            vec![live("name", "varchar")],
            vec![vec![serde_json::json!("x")]],
        );
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "`required` means EVERY branch returns it, so its absence is drift"
        );
    }

    /// A snapshot frozen before `required` existed deserializes with every
    /// column `required: true` (Task 1), and that is what keeps an
    /// already-published endpoint on exact-match semantics. Pinned here on the
    /// serving side, where the loosening would have shown up.
    #[test]
    fn an_all_required_snapshot_still_demands_every_column() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", true),
        ]);
        let out = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "subset + all-required is SET equality. Pre-M1 was SEQUENCE equality, \
             so this is that contract minus its order clause and nothing else"
        );
    }

    #[test]
    fn an_undeclared_column_is_drift() {
        let snap = snapshot_with_columns(vec![spec("id", "integer", true)]);
        let out = result(
            vec![live("id", "integer"), live("secret", "varchar")],
            vec![vec![serde_json::json!(1), serde_json::json!("s")]],
        );
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "an undeclared column is a field nobody reviewed at publish reaching a caller"
        );
    }

    #[test]
    fn a_declared_column_returning_the_wrong_type_is_drift() {
        let snap = snapshot_with_columns(vec![spec("v", "varchar", true)]);
        let out = result(vec![live("v", "integer")], vec![vec![serde_json::json!(1)]]);
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "this is the gap that served 200 with an integer under a declared varchar"
        );
    }

    /// The hole containment opens that equality closed by arithmetic: a repeated
    /// name used to make the COUNTS disagree. Both columns here are declared,
    /// both type-match, and the required one is present — so every other clause
    /// passes, and without this check the `Map` keeps the last value and the
    /// column vanishes from every row.
    #[test]
    fn two_live_columns_of_one_name_are_drift() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", false),
        ]);
        let out = result(
            vec![live("id", "integer"), live("id", "integer")],
            vec![vec![serde_json::json!(1), serde_json::json!(2)]],
        );
        // Anti-triviality: the SAME two-column result with distinct declared
        // names is served, so the refusal is caused by the repeat and not by the
        // arity or by anything else in this fixture.
        let ok = result(
            vec![live("id", "integer"), live("name", "varchar")],
            vec![vec![serde_json::json!(1), serde_json::json!("x")]],
        );
        assert_eq!(
            shape_sole(&ok, &snap).expect("two distinct declared columns are fine"),
            serde_json::json!([{"id": 1, "name": "x"}])
        );
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "one key would keep the last value and the other column would be gone"
        );
    }

    /// Order stops being part of the `objects` predicate, so this pins what
    /// replaces it: each value under ITS OWN name. The old code zipped the
    /// FROZEN names onto the row, which bound 88 to `a` when the engine had
    /// returned it as `b`.
    #[test]
    fn a_reordered_result_binds_each_value_to_its_own_name() {
        let snap =
            snapshot_with_columns(vec![spec("a", "integer", true), spec("b", "integer", true)]);
        let out = result(
            vec![live("b", "integer"), live("a", "integer")],
            vec![vec![serde_json::json!(88), serde_json::json!(89)]],
        );
        let v = shape_sole(&out, &snap).expect("a reorder is not a different object");
        assert_eq!(
            v,
            serde_json::json!([{"b": 88, "a": 89}]),
            "the value the engine returned as `b` must be served as `b`"
        );
    }

    /// `apply_naming` has to reach the LIVE names, not only the frozen list the
    /// collision check walks. Dropping it there would serve the raw column name
    /// under an endpoint published as `camel`.
    #[test]
    fn naming_is_applied_to_the_columns_this_branch_returned() {
        let mut snap = snapshot_with_columns(vec![
            spec("total_revenue", "integer", true),
            spec("unit_count", "integer", false),
        ]);
        snap.output.naming = "camel".into();
        let out = result(
            vec![live("total_revenue", "integer")],
            vec![vec![serde_json::json!(7)]],
        );
        assert_eq!(
            shape_sole(&out, &snap).expect("an optional column may be absent"),
            serde_json::json!([{"totalRevenue": 7}])
        );
    }

    /// A collision between two DECLARED names refuses on every branch, including
    /// the branch that returns only one of them. Serving and the generated
    /// document have to agree about naming for the whole contract, not for the
    /// call that happened to be made.
    #[test]
    fn a_frozen_collision_refuses_even_the_branch_that_returns_one_column() {
        let mut snap = snapshot_with_columns(vec![
            spec("total_revenue", "integer", false),
            spec("totalRevenue", "integer", false),
        ]);
        snap.output.naming = "camel".into();
        let out = result(
            vec![live("total_revenue", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        assert!(
            matches!(
                shape_sole(&out, &snap),
                Err(ServingError::OutputNamingCollision)
            ),
            "the contract collides whichever branch is served"
        );
    }

    /// The relaxation must not leak onto the positional path. `arrays` cannot
    /// branch (publish refuses it), so a short live list there is drift, not a
    /// branch — and a tuple one member short cannot be read by anyone.
    #[test]
    fn an_arrays_endpoint_does_not_get_the_subset_rule() {
        let mut snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", false),
        ]);
        snap.output.shape = "arrays".into();
        let out = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        assert!(
            matches!(shape_sole(&out, &snap), Err(ServingError::SchemaDrift)),
            "a positional row that is one member short is not a contract"
        );

        // …and the same-order, same-names case still serves positionally.
        let full = result(
            vec![live("id", "integer"), live("name", "varchar")],
            vec![vec![serde_json::json!(1), serde_json::json!("x")]],
        );
        assert_eq!(
            shape_sole(&full, &snap).expect("arrays serves an equal column list"),
            serde_json::json!([[1, "x"]])
        );

        // Order is still all of the `arrays` predicate, unlike `objects`.
        let swapped = result(
            vec![live("name", "varchar"), live("id", "integer")],
            vec![vec![serde_json::json!("x"), serde_json::json!(1)]],
        );
        assert!(
            matches!(shape_sole(&swapped, &snap), Err(ServingError::SchemaDrift)),
            "a transposed tuple reads as the wrong column at every index"
        );
    }

    /// `arrays` cannot branch, but nothing stops a TYPE drifting under it — and
    /// a positional row carries no names, so a client reading `data[i][0]` as the
    /// declared `varchar` has nothing in the body to notice the integer by. The
    /// type comparison is therefore on both paths, not only on `objects`.
    #[test]
    fn an_arrays_endpoint_refuses_a_declared_column_that_changed_type() {
        let mut snap = snapshot_with_columns(vec![spec("v", "varchar", true)]);
        snap.output.shape = "arrays".into();

        let drifted = result(vec![live("v", "integer")], vec![vec![serde_json::json!(1)]]);
        assert!(
            matches!(shape_sole(&drifted, &snap), Err(ServingError::SchemaDrift)),
            "a positional column whose type moved is drift, exactly as under objects"
        );

        // Anti-triviality: the same one-column, same-order result with the
        // DECLARED type still serves positionally, so the refusal is caused by
        // the type and not by the fixture.
        let ok = result(
            vec![live("v", "varchar")],
            vec![vec![serde_json::json!("x")]],
        );
        assert_eq!(
            shape_sole(&ok, &snap).expect("the declared type is served"),
            serde_json::json!([["x"]])
        );
    }

    /// A live result with NO columns, against a union where every column is
    /// optional — which is what `SELECT {% if x %}a{% else %}b{% endif %}`
    /// freezes, since neither column is in every branch.
    ///
    /// Every containment clause passes vacuously here: nothing is undeclared, no
    /// `required` column is missing, no name repeats. Before the explicit
    /// refusal this measured `Ok("[{}]")` — a 200 carrying one empty object per
    /// row, under a `meta.columns` advertising two. The pre-M1 count equality
    /// refused it by arithmetic, so accepting it would make this gate looser
    /// than the one it replaced.
    #[test]
    fn a_result_with_no_columns_at_all_is_drift() {
        let snap = snapshot_with_columns(vec![
            spec("a", "integer", false),
            spec("b", "integer", false),
        ]);
        assert!(
            matches!(
                shape_sole(&result(vec![], vec![vec![]]), &snap),
                Err(ServingError::SchemaDrift)
            ),
            "an all-optional union must not turn an empty result into 200 [{{}}]"
        );
        // With no rows either — the loop below the guard would never run, so the
        // refusal has to come from the column list and not from a row.
        assert!(
            matches!(
                shape_sole(&result(vec![], vec![]), &snap),
                Err(ServingError::SchemaDrift)
            ),
            "no rows is not a reason to stop checking the columns"
        );
        // And on the positional path, where it would otherwise serve `[[]]`.
        let mut arrays = snapshot_with_columns(vec![spec("a", "integer", false)]);
        arrays.output.shape = "arrays".into();
        assert!(matches!(
            shape_sole(&result(vec![], vec![vec![]]), &arrays),
            Err(ServingError::SchemaDrift)
        ));
    }

    /// The row-arity guard measures against the LIVE column list now, because a
    /// branch's rows are as wide as the columns THAT BRANCH returned. Against
    /// the frozen union, a legitimate short branch would report a connector bug.
    #[test]
    fn a_row_is_measured_against_the_live_column_list() {
        let snap = snapshot_with_columns(vec![
            spec("id", "integer", true),
            spec("name", "varchar", false),
        ]);
        // One live column, one value: correct, and two values wide of the frozen
        // union would be the reading that refuses it.
        assert!(shape_sole(
            &result(
                vec![live("id", "integer")],
                vec![vec![serde_json::json!(1)]]
            ),
            &snap
        )
        .is_ok());
        // One live column, two values: a connector bug, and still refused.
        let bug = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1), serde_json::json!(2)]],
        );
        assert!(
            matches!(shape_sole(&bug, &snap), Err(ServingError::Internal { .. })),
            "a row that disagrees with the connector's own column list is a fault"
        );
    }

    fn snapshot_with_limits(max_rows: u32, timeout_seconds: u32) -> EndpointSnapshot {
        EndpointSnapshot {
            statements: vec![egret_dataapi_api::StatementSnapshot {
                name: "main".into(),
                source: egret_dataapi_api::StatementSource::Sql("SELECT 1".into()),
                columns: vec![],
            }],
            iterated: Vec::new(),
            method: "GET".into(),
            path: "/x".into(),
            database_id: Uuid::nil(),
            params: vec![],
            output: serde_json::from_value(serde_json::json!({})).unwrap(),
            cache: serde_json::from_value(serde_json::json!({})).unwrap(),
            limits: egret_dataapi_api::LimitsSpec {
                max_rows,
                timeout_seconds,
            },
            rate_limit: serde_json::from_value(serde_json::json!({})).unwrap(),
            delegated: false,
            impersonate: false,
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }

    // -----------------------------------------------------------------------
    // M2 Task 7 — per-statement shaping, and what fills `data`
    // -----------------------------------------------------------------------

    fn stmt(name: &str, sql: &str, columns: Vec<ColumnSpec>) -> StatementSnapshot {
        StatementSnapshot {
            name: name.into(),
            source: egret_dataapi_api::StatementSource::Sql(sql.into()),
            columns,
        }
    }

    /// **Each statement is judged against ITS OWN frozen columns.**
    ///
    /// The crossed case at the end is the defect this task closes, stated at the
    /// level the six clauses live at: the `total` statement's result, judged
    /// against the `rows` statement's contract, is a drift — so a serving path
    /// that shaped everything against `statements[0]` would answer `503` on an
    /// endpoint nobody had changed. Both directions are asserted, because a
    /// `shape_rows` that had stopped comparing anything would pass the first two
    /// alone.
    #[test]
    fn each_statement_is_shaped_against_its_own_frozen_columns() {
        let rows = stmt(
            "rows",
            "SELECT id FROM t",
            vec![spec("id", "integer", true)],
        );
        let total = stmt(
            "total",
            "SELECT count(*) AS total FROM t",
            vec![spec("total", "bigint", true)],
        );
        let output = OutputSpec::default();

        let rows_out = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        let total_out = result(
            vec![live("total", "bigint")],
            vec![vec![serde_json::json!(9)]],
        );

        assert_eq!(
            shape_rows(&rows_out, &output, &rows).expect("its own contract"),
            serde_json::json!([{"id": 1}]),
        );
        assert_eq!(
            shape_rows(&total_out, &output, &total).expect("its own contract"),
            serde_json::json!([{"total": 9}]),
        );

        assert!(
            matches!(
                shape_rows(&total_out, &output, &rows),
                Err(ServingError::SchemaDrift)
            ),
            "the second statement's result against the first's contract is a \
             drift — which is what a path shaping everything against \
             `statements[0]` would answer for it",
        );
    }

    /// `data` is the rows for one statement and an object keyed by name for
    /// several (spec §4.4).
    ///
    /// The one-statement arm is the compatibility claim `LEGACY_STATEMENT_NAME`
    /// rests on: every endpoint published before M2 has exactly one statement,
    /// so the synthesized name `main` never reaches the wire.
    #[test]
    fn one_statement_is_an_array_and_several_are_keyed_by_name() {
        let rows = serde_json::json!([{"id": 1}]);
        assert_eq!(
            statements_data(vec![("main", rows.clone())]),
            rows,
            "one statement's body is the rows themselves, under no name at all",
        );

        assert_eq!(
            statements_data(vec![
                ("rows", rows.clone()),
                ("total", serde_json::json!([{"total": 9}]))
            ]),
            serde_json::json!({"rows": [{"id": 1}], "total": [{"total": 9}]}),
        );
    }

    /// **The contract is keyed the only way the statement count permits.**
    ///
    /// This replaces Task 7's `the_reported_contract_covers_every_statement`,
    /// which pinned `contract_columns` — the interim that reported every
    /// statement's frozen columns CONCATENATED into one flat list because
    /// `meta.columns` had no way to key them. Task 7 wrote it expecting this
    /// task to redden it, and it did: `contract_columns` no longer exists.
    ///
    /// What carried over is the property that interim existed to hold — a second
    /// statement's columns are never dropped, and one statement's are never
    /// reported as if they were the endpoint's — asserted here against the shape
    /// that can actually say which is which.
    #[test]
    fn the_contract_is_keyed_by_statement_once_there_is_more_than_one() {
        let base = snapshot_with_limits(1, 1);
        let one = EndpointSnapshot {
            statements: vec![stmt("main", "SELECT 1", vec![spec("id", "integer", true)])],
            ..base.clone()
        };
        match meta_contract(&one, &[7]) {
            MetaContract::Single { row_count, columns } => {
                assert_eq!(row_count, 7);
                assert_eq!(
                    columns,
                    vec![spec("id", "integer", true)],
                    "one statement reports exactly what it froze, under no name \
                     at all — the shape every endpoint published before M2 has",
                );
            }
            other => panic!("one statement must not be keyed by name: {other:?}"),
        }

        let two = EndpointSnapshot {
            statements: vec![
                stmt("rows", "SELECT 1", vec![spec("id", "integer", true)]),
                stmt("total", "SELECT 2", vec![spec("total", "bigint", true)]),
            ],
            ..base
        };
        match meta_contract(&two, &[3, 1]) {
            MetaContract::Several { statements } => {
                assert_eq!(
                    statements
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>(),
                    vec!["rows", "total"],
                    "keyed by name, in the author's published order",
                );
                assert_eq!(statements[0].1.row_count, 3);
                assert_eq!(statements[1].1.row_count, 1);
                assert_eq!(
                    statements[0].1.columns,
                    vec![spec("id", "integer", true)],
                    "each statement's own frozen columns, not the concatenation \
                     and not the first statement's list",
                );
                assert_eq!(statements[1].1.columns, vec![spec("total", "bigint", true)]);
            }
            other => panic!("several statements must be keyed by name: {other:?}"),
        }
    }

    /// The two shapes are chosen by the SNAPSHOT, so `data` and `meta` cannot
    /// disagree about which one this endpoint has.
    ///
    /// Driven with a count of ZERO for the only statement, which is the reading
    /// that would tempt an implementation to branch on the counts instead: an
    /// endpoint does not stop being a one-statement endpoint because its
    /// statement returned nothing.
    #[test]
    fn the_envelope_shape_follows_the_snapshot_not_this_call() {
        let one = snapshot_with_columns(vec![spec("id", "integer", true)]);
        assert!(
            matches!(meta_contract(&one, &[0]), MetaContract::Single { .. }),
            "one statement returning no rows is still one statement",
        );
        assert_eq!(
            statements_data(vec![("main", serde_json::json!([]))]),
            serde_json::json!([]),
            "and `data` agrees: the empty rows array, under no name",
        );
    }

    /// `envelope` falls back like `shape` and `naming`, and the default is the
    /// WRAPPER.
    ///
    /// The default matters more than the fallback: `OutputSpec`'s serde default
    /// is what every snapshot frozen before M2 reads back as, so a default of
    /// `bare` would strip the wrapper off every endpoint published since
    /// 2026-08-16 without anybody re-publishing.
    #[test]
    fn an_unrecognised_envelope_falls_back_to_wrapped() {
        let mut output = OutputSpec::default();
        assert_eq!(envelope_of(&output), Envelope::Wrapped);

        output.envelope = "bare".into();
        assert_eq!(envelope_of(&output), Envelope::Bare);

        for junk in ["", "Bare", "wrapped ", "true", "nonsense"] {
            output.envelope = junk.into();
            assert_eq!(
                envelope_of(&output),
                Envelope::Wrapped,
                "an unrecognised envelope serves the documented default: {junk:?}",
            );
        }
    }

    /// `status` is read off the status line, so the two cannot disagree.
    #[test]
    fn the_status_field_mirrors_the_http_status() {
        assert_eq!(ServingStatus::mirroring(StatusCode::OK), ServingStatus::Ok);
        for bad in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(
                ServingStatus::mirroring(bad),
                ServingStatus::Error,
                "{bad} is not a success",
            );
        }
    }

    /// **Every clause that reads a frozen contract reads the one it was HANDED**,
    /// stated clause by clause rather than once.
    ///
    /// This is the hazard M2 Task 4 recorded for this task: `shape_rows` judged
    /// six things against one `frozen` binding, and converting some of them to
    /// per-statement while leaving others reading `statements[0]` is a hole no
    /// single-statement test can show — at N=1 the two readings ARE the same
    /// list. So each case below hands the SAME live result to TWO statements and
    /// requires the verdicts to DISAGREE; a clause still reading a fixed
    /// statement would answer both the same way.
    ///
    /// The three clauses not covered here do not read a frozen list at all:
    /// row arity and the zero-column refusal judge `out`, and the live-duplicate
    /// refusal judges `out.columns`. They are pinned in the tests above and
    /// cannot become per-statement, because there is nothing per-statement in
    /// them to get wrong.
    #[test]
    fn every_clause_judges_the_statement_it_was_given() {
        let output = OutputSpec::default();

        // 1. Containment. `total` is declared by one statement and not by the
        //    other, so the same result is served under one and refused under it.
        let rows = stmt("rows", "SELECT id", vec![spec("id", "integer", true)]);
        let total = stmt(
            "total",
            "SELECT count(*)",
            vec![spec("total", "bigint", true)],
        );
        let total_out = result(
            vec![live("total", "bigint")],
            vec![vec![serde_json::json!(9)]],
        );
        assert_eq!(
            shape_rows(&total_out, &output, &total).expect("its own contract"),
            serde_json::json!([{"total": 9}]),
        );
        assert!(
            matches!(
                shape_rows(&total_out, &output, &rows),
                Err(ServingError::SchemaDrift)
            ),
            "an undeclared column is judged against the statement handed in",
        );

        // 2. `required`. One statement requires a column the result omits; the
        //    other declares the same column optional.
        let strict = stmt(
            "strict",
            "SELECT a, b",
            vec![spec("a", "integer", true), spec("b", "integer", true)],
        );
        let lax = stmt(
            "lax",
            "SELECT a, b",
            vec![spec("a", "integer", true), spec("b", "integer", false)],
        );
        let partial = result(vec![live("a", "integer")], vec![vec![serde_json::json!(1)]]);
        assert_eq!(
            shape_rows(&partial, &output, &lax).expect("an optional column may be absent"),
            serde_json::json!([{"a": 1}]),
        );
        assert!(
            matches!(
                shape_rows(&partial, &output, &strict),
                Err(ServingError::SchemaDrift)
            ),
            "`required` is read from the statement handed in",
        );

        // 3. Types.
        let as_varchar = stmt("as_varchar", "SELECT v", vec![spec("v", "varchar", true)]);
        let as_integer = stmt("as_integer", "SELECT v", vec![spec("v", "integer", true)]);
        let integer_out = result(vec![live("v", "integer")], vec![vec![serde_json::json!(1)]]);
        assert_eq!(
            shape_rows(&integer_out, &output, &as_integer).expect("the declared type is served"),
            serde_json::json!([{"v": 1}]),
        );
        assert!(
            matches!(
                shape_rows(&integer_out, &output, &as_varchar),
                Err(ServingError::SchemaDrift)
            ),
            "the declared type is read from the statement handed in",
        );

        // 4. The `output.naming` collision, which is judged over the FROZEN
        //    union rather than over this call's columns — so it too is a
        //    property of one statement, and under `camel` one of these two folds
        //    its declared names onto one key while the other does not.
        let camel = OutputSpec {
            naming: "camel".into(),
            ..OutputSpec::default()
        };
        let colliding = stmt(
            "colliding",
            "SELECT total_revenue, totalRevenue",
            vec![
                spec("total_revenue", "integer", false),
                spec("totalRevenue", "integer", false),
            ],
        );
        let clean = stmt("clean", "SELECT id", vec![spec("id", "integer", true)]);
        let id_out = result(
            vec![live("id", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        assert_eq!(
            shape_rows(&id_out, &camel, &clean).expect("no two frozen names fold together"),
            serde_json::json!([{"id": 1}]),
        );
        let one_of_them = result(
            vec![live("total_revenue", "integer")],
            vec![vec![serde_json::json!(1)]],
        );
        assert!(
            matches!(
                shape_rows(&one_of_them, &camel, &colliding),
                Err(ServingError::OutputNamingCollision)
            ),
            "the collision is judged over the frozen union of the statement \
             handed in — and it is the OTHER 503, which is why the variant is \
             matched rather than the status",
        );
    }

    /// **A drift is logged against the statement that drifted.**
    ///
    /// The operator reads this one line to decide what moved. While `drift`
    /// fetched its own columns from `statements[0]` it would have named
    /// the FIRST statement's contract for a drift in the second — sending them
    /// to a statement that was fine, silently, at exactly the moment they need
    /// the truth. Both statements are exercised, because a line hard-coded to
    /// either one would satisfy a test that only ever drifts the same one.
    #[test]
    fn a_drift_is_logged_against_the_statement_that_drifted() {
        let output = OutputSpec::default();
        let rows = stmt("rows", "SELECT id", vec![spec("id", "integer", true)]);
        let total = stmt(
            "total",
            "SELECT count(*)",
            vec![spec("total", "bigint", true)],
        );
        let renamed = result(
            vec![live("grand_total", "bigint")],
            vec![vec![serde_json::json!(9)]],
        );

        let logs = logs_from_statement_shaping(&renamed, &output, &total);
        assert!(logs.contains("statement=total"), "{logs}");
        assert!(
            logs.contains(r#"frozen=["total"]"#),
            "the frozen list logged is the drifting statement's own: {logs}"
        );
        assert!(
            !logs.contains("statement=rows") && !logs.contains(r#"frozen=["id"]"#),
            "a sibling statement, which is fine, must not appear: {logs}"
        );

        // The other one, so neither assertion above can be satisfied by a
        // constant.
        let logs = logs_from_statement_shaping(&renamed, &output, &rows);
        assert!(logs.contains("statement=rows"), "{logs}");
        assert!(logs.contains(r#"frozen=["id"]"#), "{logs}");
    }

    // -----------------------------------------------------------------------
    // C2 Task 6 — a transform produces `data`, and fails closed
    // -----------------------------------------------------------------------

    /// A snapshot whose ONE statement is `main` returning `n`, carrying `script`
    /// as its transform and `returns` as the output it declares.
    fn transformed_snapshot(script: &str, returns: Vec<ColumnSpec>) -> EndpointSnapshot {
        let base = snapshot_with_limits(1, 1);
        EndpointSnapshot {
            statements: vec![stmt(
                "main",
                "SELECT n FROM t",
                vec![spec("n", "integer", true)],
            )],
            output: OutputSpec {
                transform: Some(TransformSpec {
                    script: script.into(),
                    returns,
                }),
                ..OutputSpec::default()
            },
            ..base
        }
    }

    /// `data` for a transformed snapshot, driven through the REAL function the
    /// serving path calls — `transform_response`, which is what
    /// `transformed_data` runs inside its `spawn_blocking`, and nothing else.
    ///
    /// The rows are keyed per statement exactly as `transformed_data` keys them,
    /// so a script written against `statements["main"]` here is the script that
    /// would run in production.
    fn build_data(
        snap: &EndpointSnapshot,
        rows: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, ServingError> {
        build_data_with(snap, rows, &BTreeMap::new(), false)
    }

    fn build_data_with(
        snap: &EndpointSnapshot,
        rows: Vec<serde_json::Value>,
        params: &BTreeMap<String, serde_json::Value>,
        has_repeats: bool,
    ) -> Result<serde_json::Value, ServingError> {
        let spec = snap
            .output
            .transform
            .as_ref()
            .expect("this helper is for transformed fixtures");
        let statements: BTreeMap<String, Vec<serde_json::Value>> = snap
            .statements
            .iter()
            .map(|s| (s.name.clone(), rows.clone()))
            .collect();
        transform_response(spec, params, &statements, has_repeats)
    }

    /// The whole success body, serialised — the object a consumer actually
    /// reads, built from the real envelope types and the real `meta_contract`.
    fn build_body(
        snap: &EndpointSnapshot,
        row_counts: &[u64],
        data: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::to_value(ServingResponse {
            status: ServingStatus::mirroring(StatusCode::OK),
            message: None,
            data,
            meta: ServingMeta {
                version: 3,
                cached: false,
                request_id: "req-1".into(),
                contract: meta_contract(snap, row_counts),
            },
        })
        .expect("the serving envelope serialises")
    }

    fn one_row() -> Vec<serde_json::Value> {
        vec![serde_json::json!({"n": 7})]
    }

    /// What `params::resolve` returns for a request, carrying an
    /// already-resolved `params` dict. Built through the real struct so a field
    /// added to it is a compile error here rather than a default nobody chose.
    fn resolved_with(json: BTreeMap<String, serde_json::Value>) -> params::Resolved {
        params::Resolved {
            values: crate::render::ParamValues::new(),
            key: BTreeMap::new(),
            has_repeats: false,
            json,
            generation: crate::transform::GenerationParams::default(),
        }
    }

    /// **The real guarantee.**
    ///
    /// Publish's probe cannot prove a script always conforms — it runs against
    /// synthetic rows and an empty set. This is what holds when the data is not
    /// what the probe assumed, and the script below is written so the two
    /// disagree on purpose: it returns the declared object for the probe's
    /// stand-in `1` and an undeclared field for anything else, so it publishes
    /// green and must still be refused here.
    #[test]
    fn a_transformed_response_is_validated_against_returns_and_fails_closed() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  \
             rows = statements[\"main\"]\n  \
             if len(rows) == 0 or rows[0][\"n\"] == 1:\n    \
             return {\"total\": 0}\n  \
             return {\"surprise\": 1}",
            vec![spec("total", "bigint", true)],
        );

        // The shape publish's probe sees: `probe_value("integer")` is 1.
        assert_eq!(
            build_data(&snap, vec![serde_json::json!({"n": 1})])
                .expect("the probe's own row is what publish accepted"),
            serde_json::json!({"total": 0}),
        );

        // A real row, and the same script answers something the endpoint never
        // declared.
        let err = build_data(&snap, one_row())
            .expect_err("undeclared `surprise` must not reach a caller");
        assert!(
            matches!(err, ServingError::Internal { .. }),
            "a response that contradicts `returns` is refused, not served: {err:?}"
        );
        assert!(
            format!("{err:?}").contains("`surprise`"),
            "the operator is told which field: {err:?}"
        );
    }

    /// A missing declared field fails closed too, not only an extra one.
    ///
    /// Without this, a check that only looked for undeclared keys would pass
    /// every assertion above while serving a body with no `total` in it to a
    /// client reading `body["total"]`.
    #[test]
    fn a_response_missing_a_declared_field_is_refused_as_well() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  return {}",
            vec![spec("total", "bigint", true)],
        );
        let err = build_data(&snap, one_row()).expect_err("`total` was declared required");
        assert!(matches!(err, ServingError::Internal { .. }), "{err:?}");
        assert!(format!("{err:?}").contains("`total`"), "{err:?}");
    }

    /// A `required: false` field may be absent — the serving check is the SAME
    /// rule publish probed with, so it must not be stricter than the one an
    /// author published against.
    #[test]
    fn an_optional_declared_field_may_be_absent() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  return {\"total\": 1}",
            vec![
                spec("total", "bigint", true),
                spec("note", "varchar", false),
            ],
        );
        assert_eq!(
            build_data(&snap, one_row()).expect("an optional field may be omitted"),
            serde_json::json!({"total": 1}),
        );
    }

    /// **The caller sent a valid request; the ENDPOINT is broken.**
    ///
    /// A 400 would blame the caller for the author's script and put
    /// `param_invalid` in `access_log` — the one table an operator reads to find
    /// a broken endpoint would report a client-side mistake. The two 400
    /// variants are ruled out by name as well, because those are the two a
    /// careless mapping would actually reach for.
    #[test]
    fn a_transform_failure_is_an_internal_error_never_a_400() {
        for script in [
            // An explicit `fail()`.
            "def transform(params, statements):\n  fail(\"boom\")",
            // A runtime fault the author did not intend.
            "def transform(params, statements):\n  return statements[\"absent\"]",
            // No entry point at all — publish refuses this, so reaching it means
            // a snapshot frozen by some other path.
            "x = 1",
            // A value `returns` cannot describe, because it has no named fields.
            "def transform(params, statements):\n  return [1, 2, 3]",
        ] {
            let snap = transformed_snapshot(script, vec![spec("total", "bigint", true)]);
            let err = build_data(&snap, one_row()).expect_err("the script did not conform");
            assert!(
                matches!(err, ServingError::Internal { .. }),
                "must be internal: {err:?} for {script:?}"
            );
            assert!(
                !matches!(err, ServingError::ParamInvalid | ServingError::ParamMissing),
                "must not be ParamInvalid or ParamMissing: {err:?} for {script:?}"
            );
        }
    }

    /// A repeat may inform the MESSAGE and must never choose the STATUS.
    ///
    /// It is the one way a parameter reaches a script in a shape publish's probe
    /// never showed the author, so the operator gets told; it is also entirely
    /// caller-controlled, which is why it cannot be allowed to downgrade a 500
    /// to a 400 — the defect `render_error` records at length, one layer up.
    #[test]
    fn a_repeated_parameter_is_named_in_the_detail_and_changes_no_status() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  fail(\"boom\")",
            vec![spec("total", "bigint", true)],
        );
        let params = BTreeMap::from([("region".to_string(), serde_json::json!(["EU", "US"]))]);

        let with = build_data_with(&snap, one_row(), &params, true).expect_err("the script raised");
        assert!(matches!(with, ServingError::Internal { .. }), "{with:?}");
        assert!(
            format!("{with:?}").contains("more than once"),
            "the operator is told the one fact that distinguishes this call: {with:?}"
        );

        let without =
            build_data_with(&snap, one_row(), &params, false).expect_err("the script raised");
        assert!(
            matches!(without, ServingError::Internal { .. }),
            "the status is the same either way: {without:?}"
        );
        assert!(
            !format!("{without:?}").contains("more than once"),
            "and the note is not a constant: {without:?}"
        );
    }

    /// A script is handed the parameters the request resolved, and reads them.
    ///
    /// Driven through a script that fails unless the value arrived, so an
    /// implementation passing an empty dict reddens this rather than returning a
    /// plausible object. The second half is the repeat gap stated as a
    /// measurement: the list the caller sent reaches the script UNCOLLAPSED,
    /// because the script and the SQL must be two readings of one request.
    #[test]
    fn the_script_reads_the_bound_parameters() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  \
             if params[\"region\"] != \"EU\":\n    \
             fail(\"saw \" + str(params[\"region\"]))\n  \
             return {\"total\": 1}",
            vec![spec("total", "bigint", true)],
        );
        let params = BTreeMap::from([("region".to_string(), serde_json::json!("EU"))]);
        assert_eq!(
            build_data_with(&snap, one_row(), &params, false).expect("the parameter arrived"),
            serde_json::json!({"total": 1}),
        );

        let repeated = BTreeMap::from([("region".to_string(), serde_json::json!(["EU", "US"]))]);
        let err = build_data_with(&snap, one_row(), &repeated, true)
            .expect_err("a repeat is a list, and this script expects a scalar");
        assert!(
            format!("{err:?}").contains(r#"["EU", "US"]"#),
            "the script saw the list the request actually sent, uncollapsed: {err:?}"
        );
    }

    /// **`meta.row_count` is omitted for a transformed endpoint.**
    ///
    /// `total_row_count()` sums the statements, so the number would be
    /// *consistent* with `meta.statements` and still be read as describing
    /// `data` — which after a transform is the single object below.
    #[test]
    fn meta_row_count_is_omitted_for_a_transformed_endpoint() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  return {\"total\": 1}",
            vec![spec("total", "bigint", true)],
        );
        let data = build_data(&snap, one_row()).expect("runs");
        let body = build_body(&snap, &[2], data);
        assert!(
            body["meta"].get("row_count").is_none(),
            "row_count describes the SQL, and a consumer reads it as describing \
             `data`: {body}"
        );
        assert_eq!(body["data"], serde_json::json!({"total": 1}), "{body}");
    }

    /// **`meta.statements` still describes the SQL.** Two different truths, kept
    /// separate: what the engine returned, and what the endpoint promised.
    ///
    /// Asserted as a whole object rather than key by key, because the claim is
    /// that the SQL half of `meta` is untouched by the transform — a per-key
    /// assertion would pass on a `meta` that had quietly grown or lost one.
    #[test]
    fn meta_statements_still_describes_the_sql_after_a_transform() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  return {\"total\": 1}",
            vec![spec("total", "bigint", true)],
        );
        let data = build_data(&snap, one_row()).expect("runs");
        let body = build_body(&snap, &[2], data);
        assert!(body["meta"]["statements"].is_object(), "{body}");
        assert_eq!(
            body["meta"]["statements"],
            serde_json::json!({
                "main": {
                    "row_count": 2,
                    "columns": [{"name": "n", "ty": "integer", "required": true}],
                }
            }),
            "the SQL's own name, count and frozen columns — not the transform's \
             `returns`, and not the length of `data`: {body}"
        );
    }

    /// **An untransformed endpoint is byte for byte unchanged.**
    ///
    /// The whole feature must be invisible to every endpoint that does not use
    /// it, and this is the test that catches a refactor which "just moves" the
    /// shaping code. The literal below is the pre-C2 body for one statement:
    /// `data` is the rows themselves under no name, `meta` carries a top-level
    /// `row_count` and `columns`, and there is no `statements` key at all.
    ///
    /// The `shape`/`naming` half is asserted too, because that is the half C2
    /// touched: `shaping_output` must hand `shape_rows` the endpoint's OWN
    /// output spec, and `Cow::Borrowed` is the evidence it did not build a
    /// canonical one behind its back.
    #[test]
    fn an_untransformed_endpoint_is_byte_for_byte_unchanged() {
        let snap = snapshot_with_columns(vec![spec("n", "integer", true)]);
        assert!(
            snap.output.transform.is_none(),
            "the fixture is the old world"
        );

        let out = result(vec![live("n", "integer")], vec![vec![serde_json::json!(7)]]);
        let data = statements_data(vec![(
            snap.statements[0].name.as_str(),
            shape_rows(&out, &shaping_output(&snap.output), &snap.statements[0]).expect("shapes"),
        )]);

        assert_eq!(
            build_body(&snap, &[1], data),
            serde_json::json!({
                "status": "ok",
                "message": null,
                "data": [{"n": 7}],
                "meta": {
                    "version": 3,
                    "cached": false,
                    "request_id": "req-1",
                    "row_count": 1,
                    "columns": [{"name": "n", "ty": "integer", "required": true}],
                }
            }),
        );

        // And the spec handed to `shape_rows` is the endpoint's own, not a copy.
        for naming in ["as_is", "camel", "snake"] {
            for shape in ["objects", "arrays"] {
                let output = OutputSpec {
                    shape: shape.into(),
                    naming: naming.into(),
                    ..OutputSpec::default()
                };
                assert!(
                    matches!(shaping_output(&output), Cow::Borrowed(_)),
                    "an untransformed endpoint is shaped against its own spec: \
                     {shape}/{naming}"
                );
            }
        }
    }

    /// **A script is handed the rows publish probed it with**, whatever the
    /// endpoint's presentation knobs say.
    ///
    /// `lifecycle::probe_row` builds `{column name: value}` objects from the
    /// described columns and applies neither `shape` nor `naming`. If serving
    /// applied them, a script probed against `total_revenue` would be called
    /// with `totalRevenue` under `camel`, and with a positional LIST under
    /// `arrays` — green at publish, broken on every real call.
    #[test]
    fn a_transformed_endpoint_shapes_the_rows_the_script_was_probed_with() {
        for (shape, naming) in [
            ("objects", "camel"),
            ("arrays", "as_is"),
            ("arrays", "snake"),
            ("nonsense", "kebab"),
        ] {
            let output = OutputSpec {
                shape: shape.into(),
                naming: naming.into(),
                transform: Some(TransformSpec {
                    script: "def transform(params, statements):\n  return {}".into(),
                    returns: vec![],
                }),
                ..OutputSpec::default()
            };
            let shaping = shaping_output(&output);
            assert_eq!(shaping.shape, "objects", "{shape}/{naming}");
            assert_eq!(shaping.naming, "as_is", "{shape}/{naming}");

            // And it is the rows themselves, not just the two strings: the
            // frozen name survives verbatim into what the script reads.
            let declared = stmt(
                "main",
                "SELECT total_revenue",
                vec![spec("total_revenue", "integer", true)],
            );
            let out = result(
                vec![live("total_revenue", "integer")],
                vec![vec![serde_json::json!(7)]],
            );
            assert_eq!(
                shape_rows(&out, &shaping, &declared).expect("shapes"),
                serde_json::json!([{"total_revenue": 7}]),
                "{shape}/{naming}",
            );
        }
    }

    /// The contract clauses `shape_rows` judges still run for a transformed
    /// endpoint, so a script never sees a row a drifted endpoint produced.
    ///
    /// Each of these is refused BEFORE the transform, which is why the whole
    /// call stays a 503 rather than becoming a 500 — the operator's fix is to
    /// re-publish, not to read a script.
    #[test]
    fn drift_is_still_refused_before_a_script_sees_a_row() {
        let output = OutputSpec {
            naming: "camel".into(),
            transform: Some(TransformSpec {
                script: "def transform(params, statements):\n  return {}".into(),
                returns: vec![],
            }),
            ..OutputSpec::default()
        };
        let shaping = shaping_output(&output);
        let declared = stmt(
            "main",
            "SELECT id",
            vec![spec("id", "integer", true), spec("name", "varchar", false)],
        );

        for (why, out) in [
            (
                "a column that was never declared",
                result(
                    vec![live("nope", "integer")],
                    vec![vec![serde_json::json!(1)]],
                ),
            ),
            (
                "a declared column that changed type",
                result(
                    vec![live("id", "varchar")],
                    vec![vec![serde_json::json!(1)]],
                ),
            ),
            (
                "a required column that was not returned",
                result(
                    vec![live("name", "varchar")],
                    vec![vec![serde_json::json!(1)]],
                ),
            ),
            ("no columns at all", result(vec![], vec![])),
        ] {
            assert!(
                matches!(
                    shape_rows(&out, &shaping, &declared),
                    Err(ServingError::SchemaDrift)
                ),
                "{why} must be refused before the script runs",
            );
        }
    }

    // -----------------------------------------------------------------------
    // C2 Task 6, fix round 1 — the request deadline, and the two arms that
    // answer when the script does not
    // -----------------------------------------------------------------------

    /// The two facts a refusal is judged on: the status a caller sees and the
    /// code that lands in `access_log`. Read off the REAL wire mapping rather
    /// than matched on the variant, because those two are the payload and the
    /// variant is only how it is spelled.
    fn wire_of(err: &ServingError) -> (StatusCode, &'static str) {
        let w = to_wire(err);
        (w.status, w.code)
    }

    /// **What [`transformed_data`] hands the script**: every statement keyed by
    /// its own NAME, and the request's resolved parameters.
    ///
    /// Driven with TWO statements, because at N = 1 a path that keyed by index,
    /// or that handed over only the first statement, is indistinguishable from
    /// the correct one — the same reading M2 had to remove from `shape_rows`.
    /// The script reads from both keys and from `params`, so an assembly that
    /// dropped any of the three fails rather than returning a plausible object.
    ///
    /// Note what `data` does NOT do here: `statements_data` unwraps a single
    /// statement's rows onto the wire under no name, and a script never sees
    /// that unwrapping — its dict is keyed at every count.
    #[tokio::test]
    async fn a_transform_is_handed_every_statement_by_name_and_the_request_s_parameters() {
        let snap = transformed_snapshot(
            "def transform(params, statements):\n  \
             return {\"total\": statements[\"total\"][0][\"n\"] + len(statements[\"rows\"]), \
             \"region\": params[\"region\"]}",
            vec![
                spec("total", "bigint", true),
                spec("region", "varchar", true),
            ],
        );
        let resolved = resolved_with(BTreeMap::from([(
            "region".to_string(),
            serde_json::json!("EU"),
        )]));

        assert_eq!(
            transformed_data(
                snap.output.transform.as_ref().expect("transformed fixture"),
                vec![
                    ("rows", serde_json::json!([{"n": 1}, {"n": 2}])),
                    ("total", serde_json::json!([{"n": 40}])),
                ],
                &resolved,
                ScriptDeadline::default(),
            )
            .await
            .expect("the script reads both statements and the parameter"),
            serde_json::json!({"total": 42, "region": "EU"}),
        );
    }

    /// **The request's own deadline is what refuses work that has not
    /// finished — and it stops the WAITING, not the work.**
    ///
    /// This is the test that did not exist. `transformed_data`'s
    /// `tokio::time::timeout` could be deleted outright and nothing went red,
    /// because the only way to reach the arm was to wait five real seconds.
    /// [`ScriptDeadline`] is the injection point that makes it reachable, the
    /// same way `transform::Budgets` makes a tick budget reachable.
    ///
    /// # Two dead ends, recorded because each looked correct
    ///
    /// **A slow script.** An unbounded starlark loop with a 50 ms deadline
    /// works, and it reddened
    /// `transform::a_wide_shared_fan_out_is_stopped_by_the_output_deadline` in
    /// the same binary about half the time: that test asserts which of two
    /// output-deadline pollers fires first inside one evaluation, so it loses to
    /// any concurrent CPU, and libtest runs the two in parallel. The load came
    /// from the run this test stops waiting for — detached, still burning a core
    /// for the full `transform::MAX_WALL_CLOCK` after the answer. Shrinking it
    /// to a ~39 ms loop still measured 3 failures in 12 runs.
    ///
    /// The neighbour is load-sensitive **on its own** — 2 failures in 24 runs on
    /// the commit before Task 6 existed, so it is genuinely pre-existing — and
    /// that is not licence to make it worse. The version below measures **2 in
    /// 24, the same rate**, which is the check that this test is not a
    /// contributor; the 12-run numbers above are too small to separate 1 from 3
    /// and are quoted as the signal that sent this looking, not as a rate.
    ///
    /// **A trivial script with a `Duration::ZERO` deadline.** The reasoning said
    /// the poll must win, since there is no yield point between `spawn_blocking`
    /// and the first poll. Measured, **the blocking task completes first and
    /// `timeout` returns `Ok`** — it polls the inner future before it looks at
    /// the clock, and a one-line parse on an already-warm pool thread is faster
    /// than that reasoning assumed.
    ///
    /// # What works: park the thread instead of burning it
    ///
    /// [`run_under_deadline`] is the seam. The closure below blocks on a channel
    /// this test owns, so it consumes no CPU, cannot finish early, and is
    /// released deliberately at the end — deterministic in any build profile.
    ///
    /// The `finished` flag is the part worth having beyond the arm itself: it is
    /// still `false` when the caller has been answered, which is the module
    /// comment's central claim about detachment turned into a measurement.
    #[tokio::test]
    async fn the_request_deadline_stops_the_waiting_and_not_the_work() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // 1. Work that finishes is served, so the refusal below is not "every
        //    transform times out".
        assert_eq!(
            run_under_deadline(ScriptDeadline::default(), ScriptPhase::Transform, || Ok(
                serde_json::json!({"total": 1})
            ))
            .await
            .expect("work that returns at once is served"),
            serde_json::json!({"total": 1}),
        );

        // 2. Work that has not finished when the deadline is up is refused.
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&finished);

        // The OUTER timeout is the test's own, and it is not redundant: with the
        // production wrapper deleted this await never returns, and a mutation
        // that HANGS the suite is indistinguishable from a slow machine. It
        // turns that into a named failure. Panicking here still drops
        // `release`, so the parked thread is not leaked either.
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            run_under_deadline(
                ScriptDeadline::of(Duration::from_millis(20)),
                ScriptPhase::Transform,
                move || {
                    // Parks. `recv` returns `Err` once the sender is dropped,
                    // which is how the thread is released at the end of this
                    // test rather than leaked.
                    let _ = blocked.recv();
                    flag.store(true, Ordering::SeqCst);
                    Ok(serde_json::json!({"total": 1}))
                },
            ),
        )
        .await
        .expect("the request deadline must answer — a hang here is the production timeout gone")
        .expect_err("the work had not finished");

        assert_eq!(
            wire_of(&err),
            (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            "work that did not finish is the endpoint failing to answer: {err:?}"
        );
        let detail = format!("{err:?}");
        assert!(
            detail.contains("20 ms request deadline"),
            "the detail names the deadline that fired, in the units it was set \
             in — and it is the INJECTED one, not the shipped constant: {detail}"
        );
        assert!(
            !detail.contains("wall-clock"),
            "and it is the REQUEST's deadline, not `transform::MAX_WALL_CLOCK`: \
             {detail}"
        );

        // The claim the module comment makes, measured: the caller has its
        // answer and the work is still running.
        assert!(
            !finished.load(Ordering::SeqCst),
            "the deadline stops the WAITING; a `spawn_blocking` task cannot be \
             cancelled, and this is what the module comment means by the run \
             still holding a thread after the 500"
        );

        // Release it, and confirm it really was only parked — so "not finished"
        // above cannot have been a closure that never ran at all.
        drop(release);
        for _ in 0..1000 {
            if finished.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            finished.load(Ordering::SeqCst),
            "the detached work resumes and completes once it is unblocked"
        );
    }

    /// **Both refusal arms of [`blocking_outcome`], driven with REAL values.**
    ///
    /// A fabricated `JoinError` is not a thing that can be constructed, and that
    /// is the point: the one below comes from a task that actually panicked, and
    /// the `Elapsed` from a timeout that actually elapsed. The `Ok` arm is
    /// asserted on its PAYLOAD, so a mapping that swallowed the value and
    /// answered a default would redden here.
    #[tokio::test]
    async fn every_arm_of_the_join_outcome_is_decided() {
        // The value path.
        assert_eq!(
            blocking_outcome(
                Ok(Ok(Ok(serde_json::json!({"total": 1})))),
                ScriptDeadline::default(),
                ScriptPhase::Transform,
            )
            .expect("a completed run is served"),
            serde_json::json!({"total": 1}),
            "the script's value reaches the caller unaltered",
        );

        // A refusal the script itself produced travels through unchanged, so
        // this function cannot be what turns a 500 into something else.
        let refused = blocking_outcome(
            Ok(Ok(Err::<serde_json::Value, _>(ServingError::internal(
                "the script said no",
            )))),
            ScriptDeadline::default(),
            ScriptPhase::Transform,
        )
        .expect_err("a refusal stays a refusal");
        assert_eq!(
            wire_of(&refused),
            (StatusCode::INTERNAL_SERVER_ERROR, "internal")
        );
        assert!(
            format!("{refused:?}").contains("the script said no"),
            "and it is the SAME refusal, not a re-worded one: {refused:?}"
        );

        // A real `JoinError`, from a task that really panicked.
        let join = tokio::spawn(async { panic!("plumbing") })
            .await
            .expect_err("the task panicked");
        assert!(
            join.is_panic(),
            "the fixture must be a real panic: {join:?}"
        );
        let err = blocking_outcome::<serde_json::Value>(
            Ok(Err(join)),
            ScriptDeadline::default(),
            ScriptPhase::Transform,
        )
        .expect_err("a task that did not complete is a refusal");
        assert_eq!(
            wire_of(&err),
            (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            "a panic in the plumbing is not the caller's doing: {err:?}"
        );
        assert!(
            format!("{err:?}").contains("did not complete"),
            "and it is diagnosable as one: {err:?}"
        );

        // A real `Elapsed`, from a timeout that really elapsed.
        let elapsed = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .expect_err("zero is already up");
        let err = blocking_outcome::<serde_json::Value>(
            Err(elapsed),
            ScriptDeadline::of(Duration::from_millis(7)),
            ScriptPhase::Transform,
        )
        .expect_err("the deadline is a refusal");
        assert_eq!(
            wire_of(&err),
            (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            "not a 400 and not a 503: {err:?}"
        );
        let detail = format!("{err:?}");
        assert!(
            detail.contains("7 ms request deadline"),
            "the detail states the deadline that was actually applied, not the \
             shipped constant: {detail}"
        );
        assert!(
            detail.contains("still holding a blocking thread"),
            "and it says what answering did NOT stop — see the module comment: \
             {detail}"
        );
    }

    /// The shipped deadline is the only one a non-test build can express.
    ///
    /// `ScriptDeadline::of` is `#[cfg(test)]`, so this pins the DEFAULT
    /// against the constant; a relaxation would have to change the constant,
    /// which is the thing a reviewer reads.
    #[test]
    fn the_shipped_deadline_is_the_constant() {
        assert_eq!(
            ScriptDeadline::default(),
            ScriptDeadline::of(SCRIPT_REQUEST_DEADLINE)
        );
        assert_eq!(SCRIPT_REQUEST_DEADLINE, Duration::from_secs(5));
        assert!(
            SCRIPT_REQUEST_DEADLINE > crate::transform::MAX_WALL_CLOCK,
            "the request deadline must not be TIGHTER than the evaluator's own \
             budget, or a script that legitimately runs close to it and \
             finishes is cut off by a race between two clocks measuring one \
             piece of work",
        );
    }

    /// A statement that did not shape to an array is a fault, not a panic.
    ///
    /// `shape_rows` returns an array on every `Ok` path, so this arm is
    /// unreachable today — which is exactly why it is asserted rather than
    /// written as an `expect`: the cost of being wrong about "unreachable"
    /// inside a request handler is a panicking worker.
    #[test]
    fn a_statement_that_did_not_shape_to_rows_is_a_fault() {
        assert_eq!(
            rows_of("main", serde_json::json!([{"n": 1}])).expect("an array is rows"),
            vec![serde_json::json!({"n": 1})],
        );

        for (value, named) in [
            (serde_json::json!({"n": 1}), "an object"),
            (serde_json::json!(null), "null"),
            (serde_json::json!(7), "a scalar"),
        ] {
            let err = rows_of("main", value).expect_err("not an array of rows");
            assert_eq!(
                wire_of(&err),
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            );
            let detail = format!("{err:?}");
            assert!(
                detail.contains("`main`") && detail.contains(named),
                "the detail names the statement and what it was: {detail}"
            );
        }
    }

    /// **The published contract answers for every answer the surface gives.**
    ///
    /// The finding: the two `utoipa::path` blocks declared `500 internal` and no
    /// `502`, no `504` and no `endpoint_query_failed`, while all three were live
    /// on this branch. A machine consumer branching on the document — which is
    /// what a document is for — receives an undocumented status, which is the
    /// same "the operator cannot act on it" defect this package exists to fix,
    /// one layer out.
    ///
    /// It survived review because the `openapi.json` byte gate was green: it
    /// compares the committed file to the generated one and passes *precisely
    /// because nothing changed*. Nothing in the pipeline compared the document
    /// to the taxonomy. This does.
    ///
    /// Driven off `census::every_variant` (checked against the enum itself), so
    /// a variant added later fails here until both operations document it —
    /// and, because the assertion is on the generated document, the committed
    /// `openapi.json` has to be regenerated too.
    ///
    /// Asserted on the SERIALISED document rather than through utoipa's typed
    /// model, because the serialised form is what a consumer reads.
    #[test]
    fn the_published_contract_names_every_answer_the_surface_can_give() {
        use crate::serving::error::to_wire;

        let doc = serde_json::to_value(super::serving_openapi()).expect("serialise");
        let item = &doc["paths"]["/data-api/v1/{ws}/{path}"];
        assert!(item.is_object(), "the serving path must be in the document");

        for verb in ["get", "post"] {
            let responses = &item[verb]["responses"];
            assert!(
                responses["200"].is_object(),
                "{verb}: the success answer must be documented"
            );
            for (name, e) in crate::serving::census::every_variant() {
                let w = to_wire(&e);
                let status = w.status.as_str();
                let described = &responses[status]["description"];
                let described = described.as_str().unwrap_or_else(|| {
                    panic!(
                        "{verb}: `{name}` answers {status} `{}`, and the \
                         published contract has no {status} response at all. A \
                         consumer branching on this document cannot handle a \
                         status it was never told about.",
                        w.code
                    )
                });
                assert!(
                    described.contains(&format!("`{}`", w.code)),
                    "{verb}: the {status} response does not name `{}` (from \
                     `{name}`). `error.code` is what a client branches on, so a \
                     status documented without its codes is documented in name \
                     only.\n  description: {described}",
                    w.code
                );
            }
        }
    }
}
