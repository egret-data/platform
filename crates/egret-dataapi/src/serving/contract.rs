//! Package 8 — the contract a consumer can fetch with the credential it
//! actually holds.
//!
//! **Not [`crate::contract`]**, which is Package 6's publish-time diff between
//! two frozen versions. This module is the OpenAPI *document*: one route, and
//! the argument for why a bearer may have it.
//!
//! ```text
//! GET /data-api/v1/{ws}      Authorization: Bearer <data-api JWT>
//! ```
//!
//! # The gap this closes
//!
//! `GET /api/ws/{workspace}/data-api/openapi` generates the document a consumer
//! builds a client from, and it sits behind a human session and
//! `data_api_endpoint:manage`. A service account holds neither: it holds a
//! `data-api` bearer token. So the one party the document exists for could not
//! fetch it, and every consuming team had to be mailed a copy by a workspace
//! admin — a copy that goes stale the moment anyone publishes.
//!
//! # Why this is a SECOND route and not a looser gate on the first one
//!
//! `routes.rs`' module comment puts the management route on `:manage` for a
//! reason that is still true, and it is worth quoting rather than paraphrasing:
//! reading grants "enumerates every consumer of the endpoint", and the generated
//! document "answers that question per account and iterable" — a caller holding
//! `:read` could walk the workspace's service accounts, ask for each one's
//! document, and reconstruct the entire grant graph together with every path,
//! parameter and column name in the workspace. Loosening that route would hand
//! `:read` exactly what `:manage` is there to protect.
//!
//! A bearer token is not a weaker version of that caller — it is a *different*
//! one. It cannot name a service account, because it IS one: the account is read
//! off the token's `egret_user_id` claim and nothing in the request can change
//! it. There is no iteration to do and no roster to enumerate; the only document
//! reachable this way is the account's own.
//!
//! # The property that makes this safe
//!
//! **The document describes exactly what this service account may invoke, and
//! nothing else.** Endpoint paths, parameter names and column names are the
//! reconnaissance an unauthorised caller wants, so a document listing every
//! endpoint in the workspace would be an information disclosure even though
//! calling them all would still be refused.
//!
//! That property is not re-implemented here. [`crate::openapi_gen::build`] asks
//! [`crate::invoke::can_invoke`] per candidate endpoint — the SAME expression
//! `serving::router`'s step 4 evaluates before it runs anything — and skips
//! every endpoint the answer is `false` for. So "what may this account call" has
//! one answer on this surface, not two that can drift: a grant revoked between
//! two fetches is gone from the second document, and an endpoint reachable only
//! through the Cedar `ws_admin` cascade is present in both the document and the
//! call.
//!
//! # A document is not a lesser thing than a call
//!
//! Every check the serving path applies before it decides WHICH endpoint is
//! being called is applied here too, in the same order and with the same answer:
//!
//! * the bearer token must verify, for the `data-api` audience — one
//!   [`ServingError::Unauthorized`] for every way it can fail, as on the call
//!   path, so the refusal is not an oracle;
//! * the workspace in the URL must be the workspace in the token, and it must
//!   not be archived — 404 either way, never 403, so a caller cannot learn that
//!   a workspace exists;
//! * the per-endpoint invoke grant, via `build`, as above.
//!
//! What it deliberately does NOT apply is the response cache, and the reason is
//! in "Not cacheable" below. The surface-wide bounds — body cap, deadline,
//! bulkhead — DO apply, because the route is registered inside
//! `serving_router_sharing_bulkhead`'s layers.
//!
//! # It IS rate limited, and the asymmetry is the reason
//!
//! Building this document costs one PDP decision per live endpoint — two DB
//! round trips each — bounded only by
//! [`crate::openapi_gen::MAX_DOCUMENTED_ENDPOINTS`], i.e. **up to about 400
//! round trips for one request**, and a holder of one valid bearer can loop it.
//! That makes it the highest-amplification request on this surface. Left
//! unlimited it would also have been the ONLY unlimited one: the cheapest
//! request here — a call to an endpoint whose author configured a limit — is
//! bounded, and the most expensive was not. The bulkhead does not close that.
//! It bounds CONCURRENCY, so a serial loop never trips it; it caps how many
//! expensive requests run at once, not how many an account may issue.
//!
//! The per-endpoint limiter genuinely does not fit — it is keyed on
//! `endpoint_id` and this request names none — but "the existing bucket does not
//! fit" is an argument for a different bucket, not for no bucket. So the route
//! takes its own: [`crate::serving::ratelimit::contract_key`], a third `d:`
//! namespace keyed on the workspace and the SERVICE ACCOUNT, both read off the
//! signed token, at [`crate::serving::ratelimit::contract_rate`].
//!
//! Three properties that bucket has to have, and why it has them:
//!
//! * **It cannot starve a call.** Its own namespace, so a contract fetch never
//!   spends a token an endpoint call then needs. Reusing the endpoint-wide
//!   bucket under a synthetic key would have done exactly that — refusing paid
//!   traffic in order to bound a discovery request.
//! * **The caller cannot choose its own rate.** Both halves of the key are token
//!   claims. Package 4's lesson is that a bucket keyed without the dimension
//!   that varies lets a caller pick its rate; the dimension the document varies
//!   by is the service account, and that is the key. Not `client_id` — see
//!   `contract_key`.
//! * **A store outage does not make it unlimited.** The rate is a constant
//!   rather than an author's published number, so unlike a call this route
//!   ALWAYS consults the store, and a `Refusal::Unavailable` is `503
//!   rate_limiter_unavailable` exactly as on the call path. The cost of that is
//!   stated rather than hidden: a call to an endpoint with no configured limit
//!   keeps serving through a store outage and a contract fetch does not. That is
//!   the fail-closed policy applied to a route that has no unlimited mode to
//!   fall back to, and it is the cheaper failure — a consumer needs this
//!   document when it BUILDS a client, not per call, and the one it already has
//!   keeps working.
//!
//! There is deliberately no second, workspace-wide contract bucket. It would let
//! one account's loop deny every other consumer in the workspace its contract —
//! a worse failure than the load it prevents — and the workspace-wide bound that
//! does exist is the bulkhead.
//!
//! The refusal is `429 rate_limited` with `Retry-After`, the same variant and
//! the same header the call path answers with, so a consumer needs no new case
//! for it. No new [`ServingError`] variant: see `contract_error`.
//!
//! # Why the URL is the server URL itself, and why it cannot be shadowed
//!
//! `servers[0].url` in the generated document is `{base}/data-api/v1/{slug}`, so
//! this route is a GET of the very URL the document tells a consumer to build
//! its calls from. Nothing has to be documented for a consumer to find it.
//!
//! It is also the one URL under this mount that a published endpoint can never
//! occupy. The serving route is `/{ws}/{*path}`, and
//! [`crate::endpoints::normalize_path`] refuses an empty path, so every
//! published URL has at least one segment after the slug. A reserved literal
//! anywhere BELOW the slug — `/{ws}/openapi.json`, `/{ws}/.well-known/…` — would
//! not have that property: both spellings pass `normalize_path` (it allows
//! letters, digits, `.`, `_`, `~` and `-`), so an author could publish an
//! endpoint there and axum, which prefers a static segment to a wildcard, would
//! silently answer with this document instead of their rows.
//! `tests/contract_reachable.rs`'s
//! `a_published_endpoint_cannot_be_shadowed_by_the_contract_url` pins both
//! halves, and it fails under exactly that refactor.
//!
//! # No access-log row; a metric, under a label of its own
//!
//! `dataapi.access_log` is shaped around a CALL to an endpoint: the row carries
//! `endpoint_id`, `version` and `row_count`, and `activity` aggregates call
//! counts and error rates PER ENDPOINT from it. A contract fetch names no
//! endpoint, so every one of those columns would be NULL — and the rows would
//! still be counted by the workspace-level statistics and by the Package 2
//! alerting built on them, quietly changing what "this endpoint's error rate"
//! means. So there is no row, and if "who fetched a contract, and when" is ever
//! wanted as a queryable fact it belongs in `egret-audit` beside the other
//! management-plane events, not in a table whose every consumer assumes a row is
//! a served query.
//!
//! **The metrics are a different case, and this module used to get it wrong.**
//! The argument above was extended to `egret_dataapi_requests_total` on the
//! grounds that an operator reads it as serving traffic. What that produced was
//! measured on a two-replica cluster: twenty concurrent contract fetches
//! answered nineteen `200`s and one `429`, and afterwards `/metrics` on BOTH
//! replicas showed only the serving path's `key_invalid` and `not_granted`
//! refusals. Neither the successes nor the rate-limit refusal existed anywhere.
//! The most expensive request this surface serves — up to
//! [`crate::openapi_gen::MAX_DOCUMENTED_ENDPOINTS`] PDP decisions, two DB round
//! trips each — and the bucket added specifically to bound it were invisible to
//! the only instrument that can see them, which is the design doc's §5 blind
//! spot exactly: a refusal no instrument records. It is worse here than on the
//! call path, because the access log cannot be the backstop — a contract fetch
//! is not merely unrecorded by it, it is unrecordABLE.
//!
//! So this route now records the same counter and the same histogram the call
//! path does, under `route="contract"` — see [`super::metrics::Route`] for why
//! that is a label rather than a second metric name, and why the label names
//! which of two ROUTES answered rather than anything about the caller.
//! `{route="call"}` is what every panel written before this change meant, so an
//! operator narrows back to it in one selector; `sum by (code)` without a route
//! selector is now the whole mount rather than most of it.
//!
//! The `tracing` lines stay as they are — one `debug` on success naming how many
//! paths were described, one `warn` on every refusal. They carry the
//! `request_id`, the workspace and the service account, which is what a metric
//! must not.
//!
//! # Not cacheable, and not by omission
//!
//! The response carries `Cache-Control: no-store`, and this surface's own
//! response cache is not consulted at all.
//!
//! The document varies by SERVICE ACCOUNT — that is the whole point of it — so a
//! cache keyed on the URL alone would hand one consumer another's contract. That
//! is the exact trap `serving::cache`'s principal-keyed key exists to avoid, and
//! it is worse here than on a call: a shared entry would leak the paths and
//! parameter names of endpoints the recipient was never granted, which is the
//! one property this module exists to hold.
//!
//! Keying it on the principal would fix that and still be wrong. The document
//! also varies with every grant written or revoked and with every publish, none
//! of which invalidates anything; a consumer that fetched its contract seconds
//! after being granted an endpoint would be told it has none, and a revocation
//! would keep describing an endpoint that now answers 403. Grants are read per
//! request on the serving path for that reason, and a document must not be
//! staler than the calls it describes. The cost is bounded by the same bulkhead
//! and the same `MAX_DOCUMENTED_ENDPOINTS` cap the management route pays.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::time::Instant;
use uuid::Uuid;

use super::error::{to_wire, ServingErrorBody};
use super::metrics;
// No `RateLimitStore` import: `st.rate_limits` is a `dyn` trait object, whose
// trait methods are inherent to the object type.
use super::ratelimit;
use super::ServingError;
use crate::openapi_gen::OpenApiGenError;
use crate::routes::DataApiState;

/// Map a generation failure onto the serving taxonomy.
///
/// Exhaustive with no `_` arm, exactly as [`to_wire`] is, so a new
/// [`OpenApiGenError`] variant is a compile error here rather than a silent 500.
///
/// * `NamingCollision` becomes the code the serving path already answers for
///   that very endpoint — `503 output_naming_collision`. It is the same
///   condition, said the same way, and the account holds invoke on the endpoint
///   in question (`build` filters before it reads the snapshot), so nothing is
///   disclosed by admitting it exists. The management route answers 409 and
///   names the endpoint; that message is for the author who can re-publish it,
///   and this surface does not have one.
/// * `TooManyEndpoints` becomes `500 internal`, and the honest reason is that a
///   consumer has no remedy for it. The management route's remedy is
///   `?endpoint_id=`, which this route deliberately does not offer: a consumer
///   asking "what may I call" cannot narrow by an id it does not yet know, and
///   accepting one would make the route an existence oracle over endpoint ids
///   for no gain. So the bound is an egret-side limit reached in this workspace,
///   which is what `internal` means; the `warn` below is what an operator acts
///   on.
/// * `Db` and `Core` are `internal` for the reason everything on this surface
///   is: the caller is a machine outside the network and the detail goes to the
///   log.
///
/// **No new [`ServingError`] variant.** `serving_openapi`'s
/// `the_published_contract_names_every_answer_the_surface_can_give` requires
/// every census variant's status to be documented on `serve_get` AND
/// `serve_post`, so a variant only this route can produce would have to be
/// declared on two operations that can never produce it — a document that lies
/// in order to satisfy a test.
fn contract_error(ws: egret_core::WorkspaceId, e: OpenApiGenError) -> ServingError {
    match e {
        OpenApiGenError::NamingCollision { endpoint } => {
            tracing::warn!(
                workspace_id = %ws.0,
                %endpoint,
                "data-api contract: an invocable endpoint's output naming collides; re-publish it"
            );
            ServingError::OutputNamingCollision
        }
        e @ OpenApiGenError::TooManyEndpoints { .. } => {
            tracing::warn!(
                workspace_id = %ws.0,
                error = %e,
                "data-api contract refused: this workspace has more live endpoints than one \
                 generated document may describe"
            );
            ServingError::internal(e)
        }
        OpenApiGenError::Db(e) => ServingError::internal(e),
        OpenApiGenError::Core(e) => ServingError::internal(e),
    }
}

// `GET /data-api/v1/{ws}` — the OpenAPI document for the calling service
// account. See the module comment for why this exists beside the
// `:manage`-gated management route rather than replacing it, and for the
// scoping property it holds.
//
// A `//` comment and NOT a doc comment, exactly as on `serve_get` below it and
// for the same reason: `utoipa` lifts a doc comment on a handler into the
// operation's `summary`/`description`, and a published document telling a
// machine consumer to "see the module comment" is telling them to go and read
// egret's Rust source. The `summary` and `description` written into the
// attribute below are the consumer-facing ones.
#[utoipa::path(
    get,
    path = "/data-api/v1/{ws}",
    tag = "data-api-serving",
    summary = "Fetch this service account's own API contract",
    description = "Returns an OpenAPI 3.0.3 document describing every published endpoint \
                   THIS service account may invoke in this workspace, taken from each \
                   endpoint's frozen published version. The account is read from the bearer \
                   token, so the document is always the caller's own and never another \
                   account's. Endpoints the account holds no invoke grant on are absent \
                   entirely — paths, parameters and column names alike — so an empty `paths` \
                   object means this account has been granted nothing rather than that the \
                   workspace is empty. The document is generated per request and reflects \
                   grants, publishes and endpoint state as they are at that instant; it is \
                   not cacheable.",
    params(
        ("ws" = String, Path, description = "Workspace slug; must match the token's `ws` claim"),
    ),
    responses(
        (status = 200, description = "An OpenAPI 3.0.3 document describing exactly the endpoints THIS service account may invoke, and nothing else in the workspace. `servers[0].url` is this same URL, so joining it with a `paths` key gives a URL that can be called with the same token. `Cache-Control: no-store`: the document is per service account and reflects grants as they are at this instant", body = serde_json::Value),
        (status = 401, description = "`key_invalid` — bearer token missing, expired, or not for this audience", body = ServingErrorBody),
        (status = 404, description = "`not_found` — no such workspace, or it is not the one the token was issued for, or it is archived. One code for all three, so a caller cannot learn that a workspace exists", body = ServingErrorBody),
        (status = 413, description = "`payload_too_large` — the request declared a body larger than this surface accepts", body = ServingErrorBody),
        (status = 429, description = "`rate_limited` — this service account's contract allowance is spent; `Retry-After` states when it refills. The allowance is egret's, not an endpoint's: building this document is the most expensive request on the surface, so it has a bucket of its own, keyed on the service account and shared by every client the account holds", body = ServingErrorBody),
        (status = 500, description = "`internal` — an egret-side fault, including a workspace with more live endpoints than one document may describe; quote `request_id` to support", body = ServingErrorBody),
        (status = 503, description = "`output_naming_collision` — an endpoint this account may invoke has a published output that maps two columns onto one key, so there is no document to describe it; its author must re-publish. `overloaded` — egret shed the request because the serving surface was at its concurrency limit. `rate_limiter_unavailable` — the shared rate-limit store could not be consulted, so the request was refused rather than admitted without an allowance. The last two clear by themselves", body = ServingErrorBody),
        (status = 504, description = "`deadline_exceeded` — the request outran egret's own serving deadline", body = ServingErrorBody),
    ),
    security(("data_api_bearer" = [])),
)]
pub(super) async fn serve_contract(
    State(st): State<DataApiState>,
    Path(ws_slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4().to_string();
    // ONE reading, feeding the counter and the histogram, taken around
    // everything the caller waited for — `router::serve` measures the same span
    // for the same reason.
    let started = Instant::now();
    let (response, status, code) = match build_contract(&st, &ws_slug, &headers).await {
        Ok(doc) => {
            // The line carries what a metric may not: the request id and the
            // size of the answer. See the module comment for why there is no
            // `dataapi.access_log` row — that instrument is per ENDPOINT, and
            // this request names none.
            tracing::debug!(
                %request_id,
                paths = doc["paths"].as_object().map_or(0, serde_json::Map::len),
                "data-api contract served"
            );
            (
                (
                    // `no-store`, not `no-cache`: see the module comment. A
                    // shared proxy that kept this by URL would serve one
                    // consumer another's contract, because the URL is the same
                    // for every service account in the workspace and the token
                    // is what tells them apart.
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(doc),
                )
                    .into_response(),
                StatusCode::OK,
                metrics::OK_CODE,
            )
        }
        Err(e) => {
            let wire = to_wire(&e);
            // The only place the detail is allowed to appear, exactly as on the
            // call path (`router::serve`): `?e` renders the variant and, for
            // `Internal`, its source — which is what the body must not carry.
            tracing::warn!(
                %request_id,
                code = wire.code,
                status = wire.status.as_u16(),
                error = ?e,
                "data-api contract request failed"
            );
            let (status, code) = (wire.status, wire.code);
            (wire.into_response(&request_id), status, code)
        }
    };

    // The same two labels the call path records, under `route="contract"`.
    // Taken from the match above rather than re-derived from the response, so
    // one request is one increment and the counter cannot disagree with the
    // `tracing` line about what happened — `router::serve`'s arrangement, for
    // its reason. `Route::Contract` is named here, not read off the URI: this
    // IS the contract handler.
    metrics::record_request(metrics::Route::Contract, status, code, started.elapsed());
    response
}

/// Spend one token from this service account's contract bucket.
///
/// See the module comment for why this route has a bucket of its own rather than
/// none and rather than the endpoint's. The two refusals are mapped exactly as
/// `router::run` maps them, deliberately: a consumer already has a case for
/// `429 rate_limited` and for `503 rate_limiter_unavailable` from the call path,
/// and inventing a third answer for the same two conditions would give it a
/// third case to write for no new information.
///
/// The log lines differ from the call path's in one way only — they name the
/// service account instead of the endpoint, because there is no endpoint. An
/// operator seeing these is looking at one account looping on discovery, which
/// is a different picture from one client hammering one endpoint and should not
/// be reported as if it were the same.
async fn contract_limit(
    st: &DataApiState,
    principal: &super::ServingPrincipal,
) -> Result<(), ServingError> {
    let key = ratelimit::contract_key(principal.workspace_id, principal.user_id.0);
    st.rate_limits
        .check_at(&key, ratelimit::contract_rate(), Instant::now())
        .await
        .map_err(|refusal| match refusal {
            ratelimit::Refusal::Exhausted(retry_after) => {
                tracing::debug!(
                    workspace_id = %principal.workspace_id,
                    service_account_id = %principal.user_id.0,
                    retry_after_secs = ratelimit::retry_after_secs(retry_after),
                    "data-api contract request rate limited"
                );
                ServingError::RateLimited { retry_after }
            }
            ratelimit::Refusal::Unavailable => {
                tracing::warn!(
                    workspace_id = %principal.workspace_id,
                    service_account_id = %principal.user_id.0,
                    "data-api contract rate limit could not be checked; refusing the request \
                     rather than admitting it without an allowance"
                );
                ServingError::LimiterUnavailable
            }
        })
}

/// The chain proper, so [`serve_contract`] owns only the envelope.
///
/// The order is `router::run`'s order for the steps that apply: authenticate,
/// then resolve the workspace against the TOKEN's claim, then let
/// [`crate::openapi_gen::build`] apply the per-endpoint invoke grant.
async fn build_contract(
    st: &DataApiState,
    ws_slug: &str,
    headers: &HeaderMap,
) -> Result<serde_json::Value, ServingError> {
    let principal = super::verify_serving_token(&*st.keys, super::router::bearer(headers)?).await?;

    // The limiter, BEFORE the workspace lookup and therefore before any query
    // this request would otherwise run. Both halves of the key are claims of the
    // token just verified, so nothing in the request — not the slug, not a
    // header — can move this caller to a different bucket or a different rate.
    //
    // A consequence worth naming: a request whose slug is wrong spends a token
    // before it is answered 404. That is not an oracle (the key never consults
    // the slug, so the answer is the same for every slug) and it is the right
    // way round — a caller looping on a slug that does not exist is exactly the
    // loop this bucket is here to bound.
    contract_limit(st, &principal).await?;

    // The workspace comes from the TOKEN; the slug in the path must agree, and
    // the workspace must still be active. Byte for byte `router::run`'s step 2,
    // including the 404-for-everything: a token for workspace A asking for
    // workspace B's contract must not be able to tell "B does not exist" from
    // "B is not yours", and an archived workspace must stop answering even
    // though a token minted before the archive is still cryptographically
    // valid until it expires.
    let ws = st
        .workspaces
        .get_by_slug(ws_slug)
        .await
        .map_err(ServingError::internal)?
        .filter(|w| w.id.0 == principal.workspace_id && w.status == "active")
        .ok_or(ServingError::NotFound)?;

    // `principal.user_id` — the service account the token was issued for, read
    // off `egret_user_id` and unforgeable without the signing key. Nothing in
    // the request can name a different account, which is the whole difference
    // between this route and the `:manage`-gated one it sits beside.
    //
    // `only: None` — the document is the account's whole reachable set. See
    // `contract_error` for why `?endpoint_id=` is not offered here.
    crate::openapi_gen::build(st, ws.id, principal.user_id, None)
        .await
        .map_err(|e| contract_error(ws.id, e))
}
