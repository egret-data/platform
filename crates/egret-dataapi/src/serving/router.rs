//! URL → published snapshot → rows. The serving surface's request chain
//! (spec §6.2 steps 1-3, 5-6 and 8-10; steps 4 and 7 are noted below and belong
//! to later tasks).
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
//! # Why nothing here answers 403
//!
//! Unknown endpoint, disabled endpoint, an endpoint in another workspace and an
//! endpoint in an ARCHIVED workspace are all [`ServingError::NotFound`]. A
//! caller holding a token for workspace A must not be able to tell "no such
//! endpoint" from "exists, but not yours" — that difference is a free directory
//! of another tenant's URLs. The management API applies the same rule (spec
//! §6.1), and so does [`super::auth`] one layer up for authentication reasons.
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
//! * spec §6.2 step 4 — the Cedar `dataapi:invoke` check, and it is **a hole
//!   wherever Ranger is not enforcing**, not merely an unfinished step. Without
//!   it, any valid token for a workspace reaches EVERY live endpoint in that
//!   workspace: service-account granularity collapses to workspace granularity
//!   and an endpoint stops being an access-control boundary at all. The only
//!   thing standing behind it is the engine's own row filtering, and on
//!   StarRocks that enforcement is resource-only. Being tracked as its own task.
//! * step 7 — rendering declared `params` into the SQL. Until it lands, a
//!   snapshot containing `{{placeholder}}` is sent to the engine verbatim and
//!   fails there as a syntax error. That is a broken endpoint, not an injection:
//!   P3's `check_params` already refuses to publish a placeholder that does not
//!   stand where a value stands, and no caller input reaches the SQL at all
//!   until step 7 exists.
//! * the error envelope and the access log (Task 5). [`ServingError`]'s
//!   `IntoResponse` below is a placeholder with a fixed body and no
//!   `request_id`; `serving/error.rs` replaces it.
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

use super::cache::{cache_key, cache_ttl, shared_cache_allowed};
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
    components(schemas(ServingResponse, ServingMeta, egret_dataapi_api::ColumnSpec)),
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
        (status = 400, description = "The result exceeded the endpoint's row cap"),
        (status = 401, description = "Bearer token missing, expired, or not for this audience"),
        (status = 403, description = "The engine denied the data to this service account"),
        (status = 404, description = "No such live endpoint in this workspace"),
        (status = 429, description = "A published rate limit was exceeded; `Retry-After` states the wait"),
        (status = 500, description = "Internal error"),
        (status = 503, description = "The published output contract, or a shared cache's connection setting, no longer matches"),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_get(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
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
        (status = 400, description = "The result exceeded the endpoint's row cap"),
        (status = 401, description = "Bearer token missing, expired, or not for this audience"),
        (status = 403, description = "The engine denied the data to this service account"),
        (status = 404, description = "No such live endpoint in this workspace"),
        (status = 429, description = "A published rate limit was exceeded; `Retry-After` states the wait"),
        (status = 500, description = "Internal error"),
        (status = 503, description = "The published output contract, or a shared cache's connection setting, no longer matches"),
    ),
    security(("data_api_bearer" = [])),
)]
async fn serve_post(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    query: RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
    serve(state, path, query, method, headers).await
}

async fn serve(
    State(st): State<DataApiState>,
    Path((ws_slug, raw_path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    method: Method,
    headers: HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
    let principal = verify_serving_token(&*st.keys, bearer(&headers)?).await?;

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

    // …spec §6.2 step 4 (the Cedar `dataapi:invoke` check) is its own task; see
    // the module comment for why its absence is a hole and not merely an
    // unfinished step. Step 7 (param rendering) likewise.

    // One clock reading for the whole request, so the rate limit and the cache
    // agree about when "now" was. Monotonic, so an NTP step cannot make an entry
    // immortal or refill every bucket at once.
    let now = Instant::now();

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
    let params = query_params(raw_query.as_deref());
    // The principal is in the key unless a re-checked `shared` says otherwise —
    // Ranger filters per principal, so this is the difference between a cache
    // and a cross-identity leak.
    let key = cache_key(
        endpoint_id,
        version,
        &params,
        (!shared).then_some(principal.user_id),
    );

    if snap.cache.enabled {
        if let Some(hit) = st.cache.get_at(&key, now) {
            let request_id = Uuid::new_v4().to_string();
            tracing::debug!(
                %request_id,
                endpoint_id = %endpoint_id,
                version,
                client_id = %principal.client_id,
                "data-api request served from cache"
            );
            return Ok(Json(ServingResponse {
                // One clone per hit. The alternative is `Arc` in the response
                // type, which needs serde's `rc` feature and leaks a caching
                // detail into the public wire type.
                data: (*hit.data).clone(),
                meta: ServingMeta {
                    columns: snap.columns.clone(),
                    row_count: hit.row_count,
                    version,
                    cached: true,
                    request_id,
                },
            }));
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

    let request_id = Uuid::new_v4().to_string();
    tracing::debug!(
        %request_id,
        endpoint_id = %endpoint_id,
        version,
        client_id = %principal.client_id,
        row_count = out.row_count,
        "data-api request served"
    );

    // Shaped BEFORE it is cached, and the shaped form is what is stored: shaping
    // is where `output.shape`/`naming` and the drift refusal apply, so caching
    // the raw result would let a hit and a miss disagree about whether the
    // endpoint had drifted.
    let data = shape_rows(&out, &snap)?;
    if snap.cache.enabled {
        st.cache.put_at(
            key,
            Arc::new(data.clone()),
            out.row_count,
            cache_ttl(&snap),
            now,
        );
    }

    Ok(Json(ServingResponse {
        data,
        meta: ServingMeta {
            columns: snap.columns.clone(),
            row_count: out.row_count,
            version,
            cached: false,
            request_id,
        },
    }))
}

/// The request's query string, as the cache key's parameter map.
///
/// **Not decoded, and not validated.** Step 7 (rendering declared `params` into
/// the SQL) is a later task and owns both; until it lands no caller input
/// reaches the engine at all, so this map's only job is to keep two calls that
/// differ in their query string out of each other's cache entry. Leaving it
/// undecoded errs toward a MISS: two encodings of the same value are two
/// entries, which costs an execution and never serves the wrong rows. When step
/// 7 lands, the rendered parameters (defaults applied, body parameters
/// included) replace this — a default-valued parameter is part of what produced
/// the rows and must be part of the key.
///
/// A repeated key keeps every occurrence rather than the last, so `?a=1&a=2`
/// and `?a=2` cannot share an entry.
fn query_params(raw: Option<&str>) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for pair in raw.unwrap_or("").split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.entry(k.to_string())
            .and_modify(|held| {
                held.push('\u{1}');
                held.push_str(v);
            })
            .or_insert_with(|| v.to_string());
    }
    out
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
/// Only the Ranger denial is given its own answer here, because it is the one
/// the operator can act on and the one that must never be reported as bad input.
/// **Everything else collapses to `Internal`, which Task 5 must split**: a
/// published snapshot whose SQL no longer parses, a relation that has been
/// dropped, an unreachable coordinator and a timeout are four different
/// operator stories and spec §7 gives three of them their own codes
/// (`engine_unavailable`, `timeout`, and the bad-SQL case which the table does
/// not yet name). Collapsing them is safe — nothing from the engine reaches the
/// body — but it is not finished.
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

// ---------------------------------------------------------------------------
// PLACEHOLDER wire mapping. Task 5 replaces this whole block with
// `serving/error.rs`: spec §7's fixed code table, the
// `{ "error": { code, message, request_id } }` envelope, and the `request_id`
// correlation into `dataapi.access_log`.
//
// What it already gets right, and what Task 5 must keep: the body is a FIXED
// string per variant and never renders the source. Engine errors carry table
// names, column names and sometimes hosts; on a key-authenticated public surface
// that is a metadata leak.
// ---------------------------------------------------------------------------
impl IntoResponse for ServingError {
    fn into_response(self) -> Response {
        // Handled ahead of the table because it is the one answer carrying a
        // HEADER. A 429 without `Retry-After` tells a client it was refused and
        // not when to come back, which every retry loop resolves by guessing —
        // usually immediately, and usually again.
        if let ServingError::RateLimited { retry_after } = &self {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("Retry-After", retry_after_secs(*retry_after).to_string())],
                "rate limit exceeded",
            )
                .into_response();
        }
        let (status, message) = match &self {
            ServingError::Unauthorized => (StatusCode::UNAUTHORIZED, "authentication failed"),
            ServingError::NotFound => (StatusCode::NOT_FOUND, "no such endpoint"),
            ServingError::DataAccessDenied => (
                StatusCode::FORBIDDEN,
                "the service account is not granted access to this data",
            ),
            ServingError::RowLimitExceeded { .. } => {
                (StatusCode::BAD_REQUEST, "result exceeded the row cap")
            }
            // 503 and not 500: nothing is broken on egret's side, the endpoint's
            // published contract has stopped matching its data (or was never
            // satisfiable) and an AUTHOR has to act. Same shape spec §7 gives
            // `cache_scope_stale`, whose situation is the same one.
            //
            // TASK 5 MUST DECIDE `Retry-After` FOR BOTH, deliberately. A bare 503
            // is a "come back later" on a condition that never self-heals —
            // only a human re-publishing clears it — so well-behaved clients and
            // the proxies in front of them will retry-storm a permanent state.
            // Neither spec §7's table nor Task 5's has a 503 row in either
            // language yet; these two variants are the first, so the table has
            // to grow rather than be matched against.
            ServingError::SchemaDrift => (
                StatusCode::SERVICE_UNAVAILABLE,
                "this endpoint's published output schema no longer matches its data; re-publish it",
            ),
            ServingError::OutputNamingCollision => (
                StatusCode::SERVICE_UNAVAILABLE,
                "this endpoint's output naming maps two columns onto one key; change it and re-publish",
            ),
            // Deliberately NOT a downgrade to a per-principal answer: the
            // published configuration is no longer safe and the operator has to
            // see that. Same 503 shape as the two above, and the same reason —
            // an author or operator acts, nothing on egret's side is broken.
            ServingError::CacheScopeStale => (
                StatusCode::SERVICE_UNAVAILABLE,
                "this endpoint's shared cache was published against a different connection setting; re-publish it",
            ),
            // Answered above, with its header.
            ServingError::RateLimited { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded")
            }
            ServingError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        // The only place the internal detail appears.
        if let ServingError::Internal(e) = &self {
            tracing::warn!(error = %e, "data-api serving request failed");
        }
        (status, message).into_response()
    }
}

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
