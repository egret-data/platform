//! URL → published snapshot → rows. The serving surface's request chain
//! (spec §6.2 steps 1-3 and 8-10; steps 4-7 are noted below and belong to later
//! tasks).
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
//! `QueryExecPort::execute_as` takes no `CancellationToken`, so
//! `limits.timeout` is the whole budget. **The decision is not to add one**, and
//! the reasoning has to be read together with its known gap:
//!
//! * axum drops the handler future when the client disconnects, which drops the
//!   `execute_as` future, which drops the connector's future — so egret stops
//!   waiting and frees its own pool slot without any token being threaded.
//! * On Trino the ENGINE side is bounded too: `execute_as` sets
//!   `query_max_run_time` from `limits.timeout`, so the coordinator kills the
//!   query itself.
//! * **On StarRocks it is not.** There is no `query_max_run_time` equivalent,
//!   so a query that outlives its budget keeps running server-side until it
//!   finishes; the timeout bounds egret, not the engine.
//!
//! A `CancellationToken` would not close that gap either — cancelling a Rust
//! future does not kill a running StarRocks query. Closing it needs a
//! StarRocks-side `KILL QUERY <id>`, i.e. capturing the connection/query id and
//! issuing a second statement on a second connection, which is a connector-level
//! change and not a port-signature one. Adding the parameter now would cost 2
//! impls plus 4 test fakes, and become a cross-crate contract revision once the
//! Data API moves to its own binary, in exchange for a guarantee it does not
//! actually provide. **Revisit it only alongside a StarRocks `KILL`
//! implementation** — that is the change that would make a token meaningful.
//!
//! # What is deliberately NOT here yet
//!
//! Named so nothing reads as done that is not:
//!
//! * spec §6.2 step 4 — the Cedar `dataapi:invoke` check. Not assigned to any
//!   P4 task; it is the one chain step with no owner.
//! * step 5 — rate limiting (Task 4).
//! * step 6 — the cache, and with it the `delegated` re-check against the live
//!   `query.databases` column (Task 4).
//! * step 7 — rendering declared `params` into the SQL. Until it lands, a
//!   snapshot containing `{{placeholder}}` is sent to the engine verbatim and
//!   fails there as a syntax error. That is a broken endpoint, not an injection:
//!   P3's `check_params` already refuses to publish a placeholder that does not
//!   stand where a value stands, and no caller input reaches the SQL at all
//!   until step 7 exists.
//! * the error envelope and the access log (Task 5). [`ServingError`]'s
//!   `IntoResponse` below is a placeholder with a fixed body and no
//!   `request_id`; `serving/error.rs` replaces it.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_dataapi_api::{ColumnSpec, EndpointSnapshot};
use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

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
    tags((
        name = "data-api-serving",
        description = "Published Data API endpoints, called by service accounts with a bearer token"
    )),
)]
struct DataApiServingApi;

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
        (status = 401, description = "Bearer token missing, expired, or not for this audience"),
        (status = 403, description = "The engine denied the data to this service account"),
        (status = 404, description = "No such live endpoint in this workspace"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer" = [])),
)]
async fn serve_get(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
    serve(state, path, method, headers).await
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
        (status = 401, description = "Bearer token missing, expired, or not for this audience"),
        (status = 403, description = "The engine denied the data to this service account"),
        (status = 404, description = "No such live endpoint in this workspace"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer" = [])),
)]
async fn serve_post(
    state: State<DataApiState>,
    path: Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
    serve(state, path, method, headers).await
}

async fn serve(
    State(st): State<DataApiState>,
    Path((ws_slug, raw_path)): Path<(String, String)>,
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

    // …spec §6.2 steps 4-7 (Cedar, rate limit, cache, param rendering) land in
    // Tasks 4 and 5; see the module comment.

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

    Ok(Json(ServingResponse {
        data: shape_rows(&out, &snap.output),
        meta: ServingMeta {
            columns: snap.columns.clone(),
            row_count: out.row_count,
            version,
            // Task 4 owns the cache; until it exists this is honestly false.
            cached: false,
            request_id,
        },
    }))
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

/// Shape rows per the snapshot's frozen `output` spec.
///
/// `shape` and `naming` are unvalidated author input, exactly like `limits` (spec
/// §5.3 freezes them with no rule anywhere). An unrecognised value has no
/// meaning to honour, so it falls back to the documented default and logs —
/// rather than refusing, which would make a published endpoint permanently dead
/// over a cosmetic field.
fn shape_rows(
    out: &egret_core::TabularResult,
    output: &egret_dataapi_api::OutputSpec,
) -> serde_json::Value {
    if output.shape == "arrays" {
        return serde_json::Value::Array(
            out.rows
                .iter()
                .map(|r| serde_json::Value::Array(r.clone()))
                .collect(),
        );
    }
    if output.shape != "objects" {
        tracing::warn!(
            shape = %output.shape,
            "published snapshot carries an unrecognised output.shape; serving the default"
        );
    }
    let names: Vec<String> = out
        .columns
        .iter()
        .map(|c| apply_naming(&c.name, &output.naming))
        .collect();
    serde_json::Value::Array(
        out.rows
            .iter()
            .map(|row| {
                serde_json::Value::Object(
                    names
                        .iter()
                        .cloned()
                        .zip(row.iter().cloned())
                        .collect::<serde_json::Map<_, _>>(),
                )
            })
            .collect(),
    )
}

/// `as_is` (default) | `camel` | `snake`. Unrecognised falls back to `as_is`.
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
                } else {
                    out.extend(c.to_lowercase());
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
