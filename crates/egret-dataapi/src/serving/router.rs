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
//! parameter, so `limits.timeout` is this handler's whole budget. **The decision
//! is not to add one**, because the port already owns the mechanism a
//! caller-supplied token would duplicate:
//!
//! * `port_impl::run_bounded` — shared by `execute_as` and
//!   `execute_as_with_params`, which is why the wrapper cannot be present on one
//!   path and missing on the other — wraps the connector call in
//!   `tokio::time::timeout(limits.timeout, …)`. **Expiry releases egret by
//!   DROPPING the connector future**, which drops the connection with it. That
//!   wrapper is the whole wall clock on this path: neither
//!   `Connector::execute_with_params` implementation reads `limits.timeout`
//!   itself, and StarRocks sets no socket timeout at all.
//! * On client disconnect axum drops this handler's future, which drops the
//!   `execute_as` future, which drops the connector's — the same release path,
//!   with no token threaded anywhere.
//! * Trino additionally bounds the coordinator with `query_max_run_time`,
//!   derived from the same `limits.timeout`.
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
//!   output, never a statement's own `sql`: every `{{ … }}` has become a `?` and
//!   the values travel beside it as binds.
//!
//! **A `StatementSnapshot`'s `sql` must not be executed from this module
//! again.** It is the template, not the statement; sending it would send
//! `{{ region }}` to the engine verbatim, which is the state P4 shipped in and
//! P4.5 exists to end.
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

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_dataapi_api::{
    compare_column_types, ColumnSpec, EndpointSnapshot, OutputSpec, StatementSnapshot,
};
use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

use super::access_log::{AccessLogEntry, AccessLogStore};
use super::cache::{cache_key, cache_ttl, shared_cache_allowed};
use super::error::{is_permission_denied, to_wire, ServingErrorBody};
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
const MAX_ROWS_CEILING: u32 = 1_000_000;

/// Upper bound applied to a snapshot's `limits.timeout_seconds`. Same reasoning
/// as [`MAX_ROWS_CEILING`]; a 0 would make every call time out instantly and an
/// unbounded one would pin a connection for as long as the author felt like.
const TIMEOUT_CEILING_SECS: u64 = 300;

/// The success body (spec §5.1.2).
///
/// `meta.version` is the published version that served the call, so a
/// client-side bug is reproducible against the exact snapshot that produced it.
/// `meta.cached` is likewise not cosmetic: without it a stale-looking result is
/// indistinguishable from a fresh one while debugging. It is a real value — Task
/// 4 landed the cache and [`run`] sets it `true` on a hit — and this comment
/// said "hard-`false` until Task 4 lands" for three tasks after Task 4 landed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingResponse {
    /// Rows, shaped by the snapshot's `output.shape`: an array of objects
    /// (default) or an array of arrays.
    #[schema(value_type = Vec<Object>)]
    pub data: serde_json::Value,
    pub meta: ServingMeta,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServingMeta {
    /// The FROZEN columns from the snapshot, not the ones this execution
    /// happened to return. The published contract is what a client codes
    /// against; if the underlying table has drifted, the difference is a defect
    /// to surface, not something to paper over by reporting today's shape.
    pub columns: Vec<ColumnSpec>,
    pub row_count: u64,
    pub version: i32,
    pub cached: bool,
    pub request_id: String,
}

/// The serving surface. Mounted at `/data-api/v1` by `egret_server::build_router`
/// as its own group with **no CSRF layer** — see the comment at that call site.
///
/// `GET` and `POST` only, matching `endpoints.method`'s CHECK constraint. A
/// method outside the pair never reaches [`serve`]; axum answers 405, which is
/// correct and leaks nothing (it is the same answer for every path, existing or
/// not, under this router).
pub fn serving_router() -> Router<DataApiState> {
    Router::new().route("/{ws}/{*path}", get(serve_get).post(serve_post))
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
    paths(serve_get, serve_post),
    components(schemas(
        ServingResponse,
        ServingMeta,
        ServingErrorBody,
        super::error::ServingErrorDetail,
        egret_dataapi_api::ColumnSpec
    )),
    modifiers(&BearerAddon),
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

#[utoipa::path(
    get,
    path = "/data-api/v1/{ws}/{path}",
    tag = "data-api-serving",
    params(
        ("ws" = String, Path, description = "Workspace slug; must match the token's `ws` claim"),
        ("path" = String, Path, description = "The endpoint's published path, e.g. `reports/daily`"),
    ),
    responses(
        (status = 200, description = "Rows from the published snapshot", body = ServingResponse),
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap; `param_missing` — a required parameter was not supplied; `param_invalid` — a supplied parameter failed its declared type, its `allowed` list, or was repeated where one value is required. Neither parameter error names the parameter: the name is the author's, and naming it turns the error surface into a directory of what an endpoint declares", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine refused this service account the underlying data, either because it lacks a data permission (a Ranger grant, say) or because its engine credential is bad or expired; one code, because the two recognitions available (Trino's typed 401/403 from the coordinator, and every other engine failure matched on its own message) cannot tell those causes apart. `not_granted` — egret refused the call because the service account holds no invoke grant on this endpoint", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault; quote `request_id` to support", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` / `param_gate_stale` — the published configuration no longer describes the data, or was frozen before the current parameter-safety rules existed. `Retry-After` is a floor, not a forecast: only a re-publish clears these", body = ServingErrorBody),
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
    ),
    responses(
        (status = 200, description = "Rows from the published snapshot", body = ServingResponse),
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap; `param_missing` — a required parameter was not supplied; `param_invalid` — a supplied parameter failed its declared type, its `allowed` list, or was repeated where one value is required. Neither parameter error names the parameter: the name is the author's, and naming it turns the error surface into a directory of what an endpoint declares", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine refused this service account the underlying data, either because it lacks a data permission (a Ranger grant, say) or because its engine credential is bad or expired; one code, because the two recognitions available (Trino's typed 401/403 from the coordinator, and every other engine failure matched on its own message) cannot tell those causes apart. `not_granted` — egret refused the call because the service account holds no invoke grant on this endpoint", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault; quote `request_id` to support", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` / `param_gate_stale` — the published configuration no longer describes the data, or was frozen before the current parameter-safety rules existed. `Retry-After` is a floor, not a forecast: only a re-publish clears these", body = ServingErrorBody),
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

    // Measured before the log write, and saturating rather than wrapping: a
    // request that somehow ran for 25 days should record `i32::MAX`, not a
    // negative duration that poisons every average computed over the column.
    entry.duration_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;

    let response = match outcome {
        Ok(body) => {
            entry.status_code = StatusCode::OK.as_u16() as i32;
            entry.row_count = Some(body.meta.row_count as i64);
            entry.cache_hit = body.meta.cached;
            // Fires only once the response actually EXISTS. It used to fire
            // before `shape_rows`, so a drift or naming-collision 503 was logged
            // as served — the one line an operator would read to decide the call
            // was fine said so about a call that failed.
            tracing::debug!(
                %request_id,
                endpoint_id = ?entry.endpoint_id,
                version = ?entry.version,
                client_id = entry.client_id.as_deref().unwrap_or(""),
                row_count = body.meta.row_count,
                cached = body.meta.cached,
                duration_ms = entry.duration_ms,
                "data-api request served"
            );
            Json(body).into_response()
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
            wire.into_response(&request_id)
        }
    };

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
    AccessLogStore::new(st.db.clone()).record(&entry).await;
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
) -> Result<ServingResponse, ServingError> {
    let principal = verify_serving_token(&*st.keys, bearer(headers)?).await?;
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

    let (endpoint_id, version, snap) = live_snapshot(&st.db, ws.id.0, method.as_str(), &path)
        .await?
        .ok_or(ServingError::NotFound)?;
    // From here every failure is attributable to a specific published version,
    // which is what makes "this endpoint started 503ing at 14:02" answerable
    // from the log alone.
    entry.endpoint_id = Some(endpoint_id);
    entry.version = Some(version);

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
    check_request(
        &*st.rate_limits,
        endpoint_id,
        &principal.client_id,
        &snap.rate_limit,
        now,
    )
    .map_err(|retry_after| {
        tracing::debug!(
            endpoint_id = %endpoint_id,
            client_id = %principal.client_id,
            retry_after_secs = retry_after_secs(retry_after),
            "data-api request rate limited"
        );
        ServingError::RateLimited { retry_after }
    })?;

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
    let shared = shared_cache_allowed(&st.db, &snap).await?;
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
    // all: `enabled: false` and a `ttl_seconds` of 0 both answer `None`.
    let ttl = cache_ttl(&snap);
    if ttl.is_some() {
        if let Some(hit) = st.cache.get_at(&key, now) {
            // The id comes from the caller, not from a fresh `Uuid::new_v4()`.
            // A hit that minted its own would be uncorrelatable with the
            // access-log row describing the very same call.
            return Ok(ServingResponse {
                // One clone per hit. The alternative is `Arc` in the response
                // type, which needs serde's `rc` feature and leaks a caching
                // detail into the public wire type.
                data: (*hit.data).clone(),
                meta: ServingMeta {
                    columns: contract_columns(&snap),
                    row_count: hit.row_count,
                    version,
                    cached: true,
                    request_id: request_id.to_string(),
                },
            });
        }
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
    let mut row_count: u64 = 0;
    for stmt in &snap.statements {
        let (sql, binds) = crate::render::render(&stmt.sql, &resolved.values)
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
        shaped.push((stmt.name.as_str(), shape_rows(&out, &snap.output, stmt)?));
        row_count = row_count.saturating_add(out.row_count);
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
    let data = statements_data(shaped);
    if let Some(ttl) = ttl {
        st.cache
            .put_at(key, Arc::new(data.clone()), row_count, ttl, now);
    }

    Ok(ServingResponse {
        data,
        meta: ServingMeta {
            columns: contract_columns(&snap),
            row_count,
            version,
            cached: false,
            request_id: request_id.to_string(),
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

/// What `ServingMeta.columns` reports: the frozen contract, never this
/// execution's shape.
///
/// One statement — every endpoint published before M2, and most after it — is
/// that statement's frozen union, byte for byte what this field has always
/// carried.
///
/// **Several statements do not fit in this field, and Task 8 replaces it** with
/// `meta.statements: {<name>: {row_count, columns}}` (spec §4.4). Until then the
/// concatenation, in statement order, is the least wrong filling available:
///
/// * reporting the FIRST statement's list would describe one statement as if it
///   were the endpoint — silently, and to a caller who has no way to notice.
///   That is the exact defect shape this task exists to close;
/// * an empty list would claim the endpoint declares no columns at all, which no
///   published endpoint can (publish refuses a statement that describes none).
///
/// So: complete but unkeyed, until the field that can key it exists.
fn contract_columns(snap: &EndpointSnapshot) -> Vec<ColumnSpec> {
    snap.statements
        .iter()
        .flat_map(|s| s.columns.iter().cloned())
        .collect()
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
async fn live_snapshot(
    db: &Db,
    ws: Uuid,
    method: &str,
    path: &str,
) -> Result<Option<(Uuid, i32, EndpointSnapshot)>, ServingError> {
    let row: Option<(Uuid, i32, serde_json::Value)> = sqlx::query_as(
        "SELECT e.id, v.version, v.snapshot \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.method = $2 AND e.path = $3 \
           AND e.state = 'live'",
    )
    .bind(ws)
    .bind(method)
    .bind(path)
    .fetch_optional(&db.pool)
    .await
    .map_err(ServingError::internal)?;

    let Some((id, version, raw)) = row else {
        return Ok(None);
    };
    // A snapshot that will not deserialise is OUR fault, not the caller's: the
    // only writer is publish. 500, not 404 — a 404 would quietly hide a
    // corrupted or forward-incompatible version behind "no such endpoint".
    let snapshot: EndpointSnapshot = serde_json::from_value(raw).map_err(ServingError::internal)?;
    Ok(Some((id, version, snapshot)))
}

/// The `Authorization: Bearer …` value, or [`ServingError::Unauthorized`].
///
/// A missing header and a malformed one are the same answer for the same reason
/// every other authentication failure is: see [`super::auth`].
fn bearer(headers: &HeaderMap) -> Result<&str, ServingError> {
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
/// # Everything else is `Internal`, and that is now a decision rather than a gap
///
/// A published snapshot whose SQL no longer parses, a relation that has been
/// dropped, an unreachable coordinator and a timeout are four different operator
/// stories, and spec §7 names two of them (`engine_unavailable`, `timeout`).
/// **Neither is decidable here**, and the reason is upstream of this function:
///
/// * `port_impl::connect_as` produces `egret_core::Error::Internal` for EGRET's
///   own faults — resolving the principal's email, opening the database secret
///   (`egret-query/src/execution/port_impl.rs:120-137`) — using the same variant
///   `engine_error` produces for the engine's `Connect`/`Storage`/`Db`. So
///   `Internal` does not mean "the engine is unreachable", and answering 502
///   would page an operator about Trino when the metastore is what failed.
/// * `EngineError::Timeout` becomes `Error::InvalidInput("query timed out")`,
///   deliberately and for `describe_as`'s sake
///   (`port_impl.rs:53`). Recognising it here would mean matching another
///   crate's message string — a check that keeps compiling and silently stops
///   working the day someone rewords it.
///
/// # What the widening must bring, when someone does it
///
/// Recorded here because this is the function they will open, and because two
/// of the three are easy to miss while thinking only about 502 and 504:
///
/// 1. **`execute_as` needs its own timeout error, not `describe_as`'s.** They
///    share one `engine_error` table today, and that table maps
///    `EngineError::Timeout` to `InvalidInput("query timed out")` for a reason
///    that is true only of `describe_as` — a `LIMIT 0` plan that times out
///    really is a query problem. For `execute_as` it means the caller's
///    published budget was exceeded. Widening the error type without splitting
///    that arm leaves 504 undecidable forever.
/// 2. **A distinct AUTHOR-FAULT code, not only 502/504.**
///    `EngineError::BadRequest`/`Engine`/`Limit` all become
///    `Error::InvalidInput` and land here on `500 internal`, so a published
///    endpoint whose SQL stopped parsing is indistinguishable in
///    `access_log.error_code` from an egret fault. Spec §7's `internal` row does
///    cover it — it is disclosed, not hidden — but an operator scanning the log
///    for "which endpoints are broken" cannot separate the two, and those are
///    different people's problems.
/// 3. It must not become a 400. The caller did nothing wrong; the endpoint did.
///
/// Until then the answer is 500 with the detail in the log, which is safe —
/// nothing from the engine reaches the body — and honest, which a guessed 502
/// would not be. See `ServingError`'s comment.
fn engine_error(e: egret_core::Error) -> ServingError {
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
        other => ServingError::internal(other),
    }
}

/// Shape rows per the snapshot's frozen `output` spec, refusing anything that
/// would silently contradict the frozen contract.
///
/// # The contract is checked, not assumed
///
/// [`ServingMeta::columns`] reports the FROZEN contract — since M1 the UNION of
/// every branch described at publish — so every key in an `objects` response and
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
                sql: "SELECT 1".into(),
                columns: vec![],
            }],
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
            sql: sql.into(),
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

    /// `meta.columns` is one statement's frozen union, or every statement's.
    ///
    /// **Task 8 replaces this field** with `meta.statements`, so this test is
    /// meant to redden then — it pins an interim, and the interim's whole point
    /// is that it never reports one statement's columns as if they were the
    /// endpoint's.
    #[test]
    fn the_reported_contract_covers_every_statement() {
        let base = snapshot_with_limits(1, 1);
        let one = EndpointSnapshot {
            statements: vec![stmt("main", "SELECT 1", vec![spec("id", "integer", true)])],
            ..base.clone()
        };
        assert_eq!(
            contract_columns(&one),
            vec![spec("id", "integer", true)],
            "one statement reports exactly what it froze",
        );

        let two = EndpointSnapshot {
            statements: vec![
                stmt("rows", "SELECT 1", vec![spec("id", "integer", true)]),
                stmt("total", "SELECT 2", vec![spec("total", "bigint", true)]),
            ],
            ..base
        };
        assert_eq!(
            contract_columns(&two),
            vec![spec("id", "integer", true), spec("total", "bigint", true)],
            "a second statement's columns are not dropped from the contract the \
             caller is handed",
        );
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
    /// fetched its own columns through `sole_statement()` it would have named
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
}
