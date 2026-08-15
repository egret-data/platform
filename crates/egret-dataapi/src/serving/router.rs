//! URL → published snapshot → rows. The serving surface's request chain
//! (spec §6.2 steps 1-6 and 8-10; step 7 is noted below and belongs to a later
//! task).
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
//! `QueryExecPort::execute_as` takes no `CancellationToken` parameter, so
//! `limits.timeout` is this handler's whole budget. **The decision is not to add
//! one**, because the port already owns the mechanism a caller-supplied token
//! would duplicate:
//!
//! * `port_impl::execute_as` wraps the connector call in
//!   `tokio::time::timeout(limits.timeout, …)`
//!   (`egret-query/src/execution/port_impl.rs:201-210`). **Expiry releases egret
//!   by DROPPING the connector future**, which drops the connection with it.
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
//! # What is deliberately NOT here yet
//!
//! Named so nothing reads as done that is not:
//!
//! * step 7 — rendering declared `params` into the SQL. Until it lands, a
//!   snapshot containing `{{placeholder}}` is sent to the engine verbatim and
//!   fails there as a syntax error. That is a broken endpoint, not an injection:
//!   P3's `check_params` already refuses to publish a placeholder that does not
//!   stand where a value stands, and no caller input reaches the SQL at all
//!   until step 7 exists.
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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_dataapi_api::{ColumnSpec, EndpointSnapshot};
use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

use super::access_log::{AccessLogEntry, AccessLogStore};
use super::cache::{cache_key, cache_ttl, shared_cache_allowed};
use super::error::{to_wire, ServingErrorBody};
use super::ratelimit::{check_request, retry_after_secs};
use super::{verify_serving_token, ServingError};
use crate::routes::DataApiState;

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
/// indistinguishable from a fresh one while debugging. It is hard-`false` until
/// Task 4 lands the cache — a field that lies would be worse than no field.
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
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine denied the data to this service account", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault; quote `request_id` to support", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` — the published configuration no longer describes the data. `Retry-After` is a floor, not a forecast: only a re-publish clears these", body = ServingErrorBody),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_get(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Response {
    serve(state, path, query, method, headers).await
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
        (status = 400, description = "`row_limit_exceeded` — the result exceeded the endpoint's row cap", body = ServingErrorBody),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 403, description = "`data_access_denied` — the engine denied the data to this service account", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such live endpoint in this workspace", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — a published rate limit was exceeded; `Retry-After` states when the bucket refills", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault; quote `request_id` to support", body = ServingErrorBody),
        (status = 503, description = "`schema_drift` / `output_naming_collision` / `cache_scope_stale` — the published configuration no longer describes the data. `Retry-After` is a floor, not a forecast: only a re-publish clears these", body = ServingErrorBody),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_post(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Response {
    serve(state, path, query, method, headers).await
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
async fn serve(
    State(st): State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Response {
    // One clock reading for the whole request, so the rate limit, the cache and
    // the recorded duration all agree about when "now" was. Monotonic, so an NTP
    // step cannot make a cache entry immortal, refill every bucket at once, or
    // record a negative duration.
    let started = Instant::now();
    let request_id = Uuid::new_v4().to_string();
    let mut entry = AccessLogStore::entry(&request_id, &headers);

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
    let shared = shared_cache_allowed(&st.db, &snap).await?;
    let params = declared_query_params(raw_query.as_deref(), &snap);
    // The principal is in the key unless a re-checked `shared` says otherwise —
    // Ranger filters per principal, so this is the difference between a cache
    // and a cross-identity leak.
    let key = cache_key(
        endpoint_id,
        version,
        &params,
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
                    columns: snap.columns.clone(),
                    row_count: hit.row_count,
                    version,
                    cached: true,
                    request_id: request_id.to_string(),
                },
            });
        }
    }

    let limits = execution_limits(&snap);
    let cap = limits.max_rows as u32;
    let out = st
        .query
        .execute_as(
            ws.id.0,
            snap.database_id,
            principal.user_id,
            &snap.sql,
            limits,
        )
        .await
        .map_err(engine_error)?;

    // `truncated`, never `row_count == cap`: the connector fetches `cap + 1` and
    // sets the flag, so a count equal to the cap cannot distinguish "exactly at"
    // from "over". Returning a truncated 200 is the worst available failure mode
    // for a data API — the client believes it has every row.
    if out.truncated {
        return Err(ServingError::RowLimitExceeded { cap });
    }

    // Shaped BEFORE it is cached, and the shaped form is what is stored: shaping
    // is where `output.shape`/`naming` and the drift refusal apply, so caching
    // the raw result would let a hit and a miss disagree about whether the
    // endpoint had drifted.
    //
    // Nothing is logged as SERVED before this line. `shape_rows` is the last
    // thing that can refuse the call, and the "served" line used to sit above it
    // — so a drift or naming-collision 503 produced a log saying the request had
    // been served with N rows. That line now lives in `serve`, on the Ok arm.
    let data = shape_rows(&out, &snap)?;
    if let Some(ttl) = ttl {
        st.cache
            .put_at(key, Arc::new(data.clone()), out.row_count, ttl, now);
    }

    Ok(ServingResponse {
        data,
        meta: ServingMeta {
            columns: snap.columns.clone(),
            row_count: out.row_count,
            version,
            cached: false,
            request_id: request_id.to_string(),
        },
    })
}

/// The DECLARED query parameters of this request, as the cache key's map.
///
/// # Only what the snapshot declares
///
/// Anything the author did not declare is dropped, which bounds the key space
/// for one endpoint and one principal to the parameters that exist. Hashing the
/// whole query string instead — the first version — let one authenticated client
/// mint unbounded distinct entries with `?junk=1`, `?junk=2`, …, each holding
/// memory in a process-wide cache shared by every workspace. The cache's own
/// eviction now bounds the damage, but a key space bounded at the source is the
/// better half of that fix: an undeclared parameter cannot affect the response,
/// so it must not be able to affect the entry either.
///
/// It is also what step 7 needs regardless — the rendered set is exactly the
/// declared set.
///
/// # Still not decoded, and still not validated
///
/// Step 7 owns both, and until it lands no caller input reaches the engine at
/// all. Leaving values raw errs toward a MISS: two encodings of one value are
/// two entries, which costs an execution and never serves the wrong rows. When
/// step 7 lands, the RENDERED parameters (defaults applied, body parameters
/// included) replace this — a default-valued parameter absent from the URL is
/// still part of what produced the rows.
///
/// Repeats are kept as a LIST, not joined: [`cache_key`] length-prefixes each
/// element, so `?a=1&a=2` cannot be spelled as a single value and vice versa.
/// A joiner byte would reintroduce exactly the forgeability the length prefixes
/// remove — see [`cache_key`]'s comment on why that matters the moment values
/// are decoded.
fn declared_query_params(
    raw: Option<&str>,
    snap: &EndpointSnapshot,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if snap.params.is_empty() {
        return out;
    }
    for pair in raw.unwrap_or("").split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if !snap.params.iter().any(|p| p.name == k) {
            continue;
        }
        out.entry(k.to_string()).or_default().push(v.to_string());
    }
    out
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
/// ITSELF can allow the call. That is deliberate for now: endpoint folders are
/// not creatable (`egret-bi`'s `kind_perms` has no entry for them, see
/// [`crate::endpoints::FOLDER_ENTITY_TYPE`]), so a cascade would be a code path
/// nothing can currently exercise and nothing tests. Reading the folder would
/// also cost a second query per request for that unreachable case. When P5 makes
/// folders reachable, whoever wires them must decide whether "invoke everything
/// in this folder" is a grant an operator should be able to write, and pass the
/// real `folder_id` here if so — it widens access, so it is a decision, not a
/// fix.
async fn invoke_allowed(
    st: &DataApiState,
    principal: &super::ServingPrincipal,
    ws: egret_core::WorkspaceId,
    endpoint_id: Uuid,
) -> Result<(), ServingError> {
    let allowed = st
        .pdp
        .authorize(
            &egret_core::authz::Subject {
                user_id: principal.user_id,
                group_ids: Vec::new(),
            },
            crate::routes::EP_INVOKE,
            &egret_core::authz::Resource::Object(egret_core::authz::ObjectRef {
                kind: crate::endpoints::OBJECT_KIND.to_string(),
                id: endpoint_id,
                folder_id: None,
                workspace_id: ws,
            }),
        )
        .await
        // A PDP that could not answer is OUR fault, and it must not read as a
        // refusal: `Internal` (500) sends an operator to egret, `NotGranted`
        // (403) would send them to write a grant that already exists.
        .map_err(ServingError::internal)?
        .allow;
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
/// Only the Ranger denial is given its own answer, because it is the one the
/// operator can act on and the one that must never be reported as bad input.
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
        egret_core::Error::Forbidden => ServingError::DataAccessDenied,
        other => ServingError::internal(other),
    }
}

/// Shape rows per the snapshot's frozen `output` spec, refusing anything that
/// would silently contradict the frozen contract.
///
/// # The contract is checked, not assumed
///
/// [`ServingMeta::columns`] reports the FROZEN columns, so every key and every
/// position in `data` must come from those same frozen columns or the response
/// contradicts its own advertised shape with no signal. Three ways that
/// happened before this function refused anything, each of which produced a
/// perfectly ordinary-looking 200:
///
/// * keys taken from the LIVE `out.columns` while `meta.columns` reported the
///   frozen ones — a drifted endpoint returning rows whose keys disagree with
///   its contract;
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
/// Column TYPES are deliberately not compared: a widened `integer` → `bigint`
/// does not change any key or position, and refusing it would break endpoints
/// over a change their clients cannot observe. Names and arity are what the
/// response shape actually depends on.
fn shape_rows(
    out: &egret_core::TabularResult,
    snap: &EndpointSnapshot,
) -> Result<serde_json::Value, ServingError> {
    // Order-sensitive, because `arrays` shape is positional: two identical name
    // SETS in a different order are still a different contract.
    let drifted = out.columns.len() != snap.columns.len()
        || out
            .columns
            .iter()
            .zip(snap.columns.iter())
            .any(|(live, frozen)| live.name != frozen.name);
    if drifted {
        tracing::warn!(
            frozen = ?snap.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
            live = ?out.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "published output contract no longer matches the engine's columns"
        );
        return Err(ServingError::SchemaDrift);
    }

    // From here the two sets are equal, so the FROZEN names are used throughout:
    // the contract is the thing being served, and reading them from the snapshot
    // makes that true by construction rather than by the check above holding.
    let frozen: Vec<&str> = snap.columns.iter().map(|c| c.name.as_str()).collect();

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
    // It is a connector bug, not a caller's or an author's problem, so it is
    // `Internal` rather than one of the contract variants — but it is refused
    // rather than zipped past, which is the whole point of this function.
    for row in &out.rows {
        if row.len() != frozen.len() {
            return Err(ServingError::internal(format!(
                "connector returned a row of {} values for {} columns",
                row.len(),
                frozen.len()
            )));
        }
    }

    if snap.output.shape == "arrays" {
        return Ok(serde_json::Value::Array(
            out.rows
                .iter()
                .map(|r| serde_json::Value::Array(r.clone()))
                .collect(),
        ));
    }
    if snap.output.shape != "objects" {
        tracing::warn!(
            shape = %snap.output.shape,
            "published snapshot carries an unrecognised output.shape; serving the default"
        );
    }

    let names: Vec<String> = frozen
        .iter()
        .map(|n| apply_naming(n, &snap.output.naming))
        .collect();
    // A `Map` silently keeps the LAST value for a duplicated key, which is how a
    // whole column disappears from every row without a single error.
    let mut seen = std::collections::HashSet::with_capacity(names.len());
    if let Some(dup) = names.iter().find(|n| !seen.insert(n.as_str())) {
        tracing::warn!(
            key = %dup,
            naming = %snap.output.naming,
            frozen = ?frozen,
            "output.naming maps two frozen columns onto one key"
        );
        return Err(ServingError::OutputNamingCollision);
    }

    // `names` is `frozen` mapped one-to-one, so every row's arity was already
    // checked against it above and the zip below cannot truncate.
    let mut shaped = Vec::with_capacity(out.rows.len());
    for row in &out.rows {
        shaped.push(serde_json::Value::Object(
            names
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .collect::<serde_json::Map<_, _>>(),
        ));
    }
    Ok(serde_json::Value::Array(shaped))
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
fn apply_naming(name: &str, naming: &str) -> String {
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

    fn snapshot_with_limits(max_rows: u32, timeout_seconds: u32) -> EndpointSnapshot {
        EndpointSnapshot {
            sql: "SELECT 1".into(),
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
            columns: vec![],
        }
    }
}
