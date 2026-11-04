//! The OPA decision surface (governance SP2b, Task 9): the HTTP routes Trino
//! 483's OPA access-control plugin calls on egret.
//!
//! | Route | Plugin setting | Refusal |
//! |---|---|---|
//! | `/allow` | `opa.policy.uri` | 200 `{"result": false}` |
//! | `/batch` | `opa.policy.batched-uri` | 200 `{"result": []}` |
//! | `/row_filters` | `opa.policy.row-filters-uri` | 403 `{"error": reason}` |
//! | `/batch_column_masks` | `opa.policy.batch-column-masking-uri` | 403 `{"error": reason}` |
//! | `/column_mask` | `opa.policy.column-masking-uri` | 403 `{"error": reason}` |
//!
//! **Fail closed.** On the three value routes `[]` / `null` means "no filter,
//! no mask", so only an evaluation against a `Ready` snapshot answers 200
//! there; every refusal is a 403 (C1). An internal error (the verifier's store
//! unreachable, `EvalError::TwoMasks`, a result index out of range) is a 500 on
//! every route. The plugin turns any non-2xx into a failed query.
//!
//! **Each request** is parsed once ([`wire::parse_request`]); the credential
//! check and the decision read the same parse. Then:
//!
//! 1. the engine's secret (`egret.engine`/`egret.secret` in the plugin's
//!    `opa.context-file`) is verified through the injected
//!    [`EngineCredentialVerifier`], the constant-time path the Ranger surface
//!    uses, behind a short [`VerifiedCache`];
//! 2. the profile is read from [`SnapshotCache::binding`], which the refresher
//!    replaces every period, never from the verifier's
//!    [`egret_engine_mgmt_api::credential::EngineIdentity`] and never from the
//!    credential cache (I3), so a rebound or unbound engine stops being
//!    answered from its old profile within one refresh;
//! 3. only [`Lookup::Ready`] evaluates; `NotOpa` and `NotBuilt` refuse.
//!
//! **The secret** is read only through [`wire::Secret::expose`], hashed for
//! the cache key, and handed to the verifier. It is never logged, never in a
//! response, and the cache holds only its SHA-256.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Json;
use egret_engine_mgmt_api::credential::EngineCredentialVerifier;
use egret_perm_eval::{Decision, EvalError, Resource, Snapshot};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use uuid::Uuid;

use super::cache::{Lookup, SnapshotCache};
use super::wire::{self, OpaRequest, FILTER_COLUMNS};

pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";

/// The largest request body accepted: 4 MiB. A larger body is refused with
/// 413 (a non-2xx, so the plugin fails the query) before any parse, and the
/// refusal is logged and counted like every other one. The body is parsed
/// into a `serde_json::Value` BEFORE the engine's credential is checked, so
/// this bound is what an unauthenticated caller can make egret parse per
/// request (Task 9 review, M6; it was 16 MiB).
///
/// Why 4 MiB: Trino 483's plugin sends one request per access check and one
/// per filter batch. The largest is `FilterTables`, one
/// `{"table":{"catalogName","schemaName","tableName"}}` object per table: 59
/// bytes of JSON plus the three names, so 120 to 150 bytes with 20- to
/// 30-character names. 4 MiB holds 28 000 to 35 000 tables in one schema,
/// three times the plan's 10 000-table figure. `FilterColumns` carries one
/// table and about 25 bytes per column. A larger listing fails the query
/// rather than being answered. A parsed `Value` costs about ten times its
/// bytes, so one parse at the limit is about 40 MB; [`OPA_MAX_CONCURRENT_PARSES`]
/// bounds how many run at once.
pub const OPA_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// The most request bodies parsed at once, before authentication. At the
/// body limit a parse holds about 40 MB, so 16 bound the transient pre-auth
/// parse memory near 640 MB. A permit is held only while the body is parsed
/// (microseconds for the small `/allow` bodies that make up nearly all of
/// Trino's traffic), so 16 is not a throughput limit in practice.
pub const OPA_MAX_CONCURRENT_PARSES: usize = 16;

/// How long a request waits for a parse permit before it is answered 503
/// (the plugin fails the query: fail closed).
pub const OPA_PARSE_WAIT: Duration = Duration::from_secs(1);

/// Bounds the pre-authentication parses ([`OPA_MAX_CONCURRENT_PARSES`],
/// [`OPA_PARSE_WAIT`] in production).
pub struct ParseGate {
    permits: tokio::sync::Semaphore,
    wait: Duration,
}

impl ParseGate {
    pub fn new(permits: usize, wait: Duration) -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(permits),
            wait,
        }
    }

    /// A permit, or `None` once the wait has passed without one.
    pub async fn acquire(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        tokio::time::timeout(self.wait, self.permits.acquire())
            .await
            .ok()
            .and_then(Result::ok)
    }
}

impl Default for ParseGate {
    fn default() -> Self {
        Self::new(OPA_MAX_CONCURRENT_PARSES, OPA_PARSE_WAIT)
    }
}

/// How long a verified `(engine, secret)` is remembered before the verifier is
/// asked again (spec: 5 s). A retired secret keeps being accepted for at most
/// this long.
pub const VERIFIED_TTL: Duration = Duration::from_secs(5);

/// How long a refused `(engine, secret)` is remembered as refused. A negative
/// entry can only refuse, never admit, and it is keyed by the secret it
/// refused, so it never refuses a different (right) secret.
pub const NEGATIVE_TTL: Duration = Duration::from_secs(5);

/// The most `(engine, secret)` verdicts held in each of the verified and the
/// refused map. Past it, that map's expired verdicts are dropped first; if it
/// is still full, the new verdict is simply not cached (the next request asks
/// the verifier again), so a flood of distinct wrong secrets cannot grow it.
pub const MAX_VERDICTS: usize = 4096;

#[derive(Clone)]
pub struct OpaState {
    pub cache: Arc<SnapshotCache>,
    pub verifier: Arc<dyn EngineCredentialVerifier>,
    pub credentials: Arc<VerifiedCache>,
    pub parses: Arc<ParseGate>,
}

impl OpaState {
    /// The production state: credentials remembered for [`VERIFIED_TTL`],
    /// parses bounded by [`ParseGate::default`].
    pub fn new(cache: Arc<SnapshotCache>, verifier: Arc<dyn EngineCredentialVerifier>) -> Self {
        Self {
            cache,
            verifier,
            credentials: Arc::new(VerifiedCache::new(VERIFIED_TTL)),
            parses: Arc::new(ParseGate::default()),
        }
    }
}

/// `(engine, sha256(secret))`.
type VerdictKey = (Uuid, [u8; 32]);

/// Remembers only "this (engine, sha256(secret)) verified / was refused" (I3).
/// Never the binding, never the profile, never the secret.
///
/// Verified and refused verdicts are held in separate maps, each capped at
/// [`MAX_VERDICTS`], so a flood of distinct wrong secrets fills only the
/// refused map and never stops a right secret from being remembered. The
/// verified map holds only secrets the verifier accepted, which a caller
/// without a valid secret cannot add to.
pub struct VerifiedCache {
    ttl: Duration,
    negative_ttl: Duration,
    verdicts: Mutex<Verdicts>,
}

#[derive(Default)]
struct Verdicts {
    /// `(engine, sha256(secret))` → expires.
    verified: HashMap<VerdictKey, Instant>,
    refused: HashMap<VerdictKey, Instant>,
}

/// Remember `key` in `map` until `expires`, unless the map is full of live
/// entries.
fn remember(
    map: &mut HashMap<VerdictKey, Instant>,
    key: VerdictKey,
    expires: Instant,
    now: Instant,
) {
    if map.len() >= MAX_VERDICTS && !map.contains_key(&key) {
        map.retain(|_, e| *e > now);
    }
    if map.len() < MAX_VERDICTS || map.contains_key(&key) {
        map.insert(key, expires);
    }
}

impl VerifiedCache {
    /// `ttl`: how long a verified secret is remembered (production: 5 s). A
    /// refused one is remembered for `min(ttl, NEGATIVE_TTL)`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            negative_ttl: ttl.min(NEGATIVE_TTL),
            verdicts: Mutex::new(Verdicts::default()),
        }
    }

    /// true: verified now or within the TTL. Err only for an infrastructure
    /// failure of the verifier, which is never cached.
    pub async fn verified(
        &self,
        v: &dyn EngineCredentialVerifier,
        engine: Uuid,
        secret: &str,
    ) -> Result<bool, String> {
        let key: VerdictKey = (engine, Sha256::digest(secret.as_bytes()).into());
        {
            let verdicts = self.verdicts.lock().unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            let live = |m: &HashMap<VerdictKey, Instant>| m.get(&key).is_some_and(|e| *e > now);
            if live(&verdicts.verified) {
                return Ok(true);
            }
            if live(&verdicts.refused) {
                return Ok(false);
            }
        }
        // An identity for another engine than the one asked about is a
        // refusal, whatever the verifier meant by it.
        let ok = v
            .verify(engine, secret)
            .await?
            .is_some_and(|id| id.engine_id == engine);
        let now = Instant::now();
        let mut verdicts = self.verdicts.lock().unwrap_or_else(|p| p.into_inner());
        if ok {
            remember(&mut verdicts.verified, key, now + self.ttl, now);
        } else {
            remember(&mut verdicts.refused, key, now + self.negative_ttl, now);
        }
        Ok(ok)
    }
}

/// The path the router mounts `route` at.
fn route_path(route: Route) -> String {
    format!("{OPA_SURFACE_PREFIX}/{}", route.name())
}

/// The five paths [`opa_surface`] mounts, each from the same function that
/// mounts it. The engine-side plugin configuration
/// (`egret_engine_mgmt_api::opa_config::opa_plugin_files`) must point at
/// exactly these; a test compares them, so a renamed route cannot silently
/// break the engine.
pub fn opa_route_paths() -> [String; 5] {
    [
        route_path(Route::Allow),
        route_path(Route::Batch),
        route_path(Route::RowFilters),
        route_path(Route::BatchColumnMasks),
        route_path(Route::ColumnMask),
    ]
}

/// The routes, under [`OPA_SURFACE_PREFIX`].
pub fn opa_surface(state: OpaState) -> axum::Router {
    axum::Router::new()
        .route(&route_path(Route::Allow), post(allow))
        .route(&route_path(Route::Batch), post(batch))
        .route(&route_path(Route::RowFilters), post(row_filters))
        .route(
            &route_path(Route::BatchColumnMasks),
            post(batch_column_masks),
        )
        .route(&route_path(Route::ColumnMask), post(column_mask))
        .layer(DefaultBodyLimit::max(OPA_BODY_LIMIT_BYTES))
        .with_state(state)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Allow,
    Batch,
    RowFilters,
    BatchColumnMasks,
    ColumnMask,
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Route::Allow => "allow",
            Route::Batch => "batch",
            Route::RowFilters => "row_filters",
            Route::BatchColumnMasks => "batch_column_masks",
            Route::ColumnMask => "column_mask",
        }
    }

    /// The denying answer: `false` / `[]` on the decision routes, 403 on the
    /// value routes, where `[]` / `null` would mean "no filter, no mask".
    fn refusal(self, reason: &str) -> Response {
        match self {
            Route::Allow => (StatusCode::OK, Json(wire::allow_result(false))).into_response(),
            Route::Batch => (StatusCode::OK, Json(json!({ "result": [] }))).into_response(),
            Route::RowFilters | Route::BatchColumnMasks | Route::ColumnMask => {
                (StatusCode::FORBIDDEN, Json(json!({ "error": reason }))).into_response()
            }
        }
    }
}

/// What a log line says about the request. Caller-chosen strings (user,
/// operation) are logged with `?` so a newline in them cannot forge a line.
struct Who<'a> {
    route: Route,
    engine: Option<Uuid>,
    req: Option<&'a OpaRequest>,
}

impl Who<'_> {
    fn engine(&self) -> String {
        self.engine.map_or_else(|| "-".into(), |e| e.to_string())
    }
    fn user(&self) -> &str {
        self.req.map_or("-", |r| r.user.as_str())
    }
    fn operation(&self) -> &str {
        self.req.map_or("-", |r| r.operation.as_str())
    }

    /// Log one denial and count it.
    fn denied(&self, resource: Option<&Resource>, reason: &str) {
        tracing::info!(
            target: "egret::opa",
            route = self.route.name(),
            engine = %self.engine(),
            user = ?self.user(),
            operation = ?self.operation(),
            resource = ?resource,
            reason = %reason,
            "opa: denied"
        );
        metrics::counter!("egret_opa_deny_total", "route" => self.route.name()).increment(1);
    }

    fn allowed(&self) {
        metrics::counter!("egret_opa_allow_total", "route" => self.route.name()).increment(1);
    }

    /// Too many bodies being parsed: 503, so the plugin fails the query.
    fn busy(&self) -> Response {
        self.denied(None, "too many requests being parsed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "busy" })),
        )
            .into_response()
    }

    /// Log and count a refusal, and answer it.
    fn refuse(&self, reason: &str) -> Response {
        self.refuse_logging(reason, reason)
    }

    /// A refusal whose log line carries more than the answer: `answered` goes
    /// to the engine, `logged` only to egret's log.
    fn refuse_logging(&self, answered: &str, logged: &str) -> Response {
        self.denied(self.req.map(|r| &r.resource), logged);
        self.route.refusal(answered)
    }

    /// An internal error: 500, so the plugin fails the query.
    fn internal(&self, what: &str, detail: &str) -> Response {
        tracing::error!(
            target: "egret::opa",
            route = self.route.name(),
            engine = %self.engine(),
            user = ?self.user(),
            operation = ?self.operation(),
            error = %detail,
            "opa: {what}"
        );
        metrics::counter!("egret_opa_error_total", "route" => self.route.name()).increment(1);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": what })),
        )
            .into_response()
    }

    /// A result builder's answer: its `Err` (an index outside the request,
    /// i.e. an evaluator bug) is a 500, never a partial or empty result.
    fn built(&self, result: Result<Value, String>) -> Response {
        match result {
            Ok(v) => Json(v).into_response(),
            Err(e) => self.internal("result index outside the request", &e),
        }
    }

    /// `EvalError` → 500. The line names the user and the column only, never
    /// a mask expression.
    fn eval_error(&self, e: EvalError) -> Response {
        match e {
            EvalError::TwoMasks { column, .. } => {
                tracing::error!(
                    target: "egret::opa",
                    route = self.route.name(),
                    engine = %self.engine(),
                    user = ?self.user(),
                    column = ?column,
                    "opa: two masks on one column"
                );
                metrics::counter!("egret_opa_error_total", "route" => self.route.name())
                    .increment(1);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "two masks on one column" })),
                )
                    .into_response()
            }
        }
    }
}

/// Parse, authenticate, and find the engine's Ready snapshot. `Err` is the
/// response to send (already logged and counted), boxed because a
/// `Response` is large.
async fn admit(
    state: &OpaState,
    route: Route,
    body: Result<Bytes, BytesRejection>,
) -> Result<(OpaRequest, Uuid, Arc<Snapshot>), Box<Response>> {
    let mut who = Who {
        route,
        engine: None,
        req: None,
    };
    // Over the body limit (413) or unreadable: logged and counted like any
    // refusal, answered with the extractor's own non-2xx status.
    let body = match body {
        Ok(b) => b,
        Err(rejection) => {
            who.denied(None, &format!("request body refused: {rejection}"));
            return Err(rejection.into_response().into());
        }
    };
    let parsed = {
        let Some(_permit) = state.parses.acquire().await else {
            return Err(who.busy().into());
        };
        serde_json::from_slice::<Value>(&body)
            .map_err(|_| "body is not JSON".to_string())
            .and_then(|v| wire::parse_request(&v))
    };
    let req = match parsed {
        Ok(r) => r,
        Err(e) => return Err(who.refuse(&format!("malformed input: {e}")).into()),
    };
    who.req = Some(&req);

    let engine = req.engine.as_deref().and_then(|e| Uuid::parse_str(e).ok());
    let (Some(engine), Some(secret)) = (engine, req.secret.as_ref()) else {
        return Err(who
            .refuse("no engine credential in opa.context-file")
            .into());
    };
    who.engine = Some(engine);

    match state
        .credentials
        .verified(state.verifier.as_ref(), engine, secret.expose())
        .await
    {
        Ok(true) => {}
        Ok(false) => return Err(who.refuse("engine credential refused").into()),
        Err(e) => return Err(who.internal("engine credential check failed", &e).into()),
    }

    // I3: the binding comes from the refresher's map, read now.
    let Some(service_id) = state.cache.binding(engine) else {
        return Err(who
            .refuse("engine is bound to no profile served on OPA")
            .into());
    };
    let snapshot = match state.cache.get(service_id) {
        Lookup::Ready(s) => s,
        Lookup::NotOpa => return Err(who.refuse("profile is not on OPA").into()),
        // The stored build error can name rules, tables, other users and
        // database internals: it is logged, never answered.
        Lookup::NotBuilt { error: None } => return Err(who.refuse("no snapshot: building").into()),
        Lookup::NotBuilt { error: Some(e) } => {
            return Err(who
                .refuse_logging("no snapshot: build failed", &format!("no snapshot: {e:?}"))
                .into())
        }
    };
    Ok((req, engine, snapshot))
}

async fn allow(State(state): State<OpaState>, body: Result<Bytes, BytesRejection>) -> Response {
    let (req, engine, s) = match admit(&state, Route::Allow, body).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let who = Who {
        route: Route::Allow,
        engine: Some(engine),
        req: Some(&req),
    };
    match s.decide(&req.user, &req.operation, &req.resource) {
        Decision::Allow { .. } => {
            who.allowed();
            Json(wire::allow_result(true)).into_response()
        }
        Decision::Deny { reason, .. } => {
            who.denied(Some(&req.resource), &reason);
            Json(wire::allow_result(false)).into_response()
        }
    }
}

async fn batch(State(state): State<OpaState>, body: Result<Bytes, BytesRejection>) -> Response {
    let (req, engine, s) = match admit(&state, Route::Batch, body).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let who = Who {
        route: Route::Batch,
        engine: Some(engine),
        req: Some(&req),
    };
    let expanded;
    let resources: &[Resource] = if req.operation == FILTER_COLUMNS {
        match wire::expand_filter_columns(&req) {
            Ok(r) => {
                expanded = r;
                &expanded
            }
            Err(e) => return who.refuse(&e),
        }
    } else {
        &req.filter_resources
    };
    let request_len = match wire::batch_len(&req) {
        Ok(n) => n,
        Err(e) => return who.refuse(&e),
    };
    let allowed = s.filter(&req.user, &req.operation, resources);
    let denied = resources.len().saturating_sub(allowed.len());
    if denied > 0 {
        // One line per request, however many items it denies.
        let first = (0..resources.len()).find(|i| !allowed.contains(i));
        tracing::info!(
            target: "egret::opa",
            route = Route::Batch.name(),
            engine = %who.engine(),
            user = ?who.user(),
            operation = ?who.operation(),
            resource = ?first.map(|i| &resources[i]),
            denied,
            reason = "not allowed",
            "opa: denied"
        );
        metrics::counter!("egret_opa_deny_total", "route" => Route::Batch.name()).increment(1);
    } else {
        who.allowed();
    }
    who.built(wire::batch_result(&allowed, request_len))
}

async fn row_filters(
    State(state): State<OpaState>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (req, engine, s) = match admit(&state, Route::RowFilters, body).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let who = Who {
        route: Route::RowFilters,
        engine: Some(engine),
        req: Some(&req),
    };
    let t = match wire::row_filter_target(&req) {
        Ok(t) => t,
        Err(e) => return who.refuse(&e),
    };
    match s.row_filters(&req.user, &t.catalog, &t.schema, &t.table) {
        Ok(filters) => {
            who.allowed();
            Json(wire::row_filters_result(&filters)).into_response()
        }
        Err(e) => who.eval_error(e),
    }
}

async fn batch_column_masks(
    State(state): State<OpaState>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (req, engine, s) = match admit(&state, Route::BatchColumnMasks, body).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let who = Who {
        route: Route::BatchColumnMasks,
        engine: Some(engine),
        req: Some(&req),
    };
    let targets = match wire::column_mask_targets(&req) {
        Ok(t) => t,
        Err(e) => return who.refuse(&e),
    };
    let request_len = match wire::batch_len(&req) {
        Ok(n) => n,
        Err(e) => return who.refuse(&e),
    };
    let t = &targets.table;
    let masks = match s.column_masks(&req.user, &t.catalog, &t.schema, &t.table, &targets.columns) {
        Ok(m) => m,
        Err(e) => return who.eval_error(e),
    };
    // `column_masks` keys by column name; the plugin addresses the request's
    // own list by index.
    let indexed: Vec<(usize, String)> = targets
        .columns
        .iter()
        .enumerate()
        .filter_map(|(i, (name, _))| masks.get(name).map(|e| (i, e.clone())))
        .collect();
    let built = wire::column_masks_result(&indexed, request_len);
    if built.is_ok() {
        who.allowed();
    }
    who.built(built)
}

async fn column_mask(
    State(state): State<OpaState>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (req, engine, s) = match admit(&state, Route::ColumnMask, body).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let who = Who {
        route: Route::ColumnMask,
        engine: Some(engine),
        req: Some(&req),
    };
    let targets = match wire::column_mask_target(&req) {
        Ok(t) => t,
        Err(e) => return who.refuse(&e),
    };
    let t = &targets.table;
    match s.column_masks(&req.user, &t.catalog, &t.schema, &t.table, &targets.columns) {
        Ok(masks) => {
            who.allowed();
            let column = &targets.columns[0].0;
            Json(wire::column_mask_result(
                masks.get(column).map(String::as_str),
            ))
            .into_response()
        }
        Err(e) => who.eval_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carry 2: a builder `Err` is a 500 on both batch routes; `Ok` is the
    /// builder's own value. Not reachable through the router without an
    /// evaluator bug, so pinned here.
    #[test]
    fn a_builder_error_is_a_500_not_a_result() {
        for route in [Route::Batch, Route::BatchColumnMasks] {
            let who = Who {
                route,
                engine: None,
                req: None,
            };
            assert_eq!(
                who.built(wire::batch_result(&[3], 2)).status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
            assert_eq!(
                who.built(wire::column_masks_result(&[(0, "NULL".into())], 0))
                    .status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
            assert_eq!(
                who.built(wire::batch_result(&[1], 2)).status(),
                StatusCode::OK
            );
        }
    }
}
